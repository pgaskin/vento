// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.input;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import net.pgaskin.remotedesktop.control.harness.Harness;

import org.junit.Test;

import java.util.List;

/**
 * The physical keyboard, against a stand-in for a US layout.
 *
 * <p>{@link PhysicalKeyboard.CharMap} is the seam that makes this testable at
 * all: on a device it is the {@code KeyCharacterMap} of whichever keyboard sent
 * the event, and here it is {@link #US} — twenty lines that behave the way a
 * layout does, including the part that matters, which is that the modifiers it
 * is <em>shown</em> decide the character it returns.
 */
public class PhysicalKeyboardTest {

    // Android keycodes.
    private static final int A = 29, B = 30, C = 31;
    private static final int ONE = 8;
    private static final int SHIFT_L = 59, CTRL_L = 113, ALT_L = 57, ALT_R = 58;
    private static final int ENTER = 66, ESC = 111, BACK = 4, VOL_UP = 24;
    private static final int CAPS_LOCK = 115, NUM_LOCK = 143;
    private static final int NUMPAD_1 = 145, NUMPAD_7 = 151, NUMPAD_DIV = 154;

    // MetaState bits.
    private static final int M_SHIFT = 0x1;
    private static final int M_ALT = 0x02;
    private static final int M_ALT_RIGHT = 0x20;
    private static final int M_CTRL = 0x1000;
    private static final int M_CAPS = 0x100000;
    private static final int M_NUM = 0x200000;

    /**
     * A layout: letters, the digit row, and one AltGr character so the third
     * level can be told apart from the first two. Ctrl and left Alt are
     * deliberately <em>not</em> handled — a real {@code KeyCharacterMap} returns
     * the plain character for Ctrl+C too, and the point of the test is that the
     * lookup never sees those bits at all.
     */
    private static final PhysicalKeyboard.CharMap US = (keyCode, metaState) -> {
        final boolean shift = (metaState & M_SHIFT) != 0;
        final boolean caps = (metaState & M_CAPS) != 0;
        final boolean altGr = (metaState & M_ALT_RIGHT) != 0;
        if (keyCode >= A && keyCode <= A + 25) {
            if (altGr) {
                return keyCode == A ? 0x00e1 : 0;   // á, as AltGr+a is on many layouts
            }
            final char base = (char) ('a' + (keyCode - A));
            return (shift ^ caps) ? Character.toUpperCase(base) : base;
        }
        if (keyCode == ONE) {
            return shift ? '!' : '1';
        }
        return 0;
    };

    /** What the far end was told, as {@code key down a} / {@code key up a}. */
    private static Harness kb() {
        return Harness.improved().withPhysicalKeys().reset();
    }

    private static void down(Harness h, int keyCode, int meta) {
        h.physicalKeys.keyDown(keyCode, meta, 0, US);
    }

    private static void up(Harness h, int keyCode) {
        h.physicalKeys.keyUp(keyCode);
    }

    private static void tap(Harness h, int keyCode, int meta) {
        down(h, keyCode, meta);
        up(h, keyCode);
    }

    // ---- the layout rule ---------------------------------------------------

    @Test
    public void aPrintingKeyIsItsCharacter() {
        final Harness h = kb();
        tap(h, A, 0);
        assertEquals(List.of("key down a", "key up a"), h.keys);
    }

    @Test
    public void shiftIsBothHeldAndAppliedToTheCharacter() {
        final Harness h = kb();
        down(h, SHIFT_L, 0);
        tap(h, A, M_SHIFT);
        up(h, SHIFT_L);
        // The modifier goes out as a key of its own *and* chooses the
        // character: an X server told Shift+A finds A where Shift is already
        // held, which is the consistent pair. Sending "a" under a held Shift
        // would not be.
        assertEquals(List.of("key down Shift", "key down A", "key up A", "key up Shift"), h.keys);
    }

