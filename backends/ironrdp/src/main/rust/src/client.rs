//! The session: one connection, a protocol thread and a writer thread.
//!
//! Deliberately the same shape as `rfb::Client`, because `Backend` is what both
//! have to fit and a second implementation that needed a different shape would
//! have been telling us something about the seam. The differences are all
//! forced by the protocol, and each one is marked where it happens.

use crate::clipboard::{Clipboard, Want};
use crate::error::{Error, Result, describe};
use crate::framebuffer::Framebuffer;
use crate::keymap::{self, Key};
use crate::tls;

use ironrdp_blocking::{Framed, connect_begin, connect_finalize, mark_as_upgraded};
use ironrdp_cliprdr::CliprdrClient;
use ironrdp_cliprdr::pdu::OwnedFormatDataResponse;
use ironrdp_connector::connection_activation::{ConnectionActivationSequence, ConnectionActivationState};
use ironrdp_connector::sspi::network_client::NetworkClient;
use ironrdp_connector::{
    ClientConnector, ConnectionResult, DesktopSize, Sequence as _, ServerName,
};
use ironrdp_core::WriteBuf;
use ironrdp_displaycontrol::client::DisplayControlClient;
use ironrdp_displaycontrol::pdu::{DisplayControlMonitorLayout, DisplayControlPdu, MonitorLayoutEntry};
use ironrdp_dvc::{DrdynvcClient, encode_dvc_messages};
use ironrdp_svc::ChannelFlags;
use ironrdp_input::{Database, MouseButton, MousePosition, Operation, Scancode, WheelRotations};
use ironrdp_pdu::input::fast_path::{FastPathInput, FastPathInputEvent, KeyboardFlags};
use ironrdp_pdu::rdp::capability_sets::{InputFlags, client_codecs_capabilities};
use ironrdp_pdu::rdp::client_info::CompressionType;
use ironrdp_pdu::rdp::headers::ShareDataPdu;
use ironrdp_pdu::rdp::suppress_output::SuppressOutputPdu;
use ironrdp_pdu::geometry::InclusiveRectangle;
use ironrdp_session::{ActiveStage, ActiveStageBuilder, ActiveStageOutput};

use std::collections::HashMap;
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

/// How often the protocol thread comes up for air when the desktop is silent.
///
/// Everything a person does goes out on the writer thread and is not affected
/// by this; what waits for it is the handful of things that have to be encoded
/// by the active stage — the pause, the clipboard, a redraw — because that
/// object lives on this thread and is not `Sync`. A quarter of a second is
/// invisible on all three and costs four wakeups a second on an idle session.
const POLL: Duration = Duration::from_millis(250);

/// A wheel notch. RDP counts wheel rotation in 120ths of a click, as Windows
/// does everywhere; RFB counts clicks, as button presses. The whole of that
/// mismatch is this constant and the edge detection in [`Client::pointer`].
const WHEEL_UNITS: i16 = 120;

/// What a connection is told before it starts.
pub struct Config {
    /// `host` or `host:port`; 3389 when there is no port.
    pub address: String,
    pub user_name: Option<String>,
    /// RDP's third credential, which RFB has no notion of. Empty is normal —
    /// a local account on a machine that is not in a domain.
    pub domain: Option<String>,
    pub password: Option<String>,
    /// What to ask the server for. RDP negotiates the desktop size from the
    /// *client*, which is the opposite of RFB, where the desktop is whatever
    /// size it already was.
    pub desktop_size: (u16, u16),
    /// How much authentication to insist on before the session starts.
    pub nla: Nla,
    /// Bulk compression of the stream that is not graphics — input echoes,
    /// channel data, the pointer. `None` tells the server not to, which is
    /// what a client that cannot decompress has to say.
    pub compression: Option<CompressionType>,
    /// Whether to offer RemoteFX. Off leaves the server with plain bitmap
    /// updates, which is a different picture rather than a slower one.
    pub remote_fx: bool,
    /// What the remote machine may spend bandwidth drawing.
    pub experience: Experience,
    /// How many monitors to ask the far end to make, each of
    /// [`desktop_size`](Config::desktop_size), side by side. One is a plain
    /// session and asks for nothing.
    pub monitors: u8,
    /// What the far end lists this connection as.
    pub client_name: String,
    /// The layout the *server* should assume. `0x0409` is US English; it does
    /// not change what this client sends, only how the server reads it (see
    /// `keymap`).
    pub keyboard_layout: u32,
    pub connect_timeout: Duration,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            address: String::new(),
            user_name: None,
            domain: None,
            password: None,
            desktop_size: (1920, 1200),
            nla: Nla::Prefer,
            compression: None,
            remote_fx: true,
            experience: Experience::Balanced,
            monitors: 1,
            client_name: "remotedesktop".to_string(),
            keyboard_layout: 0x0409,
            // The same twenty seconds both other backends wait: a mistyped
            // address must not be a spinner that never stops.
            connect_timeout: Duration::from_secs(20),
        }
    }
}

/// Network Level Authentication, i.e. CredSSP: the credentials are checked
/// before a session exists at all, rather than by a login screen drawn inside
/// one.
///
/// The three cases are the same shape as the RFB client's Encryption option,
/// and for the same reason — a security choice with a default that works is
/// worth having, and a way to insist is worth having separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nla {
    /// Offer both, let the server pick. Windows takes NLA; xrdp, which has
    /// none, takes TLS and shows its own login window.
    Prefer,
    /// Offer only NLA. A server without it is refused rather than reached
    /// through a login screen that the whole connection sequence has already
    /// been opened to.
    Require,
    /// Offer only TLS. For a server whose CredSSP does not work — and it is a
    /// real category, because CredSSP is where third-party servers most often
    /// differ from Windows.
    Off,
}

/// How much of the remote machine's own decoration is worth the bandwidth.
///
/// RDP asks this as a set of "disable that" flags plus a connection-type hint
/// the server uses to pick its own defaults, which is the Experience tab mstsc
/// has had since 2001. They move together, so they are one setting here: a
/// person choosing between them is choosing how plain the desktop may look,
/// not which nine flags to send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Experience {
    /// Everything the machine would draw for someone sitting at it.
    Full,
    /// IronRDP's own default, and mstsc's for a LAN: no window contents while
    /// dragging and no menu fades, both of which are animation rather than
    /// picture.
    Balanced,
    /// Wallpaper, themes and cursor decoration off as well, and the link
    /// described as a modem so the server tunes for one.
    Plain,
}

/// The experience option by the name the row offering it uses.
pub fn experience(value: Option<&str>) -> Experience {
    match value {
        Some("full") => Experience::Full,
        Some("plain") => Experience::Plain,
        _ => Experience::Balanced,
    }
}

/// Bulk compression, by the names the option offering it uses. The protocol's
/// four levels are historical rather than a trade-off — 6.1 is what every
/// current client asks for — so anything unrecognised, including nothing, is
/// off.
pub fn compression(value: Option<&str>) -> Option<CompressionType> {
    match value {
        Some("rdp61") => Some(CompressionType::Rdp61),
        Some("rdp60") => Some(CompressionType::Rdp6),
        Some("64k") => Some(CompressionType::K64),
        Some("8k") => Some(CompressionType::K8),
        _ => None,
    }
}

/// What a person answered a credentials prompt with.
#[derive(Clone, Default)]
pub struct Credentials {
    pub user: String,
    pub domain: String,
    pub password: String,
}

