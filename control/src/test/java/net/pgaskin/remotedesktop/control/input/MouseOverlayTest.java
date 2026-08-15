// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.input;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotEquals;
import static org.junit.Assert.assertTrue;

import net.pgaskin.remotedesktop.control.harness.Harness;

import org.junit.Test;

import java.util.List;

/**
 * The mouse button / wheel overlay.
 *
 * <p>Positions are for the harness's 2400×1080 view at density 2.625, where the
 * button row is 189 px high (72 dp) starting at y = 891, the wheel strip is the
 * 157.5 px (60 dp) up the right edge above it, and the dismiss button is the
 * 105 px (40 dp) square in the corner.
 */
public class MouseOverlayTest {

    private static final float[] LEFT = {400, 1000};
    private static final float[] MIDDLE = {1100, 1000};
    private static final float[] RIGHT = {1700, 1000};
    private static final float[] DISMISS = {2300, 985};
    /** Top of the strip: full rate, upwards. */
    private static final float[] STRIP_TOP = {2320, 0};
    /** Quarter of the way up: rate 1 rather than 4. */
    private static final float[] STRIP_QUARTER_UP = {2320, 334.125f};
    private static final float[] STRIP_BOTTOM = {2320, 890};
    /** Nowhere near the overlay. */
    private static final float[] PAD = {1200, 540};

    private static Harness overlay() {
        return Harness.improved().withOverlay().reset();
    }

    private static Harness press(Harness h, int id, float[] p) {
        return h.down(id, p[0], p[1]);
    }

    // ---- buttons ----------------------------------------------------------

    @Test
    public void eachButtonPressesAndReleasesItsOwnMask() {
        for (float[][] c : new float[][][]{{LEFT, {1}}, {MIDDLE, {2}}, {RIGHT, {4}}}) {
            final Harness h = overlay();
            press(h, 0, c[0]);
            final String name = Button.maskName((int) c[1][0]);
            assertEquals(List.of("ovl down " + name), h.overlayEvents());
            assertEquals((int) c[1][0], h.cursor.buttons());
            h.up(0);
            assertEquals(List.of("ovl down " + name, "ovl up " + name), h.overlayEvents());
            assertEquals(0, h.cursor.buttons());
        }
    }

    /** Two fingers, two buttons: the mask is a set, and the overlay says so. */
    @Test
    public void twoButtonsCanBeHeldAtOnce() {
        final Harness h = overlay();
        press(h, 0, LEFT);
        press(h, 1, RIGHT);
        assertEquals(Button.LEFT.mask() | Button.RIGHT.mask(), h.cursor.buttons());
        h.up(1);
        assertEquals(Button.LEFT.mask(), h.cursor.buttons());
        h.up(0);
        assertEquals(0, h.cursor.buttons());
    }

    /**
     * The reason each producer keeps its own mask. A tap on the touchpad presses
     * LEFT and releases it 250 ms later; if the two shared a mask that release
     * would drop whatever the overlay was holding — which is precisely the drag
     * the overlay exists to make possible.
     */
    @Test
    public void aTapOnTheTouchpadDoesNotReleaseTheOverlaysButton() {
        final Harness h = overlay();
        press(h, 0, LEFT);
        h.down(1, PAD[0], PAD[1]).up(1).advance(h.cfg.clickHoldMs + 50);

        assertEquals(Button.LEFT.mask(), h.cursor.buttons());
        for (String l : h.pointer) {
            assertFalse("button was dropped mid-drag: " + l, l.endsWith(" -"));
        }
        h.up(0);
        assertEquals(0, h.cursor.buttons());
    }

