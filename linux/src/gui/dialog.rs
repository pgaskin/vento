// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The connection dialog: what `vento` opens with when it is given nothing
//! on the command line, and what a session returns to when it ends.
//!
//! A backend to pick, the address, user name and password, a domain for a
//! backend whose user name carries one, and under them that backend's
//! options drawn from the registry in `api::options`: the same rows `-o`
//! checks against, rendered by their kind and nothing else, so the dialog
//! knows no option by name. Under those, the client's own
//! settings (`settings.rs`), which are not the connection's: they are one
//! answer for every connection, so they are drawn once, apart, and not
//! filled in from a request. Nothing is saved: there are no connections to
//! keep and no file to keep them in.
//!
//! It is drawn as a panel in the connection panel's frame, centred across
//! the window over the [`BACKDROP`], and scrolls where the window is too
//! short for it. A window drawing its own decorations (see `csd.rs`) is
//! the panel instead: its frame fills the window, with the window's close
//! and minimise buttons in the top right corner and resize edges all
//! round, and a drag on it anywhere but a control moves the window. Such a
//! window is made the panel's size when the dialog comes up ([`Dialog::fit`])
//! and the size it has without its own decorations when a session starts
//! (see `App::connect`).

use super::csd;
use super::panel::{self, MARGIN};
use super::settings::Settings;
use super::{BACKDROP, Host, Request};
use crate::backends::{self, Kind, Platform};
use crate::options::{self, Options, Spec, Type};
use egui::{
    Align, ComboBox, Frame, Grid, Id, Layout, Order, PointerButton, Pos2, RichText, ScrollArea, Sense, TextEdit, Ui,
    Vec2, pos2, vec2,
};
use std::collections::BTreeMap;

/// The dialog's width, its frame included, in points, where the window has
/// room for it.
pub const WIDTH: f32 = 520.0;

/// Its least width, in a window too narrow for [`WIDTH`]: the connection
/// panel's, which the window's minimum size is made for.
const MIN_WIDTH: f32 = panel::WIDTH;

/// The most space left above the dialog, in points: less in a window too
/// short for it, where the dialog is centred in what room there is.
const TOP: f32 = 40.0;

/// The window's buttons in the dialog's corner, with its own decorations:
/// their side, and how far they are from the window's edges, in points.
const BUTTON: f32 = 24.0;
const BUTTON_INSET: f32 = 6.0;

/// Makes a window drawn with its own decorations the panel's size: the
/// dialog's width, and `height`, the panel's, up to the height of the
/// monitor it is on, and where `centre`, with its centre kept where it is
/// (X11). Not while maximised, which keeps its size, nor while fullscreen,
/// which may be on its way out (`--fullscreen` leaving it for the dialog):
/// whether it is done with, which it is not while fullscreen.
fn fit_window(host: &Host, height: f32, centre: bool) -> bool {
    let window = host.window;
    if window.fullscreen().is_some() {
        return false;
    }
    if window.is_maximized() {
        return true;
    }
    let mut size = winit::dpi::LogicalSize::new(WIDTH, height.ceil());
    if let Some(monitor) = window.current_monitor() {
        let room = monitor.size().to_logical::<f32>(window.scale_factor());
        size.height = size.height.min(room.height);
    }
    log::debug!("fitting the window to the connection dialog: {}×{}", size.width, size.height);
    if centre
        && let Ok(at) = window.outer_position()
    {
        let now = window.inner_size().height as i32;
        let new = (size.height as f64 * window.scale_factor()).round() as i32;
        window.set_outer_position(winit::dpi::PhysicalPosition::new(at.x, at.y + (now - new) / 2));
    }
    let _ = window.request_inner_size(size);
    true
}

/// Where a dialog of `size` goes in `window`: its top left corner, centred
/// across, and [`TOP`] down or centred, at least [`MARGIN`] from the edges.
pub fn left_top(size: Vec2, window: egui::Rect) -> Pos2 {
    let free = window.shrink(MARGIN).size() - size;
    pos2(
        window.min.x + MARGIN + (free.x / 2.0).max(0.0),
        window.min.y + MARGIN + (free.y / 2.0).clamp(0.0, TOP),
    )
}

