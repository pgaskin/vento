// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.input;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNotEquals;

import org.junit.Test;

/**
 * The keysym mappings. Small, but the whole keyboard rests on them, and they are
 * exactly the kind of table that is wrong in one branch and right everywhere
 * else.
 */
public class KeysymTest {

    /** The X11 rule: Latin-1 is itself, everything else is {@code 0x01000000 | cp}. */
    @Test
    public void unicodeMapsByTheStandardRule() {
        assertEquals('a', Keysym.fromUnicode('a'));
        assertEquals(' ', Keysym.fromUnicode(' '));
        assertEquals(0xe9, Keysym.fromUnicode('é'));
        assertEquals(0x0100_20ac, Keysym.fromUnicode('€'));
        assertEquals(0x0101_f600, Keysym.fromUnicode(0x1f600));
        // Control characters have no Latin-1 keysym and must not collide with
        // the function keys, which live in the same low range.
        assertNotEquals(Keysym.RETURN, Keysym.fromUnicode('\r'));
    }

    @Test
    public void unicodeRoundTrips() {
        for (int cp : new int[]{'a', 'Z', '~', 0xa0, 0xff, 0x20ac, 0x1f600}) {
            assertEquals(cp, Keysym.toUnicode(Keysym.fromUnicode(cp)));
        }
        // A named key is not a character, and must not be reported as one.
        assertEquals(0, Keysym.toUnicode(Keysym.RETURN));
        assertEquals(0, Keysym.toUnicode(Keysym.F1));
    }

    @Test
    public void functionKeysAreConsecutive() {
        assertEquals(Keysym.F1, Keysym.f(1));
        assertEquals(0xffc9, Keysym.f(12));
        assertEquals("F7", Keysym.name(Keysym.f(7)));
    }

    /** The values RealVNC's {@code SymbolBindings} publishes, as a spot check. */
    @Test
    public void namedKeysMatchTheOriginalsValues() {
        assertEquals(65505, Keysym.SHIFT_L);
        assertEquals(65507, Keysym.CONTROL_L);
        assertEquals(65513, Keysym.ALT_L);
        assertEquals(65515, Keysym.SUPER_L);
        assertEquals(65027, Keysym.ISO_LEVEL3_SHIFT);
        assertEquals(65288, Keysym.BACKSPACE);
        assertEquals(65293, Keysym.RETURN);
        assertEquals(65307, Keysym.ESCAPE);
        assertEquals(65535, Keysym.DELETE);
        assertEquals(65470, Keysym.F1);
    }

    @Test
    public void androidKeyCodesMapForTheNonPrintingKeys() {
        assertEquals(Keysym.BACKSPACE, Keysym.fromAndroidKeyCode(67));   // KEYCODE_DEL
        assertEquals(Keysym.DELETE, Keysym.fromAndroidKeyCode(112));     // KEYCODE_FORWARD_DEL
        assertEquals(Keysym.RETURN, Keysym.fromAndroidKeyCode(66));
        assertEquals(Keysym.LEFT, Keysym.fromAndroidKeyCode(21));
        assertEquals(Keysym.f(5), Keysym.fromAndroidKeyCode(135));
        // A printing key carries a character instead, and must fall through.
        assertEquals(0, Keysym.fromAndroidKeyCode(29));                  // KEYCODE_A
    }

