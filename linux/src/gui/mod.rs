// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The window: one winit event loop, one GPU device with a wgpu surface for
//! each window on it, egui drawn into them, and either the connection
//! dialog or a session on screen.
//!
//! winit, egui-winit and egui-wgpu are driven directly rather than through
//! eframe. eframe is less code for an ordinary application, but this one
//! needs what eframe wraps away: the window's raw handles for the shortcuts
//! inhibitor (`wayland.rs`), the X11 keyboard grab (`x11.rs`) and the
//! Wayland clipboard (`clipboard/`), device events for the raw motion a
//! captured pointer sends, focus and keyboard events as winit delivers them
//! (with the layout's logical key, which egui's events drop), and a say in
//! when the loop wakes: the backend's mailbox wakes it through an event-loop
//! proxy, and egui's own repaint requests set a deadline it waits for. That
//! is four things eframe would have to be worked around for, against a page
//! of surface setup to do without it.
//!
//! Everything that can be shared between windows is, so that a session can
//! show the far end in more than one: the GPU device, its queue and egui's
//! renderer ([`Gpu`]) are the process's, and so is the one egui context,
//! whose texture manager holds the desktop once whichever window draws it.
//! What is a window's own is its [`Surface`]: the wgpu surface, egui-winit's
//! state for it, and egui's viewport, the main window being the root one.
//!
//! A session shows the far end in the main window, and in a window of its
//! own for each monitor popped out of it (`session.rs`, `pane.rs`): the
//! session asks for those between frames ([`WindowOp`]) and this opens and
//! closes them. The main window is the one that matters most (the dialog,
//! the prompts and the panel are only ever there, and closing it ends the
//! session), and the rest come and go with the session: closed before it
//! ends, each letting go of what it held. What is the process's rather than
//! a window's follows the focus between them: X11's keyboard grab, the
//! pointer capture, the shortcuts inhibitor (one per surface, only the
//! focused one's asked for); the clipboard is read once when the focus
//! comes to one of the windows from elsewhere, and not when it moves
//! between them.
//!
//! The main thread never waits on a backend: a session is created
//! connecting and asked questions that return at once, and everything it
//! has to say arrives through the mailbox between frames.

mod clipboard;
mod csd;
mod cursor;
mod desktop;
mod dialog;
mod display_server;
mod follow;
mod hud;
mod icons;
mod keys;
mod panel;
mod prompts;
mod session;
mod settings;
mod toolbar;
mod view;
mod wayland;
mod x11;
mod zerocopy;

use crate::backends::{Dirs, Kind};
use crate::options::Options;
use clipboard::{Clipboard, Why};
use dialog::Dialog;
pub use display_server::DisplayServer;
use egui::ViewportId;
use session::{Session, Teardown, WindowOp};
pub use settings::Settings;
pub use view::ScaleMode;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use toolbar::Placement;
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{DeviceEvent, DeviceId, ElementState, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, DeviceEvents, EventLoop, EventLoopProxy};
use winit::window::{Icon, UserAttentionType, Window, WindowAttributes, WindowButtons, WindowId};

/// A connection to open, as the command line or the dialog describes one.
#[derive(Clone)]
pub struct Request {
    pub kind: &'static Kind,
    pub address: String,
    pub user: String,
    pub domain: String,
    pub password: Option<String>,
    pub options: Options,
}

/// What the window opens with.
pub struct Launch {
    /// What the command line described, if anything: the dialog is filled
    /// in from it, or, with `connect`, a session is opened on it at once.
    pub request: Option<Request>,
    pub connect: bool,
    pub dirs: Dirs,
    /// The client's own settings, from their flags.
    pub settings: Settings,
    /// The display server to open the window on, where the command line
    /// named one; `None` is winit's choice.
    pub display_server: Option<DisplayServer>,
    /// The windows draw their own title bars and resize edges (see
    /// `csd.rs`) rather than have the window manager or the compositor
    /// decorate them; off with `--no-csd`.
    pub csd: bool,
}

/// Why the loop was woken from another thread or from egui.
enum Wake {
    /// Something happened (the backend's mailbox, or egui wanting a frame
    /// at once): repaint one viewport's window, or with `None` every
    /// window, since the session's news may be any of theirs.
    Now(Option<ViewportId>),
    /// egui wants a frame later: only the deadlines are looked at again, in
    /// `about_to_wait`, in case the loop was waiting without one.
    Later,
}

/// What the session needs of the window, for a frame.
pub struct Host<'a> {
    pub window: &'a Window,
    /// The window's egui viewport, which is the session's pane for it.
    pub viewport: ViewportId,
    pub winit: &'a mut egui_winit::State,
    pub event_loop: &'a ActiveEventLoop,
    pub focused: bool,
    /// Where the keyboard shortcuts inhibitor is, for the debug overlay.
    pub inhibitor: &'static str,
    /// Which display server the window is on.
    pub display_server: DisplayServer,
    /// Where the pointer last was in the window, as the window saw it
    /// (not while the session holds it at the centre).
    pub pointer: Option<PhysicalPosition<f64>>,
    /// The local output the window is on, by name, for the overlay.
    pub output: Option<String>,
    /// The local clipboard, where there is one (see `clipboard/`).
    pub clipboard: Option<&'a Clipboard>,
    /// The GPU, for a picture left on it: only in a window's frame, and
    /// only where it can draw one.
    pub video: Option<Video<'a>>,
    /// The window draws its own decorations (see `csd.rs`).
    pub csd: bool,
    /// The window buttons the window manager or the compositor offers:
    /// close always, minimise and maximise where it does them.
    pub buttons: WindowButtons,
    /// X11's window manager, for its window menu.
    pub wm: Option<&'a x11::WindowManager>,
    /// The window's own close button was pressed in this frame, which is
    /// handled as the window manager's close is.
    pub close: bool,
}

/// Runs the window until it is closed; the process's exit status.
pub fn run(launch: Launch) -> i32 {
    let mut builder = EventLoop::<Wake>::with_user_event();
    // Fully qualified: both traits have a `with_any_thread` too.
    match launch.display_server {
        Some(DisplayServer::X11) => {
            winit::platform::x11::EventLoopBuilderExtX11::with_x11(&mut builder);
        }
        Some(DisplayServer::Wayland) => {
            winit::platform::wayland::EventLoopBuilderExtWayland::with_wayland(&mut builder);
        }
        _ => {}
    }
    let event_loop = match builder.build() {
        Ok(event_loop) => event_loop,
        Err(e) => {
            eprintln!("cannot open a window: {e}");
            return 1;
        }
    };
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();
    let mut app = App {
        dirs: launch.dirs.clone(),
        settings: launch.settings,
        csd: launch.csd,
        dialog_type: false,
        launch: Some(launch),
        placement: Placement::default(),
        proxy,
        gpu: None,
        egui: egui::Context::default(),
        windows: HashMap::new(),
        main: None,
        screen: Screen::Dialog(Dialog::new()),
        keyboard: None,
        clipboard: None,
        display_server: DisplayServer::Other,
        focused: None,
        focus_events: Vec::new(),
        modifiers: Default::default(),
        shown: true,
        repaint_at: Arc::new(Mutex::new(HashMap::new())),
        teardowns: Vec::new(),
        launched: false,
        exit_code: 0,
        clipboard_running: false,
    };
    let ran = event_loop.run_app(&mut app);
    let exit_code = app.exit_code;
    // The windows go first, and then whatever backend is still being
    // destroyed is given a moment: a process that exits under a protocol
    // thread takes the C libraries' exit handlers down with it, but a window
    // that stays up while it waits is the freeze `Teardown` exists to avoid.
    // Hidden rather than only dropped, because on X11 dropping one queues the
    // destroy without sending it, and the window stayed on screen until the
    // process was gone. (Wayland has no hiding; there it stays up for the
    // wait, which is the rare case of a backend that would not let go.)
    let teardowns = std::mem::take(&mut app.teardowns);
    if teardowns.iter().any(|t| !t.is_finished()) {
        for surface in app.windows.values() {
            surface.window.set_visible(false);
        }
    }
    if app.clipboard_running {
        // Its windows hold winit's connection, which the clipboard's thread
        // may still be using: never dropped, it stays open until the
        // process is gone, and the thread with it.
        log::warn!("the clipboard's thread did not stop; the windows are left to the process's exit");
        std::mem::forget(app);
    } else {
        drop(app);
    }
    let deadline = Instant::now() + TEARDOWN_PATIENCE;
    for teardown in teardowns {
        if !teardown.wait_until(deadline) {
            log::warn!("a backend was still being destroyed {TEARDOWN_PATIENCE:?} after the window closed; exiting anyway");
            break;
        }
    }
    if let Err(e) = ran {
        eprintln!("the event loop failed: {e}");
        return 1;
    }
    exit_code
}

/// How long the process waits, once the window has closed, for a backend
/// that is still being destroyed.
const TEARDOWN_PATIENCE: Duration = Duration::from_secs(2);

/// What every window shares: the device, its queue and egui's renderer,
/// whose textures (the desktop's among them) are uploaded once and drawn
/// into any surface. The instance and the adapter are kept for the
/// surfaces of windows opened later.
struct Gpu {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// The one format every surface is configured in: the renderer's
    /// pipeline was made for it.
    format: wgpu::TextureFormat,
    renderer: egui_wgpu::Renderer,
    /// Hardware pictures drawn from the GPU memory they were decoded into
    /// (`zerocopy.rs`), or why the device cannot.
    zerocopy: Result<zerocopy::Zerocopy, String>,
}

