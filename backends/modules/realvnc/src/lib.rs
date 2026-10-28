// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! RealVNC's viewer core as a [`Backend`](api::backend::Backend) for the
//! Linux client: `vento_realvnc_config` and `RealVncBackend`'s option
//! handling and rows. The phone's RealVNC backend is the module's Java over
//! the viewer's own JNI, so on any target other than Linux this crate is
//! empty.
//!
//! The library, `libvento_realvnc.so`, is opened on first use rather than
//! linked: the client's build makes it only where clang is on the path, and
//! decides that after this crate is built. The backend is therefore always
//! listed, and reports why it cannot start where the library cannot be
//! opened ([`library`]), or that it was not built where the client says so
//! ([`not_built`]). It is found by name through the shared loader of the C
//! backends' libraries ([`api::c::library`]): through the binary's rpath
//! (`$ORIGIN/lib`, the `lib/` directory beside the binary where the client's
//! build puts every `libvento_*.so`) or `LD_LIBRARY_PATH`. Once open it stays
//! open, since the core it hosts never releases a session.
//!
//! Nothing of RealVNC's is in the tree: the session loads the two libraries,
//! extracted from the viewer's APK, from `library_dir` and verifies their
//! hashes, and the core's persistent files go under `state_dir`. The same
//! hashes are checked here first ([`libraries`]), so that a session on files
//! that are missing or from another build fails with that reason, and the
//! client's `--help` and `--list` can say so too. Both come
//! from [`Dirs`](api::kind::Dirs), with XDG defaults, and can be overridden
//! on the client's command line. The client keeps the identity the core asks
//! to have remembered, handed out through
//! [`Backend::saved_identity`](api::backend::Backend::saved_identity) and
//! passed back in as [`Connect::identity`].

#![cfg(target_os = "linux")]

use api::backend::{Backend, ConnectionFact, Field, Traffic};
use api::c::library::{Library, Scope};
use api::c::{Behaviour, CSession, Security, start};
use api::kind::{Connect, Dirs, Kind};
use api::options::Options;
use std::ffi::CString;
use std::os::raw::c_void;

/// The bindings `build.rs` generated for `vento/realvnc.h`, over the shared
/// declarations it uses: its types, and its functions as the fields of a
/// [`Library`](ffi::Library), each looked up when the library is opened.
mod ffi {
    #![allow(non_camel_case_types, dead_code)]
    pub use api::c::ffi::*;
    include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
}

/// The library, opened on first use. Opened with `RTLD_GLOBAL`, like a
/// library linked into the binary, so that the hosting layer's lookups in the
/// process's global scope find the same symbols as when it was linked in.
static LIBRARY: Library<ffi::Library> =
    Library::new("libvento_realvnc.so", "vento/realvnc.h", Scope::Global, ffi::Library::from_library);

/// The library, or why it could not be opened.
fn library() -> Result<&'static ffi::Library, &'static str> {
    LIBRARY.get()
}

/// The file `libvento_realvnc.so` was opened from, opening it first, or its
/// name and why it cannot be opened: for the client's list of its backends.
pub fn library_path() -> Result<std::path::PathBuf, (&'static str, &'static str)> {
    LIBRARY.path()
}

/// Called before first use when the client's build did not make the
/// library: the backend is then unavailable for that reason, rather than
/// because the loader could not find the file.
pub fn not_built() {
    LIBRARY.not_built("libvento_realvnc.so not available (was clang installed when building?)".to_string());
}

/// The viewer's build the session's declarations were read from, and the
/// SHA-256 of each of its two libraries for this machine's ABI, from
/// `Library.java` (`build.rs`).
pub const VERSION: &str = env!("VENTO_REALVNC_VERSION");
/// The directory under `lib/` in the APK, empty for an architecture with no
/// build.
pub const ABI: &str = env!("VENTO_REALVNC_ABI");
/// Each library's name and SHA-256, in the order they are loaded.
pub const PINS: [(&str, &str); 2] = [
    ("libc++_shared.so", env!("VENTO_REALVNC_SHA256_LIBCXX_SHARED")),
    ("libvncviewer.so", env!("VENTO_REALVNC_SHA256_LIBVNCVIEWER")),
];

