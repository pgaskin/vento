// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The session: one connection, a protocol thread and a writer thread.
//!
//! Deliberately structured like `rfb::Client`, since both implement `Backend`,
//! and a second implementation needing a different structure would point to
//! a problem with the interface. The differences are all forced by the
//! protocol, and each is commented where it happens.

use crate::clipboard::{Clipboard, Want};
use crate::error::{Error, Result, describe};
use crate::framebuffer::Framebuffer;
use crate::keymap::{self, Key};
use crate::pipeline::{self, State as PipelineState};
use crate::sound::{QUIET, Sound};
use crate::tls;
use runtime::media::audio::{Outputs, Player};
use runtime::media::video::Decoders;
use runtime::net::{Connection, EndReason, Liveness, Network, Peer, Timeout};
use runtime::rate::Rate;
use runtime::{Credentials, Handler, Hangup};

use ironrdp_blocking::{Framed, connect_begin, connect_finalize, mark_as_upgraded};
use ironrdp_cliprdr::CliprdrClient;
use ironrdp_cliprdr::pdu::OwnedFormatDataResponse;
use ironrdp_connector::connection_activation::{ConnectionActivationSequence, ConnectionActivationState};
use ironrdp_connector::sspi::network_client::NetworkClient;
use ironrdp_connector::{
    ClientConnector, ConnectionOutcome, ConnectionResult, DesktopSize, Sequence as _, ServerName,
};
use ironrdp_core::WriteBuf;
use ironrdp_displaycontrol::client::DisplayControlClient;
use ironrdp_displaycontrol::pdu::{DisplayControlMonitorLayout, DisplayControlPdu, MonitorLayoutEntry};
use ironrdp_dvc::{DrdynvcClient, DvcMessage, encode_dvc_messages};
use ironrdp_egfx::client::GraphicsPipelineClient;
use ironrdp_rdpdr::{NoopRdpdrBackend, Rdpdr};
use ironrdp_svc::ChannelFlags;
use ironrdp_input::{Database, MouseButton, MousePosition, Operation, Scancode, WheelRotations};
use ironrdp_pdu::input::fast_path::{FastPathInput, FastPathInputEvent, KeyboardFlags};
use ironrdp_pdu::rdp::capability_sets::{InputFlags, client_codecs_capabilities};
use ironrdp_pdu::rdp::client_info::CompressionType;
use ironrdp_pdu::rdp::headers::ShareDataPdu;
use ironrdp_pdu::rdp::suppress_output::SuppressOutputPdu;
use ironrdp_pdu::gcc::{Monitor, MonitorFlags};
use ironrdp_pdu::geometry::InclusiveRectangle;
use ironrdp_pdu::rdp::server_redirection::ServerRedirectionPdu;
use ironrdp_session::{ActiveStage, ActiveStageBuilder, ActiveStageOutput};

use std::collections::HashMap;
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

/// How often the protocol thread checks for other work when the desktop is
/// silent.
///
/// User input goes out on the writer thread and is not affected by this; what
/// waits for it is the few things the active stage has to encode (the pause,
/// the clipboard, a redraw), because that object lives on this thread and is
/// not `Sync`. A quarter of a second is unnoticeable for all three and costs
/// four wakeups a second on an idle session.
const POLL: Duration = Duration::from_millis(250);

/// A wheel notch. RDP counts wheel rotation in 120ths of a click, as Windows
/// does everywhere; RFB counts clicks, as button presses. That mismatch is
/// handled entirely by this constant and the edge detection in
/// [`Client::pointer`].
const WHEEL_UNITS: i16 = 120;

/// A connection's settings.
pub struct Config {
    /// `host` or `host:port`; 3389 when there is no port.
    pub address: String,
    pub user_name: Option<String>,
    /// RDP's third credential, which RFB lacks. Empty is normal: a local
    /// account on a machine that is not in a domain.
    pub domain: Option<String>,
    pub password: Option<String>,
    /// The desktop size to ask the server for. In RDP the *client* chooses
    /// the desktop size, the opposite of RFB, where the desktop keeps its
    /// existing size.
    pub desktop_size: (u16, u16),
    /// How much authentication to require before the session starts.
    pub nla: Nla,
    /// Bulk compression of the non-graphics stream (input echoes, channel
    /// data, the pointer). `None` tells the server not to compress, as a
    /// client that cannot decompress must.
    pub compression: Option<CompressionType>,
    /// How the picture is to arrive.
    pub graphics: Graphics,
    /// The platform's video decoders, for the graphics pipeline's H.264, or
    /// `None` for a pipeline that offers no H.264 and gets the server's other
    /// codecs.
    pub decoders: Option<Arc<dyn Decoders>>,
    /// Which visual effects the remote machine may spend bandwidth on.
    pub experience: Experience,
    /// How many monitors to ask the far end for, each of
    /// [`desktop_size`](Config::desktop_size), side by side. One is a plain
    /// session and requests no layout.
    pub monitors: u8,
    /// The name the far end shows for this connection.
    pub client_name: String,
    /// The platform this end runs on, as sent in the capabilities.
    pub platform: Platform,
    /// The layout the *server* should assume. `0x0409` is US English; it does
    /// not change what this client sends, only how the server reads it (see
    /// `keymap`).
    pub keyboard_layout: u32,
    pub connect_timeout: Duration,
    /// The output for the server's sound, or `None` for a session without
    /// sound: the server is told not to play it here, and no rdpsnd channel
    /// is offered.
    pub audio: Option<Arc<dyn Outputs>>,
    /// What the connection is made through.
    pub network: Arc<dyn Network>,
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
            graphics: Graphics::RemoteFx,
            decoders: None,
            experience: Experience::Balanced,
            monitors: 1,
            client_name: "remotedesktop".to_string(),
            platform: Platform::host(),
            keyboard_layout: 0x0409,
            // The same twenty seconds as `api::kind::CONNECT_TIMEOUT_MS`: a
            // mistyped address must not leave the connection spinning
            // indefinitely.
            connect_timeout: Duration::from_secs(20),
            audio: None,
            network: Arc::new(runtime::net::Direct),
        }
    }
}

/// The platform sent as the general capability set's major platform type:
/// `ANDROID` from the phone and `UNIX` from Linux, as FreeRDP's X11 and
/// Wayland clients and IronRDP's own viewer send. A server may log or show
/// it, so a Linux client must not report itself as a phone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Android,
    Linux,
}

impl Platform {
    /// The build target's platform, for a caller that does not specify one
    /// (the probe); the Android bindings and the Linux client each specify
    /// theirs.
    pub fn host() -> Platform {
        if cfg!(target_os = "android") {
            Platform::Android
        } else {
            Platform::Linux
        }
    }

    fn wire(self) -> ironrdp_pdu::rdp::capability_sets::MajorPlatformType {
        use ironrdp_pdu::rdp::capability_sets::MajorPlatformType;
        match self {
            Platform::Android => MajorPlatformType::ANDROID,
            Platform::Linux => MajorPlatformType::UNIX,
        }
    }
}

/// Network Level Authentication, i.e. CredSSP: the credentials are checked
/// before a session exists at all, rather than by a login screen drawn inside
/// one.
///
/// The three cases mirror the RFB client's Encryption option, for the same
/// reason: a security option with a working default is useful, and so,
/// separately, is a way to require it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nla {
    /// Offer both and let the server pick. Windows picks NLA; xrdp, which
    /// lacks it, picks TLS and shows its own login window.
    Prefer,
    /// Offer only NLA. A server without it is refused rather than reached
    /// through a login screen, after the whole connection sequence has
    /// already been completed.
    Require,
    /// Offer only TLS. For a server whose CredSSP does not work, which
    /// happens in practice, because CredSSP is where third-party servers most
    /// often differ from Windows.
    Off,
}

/// How the picture arrives: which of the three ways an RDP server has of
/// sending a desktop this client offers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Graphics {
    /// The graphics pipeline (MS-RDPEGFX), over a dynamic channel: what
    /// Windows 8 and later prefer, and the only path that carries H.264
    /// (`pipeline`). RemoteFX is offered beside it for a server without the
    /// pipeline.
    Pipeline,
    /// RemoteFX in the legacy surface commands, and the client's default:
    /// the codec every server here supports.
    RemoteFx,
    /// Plain bitmap updates, which change the picture quality rather than
    /// the speed.
    Bitmap,
}

