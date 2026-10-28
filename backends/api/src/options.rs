// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! Backend options, described well enough for a client to offer them
//! without knowing what they mean: `BackendOption` in Java. Each backend's
//! list is in `backends/modules/<name>/catalog.rs` beside its entry in the
//! [`catalog`](crate::catalog), and is the only copy; this module holds the
//! shape of a row, the shorthands the lists are written in, and the rows
//! several backends offer with the same wording. The phone's lists are
//! generated from them (`vento-codegen`, as a generated source of each
//! backend's module), worded for the phone; where a desktop needs different
//! wording (a phone vibrates for a bell, a window cannot), a row carries it
//! separately ([`Spec::desktop_summary`], [`Choice::desktop_label`]).
//!
//! The values are strings, because the backends' own configuration layers
//! take strings; [`Type`] says how to ask for a value, not how to store it.
//! A `-o key=value` on the command line and a row in a dialog are handled
//! the same way (the key checked against the backend's list, the value
//! against the type), which keeps both generic.
//!
//! The lists are `static` and the descriptions are `&'static str`, so a
//! dialog can hold a reference to a row without owning anything. The
//! per-backend helpers that convert a value for a library (a compression
//! level, an encodings string) are in that backend's adapter, beside the
//! call that uses it.

use crate::kind::Platform;
use std::collections::{BTreeMap, BTreeSet};

/// Which control to offer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Type {
    /// `"true"` / `"false"`.
    Bool,
    /// One of the choices.
    Choice,
    /// A decimal integer.
    Int,
    /// Free text.
    Text,
}

/// Whose setting it is: the connection's, the backend's, or the connection's
/// with the backend's value as its default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Connection,
    Global,
    Layered,
}

/// A value and its on-screen label. `summary` explains why to pick this one
/// over the others, where that needs explaining; `requires` is the key of a
/// `Bool` option this choice depends on, so a choice whose switch is off is
/// not offered at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Choice {
    pub value: &'static str,
    /// The label, worded for the phone.
    pub label: &'static str,
    /// The desktop's label where the phone's is phone-specific ("this
    /// phone"), or `None`. See [`Choice::label_on`].
    pub desktop_label: Option<&'static str>,
    pub summary: Option<&'static str>,
    pub requires: Option<&'static str>,
}

impl Choice {
    /// The label for the client showing it.
    pub fn label_on(&self, platform: Platform) -> &'static str {
        match platform {
            Platform::Android => self.label,
            Platform::Linux => self.desktop_label.unwrap_or(self.label),
        }
    }
}

/// The values another option must have for this one to be offered at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gate {
    pub key: &'static str,
    pub values: &'static [&'static str],
}

impl Gate {
    pub fn open(&self, value: &str) -> bool {
        self.values.contains(&value)
    }
}

/// One setting a backend understands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spec {
    /// The backend's own name for it; the map key.
    pub key: &'static str,
    /// One line, for a preference title.
    pub label: &'static str,
    /// The longer explanation, worded for the phone.
    pub summary: Option<&'static str>,
    /// The same explanation where a desktop needs different wording (a
    /// phone vibrates for a bell and a window cannot), or `None` where the
    /// phone's wording fits both. See [`Spec::summary_on`].
    pub desktop_summary: Option<&'static str>,
    pub ty: Type,
    /// For `Choice`, in display order; empty otherwise.
    pub choices: &'static [Choice],
    /// What the backend does when nothing is set.
    pub default: &'static str,
    pub scope: Scope,
    /// Whether `set_option` does anything mid-session.
    pub live: bool,
    pub gate: Option<Gate>,
    /// Set only on a client's command line (`-o`): not a row in a form, so
    /// the phone, which has no command line, leaves it out altogether.
    pub command_line_only: bool,
}

