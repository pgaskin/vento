// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! Test cases for the rules every client applies the same way. The
//! credentials and known-hosts cases are written out here, since both
//! clients apply those rules from this crate and only these tests use them.
//! The options cases are in `options.json` beside this file, which the Java
//! tests in `backends/platform/android/javatest` also read, for the only
//! rule with a Java copy (`BackendOption.settle`, which the option screens
//! run during editing), so that the two copies cannot drift apart without a
//! test failing.

use serde_json::Value;
use std::path::PathBuf;
use api::Credentials;
use api::credentials::{fill_user, rdp_identity, split_domain};
use api::events::Identity;
use api::options::{Choice, Options, Scope, Spec, Type};
use api::trust::{self, Verdict};

// How a credentials prompt's answer is read, for every client. A name is
// trimmed everywhere, so one that is only spaces counts as empty.

/// A typed user name, and the expected `(domain, user)`. A backslash comes
/// first, and the rest is kept whole; an address at a common email domain is
/// a Microsoft account; any other `@` name is split.
const SPLIT_DOMAIN: &[(&str, &str, &str)] = &[
    ("CORP\\alice", "CORP", "alice"),
    ("alice@corp.example", "corp.example", "alice"),
    (" alice ", "", "alice"),
    ("", "", ""),
    ("@odd", "", "@odd"),
    ("alice@", "", "alice"),
    ("x@y@z", "y@z", "x"),
    ("a\\b\\c", "a", "b\\c"),
    ("CORP\\alice@x", "CORP", "alice@x"),
    (".\\alice", ".", "alice"),
    ("\\alice", "", "alice"),
    ("DOM\\", "DOM", ""),
    // Microsoft accounts, typed with their domain.
    ("MicrosoftAccount\\someone@example.com", "MicrosoftAccount", "someone@example.com"),
    ("microsoftaccount\\someone@corp.example", "microsoftaccount", "someone@corp.example"),
    ("CORP\\someone@outlook.com", "CORP", "someone@outlook.com"),
    (".\\x@y", ".", "x@y"),
    (".\\someone@gmail.com", ".", "someone@gmail.com"),
    // Addresses at common email domains, taken to be Microsoft accounts.
    ("someone@outlook.com", "MicrosoftAccount", "someone@outlook.com"),
    (" someone@Outlook.COM ", "MicrosoftAccount", "someone@Outlook.COM"),
    ("someone@outlook.co.uk", "MicrosoftAccount", "someone@outlook.co.uk"),
    ("someone@outlook.de", "MicrosoftAccount", "someone@outlook.de"),
    ("someone@hotmail.com", "MicrosoftAccount", "someone@hotmail.com"),
    ("someone@hotmail.fr", "MicrosoftAccount", "someone@hotmail.fr"),
    ("someone@live.com.au", "MicrosoftAccount", "someone@live.com.au"),
    ("someone@msn.com", "MicrosoftAccount", "someone@msn.com"),
    ("someone@passport.com", "MicrosoftAccount", "someone@passport.com"),
    ("someone@gmail.com", "MicrosoftAccount", "someone@gmail.com"),
    ("someone@googlemail.com", "MicrosoftAccount", "someone@googlemail.com"),
    ("someone@yahoo.co.jp", "MicrosoftAccount", "someone@yahoo.co.jp"),
    ("someone@icloud.com", "MicrosoftAccount", "someone@icloud.com"),
    ("someone@me.com", "MicrosoftAccount", "someone@me.com"),
    ("someone@aol.com", "MicrosoftAccount", "someone@aol.com"),
    ("someone@proton.me", "MicrosoftAccount", "someone@proton.me"),
    ("someone@gmx.net", "MicrosoftAccount", "someone@gmx.net"),
    // Any other domain, a subdomain or a longer name included, is split.
    ("someone@example.com", "example.com", "someone"),
    ("someone@mail.outlook.com", "mail.outlook.com", "someone"),
    ("someone@outlook.corp.example", "outlook.corp.example", "someone"),
    ("someone@gmail.com.example", "gmail.com.example", "someone"),
    ("someone@outlook.company", "outlook.company", "someone"),
    ("someone@myoutlook.com", "myoutlook.com", "someone"),
    ("user@DESKTOP-12B9288", "DESKTOP-12B9288", "user"),
    ("x@gmail.com@y", "gmail.com@y", "x"),
    ("@gmail.com", "", "@gmail.com"),
];

