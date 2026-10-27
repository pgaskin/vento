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
//! hashes, and the core's persistent files go under `state_dir`. Both come
//! from [`Dirs`](api::kind::Dirs), with XDG defaults, and can be overridden
//! on the client's command line. The client keeps the identity the core asks
//! to have remembered, handed out through
//! [`Backend::saved_identity`](api::backend::Backend::saved_identity) and
//! passed back in as [`Connect::identity`].

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
    // Debug builds only: VENTO_REALVNC_TEST_UNHOOKED=1 has the session act as
    // if the core's socket calls could not be hooked, to check that a route
    // that is not direct then refuses to start rather than dialling directly.
    config.test_unhooked =
        cfg!(debug_assertions) && std::env::var_os("VENTO_REALVNC_TEST_UNHOOKED").is_some_and(|v| v == "1");

    log::info!("connecting to {}", c.address);
    start(
        c,
        &BEHAVIOUR,
        |callbacks| {
            // On a route that is not direct, the core's connections are made
            // through the session's network, which the session owns from this
            // call on. On a direct one the core dials the server itself.
            let net = (!c.network.direct()).then(|| api::c::net::table(c.network.clone()));
            config.net = net.as_ref().map_or(std::ptr::null(), |n| n);
            // What carries it, which tells two sessions' routes apart.
            let tunnel = net
                .as_ref()
                .map(|_| CString::new(c.network.tunnel().unwrap_or_default()).unwrap_or_default());
            config.tunnel = tunnel.as_ref().map_or(std::ptr::null(), |t| t.as_ptr());
            unsafe { library.vento_realvnc_create(&config, callbacks) }
        },
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