pub struct Dialog {
    kinds: Vec<&'static Kind>,
    selected: usize,
    address: String,
    user: String,
    domain: String,
    password: String,
    /// Each backend's values, kept per backend so that switching the picker
    /// and back loses nothing.
    values: Vec<BTreeMap<&'static str, String>>,
    /// Why the last session ended, shown until the next attempt.
    pub notice: Option<String>,
    /// What was wrong with the last attempt to connect from here.
    error: Option<String>,
    /// The window, drawn with its own decorations, is to be made the
    /// panel's size once the panel has been measured.
    pub fit: bool,
    /// The dialog has been drawn at least once, and so measured: until
    /// then egui's size for it is an earlier dialog's.
    drawn: bool,
    /// The next fit keeps the window's centre where it is rather than its
    /// top: the window opened taller than the dialog, and was placed by
    /// the window manager for that height (X11).
    pub centre: bool,
    /// The dialog's height with all of its form shown, as last drawn: its
    /// height and what its form scrolls by, where the window is too short
    /// for it.
    natural: Option<f32>,
}

impl Dialog {
    pub fn new() -> Dialog {
        let kinds = backends::all();
        let values = kinds.iter().map(|k| defaults(k.options)).collect();
        Dialog {
            kinds,
            selected: 0,
            address: String::new(),
            user: String::new(),
            domain: String::new(),
            password: String::new(),
            values,
            notice: None,
            error: None,
            fit: false,
            drawn: false,
            natural: None,
            centre: false,
        }
    }

    /// Filled in from a request, so that a session that ended can be
    /// started again with a click.
    pub fn from_request(request: &Request) -> Dialog {
        let mut dialog = Dialog::new();
        if let Some(i) = dialog.kinds.iter().position(|k| k.id == request.kind.id) {
            dialog.selected = i;
            for (key, value) in request.options.iter() {
                if let Some(spec) = request.kind.options.iter().find(|s| s.key == key) {
                    dialog.values[i].insert(spec.key, value.to_string());
                }
            }
        }
        dialog.address = request.address.clone();
        dialog.user = request.user.clone();
        dialog.domain = request.domain.clone();
        dialog.password = request.password.clone().unwrap_or_default();
        dialog
    }

    /// Draws the dialog, with the client's `settings` as its own section;
    /// a request on the frame "Connect" is pressed with something that
    /// passes the checks.
    pub fn ui(&mut self, root: &mut Ui, settings: &mut Settings, host: &mut Host) -> Option<Request> {
        let ctx = root.ctx().clone();
        let panel = panel::frame(&ctx.global_style());
        let frame = if host.csd {
            // The window is the panel: square, as the window is, and with
            // the panel's edge at the window's.
            egui::CentralPanel::default()
                .frame(panel.corner_radius(0).inner_margin(0))
                .show(root, |ui| {
                    let background = ui.interact(ui.max_rect(), Id::new("dialog-background"), Sense::click_and_drag());
                    if background.drag_started_by(PointerButton::Primary) {
                        csd::drag_window(&ctx, host);
                    }
                });
            egui::Area::new(Id::new("window-buttons"))
                .order(Order::Foreground)
                .fixed_pos(ctx.content_rect().min)
                .constrain(false)
                .show(&ctx, |ui| csd::dialog_buttons(ui, host, BUTTON, BUTTON_INSET));
            let _ = csd::resize_edges(&ctx, host, csd::DIALOG_EDGE, csd::DIALOG_EDGE);
            Frame::NONE.inner_margin(panel.inner_margin + panel.stroke.width as i8)
        } else {
            egui::CentralPanel::default()
                .frame(Frame::NONE.fill(BACKDROP))
                .show(root, |_| {});
            panel
        };
        let screen = ctx.content_rect();
        let margin = frame.total_margin().sum();
        // The window's own panel reaches its edges; one over the backdrop
        // is kept off them.
        let bounds = if host.csd { screen } else { screen.shrink(MARGIN) };
        let width = bounds.width().clamp(MIN_WIDTH, WIDTH);
        let id = Id::new("connection-dialog");
        // Placed by last frame's size; the first frame is egui's sizing
        // pass, which is not drawn.
        let measured = ctx.memory(|m| m.area_rect(id));
        let size = measured.map_or(vec2(width, 0.0), |r| r.size());
        if self.fit && self.drawn && host.csd && let Some(natural) = self.natural {
            self.fit = !fit_window(host, natural, std::mem::take(&mut self.centre));
        }
        let at = if host.csd {
            pos2(screen.center().x - size.x / 2.0, screen.min.y).max(screen.min)
        } else {
            left_top(size, screen)
        };
        let mut connect = false;
        let mut overflow = 0.0;
        let shown = egui::Area::new(id)
            .fixed_pos(at)
            // With the window's own decorations, a drag on it that is not
            // a control's moves the window, as one beside it does, once the
            // pointer has moved far enough to be dragging rather than
            // clicking.
            .sense(if host.csd { Sense::click_and_drag() } else { Sense::click() })
            .show(&ctx, |ui| {
                frame.show(ui, |ui| {
                    ui.set_width(width - margin.x);
                    ui.set_max_height(bounds.height() - margin.y);
                    // Its heading and labels are not text to copy, and with
                    // the window's own decorations a drag on them moves the
                    // window; why the last session ended can still be
                    // selected.
                    ui.style_mut().interaction.selectable_labels = false;
                    ui.vertical_centered(|ui| {
                        ui.add_space(4.0);
                        ui.heading("Connect to a desktop");
                        ui.add_space(8.0);
                        if let Some(notice) = &self.notice {
                            let text = RichText::new(notice).color(ui.visuals().warn_fg_color);
                            ui.add(egui::Label::new(text).selectable(true));
                            ui.add_space(8.0);
                        }
                    });
                    let selected = self.selected;
                    let form = ScrollArea::vertical().show(ui, |ui| {
                        connect = self.form(ui, settings);
                    });
                    overflow = (form.content_size.y - form.inner_rect.height()).max(0.0);
                    // Another backend's form is another height: the window
                    // is fitted to it once it has been drawn.
                    if self.selected != selected {
                        self.fit = true;
                        self.drawn = false;
                    }
                });
            });
        if host.csd && shown.response.drag_started_by(PointerButton::Primary) {
            csd::drag_window(&ctx, host);
        }
        self.natural = Some(shown.response.rect.height() + overflow);
        self.drawn = true;
        if !connect {
            return None;
        }
        match self.request() {
            Ok(request) => {
                self.error = None;
                self.notice = None;
                Some(request)
            }
            Err(e) => {
                self.error = Some(e);
                None
            }
        }
    }

