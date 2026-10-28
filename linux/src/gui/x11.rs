// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! X11's stand-in for the keyboard shortcuts inhibitor: an active keyboard
//! grab on the window while it has focus and a session takes input
//! (connected, not view only, no prompt up) and the pointer is not on the
//! toolbar, so the window manager's chords (Alt+Tab, Super+…) reach the far
//! end.
//!
//! It is sent over winit's own connection (the Xlib display from the raw
//! handle, as XCB) because X reports a grabbed key only to the client that
//! grabbed it: a connection of our own would take every key away from
//! winit. Xlib keeps owning the event queue; this only sends requests and
//! waits for their replies, never reads events, which libxcb is safe for.
//! winit itself wraps the same display the same way. With
//! `owner_events` on, winit keeps receiving every key on its window as an
//! ordinary `KeyboardInput`.
//!
//! The grab can be refused (the window manager's Alt+Tab switcher or a
//! menu holds the keyboard), and is then asked for again every
//! [`RETRY`] while it is still wanted; the overlay says why meanwhile.
//! The way out of a grabbed window is the mouse: a click elsewhere has the
//! window manager move the focus, and the focus-out ends the grab.
//!
//! Also here: a focus-out the grab itself caused, told apart from a real
//! one. A grab and an ungrab make the server send FocusOut and FocusIn with
//! the Grab and Ungrab modes, which winit reports as plain focus changes;
//! where the focus is PointerRoot (no window manager) that becomes a loop of
//! ungrab and grab, thousands a second. [`Keyboard::focus_still_ours`] asks
//! the server where the focus really is.
//!
//! And relative mode's [`Fence`]: four XFixes pointer barriers on the
//! window's edges, so that a captured pointer cannot leave the window
//! between a move and the warp back to the centre that undoes it.
//!
//! And whether the window is viewable at all ([`Viewable`]): a window
//! manager that unmaps a window's frame to put its workspace away (i3's
//! workspaces not on any output and its scratchpad, Openbox's other
//! desktops) tells the window nothing: the unmap is the frame's, X sends
//! no visibility event for a window that stops being viewable, and i3 sets
//! `_NET_WM_STATE_HIDDEN` only on a tab that is not the active one. The
//! server knows, though, and says so to a `GetWindowAttributes`, which
//! [`Viewability`] asks for once a second while frames run.
//!
//! And what the window manager offers for the title bar the window draws
//! itself ([`WindowManager`]): which of minimise and maximise it does, and
//! the window menu, which a window manager that takes
//! `_GTK_SHOW_WINDOW_MENU` shows on request.

use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use std::time::{Duration, Instant};
use winit::dpi::PhysicalSize;
use winit::window::{Window, WindowButtons};
use x11rb::connection::Connection;
use x11rb::protocol::xfixes::{BarrierDirections, ConnectionExt as _};
use x11rb::protocol::xproto::{
    AtomEnum, ClientMessageEvent, ConnectionExt as _, EventMask, GrabMode, GrabStatus, InputFocus, MapState,
};
use x11rb::xcb_ffi::XCBConnection;

/// How long a refused grab waits before it is asked for again.
pub const RETRY: Duration = Duration::from_millis(250);

/// What became of the last grab asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Last {
    Released,
    Grabbed,
    Refused(GrabStatus),
    Failed,
}

pub struct Keyboard {
    conn: XCBConnection,
    window: u32,
    held: bool,
    wanted: bool,
    last: Last,
    retry_at: Option<Instant>,
}

