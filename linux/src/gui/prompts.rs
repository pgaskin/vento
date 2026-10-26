// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The questions a connection stops and asks, as modal dialogs over the
//! session that has stopped for them: `PromptDialogs.java`, in egui.
//!
//! Every one of them owes an answer (the backend's thread is blocked until
//! it gets one), so closing a dialog by any route is a no, and the session
//! queues what arrives while one is already up. Nothing is written from here:
//! a certificate is put as the phone's `NativeBackend.onTrustNeeded` puts it,
//! with why it is being asked about and a *remember server* box, and the
//! session hands the answer to `api::trust::answered`, which pins what was
//! accepted with the box ticked; RealVNC's own *remember* goes to its core.

use crate::backend::Credentials;
use crate::events::{Encryption, Event, Identity, Severity};
use egui::{Align, Id, Key, Layout, Modal, RichText, TextEdit, Ui};

/// A prompt event, held until it is answered.
#[derive(Clone, Debug)]
pub enum Prompt {
    Credentials {
        needs_user: bool,
    },
    /// A certificate the [known hosts](crate::known_hosts) did not match,
    /// with why: never seen, seen under another address, or changed.
    Trust {
        fingerprint: String,
        identity: Identity,
        matching_address: String,
    },
    Unverified {
        why: String,
    },
    Question {
        title: String,
        text: String,
    },
    Security {
        identity: Identity,
        encryption: Encryption,
        signature: String,
        catchphrase: String,
        matching_address: String,
    },
    Message {
        text: String,
        severity: Severity,
        question: bool,
        confirm_label: String,
    },
}

impl Prompt {
    /// The prompt an event asks for. Not a trust event, which is put to the
    /// known hosts first and becomes a [`Prompt::Trust`] only if they do not
    /// answer it.
    pub fn from_event(event: &Event) -> Option<Prompt> {
        Some(match event {
            Event::CredentialsNeeded { needs_user } => Prompt::Credentials {
                needs_user: *needs_user,
            },
            Event::Unverified { why } => Prompt::Unverified { why: why.clone() },
            Event::Question { title, text } => Prompt::Question {
                title: title.clone(),
                text: text.clone(),
            },
            Event::SecurityNeeded {
                identity,
                encryption,
                signature,
                catchphrase,
                matching_address,
            } => Prompt::Security {
                identity: *identity,
                encryption: *encryption,
                signature: signature.clone(),
                catchphrase: catchphrase.clone(),
                matching_address: matching_address.clone(),
            },
            Event::Message {
                text,
                severity,
                question,
                confirm_label,
            } => Prompt::Message {
                text: text.clone(),
                severity: *severity,
                question: *question,
                confirm_label: confirm_label.clone(),
            },
            _ => return None,
        })
    }
}

/// What a dialog answered with, to be handed to the backend.
#[derive(Clone)]
pub enum Answer {
    Credentials(Option<Credentials>),
    Trust(bool),
    /// A [`Prompt::Trust`]'s: whether to go on, and whether to pin
    /// `fingerprint` for the address.
    Pin {
        accept: bool,
        remember: bool,
        fingerprint: String,
    },
    Security { accept: bool, remember: bool },
}

/// What is typed into a dialog, kept across frames and reset per prompt.
#[derive(Default)]
pub struct Form {
    pub user: String,
    pub password: String,
    pub remember: bool,
    /// Whether the fields have been set up for the prompt on screen.
    ready: bool,
    /// Whether the first field has been given focus, done once per prompt
    /// so that a click elsewhere in the dialog is not undone.
    focused: bool,
}

impl Form {
    /// Fresh fields for a new prompt.
    pub fn reset(&mut self) {
        *self = Form::default();
    }

    fn prepare(&mut self, prompt: &Prompt, user: &str) {
        if self.ready {
            return;
        }
        self.ready = true;
        match prompt {
            Prompt::Credentials { .. } => self.user = user.to_string(),
            // Ticked unless the identity has changed, as on the phone: a
            // changed one is the case for saying yes once and looking again.
            Prompt::Trust { identity, .. } | Prompt::Security { identity, .. } => {
                self.remember = *identity != Identity::Changed
            }
            _ => {}
        }
    }
}

