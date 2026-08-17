// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! X11 keysyms in, PC/AT scancodes out.
//!
//! The one place the protocol does not match the stack. Everything above
//! `CursorController` speaks keysyms, because that is RFB's vocabulary and the
//! extension keyboard was built against it; RDP's keyboard is
//! **scancodes** — set 1, with an `E0` prefix for the keys the original PC
//! keyboard did not have.
//!
//! Three things follow from that, and each is a decision rather than a detail:
//!
//! 1. **A scancode is a position, not a character.** `0x1E` is "the key left of
//!    S", which is `a` on a US layout and `q` on a French one. So this table is
//!    a US layout and says so, and the server's own layout is what turns a
//!    position back into a character. That is how every RDP client works, and
//!    it is why [`scancode`] is allowed to fail.
//! 2. **What it cannot place goes as Unicode.** RDP has a keyboard event that
//!    carries a UTF-16 code unit instead of a position ([`Key::Unicode`]), which
//!    is what makes `é` and `→` arrive from a phone's IME without a layout
//!    between them. It is the fallback and not the rule, because a Unicode
//!    event carries no modifier state — `Ctrl` plus one is not a shortcut on
//!    the far end, and a modifier held on one keyboard while a letter arrives
//!    from another is exactly what this stack lets happen.
//! 3. **Shift is part of the answer.** `!` is not a key; it is `1` with Shift
//!    held. So a mapping is a scancode *and* whether the layout needs Shift for
//!    it, and the caller synthesises the modifier — see `Client::key_down`, which
//!    also has to know not to synthesise one that is already down.

use ironrdp_input::Scancode;

/// What a keysym turns into on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// A position on the keyboard, and whether the US layout needs Shift to
    /// produce the character that keysym names.
    Scancode { code: Scancode, shift: bool },
    /// A character with no position: sent as a Unicode keyboard event, which
    /// the server turns into text without consulting a layout.
    Unicode(char),
    /// Pause, which is not a scancode at all — see [`named`].
    Pause,
}

/// `E0`-prefixed, i.e. one of the keys the original 84-key PC keyboard did not
/// have.
const fn ext(code: u8) -> Scancode {
    Scancode::from_u8(true, code)
}

const fn plain(code: u8) -> Scancode {
    Scancode::from_u8(false, code)
}