/// winit's own connection and the window's id, or `None` on anything but
/// an X11 window. `what` is for the log, should the connection be refused.
fn winit_connection(window: &Window, what: &str) -> Option<(XCBConnection, u32)> {
    let raw_window = window_id(window)?;
    let ptr = match window.display_handle().ok()?.as_raw() {
        RawDisplayHandle::Xlib(h) => {
            let display = h.display?;
            let xlib_xcb = x11_dl::xlib_xcb::Xlib_xcb::open().ok()?;
            // SAFETY: the display is winit's process-wide connection,
            // live for as long as the window is.
            unsafe { (xlib_xcb.XGetXCBConnection)(display.as_ptr().cast()) }.cast()
        }
        RawDisplayHandle::Xcb(h) => h.connection?.as_ptr(),
        _ => return None,
    };
    if ptr.is_null() {
        return None;
    }
    // SAFETY: winit's connection, which is never closed while the window
    // lives; `should_drop` false, so dropping this wrapper leaves it be.
    match unsafe { XCBConnection::from_raw_xcb_connection(ptr, false) } {
        Ok(conn) => Some((conn, raw_window)),
        Err(e) => {
            log::warn!("X11: cannot use winit's connection for {what}: {e}");
            None
        }
    }
}

/// The X window behind a winit window, or `None` on anything but X11.
fn window_id(window: &Window) -> Option<u32> {
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Xlib(w) => u32::try_from(w.window).ok(),
        RawWindowHandle::Xcb(w) => Some(w.window.get()),
        _ => None,
    }
}

impl Keyboard {
    /// `None` on anything but an X11 window.
    pub fn new(window: &Window) -> Option<Keyboard> {
        let (conn, raw_window) = winit_connection(window, "the keyboard grab")?;
        let xwayland = conn
            .query_extension(b"XWAYLAND")
            .ok()
            .and_then(|c| c.reply().ok())
            .is_some_and(|r| r.present);
        if xwayland {
            log::info!(
                "X11: this X server is Xwayland; its compositor decides whether a keyboard grab holds \
                 off the compositor's own shortcuts — the Wayland backend is the better choice here"
            );
        }
        Some(Keyboard {
            conn,
            window: raw_window,
            held: false,
            wanted: false,
            last: Last::Released,
            retry_at: None,
        })
    }

    /// Grab the keyboard, from now until `release`; a refusal is retried.
    pub fn inhibit(&mut self) {
        self.wanted = true;
        if !self.held && self.retry_at.is_none_or(|t| t <= Instant::now()) {
            self.grab();
        }
    }

    /// The grab is to be on `window` from now on, one of this client's: a
    /// client holds one keyboard grab whichever of its windows it names,
    /// and a second GrabKeyboard moves it rather than stacking, so one
    /// held elsewhere is moved at once and a wanted one is asked for there.
    pub fn retarget(&mut self, window: &Window) {
        let Some(raw_window) = window_id(window) else {
            return;
        };
        if raw_window != self.window {
            self.window = raw_window;
            if self.held {
                self.held = false;
                self.grab();
            }
        }
    }

    /// A refused grab, asked for again once it is due.
    pub fn pump(&mut self) {
        if self.wanted && !self.held && self.retry_at.is_some_and(|t| t <= Instant::now()) {
            self.grab();
        }
    }

    fn grab(&mut self) {
        let reply = self
            .conn
            .grab_keyboard(true, self.window, x11rb::CURRENT_TIME, GrabMode::ASYNC, GrabMode::ASYNC)
            .map_err(|e| e.to_string())
            .and_then(|c| c.reply().map_err(|e| e.to_string()));
        match reply {
            Ok(r) if r.status == GrabStatus::SUCCESS => {
                self.held = true;
                self.retry_at = None;
                self.last = Last::Grabbed;
                log::info!("X11 keyboard grabbed");
            }
            Ok(r) => {
                self.retry_at = Some(Instant::now() + RETRY);
                if self.last != Last::Refused(r.status) {
                    log::info!("X11 keyboard grab refused: {}; retrying", refusal(r.status));
                }
                self.last = Last::Refused(r.status);
            }
            Err(e) => {
                self.retry_at = Some(Instant::now() + RETRY);
                if self.last != Last::Failed {
                    log::warn!("X11 keyboard grab failed: {e}; retrying");
                }
                self.last = Last::Failed;
            }
        }
    }

