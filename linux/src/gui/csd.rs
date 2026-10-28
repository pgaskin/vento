// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The window's own decorations, drawn in place of the window manager's or
//! the compositor's unless `--no-csd` is given: a title bar above the
//! toolbar, the window's buttons, and the edges a window is resized by.
//!
//! The title bar is as wide as the toolbar and moves with it, over the
//! desktop rather than beside it, so that the toolbar's drag still moves
//! both out of the way of what is under them; it is drawn in a window and
//! while maximised, and not in fullscreen. A drag on it moves the window, a
//! double click maximises or restores it, a drag with the right button
//! resizes it from the top corner on that side of the bar's middle, and a
//! right click opens the window manager's or the compositor's window menu
//! where there is one. Its
//! buttons are close, and minimise and maximise where the window manager
//! or the compositor offers them (see `Host::buttons`).
//!
//! The edges resize the window from [`CORNER`] at each corner and [`EDGE`]
//! along each side, in a window that is not maximised or fullscreen. They
//! are on a layer of their own above everything else, so that a press on
//! them is never the desktop's and never reaches the server. On X11 a side
//! is resized here rather than by the window manager ([`EdgeResize`]): i3
//! takes `_NET_WM_MOVERESIZE` for a side as one for the corner nearest the
//! pointer.
//!
//! A move, a resize and a window menu are the window manager's or the
//! compositor's once asked for: it takes the pointer, and the release that
//! ends them goes to it rather than to the window. egui is given that
//! release itself (see [`let_go`]), or it would take the button to be held
//! still, and every widget under the pointer after it as being dragged
//! over.

use super::Host;
use egui::{
    Align2, Color32, CursorIcon, FontId, Id, Order, PointerButton, Pos2, Rect, Response, Sense, Shape, Stroke, Ui,
    pos2, vec2,
};
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::window::{ResizeDirection, WindowButtons};

/// The title bar's height, in points: about three fifths of the toolbar's,
/// and the side of each of its square buttons.
pub const TITLE_BAR: f32 = 25.0;

/// How far from each corner of a window, along both edges, the pointer
/// resizes it from that corner, in points.
pub const CORNER: f32 = 4.0;

/// How far in from each edge of a window, between the corners, the pointer
/// resizes it from that edge, in points.
pub const EDGE: f32 = 2.0;

/// The title text's size, in points.
const TITLE_TEXT: f32 = 11.0;

/// The title text's colour.
const TITLE_COLOR: Color32 = Color32::from_gray(0xb8);

/// A close button's fill under the pointer.
const CLOSE_HOVER: Color32 = Color32::from_rgb(0xc4, 0x2b, 0x1c);

/// The window's buttons, as drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Minimize,
    Maximize,
    Restore,
    Close,
}

/// The eight places a window is resized from, inside `window`: a square of
/// `corner` at each corner, and a strip `edge` deep along each side between
/// them. Corners first, so that where a strip and a corner would meet the
/// corner is found first.
pub fn zones(window: Rect, corner: f32, edge: f32) -> [(Rect, ResizeDirection); 8] {
    let (l, t, r, b) = (window.min.x, window.min.y, window.max.x, window.max.y);
    let rect = |x0: f32, y0: f32, x1: f32, y1: f32| Rect::from_min_max(pos2(x0, y0), pos2(x1, y1));
    [
        (rect(l, t, l + corner, t + corner), ResizeDirection::NorthWest),
        (rect(r - corner, t, r, t + corner), ResizeDirection::NorthEast),
        (rect(l, b - corner, l + corner, b), ResizeDirection::SouthWest),
        (rect(r - corner, b - corner, r, b), ResizeDirection::SouthEast),
        (rect(l + corner, t, r - corner, t + edge), ResizeDirection::North),
        (rect(l + corner, b - edge, r - corner, b), ResizeDirection::South),
        (rect(l, t + corner, l + edge, b - corner), ResizeDirection::West),
        (rect(r - edge, t + corner, r, b - corner), ResizeDirection::East),
    ]
}

/// The zone of [`zones`] a pointer at `p` is on, if any. Half-open, so that
/// an edge of two points is two points and not three, and a pointer on the
/// line between a corner and an edge is on one of them only.
pub fn zone_at(window: Rect, corner: f32, edge: f32, p: Pos2) -> Option<(Rect, ResizeDirection)> {
    let within = |r: Rect| (r.min.x..r.max.x).contains(&p.x) && (r.min.y..r.max.y).contains(&p.y);
    zones(window, corner, edge).into_iter().find(|&(r, _)| within(r))
}