/// An RDP prompt's answer given the session's own domain and user (already
/// split): `(session domain, session user, answer user, answer domain,
/// domain, user)`. The answer domain is one given separately from the name,
/// which only the Linux client's `--domain` provides; empty otherwise.
const RDP_IDENTITY: &[(&str, &str, &str, &str, &str, &str)] = &[
    ("CORP", "proto", "", "", "CORP", "proto"),
    ("CORP", "proto", "   ", "", "CORP", "proto"),
    ("CORP", "proto", "proto", "", "CORP", "proto"),
    ("CORP", "proto", "alice", "", "CORP", "alice"),
    ("CORP", "proto", " alice ", "", "CORP", "alice"),
    ("CORP", "proto", "LAB\\alice", "", "LAB", "alice"),
    ("CORP", "proto", "alice@lab.example", "", "lab.example", "alice"),
    ("CORP", "proto", ".\\alice", "", ".", "alice"),
    ("", "proto", "alice", "", "", "alice"),
    ("", "", "", "", "", ""),
    ("CORP", "proto", "alice", "LAB", "LAB", "alice"),
    ("CORP", "proto", "", "LAB", "CORP", "proto"),
    // A domain in the name is used over one given separately.
    ("CORP", "proto", "OTHER\\alice", "LAB", "OTHER", "alice"),
    ("CORP", "proto", "alice@lab.example", "LAB", "lab.example", "alice"),
    // A Microsoft account's address is never split again.
    ("MicrosoftAccount", "someone@example.com", "", "", "MicrosoftAccount", "someone@example.com"),
    ("", "proto", "MicrosoftAccount\\someone@example.com", "", "MicrosoftAccount", "someone@example.com"),
    ("", "proto", "MicrosoftAccount\\someone@example.com", "LAB", "MicrosoftAccount", "someone@example.com"),
    ("CORP", "proto", "someone@outlook.com", "", "MicrosoftAccount", "someone@outlook.com"),
    ("CORP", "proto", "CORP\\someone@outlook.com", "", "CORP", "someone@outlook.com"),
];

/// Any other backend's answer, where an empty user name means the
/// session's: `(session user, answer user, user)`.
const FILL_USER: &[(&str, &str, &str)] = &[
    ("proto", "", "proto"),
    ("proto", "alice", "alice"),
    ("", "", ""),
    ("proto", " ", "proto"),
    ("proto", " alice ", "alice"),
];

// The known-hosts rule, for every client that pins a far end's identity.

/// The address a pin is filed under, folded for case and trimmed as Java's
/// `trim()` trims (ASCII controls and the space, nothing wider), since the
/// phone's pins are already filed that way.
const KEY: &[(&str, &str)] = &[
    ("Host.Example", "host.example"),
    ("  [FE80::1]:5900 ", "[fe80::1]:5900"),
    ("\tHOST:3389\n", "host:3389"),
    // No-break spaces, which are not trimmed.
    ("\u{a0}host\u{a0}", "\u{a0}host\u{a0}"),
    ("", ""),
    ("123456789@hbbs.Example", "123456789@hbbs.example"),
];

/// A fingerprint in the form used for comparison: only its ASCII hex
/// digits, upper case.
const CANONICAL: &[(&str, &str)] = &[
    ("66:cc:ED", "66CCED"),
    ("66 CC-ed", "66CCED"),
    ("", ""),
    ("SHA256:ab:cd", "A256ABCD"),
    // Fullwidth digits, which are not ASCII.
    ("\u{ff10}\u{ff11}:ab", "AB"),
];

/// A store of pins: `(key, fingerprint)`.
type Pins = &'static [(&'static str, &'static str)];

