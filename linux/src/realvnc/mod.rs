// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! Where RealVNC's viewer libraries come from when they are missing or of
//! another build: the RealVNC backend's `AutoFetchLibraries` option, which
//! only this client offers (the phone's add-on has a screen of its own).
//! The backend itself ignores the key; the client reads it before a session
//! starts, and installs the libraries from the source it names ([`install`]),
//! checked against the pins as the add-on does.

use crate::backends::{Dirs, Kind};
use crate::options::Options;
use sha2::Digest;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

mod zip;

/// The option's key.
pub const OPTION: &str = "AutoFetchLibraries";

/// What `AutoFetchLibraries` says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fetch {
    /// Never: the reason a session cannot start says how to install them.
    Off,
    /// Ask, then download the pinned APK. The default.
    Prompt,
    /// Download the pinned APK without asking.
    Auto,
    /// Extract them from this APK.
    Apk(PathBuf),
    /// Download the APK from this `http://` or `https://` URL.
    Url(String),
}

impl Fetch {
    /// The value as the option holds it: `off`, `prompt` (or empty, the
    /// default), `auto`, a URL, or anything else as a path.
    pub fn parse(value: &str) -> Result<Fetch, String> {
        let scheme = |s: &str| value.len() >= s.len() && value[..s.len()].eq_ignore_ascii_case(s);
        Ok(match value {
            "" | "prompt" => Fetch::Prompt,
            "off" => Fetch::Off,
            "auto" => Fetch::Auto,
            _ if scheme("http://") || scheme("https://") => Fetch::Url(value.to_string()),
            _ if value.contains("://") => {
                return Err(format!(
                    "{OPTION}: off, prompt, auto, the path of an APK, or an http:// or https:// URL"
                ));
            }
            _ => Fetch::Apk(PathBuf::from(value)),
        })
    }

    /// The option in a backend's options.
    pub fn of(options: &Options) -> Result<Fetch, String> {
        Fetch::parse(options.get(OPTION))
    }
}

/// Whether a session on `kind` needs RealVNC's libraries installed first:
/// `None` for every other backend and where they are in place, and
/// otherwise why they are needed (the reason a session would fail with)
/// and what the option says to do.
pub fn needed(kind: &Kind, options: &Options, dirs: &Dirs) -> Option<(String, Fetch)> {
    if !std::ptr::eq(kind, &remotedesktop_realvnc::KIND) {
        return None;
    }
    let why = remotedesktop_realvnc::libraries(&dirs.realvnc_libs).err()?;
    // The option was checked when the request was made.
    Some((why, Fetch::of(options).unwrap_or(Fetch::Off)))
}

/// Where the libraries are installed to: `realvnc/` under `--extlib-dir`
/// where it was given, and otherwise under the XDG data directory, which
/// the default `--extlib-dir` resolves to once they are there.
pub fn install_dir(extlib: Option<&Path>) -> PathBuf {
    match extlib {
        Some(dir) => dir.join("realvnc"),
        None => crate::backends::data_dir().join("realvnc"),
    }
}

/// What the libraries are installed from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    Apk(PathBuf),
}

/// The source `fetch` names, for those that name one without asking.
pub fn source(fetch: &Fetch) -> Option<Source> {
    match fetch {
        Fetch::Apk(path) => Some(Source::Apk(path.clone())),
        Fetch::Off | Fetch::Prompt | Fetch::Auto | Fetch::Url(_) => None,
    }
}

/// An installation nobody watches: the probe's, which logs its steps.
pub struct Unattended;

impl Report for Unattended {
    fn step(&self, _: Step<'_>) {}
    fn progress(&self, _: u64, _: Option<u64>) {}
    fn cancelled(&self) -> bool {
        false
    }
}

/// A step of an installation, for the screen to show, worded as the
/// add-on's (`plugins/src/main/res/values/strings.xml`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step<'a> {
    /// Reading a local APK.
    Reading,
    /// Opening the APK.
    Opening,
    /// Extracting one library and checking it against its pin.
    Extracting(&'a str),
}

impl Step<'_> {
    pub fn text(&self) -> String {
        match self {
            Step::Reading => "Reading the chosen APK".into(),
            Step::Opening => "Reading the APK".into(),
            Step::Extracting(name) => format!("Extracting and verifying {name}"),
        }
    }
}

