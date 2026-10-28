// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! One window's part of a session: how the desktop is shown in it, its
//! toolbar, the pointer over it, the far end's cursor as its own, and its
//! share of the debug overlay.
//!
//! A session is one connection, one backend, one desktop texture, one set
//! of keys held at the far end, one capture and one set of prompts, and
//! every window it is shown in draws from those (`session.rs`). What each
//! window has of its own is here: a [`View`] (the scale mode and the pan),
//! which of the far end's monitors it shows, where its toolbar sits and
//! whether the pointer is on it, whether the pointer is over its desktop,
//! the cursor its window was last given, and its own frame counts and
//! latencies. A pane is keyed by its window's egui viewport.
//!
//! The main window's pane is there for the whole session. The others are
//! pop-outs: one of the far end's monitors in a window of its own, opened
//! from the main window's toolbar (Pop out, which pops out the monitor the
//! main window shows and moves it on to the next, or, while it shows the
//! whole desktop, gives every monitor a window) and closed by their own
//! (Return, or the window manager's close, either of which gives the
//! monitor back to the main window). A pop-out is an ordinary new window:
//! where it goes is the window manager's or the compositor's to say, and
//! the user's to change. A pop-out's toolbar has what is a window's own (the
//! scale, the monitor, fullscreen) and view only, which is the session's;
//! the panel, the prompts and the disconnect are the main window's alone.
//!
//! A child of the session's module, though a file beside it, so that a
//! pane's frame can read and drive the session's private state.

use super::super::csd;
use super::super::cursor::{Key, Native};
use super::super::desktop::Desktop;
use super::super::hud::{self, Rate};
use super::super::icons;
use super::super::toolbar::{self, Placement};
use super::super::view::{Layout, ScaleMode, View};
use super::super::{BACKDROP, Host};
use super::{BACK, Capture, FORWARD, LEFT, MIDDLE, NOTCH_POINTS, RIGHT, Session, WindowOp, on_off};
use crate::backend::Monitor;
use runtime::media::audio::Switch;
use egui::{
    Align2, Color32, CursorIcon, Frame, Id, LayerId, Margin, Order, PointerButton, Pos2, Rect, Response, RichText,
    Sense, Stroke, Vec2, ViewportId, pos2, vec2,
};
use std::time::{Duration, Instant};
use winit::dpi::PhysicalSize;
use winit::window::Fullscreen;

/// A pane is known by its window's egui viewport: the root one for the
/// main window.
pub type PaneId = ViewportId;

/// How long the toolbar stays out after the pane opens, so that it is seen
/// once.
const TOOLBAR_REVEAL: Duration = Duration::from_secs(3);
/// The toolbar's buttons' height, in points.
const TOOLBAR_BUTTON: f32 = 28.0;
/// The size of the toolbar's icons, in points: Material Symbols are drawn
/// for 24, and read well down to 18.
const TOOLBAR_ICON: f32 = 20.0;

/// Which of the far end's monitors a pane shows, kept as enough to find it
/// again when the far end's layout changes under it: the far end's own id
/// for it, its rectangle, and where it was in the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MonitorKey {
    pub id: u32,
    pub rect: (i32, i32, i32, i32),
    pub index: usize,
}

impl MonitorKey {
    pub fn of(index: usize, monitor: &Monitor) -> MonitorKey {
        MonitorKey {
            id: monitor.id,
            rect: (monitor.x, monitor.y, monitor.width, monitor.height),
            index,
        }
    }

    /// The same monitor in `monitors`, the layout as it is now: the one
    /// with the same id, which is the far end's own name for it and stays
    /// with it through a resize or a move (the follower resizing each
    /// monitor to its window is exactly that); failing that the one with the
    /// same rectangle wherever it is in the list, since a far end that
    /// renumbered its screens may still have this one where it was; failing
    /// that the one at the same place in the list; and failing all three,
    /// none, as it is gone. (Where a protocol has no ids the id is the
    /// index, and its order is the only identity it gives a screen.)
    pub fn find(self, monitors: &[Monitor]) -> Option<MonitorKey> {
        let index = monitors
            .iter()
            .position(|m| m.id == self.id)
            .or_else(|| monitors.iter().position(|m| (m.x, m.y, m.width, m.height) == self.rect))
            .or((self.index < monitors.len()).then_some(self.index))?;
        Some(MonitorKey::of(index, &monitors[index]))
    }
}

/// What became of a pane's monitor when the far end's layout was read again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rebound {
    /// Still there (or the pane shows the whole desktop).
    Kept,
    /// Gone: the monitor that was `index` in the list.
    Gone(usize),
}

/// The main window's monitor once monitor `popped` of `count` has been
/// popped out of it: the next one after it, going round, that no pop-out
/// shows (`shown`), or the whole desktop (`None`) once every monitor has a
/// window of its own.
pub fn next_free(popped: usize, count: usize, shown: &[usize]) -> Option<usize> {
    (1..count).map(|k| (popped + k) % count).find(|i| !shown.contains(i))
}

/// Whether `pane` may send pointer events while `owner` holds the pointer:
/// a button pressed in one window and dragged across another is the first
/// window's until it is let go of, and the second sends nothing meanwhile.
pub fn may_send(owner: Option<PaneId>, pane: PaneId) -> bool {
    owner.is_none_or(|o| o == pane)
}

/// Which pane holds the pointer once `pane` has sent `mask`: the pane,
/// while any button is down; none, once every button is up.
pub fn owner_after(pane: PaneId, mask: i32) -> Option<PaneId> {
    (mask != 0).then_some(pane)
}

pub struct Pane {
    pub id: PaneId,
    pub view: View,
    /// Which of the far end's monitors is on screen, or `None` for the
    /// whole desktop.
    monitor: Option<MonitorKey>,
    /// Where the toolbar sits along the top edge: where the last session
    /// left it, then wherever it is dragged.
    pub placement: Placement,
    /// Where the toolbar's left edge was when the drag under way began.
    drag_from: Option<f32>,
    /// The toolbar's size as last drawn, which is where it would be when
    /// out, and so the span of the edge that brings it out.
    toolbar_size: Vec2,
    /// The width of the toolbar's row of buttons as last drawn, which is
    /// centred in a toolbar made wider for its title bar's title.
    buttons_width: f32,
    /// The pointer is on the toolbar or the title bar above it: the toolbar
    /// stays out, and the keyboard is this end's meanwhile.
    pub toolbar_hovered: bool,
    /// The pointer is on one of the window's own resize edges (see
    /// `csd.rs`): the keyboard is this end's there too, as the window
    /// manager takes it for the resize.
    pub on_edge: bool,
    /// When the pane opened, which is how long its toolbar stays out first.
    opened: Instant,
    /// Whether the pointer was over the desktop in the last frame.
    hovered: bool,
    /// The far end's cursor as this window's own.
    pub native: Native,
    /// The desktop is drawn one to one here, as of the last frame: the
    /// texture is sampled nearest only while every window draws it so.
    exact: bool,
    /// The shape the window's cursor should be this frame, if the far
    /// end's: the software cursor setting off, absolute mode, the pointer
    /// over the desktop, and a shape to show.
    pub native_key: Option<Key>,
    /// Frames drawn, and their rate, for the overlay.
    frames: u64,
    fps: Rate,
    /// The desktop's upload count at the last frame, for the per-frame
    /// figure.
    uploads_seen: u64,
    /// When the oldest frame taken but not yet presented here ended at the
    /// backend: the start of the overlay's render latency.
    pub unpresented: Option<Instant>,
    /// When winit handed over the first pointer event egui has not yet
    /// read, which the next frame turns into the pointer event the backend
    /// is given: the start of the overlay's pointer latency.
    pub pointer_since: Option<Instant>,
    /// The window's size in pixels, as of its last frame: what the follower
    /// asks for this pane's monitor (see `follow.rs`).
    pub size: Option<(u32, u32)>,
    /// Whether egui had a widget of this window's focused at the end of its
    /// last frame, so that a key typed into it is egui's. Kept per window:
    /// egui's own answer is for whichever window ran its pass last.
    pub wants_keys: bool,
    /// The window is to be made the desktop's size once that is known
    /// (`--scale none-resize`).
    pub fit_pending: bool,
}

