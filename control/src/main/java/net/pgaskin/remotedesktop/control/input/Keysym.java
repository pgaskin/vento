// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.input;

/**
 * X11 keysyms, and the two mappings onto them that an Android client needs.
 *
 * <p>Only the ones the keyboard names are here; everything else arrives as a
 * character and goes through {@link #fromUnicode}.
 */
public final class Keysym {

    private Keysym() {
    }

    // Modifiers. The right-hand ones are keys of their own, which Android says
    // and every backend here can carry; right Alt is the exception and is a
    // level rather than a modifier (see fromAndroidKeyCode).
    public static final int SHIFT_L = 0xffe1;             // 65505
    public static final int SHIFT_R = 0xffe2;
    public static final int CONTROL_L = 0xffe3;           // 65507
    public static final int CONTROL_R = 0xffe4;
    public static final int ALT_L = 0xffe9;               // 65513
    public static final int SUPER_L = 0xffeb;             // 65515
    public static final int SUPER_R = 0xffec;
    public static final int ISO_LEVEL3_SHIFT = 0xfe03;    // 65027 — AltGr / macOS Option

    // Editing and navigation.
    public static final int BACKSPACE = 0xff08;
    public static final int TAB = 0xff09;
    public static final int RETURN = 0xff0d;
    public static final int ESCAPE = 0xff1b;
    public static final int HOME = 0xff50;
    public static final int LEFT = 0xff51;
    public static final int UP = 0xff52;
    public static final int RIGHT = 0xff53;
    public static final int DOWN = 0xff54;
    public static final int PAGE_UP = 0xff55;
    public static final int PAGE_DOWN = 0xff56;
    public static final int END = 0xff57;
    public static final int INSERT = 0xff63;
    public static final int DELETE = 0xffff;

    // Keys only a physical keyboard has. The lock keys are deliberately absent
    // from fromAndroidKeyCode: see there.
    public static final int MENU = 0xff67;
    public static final int PRINT = 0xff61;
    public static final int PAUSE = 0xff13;
    // The conversion keys on a Japanese board. The two that switch kana are not
    // here, and that is a finding rather than an omission: see fromAndroidKeyCode.
    public static final int MUHENKAN = 0xff22;
    public static final int HENKAN_MODE = 0xff23;
    public static final int HIRAGANA_KATAKANA = 0xff27;
    // The three dedicated editing keys, which core X11 never named — these are
    // XFree86's, and are what a Linux desktop binds them to.
    public static final int XF86_COPY = 0x1008ff57;
    public static final int XF86_CUT = 0x1008ff58;
    public static final int XF86_PASTE = 0x1008ff6d;

    // The keypad; see keypadKeysym for why it is a separate set of keysyms.
    public static final int KP_ENTER = 0xff8d;
    public static final int KP_MULTIPLY = 0xffaa;
    public static final int KP_ADD = 0xffab;
    public static final int KP_SUBTRACT = 0xffad;
    public static final int KP_DECIMAL = 0xffae;
    public static final int KP_DIVIDE = 0xffaf;
    public static final int KP_EQUAL = 0xffbd;
    public static final int KP_SEPARATOR = 0xffac;
    /** {@code KP_0} … {@code KP_9} are consecutive from here. */
    public static final int KP_0 = 0xffb0;
    // The Num-Lock-off half of the keypad, in Android keycode order below.
    public static final int KP_INSERT = 0xff9e;
    public static final int KP_END = 0xff9c;
    public static final int KP_DOWN = 0xff99;
    public static final int KP_PAGE_DOWN = 0xff9b;
    public static final int KP_LEFT = 0xff96;
    public static final int KP_BEGIN = 0xff9d;
    public static final int KP_RIGHT = 0xff98;
    public static final int KP_HOME = 0xff95;
    public static final int KP_UP = 0xff97;
    public static final int KP_PAGE_UP = 0xff9a;
    public static final int KP_DELETE = 0xff9f;

    /** {@code F1} … {@code F24} are consecutive from here. */
    public static final int F1 = 0xffbe;

    // KeyCharacterMap.COMBINING_ACCENT, as a literal for the same reason the
    // keycodes below are: this class stays off Android.
    private static final int COMBINING_ACCENT = 0x80000000;

    public static int f(int n) {
        return F1 + (n - 1);
    }