/// Everything the session tells the app. Every method arrives on the protocol
/// thread and must be cheap; [`Handler::credentials`] and [`Handler::trust`] are
/// the exceptions and are allowed to take as long as a person does.
pub trait Handler {
    fn connected(&mut self, width: usize, height: usize);
    fn desktop_size(&mut self, width: usize, height: usize);
    fn damaged(&mut self, x: usize, y: usize, width: usize, height: usize);
    fn frame_end(&mut self);
    /// `pixels` is `width × height`, laid out as `R, G, B, A`. A zero size
    /// means the server has hidden the pointer.
    fn cursor(&mut self, pixels: &[u32], width: usize, height: usize, hot_x: i32, hot_y: i32);
    fn bell(&mut self);
    fn clipboard(&mut self, text: &str);
    /// The connection needs credentials and the config had none. `None` gives
    /// up on the connection.
    ///
    /// Unlike RFB, this is asked **before** anything is connected: RDP carries
    /// the user name and password in the connection sequence rather than being
    /// asked for them by the server, so there is nothing to wait for.
    fn credentials(&mut self, needs_user: bool) -> Option<Credentials>;
    /// The server's certificate, by fingerprint, after the handshake and before
    /// anything secret is sent. `false` ends the connection.
    fn trust(&mut self, fingerprint: &str) -> bool;
}

/// What the connection panel can be told, as strings, because that is what it
/// puts on screen. There is no desktop name and no encoding: RDP has neither
/// concept, so those rows are absent from this list rather than empty in it.
#[derive(Clone, Default)]
pub struct Info {
    pub protocol: String,
    pub connection: String,
    pub security: String,
    pub line_speed: String,
    pub server_pixels: String,
    pub viewer_pixels: String,
}

/// One outbound frame, already encoded. Encoding on the caller's thread and
/// writing on one of our own is what keeps a stalled socket off the main
/// thread — the app calls [`Client::pointer`] from wherever a finger moved.
enum Out {
    Bytes(Vec<u8>),
    Stop,
}

/// Something only the active stage can encode, so it waits for the protocol
/// thread to come up for air.
enum Task {
    Focus(bool),
    /// A monitor layout to put on the display-control channel: one entry per
    /// monitor, already laid out. Both a resize and a change of monitor count
    /// are this — MS-RDPEDISP has one message and the layout is the whole of it.
    Layout(Vec<MonitorLayoutEntry>),
}

/// A monitor layout that has been asked for, and what the desktop would have to
/// become for it to have happened.
#[derive(Default)]
struct Layout {
    /// Four ints per monitor — x, y, width, height — as the seam wants them.
    monitors: Vec<i32>,
    /// The bounding box of those monitors. A desktop of any other size means
    /// the server did something else with the layout, and the monitors are then
    /// not something this end can describe.
    desktop: (u16, u16),
}

/// A key that is down, and what it took to press it.
struct Held {
    key: Key,
    /// Whether *we* pressed Shift to get it, as opposed to the person holding
    /// Shift on the extension keyboard. Only what we pressed do we release.
    synthetic_shift: bool,
}

struct Shared {
    framebuffer: RwLock<Framebuffer>,
    out: Sender<Out>,
    tasks: Mutex<VecDeque<Task>>,
    socket: Mutex<Option<TcpStream>>,
    info: Mutex<Info>,
    /// The keyboard and mouse state IronRDP keeps so that a redundant event is
    /// not sent twice. Locked on whichever thread the input arrived on.
    input: Mutex<Database>,
    /// keyId → what went down, so a release names the key its press named.
    held: Mutex<HashMap<i64, Held>>,
    /// The RFB button mask as it was last seen, because RDP wants the *edges*
    /// and the app sends the level.
    ///
    /// Nine bits, not eight: a physical mouse has back and forward buttons, and
    /// RDP has somewhere to put them. RFB's own mask is one byte and cannot
    /// carry the ninth.
    buttons: Mutex<u16>,
    running: AtomicBool,
    closing: AtomicBool,
    view_only: AtomicBool,
    /// The display-control channel is open and its capabilities have arrived,
    /// which is the whole of "can this desktop be reshaped". Nothing before
    /// that point may be sent on it.
    display_control: AtomicBool,
    /// How many monitors this session asks for. Not read from the config after
    /// the connection because a resize rebuilds the layout and has to keep the
    /// count.
    monitor_count: AtomicU8,
    /// The layout that was asked for and the desktop size that would prove it
    /// was granted. Reported as the monitors only once the desktop has become
    /// that size — the server may make something else of a layout, and there is
    /// nothing on the wire that says what it made.
    layout: Mutex<Layout>,
    /// What the session has moved, both directions, since the socket was
    /// opened. Protocol bytes: this counts what goes into and comes out of the
    /// TLS session rather than what the link carried, so there is no record
    /// framing in it and no TCP or IP headers.
    bytes_in: Arc<AtomicU64>,
    bytes_out: Arc<AtomicU64>,
    /// Shared with the CLIPRDR channel, which the active stage owns.
    clipboard: Arc<Clipboard>,
}

/// A connection. Created first, connected second: the framebuffer exists from
/// the start so the drawing thread has something to lock, and input sent before
/// the session is up is dropped rather than queued.
pub struct Client {
    shared: Arc<Shared>,
    rx: Mutex<Option<Receiver<Out>>>,
}

impl Client {
    pub fn new() -> Client {
        let (tx, rx) = channel();
        Client {
            shared: Arc::new(Shared {
                framebuffer: RwLock::new(Framebuffer::new()),
                out: tx,
                tasks: Mutex::new(VecDeque::new()),
                socket: Mutex::new(None),
                info: Mutex::new(Info::default()),
                input: Mutex::new(Database::new()),
                held: Mutex::new(HashMap::new()),
                buttons: Mutex::new(0),
                running: AtomicBool::new(false),
                closing: AtomicBool::new(false),
                view_only: AtomicBool::new(false),
                display_control: AtomicBool::new(false),
                monitor_count: AtomicU8::new(1),
                layout: Mutex::new(Layout::default()),
                bytes_in: Arc::new(AtomicU64::new(0)),
                bytes_out: Arc::new(AtomicU64::new(0)),
                clipboard: Arc::new(Clipboard::default()),
            }),
            rx: Mutex::new(Some(rx)),
        }
    }

    /// The desktop, for the drawing thread.
    pub fn framebuffer(&self) -> &RwLock<Framebuffer> {
        &self.shared.framebuffer
    }

    pub fn info(&self) -> Info {
        self.shared.info.lock().unwrap().clone()
    }

    /// Received and sent, in bytes, since the socket was opened.
    pub fn traffic(&self) -> (u64, u64) {
        (
            self.shared.bytes_in.load(Ordering::Relaxed),
            self.shared.bytes_out.load(Ordering::Relaxed),
        )
    }

    pub fn is_running(&self) -> bool {
        self.shared.running.load(Ordering::Acquire)
    }

    // ---- input, callable from any thread ----------------------------------