    pub fn release(&mut self) {
        self.wanted = false;
        self.retry_at = None;
        if self.held {
            if let Ok(cookie) = self.conn.ungrab_keyboard(x11rb::CURRENT_TIME) {
                cookie.ignore_error();
            }
            let _ = self.conn.flush();
            self.held = false;
            log::info!("X11 keyboard released");
        }
        self.last = Last::Released;
    }

    /// Where the grab is, in a few words, for the debug overlay.
    pub fn state(&self) -> &'static str {
        if self.held {
            return "grabbed";
        }
        if !self.wanted {
            return "released";
        }
        match self.last {
            Last::Refused(s) if s == GrabStatus::ALREADY_GRABBED => "refused (already grabbed)",
            Last::Refused(s) if s == GrabStatus::FROZEN => "refused (frozen)",
            Last::Refused(s) if s == GrabStatus::NOT_VIEWABLE => "refused (not viewable)",
            Last::Refused(s) if s == GrabStatus::INVALID_TIME => "refused (invalid time)",
            Last::Refused(_) => "refused",
            Last::Failed => "failed",
            Last::Released | Last::Grabbed => "waiting",
        }
    }

    /// When a refused grab is due to be asked for again.
    pub fn retry_at(&self) -> Option<Instant> {
        self.retry_at.filter(|_| self.wanted && !self.held)
    }

    /// Where the grab is as `window`'s overlay should say it: the grab is
    /// the client's, so a window it is not on says so rather than taking
    /// another window's "grabbed" for its own.
    pub fn state_in(&self, window: &Window) -> &'static str {
        if self.held && window_id(window) != Some(self.window) {
            return "on another window";
        }
        self.state()
    }

    /// Whether the X focus is still one of `ours` although winit said
    /// otherwise: the focus is one of our windows, or PointerRoot with the
    /// pointer inside one. Any of them, and not only the one the grab is
    /// on: a focus-out from one window of ours to another is a move, which
    /// the window resolves as one (see `resolve_focus`), and not the
    /// process losing the focus. Two round trips, and only on a focus-out
    /// while the grab is held.
    pub fn focus_still_ours(&self, ours: &[&Window]) -> bool {
        if !self.held {
            return false;
        }
        let Some(focus) = self.conn.get_input_focus().ok().and_then(|c| c.reply().ok()) else {
            return false;
        };
        let ids: Vec<(u32, PhysicalSize<u32>)> =
            ours.iter().filter_map(|w| Some((window_id(w)?, w.inner_size()))).collect();
        if ids.iter().any(|&(id, _)| id == focus.focus) {
            return true;
        }
        if focus.focus == u32::from(InputFocus::POINTER_ROOT) {
            return ids.iter().any(|&(id, size)| {
                self.conn.query_pointer(id).ok().and_then(|c| c.reply().ok()).is_some_and(|p| {
                    p.same_screen
                        && p.win_x >= 0
                        && p.win_y >= 0
                        && (p.win_x as u32) < size.width
                        && (p.win_y as u32) < size.height
                })
            });
        }
        false
    }
}

impl Drop for Keyboard {
    fn drop(&mut self) {
        self.release();
    }
}

