// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.input;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotEquals;
import static org.junit.Assert.assertTrue;

import net.pgaskin.remotedesktop.control.harness.Harness;

import org.junit.Test;

import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.Set;

/**
 * The extension keyboard.
 *
 * <p>Positions are for the harness's 2400×1080 view at density 2.625, with no
 * IME under it and the model's built-in label estimate standing in for a real
 * measurer: the key row is the 120.75 px (46 dp) at the bottom starting at
 * y = 959.25, and the info bar is the 78.75 px (30 dp) above that. The x
 * constants below are worked out from those widths.
 */
public class ExtensionKeyboardTest {

    private static final float ROW_Y = 1000;
    private static final float BAR_Y = 900;

    // The first group runs Shift · Ctrl · Alt · Windows · Option · CMD from
    // x = 0 and ends at 531; then a 57.75 px (22 dp) gap between every group.
    // The groups in order: Backspace · Del · Esc · Tab · Ins, Enter, the four
    // arrows, Paste on its own, Home · End · Page Up · Page Down, F1-F12. Icon
    // keys are 18 dp of icon plus padding; the rest are their label, or the
    // 32 dp minimum.
    private static final float SHIFT_X = 50;
    private static final float CTRL_X = 150;
    private static final float ALT_X = 230;
    private static final float WIN_X = 300;
    private static final float CMD_X = 480;
    private static final float GAP_X = 560;
    private static final float BKSP_X = 640;
    private static final float ESC_X = 825;
    private static final float ENTER_X = 1148;
    private static final float PASTE_X = 1814;

    /** The info bar's two buttons, at its right end. */
    private static final float DISMISS_X = 2360;
    private static final float MASK_X = 2280;

    /** Nowhere near the keyboard. */
    private static final float[] PAD = {1200, 400};

    private static Harness kbd() {
        return Harness.improved().withKeyboard().reset();
    }

    private static Harness tapKey(Harness h, float x) {
        return h.down(0, x, ROW_Y).up(0);
    }

    // ---- ordinary keys -----------------------------------------------------

    /**
     * A key fires on release, not on press — which is what lets a drag across
     * the row scroll it instead, and is what both reference implementations do
     * (RealVNC through {@code onSingleTapUp}, cmus-android through
     * {@code setOnClickListener}).
     */
    @Test
    public void aTapOnAKeyPressesAndReleasesIt() {
        final Harness h = kbd();
        h.down(0, ESC_X, ROW_Y);
        assertEquals(List.of(), h.keys);
        h.up(0);
        assertEquals(List.of("key down Escape", "key up Escape"), h.keys);
    }

    @Test
    public void aTapOnTheGapBetweenGroupsDoesNothing() {
        final Harness h = kbd();
        tapKey(h, GAP_X);   // in the group gap after CMD
        assertEquals(List.of(), h.keys);
    }

    // ---- sticky modifiers --------------------------------------------------

    /**
     * The point of the whole widget: a modifier is <em>held down at the remote</em>
     * from the moment it is armed until the key it applies to has been sent. That
     * is the only thing that can join a Ctrl from this row to a C from the system
     * IME, since neither end can see the other.
     */
    @Test
    public void aModifierIsHeldAtTheRemoteUntilTheNextKey() {
        final Harness h = kbd();
        tapKey(h, CTRL_X);
        assertEquals(List.of("key down Ctrl"), h.keys);
        assertEquals(ExtensionKeyboard.Sticky.ONESHOT, sticky(h, "Ctrl"));

        tapKey(h, ESC_X);
        assertEquals(List.of("key down Ctrl", "key down Escape", "key up Escape", "key up Ctrl"),
                h.keys);
        assertEquals(ExtensionKeyboard.Sticky.OFF, sticky(h, "Ctrl"));
    }