impl Gpu {
    /// What the session draws a picture left on the GPU with, where it can.
    fn video(&mut self) -> Option<Video<'_>> {
        let zerocopy = self.zerocopy.as_mut().ok()?;
        Some(Video {
            zerocopy,
            renderer: &mut self.renderer,
        })
    }
}

/// The GPU's half of drawing a picture left on it: the conversion, and the
/// renderer its texture is registered with.
pub struct Video<'a> {
    pub zerocopy: &'a mut zerocopy::Zerocopy,
    pub renderer: &'a mut egui_wgpu::Renderer,
}

/// One window and what is its own: where it is drawn, egui-winit's state
/// for its input, and what it has said about itself.
struct Surface {
    window: Arc<Window>,
    /// `None` once another client has destroyed the window, from when
    /// nothing may be drawn into it any more (see `window_event`).
    surface: Option<wgpu::Surface<'static>>,
    config: wgpu::SurfaceConfiguration,
    winit: egui_winit::State,
    /// egui's viewport for it: the root one for the main window.
    viewport: ViewportId,
    /// The last pointer position that was not swallowed (see `centred`).
    pointer: Option<PhysicalPosition<f64>>,
    /// The window cannot be seen, as winit's `Occluded` says: on X11 wholly
    /// covered, by its visibility events; on Wayland suspended, by the
    /// compositor.
    occluded: bool,
    /// winit's `is_minimized` when last asked: X11 says, elsewhere `None`.
    minimized: Option<bool>,
    /// X11's `GetWindowAttributes`, asked whether the window is viewable at
    /// all (see `App::sync_shown`); `None` elsewhere.
    viewable: Option<x11::Viewable>,
    /// What it last said, and when it is next due.
    viewability: x11::Viewability,
    /// X11's window manager, asked what it offers the window's own title
    /// bar; `None` elsewhere.
    wm: Option<x11::WindowManager>,
    /// The window buttons offered as of the last frame, for the log.
    buttons: Option<WindowButtons>,
    /// Wayland's keyboard shortcuts inhibitor, which is a surface's own;
    /// X11's grab is the client's, and the `App`'s.
    inhibitor: Option<wayland::Shortcuts>,
    /// The local output the window is on, by name: read when it opens and
    /// again whenever it moves or changes scale, which is also when an
    /// output that went away has had its windows moved elsewhere.
    output: Option<String>,
    /// Pointer events kept from egui until its next frame (see
    /// [`PointerSplit`]).
    held: VecDeque<WindowEvent>,
    split: PointerSplit,
}

/// Where egui's frames are split in a burst of pointer events. egui finds
/// the widget a press is on, and so what it clicks or drags, at the
/// pointer's position at the end of the frame the press is read in, not at
/// the press: a press and a quick move away in one frame went to whatever
/// the pointer was over after the move (a drag that started on the
/// connection panel went to the desktop, and so to the far end). And a
/// widget is not dragged in the frame its button is released, so moves
/// read in that frame are lost to it. A press is therefore the last
/// pointer event its frame reads, and a release the first; the events
/// that would break that wait for the next frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct PointerSplit {
    /// A pointer event has been given to egui since its last frame.
    fed: bool,
    /// The last of them was a press.
    pressed: bool,
}

impl PointerSplit {
    /// Whether a pointer event (`button`: `Some(true)` a press,
    /// `Some(false)` a release) can be given to egui in this frame, and if
    /// so, counted as given. `queued` is whether earlier ones are waiting,
    /// which this one then waits behind.
    fn admit(&mut self, button: Option<bool>, queued: bool) -> bool {
        if queued || self.pressed || (button == Some(false) && self.fed) {
            return false;
        }
        self.fed = true;
        self.pressed = button == Some(true);
        true
    }
}

impl Surface {
    /// An event given to egui, or, for a pointer event, held until its next
    /// frame where egui would misread it in this one (see [`PointerSplit`]).
    /// Whether a frame is wanted for it.
    fn give_egui(&mut self, event: WindowEvent) -> bool {
        let button = match &event {
            WindowEvent::MouseInput { state, .. } => Some(*state == ElementState::Pressed),
            WindowEvent::CursorMoved { .. }
            | WindowEvent::CursorEntered { .. }
            | WindowEvent::CursorLeft { .. }
            | WindowEvent::MouseWheel { .. }
            | WindowEvent::Touch(_) => None,
            _ => return self.winit.on_window_event(&self.window, &event).repaint,
        };
        if !self.split.admit(button, !self.held.is_empty()) {
            self.held.push_back(event);
            return true;
        }
        self.winit.on_window_event(&self.window, &event).repaint
    }

    /// Once a frame has taken egui's input, the events held from it given
    /// for the next. Whether there were any, and so a next frame is wanted.
    fn release_held(&mut self) -> bool {
        self.split = PointerSplit::default();
        let held = std::mem::take(&mut self.held);
        let any = !held.is_empty();
        for event in held {
            self.give_egui(event);
        }
        any
    }

    /// Whether the window can be seen (see `App::sync_shown`).
    fn shown(&self) -> bool {
        !self.occluded && self.minimized != Some(true) && !self.viewability.hidden()
    }

    /// Ask X whether the window is viewable, when an answer is due.
    fn ask_viewable(&mut self) {
        if let Some(viewable) = &self.viewable {
            let now = Instant::now();
            if self.viewability.due(now) && self.viewability.answered(viewable.ask(), now) {
                log::debug!("X11 map state: window {:?} {}", self.window.id(), self.signals());
            }
        }
    }

    /// Ask winit whether the window manager says minimised (X11's
    /// `_NET_WM_STATE_HIDDEN`), and log a change.
    fn ask_minimized(&mut self) {
        let minimized = self.window.is_minimized();
        if minimized != self.minimized {
            self.minimized = minimized;
            log::debug!("_NET_WM_STATE: window {:?} {}", self.window.id(), self.signals());
        }
    }

    /// The three signals `shown` is made of, for the log: which one said
    /// hidden is what a report of a window paused on screen needs.
    fn signals(&self) -> String {
        format!(
            "occluded {}, minimised {}, viewable {}",
            if self.occluded { "yes" } else { "no" },
            match self.minimized {
                Some(true) => "yes",
                Some(false) => "no",
                None => "unknown",
            },
            if self.viewability.hidden() { "no" } else { "yes" }
        )
    }
}

/// What a window shows where nothing else is drawn: behind the connection
/// dialog, and a session's around and before its desktop.
const BACKDROP: egui::Color32 = egui::Color32::from_gray(0x11);

/// The main window's size, in points, when it opens, and with its own
/// decorations when a session starts (see `App::grow`).
const WINDOW_SIZE: winit::dpi::LogicalSize<f64> = winit::dpi::LogicalSize::new(1280.0, 800.0);

/// The least height of a window, in points. The dialog and the panel
/// scroll, so this is what is kept in view rather than what their content
/// needs: the dialog's heading, a notice, and its connection fields.
const MIN_HEIGHT: f32 = 360.0;

/// The smallest a window may be made, in points: wide enough for the
/// connection panel and the connection dialog at its narrowest, which is
/// the panel's width, with [`panel::MARGIN`] at both sides; a prompt with
/// its frame; and the toolbar with all of its buttons.
fn min_size(style: &egui::Style) -> winit::dpi::LogicalSize<f32> {
    let panel = panel::WIDTH + 2.0 * panel::MARGIN;
    let prompt = prompts::WIDTH + egui::Frame::popup(style).total_margin().sum().x;
    let width = panel.max(prompt).max(session::toolbar_width(style));
    winit::dpi::LogicalSize::new(width.ceil(), MIN_HEIGHT)
}

/// The window icon, `metadata/icon.png` as `build.rs` decodes and scales it:
/// square, 8-bit RGBA rows.
const ICON: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/icon.rgba"));

/// The name window managers and compositors know the windows by: the
/// Wayland app ID, and both parts of X11's `WM_CLASS`.
const APP_ID: &str = "vento";

/// [`ICON`] as winit takes it. X11 shows it (`_NET_WM_ICON`); winit sets
/// nothing on Wayland, where the compositor looks the icon up by app ID.
fn window_icon() -> Icon {
    let side = (ICON.len() / 4).isqrt() as u32;
    Icon::from_rgba(ICON.to_vec(), side, side).expect("build.rs writes a square RGBA icon")
}

enum Screen {
    Dialog(Dialog),
    Session(Box<Session>),
}

/// What the window does once a session has ended.
#[derive(Debug, PartialEq, Eq)]
enum After {
    /// Back to the dialog, filled in, with the reason if there is one.
    Dialog,
    /// Close the window and exit, as closing it would.
    Quit,
}

/// `vento <backend> <address> --connect` is a session and nothing else, so
/// disconnecting it is the end of the program; any other ending of it (the
/// far end hanging up, an error) goes back to the dialog so that the reason
/// stays on screen, and every session after it is the dialog's.
/// `launched`: the session was the one `--connect` opened; `asked`: its
/// ending was a disconnect from this side (the toolbar's, or the Cancel
/// while connecting).
fn after_session(launched: bool, asked: bool) -> After {
    if launched && asked { After::Quit } else { After::Dialog }
}

/// What the focus events of one turn of the loop came to, between the
/// windows of this process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FocusChange<W> {
    /// The focus is where it was: no events, or ones that undid each other.
    Unchanged,
    /// None of our windows had it, and `W` has.
    Gained(W),
    /// `W` had it, and none of ours has.
    Lost(W),
    /// It went from one of our windows to another: nothing has left the
    /// process, so nothing is let go of: the keys held stay down, and the
    /// clipboard is not read again.
    Moved { from: W, to: W },
}