    /**
     * The one that decides whether shortcuts work at all. A layout asked about
     * Ctrl+C usually answers with nothing, so a mapping that hands the whole
     * metaState to the lookup sends no key and Ctrl+C silently does not exist.
     */
    @Test
    public void ctrlDoesNotReachTheCharacterLookup() {
        final Harness h = kb();
        down(h, CTRL_L, 0);
        tap(h, C, M_CTRL);
        up(h, CTRL_L);
        assertEquals(List.of("key down Ctrl", "key down c", "key up c", "key up Ctrl"), h.keys);
    }

    @Test
    public void leftAltIsHeldButRightAltIsALevel() {
        final Harness altShortcut = kb();
        down(altShortcut, ALT_L, 0);
        tap(altShortcut, A, M_ALT);
        assertEquals(List.of("key down Alt", "key down a", "key up a"), altShortcut.keys);

        // AltGr is the other kind of modifier: it is *in* the character, and the
        // ISO_Level3_Shift that produced it goes out beside it so the far end's
        // own lookup finds the same level held.
        final Harness altGr = kb();
        down(altGr, ALT_R, 0);
        tap(altGr, A, M_ALT | M_ALT_RIGHT);
        assertEquals(List.of("key down AltGr", "key down á", "key up á"), altGr.keys);

        // And a key with nothing on the third level — which on a US layout is
        // every key — sends its own character rather than nothing, so the far
        // end can apply a level this phone's layout does not have.
        final Harness empty = kb();
        down(empty, ALT_R, 0);
        tap(empty, B, M_ALT | M_ALT_RIGHT);
        assertEquals(List.of("key down AltGr", "key down b", "key up b"), empty.keys);
    }

    @Test
    public void capsLockIsAppliedHereAndNeverForwarded() {
        final Harness h = kb();
        // The key itself is refused...
        assertFalse(h.physicalKeys.keyDown(CAPS_LOCK, 0, 0, US));
        // ...and the state it leaves behind is what chooses the character.
        tap(h, A, M_CAPS);
        assertEquals(List.of("key down A", "key up A"), h.keys);
    }

    // ---- keys that are positions -------------------------------------------

    @Test
    public void nonPrintingKeysGoByPosition() {
        final Harness h = kb();
        tap(h, ENTER, 0);
        tap(h, ESC, 0);
        assertEquals(List.of("key down Return", "key up Return",
                "key down Escape", "key up Escape"), h.keys);
    }

    @Test
    public void theKeypadFollowsNumLock() {
        final Harness on = kb();
        tap(on, NUMPAD_1, M_NUM);
        tap(on, NUMPAD_7, M_NUM);
        assertEquals(List.of("key down KP_1", "key up KP_1",
                "key down KP_7", "key up KP_7"), on.keys);

        // With the lock off the keypad *is* the navigation cluster, and a digit
        // would be wrong rather than merely different.
        final Harness off = kb();
        tap(off, NUMPAD_1, 0);
        tap(off, NUMPAD_7, 0);
        assertEquals(List.of("key down KP_End", "key up KP_End",
                "key down KP_Home", "key up KP_Home"), off.keys);

        // The operators mean the same thing either way.
        final Harness div = kb();
        tap(div, NUMPAD_DIV, 0);
        assertEquals(List.of("key down KP_Divide", "key up KP_Divide"), div.keys);
    }

    // ---- what this client keeps --------------------------------------------

