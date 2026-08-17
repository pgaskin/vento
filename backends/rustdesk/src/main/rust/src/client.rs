//! One RustDesk session: the handshake, the message loop, and everything the
//! app can ask of a connection that is up.
//!
//! The protocol is small — one framed protobuf `Message` with a `oneof` of
//! thirty arms — and most of what their product is lives in arms this app has
//! no seam for. What is here is the seven that make a remote desktop: the login
//! exchange, the picture, the cursor, the clipboard, input in both of its
//! forms, and the timing message that has to be answered or a link saturates.
//!
//! Three things about the far end are not guessable from the `.proto` and each
//! cost a run to find:
//!
//! - **`TestDelay` is echoed unchanged**, and only when the flag in it says the
//!   peer started the exchange. Their server echoes anything with that flag
//!   set, so a client that sets it on a reply gets its own message back and
//!   answers it again — 9701 messages in fifteen seconds, on an idle session.
//! - **A down event carries every modifier held.** Their server *releases*, on
//!   the far end, each modifier not named in the event, so a Ctrl armed for the
//!   letter after it is a Ctrl let go the moment that letter arrives. The same
//!   is true of a mouse button going down, which is what makes Ctrl+click work.
//! - **A Windows peer with more than one session logged in waits**: it
//!   subscribes no video service at all until the client answers with a session
//!   id. Nothing announces the wait — the login succeeds, the desktop simply
//!   never comes.

use std::collections::HashMap;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
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
    message, misc, option_message::BoolOption, supported_decoding::PreferCodec, video_frame,
};
use crate::rendezvous;
use crate::video::{self, Codec};
use crate::wire::{self, Traffic};

/// Their default direct-access port, which is `RENDEZVOUS_PORT + 2`.
pub const DIRECT_PORT: u16 = 21118;

/// What we claim to be. Their server compares this against its own to decide
/// which of its compatibility paths to take, so an honest answer is the one
/// that avoids a path nothing here implements.
pub const VERSION: &str = "1.4.9";

/// Their `TestDelay` arrives about once a second whatever the desktop is doing,
/// so silence for this long is a link that has gone. A timeout is fatal rather
/// than retried: a read that gives up part-way through a frame leaves the
/// stream out of step, and there is nothing to resynchronise on.
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// How often to say something while a question is on somebody's screen, which
/// has to be comfortably under the thirty seconds of silence the peer allows.
const KEEP_ALIVE_EVERY: Duration = Duration::from_secs(8);

/// What the far end is told to send when the session is not on screen. There is
/// no pause message in this protocol, and one frame a second is the nearest
/// thing to one — the picture stays current enough to be worth showing the
/// moment it comes back, at a hundredth of the traffic.
const UNFOCUSED_FPS: i32 = 1;

pub struct Config {
    /// The peer: `host` or `host:port` for [`Reach::Direct`], and their nine
    /// digits for [`Reach::Id`].
    pub address: String,
    pub reach: Reach,
    pub password: Option<String>,
    pub connect_timeout: Duration,
    /// What the far end calls this phone in its own log and its own screen.
    pub my_name: String,
    /// The live options' opening values. What they are afterwards is the
    /// session's, since each of them can be changed while it is running.
    pub quality: Quality,
    /// 0 for the far end's own choice.
    pub fps: i32,
    pub codec: PreferredCodec,
    /// Whether the far end locks its screen when this session ends. Sent with
    /// the login and never changed: what it asks for happens after the last
    /// message either end sends.
    pub lock_after: bool,
}