/// Where the focus is after `events`, in the order winit delivered them,
/// starting from `focused`, and what that change was.
///
/// Resolved once the loop has delivered everything it had rather than
/// event by event, because a move between two of our windows comes as a
/// focus-out from one and a focus-in to the other in the same batch
/// (measured on X11 in the same millisecond), and taken one at a time the
/// first would let go of the keys and the grab as if the process had lost
/// the focus. A focus-out from a window that no longer has it is stale and
/// changes nothing, whichever order the pair came in; a window said twice
/// to have it, as winit repeats itself around a keyboard grab, has it once.
fn resolve_focus<W: Copy + Eq>(focused: Option<W>, events: &[(W, bool)]) -> (Option<W>, FocusChange<W>) {
    let mut now = focused;
    for &(window, gained) in events {
        if gained {
            now = Some(window);
        } else if now == Some(window) {
            now = None;
        }
    }
    let change = match (focused, now) {
        (a, b) if a == b => FocusChange::Unchanged,
        (None, Some(to)) => FocusChange::Gained(to),
        (Some(from), None) => FocusChange::Lost(from),
        (Some(from), Some(to)) => FocusChange::Moved { from, to },
        (None, None) => unreachable!("equal, matched above"),
    };
    (now, change)
}

struct App {
    launch: Option<Launch>,
    dirs: Dirs,
    /// The settings a session starts with, and what the dialog edits; a
    /// session that changed them hands them back when it ends.
    settings: Settings,
    /// Where the toolbar sits, kept from one session to the next.
    placement: Placement,
    proxy: EventLoopProxy<Wake>,
    /// Made with the first window, whose surface the adapter is chosen for.
    gpu: Option<Gpu>,
    /// The one context every window's frames run in, each as its own
    /// viewport, so that they share its textures.
    egui: egui::Context,
    windows: HashMap<WindowId, Surface>,
    /// The main window: the root viewport, the one the dialog, the prompts
    /// and the connection panel are in, and whose closing ends it all.
    main: Option<WindowId>,
    screen: Screen,
    /// X11's keyboard grab, which a client holds on one of its windows at a
    /// time: on the focused one, moved when the focus moves.
    keyboard: Option<x11::Keyboard>,
    /// The local clipboard: read when the focus comes to one of the windows
    /// from elsewhere, set when the far end or egui copies (see
    /// `clipboard/`).
    clipboard: Option<Clipboard>,
    /// Which display server the window is on, once it is open.
    display_server: DisplayServer,
    /// Which of our windows has the keyboard focus, if any does.
    focused: Option<WindowId>,
    /// The focus events of this turn of the loop, resolved in
    /// `about_to_wait` (see `resolve_focus`).
    focus_events: Vec<(WindowId, bool)>,
    /// The modifiers held, as winit last said: for the paste command.
    modifiers: winit::keyboard::ModifiersState,
    /// The windows draw their own decorations (`Launch::csd`).
    csd: bool,
    /// The main window is still X11's dialog type it opened as, to be
    /// floated by a tiling window manager (see `open`).
    dialog_type: bool,
    /// Whether any window could be seen when last looked, which is what the
    /// session is told (see `sync_shown`).
    shown: bool,
    /// When egui last asked for each viewport to be repainted by.
    repaint_at: Arc<Mutex<HashMap<ViewportId, Instant>>>,
    /// The backends of sessions that have ended, still being destroyed.
    teardowns: Vec<Teardown>,
    /// The session on screen is the one `--connect` opened, which quits
    /// the window when it is disconnected (see `after_session`). Cleared
    /// when it ends, so a session started later from the dialog does not.
    launched: bool,
    exit_code: i32,
    /// The clipboard's thread did not stop when the event loop ended, and
    /// may still be on winit's Wayland connection (see `Clipboard::stop`).
    clipboard_running: bool,
}

impl App {
    fn open(&mut self, event_loop: &ActiveEventLoop) {
        // With its own decorations the dialog is the window, which opens
        // the dialog's width and is fitted to it once drawn; one opened for
        // a session is a session's size.
        let session = self.launch.as_ref().is_some_and(|l| l.connect && l.request.is_some());
        let compact = self.csd && !session;
        let size = if compact { winit::dpi::LogicalSize::new(dialog::WIDTH as f64, WINDOW_SIZE.height) } else { WINDOW_SIZE };
        let attributes = Window::default_attributes()
            .with_title("Vento")
            .with_inner_size(size)
            .with_min_inner_size(min_size(&self.egui.global_style()))
            .with_decorations(!self.csd);
        // A dialog as far as X11 window managers go, which tiling ones float
        // (i3); made an ordinary window when a session starts (see `grow`).
        let attributes = if compact {
            winit::platform::x11::WindowAttributesExtX11::with_x11_window_type(
                attributes,
                vec![winit::platform::x11::WindowType::Dialog],
            )
        } else {
            attributes
        };
        let id = match self.add_window(event_loop, attributes, ViewportId::ROOT) {
            Ok(id) => id,
            Err(e) => {
                eprintln!("{e}");
                self.exit_code = 1;
                event_loop.exit();
                return;
            }
        };
        self.main = Some(id);
        // The toolbar's icons, Material Symbols, as a font family of their
        // own (see icons.rs).
        self.egui.add_font(icons::font_insert());
        // No animations of egui's own: a dropdown, a tooltip or a window
        // is there the frame it is asked for, at full opacity, and gone
        // the frame it is closed, and a collapsing section opens at once.
        // A fade is a frame or two of something half-drawn over a desktop
        // that is itself moving, and costs repaints at a time when the
        // remote picture is what should be getting them. Every fade and
        // fold in egui takes its length from `animation_time`, which an
        // `Area`'s fade-in and a `Window`'s fade-out read as instant at
        // zero; a programmatic scroll has its own, `scroll_animation`.
        // The toolbar's slide names its own time (`animate_value_with_time`)
        // and stays.
        self.egui.all_styles_mut(|style| {
            style.animation_time = 0.0;
            style.scroll_animation = egui::style::ScrollAnimation::none();
        });
        // Every window but the main one is a viewport of egui's that this
        // loop draws itself, in a window it made: egui is never to draw one
        // as a window inside another, which it does by default for an
        // integration it believes cannot open windows.
        self.egui.set_embed_viewports(false);
        {
            // egui's repaint requests become a deadline the loop waits for,
            // one per viewport; the proxy wakes it in case it is already
            // waiting. A request for later must not draw at once: one made
            // from inside a frame (the session's once-a-second poll is one)
            // would otherwise draw the next frame straight away, and that
            // one ask again: a loop that a Wayland compositor's frame
            // callbacks slow to the refresh rate and X11 does not slow at
            // all.
            let repaint_at = Arc::clone(&self.repaint_at);
            let proxy = self.proxy.clone();
            self.egui.set_request_repaint_callback(move |info| {
                let at = Instant::now() + info.delay;
                let mut slots = repaint_at.lock().unwrap();
                let slot = slots.entry(info.viewport_id).or_insert(at);
                if at < *slot {
                    *slot = at;
                }
                let _ = proxy.send_event(if info.delay.is_zero() {
                    Wake::Now(Some(info.viewport_id))
                } else {
                    Wake::Later
                });
            });
        }
        let window = Arc::clone(&self.windows[&id].window);
        self.display_server = DisplayServer::of(&window);
        log::info!("display server: {}", self.display_server.name());
        if self.display_server == DisplayServer::X11 {
            // winit turns XI2's raw events off on every focus-out, and that
            // includes the ones our own keyboard grab causes, which are
            // filtered out below as not a focus change at all; relative
            // mode needs them back. The session drops motion it is not
            // capturing, so having them unfocused costs nothing.
            event_loop.listen_device_events(DeviceEvents::Always);
        }
        let proxy = self.proxy.clone();
        let wake: Box<dyn Fn() + Send + Sync> = Box::new(move || {
            let _ = proxy.send_event(Wake::Now(None));
        });
        self.clipboard = Clipboard::new(&window, self.display_server, wake);
        self.keyboard = x11::Keyboard::new(&window);
        if self.keyboard.is_some() {
            log::info!(
                "X11: the keyboard is grabbed while a session has focus and takes input, and not over the \
                 toolbar, unless the grab keyboard setting is off"
            );
        } else if self.windows[&id].inhibitor.is_some() {
            log::info!("Wayland: looking for the keyboard shortcuts inhibitor");
        } else {
            log::info!("no shortcuts inhibitor on this display server");
        }

        if let Some(launch) = self.launch.take() {
            self.dirs = launch.dirs;
            self.settings = launch.settings;
            if let Some(request) = launch.request {
                if launch.connect {
                    self.connect(request);
                    self.launched = matches!(self.screen, Screen::Session(_));
                } else {
                    self.screen = Screen::Dialog(Dialog::from_request(&request));
                }
            }
        }
        if let Screen::Dialog(dialog) = &mut self.screen {
            dialog.fit = compact;
        }
        self.dialog_type = compact && self.windows[&id].wm.is_some();
        window.request_redraw();
    }