impl Pane {
    pub fn new(id: PaneId, placement: Placement, monitor: Option<MonitorKey>) -> Pane {
        Pane {
            id,
            view: View::default(),
            monitor,
            placement,
            drag_from: None,
            toolbar_size: vec2(0.0, 40.0),
            buttons_width: 0.0,
            toolbar_hovered: false,
            on_edge: false,
            opened: Instant::now(),
            hovered: false,
            native: Native::default(),
            exact: true,
            native_key: None,
            frames: 0,
            fps: Rate::default(),
            uploads_seen: 0,
            unpresented: None,
            pointer_since: None,
            size: None,
            wants_keys: false,
            fit_pending: false,
        }
    }

    fn main(&self) -> bool {
        self.id == ViewportId::ROOT
    }

    /// Where the title last given to the pane's window is kept, in egui's
    /// data: per pane, as that store is the context's and not a
    /// viewport's.
    fn title_id(&self) -> Id {
        Id::new(("title", self.id))
    }

    fn title_set(&self, ctx: &egui::Context) -> String {
        ctx.data(|d| d.get_temp::<String>(self.title_id())).unwrap_or_default()
    }

    /// The pane's window is no longer the session's: the title it was
    /// given forgotten, so that the next session's is set again, and the
    /// cursor put back.
    pub fn close(&mut self, ctx: &egui::Context, window: &winit::window::Window) {
        ctx.data_mut(|d| d.remove_temp::<String>(self.title_id()));
        self.native.reset(window);
    }

    /// Which of the far end's monitors the pane shows, if one.
    pub fn monitor(&self) -> Option<MonitorKey> {
        self.monitor
    }

    /// A pop-out's monitor, given back: shown here, if this pane shows a
    /// monitor at all (one showing the whole desktop has it already).
    pub fn give_back(&mut self, monitor: MonitorKey) {
        if self.monitor.is_some() {
            self.monitor = Some(monitor);
            self.view.pan = Vec2::ZERO;
        }
    }

    /// The pane's monitor found again in `monitors`, the far end's layout
    /// as it is now (see `MonitorKey::find`); one that is gone leaves the
    /// pane on the whole desktop, and says so.
    pub fn rebind(&mut self, monitors: &[Monitor]) -> Rebound {
        let Some(key) = self.monitor else {
            return Rebound::Kept;
        };
        self.monitor = key.find(monitors);
        match self.monitor {
            Some(_) => Rebound::Kept,
            None => Rebound::Gone(key.index),
        }
    }

    /// This pane's frame: the desktop, the pointer over it, the toolbar,
    /// and the overlay, and in the main window the panel and the prompts,
    /// which are only ever there.
    pub fn frame(&mut self, s: &mut Session, root: &mut egui::Ui, host: &mut Host) {
        let ctx = &root.ctx().clone();
        s.sync_capture(host);

        // Each window names the monitor it shows, so that a window list or
        // a task bar tells the pop-outs apart, and the one with the pointer
        // captured says how to let go of it.
        let count = s.snapshot.facts.monitors.len();
        let title = s.title_of(self.monitor.map(|k| k.index).filter(|_| count > 1), s.captured_in(self.id));
        if title != self.title_set(ctx) {
            host.window.set_title(&title);
            ctx.data_mut(|d| d.insert_temp(self.title_id(), title));
        }

        if self.fit_pending && s.connected() && s.desktop.is_some() {
            self.fit_pending = false;
            self.fit_window(s, host);
        }

        let pixels_per_point = ctx.pixels_per_point();
        let dt = ctx.input(|i| i.stable_dt).min(0.1);
        let mut layout = None;
        let mut response = None;
        egui::CentralPanel::default()
            .frame(Frame::NONE.fill(BACKDROP))
            .show(root, |ui| {
                let window = ui.max_rect();
                let Some((width, height)) = s.desktop.as_ref().map(Desktop::size) else {
                    s.waiting(ui);
                    return;
                };
                let source = self.source_rect(width, height);
                let l = self.view.layout(source, window, pixels_per_point);
                // Nearest only while every window draws the desktop one to
                // one, since the sampler is the texture's and not a
                // window's: two windows at different scales asking for
                // their own would change it back and forth every frame,
                // re-reading a pixel of every tile each time. One drawn
                // scaled anywhere makes it linear everywhere, which at one
                // to one, on whole device pixels, samples each pixel's
                // centre and so draws the same picture.
                let exact = (l.scale * pixels_per_point - 1.0).abs() < 1e-3;
                self.exact = exact;
                let everywhere = exact && s.panes.iter().all(|p| p.exact);
                let desktop = s.desktop.as_mut().expect("checked above");
                desktop.set_exact(ctx, &*s.backend, everywhere);
                // And a picture drawn from the GPU, whose texture is the
                // same one in every window too.
                if let Some(video) = &mut host.video {
                    video.zerocopy.set_nearest(everywhere, video.renderer);
                }
                desktop.paint(ui.painter(), &l);
                response = Some(ui.interact(window, Id::new("desktop"), Sense::click_and_drag()));
                layout = Some(l);
            });
        self.hovered = response.as_ref().is_some_and(Response::hovered);
        self.native_key = None;
        // Taken whether or not there is a desktop for it to be sent over,
        // so that a move from before one was there is not counted later.
        let pointer_since = self.pointer_since.take();
        if let (Some(layout), Some(response)) = (layout, response) {
            self.pointer(s, ctx, &response, &layout, dt, pointer_since);
            if s.settings.software_cursor {
                draw_cursor(s, ctx, &response);
            } else {
                // Device pixels per desktop pixel: what the shape is
                // resampled by so that its hotspot lands where the pointer is.
                let factor = layout.scale * pixels_per_point;
                self.native_key = (self.hovered && !s.relative && s.connected())
                    .then(|| s.cursor.as_ref().map(|c| Key::new(c.hash, factor)))
                    .flatten();
            }
        }

        self.toolbar(s, ctx, host, layout.as_ref());
        if let Some(until) = s.release_hint
            && s.captured_in(self.id)
        {
            let now = Instant::now();
            if now < until {
                let title = if titled(host) { csd::TITLE_BAR } else { 0.0 };
                release_hint(ctx, title + self.toolbar_size.y);
                ctx.request_repaint_after(until - now);
            } else {
                s.release_hint = None;
            }
        }
        // The keyboard is this end's while the pointer is on the toolbar
        // (see `holds_shortcuts`), so what the far end has down is let go
        // of there, as for anything else that takes input away; the
        // releases that follow are `key_elsewhere`'s.
        if self.toolbar_hovered || self.on_edge {
            s.release_keys();
        }
        if self.main() {
            s.panel(ctx, host);
        }
        if s.hud.on {
            let lines = self.hud_lines(s, host, layout.as_ref(), pixels_per_point);
            hud::Hud::draw(ctx, &lines);
        }
        if self.main() {
            s.prompt(ctx);
            if let Some((text, until)) = &s.notice {
                let now = Instant::now();
                if now < *until {
                    notice(ctx, text, Align2::CENTER_BOTTOM);
                    ctx.request_repaint_after(*until - now);
                } else {
                    s.notice = None;
                }
            }
        } else if !s.prompts.is_empty() {
            // The prompt is the main window's, which has been asked to be
            // brought to attention (see `App::draw`).
            notice(ctx, "Answer in the main window", Align2::CENTER_CENTER);
        }
        self.on_edge = csd::resize_edges(ctx, host, csd::CORNER, csd::EDGE);
    }