impl Spec {
    /// The explanation worded for the client showing it.
    pub fn summary_on(&self, platform: Platform) -> Option<&'static str> {
        match platform {
            Platform::Android => self.summary,
            Platform::Linux => self.desktop_summary.or(self.summary),
        }
    }

    /// For a `Choice`: the label for `value` (a `&'static str` from the
    /// list) in the client showing it, or the value itself when it is not
    /// one of the choices.
    pub fn label_for<'a>(&self, value: &'a str, platform: Platform) -> &'a str {
        self.choices
            .iter()
            .find(|c| c.value == value)
            .map(|c| c.label_on(platform))
            .unwrap_or(value)
    }

    /// Whether this option accepts `value`, with an error message when not.
    pub fn check(&self, value: &str) -> Result<(), String> {
        match self.ty {
            Type::Bool => match value {
                "true" | "false" => Ok(()),
                _ => Err(format!("{}: true or false, not {value:?}", self.key)),
            },
            Type::Int => value
                .parse::<i64>()
                .map(|_| ())
                .map_err(|_| format!("{}: a whole number, not {value:?}", self.key)),
            Type::Choice => {
                if self.choices.iter().any(|c| c.value == value) {
                    Ok(())
                } else {
                    let values: Vec<&str> = self.choices.iter().map(|c| c.value).collect();
                    Err(format!(
                        "{}: one of {}, not {value:?}",
                        self.key,
                        values.join(", ")
                    ))
                }
            }
            Type::Text => Ok(()),
        }
    }

    /// For a `Choice`: the choice `value` names, if it requires a switch
    /// that `on` says is off (a value left over after its switch was turned
    /// off, or given without it). Such a value is not offered, and the
    /// backend would not honour it either (TigerVNC's `Encoding=h264`
    /// without `H264` negotiates something else), so it must not stay
    /// selected.
    pub fn stranded(&self, value: &str, on: impl Fn(&str) -> bool) -> Option<&'static Choice> {
        let choices: &'static [Choice] = self.choices;
        choices
            .iter()
            .find(|c| c.value == value)
            .filter(|c| c.requires.is_some_and(|key| !on(key)))
    }

    /// The fallback for a stranded choice: the option's default, as if it
    /// had never been picked, or the first choice on offer if the default is
    /// stranded too (no list here has such a default).
    pub fn fallback(&self, on: impl Fn(&str) -> bool) -> &'static str {
        if self.stranded(self.default, &on).is_none() {
            return self.default;
        }
        let choices: &'static [Choice] = self.choices;
        choices
            .iter()
            .find(|c| c.requires.is_none_or(&on))
            .map_or(self.default, |c| c.value)
    }
}

pub(crate) const fn choice(value: &'static str, label: &'static str) -> Choice {
    Choice {
        value,
        label,
        desktop_label: None,
        summary: None,
        requires: None,
    }
}

pub(crate) const fn described(value: &'static str, label: &'static str, summary: &'static str) -> Choice {
    Choice {
        value,
        label,
        desktop_label: None,
        summary: Some(summary),
        requires: None,
    }
}

pub(crate) const fn requiring(value: &'static str, label: &'static str, requires: &'static str) -> Choice {
    Choice {
        value,
        label,
        desktop_label: None,
        summary: None,
        requires: Some(requires),
    }
}

// Sets eight fields at once; the lists use the shorthands below. They and
// the shared rows after them are crate-private: the lists that use them are
// compiled here, under `catalog`.
#[allow(clippy::too_many_arguments)]
pub(crate) const fn spec(
    key: &'static str,
    label: &'static str,
    summary: Option<&'static str>,
    ty: Type,
    choices: &'static [Choice],
    default: &'static str,
    scope: Scope,
    live: bool,
) -> Spec {
    Spec {
        key,
        label,
        summary,
        desktop_summary: None,
        ty,
        choices,
        default,
        scope,
        live,
        gate: None,
        command_line_only: false,
    }
}