    @Test
    public void tappingAnArmedModifierAgainLetsGoOfIt() {
        final Harness h = kbd();
        tapKey(h, CTRL_X);
        h.advance(h.cfg.keyDoubleTapMs + 50);   // else it is a double tap, i.e. a lock
        tapKey(h, CTRL_X);
        assertEquals(List.of("key down Ctrl", "key up Ctrl"), h.keys);
        assertEquals(ExtensionKeyboard.Sticky.OFF, sticky(h, "Ctrl"));
    }

    /**
     * Two rapid taps lock — the original's gesture. Locking is asked only of
     * modifiers, which is the whole difference from the original: it routes
     * every key through one double-tap detector and then discards the second tap
     * for keys that are not modifiers (vncpatch 0003, and
     * {@link #twoRapidTapsOfAnOrdinaryKeySendBoth}).
     */
    @Test
    public void twoRapidTapsLockAModifierUntilItIsTappedAgain() {
        final Harness h = kbd();
        tapKey(h, CTRL_X);
        assertEquals(ExtensionKeyboard.Sticky.ONESHOT, sticky(h, "Ctrl"));
        tapKey(h, CTRL_X);
        assertEquals(ExtensionKeyboard.Sticky.LOCKED, sticky(h, "Ctrl"));
        // Still held from the first tap: the state changed, not the key.
        assertEquals(List.of("key down Ctrl"), h.keys);

        // And it survives a key, where a one-shot would not.
        tapKey(h, ESC_X);
        assertEquals(List.of("key down Ctrl", "key down Escape", "key up Escape"), h.keys);
        assertEquals(ExtensionKeyboard.Sticky.LOCKED, sticky(h, "Ctrl"));

        tapKey(h, CTRL_X);
        assertEquals(ExtensionKeyboard.Sticky.OFF, sticky(h, "Ctrl"));
        assertEquals("key up Ctrl", h.keys.get(h.keys.size() - 1));
    }

    /** Two taps too far apart are two taps: arm, then disarm. */
    @Test
    public void twoSlowTapsDoNotLock() {
        final Harness h = kbd();
        tapKey(h, CTRL_X);
        h.advance(h.cfg.keyDoubleTapMs + 50);
        tapKey(h, CTRL_X);
        assertEquals(ExtensionKeyboard.Sticky.OFF, sticky(h, "Ctrl"));
        assertEquals(List.of("key down Ctrl", "key up Ctrl"), h.keys);
    }

    /** ... and so are two taps on different keys, however quick. */
    @Test
    public void aDoubleTapHasToBeOnTheSameKey() {
        final Harness h = kbd();
        tapKey(h, CTRL_X);
        tapKey(h, ALT_X);
        assertEquals(ExtensionKeyboard.Sticky.ONESHOT, sticky(h, "Ctrl"));
        assertEquals(ExtensionKeyboard.Sticky.ONESHOT, sticky(h, "Alt"));
    }

    /**
     * The bug the original's shared double-tap detector causes: pressing
     * Backspace twice quickly deletes one character, because the second tap goes
     * to a handler that ignores non-modifiers. Only modifiers ask the question
     * here, so this is two presses.
     */
    @Test
    public void twoRapidTapsOfAnOrdinaryKeySendBoth() {
        final Harness h = kbd();
        tapKey(h, BKSP_X);
        tapKey(h, BKSP_X);
        assertEquals(2, presses(h));
    }

    /**
     * Windows and Cmd are two keys with one keysym — the original ships both and
     * lets you pick by eye, since the label that makes sense depends on the
     * remote OS. The remote must still hear exactly one press and one release.
     */
    @Test
    public void twoKeysSharingAKeysymPressItOnce() {
        final Harness h = kbd();
        tapKey(h, WIN_X);
        tapKey(h, CMD_X);
        assertEquals(List.of("key down Super"), h.keys);
        assertEquals(ExtensionKeyboard.Sticky.ONESHOT, sticky(h, "Windows"));
        assertEquals(ExtensionKeyboard.Sticky.ONESHOT, sticky(h, "CMD"));

        tapKey(h, ESC_X);
        assertEquals(List.of("key down Super", "key down Escape", "key up Escape", "key up Super"),
                h.keys);
        assertEquals("nothing left held at the far end", Map.of(), h.held);
    }