    /// The desktop pixels on screen: one monitor's, or all of them. The
    /// session follows a monitor that has moved in the far end's list
    /// before the frame (`Session::rebind`); a rectangle that has fallen
    /// off the desktop shows the whole of it.
    fn source_rect(&self, width: usize, height: usize) -> Rect {
        let whole = Rect::from_min_size(Pos2::ZERO, vec2(width as f32, height as f32));
        match self.monitor {
            Some(MonitorKey { rect: (x, y, w, h), .. }) => {
                let r = Rect::from_min_size(pos2(x as f32, y as f32), vec2(w as f32, h as f32)).intersect(whole);
                if r.width() >= 1.0 && r.height() >= 1.0 {
                    r
                } else {
                    whole
                }
            }
            None => whole,
        }
    }

    /// The pointer over the desktop: the edge pan, and in absolute mode the
    /// position, the buttons and the wheel.
    /// `since` is when winit handed over the first of the frame's pointer
    /// events; what was sent to the backend for them is timed from it.
    fn pointer(
        &mut self,
        s: &mut Session,
        ctx: &egui::Context,
        response: &Response,
        layout: &Layout,
        dt: f32,
        since: Option<Instant>,
    ) {
        let over = response.hovered() || response.dragged() || response.is_pointer_button_down_on();
        let pos = ctx.input(|i| i.pointer.latest_pos());

        if over && !s.captured() && let Some(pos) = pos {
            let velocity = layout.edge_velocity(pos);
            if velocity != Vec2::ZERO {
                self.view.pan += velocity * dt;
                ctx.request_repaint();
            }
        }

        if response.clicked() || response.drag_started() {
            // A click on the desktop takes the pointer back after the chord.
            s.escaped = false;
        }
        if !s.accepts_input() || s.relative || !may_send(s.pointer_owner, self.id) {
            return;
        }

        // The frame's events in order rather than the state at its end,
        // so that a press and a release in the same frame are both sent,
        // and in that order.
        let events = ctx.input(|i| i.events.clone());
        let (_, _, mut mask) = s.last_pointer;
        let mut sent = false;
        let desktop = s.desktop.as_ref().map(Desktop::size).unwrap_or((0, 0));
        // With a button held the pointer is this window's even outside it
        // (the display server's implicit grab, and `may_send`), and is
        // carried on past its edges onto the rest of the desktop; without
        // one it stays on the monitor shown, pressing its edge.
        let at_point = |p: Pos2, mask: i32| {
            if mask != 0 {
                layout.desktop_beyond(p, desktop)
            } else {
                layout.desktop_at(p)
            }
        };
        if over && let Some(pos) = pos {
            let mut at = at_point(pos, mask);
            for event in &events {
                match event {
                    egui::Event::PointerMoved(p) => at = at_point(*p, mask),
                    egui::Event::PointerButton {
                        pos: p,
                        button,
                        pressed,
                        ..
                    } => {
                        let bit = match button {
                            PointerButton::Primary => LEFT,
                            PointerButton::Middle => MIDDLE,
                            PointerButton::Secondary => RIGHT,
                            PointerButton::Extra1 => BACK,
                            PointerButton::Extra2 => FORWARD,
                        };
                        at = at_point(*p, mask);
                        mask = if *pressed { mask | bit } else { mask & !bit };
                    }
                    egui::Event::MouseWheel { unit, delta, .. } => {
                        let notches = match unit {
                            egui::MouseWheelUnit::Line => *delta,
                            egui::MouseWheelUnit::Point => *delta / NOTCH_POINTS,
                            egui::MouseWheelUnit::Page => *delta,
                        };
                        for bit in s.notches(notches) {
                            s.backend.pointer(at.0, at.1, mask | bit);
                            s.backend.pointer(at.0, at.1, mask);
                            sent = true;
                        }
                        continue;
                    }
                    _ => continue,
                }
                sent |= s.send_pointer(at, mask);
            }
            sent |= s.send_pointer(at, mask);
        } else if mask != 0 {
            // A button pressed over the desktop and let go elsewhere: the
            // far end is told at the last place it saw.
            let released = events
                .iter()
                .any(|e| matches!(e, egui::Event::PointerButton { pressed: false, .. }));
            if released {
                let (x, y, _) = s.last_pointer;
                sent |= s.send_pointer((x, y), 0);
            }
        }
        s.pointer_owner = owner_after(self.id, s.last_pointer.2);
        if sent && let Some(since) = since {
            s.hud.pointer.record(since.elapsed());
        }
    }

    // ---- the toolbar -------------------------------------------------------