pub(crate) const fn bool(
    key: &'static str,
    label: &'static str,
    summary: &'static str,
    default: bool,
    scope: Scope,
    live: bool,
) -> Spec {
    spec(
        key,
        label,
        Some(summary),
        Type::Bool,
        &[],
        if default { "true" } else { "false" },
        scope,
        live,
    )
}

pub(crate) const fn integer(
    key: &'static str,
    label: &'static str,
    summary: &'static str,
    default: &'static str,
    scope: Scope,
    live: bool,
) -> Spec {
    spec(key, label, Some(summary), Type::Int, &[], default, scope, live)
}

pub(crate) const fn text(
    key: &'static str,
    label: &'static str,
    summary: &'static str,
    default: &'static str,
    scope: Scope,
    live: bool,
) -> Spec {
    spec(key, label, Some(summary), Type::Text, &[], default, scope, live)
}

pub(crate) const fn choices(
    key: &'static str,
    label: &'static str,
    summary: Option<&'static str>,
    default: &'static str,
    scope: Scope,
    live: bool,
    choices: &'static [Choice],
) -> Spec {
    spec(key, label, summary, Type::Choice, choices, default, scope, live)
}

/// The same option, set only on a command line ([`Spec::command_line_only`]).
pub(crate) const fn command_line_only(spec: Spec) -> Spec {
    Spec {
        command_line_only: true,
        ..spec
    }
}

/// The same option, offered only when `key` has one of `values`.
pub(crate) const fn when(spec: Spec, key: &'static str, values: &'static [&'static str]) -> Spec {
    Spec {
        gate: Some(Gate { key, values }),
        ..spec
    }
}

pub(crate) const VIEW_ONLY: Spec = bool(
    "ViewOnly",
    "View only",
    "Do not send input or clipboard events.",
    false,
    Scope::Connection,
    true,
);

pub(crate) const BELL: Spec = Spec {
    desktop_summary: Some("Show bell icon in the window title when server rings bell."),
    ..bool(
        "AcceptBell",
        "Bell",
        "Vibrate when the server rings the bell.",
        true,
        Scope::Global,
        false,
    )
};

pub(crate) const SHARED: Spec = bool(
    "Shared",
    "Shared",
    "Do not ask the server to disconnect other clients.",
    true,
    Scope::Connection,
    false,
);

pub(crate) const ANONYMOUS_TLS: Spec = bool(
    "AnonymousTLS",
    "Unverified encryption",
    "Ask before using encryption without a peer certificate.",
    true,
    Scope::Connection,
    false,
);

pub(crate) const COMPRESSION_LEVELS: &[Choice] = &[
    choice("auto", "Automatic"),
    choice("1", "1 (fastest)"),
    choice("3", "3"),
    choice("6", "6 (balanced)"),
    choice("9", "9 (smallest)"),
];

pub(crate) const DESKTOP_SIZES: &[Choice] = &[
    choice("remote", "Do not change"),
    choice("device", "Match local device"),
    choice("1920x1200", "1920 × 1200"),
    choice("1920x1080", "1920 × 1080"),
    choice("1600x1000", "1600 × 1000"),
    choice("1366x768", "1366 × 768"),
    choice("1280x800", "1280 × 800"),
    choice("2560x1440", "2560 × 1440"),
];

pub(crate) const MONITOR_COUNTS: &[Choice] = &[
    choice("1", "One"),
    choice("2", "Two"),
    choice("3", "Three"),
];

/// A backend's settings: every key from its list at its default, with any
/// values set on top. Remembers which keys were set, because one backend
/// (RealVNC) treats an explicitly chosen colour depth differently from the
/// default.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Options {
    values: BTreeMap<String, String>,
    set: BTreeSet<String>,
}

impl Options {
    pub fn defaults(specs: &[Spec]) -> Options {
        Options {
            values: specs
                .iter()
                .map(|s| (s.key.to_string(), s.default.to_string()))
                .collect(),
            set: BTreeSet::new(),
        }
    }

