// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The connection-info panel: `ConnectionPanel.java`, as a window over the
//! live desktop.
//!
//! Controls above facts, as on the phone: the panel's real job is "change
//! the quality", and the facts are what is read *after* deciding something
//! is wrong. The controls are the backend's live options from the registry,
//! rendered by their kind and nothing else, so the panel knows no option by
//! name; then the two that are not options (the desktop's size, where the
//! far end takes one, and which of its displays it is sending, where it
//! sends one at a time), which are facts about this connection rather than
//! descriptions of the backend; whether the desktop should follow this
//! window's size; the client's own settings (`settings.rs`), which a
//! session runs with a copy of and takes a change to at once; and the debug
//! overlay.
//!
//! Then the desktop's name on top of what the backend has to say about the
//! connection in its own words, all of it in the backend's order, and the
//! counts this client keeps itself. The session refreshes the [`Snapshot`]
//! once a second while the panel is open, which is fast enough that a
//! change reads as a result and slow enough that the question costs the
//! protocol nothing. Nothing is rebuilt by a refresh: egui redraws from the
//! same ids every frame, so an open dropdown stays open and a field being
//! typed in keeps its text until it is left.
//!
//! The panel opens in the top right corner and can be dragged by anything
//! on it that is not a control; its text cannot be selected, so a drag
//! that starts on a label moves the panel. Where it was put is kept for
//! the session as an [`Anchor`], not saved, so that it keeps its place
//! when the window is resized, and it is kept [`MARGIN`] from every edge
//! of the window. It is an ordinary egui window, without a shadow, in
//! [`Order::Middle`], so the toolbar (in [`Order::Foreground`]) is drawn
//! over it however recently the panel was clicked.
//!
//! The panel does nothing to the backend itself. What is changed on it
//! comes back as [`Action`]s for the session to apply, which is what keeps
//! the panel a drawing of state rather than an owner of any.

use super::settings::Settings;
use crate::backend::{ConnectionFact, Facts, Field, Monitor, Traffic};
use crate::backends::Platform;
use crate::options::{self, Options, Spec, Type};
use runtime::media::audio::Switch;
use egui::{
    Align, ComboBox, CursorIcon, Frame, Grid, Id, Layout, Order, Pos2, Rangef, Rect, Response, RichText, ScrollArea,
    Sense, Shadow, TextEdit, Ui, Vec2, WidgetInfo, WidgetType, pos2, vec2,
};
use std::collections::BTreeMap;

/// What the backend said at the last poll: one instant, so that no row is
/// drawn from one answer and its neighbour from another.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub facts: Facts,
    pub info: Vec<ConnectionFact>,
    pub traffic: Option<Traffic>,
}

impl Snapshot {
    /// What the backend said, with the desktop's name made [`printable`]:
    /// here, once, so that every window's title, the panel's heading and its
    /// row, and the name a mixer lists the session's sound under have the
    /// same, and none of them a far end's control characters.
    pub fn new(facts: Facts, mut info: Vec<ConnectionFact>, traffic: Option<Traffic>) -> Snapshot {
        for fact in info.iter_mut().filter(|f| f.field == Field::DesktopName) {
            fact.value = printable(&fact.value);
        }
        Snapshot { facts, info, traffic }
    }

    /// The desktop's own name for itself, where the protocol has said it
    /// (and it is not all control characters).
    pub fn desktop_name(&self) -> Option<&str> {
        self.info
            .iter()
            .find(|f| f.field == Field::DesktopName)
            .map(|f| f.value.as_str())
            .filter(|v| !v.is_empty())
    }
}

/// The most of a far end's name for itself that is kept, in characters: a
/// window list or a task bar shows a few dozen, and the name is the far
/// end's to make as long as it likes (RFB's may be 4 GB).
pub const NAME_MAX: usize = 128;