    /**
     * The one that was a bug on a real server. An IME may commit a newline as
     * <em>text</em>, and {@code fromUnicode} answers that with a well-formed
     * keysym for "Unicode code point 10" — which every server reports as
     * {@code NoSymbol}, so Enter silently did nothing whenever it took that
     * path rather than arriving as a key event.
     */
    @Test
    public void aTypedNewlineIsReturnAndNotACodePoint() {
        assertEquals(0x0100000a, Keysym.fromUnicode('\n'));
        assertEquals(Keysym.RETURN, Keysym.forCharacter('\n'));
        assertEquals(Keysym.RETURN, Keysym.forCharacter('\r'));
        assertEquals(Keysym.TAB, Keysym.forCharacter('\t'));
        assertEquals(Keysym.ESCAPE, Keysym.forCharacter(0x1b));
        assertEquals(Keysym.BACKSPACE, Keysym.forCharacter(0x08));
        assertEquals(Keysym.DELETE, Keysym.forCharacter(0x7f));
        // The rest of the control range has no key to press for it at all.
        assertEquals(0, Keysym.forCharacter(0x07));       // bell
        assertEquals(0, Keysym.forCharacter(0x0c));       // form feed
        // And everything printable is unchanged.
        assertEquals('a', Keysym.forCharacter('a'));
        assertEquals(Keysym.fromUnicode(0x2603), Keysym.forCharacter(0x2603));
    }

    /**
     * The value a layout with dead keys answers with, which is not a code point:
     * {@code KeyCharacterMap.COMBINING_ACCENT} is bit 31, so it is negative, and
     * both paths that consume {@code getUnicodeChar} used to get it wrong in
     * opposite directions — one built a keysym out of the flag, the other read
     * the sign as a control character and dropped the key.
     */
    @Test
    public void aDeadKeyIsAKeyRatherThanAnAccent() {
        final int acute = 0x80000000 | 0x0301;
        assertEquals(0xfe51, Keysym.fromKeyChar(acute));
        assertEquals(0xfe51, Keysym.forCharacter(acute));
        assertEquals(0xfe50, Keysym.deadKeysym(0x0300));
        assertEquals(0xfe5b, Keysym.deadKeysym(0x0327));
        // An accent X11 has no dead key for goes as the character it is.
        assertEquals(Keysym.fromUnicode(0x0335), Keysym.deadKeysym(0x0335));
        // And an ordinary character is untouched by any of it.
        assertEquals('a', Keysym.fromKeyChar('a'));
        assertEquals(0, Keysym.fromKeyChar(0));
    }

    /**
     * A character typed on a keyboard that cannot see the modifiers another one
     * is holding. The far end resolves the disagreement by letting go of the
     * modifier, so the case has to be settled here.
     */
    @Test
    public void heldModifiersDecideTheCaseOfATypedCharacter() {
        // Shift on the row, c from the IME: Ctrl+Shift+C, not Ctrl+C.
        assertEquals('C', Keysym.forCharacter('c', true, true));
        assertEquals('C', Keysym.forCharacter('c', true, false));
        // Ctrl alone, and a keyboard that capitalised on its own: Ctrl+c.
        assertEquals('c', Keysym.forCharacter('C', false, true));
        // Nothing held changes nothing, in either direction.
        assertEquals('c', Keysym.forCharacter('c', false, false));
        assertEquals('C', Keysym.forCharacter('C', false, false));
        // A character with no case is a character with no case.
        assertEquals('1', Keysym.forCharacter('1', true, false));
        assertEquals('/', Keysym.forCharacter('/', false, true));
        // And the named keys stay named ones whatever is held.
        assertEquals(Keysym.RETURN, Keysym.forCharacter('\n', true, false));
        assertEquals(Keysym.BACKSPACE, Keysym.forCharacter(0x08, false, true));
    }

    // ---- the whole keyboard, key by key ------------------------------------

    /**
     * The two tables below are the keyboard written out a second time, from the
     * Android keycode list and X11's {@code keysymdef.h} rather than from the
     * code under test — which is the only thing that makes a lookup table
     * testable at all. Hex literals for the same reason: a keysym is a published
     * number, and asserting {@code Keysym.HOME} against {@code Keysym.HOME}
     * asserts nothing.
     *
     * <p>{@code KEYCODE_} names are given so a row can be read against
     * {@code KeyEvent}; the highest one that exists at API 36 is 337, and the
     * list only ever gets longer.
     */
    private static final int MAX_KEYCODE = 337;