    fn form(&mut self, ui: &mut Ui, settings: &mut Settings) -> bool {
        let mut connect = false;
        Grid::new("connection").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            ui.label("Backend");
            let kind = self.kinds[self.selected];
            ComboBox::from_id_salt("backend")
                .selected_text(kind.name)
                .width(ui.available_width())
                .show_ui(ui, |ui| {
                    for (i, k) in self.kinds.iter().enumerate() {
                        ui.selectable_value(&mut self.selected, i, k.name)
                            .on_hover_text(k.description);
                    }
                });
            ui.end_row();

            ui.label("Address");
            let address = ui.add(
                TextEdit::singleline(&mut self.address)
                    .hint_text("host, host:port, host:display")
                    .desired_width(f32::INFINITY),
            );
            if address.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                connect = true;
            }
            ui.end_row();

            ui.label("User");
            ui.add(
                TextEdit::singleline(&mut self.user)
                    .hint_text(if self.kinds[self.selected].user_has_domain {
                        "DOMAIN\\user, .\\user or MicrosoftAccount\\email"
                    } else {
                        ""
                    })
                    .desired_width(f32::INFINITY),
            );
            ui.end_row();

            ui.label("Password");
            let password = ui.add(
                TextEdit::singleline(&mut self.password)
                    .password(true)
                    .hint_text("will be prompted if required")
                    .desired_width(f32::INFINITY),
            );
            if password.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                connect = true;
            }
            ui.end_row();

            // Only for a backend whose user name carries a domain. One typed
            // for it stays, hidden and not sent, while another is picked.
            if self.kinds[self.selected].user_has_domain {
                ui.label("Domain");
                ui.add(
                    TextEdit::singleline(&mut self.domain)
                        .hint_text("if not given with the user name")
                        .desired_width(f32::INFINITY),
                );
                ui.end_row();
            }
        });

        let kind = self.kinds[self.selected];
        ui.add_space(12.0);
        ui.separator();
        ui.add_space(4.0);
        ui.label(RichText::new(format!("{} options", kind.name)).strong());
        ui.add_space(4.0);
        let values = &mut self.values[self.selected];
        Grid::new(("options", kind.id))
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                for spec in kind.options {
                    if let Some(gate) = spec.gate
                        && !gate.open(values.get(gate.key).map(String::as_str).unwrap_or(""))
                    {
                        continue;
                    }
                    option_row(ui, spec, values);
                }
            });
        // A choice whose switch was turned off this frame goes back to its
        // default (`Options::settle`'s rule), rather than staying selected
        // and being sent; the row above it was drawn before the switch
        // moved, so the frame after shows it.
        if settle(kind.options, values) {
            ui.ctx().request_repaint();
        }

        // Not the connection's: every connection's.
        ui.add_space(12.0);
        ui.separator();
        ui.add_space(4.0);
        ui.label(RichText::new("Settings").strong());
        ui.add_space(4.0);
        Grid::new("settings").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            settings.rows(ui, false);
        });

        ui.add_space(12.0);
        if let Some(error) = &self.error {
            ui.label(RichText::new(error).color(ui.visuals().error_fg_color));
            ui.add_space(4.0);
        }
        ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
            if ui.button("Connect").clicked() {
                connect = true;
            }
        });
        connect
    }

    /// The fields as a request, with every option checked against its spec
    /// and only the ones that differ from the default counted as set.
    fn request(&self) -> Result<Request, String> {
        let kind = self.kinds[self.selected];
        if self.address.trim().is_empty() {
            return Err("Missing address".to_string());
        }
        let mut options = Options::defaults(kind.options);
        for spec in kind.options {
            let value = self.values[self.selected]
                .get(spec.key)
                .map(String::as_str)
                .unwrap_or(spec.default);
            if value != spec.default {
                spec.check(value)?;
                options.set(spec.key, value);
            }
        }
        options.check_requirements(kind.options)?;
        Ok(Request {
            kind,
            address: self.address.trim().to_string(),
            user: self.user.trim().to_string(),
            domain: self.domain.trim().to_string(),
            password: (!self.password.is_empty()).then(|| self.password.clone()),
            options,
        })
    }
}