/// The width of a prompt's content, in points.
pub const WIDTH: f32 = 380.0;

/// Draws `prompt` as a modal and returns its answer on the frame it is
/// given. `address` titles the dialogs that are about the far end, with the
/// connecting screen's `status` under it, since the modal covers the screen;
/// `user` is what the credentials form starts with.
pub fn show(
    ctx: &egui::Context,
    prompt: &Prompt,
    form: &mut Form,
    address: &str,
    status: Option<&str>,
    user: &str,
) -> Option<Answer> {
    form.prepare(prompt, user);
    let modal = Modal::new(Id::new("prompt")).show(ctx, |ui| {
        ui.set_width(WIDTH);
        match prompt {
            Prompt::Credentials { needs_user } => credentials(ui, form, address, status, *needs_user),
            Prompt::Trust {
                fingerprint,
                identity,
                matching_address,
            } => {
                // The phone's trust prompt: encrypted, the fingerprint as
                // the signature, and no catchphrase.
                identity_prompt(
                    ui,
                    form,
                    (address, status),
                    *identity,
                    Encryption::Encrypted,
                    fingerprint,
                    "",
                    matching_address,
                )
                .map(|(accept, remember)| Answer::Pin {
                        accept,
                        remember,
                        fingerprint: fingerprint.clone(),
                    })
            }
            Prompt::Unverified { why } => {
                heading(ui, "Connect anyway?", address, status);
                ui.label("This server could not be verified.");
                ui.add_space(6.0);
                ui.label(why);
                yes_no(ui, "Continue", "Cancel", true).map(Answer::Trust)
            }
            Prompt::Question { title, text } => {
                heading(ui, if title.is_empty() { "Question" } else { title }, address, status);
                ui.label(text);
                yes_no(ui, "Continue", "Cancel", true).map(Answer::Trust)
            }
            Prompt::Security {
                identity,
                encryption,
                signature,
                catchphrase,
                matching_address,
            } => identity_prompt(
                ui,
                form,
                (address, status),
                *identity,
                *encryption,
                signature,
                catchphrase,
                matching_address,
            )
            .map(|(accept, remember)| Answer::Security { accept, remember }),
            Prompt::Message {
                text,
                severity,
                question,
                confirm_label,
            } => {
                let title = match severity {
                    Severity::Information => "Message",
                    Severity::Question => "Question",
                    Severity::Warning => "Warning",
                    Severity::Error => "Error",
                };
                heading(ui, title, address, status);
                ui.label(text);
                let confirm = if confirm_label.is_empty() {
                    "OK"
                } else {
                    confirm_label
                };
                if *question {
                    yes_no(ui, confirm, "Cancel", true).map(Answer::Trust)
                } else {
                    ui.add_space(12.0);
                    let mut answer = None;
                    ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                        if ui.button(confirm).clicked() || enter(ui) {
                            answer = Some(Answer::Trust(true));
                        }
                    });
                    answer
                }
            }
        }
    });
    let dismissed = modal.should_close();
    let answer = modal.inner;
    if answer.is_some() {
        form.reset();
        return answer;
    }
    // Escape or a click outside is a no: the backend is waiting on one.
    if dismissed {
        form.reset();
        return Some(match prompt {
            Prompt::Credentials { .. } => Answer::Credentials(None),
            Prompt::Trust { fingerprint, .. } => Answer::Pin {
                accept: false,
                remember: false,
                fingerprint: fingerprint.clone(),
            },
            Prompt::Security { .. } => Answer::Security {
                accept: false,
                remember: false,
            },
            _ => Answer::Trust(false),
        });
    }
    None
}