/// Whether RealVNC's two libraries are in `dir` and are the pinned build,
/// as the session checks before it loads them; otherwise why not, with what
/// to do about it. Missing files are reported first, then one that cannot
/// be read, then the first that is another build.
pub fn libraries(dir: &std::path::Path) -> Result<(), String> {
    if ABI.is_empty() {
        return Err(format!("RealVNC Viewer {VERSION} has no build for this machine's architecture"));
    }
    let copy = |into: &str| {
        format!(
            "Copy libc++_shared.so and libvncviewer.so from lib/{ABI}/ in the RealVNC Viewer {VERSION} APK into \
             {into}, or set AutoFetchLibraries to the APK's path"
        )
    };
    let missing: Vec<&str> = PINS
        .iter()
        .map(|(name, _)| *name)
        .filter(|name| !dir.join(name).is_file())
        .collect();
    if !missing.is_empty() {
        let names = match missing.as_slice() {
            [one] => format!("{one} is"),
            _ => format!("{} are", missing.join(" and ")),
        };
        return Err(format!("{names} not in {}. {}", dir.display(), copy("it")));
    }
    for (name, pin) in PINS {
        let path = dir.join(name);
        let hash = sha256(&path).map_err(|e| format!("{} cannot be read: {e}", path.display()))?;
        if hash != pin {
            log::debug!("{}: SHA-256 {hash}, where RealVNC Viewer {VERSION}'s is {pin}", path.display());
            return Err(format!(
                "{} is not the one from RealVNC Viewer {VERSION}. {}",
                path.display(),
                copy(&dir.display().to_string())
            ));
        }
    }
    Ok(())
}