/// Relative mode's fence: XFixes pointer barriers on the window's four
/// edges while the pointer is captured.
///
/// The capture warps the pointer back to the centre each time it moves,
/// but only once the event loop has the move, and until then the pointer
/// is wherever the move took it. A fast one goes past the edge, and a click
/// in that moment went to the window under it: `xdotool mousemove_relative
/// -- 700 0 click 1` activated the xterm beside the window and ended the
/// capture every time. A barrier stops the pointer at the edge, inside, so
/// the click is the window's.
///
/// Barriers rather than the active pointer grab that `confine_to` would be,
/// because with a grab the X server delivers each XI2 raw motion twice, once
/// for the grab and once for the root window's selection, and relative mode
/// is made of raw motion. A barrier changes nothing of what is delivered:
/// raw motion is the device's, measured before the barrier clamps the
/// pointer, so a move into the edge still reaches the far end at its full
/// length. They only hold relative motion: a warp passes, which is what the
/// centring and the restore are, and so does an absolute device, which
/// relative mode cannot use anyway.
///
/// Made over winit's connection like the keyboard grab, so they are gone
/// with the process whatever becomes of it. XFixes 5 is needed, asked for on
/// that connection (winit itself uses no XFixes); without it the capture
/// works unfenced.
pub struct Fence {
    conn: XCBConnection,
    window: u32,
    barriers: Vec<u32>,
    /// The rectangle the barriers are on, in root coordinates.
    around: Option<(i32, i32, u32, u32)>,
}

impl Fence {
    /// `None` on anything but an X11 window, or an X server without
    /// XFixes 5.
    pub fn new(window: &Window) -> Option<Fence> {
        let (conn, raw_window) = winit_connection(window, "the pointer barriers")?;
        let version = conn.xfixes_query_version(5, 0).ok().and_then(|c| c.reply().ok());
        match version {
            Some(v) if v.major_version >= 5 => {}
            Some(v) => {
                log::warn!(
                    "X11: XFixes {}.{} has no pointer barriers; a captured pointer can leave \
                     the window on a fast move",
                    v.major_version,
                    v.minor_version
                );
                return None;
            }
            None => {
                log::warn!("X11: no XFixes; a captured pointer can leave the window on a fast move");
                return None;
            }
        }
        Some(Fence { conn, window: raw_window, barriers: Vec::new(), around: None })
    }

    /// The barriers are to be on `window` from now on, one of this
    /// client's: the fence is the session's, one for every window, and
    /// goes with the capture to whichever window holds it. Lowered from
    /// where they were; `raise` puts them up around the new one.
    pub fn retarget(&mut self, window: &Window) {
        if let Some(id) = window_id(window)
            && id != self.window
        {
            self.lower();
            self.window = id;
        }
    }

    /// Barriers on the window's edges as it is now, moved there if they
    /// were somewhere else.
    pub fn raise(&mut self) {
        let Some(geometry) = self.conn.get_geometry(self.window).ok().and_then(|c| c.reply().ok()) else {
            return;
        };
        let Some(at) = self
            .conn
            .translate_coordinates(self.window, geometry.root, 0, 0)
            .ok()
            .and_then(|c| c.reply().ok())
        else {
            return;
        };
        let around = (
            i32::from(at.dst_x),
            i32::from(at.dst_y),
            u32::from(geometry.width),
            u32::from(geometry.height),
        );
        if self.around == Some(around) {
            return;
        }
        self.lower();
        for (x1, y1, x2, y2) in edges(around) {
            let Ok(id) = self.conn.generate_id() else {
                break;
            };
            // No direction let through, and every master pointer.
            let made = self
                .conn
                .xfixes_create_pointer_barrier(id, geometry.root, x1, y1, x2, y2, BarrierDirections::from(0u8), &[])
                .map(|c| c.check());
            match made {
                Ok(Ok(())) => self.barriers.push(id),
                Ok(Err(e)) => log::warn!("X11: pointer barrier refused: {e:?}"),
                Err(e) => log::warn!("X11: pointer barrier failed: {e}"),
            }
        }
        self.around = Some(around);
        let (x, y, w, h) = around;
        log::debug!("X11: pointer fenced to {w}x{h} at {x},{y} ({} barriers)", self.barriers.len());
    }

    /// No barriers.
    pub fn lower(&mut self) {
        for id in self.barriers.drain(..) {
            if let Ok(cookie) = self.conn.xfixes_delete_pointer_barrier(id) {
                cookie.ignore_error();
            }
        }
        if self.around.take().is_some() {
            let _ = self.conn.flush();
        }
    }
}