    // ---- action keys -------------------------------------------------------

    /**
     * Paste sends nothing: it reports its name and the host decides, because
     * what it does — read a clipboard, ask about a long one, type it out — is
     * not something a plain-JVM model can do or should know about.
     */
    @Test
    public void anActionKeyReportsItselfInsteadOfSendingAKey() {
        final Harness h = kbd();
        tapKey(h, PASTE_X);
        assertEquals(List.of("action paste"), h.keyActions);
        assertEquals("nothing went to the far end", List.of(), h.keys);
        assertEquals(Map.of(), h.held);
    }

    /** It still counts as a key for the armed modifiers, which it consumes. */
    @Test
    public void anActionKeyConsumesTheArmedModifiers() {
        final Harness h = kbd();
        tapKey(h, SHIFT_X);
        h.reset();

        tapKey(h, PASTE_X);
        assertEquals(List.of("action paste"), h.keyActions);
        assertEquals(List.of("key up Shift"), h.keys);
        assertEquals(ExtensionKeyboard.Sticky.OFF, sticky(h, "Shift"));
    }

    /** An action is not typing, so the info bar does not claim anything was. */
    @Test
    public void anActionKeyDoesNotAppearInTheInfoBar() {
        final Harness h = kbd();
        type(h, "hi");
        tapKey(h, PASTE_X);
        assertEquals("hi", h.keyboard.infoText());
    }

    // ---- haptics -----------------------------------------------------------

    @Test
    public void hapticsCanBeTurnedOff() {
        final Harness on = kbd();
        tapKey(on, ESC_X);
        assertTrue("a key normally asks for one", on.keyFeedbacks > 0);

        final Harness off = kbd();
        off.cfg.keyboardHaptics = false;
        off.keyFeedbacks = 0;
        tapKey(off, ESC_X);
        tapKey(off, CTRL_X);
        tapKey(off, CTRL_X);   // a lock, which is the loudest of them
        assertEquals("and none at all with them off", 0, off.keyFeedbacks);
        assertEquals("without becoming a different keyboard",
                ExtensionKeyboard.Sticky.LOCKED, sticky(off, "Ctrl"));
    }

    // ---- the key id --------------------------------------------------------

    /**
     * Every key is its own thing at the far end. RFB's client-side keyboard is a
     * {@code map<keycode, keysym>} and a release lets go of the keysym the
     * <em>key</em> went down with, so two keys held
     * at once have to be two entries, not one — which the original cannot manage
     * because it passes the keysym as the id.
     */
    @Test
    public void keysHeldTogetherHaveDistinctIds() {
        final Harness h = kbd();
        tapKey(h, CTRL_X);
        tapKey(h, SHIFT_X);
        assertEquals(2, h.held.size());
        assertEquals("both are ours", 2, h.held.keySet().stream()
                .filter(id -> id >= KeySink.ID_EXTENSION_KEYBOARD
                        && id < KeySink.ID_SYSTEM_KEYBOARD).count());
    }

    /**
     * The release has to name the id the <em>press</em> used, which is not
     * always the id of the key that ends the hold: with Windows and CMD both
     * armed it is CMD's release that lets go, while what the far end recorded is
     * Windows'. Getting this wrong leaves Super down for ever, and the harness's
     * fake remote is what notices — it logs {@code key up ?} for an id nobody
     * pressed.
     */
    @Test
    public void aReleaseNamesTheIdItsPressUsed() {
        final Harness h = kbd();
        tapKey(h, WIN_X);
        final int windowsId = h.held.keySet().iterator().next();
        tapKey(h, CMD_X);
        assertEquals("CMD's press is suppressed, so Windows' id still owns it",
                Map.of(windowsId, Keysym.SUPER_L), h.held);
        h.reset();

        h.keyboard.clearModifiers();
        assertEquals(List.of("key up Super"), h.keys);
        assertEquals("released under the id it was pressed with", Map.of(), h.held);
    }