/// The `Graphics` option value, as named in the catalog.
pub fn graphics(value: Option<&str>) -> Graphics {
    match value {
        Some("egfx") => Graphics::Pipeline,
        Some("bitmap") => Graphics::Bitmap,
        _ => Graphics::RemoteFx,
    }
}

/// How much of the remote machine's own visual effects to spend bandwidth on.
///
/// RDP expresses this as a set of "disable" flags plus a connection-type hint
/// the server uses to pick its own defaults, matching mstsc's Experience tab
/// since 2001. They change together, so they are one setting here: the
/// choice is how plain the desktop may look, not which nine flags to send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Experience {
    /// Everything the machine draws on its local display.
    Full,
    /// IronRDP's own default, and mstsc's for a LAN: no window contents while
    /// dragging and no menu fades, both of which are animation rather than
    /// content.
    Balanced,
    /// Wallpaper, themes and cursor decoration off as well, and the link
    /// described as a modem so the server tunes for one.
    Plain,
}

/// The `Experience` option value, as named in the catalog.
pub fn experience(value: Option<&str>) -> Experience {
    match value {
        Some("full") => Experience::Full,
        Some("plain") => Experience::Plain,
        _ => Experience::Balanced,
    }
}

/// Bulk compression, by the option values' names. The protocol's four levels
/// are historical rather than a trade-off (every current client asks for
/// 6.1), so anything unrecognised, including no value, means off.
pub fn compression(value: Option<&str>) -> Option<CompressionType> {
    match value {
        Some("rdp61") => Some(CompressionType::Rdp61),
        Some("rdp60") => Some(CompressionType::Rdp6),
        Some("64k") => Some(CompressionType::K64),
        Some("8k") => Some(CompressionType::K8),
        _ => None,
    }
}

/// The connection panel's information, as strings for display. There is no
/// desktop name and no encoding: RDP has neither concept, so those rows are
/// absent from this list rather than empty in it.
#[derive(Clone, Default)]
pub struct Info {
    pub protocol: String,
    pub connection: String,
    pub security: String,
    /// The graphics pipeline's version and codec, as `pipeline::State`
    /// words it, and empty on the other two paths, where the codec changes
    /// per update and is not named.
    pub encoding: String,
    pub line_speed: String,
    pub server_pixels: String,
    pub viewer_pixels: String,
}

/// One outbound frame, already encoded. Encoding on the caller's thread and
/// writing on a dedicated thread keeps a stalled socket from blocking the main
/// thread, which is where the app calls [`Client::pointer`] from.
enum Out {
    Bytes(Vec<u8>),
    Stop,
}

/// Work only the active stage can encode, so it waits for the protocol
/// thread's next check.
enum Task {
    Focus(bool),
    /// A monitor layout to put on the display-control channel: one entry per
    /// monitor, already laid out. Both a resize and a change of monitor count
    /// use this, since MS-RDPEDISP has one message, carrying the whole layout.
    Layout(Vec<MonitorLayoutEntry>),
}

/// The desktop's monitors, four ints each (x, y, width, height) as `Backend`
/// expects them: as the server last stated them, and as this end last asked
/// for them. Either one applies only while it covers the current desktop
/// (see `covers`); a layout from before the desktop last changed does not
/// describe it.
#[derive(Default)]
struct Layout {
    /// From the Monitor Layout PDU, during the connection, a reactivation or
    /// the active session; empty until the server has sent one.
    server: Vec<i32>,
    /// The last layout sent on the display-control channel.
    asked: Vec<i32>,
}

/// A key that is down, and how it was pressed.
struct Held {
    key: Key,
    /// Whether this client pressed Shift to produce it, as opposed to the
    /// user holding Shift on the extension keyboard. Only a synthetic Shift
    /// is released.
    synthetic_shift: bool,
}

