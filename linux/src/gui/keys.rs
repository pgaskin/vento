// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! A winit key as the far end wants it: an X11 keysym, and an identity for
//! the physical key that ties a release to its press.
//!
//! The keysym comes from the *logical* key, which is what the layout says
//! the key means: the key that types `y` on a German keyboard is `y` at the
//! far end too, whichever key it is on a US one. A character is its Latin-1
//! keysym where it has one and the Unicode keysym
//! (`0x01000000 | codepoint`) otherwise, which is `xkeysym`'s `from_char`; a
//! named key is looked up in a table of the ones a desktop has a use for. Modifiers are
//! told apart by side, since the far end's applications can be, and the
//! number pad by location, since `KP_1` and `1`, or `KP_Home` and `Home`,
//! are different keys to the same applications.
//!
//! Where the layout's level for the modifiers held has no name winit gives
//! or this table sends, the key's own (its first level, the layout's again)
//! is used instead, and failing that the keysym itself. The first is what
//! keeps Alt pressed after Shift: on most layouts that level of the Alt key
//! is `Meta_L`, which winit leaves unidentified, and under
//! `grp:alt_shift_toggle` it is `ISO_Next_Group`; either way the far end
//! wants the Alt it has its own layout to make sense of, and the Ctrl+Alt
//! chords want it too. The second is for a key winit has no name for at any
//! level, such as the number pad's 5 without Num Lock (`KP_Begin`): the
//! keysym is what the far end takes anyway.
//!
//! The identity is per physical key rather than per keysym, because the
//! keysym of a key can change while it is held (Shift released before the
//! letter is), and the release must still find its press. `KeyIds` hands out
//! a small integer per `PhysicalKey` as they are first seen, which is stable
//! for the life of the process and is all the backends ask of it.
//!
//! `Keyboard` keeps what the far end has down, which the one chord this end
//! reads (Ctrl+Alt with the pointer captured) is read from, and what a key
//! repeating is sent as.