    /**
     * The whole list, written out from the Android keycode names rather than
     * from the code: this is the set that keeps a phone a phone, and a key
     * quietly joining or leaving it is a behaviour change nothing else would
     * catch.
     */
    @Test
    public void theReservedSetIsExactlyTheseKeys() {
        final int[] expected = {
                3, 4, 5, 6,            // HOME, BACK, CALL, ENDCALL
                24, 25,                // VOLUME_UP, VOLUME_DOWN
                26,                    // POWER
                27,                    // CAMERA
                79,                    // HEADSETHOOK
                85, 86, 87, 88, 89, 90, 91,     // MEDIA_PLAY_PAUSE … MUTE (the mic)
                115, 116,              // CAPS_LOCK, SCROLL_LOCK
                126, 127, 128, 129, 130,        // MEDIA_PLAY … MEDIA_RECORD
                143,                   // NUM_LOCK
                164,                   // VOLUME_MUTE
                187,                   // APP_SWITCH
                220, 221,              // BRIGHTNESS_DOWN, BRIGHTNESS_UP
                223, 224,              // SLEEP, WAKEUP
        };
        final boolean[] want = new boolean[338];
        for (int keyCode : expected) {
            want[keyCode] = true;
        }
        for (int keyCode = 0; keyCode < want.length; keyCode++) {
            assertEquals("keycode " + keyCode,
                    want[keyCode], PhysicalKeyboard.reserved(keyCode));
        }
    }

    /**
     * A layout with dead keys, which no US one has: the character lookup answers
     * with the accent and bit 31 set, and what has to go out is the key rather
     * than the accent, or the far end places a mark and composes nothing.
     */
    @Test
    public void aDeadKeyGoesOutAsADeadKey() {
        final PhysicalKeyboard.CharMap intl = (keyCode, metaState) ->
                keyCode == 68 ? 0x80000000 | 0x0301 : 0;   // GRAVE, as acute
        final Harness h = kb();
        h.physicalKeys.keyDown(68, 0, 0, intl);
        h.physicalKeys.keyUp(68);
        assertEquals(List.of("key down 0xfe51", "key up 0xfe51"), h.keys);
    }

    @Test
    public void reservedKeysAreRefusedInBothDirections() {
        final Harness h = kb();
        for (int keyCode : new int[]{BACK, VOL_UP, NUM_LOCK}) {
            assertFalse("down " + keyCode, h.physicalKeys.keyDown(keyCode, 0, 0, US));
            assertEquals("up " + keyCode, 0, h.physicalKeys.keyUp(keyCode));
        }
        assertEquals(List.of(), h.keys);
    }

    // ---- holding, repeating, releasing -------------------------------------

    /**
     * The repeat bug that is only visible at the far end. Android reports a
     * repeat with the modifiers as they are <em>now</em>, so pressing Shift
     * during a held "a" would look like a press of "A" — and since the far end
     * keys its held-key map on the id, that press would overwrite the entry and
     * the release would let go of "A" while "a" stayed down for ever.
     */
    @Test
    public void aRepeatResendsTheKeysymItWentDownWith() {
        final Harness h = kb();
        down(h, A, 0);
        h.physicalKeys.keyDown(A, M_SHIFT, 1, US);
        h.physicalKeys.keyDown(A, M_SHIFT, 2, US);
        up(h, A);
        assertEquals(List.of("key down a", "key down a", "key down a", "key up a"), h.keys);
        // And nothing is left held at the far end.
        assertEquals(0, h.held.size());
    }

    @Test
    public void releaseAllLetsGoOfEverythingExactlyOnce() {
        final Harness h = kb();
        down(h, CTRL_L, 0);
        down(h, SHIFT_L, M_CTRL);
        down(h, B, M_CTRL | M_SHIFT);
        assertEquals(3, h.physicalKeys.heldCount());

        h.physicalKeys.releaseAll();
        assertEquals(0, h.physicalKeys.heldCount());
        assertEquals(0, h.held.size());
        // A late up for a key already released is not a second release, which is
        // what would arrive when the window comes back and the key is let go.
        assertEquals(0, h.physicalKeys.keyUp(B));
        assertEquals(3 + 3, h.keys.size());
    }

    @Test
    public void aReleaseNamesTheKeyRatherThanTheKeysym() {
        // Two keys whose keysym is the same one — a layout is free to put a
        // character on more than one key — and releasing one must not let go of
        // the other's.
        final PhysicalKeyboard.CharMap oneCharacter = (keyCode, metaState) -> 'a';
        final Harness h = kb();
        h.physicalKeys.keyDown(A, 0, 0, oneCharacter);
        h.physicalKeys.keyDown(B, 0, 0, oneCharacter);
        assertEquals(2, h.held.size());
        up(h, A);
        assertEquals(1, h.held.size());
        assertTrue(h.held.containsKey(KeySink.ID_SYSTEM_KEYBOARD + B));
    }