    /** The whole point: hold with one finger, drag the cursor with another. */
    @Test
    public void aSecondFingerStillDrivesTheCursorWhileAButtonIsHeld() {
        final Harness h = overlay();
        press(h, 0, LEFT);
        final float x0 = h.cursor.x();

        h.down(1, PAD[0], PAD[1]);
        for (int i = 1; i <= 10; i++) {
            h.move(1, PAD[0] + i * 20, PAD[1]);
        }
        assertTrue(Harness.count(h.mouse, "move ") > 0);
        assertNotEquals(x0, h.cursor.x(), 1.0f);
        assertEquals(Button.LEFT.mask(), h.cursor.buttons());
    }

    /**
     * Two fingers on the pad still scroll while the overlay holds a button —
     * three pointers down, of which the gesture layer sees exactly the two it
     * needs. Both this and the drag above were confirmed with real fingers.
     */
    @Test
    public void twoFingersStillScrollWhileAButtonIsHeld() {
        final Harness h = overlay();
        press(h, 0, LEFT);
        h.down(1, 1000, 500).down(2, 1200, 500);
        for (int i = 1; i <= 4; i++) {
            h.move(1, 1000, 500 + 10 * i, 2, 1200, 500 + 10 * i);
        }
        h.up(2).up(1).advance(300);

        assertEquals(5, Harness.count(h.mouse, "down WHEEL_DOWN"));
        assertEquals(Button.LEFT.mask(), h.cursor.buttons());
    }

    /** A pointer the overlay claimed is invisible to the gesture layer. */
    @Test
    public void touchesOnTheOverlayNeverReachTheGestureLayer() {
        final Harness h = overlay();
        press(h, 0, LEFT);
        h.move(0, LEFT[0] + 200, LEFT[1] - 60);
        assertEquals(0, h.gestures.downCount());
        assertEquals(0, Harness.count(h.mouse, "move "));
        h.up(0).advance(500);
        // Only the overlay's own press and release; no tap-click from the pad.
        assertEquals(List.of("ovl down LEFT", "ovl up LEFT"), h.overlayEvents());
        assertEquals(0, Harness.count(h.mouse, "down "));
    }

    /** Sliding off a button keeps it down, as an Android Button does. */
    @Test
    public void slidingOffAButtonKeepsItDownUntilTheFingerLifts() {
        final Harness h = overlay();
        press(h, 0, LEFT);
        h.move(0, PAD[0], PAD[1]);
        assertEquals(Button.LEFT.mask(), h.cursor.buttons());
        h.up(0);
        assertEquals(0, h.cursor.buttons());
    }

    @Test
    public void dismissHidesTheOverlay() {
        final Harness h = overlay();
        press(h, 0, DISMISS).up(0);
        assertFalse(h.overlay.visible());
        assertEquals(List.of(), h.overlayEvents());

        // And with it hidden, the same corner is an ordinary click again.
        h.reset().tap(LEFT[0], LEFT[1]);
        assertEquals(List.of("down LEFT", "up LEFT"), h.buttonEvents());
    }

    /** A button held by a widget that has gone away would be held forever. */
    @Test
    public void hidingReleasesWhateverIsHeld() {
        final Harness h = overlay();
        press(h, 0, LEFT);
        h.overlay.setVisible(false);
        assertEquals(0, h.cursor.buttons());
        assertEquals(List.of("ovl down LEFT", "ovl up LEFT"), h.overlayEvents());
    }

    @Test
    public void cancelReleasesEverything() {
        final Harness h = overlay();
        press(h, 0, LEFT);
        press(h, 1, RIGHT);
        h.cancel();
        assertEquals(0, h.cursor.buttons());
        assertEquals(0, h.overlay.heldMask());
    }

    // ---- the wheel strip --------------------------------------------------

    @Test
    public void aTapOnTheStripIsExactlyOneWheelClick() {
        final Harness h = overlay();
        press(h, 0, STRIP_TOP).up(0).advance(1000);
        assertEquals(List.of("ovl down WHEEL_UP", "ovl up WHEEL_UP"), h.overlayEvents());
    }