    /** Every repeat of a held key is the same key, so it is the same id. */
    @Test
    public void aRepeatKeepsOneIdAndLeavesNothingHeld() {
        final Harness h = kbd();
        h.down(0, BKSP_X, ROW_Y).advance(h.cfg.keyLongPressMs + h.cfg.keyRepeatMs * 3).up(0);
        assertTrue(presses(h) >= 3);
        assertEquals("no repeat left half-pressed", Map.of(), h.held);
        assertFalse("no release of an id nobody pressed", h.keys.contains("key up ?"));
    }

    // ---- key repeat --------------------------------------------------------

    /**
     * Hold an ordinary key and it repeats, which the original does not do at all
     * — the reason its arrow keys are painful (vncpatch 0004).
     *
     * <p>Nothing is sent at the long-press threshold itself: the single press of
     * a key is always its release, and the threshold is only where the repeat
     * clock starts.
     */
    @Test
    public void holdingAnOrdinaryKeyRepeatsIt() {
        final Harness h = kbd();
        h.down(0, BKSP_X, ROW_Y);
        h.advance(h.cfg.keyLongPressMs);
        assertEquals("nothing at the threshold", 0, presses(h));
        h.advance(h.cfg.keyRepeatMs * 3);
        assertEquals(3, presses(h));
        h.up(0).advance(1000);
        assertEquals("lifting stops the repeat, and adds nothing", 3, presses(h));
    }

    /**
     * Only the keys where holding means "again" repeat: the arrows, the two
     * deletes, Tab and the two page keys. A hold on Esc is a slow tap, because
     * eight Escs is a mistake and there is no undo at the far end.
     */
    @Test
    public void aKeyThatIsNotFlaggedRepeatingNeverDoes() {
        final Harness h = kbd();
        h.down(0, ESC_X, ROW_Y).advance(h.cfg.keyLongPressMs + h.cfg.keyRepeatMs * 20);
        assertEquals(List.of(), h.keys);
        h.up(0);
        assertEquals(List.of("key down Escape", "key up Escape"), h.keys);
    }

    /** A hold that ends before the first repeat is just a slow tap. */
    @Test
    public void aHoldTooShortToRepeatStillPressesOnRelease() {
        final Harness h = kbd();
        h.down(0, BKSP_X, ROW_Y).advance(h.cfg.keyLongPressMs + h.cfg.keyRepeatMs / 2);
        assertEquals(0, presses(h));
        h.up(0);
        assertEquals(1, presses(h));
    }

    /** A modifier does not repeat: the same hold locks it, as two taps do. */
    @Test
    public void holdingAModifierLocksIt() {
        final Harness h = kbd();
        h.down(0, CTRL_X, ROW_Y).advance(h.cfg.keyLongPressMs);
        assertEquals(ExtensionKeyboard.Sticky.LOCKED, sticky(h, "Ctrl"));
        h.advance(h.cfg.keyRepeatMs * 10).up(0);
        assertEquals("held once, not repeated, and the lift changes nothing",
                List.of("key down Ctrl"), h.keys);
        assertEquals(ExtensionKeyboard.Sticky.LOCKED, sticky(h, "Ctrl"));
    }

    // ---- scrolling the row -------------------------------------------------

    /**
     * The standard set is wider than a phone, so the row scrolls; a horizontal
     * drag takes over from the key it started on and nothing is sent.
     */
    @Test
    public void draggingSidewaysScrollsTheRowAndSendsNothing() {
        final Harness h = kbd();
        final float before = h.keyboard.keys().get(0).left();
        h.down(0, ESC_X, ROW_Y);
        for (int i = 1; i <= 10; i++) {
            h.move(0, ESC_X - i * 30, ROW_Y);
        }
        h.up(0);
        assertEquals(List.of(), h.keys);
        assertNotEquals(before, h.keyboard.keys().get(0).left(), 1.0f);
        // ... and the keys that scrolled off the left really are off the left.
        assertTrue(h.keyboard.keys().get(0).left() < before);
    }

