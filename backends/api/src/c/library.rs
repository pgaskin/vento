// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! A C backend's library on the Linux client: opened by name on first use
//! rather than linked, so that a library that cannot be loaded (a system
//! library it needs is missing or too old, the file is from another build,
//! or there is no file) makes only that backend unavailable and the client
//! starts with the rest.
//!
//! A backend's crate declares one [`Library`] over the table its build script
//! generated from its header (bindgen's `dynamic_library_name`), calls the
//! header's functions through [`call!`](crate::call), and wraps its `Kind`'s
//! `create` in [`create_if_loaded!`](crate::create_if_loaded), which reports
//! the backend as unavailable when the library cannot be opened. On the phone
//! the C is linked into the same library as the crate, which exports only
//! `JNI_OnLoad`, so there the library is a `Linked`, the calls are the extern
//! declarations and `create` is the crate's own function. The vtable is
//! reached through the session's own pointer, never through a symbol, so
//! [`start`](super::start) and [`CSession`](super::CSession) are the same on
//! both.

#[cfg(target_os = "linux")]
use std::sync::OnceLock;

/// How the Linux client opens a backend's library.
#[cfg(target_os = "linux")]
#[derive(Clone, Copy)]
pub enum Scope {
    /// Keeps its symbols private (`RTLD_LOCAL`): libraries opened later do
    /// not bind to them. For the open backends' libraries, which export only
    /// their header's functions and `vento_log`.
    Local,
    /// Adds its symbols to the process's global scope (`RTLD_GLOBAL`), as for
    /// a library the binary is linked against. For RealVNC's hosting layer,
    /// whose loader looks symbols up there.
    Global,
}

/// A backend's library and its functions, `T` being the table bindgen
/// generated from its header: opened once, the first time [`get`](Self::get)
/// is called, and never closed (the libraries register exit handlers, and a
/// session's threads run in them). A failure is cached too, and every later
/// call gets the same message.
#[cfg(target_os = "linux")]
pub struct Library<T: 'static> {
    name: &'static str,
    header: &'static str,
    scope: Scope,
    bind: unsafe fn(libloading::Library) -> Result<T, libloading::Error>,
    loaded: OnceLock<Result<T, String>>,
}

#[cfg(target_os = "linux")]
impl<T> Library<T> {
    /// `name` is the file name the client's Meson build writes, found through
    /// the binary's rpath (or `LD_LIBRARY_PATH`); `header` is the header the
    /// table was generated from, for the error when the file is a different
    /// library; `bind` is the generated table's `from_library`.
    pub const fn new(
        name: &'static str,
        header: &'static str,
        scope: Scope,
        bind: unsafe fn(libloading::Library) -> Result<T, libloading::Error>,
    ) -> Self {
        Library { name, header, scope, bind, loaded: OnceLock::new() }
    }

    /// The table, or why the library could not be opened.
    pub fn get(&self) -> Result<&T, &str> {
        self.loaded.get_or_init(|| self.open()).as_ref().map_err(String::as_str)
    }

    /// The table of a library [`get`](Self::get) has already opened, for use
    /// within a session, which is started only after that.
    pub fn loaded(&self) -> &T {
        self.get().expect("a session is started only over its opened library")
    }

    /// Marks the library as not built, for the reason `why`, unless it has
    /// been opened already: every later call gets `why` rather than the
    /// loader's "cannot open shared object file", which does not say how to
    /// get it. For a client whose build knows it made no such file.
    pub fn not_built(&self, why: String) {
        let _ = self.loaded.set(Err(why));
    }

    /// The file the library was opened from, as the process has it mapped,
    /// opening it first; or, where it cannot be opened, its file name and
    /// why, as [`get`](Self::get) says.
    pub fn path(&self) -> Result<std::path::PathBuf, (&'static str, &str)> {
        self.get().map_err(|why| (self.name, why))?;
        mapped(self.name).ok_or((self.name, "opened, but not among the process's mappings"))
    }

    /// A backend's `version`: the library's version as `read` gets it, or
    /// "not available: <why>".
    pub fn version(&self, read: impl FnOnce(&T) -> String) -> String {
        match self.get() {
            Ok(table) => read(table),
            Err(why) => format!("not available: {why}"),
        }
    }

    fn open(&self) -> Result<T, String> {
        use libloading::os::unix::{Library, RTLD_GLOBAL, RTLD_LOCAL, RTLD_NOW};
        let scope = match self.scope {
            Scope::Local => RTLD_LOCAL,
            Scope::Global => RTLD_GLOBAL,
        };
        // Bind every symbol now: a missing symbol or symbol version (a glibc
        // older than the build machine's) fails here, where it can be
        // reported, rather than mid-session. Opening it runs its constructors
        // (the vendored libraries' and the C++ runtime's), and the lookups
        // read the header's functions; this relies on the library coming from
        // the client's own build.
        let library = unsafe { Library::open(Some(self.name), RTLD_NOW | scope) }.map_err(|e| {
            // glibc names the file it could not load, which for a missing
            // dependency is the dependency, so prefix the library's name to
            // show which backend it belongs to.
            let e = e.to_string();
            if e.starts_with(self.name) { e } else { format!("{}: {e}", self.name) }
        })?;
        unsafe { (self.bind)(library.into()) }
            .map_err(|e| format!("{} is not the library {} describes: {e}", self.name, self.header))
    }
}