    /**
     * A Unicode code point as a keysym, by the standard X11 rule: Latin-1 is
     * itself, everything else is {@code 0x01000000 | codepoint}.
     */
    public static int fromUnicode(int cp) {
        if ((cp >= 0x20 && cp <= 0x7e) || (cp >= 0xa0 && cp <= 0xff)) {
            return cp;
        }
        return 0x01000000 | cp;
    }

    /**
     * What {@code KeyEvent.getUnicodeChar} returned, as a keysym — which is not
     * the same question as {@link #fromUnicode}, because the value is not always
     * a code point. A layout with dead keys answers with the accent and bit 31
     * set, and that value read as a code point is negative: one path here made
     * {@code 0x81000300} of it, a keysym in no encoding, and the other took it
     * for a control character and dropped it.
     */
    public static int fromKeyChar(int unicodeChar) {
        if (unicodeChar == 0) {
            return 0;   // no character, rather than the encoding of code point 0
        }
        if ((unicodeChar & COMBINING_ACCENT) != 0) {
            return deadKeysym(unicodeChar & ~COMBINING_ACCENT);
        }
        return fromUnicode(unicodeChar);
    }

    /**
     * The X11 dead key for a combining accent. A dead key is a key: what makes
     * it worth pressing is that the far end composes it with whatever follows,
     * so sending the accent as a character would place an accent and compose
     * nothing. An accent with no dead key of its own goes as the character it
     * is, which is what a far end that does not compose would have produced.
     */
    public static int deadKeysym(int accent) {
        return switch (accent) {
            case 0x0300 -> 0xfe50;   // dead_grave
            case 0x0301 -> 0xfe51;   // dead_acute
            case 0x0302 -> 0xfe52;   // dead_circumflex
            case 0x0303 -> 0xfe53;   // dead_tilde
            case 0x0304 -> 0xfe54;   // dead_macron
            case 0x0306 -> 0xfe55;   // dead_breve
            case 0x0307 -> 0xfe56;   // dead_abovedot
            case 0x0308 -> 0xfe57;   // dead_diaeresis
            case 0x030a -> 0xfe58;   // dead_abovering
            case 0x030b -> 0xfe59;   // dead_doubleacute
            case 0x030c -> 0xfe5a;   // dead_caron
            case 0x0327 -> 0xfe5b;   // dead_cedilla
            case 0x0328 -> 0xfe5c;   // dead_ogonek
            default -> fromUnicode(accent);
        };
    }

    /**
     * A code point from a <em>string</em> as a keysym — committed text, a
     * clipboard, anything typed out character by character.
     *
     * <p>Not the same question as {@link #fromUnicode}, which is the X11 encoding
     * rule and answers it for every code point including the ones no keyboard
     * has. A newline is the one that matters: {@code fromUnicode('\n')} is a
     * well-formed keysym for "Unicode code point 10" that no server has a key
     * for, so it arrives as {@code NoSymbol} and nothing happens. What was meant
     * is {@link #RETURN}. {@code 0} for the other control characters: a string
     * can carry a bell or a form feed and there is no key to press for either.
     *
     * <p>The five here are the five the original's native {@code unicodeToKeysym}
     * table carries at its head, plus {@code \r}, which an IME committing a line
     * ending can produce.
     */
    public static int forCharacter(int cp) {
        return switch (cp) {
            case '\n', '\r' -> RETURN;
            case '\t' -> TAB;
            case 0x1b -> ESCAPE;
            case 0x08 -> BACKSPACE;
            case 0x7f -> DELETE;
            // A dead key can arrive here too, from an IME's own key events, and
            // is not a control character however negative it looks.
            default -> cp >= 0x20 || (cp & COMBINING_ACCENT) != 0 ? fromKeyChar(cp) : 0;
        };
    }