    /**
     * A flick keeps the row moving after the finger has gone, and decays to a
     * stop. Without it a keyboard whose F-keys are three screens to the right is
     * a chore to reach — the original gets this from {@code HorizontalScrollView}
     * and we have to build it, since nothing here is a scrolling view.
     */
    @Test
    public void aFlickKeepsTheRowMovingAndThenStops() {
        final Harness h = kbd();
        h.down(0, 2000, ROW_Y);
        for (int i = 1; i <= 6; i++) {
            h.move(0, 2000 - i * 120, ROW_Y);   // 15 px/ms, well over the gate
        }
        h.up(0);
        assertTrue("the flick should fling", h.keyboard.flinging());

        final float atRelease = h.keyboard.keys().get(0).left();
        h.advance(300);
        final float afterGlide = h.keyboard.keys().get(0).left();
        assertTrue("the glide should carry it further", afterGlide < atRelease - 10);

        h.advance(3000);
        assertFalse("and then stop", h.keyboard.flinging());
        assertEquals(afterGlide, h.keyboard.keys().get(0).left(), 60f);
    }

    /** A slow drag that ends where it stops does not fling. */
    @Test
    public void aSlowDragDoesNotFling() {
        final Harness h = kbd();
        h.down(0, 2000, ROW_Y);
        for (int i = 1; i <= 8; i++) {
            h.move(0, 2000 - i * 5, ROW_Y);     // 0.6 px/ms, under the gate
        }
        h.up(0);
        assertFalse(h.keyboard.flinging());
    }

    /** Touching the row again catches it, as every scroller does. */
    @Test
    public void aTouchStopsAFlingInsteadOfPressingAKey() {
        final Harness h = kbd();
        h.down(0, 2000, ROW_Y);
        for (int i = 1; i <= 6; i++) {
            h.move(0, 2000 - i * 120, ROW_Y);
        }
        h.up(0);
        h.advance(32);
        final float caught = h.keyboard.keys().get(0).left();
        h.down(1, 600, ROW_Y);
        assertFalse(h.keyboard.flinging());
        h.advance(500);
        assertEquals("and it stays where it was caught",
                caught, h.keyboard.keys().get(0).left(), 0.01f);
    }

    @Test
    public void aRowThatFitsDoesNotScroll() {
        final Harness h = Harness.improved()
                .withKeyboard(List.of(ExtensionKeyboard.Key.normal("Esc", Keysym.ESCAPE, 0)))
                .reset();
        final float before = h.keyboard.keys().get(0).left();
        h.down(0, before + 10, ROW_Y);
        for (int i = 1; i <= 10; i++) {
            h.move(0, before + 10 - i * 40, ROW_Y);
        }
        h.up(0);
        assertEquals(before, h.keyboard.keys().get(0).left(), 0.01f);
        // A single key is centred, not left-aligned.
        assertTrue(before > 1000);
    }

    // ---- clicks ------------------------------------------------------------

    /**
     * A click consumes the armed modifiers exactly as a key does — Ctrl+click is
     * the same chord Ctrl+C is, and this row is the only half of either that can
     * be armed — and the release comes <em>after</em> the click has gone out, or
     * the far end would be told to let go of Ctrl in the middle of one.
     */
    @Test
    public void aClickConsumesTheArmedModifiers() {
        final Harness h = kbd();
        tapKey(h, CTRL_X);
        h.reset().tap(PAD[0], PAD[1]);
        assertEquals(List.of("down LEFT", "up LEFT", "key up Ctrl"), merged(h));
        assertEquals(ExtensionKeyboard.Sticky.OFF, sticky(h, "Ctrl"));
    }

