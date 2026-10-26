// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! One RustDesk session: the handshake, the message loop, and everything the
//! app can ask of an established connection.
//!
//! The protocol is small (one framed protobuf `Message` with a `oneof` of
//! thirty arms), and most of RustDesk's product features are in arms this app
//! has no use for. This implements the seven a remote desktop needs: the login
//! exchange, the picture, the cursor, the clipboard, both forms of input, and
//! the timing message, which has to be answered or the link saturates.
//!
//! Three behaviours of the far end cannot be inferred from the `.proto`, and
//! each was found by testing:
//!
//! - **`TestDelay` is echoed unchanged**, and only when its flag says the peer
//!   started the exchange. RustDesk's server echoes anything with that flag
//!   set, so a client that sets it on a reply gets its own message back and
//!   answers it again: 9701 messages in fifteen seconds on an idle session.
//! - **A down event carries every modifier held.** RustDesk's server
//!   *releases*, on the far end, each modifier not named in the event, so a
//!   Ctrl pressed for the next letter is released as soon as that letter
//!   arrives. The same applies to a mouse button press, which is what makes
//!   Ctrl+click work.
//! - **A Windows peer with more than one logged-in session waits**: it
//!   subscribes no video service until the client answers with a session id.
//!   Nothing signals the wait: the login succeeds, and the desktop never
//!   arrives.

use std::collections::{HashMap, VecDeque};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::mpsc::{RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use protobuf::{Message as _, MessageField};
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::framebuffer::{Back, Framebuffer};
use crate::keymap::{self, Key};
use crate::crypto;
use crate::protos::message::{
    CaptureDisplays, Clipboard, ClipboardFormat, ControlKey, DisplayResolution, Hash, KeyEvent,
    ImageQuality, KeyboardMode, LoginRequest, Message, Misc, MouseEvent, OptionMessage, PeerInfo,
    PublicKey, Resolution, SupportedDecoding, SwitchDisplay, TestDelay, VideoFrame, login_response,
    message, misc, option_message::BoolOption, permission_info::Permission, supported_decoding::PreferCodec,
    video_frame,
};
use crate::rendezvous;
use crate::wire::{self, Traffic};
use runtime::media::audio::decode::Codec as SoundCodec;
use runtime::media::audio::{Format, Outputs, Player};
use runtime::{Asked, Handler, Hangup};
use runtime::media::video::{self, Codec, Decoded, Decoders, GpuPicture, Hardware};

/// RustDesk's default direct-access port, `RENDEZVOUS_PORT + 2`.
pub const DIRECT_PORT: u16 = 21118;

/// The version this client reports. The peer compares it with the versions
/// that introduced its features to choose compatibility paths for older
/// clients, so it is a version whose gated behaviour this client was checked
/// against.
pub const VERSION: &str = "1.5.0";

/// The peer's `TestDelay` arrives about once a second regardless of activity,
/// so silence for this long means the link is gone. A timeout is fatal rather
/// than retried: a read that gives up part-way through a frame leaves the
/// stream out of sync, with nothing to resynchronise on.
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// The connecting screen's status while the far end's user is shown its
/// accept dialog: for a login without a password, a wrong one, or any login
/// to a peer that takes none ("No Password Access"). Upstream's client says
/// "Please wait for the remote side to accept your session request...".
const WAITING_FOR_ACCEPT: &str = "Waiting for the remote user to accept the session…";

/// How long the reader waits for a message to begin arriving while a password
/// prompt is open, before it looks again whether the prompt was answered.
const PROMPT_POLL: Duration = Duration::from_millis(100);

/// How often to send something while a prompt is on screen; must be well
/// under the thirty seconds of silence the peer allows.
const KEEP_ALIVE_EVERY: Duration = Duration::from_secs(8);

/// The frame rate requested while the session is not focused. There is no
/// pause message in this protocol, and one frame a second is the closest
/// substitute: the picture stays recent enough to show as soon as the session
/// is focused again, at a fraction of the traffic.
const UNFOCUSED_FPS: i32 = 1;

/// How often the decoder's status line is rebuilt: otherwise it would be a
/// string per frame, for an overlay read by eye.
const STATUS_EVERY: Duration = Duration::from_millis(500);

pub struct Config {
    /// The peer: `host` or `host:port` for [`Reach::Direct`], and its nine-digit
    /// id for [`Reach::Id`].
    pub address: String,
    pub reach: Reach,
    pub password: Option<String>,
    pub connect_timeout: Duration,
    /// The name the far end shows for this client in its log and on screen.
    pub my_name: String,
    /// The platform this end reports in the login.
    pub platform: Platform,
    /// The live options' initial values. After that the session holds them,
    /// since each can be changed while it is running.
    pub quality: Quality,
    /// 0 lets the far end choose.
    pub fps: i32,
    pub codec: PreferredCodec,
    /// Whether the far end locks its screen when this session ends. Sent with
    /// the login and never changed, since it takes effect after the last
    /// message either end sends.
    pub lock_after: bool,
    /// Where the peer's sound goes, or `None` for a session that never
    /// requests it. Requested only where the platform decodes Opus, the only
    /// format the peer sends.
    pub audio: Option<Arc<dyn Outputs>>,
    /// Whether the peer's sound is requested at login. This is the initial
    /// state of the session's Enable switch, which changes it later with the
    /// option message.
    pub sound: bool,
}

/// How long a stream stays open after the last audible sound: the gap between
/// sounds it bridges, as in the other backends. The peer keeps sending silence
/// for eight seconds after its desktop goes quiet and then sends nothing (its
/// "zero gate"), so this also makes a session that has gone quiet hide its
/// Sound button ten seconds later.
const QUIET: Duration = Duration::from_secs(2);

/// The platform the login reports. Not cosmetic: the peer enables features
/// based on it (a file clipboard only between two Windows or two macOS ends, a
/// printer only for a Windows one, the multiple clipboard formats refused to an
/// Android client older than 1.3.3), and it is part of how a trusted device is
/// recognised. So each client reports what RustDesk's own client would on the
/// same machine: `Android` from its Android build, `whoami::platform()`
/// (`Linux`) from its desktop one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Android,
    Linux,
}

impl Platform {
    /// RustDesk's spelling, which is `whoami::Platform`'s `Display`.
    pub fn wire(self) -> &'static str {
        match self {
            Platform::Android => "Android",
            Platform::Linux => "Linux",
        }
    }
}

/// The two ways to reach a peer, which are separate modes rather than two
/// transports for one connection.
///
/// Direct access is a mode enabled on the far end, reaching a machine that is
/// already addressable, and it has no key exchange at all. The id path reaches
/// a machine that may not be addressable from this network, and encryption is
/// part of it rather than a separate choice: the session is verifiable through
/// the peer key the rendezvous server signs.
pub enum Reach {
    Direct,
    Id {
        /// `host` or `host:port`; empty for the public network.
        server: String,
        /// The server's public key, base64, as in its `id_ed25519.pub`; empty
        /// for the public network's key, which is built into every RustDesk
        /// build.
        key: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    Low,
    Balanced,
    Best,
}

impl Quality {
    fn wire(self) -> ImageQuality {
        match self {
            Quality::Low => ImageQuality::Low,
            Quality::Balanced => ImageQuality::Balanced,
            Quality::Best => ImageQuality::Best,
        }
    }

    fn of(ordinal: i32) -> Quality {
        match ordinal {
            0 => Quality::Low,
            2 => Quality::Best,
            _ => Quality::Balanced,
        }
    }

    fn ordinal(self) -> i32 {
        match self {
            Quality::Low => 0,
            Quality::Balanced => 1,
            Quality::Best => 2,
        }
    }
}

/// Which codec the far end is asked to encode in.
///
/// A preference rather than an instruction: the peer picks from what it can
/// encode and what this end says it can decode, and [`Auto`](Self::Auto) leaves
/// the choice to the peer. RustDesk's server then uses whatever hardware
/// encoder the machine has, falling back to VP9, the one codec compiled into
/// every RustDesk build.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreferredCodec {
    Auto,
    Vp9,
    Vp8,
    Av1,
    H264,
    H265,
}

impl PreferredCodec {
    fn wire(self) -> PreferCodec {
        match self {
            PreferredCodec::Auto => PreferCodec::Auto,
            PreferredCodec::Vp9 => PreferCodec::VP9,
            PreferredCodec::Vp8 => PreferCodec::VP8,
            PreferredCodec::Av1 => PreferCodec::AV1,
            PreferredCodec::H264 => PreferCodec::H264,
            PreferredCodec::H265 => PreferCodec::H265,
        }
    }

    /// The requested codec, or `None` for [`Auto`](Self::Auto). Used to check
    /// the preference before sending it: requesting a codec this end cannot
    /// decode leaves the session without a picture, and the far end cannot
    /// tell.
    fn codec(self) -> Option<Codec> {
        match self {
            PreferredCodec::Auto => None,
            PreferredCodec::Vp9 => Some(Codec::Vp9),
            PreferredCodec::Vp8 => Some(Codec::Vp8),
            PreferredCodec::Av1 => Some(Codec::Av1),
            PreferredCodec::H264 => Some(Codec::H264),
            PreferredCodec::H265 => Some(Codec::H265),
        }
    }

    fn ordinal(self) -> i32 {
        match self {
            PreferredCodec::Auto => 0,
            PreferredCodec::Vp9 => 1,
            PreferredCodec::Vp8 => 2,
            PreferredCodec::Av1 => 3,
            PreferredCodec::H264 => 4,
            PreferredCodec::H265 => 5,
        }
    }

    fn of(ordinal: i32) -> PreferredCodec {
        match ordinal {
            1 => PreferredCodec::Vp9,
            2 => PreferredCodec::Vp8,
            3 => PreferredCodec::Av1,
            4 => PreferredCodec::H264,
            5 => PreferredCodec::H265,
            _ => PreferredCodec::Auto,
        }
    }
}

/// The handshake's outcome, which the app still has to accept.
enum Verified {
    /// The peer proved it holds the key the rendezvous server vouched for, and
    /// both directions of the socket are encrypted with keys agreed with it,
    /// made by this key exchange version.
    Encrypted { peer_key: [u8; 32], kx_version: u32 },
    /// A check failed, for this reason, and the session is unencrypted, which
    /// the app still has to agree to.
    Clear(String),
}

/// The Security row of an encrypted session: the cipher, the key exchange
/// version, and who vouched for the peer's key, the rendezvous server whose
/// key the user entered (`own_server`) or the public network.
fn encrypted_security(kx_version: u32, own_server: bool) -> String {
    format!(
        "xsalsa20-poly1305, key exchange version {kx_version}, vouched for by {}",
        if own_server {
            "your rendezvous server"
        } else {
            "the public network"
        }
    )
}

/// The facts shown in the connection panel.
#[derive(Clone, Default)]
pub struct Info {
    pub desktop_name: String,
    pub protocol: String,
    pub connection: String,
    pub security: String,
    pub encoding: String,
    /// The round trip measured by the peer's timing message, the only
    /// link measurement this protocol offers.
    pub round_trip: String,
    pub platform: String,
    pub display: String,
}

/// The session's current decoder and what it was created for: a codec, a
/// picture size and a decoding mode. A change in any of them creates a new
/// one.
struct Current {
    codec: Codec,
    size: (usize, usize),
    hardware: Hardware,
    /// The hardware flag the Encoding row last showed.
    shown: Option<bool>,
    /// When its status line was last built, `None` before the first.
    status_at: Option<Instant>,
    decoder: Box<dyn video::Decoder>,
}

/// The Encoding row: the codec, and whether it is decoded in hardware when the
/// decoder reports it (not before the first picture, nor for a decoder that
/// never reports it).
fn encoding(codec: Codec, hardware: Option<bool>) -> String {
    match hardware {
        Some(true) => format!("{} (hardware)", codec.name()),
        Some(false) => format!("{} (software)", codec.name()),
        None => codec.name().to_string(),
    }
}

/// RustDesk's mouse event types, and its buttons as mask bits.
mod mouse {
    pub const TYPE_MOVE: i32 = 0;
    pub const TYPE_DOWN: i32 = 1;
    pub const TYPE_UP: i32 = 2;
    pub const TYPE_WHEEL: i32 = 3;

    pub const LEFT: i32 = 0x01;
    pub const RIGHT: i32 = 0x02;
    pub const MIDDLE: i32 = 0x04;
    pub const BACK: i32 = 0x08;
    pub const FORWARD: i32 = 0x10;
}

/// RFB button mask bits (what the input stack uses) mapped to RustDesk's.
/// The wheel is not included: it is four bits of the same mask and becomes an
/// axis rather than a button.
const BUTTONS: [(i32, i32); 5] = [
    (0x01, mouse::LEFT),
    (0x02, mouse::MIDDLE),
    (0x04, mouse::RIGHT),
    (0x80, mouse::BACK),
    (0x100, mouse::FORWARD),
];

/// How far one wheel notch scrolls. RustDesk's server passes the number
/// directly to the platform's scroll call, so the unit is that platform's
/// line, and one notch is one line on both test far ends.
const WHEEL_NOTCH: i32 = 1;

/// What [`Client::reach`] returns: the session's socket, the introduction still
/// to be checked for a peer reached by id (its signed key and the rendezvous
/// key that vouches for it), and the panel's description of the connection.
struct Dialled {
    sock: TcpStream,
    introduction: Option<(Vec<u8>, String)>,
    connection: String,
}