use crate::backend::Backend;
use std::collections::HashMap;
use winit::event::{ElementState, KeyEvent};
use winit::keyboard::{Key, KeyCode, KeyLocation, NamedKey, NativeKey, PhysicalKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use xkeysym::Keysym;

/// The keysym for a key event, or `None` for a key with no keysym worth
/// sending (a dead key, or one with no keysym at all).
pub fn keysym(event: &KeyEvent) -> Option<u32> {
    resolve(
        &event.logical_key,
        &event.key_without_modifiers(),
        event.location,
        event.physical_key,
    )
}

/// `keysym` on its parts: the logical key, the same key without modifiers,
/// the location winit read from the logical key's keysym, and the physical
/// key.
fn resolve(
    logical: &Key,
    unmodified: &Key,
    location: KeyLocation,
    physical: PhysicalKey,
) -> Option<u32> {
    // A dead key composes here; the character it ends in is what is sent.
    if matches!(logical, Key::Dead(_)) {
        return None;
    }
    let numpad = location == KeyLocation::Numpad;
    if let Some(sym) = from_key(logical, location == KeyLocation::Right, numpad) {
        return Some(sym);
    }
    // The location came from a keysym that may say nothing of a side
    // (`ISO_Next_Group`), so the side is the physical key's then.
    let right = match location {
        KeyLocation::Right => true,
        KeyLocation::Left => false,
        _ => is_right(physical),
    };
    if let Some(sym) = from_key(unmodified, right, numpad) {
        return Some(sym);
    }
    match logical {
        Key::Unidentified(NativeKey::Xkb(sym)) if *sym != 0 => Some(*sym),
        _ => None,
    }
}

/// A key winit has named or given the character of, as a keysym.
fn from_key(key: &Key, right: bool, numpad: bool) -> Option<u32> {
    let sym = match key {
        Key::Character(text) => {
            let c = text.chars().next()?;
            if numpad {
                keypad(c)
            } else {
                Keysym::from_char(c)
            }
        }
        Key::Named(named) => named_keysym(*named, right, numpad)?,
        Key::Dead(_) | Key::Unidentified(_) => return None,
    };
    (sym != Keysym::NoSymbol).then_some(sym.raw())
}

fn is_right(physical: PhysicalKey) -> bool {
    matches!(
        physical,
        PhysicalKey::Code(
            KeyCode::AltRight | KeyCode::ControlRight | KeyCode::ShiftRight | KeyCode::SuperRight
        )
    )
}

/// The number pad's own keysyms for the characters it types.
fn keypad(c: char) -> Keysym {
    match c {
        '0'..='9' => Keysym::new(Keysym::KP_0.raw() + (c as u32 - '0' as u32)),
        '.' => Keysym::KP_Decimal,
        ',' => Keysym::KP_Separator,
        '+' => Keysym::KP_Add,
        '-' => Keysym::KP_Subtract,
        '*' => Keysym::KP_Multiply,
        '/' => Keysym::KP_Divide,
        '=' => Keysym::KP_Equal,
        other => Keysym::from_char(other),
    }
}

fn named_keysym(key: NamedKey, right: bool, numpad: bool) -> Option<Keysym> {
    let side = |left: Keysym, right_sym: Keysym| if right { right_sym } else { left };
    // The number pad without Num Lock: winit names these keys by what they
    // do, and only the location says they were the pad's.
    if numpad {
        let pad = match key {
            NamedKey::Home => Some(Keysym::KP_Home),
            NamedKey::End => Some(Keysym::KP_End),
            NamedKey::PageUp => Some(Keysym::KP_Page_Up),
            NamedKey::PageDown => Some(Keysym::KP_Page_Down),
            NamedKey::ArrowLeft => Some(Keysym::KP_Left),
            NamedKey::ArrowUp => Some(Keysym::KP_Up),
            NamedKey::ArrowRight => Some(Keysym::KP_Right),
            NamedKey::ArrowDown => Some(Keysym::KP_Down),
            NamedKey::Insert => Some(Keysym::KP_Insert),
            NamedKey::Delete => Some(Keysym::KP_Delete),
            NamedKey::Enter => Some(Keysym::KP_Enter),
            NamedKey::Tab => Some(Keysym::KP_Tab),
            NamedKey::Space => Some(Keysym::KP_Space),
            NamedKey::F1 => Some(Keysym::KP_F1),
            NamedKey::F2 => Some(Keysym::KP_F2),
            NamedKey::F3 => Some(Keysym::KP_F3),
            NamedKey::F4 => Some(Keysym::KP_F4),
            _ => None,
        };
        if pad.is_some() {
            return pad;
        }
    }
    Some(match key {
        NamedKey::Alt => side(Keysym::Alt_L, Keysym::Alt_R),
        NamedKey::AltGraph => Keysym::ISO_Level3_Shift,
        NamedKey::CapsLock => Keysym::Caps_Lock,
        NamedKey::Control => side(Keysym::Control_L, Keysym::Control_R),
        NamedKey::NumLock => Keysym::Num_Lock,
        NamedKey::ScrollLock => Keysym::Scroll_Lock,
        NamedKey::Shift => side(Keysym::Shift_L, Keysym::Shift_R),
        NamedKey::Meta | NamedKey::Super => side(Keysym::Super_L, Keysym::Super_R),
        NamedKey::Hyper => side(Keysym::Hyper_L, Keysym::Hyper_R),
        NamedKey::Enter => Keysym::Return,
        NamedKey::Tab => Keysym::Tab,
        NamedKey::Space => Keysym::space,
        NamedKey::ArrowDown => Keysym::Down,
        NamedKey::ArrowLeft => Keysym::Left,
        NamedKey::ArrowRight => Keysym::Right,
        NamedKey::ArrowUp => Keysym::Up,
        NamedKey::End => Keysym::End,
        NamedKey::Home => Keysym::Home,
        NamedKey::PageDown => Keysym::Page_Down,
        NamedKey::PageUp => Keysym::Page_Up,
        NamedKey::Backspace => Keysym::BackSpace,
        NamedKey::Clear => Keysym::Clear,
        NamedKey::Copy => Keysym::XF86_Copy,
        NamedKey::Cut => Keysym::XF86_Cut,
        NamedKey::Paste => Keysym::XF86_Paste,
        NamedKey::Delete => Keysym::Delete,
        NamedKey::Insert => Keysym::Insert,
        NamedKey::Undo => Keysym::Undo,
        NamedKey::Redo => Keysym::Redo,
        NamedKey::Cancel => Keysym::Cancel,
        NamedKey::ContextMenu => Keysym::Menu,
        NamedKey::Escape => Keysym::Escape,
        NamedKey::Execute => Keysym::Execute,
        NamedKey::Find => Keysym::Find,
        NamedKey::Help => Keysym::Help,
        NamedKey::Pause => Keysym::Pause,
        NamedKey::Select => Keysym::Select,
        NamedKey::PrintScreen => Keysym::Print,
        NamedKey::Compose => Keysym::Multi_key,
        NamedKey::Convert => Keysym::Henkan,
        NamedKey::NonConvert => Keysym::Muhenkan,
        NamedKey::ModeChange => Keysym::Mode_switch,
        NamedKey::HangulMode => Keysym::Hangul,
        NamedKey::HanjaMode => Keysym::Hangul_Hanja,
        NamedKey::Eisu => Keysym::Eisu_toggle,
        NamedKey::Hankaku => Keysym::Hankaku,
        NamedKey::Hiragana => Keysym::Hiragana,
        NamedKey::HiraganaKatakana | NamedKey::KanaMode => Keysym::Hiragana_Katakana,
        NamedKey::Katakana => Keysym::Katakana,
        NamedKey::Romaji => Keysym::Romaji,
        NamedKey::Zenkaku => Keysym::Zenkaku,
        NamedKey::ZenkakuHankaku => Keysym::Zenkaku_Hankaku,
        NamedKey::AudioVolumeDown => Keysym::XF86_AudioLowerVolume,
        NamedKey::AudioVolumeUp => Keysym::XF86_AudioRaiseVolume,
        NamedKey::AudioVolumeMute => Keysym::XF86_AudioMute,
        NamedKey::MediaPlayPause | NamedKey::MediaPlay => Keysym::XF86_AudioPlay,
        NamedKey::MediaPause => Keysym::XF86_AudioPause,
        NamedKey::MediaStop => Keysym::XF86_AudioStop,
        NamedKey::MediaTrackNext => Keysym::XF86_AudioNext,
        NamedKey::MediaTrackPrevious => Keysym::XF86_AudioPrev,
        NamedKey::BrowserBack => Keysym::XF86_Back,
        NamedKey::BrowserForward => Keysym::XF86_Forward,
        NamedKey::BrowserHome => Keysym::XF86_HomePage,
        NamedKey::BrowserRefresh => Keysym::XF86_Refresh,
        NamedKey::BrowserSearch => Keysym::XF86_Search,
        NamedKey::BrowserStop => Keysym::XF86_Stop,
        NamedKey::BrowserFavorites => Keysym::XF86_Favorites,
        NamedKey::LaunchMail => Keysym::XF86_Mail,
        NamedKey::Eject => Keysym::XF86_Eject,
        NamedKey::Power => Keysym::XF86_PowerOff,
        NamedKey::Standby => Keysym::XF86_Sleep,
        NamedKey::WakeUp => Keysym::XF86_WakeUp,
        NamedKey::F1 => Keysym::F1,
        NamedKey::F2 => Keysym::F2,
        NamedKey::F3 => Keysym::F3,
        NamedKey::F4 => Keysym::F4,
        NamedKey::F5 => Keysym::F5,
        NamedKey::F6 => Keysym::F6,
        NamedKey::F7 => Keysym::F7,
        NamedKey::F8 => Keysym::F8,
        NamedKey::F9 => Keysym::F9,
        NamedKey::F10 => Keysym::F10,
        NamedKey::F11 => Keysym::F11,
        NamedKey::F12 => Keysym::F12,
        NamedKey::F13 => Keysym::F13,
        NamedKey::F14 => Keysym::F14,
        NamedKey::F15 => Keysym::F15,
        NamedKey::F16 => Keysym::F16,
        NamedKey::F17 => Keysym::F17,
        NamedKey::F18 => Keysym::F18,
        NamedKey::F19 => Keysym::F19,
        NamedKey::F20 => Keysym::F20,
        NamedKey::F21 => Keysym::F21,
        NamedKey::F22 => Keysym::F22,
        NamedKey::F23 => Keysym::F23,
        NamedKey::F24 => Keysym::F24,
        _ => return None,
    })
}

/// A stable small integer per physical key, handed out as keys are first
/// seen. Only ever grows, by one entry per distinct key on the keyboard.
#[derive(Default)]
pub struct KeyIds {
    ids: HashMap<PhysicalKey, i64>,
}

impl KeyIds {
    pub fn id(&mut self, key: PhysicalKey) -> i64 {
        let next = self.ids.len() as i64 + 1;
        *self.ids.entry(key).or_insert(next)
    }
}

/// A key event as the far end's keyboard is kept from it: winit's
/// `KeyEvent` cannot be made outside winit, and this is all of it that is
/// read.
#[derive(Clone, Copy, Debug)]
pub struct Stroke {
    pub physical: PhysicalKey,
    pub pressed: bool,
    /// The press is the key repeating, not a press of its own.
    pub repeat: bool,
    pub keysym: Option<u32>,
}

impl Stroke {
    pub fn of(event: &KeyEvent) -> Stroke {
        Stroke {
            physical: event.physical_key,
            pressed: event.state == ElementState::Pressed,
            repeat: event.repeat,
            keysym: keysym(event),
        }
    }
}

/// What a key handed to the far end came to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Taken {
    /// It went to the far end, as a press (the overlay's latency counts it).
    pub sent: bool,
    /// It made Ctrl+Alt with the pointer captured: let the pointer go.
    pub escape: bool,
}