    private static final int[][] BY_POSITION = {
            {19, 0xff52},    // DPAD_UP
            {20, 0xff54},    // DPAD_DOWN
            {21, 0xff51},    // DPAD_LEFT
            {22, 0xff53},    // DPAD_RIGHT
            {57, 0xffe9},    // ALT_LEFT      → Alt_L
            {58, 0xfe03},    // ALT_RIGHT     → ISO_Level3_Shift, not Alt_R
            {59, 0xffe1},    // SHIFT_LEFT    → Shift_L
            {60, 0xffe2},    // SHIFT_RIGHT   → Shift_R
            {61, 0xff09},    // TAB
            {66, 0xff0d},    // ENTER         → Return
            {67, 0xff08},    // DEL           → BackSpace
            {82, 0xff67},    // MENU
            {92, 0xff55},    // PAGE_UP       → Prior
            {93, 0xff56},    // PAGE_DOWN     → Next
            {111, 0xff1b},   // ESCAPE
            {112, 0xffff},   // FORWARD_DEL   → Delete
            {113, 0xffe3},   // CTRL_LEFT     → Control_L
            {114, 0xffe4},   // CTRL_RIGHT    → Control_R
            {117, 0xffeb},   // META_LEFT     → Super_L
            {118, 0xffec},   // META_RIGHT    → Super_R
            {120, 0xff61},   // SYSRQ         → Print
            {121, 0xff13},   // BREAK         → Pause
            {122, 0xff50},   // MOVE_HOME     → Home
            {123, 0xff57},   // MOVE_END      → End
            {124, 0xff63},   // INSERT
            {131, 0xffbe}, {132, 0xffbf}, {133, 0xffc0}, {134, 0xffc1},
            {135, 0xffc2}, {136, 0xffc3}, {137, 0xffc4}, {138, 0xffc5},
            {139, 0xffc6}, {140, 0xffc7}, {141, 0xffc8}, {142, 0xffc9},   // F1–F12
            {213, 0xff22},   // MUHENKAN
            {214, 0xff23},   // HENKAN            → Henkan_Mode
            {215, 0xff27},   // KATAKANA_HIRAGANA → Hiragana_Katakana
            {277, 0x1008ff58},   // CUT           → XF86Cut
            {278, 0x1008ff57},   // COPY          → XF86Copy
            {279, 0x1008ff6d},   // PASTE         → XF86Paste
            {326, 0xffca}, {327, 0xffcb}, {328, 0xffcc}, {329, 0xffcd},
            {330, 0xffce}, {331, 0xffcf}, {332, 0xffd0}, {333, 0xffd1},
            {334, 0xffd2}, {335, 0xffd3}, {336, 0xffd4}, {337, 0xffd5},   // F13–F24
    };

    /** {@code {keycode, Num Lock on, Num Lock off}}. */
    private static final int[][] KEYPAD = {
            {144, 0xffb0, 0xff9e},   // NUMPAD_0   → KP_0 / KP_Insert
            {145, 0xffb1, 0xff9c},   //            → KP_1 / KP_End
            {146, 0xffb2, 0xff99},   //            → KP_2 / KP_Down
            {147, 0xffb3, 0xff9b},   //            → KP_3 / KP_Next
            {148, 0xffb4, 0xff96},   //            → KP_4 / KP_Left
            {149, 0xffb5, 0xff9d},   //            → KP_5 / KP_Begin
            {150, 0xffb6, 0xff98},   //            → KP_6 / KP_Right
            {151, 0xffb7, 0xff95},   //            → KP_7 / KP_Home
            {152, 0xffb8, 0xff97},   //            → KP_8 / KP_Up
            {153, 0xffb9, 0xff9a},   // NUMPAD_9   → KP_9 / KP_Prior
            {154, 0xffaf, 0xffaf},   // NUMPAD_DIVIDE
            {155, 0xffaa, 0xffaa},   // NUMPAD_MULTIPLY
            {156, 0xffad, 0xffad},   // NUMPAD_SUBTRACT
            {157, 0xffab, 0xffab},   // NUMPAD_ADD
            {158, 0xffae, 0xff9f},   // NUMPAD_DOT → KP_Decimal / KP_Delete
            {159, 0xffac, 0xffac},   // NUMPAD_COMMA → KP_Separator
            {160, 0xff8d, 0xff8d},   // NUMPAD_ENTER
            {161, 0xffbd, 0xffbd},   // NUMPAD_EQUALS
            {162, 0x28, 0x28},       // NUMPAD_LEFT_PAREN — a character, not a KP_ keysym
            {163, 0x29, 0x29},       // NUMPAD_RIGHT_PAREN
    };