    /**
     * The right-hand modifiers are keys of their own at the far end, which is
     * what Android says they are and what every backend here can carry — and
     * they are still modifiers, or one of them would spend a one-shot armed on
     * the extension row.
     */
    @Test
    public void theRightHandModifiersAreTheirOwnKeys() {
        final Harness h = kb();
        tap(h, 60, 0);     // SHIFT_RIGHT
        tap(h, 114, 0);    // CTRL_RIGHT
        tap(h, 118, 0);    // META_RIGHT
        assertEquals(List.of("key down Shift_R", "key up Shift_R",
                "key down Ctrl_R", "key up Ctrl_R",
                "key down Super_R", "key up Super_R"), h.keys);
        assertTrue(PhysicalKeyboard.isModifier(Keysym.SHIFT_R));
        assertTrue(PhysicalKeyboard.isModifier(Keysym.CONTROL_R));
        assertTrue(PhysicalKeyboard.isModifier(Keysym.SUPER_R));
    }

    /**
     * A key past the end of the held table, which is where F13 lives: Android's
     * keycode list grows with each release, and a press that cannot be recorded
     * is a key held down at the far end for ever.
     */
    @Test
    public void aKeycodeBeyondTheTableIsStillAKey() {
        final Harness h = kb();
        tap(h, 326, 0);    // F13
        assertEquals(List.of("key down F13", "key up F13"), h.keys);
        assertEquals(0, h.held.size());
        assertEquals(0, h.physicalKeys.heldCount());
    }

    // ---- the two keyboards are one keyboard --------------------------------

    /**
     * The cross-keyboard merge from the other side. The extension row holds Ctrl
     * — which is the whole reason it exists — and the character comes from the
     * physical keyboard, exactly as it comes from the system IME.
     */
    @Test
    public void aRowModifierAndAPhysicalKeyMeetAtTheFarEnd() {
        final Harness h = Harness.improved().withKeyboard().withPhysicalKeys().reset();
        // Ctrl is the second key of the row's first group.
        h.down(0, 150, 1000).up(0);
        assertEquals(List.of("key down Ctrl"), h.keys);
        tap(h, C, 0);
        assertEquals(List.of("key down Ctrl", "key down c", "key up c"), h.keys);
    }

    @Test
    public void aModifierOnThisKeyboardDoesNotConsumeAOneShotOnTheRow() {
        // isModifier is what the caller asks; the row's one-shot survives a
        // Shift pressed here and is spent by the character that follows.
        assertTrue(PhysicalKeyboard.isModifier(Keysym.CONTROL_L));
        assertTrue(PhysicalKeyboard.isModifier(Keysym.ISO_LEVEL3_SHIFT));
        assertFalse(PhysicalKeyboard.isModifier('c'));
        assertFalse(PhysicalKeyboard.isModifier(Keysym.RETURN));
    }

    // ---- the mask itself ---------------------------------------------------

    @Test
    public void charMetaKeepsWhatChoosesACharacterAndDropsTheRest() {
        assertEquals(M_SHIFT, PhysicalKeyboard.charMeta(M_SHIFT | M_CTRL));
        assertEquals(M_CAPS, PhysicalKeyboard.charMeta(M_CAPS | M_ALT));
        assertEquals(0, PhysicalKeyboard.charMeta(M_CTRL | M_ALT));
        // Left Alt alone is a shortcut modifier; right Alt turns the lookup's
        // Alt bit back on, because that is what a layout wants to see for AltGr.
        assertEquals(M_ALT | M_ALT_RIGHT,
                PhysicalKeyboard.charMeta(M_ALT | M_ALT_RIGHT));
    }
}