/// The keys as the far end has them, which is not always as they are
/// here: everything held when input goes elsewhere (a prompt, view only,
/// the toolbar) is let go of at the far end, while it stays down here.
///
/// A key repeating is held: its repeats go on to the far end as the
/// presses its own autorepeat would make, but only while the far end has
/// it down, and they are never the escape. A key the far end does not have
/// down is sent again only once it is pressed again, after its release.
///
/// The escape's keys go to the far end like any others, and stay down there
/// until they are released here, so that whatever the far end does with
/// Ctrl+Alt (or with a key pressed while they are held) it still does.
#[derive(Default)]
pub struct Keyboard {
    ids: KeyIds,
    /// The physical keys down at the far end, and the keysym each went
    /// down as.
    held: HashMap<PhysicalKey, u32>,
}

impl Keyboard {
    /// A key that is the far end's. `captured` is whether the pointer is,
    /// which makes Ctrl+Alt the escape.
    pub fn key(&mut self, stroke: Stroke, captured: bool, backend: &dyn Backend) -> Taken {
        let physical = stroke.physical;
        let mut taken = Taken::default();
        if !stroke.pressed {
            self.up(physical, backend);
            return taken;
        }
        if stroke.repeat {
            if let Some(keysym) = stroke.keysym.filter(|_| self.held.contains_key(&physical)) {
                self.press(physical, keysym, backend);
                taken.sent = true;
            }
            return taken;
        }
        if let Some(keysym) = stroke.keysym {
            self.press(physical, keysym, backend);
            taken.sent = true;
        }
        taken.escape = captured && chord(&self.held);
        taken
    }

