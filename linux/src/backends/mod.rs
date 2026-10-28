// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The eight backends, each as a [`Kind`]: what the picker and `--help` show,
//! the options it takes, and how to start a session on it; the library each
//! runs from, for `--list`; and the directories a session is given.
//!
//! None of them is this client's own. The four Rust protocols each carry
//! their adapter in their own crate (`backend.rs` beside the protocol), the
//! four C libraries in a small crate beside their C sources, over the one
//! adapter for `vento/backend.h` in `remotedesktop-api`. The order is the
//! catalog's, which is the picker's on the phone too.

pub use api::kind::{CONNECT_TIMEOUT_MS, Connect, Dirs, Kind, Platform};
pub use api::trust::{Pinned, pinned_as};

pub use platform_linux::audio::{Audio, Output as AudioOutput};
pub use platform_linux::ffmpeg::{FFmpeg, Hardware, hardware_name, hardware_named};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// `$XDG_STATE_HOME/vento`, or `~/.local/state/vento`: the default
/// `--state-dir`, which holds the known hosts, RealVNC's identities and each
/// backend's state in a directory named for it.
pub fn default_state_dir() -> PathBuf {
    xdg("XDG_STATE_HOME", ".local/state").join("vento")
}

/// The default `--extlib-dir`, the directory of the libraries some backends
/// need and this project does not build, each backend's in a directory named
/// for it (RealVNC's two in `realvnc/`): `$XDG_DATA_HOME/vento` (or
/// `~/.local/share/vento`) where RealVNC's are there, and otherwise `lib/`
/// beside the binary, the directory of the release tree's backend libraries.
pub fn default_extlib_dir() -> PathBuf {
    let data = xdg("XDG_DATA_HOME", ".local/share").join("vento");
    if data.join(REALVNC_LIBRARY).is_file() {
        return data;
    }
    match std::env::current_exe() {
        Ok(exe) => exe.parent().map_or(data, |dir| dir.join("lib")),
        Err(_) => data,
    }
}

/// The RealVNC viewer's library, under the extra libraries' directory: the
/// one the default `--extlib-dir` looks for.
const REALVNC_LIBRARY: &str = "realvnc/libvncviewer.so";

/// An XDG base directory: `var`, or `under` in the home directory where it
/// is unset or empty.
fn xdg(var: &str, under: &str) -> PathBuf {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join(under))
}

/// The directories a session is given: `state` for the known hosts and
/// every backend's state, and RealVNC's libraries in `realvnc/` under
/// `extlib`.
pub fn dirs(state: &Path, extlib: &Path) -> Dirs {
    Dirs {
        config: state.to_path_buf(),
        state: state.to_path_buf(),
        realvnc_libs: extlib.join("realvnc"),
    }
}

/// The file a backend's code is in: for the C backends the library they
/// open, where it was found, opening it first; for the Rust ones this
/// binary. Where a library cannot be opened, the path it was looked for at
/// (the first of the rpath's directories that has it, or `lib/` beside the
/// binary) and why.
pub fn library_path(kind: &Kind) -> Result<PathBuf, (PathBuf, String)> {
    type Opened = fn() -> Result<PathBuf, (&'static str, &'static str)>;
    let c: [(&Kind, Opened); 4] = [
        (&remotedesktop_libvnc::KIND, remotedesktop_libvnc::library_path),
        (&remotedesktop_tigervnc::KIND, remotedesktop_tigervnc::library_path),
        (&remotedesktop_freerdp::KIND, remotedesktop_freerdp::library_path),
        (&remotedesktop_realvnc::KIND, remotedesktop_realvnc::library_path),
    ];
    let exe = std::env::current_exe().map_err(|e| (PathBuf::new(), format!("this binary's path: {e}")))?;
    let Some((_, path)) = c.iter().find(|(k, _)| std::ptr::eq(*k, kind)) else {
        return Ok(exe);
    };
    path().map_err(|(name, why)| {
        // The rpath (`build.rs`): `lib/` beside the binary, then `../lib`.
        let dir = exe.parent().unwrap_or(Path::new("/"));
        let tried = [Some(dir), dir.parent()]
            .into_iter()
            .flatten()
            .map(|d| d.join("lib").join(name))
            .find(|p| p.is_file())
            .unwrap_or_else(|| dir.join("lib").join(name));
        (tried, why.to_string())
    })
}