/// [`Options::settle`] over the dialog's own map; whether anything changed.
fn settle(specs: &[Spec], values: &mut BTreeMap<&'static str, String>) -> bool {
    let mut changed = false;
    for spec in specs {
        let fallback = {
            let on = |key: &str| values.get(key).is_some_and(|v| v.eq_ignore_ascii_case("true"));
            let value = values.get(spec.key).map(String::as_str).unwrap_or(spec.default);
            spec.stranded(value, on).map(|_| spec.fallback(on))
        };
        if let Some(value) = fallback {
            values.insert(spec.key, value.to_string());
            changed = true;
        }
    }
    changed
}

fn defaults(specs: &[Spec]) -> BTreeMap<&'static str, String> {
    specs.iter().map(|s| (s.key, s.default.to_string())).collect()
}

/// One option as the control its kind calls for.
fn option_row(ui: &mut Ui, spec: &'static Spec, values: &mut BTreeMap<&'static str, String>) {
    let label = ui.label(spec.label);
    if let Some(summary) = spec.summary_on(Platform::Linux) {
        label.on_hover_text(summary);
    }
    let value = values.entry(spec.key).or_insert_with(|| spec.default.to_string());
    match spec.ty {
        Type::Bool => {
            let mut on = value == "true";
            if ui.checkbox(&mut on, "").changed() {
                *value = if on { "true" } else { "false" }.to_string();
            }
        }
        Type::Choice => {
            // A choice whose switch is off is not offered at all.
            let offered: Vec<&options::Choice> = spec
                .choices
                .iter()
                .filter(|c| {
                    c.requires
                        .is_none_or(|key| values.get(key).map(String::as_str) == Some("true"))
                })
                .collect();
            let value = values.entry(spec.key).or_insert_with(|| spec.default.to_string());
            ComboBox::from_id_salt(spec.key)
                .selected_text(spec.label_for(value, Platform::Linux).to_string())
                .width(ui.available_width())
                .show_ui(ui, |ui| {
                    for choice in offered {
                        let row = ui.selectable_value(value, choice.value.to_string(), choice.label_on(Platform::Linux));
                        if let Some(summary) = choice.summary {
                            row.on_hover_text(summary);
                        }
                    }
                });
        }
        Type::Int | Type::Text => {
            ui.add(TextEdit::singleline(value).desired_width(f32::INFINITY));
        }
    }
    ui.end_row();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dialog_is_centred_across_and_near_the_top() {
        let window = egui::Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 800.0));
        assert_eq!(left_top(vec2(520.0, 580.0), window), pos2(380.0, 48.0));
        // Too short for the space above it: centred in what room there is.
        let short = egui::Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 640.0));
        assert_eq!(left_top(vec2(520.0, 580.0), short), pos2(380.0, 30.0));
        // Too small for it at all: a margin from the top left corner.
        let small = egui::Rect::from_min_size(pos2(0.0, 0.0), vec2(300.0, 200.0));
        assert_eq!(left_top(vec2(360.0, 580.0), small), pos2(8.0, 8.0));
    }
}