/// The cursor for resizing `direction`.
pub fn cursor(direction: ResizeDirection) -> CursorIcon {
    match direction {
        ResizeDirection::North => CursorIcon::ResizeNorth,
        ResizeDirection::South => CursorIcon::ResizeSouth,
        ResizeDirection::East => CursorIcon::ResizeEast,
        ResizeDirection::West => CursorIcon::ResizeWest,
        ResizeDirection::NorthEast => CursorIcon::ResizeNorthEast,
        ResizeDirection::NorthWest => CursorIcon::ResizeNorthWest,
        ResizeDirection::SouthEast => CursorIcon::ResizeSouthEast,
        ResizeDirection::SouthWest => CursorIcon::ResizeSouthWest,
    }
}

/// The title bar of a toolbar `width` wide whose left edge is at `left`:
/// above it, at the top of `window`.
pub fn title_rect(window: Rect, left: f32, width: f32) -> Rect {
    Rect::from_min_size(pos2(left, window.min.y), vec2(width, TITLE_BAR))
}

/// The buttons a title bar shows, left to right, for what the window
/// manager offers (`buttons`) and whether the window is `maximized`: close
/// always, minimise and maximise or restore where offered.
pub fn title_buttons(buttons: WindowButtons, maximized: bool) -> Vec<Button> {
    let mut shown = Vec::new();
    if buttons.contains(WindowButtons::MINIMIZE) {
        shown.push(Button::Minimize);
    }
    if buttons.contains(WindowButtons::MAXIMIZE) {
        shown.push(if maximized { Button::Restore } else { Button::Maximize });
    }
    shown.push(Button::Close);
    shown
}

/// A resize from one side of the window done here, on X11: the pointer is
/// the window's while the button pressed on the side is held (the server's
/// implicit grab), and the window is made the size and the place it asks
/// for, frame by frame. All in device pixels, on the screen.
#[derive(Clone, Copy, Debug, PartialEq)]
struct EdgeResize {
    direction: ResizeDirection,
    /// Where the pointer was pressed.
    from: (i32, i32),
    /// The window's position and inner size then.
    at: (i32, i32),
    size: PhysicalSize<u32>,
}

impl EdgeResize {
    /// The window's position and size with the pointer at `pointer`: the
    /// side moved with it, the opposite side where it was, and the size no
    /// smaller than `min`.
    fn geometry(&self, pointer: (i32, i32), min: PhysicalSize<u32>) -> ((i32, i32), PhysicalSize<u32>) {
        let (dx, dy) = (pointer.0 - self.from.0, pointer.1 - self.from.1);
        let (mut x, mut y) = self.at;
        let (mut w, mut h) = (self.size.width as i32, self.size.height as i32);
        let (min_w, min_h) = (min.width as i32, min.height as i32);
        match self.direction {
            ResizeDirection::East => w = (w + dx).max(min_w),
            ResizeDirection::South => h = (h + dy).max(min_h),
            ResizeDirection::West => {
                let right = x + w;
                w = (w - dx).max(min_w);
                x = right - w;
            }
            ResizeDirection::North => {
                let bottom = y + h;
                h = (h - dy).max(min_h);
                y = bottom - h;
            }
            _ => {}
        }
        ((x, y), PhysicalSize::new(w as u32, h as u32))
    }
}

/// Whether `direction` is one side rather than a corner.
fn side(direction: ResizeDirection) -> bool {
    matches!(
        direction,
        ResizeDirection::North | ResizeDirection::South | ResizeDirection::East | ResizeDirection::West
    )
}

/// Where an [`EdgeResize`] under way is kept, in egui's data.
fn edge_resize_id() -> Id {
    Id::new("edge-resize")
}

/// Whether the window is resized from its edges: drawn with its own
/// decorations, resizable, and neither maximised nor fullscreen.
pub fn resizable(host: &Host) -> bool {
    host.csd && host.window.is_resizable() && !host.window.is_maximized() && host.window.fullscreen().is_none()
}

