// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! A live session on screen: the desktop drawn, the mouse and keyboard
//! forwarded, the toolbar, the info panel, the prompts, and the cursor.
//!
//! Everything the window does with a backend goes through here, and all of
//! it is done from the main thread without waiting on the backend: events
//! come off the [`Mailbox`] when the window is repainted (the mailbox's wake
//! hook asks for the repaint), pixels come out through `read_region`, which
//! never queues behind the protocol, and every call into the backend is one
//! that returns at once. The facts and the connection info are asked for
//! once a second, which is what names the window and fills the panel; the
//! same tick is what the follower (`follow.rs`) is asked on. What is changed
//! on the panel comes back as actions and is applied here, so the session
//! is the one owner of what an option is set to.
//!
//! Input goes to the far end only while there is somewhere for it to go:
//! connected, not view-only, no prompt on screen, and no egui widget asking
//! for the keyboard. The mouse is forwarded from the desktop's own widget
//! response, so a click on the toolbar is a click on the toolbar; the
//! keyboard comes straight from winit (see `keys.rs`) because egui's key
//! events do not carry the layout's idea of a key.
//!
//! The cursor is the one place the two pointer modes differ on screen. In
//! absolute mode the far end sends its cursor's shape and this makes it the
//! window's own pointer (`cursor.rs`), or, with the software cursor
//! setting, draws it under the local pointer, hiding the OS cursor over the
//! desktop, as the Android app does. In relative mode the far end draws
//! its own cursor into the picture and owns the motion, so the local
//! pointer is captured and hidden while the window has focus and its raw
//! motion is sent as deltas; Ctrl+Alt held together lets it go, and a click
//! on the desktop takes it back. On Wayland the capture is a pointer lock;
//! on X11, which has none, the pointer is hidden, warped back to the
//! window's centre whenever it moves, and fenced in by pointer barriers on
//! the window's edges meanwhile (see `sync_capture`).
//!
//! The clipboard crosses in both directions under the two settings the
//! phone has (`settings.rs`), with its rule: the last text seen crossing
//! either way is remembered, and is never sent back where it came from.
//!
//! The debug overlay (`hud.rs`) is the phone's `SessionHud`: the state of
//! each part of the session, in a corner, from the connection panel.
//!
//! What is here is the same in every window the session is shown in: the
//! backend and its mailbox, the desktop, the follower, the prompts and the
//! trust stores, the far end's cursor, the keys held, the capture, the
//! clipboard. What one window has of its own (its view, its monitor, its
//! toolbar, the pointer over it) is its [`Pane`] (`pane.rs`).
//!
//! A session with a far end of several monitors can be shown in several
//! windows, one monitor in each: the main window's toolbar pops one out
//! (`pane.rs`), and the session asks the window for a window to put it in
//! ([`WindowOp`]), which it opens and gives back to the session as a pane.
//! The mailbox is drained in whichever window draws first, and the others
//! are asked to draw after it, so that no window draws a picture another
//! has already moved on from; the prompts, the panel and the notices stay
//! in the main window, whose closing is the end of the session.

// A child of this module, though a file beside it, for the state it shares.
#[path = "pane.rs"]
mod pane;

use super::cursor::{self as native};
use super::desktop::Desktop;
use super::follow::{Ask, Follower, Shown, Takes};
use super::hud::Hud;

use super::keys::{Keyboard, Stroke};
use super::panel::{Action, Controls, Panel, Snapshot, printable};
use super::display_server::{self, DisplayServer};
use super::prompts::{self, Answer, Form, Prompt};
use super::settings::Settings;
use super::toolbar::Placement;
use super::x11::Fence;
use super::{Host, Request};
use crate::backend::Backend;
use crate::backends::{self, Connect, Dirs, Hardware, Pinned};
use crate::events::{Cursor, Event, Mailbox, State};
use crate::known_hosts::{self, KnownHosts, Store};
use api::trust::{self, Decision};
use runtime::media::audio::{Outputs, Playback, Sound, Switch};
use crate::options::{self, Options};
use egui::{CursorIcon, TextureHandle, TextureOptions, Vec2, ViewportId, vec2};
use pane::{MonitorKey, Pane, PaneId, Rebound};
pub use pane::toolbar_width;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta};
use winit::window::{CursorGrabMode, Window};

/// RFB's button bits, which every backend takes.
const LEFT: i32 = 1;
const MIDDLE: i32 = 2;
const RIGHT: i32 = 4;
const WHEEL_UP: i32 = 8;
const WHEEL_DOWN: i32 = 16;
const WHEEL_LEFT: i32 = 32;
const WHEEL_RIGHT: i32 = 64;
/// Buttons 8 and 9, a mouse's side buttons, which browsers take as back and
/// forward: RFB's ExtendedMouseButtons, RDP's X1 and X2, SPICE's side and
/// extra, RustDesk's back and forward. A backend whose protocol has no
/// room for one drops it (claude-notes.md's list).
const BACK: i32 = 128;
const FORWARD: i32 = 256;

/// How many points of smooth scrolling make one notch of the wheel; what
/// egui-winit turns one line of a wheel into.
const NOTCH_POINTS: f32 = 50.0;

/// How long the bell rings in the title.
const BELL: Duration = Duration::from_millis(600);
/// How long a disconnect is given before the session is closed regardless.
const DISCONNECT_PATIENCE: Duration = Duration::from_secs(10);
/// How often the facts and the info are asked for.
const POLL: Duration = Duration::from_secs(1);
/// How early a frame may come and still count as the poll's. egui takes a
/// frame's predicted time (1/60 s by default) off every repaint asked for
/// later, so the wake for the poll lands that much before the second is up;
/// counted as not due, it asked again and a second frame followed straight
/// after, two draws a second where one will do.
const POLL_EARLY: Duration = Duration::from_millis(50);

/// How the local pointer is held in relative mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Capture {
    /// Wayland: the compositor's pointer lock.
    Locked,
    /// X11: hidden, no grab, fenced, and warped back to the window's centre.
    Centred,
}

/// How long a notice stays on the main window.
const NOTICE: Duration = Duration::from_secs(8);

/// What the session asks of the window between frames: a window opened for
/// a pane it has made, or a pane's window closed.
pub enum WindowOp {
    /// Open a window for `pane`, `size` in logical pixels, wherever the
    /// window manager puts it.
    Open { pane: PaneId, title: String, size: (u32, u32) },
    /// Close `pane`'s window, which is a pop-out's: `returned` when it was
    /// given back (its Return, or the window manager's close), which hands
    /// its monitor back to the main window, rather than closed because its
    /// monitor went away.
    Close { pane: PaneId, returned: bool },
}

/// The far end's cursor, uploaded once per shape, and kept as it arrived
/// for the native cursor, which resamples it to the view's scale.
struct CursorShape {
    texture: TextureHandle,
    hot: Vec2,
    size: Vec2,
    source: Cursor,
    hash: u64,
}

/// What a move of the focus out of window `from` does to the capture held
/// in `holder`, the capture being `wanted` still: nothing where `from` did
/// not hold it, and otherwise a move to the window the focus went to, or a
/// release where it is no longer wanted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Handover {
    Nothing,
    Move,
    Release,
}

fn handover(holder: Option<PaneId>, from: PaneId, wanted: bool) -> Handover {
    match holder {
        Some(holder) if holder == from && wanted => Handover::Move,
        Some(holder) if holder == from => Handover::Release,
        _ => Handover::Nothing,
    }
}

/// A pop-out's first size, in logical pixels: a far-end monitor `w`×`h`'s
/// shape, as large as the main window starts out at most.
fn popout_size(w: i32, h: i32) -> (u32, u32) {
    let (w, h) = (w.max(1) as f32, h.max(1) as f32);
    let fit = (1280.0 / w).min(800.0 / h).min(1.0);
    ((w * fit).round().max(200.0) as u32, (h * fit).round().max(150.0) as u32)
}

/// A window's title from the session's `name` (already [`printable`]): the
/// bell on it while it rings, the far end's `monitor` the window shows,
/// where it shows one of several, the app's name where `app` (not in the
/// window's own title bar, which is the app's already), and how to let go
/// of the pointer while the window has it `captured`.
fn window_title(name: &str, ringing: bool, monitor: Option<usize>, captured: bool, app: bool) -> String {
    let bell = if ringing { "🔔 " } else { "" };
    let app = if app { " - Vento" } else { "" };
    let hint = if captured { " (Press Ctrl+Alt to release pointer)" } else { "" };
    match monitor {
        Some(i) => format!("{bell}{name} - monitor {}{app}{hint}", i + 1),
        None => format!("{bell}{name}{app}{hint}"),
    }
}

