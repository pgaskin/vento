// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The client's own settings: the phone's `AppSettings`, as far as this
//! window has any. One answer for every connection rather than a field of
//! one, and nothing saved, since there is nowhere this client saves.
//!
//! They are drawn in two places with one function: a section of the
//! connection dialog, which is where they are set before a session, and the
//! connection panel, where a session shows what it is running with and
//! takes a change at once. The session owns a copy while it runs and hands
//! it back when it ends, so what was switched mid-session is what the next
//! one starts with.

use super::view::ScaleMode;
use crate::backends::{AudioOutput, Hardware};
use egui::{RichText, Ui};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Send locally copied text to the far end: `KEY_CLIPBOARD_OUT`.
    pub clipboard_out: bool,
    /// Update the local clipboard with what the far end copies:
    /// `KEY_CLIPBOARD_IN`.
    pub clipboard_in: bool,
    /// In absolute mode, draw the far end's cursor shape into the picture
    /// under the local pointer, with the OS cursor hidden over the desktop,
    /// as the phone does, rather than making it the window's own cursor.
    pub software_cursor: bool,
    /// Hold the window manager's or the compositor's shortcuts off while a
    /// session has the focus and takes input (the X11 keyboard grab, or
    /// Wayland's shortcuts inhibitor), so that Alt+Tab and the rest reach
    /// the far end. Off, neither is ever taken and they stay this
    /// desktop's.
    pub grab_keyboard: bool,
    /// Decode video on the GPU where it can be, as `video` says. Off, the
    /// default, no device is opened and libva is not loaded. Like the rest,
    /// not saved: a run that wants it asks for it (`--hw-decode`) or
    /// switches it on here.
    pub hw_decode: bool,
    /// How video is decoded on the GPU while `hw_decode` is on: through
    /// VAAPI (`vaapi`, or `auto`, which is the same), or not at all (`off`,
    /// which leaves the box disabled). Command line only (`--video`).
    pub video: Hardware,
    /// Where the far end's sound plays: PipeWire's own stream where a
    /// daemon answers and ALSA's default device otherwise (`auto`), one of
    /// the two by name, or nowhere (`off`). Command line only (`--audio`);
    /// not drawn with the rest.
    pub audio_output: AudioOutput,
    /// The scale a session's main window starts at, and whether the window
    /// is made the desktop's size once that is known (`--scale
    /// none-resize`, with `ScaleMode::Original`). Command line only
    /// (`--scale`).
    pub scale: ScaleMode,
    pub fit_window: bool,
    /// Every session is view only, and the View only button and option
    /// cannot turn that off. Command line only (`--view-only`).
    pub view_only: bool,
    /// Every trust, security and message prompt is accepted without being
    /// shown, and nothing is pinned or remembered for it: a certificate the
    /// known hosts match is still taken as matching. Command line only
    /// (`--trust`).
    pub trust: bool,
    /// Every connection a backend makes through vento goes through a
    /// socketpair, with vento relaying it to the server, to test the
    /// backends on a socket that is not TCP. Command line only
    /// (`--dev-force-socketpair`, which the help leaves out); not drawn.
    pub force_socketpair: bool,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            clipboard_out: true,
            clipboard_in: true,
            software_cursor: false,
            grab_keyboard: true,
            hw_decode: false,
            video: Hardware::Auto,
            audio_output: AudioOutput::Auto,
            scale: ScaleMode::Auto,
            fit_window: false,
            view_only: false,
            trust: false,
            force_socketpair: false,
        }
    }
}

impl Settings {
    /// How video is decoded: `video` while hardware decoding is on, and
    /// off otherwise.
    pub fn hardware(&self) -> Hardware {
        if self.hw_decode { self.video } else { Hardware::Off }
    }

    /// The five as rows of a two-column grid (a label and a box each, the
    /// shape the dialog's and the panel's options are drawn in). In the
    /// `panel` the labels are weak, as the panel's are, and have no
    /// tooltips. True if one was changed this frame.
    pub fn rows(&mut self, ui: &mut Ui, panel: bool) -> bool {
        let mut changed = false;
        let label = |ui: &mut Ui, label: &str, summary: &str| {
            if panel {
                ui.label(RichText::new(label).weak());
            } else {
                ui.label(label).on_hover_text(summary);
            }
        };
        let mut row = |ui: &mut Ui, on: &mut bool, text: &str, summary: &str| {
            label(ui, text, summary);
            changed |= ui.checkbox(on, "").changed();
            ui.end_row();
        };
        row(
            ui,
            &mut self.clipboard_out,
            "Send clipboard",
            "Send locally copied text to the remote machine.",
        );
        row(
            ui,
            &mut self.clipboard_in,
            "Receive clipboard",
            "Update the local clipboard with copied text.",
        );
        // With the clipboard's, as the other switch about what crosses
        // between the two desktops: here, the keyboard's chords.
        row(
            ui,
            &mut self.grab_keyboard,
            "Grab keyboard",
            "Capture global keybindings and send them to the server while focused.",
        );
        row(
            ui,
            &mut self.software_cursor,
            "Software cursor",
            "Hide the native cursor and draw the cursor directly. Always enabled if the \
             remote machine uses a relative pointer.",
        );
        // A box like the others, decoding as `--video` says; disabled where
        // that is `off`.
        label(
            ui,
            "Hardware decoding",
            "Decode video using the GPU (VAAPI) if supported by the backend and local \
             hardware. Not supported by all backends, and may be slower on some hardware.",
        );
        let enabled = self.video != Hardware::Off;
        let mut on = self.hw_decode && enabled;
        if ui.add_enabled(enabled, egui::Checkbox::without_text(&mut on)).changed() {
            self.hw_decode = on;
            changed = true;
        }
        ui.end_row();
        changed
    }
}