    /// An absolute position and an RFB button mask, which is what the whole
    /// control stack speaks (`control.input.Button`).
    ///
    /// Three of those bits are buttons and four are wheel notches, and RDP has
    /// nothing pseudo about its wheel — so the buttons are held state and the
    /// wheel is an edge. Diffing here rather than in Java keeps the mask the
    /// one vocabulary above `Backend`.
    pub fn pointer(&self, x: u16, y: u16, mask: u16) {
        if self.shared.view_only.load(Ordering::Relaxed) {
            return;
        }
        let previous = {
            let mut buttons = self.shared.buttons.lock().unwrap();
            std::mem::replace(&mut *buttons, mask)
        };
        let mut ops = Vec::with_capacity(4);
        // The move goes first so that a click lands where the finger is: the
        // database stamps every button event with the position it holds.
        ops.push(Operation::MouseMove(MousePosition { x, y }));
        for (bit, button) in [
            (0x01, MouseButton::Left),
            (0x02, MouseButton::Middle),
            (0x04, MouseButton::Right),
            // Buttons 8 and 9. RDP calls them X1 and X2 and Windows means
            // browser back and forward by them, which is what they are.
            (0x80, MouseButton::X1),
            (0x100, MouseButton::X2),
        ] {
            let now = mask & bit != 0;
            let was = previous & bit != 0;
            if now && !was {
                ops.push(Operation::MouseButtonPressed(button));
            } else if !now && was {
                ops.push(Operation::MouseButtonReleased(button));
            }
        }
        for (bit, vertical, units) in [
            (0x08, true, WHEEL_UNITS),
            (0x10, true, -WHEEL_UNITS),
            (0x20, false, -WHEEL_UNITS),
            (0x40, false, WHEEL_UNITS),
        ] {
            if mask & bit != 0 && previous & bit == 0 {
                ops.push(Operation::WheelRotations(WheelRotations {
                    is_vertical: vertical,
                    rotation_units: units,
                }));
            }
        }
        self.apply(ops);
    }

    pub fn key_down(&self, keysym: u32, key_id: i64) {
        if self.shared.view_only.load(Ordering::Relaxed) {
            return;
        }
        let Some(key) = keymap::scancode(keysym) else {
            log::debug!("no RDP key for keysym {keysym:#x}");
            return;
        };
        let mut held = self.shared.held.lock().unwrap();
        let mut ops = Vec::with_capacity(2);
        // A key already down going down again is Android's auto-repeat, which
        // re-presses without releasing. What must not move is the synthetic
        // Shift: by the second press it *is* down, so asking again answers "no
        // need" and the entry that would have released it is overwritten with
        // one that will not — leaving Shift held at the far end for ever.
        let mut synthetic_shift = held.get(&key_id).is_some_and(|h| h.synthetic_shift);
        match key {
            Key::Scancode { code, shift } => {
                if shift && !synthetic_shift && !self.shift_is_down() {
                    synthetic_shift = true;
                    ops.push(Operation::KeyPressed(SHIFT));
                }
                ops.push(Operation::KeyPressed(code));
            }
            Key::Unicode(ch) => ops.push(Operation::UnicodeKeyPressed(ch)),
            Key::Pause => {}
        }
        held.insert(
            key_id,
            Held {
                key,
                synthetic_shift,
            },
        );
        drop(held);
        if key == Key::Pause {
            self.send_pause();
        } else {
            self.apply(ops);
        }
    }

    pub fn key_up(&self, key_id: i64) {
        if self.shared.view_only.load(Ordering::Relaxed) {
            return;
        }
        // Unknown means it was never pressed, or was released twice: sending
        // nothing is right either way.
        let Some(held) = self.shared.held.lock().unwrap().remove(&key_id) else {
            return;
        };
        self.apply(release(&held));
    }

    /// Let go of everything, for a screen going away with keys still held:
    /// "held" means held at the far end, and it stays that way until told.
    pub fn release_all_keys(&self) {
        let held: Vec<Held> = self.shared.held.lock().unwrap().drain().map(|(_, h)| h).collect();
        let mut ops = Vec::new();
        for key in &held {
            ops.extend(release(key));
        }
        self.apply(ops);
    }

    /// This phone copied something. Announced to the remote when the protocol
    /// thread next comes up for air, and handed over only if it asks.
    ///
    /// Not while view-only: the far end's clipboard is the far end's, and
    /// replacing it is driving rather than watching.
    pub fn clipboard(&self, text: &str) {
        if self.shared.view_only.load(Ordering::Relaxed) {
            return;
        }
        self.shared.clipboard.set_local(text);
    }

    pub fn set_view_only(&self, view_only: bool) {
        self.shared.view_only.store(view_only, Ordering::Relaxed);
    }

    /// Whether the far end will reshape the desktop right now.
    ///
    /// The size is settled at connect time in RDP, so this is the display
    /// control channel and nothing else: true once the server has opened it and
    /// sent its capabilities, which is after the connection and may be never —
    /// the same shape of answer as an RFB server's `ExtendedDesktopSize`, for a
    /// completely different reason.
    pub fn can_resize(&self) -> bool {
        self.shared.display_control.load(Ordering::Relaxed)
            && !self.shared.view_only.load(Ordering::Relaxed)
    }

    /// The monitors the desktop is made of, four ints each, or empty for a
    /// desktop this end cannot describe — which includes every single-monitor
    /// session, since one monitor is not a layout.
    pub fn monitors(&self) -> Vec<i32> {
        let layout = self.shared.layout.lock().unwrap();
        let (w, h) = layout.desktop;
        let fb = self.shared.framebuffer.read().unwrap();
        if layout.monitors.len() > 4 && fb.width() == usize::from(w) && fb.height() == usize::from(h) {
            layout.monitors.clone()
        } else {
            Vec::new()
        }
    }

    /// Ask for a desktop this size, which is the whole of it and not one
    /// monitor: the seam's `requestDesktopSize` means the framebuffer
    /// everywhere else, and a caller that has just read 2560×800 off the panel
    /// and sent it back must get the desktop it already has.
    ///
    /// So the monitors divide it. What came of it arrives as a reactivation
    /// carrying the desktop's new size, or as nothing at all.
    pub fn request_desktop_size(&self, width: u16, height: u16) {
        if !self.can_resize() {
            return;
        }
        let count = self.shared.monitor_count.load(Ordering::Relaxed).max(1);
        let each = width / u16::from(count).max(1);
        match monitor_layout(each, height, count) {
            Ok(entries) => self.push(Task::Layout(entries)),
            Err(e) => log::warn!(
                "{count} monitors of {each}x{height} is not a layout the protocol allows: {e}"
            ),
        }
    }

    /// How many monitors to ask for. Takes effect at the next layout, which is
    /// the next resize — a server that has already made a desktop does not have
    /// its monitors rearranged underneath somebody as a side effect of a
    /// setting being changed.
    pub fn set_monitor_count(&self, count: u8) {
        self.shared.monitor_count.store(count.clamp(1, 16), Ordering::Relaxed);
    }

    /// Whether the session is on screen.
    ///
    /// RFB's pause is "stop asking"; RDP's is the opposite, because the server
    /// sends what it likes — so this is a Suppress Output PDU, which is the
    /// protocol saying "send me nothing until I ask again".
    pub fn set_focused(&self, focused: bool) {
        self.push(Task::Focus(focused));
    }

    /// End the session. The protocol thread is blocked on a socket read, so it
    /// is woken by shutting the socket down rather than by a flag it cannot see.
    pub fn close(&self) {
        self.shared.closing.store(true, Ordering::Release);
        let _ = self.shared.out.send(Out::Stop);
        if let Some(socket) = self.shared.socket.lock().unwrap().as_ref() {
            let _ = socket.shutdown(std::net::Shutdown::Both);
        }
    }

    fn shift_is_down(&self) -> bool {
        let input = self.shared.input.lock().unwrap();
        input.is_key_pressed(SHIFT) || input.is_key_pressed(RIGHT_SHIFT)
    }

    /// Apply operations to the input state and put whatever they turn into on
    /// the wire. Operations that change nothing produce nothing, which is
    /// IronRDP's `Database` doing the job the RFB client did not need done.
    fn apply(&self, ops: Vec<Operation>) {
        if ops.is_empty() || !self.shared.running.load(Ordering::Acquire) {
            return;
        }
        // The lock is held until the frame is queued, so that the order events
        // were applied in is the order they reach the wire. Releasing it here
        // would let two callers apply a press and a release and then send them
        // the other way round, which the far end reads as a key that is still
        // down.
        let mut input = self.shared.input.lock().unwrap();
        let events = input.apply(ops);
        self.send_events(&events);
    }