    /// A key that is egui's rather than the far end's: view only, a prompt,
    /// a text field. It goes no further, except that a key that went down
    /// at the far end before its release came here (a text field taking the
    /// keyboard mid-press) is released there, which nothing else would do
    /// until the focus left.
    pub fn elsewhere(&mut self, stroke: Stroke, backend: &dyn Backend) {
        if !stroke.pressed {
            self.up(stroke.physical, backend);
        }
    }

    /// Whether the server has a modifier down that a window manager's mouse
    /// bindings could use: Control, Alt, Meta, Super, Hyper or AltGr, but
    /// not Shift (see `Session::holds_pointer`).
    pub fn modifier_held(&self) -> bool {
        const MODIFIERS: [Keysym; 11] = [
            Keysym::Control_L,
            Keysym::Control_R,
            Keysym::Alt_L,
            Keysym::Alt_R,
            Keysym::Meta_L,
            Keysym::Meta_R,
            Keysym::Super_L,
            Keysym::Super_R,
            Keysym::Hyper_L,
            Keysym::Hyper_R,
            Keysym::ISO_Level3_Shift,
        ];
        self.held.values().any(|&k| MODIFIERS.iter().any(|m| m.raw() == k))
    }

    /// How many keys the far end has down: the overlay's count.
    pub fn held(&self) -> usize {
        self.held.len()
    }