/// What the far end lists this machine as, where a phone says its model.
pub fn client_name() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "vento".to_string())
}

/// The video decoders here: libavcodec, where the phone hands over
/// MediaCodec, decoding as `hardware` says, which is off unless the user
/// turned it on.
///
/// One per session, and held by it as well as handed to its backend in
/// [`Connect::decoders`]: switching the way of decoding under a running
/// session is [`FFmpeg::set_hardware`] on this, which every backend decoding
/// through it takes at its next picture: RustDesk's video, and TigerVNC's
/// and FreeRDP's H.264, which ask `api::c::video`'s table for the mode.
///
/// [`FFmpeg::set_hardware`]: platform_linux::ffmpeg::FFmpeg::set_hardware
pub fn decoders(hardware: Hardware) -> Arc<FFmpeg> {
    Arc::new(FFmpeg::new(hardware))
}

/// Where the far end's sound goes here: a PipeWire stream of the session's
/// own, or ALSA's default device where there is no PipeWire (`output` picks,
/// `--audio`), where the phone hands over AAudio. One per session, as
/// [`Connect::audio`], so that its controls (the gain and mute, which a
/// window sets while the session plays) are that session's alone, and its
/// streams are listed under its name (`title`) in a mixer.
pub fn audio(title: &str, output: AudioOutput) -> Arc<Audio> {
    Arc::new(Audio::new(title, output))
}

/// Every backend this binary has, in the picker's order. The C backends are
/// always among them, since their libraries are opened only when first
/// asked for: where one cannot be loaded, its version says it is not
/// available and why, and a session on it fails to start with the same
/// sentence. Where `build.rs` had no clang to build RealVNC's library with,
/// the reason is that, rather than a file the loader did not find.
pub fn all() -> Vec<&'static Kind> {
    if env!("VENTO_REALVNC_BUILT") == "0" {
        remotedesktop_realvnc::not_built();
    }
    vec![
        &remotedesktop_rfb::backend::KIND,
        &remotedesktop_libvnc::KIND,
        &remotedesktop_tigervnc::KIND,
        &remotedesktop_freerdp::KIND,
        &remotedesktop_ironrdp::backend::KIND,
        &remotedesktop_realvnc::KIND,
        &remotedesktop_spice::backend::KIND,
        &remotedesktop_rustdesk::backend::KIND,
    ]
}

pub fn find(id: &str) -> Option<&'static Kind> {
    all().into_iter().find(|k| k.id == id)
}

/// The domain to connect to `kind` with: `domain` for a backend whose user
/// name carries one ([`About::user_has_domain`](api::catalog::About::user_has_domain),
/// the two RDP backends), and empty for the others, which take none. A
/// request keeps the domain it was given, so that the dialog has it again
/// after a session on another backend.
pub fn domain<'a>(kind: &Kind, domain: &'a str) -> &'a str {
    if kind.user_has_domain { domain } else { "" }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The catalog's order, which `all` has to keep by hand: the kinds are
    /// in eight crates and the list is written out here. Every one of them,
    /// RealVNC's too, whatever the build found.
    #[test]
    fn the_kinds_are_in_the_catalogs_order() {
        assert_eq!(all().len(), api::catalog::ALL.len());
        let orders: Vec<i32> = all().iter().map(|k| k.order).collect();
        assert!(orders.windows(2).all(|w| w[0] < w[1]), "{orders:?}");
        for kind in all() {
            assert!(api::catalog::ALL.iter().any(|a| std::ptr::eq(*a, kind.about)));
        }
    }

    #[test]
    fn only_the_rdp_backends_are_sent_a_domain() {
        let sent: Vec<&str> = all().into_iter().filter(|k| !domain(k, "CORP").is_empty()).map(|k| k.id).collect();
        assert_eq!(sent, ["freerdp", "rdp"]);
        assert_eq!(domain(find("rdp").unwrap(), "CORP"), "CORP");
    }
}