    /// Pause, which is not a position and so does not go through the input
    /// database at all. The key never had a scancode of its own — a keyboard
    /// puts `E1 1D 45` on the wire for it — and what mstsc sends is a Ctrl
    /// marked `E1` around a Num Lock, which is what a Windows server is
    /// looking for. All four events are balanced, so the database it bypasses
    /// is left with nothing to disagree about.
    fn send_pause(&self) {
        if !self.shared.running.load(Ordering::Acquire) {
            return;
        }
        const CTRL: u8 = 0x1d;
        const NUM_LOCK: u8 = 0x45;
        let e1 = KeyboardFlags::EXTENDED1;
        let up = KeyboardFlags::RELEASE;
        let input = self.shared.input.lock().unwrap();
        self.send_events(&[
            FastPathInputEvent::KeyboardEvent(e1, CTRL),
            FastPathInputEvent::KeyboardEvent(KeyboardFlags::empty(), NUM_LOCK),
            FastPathInputEvent::KeyboardEvent(e1 | up, CTRL),
            FastPathInputEvent::KeyboardEvent(up, NUM_LOCK),
        ]);
        drop(input);
    }

    /// Called with the input lock held, for the ordering reason above.
    fn send_events(&self, events: &[FastPathInputEvent]) {
        if events.is_empty() {
            return;
        }
        let frame = match FastPathInput::new(events.to_vec())
            .map_err(|e| e.to_string())
            .and_then(|input| ironrdp_core::encode_vec(&input).map_err(|e| e.to_string()))
        {
            Ok(frame) => frame,
            Err(e) => {
                log::warn!("could not encode input: {e}");
                return;
            }
        };
        let _ = self.shared.out.send(Out::Bytes(frame));
    }

    fn push(&self, task: Task) {
        self.shared.tasks.lock().unwrap().push_back(task);
    }

    // ---- the session -------------------------------------------------------

    /// Connect and run until the session ends. Blocks; every [`Handler`] call
    /// happens on this thread.
    pub fn run(&self, config: &Config, handler: &mut dyn Handler) -> Result<()> {
        let result = self.session(config, handler);
        self.shared.running.store(false, Ordering::Release);
        let _ = self.shared.out.send(Out::Stop);
        if self.shared.closing.load(Ordering::Acquire) {
            return Err(Error::Closed);
        }
        result
    }

    fn session(&self, config: &Config, handler: &mut dyn Handler) -> Result<()> {
        let (host, port) = resolve(&config.address)?;

        // Asked before anything is connected, and that is the protocol rather
        // than impatience: RDP puts the credentials in the connection
        // sequence, so there is no point at which a server asks for them.
        let credentials = match stored(config) {
            Some(credentials) => credentials,
            None => handler
                .credentials(true)
                .ok_or(Error::Closed)?,
        };

        let socket = connect(&host, port, config.connect_timeout)?;
        socket.set_nodelay(true).ok();
        socket.set_read_timeout(Some(config.connect_timeout)).ok();
        *self.shared.socket.lock().unwrap() = Some(socket.try_clone()?);
        let client_addr = socket.local_addr()?;

        let mut connector = ClientConnector::new(connector_config(config, &credentials), client_addr);
        // Before the connection sequence rather than after it: the list of
        // static channels goes out in the client's settings, so a channel
        // attached later is one the server was never told about.
        connector.attach_static_channel(CliprdrClient::new(self.shared.clipboard.backend()));
        // The dynamic channel carrier, and the one dynamic channel on it.
        // Display control is how an RDP desktop is reshaped after connect —
        // and, since its message is a whole monitor layout, the only way this
        // client asks for more than one monitor at all. The callback fires when
        // the server's capabilities arrive, which is the moment the channel
        // becomes usable; it answers with no messages of its own.
        let display_control = Arc::clone(&self.shared);
        connector.attach_static_channel(DrdynvcClient::new().with_dynamic_channel(
            DisplayControlClient::new(move |caps| {
                log::info!("display control: {caps:?}");
                display_control.display_control.store(true, Ordering::Relaxed);
                Ok(Vec::new())
            }),
        ));
        self.shared
            .monitor_count
            .store(config.monitors.clamp(1, 16), Ordering::Relaxed);

        // The X.224 negotiation, in clear text: everything up to the point
        // where the server has agreed to a TLS upgrade.
        let mut framed = Framed::new(Counting::new(socket.try_clone()?, &self.shared));
        let should_upgrade = connect_begin(&mut framed, &mut connector).map_err(protocol)?;
        let leftover = framed.into_inner().1;
        if !leftover.is_empty() {
            return Err(Error::Protocol(format!(
                "{} bytes arrived before the TLS handshake",
                leftover.len()
            )));
        }

        let (wire, peer) = tls::connect(&socket, &host)?;
        // The last moment before a password goes out, and the first one on a
        // thread that is allowed to wait for a person. Same rule, same place,
        // as the RFB client's.
        if !handler.trust(&peer.fingerprint) {
            return Err(Error::Refused(
                "The server's certificate was not accepted.".into(),
            ));
        }

        let upgraded = mark_as_upgraded(should_upgrade, &mut connector);
        // After the upgrade is marked done and not before: the connector only
        // enters its CredSSP state at that point, so asking any earlier is a
        // question that always answers "no" — which is what the panel's
        // security row said on every NLA connection there has ever been.
        let credssp = connector.should_perform_credssp();
        let public_key = peer.public_key.clone();
        let mut framed = Framed::new(Counting::new(wire.clone_handle(), &self.shared));
        let mut network = NoNetworkClient;
        let result = connect_finalize(
            upgraded,
            connector,
            &mut framed,
            &mut network,
            ServerName::new(host.clone()),
            public_key,
            None,
        )
        .map_err(protocol)?;

        let desktop = result.desktop_size;
        {
            let mut info = self.shared.info.lock().unwrap();
            info.protocol = "RDP".to_string();
            info.connection = peer.description.clone();
            info.security = if credssp {
                "TLS with CredSSP (NLA)".to_string()
            } else {
                "TLS".to_string()
            };
            info.server_pixels = format!("{}×{}, 32 bpp", desktop.width, desktop.height);
            info.viewer_pixels = "32 bpp RGBA".to_string();
            info.line_speed = String::new();
        }

        // What the server will take from us, in its own words. Logged whole
        // because the interesting bit is not one we act on yet: MOUSE_RELATIVE
        // is the far end saying it would accept deltas.
        log::info!(
            "server input flags {:?} (relative pointer {})",
            result.input_flags,
            if result.input_flags.contains(InputFlags::MOUSE_RELATIVE) {
                "offered"
            } else {
                "not offered"
            }
        );

        if !result
            .input_flags
            .intersects(InputFlags::FASTPATH_INPUT | InputFlags::FASTPATH_INPUT_2)
        {
            // Not fatal, and worth saying rather than being a session in which
            // nothing a finger does has any effect.
            log::warn!("the server did not offer fast-path input; input may be ignored");
        }

        // The handshake is the only part with a deadline. Past it the poll
        // takes over, and an idle desktop is silent for as long as it likes.
        socket.set_read_timeout(Some(POLL)).ok();

        self.shared
            .framebuffer
            .write()
            .unwrap()
            .resize(desktop.width, desktop.height);
        self.shared.running.store(true, Ordering::Release);
        handler.connected(usize::from(desktop.width), usize::from(desktop.height));

        let writer = self.spawn_writer(Counting::new(wire.clone_handle(), &self.shared));
        let outcome = self.active_stage(result, &mut framed, handler);
        let _ = self.shared.out.send(Out::Stop);
        let _ = socket.shutdown(std::net::Shutdown::Both);
        let _ = writer.join();
        outcome
    }