/// The connecting screen, before there is a desktop: the address, what is
/// being waited for (`status`, and the bytes of a download in `progress`,
/// with the total where it is known), and a Cancel button, which is not
/// offered while `disconnecting`; whether Cancel was pressed.
pub(super) fn waiting(
    ui: &mut egui::Ui,
    address: &str,
    status: Option<&str>,
    progress: Option<(u64, Option<u64>)>,
    disconnecting: bool,
) -> bool {
    let mut cancel = false;
    ui.centered_and_justified(|ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() / 2.0 - 40.0);
            if disconnecting {
                ui.label("Disconnecting…");
                return;
            }
            ui.label(format!("Connecting to {address}…"));
            // Wrapped at the connection dialog's width, centred line by
            // line, with a margin in a narrower window.
            let width = (ui.available_width() - 32.0).min(super::dialog::WIDTH);
            if let Some(status) = status {
                ui.scope(|ui| {
                    ui.set_max_width(width);
                    ui.add(egui::Label::new(status).wrap().halign(egui::Align::Center));
                });
            }
            if let Some((done, total)) = progress {
                ui.add_space(4.0);
                ui.add(super::prompts::progress_bar(done, total).desired_width(width.min(super::prompts::WIDTH)));
            }
            ui.add_space(8.0);
            cancel = ui.button("Cancel").clicked();
        });
    });
    cancel
}

/// The title of a connecting session's window, before the server has
/// named its desktop.
pub(super) fn connecting_title(address: &str) -> String {
    window_title(&printable(address), false, None, false, true)
}

/// How long the hint over the desktop that says how to let go of the
/// pointer stays up after the session's first capture.
const RELEASE_HINT: Duration = Duration::from_secs(5);

pub struct Session {
    pub request: Request,
    /// The client's settings as this session runs with them: what it
    /// started with, then whatever the panel changed.
    pub settings: Settings,
    backend: Arc<dyn Backend>,
    /// What decodes the session's video, also handed to the backend: the
    /// hardware decoding setting is switched on it.
    decoders: Arc<backends::FFmpeg>,
    /// Where the session's sound plays, also handed to the backend: the
    /// sound switch is set on its controls (a mute, which every stream of
    /// the session plays under, at once and without the backend hearing of
    /// it, or an enable, which the backend asks the far end for).
    audio: Arc<backends::Audio>,
    /// The session's sound as it last said it (`Event::Sound`): whether
    /// there is a switch and what it does, and whether anything plays.
    pub sound: Sound,
    /// Whether a hardware picture the backend leaves on the GPU is drawn
    /// from there (`zerocopy.rs`): `Ok` while it can be, or why not (the
    /// device cannot, or an import failed, after which the session's
    /// pictures are read back to the end of it).
    zero_copy: Result<(), String>,
    mailbox: Arc<Mailbox>,
    desktop: Option<Desktop>,
    snapshot: Snapshot,
    last_poll: Option<Instant>,
    /// The windows the session is shown in, the main window's first.
    panes: Vec<Pane>,
    panel: Panel,
    /// Every option's value as the session has it: what it connected with,
    /// then whatever has been set since, which is what the panel's controls
    /// show.
    live: Options,
    follower: Follower,
    prompts: VecDeque<Prompt>,
    form: Form,
    /// The pins a certificate is checked against, and what this session's
    /// are pinned as.
    known_hosts: KnownHosts,
    pinned: Pinned,
    /// Where RealVNC's remembered identity is kept, by address.
    identities: Store,
    cursor: Option<CursorShape>,
    /// The far end's cursor made into window cursors, at each scale a
    /// window wanted it at; each pane says which its window shows.
    cursors: native::Cache,
    /// What the far end said about who owns the pointer.
    relative: bool,
    /// How the local pointer is held right now, if it is, and by which
    /// pane's window: only ever one, the focused one.
    capture: Option<(PaneId, Capture)>,
    /// X11: where the pointer was before it was captured, put back when it
    /// is let go.
    restore: Option<PhysicalPosition<f64>>,
    /// X11: the barriers that keep a captured pointer on the window, made
    /// at the first capture and up while it lasts.
    fence: Option<Fence>,
    /// Raw motion sent while captured, in all: the overlay's `Δ`, which is
    /// how doubled motion would show.
    motion_sent: (i64, i64),
    /// The escape chord was used; a click on the desktop ends it.
    escaped: bool,
    /// Until when the hint that says how to let go of the pointer is shown:
    /// set at the session's first capture, and cleared when that capture
    /// ends.
    pub release_hint: Option<Instant>,
    /// Whether the pointer has been captured in this session yet.
    captured_once: bool,
    /// The keys down at the far end, and here.
    keyboard: Keyboard,
    /// The last pointer event sent, so the same one is not sent twice.
    last_pointer: (i32, i32, i32),
    /// The pane a button was pressed in, while any is held: the far end's
    /// pointer is that window's until the buttons are up (see
    /// `pane::may_send`).
    pointer_owner: Option<PaneId>,
    /// Smooth scrolling not yet amounting to a notch.
    wheel: Vec2,
    /// Raw motion not yet amounting to a pixel.
    motion_rest: (f64, f64),
    /// The buttons held while captured, where egui's response is not asked.
    captured_mask: i32,
    view_only: bool,
    /// Whether the backend was last told the window is on screen; it starts
    /// out so.
    shown: bool,
    /// What of the clipboard has crossed, and what waits to.
    clipboard: Crossing,
    bell_until: Option<Instant>,
    title: String,
    disconnecting: Option<Instant>,
    hud: Hud,
    /// Damage rectangles received, in all.
    damage: u64,
    /// The session is over, with the detail the backend gave (empty for an
    /// ending that was asked for).
    pub closed: Option<String>,
    /// Windows to open and close, for the window to act on after the frame.
    ops: Vec<WindowOp>,
    /// How many pop-outs have been made, which names the next one.
    popped: u64,
    /// This frame took something off the mailbox, and the other windows
    /// are to draw after it.
    redraw_others: bool,
    /// A prompt is up; `attention` once when it came up, for the window to
    /// draw the main window's attention to it.
    prompting: bool,
    attention: bool,
    /// Something to tell in the main window, and until when.
    notice: Option<(String, Instant)>,
    /// What the connection is waiting for (`Event::Status`), shown under
    /// "Connecting to" and in a prompt; dropped once it connects or ends.
    status: Option<String>,
}

impl Session {
    /// Starts connecting. `screen` is the window's size in pixels, which
    /// is what the RDP backends' `DesktopSize=device` asks for.
    pub fn start(
        mut request: Request,
        dirs: &Dirs,
        settings: Settings,
        placement: Placement,
        screen: (u32, u32),
        zero_copy: Result<(), String>,
        wake: Box<dyn Fn() + Send + Sync>,
    ) -> Result<Session, String> {
        let mailbox = Mailbox::new(Some(wake));
        let client_name = backends::client_name();
        let decoders = backends::decoders(settings.hardware());
        // Told before the backend starts, so its first picture can be one:
        // only a hardware picture is ever left on the GPU, and only by a
        // backend that asks to, so with hardware decoding off this changes
        // nothing.
        decoders.set_gpu_pictures(zero_copy.is_ok());
        // Its streams are listed under the address until the far end says
        // its desktop's name (`poll`).
        let audio = backends::audio(&request.address, settings.audio_output);
        // `--view-only`: the backend starts view only, whatever the dialog
        // or `-o` said, and stays so (`set_option`).
        if settings.view_only && options::find(request.kind.options, "ViewOnly").is_ok() {
            request.options.set("ViewOnly", "true");
        }
        let identity = known_hosts::saved_identity(&dirs.config, request.kind, &request.address);
        let connect = Connect {
            address: &request.address,
            user: &request.user,
            domain: backends::domain(request.kind, &request.domain),
            password: request.password.as_deref(),
            options: &request.options,
            mailbox: Arc::clone(&mailbox),
            dirs,
            screen: Some(screen),
            last_size: None,
            identity: identity.as_deref(),
            client_name: &client_name,
            platform: backends::Platform::Linux,
            decoders: Some(Arc::clone(&decoders) as _),
            audio: audio.plays().then(|| Arc::clone(&audio) as _),
        };
        let backend = request.kind.start(&connect)?;
        // Typed or pasted, and a title before the desktop has a name.
        let title = printable(&request.address);
        let live = request.options.clone();
        // What the backend was created with, which it already acts on: the
        // toolbar and the window's own idea of where input goes start from
        // it rather than from off, which had `-o ViewOnly=true` showing
        // "View only: off" while the window forwarded keys the backend
        // dropped, and held the keyboard grab for them.
        let view_only = live.bool("ViewOnly") || settings.view_only;
        let pinned = backends::pinned_as(request.kind, &request.address, &request.options);
        let mut main = Pane::new(ViewportId::ROOT, placement, None);
        main.view.mode = settings.scale;
        main.fit_pending = settings.fit_window;
        Ok(Session {
            request,
            settings,
            backend,
            decoders,
            audio,
            sound: Sound::default(),
            zero_copy,
            mailbox,
            desktop: None,
            snapshot: Snapshot::default(),
            last_poll: None,
            panes: vec![main],
            panel: Panel::default(),
            live,
            follower: Follower::default(),
            prompts: VecDeque::new(),
            form: Form::default(),
            known_hosts: KnownHosts::in_dir(&dirs.config),
            pinned,
            identities: known_hosts::realvnc_identities(&dirs.config),
            cursor: None,
            cursors: native::Cache::default(),
            relative: false,
            capture: None,
            restore: None,
            fence: None,
            motion_sent: (0, 0),
            escaped: false,
            release_hint: None,
            captured_once: false,
            keyboard: Keyboard::default(),
            last_pointer: (-1, -1, 0),
            pointer_owner: None,
            wheel: Vec2::ZERO,
            motion_rest: (0.0, 0.0),
            captured_mask: 0,
            view_only,
            shown: true,
            clipboard: Crossing::default(),
            bell_until: None,
            title,
            disconnecting: None,
            hud: Hud::default(),
            damage: 0,
            closed: None,
            ops: Vec::new(),
            popped: 0,
            redraw_others: false,
            prompting: false,
            attention: false,
            notice: None,
            status: None,
        })
    }