    /// Slid down from the top edge, flush with it, while the pointer is at
    /// the edge within the toolbar's own span or on the toolbar itself, and
    /// back up otherwise, to a peek of its lower edge in a window or with
    /// the pointer on fullscreen's top row, and out of sight in fullscreen
    /// with the pointer anywhere else. Only its own span, because the rest
    /// of the top edge is the desktop's: a pointer heading for something at
    /// the top of the far end's screen, a menu bar or a tab, had the toolbar
    /// come down over it wherever it touched the edge.
    ///
    /// A window drawing its own decorations has its title bar above the
    /// toolbar, as wide as it and moved with it (see `csd.rs`), in a window
    /// and while maximised. There the toolbar comes out from under the
    /// title bar while the pointer is on either, and is otherwise wholly
    /// under it: the title bar shows where it is, so it does not peek.
    ///
    /// It can be dragged along the edge, out of the way of what is under
    /// it, by any part of it: the area senses drags beneath its buttons,
    /// which sense only clicks, so egui gives a press on a button the click
    /// and the area the drag (`hit_test`'s "the top Button and the
    /// ScrollArea behind it"). A drag-only widget starts dragging at the
    /// press, so the toolbar moves only once the pointer is decidedly
    /// dragging, which is also when a release stops being a click: a
    /// button is either clicked or dragged, never both.
    fn toolbar(&mut self, s: &mut Session, ctx: &egui::Context, host: &mut Host, layout: Option<&Layout>) {
        let pointer = ctx.input(|i| i.pointer.latest_pos());
        let screen = ctx.content_rect();
        let fullscreen = host.window.fullscreen().is_some();
        let width = self.toolbar_size.x;
        let left = self.placement.left(screen, width);
        let title = titled(host).then(|| csd::title_rect(screen, left, width));
        // The title bar's title, without the app's name, which a pop-out's
        // monitor makes long: the toolbar, and so the title bar, is made
        // wide enough for it, up to the window's width, rather than cut it
        // short beside a few buttons.
        let count = s.snapshot.facts.monitors.len();
        let name = s.bar_title_of(self.monitor.map(|k| k.index).filter(|_| count > 1), s.captured_in(self.id));
        let wide = if title.is_some() { csd::title_width(ctx, host, &name).min(screen.width()) } else { 0.0 };
        let at_edge = pointer.is_some_and(|p| match title {
            Some(title) => title.contains(p),
            None => toolbar::reveals(screen, left, width, fullscreen, ctx.pixels_per_point(), p),
        });
        let reveal = self.opened.elapsed() < TOOLBAR_REVEAL;
        // A menu of the toolbar's (Pop out's) keeps it out while it is
        // open, as the pointer on it would: the menu hangs from it.
        let menu = egui::Popup::is_any_open(ctx);
        let show = at_edge || self.toolbar_hovered || menu || self.drag_from.is_some() || reveal || s.disconnecting.is_some();
        let style = ctx.global_style();
        // Square, and flush with the top edge: nothing between it and the
        // edge for the pointer to fall into. Flat, too: a popup's frame
        // without the popup's shadow, so that hidden nothing but the peek
        // `hidden_y` leaves shows, and shown nothing of it lies
        // over the desktop past its edge.
        //
        // Edged on three sides and not the top, which is the window's own
        // edge: a line there would be a rule across the top of the screen
        // and, hidden, all that peeked out. A frame's stroke goes all the
        // way round, so the frame is drawn without one and the three sides
        // are painted over it after, inside its rect as a frame's stroke is,
        // with the margin keeping the stroke's width so that the buttons sit
        // where they did.
        let edge = style.visuals.window_stroke();
        let frame = toolbar_frame(&style);
        // Hidden, it peeks in a window always and in fullscreen while the
        // pointer is on the screen's top row anywhere along it; the slide
        // to and from the peek is the same slide, the same 0.2 s.
        let peeks = toolbar::peeks(screen, fullscreen, ctx.pixels_per_point(), pointer);
        // Under a title bar, out below it and hidden wholly behind it.
        let (out, hidden) = match title {
            Some(title) => (title.height(), title.height() - self.toolbar_size.y),
            None => (0.0, toolbar::hidden_y(self.toolbar_size.y, peeks)),
        };
        // Keyed by the pane: egui's animations are the context's, by id
        // alone, and two toolbars on one id would slide as one.
        let y = ctx.animate_value_with_time(Id::new(("toolbar-y", self.id)), if show { out } else { hidden }, 0.2);
        if reveal {
            ctx.request_repaint_after(TOOLBAR_REVEAL.saturating_sub(self.opened.elapsed()));
        }
        let title_hovered = title.is_some_and(|rect| {
            // The toolbar shows nothing below the title bar.
            let hidden = y + self.toolbar_size.y <= out + 0.5;
            // The shape of what the window shows, which a resize keeping
            // the aspect ratio keeps, so that it has no black bars.
            let aspect = s.desktop.as_ref().map(Desktop::size).map(|(width, height)| {
                let source = self.source_rect(width, height);
                source.width() / source.height()
            });
            csd::title_bar(ctx, host, rect, &name, hidden, aspect).contains_pointer()
        });

        let area = egui::Area::new(Id::new("toolbar"))
            .order(Order::Foreground)
            .fixed_pos(pos2(left, screen.min.y + y))
            // Drawn and hit-tested only below the title bar, if there is
            // one, which it slides out from under.
            .constrain_to(Rect::from_min_max(pos2(screen.min.x, screen.min.y + out), screen.max))
            // Unconstrained, or egui would keep it on screen: off the top
            // is where it goes to hide.
            .constrain(false)
            .interactable(true)
            .sense(Sense::drag())
            .show(ctx, |ui| {
                let rect = frame
                    .show(ui, |ui| {
                        // A little taller than egui's buttons, which were
                        // made for a menu rather than for a bar at a
                        // screen's edge.
                        ui.spacing_mut().interact_size.y = TOOLBAR_BUTTON;
                        // The row of buttons after the first space is spaced
                        // from it as any item is.
                        let spacing = ui.spacing().item_spacing.x;
                        let pad = ((wide - frame.total_margin().sum().x - self.buttons_width - spacing) / 2.0).max(0.0);
                        ui.horizontal(|ui| {
                            ui.add_space(pad);
                            let row = ui.horizontal(|ui| self.toolbar_items(s, ui, host, layout)).response;
                            self.buttons_width = row.rect.width();
                            ui.add_space(pad);
                        });
                    })
                    .response
                    .rect;
                let inner = rect.shrink(edge.width / 2.0);
                let sides = vec![
                    pos2(inner.min.x, rect.min.y),
                    inner.left_bottom(),
                    inner.right_bottom(),
                    pos2(inner.max.x, rect.min.y),
                ];
                ui.painter().add(egui::Shape::line(sides, edge));
            });
        let response = &area.response;
        if response.drag_started() {
            self.drag_from = Some(left);
        }
        if let Some(from) = self.drag_from {
            if !response.dragged() {
                self.drag_from = None;
            } else if ctx.input(|i| i.pointer.is_decidedly_dragging()) {
                let moved = response.total_drag_delta().unwrap_or_default().x;
                self.placement.move_to(screen, width, from + moved);
                ctx.set_cursor_icon(CursorIcon::Grabbing);
            }
        } else if response.hovered() && !over_button(ctx, response) {
            // The margins, which are where to take hold of it (the buttons'
            // interact rects meet, so there are no gaps between them), but
            // not the buttons, which are pressed before they are dragged.
            ctx.set_cursor_icon(CursorIcon::Grab);
        }
        // A button that came or went this frame (Sound, as the backend offers
        // its switch; Monitor and Pop out) changed the width it
        // was placed by, and the hover was hit-tested against the old
        // buttons: one more frame puts it where its placement says, and the
        // highlight under a pointer that has not moved, which nothing else
        // may draw again for a while.
        if response.rect.size() != self.toolbar_size {
            ctx.request_repaint();
        }
        self.toolbar_size = response.rect.size();
        // A drag that has run off the toolbar is still the toolbar's, and
        // so is the keyboard while it lasts; the title bar is this end's
        // as the toolbar is.
        self.toolbar_hovered = response.contains_pointer() || title_hovered || self.drag_from.is_some() || menu;
    }