    fn spawn_writer(&self, mut wire: Counting<tls::Handle>) -> std::thread::JoinHandle<()> {
        let rx = self.rx.lock().unwrap().take();
        std::thread::Builder::new()
            .name("rdp-write".into())
            .spawn(move || {
                let Some(rx) = rx else { return };
                while let Ok(msg) = rx.recv() {
                    match msg {
                        Out::Bytes(bytes) => {
                            if wire.write_all(&bytes).is_err() {
                                break;
                            }
                        }
                        Out::Stop => break,
                    }
                }
            })
            .expect("spawning the RDP writer thread")
    }

    fn active_stage<S: Read + Write>(
        &self,
        result: ConnectionResult,
        framed: &mut Framed<S>,
        handler: &mut dyn Handler,
    ) -> Result<()> {
        let io_channel_id = result.io_channel_id;
        let user_channel_id = result.user_channel_id;
        let refresh_rect = result.refresh_rect_support;
        let suppress_output = result.suppress_output_support;
        let activation_factory = result.activation_factory.clone();
        let mut desktop = result.desktop_size;
        let mut saw_pointer_position = false;
        let mut stage = ActiveStageBuilder {
            static_channels: result.static_channels,
            user_channel_id,
            io_channel_id,
            message_channel_id: result.message_channel_id,
            share_id: result.share_id,
            compression_type: result.compression_type,
            // The pointer is *ours* to draw: the whole control stack rests on
            // a cursor owned client-side, so the server's shape has to arrive as a bitmap rather than be
            // composited into the picture. That is exactly what turning
            // software rendering off does — with it on, `DecodedImage` draws
            // the pointer into the framebuffer and the desktop would carry a
            // second cursor around.
            enable_server_pointer: result.enable_server_pointer,
            pointer_software_rendering: false,
        }
        .build();
        let mut rate = Rate::new(Arc::clone(&self.shared.bytes_in));
        // One monitor's worth, which is what the connection sequence asked for
        // and what every monitor after the first is a copy of. Kept separately
        // because `desktop` becomes the bounding box the moment a layout is
        // granted, and a second layout built from that would double again.
        let unit = result.desktop_size;
        let mut asked = false;

        loop {
            // The channel is opened by the server some time after the session
            // starts, so the first layout goes out when it is ready rather than
            // at a point in the connection sequence. A single-monitor session
            // asks for nothing: it already has what it asked for.
            if !asked
                && self.shared.monitor_count.load(Ordering::Relaxed) > 1
                && stage.display_control_ready() == Some(true)
            {
                asked = true;
                let count = self.shared.monitor_count.load(Ordering::Relaxed);
                match monitor_layout(unit.width, unit.height, count) {
                    Ok(entries) => self.send_layout(framed, &mut stage, entries)?,
                    Err(e) => log::warn!("{count} monitors of {}x{} is not a layout the protocol allows: {e}", unit.width, unit.height),
                }
            }
            match framed.read_pdu() {
                Ok((action, payload)) => {
                    let outputs = {
                        let mut framebuffer = self.shared.framebuffer.write().unwrap();
                        let image = framebuffer
                            .image_mut()
                            .ok_or_else(|| Error::Protocol("no framebuffer".into()))?;
                        // The write lock is held for the decode, not just for
                        // a blit as in the RFB client: IronRDP's decoders write
                        // straight into the destination, so there is no scratch
                        // copy to make this cheaper and no reason to add one.
                        stage.process(image, action, &payload).map_err(session)?
                    };
                    let mut drew = false;
                    for output in outputs {
                        match output {
                            ActiveStageOutput::ResponseFrame(frame) => {
                                framed.write_all(&frame)?;
                            }
                            ActiveStageOutput::GraphicsUpdate(rect) => {
                                let (x, y, w, h) = rectangle(&rect);
                                handler.damaged(x, y, w, h);
                                drew = true;
                            }
                            ActiveStageOutput::PointerBitmap(pointer) => {
                                let pixels = words(&pointer.bitmap_data);
                                handler.cursor(
                                    &pixels,
                                    usize::from(pointer.width),
                                    usize::from(pointer.height),
                                    i32::from(pointer.hotspot_x),
                                    i32::from(pointer.hotspot_y),
                                );
                            }
                            ActiveStageOutput::PointerHidden => {
                                handler.cursor(&[], 0, 0, 0, 0);
                            }
                            ActiveStageOutput::PointerPosition { x, y } => {
                                // The far end saying where it has put the
                                // cursor. Nothing here takes instruction from
                                // it — the cursor is owned at the phone and the
                                // viewport follows it — but whether a server
                                // sends these at all is a fact about the
                                // session worth one line, so the first is
                                // logged and the stream after it is not.
                                if !saw_pointer_position {
                                    saw_pointer_position = true;
                                    log::info!(
                                        "server moved its pointer to {x},{y} \
                                         (the first of this session)"
                                    );
                                }
                            }
                            ActiveStageOutput::PointerDefault => {
                                // "The system arrow", which nobody sends us the
                                // pixels of. Keeping the last shape is better
                                // than hiding the cursor, which is what an
                                // empty bitmap would do.
                                log::debug!("the server asked for the default pointer");
                            }
                            ActiveStageOutput::Terminate(reason) => {
                                return Err(Error::Protocol(reason.description()));
                            }
                            ActiveStageOutput::DeactivateAll => {
                                // Logged at info because it is where a session
                                // stops if it stops: a Windows host sends one
                                // when the logon desktop hands over to the
                                // shell, and everything after it depends on the
                                // sequence below completing.
                                log::info!("server deactivated: reactivating");
                                let mut sequence = activation_factory.create();
                                let size = self.reactivate(
                                    framed,
                                    &mut sequence,
                                    &mut stage,
                                    io_channel_id,
                                    user_channel_id,
                                )?;
                                if size != desktop {
                                    desktop = size;
                                    self.shared
                                        .framebuffer
                                        .write()
                                        .unwrap()
                                        .resize(size.width, size.height);
                                    self.shared.info.lock().unwrap().server_pixels =
                                        format!("{}×{}, 32 bpp", size.width, size.height);
                                    handler.desktop_size(
                                        usize::from(size.width),
                                        usize::from(size.height),
                                    );
                                }
                                log::info!("reactivated: {}×{}", desktop.width, desktop.height);
                                self.request_redraw(
                                    framed,
                                    &stage,
                                    desktop,
                                    refresh_rect,
                                    suppress_output,
                                )?;
                            }
                            _ => {}
                        }
                    }
                    if drew {
                        if let Some(speed) = rate.sample() {
                            self.shared.info.lock().unwrap().line_speed = speed;
                        }
                        handler.frame_end();
                    }
                }
                Err(e) if timed_out(&e) => {}
                Err(e) => return Err(Error::Io(e)),
            }

            // The clipboard, in both directions. What the channel decided while
            // decoding is a `Want` because the channel object is inside the
            // active stage and cannot encode from where it is called.
            for want in self.shared.clipboard.take_wants() {
                let messages = {
                    let Some(cliprdr) = stage.get_svc_processor_mut::<CliprdrClient>() else {
                        // The server refused the channel, so nothing about the
                        // clipboard is going to work this session.
                        break;
                    };
                    match want {
                        Want::Advertise => {
                            cliprdr.initiate_copy(&self.shared.clipboard.formats())
                        }
                        Want::Paste(format) => cliprdr.initiate_paste(format),
                        Want::Send(format) => cliprdr.submit_format_data(
                            match self.shared.clipboard.data(format) {
                                Some(data) => OwnedFormatDataResponse::new_data(data),
                                None => OwnedFormatDataResponse::new_error(),
                            },
                        ),
                    }
                };
                match messages {
                    // Not fatal: a clipboard that will not go across is worth
                    // less than the session it would take down with it.
                    Err(e) => log::warn!("clipboard: {e}"),
                    Ok(messages) => {
                        let frame = stage
                            .process_svc_processor_messages(messages)
                            .map_err(session)?;
                        framed.write_all(&frame)?;
                    }
                }
            }
            for text in self.shared.clipboard.take_incoming() {
                handler.clipboard(&text);
            }

            // Everything that has to be encoded by the active stage, which
            // lives on this thread and nowhere else.
            let tasks: Vec<Task> = self.shared.tasks.lock().unwrap().drain(..).collect();
            for task in tasks {
                match task {
                    Task::Focus(focused) => {
                        let mut frame = WriteBuf::new();
                        stage
                            .encode_static(
                                &mut frame,
                                ShareDataPdu::SuppressOutput(SuppressOutputPdu {
                                    desktop_rect: focused.then(|| whole(desktop)),
                                }),
                            )
                            .map_err(session)?;
                        framed.write_all(frame.filled())?;
                        if focused {
                            // Nothing repaints on its own after a pause: the
                            // server sent nothing while output was suppressed
                            // and has no idea what we missed.
                            self.request_redraw(
                                framed,
                                &stage,
                                desktop,
                                refresh_rect,
                                suppress_output,
                            )?;
                        }
                    }
                    Task::Layout(entries) => {
                        self.send_layout(framed, &mut stage, entries)?;
                    }
                }
            }
        }
    }