/// The four barriers around a rectangle in root coordinates, as x1, y1, x2,
/// y2: left, right, top, bottom. A vertical barrier stops a pointer moving
/// right at its x - 1 and one moving left at its x, a horizontal one
/// likewise, so on the edges themselves they keep the pointer on the
/// window's own pixels. Coordinates are unsigned: an edge off the screen's
/// top or left is held at 0, which is still inside the window. One that
/// would be a point (a window of no width) is left out, as the server
/// refuses it.
fn edges((x, y, w, h): (i32, i32, u32, u32)) -> Vec<(u16, u16, u16, u16)> {
    let clamp = |v: i64| v.clamp(0, i64::from(u16::MAX)) as u16;
    let (left, top) = (clamp(x.into()), clamp(y.into()));
    let (right, bottom) = (clamp(i64::from(x) + i64::from(w)), clamp(i64::from(y) + i64::from(h)));
    [
        (left, top, left, bottom),
        (right, top, right, bottom),
        (left, top, right, top),
        (left, bottom, right, bottom),
    ]
    .into_iter()
    .filter(|&(x1, y1, x2, y2)| x1 != x2 || y1 != y2)
    .collect()
}

impl Drop for Fence {
    fn drop(&mut self) {
        self.lower();
    }
}

/// How often the server is asked whether the window is viewable, while
/// frames run: a round trip, which once a second is nothing (once a frame
/// is what `patches/winit/0005` took out of `is_minimized`).
pub const ASK: Duration = Duration::from_secs(1);

/// Asks the X server whether the window is viewable: mapped, and every
/// window above it in the tree too. The window manager's frame unmapped (a
/// workspace put away, i3's scratchpad, Openbox's other desktops) is what
/// makes it not. Over winit's connection like the keyboard grab.
pub struct Viewable {
    conn: XCBConnection,
    window: u32,
}

impl Viewable {
    /// `None` on anything but an X11 window.
    pub fn new(window: &Window) -> Option<Viewable> {
        let (conn, raw_window) = winit_connection(window, "the map state")?;
        Some(Viewable { conn, window: raw_window })
    }

    /// Whether the window is viewable now; `None` when the server would not
    /// say (the window gone).
    pub fn ask(&self) -> Option<bool> {
        let reply = self.conn.get_window_attributes(self.window).ok()?.reply().ok()?;
        Some(reply.map_state == MapState::VIEWABLE)
    }
}

/// What the window's viewability is taken to be: the last answer, and when
/// the next one is due. A visibility event is the server's word that the
/// window is viewable (it sends none for one that is not), so one ends an
/// "unviewable" at once, without waiting for the next ask: a workspace
/// brought back is live the moment its frame is mapped.
#[derive(Debug, Default)]
pub struct Viewability {
    /// The last answer was that the window is not viewable.
    unviewable: bool,
    asked: Option<Instant>,
}

impl Viewability {
    /// Whether the window is not viewable, as far as is known.
    pub fn hidden(&self) -> bool {
        self.unviewable
    }

    /// Whether the server should be asked again now.
    pub fn due(&self, now: Instant) -> bool {
        self.asked.is_none_or(|at| now.saturating_duration_since(at) >= ASK)
    }

    /// The server's answer to an ask made at `now`; `None` leaves what was
    /// known (a window the server does not know is being closed). Whether
    /// the answer changed anything.
    pub fn answered(&mut self, viewable: Option<bool>, now: Instant) -> bool {
        self.asked = Some(now);
        let was = self.unviewable;
        if let Some(viewable) = viewable {
            self.unviewable = !viewable;
        }
        self.unviewable != was
    }

    /// A visibility event came for the window: it is viewable.
    pub fn seen(&mut self) {
        self.unviewable = false;
    }
}