    @Test
    public void aLockedModifierSurvivesAClick() {
        final Harness h = kbd();
        tapKey(h, CTRL_X);
        tapKey(h, CTRL_X);   // locked
        h.reset().tap(PAD[0], PAD[1]);
        assertEquals(List.of(), h.keys);
        assertEquals(ExtensionKeyboard.Sticky.LOCKED, sticky(h, "Ctrl"));
    }

    /**
     * Why the release is the button's <em>up</em>: a drag starts as a tap, so a
     * modifier let go of at the press would be gone for the whole of the
     * Shift+drag it was armed for.
     */
    @Test
    public void aDragKeepsTheModifierUntilTheButtonComesUp() {
        final Harness h = kbd();
        tapKey(h, SHIFT_X);
        h.reset();

        h.down(0, PAD[0], PAD[1]).up(0);        // tap …
        h.down(0, PAD[0], PAD[1]);              // … and hold: LEFT stays down
        for (int i = 1; i <= 10; i++) {
            h.move(0, PAD[0] + i * 20, PAD[1]);
        }
        assertEquals("still held for the drag", List.of("down LEFT"), merged(h));
        assertEquals(ExtensionKeyboard.Sticky.ONESHOT, sticky(h, "Shift"));

        h.up(0);
        assertEquals(List.of("down LEFT", "up LEFT", "key up Shift"), merged(h));
    }

    /**
     * A scroll is not a click. Ctrl+scroll zooms and Shift+scroll goes sideways,
     * and both are many notches of one gesture: consuming the modifier on the
     * first would leave the rest of it unmodified.
     */
    @Test
    public void scrollingDoesNotConsumeTheModifiers() {
        final Harness h = kbd();
        tapKey(h, CTRL_X);
        h.reset();
        h.down(0, PAD[0], PAD[1]).down(1, PAD[0] + 200, PAD[1]);
        for (int i = 1; i <= 4; i++) {
            h.move(0, PAD[0], PAD[1] + 10 * i, 1, PAD[0] + 200, PAD[1] + 10 * i);
        }
        h.up(1).up(0).advance(300);

        assertTrue(Harness.count(h.mouse, "down WHEEL_DOWN") > 0);
        assertEquals(List.of(), h.keys);
        assertEquals(ExtensionKeyboard.Sticky.ONESHOT, sticky(h, "Ctrl"));
    }

    /** Any producer of buttons, not just the touchpad: the union is the funnel. */
    @Test
    public void aRealMousesClickConsumesThemToo() {
        final Harness h = Harness.improved().withKeyboard().withMouse().reset();
        tapKey(h, CTRL_X);
        h.reset();
        h.physicalMouse.buttonState(1);
        assertEquals(List.of(), h.keys);
        h.physicalMouse.buttonState(0);
        assertEquals(List.of("key up Ctrl"), h.keys);
    }

    // ---- sharing the touch surface ----------------------------------------

    @Test
    public void touchesOnTheKeyboardNeverReachTheGestureLayer() {
        final Harness h = kbd();
        h.down(0, ESC_X, ROW_Y);
        h.move(0, ESC_X + 200, ROW_Y - 40);
        assertEquals(0, h.gestures.downCount());
        assertEquals(0, Harness.count(h.mouse, "move "));
        h.up(0).advance(500);
        assertEquals("no tap-click from the pad", 0, Harness.count(h.mouse, "down "));
    }

    /** Everything above the info bar is still the touchpad. */
    @Test
    public void theTouchpadStillWorksAboveTheKeyboard() {
        final Harness h = kbd();
        final float x0 = h.cursor.x();
        h.down(0, PAD[0], PAD[1]);
        for (int i = 1; i <= 10; i++) {
            h.move(0, PAD[0] + i * 20, PAD[1]);
        }
        h.up(0);
        assertTrue(Harness.count(h.mouse, "move ") > 0);
        assertNotEquals(x0, h.cursor.x(), 1.0f);
        assertEquals(List.of(), h.keys);
    }