    /**
     * The same, for a character typed while another keyboard is holding
     * modifiers down at the far end.
     *
     * <p>A soft keyboard cannot see those modifiers, so it reports the character
     * it would have produced on its own — and the far end, asked for that
     * character while a Shift it is holding says otherwise, resolves the
     * disagreement by letting go of the Shift. Ctrl+Shift+C arrives as Ctrl+C.
     *
     * <p>So the character is put in the case the held modifiers imply: upper
     * under Shift, lower under any other modifier without Shift (so a keyboard
     * that capitalised on its own does not turn Ctrl+c into Ctrl+Shift+c), and
     * as typed under nothing. Only letters — which character Shift and {@code 1}
     * make is a property of a layout at the far end, and guessing is worse than
     * leaving it.
     */
    public static int forCharacter(int cp, boolean shiftHeld, boolean otherHeld) {
        if (shiftHeld) {
            return forCharacter(Character.toUpperCase(cp));
        }
        if (otherHeld) {
            return forCharacter(Character.toLowerCase(cp));
        }
        return forCharacter(cp);
    }

    /**
     * An {@code android.view.KeyEvent} keycode as a keysym, for the
     * non-printing keys a hardware keyboard or an IME's own edit keys produce.
     * {@code 0} when there is no sensible mapping — printing keys included,
     * since those carry a character and belong in {@link #fromUnicode}.
     *
     * <p>Literal keycodes rather than {@code KeyEvent} constants so this class
     * stays off Android; the values are fixed API.
     *
     * <p><b>The lock keys are not here</b>, on purpose: their effect is applied
     * to the character locally, and forwarding the key as well would apply it
     * twice and leave two lock states to disagree with each other
     * ({@code ARCHITECTURE.md} §3.14).
     */
    public static int fromAndroidKeyCode(int keyCode) {
        return switch (keyCode) {
            case 59 -> SHIFT_L;
            case 60 -> SHIFT_R;
            case 113 -> CONTROL_L;
            case 114 -> CONTROL_R;
            case 57 -> ALT_L;                // ALT_LEFT
            case 58 -> ISO_LEVEL3_SHIFT;     // ALT_RIGHT is AltGr on a PC layout
            case 117 -> SUPER_L;             // META_LEFT
            case 118 -> SUPER_R;
            case 67 -> BACKSPACE;            // DEL
            case 112 -> DELETE;              // FORWARD_DEL
            case 61 -> TAB;
            case 66 -> RETURN;               // ENTER
            case 111 -> ESCAPE;
            case 122 -> HOME;                // MOVE_HOME
            case 123 -> END;                 // MOVE_END
            case 92 -> PAGE_UP;
            case 93 -> PAGE_DOWN;
            case 124 -> INSERT;
            case 19 -> UP;                   // DPAD_*
            case 20 -> DOWN;
            case 21 -> LEFT;
            case 22 -> RIGHT;
            case 82 -> MENU;                 // the context-menu key
            case 120 -> PRINT;               // SYSRQ
            case 121 -> PAUSE;               // BREAK
            case 213 -> MUHENKAN;
            case 214 -> HENKAN_MODE;
            case 215 -> HIRAGANA_KATAKANA;   // KATAKANA_HIRAGANA
            // Not KANA (218) and not EISU (212), which look like the other two
            // and are not: this phone's own key layout gives those two keycodes
            // to a Korean board's Hangul and Hanja keys, so a keysym chosen here
            // would be right for one keyboard and wrong for the other.
            case 277 -> XF86_CUT;
            case 278 -> XF86_COPY;
            case 279 -> XF86_PASTE;
            default -> {
                if (keyCode >= 131 && keyCode <= 142) {
                    yield f(keyCode - 130);      // F1–F12
                }
                // F13–F24, which are nowhere near the others in Android's
                // numbering and are consecutive in X11's.
                yield keyCode >= 326 && keyCode <= 337 ? f(keyCode - 313) : 0;
            }
        };
    }