    /// Turns the session's sound switch, which is the session's in every
    /// window it is shown in: an Enable switch asks the far end to send its
    /// sound (`on`) or to stop, and a Mute switch mutes it here (`on`) or
    /// lets it be heard again, at once: the device takes the samples through
    /// the controls, so what is queued keeps draining under the mute and
    /// unmuting is the sound as it is now. Not saved: the next session
    /// starts as its connection's option (Audio, Sound) says.
    pub fn set_sound(&mut self, on: bool) {
        let controls = self.audio.controls();
        match self.sound.switch {
            Some(Switch::Enable) => {
                if controls.set_enabled(on) {
                    log::info!("sound {}", if on { "enabled: the far end is asked for it" } else { "disabled: the far end is asked to stop it" });
                }
            }
            Some(Switch::Mute) => {
                controls.set_muted(on);
                log::info!("sound {}", if on { "muted" } else { "unmuted" });
            }
            None => return,
        }
        // Said back as an event too, but the button and the panel follow the
        // click in this frame rather than the next.
        self.sound = controls.sound();
        // The panel's row says it, asked again now rather than within the
        // second, and every window's button: the other windows draw again.
        self.last_poll = None;
        self.redraw_others = true;
    }

    /// What is playing, for the overlay.
    fn playback(&self) -> Option<Playback> {
        self.audio.controls().playback()
    }

    pub fn connected(&self) -> bool {
        self.mailbox.state() == State::Connected && self.closed.is_none()
    }

    /// Whether input has somewhere to go. Also what the window holds the
    /// compositor's or the window manager's shortcuts off for: while it is
    /// not, there is nowhere at the far end for a chord to go, and it is
    /// the window manager's.
    pub fn accepts_input(&self) -> bool {
        self.connected() && !self.view_only && self.prompts.is_empty() && self.disconnecting.is_none()
    }

    /// Whether the window should hold the compositor's or the window
    /// manager's shortcuts off: while input has somewhere to go, and not
    /// while the pointer is on the toolbar. The toolbar is this end's, so
    /// while it is being used the keyboard is too (Alt+Tab included, which
    /// is how a grabbed window is otherwise left only with the mouse).
    /// Never with the grab keyboard setting off. `pane` is the window that
    /// has the focus, whose toolbar it is.
    pub fn holds_shortcuts(&self, pane: PaneId) -> bool {
        self.settings.grab_keyboard && self.accepts_input() && !self.toolbar_hovered(pane)
    }

    /// Whether the window should hold the pointer as well as the keyboard
    /// (X11, see `x11.rs`, which keeps it until the last button is let go
    /// of): while it holds the shortcuts and the server has a modifier
    /// down, so that the window manager's mouse bindings on that modifier
    /// go to the server too, or one is about to be (`waiting`: a modifier
    /// press held behind pointer events). Not while the pointer is
    /// captured, where a pointer grab doubles the raw motion.
    pub fn holds_pointer(&self, pane: PaneId, waiting: bool) -> bool {
        self.holds_shortcuts(pane) && !self.captured() && (waiting || self.keyboard.modifier_held())
    }

    /// Whether a key event from winit, to `pane`'s window, is the far end's
    /// rather than egui's: not while the pointer is on that window's
    /// toolbar, where the shortcuts are this desktop's again and a key
    /// typed is not meant for the far end, nor while a widget of that
    /// window has egui's focus (a text field in the main window's panel
    /// does not take keys typed into a pop-out).
    pub fn takes_keys(&self, pane: PaneId) -> bool {
        self.accepts_input() && !self.toolbar_hovered(pane) && !self.pane(pane).is_some_and(|p| p.wants_keys)
    }

    /// Whether the pointer is on `pane`'s toolbar or its title bar, or on
    /// one of its resize edges, where an X11 window manager needs the
    /// keyboard to resize the window.
    fn toolbar_hovered(&self, pane: PaneId) -> bool {
        self.pane(pane).is_some_and(|p| p.toolbar_hovered || p.on_edge)
    }

    /// Whether mouse events from winit go to the far end as raw input,
    /// which is only while the pointer is captured.
    pub fn takes_raw_mouse(&self) -> bool {
        self.captured() && self.accepts_input()
    }

    fn captured(&self) -> bool {
        self.capture.is_some()
    }

    /// Whether the pointer is being held at the centre of `pane`'s window
    /// (X11's capture), so the window swallows its motion and warps it back.
    pub fn centred(&self, pane: PaneId) -> bool {
        self.capture == Some((pane, Capture::Centred)) && self.accepts_input()
    }

    /// What the window is called: the desktop's name once known, with the
    /// bell on it while it rings.
    pub fn title(&self) -> String {
        self.title_of(None, false)
    }

    /// A window's title: the session's, with the far end's `monitor` the
    /// window shows, where it shows one of several, and how to let go of
    /// the pointer where the window has it `captured`. The name in it is
    /// [`printable`], as the snapshot and the session keep it.
    pub fn title_of(&self, monitor: Option<usize>, captured: bool) -> String {
        self.title_with(monitor, captured, true)
    }

    /// [`title_of`](Self::title_of) without the app's name, for the
    /// window's own title bar.
    pub fn bar_title_of(&self, monitor: Option<usize>, captured: bool) -> String {
        self.title_with(monitor, captured, false)
    }

    fn title_with(&self, monitor: Option<usize>, captured: bool, app: bool) -> String {
        let name = self.snapshot.desktop_name().unwrap_or(&self.title);
        let ringing = self.bell_until.is_some_and(|t| Instant::now() < t);
        window_title(name, ringing, monitor, captured, app)
    }

    /// Whether the pointer is captured in `pane`'s window.
    pub fn captured_in(&self, pane: PaneId) -> bool {
        self.capture.is_some_and(|(holder, _)| holder == pane)
    }

    // ---- events from the backend -------------------------------------------

    /// The events waiting when the frame began. Those that arrive while it
    /// draws wait for the next frame, which the mailbox's wake asks for. A
    /// session on a busy screen can queue frames faster than `draw_frame`
    /// reads them, so taking events until the mailbox was empty did not end
    /// while they kept coming, and each frame taken added a staging image to
    /// the pass (gigabytes within a minute against a full-screen video).
    fn drain(&mut self, ctx: &egui::Context, host: &mut Host) {
        for event in self.mailbox.take_all() {
            self.redraw_others = true;
            match event {
                Event::Connected { width, height } | Event::DesktopSize { width, height } => {
                    if let Event::DesktopSize { .. } = event {
                        log::info!("desktop size now {width}×{height}");
                    }
                    self.status = None;
                    if let Some(old) = self.desktop.take() {
                        old.free(ctx);
                    }
                    if width > 0 && height > 0 {
                        self.desktop = Some(Desktop::new(ctx, width, height));
                        // What is there already, in case the frame that drew
                        // it came before this size did.
                        self.draw_frame(
                            ctx,
                            host,
                            &[crate::events::Rect {
                                x: 0,
                                y: 0,
                                width,
                                height,
                            }],
                        );
                    }
                    self.last_poll = None;
                    self.relative = self.mailbox.pointer_relative();
                }
                Event::Frame { rects, ended } => {
                    self.damage += rects.len() as u64;
                    for pane in &mut self.panes {
                        pane.unpresented = Some(pane.unpresented.map_or(ended, |t| t.min(ended)));
                    }
                    self.draw_frame(ctx, host, &rects);
                }
                Event::Cursor(shape) => self.set_cursor(ctx, shape),
                Event::PointerMode { relative } => {
                    self.relative = relative;
                    self.escaped = false;
                    log::info!("pointer mode: {}", if relative { "relative" } else { "absolute" });
                }
                Event::Bell => {
                    self.bell_until = Some(Instant::now() + BELL);
                    ctx.request_repaint_after(BELL);
                }
                // What the toolbars show their sound button by: from the
                // first sound until the last stream closes, which a backend
                // does a couple of seconds after the sound's end (see
                // `Pane::toolbar_items`).
                Event::Sound(sound) => {
                    if sound.playing != self.sound.playing {
                        log::info!("sound {}", if sound.playing { "playing" } else { "stopped" });
                    }
                    self.sound = sound;
                }
                Event::Clipboard(text) => {
                    // `SessionClipboard.fromRemote`: taken only under the
                    // setting, and what is taken is remembered so it is not
                    // offered straight back.
                    if !self.settings.clipboard_in {
                        log::debug!("clipboard from the far end: {} bytes, not taken", text.len());
                    } else if self.clipboard.far(&text) {
                        log::debug!("clipboard from the far end: {} bytes", text.len());
                        // egui's own as well, for a paste into a text field.
                        host.winit.set_clipboard_text(text.clone());
                        if let Some(clipboard) = host.clipboard {
                            clipboard.set(text);
                        }
                    }
                }
                // Only while connecting: the connecting screen and the prompts
                // over it are where it is shown.
                Event::Status(text) => {
                    if self.mailbox.state() == State::Connecting {
                        if let Some(text) = &text {
                            log::info!("status: {text}");
                        }
                        self.status = text;
                    }
                }
                Event::Closed { detail } => {
                    self.status = None;
                    // An ending that was asked for needs no telling, whatever
                    // words the library closed with.
                    self.closed = Some(if self.disconnecting.is_some() { String::new() } else { detail });
                }
                Event::TrustNeeded { fingerprint } => self.trust_needed(fingerprint),
                // Closed without an answer: the session no longer wants one.
                Event::PromptWithdrawn => {
                    if let Some(i) = self.prompts.iter().position(|p| matches!(p, Prompt::Credentials { .. })) {
                        log::info!("the credentials prompt is withdrawn");
                        self.prompts.remove(i);
                        if i == 0 {
                            self.form.reset();
                        }
                    }
                }
                other if self.settings.trust && self.trust_answered(&other) => {}
                other => {
                    if let Some(prompt) = Prompt::from_event(&other) {
                        self.prompts.push_back(prompt);
                    }
                }
            }
        }
    }