/// `name`, a far end's or one typed or pasted, fit to be a window's title:
/// control characters dropped (a NUL, which neither X11's title property
/// nor Wayland's `set_title` can carry and winit panics on; an escape; a
/// bell) and the ones that are white space (a tab, a newline) made a space;
/// white space at either end trimmed; and no more than [`NAME_MAX`]
/// characters, the last of them an ellipsis where it was cut.
pub fn printable(name: &str) -> String {
    let mut chars = name
        .chars()
        .filter_map(|c| match c {
            c if c.is_control() && c.is_whitespace() => Some(' '),
            c if c.is_control() => None,
            c => Some(c),
        })
        .skip_while(|c| c.is_whitespace());
    let mut out: String = chars.by_ref().take(NAME_MAX).collect();
    if chars.next().is_some() {
        out.pop();
        out.truncate(out.trim_end().len());
        out.push('…');
    }
    out.truncate(out.trim_end().len());
    out
}

/// What the session's controls are set to, for drawing them.
pub struct Controls<'a> {
    /// The backend's options; only the live ones are drawn.
    pub specs: &'static [Spec],
    /// Every option's value right now: what the session connected with,
    /// then whatever has been set since. All of them rather than the live
    /// ones, since a gate may hang off one that is not.
    pub values: &'a Options,
    /// Whether the controls mean anything: none of them is live against a
    /// session that is connecting or gone.
    pub connected: bool,
    /// This window's size in desktop pixels, offered as a size.
    pub window: Option<(u32, u32)>,
    pub follow: bool,
    /// What the session is running with.
    pub settings: Settings,
    /// Whether the debug overlay is showing.
    pub debug: bool,
    /// The session's sound switch, where the far end may send sound: what
    /// it does, and whether it is on.
    pub sound: Option<(Switch, bool)>,
}

/// Something changed on the panel, to be done to the session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    SetOption(&'static str, String),
    DesktopSize(u32, u32),
    Display(i32),
    Follow(bool),
    /// The settings as they are now wanted, one of them changed.
    Settings(Settings),
    /// The debug overlay on or off.
    Debug(bool),
    /// The sound switch turned on or off: the far end's sound enabled, or
    /// muted here.
    Sound(bool),
}

/// The panel's own state between frames.
#[derive(Default)]
pub struct Panel {
    pub open: bool,
    /// What is being typed into a text field, per option, while it has the
    /// focus: a refresh must not rewrite a field while it is typed in, and
    /// the value is only sent once the field loses the focus.
    edits: BTreeMap<&'static str, String>,
    /// Where the panel was last dragged to; the top right corner until it
    /// has been.
    anchor: Option<Anchor>,
}

/// The panel's distance from the window's edges where it opens.
const INSET: f32 = 16.0;

/// The least distance kept between the panel and each edge of the window,
/// wherever it is dragged or placed and however small the window is. The
/// connection dialog keeps the same.
pub const MARGIN: f32 = 8.0;

/// The panel's width, its frame included, in points. The window's minimum
/// size is made to fit it.
pub const WIDTH: f32 = 360.0;

/// The middle of the room the panel has to move in on either axis, as a
/// fraction of it: a panel there is placed by its centre on that axis.
/// Measured in the room rather than the window so that a panel taller than
/// a third of the window, dragged against its bottom edge, stays against
/// it.
const MIDDLE: f32 = 0.30;

/// Where the panel was dragged to, relative to the window, so that it keeps
/// its place when the window is resized. Per axis, the point of the panel
/// that is kept in place and where it is, as a fraction of the window's
/// size from its left or top edge: the panel's edge nearest that edge of
/// the window, or its centre where the panel is in the [`MIDDLE`].
#[derive(Clone, Copy, Debug, PartialEq)]
struct Anchor {
    x: (Align, f32),
    y: (Align, f32),
}

impl Anchor {
    /// The anchor of a panel drawn at `panel` in `window`.
    fn of(panel: Rect, window: Rect) -> Anchor {
        Anchor {
            x: axis_anchor(panel.x_range(), window.x_range()),
            y: axis_anchor(panel.y_range(), window.y_range()),
        }
    }

    /// Where a panel of `size` goes in `window`: its top left corner, kept
    /// so that all of the panel is in the window, [`MARGIN`] from its
    /// edges, where it fits.
    fn left_top(self, size: Vec2, window: Rect) -> Pos2 {
        pos2(
            axis_place(self.x, size.x, window.x_range()),
            axis_place(self.y, size.y, window.y_range()),
        )
    }
}

