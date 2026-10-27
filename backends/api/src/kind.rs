// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! A backend as a client holds it ([`Kind`]: its [`About`] from the
//! [`catalog`](crate::catalog), and how to start a session on it), and
//! [`Connect`], the parameters a session is started with.

use crate::backend::Backend;
use crate::catalog::About;
use crate::events::Mailbox;
use crate::options::Options;
use runtime::media::audio::Outputs;
use runtime::media::video::Decoders;
use runtime::net::Network;
use std::path::PathBuf;
use std::sync::Arc;

/// Where a client keeps things on disk.
#[derive(Clone, Debug)]
pub struct Dirs {
    /// The client's persistent decisions: the known hosts and RealVNC's
    /// identities.
    pub config: PathBuf,
    /// Other session state: FreeRDP's config directory (which stays empty)
    /// and RealVNC's key, own identities and logs, each under a directory
    /// named for its backend.
    pub state: PathBuf,
    /// The directory holding `libvncviewer.so` and `libc++_shared.so` from
    /// the RealVNC viewer's APK, for the only backend whose library is not
    /// part of this project's build.
    pub realvnc_libs: PathBuf,
}

/// Which client a session is in, for the protocols that send it to the far
/// end: RustDesk's login names the platform, and RDP's general capability
/// set carries a major type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Android,
    Linux,
}

/// The parameters a session is started with.
pub struct Connect<'a> {
    pub address: &'a str,
    /// Empty for none. For the RDP backends, `DOMAIN\user` and `user@domain`
    /// are split by [`Connect::domain_and_user`] (a backslash first, and an
    /// address at a common email domain is `MicrosoftAccount\address`), and
    /// a domain given this way overrides `domain`.
    pub user: &'a str,
    pub domain: &'a str,
    /// `None` prompts. Callers pass `None` rather than an empty password,
    /// since an empty string looks like a stored secret and fails the
    /// connection instead of prompting for the real one (first seen with
    /// RealVNC).
    pub password: Option<&'a str>,
    pub options: &'a Options,
    pub mailbox: Arc<Mailbox>,
    pub dirs: &'a Dirs,
    /// The size of the screen this session will be drawn on, if known, for
    /// the RDP backends' `DesktopSize=device`.
    pub screen: Option<(u32, u32)>,
    /// The desktop size this address ended up at last time, if the client
    /// records it (the phone does, per address, for both RDP backends), for
    /// the RDP backends' `DesktopSize=remote`.
    pub last_size: Option<(u32, u32)>,
    /// The identity this address's server was last accepted with, if the
    /// client records it: the value a backend returned from
    /// [`Backend::saved_identity`] after a session at this address, passed
    /// back so that it does not ask again about a server it recognises.
    /// Only for a backend with [`About::saves_identity`] set (RealVNC's core
    /// is the only library with its own identity store); `None` for the
    /// others, whose far ends the client pins.
    pub identity: Option<&'a str>,
    /// The name the far end shows for this machine: a phone's model, a
    /// desktop's host name.
    pub client_name: &'a str,
    pub platform: Platform,
    /// The video decoders, for the protocol whose picture is always video
    /// (RustDesk) and the two with a video encoding (TigerVNC's and FreeRDP's
    /// H.264, through `vento/video.h`): the phone's MediaCodec, or libavcodec
    /// on a desktop. With `None`, a RustDesk session has no picture and
    /// reports that, and TigerVNC and FreeRDP never offer H.264.
    pub decoders: Option<Arc<dyn Decoders>>,
    /// The output for the far end's sound, for the backends that have sound
    /// (TigerVNC's QEMU audio and FreeRDP's rdpsnd through `vento/audio.h`,
    /// SPICE's, IronRDP's and RustDesk's in Rust): the phone's AAudio, or on
    /// a desktop PipeWire (ALSA when no daemon answers). One per session, so
    /// that its controls (the switch, and the mute a window sets) apply to
    /// this session only. With `None`, the sound is dropped.
    pub audio: Option<Arc<dyn Outputs>>,
    /// Where the session's connections go: [`Direct`](runtime::net::Direct)
    /// for this end's own TCP connections, as every client has by default.
    /// The Rust backends dial through it, and the C backends through its
    /// table in `vento/net.h` (`api::c::net`). RealVNC's core dials for
    /// itself: on a network that is not direct its socket calls are hooked
    /// to go through this one (`realvnc_net.h` in its module), and on a
    /// direct one it dials as it always has.
    pub network: Arc<dyn Network>,
}