/// The resize edges of a window drawn with its own decorations (see
/// [`zones`]), where it is [`resizable`]: the resize cursors over them, and
/// a press on one hands the resize to the window manager or the compositor.
/// Whether the pointer is on one.
pub fn resize_edges(ctx: &egui::Context, host: &mut Host, corner: f32, edge: f32) -> bool {
    if !resizable(host) {
        return false;
    }
    let screen = ctx.content_rect();
    if let Some(resize) = ctx.data(|d| d.get_temp::<EdgeResize>(edge_resize_id())) {
        if ctx.input(|i| i.pointer.primary_down()) {
            // The whole window is the resize's while it lasts.
            egui::Area::new(Id::new("resize-edges"))
                .order(Order::Tooltip)
                .fixed_pos(screen.min)
                .constrain(false)
                .interactable(true)
                .show(ctx, |ui| ui.interact(screen, Id::new("resize-edge"), Sense::drag()));
            ctx.set_cursor_icon(cursor(resize.direction));
            if let Some(pointer) = host.wm.and_then(|wm| wm.pointer()) {
                let min = super::min_size(&ctx.global_style()).to_physical(host.window.scale_factor());
                let (at, size) = resize.geometry(pointer, min);
                if host.window.outer_position().ok() != Some(PhysicalPosition::new(at.0, at.1)) {
                    host.window.set_outer_position(PhysicalPosition::new(at.0, at.1));
                }
                if host.window.inner_size() != size {
                    let _ = host.window.request_inner_size(size);
                }
            }
            return true;
        }
        ctx.data_mut(|d| d.remove::<EdgeResize>(edge_resize_id()));
    }
    // Which way is the pointer's position alone, and only the zone it is
    // on has a widget: egui's hit test takes a pointer a few points past a
    // widget as on it, which would make edges of two points as wide as
    // seven, and a pointer beside a corner a corner's.
    let Some((rect, direction)) = ctx.input(|i| i.pointer.latest_pos()).and_then(|p| zone_at(screen, corner, edge, p))
    else {
        return false;
    };
    let mut on = false;
    egui::Area::new(Id::new("resize-edges"))
        // Above the toolbar, the title bar and the panel, which reach the
        // window's edges.
        .order(Order::Tooltip)
        .fixed_pos(screen.min)
        .constrain(false)
        .interactable(true)
        .show(ctx, |ui| {
            let response = ui.interact(rect, Id::new("resize-edge"), Sense::drag());
            if !response.hovered() {
                return;
            }
            on = true;
            ctx.set_cursor_icon(cursor(direction));
            if ctx.input(|i| i.pointer.primary_pressed()) {
                log::debug!("resizing the window from its {direction:?} edge");
                // X11: a side resized here (see `EdgeResize`).
                if side(direction)
                    && let Some(from) = host.wm.and_then(|wm| wm.pointer())
                    && let Ok(at) = host.window.outer_position()
                {
                    let resize = EdgeResize { direction, from, at: (at.x, at.y), size: host.window.inner_size() };
                    ctx.data_mut(|d| d.insert_temp(edge_resize_id(), resize));
                    return;
                }
                match host.window.drag_resize_window(direction) {
                    Ok(()) => let_go(ctx, host, PointerButton::Primary),
                    Err(e) => log::debug!("cannot resize the window: {e}"),
                }
            }
        });
    on
}