pub struct Client {
    framebuffer: RwLock<Framebuffer>,
    /// The platform's video decoders, not known in advance here: they are
    /// queried for what to advertise to the far end whenever the options are
    /// sent, and a decoder is created from them whenever the codec or the size
    /// changes.
    decoders: Arc<dyn Decoders>,
    traffic: Arc<Traffic>,
    /// Serialized messages queued for the writer thread. Input is encoded on
    /// the thread that produced it and written on a separate thread, so a
    /// stalled socket never blocks the main one.
    out: Mutex<Option<Sender<Vec<u8>>>>,
    /// The sockets, for closing them from another thread: shutting a socket
    /// down is the only way to wake a read or dial blocked on it. Each
    /// rendezvous socket is held here while it is being waited on, as well as
    /// the session's own.
    hangup: Hangup,
    info: Mutex<Info>,
    /// The decoder's status line, for an overlay: built on the protocol
    /// thread, where the decoder is, at most every [`STATUS_EVERY`].
    decoder_status: Mutex<Option<String>>,
    /// The peer name the login sends: for a direct connection the address the
    /// socket connected to, and for an id that id.
    login_as: Mutex<String>,
    /// The sizes the far end accepts, a fixed list rather than RFB's free
    /// choice; empty for a peer that offers none.
    resolutions: Mutex<Vec<(i32, i32)>>,
    /// The far end's displays as rectangles of its whole desktop, in the peer's
    /// order; empty if it has only one, since there is nothing to choose.
    displays: Mutex<Vec<(i32, i32, i32, i32)>>,
    /// The captured display's top left corner on the peer's whole desktop.
    /// Input is injected into that desktop rather than into the picture, so a
    /// pointer event on a second screen is off by this much unless it is
    /// added, and lands at those coordinates on the primary one. The pair is
    /// under one lock because a switch changes both.
    origin: Mutex<(i32, i32)>,
    /// What each held key was sent as, by the key id the app uses to match a
    /// release to its press.
    keys: Mutex<HashMap<i64, Key>>,
    /// The modifiers held, in the order they went down. Every down event
    /// carries this list.
    modifiers: Mutex<Vec<ControlKey>>,
    /// The last pointer position and mask, so that a button press does not
    /// repeat an unchanged move.
    pointer: Mutex<(i32, i32, i32)>,
    view_only: AtomicBool,
    focused: AtomicBool,
    width: AtomicI32,
    height: AtomicI32,
    display: AtomicI32,
    /// The live options as last sent, held here because each can be changed
    /// while the session is running.
    quality: AtomicI32,
    /// The configured frame rate. Losing focus lowers the rate sent, and
    /// regaining it restores this one.
    fps: AtomicI32,
    /// The requested codec, and the last one sent to the peer. The second
    /// keeps a quality change from causing a codec renegotiation: the
    /// preference is sent only when it has changed.
    codec: AtomicI32,
    codec_sent: AtomicI32,
    /// The last round trip measured by the peer's timing message, in
    /// milliseconds.
    round_trip: AtomicU64,
    /// Whether the options request the peer's sound: set from the [`Config`]
    /// when the session runs and by the session's switch, and read whenever
    /// the options are sent. Shared with the switch, which is called on a
    /// window's thread.
    sound: Arc<AtomicBool>,
}