    /**
     * The keypad, which needs the Num Lock state to answer and so is separate
     * from {@link #fromAndroidKeyCode}. {@code 0} for any other key.
     *
     * <p>These keysyms exist because the keypad is a distinct set of keys, not a
     * duplicate set of characters: with Num Lock off it <em>is</em> the
     * navigation cluster. Sending the digit instead would work with the lock on
     * and do nothing at all with it off.
     */
    public static int keypadKeysym(int keyCode, boolean numLockOn) {
        // The operators and the separators mean the same thing either way.
        switch (keyCode) {
            case 154: return KP_DIVIDE;
            case 155: return KP_MULTIPLY;
            case 156: return KP_SUBTRACT;
            case 157: return KP_ADD;
            case 159: return KP_SEPARATOR;   // NUMPAD_COMMA
            case 160: return KP_ENTER;
            case 161: return KP_EQUAL;
            case 162: return '(';            // NUMPAD_LEFT_PAREN
            case 163: return ')';
            default: break;
        }
        if (keyCode < 144 || keyCode > 158) {
            return 0;
        }
        if (numLockOn) {
            return keyCode == 158 ? KP_DECIMAL : KP_0 + (keyCode - 144);
        }
        return switch (keyCode) {
            case 144 -> KP_INSERT;
            case 145 -> KP_END;
            case 146 -> KP_DOWN;
            case 147 -> KP_PAGE_DOWN;
            case 148 -> KP_LEFT;
            case 149 -> KP_BEGIN;
            case 150 -> KP_RIGHT;
            case 151 -> KP_HOME;
            case 152 -> KP_UP;
            case 153 -> KP_PAGE_UP;
            default -> KP_DELETE;            // 158, NUMPAD_DOT
        };
    }

    /** A short name, for the HUD and for test assertions. */
    public static String name(int keysym) {
        return switch (keysym) {
            case SHIFT_L -> "Shift";
            case SHIFT_R -> "Shift_R";
            case CONTROL_L -> "Ctrl";
            case CONTROL_R -> "Ctrl_R";
            case ALT_L -> "Alt";
            case SUPER_L -> "Super";
            case SUPER_R -> "Super_R";
            case ISO_LEVEL3_SHIFT -> "AltGr";
            case BACKSPACE -> "BackSpace";
            case TAB -> "Tab";
            case RETURN -> "Return";
            case ESCAPE -> "Escape";
            case HOME -> "Home";
            case LEFT -> "Left";
            case UP -> "Up";
            case RIGHT -> "Right";
            case DOWN -> "Down";
            case PAGE_UP -> "PageUp";
            case PAGE_DOWN -> "PageDown";
            case END -> "End";
            case INSERT -> "Insert";
            case DELETE -> "Delete";
            case MENU -> "Menu";
            case PRINT -> "Print";
            case PAUSE -> "Pause";
            case MUHENKAN -> "Muhenkan";
            case HENKAN_MODE -> "Henkan_Mode";
            case HIRAGANA_KATAKANA -> "Hiragana_Katakana";
            case XF86_CUT -> "XF86Cut";
            case XF86_COPY -> "XF86Copy";
            case XF86_PASTE -> "XF86Paste";
            case KP_ENTER -> "KP_Enter";
            case KP_MULTIPLY -> "KP_Multiply";
            case KP_ADD -> "KP_Add";
            case KP_SUBTRACT -> "KP_Subtract";
            case KP_DECIMAL -> "KP_Decimal";
            case KP_DIVIDE -> "KP_Divide";
            case KP_EQUAL -> "KP_Equal";
            case KP_SEPARATOR -> "KP_Separator";
            case KP_INSERT -> "KP_Insert";
            case KP_END -> "KP_End";
            case KP_DOWN -> "KP_Down";
            case KP_PAGE_DOWN -> "KP_Next";
            case KP_LEFT -> "KP_Left";
            case KP_BEGIN -> "KP_Begin";
            case KP_RIGHT -> "KP_Right";
            case KP_HOME -> "KP_Home";
            case KP_UP -> "KP_Up";
            case KP_PAGE_UP -> "KP_Prior";
            case KP_DELETE -> "KP_Delete";
            default -> {
                if (keysym >= F1 && keysym <= f(24)) {
                    yield "F" + (keysym - F1 + 1);
                }
                if (keysym >= KP_0 && keysym <= KP_0 + 9) {
                    yield "KP_" + (keysym - KP_0);
                }
                final int cp = toUnicode(keysym);
                yield cp > 0 ? new String(Character.toChars(cp)) : "0x" + Integer.toHexString(keysym);
            }
        };
    }

    /** The inverse of {@link #fromUnicode}, or {@code 0} if it has no character. */
    public static int toUnicode(int keysym) {
        if ((keysym & 0xff000000) == 0x01000000) {
            return keysym & 0x00ffffff;
        }
        if ((keysym >= 0x20 && keysym <= 0x7e) || (keysym >= 0xa0 && keysym <= 0xff)) {
            return keysym;
        }
        return 0;
    }
}