/// The title bar at `rect`, with `title` and the buttons the window manager
/// offers, edged along its foot where `foot` (the toolbar wholly hidden
/// behind it). Its response, which is hovered on the bar and on its buttons
/// alike.
pub fn title_bar(ctx: &egui::Context, host: &mut Host, rect: Rect, title: &str, foot: bool) -> Response {
    let style = ctx.global_style();
    let edge = style.visuals.window_stroke();
    let fill = darker(style.visuals.window_fill);
    let maximized = host.window.is_maximized();
    let buttons = title_buttons(host.buttons, maximized);
    let area = egui::Area::new(Id::new("title-bar"))
        .order(Order::Foreground)
        .fixed_pos(rect.min)
        .constrain(false)
        .interactable(true)
        .sense(Sense::click_and_drag())
        .show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(rect.size(), Sense::hover());
            let painter = ui.painter();
            painter.rect_filled(rect, 0, fill);
            // One outline with the toolbar's: edged at its sides, which
            // continue the toolbar's, and not at the top, which is the
            // window's own edge. At its foot only while the toolbar is
            // hidden: out, the toolbar's own edges go on from there, and the
            // darker fill is what tells the two apart.
            let inner = rect.shrink(edge.width / 2.0);
            let (left, right) = (pos2(inner.min.x, rect.min.y), pos2(inner.max.x, rect.min.y));
            if foot {
                painter.add(Shape::line(vec![left, inner.left_bottom(), inner.right_bottom(), right], edge));
            } else {
                painter.line_segment([left, pos2(inner.min.x, rect.max.y)], edge);
                painter.line_segment([right, pos2(inner.max.x, rect.max.y)], edge);
            }
            let side = rect.height();
            let mut x = rect.max.x - side * buttons.len() as f32;
            let text = Rect::from_min_max(pos2(rect.min.x + 8.0, rect.min.y), pos2(x - 4.0, rect.max.y));
            draw_title(ui, text, title);
            for &button in &buttons {
                let at = Rect::from_min_size(pos2(x, rect.min.y), vec2(side, side));
                x += side;
                if window_button(ui, at, button, 0.4).clicked() {
                    press(host, button);
                }
            }
        });
    let response = area.response.clone();
    if response.drag_started_by(PointerButton::Primary) {
        match host.window.drag_window() {
            Ok(()) => let_go(ctx, host, PointerButton::Primary),
            Err(e) => log::debug!("cannot move the window: {e}"),
        }
    }
    if response.double_clicked() && host.buttons.contains(WindowButtons::MAXIMIZE) {
        host.window.set_maximized(!maximized);
    }
    // The right button's drag resizes, from the top corner on the side of
    // the bar's middle it was pressed on; let go of without a drag, it is a
    // click, and the window menu. A press cannot be told from the start of
    // a drag, so the menu waits for the release.
    if response.drag_started_by(PointerButton::Secondary) && resizable(host) {
        let left = response.interact_pointer_pos().is_some_and(|p| p.x < response.rect.center().x);
        resize_from_top(ctx, host, left);
    }
    if response.secondary_clicked() {
        window_menu(ctx, host);
    }
    response
}

/// Resizes the window from its top left (`left`) or top right corner while
/// the right button is held: on X11 with the window manager told that it is
/// that button (see `x11::WindowManager::resize_from_top`), elsewhere
/// through winit, which names none.
fn resize_from_top(ctx: &egui::Context, host: &mut Host, left: bool) {
    let started = match host.wm {
        Some(wm) => wm.resize_from_top(left, 3),
        None => {
            let direction = if left { ResizeDirection::NorthWest } else { ResizeDirection::NorthEast };
            host.window.drag_resize_window(direction).is_ok()
        }
    };
    if started {
        let_go(ctx, host, PointerButton::Secondary);
    } else {
        log::debug!("cannot resize the window");
    }
}