    @Test
    public void aHiddenKeyboardIsNotThere() {
        final Harness h = kbd();
        h.keyboard.setVisible(false);
        h.reset().tap(ESC_X, ROW_Y);
        assertEquals(List.of(), h.keys);
        assertEquals(List.of("down LEFT", "up LEFT"), h.buttonEvents());
    }

    // ---- hiding, cancelling ------------------------------------------------

    /** A modifier you cannot see must never eat the next key. */
    @Test
    public void hidingReleasesEveryModifierIncludingLockedOnes() {
        final Harness h = kbd();
        tapKey(h, CTRL_X);
        tapKey(h, CTRL_X);   // locked
        tapKey(h, SHIFT_X);  // one-shot
        h.reset();

        h.keyboard.setVisible(false);
        assertEquals(List.of("key up Shift", "key up Ctrl"), h.keys);
        assertEquals(ExtensionKeyboard.Sticky.OFF, sticky(h, "Ctrl"));
    }

    @Test
    public void aCancelledTouchSendsNothingAndStopsTheRepeat() {
        final Harness h = kbd();
        h.down(0, BKSP_X, ROW_Y).advance(h.cfg.keyLongPressMs + h.cfg.keyRepeatMs * 2);
        assertEquals(2, presses(h));
        h.cancel().advance(1000);
        assertEquals("the repeat is cancelled with the touch", 2, presses(h));
    }

    @Test
    public void dismissHidesTheWholeThing() {
        final Harness h = kbd();
        h.down(0, DISMISS_X, BAR_Y).up(0);
        assertFalse(h.keyboard.visible());
    }

    // ---- the info bar ------------------------------------------------------

    /**
     * The readout is the line being typed: Backspace erases, and Return clears
     * the buffer rather than adding to it, so it resets when the line is
     * committed. The original's {@code InfoBar} exactly.
     */
    @Test
    public void theInfoBarShowsTheLineBeingTyped() {
        final Harness h = kbd();
        type(h, "hi!");
        assertEquals("hi!", h.keyboard.infoText());

        tapKey(h, BKSP_X);
        assertEquals("hi", h.keyboard.infoText());

        tapKey(h, ENTER_X);
        assertEquals("", h.keyboard.infoText());
    }

    /** Characters typed on the IME consume an armed modifier, as our own keys do. */
    @Test
    public void anExternalKeyConsumesTheArmedModifiers() {
        final Harness h = kbd();
        tapKey(h, CTRL_X);
        h.reset();
        type(h, "c");
        assertEquals(List.of("key up Ctrl"), h.keys);
        assertEquals(ExtensionKeyboard.Sticky.OFF, sticky(h, "Ctrl"));
    }

    @Test
    public void maskingHidesWhatWasTyped() {
        final Harness h = kbd();
        type(h, "pass");
        assertFalse(h.keyboard.masked());
        h.down(0, MASK_X, BAR_Y).up(0);
        assertTrue(h.keyboard.masked());
        assertEquals("●●●●", h.keyboard.infoText());
        h.down(0, MASK_X, BAR_Y).up(0);
        assertEquals("pass", h.keyboard.infoText());
    }

    /** Tapping the bar anywhere but its two buttons toggles the mask, as the original does. */
    @Test
    public void tappingTheBarBodyTogglesTheMask() {
        final Harness h = kbd();
        h.down(0, 900, BAR_Y).up(0);
        assertTrue(h.keyboard.masked());
    }

    // ---- geometry ----------------------------------------------------------

    /**
     * The desktop insets by the key row and by the IME under it, but not by the
     * info bar: the row is opaque and full of tap targets, so nothing may be
     * under it, while the bar is a readout that floats over the desktop and
     * thins out as the cursor nears it. Inset by the bar too and there would be
     * nothing behind it to reveal.
     */
    @Test
    public void theKeyboardInsetsByTheRowButNotTheInfoBar() {
        final Harness h = kbd();
        assertEquals(h.cfg.dp(46), h.keyboard.insetBottomPx(), 0.01f);
        // The bar is the difference between what it covers and what it hides.
        assertEquals(h.cfg.dp(30),
                h.keyboard.keyRowTop() - h.keyboard.infoBarTop(), 0.01f);

        h.keyboard.setBottomOffset(400);
        assertEquals(400 + h.cfg.dp(46), h.keyboard.insetBottomPx(), 0.01f);
        // The row rides on top of the IME rather than behind it.
        assertEquals(1080 - 400, h.keyboard.keyRowBottom(), 0.01f);

        h.keyboard.setVisible(false);
        assertEquals(0f, h.keyboard.insetBottomPx(), 0.01f);
    }