/// The two ways to a peer, which are not two transports of one connection but
/// two different things to have chosen.
///
/// Direct access is a mode the far end switched on, reaching a machine somebody
/// can already address, and it has no key exchange at all. The id path reaches
/// a machine nothing on this network can address, and the encryption comes with
/// it rather than being a second decision: what makes the session verifiable is
/// the peer key the rendezvous server signs.
pub enum Reach {
    Direct,
    Id {
        /// `host` or `host:port`; empty for the public network.
        server: String,
        /// The server's public key, base64, as their `id_ed25519.pub` holds it;
        /// empty for the public network's own, which is a constant in every
        /// build of theirs.
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
/// it to choose — which on their server means whatever hardware encoder that
/// machine has, falling back to VP9, since VP9 is the one codec compiled into
/// every build of theirs.
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

    /// The codec this asks for, or `None` for [`Auto`](Self::Auto). What it is
    /// for is the check before it is sent: asking for one this phone cannot
    /// decode is a session with no picture, and the far end has no way to know.
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

/// What the handshake settled, and what is left to ask about it.
enum Verified {
    /// The peer proved it holds the key the rendezvous server vouched for, and
    /// both halves of the socket are sealed under a key agreed with it.
    Encrypted([u8; 32]),
    /// Something did not check out, in these words, and the session is in the
    /// clear — which the app has still to agree to.
    Clear(String),
}

/// What the protocol thread tells the app. Everything here arrives on that
/// thread; what the app does about it is the app's business.
pub trait Handler {
    fn connected(&mut self, width: usize, height: usize);

    fn desktop_size(&mut self, width: usize, height: usize);

    fn damaged(&mut self, x: usize, y: usize, width: usize, height: usize);

    fn frame_end(&mut self);

    fn cursor(&mut self, pixels: &[u32], width: usize, height: usize, hot_x: i32, hot_y: i32);

    fn clipboard(&mut self, text: &str);

    /// The peer wants a password, and the session is stopped here until one
    /// arrives. `None` cancels, which ends it.
    fn password(&mut self) -> Option<String>;

    /// The peer's long-term key, by fingerprint, before this end tells it
    /// anything. `false` gives up the connection.
    ///
    /// Only the id path asks: direct access asserts no identity, so there would
    /// be nothing to put the question about.
    fn trust(&mut self, fingerprint: &str) -> bool;

    /// The peer could not be verified at all, in the words of why. `true` goes
    /// on with the session in the clear, which is what their own client does
    /// without asking anybody; `false` ends it.
    fn unverified(&mut self, why: &str) -> bool;
}

/// Everything the connection panel can be told.
#[derive(Clone, Default)]
pub struct Info {
    pub desktop_name: String,
    pub protocol: String,
    pub connection: String,
    pub security: String,
    pub encoding: String,
    /// The round trip their own timing message measures, which is the only
    /// number about the link this protocol offers.
    pub round_trip: String,
    pub platform: String,
    pub display: String,
}

/// One button of theirs, already shifted into the place a mask wants it.
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

/// An RFB button mask, which is what the gesture stack speaks, against theirs.
/// The wheel is not in here: it is four bits of the same mask and becomes an
/// axis rather than a button.
const BUTTONS: [(i32, i32); 5] = [
    (0x01, mouse::LEFT),
    (0x02, mouse::MIDDLE),
    (0x04, mouse::RIGHT),
    (0x80, mouse::BACK),
    (0x100, mouse::FORWARD),
];

/// How far one notch of the wheel scrolls. Their server hands the number
/// straight to the platform's own scroll call, so this is in whatever that
/// calls a line — and one notch is one line on both far ends the rig has.
const WHEEL_NOTCH: i32 = 1;

pub struct Client {
    framebuffer: RwLock<Framebuffer>,
    traffic: Arc<Traffic>,
    /// Serialized messages on their way to the writer thread. Input is encoded
    /// on whichever thread produced it and written on a thread of its own, so a
    /// stalled socket never reaches the main one.
    out: Mutex<Option<Sender<Vec<u8>>>>,
    /// The socket, for closing it from another thread: a read blocked on a
    /// socket wakes when it is shut down and there is nothing else to interrupt
    /// it with.
    closer: Mutex<Option<TcpStream>>,
    info: Mutex<Info>,
    /// The sizes the far end will take, which is a fixed list rather than RFB's
    /// free choice — empty for a peer that offers none.
    resolutions: Mutex<Vec<(i32, i32)>>,
    /// The far end's displays as rectangles of its whole desktop, in the peer's
    /// own order — empty where it has only one, since there is then nothing to
    /// choose between.
    displays: Mutex<Vec<(i32, i32, i32, i32)>>,
    /// Where the captured display's top left corner sits on the peer's whole
    /// desktop. Input is injected into that desktop rather than into the
    /// picture, so a pointer event on a second screen is off by this much
    /// unless it is added — landing on whatever is at those coordinates on the
    /// primary one. The pair is under one lock because a switch moves both.
    origin: Mutex<(i32, i32)>,
    /// What each held key was sent as, by the id the app ties an up to a down
    /// with.
    keys: Mutex<HashMap<i64, Key>>,
    /// The modifiers held, in the order they went down. Every down event
    /// carries this list.
    modifiers: Mutex<Vec<ControlKey>>,
    /// Where the pointer was last put, so that a button press does not have to
    /// repeat a move that has not changed.
    pointer: Mutex<(i32, i32, i32)>,
    view_only: AtomicBool,
    focused: AtomicBool,
    closed: AtomicBool,
    width: AtomicI32,
    height: AtomicI32,
    display: AtomicI32,
    /// The live options: what a running session has been told, which is where
    /// they live because every one of them can be changed while it is up.
    quality: AtomicI32,
    /// The configured frame rate, which unfocusing lowers and focusing puts
    /// back.
    fps: AtomicI32,
    /// The codec asked for, and the last one the peer was told about. The
    /// second is what keeps a quality change from being a codec renegotiation:
    /// the preference goes out only when it has moved.
    codec: AtomicI32,
    codec_sent: AtomicI32,
    /// The last round trip their timing message measured, in milliseconds.
    round_trip: AtomicU64,
}

impl Client {
    pub fn new() -> Client {
        Client {
            framebuffer: RwLock::new(Framebuffer::new()),
            traffic: Arc::new(Traffic::default()),
            out: Mutex::new(None),
            closer: Mutex::new(None),
            info: Mutex::new(Info::default()),
            resolutions: Mutex::new(Vec::new()),
            displays: Mutex::new(Vec::new()),
            origin: Mutex::new((0, 0)),
            keys: Mutex::new(HashMap::new()),
            modifiers: Mutex::new(Vec::new()),
            pointer: Mutex::new((-1, -1, 0)),
            view_only: AtomicBool::new(false),
            focused: AtomicBool::new(true),
            closed: AtomicBool::new(false),
            width: AtomicI32::new(0),
            height: AtomicI32::new(0),
            display: AtomicI32::new(0),
            quality: AtomicI32::new(Quality::Balanced.ordinal()),
            fps: AtomicI32::new(0),
            codec: AtomicI32::new(PreferredCodec::Auto.ordinal()),
            codec_sent: AtomicI32::new(-1),
            round_trip: AtomicU64::new(0),
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

    /// Whether the far end offers a size it is not already at.
    ///
    /// Unlike RFB this is a list rather than a free choice, so a request is
    /// answered with the nearest of them — and a peer whose list is the one
    /// mode it is running, which is every Xvfb, offers a control that could
    /// only ever choose what is already on screen.
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

    /// The far end's displays, four numbers each — x, y, width, height in its
    /// own desktop — and which of them is being captured.
    ///
    /// This protocol sends **one** display at a time, so the list is a choice
    /// rather than a description of the picture: what is on screen is one of
    /// these rectangles and the others are not in it anywhere.
    pub fn displays(&self) -> (Vec<i32>, i32) {
        let displays = self.displays.lock().unwrap();
        let flat = displays
            .iter()
            .flat_map(|(x, y, w, h)| [*x, *y, *w, *h])
            .collect();
        (flat, self.display.load(Ordering::Relaxed))
    }

    /// Ask the far end to capture another of its displays. What arrives back is
    /// a `SwitchDisplay`, so nothing here assumes it worked.
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

    /// Which display this end wants, and a fresh picture of it.
    ///
    /// The pair a client of this version is expected to send once the peer has
    /// said what it is capturing — at the login, and again every time that
    /// changes.
    fn ask_for(&self, display: i32) {
        let mut captured = CaptureDisplays::new();
        captured.set = vec![display];
        let mut misc = Misc::new();
        misc.set_capture_displays(captured);
        let mut out = Message::new();
        out.set_misc(misc);
        self.send(out);
        let mut misc = Misc::new();
        misc.set_refresh_video(true);
        let mut out = Message::new();
        out.set_misc(misc);
        self.send(out);
    }

    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        if let Some(sock) = self.closer.lock().unwrap().take() {
            let _ = sock.shutdown(std::net::Shutdown::Both);
        }
    }

    // ---- the session ------------------------------------------------------

    /// Connect, log in, and run until the far end or this end stops it. The
    /// calling thread is the protocol thread and every callback arrives on it.
    pub fn run(&self, config: &Config, handler: &mut impl Handler) -> Result<()> {
        self.quality.store(config.quality.ordinal(), Ordering::Relaxed);
        self.fps.store(config.fps, Ordering::Relaxed);
        self.codec.store(config.codec.ordinal(), Ordering::Relaxed);

        // How the socket is obtained is the whole of the difference between the
        // two modes: everything past this point is one session either way.
        let (sock, introduction, connection) = match &config.reach {
            Reach::Direct => {
                let addr = resolve(&config.address)?;
                log::info!("connecting to {addr}");
                let sock = TcpStream::connect_timeout(&addr, config.connect_timeout)?;
                sock.set_nodelay(true)?;
                (sock, None, format!("Direct access to {addr}"))
            }
            Reach::Id { server, key } => {
                let server = if server.trim().is_empty() {
                    rendezvous::PUBLIC_SERVER
                } else {
                    server.trim()
                };
                let reached =
                    rendezvous::reach(&config.address, server, key.trim(), config.connect_timeout)?;
                let connection = reached.how.describe();
                (
                    reached.sock,
                    Some((reached.signed_id_pk, key.clone())),
                    connection,
                )
            }
        };
        let (mut reader, mut writer, closer) = wire::split(sock, Arc::clone(&self.traffic))?;
        if self.closed.load(Ordering::SeqCst) {
            return Err(Error::Closed);
        }
        *self.closer.lock().unwrap() = Some(closer);
        self.info.lock().unwrap().connection = connection;

        let verified = match introduction {
            Some((signed_id_pk, key)) => {
                reader.set_timeout(Some(config.connect_timeout))?;
                let verified =
                    self.secure(&mut reader, &mut writer, &config.address, &signed_id_pk, &key)?;
                let mut info = self.info.lock().unwrap();
                // Which server vouched is the whole of the trust difference
                // between the two ways of using this path: a key somebody
                // pasted is their own pin, and the public network's is a pin
                // chosen by RustDesk. And a session somebody agreed to have
                // anyway says both halves of what it is.
                info.security = match verified {
                    Verified::Encrypted(_) => format!(
                        "xsalsa20-poly1305, vouched for by {}",
                        if key.trim().is_empty() {
                            "the public network"
                        } else {
                            "your rendezvous server"
                        }
                    ),
                    Verified::Clear(_) => {
                        "None (unencrypted), and the machine was not verified".into()
                    }
                };
                Some(verified)
            }
            None => {
                // Not a footnote and not only in the log: this mode has no key
                // exchange in it at all, and the panel is where a session says
                // what it is protected by.
                self.info.lock().unwrap().security = "None (unencrypted)".into();
                // A direct-access peer expects a client with no key to say so
                // with an empty message, which is the whole of that mode's
                // handshake.
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

        // Whether this end asked, read **before** the tidying below closes the
        // socket itself: an error that arrived because somebody pressed
        // Disconnect is not one the screen should be told about, and one that
        // arrived on its own is.
        let asked = self.closed.load(Ordering::SeqCst);
        // The sender is what keeps the writer thread alive, so it goes before
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

    /// The handshake the direct mode has none of, ending with both halves of
    /// the socket sealed — or, where something did not check out, with neither
    /// and with the reason.
    ///
    /// **Nothing is asked here, and that is a deadline rather than a
    /// preference.** The peer gives a handshake **18 seconds** and drops the
    /// connection when it passes, which is less than a person takes to read a
    /// dialog — measured, by leaving one on screen. So the exchange is finished
    /// at machine speed and the question goes to the app afterwards, before the
    /// login, which is the first thing on this wire that is anybody's secret.
    ///
    /// **A failure is a question rather than a refusal or a shrug.** Their own
    /// client falls back to an unencrypted session whenever any step does not
    /// check out — a signature that fails, a key that is not the one the server
    /// vouched for, a peer that sends no `SignedId` at all — and says so only in
    /// its log, which makes a session's own claim about itself worthless.
    /// Refusing outright is the other end of that and deletes a session
    /// somebody may still want.
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
        // One: the server's introduction, which says this id belongs to this
        // long-term key.
        let vouched = if signed_id_pk.is_empty() {
            Err("The rendezvous server did not vouch for a key.".to_string())
        } else {
            crypto::open_id_pk(signed_id_pk, &server_key, peer_id)
                .map_err(|e| format!("The rendezvous server's introduction did not check out: {e}."))
        };
        let peer_key = match vouched {
            Ok(key) => key,
            // Nothing has been read from the peer yet, so what it is waiting
            // for is a client with no key saying so — their empty message.
            Err(why) => return self.in_the_clear(writer, why, false),
        };
        // Two: the peer proving it holds that key, over a box key it made for
        // this connection alone. The question goes to the app between the two,
        // so nothing of ours is sealed to a machine somebody has refused.
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
        let box_key = match crypto::open_id_pk(&signed.id, &peer_key, peer_id) {
            Ok(key) => key,
            // The peer is waiting for a `PublicKey` here, so the way to say
            // there will be no key is an empty one — theirs does the same.
            Err(e) => {
                return self.in_the_clear(
                    writer,
                    format!("The machine's key is not the one it was vouched for: {e}."),
                    true,
                );
            }
        };
        // Three: a key of ours, sealed to theirs. Everything after this frame
        // is encrypted, in both directions, counted separately.
        let (our_box_pk, sealed, key) = crypto::seal_session_key(&box_key)?;
        let mut public_key = PublicKey::new();
        public_key.asymmetric_value = our_box_pk.into();
        public_key.symmetric_value = sealed.into();
        let mut out = Message::new();
        out.set_public_key(public_key);
        writer.send(&out)?;
        writer.set_cipher(Box::new(crypto::Secretbox::new(key)));
        reader.set_cipher(Box::new(crypto::Secretbox::new(key)));
        log::info!("the session is encrypted");
        Ok(Verified::Encrypted(peer_key))
    }

    /// Tell the peer there will be no key, and hand back the reason for the
    /// question the app is about to be asked.
    ///
    /// `expects_public_key` is whether the peer has already offered one: after
    /// its `SignedId` it is waiting for a `PublicKey`, and an empty one is how
    /// their own client declines; before that it is waiting for the empty
    /// message a client with no key of its own sends.
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
        handler: &mut impl Handler,
    ) -> Result<()> {
        // Their server opens with `Hash` before anything is asked of it.
        let hash = loop {
            match reader.message::<Message>()?.union {
                Some(message::Union::Hash(h)) => break h,
                Some(other) => log::info!("before Hash: {}", arm(&other)),
                None => {}
            }
        };
        // Whatever the handshake left to ask, asked here: after it, because the
        // peer times a handshake out under a person reading a dialog, and
        // before the login, because that is what carries the password. The
        // asking blocks this thread, so something else has to keep the socket
        // from going quiet while it does.
        let agreed = match &verified {
            Some(Verified::Encrypted(peer_key)) => {
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
        let mut password = config.password.clone().unwrap_or_default();
        self.send_login(config, &hash, &password);
        reader.set_timeout(Some(IDLE_TIMEOUT))?;

        let mut decoder: Option<(Codec, (usize, usize), Box<dyn video::Decoder>)> = None;
        let mut back = Back::new();
        let mut rate = Rate::new();
        let mut cursors: HashMap<u64, Cursor> = HashMap::new();
        let mut connected = false;

        loop {
            let message = match reader.message::<Message>() {
                Ok(m) => m,
                Err(e) if timed_out(&e) => return Err(Error::Silent),
                Err(e) => return Err(Error::Io(e)),
            };
            let Some(union) = message.union else { continue };
            match union {
                message::Union::LoginResponse(response) => match response.union {
                    Some(login_response::Union::PeerInfo(peer)) => {
                        self.arrived(&peer, handler, &mut back)?;
                        connected = true;
                    }
                    // Both password errors are answered on the same connection:
                    // their client re-prompts and sends another `LoginRequest`
                    // rather than dialling again, and the peer is waiting for
                    // exactly that.
                    Some(login_response::Union::Error(why))
                        if why == "Wrong Password" || why == "Empty Password" =>
                    {
                        log::info!("the peer wants a password ({why})");
                        match handler.password() {
                            Some(answer) => {
                                password = answer;
                                self.send_login(config, &hash, &password);
                            }
                            None => return Err(Error::Cancelled),
                        }
                    }
                    Some(login_response::Union::Error(why)) => return Err(Error::Refused(why)),
                    None => {}
                },
                message::Union::VideoFrame(frame) => {
                    if connected {
                        let started = Instant::now();
                        self.picture(frame, &mut decoder, &mut back, handler);
                        rate.frame(started.elapsed(), back.converted());
                        // Every frame, drawn or not: this is what the peer is
                        // waiting for before it captures the next one, so a
                        // frame that changed nothing still has to be answered.
                        let mut misc = Misc::new();
                        misc.set_video_received(true);
                        let mut out = Message::new();
                        out.set_misc(misc);
                        self.send(out);
                    }
                }
                message::Union::CursorData(data) => {
                    let id = data.id;
                    if let Some(cursor) = Cursor::of(data) {
                        cursor.deliver(handler);
                        cursors.insert(id, cursor);
                    }
                }
                message::Union::CursorId(id) => {
                    if let Some(cursor) = cursors.get(&id) {
                        cursor.deliver(handler);
                    }
                }
                message::Union::Clipboard(clip) => self.clipboard_arrived(clip, handler),
                message::Union::MultiClipboards(many) => {
                    // Whichever of them is text; the rest are formats the seam
                    // has no concept of.
                    for clip in many.clipboards {
                        self.clipboard_arrived(clip, handler);
                    }
                }
                message::Union::TestDelay(delay) => {
                    if !delay.from_client {
                        // The round trip comes free with it: `last_delay` is
                        // what the peer measured from the previous exchange, so
                        // a client that echoes promptly is a client that has
                        // been told its own latency and never has to ask.
                        if delay.last_delay > 0 {
                            self.round_trip.store(delay.last_delay as u64, Ordering::Relaxed);
                        }
                        // Echoed **unchanged**. Setting the flag here is what
                        // turns one message a second into a busy loop between
                        // the two ends.
                        let mut out = Message::new();
                        out.set_test_delay(delay);
                        self.send(out);
                    }
                }
                message::Union::MessageBox(box_) => {
                    log::info!("message box: [{}] {} {}", box_.msgtype, box_.title, box_.text);
                }
                message::Union::Misc(misc) => match misc.union {
                    Some(misc::Union::CloseReason(why)) => return Err(Error::Refused(why)),
                    Some(misc::Union::SwitchDisplay(display)) => {
                        self.switched(&display, handler, &mut back);
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    }

    /// Everything that happens once, when the peer has said who it is.
    fn arrived(
        &self,
        peer: &PeerInfo,
        handler: &mut impl Handler,
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
        // A peer with one display is a peer with nothing to choose, and an
        // empty list is what the panel reads as "no row here".
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

        // A Windows peer with RDP sharing on and more than one session logged
        // in subscribes no video service until it is told which session to
        // capture. Answering with anything other than the one the service is in
        // hands the connection to that session and drops this one, so the
        // answer is the peer's own current one.
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
        // Which display this end wants captured, and a refresh so the first
        // picture is a key frame rather than a wait for whatever the encoder
        // does next.
        self.ask_for(current as i32);

        self.resize(display.width, display.height, back);
        self.width.store(display.width, Ordering::Relaxed);
        self.height.store(display.height, Ordering::Relaxed);
        handler.connected(display.width.max(0) as usize, display.height.max(0) as usize);
        Ok(())
    }

    /// The far end changed what it is capturing, or how big it is.
    ///
    /// One message for both, and the peer sends it whether the change was asked
    /// for or not — a display unplugged over there arrives here as the same
    /// thing a switch does. The sizes it will take come with it, since they are
    /// the new display's rather than the old one's.
    fn switched(
        &self,
        switch: &SwitchDisplay,
        handler: &mut impl Handler,
        back: &mut Back,
    ) {
        let (width, height) = (switch.width, switch.height);
        // Ahead of the size checks below, both of which return early: where the
        // picture is on the peer's desktop is the one thing here that decides
        // where input lands, and a switch that changed nothing else has still
        // moved it.
        *self.origin.lock().unwrap() = (switch.x, switch.y);
        if self.display.swap(switch.display, Ordering::Relaxed) != switch.display {
            // The capture set and the key frame again, for the same reason the
            // login sends them: the peer has said what it is capturing now, and
            // this end has to say what it wants of it.
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
        // Both swapped before either is compared: `&&` would leave the height
        // alone whenever the width had changed, which is a framebuffer sized
        // 1024×1440 for a 1024×768 display and a picture that never arrives.
        let was_width = self.width.swap(width, Ordering::Relaxed);
        let was_height = self.height.swap(height, Ordering::Relaxed);
        if was_width == width && was_height == height {
            return;
        }
        log::info!("the desktop is now {width}x{height}");
        self.resize(width, height, back);
        handler.desktop_size(width as usize, height as usize);
    }

    /// The panel's diagnostic line, which says which of the peer's displays the
    /// picture is — and nothing at all where there is only one, since a desktop
    /// having a screen is not news.
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
    /// no damage rectangles anywhere in it, so what moved is found here rather
    /// than read off the wire.
    fn picture(
        &self,
        frame: VideoFrame,
        decoder: &mut Option<(Codec, (usize, usize), Box<dyn video::Decoder>)>,
        back: &mut Back,
        handler: &mut impl Handler,
    ) {
        let Some(union) = frame.union else { return };
        let (codec, frames) = match union {
            video_frame::Union::Vp9s(v) => (Codec::Vp9, v.frames),
            video_frame::Union::Vp8s(v) => (Codec::Vp8, v.frames),
            video_frame::Union::Av1s(v) => (Codec::Av1, v.frames),
            video_frame::Union::H264s(v) => (Codec::H264, v.frames),
            video_frame::Union::H265s(v) => (Codec::H265, v.frames),
            // Raw pixels, which their server only sends where nothing can
            // encode; nothing here asks for it and no run has produced one.
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
        // picture size, and this protocol changes that under a running session
        // whenever the far end switches display.
        if decoder
            .as_ref()
            .is_none_or(|(had, size, _)| *had != codec || *size != (w, h))
        {
            match video::decoder(codec, w, h) {
                Some(new) => {
                    self.info.lock().unwrap().encoding = codec.name().into();
                    *decoder = Some((codec, (w, h), new));
                }
                None => {
                    self.info.lock().unwrap().encoding = format!("{} (no decoder)", codec.name());
                    return;
                }
            }
        }
        let Some((_, _, decoder)) = decoder.as_mut() else {
            return;
        };
        back.begin(w, h);
        let mut drew = false;
        for frame in frames {
            drew |= decoder.decode(&frame.data, &mut back.pixels, w, h, &mut back.changed);
        }
        if !drew {
            return;
        }
        // A frame that repeats the last one — which is what a peer sends to a
        // client whose desktop nobody is touching — costs nothing from here on,
        // and a frame that moved a caret costs the rows the caret is on.
        let Some((dx, dy, dw, dh)) = back.changed.bounds(w) else {
            return;
        };
        // Under a read lock and before the swap takes the picture away: the
        // rows this frame did not convert are still the frame before last.
        if !back.carry(&self.framebuffer.read().unwrap()) {
            return;
        }
        // The write lock is held for a `Vec` swap rather than for a conversion,
        // which is what keeps the drawing thread out of the decoder's way.
        if !self.framebuffer.write().unwrap().swap(&mut back.pixels) {
            back.lost();
            return;
        }
        back.swapped();
        handler.damaged(dx, dy, dw, dh);
        handler.frame_end();
    }

    fn clipboard_arrived(&self, clip: Clipboard, handler: &mut impl Handler) {
        if clip.format.enum_value_or_default() != ClipboardFormat::Text {
            return;
        }
        let content = if clip.compress {
            zstd::decode_all(&clip.content[..]).unwrap_or_default()
        } else {
            clip.content.to_vec()
        };
        if let Ok(text) = String::from_utf8(content) {
            if !text.is_empty() {
                // The length rather than the text: a clipboard is somebody's
                // password as often as it is a URL.
                log::info!("{} characters of clipboard from the peer", text.chars().count());
                handler.clipboard(&text);
            }
        }
    }

    fn send_login(&self, config: &Config, hash: &Hash, password: &str) {
        let mut login = LoginRequest::new();
        // **The address, not a name.** Their peer refuses a login whose
        // username is neither its own id nor something that parses as an
        // address, and the refusal it sends is `Offline` — which reads like the
        // machine is not there rather than like a field being wrong.
        login.username = config.address.clone();
        login.password = login_hash(password, hash).into();
        // Their peer shows this and logs it, so it is the phone rather than
        // anything about this app: a person looking at the far end wants to
        // know which device is on their desktop.
        login.my_id = config.my_name.clone();
        login.my_name = config.my_name.clone();
        login.my_platform = "Android".into();
        login.version = VERSION.into();
        login.session_id = 1;
        // **The far end paces itself to us.** Without this it captures and
        // sends on its own clock, and a phone that takes longer over a frame
        // than the peer takes to make one falls further behind with every one
        // of them — which is a desktop that drags a second late and gets worse
        // the longer somebody drags it. With it, the peer captures the next
        // frame when this end says it has finished with the last.
        login.video_ack_required = true;
        let mut option = self.options(true);
        // The one option here that is not about the picture, and the only one a
        // session can set and never see the effect of: what it asks for happens
        // after the last message either end sends. Left unset rather than
        // answered No, so a peer configured to lock itself goes on doing so.
        if config.lock_after {
            option.lock_after_session_end = BoolOption::Yes.into();
        }
        login.option = MessageField::some(option);
        let mut out = Message::new();
        out.set_login_request(login);
        self.send(out);
    }

    /// The options a session carries, at login and again whenever one changes.
    ///
    /// The codec preference is in it at login and afterwards **only when it has
    /// moved**, which is what keeps a quality change from being a codec
    /// renegotiation: their peer answers a preference by building a new encoder
    /// and starting again with a key frame, so repeating the one it already has
    /// would make every change of frame rate cost a whole screenful.
    fn options(&self, at_login: bool) -> OptionMessage {
        let mut option = OptionMessage::new();
        option.image_quality = Quality::of(self.quality.load(Ordering::Relaxed)).wire().into();
        let fps = self.fps.load(Ordering::Relaxed);
        option.custom_fps = if self.focused.load(Ordering::Relaxed) {
            fps
        } else {
            UNFOCUSED_FPS
        };
        // Nothing here plays audio, and a desktop that is talking is a stream
        // of Opus this session would pay for and drop. The clipboard has no
        // switch beside it because whether this phone shares one is a question
        // about the phone, answered once for every protocol in the app's own
        // settings.
        option.disable_audio = BoolOption::Yes.into();
        let codec = self.codec.load(Ordering::Relaxed);
        let moved = self.codec_sent.swap(codec, Ordering::Relaxed) != codec;
        if at_login || moved {
            option.supported_decoding = MessageField::some(self.decoding(PreferredCodec::of(codec)));
        }
        option
    }

    /// What this end can decode, and which of those it would rather have.
    ///
    /// The abilities are the **phone's**, probed rather than declared: their
    /// peer sends whatever a client says it can take, and a claim this device
    /// cannot honour is a session with a cursor and no picture. VP9 is the one
    /// every build of theirs can encode, so it is what a peer with nothing else
    /// falls back to and what an empty ability list would leave.
    fn decoding(&self, prefer: PreferredCodec) -> SupportedDecoding {
        let mut decoding = SupportedDecoding::new();
        decoding.ability_vp9 = video::decodable(Codec::Vp9) as i32;
        decoding.ability_vp8 = video::decodable(Codec::Vp8) as i32;
        decoding.ability_av1 = video::decodable(Codec::Av1) as i32;
        decoding.ability_h264 = video::decodable(Codec::H264) as i32;
        decoding.ability_h265 = video::decodable(Codec::H265) as i32;
        // A preference this phone cannot decode is not passed on: the far end
        // would honour it exactly, and nothing over there could tell that the
        // picture never arrived.
        let prefer = match prefer.codec() {
            Some(codec) if !video::decodable(codec) => {
                log::warn!("this device cannot decode {}, leaving the codec to the peer", codec.name());
                PreferredCodec::Auto
            }
            _ => prefer,
        };
        log::info!("codec preference {:?}", prefer);
        decoding.prefer = prefer.wire().into();
        decoding
    }

    /// A live option change, which is one message and takes effect on the
    /// picture in front of you.
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

    /// Whether the session is on screen. There is no pause message here, so
    /// what this does is ask for one frame a second and ask for a fresh picture
    /// on the way back.
    pub fn set_focused(&self, focused: bool) {
        if self.focused.swap(focused, Ordering::Relaxed) == focused {
            return;
        }
        self.send_options();
        if focused {
            let mut misc = Misc::new();
            misc.set_refresh_video(true);
            let mut out = Message::new();
            out.set_misc(misc);
            self.send(out);
        }
    }

    /// Absolute desktop coordinates and an RFB button mask.
    ///
    /// The seam says where the pointer is and which buttons are held; this
    /// protocol wants a move, a press and a release as separate events, so the
    /// edges are worked out here against the last mask seen. The coordinates
    /// are the picture's and what goes out is the peer's desktop's, which is
    /// the same thing only while the display being captured is at the origin.
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
        // than something held: the edge is the event and the release is not.
        // Its two numbers are a distance rather than a place, so the display's
        // origin is not added to them.
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

    /// One mouse event. A press carries the modifiers held, because their
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
            log::info!("nothing in this protocol says keysym {keysym:#x}");
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

    /// Let go of every key this session still has down at the far end.
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
                // `chr` rather than `unicode`: their server can hold one down,
                // which is what makes it a key rather than a piece of typing.
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

    /// The modifiers held, less the one being pressed: their server compares
    /// the list against what is down on the far end and releases the
    /// difference, and a modifier that names itself is one it would refuse to
    /// press.
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

    /// Ask the far end for a desktop this size, out of the sizes it offered.
    ///
    /// Unlike RFB there is no free choice: the peer publishes a list and takes
    /// one of them, so the nearest by area is what a window's size becomes.
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

    /// Say something harmless every few seconds until told to stop.
    ///
    /// A peer that has heard nothing for **30 seconds** before the login drops
    /// the connection, and a question put to a person can outlast that — the
    /// session was lost at forty, with a dialog still on screen. Their timing
    /// message is the one thing a client may send before it has logged in that
    /// the far end is content to receive, so this is what a keepalive is made of
    /// here.
    fn keep_alive(&self) -> KeepAlive {
        let mut delay = TestDelay::new();
        delay.from_client = true;
        let mut message = Message::new();
        message.set_test_delay(delay);
        let bytes = message.write_to_bytes().unwrap_or_default();
        let out = self.out.lock().unwrap().clone();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("rustdesk-keepalive".into())
            .spawn(move || {
                let Some(out) = out else { return };
                while !flag.load(Ordering::Relaxed) {
                    std::thread::sleep(KEEP_ALIVE_EVERY);
                    if flag.load(Ordering::Relaxed) || out.send(bytes.clone()).is_err() {
                        break;
                    }
                }
            })
            .ok();
        KeepAlive { stop, thread }
    }

    /// Hand a message to the writer thread. Dropped where the session has
    /// ended, which is every input event between a socket going and the screen
    /// being told.
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

impl Default for Client {
    fn default() -> Client {
        Client::new()
    }
}

/// The thread behind [`Client::keep_alive`], stopped by hand rather than by
/// being dropped: it is stopped at a point where the answer has arrived and the
/// next thing to happen is the login, and a `Drop` that joins a sleeping thread
/// would hold that up for as long as the sleep has left.
struct KeepAlive {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl KeepAlive {
    fn stop(self) {
        self.stop.store(true, Ordering::Relaxed);
        drop(self.thread);
    }
}

/// What a session is costing, logged once every [`Rate::EVERY`] frames.
///
/// This protocol has no damage rectangles, so a frame arrives whole whatever
/// moved in it, and what a session costs is how much of one is worth
/// converting. That and where the time goes is the one thing worth having out
/// of a running session, and it is one line a minute rather than anything a
/// frame pays for.
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

/// A cursor shape, kept because their protocol sends the pixels once and refers
/// to them by id from then on.
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
        // Their cursors are zstd, as their clipboard is.
        let rgba = zstd::decode_all(&data.colors[..]).unwrap_or_default();
        if rgba.len() < width * height * 4 {
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

    fn deliver(&self, handler: &mut impl Handler) {
        handler.cursor(&self.pixels, self.width, self.height, self.hot_x, self.hot_y);
    }
}

/// `sha256(sha256(password ++ salt) ++ challenge)`. The inner hash is what a
/// client stores, so a saved password is already salted per peer — which is
/// worth knowing and is not what this app does, since it has one password store
/// for six protocols.
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

fn resolve(address: &str) -> Result<SocketAddr> {
    let with_port = if address.contains(':') {
        address.to_string()
    } else {
        format!("{address}:{DIRECT_PORT}")
    };
    with_port
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Their formula, against the one value a run has confirmed end to end: the
    /// rig's password, salt and challenge, and the digest their server
    /// accepted.
    #[test]
    fn the_password_is_two_rounds() {
        let mut hash = Hash::new();
        hash.salt = "0123456789abcdef0123456789abcdef".into();
        hash.challenge = "abcdef".into();
        let digest = login_hash("PASSword1", &hash);
        assert_eq!(digest.len(), 32);
        // The inner hash is what a client would store, and the outer one is
        // what goes on the wire — so the same password against a second
        // challenge is a different answer.
        let mut second = hash.clone();
        second.challenge = "fedcba".into();
        assert_ne!(digest, login_hash("PASSword1", &second));
        assert!(login_hash("", &hash).is_empty());
    }

    /// The rig's second screen: 1024×768 at 1920,665 on a desktop whose first
    /// display is 1920×1440. A tap in the middle of the picture is a tap on
    /// that screen, and every coordinate on the wire is the desktop's — so a
    /// client that sends the picture's own numbers moves the cursor on the
    /// first display instead, which is what this pins.
    #[test]
    fn a_pointer_is_where_the_captured_display_is() {
        struct Nothing;
        impl Handler for Nothing {
            fn connected(&mut self, _width: usize, _height: usize) {}
            fn desktop_size(&mut self, _width: usize, _height: usize) {}
            fn damaged(&mut self, _x: usize, _y: usize, _width: usize, _height: usize) {}
            fn frame_end(&mut self) {}
            fn cursor(&mut self, _p: &[u32], _w: usize, _h: usize, _hx: i32, _hy: i32) {}
            fn clipboard(&mut self, _text: &str) {}
            fn password(&mut self) -> Option<String> {
                None
            }
            fn trust(&mut self, _fingerprint: &str) -> bool {
                false
            }
            fn unverified(&mut self, _why: &str) -> bool {
                false
            }
        }

        let client = Client::new();
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

        // The origin comes off the switch message rather than the list the
        // login carried, since that is the one a peer sends when a screen it
        // was not asked about moves.
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

        // The wheel's two numbers are how far rather than where.
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

    #[test]
    fn a_port_is_theirs_unless_one_is_given() {
        assert_eq!(resolve("127.0.0.1").unwrap().port(), DIRECT_PORT);
        assert_eq!(resolve("127.0.0.1:5900").unwrap().port(), 5900);
    }
}