    /// A window drawn with its own decorations, the dialog's size, made the
    /// size it opens at without them ([`WINDOW_SIZE`]) as its session
    /// starts, as large as the monitor it is on at most. It grows around
    /// the dialog, which ends up where it is drawn in a window of that size
    /// without the window's own decorations, though with its top edge on the
    /// monitor. Where the window can be placed (X11); on Wayland the
    /// compositor decides where a window's size changes from. Not while
    /// maximised or fullscreen.
    fn grow(window: &Window) {
        if window.is_maximized() || window.fullscreen().is_some() {
            return;
        }
        let scale = window.scale_factor();
        let monitor = window.current_monitor();
        let mut size = WINDOW_SIZE;
        if let Some(monitor) = &monitor {
            let room = monitor.size().to_logical::<f64>(scale);
            size.width = size.width.min(room.width);
            size.height = size.height.min(room.height);
        }
        let dialog = window.inner_size().to_logical::<f32>(scale);
        let within = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size.width as f32, size.height as f32));
        let offset = dialog::left_top(egui::vec2(dialog.width, dialog.height), within);
        log::info!("resizing the window to {}×{} for the session", size.width, size.height);
        Self::place(window, &monitor, -(offset.x as f64 * scale), -(offset.y as f64 * scale));
        let _ = window.request_inner_size(size);
    }

    /// A window drawn with its own decorations back from a session to the
    /// dialog: the dialog's width, placed where the dialog is drawn in the
    /// window as it is without its own decorations, and fitted to the
    /// dialog's height once that is measured (`Dialog::fit`). Not while
    /// maximised or fullscreen.
    fn shrink(window: &Window) {
        if window.is_maximized() || window.fullscreen().is_some() {
            return;
        }
        let scale = window.scale_factor();
        let inner = window.inner_size().to_logical::<f32>(scale);
        let within = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(inner.width, inner.height));
        let offset = dialog::left_top(egui::vec2(dialog::WIDTH, 0.0), within);
        Self::place(window, &window.current_monitor(), offset.x as f64 * scale, offset.y as f64 * scale);
        let _ = window.request_inner_size(winit::dpi::LogicalSize::new(dialog::WIDTH, inner.height));
    }

    /// Moves `window` by `dx`, `dy` device pixels where the display server
    /// lets it be placed, keeping its top edge no higher than the top of
    /// `monitor`.
    fn place(window: &Window, monitor: &Option<winit::monitor::MonitorHandle>, dx: f64, dy: f64) {
        let Ok(at) = window.outer_position() else {
            return;
        };
        let x = at.x + dx.round() as i32;
        let mut y = at.y + dy.round() as i32;
        if let Some(monitor) = monitor {
            y = y.max(monitor.position().y);
        }
        window.set_outer_position(PhysicalPosition::new(x, y));
    }

    /// Opens a window as `viewport`, with the window icon, the app ID and a
    /// surface on the shared device (made now, for the first window, with
    /// the adapter chosen to suit its surface).
    fn add_window(
        &mut self,
        event_loop: &ActiveEventLoop,
        attributes: WindowAttributes,
        viewport: ViewportId,
    ) -> Result<WindowId, String> {
        // One name for both: winit keeps a single one for Wayland and X11.
        let attributes = winit::platform::wayland::WindowAttributesExtWayland::with_name(attributes, APP_ID, APP_ID);
        let window = event_loop
            .create_window(attributes.with_window_icon(Some(window_icon())))
            .map_err(|e| format!("cannot create the window: {e}"))?;
        let window = Arc::new(window);
        let surface = match &self.gpu {
            None => {
                let (gpu, surface) = pollster::block_on(Gpu::new(&window))
                    .map_err(|e| format!("cannot set up the GPU: {e}"))?;
                self.gpu = Some(gpu);
                surface
            }
            Some(gpu) => gpu
                .instance
                .create_surface(Arc::clone(&window))
                .map_err(|e| format!("cannot make the window's surface: {e}"))?,
        };
        let gpu = self.gpu.as_ref().expect("made above");
        let config = gpu.configure(&window, &surface)?;
        let winit = egui_winit::State::new(
            self.egui.clone(),
            viewport,
            &*window,
            Some(window.scale_factor() as f32),
            None,
            Some(gpu.device.limits().max_texture_dimension_2d as usize),
        );
        let id = window.id();
        self.windows.insert(
            id,
            Surface {
                inhibitor: wayland::Shortcuts::new(&window),
                output: window.current_monitor().and_then(|m| m.name()),
                viewable: x11::Viewable::new(&window),
                viewability: x11::Viewability::default(),
                wm: x11::WindowManager::new(&window),
                buttons: None,
                window,
                surface: Some(surface),
                config,
                winit,
                viewport,
                pointer: None,
                occluded: false,
                minimized: None,
                held: VecDeque::new(),
                split: PointerSplit::default(),
            },
        );
        Ok(id)
    }

    /// The main window, while it is open.
    fn main_surface(&mut self) -> Option<&mut Surface> {
        self.windows.get_mut(&self.main?)
    }

    /// The window drawing `viewport`, if one is.
    fn window_of(&self, viewport: ViewportId) -> Option<&Arc<Window>> {
        self.windows.values().find(|s| s.viewport == viewport).map(|s| &s.window)
    }

    /// Starts a session, or shows why one could not be started.
    fn connect(&mut self, request: Request) {
        let Some(window) = self.main_surface().map(|s| Arc::clone(&s.window)) else {
            return;
        };
        let size = window.inner_size();
        let proxy = self.proxy.clone();
        let wake: Box<dyn Fn() + Send + Sync> = Box::new(move || {
            let _ = proxy.send_event(Wake::Now(None));
        });
        let zerocopy = match self.gpu.as_ref().map(|g| &g.zerocopy) {
            Some(Ok(_)) => Ok(()),
            Some(Err(why)) => Err(why.clone()),
            None => Err("no GPU yet".to_string()),
        };
        match Session::start(
            request.clone(),
            &self.dirs,
            self.settings,
            self.placement,
            (size.width, size.height),
            zerocopy,
            wake,
        ) {
            Ok(mut session) => {
                if !self.shown {
                    session.shown(false);
                }
                if self.dialog_type {
                    self.dialog_type = false;
                    if let Some(wm) = self.main.and_then(|id| self.windows.get(&id)).and_then(|s| s.wm.as_ref()) {
                        wm.make_normal(&window);
                    }
                }
                if self.csd {
                    Self::grow(&window);
                }
                self.screen = Screen::Session(Box::new(session));
            }
            Err(e) => {
                let mut dialog = Dialog::from_request(&request);
                dialog.notice = Some(format!("Could not connect: {e}"));
                self.screen = Screen::Dialog(dialog);
            }
        }
        window.request_redraw();
    }

    fn session(&mut self) -> Option<&mut Session> {
        match &mut self.screen {
            Screen::Session(session) => Some(session),
            Screen::Dialog(_) => None,
        }
    }

    /// Keep the shortcuts inhibitor in step with the focus and the session:
    /// held while one of the windows has the focus and the session takes
    /// input, released otherwise. Not merely while connected: in view only,
    /// or under a prompt, the keys go nowhere at the far end, and the grab
    /// would keep Alt+Tab and the rest from the window manager too. Released
    /// while the pointer is on the toolbar as well, and never taken with the
    /// grab keyboard setting off (`Session::holds_shortcuts`). This runs
    /// after every frame and on focus changes, but the grab and the
    /// inhibitor act only when what is wanted changes, so a pointer resting
    /// on the toolbar costs nothing.
    ///
    /// Wayland's inhibitor is a surface's, and only the focused window's is
    /// asked for; X11's grab is the client's, and goes to the focused
    /// window.
    fn sync_shortcuts(&mut self) {
        let wanted = self.focused.filter(|id| {
            let pane = self.windows.get(id).map(|s| s.viewport);
            matches!((&self.screen, pane), (Screen::Session(s), Some(pane)) if s.holds_shortcuts(pane))
        });
        for (id, surface) in &mut self.windows {
            if let Some(inhibitor) = &mut surface.inhibitor {
                inhibitor.pump();
                if wanted == Some(*id) {
                    inhibitor.inhibit();
                } else {
                    inhibitor.release();
                }
            }
        }
        if let Some(keyboard) = &mut self.keyboard {
            keyboard.pump();
            match wanted.and_then(|id| self.windows.get(&id)) {
                Some(surface) => {
                    keyboard.retarget(&surface.window);
                    keyboard.inhibit();
                    // A refused X grab is asked for again when due, without
                    // waiting for input to wake the loop.
                    if let Some(at) = keyboard.retry_at() {
                        let mut slots = self.repaint_at.lock().unwrap();
                        let slot = slots.entry(surface.viewport).or_insert(at);
                        if at < *slot {
                            *slot = at;
                        }
                    }
                }
                None => keyboard.release(),
            }
        }
    }

    /// Wayland: dispatch what the compositor said to each inhibitor. X11:
    /// retry a refused grab once it is due.
    fn pump_shortcuts(&mut self) {
        for surface in self.windows.values_mut() {
            if let Some(inhibitor) = &mut surface.inhibitor {
                inhibitor.pump();
            }
        }
        if let Some(keyboard) = &mut self.keyboard {
            keyboard.pump();
        }
    }

    /// Tell the session whether it can be seen, which is what lets a
    /// backend pause: not while a window merely lacks the keyboard focus,
    /// which leaves the picture on screen, but while every window it is in
    /// is minimised or wholly covered. One window that can be seen keeps
    /// the session from pausing.
    ///
    /// What can be told depends on the display server. X11 says both: the
    /// window manager's `_NET_WM_STATE_HIDDEN`, asked on every frame (a
    /// minimised session still runs one a second, for the poll, so it is
    /// seen within a second, or at once with the focus-out a minimise
    /// usually brings) with no round trip to the server, since winit keeps
    /// the property from its change notifications (`patches/winit/0005`);
    /// and the server's visibility events, which winit reports as
    /// `Occluded`. Under a compositing manager the window is never reported
    /// covered, and then only minimising pauses. Neither
    /// says anything of a window whose frame the window manager unmapped
    /// (i3's workspaces not on any output and its scratchpad, Openbox's
    /// other desktops): X sends no visibility event for a window that
    /// stops being viewable, and i3 sets `_NET_WM_STATE_HIDDEN` only on an
    /// inactive tab. So the server is asked whether the window is viewable
    /// at all, once a second while frames run (`x11::Viewable`), and the
    /// visibility event the frame's remap brings ends that at once. Wayland
    /// has no minimised state, but xdg-shell v6 has `suspended`, which a
    /// compositor may send while the window cannot be seen at all
    /// (minimised, on a workspace not shown), and which our winit patch
    /// reports as `Occluded`. A compositor that does not send it (sway
    /// 1.11, niri) never pauses a session: a hidden window's frame
    /// callbacks stop, and so do its frames, but the backend is not told
    /// and keeps receiving.
    fn sync_shown(&mut self) {
        if self.windows.is_empty() {
            return;
        }
        let shown = self.windows.values().any(Surface::shown);
        if shown != self.shown {
            self.shown = shown;
            for (id, surface) in &self.windows {
                log::info!("window {id:?}: {}", surface.signals());
            }
            if let Screen::Session(session) = &mut self.screen {
                session.shown(shown);
            }
        }
    }

    /// The inhibitor's state in a word, for the session's overlay in `id`.
    fn inhibitor(&self, id: WindowId) -> &'static str {
        if let Some(keyboard) = &self.keyboard {
            return match self.windows.get(&id) {
                Some(surface) => keyboard.state_in(&surface.window),
                None => keyboard.state(),
            };
        }
        self.windows
            .get(&id)
            .and_then(|s| s.inhibitor.as_ref())
            .map(wayland::Shortcuts::state)
            .unwrap_or("none")
    }

    /// The window's surface made `size`, in device pixels, and the session
    /// told.
    fn resize_surface(&mut self, id: WindowId, size: PhysicalSize<u32>) {
        if let (Some(gpu), Some(surface)) = (&self.gpu, self.windows.get_mut(&id))
            && size.width > 0
            && size.height > 0
        {
            surface.config.width = size.width;
            surface.config.height = size.height;
            if let Some(s) = &surface.surface {
                s.configure(&gpu.device, &surface.config);
            }
        }
        if let Screen::Session(session) = &mut self.screen {
            session.resized();
        }
    }

    fn draw(&mut self, event_loop: &ActiveEventLoop, id: WindowId) {
        // A size the window asked for itself is applied at once on Wayland,
        // where winit sends no `Resized` for it: the surface follows the
        // window's size here, or the compositor scales the old one to it.
        let Some(size) = self.windows.get(&id).map(|s| s.window.inner_size()) else {
            return;
        };
        if size.width > 0
            && size.height > 0
            && self.windows.get(&id).is_some_and(|s| (s.config.width, s.config.height) != (size.width, size.height))
        {
            self.resize_surface(id, size);
        }
        let Some(surface) = self.windows.get_mut(&id) else {
            return;
        };
        surface.ask_minimized();
        surface.ask_viewable();
        let viewport = surface.viewport;
        self.sync_shown();
        self.repaint_at.lock().unwrap().remove(&viewport);
        self.pump_shortcuts();
        let inhibitor = self.inhibitor(id);
        let focused = self.focused == Some(id);
        // The read of the local clipboard is handled in the window it is
        // for: the focused one, where a Ctrl+V asked for it, or the main
        // window when none of ours has the focus.
        let takes_clipboard = focused || (self.focused.is_none() && viewport == ViewportId::ROOT);
        // The other windows' viewports, declared in the main window's pass.
        let children: Vec<ViewportId> = if viewport == ViewportId::ROOT {
            self.windows.values().map(|s| s.viewport).filter(|v| *v != ViewportId::ROOT).collect()
        } else {
            Vec::new()
        };
        let (Some(gpu), Some(surface)) = (self.gpu.as_mut(), self.windows.get_mut(&id)) else {
            return;
        };
        let window = Arc::clone(&surface.window);

        // A read of the local clipboard that has come back: pasted where a
        // Ctrl+V asked for it, offered to the far end where the focus
        // coming back did, and egui's own clipboard kept in step.
        if takes_clipboard && let Some((text, why)) = self.clipboard.as_ref().and_then(Clipboard::take) {
            if let Some(text) = &text {
                surface.winit.set_clipboard_text(text.clone());
                // As egui-winit pastes: CRLF made LF, nothing for nothing.
                if why.paste && !text.is_empty() {
                    surface
                        .winit
                        .egui_input_mut()
                        .events
                        .push(egui::Event::Paste(text.replace("\r\n", "\n")));
                }
            }
            if why.offer
                && let Screen::Session(session) = &mut self.screen
            {
                session.local_clipboard(text);
            }
        }
        let raw_input = surface.winit.take_egui_input(&window);
        if surface.release_held() {
            window.request_redraw();
        }
        let buttons = match &mut surface.wm {
            Some(wm) => wm.buttons(),
            None => window.enabled_buttons(),
        };
        if self.csd && surface.buttons != Some(buttons) {
            surface.buttons = Some(buttons);
            log::debug!("window {id:?}: window buttons offered: {buttons:?}");
        }
        let mut next: Option<Request> = None;
        let mut ended: Option<(String, bool)> = None;
        let mut host = Host {
            window: &window,
            viewport,
            winit: &mut surface.winit,
            event_loop,
            focused,
            inhibitor,
            display_server: self.display_server,
            pointer: surface.pointer,
            output: surface.output.clone(),
            clipboard: self.clipboard.as_ref(),
            video: gpu.video(),
            csd: self.csd,
            buttons,
            wm: surface.wm.as_ref(),
            close: false,
        };
        let egui = self.egui.clone();
        let settings = &mut self.settings;
        let mut output = egui.run_ui(raw_input, |ui| {
            // egui keeps a viewport (its input, its memory of areas and of
            // what the pointer is on) only while the root's pass says it is
            // still wanted, and forgets the rest when that pass ends. Every
            // other window is said to be here, with a callback egui is never
            // to call (its pass is run from this loop, in its own window's
            // turn), so that each keeps its state between its own frames.
            for child in &children {
                ui.ctx()
                    .show_viewport_deferred(*child, egui::ViewportBuilder::default(), |_, _| {});
            }
            match &mut self.screen {
                // The dialog is the main window's alone.
                Screen::Dialog(_) if viewport != ViewportId::ROOT => {}
                Screen::Dialog(dialog) => {
                    next = dialog.ui(ui, settings, &mut host);
                }
                Screen::Session(session) => {
                    session.frame(ui, &mut host);
                    if let Some(detail) = &session.closed {
                        ended = Some((detail.clone(), session.disconnect_asked()));
                    }
                }
            }
        });
        // What egui copied (a text field's selection) becomes the local
        // clipboard; egui-winit still keeps it as its own as well.
        if let Some(clipboard) = &self.clipboard {
            for command in &output.platform_output.commands {
                if let egui::OutputCommand::CopyText(text) = command {
                    clipboard.set(text.clone());
                }
            }
        }
        // The window's cursor: egui-winit's icon, and the far end's shape
        // over it where the session wants that: put back before, applied
        // after, so the two never fight (see `cursor.rs`).
        let icon = output.platform_output.cursor_icon;
        if let Screen::Session(session) = &mut self.screen {
            session.cursor_before(&mut host);
        }
        host.winit.handle_platform_output(&window, output.platform_output);
        if let Screen::Session(session) = &mut self.screen {
            session.cursor_after(&mut host, icon);
        }
        let close = host.close;

        for (id, deltas) in &output.textures_delta.set {
            for delta in deltas {
                gpu.renderer.update_texture(&gpu.device, &gpu.queue, *id, delta);
            }
        }
        let paint_jobs = egui.tessellate(output.shapes, output.pixels_per_point);
        let presented = gpu.present(surface, &paint_jobs, output.pixels_per_point);
        // The overlay's render latency ends here: the frame's pixels have
        // been submitted and the surface handed to the compositor, which
        // is as far as this end can follow them.
        if let Screen::Session(session) = &mut self.screen {
            session.presented(viewport, presented);
        }
        for id in &output.textures_delta.free {
            gpu.renderer.free_texture(id);
        }
        // Applied above; epaint asserts that none is dropped unapplied.
        output.textures_delta.clear();

        // What the frame asked of the other windows: to draw what it took
        // off the mailbox, which they have not drawn yet; and the main
        // window, a prompt that has come up there while another window may
        // be the one in front.
        let (redraw_others, attention) = match &mut self.screen {
            Screen::Session(session) => (session.take_redraw_others(), session.take_attention()),
            Screen::Dialog(_) => (false, false),
        };
        if redraw_others {
            for (other, surface) in &self.windows {
                if *other != id {
                    surface.window.request_redraw();
                }
            }
        }
        if attention
            && let Some(main) = self.main
            && let Some(surface) = self.windows.get(&main)
        {
            surface.window.request_redraw();
            if self.windows.len() > 1 && self.focused != Some(main) {
                log::info!("a prompt is up in the main window; asking for attention to it");
                surface.window.request_user_attention(Some(UserAttentionType::Informational));
            }
        }

        // After the frame, so that the session's last picture was drawn.
        if let Some((detail, asked)) = ended {
            match after_session(self.launched, asked) {
                After::Quit => {
                    log::info!("the session --connect opened was disconnected; quitting");
                    self.close(event_loop);
                    return;
                }
                After::Dialog => self.end_session(&detail, event_loop),
            }
        }
        if let Some(request) = next {
            self.connect(request);
        }
        // The title bar's close, as the window manager's (see
        // `window_event`).
        if close {
            if Some(id) == self.main {
                self.close(event_loop);
                return;
            }
            self.close_window(event_loop, id, true);
        }
        self.window_ops(event_loop);
        self.sync_shortcuts();
    }

    /// Opens and closes the windows the session asked for in its frame.
    fn window_ops(&mut self, event_loop: &ActiveEventLoop) {
        let ops = match &mut self.screen {
            Screen::Session(session) => session.take_window_ops(),
            Screen::Dialog(_) => return,
        };
        if ops.is_empty() {
            return;
        }
        for op in ops {
            match op {
                WindowOp::Open { pane, title, size } => {
                    let attributes = Window::default_attributes()
                        .with_title(title)
                        .with_inner_size(winit::dpi::LogicalSize::new(size.0, size.1))
                        .with_min_inner_size(min_size(&self.egui.global_style()))
                        .with_decorations(!self.csd);
                    match self.add_window(event_loop, attributes, pane) {
                        Ok(id) => self.windows[&id].window.request_redraw(),
                        Err(e) => {
                            log::warn!("{e}");
                            if let Some(session) = self.session() {
                                session.forget_pane(pane, &e);
                            }
                        }
                    }
                }
                WindowOp::Close { pane, returned } => {
                    let found = self.windows.iter().find(|(_, s)| s.viewport == pane).map(|(id, _)| *id);
                    match found {
                        Some(id) if Some(id) != self.main => self.close_window(event_loop, id, returned),
                        _ => {}
                    }
                }
            }
        }
        // The main window's pass is where egui is told which viewports
        // there are now.
        if let Some(surface) = self.main_surface() {
            surface.window.request_redraw();
        }
    }

    /// Closes pop-out `id`: the session lets go of what its pane held (the
    /// capture, a drag, its cursor) and forgets it, `returned` handing its
    /// monitor back to the main window, and the window goes. A window that
    /// had the focus takes it with it, as far as the session can tell: the
    /// keys held are let go of, and the window manager says where the focus
    /// goes next.
    fn close_window(&mut self, event_loop: &ActiveEventLoop, id: WindowId, returned: bool) {
        let Some(viewport) = self.windows.get(&id).map(|s| s.viewport) else {
            return;
        };
        if let Some(()) = self.with_host(event_loop, id, |session, host, egui| session.close_pane(egui, host, returned)) {
            log::info!("closed a pop-out{}", if returned { "; its monitor is back in the main window" } else { "" });
        }
        if let Some(surface) = self.windows.remove(&id) {
            // Hidden first: on X11 a dropped window's destroy waits in the
            // queue for the next request that flushes it.
            surface.window.set_visible(false);
        }
        self.repaint_at.lock().unwrap().remove(&viewport);
        self.focus_events.retain(|(w, _)| *w != id);
        if self.focused == Some(id) {
            self.focused = None;
            if let Some(session) = self.session() {
                session.focus_lost();
            }
        }
        if let Some(surface) = self.main_surface() {
            surface.window.request_redraw();
        }
        self.sync_shortcuts();
        self.sync_shown();
    }

    /// Every window but the main one closed, before the session ends.
    fn close_popouts(&mut self, event_loop: &ActiveEventLoop) {
        let popouts: Vec<WindowId> = self.windows.keys().copied().filter(|id| Some(*id) != self.main).collect();
        for id in popouts {
            self.close_window(event_loop, id, false);
        }
    }

    /// Runs `f` on the session with a `Host` for window `id`, if there is a
    /// session and the window is open.
    fn with_host<R>(
        &mut self,
        event_loop: &ActiveEventLoop,
        id: WindowId,
        f: impl FnOnce(&mut Session, &mut Host, &egui::Context) -> R,
    ) -> Option<R> {
        let inhibitor = self.inhibitor(id);
        let focused = self.focused == Some(id);
        let Screen::Session(session) = &mut self.screen else {
            return None;
        };
        let surface = self.windows.get_mut(&id)?;
        let window = Arc::clone(&surface.window);
        let mut host = Host {
            window: &window,
            viewport: surface.viewport,
            winit: &mut surface.winit,
            event_loop,
            focused,
            inhibitor,
            display_server: self.display_server,
            pointer: surface.pointer,
            output: surface.output.clone(),
            clipboard: self.clipboard.as_ref(),
            video: None,
            csd: self.csd,
            buttons: WindowButtons::all(),
            wm: None,
            close: false,
        };
        Some(f(session, &mut host, &self.egui))
    }

    /// Back to the dialog, filled in with what was connected to, and told
    /// why it ended where there is a why.
    fn end_session(&mut self, detail: &str, event_loop: &ActiveEventLoop) {
        let Some(main) = self.main else {
            return;
        };
        self.close_popouts(event_loop);
        let inhibitor = self.inhibitor(main);
        let focused = self.focused == Some(main);
        let Some(surface) = self.windows.get_mut(&main) else {
            return;
        };
        let window = Arc::clone(&surface.window);
        let screen = std::mem::replace(&mut self.screen, Screen::Dialog(Dialog::new()));
        if let Screen::Session(session) = screen {
            let mut dialog = Dialog::from_request(&session.request);
            if !detail.is_empty() {
                dialog.notice = Some(format!("Disconnected: {detail}"));
            }
            if self.csd {
                Self::shrink(&window);
                dialog.fit = true;
            }
            let mut host = Host {
                window: &window,
                viewport: surface.viewport,
                winit: &mut surface.winit,
                event_loop,
                focused,
                inhibitor,
                display_server: self.display_server,
                pointer: surface.pointer,
                output: surface.output.clone(),
                clipboard: self.clipboard.as_ref(),
                video: None,
                csd: self.csd,
                buttons: WindowButtons::all(),
                wm: None,
                close: false,
            };
            self.settings = session.settings;
            self.placement = session.placement();
            let teardown = session.shutdown(&self.egui, &mut host);
            self.reap(teardown);
            self.forget_video();
            self.screen = Screen::Dialog(dialog);
        }
        self.launched = false;
        window.set_title("Vento");
        window.request_redraw();
    }

    fn close(&mut self, event_loop: &ActiveEventLoop) {
        self.close_popouts(event_loop);
        if let Some(main) = self.main {
            let inhibitor = self.inhibitor(main);
            let focused = self.focused == Some(main);
            if let Some(surface) = self.windows.get_mut(&main) {
                let window = Arc::clone(&surface.window);
                let screen = std::mem::replace(&mut self.screen, Screen::Dialog(Dialog::new()));
                if let Screen::Session(session) = screen {
                    let mut host = Host {
                        window: &window,
                        viewport: surface.viewport,
                        winit: &mut surface.winit,
                        event_loop,
                        focused,
                        inhibitor,
                        display_server: self.display_server,
                        pointer: surface.pointer,
                        output: surface.output.clone(),
                        clipboard: self.clipboard.as_ref(),
                        video: None,
                        csd: self.csd,
                        buttons: WindowButtons::all(),
                        wm: None,
                        close: false,
                    };
                    let teardown = session.shutdown(&self.egui, &mut host);
                    self.reap(teardown);
                    self.forget_video();
                }
            }
        }
        self.keyboard = None;
        for surface in self.windows.values_mut() {
            surface.inhibitor = None;
        }
        event_loop.exit();
    }

    /// What an ended session left on the GPU let go of: the pictures'
    /// imports, which hold their surfaces' memory, and their texture.
    fn forget_video(&mut self) {
        if let Some(Video { zerocopy, renderer }) = self.gpu.as_mut().and_then(Gpu::video) {
            zerocopy.forget(renderer);
        }
    }

    /// Keeps `teardown` until it has finished, and forgets the ones that have.
    fn reap(&mut self, teardown: Teardown) {
        self.teardowns.retain(|t| !t.is_finished());
        self.teardowns.push(teardown);
    }

    /// The focus events of this turn of the loop, resolved (see
    /// `resolve_focus`): the session and egui told of what changed, the
    /// clipboard read where the focus came from outside, the shortcuts and
    /// the windows' visibility brought up to date.
    fn settle_focus(&mut self, event_loop: &ActiveEventLoop) {
        if self.focus_events.is_empty() {
            return;
        }
        let mut events = std::mem::take(&mut self.focus_events);
        events.retain(|(id, _)| self.windows.contains_key(id));
        let (now, change) = resolve_focus(self.focused, &events);
        self.focused = now;
        // egui hears of each window's own change; the session only of the
        // focus coming to the process or leaving it.
        let (lost, gained, session) = match change {
            FocusChange::Unchanged => return,
            FocusChange::Gained(to) => (None, Some(to), Some((to, true))),
            FocusChange::Lost(from) => (Some(from), None, Some((from, false))),
            FocusChange::Moved { from, to } => {
                log::debug!("the focus moved between our windows; nothing let go of");
                // The capture goes with it in one step, carrying the
                // buttons held, rather than let go of in one window's frame
                // and taken in the other's.
                if let (Screen::Session(session), Some(f), Some(t)) =
                    (&mut self.screen, self.windows.get(&from), self.windows.get(&to))
                {
                    session.hand_over_capture((&f.window, f.viewport), (&t.window, t.viewport), self.display_server, t.pointer);
                }
                (Some(from), Some(to), None)
            }
        };
        for (id, focused) in lost.map(|id| (id, false)).into_iter().chain(gained.map(|id| (id, true))) {
            if let Some(surface) = self.windows.get_mut(&id) {
                let window = Arc::clone(&surface.window);
                let _ = surface.winit.on_window_event(&window, &WindowEvent::Focused(focused));
                window.request_redraw();
            }
        }
        if let Some((id, focused)) = session {
            let inhibitor = self.inhibitor(id);
            if let (Screen::Session(session), Some(surface)) = (&mut self.screen, self.windows.get_mut(&id)) {
                let window = Arc::clone(&surface.window);
                let mut host = Host {
                    window: &window,
                    viewport: surface.viewport,
                    winit: &mut surface.winit,
                    event_loop,
                    focused,
                    inhibitor,
                    display_server: self.display_server,
                    pointer: surface.pointer,
                    output: surface.output.clone(),
                    clipboard: self.clipboard.as_ref(),
                    video: None,
                    csd: self.csd,
                    buttons: WindowButtons::all(),
                    wm: None,
                    close: false,
                };
                session.focus(focused, &mut host);
            }
            // What was copied elsewhere while the windows were away: read
            // on the clipboard's thread, and handled when it comes back
            // (see `draw`), dialog or session.
            if focused && let Some(clipboard) = &self.clipboard {
                clipboard.read(Why::OFFER);
            }
        }
        self.sync_shortcuts();
        // A minimise usually takes the focus with it: seen now rather than
        // at the next frame.
        for surface in self.windows.values_mut() {
            surface.ask_minimized();
        }
        self.sync_shown();
    }

    /// Asks for a frame in every window whose deadline has come; the
    /// earliest still to come, if any.
    fn redraw_due(&self) -> Option<Instant> {
        let now = Instant::now();
        let slots = self.repaint_at.lock().unwrap();
        let mut next: Option<Instant> = None;
        for (viewport, &at) in slots.iter() {
            if at <= now {
                if let Some(window) = self.window_of(*viewport) {
                    window.request_redraw();
                }
            } else if next.is_none_or(|t| at < t) {
                next = Some(at);
            }
        }
        next
    }
}

