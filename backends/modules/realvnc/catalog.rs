// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The RealVNC backend's entry in the catalog: what it is called, where it
//! goes in a picker, how its identities are pinned, and every option it
//! takes. Compiled by `remotedesktop-api`, which names this file by path
//! (`backends/api/src/catalog.rs`), and not by this module's crate: the
//! Linux client's picker and the phone's generated Java (`vento-codegen`)
//! read it without building the backend.

use crate::catalog::About;
use crate::options::{Scope, Spec, VIEW_ONLY, BELL, SHARED, choice, choices, bool, command_line_only, integer, text, when};
use crate::trust::Pinning;

pub static REALVNC: About = About {
    id: "realvnc",
    name: "VNC (RealVNC)",
    // Three of the backends speak RFB, so a badge has to name the
    // implementation as well as the protocol.
    short_name: "RealVNC",
    description: "RealVNC's proprietary client implementation. This is the only one which supports UDP, ZRLE2, and RealVNC's proprietary authentication/encryption. It performs significantly better than the others when connecting to a RealVNC server, especially for typical desktop usage.",
    // After the open VNC and RDP clients: it is the more capable client
    // against RealVNC's own server, but it has a limited lifetime. 4.9.3's
    // `libvncviewer.so` is 16 KB aligned, but it is a third-party binary and
    // cannot be rebuilt for a future page size.
    order: 30,
    pinning: Pinning::ADDRESS,
    user_has_domain: false,
    // The core asks to have an identity remembered after its security prompt,
    // and skips the prompt next time for a server whose identity it is given
    // back.
    saves_identity: true,
    options: OPTIONS,
};