struct Shared {
    framebuffer: RwLock<Framebuffer>,
    out: Sender<Out>,
    tasks: Mutex<VecDeque<Task>>,
    /// The socket, from the moment it starts dialling; `close` shuts it
    /// down.
    hangup: Hangup,
    info: Mutex<Info>,
    /// The keyboard and mouse state IronRDP keeps so that a redundant event is
    /// not sent twice. Locked on whichever thread the input arrived on.
    input: Mutex<Database>,
    /// keyId → the key pressed, so a release names the same key as its
    /// press.
    held: Mutex<HashMap<i64, Held>>,
    /// The last RFB button mask, because RDP needs the *edges* and the app
    /// sends the level.
    ///
    /// Nine bits, not eight: a physical mouse has back and forward buttons,
    /// and RDP can send them. RFB's own mask is one byte and cannot carry the
    /// ninth.
    buttons: Mutex<u16>,
    running: AtomicBool,
    view_only: AtomicBool,
    /// The display-control channel is open and its capabilities have arrived,
    /// which alone decides whether this desktop can be resized. Nothing may be
    /// sent on it before that.
    display_control: AtomicBool,
    /// How many monitors this session asks for. Not read from the config after
    /// the connection because a resize rebuilds the layout and has to keep the
    /// count.
    monitor_count: AtomicU8,
    /// The layout as the server stated it and as it was asked for.
    layout: Mutex<Layout>,
    /// The data transferred in each direction since the socket was opened.
    /// Protocol bytes: this counts what goes into and comes out of the TLS
    /// session rather than what the link carried, so it excludes record
    /// framing and TCP and IP headers.
    bytes_in: Arc<AtomicU64>,
    bytes_out: Arc<AtomicU64>,
    /// Shared with the CLIPRDR channel, which the active stage owns.
    clipboard: Arc<Clipboard>,
    /// Shared with the graphics pipeline's channel, likewise.
    pipeline: Arc<PipelineState>,
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
                hangup: Hangup::new(),
                info: Mutex::new(Info::default()),
                input: Mutex::new(Database::new()),
                held: Mutex::new(HashMap::new()),
                buttons: Mutex::new(0),
                running: AtomicBool::new(false),
                view_only: AtomicBool::new(false),
                display_control: AtomicBool::new(false),
                monitor_count: AtomicU8::new(1),
                layout: Mutex::new(Layout::default()),
                bytes_in: Arc::new(AtomicU64::new(0)),
                bytes_out: Arc::new(AtomicU64::new(0)),
                clipboard: Arc::new(Clipboard::default()),
                pipeline: Arc::new(PipelineState::default()),
            }),
            rx: Mutex::new(Some(rx)),
        }
    }

    /// The desktop, for the drawing thread.
    pub fn framebuffer(&self) -> &RwLock<Framebuffer> {
        &self.shared.framebuffer
    }

    pub fn info(&self) -> Info {
        let mut info = self.shared.info.lock().unwrap().clone();
        info.encoding = self.shared.pipeline.encoding();
        info
    }

    /// The graphics pipeline's state, for a probe's report of what arrived.
    pub fn pipeline(&self) -> &PipelineState {
        &self.shared.pipeline
    }

    /// Bytes received and sent since the socket was opened.
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

    /// An absolute position and an RFB button mask, which the whole control
    /// stack uses (`control.input.Button`).
    ///
    /// Some of those bits are buttons and four are wheel notches, while RDP
    /// has a real wheel event, so the buttons are held state and the wheel is
    /// an edge. Diffing here rather than in Java keeps the mask the only input
    /// format above `Backend`.
    pub fn pointer(&self, x: u16, y: u16, mask: u16) {
        if self.shared.view_only.load(Ordering::Relaxed) {
            return;
        }
        let previous = {
            let mut buttons = self.shared.buttons.lock().unwrap();
            std::mem::replace(&mut *buttons, mask)
        };
        let mut ops = Vec::with_capacity(4);
        // The move goes first so that a click lands at the new position: the
        // database stamps every button event with its current position.
        ops.push(Operation::MouseMove(MousePosition { x, y }));
        for (bit, button) in [
            (0x01, MouseButton::Left),
            (0x02, MouseButton::Middle),
            (0x04, MouseButton::Right),
            // Buttons 8 and 9. RDP calls them X1 and X2, and Windows treats
            // them as browser back and forward, which is what they are.
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
            // At trace level, since a keysym reveals what was typed.
            log::trace!("no RDP key for keysym {keysym:#x}");
            return;
        };
        let mut held = self.shared.held.lock().unwrap();
        let mut ops = Vec::with_capacity(2);
        // A key already down going down again is Android's auto-repeat, which
        // re-presses without releasing. The synthetic Shift must be kept: by
        // the second press Shift *is* down, so checking again would say it is
        // not needed, and the entry that would have released it would be
        // overwritten with one that does not, leaving Shift held at the far
        // end forever.
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
        // nothing is correct either way.
        let Some(held) = self.shared.held.lock().unwrap().remove(&key_id) else {
            return;
        };
        self.apply(release(&held));
    }

    /// Release everything, for a screen closing with keys still held: they
    /// are held at the far end, and stay that way until released.
    pub fn release_all_keys(&self) {
        let held: Vec<Held> = self.shared.held.lock().unwrap().drain().map(|(_, h)| h).collect();
        let mut ops = Vec::new();
        for key in &held {
            ops.extend(release(key));
        }
        self.apply(ops);
    }

    /// This end copied something. Announced to the remote at the protocol
    /// thread's next check, and sent only if the remote requests it.
    ///
    /// Not while view-only: replacing the far end's clipboard counts as
    /// input.
    pub fn clipboard(&self, text: &str) {
        if self.shared.view_only.load(Ordering::Relaxed) {
            return;
        }
        self.shared.clipboard.set_local(text);
    }

    pub fn set_view_only(&self, view_only: bool) {
        self.shared.view_only.store(view_only, Ordering::Relaxed);
    }

    /// Whether the far end accepts a desktop resize right now.
    ///
    /// The size is set at connect time in RDP, so this depends only on the
    /// display control channel: true once the server has opened it and sent
    /// its capabilities, which happens after the connection, if at all. This
    /// matches the behaviour of an RFB server's `ExtendedDesktopSize`, for a
    /// completely different reason.
    pub fn can_resize(&self) -> bool {
        self.shared.display_control.load(Ordering::Relaxed)
            && !self.shared.view_only.load(Ordering::Relaxed)
    }

    /// The desktop's monitors, four ints each, in the server's order, which
    /// is the only identity RDP gives a monitor: the server's own layout if
    /// it covers the current desktop, the requested layout if the desktop
    /// came out that size, and otherwise empty, as for every single-monitor
    /// session, since one monitor is not a layout.
    pub fn monitors(&self) -> Vec<i32> {
        let layout = self.shared.layout.lock().unwrap();
        let fb = self.shared.framebuffer.read().unwrap();
        let desktop = (fb.width(), fb.height());
        [&layout.server, &layout.asked]
            .into_iter()
            .find(|flat| covers(flat, desktop))
            .filter(|flat| flat.len() > 4)
            .cloned()
            .unwrap_or_default()
    }

    /// Ask for a desktop this size, for the whole desktop rather than one
    /// monitor: `Backend::request_desktop_size` means the framebuffer size
    /// everywhere else, and a caller that has just read 2560×800 off the panel
    /// and sent it back must get the desktop it already has.
    ///
    /// So the monitors divide it (see `divided`). The result arrives as a
    /// reactivation carrying the desktop's new size, or not at all.
    pub fn request_desktop_size(&self, width: u16, height: u16) {
        let count = self.shared.monitor_count.load(Ordering::Relaxed).max(1);
        self.request_layout(&divided(u32::from(width), u32::from(height), count));
    }

    /// Ask for a desktop of these screens, `x, y, width, height` each, in
    /// the server's order: the first is the primary, and any beyond the
    /// server's last is a new monitor. Adjusted first to a layout the
    /// protocol accepts (see `arranged`).
    pub fn request_layout(&self, screens: &[(i32, i32, u32, u32)]) {
        if !self.can_resize() || screens.is_empty() {
            return;
        }
        let screens = &screens[..screens.len().min(16)];
        match layout_of(screens) {
            Ok(entries) => self.push(Task::Layout(entries)),
            Err(e) => log::warn!("{screens:?} is not a layout the protocol allows: {e}"),
        }
    }

    /// Stores the server's monitor layout as four ints each, as `Backend`
    /// expects. A monitor's corners are inclusive and relative to the
    /// primary, which others may sit above or to the left of; the desktop's
    /// pixel 0,0 is the top left of them all.
    fn heard(&self, monitors: &[Monitor], from: &str) {
        let left = monitors.iter().map(|m| m.left).min().unwrap_or(0);
        let top = monitors.iter().map(|m| m.top).min().unwrap_or(0);
        let mut flat = Vec::with_capacity(monitors.len() * 4);
        for (i, m) in monitors.iter().take(16).enumerate() {
            let (w, h) = (m.right - m.left + 1, m.bottom - m.top + 1);
            log::info!(
                "{from}: monitor {} of {} {w}x{h}+{}+{}{}",
                i + 1,
                monitors.len(),
                m.left - left,
                m.top - top,
                if m.flags.contains(MonitorFlags::PRIMARY) { ", primary" } else { "" }
            );
            flat.extend_from_slice(&[m.left - left, m.top - top, w, h]);
        }
        self.shared.layout.lock().unwrap().server = flat;
    }

    /// How many monitors to ask for. Takes effect at the next layout, which is
    /// the next resize, so that changing the setting does not rearrange an
    /// existing desktop's monitors as a side effect.
    pub fn set_monitor_count(&self, count: u8) {
        self.shared.monitor_count.store(count.clamp(1, 16), Ordering::Relaxed);
    }

    /// Whether the session is on screen.
    ///
    /// RFB pauses by not requesting updates; RDP's server sends updates
    /// unprompted, so this sends a Suppress Output PDU, which tells the server
    /// to send nothing until asked again.
    pub fn set_focused(&self, focused: bool) {
        self.push(Task::Focus(focused));
    }

    /// End the session. The protocol thread is blocked on a socket read, or
    /// still dialling, so it is woken by shutting the socket down rather than
    /// by a flag it would not check.
    pub fn close(&self) {
        self.shared.hangup.hang_up();
        let _ = self.shared.out.send(Out::Stop);
    }

    fn shift_is_down(&self) -> bool {
        let input = self.shared.input.lock().unwrap();
        input.is_key_pressed(SHIFT) || input.is_key_pressed(RIGHT_SHIFT)
    }

    /// Apply operations to the input state and send the resulting events.
    /// Operations that change nothing produce nothing; IronRDP's `Database`
    /// handles this, which the RFB client did not need.
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

    /// Pause, which is not a position and so bypasses the input database. The
    /// key never had a scancode of its own (a keyboard sends `E1 1D 45` for
    /// it), and mstsc sends a Ctrl marked `E1` around a Num Lock, which is
    /// what a Windows server expects. The four events are balanced, so the
    /// bypassed database stays consistent.
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
        // The end of the stream on a relayed connection is whatever ended
        // the connection behind it: the route says which.
        let mut end = EndReason::none();
        let result = self.session(config, handler, &mut end).map_err(|e| e.explained(&end));
        self.shared.running.store(false, Ordering::Release);
        let _ = self.shared.out.send(Out::Stop);
        if self.shared.hangup.is_hung_up() {
            return Err(Error::Closed);
        }
        result
    }

    fn session(&self, config: &Config, handler: &mut dyn Handler, end: &mut EndReason) -> Result<()> {
        let (host, port) = resolve(&config.address)?;

        // Asked before anything is connected, as the protocol requires: RDP
        // puts the credentials in the connection sequence, so the server never
        // asks for them.
        let credentials = match stored(config) {
            Some(credentials) => credentials,
            None => handler
                .credentials(true)
                .ok_or(Error::Closed)?,
        };

        let Connection { socket, peer, end: ended } =
            connect(&*config.network, &self.shared.hangup, &host, port, config.connect_timeout)?;
        *end = ended;
        // Held for the rest of the session, including the X.224 and TLS
        // exchanges: a server that accepts the connection and sends nothing
        // blocks a read that only shutting the socket down ends.
        let _holding = self.shared.hangup.hold(&socket)?;
        socket.set_nodelay(true).ok();
        socket.set_read_timeout(Some(config.connect_timeout)).ok();
        let client_addr = client_address(&peer);

        let mut connector = ClientConnector::new(connector_config(config, &credentials), client_addr);
        // Before the connection sequence rather than after it: the list of
        // static channels is sent in the client's settings, so the server
        // would never learn of a channel attached later.
        connector.attach_static_channel(CliprdrClient::new(self.shared.clipboard.backend()));
        // The server's sound, if the session plays it. The player is held
        // here as well as by the channel, so that this thread can close a
        // stream that has gone quiet: the server's own Close does not indicate
        // when the sound will have been heard.
        //
        // Device redirection goes with it, announcing no devices at all:
        // Windows sends nothing on rdpsnd to a client without it (MS-RDPEFS,
        // appendix A, note 1), which is also why FreeRDP enables it for
        // sound.
        let player = config.audio.clone().map(|audio| {
            let (sound, player) = Sound::new(audio);
            connector.attach_static_channel(sound);
            connector.attach_static_channel(Rdpdr::new(Box::new(NoopRdpdrBackend), config.client_name.clone()));
            player
        });
        // The dynamic channel carrier, and its only dynamic channel. Display
        // control is how an RDP desktop is resized after connecting, and,
        // since its message is a whole monitor layout, the only way this
        // client asks for more than one monitor. The callback fires when the
        // server's capabilities arrive, which is when the channel becomes
        // usable; it sends no messages of its own.
        let display_control = Arc::clone(&self.shared);
        let mut drdynvc = DrdynvcClient::new().with_dynamic_channel(DisplayControlClient::new(move |caps| {
            log::info!("display control: {caps:?}");
            display_control.display_control.store(true, Ordering::Relaxed);
            Ok(Vec::new())
        }));
        // The graphics pipeline, when offered: the server opens its channel
        // once the capabilities say so, and Windows closes and reopens it,
        // so each opening gets a client of its own.
        if config.graphics == Graphics::Pipeline {
            let state = Arc::clone(&self.shared.pipeline);
            let decoders = config.decoders.clone();
            drdynvc = drdynvc.with_dynamic_channel_factory(ironrdp_egfx::CHANNEL_NAME, move || {
                pipeline::client(Arc::clone(&state), decoders.clone())
            });
        }
        connector.attach_static_channel(drdynvc);
        self.shared
            .monitor_count
            .store(config.monitors.clamp(1, 16), Ordering::Relaxed);

        // The X.224 negotiation, in clear text: everything up to the server
        // agreeing to a TLS upgrade.
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
        // The last moment before a password is sent, on a thread that may
        // wait for the user. Same rule and same place as in the RFB client.
        if !handler.trust(&peer.fingerprint) {
            return Err(Error::Refused(
                "The server's certificate was not accepted.".into(),
            ));
        }

        let upgraded = mark_as_upgraded(should_upgrade, &mut connector);
        // After the upgrade is marked done and not before: the connector only
        // enters its CredSSP state at that point, so checking any earlier
        // always returns false.
        let credssp = connector.should_perform_credssp();
        // Auto logon only without NLA, as FreeRDP does. With it, Windows
        // reconnects a disconnected session by unlocking it in place, and a
        // session in some states never finishes that unlock, leaving the
        // client on "Please wait". Without the flag, Windows checks the
        // CredSSP credentials in a temporary session and then moves the
        // connection over, which works in those states too.
        connector.config.autologon = !credssp;
        let public_key = peer.public_key.clone();
        let steps = Steps::new(Counting::new(wire.clone_handle(), &self.shared), handler.status_setter(), credssp);
        let mut framed = Framed::new(steps);
        let mut network = NoNetworkClient;
        let outcome = connect_finalize(
            upgraded,
            connector,
            &mut framed,
            &mut network,
            ServerName::new(host.clone()),
            public_key,
            None,
        )
        .map_err(|e| {
            if logon_refused(&e) {
                log::info!("{}", describe(&e));
                Error::Refused(api::credentials::logon_failed(&credentials.domain))
            } else {
                protocol(e)
            }
        })?;
        framed.get_inner_mut().0.finish();
        let result = match outcome {
            ConnectionOutcome::Connected(result) => result,
            ConnectionOutcome::Redirected(pdu) => return Err(redirected(&pdu)),
        };

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

        // The input the server accepts, as it states it. Logged in full
        // because the interesting flag is not acted on yet: MOUSE_RELATIVE
        // means the far end would accept deltas.
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
            // Not fatal, but worth logging, since input may have no effect in
            // this session.
            log::warn!("the server did not offer fast-path input; input may be ignored");
        }

        // The handshake is the only part with a deadline. After it the poll
        // interval applies, and an idle desktop may stay silent indefinitely.
        socket.set_read_timeout(Some(POLL)).ok();

        self.shared
            .framebuffer
            .write()
            .unwrap()
            .resize(desktop.width, desktop.height);
        self.shared
            .pipeline
            .set_output(usize::from(desktop.width), usize::from(desktop.height));
        self.shared.running.store(true, Ordering::Release);
        handler.connected(usize::from(desktop.width), usize::from(desktop.height));

        let writer = self.spawn_writer(Counting::new(wire.clone_handle(), &self.shared));
        let outcome = self.active_stage(result, &mut framed, handler, player.as_deref());
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
        player: Option<&Mutex<Player>>,
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
            // The pointer is drawn client-side: the whole control stack relies
            // on a client-owned cursor, so the server's shape has to arrive as
            // a bitmap rather than be composited into the picture. Turning
            // software rendering off does that; with it on, `DecodedImage`
            // draws the pointer into the framebuffer and the desktop would
            // show a second cursor.
            enable_server_pointer: result.enable_server_pointer,
            pointer_software_rendering: false,
        }
        .build();
        // Bytes received as of the last frame drawn: the line speed is the
        // bytes between two frames, including non-frame data.
        let mut rate = Rate::new();
        let mut counted = self.shared.bytes_in.load(Ordering::Relaxed);
        // One monitor's size, as requested in the connection sequence and
        // copied for every further monitor. Kept separately because `desktop`
        // becomes the bounding box once a layout is granted, and a second
        // layout built from that would double again.
        let unit = result.desktop_size;
        let mut asked = false;
        if let Some(layout) = &result.monitor_layout {
            self.heard(&layout.monitors, "monitor layout");
        }

        loop {
            // The server opens the channel some time after the session starts,
            // so the first layout is sent when it is ready rather than at a
            // point in the connection sequence. A single-monitor session sends
            // no layout: it already has the size it asked for.
            if !asked
                && self.shared.monitor_count.load(Ordering::Relaxed) > 1
                && stage.display_control_ready() == Some(true)
            {
                asked = true;
                let count = self.shared.monitor_count.load(Ordering::Relaxed);
                let (w, h) = (u32::from(unit.width), u32::from(unit.height));
                let copies: Vec<_> = (0..u32::from(count)).map(|i| ((i * w) as i32, 0, w, h)).collect();
                match layout_of(&copies) {
                    Ok(entries) => self.send_layout(framed, &mut stage, entries)?,
                    Err(e) => log::warn!("{count} monitors of {w}x{h} is not a layout the protocol allows: {e}"),
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
                        // straight into the destination, and adding a scratch
                        // copy would not make this cheaper.
                        stage
                            .process(image, action, &payload, framed.last_read_at())
                            .map_err(session)?
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
                                let (w, h) = (usize::from(pointer.width), usize::from(pointer.height));
                                match cursor_words(&pointer.bitmap_data, w, h) {
                                    Some(pixels) => handler.cursor(
                                        &pixels,
                                        w,
                                        h,
                                        i32::from(pointer.hotspot_x),
                                        i32::from(pointer.hotspot_y),
                                    ),
                                    None => log::warn!(
                                        "a {w}x{h} cursor came with {} bytes; not drawn",
                                        pointer.bitmap_data.len()
                                    ),
                                }
                            }
                            ActiveStageOutput::PointerHidden => {
                                handler.cursor(&[], 0, 0, 0, 0);
                            }
                            ActiveStageOutput::PointerPosition { x, y } => {
                                // The far end reporting where it moved the
                                // cursor. Ignored, since the cursor is owned
                                // locally and the viewport follows it, but
                                // whether a server sends these is worth one
                                // log line, so only the first is logged.
                                if !saw_pointer_position {
                                    saw_pointer_position = true;
                                    log::info!(
                                        "server moved its pointer to {x},{y} \
                                         (the first of this session)"
                                    );
                                }
                            }
                            ActiveStageOutput::PointerDefault => {
                                // "The system arrow", whose pixels the server
                                // never sends. Keeping the last shape is better
                                // than hiding the cursor, which is what an
                                // empty bitmap would do.
                                log::debug!("the server asked for the default pointer");
                            }
                            ActiveStageOutput::MonitorLayout(monitors) => {
                                self.heard(&monitors, "monitor layout");
                            }
                            ActiveStageOutput::Terminate(reason) => {
                                return Err(Error::Protocol(reason.description()));
                            }
                            ActiveStageOutput::ServerRedirection(pdu) => {
                                return Err(redirected(&pdu));
                            }
                            ActiveStageOutput::DeactivateAll => {
                                // Logged at info because a stalled session
                                // usually stops here: a Windows host sends one
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
                                // The server answers a layout with a
                                // reactivation, and restates the monitors
                                // during it.
                                if let Some(layout) = sequence.monitor_layout() {
                                    self.heard(&layout.monitors, "reactivation");
                                }
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
                    // The graphics pipeline's output reset resizes the image
                    // itself, inside `process`; after a reactivation it is
                    // the size the reactivation already set, and any other
                    // size is followed here.
                    let size = {
                        let fb = self.shared.framebuffer.read().unwrap();
                        (fb.width(), fb.height())
                    };
                    if size != (usize::from(desktop.width), usize::from(desktop.height)) {
                        log::info!("the graphics output is {}x{}", size.0, size.1);
                        desktop = DesktopSize {
                            width: size.0.min(usize::from(u16::MAX)) as u16,
                            height: size.1.min(usize::from(u16::MAX)) as u16,
                        };
                        self.shared.info.lock().unwrap().server_pixels =
                            format!("{}×{}, 32 bpp", desktop.width, desktop.height);
                        handler.desktop_size(size.0, size.1);
                    }
                    if drew {
                        let total = self.shared.bytes_in.load(Ordering::Relaxed);
                        if let Some(speed) = rate.sample(total.saturating_sub(counted)) {
                            self.shared.info.lock().unwrap().line_speed = speed;
                        }
                        counted = total;
                        handler.frame_end();
                    }
                }
                Err(e) if timed_out(&e) => {}
                Err(e) => return Err(Error::Io(e)),
            }

            // At least every poll, so a stream closes on an idle desktop too.
            if let Some(player) = player {
                player.lock().unwrap().close_if_quiet(QUIET);
            }

            // The pipeline's decoders asking for new streams (see
            // `pipeline::H264`).
            if self.shared.pipeline.take_restart() {
                self.restart_pipeline(framed, &mut stage)?;
            }

            // The clipboard, in both directions. The channel's decisions while
            // decoding are queued as `Want`s because the channel object is
            // inside the active stage and cannot encode where it is called.
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
                    // Not fatal: a failed clipboard transfer should not end
                    // the session.
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

            // Everything the active stage has to encode; it lives only on
            // this thread.
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
                            // and does not know what is out of date.
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

    /// Put a monitor layout on the display-control channel, and store the
    /// request so that a desktop arriving at that size confirms it.
    ///
    /// There is no refusal message: the server either reactivates with a new
    /// desktop or does nothing at all.
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
        // Relative to the primary on the wire, and to the top left of all
        // of them for `Backend`.
        let places: Vec<(i32, i32)> = entries.iter().map(|e| e.position().unwrap_or((0, 0))).collect();
        let left = places.iter().map(|p| p.0).min().unwrap_or(0);
        let top = places.iter().map(|p| p.1).min().unwrap_or(0);
        let mut flat = Vec::with_capacity(entries.len() * 4);
        for (entry, (x, y)) in entries.iter().zip(places) {
            let (w, h) = entry.dimensions();
            log::info!("asking for monitor {}x{}{:+}{:+}", w, h, x, y);
            flat.extend_from_slice(&[x - left, y - top, w as i32, h as i32]);
        }
        let pdu: DisplayControlPdu = DisplayControlMonitorLayout::new(&entries)
            .map_err(|e| Error::Protocol(e.to_string()))?
            .into();
        let messages = encode_dvc_messages(channel_id, vec![Box::new(pdu)], ChannelFlags::empty())
            .map_err(|e| Error::Protocol(e.to_string()))?;
        let frame = stage.encode_dvc_messages(messages).map_err(session)?;
        framed.write_all(&frame)?;
        self.shared.layout.lock().unwrap().asked = flat;
        Ok(())
    }

    /// Advertise the graphics pipeline's capabilities again, which makes the
    /// server reset the pipeline: new surfaces and new H.264 streams, each
    /// starting with a key frame, which is what a decoder that lost its
    /// pictures needs and what a Refresh Rect does not bring (FreeRDP's
    /// backend does the same). Not possible below version 10.3, where the
    /// protocol allows no reset in the channel; then logged, and the
    /// picture stays as it is until the server starts a stream by itself.
    fn restart_pipeline<S: Read + Write>(&self, framed: &mut Framed<S>, stage: &mut ActiveStage) -> Result<()> {
        let Some((channel_id, pdu)) = stage
            .get_dvc_mut::<GraphicsPipelineClient>()
            .map(|mut dvc| (dvc.channel_id(), dvc.processor_mut().request_protocol_reset()))
        else {
            log::warn!("graphics pipeline: no channel to restart");
            return Ok(());
        };
        let Some(pdu) = pdu else {
            log::warn!("graphics pipeline: the version in use cannot be restarted in the channel");
            return Ok(());
        };
        let messages = encode_dvc_messages(channel_id, vec![Box::new(pdu) as DvcMessage], ChannelFlags::empty())
            .map_err(|e| Error::Protocol(e.to_string()))?;
        let frame = stage.encode_dvc_messages(messages).map_err(session)?;
        framed.write_all(&frame)?;
        log::info!("graphics pipeline: capabilities advertised again");
        Ok(())
    }

    /// Ask for the whole desktop again, with whichever of the two PDUs the
    /// server said it accepts.
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
    /// capabilities down and rebuilds them, which is how RDP resizes a
    /// desktop. Callers see no change unless the size changed.
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
            match ironrdp_connector::state_downcast::<ConnectionActivationState>(sequence.state())
                .ok_or_else(|| Error::Protocol("reactivation lost its state".into()))?
            {
                ConnectionActivationState::Redirected { redirection } => {
                    return Err(redirected(redirection));
                }
                &ConnectionActivationState::Finalized {
                    desktop_size,
                    share_id,
                    enable_server_pointer,
                    static_channel_chunk_size,
                    ..
                } => {
                    if !stage.reactivate(
                        io_channel_id,
                        user_channel_id,
                        share_id,
                        enable_server_pointer,
                        false,
                        static_channel_chunk_size,
                    ) {
                        return Err(Error::Protocol(
                            "the server offered an invalid virtual channel chunk size".into(),
                        ));
                    }
                    return Ok(desktop_size);
                }
                _ => {}
            }
        }
    }
}