/// [`Anchor::of`] on one axis, for a panel spanning `panel` in a window
/// spanning `window`.
fn axis_anchor(panel: Rangef, window: Rangef) -> (Align, f32) {
    let fraction = |at: f32| (at - window.min) / window.span().max(1.0);
    // 0 against the start of the window, 1 against its end.
    let room = window.span() - panel.span();
    let along = if room > 0.0 { (panel.min - window.min) / room } else { 0.0 };
    if (along - 0.5).abs() <= MIDDLE / 2.0 {
        (Align::Center, fraction(panel.center()))
    } else if along < 0.5 {
        (Align::Min, fraction(panel.min))
    } else {
        (Align::Max, fraction(panel.max))
    }
}

/// [`Anchor::left_top`] on one axis: where a panel `size` long starts. One
/// too long for the window with its margins starts a margin from the start.
fn axis_place((align, fraction): (Align, f32), size: f32, window: Rangef) -> f32 {
    let at = window.min + fraction * window.span();
    let start = match align {
        Align::Min => at,
        Align::Center => at - size / 2.0,
        Align::Max => at - size,
    };
    start.min(window.max - MARGIN - size).max(window.min + MARGIN)
}

/// The sizes offered besides the window's and the desktop's own, which are
/// the RDP backends' list for the same reason they have one.
const SIZES: [(u32, u32); 6] = [
    (1920, 1200),
    (1920, 1080),
    (1600, 1000),
    (1366, 768),
    (1280, 800),
    (2560, 1440),
];

