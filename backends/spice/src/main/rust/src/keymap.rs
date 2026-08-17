// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! X11 keysyms in, PC/AT scancodes out.
//!
//! The one place the protocol does not match the stack. Everything above
//! `CursorController` speaks keysyms, because that is RFB's vocabulary and the
//! extension keyboard was built against it; SPICE's keyboard is the **AT
//! keyboard's own wire**, set 1, with an `E0` prefix for the keys the original
//! PC did not have — closer to RDP's model than to RFB's, and this is that
//! table in a third dialect.
//!
//! Two things follow, and the second is a limit rather than a decision:
//!
//! 1. **A scancode is a position, not a character.** `0x1E` is "the key left of
//!    S", which is `a` on a US layout and `q` on a French one. So this table is
//!    a US layout and says so, and the guest's own layout turns a position back
//!    into a character. Shift is part of the answer: `!` is `1` with Shift
//!    held, so a mapping is a position *and* whether the layout needs Shift,
//!    and the caller synthesises the modifier.
//! 2. **What the layout cannot place cannot be sent at all.** RDP has a
//!    Unicode keyboard event to fall back to and SPICE has nothing of the kind
//!    — the inputs channel carries scancodes and only scancodes. So an IME's
//!    `é` reaches a Windows RDP guest and does not reach a SPICE one, and the
//!    same is true of spice-gtk. What a phone can type here is what a US
//!    keyboard has keys for, until the agent's clipboard makes paste the way
//!    round it (46c).
//!
//! The wire form is a `u32` holding the bytes a keyboard would put on the wire,
//! least significant first: `0x1E` for A, `0xE04B` for Left, and the break code
//! is the make code with bit 7 set — `0x9E`, `0xE0CB`. That is what spice-gtk
//! sends and what their own `make_scancode` builds.

/// What a keysym turns into on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// A position on the keyboard, and whether the US layout needs Shift to
    /// produce the character that keysym names. `code` is the set-1 scancode
    /// with [`EXTENDED`] set for the `E0` half of the keyboard.
    Scancode { code: u16, shift: bool },
    /// Pause, which is not a scancode at all — see [`Key::press`].
    Pause,
}

/// The `E0` prefix, as a bit rather than a byte: it is a property of the key
/// and the wire form puts it back.
pub const EXTENDED: u16 = 0x100;

impl Key {
    /// The make code, as the inputs channel carries it.
    pub fn press(self) -> u32 {
        match self {
            Key::Scancode { code, .. } => wire(code, false),
            // The one key that never had a scancode of its own: what a keyboard
            // puts on the wire for it is the `E1` sequence `1D 45`, and there is
            // no break code at all — the keyboard sends the release in the same
            // burst. Packed like an extended key, least significant byte first.
            Key::Pause => 0x45_1d_e1,
        }
    }

    /// The break code, or `None` for a key that has none.
    pub fn release(self) -> Option<u32> {
        match self {
            Key::Scancode { code, .. } => Some(wire(code, true)),
            Key::Pause => None,
        }
    }
}

fn wire(code: u16, release: bool) -> u32 {
    let base = (code & 0xff) as u32 | if release { 0x80 } else { 0 };
    if code & EXTENDED != 0 {
        (base << 8) | 0xe0
    } else {
        base
    }
}

const fn ext(code: u8) -> u16 {
    EXTENDED | code as u16
}