impl ApplicationHandler<Wake> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.main.is_none() {
            self.open(event_loop);
        }
    }

    fn user_event(&mut self, _: &ActiveEventLoop, wake: Wake) {
        match wake {
            Wake::Now(Some(viewport)) => {
                if let Some(window) = self.window_of(viewport) {
                    window.request_redraw();
                }
            }
            Wake::Now(None) => {
                for surface in self.windows.values() {
                    surface.window.request_redraw();
                }
            }
            Wake::Later => {}
        }
    }

    fn new_events(&mut self, _: &ActiveEventLoop, cause: StartCause) {
        if let StartCause::ResumeTimeReached { .. } = cause {
            self.redraw_due();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some((window, viewport)) = self.windows.get(&id).map(|s| (Arc::clone(&s.window), s.viewport)) else {
            return;
        };
        match &event {
            // A pop-out closed by the window manager is given back, as its
            // Return does; the main window closed is the end of it all.
            WindowEvent::CloseRequested if Some(id) != self.main => {
                self.close_window(event_loop, id, true);
                return;
            }
            WindowEvent::CloseRequested => {
                self.close(event_loop);
                return;
            }
            WindowEvent::Destroyed => {
                // Another client destroyed the window, as `xdotool
                // windowclose` or a window manager's kill does on X11:
                // closed as if asked, but with the surface let go first,
                // since a draw before the loop ends would configure one
                // whose window is gone, which wgpu takes for fatal.
                log::info!("the window was destroyed from outside; closing");
                if let Some(surface) = self.windows.get_mut(&id) {
                    surface.surface = None;
                }
                if Some(id) != self.main {
                    self.close_window(event_loop, id, true);
                } else {
                    self.close(event_loop);
                }
                return;
            }
            WindowEvent::RedrawRequested => {
                self.draw(event_loop, id);
                return;
            }
            WindowEvent::Resized(size) => {
                if let Some(wm) = self.windows.get_mut(&id).and_then(|s| s.wm.as_mut()) {
                    wm.stale();
                }
                self.resize_surface(id, *size);
            }
            WindowEvent::Moved(_) | WindowEvent::ScaleFactorChanged { .. } => {
                // Moved to another output, or its output went away and the
                // display server re-homed it: which one it is now.
                if let Some(surface) = self.windows.get_mut(&id) {
                    surface.output = window.current_monitor().and_then(|m| m.name());
                }
                if let Screen::Session(session) = &mut self.screen {
                    session.refence();
                }
            }
            WindowEvent::Focused(focused) => {
                // On X11 the keyboard grab makes focus events of its own,
                // which are not the focus moving (see `x11.rs`): neither
                // the session nor egui hears of them.
                // Asked of every window of ours: the focus gone from one to
                // another is not the process losing it either, and the
                // focus-in that says where it went is enough.
                let ours: Vec<&Window> = self.windows.values().map(|s| &*s.window).collect();
                if !*focused && self.keyboard.as_ref().is_some_and(|k| k.focus_still_ours(&ours)) {
                    log::debug!("X11: a focus-out to our own keyboard grab or another of our windows; still ours");
                    return;
                }
                // Resolved with the rest of this turn's in `about_to_wait`,
                // and egui told there: a move between two of our windows is
                // two events, and only together are they a move.
                self.focus_events.push((id, *focused));
                return;
            }
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::Occluded(occluded) => {
                if let Some(surface) = self.windows.get_mut(&id) {
                    surface.occluded = *occluded;
                    // X sends a visibility event only for a viewable window:
                    // a frame mapped again is seen now, not at the next ask.
                    surface.viewability.seen();
                    surface.ask_minimized();
                    log::debug!(
                        "occluded: window {id:?} {} ({})",
                        if *occluded { "wholly covered" } else { "visible" },
                        surface.signals()
                    );
                }
                self.sync_shown();
            }
            WindowEvent::KeyboardInput {
                event: key,
                is_synthetic,
                ..
            } if !is_synthetic => {
                let since = Instant::now();
                if let Screen::Session(session) = &mut self.screen {
                    if session.takes_keys(viewport) {
                        session.key(key, since);
                        window.request_redraw();
                        return;
                    }
                    // egui's, but the session still hears of it: a key
                    // going up here is let go of at the far end too.
                    session.key_elsewhere(key);
                }
                // A paste into a text field is of the clipboard as it is now,
                // which egui-winit would read on this thread and wait for;
                // here it is read on the clipboard's, and pasted when it is in
                // (see `draw`), or not at all, from an owner that does not
                // answer within the clipboard's deadline.
                if key.state == ElementState::Pressed
                    && let Some(clipboard) = &self.clipboard
                    && clipboard::is_paste(self.modifiers.control_key(), &key.logical_key, key.physical_key)
                {
                    clipboard.read(Why::PASTE);
                    return;
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                // X11's capture: the pointer is hidden and held at the
                // centre, and every move away from it is undone. The far
                // end gets raw motion, which a warp does not make.
                if let Screen::Session(s) = &self.screen
                    && s.centred(viewport)
                {
                    let c = display_server::centre(&window);
                    if position.x.round() as i32 != c.x || position.y.round() as i32 != c.y {
                        let _ = window.set_cursor_position(c);
                    }
                    return;
                }
                if let Some(surface) = self.windows.get_mut(&id) {
                    surface.pointer = Some(*position);
                }
                if let Screen::Session(session) = &mut self.screen {
                    session.pointer_event(viewport);
                }
            }
            WindowEvent::CursorLeft { .. } | WindowEvent::CursorEntered { .. }
                if matches!(&self.screen, Screen::Session(s) if s.centred(viewport)) =>
            {
                let _ = window.set_cursor_position(display_server::centre(&window));
                return;
            }
            WindowEvent::MouseInput { state: pressed, button, .. } => {
                if let Screen::Session(session) = &mut self.screen
                    && session.takes_raw_mouse()
                {
                    session.mouse_button(*button, *pressed);
                    return;
                }
                if let Screen::Session(session) = &mut self.screen {
                    session.pointer_event(viewport);
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if let Screen::Session(session) = &mut self.screen
                    && session.takes_raw_mouse()
                {
                    session.mouse_wheel(*delta);
                    return;
                }
                if let Screen::Session(session) = &mut self.screen {
                    session.pointer_event(viewport);
                }
            }
            _ => {}
        }
        if let Some(surface) = self.windows.get_mut(&id)
            && surface.give_egui(event)
        {
            window.request_redraw();
        }
    }

    /// The clipboard goes before the event loop does: on Wayland its
    /// thread is on winit's connection, which ends with the loop, and on
    /// X11 it may have a clipboard manager to hand the text to.
    fn exiting(&mut self, _: &ActiveEventLoop) {
        if let Some(clipboard) = self.clipboard.take() {
            self.clipboard_running = !clipboard.stop();
        }
    }

    fn device_event(&mut self, _: &ActiveEventLoop, _: DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta } = event
            && let Some(session) = self.session()
        {
            session.mouse_motion(delta.0, delta.1);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.settle_focus(event_loop);
        match self.redraw_due() {
            Some(at) => event_loop.set_control_flow(ControlFlow::WaitUntil(at)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }
}

impl Gpu {
    /// The device, chosen to suit `window`'s surface, which comes back with
    /// it unconfigured.
    async fn new(window: &Arc<Window>) -> Result<(Gpu, wgpu::Surface<'static>), String> {
        // With the display handle, so the Vulkan instance enables the
        // surface extension this windowing system needs.
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle_from_env(
            Box::new(Arc::clone(window)),
        ));
        let surface = instance
            .create_surface(Arc::clone(window))
            .map_err(|e| format!("surface: {e}"))?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await
            .map_err(|e| format!("adapter: {e}"))?;
        // Everything the adapter can do rather than WebGPU's defaults, whose
        // 8192-pixel textures are half of what most hardware has; a desktop
        // past even the adapter's limit is tiled (see `desktop.rs`). And
        // where it has them, the DMA-BUF import and the 16-bit textures a
        // hardware picture is drawn from without reading it back
        // (`zerocopy.rs`): asked for whether or not hardware decoding is on,
        // since it can be turned on mid-session, and costing nothing until
        // a picture is imported (Vulkan's device has one extension more).
        let features = adapter.features()
            & (wgpu::Features::VULKAN_EXTERNAL_MEMORY_DMA_BUF | wgpu::Features::TEXTURE_FORMAT_16BIT_NORM);
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("vento"),
                required_features: features,
                required_limits: adapter.limits(),
                ..Default::default()
            })
            .await
            .map_err(|e| format!("device: {e}"))?;
        let caps = surface.get_capabilities(&adapter);
        // A non-sRGB format, since egui does its own gamma; the first one
        // the surface offers otherwise.
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| matches!(f, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm))
            .or_else(|| caps.formats.first().copied())
            .ok_or("the surface does not offer any format")?;
        let renderer = egui_wgpu::Renderer::new(
            &device,
            format,
            egui_wgpu::RendererOptions {
                msaa_samples: 1,
                depth_stencil_format: None,
                dithering: false,
                predictable_texture_filtering: false,
            },
        );
        log::info!("wgpu: {} ({:?})", adapter.get_info().name, adapter.get_info().backend);
        let zerocopy = zerocopy::Zerocopy::new(&adapter, &device, &queue);
        match &zerocopy {
            Ok(_) => log::info!("hardware pictures can be drawn from their DMA-BUFs on this device"),
            Err(why) => log::info!("hardware pictures will be read back: {why}"),
        }
        let gpu = Gpu {
            instance,
            adapter,
            device,
            queue,
            format,
            renderer,
            zerocopy,
        };
        Ok((gpu, surface))
    }

    /// `surface` configured for `window` in the renderer's format, with
    /// FIFO presentation; a surface without that format cannot be drawn by
    /// the one renderer.
    fn configure(&self, window: &Window, surface: &wgpu::Surface) -> Result<wgpu::SurfaceConfiguration, String> {
        let size = window.inner_size();
        let caps = surface.get_capabilities(&self.adapter);
        if !caps.formats.contains(&self.format) {
            return Err(format!("the window's surface does not offer {:?}", self.format));
        }
        let mut config = surface
            .get_default_config(&self.adapter, size.width.max(1), size.height.max(1))
            .ok_or("the surface is not supported by the adapter")?;
        config.format = self.format;
        config.present_mode = wgpu::PresentMode::Fifo;
        surface.configure(&self.device, &config);
        Ok(config)
    }

    /// Renders `paint_jobs` into `surface` and presents it; when, or `None`
    /// where nothing was presented.
    ///
    /// A minimised window shows nothing, so nothing is rendered into it:
    /// the frame has still run (the events taken, the poll made) and the
    /// textures still get their updates, but there is no present until the
    /// window is back, which its first expose asks a redraw for. Only X11
    /// says; elsewhere every frame is presented. Each window's own: under
    /// FIFO a present into a window that cannot be seen may wait for a
    /// vertical blank its compositor never gives it, and that wait would
    /// hold up every other window's frames behind it.
    fn present(&mut self, surface: &mut Surface, paint_jobs: &[egui::ClippedPrimitive], pixels_per_point: f32) -> Option<Instant> {
        if surface.minimized == Some(true) {
            return None;
        }
        let target = surface.surface.as_ref()?;
        let frame = match target.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                // A surface is lost too when another client has destroyed
                // the window, and configuring it again is then an error
                // wgpu would panic on; the `Destroyed` that follows closes
                // the window (see `window_event`).
                let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
                target.configure(&self.device, &surface.config);
                match pollster::block_on(scope.pop()) {
                    Some(error) => log::warn!("the window's surface could not be configured again: {error}"),
                    None => surface.window.request_redraw(),
                }
                return None;
            }
            _ => return None,
        };
        let descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [surface.config.width, surface.config.height],
            pixels_per_point,
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("egui") });
        let commands = self
            .renderer
            .update_buffers(&self.device, &self.queue, &mut encoder, paint_jobs, &descriptor);
        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("egui"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                })
                .forget_lifetime();
            self.renderer.render(&mut pass, paint_jobs, &descriptor);
        }
        self.queue.submit(commands.into_iter().chain(std::iter::once(encoder.finish())));
        surface.window.pre_present_notify();
        self.queue.present(frame);
        Some(Instant::now())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_icon_is_128_square() {
        assert_eq!(ICON.len(), 128 * 128 * 4);
        window_icon();
    }

    /// Which of `events` (`Some(true)` a press, `Some(false)` a release,
    /// `None` a move) each frame reads, given all at once.
    fn frames(events: &[Option<bool>]) -> Vec<Vec<Option<bool>>> {
        let mut split = PointerSplit::default();
        let mut queue: Vec<Option<bool>> = events.to_vec();
        let mut frames = Vec::new();
        while !queue.is_empty() {
            let mut frame = Vec::new();
            let mut held = Vec::new();
            for e in queue {
                if split.admit(e, !held.is_empty()) {
                    frame.push(e);
                } else {
                    held.push(e);
                }
            }
            frames.push(frame);
            split = PointerSplit::default();
            queue = held;
        }
        frames
    }

    #[test]
    fn a_press_ends_its_frame_and_a_release_starts_one() {
        const P: Option<bool> = Some(true);
        const R: Option<bool> = Some(false);
        const M: Option<bool> = None;
        // A press and a quick drag: the press is read where it was made,
        // the moves before the release.
        assert_eq!(frames(&[M, P, M, M, R, M]), vec![vec![M, P], vec![M, M], vec![R, M]]);
        // A click.
        assert_eq!(frames(&[P, R]), vec![vec![P], vec![R]]);
        // Moves alone, and a release with nothing before it, are one frame.
        assert_eq!(frames(&[M, M, M]), vec![vec![M, M, M]]);
        assert_eq!(frames(&[R, M, P]), vec![vec![R, M, P]]);
    }

    #[test]
    fn only_a_disconnected_command_line_session_quits() {
        assert_eq!(after_session(true, true), After::Quit);
        // The far end ended it: the reason is shown, even for the session
        // `--connect` opened.
        assert_eq!(after_session(true, false), After::Dialog);
        // Started from the dialog: back to it either way.
        assert_eq!(after_session(false, true), After::Dialog);
        assert_eq!(after_session(false, false), After::Dialog);
    }

    #[test]
    fn the_focus_coming_and_going_is_gained_and_lost() {
        assert_eq!(resolve_focus(None, &[(1, true)]), (Some(1), FocusChange::Gained(1)));
        assert_eq!(resolve_focus(Some(1), &[(1, false)]), (None, FocusChange::Lost(1)));
        // Nothing happened, or winit said it twice, as it does around a
        // keyboard grab.
        assert_eq!(resolve_focus(Some(1), &[]), (Some(1), FocusChange::Unchanged));
        assert_eq!(resolve_focus(Some(1), &[(1, true)]), (Some(1), FocusChange::Unchanged));
        assert_eq!(resolve_focus(None, &[(1, false)]), (None, FocusChange::Unchanged));
    }

    #[test]
    fn a_move_between_our_windows_in_one_batch_is_a_handoff() {
        // Out of one and into the other, in either order: a move, not the
        // focus leaving and coming back.
        let moved = (Some(2), FocusChange::Moved { from: 1, to: 2 });
        assert_eq!(resolve_focus(Some(1), &[(1, false), (2, true)]), moved);
        assert_eq!(resolve_focus(Some(1), &[(2, true), (1, false)]), moved);
        // A second grab from the same client moves it and adds a
        // duplicate focus-in, which changes nothing more.
        assert_eq!(resolve_focus(Some(1), &[(1, false), (2, true), (2, true)]), moved);
    }

    #[test]
    fn events_that_undo_each_other_are_no_change() {
        // Out and back in, as an ungrab and a grab make: unchanged, so
        // nothing is let go of and the clipboard is not read again.
        assert_eq!(resolve_focus(Some(1), &[(1, false), (1, true)]), (Some(1), FocusChange::Unchanged));
        // Over to the other and back within the batch.
        assert_eq!(
            resolve_focus(Some(1), &[(1, false), (2, true), (2, false), (1, true)]),
            (Some(1), FocusChange::Unchanged)
        );
        // In and out again from nowhere: still nowhere.
        assert_eq!(resolve_focus(None, &[(2, true), (2, false)]), (None, FocusChange::Unchanged));
    }

    #[test]
    fn a_move_and_then_leaving_is_lost_from_the_first() {
        // The process had it in 1 and ends up without it: lost, from where
        // it was, which is the window the session last heard of.
        assert_eq!(resolve_focus(Some(1), &[(1, false), (2, true), (2, false)]), (None, FocusChange::Lost(1)));
        // A stale focus-out from a window that no longer has it is nothing.
        assert_eq!(resolve_focus(Some(2), &[(1, false)]), (Some(2), FocusChange::Unchanged));
    }
}