impl Panel {
    /// Draws the panel. `title` is what to call the session when the desktop
    /// has not named itself, and `state` the one-line status under it. What
    /// was changed on it this frame comes back as actions.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        snapshot: &Snapshot,
        controls: &Controls,
        title: &str,
        state: &str,
    ) -> Vec<Action> {
        let mut actions = Vec::new();
        if !self.open {
            return actions;
        }
        let screen = ctx.content_rect();
        // The panel and its drag are kept within this, which also caps its
        // size: egui keeps a window no larger than the rect it is
        // constrained to.
        let bounds = screen.shrink(MARGIN);
        let room = (screen.height() - 48.0).max(120.0).min(bounds.height());
        let id = Id::new("connection-panel");
        // Placed every frame from where it was put rather than left where
        // egui last had it, so that a resized window moves it too; by its
        // top left corner, as egui moves a window by the corner that it
        // is placed by and a drag would jump if that changed during it. Its
        // size is the last frame's, which is egui's too; the first frame
        // is egui's sizing pass, which is not drawn.
        let size = ctx.memory(|m| m.area_rect(id)).map_or(vec2(WIDTH, 0.0), |r| r.size());
        let left_top = match self.anchor {
            Some(anchor) => anchor.left_top(size, screen),
            None => pos2(screen.max.x - INSET - size.x, screen.min.y + INSET),
        };
        let shown = egui::Window::new("Connection")
            .id(id)
            // No title bar: the desktop's name heads the panel, and a bar
            // above it saying "Connection" was a second heading and a strip
            // of the desktop lost for it. The close button sits at the end
            // of the name's row instead.
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            // Drawn at its content's height, and scrolled once that is
            // taller than the window it floats over. The height given here
            // is what the content is *offered* (an egui window offers its
            // content no more than that, and a scroll area never asks for
            // more) and not what is drawn, which follows the content: the
            // scroll area shrinks to it, where the window's own would not.
            // A minimum rather than a default, since egui keeps the size a
            // window was last offered and only grows it to fit its content:
            // one squeezed by a small window would stay that small once the
            // window was larger again.
            .default_width(WIDTH)
            .min_width(WIDTH)
            .default_height(room)
            .min_height(room)
            .max_height(room)
            .current_pos(left_top)
            // Under the toolbar, which is in the foreground: egui raises a
            // window when it is clicked, but only among those of its order.
            .order(Order::Middle)
            .constrain_to(bounds)
            .frame(frame(&ctx.global_style()))
            .show(ctx, |ui| {
                // Nothing on it is text to copy, and a drag that starts on
                // a label moves the panel instead.
                ui.style_mut().interaction.selectable_labels = false;
                // The close button first, from the right, so that the name
                // and the state are given what is left of the row and wrap
                // there rather than pushing the button out of the panel.
                ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                    if close_button(ui).clicked() {
                        self.open = false;
                    }
                    ui.with_layout(Layout::top_down(Align::Min), |ui| {
                        ui.heading(snapshot.desktop_name().unwrap_or(title));
                        ui.label(RichText::new(state).weak());
                    });
                });
                ui.add_space(8.0);
                ScrollArea::vertical().show(ui, |ui| {
                    ui.add_enabled_ui(controls.connected, |ui| {
                        self.controls(ui, snapshot, controls, &mut actions);
                    });
                    facts(ui, snapshot);
                });
            });
        if let Some(shown) = shown
            && shown.response.dragged()
        {
            self.anchor = Some(Anchor::of(shown.response.rect, screen));
            ctx.set_cursor_icon(CursorIcon::Grabbing);
        }
        actions
    }

    fn controls(&mut self, ui: &mut Ui, snapshot: &Snapshot, controls: &Controls, actions: &mut Vec<Action>) {
        let live: Vec<&'static Spec> = controls.specs.iter().filter(|s| s.live).collect();
        let facts = &snapshot.facts;
        let values = controls.values;
        Grid::new("controls").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
            for spec in live {
                // The same two rules the dialog draws by: an option whose
                // gate is shut is not there, and a choice whose switch is
                // off is not offered.
                if let Some(gate) = spec.gate
                    && !gate.open(values.get(gate.key))
                {
                    continue;
                }
                // `--view-only` forces it.
                let forced = spec.key == "ViewOnly" && controls.settings.view_only;
                self.option_row(ui, spec, values, !forced, actions);
            }

            if facts.can_resize {
                let have = (facts.width as u32, facts.height as u32);
                ui.label(RichText::new("Desktop size").weak());
                ComboBox::from_id_salt("desktop-size")
                    .selected_text(size_label(have))
                    .width(ui.available_width())
                    .show_ui(ui, |ui| {
                        for (size, label) in size_choices(have, controls.window, &facts.monitors) {
                            if ui.selectable_label(size == have, label).clicked() && size != have {
                                actions.push(Action::DesktopSize(size.0, size.1));
                            }
                        }
                    });
                ui.end_row();
            }

            if !facts.displays.is_empty() {
                ui.label(RichText::new("Display").weak());
                let shown = facts
                    .displays
                    .get(facts.display.max(0) as usize)
                    .map(|m| display_label(facts.display.max(0), m))
                    .unwrap_or_else(|| format!("{} available", facts.displays.len()));
                ComboBox::from_id_salt("display")
                    .selected_text(shown)
                    .width(ui.available_width())
                    .show_ui(ui, |ui| {
                        for (i, m) in facts.displays.iter().enumerate() {
                            let i = i as i32;
                            if ui.selectable_label(i == facts.display, display_label(i, m)).clicked()
                                && i != facts.display
                            {
                                actions.push(Action::Display(i));
                            }
                        }
                    });
                ui.end_row();
            }

            // The toolbar's sound button as a switch, worded by what it does:
            // there whenever the far end may send sound, whether or not any
            // is playing, as the toolbar's is.
            if let Some((switch, on)) = controls.sound {
                ui.label(RichText::new(switch.label()).weak());
                let mut on = on;
                if ui.checkbox(&mut on, "").changed() {
                    actions.push(Action::Sound(on));
                }
                ui.end_row();
            }

            ui.label(RichText::new("Follow window size").weak());
            let mut follow = controls.follow;
            let note = if follow && !facts.can_resize && !facts.can_layout {
                "server has not accepted request"
            } else {
                ""
            };
            if ui.checkbox(&mut follow, RichText::new(note).weak().italics()).changed() {
                actions.push(Action::Follow(follow));
            }
            ui.end_row();

            let mut settings = controls.settings;
            if settings.rows(ui, true) {
                actions.push(Action::Settings(settings));
            }

            // The session's, not a setting: it is for looking into a
            // problem with this connection, and is off again in the next.
            ui.label(RichText::new("Debug overlay").weak());
            let mut debug = controls.debug;
            if ui.checkbox(&mut debug, "").changed() {
                actions.push(Action::Debug(debug));
            }
            ui.end_row();
        });
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);
    }

    /// One live option as the control its kind calls for, showing what it
    /// is set to now; disabled unless `enabled`.
    fn option_row(
        &mut self,
        ui: &mut Ui,
        spec: &'static Spec,
        values: &Options,
        enabled: bool,
        actions: &mut Vec<Action>,
    ) {
        ui.label(RichText::new(spec.label).weak());
        let current = values.get(spec.key);
        match spec.ty {
            Type::Bool => {
                let mut on = current.eq_ignore_ascii_case("true");
                if ui.add_enabled(enabled, egui::Checkbox::without_text(&mut on)).changed() {
                    actions.push(Action::SetOption(spec.key, if on { "true" } else { "false" }.to_string()));
                }
            }
            Type::Choice => {
                let offered: Vec<&options::Choice> = spec
                    .choices
                    .iter()
                    .filter(|c| c.requires.is_none_or(|key| values.bool(key)))
                    .collect();
                ComboBox::from_id_salt(("live", spec.key))
                    .selected_text(spec.label_for(current, Platform::Linux).to_string())
                    .width(ui.available_width())
                    .show_ui(ui, |ui| {
                        for choice in offered {
                            let row = ui.selectable_label(choice.value == current, choice.label_on(Platform::Linux));
                            if row.clicked() && choice.value != current {
                                actions.push(Action::SetOption(spec.key, choice.value.to_string()));
                            }
                        }
                    });
            }
            Type::Int | Type::Text => {
                let buffer = self.edits.entry(spec.key).or_insert_with(|| current.to_string());
                let field = ui.add(TextEdit::singleline(buffer).desired_width(f32::INFINITY));
                if field.lost_focus() {
                    let typed = self.edits.remove(spec.key).unwrap_or_default();
                    if typed != current {
                        match spec.check(&typed) {
                            Ok(()) => actions.push(Action::SetOption(spec.key, typed)),
                            Err(why) => log::warn!("{why}; left as it was"),
                        }
                    }
                } else if !field.has_focus() {
                    // Refreshed only while it does not have the focus.
                    *buffer = current.to_string();
                }
            }
        }
        ui.end_row();
    }
}