    @Test
    public void theBottomOfTheStripScrollsDown() {
        final Harness h = overlay();
        press(h, 0, STRIP_BOTTOM).up(0);
        assertEquals(List.of("ovl down WHEEL_DOWN", "ovl up WHEEL_DOWN"), h.overlayEvents());
    }

    /**
     * Held, it repeats: one click immediately, then a pause of
     * {@code overlayWheelStartDelayTicks} (8 × 40 ms) before the repeat clock
     * starts, then one click every {@code 3/rate} ticks. At the end of the strip
     * the rate is 4, so that is every tick.
     */
    @Test
    public void holdingTheStripRepeats() {
        final Harness h = overlay();
        press(h, 0, STRIP_TOP);
        assertEquals(1, wheelClicks(h));
        h.advance(300);
        assertEquals("the start delay has not elapsed", 1, wheelClicks(h));
        h.advance(70);   // 360 ms: 8 delay ticks + 1
        assertEquals(2, wheelClicks(h));
        h.advance(80);   // 400 and 440 ms
        assertEquals(4, wheelClicks(h));
        h.up(0).advance(1000);
        assertEquals("lifting stops the repeat", 4, wheelClicks(h));
    }

    /** Nearer the middle is slower: rate 1 is one click per three ticks. */
    @Test
    public void theRepeatRateFollowsThePositionOnTheStrip() {
        final Harness h = overlay();
        press(h, 0, STRIP_QUARTER_UP);
        h.advance(400);
        assertEquals(1, wheelClicks(h));
        h.advance(50);   // 440 ms: 8 delay ticks + 3
        assertEquals(2, wheelClicks(h));
        h.advance(130);  // 560 ms
        assertEquals(3, wheelClicks(h));
    }

    /** Dragging within the strip re-reads the rate, and can reverse it. */
    @Test
    public void draggingAcrossTheMiddleReversesTheDirection() {
        final Harness h = overlay();
        press(h, 0, STRIP_TOP);
        h.move(0, STRIP_BOTTOM[0], STRIP_BOTTOM[1]);
        h.advance(400);
        assertTrue(Harness.count(h.mouse, "ovl down WHEEL_DOWN") > 0);
        // The one click at touch-down was upwards and nothing has undone it.
        assertEquals(1, Harness.count(h.mouse, "ovl down WHEEL_UP"));
    }

    @Test
    public void theStripDoesNotMoveTheCursor() {
        final Harness h = overlay();
        press(h, 0, STRIP_TOP);
        for (int i = 1; i <= 10; i++) {
            h.move(0, STRIP_TOP[0], i * 80);
        }
        h.up(0);
        assertEquals(0, Harness.count(h.mouse, "move "));
        assertEquals(0, h.gestures.downCount());
    }

    // ---- geometry ---------------------------------------------------------

    @Test
    public void theOverlayIsAnLInTheBottomRightCorner() {
        final Harness h = overlay();
        assertEquals(MouseOverlay.Part.SCROLL, h.overlay.bounds(MouseOverlay.Part.SCROLL).part());
        // 60 dp wide, and it stops at the top of the 72 dp button row.
        final MouseOverlay.Bounds s = h.overlay.bounds(MouseOverlay.Part.SCROLL);
        assertEquals(h.cfg.dp(60), s.width(), 0.01f);
        assertEquals(h.viewH - h.cfg.dp(72), s.bottom(), 0.01f);
        assertEquals(0f, s.top(), 0.01f);

        // The three buttons tile the row, in weights 2:1:2.
        final MouseOverlay.Bounds l = h.overlay.bounds(MouseOverlay.Part.LEFT);
        final MouseOverlay.Bounds m = h.overlay.bounds(MouseOverlay.Part.MIDDLE);
        final MouseOverlay.Bounds r = h.overlay.bounds(MouseOverlay.Part.RIGHT);
        assertEquals(0f, l.left(), 0.01f);
        assertEquals(l.right(), m.left(), 0.01f);
        assertEquals(m.right(), r.left(), 0.01f);
        assertEquals(2.0f, l.width() / m.width(), 0.01f);
        assertEquals(l.width(), r.width(), 0.01f);
        assertEquals(h.cfg.dp(72), l.height(), 0.01f);

        // Everything else is untouched: the top-left of the screen is the pad.
        assertFalse(h.overlay.bounds(MouseOverlay.Part.SCROLL).contains(PAD[0], PAD[1]));
        assertFalse(l.contains(PAD[0], PAD[1]));
    }