    pub fn set(&mut self, key: &str, value: &str) {
        self.values.insert(key.to_string(), value.to_string());
        self.set.insert(key.to_string());
    }

    /// The value, or empty when there is none, which every reader here
    /// expects for an unset option.
    pub fn get(&self, key: &str) -> &str {
        self.values.get(key).map(String::as_str).unwrap_or("")
    }

    /// Whether this was set explicitly rather than left at the default.
    pub fn is_set(&self, key: &str) -> bool {
        self.set.contains(key)
    }

    /// Resets every choice whose required switch is off to its
    /// [`Spec::fallback`], and returns the changed keys for a live session to
    /// pass on. A fallback was not chosen, so it is not counted as set. The
    /// dialog and the live panel both call this whenever a switch changes.
    pub fn settle(&mut self, specs: &[Spec]) -> Vec<&'static str> {
        let mut changed = Vec::new();
        for spec in specs {
            if spec.stranded(self.get(spec.key), |key| self.bool(key)).is_some() {
                let value = spec.fallback(|key| self.bool(key));
                self.values.insert(spec.key.to_string(), value.to_string());
                self.set.remove(spec.key);
                changed.push(spec.key);
            }
        }
        changed
    }

    /// An error for the first choice given without its switch: the command
    /// line refuses `-o Encoding=h264` without `-o H264=true` rather than
    /// guess which of the two was meant.
    pub fn check_requirements(&self, specs: &[Spec]) -> Result<(), String> {
        for spec in specs {
            if let Some(choice) = spec.stranded(self.get(spec.key), |key| self.bool(key)) {
                let needs = choice.requires.unwrap_or_default();
                return Err(format!("{}={} needs {needs}=true", spec.key, choice.value));
            }
        }
        Ok(())
    }

    /// `Boolean.parseBoolean`: true for `"true"` in any case, false for
    /// everything else, including an unset key.
    pub fn bool(&self, key: &str) -> bool {
        self.get(key).eq_ignore_ascii_case("true")
    }

    /// A whole number, or `default` if the value is not one.
    pub fn int(&self, key: &str, default: i64) -> i64 {
        self.get(key).trim().parse().unwrap_or(default)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.values.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }
}

/// `key=value`, as `-o` takes it. Split at the first `=`, so a value may
/// contain one; a missing `=` is an error.
pub fn parse_assignment(text: &str) -> Result<(&str, &str), String> {
    match text.split_once('=') {
        Some((key, value)) if !key.trim().is_empty() => Ok((key.trim(), value)),
        _ => Err(format!("{text:?} is not key=value")),
    }
}