    /// A frame of the backend's onto the desktop: its picture drawn from
    /// the GPU, where it was left there and this window can, and otherwise
    /// `rects` read out of the backend. A picture that fails to draw from
    /// the GPU is read back, and so is every one after it: the decoders are
    /// told to leave none there again, and the overlay says why.
    fn draw_frame(&mut self, ctx: &egui::Context, host: &mut Host, rects: &[crate::events::Rect]) {
        let Some(desktop) = &mut self.desktop else {
            return;
        };
        if self.zero_copy.is_ok()
            && let Some(picture) = self.backend.gpu_picture()
            && let Some(video) = &mut host.video
        {
            match video.zerocopy.show(&picture, video.renderer) {
                Ok(id) => {
                    desktop.show_gpu(id);
                    return;
                }
                Err(why) => {
                    log::warn!("hardware pictures will be read back for the rest of the session: {why}");
                    self.decoders.set_gpu_pictures(false);
                    self.zero_copy = Err(why);
                }
            }
        }
        // Back from the GPU, the tiles are as old as the last picture read
        // into them: all of it is read again.
        if desktop.leave_gpu() {
            let (width, height) = desktop.size();
            desktop.update(ctx, &*self.backend, &[crate::events::Rect { x: 0, y: 0, width, height }]);
        } else {
            desktop.update(ctx, &*self.backend, rects);
        }
    }

    /// The overlay's word on how the picture gets to the screen, where
    /// hardware decoding is on: drawn from the GPU, and what that costs
    /// here, or why it is read back instead.
    fn zero_copy_summary(&self, host: &Host) -> Option<String> {
        // Only RustDesk's pictures are ever left on the GPU: whole frames of
        // video, where TigerVNC's and FreeRDP's H.264 is rectangles composed
        // with the rest of their picture in memory.
        if self.settings.hardware() == Hardware::Off
            || self.request.kind.id != api::catalog::RUSTDESK.id
        {
            return None;
        }
        let on_gpu = self.desktop.as_ref().is_some_and(Desktop::on_gpu);
        let cost = host.video.as_ref().and_then(|v| v.zerocopy.cost());
        Some(match (&self.zero_copy, on_gpu, cost) {
            (Err(why), _, _) => format!("zero-copy off ({why})"),
            (Ok(()), true, Some(ms)) => format!("zero-copy {ms:.2} ms"),
            (Ok(()), true, None) => "zero-copy".to_string(),
            (Ok(()), false, _) => "zero-copy idle".to_string(),
        })
    }

    fn set_cursor(&mut self, ctx: &egui::Context, shape: Option<Cursor>) {
        self.cursor = shape.map(|c| {
            // Straight alpha, as `events::Cursor` has it.
            let image = egui::ColorImage::from_rgba_unmultiplied([c.width, c.height], &c.rgba);
            CursorShape {
                texture: ctx.load_texture("cursor", image, TextureOptions::NEAREST),
                hot: vec2(c.hot_x as f32, c.hot_y as f32),
                size: vec2(c.width as f32, c.height as f32),
                hash: native::shape_hash(&c),
                source: c,
            }
        });
    }

    /// The facts and the info, once a second: the window's title and the
    /// panel come from here, and so does the monitor list.
    fn poll(&mut self, ctx: &egui::Context) {
        if !self.connected() {
            return;
        }
        let due = self.last_poll.is_none_or(|t| t.elapsed() + POLL_EARLY >= POLL);
        if !due {
            if let Some(last) = self.last_poll {
                ctx.request_repaint_after(POLL.saturating_sub(last.elapsed()));
            }
            return;
        }
        self.last_poll = Some(Instant::now());
        let mut info = self.backend.info();
        api::backend::with_sound(&mut info, self.audio.controls());
        self.snapshot = Snapshot::new(self.backend.facts(), info, self.backend.traffic());
        // The name a mixer lists the session's sound under is the one its
        // window's title starts with.
        self.audio.set_title(self.snapshot.desktop_name().unwrap_or(&self.title));
        ctx.request_repaint_after(POLL);
        // The other windows' titles and overlays come from the poll too.
        self.redraw_others = true;
        self.follow_tick();
    }

    // ---- the window's size, followed -----------------------------------------

    /// A window changed shape: the follower, which follows every window,
    /// is asked `DEBOUNCE` later, the one debounce started again whichever
    /// window it was.
    pub fn resized(&mut self) {
        self.follower.resized(Instant::now());
        self.refence();
    }

    /// X11: the window moved or changed size under a captured pointer, and
    /// the barriers go where it is now.
    pub fn refence(&mut self) {
        if matches!(self.capture, Some((_, Capture::Centred)))
            && let Some(fence) = &mut self.fence
        {
            fence.raise();
        }
    }

    /// The window's size in desktop pixels: its device pixels, which is
    /// what a desktop of that size is one to one with at 1:1. The toolbar
    /// floats over the desktop rather than taking a strip of it, so the
    /// whole window is the rectangle the desktop is drawn in.
    fn window_size(host: &Host) -> Option<(u32, u32)> {
        let size = host.window.inner_size();
        (size.width > 0 && size.height > 0).then_some((size.width, size.height))
    }