    @Test
    public void everyKeycodeMapsByPositionOrNotAtAll() {
        final int[] expected = new int[MAX_KEYCODE + 1];
        for (int[] row : BY_POSITION) {
            expected[row[0]] = row[1];
        }
        for (int keyCode = 0; keyCode <= MAX_KEYCODE; keyCode++) {
            assertEquals("keycode " + keyCode,
                    expected[keyCode], Keysym.fromAndroidKeyCode(keyCode));
        }
        // The lock keys are the deliberate holes in that table: their effect is
        // applied to the character here, and the far end is never told.
        assertEquals(0, Keysym.fromAndroidKeyCode(115));   // CAPS_LOCK
        assertEquals(0, Keysym.fromAndroidKeyCode(116));   // SCROLL_LOCK
        assertEquals(0, Keysym.fromAndroidKeyCode(143));   // NUM_LOCK
        // And so are the two conversion keys a phone hands to a Korean board's
        // Hangul and Hanja keys, which no keysym here can be right about.
        assertEquals(0, Keysym.fromAndroidKeyCode(212));   // EISU
        assertEquals(0, Keysym.fromAndroidKeyCode(218));   // KANA
    }

    @Test
    public void everyKeypadKeyMapsBothWaysAndNothingElseDoes() {
        final int[] on = new int[MAX_KEYCODE + 1];
        final int[] off = new int[MAX_KEYCODE + 1];
        for (int[] row : KEYPAD) {
            on[row[0]] = row[1];
            off[row[0]] = row[2];
        }
        for (int keyCode = 0; keyCode <= MAX_KEYCODE; keyCode++) {
            assertEquals("keycode " + keyCode + " num lock on",
                    on[keyCode], Keysym.keypadKeysym(keyCode, true));
            assertEquals("keycode " + keyCode + " num lock off",
                    off[keyCode], Keysym.keypadKeysym(keyCode, false));
        }
    }

    /**
     * The order in {@code PhysicalKeyboard}'s lookup is position, then keypad,
     * then the layout — so a key answering both tables would have one of its two
     * answers silently unreachable.
     */
    @Test
    public void noKeyIsInBothTables() {
        for (int keyCode = 0; keyCode <= MAX_KEYCODE; keyCode++) {
            if (Keysym.fromAndroidKeyCode(keyCode) != 0) {
                assertEquals("keycode " + keyCode, 0, Keysym.keypadKeysym(keyCode, true));
                assertEquals("keycode " + keyCode, 0, Keysym.keypadKeysym(keyCode, false));
            }
        }
    }

    @Test
    public void theKeypadNeedsTheLockState() {
        assertEquals(Keysym.KP_0 + 1, Keysym.keypadKeysym(145, true));   // NUMPAD_1
        assertEquals(Keysym.KP_END, Keysym.keypadKeysym(145, false));
        assertEquals(Keysym.KP_DECIMAL, Keysym.keypadKeysym(158, true));
        assertEquals(Keysym.KP_DELETE, Keysym.keypadKeysym(158, false));
        // The operators do not care.
        assertEquals(Keysym.KP_DIVIDE, Keysym.keypadKeysym(154, true));
        assertEquals(Keysym.KP_DIVIDE, Keysym.keypadKeysym(154, false));
        assertEquals(Keysym.KP_ENTER, Keysym.keypadKeysym(160, false));
        // Anything else is not the keypad.
        assertEquals(0, Keysym.keypadKeysym(29, true));
    }
}