/// The file's SHA-256, as `sha256sum` prints it.
fn sha256(path: &std::path::Path) -> std::io::Result<String> {
    use sha2::Digest;
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = sha2::Sha256::new();
    let mut buf = vec![0; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// The library of a running session. A [`CSession`] of this backend exists
/// only after [`create`] has opened the library, and it is never closed.
fn loaded() -> &'static ffi::Library {
    LIBRARY.loaded()
}

pub static KIND: Kind = Kind {
    about: &api::catalog::REALVNC,
    version: || LIBRARY.version(|library| unsafe { api::c::string(library.vento_realvnc_version()) }),
    create,
    forget: Some(forget),
};

static BEHAVIOUR: Behaviour = Behaviour {
    info,
    // Received only: the core counts one direction and has no count of the
    // other, and reporting zero would claim nothing was sent.
    sent_counted: false,
    set_option,
    rdp_domain: false,
};

fn create(c: &Connect) -> Result<std::sync::Arc<dyn Backend>, String> {
    let library = library().map_err(|why| format!("RealVNC is not available: {why}"))?;
    libraries(&c.dirs.realvnc_libs).map_err(|why| format!("RealVNC is not available: {why}"))?;
    let o = c.options;
    // The provider's defaults, which are not all zero, then the rows over
    // them, as `RealVncBackend.connect` passes the whole options map.
    let mut config = ffi::vento_realvnc_config::default();
    unsafe { library.vento_realvnc_config_init(&mut config) };

    let state = state_dir(c.dirs);
    std::fs::create_dir_all(&state).map_err(|e| format!("the state directory: {e}"))?;

    let address = CString::new(c.address).map_err(|_| "address has a NUL")?;
    let user = (!c.user.is_empty()).then(|| CString::new(c.user).unwrap_or_default());
    let password = c.password.and_then(|p| CString::new(p).ok());
    let quality = CString::new(o.get("Quality")).unwrap_or_default();
    let encoding = CString::new(o.get("PreferredEncoding")).unwrap_or_default();
    // NULL follows the quality, as on the phone, unless a depth was chosen.
    let colour = o
        .is_set("ColorLevel")
        .then(|| CString::new(o.get("ColorLevel")).unwrap_or_default());
    let encryption = CString::new(o.get("Encryption")).unwrap_or_default();
    let library_dir = CString::new(c.dirs.realvnc_libs.to_string_lossy().into_owned())
        .map_err(|_| "the library path has a NUL")?;
    let state_dir =
        CString::new(state.to_string_lossy().into_owned()).map_err(|_| "the state path has a NUL")?;
    // The identity this server was last accepted with, passed back as
    // `RealVncBackend` does: with it the core's security prompt is silent for
    // a server it recognises. The client stores it, and stores a new one too.
    let identity = c.identity.and_then(|i| CString::new(i).ok());

    config.address = address.as_ptr();
    config.user_name = user.as_ref().map_or(std::ptr::null(), |u| u.as_ptr());
    config.password = password.as_ref().map_or(std::ptr::null(), |p| p.as_ptr());
    config.quality = quality.as_ptr();
    config.preferred_encoding = encoding.as_ptr();
    config.color_level = colour.as_ref().map_or(std::ptr::null(), |c| c.as_ptr());
    config.view_only = o.bool("ViewOnly");
    config.shared = o.bool("Shared");
    config.auto_reconnect = o.bool("AutoReconnect");
    config.enable_udp = o.bool("EnableUdpRfb");
    config.encryption = encryption.as_ptr();
    config.warn_unencrypted = o.bool("WarnUnencrypted");
    config.dot_when_no_cursor = o.bool("DotWhenNoCursor");
    config.accept_bell = o.bool("AcceptBell");
    config.keep_alive_interval_s = o.int("KeepAliveInterval", 30) as i32;
    config.keep_alive_response_timeout_s = o.int("KeepAliveResponseTimeout", 30) as i32;
    config.library_dir = library_dir.as_ptr();
    config.state_dir = state_dir.as_ptr();
    config.identity = identity.as_ref().map_or(std::ptr::null(), |i| i.as_ptr());
    // `connect_timeout_ms` keeps config_init's 0, which means the backend's
    // own twenty seconds: a failing RealVNC connection reports nothing at all.

    log::info!("connecting to {}", c.address);
    start(
        c,
        &BEHAVIOUR,
        |callbacks| unsafe { library.vento_realvnc_create(&config, callbacks) },
        Some(&SECURITY),
    )
}

/// The core's security prompt has a remember option, and an identity to keep
/// when it was answered with it.
static SECURITY: Security = Security {
    answer: |s, accept, remember| unsafe {
        loaded().vento_realvnc_answer_security(s.session, accept, remember)
    },
    saved_identity,
};

fn saved_identity(s: &CSession) -> Option<String> {
    let raw = unsafe { loaded().vento_realvnc_identity_saved(s.session) };
    if raw.is_null() {
        return None;
    }
    let identity = unsafe { api::c::string(raw) };
    unsafe { ffi::free(raw as *mut c_void) };
    (!identity.is_empty()).then_some(identity)
}

/// Every key goes through `vento_realvnc_set_option`, which carries the
/// quality group together and re-reads the encoder. View-only is already sent
/// through the vtable by [`CBackend`](api::c::CBackend), so it is not sent
/// again here.
fn set_option(session: &CSession, o: &Options, key: &str) {
    if key == "ViewOnly" {
        return;
    }
    let (k, v) = (CString::new(key), CString::new(o.get(key)));
    if let (Ok(k), Ok(v)) = (k, v) {
        unsafe { loaded().vento_realvnc_set_option(session.session, k.as_ptr(), v.as_ptr()) }
    }
}

fn info(s: &[String], traffic: Option<Traffic>) -> Vec<ConnectionFact> {
    if s.len() < 8 {
        return Vec::new();
    }
    let mut facts = vec![
        ConnectionFact::of(Field::DesktopName, "Desktop", &s[0]),
        ConnectionFact::of(Field::Protocol, "Protocol", &s[1]),
        ConnectionFact::of(Field::Connection, "Connection", &s[2]),
        ConnectionFact::of(Field::Security, "Security", &s[3]),
        ConnectionFact::of(Field::Encoding, "Encoding", &s[4]),
        ConnectionFact::of(Field::LineSpeed, "Line speed", &s[5]),
    ];
    if let Some(traffic) = traffic {
        facts.push(ConnectionFact::data(traffic));
    }
    facts.push(ConnectionFact::diagnostic(Field::ServerPixels, "Server pixels", &s[6]));
    facts.push(ConnectionFact::diagnostic(Field::ViewerPixels, "Viewer pixels", &s[7]));
    facts
}

/// The core's state directory, where it keeps its persistent files.
fn state_dir(dirs: &Dirs) -> std::path::PathBuf {
    dirs.state.join("realvnc")
}

/// The core's own record of the servers it accepted, written by the core
/// under its state directory, in a directory named after the application name
/// the session gives it (`appName` in `realvnc_session.cxx`). Each line is one
/// thing it remembers about a server: `<address>=<identity>` for a server with
/// a key and `<address>/extra=<flags>` for what was accepted of it (an
/// unencrypted connection, for example), whether or not *Remember server* was
/// ticked. The core asks nothing about a server listed here, whatever identity
/// it is given.
fn core_identities(dirs: &Dirs) -> std::path::PathBuf {
    state_dir(dirs).join("net.pgaskin.remotedesktop/.vnc/identities")
}

/// [`Kind::forget`]: removes the core's lines for `address` from its record
/// and keeps the rest unchanged. The phone's `forgetIdentities` deletes the
/// whole file instead, since it forgets every server at once. The library is
/// not loaded; a session to the same server running meanwhile may write its
/// line back when it ends.
fn forget(dirs: &Dirs, address: &str) -> Result<Vec<String>, String> {
    let path = core_identities(dirs);
    let text = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let (kept, removed) = forget_lines(&text, address);
    if removed.is_empty() {
        return Ok(Vec::new());
    }
    // Replaced atomically, as the client's own stores are, keeping the mode
    // the core gave the file.
    let temp = path.with_file_name(format!(".identities.{}.tmp", std::process::id()));
    let written = (|| {
        let mode = std::fs::metadata(&path)?.permissions();
        std::fs::write(&temp, &kept)?;
        std::fs::set_permissions(&temp, mode)?;
        std::fs::rename(&temp, &path)
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&temp);
        return Err(format!("{}: {e}", path.display()));
    }
    Ok(removed
        .into_iter()
        .map(|key| format!("{}: {key}", path.display()))
        .collect())
}

/// Returns the core's record without the lines for the server `address`
/// (matched on the key before any `/`), and the keys of the removed lines.
/// Every other line is kept byte for byte, including lines the core may read
/// but this cannot parse.
fn forget_lines(text: &[u8], address: &str) -> (Vec<u8>, Vec<String>) {
    let mut kept = Vec::with_capacity(text.len());
    let mut removed = Vec::new();
    for line in text.split_inclusive(|&b| b == b'\n') {
        let key = std::str::from_utf8(line)
            .ok()
            .and_then(|l| l.split_once('='))
            .map(|(key, _)| key.trim());
        match key {
            Some(key) if api::trust::same_server(key.split('/').next().unwrap_or(key), address) => {
                removed.push(key.to_string());
            }
            _ => kept.extend_from_slice(line),
        }
    }
    (kept, removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_server_is_forgotten_from_the_cores_record_and_nothing_else() {
        let record = b"127.0.0.1::5901/extra=0201\n10.0.0.2::5977=00000800dec7\n\
                       10.0.0.2::5977/extra=0200\nnot a line\n127.0.0.1::59010=00ff";
        // `:1` is display 1, port 5901: the same server as written by the
        // core with `::`.
        let (kept, removed) = forget_lines(record, "127.0.0.1:1");
        assert_eq!(removed, ["127.0.0.1::5901/extra"]);
        assert_eq!(
            kept,
            b"10.0.0.2::5977=00000800dec7\n10.0.0.2::5977/extra=0200\nnot a line\n127.0.0.1::59010=00ff"
        );
        let (kept, removed) = forget_lines(&kept, "10.0.0.2::5977");
        assert_eq!(removed, ["10.0.0.2::5977", "10.0.0.2::5977/extra"]);
        assert_eq!(kept, b"not a line\n127.0.0.1::59010=00ff");
        assert_eq!(forget_lines(b"", "x").1, Vec::<String>::new());
    }

    #[test]
    fn forgetting_rewrites_the_cores_file_and_a_missing_one_is_nothing() {
        let root = std::env::temp_dir().join(format!("vento-realvnc-forget-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dirs = Dirs {
            config: root.join("config"),
            state: root.join("state"),
            realvnc_libs: root.join("libs"),
        };
        assert_eq!(forget(&dirs, "127.0.0.1::5901"), Ok(Vec::new()));
        let path = core_identities(&dirs);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "127.0.0.1::5901/extra=0201\nother::5900/extra=0201\n").unwrap();
        let removed = forget(&dirs, "127.0.0.1:5901").unwrap();
        assert_eq!(removed.len(), 1);
        assert!(removed[0].ends_with(": 127.0.0.1::5901/extra"), "{removed:?}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "other::5900/extra=0201\n");
        assert_eq!(forget(&dirs, "127.0.0.1:5901"), Ok(Vec::new()));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `build.rs` read a version and a full hash for each library of this
    /// machine's ABI out of `Library.java`.
    #[test]
    fn the_pins_come_from_library_java() {
        assert!(!VERSION.is_empty());
        assert!(matches!(ABI, "x86_64" | "arm64-v8a"), "{ABI}");
        let java = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../realvnc/src/main/java/com/realvnc/vncviewer/jni/Library.java"
        ))
        .unwrap();
        assert!(java.contains(&format!("VERSION = \"{VERSION}\"")));
        for (name, pin) in PINS {
            assert_eq!(pin.len(), 64, "{name}");
            assert!(java.contains(&format!("\"{ABI}/{name}\",\n            \"{pin}\"")), "{name}");
        }
    }

    /// Missing files are named with the directory and what to copy there;
    /// a file of another build is named with the version it has to be.
    #[test]
    fn missing_or_other_libraries_say_which_and_what_to_do() {
        let dir = std::env::temp_dir().join(format!("vento-realvnc-libraries-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let copy =
            format!("Copy libc++_shared.so and libvncviewer.so from lib/{ABI}/ in the RealVNC Viewer {VERSION} APK");
        let hint = "or set AutoFetchLibraries to the APK's path";
        assert_eq!(
            libraries(&dir),
            Err(format!("libc++_shared.so and libvncviewer.so are not in {}. {copy} into it, {hint}", dir.display()))
        );
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("libc++_shared.so"), "not it").unwrap();
        assert_eq!(libraries(&dir), Err(format!("libvncviewer.so is not in {}. {copy} into it, {hint}", dir.display())));
        std::fs::write(dir.join("libvncviewer.so"), "not it either").unwrap();
        assert_eq!(
            libraries(&dir),
            Err(format!(
                "{}/libc++_shared.so is not the one from RealVNC Viewer {VERSION}. {copy} into {}, {hint}",
                dir.display(),
                dir.display()
            ))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sha256_is_sha256sums() {
        let path = std::env::temp_dir().join(format!("vento-realvnc-sha256-{}", std::process::id()));
        std::fs::write(&path, "abc").unwrap();
        assert_eq!(
            sha256(&path).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// Where the client's build made no library, the backend reports that
    /// and the reason, and starting a session gives the same message.
    #[test]
    fn a_library_not_built_says_clang() {
        not_built();
        let version = (KIND.version)();
        assert!(version.starts_with("not available: libvento_realvnc.so "), "{version}");
        assert!(version.contains("clang"), "{version}");
        assert_eq!(library().err(), version.strip_prefix("not available: "));
    }
}