/// Where an installation reports to, from the thread it runs on.
pub trait Report: Sync {
    fn step(&self, step: Step<'_>);
    /// Bytes downloaded so far, and the total where the server gave one.
    fn progress(&self, done: u64, total: Option<u64>);
    /// Whether to stop: checked between reads and before anything is
    /// written or renamed.
    fn cancelled(&self) -> bool;
}

/// Why an installation did not finish.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Cancelled,
    /// The add-on's message for it, one line per source tried.
    Failed(String),
}

/// The add-on's messages (`plugins/src/main/res/values/strings.xml`,
/// `LibraryStore.java`).
const UNREADABLE: &str = "That file cannot be read.";
const NOT_IN_IT: &str = "That file does not contain the right library.";
const OTHER_BUILD: &str = "That is not the build this backend was written for.";
const NOT_SAVED: &str = "The library could not be saved.";

/// The largest library accepted: the largest of RealVNC's is 4.8 MB, so
/// this only stops a bad archive from filling the disk.
const MAX_LIBRARY: u64 = 64 << 20;
/// The largest APK read into memory; RealVNC's is 11.7 MB.
const MAX_APK: u64 = 64 << 20;

/// Installs RealVNC's two libraries into `dir` from `source`: each is
/// extracted from `lib/<abi>/` in the APK under a temporary name, hashed as
/// it is written, and renamed into place only once both match their pins,
/// `libvncviewer.so` last, which is the file the default `--extlib-dir`
/// looks for. Nothing is left in `dir` but the two files on success, and
/// nothing changed in it on failure.
pub fn install(source: &Source, dir: &Path, report: &dyn Report) -> Result<(), Error> {
    log::info!(
        "installing the RealVNC Viewer {} library for {} into {}",
        remotedesktop_realvnc::VERSION,
        remotedesktop_realvnc::ABI,
        dir.display()
    );
    let result = (|| {
        let apk = match source {
            Source::Apk(path) => read(path, report)?,
        };
        extract(&apk, dir, report)
    })();
    match &result {
        Ok(()) => log::info!("installed the RealVNC library in {}", dir.display()),
        Err(Error::Cancelled) => log::info!("installing the RealVNC library was cancelled"),
        Err(Error::Failed(why)) => log::warn!("the RealVNC library was not installed: {why}"),
    }
    result
}

fn read(path: &Path, report: &dyn Report) -> Result<Vec<u8>, Error> {
    report.step(Step::Reading);
    log::info!("reading {}", path.display());
    let unreadable = |e: std::io::Error| {
        log::warn!("{}: {e}", path.display());
        Error::Failed(UNREADABLE.into())
    };
    let file = File::open(path).map_err(unreadable)?;
    let mut apk = Vec::new();
    file.take(MAX_APK + 1).read_to_end(&mut apk).map_err(unreadable)?;
    if apk.len() as u64 > MAX_APK {
        log::warn!("{}: larger than {MAX_APK} bytes", path.display());
        return Err(Error::Failed(NOT_IN_IT.into()));
    }
    Ok(apk)
}