    /// Put a monitor layout on the display-control channel, and remember what
    /// was asked for so that the desktop arriving at that size is what proves
    /// it happened.
    ///
    /// A refusal is silence: the server either reactivates with a new desktop
    /// or does nothing at all, and there is no message that says no.
    fn send_layout<S: Read + Write>(
        &self,
        framed: &mut Framed<S>,
        stage: &mut ActiveStage,
        entries: Vec<MonitorLayoutEntry>,
    ) -> Result<()> {
        let Some(dvc) = stage.get_dvc::<DisplayControlClient>() else {
            log::warn!("the server did not open the display control channel");
            return Ok(());
        };
        let channel_id = dvc.channel_id();
        if !dvc.processor().ready() {
            return Ok(());
        }
        let mut flat = Vec::with_capacity(entries.len() * 4);
        let mut desktop = (0u32, 0u32);
        for entry in &entries {
            let (x, y) = entry.position().unwrap_or((0, 0));
            let (w, h) = entry.dimensions();
            flat.extend_from_slice(&[x, y, w as i32, h as i32]);
            desktop.0 = desktop.0.max(x as u32 + w);
            desktop.1 = desktop.1.max(y as u32 + h);
        }
        let pdu: DisplayControlPdu = DisplayControlMonitorLayout::new(&entries)
            .map_err(|e| Error::Protocol(e.to_string()))?
            .into();
        let messages = encode_dvc_messages(channel_id, vec![Box::new(pdu)], ChannelFlags::empty())
            .map_err(|e| Error::Protocol(e.to_string()))?;
        let frame = stage.encode_dvc_messages(messages).map_err(session)?;
        framed.write_all(&frame)?;
        *self.shared.layout.lock().unwrap() = Layout {
            monitors: flat,
            desktop: (desktop.0.min(0xffff) as u16, desktop.1.min(0xffff) as u16),
        };
        Ok(())
    }

    /// Ask for the whole desktop again, by whichever of the two PDUs the server
    /// said it would accept.
    fn request_redraw<S: Read + Write>(
        &self,
        framed: &mut Framed<S>,
        stage: &ActiveStage,
        desktop: DesktopSize,
        refresh_rect: bool,
        suppress_output: bool,
    ) -> Result<()> {
        for frame in stage
            .request_full_redraw(desktop.width, desktop.height, refresh_rect, suppress_output)
            .map_err(session)?
        {
            framed.write_all(&frame)?;
        }
        Ok(())
    }

    /// The Deactivation-Reactivation Sequence: the server tears the session's
    /// capabilities down and builds them again, which is how RDP resizes a
    /// desktop. Nothing above notices unless the size changed.
    fn reactivate<S: Read + Write>(
        &self,
        framed: &mut Framed<S>,
        sequence: &mut ConnectionActivationSequence,
        stage: &mut ActiveStage,
        io_channel_id: u16,
        user_channel_id: u16,
    ) -> Result<DesktopSize> {
        let mut buf = WriteBuf::new();
        loop {
            step(framed, sequence, &mut buf)?;
            if let ConnectionActivationState::Finalized {
                desktop_size,
                share_id,
                enable_server_pointer,
                ..
            } = *ironrdp_connector::state_downcast::<ConnectionActivationState>(sequence.state())
                .ok_or_else(|| Error::Protocol("reactivation lost its state".into()))?
            {
                stage.reactivate(
                    io_channel_id,
                    user_channel_id,
                    share_id,
                    enable_server_pointer,
                    false,
                );
                return Ok(desktop_size);
            }
        }
    }
}

/// One step of a connection sequence: read what it is waiting for, hand it
/// over, write whatever it produced.
///
/// `ironrdp_blocking::single_sequence_step` is this, but only for
/// `ClientConnector`; reactivation drives a different sequence through the same
/// trait.
fn step<S: Read + Write>(
    framed: &mut Framed<S>,
    sequence: &mut ConnectionActivationSequence,
    buf: &mut WriteBuf,
) -> Result<()> {
    buf.clear();
    let written = match sequence.next_pdu_hint() {
        Some(hint) => {
            let pdu = framed.read_by_hint(hint)?;
            sequence.step(&pdu, buf).map_err(protocol)?
        }
        None => sequence.step_no_input(buf).map_err(protocol)?,
    };
    if let Some(len) = written.size() {
        framed.write_all(&buf[..len])?;
    }
    Ok(())
}

fn release(held: &Held) -> Vec<Operation> {
    let mut ops = Vec::with_capacity(2);
    match held.key {
        Key::Scancode { code, .. } => {
            ops.push(Operation::KeyReleased(code));
            if held.synthetic_shift {
                ops.push(Operation::KeyReleased(SHIFT));
            }
        }
        Key::Unicode(ch) => ops.push(Operation::UnicodeKeyReleased(ch)),
        // Its release went out with its press, since the sequence is one thing.
        Key::Pause => {}
    }
    ops
}

const SHIFT: Scancode = Scancode::from_u8(false, 0x2a);
const RIGHT_SHIFT: Scancode = Scancode::from_u8(false, 0x36);

/// `R, G, B, A` bytes as words, which is what the bindings hand to Java.
fn words(bytes: &[u8]) -> Vec<u32> {
    bytes
        .chunks_exact(4)
        .map(|px| u32::from_le_bytes([px[0], px[1], px[2], px[3]]))
        .collect()
}

/// An RDP rectangle is inclusive at both ends; everything above `Backend` uses
/// a width and a height.
fn rectangle(rect: &InclusiveRectangle) -> (usize, usize, usize, usize) {
    let x = usize::from(rect.left);
    let y = usize::from(rect.top);
    let w = usize::from(rect.right.saturating_sub(rect.left)) + 1;
    let h = usize::from(rect.bottom.saturating_sub(rect.top)) + 1;
    (x, y, w, h)
}

fn whole(desktop: DesktopSize) -> InclusiveRectangle {
    InclusiveRectangle {
        left: 0,
        top: 0,
        right: desktop.width.saturating_sub(1),
        bottom: desktop.height.saturating_sub(1),
    }
}

fn timed_out(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    )
}

fn protocol(e: ironrdp_connector::ConnectorError) -> Error {
    Error::Protocol(describe(&e))
}