/// The spec for `key` in `specs`, or an error listing the valid keys.
pub fn find<'a>(specs: &'a [Spec], key: &str) -> Result<&'a Spec, String> {
    specs.iter().find(|s| s.key == key).ok_or_else(|| {
        let keys: Vec<&str> = specs.iter().map(|s| s.key).collect();
        format!("no option {key:?}; the options are {}", keys.join(", "))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rows of every type, as in a backend's list: a choice with a value
    /// behind a switch, a choice with a desktop label, a gated row, a
    /// number, free text, and three of the shared rows.
    static FIXTURE: &[Spec] = &[
        choices(
            "Encoding",
            "Encoding",
            None,
            "tight",
            Scope::Layered,
            true,
            &[choice("tight", "Tight"), requiring("h264", "H.264", "H264"), choice("rre", "RRE")],
        ),
        choices(
            "Sound",
            "Sound",
            None,
            "off",
            Scope::Layered,
            false,
            &[
                choice("off", "Off"),
                Choice {
                    desktop_label: Some("This computer"),
                    ..choice("local", "This phone")
                },
            ],
        ),
        when(text("Server", "Server", "Anything.", "", Scope::Layered, false), "Sound", &["local"]),
        integer("KeepAlive", "Keep-alive", "Seconds.", "30", Scope::Global, false),
        bool("H264", "H.264", "Offer it.", false, Scope::Global, true),
        VIEW_ONLY,
        SHARED,
        BELL,
    ];

    #[test]
    fn assignments_split_at_the_first_equals() {
        assert_eq!(parse_assignment("Encoding=zrle"), Ok(("Encoding", "zrle")));
        assert_eq!(parse_assignment("Key=a=b"), Ok(("Key", "a=b")));
        assert_eq!(parse_assignment("Compression="), Ok(("Compression", "")));
        assert!(parse_assignment("=x").is_err());
        assert!(parse_assignment("novalue").is_err());
    }

    #[test]
    fn a_value_is_checked_against_its_type() {
        let encoding = find(FIXTURE, "Encoding").unwrap();
        assert!(encoding.check("rre").is_ok());
        assert!(encoding.check("zrle").unwrap_err().contains("one of"));
        assert_eq!(encoding.label_for("rre", Platform::Linux), "RRE");
        assert_eq!(encoding.label_for("nonsense", Platform::Linux), "nonsense");
        let sound = find(FIXTURE, "Sound").unwrap();
        assert_eq!(sound.label_for("local", Platform::Android), "This phone");
        assert_eq!(sound.label_for("local", Platform::Linux), "This computer");
        let bell = find(FIXTURE, "AcceptBell").unwrap();
        assert!(bell.summary_on(Platform::Android).unwrap().starts_with("Vibrate"));
        assert!(bell.summary_on(Platform::Linux).unwrap().starts_with("Show bell icon"));
        let shared = find(FIXTURE, "Shared").unwrap();
        assert!(shared.check("true").is_ok());
        assert!(shared.check("yes").is_err());
        let keep = find(FIXTURE, "KeepAlive").unwrap();
        assert!(keep.check("45").is_ok());
        assert!(keep.check("soon").is_err());
        assert!(find(FIXTURE, "Quality").unwrap_err().contains("the options are Encoding, Sound"));
        assert!(find(FIXTURE, "Server").unwrap().check("anything").is_ok());
    }

    #[test]
    fn defaults_are_the_lists_and_overrides_are_remembered() {
        let mut options = Options::defaults(FIXTURE);
        assert_eq!(options.get("Encoding"), "tight");
        assert!(!options.is_set("Encoding"));
        assert!(options.bool("Shared"));
        assert_eq!(options.int("KeepAlive", 0), 30);
        options.set("Encoding", "rre");
        assert!(options.is_set("Encoding"));
        assert_eq!(options.get("Encoding"), "rre");
        assert_eq!(options.get("Nothing"), "");
        assert!(!options.bool("Nothing"));
        assert!(options.int("Encoding", 7) == 7);
    }

    #[test]
    fn a_gate_opens_on_its_values() {
        let gate = find(FIXTURE, "Server").unwrap().gate.unwrap();
        assert_eq!(gate.key, "Sound");
        assert!(gate.open("local"));
        assert!(!gate.open("off"));
    }

    #[test]
    fn a_choice_whose_switch_is_off_falls_back_to_the_default() {
        let mut options = Options::defaults(FIXTURE);
        options.set("H264", "true");
        options.set("Encoding", "h264");
        assert!(options.settle(FIXTURE).is_empty(), "its switch is on");
        assert_eq!(options.check_requirements(FIXTURE), Ok(()));
        options.set("H264", "false");
        assert_eq!(options.check_requirements(FIXTURE), Err("Encoding=h264 needs H264=true".to_string()));
        assert_eq!(options.settle(FIXTURE), ["Encoding"]);
        assert_eq!(options.get("Encoding"), "tight");
        assert!(!options.is_set("Encoding"), "a fallback is not a set value");
        assert!(options.settle(FIXTURE).is_empty(), "and settled stays settled");
        options.set("Encoding", "rre");
        assert!(options.settle(FIXTURE).is_empty());
        assert_eq!(options.get("Encoding"), "rre");
    }
}