/// The atoms [`WindowManager`] reads and sends.
#[derive(Clone, Copy, Debug, Default)]
struct Atoms {
    supported: u32,
    allowed: u32,
    minimize: u32,
    maximize_horz: u32,
    maximize_vert: u32,
    maximized_horz: u32,
    maximized_vert: u32,
    show_window_menu: u32,
    moveresize: u32,
}

/// What the window manager offers to do with the window: minimise and
/// maximise, for the title bar's buttons, and the window menu. Read over
/// winit's connection like the keyboard grab, and again at most once every
/// [`ASK`] while frames run, as the window manager may change them (for a
/// fullscreen window, say) and sends nothing this end hears of.
pub struct WindowManager {
    conn: XCBConnection,
    window: u32,
    root: u32,
    atoms: Atoms,
    /// The screen's `WM_S<n>` selection, which a window manager owns.
    wm_selection: u32,
    buttons: WindowButtons,
    menu: bool,
    asked: Option<Instant>,
}

impl WindowManager {
    /// `None` on anything but an X11 window.
    pub fn new(window: &Window) -> Option<WindowManager> {
        let (conn, raw_window) = winit_connection(window, "the window manager's buttons")?;
        let root = conn.get_geometry(raw_window).ok()?.reply().ok()?.root;
        let screen = conn.setup().roots.iter().position(|s| s.root == root).unwrap_or(0);
        let intern = |name: &str| -> Option<u32> { Some(conn.intern_atom(false, name.as_bytes()).ok()?.reply().ok()?.atom) };
        let atoms = Atoms {
            supported: intern("_NET_SUPPORTED")?,
            allowed: intern("_NET_WM_ALLOWED_ACTIONS")?,
            minimize: intern("_NET_WM_ACTION_MINIMIZE")?,
            maximize_horz: intern("_NET_WM_ACTION_MAXIMIZE_HORZ")?,
            maximize_vert: intern("_NET_WM_ACTION_MAXIMIZE_VERT")?,
            maximized_horz: intern("_NET_WM_STATE_MAXIMIZED_HORZ")?,
            maximized_vert: intern("_NET_WM_STATE_MAXIMIZED_VERT")?,
            show_window_menu: intern("_GTK_SHOW_WINDOW_MENU")?,
            moveresize: intern("_NET_WM_MOVERESIZE")?,
        };
        let wm_selection = intern(&format!("WM_S{screen}"))?;
        Some(WindowManager {
            conn,
            window: raw_window,
            root,
            atoms,
            wm_selection,
            buttons: WindowButtons::CLOSE,
            menu: false,
            asked: None,
        })
    }

    /// The buttons the window manager offers, as last read; read again
    /// first where due.
    pub fn buttons(&mut self) -> WindowButtons {
        let now = Instant::now();
        if self.asked.is_none_or(|at| now.saturating_duration_since(at) >= ASK) {
            self.asked = Some(now);
            self.read();
        }
        self.buttons
    }

    /// Has the next [`buttons`](Self::buttons) read them again: the window's
    /// state may have changed.
    pub fn stale(&mut self) {
        self.asked = None;
    }

    fn read(&mut self) {
        let atoms = |window: u32, property: u32| -> Option<Vec<u32>> {
            let reply = self
                .conn
                .get_property(false, window, property, AtomEnum::ATOM, 0, 1024)
                .ok()?
                .reply()
                .ok()?;
            Some(reply.value32()?.collect())
        };
        let supported = atoms(self.root, self.atoms.supported).unwrap_or_default();
        let allowed = atoms(self.window, self.atoms.allowed).filter(|a| !a.is_empty());
        let managed = !supported.is_empty()
            || self
                .conn
                .get_selection_owner(self.wm_selection)
                .ok()
                .and_then(|c| c.reply().ok())
                .is_some_and(|r| r.owner != x11rb::NONE);
        let buttons = offered(&self.atoms, &supported, allowed.as_deref(), managed);
        let menu = supported.contains(&self.atoms.show_window_menu);
        if menu != self.menu {
            log::debug!("X11: window menu (_GTK_SHOW_WINDOW_MENU): {}", if menu { "yes" } else { "no" });
        }
        self.buttons = buttons;
        self.menu = menu;
    }