    /// One tick of the follower: on the poll, and after the debounce. Every
    /// window, the main one first, with the monitor it shows by the far
    /// end's id: where the far end takes a layout each window's monitor is
    /// asked to be the window's size, and otherwise the main window's size
    /// is asked for over a far end of one screen (see `follow::plan`).
    fn follow_tick(&mut self) {
        if !self.connected() {
            return;
        }
        let mut windows = Vec::with_capacity(self.panes.len());
        let main = self.panes.iter().filter(|p| p.id == ViewportId::ROOT);
        let popouts = self.panes.iter().filter(|p| p.id != ViewportId::ROOT);
        for pane in main.chain(popouts) {
            let Some(size) = pane.size else {
                // A window that has not drawn yet has no size to follow,
                // and the question waits for it.
                return;
            };
            windows.push(Shown { monitor: pane.monitor().map(|k| k.id), size });
        }
        let facts = &self.snapshot.facts;
        let takes = Takes { size: facts.can_resize, layout: facts.can_layout };
        match self.follower.tick(&windows, &facts.monitors, takes) {
            Some(Ask::Size(width, height)) => {
                log::info!("asking for a {width}×{height} desktop, this window's size");
                self.backend.request_desktop_size(width, height);
            }
            Some(Ask::Layout { width, height, screens }) => {
                log::info!(
                    "asking for a {width}×{height} desktop of {}, the windows' sizes",
                    screens
                        .iter()
                        .map(|m| format!("{} {}×{}", m.id, m.width, m.height))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                self.backend.request_layout(width, height, &screens);
            }
            None => {}
        }
    }

    fn set_follow(&mut self, follow: bool) {
        self.follower.set_enabled(follow);
        log::info!("follow window size: {}", if follow { "on" } else { "off" });
        if follow {
            // Asked at the next tick, which is brought forward: turning it
            // on takes effect where it was turned on.
            self.follower.resized(Instant::now());
            self.follow_tick();
        }
    }

    /// One option changed on the running session, from the panel or the
    /// toolbar: remembered as the value the controls show, and handed to
    /// the backend. View-only is the one the window has a say in too.
    fn set_option(&mut self, key: &str, value: &str) {
        if key == "ViewOnly" && self.settings.view_only && value != "true" {
            log::info!("view only is forced by --view-only");
            return;
        }
        self.live.set(key, value);
        log::info!("option {key} = {value:?}");
        if key == "ViewOnly" {
            let view_only = self.live.bool(key);
            self.view_only = view_only;
            self.backend.set_view_only(view_only);
            if view_only {
                self.release_keys();
            }
        } else {
            self.backend.set_option(key, value);
        }
        // A switch that went off takes the choices that need it with it,
        // here as in the dialog, so the panel never shows (and the backend
        // never holds) an encoding that cannot be negotiated.
        for key in self.live.settle(self.request.kind.options) {
            log::info!("option {key} = {:?}, its switch being off", self.live.get(key));
            self.backend.set_option(key, self.live.get(key));
        }
    }

    // ---- focus, capture and the clipboard ----------------------------------

    /// Whether the window can be seen: the backend's `focus`, which is what
    /// lets it pause. Not the keyboard focus: a window left visible beside
    /// the one being typed in would freeze, since the RFB backends stop
    /// reading, FreeRDP suppresses output and RustDesk drops to a frame a
    /// second, all for a picture still on screen. The window says when it
    /// is minimised or covered (see `App::sync_shown`).
    pub fn shown(&mut self, shown: bool) {
        if shown != self.shown {
            self.shown = shown;
            log::info!("{}", if shown { "window shown: updates resume" } else { "window hidden: updates may pause" });
            self.backend.focus(shown);
        }
    }

    /// The keyboard focus: the keys still down let go of when it leaves,
    /// and the capture and the shortcuts (the window's) following it. The
    /// clipboard is read when it comes back, by the window, and offered
    /// when the read is in (`local_clipboard`).
    pub fn focus(&mut self, focused: bool, host: &mut Host) {
        if !focused {
            self.focus_lost();
        }
        // The capture follows the focus on the next frame.
        host.window.request_redraw();
    }

    /// The focus left the process, or went with a window that closed.
    pub fn focus_lost(&mut self) {
        self.keyboard.focus_lost(&*self.backend);
    }

    /// What was copied here while the window was away, read when it got the
    /// focus back, offered once: `SessionClipboard.read`, which remembers
    /// what it saw whether or not the setting lets it be sent, so that
    /// turning sending on later offers the next copy and not an old one.
    /// A read that comes while input has nowhere to go (a prompt up, view
    /// only, not yet connected) is kept and offered when it has
    /// (`frame_shared`), so that a copy made before a trust prompt still
    /// reaches the far end. `None` is a clipboard with no text, or one that
    /// could not be read in time (see `clipboard/`).
    pub fn local_clipboard(&mut self, text: Option<String>) {
        if let Some(text) = text
            && let Some(text) = self.clipboard.local(text, self.accepts_input())
        {
            self.offer_clipboard(&text);
        }
    }

    /// The far end is given what was read here, under the setting.
    fn offer_clipboard(&mut self, text: &str) {
        if self.settings.clipboard_out {
            log::debug!("clipboard to the far end: {} bytes", text.len());
            self.backend.clipboard(text);
        } else {
            log::debug!("clipboard to the far end: {} bytes, not sent", text.len());
        }
    }

    fn release_keys(&mut self) {
        self.keyboard.release_all(&*self.backend);
    }

    /// Capture or free the pointer to match what the mode, the focus and
    /// the escape chord say, in `host`'s window: taken there when it has
    /// the focus and nothing holds it, let go there when it held it and
    /// should not. Only ever in the focused window, so one at a time; a
    /// move of the focus from one window to another hands it over at once
    /// (`hand_over_capture`) rather than here, a frame of each window apart.
    fn sync_capture(&mut self, host: &mut Host) {
        let want = self.wants_capture() && host.focused;
        let holder = self.capture.map(|(pane, _)| pane);
        if want {
            if holder.is_none() {
                self.take_capture(host.window, host.viewport, host.display_server, host.pointer);
            }
        } else if holder == Some(host.viewport) {
            self.let_go(host.window);
            self.release_captured_buttons();
            self.release_hint = None;
            log::info!("pointer released");
        }
    }

    /// Whether the pointer should be held, wherever the focus is.
    fn wants_capture(&self) -> bool {
        self.relative && !self.escaped && self.accepts_input()
    }

    /// The pointer captured in `window`, `pane`'s, where it is at `pointer`.
    /// The first capture of the session puts up the hint that says how to
    /// let go of it.
    fn take_capture(&mut self, window: &Window, pane: PaneId, display_server: DisplayServer, pointer: Option<PhysicalPosition<f64>>) {
        match display_server {
            DisplayServer::X11 => {
                // winit's X11 `Locked` is unsupported, and `Confined` (an
                // active pointer grab) makes the X server deliver every
                // raw motion twice, once for the grab and once for the
                // root window's selection. So: no grab, the cursor
                // hidden, raw XI2 deltas, and the pointer warped back to
                // the centre whenever it moves (the window swallows
                // those motions; a warp makes no raw motion), with
                // barriers on the window's edges so that it cannot be
                // anywhere else before the warp (see `x11::Fence`).
                self.restore = pointer;
                window.set_cursor_visible(false);
                let _ = window.set_cursor_position(display_server::centre(window));
                match &mut self.fence {
                    None => self.fence = Fence::new(window),
                    Some(fence) => fence.retarget(window),
                }
                if let Some(fence) = &mut self.fence {
                    fence.raise();
                }
                self.capture = Some((pane, Capture::Centred));
                self.motion_rest = (0.0, 0.0);
                log::info!("pointer captured: hidden and held at the centre (Ctrl+Alt releases it)");
            }
            _ => match window.set_cursor_grab(CursorGrabMode::Locked) {
                Ok(()) => {
                    window.set_cursor_visible(false);
                    self.capture = Some((pane, Capture::Locked));
                    self.motion_rest = (0.0, 0.0);
                    log::info!("pointer locked (Ctrl+Alt releases it)");
                }
                Err(e) => log::warn!("pointer lock refused: {e}"),
            },
        }
        if self.captured() && !self.captured_once {
            self.captured_once = true;
            self.release_hint = Some(Instant::now() + RELEASE_HINT);
        }
    }

    /// The buttons held at the far end through the capture let go of, now
    /// that nothing will send their releases.
    fn release_captured_buttons(&mut self) {
        if self.captured_mask != 0 {
            self.captured_mask = 0;
            self.backend.pointer_relative(0, 0, 0);
        }
    }

    /// The focus moved from one of our windows to another: the capture,
    /// held in `from`, let go of there and taken in `to` in the same step,
    /// with the buttons held through it still held: nothing left the
    /// process, so nothing at the far end is let go of. The pointer is not
    /// put back where it was before the capture, which was a place in the
    /// other window; letting go in `to` later puts it back where it was in
    /// `to`.
    pub fn hand_over_capture(
        &mut self,
        (from, from_pane): (&Window, PaneId),
        (to, to_pane): (&Window, PaneId),
        display_server: DisplayServer,
        pointer: Option<PhysicalPosition<f64>>,
    ) {
        let step = handover(self.capture.map(|(pane, _)| pane), from_pane, self.wants_capture());
        if step == Handover::Nothing {
            return;
        }
        self.restore = None;
        self.let_go(from);
        if step == Handover::Move {
            self.take_capture(to, to_pane, display_server, pointer);
        }
        if self.captured() {
            log::info!("pointer capture handed to another window");
        } else {
            self.release_captured_buttons();
            self.release_hint = None;
            log::info!("pointer released");
        }
    }

    /// The pointer as it was before the capture: unlocked, shown, and on
    /// X11 back where it was.
    fn let_go(&mut self, window: &Window) {
        match self.capture.take().map(|(_, how)| how) {
            Some(Capture::Locked) => {
                let _ = window.set_cursor_grab(CursorGrabMode::None);
            }
            Some(Capture::Centred) => {
                if let Some(fence) = &mut self.fence {
                    fence.lower();
                }
            }
            None => {}
        }
        window.set_cursor_visible(true);
        if let Some(at) = self.restore.take() {
            let _ = window.set_cursor_position(at);
        }
    }

    // ---- input from winit --------------------------------------------------

    /// A pointer event from winit, now, that egui will read in the next
    /// frame: the time of the first since the last frame is kept, for the
    /// overlay. One clock read a frame, whether the overlay is on or not.
    /// `pane` is the window it came to.
    pub fn pointer_event(&mut self, pane: PaneId) {
        if let Some(pane) = self.pane_mut(pane) {
            pane.pointer_since.get_or_insert_with(Instant::now);
        }
    }

    /// The frame just drawn went to the screen at `at`, or was not drawn
    /// (`None`: the window is minimised, or its surface had to be made
    /// again), in which case nothing is measured: a picture not shown has
    /// no latency worth the name, and the next one will be timed from its
    /// own frame.
    pub fn presented(&mut self, pane: PaneId, at: Option<Instant>) {
        let ended = self.pane_mut(pane).and_then(|p| p.unpresented.take());
        if let (Some(ended), Some(at)) = (ended, at) {
            self.hud.render.record(at.saturating_duration_since(ended));
        }
    }

    /// A key from winit, already known to be ours (see `takes_keys`),
    /// handed over at `since`.
    pub fn key(&mut self, event: &KeyEvent, since: Instant) {
        let taken = self.keyboard.key(Stroke::of(event), self.captured(), &*self.backend);
        if taken.sent {
            self.hud.keys.record(since.elapsed());
        }
        if taken.escape {
            self.escaped = true;
        }
    }

    /// A key from winit that is egui's rather than the far end's: view only,
    /// a prompt, a text field (see `Keyboard::elsewhere`).
    pub fn key_elsewhere(&mut self, event: &KeyEvent) {
        self.keyboard.elsewhere(Stroke::of(event), &*self.backend);
    }

    /// Raw motion while the pointer is locked.
    pub fn mouse_motion(&mut self, dx: f64, dy: f64) {
        if !self.takes_raw_mouse() {
            return;
        }
        let x = self.motion_rest.0 + dx;
        let y = self.motion_rest.1 + dy;
        let (ix, iy) = (x.trunc(), y.trunc());
        self.motion_rest = (x - ix, y - iy);
        if ix != 0.0 || iy != 0.0 {
            self.motion_sent.0 += ix as i64;
            self.motion_sent.1 += iy as i64;
            self.backend.pointer_relative(ix as i32, iy as i32, self.captured_mask);
        }
    }

    /// A button while the pointer is locked.
    pub fn mouse_button(&mut self, button: MouseButton, state: ElementState) {
        if !self.takes_raw_mouse() {
            return;
        }
        let bit = match button {
            MouseButton::Left => LEFT,
            MouseButton::Middle => MIDDLE,
            MouseButton::Right => RIGHT,
            MouseButton::Back => BACK,
            MouseButton::Forward => FORWARD,
            MouseButton::Other(_) => return,
        };
        self.captured_mask = match state {
            ElementState::Pressed => self.captured_mask | bit,
            ElementState::Released => self.captured_mask & !bit,
        };
        self.backend.pointer_relative(0, 0, self.captured_mask);
    }

    /// The wheel while the pointer is locked.
    pub fn mouse_wheel(&mut self, delta: MouseScrollDelta) {
        if !self.takes_raw_mouse() {
            return;
        }
        let notches = match delta {
            MouseScrollDelta::LineDelta(x, y) => vec2(x, y),
            MouseScrollDelta::PixelDelta(p) => vec2(p.x as f32, p.y as f32) / NOTCH_POINTS,
        };
        for bit in self.notches(notches) {
            self.backend.pointer_relative(0, 0, self.captured_mask | bit);
            self.backend.pointer_relative(0, 0, self.captured_mask);
        }
    }

    /// Whole notches out of `delta`, keeping the rest for next time: winit's
    /// and egui's sign, positive meaning the content moves down or right,
    /// which is the wheel rolling up or left.
    fn notches(&mut self, delta: Vec2) -> Vec<i32> {
        self.wheel += delta;
        let mut out = Vec::new();
        while self.wheel.y >= 1.0 {
            self.wheel.y -= 1.0;
            out.push(WHEEL_UP);
        }
        while self.wheel.y <= -1.0 {
            self.wheel.y += 1.0;
            out.push(WHEEL_DOWN);
        }
        while self.wheel.x >= 1.0 {
            self.wheel.x -= 1.0;
            out.push(WHEEL_LEFT);
        }
        while self.wheel.x <= -1.0 {
            self.wheel.x += 1.0;
            out.push(WHEEL_RIGHT);
        }
        out
    }

    // ---- the client's settings and the overlay -----------------------------

    /// One option of the client's own changed on the panel.
    fn set_settings(&mut self, settings: Settings) {
        if settings == self.settings {
            return;
        }
        log::info!(
            "settings: send clipboard {}, receive clipboard {}, grab keyboard {}, software cursor {}, hardware \
             decoding {}",
            on_off(settings.clipboard_out),
            on_off(settings.clipboard_in),
            on_off(settings.grab_keyboard),
            on_off(settings.software_cursor),
            backends::hardware_name(settings.hardware())
        );
        // Taken by every backend that decodes through these at its next
        // picture (RustDesk, which asks for a key frame to start the new
        // decoder on, and TigerVNC's and FreeRDP's H.264, which ask
        // `Decoders::hardware`).
        if settings.hardware() != self.settings.hardware() {
            self.decoders.set_hardware(settings.hardware());
        }
        self.settings = settings;
    }

    fn toggle_hud(&mut self) {
        self.hud.on = !self.hud.on;
        log::info!("debug overlay: {}", on_off(self.hud.on));
    }

    // ---- the frame ---------------------------------------------------------

    /// One frame of the window `host` is: first what is the session's in
    /// every window (the events, the poll, the follower, the disconnect's
    /// deadline), in whichever window draws first, then the pane's own (see
    /// `pane.rs`).
    pub fn frame(&mut self, root: &mut egui::Ui, host: &mut Host) {
        let ctx = root.ctx().clone();
        let size = Self::window_size(host);
        if let Some(pane) = self.pane_mut(host.viewport) {
            pane.size = size;
        }
        self.frame_shared(&ctx, host);
        // Out of the list while it draws, so that it can drive the session.
        let Some(i) = self.panes.iter().position(|p| p.id == host.viewport) else {
            return;
        };
        let mut pane = self.panes.remove(i);
        pane.frame(self, root, host);
        // Read inside this window's pass, which is the only time egui's
        // answer is this window's.
        pane.wants_keys = ctx.egui_wants_keyboard_input();
        self.panes.insert(i, pane);
    }

    /// The session's part of a frame. In every window's frame rather than
    /// the main window's alone: a window that cannot be seen may get no
    /// frames at all (a Wayland compositor stops a hidden window's frame
    /// callbacks), and a pop-out on screen must not wait on a main window
    /// that is not. Draining an empty mailbox is nothing, and the poll and
    /// the follower keep their own time.
    fn frame_shared(&mut self, ctx: &egui::Context, host: &mut Host) {
        self.drain(ctx, host);
        self.poll(ctx);
        self.rebind();
        if let Some(due) = self.follower.due() {
            let now = Instant::now();
            if due <= now {
                self.follow_tick();
            } else {
                ctx.request_repaint_after(due - now);
            }
        }
        if let Some(since) = self.disconnecting
            && self.closed.is_none()
        {
            match DISCONNECT_PATIENCE.checked_sub(since.elapsed()) {
                // A frame at the deadline, asked for here because nothing
                // else may ask: a session that never connected has no poll
                // ticking, and a backend that will not close sends nothing.
                // Without it "Disconnecting…" stayed up until the pointer
                // moved.
                Some(left) if !left.is_zero() => ctx.request_repaint_after(left),
                _ => {
                    log::warn!("no close within {DISCONNECT_PATIENCE:?} of the disconnect; giving up on it");
                    self.closed = Some(String::new());
                }
            }
        }
        // Whatever stopped input going to the far end (a prompt arriving,
        // view only, a disconnect), the keys it has down are let go of
        // there, since their releases will now be egui's. View only and the
        // disconnect do this themselves; this is for a prompt that comes up
        // mid-press, which would leave the key down at the far end until
        // the focus moved.
        if !self.accepts_input() {
            self.release_keys();
        } else if let Some(text) = self.clipboard.allowed() {
            // And when input goes there again, the local copy read while
            // it could not.
            self.offer_clipboard(&text);
        }
        // A prompt is answered in the main window, which may not be the one
        // in front: it is asked for once when one comes up.
        let prompting = !self.prompts.is_empty();
        if prompting && !self.prompting {
            self.attention = true;
        }
        self.prompting = prompting;
    }

    // ---- the windows -------------------------------------------------------

    /// What the window is to open and close, taken.
    pub fn take_window_ops(&mut self) -> Vec<WindowOp> {
        std::mem::take(&mut self.ops)
    }

    /// Whether the frame just drawn took news the other windows have not
    /// drawn yet, taken.
    pub fn take_redraw_others(&mut self) -> bool {
        std::mem::take(&mut self.redraw_others)
    }

    /// Whether a prompt has just come up, which is answered in the main
    /// window, taken.
    pub fn take_attention(&mut self) -> bool {
        std::mem::take(&mut self.attention)
    }

    /// A pane for `monitor`, and a window asked for to show it in, the
    /// monitor's shape. The pane is the session's from now on; its window
    /// opens after the frame (or does not, and the pane is forgotten).
    fn pop_out(&mut self, monitor: MonitorKey) {
        self.popped += 1;
        let id = ViewportId::from_hash_of(("pop-out", self.popped));
        let (_, _, w, h) = monitor.rect;
        let size = popout_size(w, h);
        log::info!("popping out monitor {} ({w}×{h})", monitor.index + 1);
        self.panes.push(Pane::new(id, Placement::default(), Some(monitor)));
        self.ops.push(WindowOp::Open {
            pane: id,
            title: self.title(),
            size,
        });
    }

    /// The window for `pane` could not be opened: the pane goes, and the
    /// main window says why.
    pub fn forget_pane(&mut self, pane: PaneId, why: &str) {
        self.panes.retain(|p| p.id != pane);
        self.notify(format!("Could not open a window for the monitor: {why}"));
    }

    /// `pane`'s window, `host`, is closing: the capture let go of if it
    /// held it, the drag it held ended, its cursor put back, and the pane
    /// gone. A pop-out that was `returned` gives its monitor back to the
    /// main window, if that is showing a monitor of its own.
    pub fn close_pane(&mut self, ctx: &egui::Context, host: &mut Host, returned: bool) {
        let Some(i) = self.panes.iter().position(|p| p.id == host.viewport) else {
            return;
        };
        let mut pane = self.panes.remove(i);
        if self.capture.is_some_and(|(holder, _)| holder == pane.id) {
            self.let_go(host.window);
            self.release_captured_buttons();
        }
        if self.pointer_owner == Some(pane.id) {
            self.pointer_owner = None;
            let (x, y, mask) = self.last_pointer;
            if mask != 0 {
                self.send_pointer((x, y), 0);
            }
        }
        pane.close(ctx, host.window);
        if returned
            && let Some(monitor) = pane.monitor()
            && let Some(main) = self.pane_mut(ViewportId::ROOT)
        {
            main.give_back(monitor);
        }
        self.redraw_others = true;
    }

    /// Something for the main window to say for a while.
    fn notify(&mut self, text: String) {
        log::info!("{text}");
        self.notice = Some((text, Instant::now() + NOTICE));
        self.redraw_others = true;
    }

    /// Every pane's monitor found again in the far end's layout as it is
    /// now (see `MonitorKey::find`). A pop-out whose monitor is gone is
    /// closed, and the main window says so; the main window's goes back to
    /// the whole desktop. A layout of no monitors at all is a far end that
    /// has said nothing about its screens, and changes nothing.
    fn rebind(&mut self) {
        let monitors = &self.snapshot.facts.monitors;
        if monitors.is_empty() {
            return;
        }
        let mut gone = Vec::new();
        for pane in &mut self.panes {
            match pane.rebind(monitors) {
                Rebound::Kept => {}
                Rebound::Gone(index) if pane.id == ViewportId::ROOT => {
                    log::info!("monitor {} of the far end is gone; the main window shows the whole desktop", index + 1);
                }
                Rebound::Gone(index) => gone.push((pane.id, index)),
            }
        }
        for (pane, index) in gone {
            self.ops.push(WindowOp::Close { pane, returned: false });
            self.notify(format!("Monitor {} of the far end is gone, and so is its window.", index + 1));
        }
    }

    /// The connection panel, in the main window, and what was changed on it.
    fn panel(&mut self, ctx: &egui::Context, host: &mut Host) {
        if !self.panel.open {
            return;
        }
        let state = self.state_line();
        let title = self.title.clone();
        let controls = Controls {
            sound: self.sound.switch.map(|switch| (switch, self.sound.on())),
            specs: self.request.kind.options,
            values: &self.live,
            connected: self.connected(),
            window: Self::window_size(host),
            follow: self.follower.enabled(),
            settings: self.settings,
            debug: self.hud.on,
        };
        let actions = self.panel.show(ctx, &self.snapshot, &controls, &title, &state);
        for action in actions {
            match action {
                Action::SetOption(key, value) => self.set_option(key, &value),
                Action::DesktopSize(width, height) => {
                    log::info!("asking for a {width}×{height} desktop");
                    self.backend.request_desktop_size(width, height);
                }
                Action::Display(index) => self.backend.request_display(index),
                Action::Follow(follow) => self.set_follow(follow),
                Action::Settings(settings) => self.set_settings(settings),
                Action::Sound(on) => self.set_sound(on),
                Action::Debug(on) => {
                    if on != self.hud.on {
                        self.toggle_hud();
                    }
                }
            }
        }
    }

    fn pane_mut(&mut self, id: PaneId) -> Option<&mut Pane> {
        self.panes.iter_mut().find(|p| p.id == id)
    }

    fn pane(&self, id: PaneId) -> Option<&Pane> {
        self.panes.iter().find(|p| p.id == id)
    }

    /// Where the main window's toolbar was left, for the next session.
    pub fn placement(&self) -> Placement {
        self.pane(ViewportId::ROOT).map(|p| p.placement).unwrap_or_default()
    }

    /// Before egui-winit sets the window's cursor from egui's icon: the
    /// far end's shape taken off it where it is no longer wanted.
    pub fn cursor_before(&mut self, host: &mut Host) {
        if let Some(pane) = self.pane_mut(host.viewport) {
            pane.native.before(host.window, pane.native_key);
        }
    }

    /// After egui-winit's turn: the far end's shape put on, where wanted.
    pub fn cursor_after(&mut self, host: &mut Host, icon: CursorIcon) {
        let Some(pane) = self.panes.iter_mut().find(|p| p.id == host.viewport) else {
            return;
        };
        let wanted = match (pane.native_key, &self.cursor) {
            (Some(key), Some(shape)) => Some((key, &shape.source)),
            _ => None,
        };
        let alpha = native::Alpha::for_display_server(host.display_server);
        pane.native
            .after(&mut self.cursors, host.window, host.event_loop, wanted, icon, alpha);
    }
    /// What the panel says under the name: the state, which client, the
    /// desktop.
    fn state_line(&self) -> String {
        let backend = self.request.kind.name;
        if self.closed.is_some() {
            return "Disconnected".to_string();
        }
        if !self.connected() {
            return format!("Connecting · {backend}");
        }
        let facts = &self.snapshot.facts;
        if facts.width == 0 {
            return format!("Connected · {backend}");
        }
        let monitors = facts.monitors.len();
        if monitors > 1 {
            format!(
                "Connected · {backend} · {} × {} · {monitors} monitors",
                facts.width, facts.height
            )
        } else {
            format!("Connected · {backend} · {} × {}", facts.width, facts.height)
        }
    }

    /// Before there is a desktop: what is being waited for, and a way out.
    fn waiting(&mut self, ui: &mut egui::Ui) {
        let disconnecting = self.disconnecting.is_some();
        if waiting(ui, &self.request.address, self.status.as_deref(), None, disconnecting) {
            self.disconnect();
        }
    }

    /// One pointer event, unless it is the one already sent; whether it
    /// was sent.
    fn send_pointer(&mut self, at: (i32, i32), mask: i32) -> bool {
        let fresh = (at.0, at.1, mask) != self.last_pointer;
        if fresh {
            self.backend.pointer(at.0, at.1, mask);
            self.last_pointer = (at.0, at.1, mask);
        }
        fresh
    }


    /// Whether the session's ending was asked for from this side, rather
    /// than the far end's or the library's doing.
    pub fn disconnect_asked(&self) -> bool {
        self.disconnecting.is_some()
    }

    pub fn disconnect(&mut self) {
        if self.disconnecting.is_none() {
            self.disconnecting = Some(Instant::now());
            self.release_keys();
            self.backend.disconnect();
        }
    }

    // ---- the prompts -------------------------------------------------------

    /// A certificate, against what this address was accepted with before,
    /// as `api::trust` decides on the phone too: a match is silent, a
    /// RustDesk key never seen is pinned without asking, and anything else
    /// is put to the user with why.
    ///
    /// With `--trust` the known hosts are only read: what they do not match
    /// is accepted, and nothing is pinned.
    fn trust_needed(&mut self, fingerprint: String) {
        let key = &self.pinned.key;
        let decision = if self.settings.trust {
            trust::decide(&trust::ReadOnly(&self.known_hosts), &self.pinned, &fingerprint)
        } else {
            trust::decide(&self.known_hosts, &self.pinned, &fingerprint)
        };
        match decision {
            Decision::Matches => {
                log::info!("certificate matches the pin for {key}");
                self.backend.answer_trust(true);
            }
            Decision::FirstSight => self.backend.answer_trust(true),
            Decision::Ask { identity, .. } if self.settings.trust => {
                log::info!("accepting the certificate for {key} ({identity:?}): --trust, not pinned");
                self.backend.answer_trust(true);
            }
            Decision::Ask {
                identity,
                matching_address,
            } => {
                log::info!("asking about the certificate for {key}: {identity:?}");
                self.prompts.push_back(Prompt::Trust {
                    fingerprint,
                    identity,
                    matching_address,
                });
            }
        }
    }

    /// `--trust`: accepts a security, unverified, question or message
    /// prompt without showing it, and remembers nothing; whether `event`
    /// was one.
    fn trust_answered(&mut self, event: &Event) -> bool {
        match event {
            Event::Unverified { why } => {
                log::info!("accepting an unverified server ({why}): --trust");
                self.backend.answer_trust(true);
            }
            Event::Question { title, .. } => {
                log::info!("answering yes to {title:?}: --trust");
                self.backend.answer_trust(true);
            }
            Event::Message { text, .. } => {
                log::info!("accepting the message {text:?}: --trust");
                self.backend.answer_trust(true);
            }
            Event::SecurityNeeded { .. } => {
                log::info!("accepting the server's security: --trust, not remembered");
                self.backend.answer_security(true, false);
            }
            _ => return false,
        }
        true
    }

    fn prompt(&mut self, ctx: &egui::Context) {
        let Some(prompt) = self.prompts.front().cloned() else {
            return;
        };
        let answer = prompts::show(
            ctx,
            &prompt,
            &mut self.form,
            &self.request.address,
            self.status.as_deref(),
            &self.request.user,
        );
        if let Some(answer) = answer {
            self.prompts.pop_front();
            match answer {
                Answer::Credentials(credentials) => self.backend.answer_credentials(credentials),
                Answer::Trust(accept) => self.backend.answer_trust(accept),
                Answer::Pin {
                    accept,
                    remember,
                    fingerprint,
                } => {
                    let accept = trust::answered(&self.known_hosts, &self.pinned, &fingerprint, accept, remember);
                    self.backend.answer_trust(accept);
                }
                Answer::Security { accept, remember } => self.backend.answer_security(accept, remember),
            }
        }
    }

    // ---- the end -----------------------------------------------------------

    /// Ends the session and releases the texture, and starts releasing the
    /// backend: what comes back is its `destroy`, under way on a thread of
    /// its own.
    ///
    /// `host` is the main window; the pop-outs were closed before this
    /// (`close_pane`), each letting go of what it held.
    pub fn shutdown(mut self, ctx: &egui::Context, host: &mut Host) -> Teardown {
        self.release_keys();
        for pane in &mut self.panes {
            if pane.id == host.viewport {
                pane.close(ctx, host.window);
            }
        }
        if self.capture.is_some_and(|(holder, _)| holder == host.viewport) {
            self.let_go(host.window);
        }
        // The screen was the session's; the dialog gets a window.
        if host.window.fullscreen().is_some() {
            host.window.set_fullscreen(None);
        }
        if let Some(desktop) = self.desktop.take() {
            desktop.free(ctx);
        }
        if self.closed.is_none() {
            self.backend.disconnect();
        }
        // `RealVncBackend.saveIdentity`: kept by the address as written, for
        // the next session to hand back. Not under `--trust`, which
        // remembers nothing.
        if let Some(identity) = self.backend.saved_identity().filter(|_| !self.settings.trust) {
            let address = &self.request.address;
            match self.identities.set(address, &identity) {
                Ok(()) => log::info!("remembered RealVNC's identity for {address}"),
                Err(e) => log::warn!("could not keep the identity for {address} in {}: {e}", self.identities.path().display()),
            }
        }
        Teardown::start(self.backend)
    }
}

/// A backend's `destroy`, on a thread of its own.
///
/// `destroy` joins the protocol thread, so it is as quick as the backend is
/// to let go, and one that is not (still dialling, stuck in a handshake, or
/// past `DISCONNECT_PATIENCE` without a close) would hold the window's
/// thread for as long: the window would not close, nor the dialog come
/// back. Every backend ends its dial when asked, so this is a fallback; the
/// window keeps the handle so that it can give whatever is still going a
/// moment before the process exits.
pub struct Teardown(Option<std::thread::JoinHandle<()>>);

impl Teardown {
    fn start(backend: Arc<dyn Backend>) -> Teardown {
        let destroying = Arc::clone(&backend);
        match std::thread::Builder::new()
            .name("destroy".into())
            .spawn(move || destroying.destroy())
        {
            Ok(thread) => Teardown(Some(thread)),
            Err(e) => {
                log::warn!("could not start a thread to destroy the backend on ({e}); destroying it here");
                backend.destroy();
                Teardown(None)
            }
        }
    }