/// The path of the first mapping in `/proc/self/maps` whose file is named
/// `name`: where the loader found a library opened by that name.
#[cfg(target_os = "linux")]
fn mapped(name: &str) -> Option<std::path::PathBuf> {
    let maps = std::fs::read_to_string("/proc/self/maps").ok()?;
    maps.lines()
        .filter_map(|line| line.find('/').map(|at| &line[at..]))
        .find(|path| path.rsplit('/').next() == Some(name))
        .map(std::path::PathBuf::from)
}

/// Calls one of a C backend's own functions (declared in its header, outside
/// the vtable): on the Linux client as a member of `$library`, the table a
/// `Library` opened; on the phone, where the C is linked into the same
/// library as the crate, as the extern declaration `$ffi::$f`, with
/// `$library` a `Linked`. Used inside an `unsafe` block, like any call into
/// the C.
#[macro_export]
macro_rules! call {
    ($library:expr, $ffi:ident :: $f:ident ( $($arg:expr),* $(,)? )) => {{
        #[cfg(target_os = "linux")]
        let result = ($library).$f($($arg),*);
        #[cfg(not(target_os = "linux"))]
        let result = {
            let _: &$crate::c::library::Linked = $library;
            $ffi::$f($($arg),*)
        };
        result
    }};
}

/// A `Kind`'s `create` over a backend's library: on the Linux client a
/// function that opens `$library` first and, if it cannot be opened, fails
/// with "<$name> is not available: <why>"; on the phone `$create` itself.
#[macro_export]
macro_rules! create_if_loaded {
    ($library:expr, $name:literal, $create:path) => {{
        #[cfg(target_os = "linux")]
        let create = |c: &$crate::kind::Connect| match ($library).get() {
            Ok(_) => $create(c),
            Err(why) => Err(format!(concat!($name, " is not available: {}"), why)),
        };
        #[cfg(not(target_os = "linux"))]
        let create = $create;
        create
    }};
}

/// Stands in for [`Library`] where the C is linked into the same library as
/// its crate, so it is always loaded. Inlined, so that the phone's code
/// compiles to direct calls into the C.
#[cfg(not(target_os = "linux"))]
pub struct Linked;

#[cfg(not(target_os = "linux"))]
impl Linked {
    #[inline]
    pub fn loaded(&self) -> &Linked {
        self
    }

    #[inline]
    pub fn version(&self, read: impl FnOnce(&Linked) -> String) -> String {
        read(self)
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    unsafe fn nothing(_: libloading::Library) -> Result<(), libloading::Error> {
        Ok(())
    }

    unsafe fn no_such_symbol(library: libloading::Library) -> Result<(), libloading::Error> {
        unsafe { library.get::<unsafe extern "C" fn()>(b"vento_nothing_version\0") }.map(|_| ())
    }

    /// A missing library makes its backend unavailable, with a message that
    /// names the file, and asking again gives the same message.
    #[test]
    fn a_missing_library_is_not_available_with_its_name() {
        static MISSING: Library<()> =
            Library::new("libvento_nothing.so", "vento/nothing.h", Scope::Local, nothing);
        let version = MISSING.version(|_| unreachable!());
        assert!(version.starts_with("not available: libvento_nothing.so: "), "{version}");
        assert_eq!(MISSING.get().unwrap_err(), version.trim_start_matches("not available: "));
    }

    /// A library marked as not built reports why, and is never opened.
    #[test]
    fn a_library_not_built_says_why() {
        static ABSENT: Library<()> = Library::new("libc.so.6", "vento/nothing.h", Scope::Local, nothing);
        ABSENT.not_built("built only with clang".to_string());
        assert_eq!(ABSENT.version(|_| unreachable!()), "not available: built only with clang");
        // Once opened, `not_built` has no effect.
        static OPENED: Library<()> = Library::new("libc.so.6", "vento/nothing.h", Scope::Local, nothing);
        assert!(OPENED.get().is_ok());
        OPENED.not_built("built only with clang".to_string());
        assert!(OPENED.get().is_ok());
    }

    /// An opened library's path is the file the loader mapped; a missing
    /// one's error names the file it looked for.
    #[test]
    fn the_path_is_the_mapped_file_or_the_name_and_why() {
        static LIBC: Library<()> = Library::new("libc.so.6", "vento/nothing.h", Scope::Local, nothing);
        let path = LIBC.path().unwrap();
        assert!(path.is_absolute() && path.ends_with("libc.so.6"), "{path:?}");
        static MISSING: Library<()> =
            Library::new("libvento_nowhere.so", "vento/nothing.h", Scope::Local, nothing);
        let (name, why) = MISSING.path().unwrap_err();
        assert_eq!(name, "libvento_nowhere.so");
        assert!(why.starts_with("libvento_nowhere.so: "), "{why}");
    }

    /// A library that opens but lacks one of the header's functions is
    /// reported as not the library the header describes, naming the
    /// function.
    #[test]
    fn another_library_is_not_the_one_its_header_describes() {
        static OTHER: Library<()> = Library::new("libc.so.6", "vento/nothing.h", Scope::Local, no_such_symbol);
        let why = OTHER.get().unwrap_err();
        assert!(why.starts_with("libc.so.6 is not the library vento/nothing.h describes: "), "{why}");
        assert!(why.contains("vento_nothing_version"), "{why}");
    }
}