/// `title` in `rect`, centred, and cut short with an ellipsis where it does
/// not fit.
fn draw_title(ui: &Ui, rect: Rect, title: &str) {
    if rect.width() <= 0.0 {
        return;
    }
    let mut job = egui::text::LayoutJob::single_section(
        title.to_string(),
        egui::TextFormat::simple(FontId::proportional(TITLE_TEXT), TITLE_COLOR),
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(rect.width());
    let galley = ui.ctx().fonts_mut(|f| f.layout_job(job));
    let at = Align2::CENTER_CENTER.align_size_within_rect(galley.size(), rect).min;
    ui.painter().with_clip_rect(rect).galley(at, galley, TITLE_COLOR);
}

/// A window button at `rect`: its symbol drawn in lines, `size` of the
/// button's side, framed under the pointer (in red for close).
fn window_button(ui: &mut Ui, rect: Rect, button: Button, size: f32) -> Response {
    let response = ui.interact(rect, Id::new(("window-button", button as u8)), Sense::click());
    let label = match button {
        Button::Minimize => "Minimize",
        Button::Maximize => "Maximize",
        Button::Restore => "Restore",
        Button::Close => "Close",
    };
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    let visuals = ui.style().interact(&response);
    let painter = ui.painter();
    let hot = response.hovered() || response.is_pointer_button_down_on();
    let color = if hot && button == Button::Close {
        painter.rect_filled(rect, 0, CLOSE_HOVER);
        Color32::WHITE
    } else if hot {
        painter.rect_filled(rect, 0, visuals.bg_fill);
        visuals.fg_stroke.color
    } else {
        TITLE_COLOR
    };
    let stroke = Stroke::new(1.0, color);
    let c = rect.center();
    let h = (rect.width() * size / 2.0).round();
    match button {
        Button::Close => {
            painter.line_segment([c + vec2(-h, -h), c + vec2(h, h)], stroke);
            painter.line_segment([c + vec2(-h, h), c + vec2(h, -h)], stroke);
        }
        Button::Minimize => {
            painter.line_segment([c + vec2(-h, 0.5), c + vec2(h, 0.5)], stroke);
        }
        Button::Maximize => {
            let square = Rect::from_center_size(c, vec2(2.0 * h, 2.0 * h));
            painter.rect_stroke(square, 0, stroke, egui::StrokeKind::Inside);
        }
        Button::Restore => {
            // Two windows, one behind the other.
            let d = (h / 2.0).round().max(1.0);
            let front = Rect::from_min_size(c + vec2(-h, -h + d), vec2(2.0 * h - d, 2.0 * h - d));
            painter.rect_stroke(front, 0, stroke, egui::StrokeKind::Inside);
            painter.add(Shape::line(
                vec![
                    pos2(front.min.x + d, front.min.y),
                    pos2(front.min.x + d, front.min.y - d),
                    pos2(front.max.x + d, front.min.y - d),
                    pos2(front.max.x + d, front.max.y - d),
                    pos2(front.max.x, front.max.y - d),
                ],
                stroke,
            ));
        }
    }
    response
}

/// What a window button does.
fn press(host: &mut Host, button: Button) {
    match button {
        Button::Minimize => host.window.set_minimized(true),
        Button::Maximize => host.window.set_maximized(true),
        Button::Restore => host.window.set_maximized(false),
        // As the window manager's close: the window's own event handler
        // decides what closing it means (see `App::draw`).
        Button::Close => host.close = true,
    }
}

/// The window manager's or the compositor's window menu, at the pointer:
/// on Wayland asked of the compositor, which ignores it where it has none;
/// on X11 only where the window manager takes `_GTK_SHOW_WINDOW_MENU`.
fn window_menu(ctx: &egui::Context, host: &mut Host) {
    let shown = match host.wm {
        Some(wm) => wm.show_menu(),
        None if host.display_server == super::DisplayServer::Wayland => {
            let Some(pos) = ctx.input(|i| i.pointer.latest_pos()) else {
                return;
            };
            let ppp = ctx.pixels_per_point();
            host.window.show_window_menu(PhysicalPosition::new(
                (pos.x * ppp).round() as i32,
                (pos.y * ppp).round() as i32,
            ));
            true
        }
        None => false,
    };
    if shown {
        let_go(ctx, host, PointerButton::Secondary);
    }
}

/// The release of `button` given to egui for its next frame: a move, a
/// resize or a window menu has taken the pointer, and the real release is
/// the window manager's. Should the window see it as well, a second release
/// changes nothing.
fn let_go(ctx: &egui::Context, host: &mut Host, button: PointerButton) {
    let Some(pos) = ctx.input(|i| i.pointer.latest_pos()) else {
        return;
    };
    host.winit.egui_input_mut().events.push(egui::Event::PointerButton {
        pos,
        button,
        pressed: false,
        modifiers: egui::Modifiers::default(),
    });
    host.window.request_redraw();
}

/// The title bar's fill: the toolbar's, a little darker.
fn darker(fill: Color32) -> Color32 {
    let [r, g, b, a] = fill.to_array();
    let d = |c: u8| (c as u16 * 3 / 4) as u8;
    Color32::from_rgba_premultiplied(d(r), d(g), d(b), a)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(width: f32, height: f32) -> Rect {
        Rect::from_min_max(pos2(0.0, 0.0), pos2(width, height))
    }

    fn resize_at(window: Rect, corner: f32, edge: f32, p: Pos2) -> Option<ResizeDirection> {
        zone_at(window, corner, edge, p).map(|(_, d)| d)
    }

    #[test]
    fn corners_are_four_points_each_way_and_edges_two() {
        let w = window(800.0, 600.0);
        let at = |x, y| resize_at(w, CORNER, EDGE, pos2(x, y));
        assert_eq!(at(0.0, 0.0), Some(ResizeDirection::NorthWest));
        assert_eq!(at(3.5, 3.5), Some(ResizeDirection::NorthWest));
        assert_eq!(at(799.0, 0.0), Some(ResizeDirection::NorthEast));
        assert_eq!(at(0.0, 599.0), Some(ResizeDirection::SouthWest));
        assert_eq!(at(796.0, 596.0), Some(ResizeDirection::SouthEast));
        // Past the corner along an edge, the edge.
        assert_eq!(at(4.0, 0.0), Some(ResizeDirection::North));
        assert_eq!(at(400.0, 1.5), Some(ResizeDirection::North));
        assert_eq!(at(0.0, 4.0), Some(ResizeDirection::West));
        assert_eq!(at(799.5, 300.0), Some(ResizeDirection::East));
        assert_eq!(at(400.0, 598.0), Some(ResizeDirection::South));
        // On the line between a corner and an edge, one of them.
        assert_eq!(at(799.0, 4.0), Some(ResizeDirection::East));
        assert_eq!(at(4.0, 599.0), Some(ResizeDirection::South));
        // Two points in from an edge, or a corner's square left, nothing.
        assert_eq!(at(400.0, 2.0), None);
        assert_eq!(at(2.0, 300.0), None);
        assert_eq!(at(4.0, 3.0), None);
        assert_eq!(at(400.0, 300.0), None);
        // Outside the window, nothing.
        assert_eq!(at(-1.0, 0.0), None);
        assert_eq!(at(800.0, 300.0), None);
    }

    #[test]
    fn the_zones_do_not_overlap() {
        let w = window(400.0, 300.0);
        let zones = zones(w, CORNER, EDGE);
        for (i, (a, _)) in zones.iter().enumerate() {
            assert!(w.contains_rect(*a));
            for (b, _) in &zones[i + 1..] {
                assert!(a.intersect(*b).area() <= 0.0, "{a:?} {b:?}");
            }
        }
    }

    #[test]
    fn a_side_resized_here_moves_that_side_alone_down_to_the_minimum() {
        let resize = |direction| EdgeResize {
            direction,
            from: (500, 300),
            at: (100, 50),
            size: PhysicalSize::new(800, 600),
        };
        let min = PhysicalSize::new(400, 360);
        let size = PhysicalSize::new;
        // Right and bottom: the size alone, by the pointer's own axis.
        assert_eq!(resize(ResizeDirection::East).geometry((530, 340), min), ((100, 50), size(830, 600)));
        assert_eq!(resize(ResizeDirection::South).geometry((530, 340), min), ((100, 50), size(800, 640)));
        // Left and top: the window moves with its side, the other side stays.
        assert_eq!(resize(ResizeDirection::West).geometry((450, 340), min), ((50, 50), size(850, 600)));
        assert_eq!(resize(ResizeDirection::North).geometry((450, 280), min), ((100, 30), size(800, 620)));
        // Past the minimum, the minimum, with the opposite side still where it was.
        assert_eq!(resize(ResizeDirection::West).geometry((1000, 300), min), ((500, 50), size(400, 600)));
        assert_eq!(resize(ResizeDirection::South).geometry((500, -100), min), ((100, 50), size(800, 360)));
    }

    #[test]
    fn the_title_bar_sits_above_the_toolbar_as_wide_as_it() {
        let w = Rect::from_min_max(pos2(0.0, 0.0), pos2(1280.0, 800.0));
        assert_eq!(title_rect(w, 400.0, 480.0), Rect::from_min_max(pos2(400.0, 0.0), pos2(880.0, TITLE_BAR)));
    }

    #[test]
    fn the_title_bar_has_close_always_and_the_rest_where_offered() {
        use Button::*;
        assert_eq!(title_buttons(WindowButtons::all(), false), vec![Minimize, Maximize, Close]);
        assert_eq!(title_buttons(WindowButtons::all(), true), vec![Minimize, Restore, Close]);
        assert_eq!(title_buttons(WindowButtons::CLOSE, false), vec![Close]);
        assert_eq!(title_buttons(WindowButtons::empty(), false), vec![Close]);
        assert_eq!(title_buttons(WindowButtons::CLOSE | WindowButtons::MAXIMIZE, false), vec![Maximize, Close]);
        assert_eq!(title_buttons(WindowButtons::MINIMIZE, false), vec![Minimize, Close]);
    }
}