fn session(e: ironrdp_session::SessionError) -> Error {
    Error::Protocol(describe(&e))
}

fn stored(config: &Config) -> Option<Credentials> {
    let user = config.user_name.clone().filter(|u| !u.is_empty())?;
    let password = config.password.clone().filter(|p| !p.is_empty())?;
    Some(Credentials {
        user,
        domain: config.domain.clone().unwrap_or_default(),
        password,
    })
}

/// A stream that counts both ways, for the line-speed readout and for what the
/// panel says the session has moved. It wraps whatever the connection is at the
/// time — the bare socket for the X.224 exchange, the TLS session for
/// everything after it — so what it counts is the protocol rather than the
/// link.
///
/// Three of them per session, because RDP is read and written from three
/// places: the negotiation before the upgrade, the connection sequence and the
/// active stage over one `Framed`, and the writer thread over a handle of its
/// own. A counter on any one of them alone would be missing a stage.
struct Counting<S> {
    inner: S,
    read: Arc<AtomicU64>,
    write: Arc<AtomicU64>,
}

impl<S> Counting<S> {
    fn new(inner: S, shared: &Shared) -> Counting<S> {
        Counting {
            inner,
            read: Arc::clone(&shared.bytes_in),
            write: Arc::clone(&shared.bytes_out),
        }
    }
}

impl<R: Read> Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

impl<W: Write> Write for Counting<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.write.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// CredSSP asks for one of these so that Kerberos can reach a KDC. Nothing here
/// speaks Kerberos — NTLM is what a phone has, and it needs no third party — so
/// this exists to say so rather than to pull an HTTP client into the app.
#[derive(Debug)]
struct NoNetworkClient;

impl NetworkClient for NoNetworkClient {
    fn send(
        &self,
        _request: &ironrdp_connector::sspi::generator::NetworkRequest,
    ) -> ironrdp_connector::sspi::Result<Vec<u8>> {
        Err(ironrdp_connector::sspi::Error::new(
            ironrdp_connector::sspi::ErrorKind::UnsupportedFunction,
            "this client does not speak Kerberos; use a password",
        ))
    }
}

fn connector_config(
    config: &Config,
    credentials: &Credentials,
) -> ironrdp_connector::Config {
    use ironrdp_connector::{BitmapConfig, Credentials as ConnectorCredentials};
    use ironrdp_pdu::gcc::{ConnectionType, KeyboardType};
    use ironrdp_pdu::rdp::capability_sets::MajorPlatformType;
    use ironrdp_pdu::rdp::client_info::{PerformanceFlags, TimezoneInfo};

    // The one codec worth choosing about. Everything else the list can hold is
    // either always on or is IronRDP's own, which no server here speaks.
    let codecs = client_codecs_capabilities(if config.remote_fx {
        &[]
    } else {
        &["remotefx:off"]
    })
    .expect("a fixed codec list");

    ironrdp_connector::Config {
        credentials: ConnectorCredentials::UsernamePassword {
            username: credentials.user.clone(),
            password: credentials.password.clone(),
        },
        domain: Some(credentials.domain.clone()).filter(|d| !d.is_empty()),
        // The TLS upgrade is ours to do, which is what `connect_begin`
        // returning a `ShouldUpgrade` means; this flag is for a caller that
        // has no socket of its own.
        // Both by default, so the server picks: `enable_tls` is the legacy
        // "TLS, then a login screen inside the session", which is all xrdp has,
        // and `enable_credssp` is NLA. Offering only the second is how you
        // insist on it, and that is what `Nla::Require` is.
        enable_tls: config.nla != Nla::Require,
        enable_credssp: config.nla != Nla::Off,
        // RC4 with a key the protocol hands over. There is no version of it
        // worth offering, so a server that will not do TLS is refused rather
        // than downgraded — the same position the RFB client takes on
        // anonymous VeNCrypt.
        enable_standard_rdp_security: false,
        keyboard_type: KeyboardType::IbmEnhanced,
        keyboard_subtype: 0,
        keyboard_layout: config.keyboard_layout,
        keyboard_functional_keys_count: 12,
        connection_type: match config.experience {
            // A hint rather than an instruction: the flags below are what the
            // server is actually told to stop doing, and this is what it uses
            // to decide the rest for itself.
            Experience::Plain => ConnectionType::Modem,
            _ => ConnectionType::Lan,
        },
        ime_file_name: String::new(),
        dig_product_id: String::new(),
        desktop_size: DesktopSize {
            width: config.desktop_size.0,
            height: config.desktop_size.1,
        },
        bitmap: Some(BitmapConfig {
            // Both halves of "how does the picture arrive". 32 bits because
            // the framebuffer is, and anything less would be a conversion on
            // the way in for a saving the codec already makes.
            lossy_compression: false,
            color_depth: 32,
            codecs,
        }),
        client_build: 0,
        client_name: config.client_name.clone(),
        client_dir: "C:\\Windows\\System32\\mstscax.dll".to_owned(),
        platform: MajorPlatformType::ANDROID,
        // The point of the whole stack: the pointer is drawn here, at the
        // position the gesture layer believes it is at, rather than arriving a
        // round trip later inside the picture.
        enable_server_pointer: true,
        request_data: None,
        // We always have credentials — they were asked for before anything was
        // connected — so there is never a reason for the server to draw its own
        // login screen over the session it has just opened.
        autologon: true,
        enable_audio_playback: false,
        compression_type: config.compression,
        pointer_software_rendering: false,
        multitransport_flags: None,
        performance_flags: match config.experience {
            Experience::Full => {
                PerformanceFlags::ENABLE_FONT_SMOOTHING | PerformanceFlags::ENABLE_DESKTOP_COMPOSITION
            }
            Experience::Balanced => PerformanceFlags::default(),
            Experience::Plain => {
                PerformanceFlags::DISABLE_WALLPAPER
                    | PerformanceFlags::DISABLE_FULLWINDOWDRAG
                    | PerformanceFlags::DISABLE_MENUANIMATIONS
                    | PerformanceFlags::DISABLE_THEMING
                    | PerformanceFlags::DISABLE_CURSOR_SHADOW
                    | PerformanceFlags::DISABLE_CURSORSETTINGS
            }
        },
        desktop_scale_factor: 0,
        hardware_id: None,
        license_cache: None,
        timezone_info: TimezoneInfo::default(),
        alternate_shell: String::new(),
        work_dir: String::new(),
    }
}

/// A throughput estimate for the connection panel, smoothed so it reads as a
/// speed rather than as whatever the last frame happened to cost.
struct Rate {
    bytes: Arc<AtomicU64>,
    last: Instant,
    at: u64,
    bytes_per_second: f64,
}

impl Rate {
    fn new(bytes: Arc<AtomicU64>) -> Rate {
        Rate {
            at: bytes.load(Ordering::Relaxed),
            bytes,
            last: Instant::now(),
            bytes_per_second: 0.0,
        }
    }

    fn sample(&mut self) -> Option<String> {
        let now = Instant::now();
        let total = self.bytes.load(Ordering::Relaxed);
        let bytes = total.saturating_sub(self.at);
        let elapsed = now.duration_since(self.last).as_secs_f64();
        self.last = now;
        self.at = total;
        // An idle desktop produces a trickle, and dividing by the gap between
        // two of those would report it as the line's speed.
        if elapsed <= 0.0 || elapsed > 1.0 || bytes < 1024 {
            return None;
        }
        let instant = bytes as f64 / elapsed;
        self.bytes_per_second = if self.bytes_per_second == 0.0 {
            instant
        } else {
            self.bytes_per_second * 0.7 + instant * 0.3
        };
        let kbit = self.bytes_per_second * 8.0 / 1000.0;
        Some(if kbit >= 1000.0 {
            format!("{:.1} Mbit/s", kbit / 1000.0)
        } else {
            format!("{kbit:.0} kbit/s")
        })
    }
}