/// One step of a connection sequence: read the PDU it is waiting for, pass it
/// on, and write whatever it produced.
///
/// `ironrdp_blocking::single_sequence_step` does this, but only for
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
            sequence.step(&pdu, framed.last_read_at(), buf).map_err(protocol)?
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
        // Its release was sent with its press, as one sequence.
        Key::Pause => {}
    }
    ops
}

const SHIFT: Scancode = Scancode::from_u8(false, 0x2a);
const RIGHT_SHIFT: Scancode = Scancode::from_u8(false, 0x36);

/// A `w`x`h` cursor's `R, G, B, A` bytes as words, as `Handler::cursor`
/// takes them, or `None` when there are not exactly `w * h` of them:
/// IronRDP's decoders currently size the buffer by the pointer's own
/// dimensions, but everything after `Handler::cursor` indexes by `w` and `h`,
/// so a short buffer is rejected here rather than trusted.
fn cursor_words(bytes: &[u8], w: usize, h: usize) -> Option<Vec<u32>> {
    if w.checked_mul(h)?.checked_mul(4)? != bytes.len() {
        return None;
    }
    Some(bytes.chunks_exact(4).map(|px| u32::from_le_bytes([px[0], px[1], px[2], px[3]])).collect())
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

/// Whether CredSSP failed because the server refused the user name or
/// password (`STATUS_LOGON_FAILURE`, `STATUS_WRONG_PASSWORD`), which FreeRDP
/// reports as its logon failure.
fn logon_refused(e: &ironrdp_connector::ConnectorError) -> bool {
    use ironrdp_connector::sspi::credssp::NStatusCode;
    match e.kind() {
        ironrdp_connector::ConnectorErrorKind::Credssp(e) => {
            e.nstatus == Some(NStatusCode::LOGON_FAILURE) || e.nstatus == Some(NStatusCode::WRONG_PASSWORD)
        }
        _ => false,
    }
}

fn protocol(e: ironrdp_connector::ConnectorError) -> Error {
    Error::Protocol(describe(&e))
}

fn session(e: ironrdp_session::SessionError) -> Error {
    Error::Protocol(describe(&e))
}

/// A Server Redirection PDU, during the connection, a reactivation or the
/// session: the server asks the client to reconnect to another host or
/// session, as a connection broker does. This client does not follow it, and
/// sends no Client Cluster Data (REDIRECTION_SUPPORTED), so a broker should
/// not redirect it.
fn redirected(pdu: &ServerRedirectionPdu) -> Error {
    log::info!(
        "server redirection to session {} at {}",
        pdu.session_id,
        pdu.target_fqdn
            .as_deref()
            .or(pdu.target_net_address.as_deref())
            .unwrap_or("the same address")
    );
    Error::Protocol("The server redirected the connection, which this client does not support.".into())
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

/// A stream that counts bytes both ways, for the line-speed readout and the
/// panel's transferred data. It wraps the current connection (the bare socket
/// for the X.224 exchange, the TLS session for everything after it), so it
/// counts protocol bytes rather than link bytes.
///
/// Three per session, because RDP is read and written from three places: the
/// negotiation before the upgrade, the connection sequence and the active
/// stage over one `Framed`, and the writer thread over its own handle. A
/// counter on only one of them would miss a stage.
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

/// The connecting screen's status during the connection sequence, which
/// `connect_finalize` runs from start to end without a word: the
/// authentication (CredSSP), and then the rest, where the server prepares the
/// session (and Windows may wait for its licence server). Each is named once
/// it has lasted long enough to read (`runtime::status::Delayed`).
///
/// Which one is under way shows in what this end writes: CredSSP's messages
/// are DER (`TSRequest`, a SEQUENCE, 0x30) and the rest of the sequence is
/// X.224 in TPKT (version 3), each write one whole message. So the first
/// write that starts with a TPKT header ends the authentication.
struct Steps<S> {
    inner: S,
    setter: Option<runtime::status::StatusSetter>,
    authenticating: bool,
    status: Option<runtime::status::Delayed>,
}

const STATUS_AUTHENTICATING: &str = "Authenticating…";
const STATUS_STARTING: &str = "Starting the session…";

impl<S> Steps<S> {
    fn new(inner: S, setter: Option<runtime::status::StatusSetter>, credssp: bool) -> Steps<S> {
        let text = if credssp { STATUS_AUTHENTICATING } else { STATUS_STARTING };
        Steps {
            status: Some(runtime::status::Delayed::new(setter.clone(), text)),
            inner,
            setter,
            authenticating: credssp,
        }
    }

    /// The sequence is over: nothing more is named, and what follows passes
    /// straight through.
    fn finish(&mut self) {
        self.status = None;
        self.setter = None;
        self.authenticating = false;
    }
}

impl<R: Read> Read for Steps<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.inner.read(buf)
    }
}