/// The expected verdict: `None` for a match, or the reason it asks (the
/// identity, and the key the fingerprint is pinned under when it matches
/// another).
type Asks = Option<(Identity, &'static str)>;

/// A fingerprint against a store of pins (keys as filed): `(pins, address,
/// fingerprint, verdict)`, where an ask carries the reason: new, matches
/// another (with the key it is pinned under; no case has two, since which
/// one a store finds first depends on its order), or changed.
const VERDICT: &[(Pins, &str, &str, Asks)] = &[
    (&[("host:3389", "AB:CD:EF")], "HOST:3389", "ab:cd:ef", None),
    (&[("host:3389", "abcdef")], " host:3389", "AB CD EF", None),
    (&[("host:3389", "AB:CD:EF")], "host:3389", "00:11", Some((Identity::Changed, ""))),
    (&[("host:3389", "AB:CD:EF")], "10.0.0.5:3389", "AB:CD:EF", Some((Identity::MatchesAnother, "host:3389"))),
    (&[("host:3389", "AB:CD:EF")], "10.0.0.5:3389", "00:11", Some((Identity::New, ""))),
    (&[], "host:3389", "AB:CD:EF", Some((Identity::New, ""))),
    (&[("a", "")], "b", "", Some((Identity::New, ""))),
    (&[("host:3389", "AB:CD:EF"), ("10.0.0.5:3389", "00:11")], "10.0.0.5:3389", "AB:CD:EF", Some((Identity::Changed, ""))),
];

#[test]
fn split_domain_agrees_with_the_vectors() {
    for case @ &(typed, domain, user) in SPLIT_DOMAIN {
        let (got_domain, got_user) = split_domain(typed);
        assert_eq!((got_domain.as_str(), got_user.as_str()), (domain, user), "{case:?}");
    }
}

#[test]
fn rdp_identity_agrees_with_the_vectors() {
    for case @ &(session_domain, session_user, answer_user, answer_domain, domain, user) in RDP_IDENTITY {
        let answer = Credentials {
            user: answer_user.into(),
            domain: answer_domain.into(),
            password: "pw".into(),
        };
        let (got_domain, got_user) = rdp_identity(&answer, session_domain, session_user);
        assert_eq!((got_domain.as_str(), got_user.as_str()), (domain, user), "{case:?}");
    }
}

#[test]
fn fill_user_agrees_with_the_vectors() {
    for case @ &(session_user, answer_user, user) in FILL_USER {
        let answer = Credentials {
            user: answer_user.into(),
            domain: String::new(),
            password: "pw".into(),
        };
        let filled = fill_user(answer, session_user);
        assert_eq!(filled.user, user, "{case:?}");
        assert_eq!(filled.password, "pw");
    }
}

#[test]
fn known_hosts_keys_and_fingerprints_agree_with_the_vectors() {
    for case @ &(address, key) in KEY {
        assert_eq!(trust::key(address), key, "{case:?}");
    }
    for case @ &(fingerprint, canonical) in CANONICAL {
        assert_eq!(trust::canonical(fingerprint), canonical, "{case:?}");
    }
}

#[test]
fn known_hosts_verdicts_agree_with_the_vectors() {
    for case @ &(pins, address, fingerprint, ask) in VERDICT {
        let want = match ask {
            None => Verdict::Matches,
            Some((identity, matching_address)) => Verdict::Ask {
                identity,
                matching_address: matching_address.into(),
            },
        };
        assert_eq!(trust::verdict(pins, address, fingerprint), want, "{case:?}");
    }
}

fn vectors(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn str<'a>(case: &'a Value, key: &str) -> &'a str {
    case[key].as_str().unwrap_or_else(|| panic!("{key} in {case}"))
}

fn cases<'a>(file: &'a Value, section: &str) -> &'a Vec<Value> {
    file[section].as_array().unwrap_or_else(|| panic!("no {section}"))
}

/// A spec from its description in the vectors. Leaked, since a spec is
/// `'static` by design and a test has only a handful.
fn spec(value: &Value) -> Spec {
    let leak = |s: &str| -> &'static str { Box::leak(s.to_string().into_boxed_str()) };
    let choices: Vec<Choice> = value["choices"]
        .as_array()
        .map(|list| {
            list.iter()
                .map(|c| Choice {
                    value: leak(str(c, "value")),
                    label: leak(str(c, "value")),
                    desktop_label: None,
                    summary: None,
                    requires: c["requires"].as_str().map(leak),
                })
                .collect()
        })
        .unwrap_or_default();
    Spec {
        key: leak(str(value, "key")),
        label: leak(str(value, "key")),
        summary: None,
        desktop_summary: None,
        ty: match str(value, "type") {
            "choice" => Type::Choice,
            "bool" => Type::Bool,
            other => panic!("type {other}"),
        },
        choices: Box::leak(choices.into_boxed_slice()),
        default: leak(str(value, "default")),
        scope: Scope::Connection,
        live: false,
        gate: None,
        command_line_only: false,
    }
}

#[test]
fn settle_agrees_with_the_vectors() {
    let file = vectors("options.json");
    let specs: &'static [Spec] = Box::leak(cases(&file, "specs").iter().map(spec).collect::<Vec<_>>().into_boxed_slice());
    for case in cases(&file, "settle") {
        let mut options = Options::defaults(specs);
        for (key, value) in case["values"].as_object().unwrap() {
            options.set(key, value.as_str().unwrap());
        }
        let changed = options.settle(specs);
        let want: Vec<&str> = case["changed"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(changed, want, "{case}");
        for (key, value) in case["settled"].as_object().unwrap() {
            assert_eq!(options.get(key), value.as_str().unwrap(), "{key} in {case}");
        }
    }
}