impl Connect<'_> {
    /// `DOMAIN\user`, `user@domain` or a bare name, as `(domain, user)`.
    /// A domain in the name overrides `domain`, which applies to a bare
    /// name.
    pub fn domain_and_user(&self) -> (String, String) {
        match crate::credentials::split_domain(self.user) {
            (named, user) if named.is_empty() => (self.domain.to_string(), user),
            split => split,
        }
    }
}

/// Twenty seconds, the same as RealVNC's Java backend: long enough for a
/// server to finish a handshake, short enough that a mistyped address does
/// not leave the connection spinning indefinitely.
pub const CONNECT_TIMEOUT_MS: i32 = 20_000;

/// One backend: what it is, and how to start a session on it. Derefs to its
/// [`About`], so `kind.id` and `kind.options` read as they always have.
pub struct Kind {
    pub about: &'static About,
    /// The library's own version string; calling it proves the library
    /// loads and links.
    pub version: fn() -> String,
    /// The backend's own constructor; sessions are started with
    /// [`Kind::start`], which wraps it.
    pub create: fn(&Connect) -> Result<Arc<dyn Backend>, String>,
    /// Forgets what the backend's library stores under `dirs` about the
    /// server `address` names
    /// ([`trust::same_server`](crate::trust::same_server)), when the client
    /// forgets that server: returns a user-facing line per item removed.
    /// Never connects anywhere. `None` for a backend whose library stores
    /// nothing about servers, which is all but RealVNC (whose core remembers
    /// the servers it accepted). The phone's counterpart is
    /// `BackendProvider.forgetHosts`.
    pub forget: Option<Forget>,
}

/// [`Kind::forget`]: takes the client's directories and the address, and
/// returns a line per item removed, or why the backend's record could not be
/// changed.
pub type Forget = fn(&Dirs, &str) -> Result<Vec<String>, String>;

impl std::ops::Deref for Kind {
    type Target = About;

    fn deref(&self) -> &About {
        self.about
    }
}