/// The panel's frame, an egui window's without the shadow; the connection
/// dialog's too.
pub fn frame(style: &egui::Style) -> Frame {
    Frame::window(style).shadow(Shadow::NONE)
}

/// An egui window's close button, the cross its title bar draws, as a
/// widget of its own: a square the height of a button, with the cross in
/// the text colour of its state, so that it lights up under the pointer as
/// the title bar's does.
fn close_button(ui: &mut Ui) -> Response {
    let side = ui.spacing().interact_size.y;
    let (rect, response) = ui.allocate_exact_size(vec2(side, side), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), "Close"));
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact(&response);
        let rect = rect.shrink(4.0).expand(visuals.expansion);
        let stroke = visuals.fg_stroke;
        ui.painter().line_segment([rect.left_top(), rect.right_bottom()], stroke);
        ui.painter().line_segment([rect.right_top(), rect.left_bottom()], stroke);
    }
    response
}

/// What to offer: this window first, since fitting it is the whole point;
/// then the size the desktop already has, so the current state has a row of
/// its own; the far end's own screens, where it has said how it is divided;
/// then the ordinary ones. No backend here lists the sizes its far end would
/// take (RFB, RDP and SPICE all take any), so there is nothing to ask.
fn size_choices(have: (u32, u32), window: Option<(u32, u32)>, monitors: &[Monitor]) -> Vec<((u32, u32), String)> {
    let mut choices: Vec<((u32, u32), String)> = Vec::new();
    let mut offer = |size: (u32, u32), label: String| {
        if size.0 > 0 && size.1 > 0 && !choices.iter().any(|(s, _)| *s == size) {
            choices.push((size, label));
        }
    };
    if let Some(window) = window {
        offer(window, format!("This window · {}", size_label(window)));
    }
    offer(have, size_label(have));
    for m in monitors {
        let size = (m.width.max(0) as u32, m.height.max(0) as u32);
        offer(size, size_label(size));
    }
    for size in SIZES {
        offer(size, size_label(size));
    }
    choices
}