/// The named keys: modifiers, editing, navigation, function keys, the keypad.
///
/// Both hands, since a physical keyboard's right Shift is a keysym of its own
/// and the extension row's is the left one.
///
/// **Pause is not in here**, because it is not a scancode: the key never had
/// one of its own, and what a keyboard puts on the wire for it is the `E1`
/// sequence `1D 45`. Both RDP clients here send what mstsc sends instead, which
/// is a Ctrl marked `E1` around a Num Lock — see `Client::key_down`.
fn named(keysym: u32) -> Option<Scancode> {
    Some(match keysym {
        0xffe1 => plain(0x2a),        // Shift_L
        0xffe2 => plain(0x36),        // Shift_R
        0xffe3 => plain(0x1d),        // Control_L
        0xffe4 => ext(0x1d),          // Control_R
        0xffe9 => plain(0x38),        // Alt_L
        0xffea | 0xfe03 => ext(0x38), // Alt_R, ISO_Level3_Shift — both AltGr
        0xffeb => ext(0x5b),          // Super_L
        0xffec => ext(0x5c),          // Super_R
        0xff67 => ext(0x5d),          // Menu
        0xffe5 => plain(0x3a),        // Caps_Lock
        0xff7f => plain(0x45),        // Num_Lock
        0xff14 => plain(0x46),        // Scroll_Lock

        0xff08 => plain(0x0e), // BackSpace
        0xff09 => plain(0x0f), // Tab
        0xff0d => plain(0x1c), // Return
        0xff1b => plain(0x01), // Escape
        0xffff => ext(0x53),   // Delete
        0xff63 => ext(0x52),   // Insert
        0xff50 => ext(0x47),   // Home
        0xff57 => ext(0x4f),   // End
        0xff55 => ext(0x49),   // Page_Up
        0xff56 => ext(0x51),   // Page_Down
        0xff51 => ext(0x4b),   // Left
        0xff52 => ext(0x48),   // Up
        0xff53 => ext(0x4d),   // Right
        0xff54 => ext(0x50),   // Down
        0xff61 => ext(0x37), // Print

        // F1–F10 are consecutive, F11 and F12 are not: the 101-key keyboard
        // added them after the numeric keypad's codes were already taken.
        0xffbe..=0xffc7 => plain(0x3b + (keysym - 0xffbe) as u8),
        0xffc8 => plain(0x57), // F11
        0xffc9 => plain(0x58), // F12
        // F13–F24, which no PC keyboard has and every layout still carries a
        // position for. F13–F23 run from 0x64; F24 does not follow them, and a
        // Windows far end resolves the position that would be F24 by counting
        // to VK_OEM_PA3 instead — which is what every keyboard type has there.
        0xffca..=0xffd4 => plain(0x64 + (keysym - 0xffca) as u8),
        0xffd5 => plain(0x76), // F24

        // The keypad. Each key appears twice, once under each Num Lock state,
        // and both spellings are the same position — which is the whole of what
        // RDP can say. Which of the two the far end produces is decided by the
        // far end's own Num Lock and not by ours, so this is the one place
        // where the phone owning the lock does not carry: with the lock off
        // here and on there, `KP_Home` arrives as `7`.
        0xffb7 | 0xff95 => plain(0x47), // KP_7, KP_Home
        0xffb8 | 0xff97 => plain(0x48), // KP_8, KP_Up
        0xffb9 | 0xff9a => plain(0x49), // KP_9, KP_Prior
        0xffb4 | 0xff96 => plain(0x4b), // KP_4, KP_Left
        0xffb5 | 0xff9d => plain(0x4c), // KP_5, KP_Begin
        0xffb6 | 0xff98 => plain(0x4d), // KP_6, KP_Right
        0xffb1 | 0xff9c => plain(0x4f), // KP_1, KP_End
        0xffb2 | 0xff99 => plain(0x50), // KP_2, KP_Down
        0xffb3 | 0xff9b => plain(0x51), // KP_3, KP_Next
        0xffb0 | 0xff9e => plain(0x52), // KP_0, KP_Insert
        0xffae | 0xff9f => plain(0x53), // KP_Decimal, KP_Delete
        0xffaa => plain(0x37),          // KP_Multiply
        0xffad => plain(0x4a),          // KP_Subtract
        0xffab => plain(0x4e),          // KP_Add
        0xffaf => ext(0x35),            // KP_Divide
        0xff8d => ext(0x1c),            // KP_Enter
        0xffac => plain(0x7e),          // KP_Separator, the Brazilian keypad comma

        // The unambiguous three of a Japanese board's conversion keys, which
        // are the three `Keysym` sends.
        0xff27 => plain(0x70), // Hiragana_Katakana
        0xff23 => plain(0x79), // Henkan_Mode
        0xff22 => plain(0x7b), // Muhenkan

        _ => return None,
    })
}

/// The printable half of a US layout: the character a keysym names, and the key
/// it sits on.
///
/// Written as the rows of the keyboard rather than as a sorted table, because
/// that is what it is, and a wrong entry is visible as a wrong neighbour.
const US_LAYOUT: &[(u8, char, char)] = &[
    // (scancode, unshifted, shifted)
    (0x29, '`', '~'),
    (0x02, '1', '!'),
    (0x03, '2', '@'),
    (0x04, '3', '#'),
    (0x05, '4', '$'),
    (0x06, '5', '%'),
    (0x07, '6', '^'),
    (0x08, '7', '&'),
    (0x09, '8', '*'),
    (0x0a, '9', '('),
    (0x0b, '0', ')'),
    (0x0c, '-', '_'),
    (0x0d, '=', '+'),
    (0x10, 'q', 'Q'),
    (0x11, 'w', 'W'),
    (0x12, 'e', 'E'),
    (0x13, 'r', 'R'),
    (0x14, 't', 'T'),
    (0x15, 'y', 'Y'),
    (0x16, 'u', 'U'),
    (0x17, 'i', 'I'),
    (0x18, 'o', 'O'),
    (0x19, 'p', 'P'),
    (0x1a, '[', '{'),
    (0x1b, ']', '}'),
    (0x2b, '\\', '|'),
    (0x1e, 'a', 'A'),
    (0x1f, 's', 'S'),
    (0x20, 'd', 'D'),
    (0x21, 'f', 'F'),
    (0x22, 'g', 'G'),
    (0x23, 'h', 'H'),
    (0x24, 'j', 'J'),
    (0x25, 'k', 'K'),
    (0x26, 'l', 'L'),
    (0x27, ';', ':'),
    (0x28, '\'', '"'),
    (0x2c, 'z', 'Z'),
    (0x2d, 'x', 'X'),
    (0x2e, 'c', 'C'),
    (0x2f, 'v', 'V'),
    (0x30, 'b', 'B'),
    (0x31, 'n', 'N'),
    (0x32, 'm', 'M'),
    (0x33, ',', '<'),
    (0x34, '.', '>'),
    (0x35, '/', '?'),
    (0x39, ' ', ' '),
];