impl<W: Write> Write for Steps<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.authenticating && buf.first() == Some(&0x03) {
            self.authenticating = false;
            // At once if the authentication was named (the wait is already
            // a long one), and otherwise once it has lasted.
            let shown = self.status.take().is_some_and(|status| status.end());
            match &self.setter {
                Some(setter) if shown => setter(Some(STATUS_STARTING)),
                _ => self.status = Some(runtime::status::Delayed::new(self.setter.clone(), STATUS_STARTING)),
            }
        }
        self.inner.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// CredSSP requires one of these so that Kerberos can reach a KDC. This client
/// does not support Kerberos (it uses NTLM, which needs no third party), so
/// this only reports that, rather than pulling an HTTP client into the app.
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
    use ironrdp_pdu::rdp::capability_sets::RailSupportLevel;
    use ironrdp_pdu::rdp::client_info::{PerformanceFlags, TimezoneInfo};

    // The only codec worth a choice. Everything else the list can hold is
    // either always on or IronRDP's own, which no server here supports.
    // RemoteFX stays offered with the graphics pipeline, for a server that
    // does not open the pipeline's channel.
    let codecs = client_codecs_capabilities(if config.graphics == Graphics::Bitmap {
        &["remotefx:off"]
    } else {
        &[]
    })
    .expect("a fixed codec list");

    ironrdp_connector::Config {
        credentials: ConnectorCredentials::UsernamePassword {
            username: credentials.user.clone(),
            password: credentials.password.clone(),
        },
        domain: Some(credentials.domain.clone()).filter(|d| !d.is_empty()),
        // Both by default, so the server picks: `enable_tls` is the legacy
        // "TLS, then a login screen inside the session", which is all xrdp has,
        // and `enable_credssp` is NLA. Offering only the second requires it,
        // which is what `Nla::Require` does.
        enable_tls: config.nla != Nla::Require,
        enable_credssp: config.nla != Nla::Off,
        // RC4 with a key exchanged by the protocol itself. No version of it
        // is worth offering, so a server without TLS is refused rather than
        // downgraded, as the RFB client does with anonymous VeNCrypt.
        enable_standard_rdp_security: false,
        keyboard_type: KeyboardType::IBM_ENHANCED,
        keyboard_subtype: 0,
        keyboard_layout: config.keyboard_layout,
        keyboard_functional_keys_count: 12,
        connection_type: match config.experience {
            // A hint rather than an instruction: the flags below tell the
            // server what to stop doing, and it uses this to decide the rest
            // itself.
            Experience::Plain => ConnectionType::Modem,
            _ => ConnectionType::Lan,
        },
        ime_file_name: String::new(),
        dig_product_id: String::new(),
        desktop_size: DesktopSize {
            width: config.desktop_size.0,
            height: config.desktop_size.1,
        },
        // Always one monitor here: more are requested over display control
        // once the session is up (`send_layout`), which also changes them.
        monitor_layout: None,
        bitmap: Some(BitmapConfig {
            // How the picture arrives. 32 bits because the framebuffer is,
            // and anything less would add a conversion on the way in for a
            // saving the codec already makes.
            lossy_compression: false,
            color_depth: 32,
            codecs,
        }),
        client_build: 0,
        client_name: config.client_name.clone(),
        client_dir: "C:\\Windows\\System32\\mstscax.dll".to_owned(),
        platform: config.platform.wire(),
        // The basis of the control stack: the pointer is drawn here, at the
        // gesture layer's position, rather than arriving a round trip later
        // inside the picture.
        enable_server_pointer: true,
        request_data: None,
        // Credentials are always present (they are asked for before
        // connecting), so the server never needs to show its own login screen
        // in the session it has just opened. Cleared once NLA is chosen
        // (`session`).
        autologon: true,
        // Without it the server plays its sound nowhere, which is what the
        // Sound option's "Off" means; leaving it on the remote machine would
        // need a client-info flag IronRDP does not expose.
        enable_audio_playback: config.audio.is_some(),
        enable_audio_capture: false,
        compression_type: config.compression,
        pointer_software_rendering: false,
        multitransport_flags: None,
        // The graphics pipeline, which the server opens as a dynamic
        // channel once the client says it may (`pipeline`).
        support_dyn_vc_gfx_protocol: config.graphics == Graphics::Pipeline,
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
        // A desktop, not a RemoteApp.
        remote_application_mode: false,
        rail_support_level: RailSupportLevel::empty(),
    }
}