    @Test
    public void keysAreGroupedWithGapsBetweenTheGroups() {
        final Harness h = kbd();
        final List<ExtensionKeyboard.Bounds> keys = h.keyboard.keys();
        assertEquals(h.keyboard.allKeys().size(), keys.size());
        for (int i = 1; i < keys.size(); i++) {
            final float gap = keys.get(i).left() - keys.get(i - 1).right();
            final boolean sameGroup = keys.get(i).key().group() == keys.get(i - 1).key().group();
            assertEquals(sameGroup ? 0 : h.cfg.keyboardGroupGapPx, gap, 0.01f);
        }
        assertEquals(h.cfg.dp(46), keys.get(0).bottom() - keys.get(0).top(), 0.01f);
    }

    /**
     * What the row is holding is readable, because the other keyboard has to ask
     * before it can decide what a typed character means.
     */
    @Test
    public void heldModifiersAreReadable() {
        final Harness h = kbd();
        assertTrue(h.keyboard.heldModifiers().isEmpty());
        tapKey(h, SHIFT_X);
        assertEquals(Set.of(Keysym.SHIFT_L), h.keyboard.heldModifiers());
        tapKey(h, CTRL_X);
        assertEquals(Set.of(Keysym.SHIFT_L, Keysym.CONTROL_L), h.keyboard.heldModifiers());
        // One ordinary key consumes both, since both are one-shot.
        tapKey(h, ESC_X);
        assertTrue(h.keyboard.heldModifiers().isEmpty());
    }

    // ---- helpers -----------------------------------------------------------

    /**
     * The double-tap memory does not outlive the row. Hiding it clears every
     * modifier, so a remembered tap would meet a key that is off and read as
     * the second of a pair — locking on what is somebody's first tap.
     */
    @Test
    public void hidingTheRowForgetsTheLastTap() {
        final Harness h = kbd();
        tapKey(h, SHIFT_X);
        assertEquals(ExtensionKeyboard.Sticky.ONESHOT, sticky(h, "Shift"));
        h.keyboard.setVisible(false);
        h.keyboard.setVisible(true);
        h.advance(50);
        tapKey(h, SHIFT_X);
        assertEquals(ExtensionKeyboard.Sticky.ONESHOT, sticky(h, "Shift"));
    }

    private static ExtensionKeyboard.Sticky sticky(Harness h, String label) {
        for (ExtensionKeyboard.Key k : h.keyboard.allKeys()) {
            if (k.label().equals(label)) {
                return h.keyboard.sticky(k);
            }
        }
        throw new AssertionError("no key labelled " + label);
    }

    /** Stand in for the system IME committing text. */
    private static void type(Harness h, String s) {
        for (int i = 0; i < s.length(); i++) {
            h.keyboard.externalKey(Keysym.fromUnicode(s.charAt(i)));
        }
    }

    private static int presses(Harness h) {
        return Harness.count(h.keys, "key down BackSpace");
    }

    /**
     * The buttons and the keys in one list, in the order they went out, with the
     * harness's timestamps dropped. What a click does to a modifier is a
     * question about ordering as much as about state.
     */
    private static List<String> merged(Harness h) {
        final List<String> out = new ArrayList<>();
        for (String l : h.all) {
            final String line = l.substring(l.indexOf(' ') + 1);
            if (line.startsWith("down ") || line.startsWith("up ") || line.startsWith("key ")) {
                out.add(line);
            }
        }
        return out;
    }
}