/// What to send for a keysym, or `None` for one that means nothing here.
pub fn scancode(keysym: u32) -> Option<Key> {
    if keysym == 0xff13 {
        return Some(Key::Pause);
    }
    if let Some(code) = named(keysym) {
        return Some(Key::Scancode { code, shift: false });
    }
    // The one keypad key a PC has no position for — FreeRDP's own xkb table
    // has `KP_Equal` as unknown — and it names a character, so it goes as one.
    if keysym == 0xffbd {
        return Some(Key::Unicode('='));
    }
    let cp = unicode(keysym)?;
    let ch = char::from_u32(cp)?;
    for &(code, lower, upper) in US_LAYOUT {
        if ch == lower {
            return Some(Key::Scancode {
                code: plain(code),
                shift: false,
            });
        }
        if ch == upper {
            return Some(Key::Scancode {
                code: plain(code),
                shift: true,
            });
        }
    }
    Some(Key::Unicode(ch))
}

/// The inverse of `Keysym.fromUnicode`: Latin-1 is itself, everything else is
/// `0x01000000 | codepoint`.
fn unicode(keysym: u32) -> Option<u32> {
    if keysym & 0xff00_0000 == 0x0100_0000 {
        return Some(keysym & 0x00ff_ffff);
    }
    if (0x20..=0x7e).contains(&keysym) || (0xa0..=0xff).contains(&keysym) {
        return Some(keysym);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code_of(keysym: u32) -> (u16, bool) {
        match scancode(keysym) {
            Some(Key::Scancode { code, shift }) => (code.as_u16(), shift),
            other => panic!("expected a scancode for {keysym:#x}, got {other:?}"),
        }
    }

    /// The three that are easy to get wrong by one: F11 is not after F10, the
    /// arrows are extended, and a left modifier is not an extended one.
    #[test]
    fn the_named_keys_land_where_the_keyboard_has_them() {
        assert_eq!(code_of(0xffbe), (0x3b, false), "F1");
        assert_eq!(code_of(0xffc7), (0x44, false), "F10");
        assert_eq!(code_of(0xffc8), (0x57, false), "F11 is not 0x45");
        assert_eq!(code_of(0xffc9), (0x58, false), "F12");
        assert_eq!(code_of(0xff51), (0xe04b, false), "Left is extended");
        assert_eq!(code_of(0xffe1), (0x2a, false), "Shift_L is not");
        assert_eq!(code_of(0xffe3), (0x1d, false), "Control_L");
        assert_eq!(code_of(0xfe03), (0xe038, false), "AltGr is right Alt");
        assert_eq!(code_of(0xffff), (0xe053, false), "Delete");
        assert_eq!(code_of(0xff08), (0x0e, false), "BackSpace is not Delete");
    }

    /// The keypad is one set of positions under two sets of keysyms, and the
    /// pairs have to line up: a Num-Lock-off keysym landing on the wrong
    /// position is a navigation key that moves the wrong way.
    #[test]
    fn both_halves_of_the_keypad_are_the_same_keys() {
        for (digit, navigation, code) in [
            (0xffb0, 0xff9e, 0x52), // KP_0, KP_Insert
            (0xffb1, 0xff9c, 0x4f), // KP_1, KP_End
            (0xffb2, 0xff99, 0x50), // KP_2, KP_Down
            (0xffb3, 0xff9b, 0x51), // KP_3, KP_Next
            (0xffb4, 0xff96, 0x4b), // KP_4, KP_Left
            (0xffb5, 0xff9d, 0x4c), // KP_5, KP_Begin
            (0xffb6, 0xff98, 0x4d), // KP_6, KP_Right
            (0xffb7, 0xff95, 0x47), // KP_7, KP_Home
            (0xffb8, 0xff97, 0x48), // KP_8, KP_Up
            (0xffb9, 0xff9a, 0x49), // KP_9, KP_Prior
            (0xffae, 0xff9f, 0x53), // KP_Decimal, KP_Delete
        ] {
            assert_eq!(code_of(digit), (code, false), "keysym {digit:#x}");
            assert_eq!(code_of(navigation), (code, false), "keysym {navigation:#x}");
        }
        assert_eq!(code_of(0xffaf), (0xe035, false), "KP_Divide is extended");
        assert_eq!(code_of(0xff8d), (0xe01c, false), "and so is KP_Enter");
        assert_eq!(code_of(0xffaa), (0x37, false), "KP_Multiply is not");
        assert_eq!(
            scancode(0xffbd),
            Some(Key::Unicode('=')),
            "KP_Equal has no position on a PC keyboard"
        );
    }

    /// The function row runs past where a keyboard stops, and F13 is nowhere
    /// near F12 in either numbering.
    #[test]
    fn the_function_row_runs_to_twenty_four() {
        assert_eq!(code_of(0xffca), (0x64, false), "F13");
        for n in 0..11u32 {
            assert_eq!(code_of(0xffca + n), (0x64 + n as u16, false), "F{}", n + 13);
        }
        // The one that does not continue the run: 0x6f is VK_OEM_PA3.
        assert_eq!(code_of(0xffd5), (0x76, false), "F24");
    }

    /// Pause is the one key that is not a position at all.
    #[test]
    fn pause_is_not_a_scancode() {
        assert_eq!(scancode(0xff13), Some(Key::Pause));
        assert_eq!(code_of(0xff7f), (0x45, false), "and Num Lock still is");
    }

    /// A character keysym is a *position plus Shift*, and the position is the
    /// same one for both cases of a letter.
    #[test]
    fn characters_carry_the_shift_the_layout_needs() {
        assert_eq!(code_of('a' as u32), (0x1e, false));
        assert_eq!(code_of('A' as u32), (0x1e, true), "same key, shifted");
        assert_eq!(code_of('1' as u32), (0x02, false));
        assert_eq!(code_of('!' as u32), (0x02, true));
        assert_eq!(code_of(' ' as u32), (0x39, false));
        assert_eq!(code_of('/' as u32), (0x35, false));
        assert_eq!(code_of('?' as u32), (0x35, true));
    }

    /// Anything the layout has no position for is a Unicode event rather than
    /// nothing at all — which is what lets an IME's own text through.
    #[test]
    fn what_the_layout_cannot_place_goes_as_unicode() {
        assert_eq!(scancode(0xe9), Some(Key::Unicode('é')), "Latin-1 is itself");
        assert_eq!(
            scancode(0x0100_2192),
            Some(Key::Unicode('→')),
            "and the rest is 0x01000000 | codepoint"
        );
        assert_eq!(scancode(0xff20), None, "a keysym with no meaning here");
    }

    /// Every entry appears once: a duplicated scancode would make two keys the
    /// same key, and a duplicated character would make the second unreachable.
    #[test]
    fn the_layout_has_no_duplicates() {
        let mut codes: Vec<u8> = US_LAYOUT.iter().map(|e| e.0).collect();
        codes.sort_unstable();
        let before = codes.len();
        codes.dedup();
        assert_eq!(codes.len(), before, "a scancode appears twice");

        let mut chars: Vec<char> = US_LAYOUT
            .iter()
            .flat_map(|e| [e.1, e.2])
            .filter(|c| *c != ' ')
            .collect();
        chars.sort_unstable();
        let before = chars.len();
        chars.dedup();
        assert_eq!(chars.len(), before, "a character appears twice");
    }
}