fn extract(apk: &[u8], dir: &Path, report: &dyn Report) -> Result<(), Error> {
    if report.cancelled() {
        return Err(Error::Cancelled);
    }
    report.step(Step::Opening);
    let not_saved = |e: std::io::Error| {
        log::warn!("{}: {e}", dir.display());
        Error::Failed(NOT_SAVED.into())
    };
    std::fs::create_dir_all(dir).map_err(not_saved)?;
    remove_parts(dir);
    let mut parts = Vec::new();
    let result = (|| {
        for (name, pin) in remotedesktop_realvnc::PINS {
            let path = format!("lib/{}/{name}", remotedesktop_realvnc::ABI);
            let entry = match zip::find(apk, &path) {
                Ok(Some(entry)) => entry,
                Ok(None) => {
                    log::warn!("the APK has no {path}");
                    return Err(Error::Failed(NOT_IN_IT.into()));
                }
                Err(why) => {
                    log::warn!("the APK cannot be read as a zip: {why}");
                    return Err(Error::Failed(NOT_IN_IT.into()));
                }
            };
            report.step(Step::Extracting(name));
            log::info!("extracting and verifying {name}");
            let part = dir.join(format!(".{name}.{}.part", std::process::id()));
            parts.push(part.clone());
            let mut reader = entry.reader(MAX_LIBRARY).map_err(|why| {
                log::warn!("{path} in the APK: {why}");
                Error::Failed(NOT_IN_IT.into())
            })?;
            let mut out = File::create(&part).map_err(not_saved)?;
            let mut hash = sha2::Sha256::new();
            let mut buf = vec![0; 64 * 1024];
            loop {
                if report.cancelled() {
                    return Err(Error::Cancelled);
                }
                let n = reader.read(&mut buf).map_err(|e| {
                    log::warn!("{path} in the APK: {e}");
                    Error::Failed(NOT_IN_IT.into())
                })?;
                if n == 0 {
                    break;
                }
                hash.update(&buf[..n]);
                out.write_all(&buf[..n]).map_err(not_saved)?;
            }
            out.sync_all().map_err(not_saved)?;
            let got: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
            if got != pin {
                log::warn!("{path} in the APK: SHA-256 {got}, where RealVNC Viewer {}'s is {pin}", remotedesktop_realvnc::VERSION);
                return Err(Error::Failed(OTHER_BUILD.into()));
            }
        }
        if report.cancelled() {
            return Err(Error::Cancelled);
        }
        for (part, (name, _)) in parts.iter().zip(remotedesktop_realvnc::PINS) {
            std::fs::rename(part, dir.join(name)).map_err(not_saved)?;
        }
        File::open(dir).and_then(|d| d.sync_all()).map_err(not_saved)
    })();
    for part in &parts {
        let _ = std::fs::remove_file(part);
    }
    result
}