/// The named keys: modifiers, editing, navigation, function keys, the keypad.
///
/// Both hands, since a physical keyboard's right Shift is a keysym of its own
/// and the extension row's is the left one.
fn named(keysym: u32) -> Option<u16> {
    Some(match keysym {
        0xffe1 => 0x2a,               // Shift_L
        0xffe2 => 0x36,               // Shift_R
        0xffe3 => 0x1d,               // Control_L
        0xffe4 => ext(0x1d),          // Control_R
        0xffe9 => 0x38,               // Alt_L
        0xffea | 0xfe03 => ext(0x38), // Alt_R, ISO_Level3_Shift — both AltGr
        0xffeb => ext(0x5b),          // Super_L
        0xffec => ext(0x5c),          // Super_R
        0xff67 => ext(0x5d),          // Menu
        0xffe5 => 0x3a,               // Caps_Lock
        0xff7f => 0x45,               // Num_Lock
        0xff14 => 0x46,               // Scroll_Lock

        0xff08 => 0x0e,      // BackSpace
        0xff09 => 0x0f,      // Tab
        0xff0d => 0x1c,      // Return
        0xff1b => 0x01,      // Escape
        0xffff => ext(0x53), // Delete
        0xff63 => ext(0x52), // Insert
        0xff50 => ext(0x47), // Home
        0xff57 => ext(0x4f), // End
        0xff55 => ext(0x49), // Page_Up
        0xff56 => ext(0x51), // Page_Down
        0xff51 => ext(0x4b), // Left
        0xff52 => ext(0x48), // Up
        0xff53 => ext(0x4d), // Right
        0xff54 => ext(0x50), // Down
        0xff61 => ext(0x37), // Print

        // F1–F10 are consecutive, F11 and F12 are not: the 101-key keyboard
        // added them after the numeric keypad's codes were already taken.
        0xffbe..=0xffc7 => 0x3b + (keysym - 0xffbe) as u16,
        0xffc8 => 0x57, // F11
        0xffc9 => 0x58, // F12
        // F13–F23 run from 0x64; F24 does not follow them, and the position
        // that would be F24 is what a keyboard type calls VK_OEM_PA3.
        0xffca..=0xffd4 => 0x64 + (keysym - 0xffca) as u16,
        0xffd5 => 0x76, // F24

        // The keypad. Each key appears twice, once under each Num Lock state,
        // and both spellings are the same position — which is the whole of what
        // a scancode can say. Which of the two the guest produces is decided by
        // the guest's own Num Lock and not by ours.
        0xffb7 | 0xff95 => 0x47, // KP_7, KP_Home
        0xffb8 | 0xff97 => 0x48, // KP_8, KP_Up
        0xffb9 | 0xff9a => 0x49, // KP_9, KP_Prior
        0xffb4 | 0xff96 => 0x4b, // KP_4, KP_Left
        0xffb5 | 0xff9d => 0x4c, // KP_5, KP_Begin
        0xffb6 | 0xff98 => 0x4d, // KP_6, KP_Right
        0xffb1 | 0xff9c => 0x4f, // KP_1, KP_End
        0xffb2 | 0xff99 => 0x50, // KP_2, KP_Down
        0xffb3 | 0xff9b => 0x51, // KP_3, KP_Next
        0xffb0 | 0xff9e => 0x52, // KP_0, KP_Insert
        0xffae | 0xff9f => 0x53, // KP_Decimal, KP_Delete
        0xffaa => 0x37,          // KP_Multiply
        0xffad => 0x4a,          // KP_Subtract
        0xffab => 0x4e,          // KP_Add
        0xffaf => ext(0x35),     // KP_Divide
        0xff8d => ext(0x1c),     // KP_Enter
        0xffac => 0x7e,          // KP_Separator, the Brazilian keypad comma

        // The unambiguous three of a Japanese board's conversion keys, which
        // are the three `Keysym` sends.
        0xff27 => 0x70, // Hiragana_Katakana
        0xff23 => 0x79, // Henkan_Mode
        0xff22 => 0x7b, // Muhenkan

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

/// What to send for a keysym, or `None` for one this keyboard has no position
/// for — which here means it is not sent at all.
pub fn scancode(keysym: u32) -> Option<Key> {
    if keysym == 0xff13 {
        return Some(Key::Pause);
    }
    if let Some(code) = named(keysym) {
        return Some(Key::Scancode { code, shift: false });
    }
    let ch = char::from_u32(unicode(keysym)?)?;
    for &(code, lower, upper) in US_LAYOUT {
        if ch == lower {
            return Some(Key::Scancode {
                code: code as u16,
                shift: false,
            });
        }
        if ch == upper {
            return Some(Key::Scancode {
                code: code as u16,
                shift: true,
            });
        }
    }
    None
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
            Some(Key::Scancode { code, shift }) => (code, shift),
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
        assert_eq!(code_of(0xff51), (ext(0x4b), false), "Left is extended");
        assert_eq!(code_of(0xffe1), (0x2a, false), "Shift_L is not");
        assert_eq!(code_of(0xfe03), (ext(0x38), false), "AltGr is right Alt");
        assert_eq!(code_of(0xffff), (ext(0x53), false), "Delete");
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
        assert_eq!(code_of(0xffaf), (ext(0x35), false), "KP_Divide is extended");
        assert_eq!(code_of(0xff8d), (ext(0x1c), false), "and so is KP_Enter");
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
        assert_eq!(code_of('?' as u32), (0x35, true));
    }

    /// The limit this protocol has and RDP does not: there is no Unicode key
    /// event to fall back to, so a character with no position on a US keyboard
    /// is not sent.
    #[test]
    fn what_the_layout_cannot_place_is_not_sent() {
        assert_eq!(scancode(0xe9), None, "é is Latin-1 and still has no key");
        assert_eq!(scancode(0x0100_2192), None, "nor has →");
        assert_eq!(scancode(0xffbd), None, "nor KP_Equal, which no PC has");
        assert_eq!(scancode(0xff20), None, "a keysym with no meaning here");
    }

    /// The wire form is the bytes a keyboard sends, least significant first,
    /// and a release is the make code with bit 7 set.
    #[test]
    fn the_wire_form_is_what_a_keyboard_puts_on_it() {
        let a = scancode('a' as u32).unwrap();
        assert_eq!(a.press(), 0x1e);
        assert_eq!(a.release(), Some(0x9e));

        let left = scancode(0xff51).unwrap();
        assert_eq!(left.press(), 0x4be0, "E0 4B");
        assert_eq!(left.release(), Some(0xcbe0), "E0 CB");

        assert_eq!(Key::Pause.press(), 0x451de1);
        assert_eq!(Key::Pause.release(), None, "Pause releases itself");
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