    /// The toolbar's buttons: Material Symbols and no words, each with its
    /// name and its state in its tooltip. A button with a state shows it in
    /// its icon (one per scale mode, all monitors or one, view only on or
    /// off) and the tooltip says the rest (which monitor or display);
    /// Info and view only are highlighted while they are on, Sound while
    /// off or muted.
    ///
    /// The main window's: Scale, Resize, Monitor, Pop out, View only,
    /// Sound, Info, Fullscreen, Disconnect. A pop-out's: Scale, Resize,
    /// Monitor, View only, Sound, Fullscreen, Return. Resize only in a
    /// window: at 1:1 while it is not already the size it asks for, in the
    /// other modes while the desktop has black bars beside it. Sound only
    /// where the far end may send sound. `layout` is where the desktop was
    /// drawn this frame, if it was.
    fn toolbar_items(&mut self, s: &mut Session, ui: &mut egui::Ui, host: &mut Host, layout: Option<&Layout>) {
        let connected = s.connected();
        let mode = self.view.mode;
        // A right click goes back through the same list.
        let scale = ui.add_enabled(connected, icon_only(scale_icon(mode))).on_hover_text(format!(
            "Scale: {} (next: {}).\nIf the desktop is larger than the window, it will pan \
             when the cursor is near the edges.",
            mode.label().to_lowercase(),
            mode.next().label().to_lowercase(),
        ));
        if scale.clicked() {
            self.view.cycle();
        } else if scale.secondary_clicked() {
            self.view.cycle_back();
        }
        // At 1:1 in a window, the window can be made the size of what it
        // shows, which at that scale is its size in device pixels. Hidden
        // while the window is that size already; a window manager that
        // keeps another size leaves it shown.
        let fitted = s
            .desktop
            .as_ref()
            .map(Desktop::size)
            .is_some_and(|(width, height)| host.window.inner_size() == fit_size(self.source_rect(width, height)));
        if mode == ScaleMode::Original
            && host.window.fullscreen().is_none()
            && !fitted
            && ui
                .add_enabled(connected, icon_only(icons::RESIZE))
                .on_hover_text("Resize window to desktop size")
                .clicked()
        {
            self.fit_window(s, host);
        }
        // In the other modes, a desktop of another shape than the window
        // has black bars beside it. A window made narrower or shorter by
        // them shows the desktop at the same scale, filling it (see
        // `trimmed_size`); taken off the window's inner size rather than
        // made the desktop's, so that anything else in the window keeps
        // its room. The 1:1 button's icon, and hidden as that is: in
        // fullscreen, and once the window is the size it asks for.
        if mode != ScaleMode::Original
            && host.window.fullscreen().is_none()
            && let Some(size) = layout.and_then(|l| {
                let min = super::super::min_size(&ui.ctx().global_style()).to_physical(host.window.scale_factor());
                trimmed_size(host.window.inner_size(), l.bars(), min)
            })
            && ui
                .add_enabled(connected, icon_only(icons::RESIZE))
                .on_hover_text("Resize window to remove black bars")
                .clicked()
        {
            request_size(host, size);
        }

        let facts = &s.snapshot.facts;
        // Several screens while showing them all, one while showing one;
        // which one is the tooltip's to say, since a number on the icon
        // would be the word again.
        if facts.monitors.len() > 1 {
            let count = facts.monitors.len();
            let current = self.monitor.map(|k| k.index);
            let symbol = if current.is_some() { icons::MONITOR } else { icons::TV_DISPLAYS };
            let button = ui.add_enabled(connected, icon_only(symbol)).on_hover_text(format!(
                "Monitor: {} (next: {}).\nThe whole desktop, or one of its monitors.",
                monitor_name(current, count),
                monitor_name(next_monitor(current, count), count),
            ));
            let choice = if button.clicked() {
                Some(next_monitor(current, count))
            } else if button.secondary_clicked() {
                Some(previous_monitor(current, count))
            } else {
                None
            };
            if let Some(choice) = choice {
                self.monitor = choice.map(|i| MonitorKey::of(i, &facts.monitors[i]));
                self.view.pan = Vec2::ZERO;
            }
            if self.main() {
                self.pop_out_button(s, ui, connected);
            }
        } else if facts.displays.len() > 1 {
            let count = facts.displays.len();
            let next = (facts.display + 1).rem_euclid(count as i32);
            let button = ui.add_enabled(connected, icon_only(icons::MONITOR)).on_hover_text(format!(
                "Display: {} of {count} (next: {}).",
                facts.display + 1,
                next + 1,
            ));
            if button.clicked() {
                s.backend.request_display(next);
            } else if button.secondary_clicked() {
                s.backend.request_display((facts.display - 1).rem_euclid(count as i32));
            }
        }

        // An eye while only watching, a mouse while in control, so that the
        // icon says which without the highlight's help.
        let view_only = s.view_only;
        let symbol = if view_only { icons::VISIBILITY } else { icons::MOUSE };
        // Disabled under `--view-only`, which forces it.
        if ui
            .add_enabled(connected && !s.settings.view_only, toggle(ui.visuals(), symbol, view_only))
            .on_hover_text(if view_only {
                "View only: on."
            } else {
                "View only: off."
            })
            .clicked()
        {
            s.set_option("ViewOnly", if view_only { "false" } else { "true" });
        }

        // Whenever the far end may send sound (from when its protocol says
        // so, which for most is before the first sound), so that a session
        // without any (a protocol with none, RDP's Sound off) has no button
        // that does nothing. What it does is the backend's to say: ask the
        // far end to stop and to send again where the protocol can, mute
        // it here where not. Highlighted while the sound is off or muted, as
        // View only is while on: the state that is not the usual one reads
        // from across the toolbar. The session's, so every window's button
        // is the same one.
        if let Some(switch) = s.sound.switch {
            let (symbol, tip) = sound_button(switch, s.sound.on());
            let quiet = !s.sound.audible();
            if ui.add(toggle(ui.visuals(), symbol, quiet)).on_hover_text(tip).clicked() {
                s.set_sound(!s.sound.on());
            }
        }

        if self.main()
            && ui
                .add_enabled(connected, toggle(ui.visuals(), icons::INFO, s.panel.open))
                .on_hover_text("Connection information and settings")
                .clicked()
        {
            s.panel.open = !s.panel.open;
        }

        let fullscreen = host.window.fullscreen().is_some();
        if ui
            .add(icon_only(if fullscreen { icons::FULLSCREEN_EXIT } else { icons::FULLSCREEN }))
            .on_hover_text(if fullscreen {
                "Leave fullscreen"
            } else {
                "Enter fullscreen"
            })
            .clicked()
        {
            log::info!("{}", if fullscreen { "leaving fullscreen" } else { "fullscreen" });
            host.window
                .set_fullscreen((!fullscreen).then_some(Fullscreen::Borderless(None)));
        }

        if !self.main() {
            if ui
                .add(icon_only(icons::PIP_EXIT))
                .on_hover_text("Reattach monitor to main window")
                .clicked()
            {
                s.ops.push(WindowOp::Close { pane: self.id, returned: true });
            }
            return;
        }

        if ui
            .add_enabled(s.disconnecting.is_none() && s.closed.is_none(), icon_only(icons::LINK_OFF))
            .on_hover_text("Disconnect")
            .clicked()
        {
            s.disconnect();
        }
    }

    /// Asks for the window to be the size of what it shows (the desktop, or
    /// its monitor), in device pixels: one to one, nothing is panned or
    /// left over.
    fn fit_window(&self, s: &Session, host: &Host) {
        let Some((width, height)) = s.desktop.as_ref().map(Desktop::size) else {
            return;
        };
        request_size(host, fit_size(self.source_rect(width, height)));
    }

    /// The main window's Pop out, beside Monitor: what this window shows,
    /// in windows of its own. Showing one monitor, that monitor goes to a
    /// new window and this one moves on to the next; showing the whole
    /// desktop, every monitor without a window gets one, this window
    /// keeping the first. The new windows are plain ones, placed where the
    /// window manager puts new windows, to be moved and made fullscreen as
    /// the user likes (their own Fullscreen fills the output each is on).
    fn pop_out_button(&mut self, s: &mut Session, ui: &mut egui::Ui, connected: bool) {
        let tip = match self.monitor {
            Some(key) => format!("Detach monitor {} into window", key.index + 1),
            None => "Detach all monitors".to_string(),
        };
        if !ui.add_enabled(connected, icon_only(icons::OPEN_IN_NEW)).on_hover_text(tip).clicked() {
            return;
        }
        match self.monitor {
            Some(key) => self.pop_out(s, key),
            None => self.pop_out_all(s),
        }
    }

    /// `key`, this window's monitor, popped out to a window of its own, and
    /// this window on to the next monitor that has no window.
    fn pop_out(&mut self, s: &mut Session, key: MonitorKey) {
        s.pop_out(key);
        let monitors = &s.snapshot.facts.monitors;
        let shown: Vec<usize> = s.panes.iter().filter_map(|p| p.monitor.map(|k| k.index)).collect();
        self.monitor = next_free(key.index, monitors.len(), &shown).map(|i| MonitorKey::of(i, &monitors[i]));
        self.view.pan = Vec2::ZERO;
    }

    /// A window for every monitor that has none, in the far end's order:
    /// this one keeps the first.
    fn pop_out_all(&mut self, s: &mut Session) {
        let monitors = s.snapshot.facts.monitors.clone();
        let shown: Vec<usize> = s.panes.iter().filter_map(|p| p.monitor.map(|k| k.index)).collect();
        let free = (0..monitors.len()).filter(|i| !shown.contains(i));
        for (k, monitor) in free.enumerate() {
            let key = MonitorKey::of(monitor, &monitors[monitor]);
            if k == 0 {
                self.monitor = Some(key);
                self.view.pan = Vec2::ZERO;
            } else {
                s.pop_out(key);
            }
        }
    }

    // ---- the debug overlay -------------------------------------------------