    pub fn is_finished(&self) -> bool {
        self.0.as_ref().is_none_or(|thread| thread.is_finished())
    }

    /// Waits for it until `deadline` at most; whether it finished.
    pub fn wait_until(self, deadline: Instant) -> bool {
        while !self.is_finished() {
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        if let Some(thread) = self.0 {
            let _ = thread.join();
        }
        true
    }
}

fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

/// The clipboard's memory, `SessionClipboard.lastSeen`'s rule: the last
/// text seen crossing either way is not sent back where it came from.
/// Beside it, the local read that came while input had nowhere to go,
/// for when it has.
#[derive(Default)]
struct Crossing {
    seen: Option<String>,
    held: Option<String>,
}

impl Crossing {
    /// A read of the local clipboard: what to offer the far end now.
    /// Remembered as seen when it may be offered, and held until it may
    /// otherwise; the latest read is what the clipboard holds, so it takes
    /// the place of anything held before it, even when it is nothing new.
    fn local(&mut self, text: String, may_offer: bool) -> Option<String> {
        self.held = None;
        if text.is_empty() || self.seen.as_deref() == Some(text.as_str()) {
            None
        } else if may_offer {
            self.seen = Some(text.clone());
            Some(text)
        } else {
            self.held = Some(text);
            None
        }
    }