/// `count` monitors side by side dividing a desktop `width` wide between them,
/// the last taking the remainder: the requested size is the whole desktop's,
/// not a monitor's. FreeRDP's backend divides it the same way.
fn divided(width: u32, height: u32, count: u8) -> Vec<(i32, i32, u32, u32)> {
    let count = u32::from(count.max(1));
    let each = width / count;
    (0..count)
        .map(|i| {
            let w = if i == count - 1 { width - each * (count - 1) } else { each };
            ((i * each) as i32, 0, w, height)
        })
        .collect()
}

/// Screens, `x, y, width, height` each, adjusted to what the protocol accepts
/// (as FreeRDP's backend also does): a monitor is 200 to 8192 pixels each way
/// with an even width, and a screen made narrower or shorter by that moves
/// those to its right or below it by the same amount, so that adjacent
/// screens stay adjacent; and the first is the primary, at the origin, with
/// the rest placed relative to it. The size is adjusted rather than refused,
/// because it comes from a window whose shape this client does not control.
fn arranged(screens: &[(i32, i32, u32, u32)]) -> Vec<(i32, i32, u32, u32)> {
    let sizes: Vec<(u32, u32)> = screens
        .iter()
        .map(|&(_, _, w, h)| MonitorLayoutEntry::adjust_display_size(w, h))
        .collect();
    let moved: Vec<(i32, i32)> = screens
        .iter()
        .map(|&(x, y, _, _)| {
            let (mut nx, mut ny) = (x, y);
            for (&(ox, oy, ow, oh), &(aw, ah)) in screens.iter().zip(&sizes) {
                if ox + ow as i32 <= x {
                    nx -= ow as i32 - aw as i32;
                }
                if oy + oh as i32 <= y {
                    ny -= oh as i32 - ah as i32;
                }
            }
            (nx, ny)
        })
        .collect();
    let (x0, y0) = moved.first().copied().unwrap_or((0, 0));
    moved
        .iter()
        .zip(&sizes)
        .map(|(&(x, y), &(w, h))| (x - x0, y - y0, w, h))
        .collect()
}