fn size_label((w, h): (u32, u32)) -> String {
    format!("{w} × {h}")
}

fn display_label(i: i32, m: &Monitor) -> String {
    format!("{} · {} × {}", i + 1, m.width, m.height)
}

/// The facts: the backend's rows, then the counts this client keeps, then
/// what carries the connection where it is not this end's own (the Tunnel
/// group).
///
/// All the backend's rows, in its order, its diagnostic ones among the
/// rest rather than folded away under a heading of their own.
fn facts(ui: &mut Ui, snapshot: &Snapshot) {
    let plain: Vec<&ConnectionFact> = snapshot
        .info
        .iter()
        .filter(|f| !matches!(f.field, Field::DesktopName | Field::Tunnel))
        .collect();
    let tunnel: Vec<&ConnectionFact> = snapshot.info.iter().filter(|f| f.field == Field::Tunnel).collect();

    Grid::new("facts").num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
        for fact in &plain {
            row(ui, fact.label, &fact.value);
        }
        // The count, unless the backend already put it among its own rows
        // (the Java versions of most of them do).
        if let Some(traffic) = snapshot.traffic
            && !plain.iter().any(|f| f.field == Field::Data)
        {
            let data = ConnectionFact::data(traffic);
            row(ui, data.label, &data.value);
        }
        let facts = &snapshot.facts;
        if facts.width > 0 && facts.height > 0 {
            row(ui, "Desktop size", &format!("{} × {}", facts.width, facts.height));
        }
        for (i, m) in facts.monitors.iter().enumerate() {
            row(
                ui,
                if i == 0 { "Monitors" } else { "" },
                &format!("{} · {} × {} at {}, {}", i + 1, m.width, m.height, m.x, m.y),
            );
        }
    });
    if !tunnel.is_empty() {
        ui.separator();
        Grid::new("tunnel").num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
            for fact in &tunnel {
                row(ui, fact.label, &fact.value);
            }
        });
    }
}