impl Kind {
    /// Starts a session on this backend: calls its `create`, after the setup
    /// shared by every backend. That includes `AcceptBell`: the adapters and
    /// the libraries behind the vtable pass a bell on regardless of the
    /// option, so it is dropped at the mailbox instead, once for all of them.
    /// (RealVNC's core is told as well, since its library has its own
    /// switch.) The option is not live in the catalog, so this is set once
    /// per session.
    ///
    /// Sound is likewise reported the same way for every backend: the
    /// session's audio controls report it through the mailbox as
    /// [`Event::Sound`](crate::events::Event::Sound), whichever backend's
    /// streams start and stop and whichever offers the switch.
    pub fn start(&self, connect: &Connect) -> Result<Arc<dyn Backend>, String> {
        if self.options.iter().any(|s| s.key == "AcceptBell") {
            connect.mailbox.accept_bell(connect.options.bool("AcceptBell"));
        }
        if let Some(audio) = &connect.audio {
            // Weak, so that a stream leaked by a backend past its session
            // does not keep the mailbox alive.
            let mailbox = Arc::downgrade(&connect.mailbox);
            audio.controls().on_sound(Box::new(move |sound| {
                if let Some(mailbox) = mailbox.upgrade() {
                    mailbox.on_sound(sound);
                }
            }));
        }
        (self.create)(connect)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::Event;
    use runtime::media::audio::{Controls, Device, Format, Queue};

    struct Quiet;
    impl Device for Quiet {}

    struct Fake(Arc<Controls>);

    impl Outputs for Fake {
        fn controls(&self) -> &Arc<Controls> {
            &self.0
        }

        fn device(&self, _queue: Arc<Queue>) -> Result<Box<dyn Device>, String> {
            Ok(Box::new(Quiet))
        }
    }

    #[test]
    fn a_session_says_its_sound_and_its_switch() {
        static KIND: Kind = Kind {
            about: &crate::catalog::FIXTURE,
            version: String::new,
            create: |_| Err("not here".into()),
            forget: None,
        };
        let audio = Arc::new(Fake(Arc::new(Controls::new())));
        let mailbox = Mailbox::new(None);
        let options = Options::default();
        let dirs = Dirs {
            config: PathBuf::new(),
            state: PathBuf::new(),
            realvnc_libs: PathBuf::new(),
        };
        let connect = Connect {
            address: "",
            user: "",
            domain: "",
            password: None,
            options: &options,
            mailbox: Arc::clone(&mailbox),
            dirs: &dirs,
            screen: None,
            last_size: None,
            identity: None,
            client_name: "",
            platform: Platform::Linux,
            decoders: None,
            audio: Some(Arc::clone(&audio) as Arc<dyn Outputs>),
            network: Arc::new(runtime::net::Direct),
        };
        assert!(KIND.start(&connect).is_err());
        let mut stream = audio.open(Format::new(48_000, 2).unwrap()).unwrap();
        stream.write(&[0, 0]);
        let playing = |playing: bool| {
            Some(Event::Sound(runtime::media::audio::Sound {
                switch: playing.then_some(runtime::media::audio::Switch::Mute),
                enabled: true,
                muted: false,
                playing,
            }))
        };
        assert_eq!(mailbox.take(), playing(true));
        drop(stream);
        assert_eq!(mailbox.take(), playing(false));
        audio.controls().offer_enable(false, Box::new(|_| {}));
        assert!(matches!(mailbox.take(), Some(Event::Sound(s)) if s.switch == Some(runtime::media::audio::Switch::Enable) && !s.enabled));
    }

    #[test]
    fn a_domain_in_the_user_name_overrides_the_domain() {
        let options = Options::default();
        let dirs = Dirs {
            config: PathBuf::new(),
            state: PathBuf::new(),
            realvnc_libs: PathBuf::new(),
        };
        let split = |user: &str, domain: &str| {
            Connect {
                address: "",
                user,
                domain,
                password: None,
                options: &options,
                mailbox: Mailbox::new(None),
                dirs: &dirs,
                screen: None,
                last_size: None,
                identity: None,
                client_name: "",
                platform: Platform::Linux,
                decoders: None,
                audio: None,
                network: Arc::new(runtime::net::Direct),
            }
            .domain_and_user()
        };
        assert_eq!(split("alice", ""), ("".into(), "alice".into()));
        assert_eq!(split("alice", "CORP"), ("CORP".into(), "alice".into()));
        assert_eq!(split("LAB\\alice", ""), ("LAB".into(), "alice".into()));
        assert_eq!(split("LAB\\alice", "CORP"), ("LAB".into(), "alice".into()));
        assert_eq!(split("alice@lab.example", "CORP"), ("lab.example".into(), "alice".into()));
        assert_eq!(split(".\\alice", "CORP"), (".".into(), "alice".into()));
        assert_eq!(split("", "CORP"), ("CORP".into(), "".into()));
        assert_eq!(
            split("MicrosoftAccount\\a@corp.example", "CORP"),
            ("MicrosoftAccount".into(), "a@corp.example".into())
        );
        assert_eq!(split("a@outlook.com", "CORP"), ("MicrosoftAccount".into(), "a@outlook.com".into()));
    }
}
