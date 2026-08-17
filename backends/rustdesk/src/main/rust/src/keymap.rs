//! X11 keysyms in, their `KeyEvent` out.
//!
//! Everything above `CursorController` speaks keysyms, because that is RFB's
//! vocabulary and the extension keyboard was built against it. This protocol
//! has two ways of saying a key and they are not interchangeable:
//!
//! 1. **`ControlKey`**, a named key out of a fixed enum of theirs — every
//!    modifier, the function keys, navigation, the keypad. Pressed and released
//!    like a key, which is what makes a chord a chord.
//! 2. **`chr`**, a Unicode scalar, also pressed and released: their server turns
//!    it into a layout's key where it can and types it where it cannot, so `A`
//!    and `é` arrive without this end knowing anything about the far end's
//!    layout. It is what everything printable goes as.
//!
//! There is a third, `unicode`, which their server *types* and cannot hold
//! down. Nothing here uses it: a key that cannot be held is one that cannot be
//! part of a chord, and `chr` covers the same characters without that.
//!
//! **What has no name here is dropped rather than approximated.** F13–F24 and a
//! Japanese board's Muhenkan are in the extension row's vocabulary and not in
//! their enum, and a client that sent the nearest thing would be typing
//! something nobody asked for.

use crate::protos::message::ControlKey;

/// What a keysym turns into on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// One of their named keys.
    Control(ControlKey),
    /// A character, held down and let go like any other key.
    Char(char),
}

/// Which modifier a keysym *is*, for the list every event has to carry.
///
/// Their server releases, on the far end, every modifier not named in a down
/// event's `modifiers` — so a Ctrl held here and left out of the letter that
/// follows it is a Ctrl let go over there, and the chord never happens. Both
/// hands map to one entry, because that is what their `fix_modifiers` compares
/// against.
pub fn modifier(keysym: u32) -> Option<ControlKey> {
    Some(match keysym {
        0xffe1 | 0xffe2 => ControlKey::Shift,
        0xffe3 | 0xffe4 => ControlKey::Control,
        0xffe9 | 0xffea | 0xfe03 => ControlKey::Alt,
        0xffeb | 0xffec => ControlKey::Meta,
        _ => return None,
    })
}

/// Which modifier one of their named keys *is*, for the same list: a right
/// Control is held as a Control, because that is the only one their
/// `fix_modifiers` knows about.
pub fn modifier_of(key: ControlKey) -> Option<ControlKey> {
    Some(match key {
        ControlKey::Shift | ControlKey::RShift => ControlKey::Shift,
        ControlKey::Control | ControlKey::RControl => ControlKey::Control,
        ControlKey::Alt | ControlKey::RAlt => ControlKey::Alt,
        ControlKey::Meta | ControlKey::RWin => ControlKey::Meta,
        _ => return None,
    })
}

/// What to send for a keysym, or `None` for one this protocol cannot say.
pub fn key(keysym: u32) -> Option<Key> {
    if let Some(named) = named(keysym) {
        return Some(Key::Control(named));
    }
    character(keysym).map(Key::Char)
}