/// A field the backend has but no answer for yet says so, rather than
/// vanishing: the shape of the panel depends on the protocol, not on how
/// much this server has got round to saying.
fn row(ui: &mut Ui, label: &str, value: &str) {
    ui.label(RichText::new(label).weak());
    if value.is_empty() {
        ui.label(RichText::new("Not set").weak().italics());
    } else {
        ui.label(value);
    }
    ui.end_row();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_comes_first_and_nothing_is_offered_twice() {
        let monitors = [
            Monitor {
                id: 0,
                x: 0,
                y: 0,
                width: 1920,
                height: 1200,
            },
            Monitor {
                id: 1,
                x: 1920,
                y: 0,
                width: 1280,
                height: 800,
            },
        ];
        let choices = size_choices((1920, 1200), Some((1600, 900)), &monitors);
        let sizes: Vec<(u32, u32)> = choices.iter().map(|(s, _)| *s).collect();
        assert_eq!(sizes[0], (1600, 900));
        assert!(choices[0].1.starts_with("This window"));
        assert_eq!(sizes[1], (1920, 1200));
        assert_eq!(sizes[2], (1280, 800));
        assert_eq!(sizes.iter().filter(|s| **s == (1920, 1200)).count(), 1);
        assert_eq!(sizes.iter().filter(|s| **s == (1280, 800)).count(), 1);
        // The window, the desktop, the second monitor, and the four of the
        // usual list that are not one of those.
        assert_eq!(sizes.len(), 7);
    }

    const WINDOW: Rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(1000.0, 800.0));

    #[test]
    fn a_panel_near_an_edge_keeps_its_distance_from_it_in_proportion() {
        // 40 from the right edge and 80 from the top of a 1000 × 800 window.
        let panel = Rect::from_min_size(pos2(600.0, 80.0), vec2(360.0, 200.0));
        let anchor = Anchor::of(panel, WINDOW);
        assert_eq!(anchor.x, (Align::Max, 0.96));
        assert_eq!(anchor.y, (Align::Min, 0.1));
        assert_eq!(anchor.left_top(panel.size(), WINDOW), panel.min);
        // Twice as wide: still 4% of the width from the right edge.
        let wide = Rect::from_min_size(pos2(0.0, 0.0), vec2(2000.0, 800.0));
        assert_eq!(anchor.left_top(panel.size(), wide), pos2(1920.0 - 360.0, 80.0));
        // Half as tall: 10% of the height from the top.
        let short = Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 400.0));
        assert_eq!(anchor.left_top(vec2(360.0, 200.0), short), pos2(600.0, 40.0));
    }

    #[test]
    fn a_panel_in_the_middle_stays_centred() {
        let panel = Rect::from_center_size(WINDOW.center(), vec2(360.0, 400.0));
        let anchor = Anchor::of(panel, WINDOW);
        assert_eq!(anchor.x, (Align::Center, 0.5));
        assert_eq!(anchor.y, (Align::Center, 0.5));
        let other = Rect::from_min_size(pos2(10.0, 20.0), vec2(1600.0, 1000.0));
        let size = vec2(360.0, 400.0);
        let placed = Rect::from_min_size(anchor.left_top(size, other), size);
        assert_eq!(placed.center(), other.center());
    }

    #[test]
    fn the_middle_is_the_middle_thirty_percent() {
        // A panel 100 long in a window 1000 long has 900 to move in, the
        // middle 30% of which is 315 to 585 from the start.
        let window = Rangef::new(0.0, 1000.0);
        assert_eq!(axis_anchor(Rangef::new(320.0, 420.0), window), (Align::Center, 0.37));
        assert_eq!(axis_anchor(Rangef::new(580.0, 680.0), window), (Align::Center, 0.63));
        assert_eq!(axis_anchor(Rangef::new(300.0, 400.0), window), (Align::Min, 0.3));
        assert_eq!(axis_anchor(Rangef::new(600.0, 700.0), window), (Align::Max, 0.7));
        // A panel most of the window's height, against its bottom edge,
        // though its centre is in the middle 30% of the window.
        let window = Rangef::new(0.0, 800.0);
        assert_eq!(axis_anchor(Rangef::new(235.0, 800.0), window), (Align::Max, 1.0));
        // One that fills it.
        assert_eq!(axis_anchor(Rangef::new(0.0, 800.0), window), (Align::Min, 0.0));
    }

    #[test]
    fn a_panel_is_kept_in_the_window() {
        // Anchored 2% from the right edge: in a window too narrow for that,
        // it is kept a margin from the left edge, and too short, the top.
        let anchor = Anchor {
            x: (Align::Max, 0.98),
            y: (Align::Max, 1.0),
        };
        let small = Rect::from_min_size(pos2(0.0, 0.0), vec2(300.0, 200.0));
        assert_eq!(anchor.left_top(vec2(360.0, 400.0), small), pos2(8.0, 8.0));
        // Anchored past an edge, or closer to it than the margin, it stops
        // a margin from the edge.
        let anchor = Anchor {
            x: (Align::Min, -0.1),
            y: (Align::Max, 1.2),
        };
        assert_eq!(anchor.left_top(vec2(360.0, 400.0), WINDOW), pos2(8.0, 392.0));
        let anchor = Anchor {
            x: (Align::Max, 0.996),
            y: (Align::Min, 0.005),
        };
        assert_eq!(anchor.left_top(vec2(360.0, 400.0), WINDOW), pos2(1000.0 - 8.0 - 360.0, 8.0));
        // Flush with a corner it is drawn at a margin from both edges.
        let corner = Rect::from_min_size(pos2(1000.0 - 360.0, 800.0 - 400.0), vec2(360.0, 400.0));
        let anchor = Anchor::of(corner, WINDOW);
        assert_eq!(anchor.left_top(corner.size(), WINDOW), pos2(632.0, 392.0));
    }

    #[test]
    fn a_desktop_with_no_size_yet_is_not_a_choice() {
        let choices = size_choices((0, 0), None, &[]);
        assert_eq!(choices.len(), SIZES.len());
    }
}