/// `count` monitors of `width` by `height`, side by side, the leftmost primary.
///
/// The protocol's own constraints, which the library enforces and which are the
/// reason this can fail: a monitor is 200 to 8192 pixels each way, a width may
/// not be odd, the primary is at the origin and exactly one monitor is it. The
/// size is adjusted into range rather than refused, because it comes from a
/// window whose shape is not ours to choose; the count is not, because it came
/// from somebody choosing it.
fn monitor_layout(width: u16, height: u16, count: u8) -> Result<Vec<MonitorLayoutEntry>> {
    let (w, h) = MonitorLayoutEntry::adjust_display_size(u32::from(width), u32::from(height));
    let mut entries = Vec::with_capacity(usize::from(count));
    for i in 0..u32::from(count) {
        let entry = if i == 0 {
            MonitorLayoutEntry::new_primary(w, h)
        } else {
            MonitorLayoutEntry::new_secondary(w, h)
                .and_then(|entry| entry.with_position((i * w) as i32, 0))
        };
        entries.push(entry.map_err(|e| Error::Protocol(e.to_string()))?);
    }
    Ok(entries)
}

/// `host` or `host:port`, plus the bracketed IPv6 form. No display-number
/// convention here — that is a VNC habit, and 3389 is the only port RDP has
/// ever used.
fn resolve(address: &str) -> Result<(String, u16)> {
    common::address::split(address, 3389, common::address::Ports::Plain).map_err(Error::Protocol)
}

fn connect(host: &str, port: u16, timeout: Duration) -> Result<TcpStream> {
    if host.is_empty() {
        return Err(Error::Protocol("no address".into()));
    }
    let mut last = None;
    for addr in (host, port).to_socket_addrs()? {
        match TcpStream::connect_timeout(&addr, timeout) {
            Ok(socket) => return Ok(socket),
            Err(e) => last = Some(e),
        }
    }
    Err(Error::Io(last.unwrap_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("no address for {host}"),
        )
    })))
}

impl Default for Client {
    fn default() -> Client {
        Client::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The forms are `common::address`'s; what is this client's own is the
    /// default port and the display rule being off.
    #[test]
    fn addresses() {
        assert_eq!(resolve("10.0.0.5").unwrap(), ("10.0.0.5".into(), 3389));
        assert_eq!(resolve("[::1]").unwrap(), ("::1".into(), 3389));
        assert!(resolve("2001:db8::1").is_err());
        // No display-number rule: `:1` is a port here, and taking it for 3390
        // would be borrowing a VNC habit RDP has never had.
        assert_eq!(resolve("host:1").unwrap(), ("host".into(), 1));
    }

    /// The three answers have to differ in what actually goes over the wire,
    /// and the middle one has to be what the client sent before the row
    /// existed — a setting whose default changes a connection is not a default.
    #[test]
    fn the_experience_row_reaches_the_connector() {
        use ironrdp_pdu::gcc::ConnectionType;
        use ironrdp_pdu::rdp::client_info::PerformanceFlags;

        let of = |e: Experience| {
            let config = Config {
                experience: e,
                ..Config::default()
            };
            let c = connector_config(&config, &Credentials::default());
            (c.performance_flags, c.connection_type)
        };
        assert_eq!(
            of(Experience::Balanced),
            (PerformanceFlags::default(), ConnectionType::Lan)
        );
        assert!(of(Experience::Full).0.contains(PerformanceFlags::ENABLE_DESKTOP_COMPOSITION));
        assert!(!of(Experience::Full).0.contains(PerformanceFlags::DISABLE_FULLWINDOWDRAG));
        assert!(of(Experience::Plain).0.contains(PerformanceFlags::DISABLE_WALLPAPER));
        assert_eq!(of(Experience::Plain).1, ConnectionType::Modem);
        assert_eq!(experience(Some("plain")), Experience::Plain);
        assert_eq!(experience(Some("nonsense")), Experience::Balanced);
        assert_eq!(experience(None), Experience::Balanced);
    }

    /// An inclusive rectangle of one pixel is one pixel wide, not zero.
    #[test]
    fn rectangles_are_inclusive() {
        assert_eq!(
            rectangle(&InclusiveRectangle {
                left: 4,
                top: 5,
                right: 4,
                bottom: 5
            }),
            (4, 5, 1, 1)
        );
        assert_eq!(
            rectangle(&InclusiveRectangle {
                left: 0,
                top: 0,
                right: 1919,
                bottom: 1199
            }),
            (0, 0, 1920, 1200)
        );
    }

    /// The mask is a level and the wheel is an edge: holding "wheel up" down
    /// must not scroll for ever, and letting it go must not scroll back.
    #[test]
    fn the_wheel_is_an_edge_and_the_buttons_are_not() {
        let client = Client::new();
        client.shared.running.store(true, Ordering::Release);
        client.pointer(10, 10, 0x08);
        client.pointer(10, 10, 0x08);
        client.pointer(10, 10, 0x00);
        client.pointer(10, 10, 0x01);
        client.pointer(10, 10, 0x00);

        let rx = client.rx.lock().unwrap().take().unwrap();
        let mut frames = 0;
        while let Ok(Out::Bytes(_)) = rx.try_recv() {
            frames += 1;
        }
        // move+wheel, nothing, nothing, press, release.
        assert_eq!(frames, 3, "a held wheel bit scrolled more than once");
    }

    /// A release names the key its press named, an unmatched one sends nothing,
    /// and a Shift we pressed ourselves is one we let go of.
    #[test]
    fn shift_is_synthesised_and_taken_back() {
        let client = Client::new();
        client.shared.running.store(true, Ordering::Release);
        client.key_down('!' as u32, 7);
        {
            let input = client.shared.input.lock().unwrap();
            assert!(input.is_key_pressed(SHIFT), "'!' needs Shift on this layout");
            assert!(input.is_key_pressed(Scancode::from_u8(false, 0x02)));
        }
        client.key_up(7);
        client.key_up(7);
        let input = client.shared.input.lock().unwrap();
        assert!(!input.is_key_pressed(SHIFT), "and we let go of it");
        assert!(!input.is_key_pressed(Scancode::from_u8(false, 0x02)));
    }

    /// Auto-repeat presses a key that is already down, with no release between,
    /// and the synthetic Shift has to survive it: by the second press Shift *is*
    /// down — ours — so asking again answers "no need", and the entry that would
    /// have released it would be replaced by one that never does.
    #[test]
    fn a_synthesised_shift_survives_auto_repeat() {
        let client = Client::new();
        client.shared.running.store(true, Ordering::Release);
        client.key_down('!' as u32, 7);
        client.key_down('!' as u32, 7);
        client.key_down('!' as u32, 7);
        client.key_up(7);
        let input = client.shared.input.lock().unwrap();
        assert!(!input.is_key_pressed(SHIFT), "the Shift we pressed is let go of");
    }

    /// A Shift the person is holding is not ours to release: the extension
    /// keyboard's locked modifier has to survive a shifted character going
    /// through it, which is the whole of the cross-keyboard merge.
    #[test]
    fn a_shift_somebody_else_is_holding_is_left_alone() {
        let client = Client::new();
        client.shared.running.store(true, Ordering::Release);
        client.key_down(0xffe1, 1); // Shift, from the extension keyboard
        client.key_down('A' as u32, 2);
        client.key_up(2);
        let input = client.shared.input.lock().unwrap();
        assert!(input.is_key_pressed(SHIFT), "still held by the row");
    }
}