/// Which of RealVNC's parameters the clients offer.
///
/// Every name here is either a parameter recovered from the binary's own
/// registry or one the original sets on `createSession`. The registry holds
/// many more (the whole underscore-prefixed `_Hosted*` / `_Udp*` / `_Opus*`
/// transport surface), and none is offered without a reason, because an
/// option that cannot be described is worse than no option.
///
/// [`Scope::Connection`] versus [`Scope::Global`] is a question about the
/// setting, not about the protocol: does one server want a different answer
/// from another? Quality does, encryption policy does not.
pub static OPTIONS: &[Spec] = &[
    // Auto, not the core's own default of High: Auto follows the measured line
    // speed, while High on every connection would be chosen before the network
    // is known.
    choices(
        "Quality",
        "Picture quality",
        Some("Encoding and color depth preset."),
        "Auto",
        Scope::Layered,
        true,
        &[
            choice("Auto", "Automatic"),
            choice("High", "High"),
            choice("Medium", "Medium"),
            choice("Low", "Low"),
            choice("Custom", "Custom"),
        ],
    ),
    // The encoding row every other RFB backend has. All eight values of the
    // enum the core registers, in its registration order, rather than a guess
    // or a subset, including the two lossy ones: the row applies only at a
    // Custom quality, where the quality setting does not choose lossiness.
    //
    // Only a Custom quality reads it, so the row is not offered at any other.
    // The picture policy builds its own encoding list for each of the three
    // fixed classes and consults this parameter only for the fourth. That is
    // the library's behaviour rather than an interface decision: it was read
    // out of the class switch and tested both ways (Automatic, High and Low
    // all sent ZRLE with Hextile requested, and Custom sent Hextile).
    when(
        choices(
            "PreferredEncoding",
            "Encoding",
            Some("Preferred image encoding. The server may still choose to use a different one. Only used when quality is set to Custom"),
            "ZRLE2",
            Scope::Layered,
            true,
            &[
                choice("JPEG", "JPEG (lossy)"),
                choice("Zlib", "Zlib"),
                choice("ZRLE", "ZRLE"),
                choice("ZRLE2", "ZRLE2"),
                choice("TRLE", "TRLE"),
                choice("Hextile", "Hextile"),
                choice("Raw", "Raw (uncompressed)"),
                choice("JRLE", "JRLE (lossy)"),
            ],
        ),
        "Quality",
        &["Custom"],
    ),
    // The colour depth, offered only where the core reads it.
    //
    // Low and Custom are the qualities that consult it. The picture policy
    // resolves a quality to one of four classes and picks the pixel format
    // from that: High and Medium ask for full colour regardless, Low and
    // Custom use this value, and Automatic follows the class the measured
    // line speed selects. RealVNC's published reference says the parameter is
    // ignored unless the quality is Custom, but the library behaves otherwise:
    // the Low case was read out of the picture policy and then tested, and at
    // High a connection with `rgb111` asks for 24-bit colour, while at Low and
    // Custom it asks for 3-bit.
    //
    // The default is full colour where the original forces `rgb222` at every
    // quality but Custom. The difference is small: at High and Medium the core
    // asks for full colour regardless, and Low with no depth chosen still gets
    // `rgb111` (`RealVncProvider.colorLevelFor`), so the default mainly lets an
    // Automatic quality keep its colour on a slow line.
    when(
        choices(
            "ColorLevel",
            "Colour depth",
            Some("Pixel colour depth. Only used when quality is set to Low or Custom. If unset, Low uses 8 colours (rgb111)."),
            "full",
            Scope::Layered,
            true,
            &[
                choice("full", "Full colour (24-bit)"),
                choice("rgb222", "64 colours (rgb222)"),
                choice("rgb111", "8 colours (rgb111)"),
            ],
        ),
        "Quality",
        &["Low", "Custom"],
    ),
    VIEW_ONLY,
    SHARED,
    bool(
        "AutoReconnect",
        "Reconnect automatically",
        "Do not ask before re-establishing dropped connections.",
        true,
        Scope::Connection,
        true,
    ),
    bool(
        "EnableUdpRfb",
        "Enable UDP",
        "Enable RealVNC's proprietary SCTP/RTP-based UDP protocol, falling back to TCP if the server doesn't respond within 500ms.",
        true,
        Scope::Connection,
        false,
    ),
    choices(
        "Encryption",
        "Encryption",
        Some("Whether to encrypt connections."),
        "PreferOn",
        Scope::Global,
        false,
        &[
            choice("AlwaysOn", "Required"),
            choice("PreferOn", "If supported by the server"),
            choice("PreferOff", "Only if required by the server"),
            choice("Server", "Let the server decide"),
        ],
    ),
    bool(
        "WarnUnencrypted",
        "Warn when unencrypted",
        "Show a warning when connecting to a server which doesn't support encryption.",
        true,
        Scope::Global,
        false,
    ),
    bool(
        "DotWhenNoCursor",
        "Dot for an invisible cursor",
        "Show a dot when the server hides the cursor.",
        true,
        Scope::Global,
        false,
    ),
    BELL,
    // The core's three clipboard parameters are deliberately not rows.
    // Whether the local clipboard is sent to the remote machine and back is a
    // client setting, set once in the client's own settings and enforced
    // there; the core's parameters would be a second setting for the same
    // thing, on a second screen, one layer down. They stay at their defaults
    // (on), so the client's setting is the only one that applies.
    integer(
        "KeepAliveInterval",
        "Keep-alive interval",
        "Seconds between keep-alives on an idle connection.",
        "30",
        Scope::Global,
        false,
    ),
    integer(
        "KeepAliveResponseTimeout",
        "Keep-alive timeout",
        "Seconds to wait for an answer before giving up.",
        "30",
        Scope::Global,
        false,
    ),
    // The Linux client's, on its command line: on the phone the add-on gets
    // the library, on a screen of its own. The values are checked and acted
    // on by the client before a session starts (`linux/src/realvnc/`); the
    // backend ignores the key.
    command_line_only(text(
        "AutoFetchLibraries",
        "Fetch RealVNC's library",
        "Where the proprietary library from the RealVNC Viewer APK comes from when it is missing: off, prompt \
         (ask, then download it), auto (download it), the path of a local APK, or an http(s) URL of an APK.",
        "prompt",
        Scope::Global,
        false,
    )),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::{Options, find};

    /// The keys of the phone's option lists these replaced, in the same
    /// order: keys are stored in connection records and preference files, so
    /// renaming one loses the setting.
    #[test]
    fn the_keys_are_the_ones_records_hold() {
        let keys: Vec<&str> = OPTIONS
            .iter()
            .filter(|s| !s.command_line_only)
            .map(|s| s.key)
            .collect();
        assert_eq!(
            keys,
            [
                "Quality",
                "PreferredEncoding",
                "ColorLevel",
                "ViewOnly",
                "Shared",
                "AutoReconnect",
                "EnableUdpRfb",
                "Encryption",
                "WarnUnencrypted",
                "DotWhenNoCursor",
                "AcceptBell",
                "KeepAliveInterval",
                "KeepAliveResponseTimeout"
            ]
        );
    }

    /// Where the library comes from is set on the desktop's command line
    /// alone.
    #[test]
    fn fetching_the_library_is_a_command_line_option() {
        let fetch = find(OPTIONS, "AutoFetchLibraries").unwrap();
        assert!(fetch.command_line_only);
        assert_eq!(fetch.default, "prompt");
    }

    #[test]
    fn the_defaults_are_the_providers_and_a_set_colour_depth_is_remembered() {
        let mut options = Options::defaults(OPTIONS);
        assert_eq!(options.get("Quality"), "Auto");
        assert_eq!(options.get("ColorLevel"), "full");
        assert!(!options.is_set("ColorLevel"));
        assert!(options.bool("Shared"));
        assert_eq!(options.int("KeepAliveInterval", 0), 30);
        options.set("ColorLevel", "rgb111");
        assert!(options.is_set("ColorLevel"));
        let keep = find(OPTIONS, "KeepAliveInterval").unwrap();
        assert!(keep.check("45").is_ok());
        assert!(keep.check("soon").is_err());
    }

    #[test]
    fn the_encoding_is_offered_only_at_a_custom_quality() {
        let gate = find(OPTIONS, "PreferredEncoding").unwrap().gate.unwrap();
        assert_eq!(gate.key, "Quality");
        assert!(gate.open("Custom"));
        assert!(!gate.open("High"));
    }
}