/// `screens` (see `arranged`) as display-control entries, the first the
/// primary. Fails for a layout the library refuses: overlapping monitors,
/// or a monitor touching none of the others.
fn layout_of(screens: &[(i32, i32, u32, u32)]) -> Result<Vec<MonitorLayoutEntry>> {
    let entries = arranged(screens)
        .into_iter()
        .enumerate()
        .map(|(i, (x, y, w, h))| {
            if i == 0 {
                MonitorLayoutEntry::new_primary(w, h)
            } else {
                MonitorLayoutEntry::new_secondary(w, h).and_then(|e| e.with_position(x, y))
            }
            .map_err(|e| Error::Protocol(e.to_string()))
        })
        .collect::<Result<Vec<_>>>()?;
    DisplayControlMonitorLayout::new(&entries).map_err(|e| Error::Protocol(e.to_string()))?;
    Ok(entries)
}

/// Whether monitors, four ints each, divide a desktop this size, i.e. their
/// bounding box matches it.
fn covers(flat: &[i32], (width, height): (usize, usize)) -> bool {
    if flat.len() < 4 || flat.chunks_exact(4).any(|m| m[0] < 0 || m[1] < 0) {
        return false;
    }
    let right = flat.chunks_exact(4).map(|m| m[0] + m[2]).max().unwrap_or(0);
    let bottom = flat.chunks_exact(4).map(|m| m[1] + m[3]).max().unwrap_or(0);
    usize::try_from(right) == Ok(width) && usize::try_from(bottom) == Ok(height)
}

/// `host` or `host:port`, plus the bracketed IPv6 form. No display-number
/// convention here: that is a VNC convention, and RDP's default port has
/// always been 3389.
fn resolve(address: &str) -> Result<(String, u16)> {
    runtime::address::split(address, 3389, runtime::address::Ports::Plain).map_err(Error::Protocol)
}

/// The Client Info PDU's client address, which the server shows as where the
/// session came from: the address the server sees the connection come from,
/// as the route reports it. A route that does not know it (a tunnel that
/// resolves at its far end) gets 0.0.0.0, which xrdp and Windows accept.
fn client_address(peer: &Peer) -> std::net::SocketAddr {
    peer.local.unwrap_or_else(|| std::net::SocketAddr::from(([0, 0, 0, 0], 0)))
}

/// Dials through `network`, which tries each address in turn, each with the
/// whole `timeout`, with `hangup` so that a close ends the dial too.
fn connect(network: &dyn Network, hangup: &Hangup, host: &str, port: u16, timeout: Duration) -> Result<Connection> {
    if host.is_empty() {
        return Err(Error::Protocol("no address".into()));
    }
    match network.connect(host, port, Timeout::PerAddress(timeout), &Liveness::default(), hangup) {
        Ok(connection) => Ok(connection),
        Err(_) if hangup.is_hung_up() => Err(Error::Closed),
        Err(e) => Err(Error::Io(e)),
    }
}

/// The interface `runtime::Runner` runs: the inherent `run` and `close` are
/// the implementation, exposed here for a session that does not know which
/// protocol it has.
impl runtime::Client for Client {
    type Config = Config;
    type Error = Error;
    const NAME: &'static str = "rdp";

    fn run(&self, config: &Config, handler: &mut dyn Handler) -> Result<()> {
        self.run(config, handler)
    }

    fn close(&self) {
        self.close();
    }

    /// `run` reports a requested close as [`Error::Closed`], the ending the
    /// screen requested, which needs no message.
    fn closed_on_request(error: &Error) -> bool {
        matches!(error, Error::Closed)
    }
}