    /** Hidden is the default, and a hidden overlay claims nothing. */
    @Test
    public void aHiddenOverlayIsNotThere() {
        final Harness h = Harness.improved().withOverlay().reset();
        h.overlay.setVisible(false);
        h.tap(LEFT[0], LEFT[1]);
        assertEquals(List.of("down LEFT", "up LEFT"), h.buttonEvents());
        assertEquals(List.of(), h.overlayEvents());
    }

    // ---- bump scroll ------------------------------------------------------

    /**
     * Bump scroll arms for a drag started while the overlay holds a button. The
     * original can only arm it inside its own 250 ms window, because that is its
     * only way to hold a button at all.
     */
    @Test
    public void aDragUnderAnOverlayHeldButtonBumpScrolls() {
        assertTrue(bumpMovesWhileDragging(true) >= 2);
        assertEquals(0, bumpMovesWhileDragging(false));
    }

    /**
     * A second finger takes the border test out of the path — it is only
     * evaluated while exactly one finger is down — so a timer left running
     * would go on scrolling for the rest of the gesture, including well away
     * from the border.
     */
    @Test
    public void aSecondFingerStopsBumpScroll() {
        final Harness h = overlay();
        press(h, 0, LEFT);
        h.down(1, 400, 540);
        for (int i = 1; i <= 10; i++) {
            h.move(1, 400 - i * 37, 540);
        }
        h.advance(250);
        assertTrue(Harness.count(h.mouse, "move -31.50,0.00") >= 2);

        h.down(2, 800, 300);
        h.reset();
        h.advance(1000);
        assertEquals(0, Harness.count(h.mouse, "move -31.50,0.00"));
    }

    private static int bumpMovesWhileDragging(boolean holdOverlayButton) {
        final Harness h = overlay();
        if (holdOverlayButton) {
            press(h, 0, LEFT);
        }
        // Drag well into the left border (24 dp = 63 px) and hold there.
        h.down(1, 400, 540);
        for (int i = 1; i <= 10; i++) {
            h.move(1, 400 - i * 37, 540);
        }
        h.reset();
        h.advance(250);
        return Harness.count(h.mouse, "move -31.50,0.00");
    }

    /**
     * The strip's centre is a rate of zero, and a slow drag through it lands on
     * the exact pixel. Forgetting which finger owned the strip there would stop
     * every later movement on it being read at all.
     */
    @Test
    public void aFingerPassingThroughTheStripCentreKeepsScrolling() {
        final Harness h = overlay();
        final float centreY = h.overlay.bounds(MouseOverlay.Part.SCROLL).centreY();
        h.down(0, stripX(h), h.overlay.bounds(MouseOverlay.Part.SCROLL).top() + 10);
        h.move(0, stripX(h), centreY);
        h.reset();
        h.move(0, stripX(h), h.overlay.bounds(MouseOverlay.Part.SCROLL).bottom() - 10);
        h.advance(2000);
        assertTrue(wheelClicks(h) > 0);
    }

    private static float stripX(Harness h) {
        final MouseOverlay.Bounds s = h.overlay.bounds(MouseOverlay.Part.SCROLL);
        return (s.left() + s.right()) / 2.0f;
    }

    private static int wheelClicks(Harness h) {
        return Harness.count(h.mouse, "ovl down WHEEL_");
    }
}