/// The title, and under it the address and the connecting screen's status.
fn heading(ui: &mut Ui, title: &str, address: &str, status: Option<&str>) {
    ui.heading(title);
    if !address.is_empty() {
        ui.label(RichText::new(address).weak());
    }
    if let Some(status) = status {
        ui.label(status);
    }
    ui.add_space(8.0);
}

/// The identity question, `PromptDialogs.trust`: why it is asked, whether the
/// link is encrypted, what identifies the far end, and *remember server*.
/// Both RealVNC's security prompt and a certificate the known hosts did not
/// match are this one; what was answered, and whether to remember it.
#[allow(clippy::too_many_arguments)]
fn identity_prompt(
    ui: &mut Ui,
    form: &mut Form,
    (address, status): (&str, Option<&str>),
    identity: Identity,
    encryption: Encryption,
    signature: &str,
    catchphrase: &str,
    matching_address: &str,
) -> Option<(bool, bool)> {
    heading(ui, "Connect anyway?", address, status);
    // OK, PRESHARED and ARD say nothing: this prompt is up because something
    // else about the connection is worth asking about.
    let why = match identity {
        Identity::Changed => {
            "This server's identity has changed since you last connected.".to_string()
        }
        Identity::MatchesAnother => format!(
            "This server's identity is the same as {}, which you have connected to before.",
            if matching_address.is_empty() {
                "a different address"
            } else {
                matching_address
            }
        ),
        Identity::New => "You have not connected to this server before.".to_string(),
        Identity::Missing => "This server does not have a verifiable identity.".to_string(),
        Identity::Ok | Identity::Preshared | Identity::Ard => String::new(),
    };
    if !why.is_empty() {
        ui.label(why);
        ui.add_space(6.0);
    }
    if encryption == Encryption::UnencryptedWarn {
        ui.label("The connection is not encrypted.");
        ui.add_space(6.0);
    }
    if !catchphrase.is_empty() {
        ui.label(format!("Catchphrase: {catchphrase}"));
    }
    if !signature.is_empty() {
        ui.label(format!("Signature: {signature}"));
    }
    ui.add_space(6.0);
    ui.checkbox(&mut form.remember, "Remember server");
    // Enter goes on, as in every other prompt, except past a changed
    // identity: that is the one a key meant for the far end, pressed as
    // the prompt came up, must not say yes to.
    yes_no(ui, "Continue", "Cancel", identity != Identity::Changed).map(|accept| (accept, form.remember))
}

/// Only the fields the scheme actually has: a scheme with no user name gets
/// no box for one, since the answer would go nowhere.
fn credentials(ui: &mut Ui, form: &mut Form, address: &str, status: Option<&str>, needs_user: bool) -> Option<Answer> {
    heading(ui, "Authentication", address, status);
    let mut submit = false;
    if needs_user {
        ui.label("Username");
        let user = ui.add(TextEdit::singleline(&mut form.user).desired_width(f32::INFINITY));
        if user.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
            submit = true;
        }
        ui.add_space(6.0);
    }
    ui.label("Password");
    let password = ui.add(
        TextEdit::singleline(&mut form.password)
            .password(true)
            .desired_width(f32::INFINITY),
    );
    if !form.focused {
        form.focused = true;
        password.request_focus();
    }
    if password.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
        submit = true;
    }
    // Enter in a field is that field's (above), not the buttons'.
    match yes_no(ui, "Connect", "Cancel", false) {
        Some(true) => submit = true,
        Some(false) => return Some(Answer::Credentials(None)),
        None => {}
    }
    submit.then(|| {
        Answer::Credentials(Some(Credentials {
            user: if needs_user { form.user.clone() } else { String::new() },
            domain: String::new(),
            password: form.password.clone(),
        }))
    })
}