/// The named keys. Both hands where their enum has both, since a physical
/// keyboard's right Shift is a keysym of its own and the extension row's is the
/// left one.
fn named(keysym: u32) -> Option<ControlKey> {
    Some(match keysym {
        0xffe1 => ControlKey::Shift,
        0xffe2 => ControlKey::RShift,
        0xffe3 => ControlKey::Control,
        0xffe4 => ControlKey::RControl,
        0xffe9 => ControlKey::Alt,
        // Alt_R and ISO_Level3_Shift are both AltGr, which is the right Alt.
        0xffea | 0xfe03 => ControlKey::RAlt,
        0xffeb => ControlKey::Meta,
        0xffec => ControlKey::RWin,
        0xff67 => ControlKey::Apps,
        0xffe5 => ControlKey::CapsLock,
        0xff7f => ControlKey::NumLock,
        0xff14 => ControlKey::Scroll,

        0xff08 => ControlKey::Backspace,
        0xff09 => ControlKey::Tab,
        0xff0d => ControlKey::Return,
        0xff1b => ControlKey::Escape,
        0xffff => ControlKey::Delete,
        0xff63 => ControlKey::Insert,
        0xff50 => ControlKey::Home,
        0xff57 => ControlKey::End,
        0xff55 => ControlKey::PageUp,
        0xff56 => ControlKey::PageDown,
        0xff51 => ControlKey::LeftArrow,
        0xff52 => ControlKey::UpArrow,
        0xff53 => ControlKey::RightArrow,
        0xff54 => ControlKey::DownArrow,
        // X11's Print is the key marked PrintScreen, which is their Snapshot;
        // their Print is a printer key nothing sends.
        0xff61 => ControlKey::Snapshot,
        0xff13 => ControlKey::Pause,
        0xff69 => ControlKey::Cancel,

        // Their enum runs F1, F10, F11, F12, F2 … F9, so the function keys are
        // named one at a time rather than counted.
        0xffbe => ControlKey::F1,
        0xffbf => ControlKey::F2,
        0xffc0 => ControlKey::F3,
        0xffc1 => ControlKey::F4,
        0xffc2 => ControlKey::F5,
        0xffc3 => ControlKey::F6,
        0xffc4 => ControlKey::F7,
        0xffc5 => ControlKey::F8,
        0xffc6 => ControlKey::F9,
        0xffc7 => ControlKey::F10,
        0xffc8 => ControlKey::F11,
        0xffc9 => ControlKey::F12,

        // The keypad's two spellings: the digits under Num Lock, and the
        // navigation names under it off, which go to the ordinary navigation
        // keys because that is the only thing their enum has for them.
        0xffb0 => ControlKey::Numpad0,
        0xffb1 => ControlKey::Numpad1,
        0xffb2 => ControlKey::Numpad2,
        0xffb3 => ControlKey::Numpad3,
        0xffb4 => ControlKey::Numpad4,
        0xffb5 => ControlKey::Numpad5,
        0xffb6 => ControlKey::Numpad6,
        0xffb7 => ControlKey::Numpad7,
        0xffb8 => ControlKey::Numpad8,
        0xffb9 => ControlKey::Numpad9,
        0xff95 => ControlKey::Home,
        0xff96 => ControlKey::LeftArrow,
        0xff97 => ControlKey::UpArrow,
        0xff98 => ControlKey::RightArrow,
        0xff99 => ControlKey::DownArrow,
        0xff9a => ControlKey::PageUp,
        0xff9b => ControlKey::PageDown,
        0xff9c => ControlKey::End,
        0xff9d => ControlKey::Clear, // KP_Begin, the 5 with the lock off
        0xff9e => ControlKey::Insert,
        0xff9f => ControlKey::Delete,
        0xffaa => ControlKey::Multiply,
        0xffab => ControlKey::Add,
        0xffac => ControlKey::Separator,
        0xffad => ControlKey::Subtract,
        0xffae => ControlKey::Decimal,
        0xffaf => ControlKey::Divide,
        0xff8d => ControlKey::NumpadEnter,

        // Two of a Japanese board's three conversion keys. Muhenkan has no name
        // in their enum, so it is one of the keys this protocol cannot send.
        0xff27 => ControlKey::Kana,
        0xff23 => ControlKey::Convert,

        _ => return None,
    })
}

/// The character a keysym names: Latin-1 by value, and everything else through
/// the Unicode keysym range, which is what an IME's output arrives as.
fn character(keysym: u32) -> Option<char> {
    match keysym {
        0x20..=0x7e | 0xa0..=0xff => char::from_u32(keysym),
        0x0100_0000..=0x0110_ffff => char::from_u32(keysym - 0x0100_0000),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_kinds_of_key() {
        assert_eq!(key(0x41), Some(Key::Char('A')));
        assert_eq!(key(0x01000101), Some(Key::Char('ā')));
        assert_eq!(key(0xffbe), Some(Key::Control(ControlKey::F1)));
        assert_eq!(key(0xffc7), Some(Key::Control(ControlKey::F10)));
        // F13 and Muhenkan are what the extension row can say and this cannot.
        assert_eq!(key(0xffca), None);
        assert_eq!(key(0xff22), None);
    }

    #[test]
    fn both_hands_are_one_modifier() {
        assert_eq!(modifier(0xffe3), Some(ControlKey::Control));
        assert_eq!(modifier(0xffe4), Some(ControlKey::Control));
        assert_eq!(modifier(0xffbe), None);
        // and the key itself still knows which hand it was
        assert_eq!(key(0xffe4), Some(Key::Control(ControlKey::RControl)));
    }
}