    /// `SessionHud.draw`'s lines, in its order where they apply here: the
    /// state and the pixel path, the cursor, the physical mouse and
    /// keyboard, and the things this window has that the phone has not
    /// (the session's, the same in every window), and under them this
    /// window's own: which window it is and what it shows, on which output,
    /// its frame rate and uploads, its view, its shortcuts and the capture.
    fn hud_lines(&mut self, s: &mut Session, host: &Host, layout: Option<&Layout>, pixels_per_point: f32) -> Vec<String> {
        let now = Instant::now();
        self.frames += 1;
        let fps = self.fps.sample(self.frames, now);
        let damage_rate = s.hud.damage.sample(s.damage, now);
        let (uploads, bytes, last) = s.desktop.as_ref().map(Desktop::stats).unwrap_or((0, 0, None));
        let per_frame = uploads.saturating_sub(self.uploads_seen);
        self.uploads_seen = uploads;
        let read_rate = s.hud.bytes.sample(bytes, now);
        let (width, height) = s.desktop.as_ref().map(Desktop::size).unwrap_or((0, 0));
        // `SessionClipboard.summary`: what the far end would be given.
        let clip = match s.clipboard.seen() {
            Some(text) if s.settings.clipboard_out => format!("{}c", text.chars().count()),
            _ => "-".to_string(),
        };
        let state = format!("{:?}", s.mailbox.state()).to_ascii_lowercase();
        let first = format!(
            "state {state}  desktop {width}x{height}  dmg {} ({damage_rate}/s)  rd {}  last {}  clip {clip}",
            s.damage,
            hud::bytes_per_second(read_rate),
            last.map(|r| format!("{},{} {}x{}", r.x, r.y, r.width, r.height))
                .unwrap_or_else(|| "-".to_string()),
        );

        let (x, y, mask) = s.last_pointer;
        let cursor = if s.relative {
            "theirs".to_string()
        } else {
            format!("{x},{y}")
        };
        let view = match layout {
            Some(l) => {
                let origin = (l.rect.min - l.window.min) * pixels_per_point;
                let content = l.rect.size() * pixels_per_point;
                format!(
                    "scale {:.3} [{}]  origin {},{}  content {}x{}",
                    l.scale * pixels_per_point,
                    self.view.mode.label(),
                    origin.x.round(),
                    origin.y.round(),
                    content.x.round(),
                    content.y.round()
                )
            }
            None => "scale -".to_string(),
        };
        let second = format!("cursor {cursor}  btn {}", hud::buttons_name(mask));

        let mouse = match s.capture {
            Some((pane, Capture::Locked)) if pane == self.id => "captured here (locked)",
            Some((pane, Capture::Centred)) if pane == self.id => "captured here (centred)",
            Some(_) => "captured in another window",
            None if self.hovered => "hover",
            None => "-",
        };
        // Every whole pixel of raw motion sent so far in relative mode, so
        // that motion counted twice shows as twice the distance.
        let sent = if s.relative {
            format!("  Δ {},{}", s.motion_sent.0, s.motion_sent.1)
        } else {
            String::new()
        };
        let third = format!(
            "mouse {mouse}{sent}  btn {}  keys {} held{}",
            hud::buttons_name(if s.captured() { s.captured_mask } else { mask }),
            s.keyboard.held(),
            if s.view_only { "   VIEW ONLY" } else { "" }
        );

        let follow = if !s.follower.enabled() {
            "off".to_string()
        } else {
            match s.follower.followed() {
                Some((w, h)) => format!("{w}x{h}"),
                None => "waiting".to_string(),
            }
        };
        let shape = match &s.cursor {
            None => "none".to_string(),
            Some(c) => {
                let how = if self.native.shown().is_some() {
                    "native"
                } else if !s.settings.software_cursor {
                    "native (off desktop)"
                } else {
                    "software"
                };
                format!("{how} {}x{} hot {},{}", c.source.width, c.source.height, c.source.hot_x, c.source.hot_y)
            }
        };
        let fourth = format!(
            "{}  follow {follow}  shape {shape}  clipboard out {} in {}",
            host.display_server.name(),
            on_off(s.settings.clipboard_out),
            on_off(s.settings.clipboard_in)
        );
        // What can be timed at this end, and only that: how long the far
        // end takes to act on input and send back what it did is not
        // something any of the protocols lets this end see.
        let fifth = format!(
            "render {}  pointer {}  keys {}",
            s.hud.render.summary(),
            s.hud.pointer.summary(),
            s.hud.keys.summary()
        );
        let count = s.snapshot.facts.monitors.len();
        let window = format!(
            "window {}  monitor {}  output {}  {}  fps {fps}  tex {per_frame}/frame  ppp {pixels_per_point:.2}",
            if self.main() { "main" } else { "pop-out" },
            match self.monitor {
                Some(k) => format!("{} of {count}", k.index + 1),
                None => "all".to_string(),
            },
            host.output.as_deref().unwrap_or("-"),
            if host.focused { "focused" } else { "unfocused" },
        );
        let shortcuts = format!("{view}  inhibitor {}", host.inhibitor);
        let mut lines = vec![first, second, third, fourth, fifth];
        // How the video is decoded, where the backend decodes video through
        // this end's decoders: the line is the decoder's, made on the
        // protocol thread a couple of times a second, and only copied here.
        if let Some(status) = s.backend.decoder_status() {
            match s.zero_copy_summary(host) {
                Some(how) => lines.push(format!("dec {status}  {how}")),
                None => lines.push(format!("dec {status}")),
            }
        }
        // The sound, wherever the session has a switch for it: what plays,
        // what is queued in front of the device and the device's own buffer
        // (the two together are how far behind what arrived it is heard),
        // and what the queue has had to do about the far end's timing; and
        // whether the far end is asked not to send it, or it is muted here.
        if let Some(line) = s.sound.line(s.playback()) {
            lines.push(line);
        }
        lines.extend([window, shortcuts]);
        lines
    }
}

/// Whether the window has its own title bar: drawn with its own
/// decorations, and not fullscreen.
fn titled(host: &Host) -> bool {
    host.csd && host.window.fullscreen().is_none()
}

/// The far end's cursor under the local pointer, in absolute mode.
fn draw_cursor(s: &Session, ctx: &egui::Context, response: &Response) {
    if s.relative || !s.connected() {
        return;
    }
    let Some(cursor) = &s.cursor else {
        return;
    };
    if !response.hovered() {
        return;
    }
    let Some(pos) = ctx.input(|i| i.pointer.latest_pos()) else {
        return;
    };
    ctx.set_cursor_icon(CursorIcon::None);
    let rect = Rect::from_min_size(pos - cursor.hot, cursor.size);
    ctx.layer_painter(LayerId::new(Order::Tooltip, Id::new("remote-cursor")))
        .image(
            cursor.texture.id(),
            rect,
            Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
            Color32::WHITE,
        );
}

/// A line of text over the desktop, in a box, at `align`: a notice in the
/// main window, or a pop-out pointing at the main window's prompt.
fn notice(ctx: &egui::Context, text: &str, align: Align2) {
    let offset = vec2(0.0, if align == Align2::CENTER_BOTTOM { -48.0 } else { 0.0 });
    egui::Area::new(Id::new("notice"))
        .order(Order::Foreground)
        .anchor(align, offset)
        .interactable(false)
        .show(ctx, |ui| {
            Frame::popup(&ctx.global_style()).show(ui, |ui| ui.label(text));
        });
}

/// The hint at the session's first capture of the pointer, top and centre
/// below where the toolbar comes down to (`toolbar_height`): framed as the
/// toolbar is, but with rounded corners and only a line of text high.
fn release_hint(ctx: &egui::Context, toolbar_height: f32) {
    let style = ctx.global_style();
    let frame = Frame::popup(&style)
        .shadow(egui::Shadow::NONE)
        .stroke(style.visuals.window_stroke())
        .inner_margin(Margin::symmetric(10, 4));
    egui::Area::new(Id::new("release-hint"))
        .order(Order::Foreground)
        .anchor(Align2::CENTER_TOP, vec2(0.0, toolbar_height + 12.0))
        .interactable(false)
        .show(ctx, |ui| {
            frame.show(ui, |ui| ui.label("Press Ctrl+Alt to release pointer."));
        });
}

/// A toolbar icon, at the toolbar's size.
fn icon(symbol: &'static str) -> RichText {
    icons::text(symbol).size(TOOLBAR_ICON)
}