/// Two buttons, right-aligned; what was pressed, if anything. Enter is the
/// first where `enter_says_yes`, and Escape is left to the modal, which
/// makes it a no (`show`).
fn yes_no(ui: &mut Ui, yes: &str, no: &str, enter_says_yes: bool) -> Option<bool> {
    ui.add_space(12.0);
    let mut answer = None;
    ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
        if ui.button(yes).clicked() {
            answer = Some(true);
        }
        if ui.button(no).clicked() {
            answer = Some(false);
        }
    });
    if answer.is_none() && enter_says_yes && enter(ui) {
        answer = Some(true);
    }
    answer
}

/// Enter, pressed (not repeating: a held key that was on its way to the far
/// end is not an answer to a prompt that came up under it) while no widget
/// has the focus. A focused button takes Enter as its own click, and a
/// field as its own.
fn enter(ui: &Ui) -> bool {
    ui.memory(|m| m.focused().is_none())
        && ui.input(|i| {
            i.events.iter().any(|e| {
                matches!(
                    e,
                    egui::Event::Key {
                        key: Key::Enter,
                        pressed: true,
                        repeat: false,
                        ..
                    }
                )
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What `prompt` answers to `key`, pressed on its second frame (the
    /// first lays it out and gives the first field the focus). With `held`
    /// the key went down before the prompt came up, and what arrives with
    /// the prompt up is its repeat.
    fn answer(prompt: &Prompt, key: Key, held: bool) -> Option<Answer> {
        let ctx = egui::Context::default();
        let mut form = Form::default();
        let press = |repeat| egui::RawInput {
            events: vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        };
        let mut frame = |input, prompt: Option<&Prompt>| {
            let mut answered = None;
            let mut out = ctx.run_ui(input, |ui| {
                answered = prompt.and_then(|p| show(ui.ctx(), p, &mut form, "far", None, ""));
            });
            out.textures_delta.clear();
            answered
        };
        if held {
            assert!(frame(press(false), None).is_none());
        }
        assert!(frame(egui::RawInput::default(), Some(prompt)).is_none());
        frame(press(held), Some(prompt))
    }

    fn yes(answer: Option<Answer>) -> Option<bool> {
        match answer? {
            Answer::Trust(accept) | Answer::Pin { accept, .. } | Answer::Security { accept, .. } => Some(accept),
            Answer::Credentials(given) => Some(given.is_some()),
        }
    }

    fn trust(identity: Identity) -> Prompt {
        Prompt::Trust {
            fingerprint: "ab:cd".into(),
            identity,
            matching_address: String::new(),
        }
    }

    #[test]
    fn enter_says_yes_and_escape_no_to_every_prompt() {
        let question = Prompt::Question {
            title: String::new(),
            text: "Go on?".into(),
        };
        let message = |question| Prompt::Message {
            text: "Hello".into(),
            severity: Severity::Information,
            question,
            confirm_label: String::new(),
        };
        let prompts = [
            question,
            Prompt::Unverified { why: "self-signed".into() },
            message(true),
            message(false),
            trust(Identity::New),
            Prompt::Security {
                identity: Identity::New,
                encryption: Encryption::Encrypted,
                signature: "11-22".into(),
                catchphrase: "a b c".into(),
                matching_address: String::new(),
            },
            Prompt::Credentials { needs_user: false },
            Prompt::Credentials { needs_user: true },
        ];
        for prompt in &prompts {
            assert_eq!(yes(answer(prompt, Key::Enter, false)), Some(true), "{prompt:?}");
            if !matches!(prompt, Prompt::Message { question: false, .. }) {
                assert_eq!(yes(answer(prompt, Key::Escape, false)), Some(false), "{prompt:?}");
            }
            // A repeat of a key held from before is no answer.
            if !matches!(prompt, Prompt::Credentials { .. }) {
                assert_eq!(yes(answer(prompt, Key::Enter, true)), None, "{prompt:?}");
            }
        }
    }

    #[test]
    fn enter_does_not_accept_a_changed_identity() {
        assert_eq!(yes(answer(&trust(Identity::Changed), Key::Enter, false)), None);
        assert_eq!(yes(answer(&trust(Identity::Changed), Key::Escape, false)), Some(false));
    }
}