    /// Asks the window manager for its window menu at the pointer, where it
    /// takes `_GTK_SHOW_WINDOW_MENU`, as GTK does: the pointer's implicit
    /// grab let go of, so that the window manager can take it. Whether it
    /// was asked.
    pub fn show_menu(&self) -> bool {
        if !self.menu {
            return false;
        }
        let Some(pointer) = self.conn.query_pointer(self.window).ok().and_then(|c| c.reply().ok()) else {
            return false;
        };
        // The device is XI2's virtual core pointer, as winit's own moves
        // and resizes name it.
        let data = [2, pointer.root_x as u32, pointer.root_y as u32, 0, 0];
        let event = ClientMessageEvent::new(32, self.window, self.atoms.show_window_menu, data);
        let sent = self.conn.ungrab_pointer(x11rb::CURRENT_TIME).is_ok()
            && self
                .conn
                .send_event(
                    false,
                    self.root,
                    EventMask::SUBSTRUCTURE_NOTIFY | EventMask::SUBSTRUCTURE_REDIRECT,
                    event,
                )
                .is_ok()
            && self.conn.flush().is_ok();
        if !sent {
            log::debug!("X11: cannot ask for the window menu");
        }
        sent
    }
}

impl WindowManager {
    /// Where the pointer is on the screen, in device pixels.
    pub fn pointer(&self) -> Option<(i32, i32)> {
        let reply = self.conn.query_pointer(self.window).ok()?.reply().ok()?;
        Some((reply.root_x.into(), reply.root_y.into()))
    }

    /// Asks the window manager to resize the window from its top left
    /// (`left`) or top right corner while `button` is held, as winit's
    /// `drag_resize_window` does for the first button only: the window
    /// manager ends the resize when the button its message names is let go
    /// of. Whether it was asked.
    pub fn resize_from_top(&self, left: bool, button: u32) -> bool {
        let Some(pointer) = self.conn.query_pointer(self.window).ok().and_then(|c| c.reply().ok()) else {
            return false;
        };
        // _NET_WM_MOVERESIZE_SIZE_TOPLEFT and _SIZE_TOPRIGHT; the source is
        // an application.
        let direction = if left { 0 } else { 2 };
        let data = [pointer.root_x as u32, pointer.root_y as u32, direction, button, 1];
        let event = ClientMessageEvent::new(32, self.window, self.atoms.moveresize, data);
        self.conn.ungrab_pointer(x11rb::CURRENT_TIME).is_ok()
            && self
                .conn
                .send_event(
                    false,
                    self.root,
                    EventMask::SUBSTRUCTURE_NOTIFY | EventMask::SUBSTRUCTURE_REDIRECT,
                    event,
                )
                .is_ok()
            && self.conn.flush().is_ok()
    }
}

/// The buttons a window manager offers, from what it says it supports
/// (`_NET_SUPPORTED`) and what it allows the window (`allowed`, its
/// `_NET_WM_ALLOWED_ACTIONS`, where it has set any): close always; minimise
/// and maximise as allowed. Where the window manager does not say,
/// maximise where it supports both of EWMH's maximised states, and
/// minimise only from a window manager without EWMH (ICCCM's iconify): one
/// with EWMH but without allowed actions is a tiling one such as i3, which
/// does not iconify. Without a window manager (`managed` false), close
/// alone.
fn offered(atoms: &Atoms, supported: &[u32], allowed: Option<&[u32]>, managed: bool) -> WindowButtons {
    let mut buttons = WindowButtons::CLOSE;
    if !managed {
        return buttons;
    }
    let (minimize, maximize) = match allowed {
        Some(allowed) if supported.contains(&atoms.allowed) => (
            allowed.contains(&atoms.minimize),
            allowed.contains(&atoms.maximize_horz) && allowed.contains(&atoms.maximize_vert),
        ),
        _ => (
            supported.is_empty() || supported.contains(&atoms.allowed),
            supported.contains(&atoms.maximized_horz) && supported.contains(&atoms.maximized_vert),
        ),
    };
    buttons.set(WindowButtons::MINIMIZE, minimize);
    buttons.set(WindowButtons::MAXIMIZE, maximize);
    buttons
}