impl Default for Client {
    fn default() -> Client {
        Client::new()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_client_address_is_the_routes_or_none() {
        use runtime::net::{Peer, Route};
        let peer = |local: Option<&str>| Peer {
            host: "rdp.example".into(),
            port: 3389,
            address: None,
            local: local.map(|a| a.parse().unwrap()),
            route: Route::Tunnel,
        };
        assert_eq!(super::client_address(&peer(Some("10.0.0.7:50123"))).to_string(), "10.0.0.7:50123");
        assert_eq!(super::client_address(&peer(None)).to_string(), "0.0.0.0:0");
    }

    use super::*;

    /// The authentication is named once it has lasted a second, and
    /// replaced as soon as the first X.224 message is written.
    #[test]
    fn the_connection_sequence_names_its_long_steps() {
        let said = Arc::new(Mutex::new(Vec::new()));
        let into = Arc::clone(&said);
        let setter: runtime::status::StatusSetter = Arc::new(move |text: Option<&str>| {
            into.lock().unwrap().push(text.map(str::to_string));
        });
        let mut steps = Steps::new(Vec::new(), Some(setter), true);
        steps.write_all(&[0x30, 0x82, 0x01]).unwrap();
        std::thread::sleep(runtime::status::READABLE + Duration::from_millis(300));
        steps.write_all(&[0x03, 0x00, 0x01, 0x00]).unwrap();
        steps.finish();
        assert_eq!(
            *said.lock().unwrap(),
            [Some(STATUS_AUTHENTICATING.to_string()), Some(STATUS_STARTING.to_string())]
        );
        assert_eq!(steps.inner, [0x30, 0x82, 0x01, 0x03, 0x00, 0x01, 0x00]);
        // A short authentication is never named, and neither is a short
        // step after it.
        let said = Arc::new(Mutex::new(Vec::new()));
        let into = Arc::clone(&said);
        let setter: runtime::status::StatusSetter = Arc::new(move |text: Option<&str>| {
            into.lock().unwrap().push(text.map(str::to_string));
        });
        let mut steps = Steps::new(Vec::new(), Some(setter), true);
        steps.write_all(&[0x30]).unwrap();
        steps.write_all(&[0x03]).unwrap();
        steps.finish();
        assert!(said.lock().unwrap().is_empty());
    }

    /// Only a CredSSP status for a refused user name or password is a
    /// logon failure; any other status or error is not.
    #[test]
    fn a_logon_failure_is_a_refused_name_or_password() {
        use ironrdp_connector::sspi::credssp::NStatusCode;
        use ironrdp_connector::{ConnectorError, ConnectorErrorKind};
        let credssp = |status: NStatusCode| {
            let e = ironrdp_connector::sspi::Error::new_with_nstatus(
                ironrdp_connector::sspi::ErrorKind::InvalidToken,
                "CredSSP server returned an error status",
                status,
            );
            ConnectorError::new("CredSSP", ConnectorErrorKind::Credssp(e))
        };
        assert!(logon_refused(&credssp(NStatusCode::LOGON_FAILURE)));
        assert!(logon_refused(&credssp(NStatusCode::WRONG_PASSWORD)));
        assert!(!logon_refused(&credssp(NStatusCode::ACCOUNT_DISABLED)));
        assert!(!logon_refused(&credssp(NStatusCode::PASSWORD_EXPIRED)));
        assert!(!logon_refused(&ConnectorError::new("CredSSP", ConnectorErrorKind::AccessDenied)));
    }

    /// A cursor's bytes become one word per pixel only when there are
    /// exactly `w * h` pixels of them.
    #[test]
    fn cursor_words_need_every_pixel() {
        let px = [1, 2, 3, 4, 5, 6, 7, 8];
        assert_eq!(cursor_words(&px, 2, 1), Some(vec![0x0403_0201, 0x0807_0605]));
        assert_eq!(cursor_words(&px[..7], 2, 1), None);
        assert_eq!(cursor_words(&px, 2, 2), None);
        assert_eq!(cursor_words(&px, 1, 1), None);
        assert_eq!(cursor_words(&[], 0, 0), Some(vec![]));
        assert_eq!(cursor_words(&px, usize::MAX, 2), None);
    }

    /// The forms are `runtime::address`'s; specific to this client are the
    /// default port and the display rule being off.
    #[test]
    fn addresses() {
        assert_eq!(resolve("10.0.0.5").unwrap(), ("10.0.0.5".into(), 3389));
        assert_eq!(resolve("[::1]").unwrap(), ("::1".into(), 3389));
        assert!(resolve("2001:db8::1").is_err());
        // No display-number rule: `:1` is a port here, since reading it as
        // 3390 would apply a VNC convention to RDP.
        assert_eq!(resolve("host:1").unwrap(), ("host".into(), 1));
    }

    /// The three values must differ in what is sent, and the middle one must
    /// match what the client sent before the option existed, so that the
    /// default does not change a connection.
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
    /// must not scroll forever, and releasing it must not scroll back.
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

    /// A release names the same key as its press, an unmatched one sends
    /// nothing, and a synthetic Shift is released.
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

    /// Auto-repeat presses a key that is already down, with no release in
    /// between, and the synthetic Shift has to survive it: by the second press
    /// Shift *is* down (the synthetic one), so checking again would say it is
    /// not needed, and the entry that would have released it would be
    /// replaced by one that never does.
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

    /// A Shift held by the user is not released: the extension keyboard's
    /// locked modifier has to survive a shifted character being typed, which
    /// is what merging input from both keyboards requires.
    #[test]
    fn a_shift_the_user_holds_is_left_alone() {
        let client = Client::new();
        client.shared.running.store(true, Ordering::Release);
        client.key_down(0xffe1, 1); // Shift, from the extension keyboard
        client.key_down('A' as u32, 2);
        client.key_up(2);
        let input = client.shared.input.lock().unwrap();
        assert!(input.is_key_pressed(SHIFT), "still held by the row");
    }

    /// The three ways the picture may arrive, as the connector is told
    /// them: the pipeline flag only for the pipeline, and RemoteFX in the
    /// codec list for everything but bitmaps.
    #[test]
    fn the_graphics_row_reaches_the_connector() {
        use ironrdp_pdu::rdp::capability_sets::CodecProperty;
        let of = |graphics: Graphics| {
            let config = Config {
                graphics,
                ..Config::default()
            };
            let c = connector_config(&config, &Credentials::default());
            let remote_fx = c.bitmap.as_ref().is_some_and(|b| {
                b.codecs
                    .0
                    .iter()
                    .any(|codec| matches!(codec.property, CodecProperty::RemoteFx(_)))
            });
            (c.support_dyn_vc_gfx_protocol, remote_fx)
        };
        assert_eq!(of(Graphics::Pipeline), (true, true));
        assert_eq!(of(Graphics::RemoteFx), (false, true));
        assert_eq!(of(Graphics::Bitmap), (false, false));
        assert_eq!(graphics(Some("egfx")), Graphics::Pipeline);
        assert_eq!(graphics(Some("bitmap")), Graphics::Bitmap);
        assert_eq!(graphics(Some("rfx")), Graphics::RemoteFx);
        assert_eq!(graphics(None), Graphics::RemoteFx);
    }

    /// The capabilities report the client's own platform, not always the
    /// phone's: `UNIX` from Linux, as IronRDP's own viewer and FreeRDP send.
    #[test]
    fn the_platform_is_the_clients_own() {
        use ironrdp_pdu::rdp::capability_sets::MajorPlatformType;
        let sent = |platform| {
            let config = Config { platform, ..Config::default() };
            connector_config(&config, &Credentials::default()).platform
        };
        assert_eq!(sent(Platform::Android), MajorPlatformType::ANDROID);
        assert_eq!(sent(Platform::Linux), MajorPlatformType::UNIX);
    }

    #[test]
    fn a_size_is_the_whole_desktop_divided_between_the_monitors() {
        // Two monitors of a desktop 2560 wide are 1280 each, not 2560 each.
        assert_eq!(divided(2560, 800, 2), vec![(0, 0, 1280, 800), (1280, 0, 1280, 800)]);
        // The last takes what the division leaves.
        assert_eq!(divided(2561, 800, 2), vec![(0, 0, 1280, 800), (1280, 0, 1281, 800)]);
        // One monitor is the size itself.
        assert_eq!(divided(1600, 900, 1), vec![(0, 0, 1600, 900)]);
    }

    #[test]
    fn a_layout_is_put_into_the_shape_the_protocol_takes() {
        // Already one: unchanged.
        let fine = [(0, 0, 1280, 800), (1280, 0, 1600, 900)];
        assert_eq!(arranged(&fine), fine.to_vec());
        // An odd width loses a pixel, and the screen beside it moves along
        // by as much, so that the two still touch.
        assert_eq!(
            arranged(&[(0, 0, 1281, 800), (1281, 0, 1600, 900)]),
            vec![(0, 0, 1280, 800), (1280, 0, 1600, 900)]
        );
        // Too small grows to the protocol's minimum, and pushes its
        // neighbour.
        assert_eq!(
            arranged(&[(0, 0, 100, 100), (100, 0, 800, 600)]),
            vec![(0, 0, 200, 200), (200, 0, 800, 600)]
        );
        // The primary goes to the origin and the others keep their places
        // around it.
        assert_eq!(
            arranged(&[(1024, 0, 800, 600), (0, 0, 1024, 768)]),
            vec![(0, 0, 800, 600), (-1024, 0, 1024, 768)]
        );
    }

    #[test]
    fn a_layout_the_library_refuses_is_an_error() {
        assert_eq!(layout_of(&[(0, 0, 800, 600), (800, 0, 800, 600)]).unwrap().len(), 2);
        assert_eq!(layout_of(&[(0, 0, 800, 600), (0, 600, 800, 600)]).unwrap().len(), 2);
        // A gap between them, and an overlap.
        assert!(layout_of(&[(0, 0, 800, 600), (1000, 0, 800, 600)]).is_err());
        assert!(layout_of(&[(0, 0, 800, 600), (400, 0, 800, 600)]).is_err());
    }

    #[test]
    fn a_layout_is_the_answer_only_while_it_covers_the_desktop() {
        let two = [0, 0, 1280, 800, 1280, 0, 1600, 900];
        assert!(covers(&two, (2880, 900)));
        // The desktop changed size since: the layout no longer applies.
        assert!(!covers(&two, (2560, 800)));
        assert!(!covers(&[], (2880, 900)));
    }
}