/// The Scale button's icon for a mode: `desktop_portrait` for Auto,
/// `aspect_ratio` for Fit and `open_in_full` for Fill; Material Symbols'
/// own fit page width and height for the fit modes, arrows kept within a
/// page as fit keeps the desktop within 1:1; the bare width and height
/// arrows for the fill modes, which stretch the desktop along that axis
/// however far the window asks; and the symbol that reads "1:1" for 1:1.
fn scale_icon(mode: ScaleMode) -> &'static str {
    match mode {
        ScaleMode::Auto => icons::DESKTOP_PORTRAIT,
        ScaleMode::Fit => icons::ASPECT_RATIO,
        ScaleMode::Fill => icons::OPEN_IN_FULL,
        ScaleMode::FitWidth => icons::FIT_PAGE_WIDTH,
        ScaleMode::FitHeight => icons::FIT_PAGE_HEIGHT,
        ScaleMode::FillWidth => icons::WIDTH,
        ScaleMode::FillHeight => icons::HEIGHT,
        ScaleMode::Original => icons::VIEW_REAL_SIZE,
    }
}

/// The sound button's icon and tooltip, for what the switch does and
/// whether it is `on`: a speaker sounding while the far end's sound is
/// heard, a speaker struck through while the far end is asked not to send
/// it (an Enable switch off), and a speaker with nothing coming out while it
/// arrives and is muted here (a Mute switch on).
fn sound_button(switch: Switch, on: bool) -> (&'static str, &'static str) {
    match (switch, on) {
        (Switch::Enable, true) => (
            icons::VOLUME_UP,
            "Sound: on (click to disable)",
        ),
        (Switch::Enable, false) => (
            icons::VOLUME_OFF,
            "Sound: off (click to enable)",
        ),
        (Switch::Mute, false) => (
            icons::VOLUME_UP,
            "Sound: on (click to mute)",
        ),
        (Switch::Mute, true) => (
            icons::VOLUME_MUTE,
            "Sound: muted (server is still sending audio, click to enable)",
        ),
    }
}

/// Whether the pointer is on one of the toolbar's buttons rather than on
/// the toolbar `area` itself. Both count as hovered, as egui hovers the top
/// button and the area that senses drags behind it alike, so it is told
/// apart by what else is under the pointer on the toolbar's layer: anything
/// there that takes clicks is a button. (On its layer only: the desktop
/// under the toolbar takes clicks too.) Read out first and then looked up,
/// as the snapshot holds the context that `read_response` takes too.
fn over_button(ctx: &egui::Context, area: &egui::Response) -> bool {
    let under: Vec<Id> = ctx.interaction_snapshot(|s| s.contains_pointer.iter().copied().collect());
    under.into_iter().any(|id| {
        id != area.id
            && ctx
                .read_response(id)
                .is_some_and(|r| r.layer_id == area.layer_id && r.sense.senses_click())
    })
}

/// The toolbar's frame (see [`Pane::toolbar`]).
fn toolbar_frame(style: &egui::Style) -> Frame {
    let edge_width = style.visuals.window_stroke().width.round() as i8;
    Frame::popup(style)
        .corner_radius(0)
        .shadow(egui::Shadow::NONE)
        .stroke(Stroke::NONE)
        .inner_margin(Margin::symmetric(8 + edge_width, 6 + edge_width))
}

/// The most buttons the toolbar has at once: the main window's nine (see
/// [`Pane::toolbar_items`]).
const TOOLBAR_MOST: usize = 9;

/// The width of the toolbar with all of its buttons, in points.
pub fn toolbar_width(style: &egui::Style) -> f32 {
    let buttons = TOOLBAR_MOST as f32;
    buttons * TOOLBAR_BUTTON
        + (buttons - 1.0) * style.spacing.item_spacing.x
        + toolbar_frame(style).total_margin().sum().x
}