    /// Lets go of every key the far end has down; here they stay down until
    /// they are released.
    pub fn release_all(&mut self, backend: &dyn Backend) {
        if !self.held.is_empty() {
            self.held.clear();
            backend.release_all_keys();
        }
    }

    /// The focus left: every key let go of at the far end, since their
    /// releases will go elsewhere.
    pub fn focus_lost(&mut self, backend: &dyn Backend) {
        self.release_all(backend);
    }

    fn press(&mut self, physical: PhysicalKey, keysym: u32, backend: &dyn Backend) {
        let id = self.ids.id(physical);
        self.held.insert(physical, keysym);
        // Trace, not debug: a line per key names everything typed, a
        // password into the far end's prompt among it (see `log::init`).
        log::trace!("key down {keysym:#x} as {id}");
        backend.key_down(keysym, id);
    }

    fn up(&mut self, physical: PhysicalKey, backend: &dyn Backend) {
        if self.held.remove(&physical).is_some() {
            let id = self.ids.id(physical);
            log::trace!("key up as {id}");
            backend.key_up(id);
        }
    }
}

/// Ctrl and Alt, either side, among `keys` together. By keysym rather than
/// by key code, so that a keyboard whose codes are not the usual ones (a
/// virtual one, say) still has the chord.
fn chord(keys: &HashMap<PhysicalKey, u32>) -> bool {
    let down = |a: Keysym, b: Keysym| keys.values().any(|&k| k == a.raw() || k == b.raw());
    down(Keysym::Control_L, Keysym::Control_R) && down(Keysym::Alt_L, Keysym::Alt_R)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn characters_are_latin_1_or_unicode_keysyms() {
        assert_eq!(Keysym::from_char('a').raw(), 0x61);
        assert_eq!(Keysym::from_char('é').raw(), 0xe9);
        assert_eq!(Keysym::from_char('€').raw(), 0x20ac);
        // A character with a legacy keysym gets that; one without gets the
        // Unicode keysym.
        assert_eq!(Keysym::from_char('я').raw(), Keysym::Cyrillic_ya.raw());
        assert_eq!(Keysym::from_char('😀').raw(), 0x0101_f600);
        assert_eq!(keypad('7').raw(), Keysym::KP_7.raw());
        assert_eq!(keypad('.').raw(), Keysym::KP_Decimal.raw());
    }

    #[test]
    fn named_keys_tell_their_side_and_pad_apart() {
        assert_eq!(named_keysym(NamedKey::Shift, false, false), Some(Keysym::Shift_L));
        assert_eq!(named_keysym(NamedKey::Shift, true, false), Some(Keysym::Shift_R));
        assert_eq!(named_keysym(NamedKey::Enter, false, true), Some(Keysym::KP_Enter));
        assert_eq!(named_keysym(NamedKey::Enter, false, false), Some(Keysym::Return));
        assert_eq!(named_keysym(NamedKey::PageDown, false, false), Some(Keysym::Page_Down));
        assert_eq!(named_keysym(NamedKey::TVPower, false, false), None);
    }

    #[test]
    fn the_number_pad_sends_its_own_keysyms_with_num_lock_off_or_on() {
        let at = PhysicalKey::Code(KeyCode::Numpad7);
        let pad = |key: Key| resolve(&key, &key, KeyLocation::Numpad, at);
        assert_eq!(pad(Key::Named(NamedKey::Home)), Some(Keysym::KP_Home.raw()));
        assert_eq!(pad(Key::Named(NamedKey::ArrowUp)), Some(Keysym::KP_Up.raw()));
        assert_eq!(pad(Key::Named(NamedKey::PageDown)), Some(Keysym::KP_Page_Down.raw()));
        assert_eq!(pad(Key::Named(NamedKey::Insert)), Some(Keysym::KP_Insert.raw()));
        assert_eq!(pad(Key::Named(NamedKey::Delete)), Some(Keysym::KP_Delete.raw()));
        assert_eq!(pad(Key::Named(NamedKey::Enter)), Some(Keysym::KP_Enter.raw()));
        assert_eq!(pad(Key::Character("7".into())), Some(Keysym::KP_7.raw()));
        assert_eq!(pad(Key::Character(".".into())), Some(Keysym::KP_Decimal.raw()));
        // The 5 without Num Lock, which winit has no name for.
        let begin = Key::Unidentified(NativeKey::Xkb(Keysym::KP_Begin.raw()));
        assert_eq!(pad(begin), Some(Keysym::KP_Begin.raw()));
        // The same names elsewhere are the keys they name.
        let home = Key::Named(NamedKey::Home);
        let at = PhysicalKey::Code(KeyCode::Home);
        assert_eq!(resolve(&home, &home, KeyLocation::Standard, at), Some(Keysym::Home.raw()));
    }

    #[test]
    fn a_modifier_s_unnamed_level_is_the_key_itself() {
        // Alt with Shift held: Meta_L on the usual layouts.
        let meta = |sym: Keysym| Key::Unidentified(NativeKey::Xkb(sym.raw()));
        let alt = Key::Named(NamedKey::Alt);
        let left = PhysicalKey::Code(KeyCode::AltLeft);
        let right = PhysicalKey::Code(KeyCode::AltRight);
        assert_eq!(
            resolve(&meta(Keysym::Meta_L), &alt, KeyLocation::Left, left),
            Some(Keysym::Alt_L.raw())
        );
        assert_eq!(
            resolve(&meta(Keysym::Meta_R), &alt, KeyLocation::Right, right),
            Some(Keysym::Alt_R.raw())
        );
        // A layout whose Alt key is something else unshifted: the layout's
        // key, not the position's (altwin:swap_alt_win has Alt on the
        // Windows key).
        let win = PhysicalKey::Code(KeyCode::SuperLeft);
        assert_eq!(
            resolve(&meta(Keysym::Meta_L), &alt, KeyLocation::Left, win),
            Some(Keysym::Alt_L.raw())
        );
        // grp:alt_shift_toggle: a group switch, which says no side.
        let group = Key::Named(NamedKey::GroupNext);
        assert_eq!(
            resolve(&group, &alt, KeyLocation::Standard, right),
            Some(Keysym::Alt_R.raw())
        );
        // No name at any level: the keysym as it is.
        assert_eq!(
            resolve(&meta(Keysym::Meta_L), &meta(Keysym::Meta_L), KeyLocation::Left, win),
            Some(Keysym::Meta_L.raw())
        );
        // A named level is taken as it is, whatever the key is unshifted.
        let tab = Key::Named(NamedKey::Tab);
        let at = PhysicalKey::Code(KeyCode::Tab);
        assert_eq!(resolve(&tab, &tab, KeyLocation::Standard, at), Some(Keysym::Tab.raw()));
    }

    #[test]
    fn dead_and_unknown_keys_send_nothing() {
        let at = PhysicalKey::Code(KeyCode::Quote);
        let dead = Key::Dead(Some('´'));
        assert_eq!(resolve(&dead, &Key::Character("'".into()), KeyLocation::Standard, at), None);
        let none = Key::Unidentified(NativeKey::Unidentified);
        assert_eq!(resolve(&none, &none, KeyLocation::Standard, at), None);
    }

    #[test]
    fn key_ids_are_stable_per_physical_key() {
        let mut ids = KeyIds::default();
        let a = ids.id(PhysicalKey::Code(KeyCode::KeyA));
        let b = ids.id(PhysicalKey::Code(KeyCode::KeyB));
        assert_ne!(a, b);
        assert_eq!(ids.id(PhysicalKey::Code(KeyCode::KeyA)), a);
        assert!(a > 0 && b > 0);
    }

    // ---- the keyboard's bookkeeping, against a backend that writes down
    // what it was told --------------------------------------------------------

    use crate::backend::{ConnectionFact, Credentials, Facts, Picture, Traffic};
    use std::sync::Mutex;

    #[derive(Debug, PartialEq, Eq)]
    enum Told {
        Down(u32, i64),
        Up(i64),
        ReleaseAll,
    }

    #[derive(Default)]
    struct Far(Mutex<Vec<Told>>);

    impl Far {
        fn told(&self) -> Vec<Told> {
            std::mem::take(&mut *self.0.lock().unwrap())
        }
    }

    impl Backend for Far {
        fn pointer(&self, _: i32, _: i32, _: i32) {}
        fn key_down(&self, keysym: u32, key_id: i64) {
            self.0.lock().unwrap().push(Told::Down(keysym, key_id));
        }
        fn key_up(&self, key_id: i64) {
            self.0.lock().unwrap().push(Told::Up(key_id));
        }
        fn release_all_keys(&self) {
            self.0.lock().unwrap().push(Told::ReleaseAll);
        }
        fn focus(&self, _: bool) {}
        fn set_view_only(&self, _: bool) {}
        fn clipboard(&self, _: &str) {}
        fn set_option(&self, _: &str, _: &str) {}
        fn facts(&self) -> Facts {
            Facts::default()
        }
        fn info(&self) -> Vec<ConnectionFact> {
            Vec::new()
        }
        fn traffic(&self) -> Option<Traffic> {
            None
        }
        fn read_region(&self, _: usize, _: usize, _: usize, _: usize, _: &mut [u8], _: usize, _: usize, _: usize) -> bool {
            false
        }
        fn thumbnail(&self, _: usize) -> Option<Picture> {
            None
        }
        fn answer_credentials(&self, _: Option<Credentials>) {}
        fn answer_trust(&self, _: bool) {}
        fn disconnect(&self) {}
        fn destroy(&self) {}
    }

    const CTRL: KeyCode = KeyCode::ControlLeft;
    const ALT: KeyCode = KeyCode::AltLeft;

    fn sym(code: KeyCode) -> u32 {
        match code {
            CTRL => Keysym::Control_L.raw(),
            ALT => Keysym::Alt_L.raw(),
            KeyCode::SuperLeft => Keysym::Super_L.raw(),
            KeyCode::ShiftLeft => Keysym::Shift_L.raw(),
            KeyCode::KeyA => 'a' as u32,
            _ => unreachable!(),
        }
    }

    fn stroke(code: KeyCode, pressed: bool, repeat: bool) -> Stroke {
        Stroke {
            physical: PhysicalKey::Code(code),
            pressed,
            repeat,
            keysym: Some(sym(code)),
        }
    }

    fn press(code: KeyCode) -> Stroke {
        stroke(code, true, false)
    }

    fn repeat(code: KeyCode) -> Stroke {
        stroke(code, true, true)
    }

    fn release(code: KeyCode) -> Stroke {
        stroke(code, false, false)
    }

    const SENT: Taken = Taken { sent: true, escape: false };
    const NOTHING: Taken = Taken { sent: false, escape: false };
    const ESCAPE: Taken = Taken { sent: true, escape: true };

    #[test]
    fn a_modifier_held_is_one_down_at_the_server_other_than_shift() {
        let far = Far::default();
        let mut keyboard = Keyboard::default();
        keyboard.key(press(KeyCode::ShiftLeft), false, &far);
        keyboard.key(press(KeyCode::KeyA), false, &far);
        assert!(!keyboard.modifier_held(), "Shift and a letter");
        keyboard.key(press(KeyCode::SuperLeft), false, &far);
        assert!(keyboard.modifier_held());
        keyboard.key(release(KeyCode::SuperLeft), false, &far);
        assert!(!keyboard.modifier_held());
        keyboard.key(press(ALT), false, &far);
        assert!(keyboard.modifier_held());
        keyboard.release_all(&far);
        assert!(!keyboard.modifier_held(), "let go of at the server");
    }

    #[test]
    fn ctrl_alt_is_the_escape_only_while_captured() {
        let far = Far::default();
        let mut keyboard = Keyboard::default();
        assert_eq!(keyboard.key(press(CTRL), false, &far), SENT);
        assert_eq!(keyboard.key(press(ALT), false, &far), SENT, "no escape without a capture");
        keyboard.key(release(ALT), false, &far);
        assert_eq!(keyboard.key(press(ALT), true, &far), ESCAPE);
    }

    #[test]
    fn the_escape_reaches_the_far_end_and_stays_down_there_until_released() {
        let far = Far::default();
        let mut keyboard = Keyboard::default();
        assert_eq!(keyboard.key(press(CTRL), true, &far), SENT);
        assert_eq!(keyboard.key(press(ALT), true, &far), ESCAPE);
        let (ctrl, alt) = (keyboard.ids.id(PhysicalKey::Code(CTRL)), keyboard.ids.id(PhysicalKey::Code(ALT)));
        assert_eq!(far.told(), [Told::Down(sym(CTRL), ctrl), Told::Down(sym(ALT), alt)]);
        assert_eq!(keyboard.held(), 2);
        // The pointer let go, the Alt held on repeats there as it would on
        // its own keyboard, and is never the escape again.
        for _ in 0..3 {
            assert_eq!(keyboard.key(repeat(ALT), false, &far), SENT);
            assert_eq!(keyboard.key(repeat(ALT), true, &far), SENT, "a repeat is not the escape");
        }
        // A key pressed with them held is a chord at the far end.
        assert_eq!(keyboard.key(press(KeyCode::KeyA), false, &far), SENT);
        let a = keyboard.ids.id(PhysicalKey::Code(KeyCode::KeyA));
        for code in [KeyCode::KeyA, ALT, CTRL] {
            assert_eq!(keyboard.key(release(code), false, &far), NOTHING);
        }
        let mut told = far.told();
        told.retain(|t| *t != Told::Down(sym(ALT), alt));
        assert_eq!(told, [Told::Down(sym(KeyCode::KeyA), a), Told::Up(a), Told::Up(alt), Told::Up(ctrl)]);
        assert_eq!(keyboard.held(), 0);
    }

    #[test]
    fn a_key_held_repeats_at_the_far_end_under_its_own_identity() {
        let far = Far::default();
        let mut keyboard = Keyboard::default();
        for stroke in [press(KeyCode::KeyA), repeat(KeyCode::KeyA), repeat(KeyCode::KeyA)] {
            assert_eq!(keyboard.key(stroke, true, &far), SENT);
        }
        assert_eq!(keyboard.held(), 1);
        keyboard.key(release(KeyCode::KeyA), true, &far);
        let a = keyboard.ids.id(PhysicalKey::Code(KeyCode::KeyA));
        let down = || Told::Down(sym(KeyCode::KeyA), a);
        assert_eq!(far.told(), [down(), down(), down(), Told::Up(a)]);
        assert_eq!(keyboard.held(), 0);
    }

    #[test]
    fn a_key_let_go_of_while_input_was_elsewhere_resumes_only_when_pressed_again() {
        // Held while a prompt came up (or the pointer went over the
        // toolbar): let go of at the far end, still down here.
        let far = Far::default();
        let mut keyboard = Keyboard::default();
        keyboard.key(press(KeyCode::KeyA), false, &far);
        keyboard.release_all(&far);
        keyboard.elsewhere(repeat(KeyCode::KeyA), &far);
        // Input back, the key still repeating: the far end is not handed a
        // press it never saw begin.
        assert_eq!(keyboard.key(repeat(KeyCode::KeyA), false, &far), NOTHING);
        assert_eq!(keyboard.key(release(KeyCode::KeyA), false, &far), NOTHING);
        assert_eq!(far.told()[1..], [Told::ReleaseAll]);
    }

    #[test]
    fn a_key_released_elsewhere_is_released_at_the_far_end() {
        let far = Far::default();
        let mut keyboard = Keyboard::default();
        // A key down at the far end whose release went elsewhere (a text
        // field took the keyboard mid-press) is released there.
        keyboard.key(press(KeyCode::KeyA), false, &far);
        keyboard.elsewhere(release(KeyCode::KeyA), &far);
        let a = keyboard.ids.id(PhysicalKey::Code(KeyCode::KeyA));
        assert_eq!(far.told()[1..], [Told::Up(a)]);
        // A press elsewhere is not sent, and neither is its release.
        keyboard.elsewhere(press(CTRL), &far);
        keyboard.elsewhere(release(CTRL), &far);
        assert_eq!(far.told(), []);
        // The focus gone, everything is up at the far end.
        keyboard.key(press(CTRL), false, &far);
        far.told();
        keyboard.focus_lost(&far);
        assert_eq!(far.told(), [Told::ReleaseAll]);
        assert_eq!(keyboard.held(), 0);
    }
}
