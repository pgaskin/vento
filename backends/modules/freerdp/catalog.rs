// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The FreeRDP backend's catalog entry: its name, its place in a picker, how
//! its identities are pinned, and its options. Compiled by
//! `remotedesktop-api`, which includes this file by path
//! (`backends/api/src/catalog.rs`), not by this module's crate, so the Linux
//! client's picker and the Android generated Java (`vento-codegen`) read it
//! without building the backend.

use crate::catalog::About;
use crate::options::{
    Scope, Spec, Choice, VIEW_ONLY, DESKTOP_SIZES, MONITOR_COUNTS, choice, choices, bool,
};
use crate::trust::Pinning;

pub static FREERDP: About = About {
    id: "freerdp",
    name: "RDP (FreeRDP)",
    short_name: "FreeRDP",
    description: "The most robust RDP implementation. Support EGFX with hardware-accelerated H.264 video, and supports audio. This is the best one to use with a Windows RDP server.",
    // Ahead of IronRDP: this one works with Windows desktops and supports
    // H.264 video.
    order: 20,
    pinning: Pinning::ADDRESS,
    // The prompt has no domain field, so the domain goes in the user name
    // (`DOMAIN\user`), as Windows shows it.
    user_has_domain: true,
    saves_identity: false,
    options: OPTIONS,
};

/// The FreeRDP backend's options.
///
/// They started as a copy of IronRDP's rows so the two would be comparable.
/// Two of them are not supported by IronRDP at all: **EGFX**, the graphics
/// pipeline, which Windows 8 and later prefer for sending a desktop and the
/// only path that carries H.264; and the **display scale**, the only way to
/// make a desktop with text sized for a monitor readable on a phone. **Sound**
/// also started here, and IronRDP has it now, except for playing it on the
/// remote machine.
///
/// Deliberately left out:
///
/// - **The keyboard layout.** Keys are sent as scancodes from a US table, and
///   the near end's layout has already turned a press into a character before
///   the backend sees it. The layout sent at connect time (US) maps those
///   scancodes back to the typed characters; any other layout would change
///   every letter.
/// - **Individual codecs.** Progressive and planar cost nothing and are
///   decoded in software either way. The two H.264 switches act as one, since
///   the pair also enables every capability version above 8.1. That leaves
///   video or no video, which is a value of the Graphics row.
/// - **Small cache and thin client.** Both tell the server to expect less of
///   the client. The cache they shrink and the memory it costs are the
///   server's, and the only part that affects the picture requests a cheaper
///   H.264 profile, on a phone that decodes in hardware.
/// - **Drives, printers and smartcards.** Printers need CUPS and smartcards
///   need PC/SC, neither of which is in this build. Drives need a POSIX path,
///   and Android grants access to a document tree, which has no path.
/// - **The clipboard** (a client setting shared by every protocol), **the
///   domain** (already a field, split out of the user name) and **the colour
///   depth** (the codec decides the cost of a pixel before it reaches the
///   framebuffer, which is 32 bits either way).
pub static OPTIONS: &[Spec] = &[
    choices(
        "Nla",
        "NLA",
        Some("Network Level Authentication authenticates using CredSSP before the session starts rather than using the server-side login screen."),
        "prefer",
        Scope::Connection,
        false,
        &[
            choice("prefer", "If supported by the server"),
            choice("require", "Required"),
            choice("off", "Never"),
        ],
    ),
    bool(
        "LegacyTls",
        "Old TLS",
        "Allow TLS 1.0 and 1.1 and their weaker keys, for a server that has nothing newer, such as Windows 7 or Server 2008 R2 without updates.",
        false,
        Scope::Connection,
        false,
    ),
    choices(
        "DesktopSize",
        "Desktop size",
        Some("Preferred screen resolution."),
        "1920x1200",
        Scope::Connection,
        false,
        DESKTOP_SIZES,
    ),
    choices(
        "Scale",
        "Display scale",
        Some("Supported on Windows 8.1 and later."),
        "100",
        Scope::Connection,
        false,
        &[
            choice("100", "100%"),
            choice("140", "140%"),
            choice("180", "180%"),
        ],
    ),
    choices(
        "Monitors",
        "Monitors",
        Some("Simulate multiple monitors."),
        "1",
        Scope::Connection,
        false,
        MONITOR_COUNTS,
    ),
    choices(
        "Graphics",
        "Graphics",
        Some("EGFX is the newest and most advanced codec, RemoteFX is older, and Bitmap sends raw images."),
        "gfx",
        Scope::Layered,
        false,
        &[
            choice("gfx", "EGFX"),
            choice("gfx-novideo", "EGFX, no video"),
            choice("rfx", "RemoteFX"),
            choice("bitmap", "Bitmap"),
        ],
    ),
    choices(
        "Experience",
        "Visual effects",
        Some("Controls the amount of animation and visual effects, if supported by the server."),
        "balanced",
        Scope::Layered,
        false,
        &[
            choice("full", "Full"),
            choice("balanced", "Balanced"),
            choice("plain", "Limited"),
        ],
    ),
    choices(
        "Sound",
        "Sound",
        Some("Controls where sound is played, if supported by the server."),
        "off",
        Scope::Layered,
        false,
        &[
            choice("off", "Off"),
            Choice {
                desktop_label: Some("This computer"),
                ..choice("local", "This phone")
            },
            choice("remote", "The remote machine"),
        ],
    ),
    bool(
        "Microphone",
        "Microphone",
        "Send this phone's microphone to the remote machine. Android asks for permission before connecting.",
        false,
        Scope::Connection,
        false,
    ),
    bool(
        "Compression",
        "Compression",
        "Enable connection-level compression. Uses less bandwidth, but more CPU.",
        true,
        Scope::Layered,
        false,
    ),
    VIEW_ONLY,
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::prompt_user;
    use crate::kind::Platform;
    use crate::options::find;

    /// The keys the Android lists had when they moved here, in the same
    /// order: keys are stored in connection records and preference files,
    /// so renaming one loses the setting.
    #[test]
    fn the_keys_are_the_ones_records_hold() {
        let keys: Vec<&str> = OPTIONS.iter().map(|s| s.key).collect();
        assert_eq!(
            keys,
            [
                "Nla",
                "LegacyTls",
                "DesktopSize",
                "Scale",
                "Monitors",
                "Graphics",
                "Experience",
                "Sound",
                "Microphone",
                "Compression",
                "ViewOnly"
            ]
        );
    }

    #[test]
    fn its_prompt_starts_with_the_domain_and_its_sound_is_this_machine() {
        assert_eq!(prompt_user(&FREERDP, "CORP\\proto"), "CORP\\proto");
        assert_eq!(prompt_user(&FREERDP, " proto "), "proto");
        let sound = find(OPTIONS, "Sound").unwrap();
        assert_eq!(sound.label_for("local", Platform::Android), "This phone");
        assert_eq!(sound.label_for("local", Platform::Linux), "This computer");
    }
}