impl Client {
    /// A client that decodes its picture with `decoders`.
    ///
    /// Given here rather than in the [`Config`] because it is also needed
    /// outside [`run`](Client::run): a live option change, on whatever thread
    /// makes it, re-sends what this end can decode, and only the client is
    /// shared with that thread.
    pub fn new(decoders: Arc<dyn Decoders>) -> Client {
        Client {
            framebuffer: RwLock::new(Framebuffer::new()),
            decoders,
            traffic: Arc::new(Traffic::default()),
            out: Mutex::new(None),
            hangup: Hangup::new(),
            info: Mutex::new(Info::default()),
            decoder_status: Mutex::new(None),
            login_as: Mutex::new(String::new()),
            resolutions: Mutex::new(Vec::new()),
            displays: Mutex::new(Vec::new()),
            origin: Mutex::new((0, 0)),
            keys: Mutex::new(HashMap::new()),
            modifiers: Mutex::new(Vec::new()),
            pointer: Mutex::new((-1, -1, 0)),
            view_only: AtomicBool::new(false),
            focused: AtomicBool::new(true),
            width: AtomicI32::new(0),
            height: AtomicI32::new(0),
            display: AtomicI32::new(0),
            quality: AtomicI32::new(Quality::Balanced.ordinal()),
            fps: AtomicI32::new(0),
            codec: AtomicI32::new(PreferredCodec::Auto.ordinal()),
            codec_sent: AtomicI32::new(-1),
            round_trip: AtomicU64::new(0),
            sound: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn framebuffer(&self) -> &RwLock<Framebuffer> {
        &self.framebuffer
    }

    /// Received and sent, in bytes, since the socket was opened.
    pub fn traffic(&self) -> (u64, u64) {
        (
            self.traffic.received.load(Ordering::Relaxed),
            self.traffic.sent.load(Ordering::Relaxed),
        )
    }

    pub fn info(&self) -> Info {
        let mut info = self.info.lock().unwrap().clone();
        let rtt = self.round_trip.load(Ordering::Relaxed);
        info.round_trip = if rtt == 0 {
            String::new()
        } else {
            format!("{rtt} ms")
        };
        info
    }

    /// The picture, if it was left on the GPU (see [`Client::picture`]).
    pub fn gpu_picture(&self) -> Option<Arc<dyn GpuPicture>> {
        self.framebuffer.read().unwrap().gpu()
    }

    /// How the picture is being decoded, as the decoder last reported: `None`
    /// before the first frame, and when there is no decoder.
    pub fn decoder_status(&self) -> Option<String> {
        self.decoder_status.lock().unwrap().clone()
    }

    /// Whether the far end offers a size it is not already at.
    ///
    /// Unlike RFB this is a list rather than a free choice, so a request gets
    /// the nearest entry, and a peer whose list holds only its current mode
    /// (every Xvfb) would offer a control that can only choose the current
    /// size.
    pub fn can_resize(&self) -> bool {
        if self.view_only.load(Ordering::Relaxed) {
            return false;
        }
        let (w, h) = (
            self.width.load(Ordering::Relaxed),
            self.height.load(Ordering::Relaxed),
        );
        self.resolutions
            .lock()
            .unwrap()
            .iter()
            .any(|offered| *offered != (w, h))
    }

    /// The far end's displays, four numbers each (x, y, width, height in its
    /// desktop), and the index of the captured one.
    ///
    /// This protocol sends **one** display at a time, so the list is a choice
    /// rather than a description of the picture: the picture shows one of these
    /// rectangles and none of the others.
    pub fn displays(&self) -> (Vec<i32>, i32) {
        let displays = self.displays.lock().unwrap();
        let flat = displays
            .iter()
            .flat_map(|(x, y, w, h)| [*x, *y, *w, *h])
            .collect();
        (flat, self.display.load(Ordering::Relaxed))
    }

    /// Asks the far end to capture another of its displays. The answer is a
    /// `SwitchDisplay`, so nothing here assumes the request succeeded.
    pub fn request_display(&self, index: i32) {
        if index < 0 || index as usize >= self.displays.lock().unwrap().len() {
            return;
        }
        let mut switch = SwitchDisplay::new();
        switch.display = index;
        let mut misc = Misc::new();
        misc.set_switch_display(switch);
        let mut out = Message::new();
        out.set_misc(misc);
        self.send(out);
    }

    /// Tells the peer which display to capture, and asks for a fresh picture.
    ///
    /// A client of this version is expected to send this pair once the peer
    /// has said what it is capturing: at login, and again whenever that
    /// changes.
    fn ask_for(&self, display: i32) {
        let mut captured = CaptureDisplays::new();
        captured.set = vec![display];
        let mut misc = Misc::new();
        misc.set_capture_displays(captured);
        let mut out = Message::new();
        out.set_misc(misc);
        self.send(out);
        self.refresh_video();
    }

    /// Asks for a fresh picture, which RustDesk's server sends as a key frame,
    /// needed by a decoder that starts mid-stream before it can draw anything.
    fn refresh_video(&self) {
        let mut misc = Misc::new();
        misc.set_refresh_video(true);
        let mut out = Message::new();
        out.set_misc(misc);
        self.send(out);
    }

    pub fn close(&self) {
        self.hangup.hang_up();
    }

    // ---- the session ------------------------------------------------------

    /// Connects, logs in, and runs until either end stops it. The calling
    /// thread is the protocol thread and every callback arrives on it.
    pub fn run(&self, config: &Config, handler: &mut dyn Handler) -> Result<()> {
        self.quality.store(config.quality.ordinal(), Ordering::Relaxed);
        self.fps.store(config.fps, Ordering::Relaxed);
        self.codec.store(config.codec.ordinal(), Ordering::Relaxed);
        self.sound.store(config.sound && sound_of(config).is_some(), Ordering::Relaxed);
        // Withdraws the sound switch when the session ends, however it ends.
        let _withdraw = Withdraw(sound_of(config));

        // The two modes differ only in how the socket is obtained; from here
        // on the session is the same.
        let Dialled {
            sock,
            introduction,
            connection,
        } = match self.reach(config) {
            Err(_) if self.hangup.is_hung_up() => return Err(Error::Closed),
            other => other?,
        };
        // Held for the rest of the session.
        let _holding = match self.hangup.hold(&sock) {
            Err(_) if self.hangup.is_hung_up() => return Err(Error::Closed),
            other => other?,
        };
        let (mut reader, mut writer, _) = wire::split(sock, Arc::clone(&self.traffic))?;
        self.info.lock().unwrap().connection = connection;

        let verified = match introduction {
            Some((signed_id_pk, key)) => {
                reader.set_timeout(Some(config.connect_timeout))?;
                let verified =
                    self.secure(&mut reader, &mut writer, &config.address, &signed_id_pk, &key)?;
                let mut info = self.info.lock().unwrap();
                // The vouching server is the only trust difference between the
                // two ways of using this path: a key the user entered is their
                // own pin, and the public network's key is chosen by RustDesk.
                // A session the user accepted unverified reports both parts.
                info.security = match verified {
                    Verified::Encrypted { kx_version, .. } => {
                        encrypted_security(kx_version, !key.trim().is_empty())
                    }
                    Verified::Clear(_) => {
                        "None (unencrypted), and the machine was not verified".into()
                    }
                };
                Some(verified)
            }
            None => {
                // Shown in the panel, not only logged: this mode has no key
                // exchange at all, and the panel is where a session reports
                // its protection.
                self.info.lock().unwrap().security = "None (unencrypted)".into();
                // A direct-access peer expects a client without a key to send
                // an empty message, which is that mode's entire handshake.
                writer.send(&Message::new())?;
                None
            }
        };

        let (tx, rx) = channel::<Vec<u8>>();
        *self.out.lock().unwrap() = Some(tx);
        let writing = std::thread::Builder::new()
            .name("rustdesk-writer".into())
            .spawn(move || {
                for bytes in rx {
                    if writer.send_bytes(bytes).is_err() {
                        break;
                    }
                }
                writer.close();
            })
            .map_err(Error::Io)?;

        reader.set_timeout(Some(config.connect_timeout))?;
        let outcome = self.session(&mut reader, config, verified, handler);

        // Whether this end asked to stop, read **before** the cleanup below
        // closes the socket itself: an error caused by the user pressing
        // Disconnect is not reported, and any other error is.
        let asked = self.hangup.is_hung_up();
        // The sender keeps the writer thread alive, so it is dropped before
        // the join; the socket is shut down either way, since an ending that
        // came from the far end has not been through `close`.
        self.out.lock().unwrap().take();
        self.close();
        let _ = writing.join();
        match outcome {
            Err(_) if asked => Err(Error::Closed),
            other => other,
        }
    }

    fn reach(&self, config: &Config) -> Result<Dialled> {
        let (sock, introduction, connection) = match &config.reach {
            Reach::Direct => {
                let addr = resolve(&config.address)?;
                log::info!("connecting to {addr}");
                let sock = self.hangup.dial(addr, config.connect_timeout)?;
                sock.set_nodelay(true)?;
                // The login names the resolved address rather than the typed
                // one, because the peer checks it against three regular
                // expressions: a v4 literal with an optional port, a v6 one
                // either bare or bracketed **with** a port, or a dotted name
                // with a port. A bracketed literal without a port matches none
                // of them, and the refusal is `Offline`.
                *self.login_as.lock().unwrap() = addr.to_string();
                (sock, None, format!("Direct access to {addr}"))
            }
            Reach::Id { server, key } => {
                let server = if server.trim().is_empty() {
                    rendezvous::PUBLIC_SERVER
                } else {
                    server.trim()
                };
                let reached = rendezvous::reach(
                    &self.hangup,
                    &config.address,
                    server,
                    key.trim(),
                    config.connect_timeout,
                )?;
                let connection = reached.how.describe();
                // The id, which is the name the peer expects here.
                *self.login_as.lock().unwrap() = config.address.clone();
                (
                    reached.sock,
                    Some((reached.signed_id_pk, key.clone())),
                    connection,
                )
            }
        };
        Ok(Dialled {
            sock,
            introduction,
            connection,
        })
    }

    /// The key exchange (which direct mode lacks), ending with both directions
    /// of the socket encrypted, or, if a check fails, with neither and the
    /// reason.
    ///
    /// **Nothing is asked here, because of a deadline.** The peer allows a
    /// handshake **18 seconds** and drops the connection after that, which is
    /// less than a user may take to read a dialog (tested by leaving one on
    /// screen). So the exchange completes without waiting, and the app is asked
    /// afterwards, before the login, the first message carrying a secret.
    ///
    /// **A failure becomes a question rather than a refusal or a silent
    /// fallback.** RustDesk's own client falls back to an unencrypted session
    /// whenever a step fails (a bad signature, a key other than the one the
    /// server vouched for, a peer that sends no `SignedId`) and reports it only
    /// in its log, so its sessions' security cannot be trusted. Refusing
    /// outright would end a session the user may still want.
    fn secure(
        &self,
        reader: &mut wire::Reader,
        writer: &mut wire::Writer,
        peer_id: &str,
        signed_id_pk: &[u8],
        server_key: &str,
    ) -> Result<Verified> {
        let server_key = crypto::server_key(if server_key.trim().is_empty() {
            rendezvous::PUBLIC_KEY
        } else {
            server_key
        })?;
        // Step one: the server's introduction, which binds this id to this
        // long-term key.
        let vouched = if signed_id_pk.is_empty() {
            Err("The rendezvous server did not vouch for a key.".to_string())
        } else {
            crypto::open_id_pk(signed_id_pk, &server_key, peer_id)
                .map_err(|e| format!("The rendezvous server's introduction did not check out: {e}."))
        };
        let peer_key = match vouched {
            Ok(signed) => signed.key,
            // Nothing has been read from the peer yet, so it is waiting for a
            // client without a key to send the empty message.
            Err(why) => return self.in_the_clear(writer, why, false),
        };
        // Step two: the peer proves it holds that key, by signing a box key it
        // made for this connection alone. The app is asked about the result
        // later, in `session`, before the login.
        let signed = match reader.message::<Message>()?.union {
            Some(message::Union::SignedId(signed)) => signed,
            _ => {
                return self.in_the_clear(
                    writer,
                    "The machine offered no key of its own.".into(),
                    false,
                );
            }
        };
        let peer_box = match crypto::open_id_pk(&signed.id, &peer_key, peer_id) {
            Ok(signed) => signed,
            // The peer is waiting for a `PublicKey` here, so an empty one says
            // there will be no key, as RustDesk's client does.
            Err(e) => {
                return self.in_the_clear(
                    writer,
                    format!("The machine's key is not the one it was vouched for: {e}."),
                    true,
                );
            }
        };
        // Step three: our session key, sealed to the peer's box key, and the
        // key exchange version picked from the one the peer signed.
        // Everything after this frame is encrypted in both directions, each
        // with its own counter and, from version 1, its own key.
        let (our_box_pk, sealed, key) = crypto::seal_session_key(&peer_box.key)?;
        let picked = crypto::kx_version_for(peer_box.kx_version);
        let keys = crypto::session_keys(
            &key,
            &crypto::Transcript {
                initiator_pk: &our_box_pk,
                responder_pk: &peer_box.key,
                advertised: peer_box.kx_version,
                picked,
            },
        )?;
        let mut public_key = PublicKey::new();
        public_key.asymmetric_value = our_box_pk.to_vec();
        public_key.symmetric_value = sealed;
        public_key.kx_version = picked;
        let mut out = Message::new();
        out.set_public_key(public_key);
        writer.send(&out)?;
        writer.set_cipher(Box::new(crypto::Secretbox::new(keys.send)));
        reader.set_cipher(Box::new(crypto::Secretbox::new(keys.receive)));
        log::info!(
            "the session is encrypted, key exchange version {picked} (the peer offered {})",
            peer_box.kx_version
        );
        Ok(Verified::Encrypted {
            peer_key,
            kx_version: picked,
        })
    }

    /// Tells the peer there will be no key, and returns the reason for the
    /// question the app will be asked.
    ///
    /// `expects_public_key` is whether the peer has already offered a key:
    /// after its `SignedId` it is waiting for a `PublicKey`, and RustDesk's
    /// client declines with an empty one; before that it is waiting for the
    /// empty message a client without a key sends.
    fn in_the_clear(
        &self,
        writer: &mut wire::Writer,
        why: String,
        expects_public_key: bool,
    ) -> Result<Verified> {
        log::warn!("cannot verify the peer: {why}");
        let mut out = Message::new();
        if expects_public_key {
            out.set_public_key(PublicKey::new());
        }
        writer.send(&out)?;
        Ok(Verified::Clear(why))
    }

    fn session(
        &self,
        reader: &mut wire::Reader,
        config: &Config,
        verified: Option<Verified>,
        handler: &mut dyn Handler,
    ) -> Result<()> {
        // The peer sends `Hash` first, unprompted.
        let hash = loop {
            match reader.message::<Message>()?.union {
                Some(message::Union::Hash(h)) => break h,
                Some(other) => log::info!("before Hash: {}", arm(&other)),
                None => {}
            }
        };
        // Any question the handshake left is asked here: after the handshake,
        // because the peer would time it out while a dialog is open, and
        // before the login, which carries the password. Asking blocks this
        // thread, so a keep-alive stops the socket going quiet meanwhile.
        let agreed = match &verified {
            Some(Verified::Encrypted { peer_key, .. }) => {
                let alive = self.keep_alive();
                let answer = handler.trust(&crypto::fingerprint(peer_key));
                alive.stop();
                answer
            }
            Some(Verified::Clear(why)) => {
                let alive = self.keep_alive();
                let answer = handler.unverified(why);
                alive.stop();
                answer
            }
            None => true,
        };
        if !agreed {
            return Err(Error::Cancelled);
        }
        // **A login without a password is a request to the far end's user**,
        // who is shown an accept dialog for it, and the peer sends nothing
        // back until they answer. It never answers "Empty Password", although
        // RustDesk's client still handles one. RustDesk's client sends that
        // login and asks for the password at the same time, so that either
        // answer gets the session in; so does this one.
        // What the peer sent while a prompt was open, for the loop below to
        // take before it reads again.
        let mut early = VecDeque::new();
        let password = config.password.clone().unwrap_or_default();
        self.send_login(config, &hash, &password);
        if password.is_empty() {
            log::info!("no password to send: asking for one, while the peer asks its user");
            // Before the prompt, which covers the connecting screen and shows
            // the status itself.
            handler.status(Some(WAITING_FOR_ACCEPT));
            if let Prompted::Password(password) = self.ask_password(reader, handler, &mut early)? {
                self.send_login(config, &hash, &password);
            }
        }
        reader.set_timeout(Some(IDLE_TIMEOUT))?;

        let mut decoder: Option<Current> = None;
        let mut back = Back::new();
        let mut rate = Rate::new();
        let mut cursors = Cursors::default();
        let mut connected = false;
        // The peer's sound and its last announced format. A peer sends
        // something at least every second (its timing message), so checking
        // for quiet after every message is often enough.
        let mut sound = sound_of(config).map(Player::new);
        let mut sound_format: Option<Format> = None;
        // Whether the peer allows this session its sound. The peer sends a
        // PermissionInfo only when it does not, at the start of the
        // connection and whenever the setting changes on the far end.
        let mut sound_permitted = true;

        loop {
            let message = match early.pop_front().unwrap_or_else(|| reader.message::<Message>()) {
                Ok(m) => m,
                Err(e) if timed_out(&e) => return Err(Error::Silent),
                Err(e) => return Err(Error::Io(e)),
            };
            if let Some(player) = &mut sound {
                // Stopped at once: what the peer sent before receiving the
                // change is not worth the second it would take to play out.
                if !self.sound.load(Ordering::Relaxed) {
                    player.close();
                }
                player.close_if_quiet(QUIET);
            }
            let Some(union) = message.union else { continue };
            match union {
                message::Union::LoginResponse(response) => match response.union {
                    Some(login_response::Union::PeerInfo(peer)) => {
                        self.arrived(&peer, handler, &mut back)?;
                        connected = true;
                        // Once logged in, the peer may send its sound: offer
                        // an Enable switch, since the option message sent at
                        // login can be sent again at any time.
                        if sound_permitted && let Some(audio) = sound_of(config) {
                            self.offer_sound(&audio);
                        }
                    }
                    Some(login_response::Union::Error(why)) => match login_error(&why) {
                        // Answered on the same connection: RustDesk's client
                        // prompts again and sends another `LoginRequest`
                        // rather than reconnecting, and the peer waits for
                        // exactly that.
                        // The peer shows its accept dialog for a wrong
                        // password too, so the wait goes on meanwhile.
                        LoginError::Password => {
                            log::info!("the peer wants a password ({why})");
                            handler.status(Some(WAITING_FOR_ACCEPT));
                            if let Prompted::Password(password) = self.ask_password(reader, handler, &mut early)? {
                                self.send_login(config, &hash, &password);
                            }
                        }
                        // A peer that takes no password: its user accepts or
                        // declines the session, and it says nothing more
                        // until then. RustDesk's client shows a wait for this
                        // rather than an error.
                        LoginError::Wait => {
                            log::info!("the peer waits for its user to accept the session ({why})");
                            handler.status(Some(WAITING_FOR_ACCEPT));
                        }
                        LoginError::Fatal => return Err(Error::Refused(why)),
                    },
                    None => {}
                },
                message::Union::VideoFrame(frame) => {
                    if connected {
                        let started = Instant::now();
                        self.picture(frame, &mut decoder, &mut back, handler);
                        rate.frame(started.elapsed(), back.converted());
                        // Sent for every frame, drawn or not: the peer waits
                        // for this before capturing the next one, so a frame
                        // that changed nothing still has to be acknowledged.
                        let mut misc = Misc::new();
                        misc.set_video_received(true);
                        let mut out = Message::new();
                        out.set_misc(misc);
                        self.send(out);
                    }
                }
                message::Union::CursorData(data) => {
                    if let Some(cursor) = cursors.arrived(data) {
                        cursor.deliver(handler);
                    }
                }
                message::Union::CursorId(id) => {
                    if let Some(cursor) = cursors.get(id) {
                        cursor.deliver(handler);
                    }
                }
                message::Union::Clipboard(clip) => self.clipboard_arrived(clip, handler),
                message::Union::MultiClipboards(many) => {
                    // Only the text one is used; the other formats have no
                    // equivalent in the session API.
                    for clip in many.clipboards {
                        self.clipboard_arrived(clip, handler);
                    }
                }
                message::Union::TestDelay(delay) => self.test_delay(delay),
                message::Union::MessageBox(box_) => {
                    log::info!("message box: [{}] {} {}", box_.msgtype, box_.title, box_.text);
                }
                message::Union::Misc(misc) => match misc.union {
                    Some(misc::Union::CloseReason(why)) => return Err(Error::Refused(why)),
                    Some(misc::Union::PermissionInfo(permission))
                        if permission.permission.enum_value() == Ok(Permission::Audio) =>
                    {
                        log::info!("the peer {} its sound", if permission.enabled { "allows" } else { "withholds" });
                        sound_permitted = permission.enabled;
                        if let Some(audio) = sound_of(config).filter(|_| connected) {
                            if permission.enabled {
                                self.offer_sound(&audio);
                            } else {
                                audio.controls().withdraw();
                            }
                        }
                    }
                    Some(misc::Union::SwitchDisplay(display)) => {
                        self.switched(&display, handler, &mut back);
                    }
                    // The format of the Opus frames that follow: one of Opus's
                    // own rates, to which the peer resamples its device.
                    Some(misc::Union::AudioFormat(format)) => {
                        sound_format = Format::new(format.sample_rate, format.channels as usize);
                        log::info!(
                            "the peer sends its sound as Opus at {} Hz in {} channel(s){}",
                            format.sample_rate,
                            format.channels,
                            if sound_format.is_some() { "" } else { ", which is not played" }
                        );
                    }
                    _ => {}
                },
                // Frames still in transit when the switch was turned off are
                // dropped rather than reopening a stream.
                message::Union::AudioFrame(frame) if self.sound.load(Ordering::Relaxed) => {
                    if let (Some(player), Some(format)) = (&mut sound, sound_format) {
                        player.packet(SoundCodec::Opus, format, &[], &frame.data);
                    }
                }
                _ => {}
            }
        }
    }

    /// The one-time setup once the peer has identified itself.
    fn arrived(
        &self,
        peer: &PeerInfo,
        handler: &mut dyn Handler,
        back: &mut Back,
    ) -> Result<()> {
        let current = peer.current_display.max(0) as usize;
        let Some(display) = peer.displays.get(current) else {
            return Err(Error::Protocol("the peer has no display".into()));
        };
        log::info!(
            "{} on {} running {}, display {} of {}, {}x{}",
            peer.hostname,
            peer.platform,
            peer.version,
            current + 1,
            peer.displays.len(),
            display.width,
            display.height
        );
        self.display.store(current as i32, Ordering::Relaxed);
        *self.origin.lock().unwrap() = (display.x, display.y);
        {
            let mut info = self.info.lock().unwrap();
            info.desktop_name = peer.hostname.clone();
            info.protocol = format!("RustDesk {}", peer.version);
            info.platform = peer.platform.clone();
        }
        // A peer with one display offers nothing to choose, and the panel
        // shows no display row for an empty list.
        *self.displays.lock().unwrap() = if peer.displays.len() > 1 {
            peer.displays
                .iter()
                .map(|d| (d.x, d.y, d.width, d.height))
                .collect()
        } else {
            Vec::new()
        };
        self.say_display(current as i32);
        *self.resolutions.lock().unwrap() = peer
            .resolutions
            .resolutions
            .iter()
            .map(|r| (r.width, r.height))
            .collect();

        // A Windows peer with RDP sharing on and more than one logged-in
        // session subscribes no video service until it is told which session
        // to capture. Answering with any session other than the service's own
        // hands the connection to that session and drops this one, so the
        // answer is the peer's current one.
        if let Some(sessions) = peer.windows_sessions.as_ref() {
            log::info!(
                "the peer offers {} windows sessions, current {}",
                sessions.sessions.len(),
                sessions.current_sid
            );
            let mut misc = Misc::new();
            misc.set_selected_sid(sessions.current_sid);
            let mut out = Message::new();
            out.set_misc(misc);
            self.send(out);
        }
        // The display to capture, and a refresh so that the first picture is a
        // key frame instead of waiting for the encoder's next one.
        self.ask_for(current as i32);

        self.resize(display.width, display.height, back);
        self.width.store(display.width, Ordering::Relaxed);
        self.height.store(display.height, Ordering::Relaxed);
        handler.connected(display.width.max(0) as usize, display.height.max(0) as usize);
        Ok(())
    }

    /// The far end changed which display it captures, or its size.
    ///
    /// One message covers both, and the peer sends it whether or not the change
    /// was requested: a display unplugged on the far end arrives the same way
    /// as a switch. The sizes it accepts come with it, since they belong to the
    /// new display.
    fn switched(
        &self,
        switch: &SwitchDisplay,
        handler: &mut dyn Handler,
        back: &mut Back,
    ) {
        let (width, height) = (switch.width, switch.height);
        // Before the size checks below, which both return early: the
        // picture's position on the peer's desktop decides where input lands,
        // and a switch that changed nothing else may still have moved it.
        *self.origin.lock().unwrap() = (switch.x, switch.y);
        if self.display.swap(switch.display, Ordering::Relaxed) != switch.display {
            // The capture set and the key frame again, for the same reason as
            // at login: the peer has said what it now captures, and this end
            // has to confirm what it wants.
            self.ask_for(switch.display);
        }
        self.say_display(switch.display);
        if !switch.resolutions.resolutions.is_empty() {
            *self.resolutions.lock().unwrap() = switch
                .resolutions
                .resolutions
                .iter()
                .map(|r| (r.width, r.height))
                .collect();
        }
        if width <= 0 || height <= 0 {
            return;
        }
        // Both swapped before either is compared: with `&&` the height would
        // not be swapped whenever the width had changed, giving a framebuffer
        // sized 1024×1440 for a 1024×768 display and no picture.
        let was_width = self.width.swap(width, Ordering::Relaxed);
        let was_height = self.height.swap(height, Ordering::Relaxed);
        if was_width == width && was_height == height {
            return;
        }
        log::info!("the desktop is now {width}x{height}");
        self.resize(width, height, back);
        handler.desktop_size(width as usize, height as usize);
    }

    /// The panel's diagnostic line saying which of the peer's displays is
    /// shown, empty if there is only one.
    fn say_display(&self, current: i32) {
        let count = self.displays.lock().unwrap().len();
        self.info.lock().unwrap().display = if count > 1 {
            format!("{} of {}", current + 1, count)
        } else {
            String::new()
        };
    }

    fn resize(&self, width: i32, height: i32, back: &mut Back) {
        let (w, h) = (width.max(0) as usize, height.max(0) as usize);
        self.framebuffer.write().unwrap().resize(w, h);
        back.resize(w, h);
    }

    /// One update, which in this protocol is always a whole picture: there are
    /// no damage rectangles, so the changed area is found here rather than read
    /// from the wire.
    ///
    /// If the session's renderer accepts GPU pictures (the platform's decoders
    /// report this every frame), a picture the decoder can leave on the GPU
    /// stays there and becomes the framebuffer's picture as is, damaging the
    /// whole desktop: without a conversion there is no way to find what moved,
    /// and nothing here reads it. The next picture converted on the CPU is then
    /// converted whole, since its buffer does not hold the previous picture.
    fn picture(
        &self,
        frame: VideoFrame,
        decoder: &mut Option<Current>,
        back: &mut Back,
        handler: &mut dyn Handler,
    ) {
        let Some(union) = frame.union else { return };
        let (codec, frames) = match union {
            video_frame::Union::Vp9s(v) => (Codec::Vp9, v.frames),
            video_frame::Union::Vp8s(v) => (Codec::Vp8, v.frames),
            video_frame::Union::Av1s(v) => (Codec::Av1, v.frames),
            video_frame::Union::H264s(v) => (Codec::H264, v.frames),
            video_frame::Union::H265s(v) => (Codec::H265, v.frames),
            // Raw pixels, which RustDesk's server sends only when it has no
            // encoder; never requested here and never seen in testing.
            video_frame::Union::Rgb(_) | video_frame::Union::Yuv(_) => {
                log::warn!("the peer sent unencoded pixels, which this client does not read");
                return;
            }
        };
        let (w, h) = (
            self.width.load(Ordering::Relaxed).max(0) as usize,
            self.height.load(Ordering::Relaxed).max(0) as usize,
        );
        if w == 0 || h == 0 || back.pixels.len() != w * h {
            return;
        }
        // The size as well as the codec: a decoder is configured for one
        // picture size, and this protocol changes it during a session whenever
        // the far end switches display. Also the decoding mode, reported by the
        // platform's decoders, which can also change during a session: checked
        // every frame, so a change takes effect at the next picture without a
        // message of its own.
        let hardware = self.decoders.hardware();
        let made_for = |c: &Current| c.codec == codec && c.size == (w, h) && c.hardware == hardware;
        if !decoder.as_ref().is_some_and(made_for) {
            let switched = decoder.as_ref().is_some_and(|c| c.hardware != hardware);
            // Dropped first, so that a hardware decoder's surfaces are released
            // before the next one allocates its own.
            *decoder = None;
            match self.decoders.decoder(codec, w, h) {
                Some(new) => {
                    self.info.lock().unwrap().encoding = encoding(codec, None);
                    *decoder = Some(Current {
                        codec,
                        size: (w, h),
                        hardware,
                        shown: None,
                        status_at: None,
                        decoder: new,
                    });
                    // A new decoder mid-stream has none of the pictures the
                    // next frames refer to. After a new codec or size the peer
                    // sends a key frame by itself; a new decoding mode is this
                    // end's change, so the peer is asked for one.
                    if switched {
                        log::info!("decoding switched to hardware {hardware:?}: asking for a key frame");
                        self.refresh_video();
                    }
                }
                None => {
                    self.info.lock().unwrap().encoding = format!("{} (no decoder)", codec.name());
                    *self.decoder_status.lock().unwrap() = None;
                    return;
                }
            }
        }
        let Some(current) = decoder.as_mut() else {
            return;
        };
        back.begin(w, h);
        let gpu = self.decoders.gpu_pictures();
        let mut drew = false;
        let mut held: Option<Arc<dyn GpuPicture>> = None;
        for frame in frames {
            let decoded = if gpu {
                current.decoder.decode_picture(&frame.data, &mut back.pixels, w, h, &mut back.changed)
            } else if current.decoder.decode(&frame.data, &mut back.pixels, w, h, &mut back.changed) {
                Decoded::Converted
            } else {
                Decoded::Nothing
            };
            match decoded {
                Decoded::Gpu(picture) if (picture.width(), picture.height()) == (w, h) => {
                    held = Some(picture);
                    // A picture converted after this one, in this update or
                    // the next, is converted whole.
                    back.changed.all(w, h);
                }
                // Not the session's size: a frame between a resize and the
                // desktop's new size. Read back, as a converted picture of
                // that size would have been written.
                Decoded::Gpu(picture) => {
                    back.changed.all(w, h);
                    if picture.read(&mut back.pixels, w, h) {
                        drew = true;
                        held = None;
                    }
                }
                Decoded::Converted => {
                    drew = true;
                    held = None;
                }
                Decoded::Nothing => {}
            }
        }
        // A decoder that lost the pictures the stream refers back to (a
        // hardware one that failed mid-stream and continued in software)
        // draws nothing correctly until the next key frame, so one is
        // requested rather than waited for.
        if current.decoder.take_key_frame_request() {
            log::info!("the decoder asks for a key frame");
            self.refresh_video();
        }
        self.note(current);
        if let Some(picture) = held {
            // The back buffer no longer holds the picture on screen.
            back.lost();
            if !self.framebuffer.write().unwrap().show_gpu(picture) {
                return;
            }
            handler.damaged(0, 0, w, h);
            handler.frame_end();
            return;
        }
        if !drew {
            return;
        }
        // A frame that repeats the last one (what a peer sends while its
        // desktop is idle) costs nothing from here on, and a frame that moved
        // a caret costs only the rows the caret is on.
        let Some((dx, dy, dw, dh)) = back.changed.bounds(w) else {
            return;
        };
        // Under a read lock and before the swap takes the picture: the rows
        // this frame did not convert still hold the frame before last.
        if !back.carry(&self.framebuffer.read().unwrap()) {
            return;
        }
        // The write lock is held for a `Vec` swap rather than a conversion, so
        // the drawing thread never waits for the decoder.
        if !self.framebuffer.write().unwrap().swap(&mut back.pixels) {
            back.lost();
            return;
        }
        back.swapped();
        handler.damaged(dx, dy, dw, dh);
        handler.frame_end();
    }

    /// Publishes what the decoder reports about itself: the Encoding row
    /// whenever its hardware flag changes (the first picture, a fallback), and
    /// the status line every [`STATUS_EVERY`].
    fn note(&self, current: &mut Current) {
        let hardware = current.decoder.hardware();
        if hardware != current.shown {
            current.shown = hardware;
            self.info.lock().unwrap().encoding = encoding(current.codec, hardware);
        }
        let now = Instant::now();
        if current.status_at.is_none_or(|at| now.duration_since(at) >= STATUS_EVERY) {
            current.status_at = Some(now);
            *self.decoder_status.lock().unwrap() = current.decoder.status();
        }
    }

    fn clipboard_arrived(&self, clip: Clipboard, handler: &mut dyn Handler) {
        if clip.format.enum_value_or_default() != ClipboardFormat::Text {
            return;
        }
        let content = if clip.compress {
            unzstd(&clip.content, runtime::MAX_CLIPBOARD).unwrap_or_default()
        } else {
            clip.content
        };
        if content.len() > runtime::MAX_CLIPBOARD {
            // Skipped entirely, as in every backend: a truncated text would
            // look like the whole of it when pasted.
            log::warn!(
                "the peer's clipboard is over the {} bytes this client takes; skipped",
                runtime::MAX_CLIPBOARD
            );
            return;
        }
        if let Ok(text) = String::from_utf8(content) {
            if !text.is_empty() {
                // The length rather than the text: a clipboard may hold a
                // password.
                log::info!("{} characters of clipboard from the peer", text.chars().count());
                handler.clipboard(&text);
            }
        }
    }

    /// The peer's timing message, about once a second.
    fn test_delay(&self, delay: TestDelay) {
        if delay.from_client {
            return;
        }
        // The round trip comes with it: `last_delay` is what the peer
        // measured from the previous exchange, so a client that echoes
        // promptly learns its latency without measuring it.
        if delay.last_delay > 0 {
            self.round_trip.store(delay.last_delay as u64, Ordering::Relaxed);
        }
        // Echoed **unchanged**. Setting the flag here would turn one message
        // a second into a busy loop between the two ends.
        let mut out = Message::new();
        out.set_test_delay(delay);
        self.send(out);
    }

    /// Asks for the password, again for an empty answer, which the peer
    /// would count as a wrong password (RustDesk's client sends the hash of
    /// the empty string for one). The keep-alive runs meanwhile: the peer
    /// drops a connection quiet for thirty seconds, and a prompt can stay
    /// open longer.
    ///
    /// **The connection is read while the prompt is open**, as RustDesk's
    /// client does, since the far end's user can answer first: their accept
    /// (the peer's `PeerInfo`) makes the password pointless, and so does "No
    /// Password Access" from a peer that takes none. Either withdraws the
    /// prompt, as does anything that ends the session. What is read is kept
    /// in `early`, in order, for the session's loop; the timing messages are
    /// answered at once.
    fn ask_password(
        &self,
        reader: &mut wire::Reader,
        handler: &mut dyn Handler,
        early: &mut VecDeque<std::io::Result<Message>>,
    ) -> Result<Prompted> {
        let alive = self.keep_alive();
        let withdraw = runtime::Withdraw::new();
        let answered = AtomicBool::new(false);
        let answer = std::thread::scope(|scope| {
            let listening = std::thread::Builder::new()
                .name("rustdesk-prompt".into())
                .spawn_scoped(scope, || self.read_during_prompt(reader, &withdraw, &answered));
            let answer = loop {
                match handler.credentials_or_withdrawn(false, &withdraw) {
                    Asked::Answered(given) if given.password.is_empty() => continue,
                    Asked::Answered(given) => break Ok(Prompted::Password(given.password)),
                    Asked::Cancelled => break Err(Error::Cancelled),
                    Asked::Withdrawn => {
                        log::info!("the password prompt is withdrawn: the peer answered first");
                        break Ok(Prompted::Withdrawn);
                    }
                }
            };
            answered.store(true, Ordering::Release);
            match listening {
                Ok(listening) => early.extend(listening.join().unwrap_or_default()),
                Err(e) => log::warn!("not reading the connection during the prompt: {e}"),
            }
            answer
        });
        alive.stop();
        reader.set_timeout(Some(IDLE_TIMEOUT))?;
        answer
    }

    /// The reader's side of [`ask_password`](Self::ask_password): until the
    /// prompt is answered, or the peer sends something that answers it.
    /// A message is read only once it has begun to arrive, so that stopping
    /// never leaves part of one read and the rest for the session's loop.
    fn read_during_prompt(
        &self,
        reader: &mut wire::Reader,
        withdraw: &runtime::Withdraw,
        answered: &AtomicBool,
    ) -> Vec<std::io::Result<Message>> {
        let mut read = Vec::new();
        while !answered.load(Ordering::Acquire) {
            match reader.arriving(PROMPT_POLL) {
                Ok(false) => continue,
                Ok(true) => {}
                Err(e) => {
                    read.push(Err(e));
                    withdraw.fire();
                    break;
                }
            }
            let message = match reader.set_timeout(Some(IDLE_TIMEOUT)).and_then(|()| reader.message::<Message>()) {
                Ok(message) => message,
                Err(e) => {
                    read.push(Err(e));
                    withdraw.fire();
                    break;
                }
            };
            let ends_the_prompt = match &message.union {
                Some(message::Union::TestDelay(delay)) => {
                    self.test_delay(delay.clone());
                    continue;
                }
                Some(message::Union::LoginResponse(response)) => match &response.union {
                    Some(login_response::Union::PeerInfo(_)) => true,
                    Some(login_response::Union::Error(why)) => !matches!(login_error(why), LoginError::Password),
                    None => false,
                },
                Some(message::Union::Misc(misc)) => matches!(misc.union, Some(misc::Union::CloseReason(_))),
                _ => false,
            };
            read.push(Ok(message));
            if ends_the_prompt {
                withdraw.fire();
                break;
            }
        }
        read
    }

    fn send_login(&self, config: &Config, hash: &Hash, password: &str) {
        let mut login = LoginRequest::new();
        // **The address, not a name.** The peer refuses a login whose username
        // is neither its own id nor something that parses as an address, and
        // the refusal it sends is `Offline`, which suggests the machine is
        // unreachable rather than that a field is wrong. So this is the
        // socket's address rather than the typed one: the two differ whenever
        // a default port was filled in.
        login.username = self.login_as.lock().unwrap().clone();
        login.password = login_hash(password, hash).into();
        // The peer shows and logs this, so it names the device rather than
        // this app: the user at the far end wants to know which device is
        // connected to their desktop.
        login.my_id = config.my_name.clone();
        login.my_name = config.my_name.clone();
        login.my_platform = config.platform.wire().into();
        login.version = VERSION.into();
        login.session_id = 1;
        // **The far end paces itself to this client.** Without this it
        // captures and sends on its own clock, and a phone that takes longer to
        // handle a frame than the peer takes to produce one falls further
        // behind with each frame: a drag lags a second behind and gets worse
        // the longer it goes on. With it, the peer captures the next frame
        // when this end acknowledges the last.
        login.video_ack_required = true;
        let mut option = self.options(true);
        // The only option here not about the picture, and its effect is never
        // visible to the session, since it happens after the last message
        // either end sends. Left unset rather than set to No, so a peer
        // configured to lock itself still does.
        if config.lock_after {
            option.lock_after_session_end = BoolOption::Yes.into();
        }
        login.option = MessageField::some(option);
        let mut out = Message::new();
        out.set_login_request(login);
        self.send(out);
    }

    /// The session's options, sent at login and again whenever one changes.
    ///
    /// The codec preference is included at login and afterwards **only when it
    /// has changed**, so that a quality change does not cause a codec
    /// renegotiation: the peer answers a preference by building a new encoder
    /// and restarting with a key frame, so repeating the current one would
    /// make every frame rate change cost a full key frame.
    fn options(&self, at_login: bool) -> OptionMessage {
        let mut option = OptionMessage::new();
        option.image_quality = Quality::of(self.quality.load(Ordering::Relaxed)).wire().into();
        let fps = self.fps.load(Ordering::Relaxed);
        option.custom_fps = if self.focused.load(Ordering::Relaxed) {
            fps
        } else {
            UNFOCUSED_FPS
        };
        // The peer's sound only if this session plays it: otherwise a desktop
        // playing sound sends a stream of Opus the session would receive and
        // drop. There is no clipboard option because clipboard sharing is a
        // device setting, set once for every protocol in the app's settings.
        option.disable_audio = if self.sound.load(Ordering::Relaxed) {
            BoolOption::No
        } else {
            BoolOption::Yes
        }
        .into();
        let codec = self.codec.load(Ordering::Relaxed);
        let moved = self.codec_sent.swap(codec, Ordering::Relaxed) != codec;
        if at_login || moved {
            option.supported_decoding = MessageField::some(self.decoding(PreferredCodec::of(codec)));
        }
        option
    }

    /// What this end can decode, and which codec it prefers.
    ///
    /// The abilities are the **platform's**, queried from its [`Decoders`]
    /// rather than hardcoded: the peer sends whatever a client claims to
    /// decode, and a false claim leaves the session with a cursor and no
    /// picture. VP9 is the codec every RustDesk build can encode, so a peer
    /// falls back to it when nothing else matches, including for an empty
    /// ability list.
    fn decoding(&self, prefer: PreferredCodec) -> SupportedDecoding {
        let mut decoding = SupportedDecoding::new();
        decoding.ability_vp9 = self.decoders.decodable(Codec::Vp9) as i32;
        decoding.ability_vp8 = self.decoders.decodable(Codec::Vp8) as i32;
        decoding.ability_av1 = self.decoders.decodable(Codec::Av1) as i32;
        decoding.ability_h264 = self.decoders.decodable(Codec::H264) as i32;
        decoding.ability_h265 = self.decoders.decodable(Codec::H265) as i32;
        // A preference this end cannot decode is not passed on: the far end
        // would honour it, with no way of telling that the picture never
        // arrives.
        let prefer = match prefer.codec() {
            Some(codec) if !self.decoders.decodable(codec) => {
                log::warn!("this end cannot decode {}, leaving the codec to the peer", codec.name());
                PreferredCodec::Auto
            }
            _ => prefer,
        };
        log::info!("codec preference {:?}", prefer);
        decoding.prefer = prefer.wire().into();
        decoding
    }

    /// Offers the session's sound switch as an Enable switch, initially in the
    /// state the options request. Toggling it sends the peer an option message
    /// with only `disable_audio` set (the peer leaves unset options
    /// unchanged), and the peer subscribes the session to its sound or
    /// unsubscribes it. Sent from the switch's thread through the writer.
    fn offer_sound(&self, audio: &Arc<dyn Outputs>) {
        let sound = Arc::clone(&self.sound);
        let out = self.out.lock().unwrap().clone();
        audio.controls().offer_enable(
            self.sound.load(Ordering::Relaxed),
            Box::new(move |on| {
                sound.store(on, Ordering::Relaxed);
                let mut option = OptionMessage::new();
                option.disable_audio = if on { BoolOption::No } else { BoolOption::Yes }.into();
                let mut misc = Misc::new();
                misc.set_option(option);
                let mut message = Message::new();
                message.set_misc(misc);
                if let (Some(out), Ok(bytes)) = (&out, message.write_to_bytes()) {
                    let _ = out.send(bytes);
                }
            }),
        );
    }

    /// A live option change: one message, which takes effect on the current
    /// picture.
    pub fn set_options(&self, quality: Quality, fps: i32, codec: PreferredCodec) {
        self.quality.store(quality.ordinal(), Ordering::Relaxed);
        self.fps.store(fps, Ordering::Relaxed);
        self.codec.store(codec.ordinal(), Ordering::Relaxed);
        self.send_options();
    }

    fn send_options(&self) {
        let mut misc = Misc::new();
        misc.set_option(self.options(false));
        let mut out = Message::new();
        out.set_misc(misc);
        self.send(out);
    }

    // ---- input ------------------------------------------------------------

    pub fn set_view_only(&self, view_only: bool) {
        self.view_only.store(view_only, Ordering::Relaxed);
        if view_only {
            self.release_all_keys();
        }
    }

    /// Whether the session has focus. There is no pause message here, so this
    /// asks for one frame a second when unfocused, and for a fresh picture when
    /// focused again.
    pub fn set_focused(&self, focused: bool) {
        if self.focused.swap(focused, Ordering::Relaxed) == focused {
            return;
        }
        self.send_options();
        if focused {
            self.refresh_video();
        }
    }

    /// Absolute desktop coordinates and an RFB button mask.
    ///
    /// The session API gives the pointer position and the buttons held; this
    /// protocol takes a move, a press and a release as separate events, so the
    /// changes are worked out here against the last mask. The coordinates are
    /// the picture's and are sent as the peer's desktop's, which differ unless
    /// the captured display is at the origin.
    pub fn pointer(&self, x: i32, y: i32, mask: i32) {
        if self.view_only.load(Ordering::Relaxed) {
            return;
        }
        let (ox, oy) = *self.origin.lock().unwrap();
        let (moved, was) = {
            let mut last = self.pointer.lock().unwrap();
            let moved = last.0 != x || last.1 != y;
            let was = last.2;
            *last = (x, y, mask);
            (moved, was)
        };
        if moved {
            self.mouse(mouse::TYPE_MOVE, x + ox, y + oy, false);
        }
        for (ours, theirs) in BUTTONS {
            let down = mask & ours != 0;
            if down != (was & ours != 0) {
                let kind = if down { mouse::TYPE_DOWN } else { mouse::TYPE_UP };
                self.mouse(kind | (theirs << 3), x + ox, y + oy, down);
            }
        }
        // The wheel is four bits of the same mask, and each is a notch rather
        // than a held button: the press is the event and the release is
        // ignored. Its two numbers are a distance rather than a position, so
        // the display's origin is not added to them.
        let pressed = mask & !was;
        let (mut dx, mut dy) = (0, 0);
        if pressed & 0x08 != 0 {
            dy += WHEEL_NOTCH;
        }
        if pressed & 0x10 != 0 {
            dy -= WHEEL_NOTCH;
        }
        if pressed & 0x20 != 0 {
            dx += WHEEL_NOTCH;
        }
        if pressed & 0x40 != 0 {
            dx -= WHEEL_NOTCH;
        }
        if dx != 0 || dy != 0 {
            self.mouse(mouse::TYPE_WHEEL, dx, dy, false);
        }
    }

    /// One mouse event. A press carries the modifiers held, because RustDesk's
    /// server releases every modifier a down event does not name.
    fn mouse(&self, mask: i32, x: i32, y: i32, with_modifiers: bool) {
        let mut event = MouseEvent::new();
        event.mask = mask;
        event.x = x;
        event.y = y;
        if with_modifiers {
            event.modifiers = self.held_modifiers(None);
        }
        let mut out = Message::new();
        out.set_mouse_event(event);
        self.send(out);
    }

    pub fn key_down(&self, keysym: u32, key_id: i64) {
        if self.view_only.load(Ordering::Relaxed) {
            return;
        }
        let Some(key) = keymap::key(keysym) else {
            // At trace level, since a keysym reveals what was typed.
            log::trace!("nothing in this protocol says keysym {keysym:#x}");
            return;
        };
        if let Some(modifier) = keymap::modifier(keysym) {
            let mut held = self.modifiers.lock().unwrap();
            if !held.contains(&modifier) {
                held.push(modifier);
            }
        }
        self.keys.lock().unwrap().insert(key_id, key);
        self.key(key, true);
    }

    pub fn key_up(&self, key_id: i64) {
        let Some(key) = self.keys.lock().unwrap().remove(&key_id) else {
            return;
        };
        if let Key::Control(control) = key {
            self.modifiers.lock().unwrap().retain(|held| *held != control);
        }
        self.key(key, false);
    }

    /// Releases every key this session still holds down at the far end.
    pub fn release_all_keys(&self) {
        let held: Vec<Key> = {
            let mut keys = self.keys.lock().unwrap();
            let held = keys.values().copied().collect();
            keys.clear();
            held
        };
        self.modifiers.lock().unwrap().clear();
        for key in held {
            self.key(key, false);
        }
    }

    fn key(&self, key: Key, down: bool) {
        let mut event = KeyEvent::new();
        event.down = down;
        event.mode = KeyboardMode::Legacy.into();
        match key {
            Key::Control(control) => {
                event.set_control_key(control);
                if down {
                    event.modifiers = self.held_modifiers(Some(control));
                }
            }
            Key::Char(c) => {
                // `chr` rather than `unicode`: RustDesk's server can hold a
                // `chr` down, so it acts as a key rather than typed text.
                event.set_chr(c as u32);
                if down {
                    event.modifiers = self.held_modifiers(None);
                }
            }
        }
        let mut out = Message::new();
        out.set_key_event(event);
        self.send(out);
    }

    /// The modifiers held, minus the one being pressed: RustDesk's server
    /// compares the list against what is down on the far end and releases the
    /// difference, and it does not press a modifier that lists itself.
    fn held_modifiers(&self, itself: Option<ControlKey>) -> Vec<protobuf::EnumOrUnknown<ControlKey>> {
        let its = itself.and_then(|c| keymap::modifier_of(c));
        self.modifiers
            .lock()
            .unwrap()
            .iter()
            .filter(|held| Some(**held) != its)
            .map(|held| protobuf::EnumOrUnknown::new(*held))
            .collect()
    }

    pub fn clipboard(&self, text: &str) {
        if self.view_only.load(Ordering::Relaxed) {
            return;
        }
        log::info!("{} characters of clipboard to the peer", text.chars().count());
        let mut clip = Clipboard::new();
        clip.content = text.as_bytes().to_vec().into();
        clip.format = ClipboardFormat::Text.into();
        let mut out = Message::new();
        out.set_clipboard(clip);
        self.send(out);
    }

    /// Asks the far end for the offered size nearest to this one.
    ///
    /// Unlike RFB there is no free choice: the peer publishes a list and
    /// accepts only entries from it, so a window's size is mapped to the
    /// nearest entry by area.
    pub fn request_desktop_size(&self, width: i32, height: i32) {
        let offered = self.resolutions.lock().unwrap().clone();
        let wanted = (width as i64) * (height as i64);
        let Some(&(w, h)) = offered.iter().min_by_key(|(ow, oh)| {
            ((*ow as i64) * (*oh as i64) - wanted).abs()
        }) else {
            return;
        };
        log::info!("asked for {width}x{height}, which is {w}x{h} of the sizes offered");
        let mut resolution = Resolution::new();
        resolution.width = w;
        resolution.height = h;
        let mut change = DisplayResolution::new();
        change.display = self.display.load(Ordering::Relaxed);
        change.resolution = MessageField::some(resolution);
        let mut misc = Misc::new();
        misc.set_change_display_resolution(change);
        let mut out = Message::new();
        out.set_misc(misc);
        self.send(out);
    }

    /// Sends a harmless message every few seconds until stopped.
    ///
    /// A peer that receives nothing for **30 seconds** before the login drops
    /// the connection, and a prompt can stay open longer (in testing the
    /// session was lost at forty seconds, with a dialog still on screen). The
    /// timing message is the only message a client may send before logging in
    /// that the far end accepts, so it serves as the keep-alive.
    fn keep_alive(&self) -> KeepAlive {
        let mut delay = TestDelay::new();
        delay.from_client = true;
        let mut message = Message::new();
        message.set_test_delay(delay);
        let bytes = message.write_to_bytes().unwrap_or_default();
        let out = self.out.lock().unwrap().clone();
        let (stop, stopped) = channel::<()>();
        let thread = std::thread::Builder::new()
            .name("rustdesk-keepalive".into())
            .spawn(move || {
                let Some(out) = out else { return };
                while let Err(RecvTimeoutError::Timeout) = stopped.recv_timeout(KEEP_ALIVE_EVERY) {
                    if out.send(bytes.clone()).is_err() {
                        break;
                    }
                }
            })
            .ok();
        KeepAlive { stop, thread }
    }

    /// Queues a message for the writer thread. Dropped if the session has
    /// ended, which happens to input events between the socket closing and
    /// the screen being notified.
    fn send(&self, message: Message) {
        let bytes = match message.write_to_bytes() {
            Ok(bytes) => bytes,
            Err(e) => {
                log::warn!("encoding a message: {e}");
                return;
            }
        };
        if let Some(out) = self.out.lock().unwrap().as_ref() {
            let _ = out.send(bytes);
        }
    }
}

/// The interface `runtime::Runner` runs: the inherent `run` and `close` are
/// the implementation, exposed here for code that does not know which
/// protocol it has.
impl runtime::Client for Client {
    type Config = Config;
    type Error = Error;
    const NAME: &'static str = "rustdesk";

    fn run(&self, config: &Config, handler: &mut dyn Handler) -> Result<()> {
        self.run(config, handler)
    }

    fn close(&self) {
        self.close();
    }

    fn closed_on_request(error: &Error) -> bool {
        matches!(error, Error::Closed)
    }
}

/// What a password prompt came to: a password to log in with, or nothing to
/// send, since the peer answered first.
enum Prompted {
    Password(String),
    Withdrawn,
}

/// The thread behind [`Client::keep_alive`]. Stopping wakes it from its
/// wait and joins it, so that it lets go of its sender to the writer thread
/// at once: a session that ends while it sleeps waits for the writer, and the
/// writer waits for every sender.
struct KeepAlive {
    stop: Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl KeepAlive {
    fn stop(self) {
        drop(self.stop);
        if let Some(thread) = self.thread {
            let _ = thread.join();
        }
    }
}

/// The session's decoding cost, logged once every [`Rate::EVERY`] frames.
///
/// This protocol has no damage rectangles, so every frame arrives whole, and
/// a session's cost depends on how much of each frame needs converting. That
/// and the time spent are the useful measurements from a running session,
/// logged as one line per [`Rate::EVERY`] frames at negligible cost per frame.
struct Rate {
    frames: u64,
    since: Instant,
    working: Duration,
    rows: u64,
}

impl Rate {
    const EVERY: u64 = 300;

    fn new() -> Rate {
        Rate {
            frames: 0,
            since: Instant::now(),
            working: Duration::ZERO,
            rows: 0,
        }
    }

    fn frame(&mut self, took: Duration, rows: usize) {
        self.frames += 1;
        self.working += took;
        self.rows += rows as u64;
        if self.frames < Rate::EVERY {
            return;
        }
        let elapsed = self.since.elapsed().as_secs_f64().max(0.001);
        log::info!(
            "{} frames in {:.1}s — {:.1} a second, {:.1} ms of decode and conversion each, {:.0} rows converted each, {:.0}% of this thread",
            self.frames,
            elapsed,
            self.frames as f64 / elapsed,
            self.working.as_secs_f64() * 1000.0 / self.frames as f64,
            self.rows as f64 / self.frames as f64,
            self.working.as_secs_f64() * 100.0 / elapsed,
        );
        self.frames = 0;
        self.working = Duration::ZERO;
        self.rows = 0;
        self.since = Instant::now();
    }
}

/// The cursor shapes the peer has sent, kept once per shape.
///
/// The peer sends a shape's pixels once with an id, and after that only the
/// id. From RustDesk 1.5.0 the id is a hash of the shape. Before that it is
/// the platform's handle for the cursor, and some applications (Chrome and
/// Electron on Windows) create a new handle each time they show a shape, so
/// the same pixels arrive again under each new id. Shapes are filed here by
/// their content, as RustDesk 1.5.0's client does for an older peer, so that
/// each is decoded and kept once; for a peer from 1.5.0 this changes nothing.
#[derive(Default)]
struct Cursors {
    /// The peer's ids, to the keys of `shapes`.
    ids: HashMap<u64, u64>,
    shapes: HashMap<u64, Cursor>,
}

impl Cursors {
    /// The shape a `CursorData` carries, decoded unless it is already kept.
    fn arrived(&mut self, data: crate::protos::message::CursorData) -> Option<&Cursor> {
        let id = data.id;
        let key = Cursor::content(&data);
        if self.shapes.contains_key(&key) {
            log::debug!("cursor {id}: a shape already kept");
        } else {
            let Some(cursor) = Cursor::of(data) else {
                // The id no longer names the shape it named before, if any.
                self.ids.remove(&id);
                return None;
            };
            log::debug!(
                "cursor {id}: {}x{}, hotspot {},{}",
                cursor.width,
                cursor.height,
                cursor.hot_x,
                cursor.hot_y
            );
            self.shapes.insert(key, cursor);
        }
        self.ids.insert(id, key);
        self.shapes.get(&key)
    }

    /// The shape the peer sent under this id, for a `CursorId`.
    fn get(&self, id: u64) -> Option<&Cursor> {
        self.shapes.get(self.ids.get(&id)?)
    }
}

/// A cursor shape, kept because the protocol sends the pixels once and refers
/// to them by id after that.
struct Cursor {
    pixels: Vec<u32>,
    width: usize,
    height: usize,
    hot_x: i32,
    hot_y: i32,
}

impl Cursor {
    fn of(data: crate::protos::message::CursorData) -> Option<Cursor> {
        let (width, height) = (data.width.max(0) as usize, data.height.max(0) as usize);
        if width == 0 || height == 0 {
            return None;
        }
        // Two 32-bit fields from the peer, so checked against the shared bound
        // before sizing anything; within it the product cannot overflow.
        if width > runtime::MAX_CURSOR_SIDE || height > runtime::MAX_CURSOR_SIDE {
            log::warn!("a {width}x{height} cursor is bigger than any cursor");
            return None;
        }
        let bytes = width * height * 4;
        // Cursors are zstd-compressed, like the clipboard, and decoded only as
        // far as the size needs: anything past it is not drawn.
        let rgba = unzstd(&data.colors, bytes).unwrap_or_default();
        if rgba.len() < bytes {
            log::warn!("a {width}x{height} cursor arrived as {} bytes", rgba.len());
            return None;
        }
        let pixels = rgba
            .chunks_exact(4)
            .take(width * height)
            .map(|p| u32::from_le_bytes([p[0], p[1], p[2], p[3]]))
            .collect();
        Some(Cursor {
            pixels,
            width,
            height,
            hot_x: data.hotx,
            hot_y: data.hoty,
        })
    }

    /// A key for the shape in a `CursorData`, from its size, hotspot and
    /// still-compressed pixels: a peer compresses the same pixels to the same
    /// bytes, so a shape sent again is recognised without decoding it.
    fn content(data: &crate::protos::message::CursorData) -> u64 {
        use std::hash::{DefaultHasher, Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        (data.width, data.height, data.hotx, data.hoty, &data.colors).hash(&mut hasher);
        hasher.finish()
    }

    fn deliver(&self, handler: &mut dyn Handler) {
        handler.cursor(&self.pixels, self.width, self.height, self.hot_x, self.hot_y);
    }
}

/// Decodes a zstd message from the peer up to one byte past `limit`, so a
/// caller can tell a message that fits from one that does not without decoding
/// the rest: the decoded size is controlled by the peer, and a few hundred
/// kilobytes of RLE blocks can decode to gigabytes. `None` if it does not
/// decode.
fn unzstd(data: &[u8], limit: usize) -> Option<Vec<u8>> {
    use std::io::Read;
    let decoder = zstd::stream::read::Decoder::with_buffer(data).ok()?;
    let mut out = Vec::new();
    decoder.take(limit as u64 + 1).read_to_end(&mut out).ok()?;
    Some(out)
}

/// What a `LoginResponse` error asks of this end.
enum LoginError {
    /// A password, or another one: prompt and log in again.
    Password,
    /// Nothing: the peer's user is being asked to accept the session.
    Wait,
    /// The session is over, for the reason given.
    Fatal,
}

/// The peer's login errors by their text, which is all the protocol has for
/// them (`LOGIN_MSG_*` in RustDesk's `client.rs`). "Empty Password" is in
/// that list but sent by no released server; it is taken as asking for a
/// password, as RustDesk's client takes it.
fn login_error(why: &str) -> LoginError {
    match why {
        "Wrong Password" | "Empty Password" => LoginError::Password,
        "No Password Access" => LoginError::Wait,
        _ => LoginError::Fatal,
    }
}

/// `sha256(sha256(password ++ salt) ++ challenge)`. RustDesk's client stores
/// the inner hash, so its saved passwords are salted per peer. This app does
/// not, since it has one password store for every backend.
fn login_hash(password: &str, hash: &Hash) -> Vec<u8> {
    if password.is_empty() {
        return Vec::new();
    }
    let stored = Sha256::new()
        .chain_update(password.as_bytes())
        .chain_update(hash.salt.as_bytes())
        .finalize();
    Sha256::new()
        .chain_update(stored)
        .chain_update(hash.challenge.as_bytes())
        .finalize()
        .to_vec()
}

/// `host` or `host:port`, plus the bracketed IPv6 form. Direct mode only; an
/// id never reaches here.
fn resolve(address: &str) -> Result<SocketAddr> {
    let (host, port) = runtime::address::split(address, DIRECT_PORT, runtime::address::Ports::Plain)
        .map_err(Error::Address)?;
    (host.as_str(), port)
        .to_socket_addrs()
        .map_err(Error::Io)?
        .next()
        .ok_or_else(|| Error::Protocol(format!("{address} resolves to nothing")))
}

fn timed_out(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    )
}

fn arm(union: &message::Union) -> &'static str {
    match union {
        message::Union::SignedId(_) => "SignedId",
        message::Union::PublicKey(_) => "PublicKey",
        message::Union::TestDelay(_) => "TestDelay",
        message::Union::VideoFrame(_) => "VideoFrame",
        message::Union::LoginResponse(_) => "LoginResponse",
        message::Union::Hash(_) => "Hash",
        message::Union::CursorData(_) => "CursorData",
        message::Union::CursorId(_) => "CursorId",
        message::Union::CursorPosition(_) => "CursorPosition",
        message::Union::Clipboard(_) => "Clipboard",
        message::Union::Misc(_) => "Misc",
        message::Union::MessageBox(_) => "MessageBox",
        _ => "something else",
    }
}

/// Withdraws the session's sound switch when the session ends, whichever
/// way it ends.
struct Withdraw(Option<Arc<dyn Outputs>>);

impl Drop for Withdraw {
    fn drop(&mut self) {
        if let Some(audio) = &self.0 {
            audio.controls().withdraw();
        }
    }
}

/// The session's sound output, if the config has one and its platform decodes
/// Opus: the peer encodes sound only in Opus, so an output without that
/// decoder plays nothing.
fn sound_of(config: &Config) -> Option<Arc<dyn Outputs>> {
    config
        .audio
        .clone()
        .filter(|audio| audio.decoders().is_some_and(|d| d.decodable(SoundCodec::Opus)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RustDesk's two-round password hash: the digest python's hashlib gives
    /// for the same inputs, dependent on the challenge, and empty for an empty
    /// password.
    #[test]
    fn the_password_is_two_rounds() {
        let mut hash = Hash::new();
        hash.salt = "0123456789abcdef0123456789abcdef".into();
        hash.challenge = "abcdef".into();
        let digest = login_hash("PASSword1", &hash);
        assert_eq!(
            digest.iter().map(|b| format!("{b:02x}")).collect::<String>(),
            "68901195eae24ddb9a814a5b22a67d36aab028d74a50e9155bc34aae5028952d"
        );
        // The inner hash is what a client would store, and the outer one is
        // sent, so the same password with a second challenge gives a
        // different digest.
        let mut second = hash.clone();
        second.challenge = "fedcba".into();
        assert_ne!(digest, login_hash("PASSword1", &second));
        assert!(login_hash("", &hash).is_empty());
    }

    /// Answers each password prompt from a list, cancelling once it runs out,
    /// and counts the prompts. `said` is each prompt and status, in order.
    struct Answers {
        given: Vec<&'static str>,
        asked: usize,
        said: Vec<String>,
    }
    impl Handler for Answers {
        fn connected(&mut self, _width: usize, _height: usize) {}
        fn desktop_size(&mut self, _width: usize, _height: usize) {}
        fn damaged(&mut self, _x: usize, _y: usize, _width: usize, _height: usize) {}
        fn frame_end(&mut self) {}
        fn cursor(&mut self, _p: &[u32], _w: usize, _h: usize, _hx: i32, _hy: i32) {}
        fn clipboard(&mut self, _text: &str) {}
        fn status(&mut self, text: Option<&str>) {
            self.said.push(format!("status {text:?}"));
        }
        /// Answers whether or not the peer has said something meanwhile,
        /// since what the peer sends here is in the socket before the
        /// session starts, and how much of it is read while the prompt is
        /// open is up to the threads.
        fn credentials_or_withdrawn(
            &mut self,
            needs_user: bool,
            _withdraw: &runtime::Withdraw,
        ) -> Asked<runtime::Credentials> {
            match self.credentials(needs_user) {
                Some(given) => Asked::Answered(given),
                None => Asked::Cancelled,
            }
        }
        fn credentials(&mut self, needs_user: bool) -> Option<runtime::Credentials> {
            assert!(!needs_user);
            self.asked += 1;
            self.said.push("password?".into());
            let password = self.given.get(self.asked - 1)?.to_string();
            Some(runtime::Credentials {
                password,
                ..Default::default()
            })
        }
        fn trust(&mut self, _fingerprint: &str) -> bool {
            false
        }
    }

    /// Runs a session's login against a peer that sends its `Hash` and then
    /// `said`, all of it before the session starts (the socket holds it), and
    /// returns how the session ended, the password digest of each login
    /// sent, and how many times the handler was asked for a password, with
    /// the prompts and statuses in order.
    fn log_in(
        password: Option<&str>,
        said: &[Message],
        answers: &[&'static str],
    ) -> (Result<()>, Vec<Vec<u8>>, usize, Vec<String>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let near = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (far, _) = listener.accept().unwrap();
        let (_, mut peer, _) = wire::split(far, Arc::default()).unwrap();
        let mut hash = Message::new();
        hash.set_hash(challenge());
        peer.send(&hash).unwrap();
        for message in said {
            peer.send(message).unwrap();
        }

        let (mut reader, _, _) = wire::split(near, Arc::default()).unwrap();
        let client = Client::new(Arc::new(video::NoDecoders));
        let (out, sent) = channel::<Vec<u8>>();
        *client.out.lock().unwrap() = Some(out);
        let config = Config {
            address: String::new(),
            reach: Reach::Direct,
            password: password.map(String::from),
            connect_timeout: Duration::from_secs(1),
            my_name: String::new(),
            platform: Platform::Linux,
            quality: Quality::Balanced,
            fps: 0,
            codec: PreferredCodec::Auto,
            lock_after: false,
            audio: None,
            sound: false,
        };
        let mut handler = Answers {
            given: answers.to_vec(),
            asked: 0,
            said: Vec::new(),
        };
        let ended = client.session(&mut reader, &config, None, &mut handler);
        let logins = sent
            .try_iter()
            .filter_map(|bytes| match Message::parse_from_bytes(&bytes).ok()?.union {
                Some(message::Union::LoginRequest(login)) => Some(login.password.to_vec()),
                _ => None,
            })
            .collect();
        (ended, logins, handler.asked, handler.said)
    }

    fn challenge() -> Hash {
        let mut hash = Hash::new();
        hash.salt = "salt".into();
        hash.challenge = "challenge".into();
        hash
    }

    fn login_error_message(why: &str) -> Message {
        let mut response = crate::protos::message::LoginResponse::new();
        response.set_error(why.into());
        let mut message = Message::new();
        message.set_login_response(response);
        message
    }

    fn close_reason(why: &str) -> Message {
        let mut misc = Misc::new();
        misc.set_close_reason(why.into());
        let mut message = Message::new();
        message.set_misc(misc);
        message
    }

    /// With no password, the peer answers an empty login with nothing (its
    /// user is shown an accept dialog), so the password is asked for at
    /// once rather than waited for, and sent in a second login on the same
    /// connection. An empty answer is asked again rather than sent.
    #[test]
    fn without_a_password_one_is_asked_for_after_the_empty_login() {
        let (ended, logins, asked, _) = log_in(None, &[close_reason("done")], &["", "secret"]);
        assert!(matches!(ended, Err(Error::Refused(why)) if why == "done"));
        assert_eq!(logins, vec![Vec::new(), login_hash("secret", &challenge())]);
        assert_eq!(asked, 2);

        let (ended, logins, asked, _) = log_in(None, &[close_reason("done")], &[]);
        assert!(matches!(ended, Err(Error::Cancelled)));
        assert_eq!(logins, vec![Vec::new()]);
        assert_eq!(asked, 1);
    }

    /// A wrong password is asked for again and sent on the same connection;
    /// "No Password Access" (a peer whose user accepts by clicking) is a
    /// wait rather than an ending; any other login error ends the session
    /// with the peer's words.
    #[test]
    fn the_peers_login_errors_ask_wait_or_end() {
        let said = [
            login_error_message("Wrong Password"),
            login_error_message("No Password Access"),
            close_reason("done"),
        ];
        let (ended, logins, asked, _) = log_in(Some("wrong"), &said, &["right"]);
        assert!(matches!(ended, Err(Error::Refused(why)) if why == "done"));
        assert_eq!(
            logins,
            vec![login_hash("wrong", &challenge()), login_hash("right", &challenge())]
        );
        assert_eq!(asked, 1);

        let (ended, _, asked, _) = log_in(Some("pw"), &[login_error_message("Offline")], &[]);
        assert!(matches!(ended, Err(Error::Refused(why)) if why == "Offline"));
        assert_eq!(asked, 0);
    }

    /// The far end's user is shown an accept dialog for a login without a
    /// password, a wrong one, and any login to a peer that takes none, so
    /// each sets the status: the first before the prompt, which shows it.
    #[test]
    fn a_wait_for_the_far_ends_user_is_the_status() {
        let waiting = format!("status {:?}", Some(WAITING_FOR_ACCEPT));
        let (_, _, _, said) = log_in(None, &[close_reason("done")], &["secret"]);
        assert_eq!(said, [waiting.as_str(), "password?"]);

        let wrong = [login_error_message("Wrong Password"), close_reason("done")];
        let (_, _, _, said) = log_in(Some("wrong"), &wrong, &[]);
        assert_eq!(said, [waiting.as_str(), "password?"]);

        let click = [login_error_message("No Password Access"), close_reason("done")];
        let (_, _, _, said) = log_in(Some("pw"), &click, &[]);
        assert_eq!(said, [waiting.as_str()]);

        // A password taken at once says nothing.
        let (_, _, _, said) = log_in(Some("pw"), &[close_reason("done")], &[]);
        assert!(said.is_empty());
    }

    /// Leaves the password prompt open until the session withdraws it, and
    /// says so; tells `opened` each time a prompt opens.
    struct Unanswered {
        opened: Sender<()>,
        said: Vec<String>,
    }
    impl Handler for Unanswered {
        fn connected(&mut self, width: usize, height: usize) {
            self.said.push(format!("connected {width}x{height}"));
        }
        fn desktop_size(&mut self, _width: usize, _height: usize) {}
        fn damaged(&mut self, _x: usize, _y: usize, _width: usize, _height: usize) {}
        fn frame_end(&mut self) {}
        fn cursor(&mut self, _p: &[u32], _w: usize, _h: usize, _hx: i32, _hy: i32) {}
        fn clipboard(&mut self, _text: &str) {}
        fn status(&mut self, text: Option<&str>) {
            self.said.push(format!("status {text:?}"));
        }
        fn credentials(&mut self, _needs_user: bool) -> Option<runtime::Credentials> {
            unreachable!("asked through credentials_or_withdrawn")
        }
        fn credentials_or_withdrawn(
            &mut self,
            _needs_user: bool,
            withdraw: &runtime::Withdraw,
        ) -> Asked<runtime::Credentials> {
            self.said.push("password?".into());
            let (fired, wait) = channel();
            withdraw.arm(move || {
                let _ = fired.send(());
            });
            let _ = self.opened.send(());
            let withdrawn = wait.recv_timeout(Duration::from_secs(5)).is_ok();
            withdraw.disarm();
            if withdrawn {
                self.said.push("withdrawn".into());
                Asked::Withdrawn
            } else {
                Asked::Cancelled
            }
        }
        fn trust(&mut self, _fingerprint: &str) -> bool {
            false
        }
    }

    /// A session without a password against a peer that sends `then` once
    /// the password prompt is open: how it ended, the password digest of
    /// each login sent, and what the handler was told.
    fn answered_by_the_peer(then: Vec<Message>) -> (Result<()>, Vec<Vec<u8>>, Vec<String>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let near = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (far, _) = listener.accept().unwrap();
        let (_, mut peer, _) = wire::split(far, Arc::default()).unwrap();
        let mut hash = Message::new();
        hash.set_hash(challenge());
        peer.send(&hash).unwrap();
        let (opened, open) = channel();
        let peer = std::thread::spawn(move || {
            open.recv_timeout(Duration::from_secs(5)).expect("a prompt");
            // Timing messages first: answered while the prompt is up, and
            // not the prompt's business.
            let mut delay = TestDelay::new();
            delay.time = 1;
            let mut timing = Message::new();
            timing.set_test_delay(delay);
            peer.send(&timing).unwrap();
            for message in &then {
                peer.send(message).unwrap();
            }
            peer
        });

        let (mut reader, _, _) = wire::split(near, Arc::default()).unwrap();
        let client = Client::new(Arc::new(video::NoDecoders));
        let (out, sent) = channel::<Vec<u8>>();
        *client.out.lock().unwrap() = Some(out);
        let config = Config {
            address: String::new(),
            reach: Reach::Direct,
            password: None,
            connect_timeout: Duration::from_secs(1),
            my_name: String::new(),
            platform: Platform::Linux,
            quality: Quality::Balanced,
            fps: 0,
            codec: PreferredCodec::Auto,
            lock_after: false,
            audio: None,
            sound: false,
        };
        let mut handler = Unanswered {
            opened,
            said: Vec::new(),
        };
        let ended = client.session(&mut reader, &config, None, &mut handler);
        let _peer = peer.join().unwrap();
        let mut echoed = false;
        let logins = sent
            .try_iter()
            .filter_map(|bytes| match Message::parse_from_bytes(&bytes).ok()?.union {
                Some(message::Union::LoginRequest(login)) => Some(login.password.to_vec()),
                Some(message::Union::TestDelay(delay)) if !delay.from_client => {
                    echoed = true;
                    None
                }
                _ => None,
            })
            .collect();
        assert!(echoed, "the peer's timing message is answered during the prompt");
        (ended, logins, handler.said)
    }

    /// The far end's user can answer before the password prompt is: an
    /// accept (`PeerInfo`) starts the session and "No Password Access"
    /// leaves only the wait, and either takes the prompt back with no
    /// second login sent. So does an ending, which comes with its reason.
    #[test]
    fn the_peer_answering_first_withdraws_the_password_prompt() {
        let waiting = format!("status {:?}", Some(WAITING_FOR_ACCEPT));

        let mut display = crate::protos::message::DisplayInfo::new();
        display.width = 640;
        display.height = 480;
        let mut peer = PeerInfo::new();
        peer.displays.push(display);
        let mut response = crate::protos::message::LoginResponse::new();
        response.set_peer_info(peer);
        let mut accepted = Message::new();
        accepted.set_login_response(response);
        let (ended, logins, said) = answered_by_the_peer(vec![accepted, close_reason("done")]);
        assert!(matches!(ended, Err(Error::Refused(why)) if why == "done"));
        assert_eq!(logins, vec![Vec::<u8>::new()]);
        assert_eq!(said, [waiting.as_str(), "password?", "withdrawn", "connected 640x480"]);

        let (ended, logins, said) =
            answered_by_the_peer(vec![login_error_message("No Password Access"), close_reason("done")]);
        assert!(matches!(ended, Err(Error::Refused(why)) if why == "done"));
        assert_eq!(logins, vec![Vec::<u8>::new()]);
        assert_eq!(said, [waiting.as_str(), "password?", "withdrawn", waiting.as_str()]);

        let (ended, _, said) = answered_by_the_peer(vec![close_reason("gone")]);
        assert!(matches!(ended, Err(Error::Refused(why)) if why == "gone"));
        assert_eq!(said, [waiting.as_str(), "password?", "withdrawn"]);
    }

    /// The Security row names the key exchange version picked, which is 0
    /// for a peer before 1.5.0, and the vouching server.
    #[test]
    fn the_security_row_names_the_key_exchange_version() {
        assert_eq!(
            encrypted_security(1, false),
            "xsalsa20-poly1305, key exchange version 1, vouched for by the public network"
        );
        assert_eq!(
            encrypted_security(0, true),
            "xsalsa20-poly1305, key exchange version 0, vouched for by your rendezvous server"
        );
    }

    /// A handler that ignores everything.
    struct Nothing;
    impl Handler for Nothing {
        fn connected(&mut self, _width: usize, _height: usize) {}
        fn desktop_size(&mut self, _width: usize, _height: usize) {}
        fn damaged(&mut self, _x: usize, _y: usize, _width: usize, _height: usize) {}
        fn frame_end(&mut self) {}
        fn cursor(&mut self, _p: &[u32], _w: usize, _h: usize, _hx: i32, _hy: i32) {}
        fn clipboard(&mut self, _text: &str) {}
        fn credentials(&mut self, _needs_user: bool) -> Option<runtime::Credentials> {
            None
        }
        fn trust(&mut self, _fingerprint: &str) -> bool {
            false
        }
    }

    /// The rig's second screen: 1024×768 at 1920,665 on a desktop whose first
    /// display is 1920×1440. A tap in the middle of the picture is a tap on
    /// that screen, and every coordinate on the wire is the desktop's, so a
    /// client that sends the picture's coordinates moves the cursor on the
    /// first display instead.
    #[test]
    fn a_pointer_is_where_the_captured_display_is() {
        let client = Client::new(Arc::new(video::NoDecoders));
        let (out, sent) = channel();
        *client.out.lock().unwrap() = Some(out);
        let events = || -> Vec<MouseEvent> {
            sent.try_iter()
                .filter_map(|bytes| match Message::parse_from_bytes(&bytes).unwrap().union {
                    Some(message::Union::MouseEvent(event)) => Some(event),
                    _ => None,
                })
                .collect()
        };

        // The origin comes from the switch message rather than the login's
        // display list, since the peer sends a switch message whenever a
        // screen moves, requested or not.
        let mut switch = SwitchDisplay::new();
        switch.display = 1;
        switch.x = 1920;
        switch.y = 665;
        switch.width = 1024;
        switch.height = 768;
        client.switched(&switch, &mut Nothing, &mut Back::new());

        client.pointer(512, 384, 0x01);
        let moved_and_pressed = events();
        assert_eq!(moved_and_pressed.len(), 2);
        assert_eq!(moved_and_pressed[0].mask, mouse::TYPE_MOVE);
        assert_eq!((moved_and_pressed[0].x, moved_and_pressed[0].y), (2432, 1049));
        assert_eq!(
            moved_and_pressed[1].mask,
            mouse::TYPE_DOWN | (mouse::LEFT << 3)
        );
        assert_eq!((moved_and_pressed[1].x, moved_and_pressed[1].y), (2432, 1049));

        // The wheel's two numbers are a distance rather than a position.
        client.pointer(512, 384, 0x08);
        let released_and_scrolled = events();
        assert_eq!(released_and_scrolled.len(), 2);
        assert_eq!(
            (released_and_scrolled[0].x, released_and_scrolled[0].y),
            (2432, 1049)
        );
        assert_eq!(released_and_scrolled[1].mask, mouse::TYPE_WHEEL);
        assert_eq!(
            (released_and_scrolled[1].x, released_and_scrolled[1].y),
            (0, WHEEL_NOTCH)
        );
    }

    /// An output that optionally decodes Opus, and plays nothing.
    struct Sounding(Arc<runtime::media::audio::Controls>, bool);

    impl Outputs for Sounding {
        fn controls(&self) -> &Arc<runtime::media::audio::Controls> {
            &self.0
        }

        fn device(&self, _queue: Arc<runtime::media::audio::Queue>) -> std::result::Result<Box<dyn runtime::media::audio::Device>, String> {
            Err("not in a test".into())
        }

        fn decoders(&self) -> Option<&dyn runtime::media::audio::decode::Decoders> {
            self.1.then_some(self as _)
        }
    }

    impl runtime::media::audio::decode::Decoders for Sounding {
        fn decodable(&self, codec: SoundCodec) -> bool {
            codec == SoundCodec::Opus
        }

        fn decoder(
            &self,
            _codec: SoundCodec,
            _format: Format,
            _config: &[u8],
        ) -> std::result::Result<Box<dyn runtime::media::audio::decode::Decoder>, String> {
            Err("not in a test".into())
        }
    }

    #[test]
    fn the_peers_sound_is_asked_for_only_where_its_opus_is_decoded() {
        let config = |audio: Option<Arc<dyn Outputs>>| Config {
            address: String::new(),
            reach: Reach::Direct,
            password: None,
            connect_timeout: Duration::from_secs(1),
            my_name: String::new(),
            platform: Platform::Linux,
            quality: Quality::Balanced,
            fps: 0,
            codec: PreferredCodec::Auto,
            lock_after: false,
            audio,
            sound: true,
        };
        let controls = Arc::new(runtime::media::audio::Controls::new());
        assert!(sound_of(&config(None)).is_none());
        assert!(sound_of(&config(Some(Arc::new(Sounding(Arc::clone(&controls), false))))).is_none());
        assert!(sound_of(&config(Some(Arc::new(Sounding(controls, true))))).is_some());

        let client = Client::new(Arc::new(video::NoDecoders));
        assert_eq!(client.options(true).disable_audio.enum_value(), Ok(BoolOption::Yes));
        client.sound.store(true, Ordering::Relaxed);
        assert_eq!(client.options(false).disable_audio.enum_value(), Ok(BoolOption::No));
    }

    #[test]
    fn the_switch_asks_the_peer_with_the_audio_option_alone() {
        let controls = Arc::new(runtime::media::audio::Controls::new());
        let audio: Arc<dyn Outputs> = Arc::new(Sounding(Arc::clone(&controls), true));
        let client = Client::new(Arc::new(video::NoDecoders));
        let (out, sent) = channel::<Vec<u8>>();
        *client.out.lock().unwrap() = Some(out);
        client.sound.store(true, Ordering::Relaxed);
        client.offer_sound(&audio);
        let sound = controls.sound();
        assert_eq!((sound.switch, sound.enabled), (Some(runtime::media::audio::Switch::Enable), true));

        let asked = |sent: &std::sync::mpsc::Receiver<Vec<u8>>| {
            let message = Message::parse_from_bytes(&sent.try_recv().unwrap()).unwrap();
            let Some(message::Union::Misc(misc)) = message.union else { panic!("not a misc") };
            let Some(misc::Union::Option(option)) = misc.union else { panic!("not an option") };
            // No other options set, so the peer changes nothing else.
            assert_eq!(option.image_quality.enum_value(), Ok(ImageQuality::NotSet));
            assert_eq!(option.custom_fps, 0);
            assert!(option.supported_decoding.is_none());
            option.disable_audio.enum_value().unwrap()
        };
        assert!(controls.set_enabled(false));
        assert_eq!(asked(&sent), BoolOption::Yes);
        assert!(!client.sound.load(Ordering::Relaxed));
        // The next options sent, for example on a quality change, agree.
        assert_eq!(client.options(false).disable_audio.enum_value(), Ok(BoolOption::Yes));
        assert!(controls.set_enabled(true));
        assert_eq!(asked(&sent), BoolOption::No);
        assert!(sent.try_recv().is_err());
    }

    #[test]
    fn a_port_is_theirs_unless_one_is_given() {
        assert_eq!(resolve("127.0.0.1").unwrap().port(), DIRECT_PORT);
        assert_eq!(resolve("127.0.0.1:5900").unwrap().port(), 5900);
        // The bracketed form without a port is the case a `contains(':')`
        // check for "has a port" gets wrong, since every IPv6 literal has a
        // colon.
        assert_eq!(resolve("[::1]").unwrap().port(), DIRECT_PORT);
        assert!(resolve("::1").is_err());
    }

    #[test]
    fn the_encoding_row_says_where_the_pictures_come_from_once_known() {
        assert_eq!(encoding(Codec::Av1, Some(false)), "AV1 (software)");
        assert_eq!(encoding(Codec::H264, Some(true)), "H.264 (hardware)");
        assert_eq!(encoding(Codec::Vp9, None), "VP9");
    }

    /// Decoders whose decoding mode can be switched, as on a desktop, and
    /// which count the decoders they create; each decoder draws one word per
    /// frame, reports hardware if created in hardware mode, and requests a key
    /// frame on demand.
    #[derive(Default)]
    struct Switchable {
        hardware: Mutex<Hardware>,
        made: AtomicI32,
        lose_references: Arc<AtomicBool>,
        /// Whether the window accepts GPU pictures, which each decoder then
        /// returns as a flat-colour picture.
        gpu: AtomicBool,
    }

    struct Drawing {
        hardware: bool,
        lose_references: Arc<AtomicBool>,
    }

    impl video::Decoder for Drawing {
        fn decode(
            &mut self,
            _data: &[u8],
            out: &mut [u32],
            _width: usize,
            _height: usize,
            changed: &mut video::Changes,
        ) -> bool {
            out[0] = out[0].wrapping_add(1);
            changed.mark(0, 0, 1);
            true
        }

        fn hardware(&self) -> Option<bool> {
            Some(self.hardware)
        }

        fn status(&self) -> Option<String> {
            Some(if self.hardware { "H.264 vaapi" } else { "H.264 sw" }.into())
        }

        fn take_key_frame_request(&mut self) -> bool {
            self.lose_references.swap(false, Ordering::Relaxed)
        }

        fn decode_picture(
            &mut self,
            _data: &[u8],
            _out: &mut [u32],
            width: usize,
            height: usize,
            _changed: &mut video::Changes,
        ) -> Decoded {
            Decoded::Gpu(crate::framebuffer::tests::flat(width, height, 9))
        }
    }

    impl Decoders for Switchable {
        fn decodable(&self, _codec: Codec) -> bool {
            true
        }

        fn decoder(&self, _codec: Codec, _width: usize, _height: usize) -> Option<Box<dyn video::Decoder>> {
            self.made.fetch_add(1, Ordering::Relaxed);
            Some(Box::new(Drawing {
                hardware: *self.hardware.lock().unwrap() != Hardware::Off,
                lose_references: Arc::clone(&self.lose_references),
            }))
        }

        fn hardware(&self) -> Hardware {
            *self.hardware.lock().unwrap()
        }

        fn gpu_pictures(&self) -> bool {
            self.gpu.load(Ordering::Relaxed)
        }
    }

    /// Enabling hardware decoding during a session creates a new decoder at
    /// the next picture and requests a key frame, and the Encoding row and the
    /// status line follow; a decoder that loses its references also requests
    /// one; nothing else does.
    #[test]
    fn a_switch_of_decoding_is_a_new_decoder_and_a_key_frame() {
        let decoders = Arc::new(Switchable::default());
        let client = Client::new(Arc::clone(&decoders) as Arc<dyn Decoders>);
        let (out, sent) = channel();
        *client.out.lock().unwrap() = Some(out);
        let refreshes = || {
            sent.try_iter()
                .filter(|bytes| match Message::parse_from_bytes(bytes).unwrap().union {
                    Some(message::Union::Misc(misc)) => misc.refresh_video(),
                    _ => false,
                })
                .count()
        };
        let mut back = Back::new();
        client.width.store(4, Ordering::Relaxed);
        client.height.store(2, Ordering::Relaxed);
        client.resize(4, 2, &mut back);
        let frame = || {
            let mut one = crate::protos::message::EncodedVideoFrame::new();
            one.data = vec![0; 4];
            let mut frames = crate::protos::message::EncodedVideoFrames::new();
            frames.frames.push(one);
            let mut frame = VideoFrame::new();
            frame.set_h264s(frames);
            frame
        };
        let mut decoder = None;

        client.picture(frame(), &mut decoder, &mut back, &mut Nothing);
        assert_eq!(decoders.made.load(Ordering::Relaxed), 1);
        assert_eq!(client.info().encoding, "H.264 (software)");
        assert_eq!(client.decoder_status().as_deref(), Some("H.264 sw"));
        assert_eq!(refreshes(), 0, "a first decoder waits for the peer's own key frame");

        client.picture(frame(), &mut decoder, &mut back, &mut Nothing);
        assert_eq!(decoders.made.load(Ordering::Relaxed), 1);

        *decoders.hardware.lock().unwrap() = Hardware::Auto;
        client.picture(frame(), &mut decoder, &mut back, &mut Nothing);
        assert_eq!(decoders.made.load(Ordering::Relaxed), 2);
        assert_eq!(refreshes(), 1);
        assert_eq!(client.info().encoding, "H.264 (hardware)");
        assert_eq!(client.decoder_status().as_deref(), Some("H.264 vaapi"));

        decoders.lose_references.store(true, Ordering::Relaxed);
        client.picture(frame(), &mut decoder, &mut back, &mut Nothing);
        assert_eq!(decoders.made.load(Ordering::Relaxed), 2);
        assert_eq!(refreshes(), 1);
        client.picture(frame(), &mut decoder, &mut back, &mut Nothing);
        assert_eq!(refreshes(), 0);
    }

    /// A handler that records the damage it receives.
    #[derive(Default)]
    struct Damage(Vec<(usize, usize, usize, usize)>);
    impl Handler for Damage {
        fn connected(&mut self, _width: usize, _height: usize) {}
        fn desktop_size(&mut self, _width: usize, _height: usize) {}
        fn damaged(&mut self, x: usize, y: usize, width: usize, height: usize) {
            self.0.push((x, y, width, height));
        }
        fn frame_end(&mut self) {}
        fn cursor(&mut self, _p: &[u32], _w: usize, _h: usize, _hx: i32, _hy: i32) {}
        fn clipboard(&mut self, _text: &str) {}
        fn credentials(&mut self, _needs_user: bool) -> Option<runtime::Credentials> {
            None
        }
        fn trust(&mut self, _fingerprint: &str) -> bool {
            false
        }
    }

    /// While the window accepts GPU pictures, the decoder's picture becomes
    /// the framebuffer's as is, the whole desktop is damaged, and nothing is
    /// converted until it is read; once the window stops accepting them, the
    /// next picture is converted whole, without a new decoder or key frame.
    #[test]
    fn a_picture_on_the_gpu_is_the_picture_until_one_is_converted() {
        let decoders = Arc::new(Switchable::default());
        let client = Client::new(Arc::clone(&decoders) as Arc<dyn Decoders>);
        let mut back = Back::new();
        client.width.store(4, Ordering::Relaxed);
        client.height.store(2, Ordering::Relaxed);
        client.resize(4, 2, &mut back);
        let frame = || {
            let mut one = crate::protos::message::EncodedVideoFrame::new();
            one.data = vec![0; 4];
            let mut frames = crate::protos::message::EncodedVideoFrames::new();
            frames.frames.push(one);
            let mut frame = VideoFrame::new();
            frame.set_h264s(frames);
            frame
        };
        let words = |client: &Client| {
            let mut out = [0u8; 32];
            assert!(client.framebuffer().read().unwrap().read_rgba(0, 0, 4, 2, &mut out, 16));
            out.chunks_exact(4)
                .map(|w| u32::from_le_bytes(w.try_into().unwrap()))
                .collect::<Vec<_>>()
        };
        let mut decoder = None;
        let mut damage = Damage::default();

        client.picture(frame(), &mut decoder, &mut back, &mut damage);
        assert!(client.gpu_picture().is_none());
        assert_eq!(words(&client)[0], 0xff00_0001);

        decoders.gpu.store(true, Ordering::Relaxed);
        damage.0.clear();
        client.picture(frame(), &mut decoder, &mut back, &mut damage);
        assert_eq!(decoders.made.load(Ordering::Relaxed), 1);
        assert!(client.gpu_picture().is_some());
        assert_eq!(damage.0, [(0, 0, 4, 2)]);
        assert!(client.framebuffer().read().unwrap().behind());
        client.framebuffer().write().unwrap().catch_up();
        assert_eq!(words(&client), [9; 8]);

        decoders.gpu.store(false, Ordering::Relaxed);
        damage.0.clear();
        client.picture(frame(), &mut decoder, &mut back, &mut damage);
        assert_eq!(decoders.made.load(Ordering::Relaxed), 1);
        assert!(client.gpu_picture().is_none());
        assert_eq!(damage.0, [(0, 0, 4, 2)], "converted whole");
    }

    /// A zstd frame that decodes to `total` bytes of `byte`, as RLE blocks of
    /// 128 KiB: four bytes on the wire each, so 5 GiB of output is 160 KiB of
    /// message, well within RustDesk's frame limit.
    fn zstd_rle(byte: u8, total: usize) -> Vec<u8> {
        const BLOCK: usize = 128 << 10;
        let mut out = vec![0x28, 0xb5, 0x2f, 0xfd]; // the magic, little-endian
        out.push(0); // no content size, no checksum, no dictionary, windowed
        out.push(7 << 3); // a 128 KiB window
        let mut left = total;
        while left > 0 {
            let size = left.min(BLOCK);
            left -= size;
            let header = (size << 3) | (1 << 1) | usize::from(left == 0); // RLE
            out.extend_from_slice(&header.to_le_bytes()[..3]);
            out.push(byte);
        }
        out
    }

    #[derive(Default)]
    struct Pasted(Vec<String>);
    impl Handler for Pasted {
        fn connected(&mut self, _width: usize, _height: usize) {}
        fn desktop_size(&mut self, _width: usize, _height: usize) {}
        fn damaged(&mut self, _x: usize, _y: usize, _width: usize, _height: usize) {}
        fn frame_end(&mut self) {}
        fn cursor(&mut self, _p: &[u32], _w: usize, _h: usize, _hx: i32, _hy: i32) {}
        fn clipboard(&mut self, text: &str) {
            self.0.push(text.to_string());
        }
        fn credentials(&mut self, _needs_user: bool) -> Option<runtime::Credentials> {
            None
        }
        fn trust(&mut self, _fingerprint: &str) -> bool {
            false
        }
    }

    fn text_clipboard(content: Vec<u8>) -> Clipboard {
        let mut clip = Clipboard::new();
        clip.compress = true;
        clip.content = content;
        clip.format = ClipboardFormat::Text.into();
        clip
    }

    /// The peer's clipboard is zstd, and its decoded size is controlled by the
    /// peer: a few hundred kilobytes that decode to gigabytes are skipped once
    /// they pass the shared bound, rather than decoded until allocation fails,
    /// and a clipboard at the bound still arrives whole.
    #[test]
    fn a_clipboard_is_decoded_only_as_far_as_the_bound() {
        let client = Client::new(Arc::new(video::NoDecoders));
        let mut pasted = Pasted::default();
        client.clipboard_arrived(text_clipboard(zstd_rle(b'x', 5 << 30)), &mut pasted);
        client.clipboard_arrived(text_clipboard(zstd_rle(b'x', runtime::MAX_CLIPBOARD + 1)), &mut pasted);
        assert!(pasted.0.is_empty(), "nothing of a clipboard past the bound");
        client.clipboard_arrived(text_clipboard(zstd_rle(b'y', runtime::MAX_CLIPBOARD)), &mut pasted);
        let plain = zstd::encode_all(&b"hello"[..], 3).unwrap();
        client.clipboard_arrived(text_clipboard(plain), &mut pasted);
        assert_eq!(pasted.0.len(), 2);
        assert_eq!(pasted.0[0].len(), runtime::MAX_CLIPBOARD);
        assert_eq!(pasted.0[1], "hello");
    }

    fn cursor_data(width: i32, height: i32, colors: Vec<u8>) -> crate::protos::message::CursorData {
        let mut data = crate::protos::message::CursorData::new();
        data.width = width;
        data.height = height;
        data.colors = colors;
        data
    }

    /// A cursor's size is two 32-bit fields from the peer and its pixels are
    /// zstd: the size is checked against the shared cursor bound, and the
    /// pixels are decoded only as far as that size needs.
    #[test]
    fn a_cursor_is_decoded_only_as_far_as_its_size() {
        let bomb = zstd_rle(0xff, 5 << 30);
        let four = Cursor::of(cursor_data(2, 2, bomb.clone())).expect("four pixels out of gigabytes");
        assert_eq!(four.pixels, [u32::MAX; 4], "and the rest never made");
        assert!(Cursor::of(cursor_data(i32::MAX, i32::MAX, bomb)).is_none());
        let side = runtime::MAX_CURSOR_SIDE as i32;
        assert!(Cursor::of(cursor_data(side + 1, 1, zstd_rle(0xff, (side as usize + 1) * 4))).is_none());

        let cursor = Cursor::of(cursor_data(side, 1, zstd_rle(0xff, side as usize * 4))).unwrap();
        assert_eq!((cursor.width, cursor.height, cursor.pixels.len()), (side as usize, 1, side as usize));
        let small = zstd::encode_all(&[1u8, 2, 3, 4, 5, 6, 7, 8][..], 3).unwrap();
        let cursor = Cursor::of(cursor_data(2, 1, small)).unwrap();
        assert_eq!(cursor.pixels, [0x0403_0201, 0x0807_0605]);
    }

    fn shape(id: u64, byte: u8) -> crate::protos::message::CursorData {
        let mut data = cursor_data(2, 2, zstd::encode_all(&[byte; 16][..], 3).unwrap());
        data.id = id;
        data
    }

    fn pixels(cursor: Option<&Cursor>) -> Option<Vec<u32>> {
        cursor.map(|c| c.pixels.clone())
    }

    /// A peer before 1.5.0 names a shape by the platform's handle, and an
    /// application can show the same shape under a new handle each time: the
    /// pixels arrive again under each id, and are kept once.
    #[test]
    fn a_shape_sent_under_several_ids_is_kept_once() {
        let mut cursors = Cursors::default();
        let arrow = Some(vec![0x0101_0101; 4]);
        assert_eq!(pixels(cursors.arrived(shape(10, 1))), arrow);
        assert_eq!(pixels(cursors.arrived(shape(11, 1))), arrow);
        assert_eq!(pixels(cursors.arrived(shape(12, 2))), Some(vec![0x0202_0202; 4]));
        assert_eq!(cursors.shapes.len(), 2);
        assert_eq!(pixels(cursors.get(10)), arrow);
        assert_eq!(pixels(cursors.get(11)), arrow);
        assert!(cursors.get(13).is_none());

        // The hotspot is part of the shape.
        let mut moved = shape(13, 1);
        moved.hotx = 1;
        assert_eq!(cursors.arrived(moved).map(|c| c.hot_x), Some(1));
        assert_eq!(cursors.shapes.len(), 3);
    }

    /// An id sent again with other pixels names the new shape, and one whose
    /// pixels do not decode names nothing, rather than the shape before.
    #[test]
    fn an_id_names_the_shape_it_last_arrived_with() {
        let mut cursors = Cursors::default();
        cursors.arrived(shape(10, 1));
        cursors.arrived(shape(10, 2));
        assert_eq!(pixels(cursors.get(10)), Some(vec![0x0202_0202; 4]));

        let mut broken = shape(10, 3);
        broken.colors = vec![0; 8];
        assert!(cursors.arrived(broken).is_none());
        assert!(cursors.get(10).is_none());
    }
}