    /// Input may go to the far end: the read held while it could not.
    fn allowed(&mut self) -> Option<String> {
        let text = self.held.take()?;
        self.local(text, true)
    }

    /// What the far end copied, under the setting that takes it: whether
    /// it is new, and so to be set here. Once set it is the local
    /// clipboard, so a read held from before it is old.
    fn far(&mut self, text: &str) -> bool {
        if text.is_empty() || self.seen.as_deref() == Some(text) {
            return false;
        }
        self.seen = Some(text.to_string());
        self.held = None;
        true
    }

    /// What the far end was last given or gave, for the overlay.
    fn seen(&self) -> Option<&str> {
        self.seen.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::super::panel::NAME_MAX;
    use super::*;
    use crate::backend::{ConnectionFact, Field};

    #[test]
    fn the_capture_moves_with_the_focus_only_from_the_window_holding_it() {
        let main = ViewportId::ROOT;
        let other = ViewportId::from_hash_of("pop-out");
        // Held in the window the focus left: taken in the one it went to.
        assert_eq!(handover(Some(main), main, true), Handover::Move);
        // Held there but no longer wanted (the escape chord, view only):
        // let go of, not moved.
        assert_eq!(handover(Some(main), main, false), Handover::Release);
        // Held elsewhere, or nowhere: the move leaves it alone.
        assert_eq!(handover(Some(other), main, true), Handover::Nothing);
        assert_eq!(handover(None, main, true), Handover::Nothing);
    }

    #[test]
    fn a_far_ends_name_is_titled_without_its_control_characters() {
        // The name a hostile or careless server gives its desktop goes into
        // every window's title, which winit hands X11 as a C string and
        // Wayland likewise: a NUL in it panicked the client. Cleaned once in
        // the snapshot, it is a title a C string can carry, for the main
        // window and a pop-out alike.
        let named = |name: &str| {
            let info = vec![ConnectionFact::of(Field::DesktopName, "Desktop", name)];
            Snapshot::new(Default::default(), info, None)
        };
        let snapshot = named("evil\0na\x1bme\u{7}\tdesk\r\n");
        assert_eq!(snapshot.desktop_name(), Some("evilname desk"));
        assert_eq!(snapshot.info[0].value, "evilname desk", "the panel's row too");
        for monitor in [None, Some(1)] {
            let title = window_title(snapshot.desktop_name().unwrap(), true, monitor, false, true);
            assert!(std::ffi::CString::new(title.clone()).is_ok(), "{title:?}");
        }
        assert_eq!(window_title("evilname desk", false, Some(1), false, true), "evilname desk - monitor 2 - Vento");
        assert_eq!(
            window_title("desk", false, None, true, true),
            "desk - Vento (Press Ctrl+Alt to release pointer)"
        );
        // The title bar's, without the app's name.
        assert_eq!(window_title("10.33.0.208", false, Some(1), false, false), "10.33.0.208 - monitor 2");
        assert_eq!(window_title("desk", false, None, false, false), "desk");
        // Nothing but control characters is no name: the address stands in.
        assert_eq!(named("\0\0\x01").desktop_name(), None);
        // A name of a megabyte is cut to NAME_MAX characters, the last an
        // ellipsis, wherever its characters' bytes fall.
        let long = named(&"é".repeat(1 << 20));
        let name = long.desktop_name().unwrap();
        assert_eq!(name.chars().count(), NAME_MAX);
        assert!(name.ends_with("é…"), "{name}");
        assert_eq!(printable(&"x".repeat(NAME_MAX)), "x".repeat(NAME_MAX), "up to the cap, kept whole");
    }

    #[test]
    fn a_copy_read_while_input_goes_nowhere_is_offered_when_it_does() {
        let mut clip = Crossing::default();
        // Read while a prompt is up (or view only, or connecting): kept,
        // not offered, and not yet seen.
        assert_eq!(clip.local("one".into(), false), None);
        assert_eq!(clip.seen(), None);
        // Offered once input goes to the far end, and only once.
        assert_eq!(clip.allowed().as_deref(), Some("one"));
        assert_eq!(clip.allowed(), None);
        assert_eq!(clip.seen(), Some("one"));
        // The same text again is not new; another is offered at once.
        assert_eq!(clip.local("one".into(), true), None);
        assert_eq!(clip.local("two".into(), true).as_deref(), Some("two"));
        // A later read takes the place of a held one, even one that is
        // what the far end already has.
        assert_eq!(clip.local("three".into(), false), None);
        assert_eq!(clip.local("two".into(), false), None);
        assert_eq!(clip.allowed(), None);
        // The far end's copy is set here, so a read held from before it
        // is old; and it is not offered back.
        assert_eq!(clip.local("four".into(), false), None);
        assert!(clip.far("theirs"));
        assert_eq!(clip.allowed(), None);
        assert_eq!(clip.local("theirs".into(), true), None);
        assert!(!clip.far("theirs"), "the same copy twice is set once");
        assert!(!clip.far(""), "nothing is not a copy");
        assert_eq!(clip.local(String::new(), true), None);
    }

    #[test]
    fn a_pop_out_starts_the_shape_of_its_monitor() {
        // As large as the main window starts at most, the monitor's shape.
        assert_eq!(popout_size(1280, 800), (1280, 800));
        assert_eq!(popout_size(1920, 1080), (1280, 720));
        assert_eq!(popout_size(1080, 1920), (450, 800));
        // A small monitor is not enlarged.
        assert_eq!(popout_size(640, 480), (640, 480));
    }
}