/// A toolbar button of an icon alone, square, and flat: no frame at rest,
/// egui's while the pointer is on it or pressing it.
fn icon_only(symbol: &'static str) -> egui::Button<'static> {
    egui::Button::new(icon(symbol)).min_size(Vec2::splat(TOOLBAR_BUTTON)).frame_when_inactive(false)
}

/// A toolbar button that is on or off: flat while off, and while on framed
/// at rest too (as `Button::selectable` is), in the selection's colours, so
/// that on reads from across the toolbar without a hover: its icon in the
/// selection's colour, on a fill of the selection's blue at about half its
/// strength rather than all of it, which three buttons on at once made the
/// loudest thing on the screen.
fn toggle(visuals: &egui::Visuals, symbol: &'static str, on: bool) -> egui::Button<'static> {
    let button = icon_only(symbol).selected(on).frame_when_inactive(on);
    if on { button.fill(visuals.selection.bg_fill.gamma_multiply(0.5)) } else { button }
}

/// The Monitor button's next choice after `current` of `count` monitors:
/// the whole desktop (`None`), then each monitor in turn, then the whole
/// desktop again.
fn next_monitor(current: Option<usize>, count: usize) -> Option<usize> {
    match current {
        None if count > 0 => Some(0),
        Some(i) if i + 1 < count => Some(i + 1),
        _ => None,
    }
}

/// The window's inner size, in device pixels, that shows `source` (in
/// desktop pixels) one to one with nothing panned or left over.
fn fit_size(source: Rect) -> PhysicalSize<u32> {
    PhysicalSize::new(source.width().round() as u32, source.height().round() as u32)
}

/// The window's inner size `inner` less the black bars `bars` (see
/// [`Layout::bars`]), both in device pixels, and no smaller than the
/// window's minimum `min`; `None` where that is no more than a pixel off
/// `inner` in either axis. A bar of a pixel is what the rounding of the
/// desktop's size to whole pixels leaves (and on Wayland that of the
/// window's to logical pixels), which a window this function made the
/// right size can still have.
fn trimmed_size(inner: PhysicalSize<u32>, bars: Vec2, min: PhysicalSize<u32>) -> Option<PhysicalSize<u32>> {
    let axis = |inner: u32, bar: f32, min: u32| {
        let trimmed = inner.saturating_sub(bar as u32).max(min);
        if inner.saturating_sub(trimmed) > 1 { trimmed } else { inner }
    };
    let size = PhysicalSize::new(axis(inner.width, bars.x, min.width), axis(inner.height, bars.y, min.height));
    (size != inner).then_some(size)
}

/// Asks for the window's inner size to be `size`, in device pixels. A
/// maximised window is unmaximised first, since a window manager keeps a
/// maximised window's size.
fn request_size(host: &Host, size: PhysicalSize<u32>) {
    log::info!("resizing the window to {}×{}", size.width, size.height);
    if host.window.is_maximized() {
        host.window.set_maximized(false);
    }
    let _ = host.window.request_inner_size(size);
}

/// The Monitor button's choice before `current`, going back through
/// [`next_monitor`]'s order: the last monitor before the whole desktop.
fn previous_monitor(current: Option<usize>, count: usize) -> Option<usize> {
    match current {
        None => count.checked_sub(1),
        Some(i) if i > 0 && i < count => Some(i - 1),
        _ => None,
    }
}

/// A Monitor choice in words, for the tooltip.
fn monitor_name(monitor: Option<usize>, count: usize) -> String {
    match monitor {
        None => format!("all {count}"),
        Some(i) => format!("{} of {count}", i + 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(x: i32, y: i32, width: i32, height: i32) -> Monitor {
        Monitor {
            id: 0,
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn the_sound_button_says_its_state_in_its_icon_and_its_tooltip() {
        let (enabled, enabled_tip) = sound_button(Switch::Enable, true);
        let (disabled, disabled_tip) = sound_button(Switch::Enable, false);
        let (heard, heard_tip) = sound_button(Switch::Mute, false);
        let (muted, muted_tip) = sound_button(Switch::Mute, true);
        assert_eq!(
            (enabled, disabled, heard, muted),
            (icons::VOLUME_UP, icons::VOLUME_OFF, icons::VOLUME_UP, icons::VOLUME_MUTE)
        );
        assert!(enabled_tip.starts_with("Sound: on") && enabled_tip.contains("disable"));
        assert!(disabled_tip.starts_with("Sound: off"));
        assert!(heard_tip.starts_with("Sound: on") && heard_tip.contains("mute"));
        assert!(muted_tip.starts_with("Sound: muted"));
    }

    #[test]
    fn the_resize_button_asks_for_the_shown_rectangle_in_device_pixels() {
        let whole = Rect::from_min_size(Pos2::ZERO, vec2(1024.0, 768.0));
        assert_eq!(fit_size(whole), PhysicalSize::new(1024, 768));
        let second = Rect::from_min_size(pos2(1920.0, 0.0), vec2(1600.0, 900.0));
        assert_eq!(fit_size(second), PhysicalSize::new(1600, 900));
    }

    #[test]
    fn every_scale_mode_has_its_own_icon() {
        let mut mode = ScaleMode::Auto;
        let mut seen = Vec::new();
        for _ in 0..8 {
            let codepoint = scale_icon(mode);
            assert!(!seen.contains(&codepoint), "{} shares its icon", mode.label());
            seen.push(codepoint);
            mode = mode.next();
        }
        assert_eq!(mode, ScaleMode::Auto);
    }

    #[test]
    fn the_monitor_button_goes_round_all_then_each() {
        let mut monitor = None;
        let mut seen = Vec::new();
        for _ in 0..4 {
            seen.push(monitor_name(monitor, 3));
            monitor = next_monitor(monitor, 3);
        }
        assert_eq!(seen, ["all 3", "1 of 3", "2 of 3", "3 of 3"]);
        assert_eq!(monitor, None);
        // A monitor gone from under the choice goes back to all of them.
        assert_eq!(next_monitor(Some(5), 2), None);
        // Back the other way round.
        let mut back = Vec::new();
        for _ in 0..4 {
            monitor = previous_monitor(monitor, 3);
            back.push(monitor_name(monitor, 3));
        }
        assert_eq!(back, ["3 of 3", "2 of 3", "1 of 3", "all 3"]);
        assert_eq!(previous_monitor(Some(5), 2), None);
    }

    fn numbered(id: u32, m: Monitor) -> Monitor {
        Monitor { id, ..m }
    }

    #[test]
    fn a_monitor_is_found_by_its_id_first() {
        let two = [numbered(1, monitor(0, 0, 1920, 1080)), numbered(2, monitor(1920, 0, 1280, 1024))];
        let right = MonitorKey::of(1, &two[1]);
        // Unchanged: itself.
        assert_eq!(right.find(&two), Some(right));
        // Resized and moved to the front of the list, as a layout asked for
        // can make it: still the one with its id, wherever and whatever
        // shape it is.
        let asked = [numbered(2, monitor(0, 0, 1600, 900)), numbered(1, monitor(1600, 0, 1920, 1080))];
        assert_eq!(right.find(&asked), Some(MonitorKey { id: 2, rect: (0, 0, 1600, 900), index: 0 }));
    }

    #[test]
    fn a_renumbered_monitor_is_found_by_its_rectangle_then_its_place() {
        let two = [numbered(1, monitor(0, 0, 1920, 1080)), numbered(2, monitor(1920, 0, 1280, 1024))];
        let right = MonitorKey::of(1, &two[1]);
        // The far end numbered its screens afresh and added one before
        // them: the same rectangle is the same monitor, at its new place.
        let three = [
            numbered(7, monitor(-1280, 0, 1280, 1024)),
            numbered(8, two[0]),
            numbered(9, two[1]),
        ];
        assert_eq!(right.find(&three), Some(MonitorKey::of(2, &three[2])));
        // Renumbered and resized where it is: the same place in the list,
        // its new id and rectangle kept from now on.
        let resized = [numbered(8, two[0]), numbered(9, monitor(1920, 0, 1600, 900))];
        let found = right.find(&resized).unwrap();
        assert_eq!(found, MonitorKey { id: 9, rect: (1920, 0, 1600, 900), index: 1 });
        // Gone, with nothing of its name, its shape or its place.
        assert_eq!(right.find(&two[..1]), None);
        assert_eq!(right.find(&[]), None);
    }

    #[test]
    fn a_drag_belongs_to_the_pane_it_was_pressed_in() {
        let main = ViewportId::ROOT;
        let other = ViewportId::from_hash_of("pop-out");
        // No pane holds the pointer: any may send.
        assert!(may_send(None, main));
        assert!(may_send(None, other));
        // A press in the main window makes it the holder; the other window
        // sends nothing while the drag lasts, even with the pointer over it.
        let owner = owner_after(main, super::super::LEFT);
        assert_eq!(owner, Some(main));
        assert!(may_send(owner, main));
        assert!(!may_send(owner, other));
        // Every button up: no pane again.
        assert_eq!(owner_after(main, 0), None);
        assert!(may_send(owner_after(main, 0), other));
        // Moving with nothing pressed holds nothing.
        assert_eq!(owner_after(other, 0), None);
    }

    #[test]
    fn a_popped_out_monitor_moves_the_main_window_on_to_the_next_free_one() {
        // Two monitors, the main window on the first: popped out, the main
        // window shows the second.
        assert_eq!(next_free(0, 2, &[0]), Some(1));
        // Going round: the last popped out, the first is next.
        assert_eq!(next_free(2, 3, &[2]), Some(0));
        // One that already has a window is passed over.
        assert_eq!(next_free(0, 3, &[0, 1]), Some(2));
        // Every monitor in a window of its own: the whole desktop.
        assert_eq!(next_free(1, 2, &[0, 1]), None);
    }

    #[test]
    fn a_pane_s_gone_monitor_is_said_to_be_gone() {
        let two = [numbered(1, monitor(0, 0, 1280, 800)), numbered(2, monitor(1280, 0, 1280, 800))];
        let mut pane = Pane::new(ViewportId::from_hash_of("pop-out"), Placement::default(), Some(MonitorKey::of(1, &two[1])));
        // Still there, moved along the list by a screen added before it.
        let three = [numbered(3, monitor(-1280, 0, 1280, 800)), two[0], two[1]];
        assert_eq!(pane.rebind(&three), Rebound::Kept);
        assert_eq!(pane.monitor().map(|k| k.index), Some(2));
        // Down to one screen of another shape: gone, from where it last was.
        assert_eq!(pane.rebind(&[monitor(0, 0, 1920, 1080)]), Rebound::Gone(2));
        assert_eq!(pane.monitor(), None);
        // The whole desktop is never gone.
        assert_eq!(pane.rebind(&[]), Rebound::Kept);
    }

    #[test]
    fn a_window_loses_its_bars_down_to_its_minimum_and_keeps_a_pixel_of_one() {
        let size = PhysicalSize::new;
        let min = size(600, 400);
        // Bars beside the desktop, or above and below it.
        assert_eq!(trimmed_size(size(1600, 900), vec2(400.0, 0.0), min), Some(size(1200, 900)));
        assert_eq!(trimmed_size(size(1600, 1000), vec2(0.0, 100.0), min), Some(size(1600, 900)));
        // None, or a pixel's rounding.
        assert_eq!(trimmed_size(size(1600, 900), Vec2::ZERO, min), None);
        assert_eq!(trimmed_size(size(1600, 900), vec2(1.0, 1.0), min), None);
        assert_eq!(trimmed_size(size(1600, 900), vec2(2.0, 1.0), min), Some(size(1598, 900)));
        // No smaller than the window may be, and once it is that small,
        // nothing more to do.
        assert_eq!(trimmed_size(size(1000, 1000), vec2(700.0, 0.0), min), Some(size(600, 1000)));
        assert_eq!(trimmed_size(size(600, 1000), vec2(300.0, 0.0), min), None);
        assert_eq!(trimmed_size(size(601, 1000), vec2(300.0, 0.0), min), None);
    }
}