/// Removes temporary files left in `dir` by an installation that was
/// stopped before it finished.
fn remove_parts(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') && name.ends_with(".part") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Records the steps, and cancels once `cancel_at` steps were shown.
    #[derive(Default)]
    struct Watch {
        steps: Mutex<Vec<String>>,
        cancel_at: Option<usize>,
    }

    impl Report for Watch {
        fn step(&self, step: Step<'_>) {
            self.steps.lock().unwrap().push(step.text());
        }
        fn progress(&self, _: u64, _: Option<u64>) {}
        fn cancelled(&self) -> bool {
            self.cancel_at.is_some_and(|n| self.steps.lock().unwrap().len() >= n)
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vento-realvnc-install-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn files(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
            .unwrap_or_default();
        names.sort();
        names
    }

    fn apk(at: &Path, files: &[(&str, &[u8], bool)]) -> Source {
        std::fs::write(at, zip::tests::zip(files, b"")).unwrap();
        Source::Apk(at.to_path_buf())
    }

    #[test]
    fn a_file_of_another_build_or_none_at_all_installs_nothing() {
        let root = scratch("refused");
        let into = root.join("realvnc");
        let abi = remotedesktop_realvnc::ABI;
        let cxx = format!("lib/{abi}/libc++_shared.so");
        let viewer = format!("lib/{abi}/libvncviewer.so");
        std::fs::create_dir_all(&into).unwrap();
        std::fs::write(into.join(".libvncviewer.so.1.part"), "left over").unwrap();

        let other = apk(&root.join("other.apk"), &[(&cxx, b"not it", true), (&viewer, b"nor this", false)]);
        let watch = Watch::default();
        assert_eq!(install(&other, &into, &watch), Err(Error::Failed(OTHER_BUILD.into())));
        assert_eq!(
            *watch.steps.lock().unwrap(),
            ["Reading the chosen APK", "Reading the APK", "Extracting and verifying libc++_shared.so"]
        );
        assert!(files(&into).is_empty(), "{:?}", files(&into));

        let half = apk(&root.join("half.apk"), &[("lib/armeabi-v7a/libvncviewer.so", b"", false)]);
        assert_eq!(install(&half, &into, &Watch::default()), Err(Error::Failed(NOT_IN_IT.into())));
        std::fs::write(root.join("page.apk"), "<html>").unwrap();
        let page = Source::Apk(root.join("page.apk"));
        assert_eq!(install(&page, &into, &Watch::default()), Err(Error::Failed(NOT_IN_IT.into())));
        let gone = Source::Apk(root.join("gone.apk"));
        assert_eq!(install(&gone, &into, &Watch::default()), Err(Error::Failed(UNREADABLE.into())));
        assert!(files(&into).is_empty());

        // A directory that cannot be made.
        std::fs::write(root.join("file"), "").unwrap();
        assert_eq!(install(&other, &root.join("file/realvnc"), &Watch::default()), Err(Error::Failed(NOT_SAVED.into())));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_cancelled_installation_leaves_nothing() {
        let root = scratch("cancelled");
        let into = root.join("realvnc");
        let abi = remotedesktop_realvnc::ABI;
        let source = apk(&root.join("a.apk"), &[(&format!("lib/{abi}/libc++_shared.so"), &[0; 300_000], true)]);
        let watch = Watch {
            cancel_at: Some(3),
            ..Watch::default()
        };
        assert_eq!(install(&source, &into, &watch), Err(Error::Cancelled));
        assert!(files(&into).is_empty(), "{:?}", files(&into));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// With the viewer's APK at `$VENTO_TEST_REALVNC_APK`, both libraries
    /// are installed and pass the backend's own check; skipped without it.
    #[test]
    fn the_viewers_apk_installs_both_libraries() {
        let Some(path) = std::env::var_os("VENTO_TEST_REALVNC_APK") else {
            return;
        };
        let root = scratch("apk");
        let into = root.join("realvnc");
        std::fs::create_dir_all(&into).unwrap();
        std::fs::write(into.join("libvncviewer.so"), "another build").unwrap();
        install(&Source::Apk(path.into()), &into, &Watch::default()).unwrap();
        assert_eq!(files(&into), ["libc++_shared.so", "libvncviewer.so"]);
        assert_eq!(remotedesktop_realvnc::libraries(&into), Ok(()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_values_are_read_as_the_option_says() {
        assert_eq!(Fetch::parse(""), Ok(Fetch::Prompt));
        assert_eq!(Fetch::parse("prompt"), Ok(Fetch::Prompt));
        assert_eq!(Fetch::parse("off"), Ok(Fetch::Off));
        assert_eq!(Fetch::parse("auto"), Ok(Fetch::Auto));
        assert_eq!(Fetch::parse("https://example.com/a.apk"), Ok(Fetch::Url("https://example.com/a.apk".into())));
        assert_eq!(Fetch::parse("HTTP://example.com/a.apk"), Ok(Fetch::Url("HTTP://example.com/a.apk".into())));
        assert_eq!(Fetch::parse("viewer.apk"), Ok(Fetch::Apk("viewer.apk".into())));
        assert_eq!(Fetch::parse("/x/viewer.apk"), Ok(Fetch::Apk("/x/viewer.apk".into())));
        assert_eq!(
            Fetch::parse("ftp://example.com/a.apk"),
            Err("AutoFetchLibraries: off, prompt, auto, the path of an APK, or an http:// or https:// URL".into())
        );
    }
}