fn refusal(status: GrabStatus) -> &'static str {
    match status {
        GrabStatus::ALREADY_GRABBED => "already grabbed",
        GrabStatus::FROZEN => "frozen",
        GrabStatus::NOT_VIEWABLE => "not viewable",
        GrabStatus::INVALID_TIME => "invalid time",
        _ => "unknown status",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fence_is_on_the_window_s_edges() {
        assert_eq!(
            edges((10, 107, 1280, 800)),
            vec![(10, 107, 10, 907), (1290, 107, 1290, 907), (10, 107, 1290, 107), (10, 907, 1290, 907)]
        );
        // Off the screen's top left: held at 0, which is still the window.
        assert_eq!(
            edges((-100, -20, 300, 200)),
            vec![(0, 0, 0, 180), (200, 0, 200, 180), (0, 0, 200, 0), (0, 180, 200, 180)]
        );
        // Wholly off it: nothing that is a point.
        assert!(edges((-500, -500, 100, 100)).is_empty());
    }

    #[test]
    fn the_window_manager_s_buttons_are_what_it_allows_or_failing_that_supports() {
        let atoms = Atoms {
            supported: 1,
            allowed: 2,
            minimize: 3,
            maximize_horz: 4,
            maximize_vert: 5,
            maximized_horz: 6,
            maximized_vert: 7,
            show_window_menu: 8,
            moveresize: 9,
        };
        let all = WindowButtons::all();
        let close = WindowButtons::CLOSE;
        // Openbox: allowed actions supported and set on the window.
        let openbox = [2, 6, 7];
        assert_eq!(offered(&atoms, &openbox, Some(&[3, 4, 5]), true), all);
        assert_eq!(offered(&atoms, &openbox, Some(&[3]), true), close | WindowButtons::MINIMIZE);
        // One of the two maximised states is not maximising.
        assert_eq!(offered(&atoms, &openbox, Some(&[4]), true), close);
        // Not set on the window yet: what the window manager supports.
        assert_eq!(offered(&atoms, &openbox, None, true), all);
        // A window manager without allowed actions: its states, and no
        // minimise (i3).
        assert_eq!(offered(&atoms, &[6, 7], Some(&[3]), true), close | WindowButtons::MAXIMIZE);
        assert_eq!(offered(&atoms, &[6], None, true), close);
        // An ICCCM window manager, with no EWMH at all: minimise.
        assert_eq!(offered(&atoms, &[], None, true), close | WindowButtons::MINIMIZE);
        // No window manager: close alone.
        assert_eq!(offered(&atoms, &[], None, false), close);
    }

    #[test]
    fn the_window_is_viewable_until_the_server_says_not_and_seen_ends_that() {
        let start = Instant::now();
        let mut v = Viewability::default();
        // Nothing known yet: taken as viewable, and asked at once.
        assert!(!v.hidden());
        assert!(v.due(start));
        assert!(!v.answered(Some(true), start));
        assert!(!v.hidden());
        // Once a second, not every frame.
        assert!(!v.due(start + ASK / 2));
        assert!(v.due(start + ASK));
        // The frame was unmapped: hidden until the server says otherwise.
        assert!(v.answered(Some(false), start + ASK));
        assert!(v.hidden());
        // The server would not say (the window is going): what was known stays.
        assert!(!v.answered(None, start + 2 * ASK));
        assert!(v.hidden());
        // A visibility event is the server's word that it is viewable again.
        v.seen();
        assert!(!v.hidden());
    }
}
