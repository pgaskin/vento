// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.input;

import static java.util.Arrays.asList;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

import net.pgaskin.remotedesktop.control.harness.Harness;

import org.junit.Test;

import java.util.List;

/**
 * The gesture state machine, driven on a virtual clock.
 *
 * <p>Every number here is one of the original's, listed in
 * {@code ARCHITECTURE.md} §1.15; at the
 * harness's 2.625 density the 12 dp displacement threshold is 31.5 px and the
 * 24 dp path threshold is 63 px.
 */
public class GestureRecognizerTest {

    private static final float FAR = 60f;   // > 31.5 px, definitely a drag
    private static final float NEAR = 1f;   // hold-still wobble, definitely not

    // ---- taps -------------------------------------------------------------

    @Test
    public void tapPressesOnUpAndReleasesAfterTheHoldWindow() {
        final Harness h = Harness.improved();
        h.down(0, 500, 500).up(0);
        assertEquals("press happens on touch-up, not touch-down",
                asList("down LEFT"), h.buttonEvents());

        h.advance(cfg(h) - 1);
        assertEquals(asList("down LEFT"), h.buttonEvents());
        h.advance(2);
        assertEquals(asList("down LEFT", "up LEFT"), h.buttonEvents());
    }

    @Test
    public void twoFingerTapIsRightClick() {
        final Harness h = Harness.improved();
        h.down(0, 500, 500).down(1, 700, 520).up(1).up(0).advance(300);
        assertEquals(asList("down RIGHT", "up RIGHT"), h.buttonEvents());
    }

    @Test
    public void threeFingerTapIsMiddleClick() {
        final Harness h = Harness.improved();
        h.down(0, 500, 500).down(1, 700, 520).down(2, 900, 540)
                .up(2).up(1).up(0).advance(300);
        assertEquals(asList("down MIDDLE", "up MIDDLE"), h.buttonEvents());
    }

    @Test
    public void fourFingerTapDoesNothing() {
        final Harness h = Harness.improved();
        h.down(0, 400, 500).down(1, 600, 500).down(2, 800, 500).down(3, 1000, 500)
                .up(3).up(2).up(1).up(0).advance(300);
        assertEquals(asList(), h.buttonEvents());
    }

    /**
     * Two taps inside the hold window are a double click: the pending release
     * is cancelled by the second touch-down, and the second touch-up releases
     * and re-presses. So the remote sees two clean click pairs, the second one
     * as fast as the taps were.
     */
    @Test
    public void twoTapsInsideTheWindowAreADoubleClick() {
        final Harness h = Harness.improved();
        h.down(0, 500, 500).up(0).advance(100);
        assertEquals("the first release is still pending at +100 ms",
                asList("down LEFT"), h.buttonEvents());

        h.down(0, 500, 500).up(0);
        assertEquals(asList("down LEFT", "up LEFT", "down LEFT"), h.buttonEvents());
        h.advance(300);
        assertEquals(asList("down LEFT", "up LEFT", "down LEFT", "up LEFT"),
                h.buttonEvents());
    }

    // ---- drags ------------------------------------------------------------

    @Test
    public void dragMovesTheCursorAndClicksNothing() {
        final Harness h = Harness.improved();
        h.drag(0, 500, 500, FAR, 0, 6).advance(500);
        assertEquals(asList(), h.buttonEvents());
        assertTrue("expected motion", Harness.count(h.mouse, "move ") >= 5);
    }

    @Test
    public void tapThenDragIsALeftButtonDrag() {
        final Harness h = Harness.improved();
        h.down(0, 500, 500).up(0).advance(100);
        h.drag(0, 500, 500, FAR, FAR, 6);
        assertEquals(asList("down LEFT", "up LEFT"), h.buttonEvents());

        final int downAt = h.mouse.indexOf("down LEFT");
        final int upAt = h.mouse.indexOf("up LEFT");
        int moved = 0;
        for (int i = downAt; i < upAt; i++) {
            if (h.mouse.get(i).startsWith("move ")) {
                moved++;
            }
        }
        assertTrue("the drag must happen while the button is held", moved >= 3);
    }

    @Test
    public void twoFingerTapThenDragIsARightButtonDrag() {
        final Harness h = Harness.improved();
        h.down(0, 500, 500).down(1, 700, 520).up(1).up(0).advance(100);
        h.drag(0, 500, 500, FAR, 0, 6);
        assertEquals(asList("down RIGHT", "up RIGHT"), h.buttonEvents());
    }

    /** A gesture that ends with a finger still down elsewhere must not click. */
    @Test
    public void cancelledGestureClicksNothing() {
        final Harness h = Harness.improved();
        h.down(0, 500, 500).cancel().advance(500);
        assertEquals(asList(), h.buttonEvents());
    }

    // ---- tap / drag disambiguation ---------------------------------------

    /**
     * vncpatch#1: a stationary tap on a high-report-rate screen emits more than
     * ten ACTION_MOVEs, and the original counts that as a drag — so the tap is
     * silently swallowed. The improved preset must not do that.
     */
    @Test
    public void manyTinyMovesAreStillATap() {
        final Harness improved = Harness.improved();
        jitter(improved, 15, NEAR);
        assertEquals("15 jitter events must not eat the click",
                asList("down LEFT", "up LEFT"), improved.buttonEvents());

        final Harness faithful = Harness.faithful();
        jitter(faithful, 15, NEAR);
        assertEquals("the original loses this tap (the Pixel 9 bug)",
                asList(), faithful.buttonEvents());
    }

    @Test
    public void tenTinyMovesAreATapEvenInTheOriginal() {
        final Harness h = Harness.faithful();
        jitter(h, 10, NEAR);
        assertEquals(asList("down LEFT", "up LEFT"), h.buttonEvents());
    }

    /**
     * The path threshold that replaced the move-count test is harder to trip,
     * but it is still an accumulator, so enough wobble eats the tap the same
     * way: 15 events at ±3 px is 87 px of path, past the 63 px threshold.
     * How much path a real stationary tap accumulates is a question for the
     * recorded fixtures, and {@link FixtureReplayTest} is where it is asked.
     */
    @Test
    public void thePathThresholdIsStillAnAccumulatorAndCanEatALongWobblyTap() {
        final Harness h = Harness.improved();
        jitter(h, 15, 3f);
        assertEquals(asList(), h.buttonEvents());

        final Harness shorter = Harness.improved();
        jitter(shorter, 8, 3f);
        assertEquals("45 px of path is still a tap",
                asList("down LEFT", "up LEFT"), shorter.buttonEvents());
    }

    /** A slow scrub that never gets 12 dp from its origin is still a drag. */
    @Test
    public void longPathWithoutDisplacementIsADrag() {
        final Harness h = Harness.improved();
        h.down(0, 500, 500);
        for (int i = 0; i < 8; i++) {
            h.move(0, 510, 500);
            h.move(0, 500, 500);
        }
        h.up(0).advance(300);
        assertEquals("160 px of travel is not a tap", asList(), h.buttonEvents());
    }

    /** A stationary touch that wobbles by ±{@code amp} px, {@code moves} times. */
    private static void jitter(Harness h, int moves, float amp) {
        h.down(0, 500, 500);
        for (int i = 0; i < moves; i++) {
            h.move(0, 500 + (i % 2 == 0 ? amp : -amp), 500);
        }
        h.up(0).advance(300);
    }

    // ---- two-finger: zoom vs. scroll -------------------------------------

    @Test
    public void separatingFingersZooms() {
        final Harness h = Harness.improved();
        h.down(0, 1000, 500).down(1, 1400, 500);
        for (int i = 1; i <= 4; i++) {
            h.move(0, 1000 - 25 * i, 500, 1, 1400 + 25 * i, 500);
        }
        h.up(1).up(0).advance(300);

        assertTrue("expected a pinch", h.zoom.contains("zoomBegan"));
        assertEquals("a pinch must not click", asList(), h.buttonEvents());
        final List<String> changes = Harness.only(h.zoom, "zoomChanged");
        assertEquals("zoomChanged 1.5000", changes.get(changes.size() - 1));
        // Fingers separating about a fixed midpoint: all zoom, no pan.
        assertEquals(0f, totalPan(h)[0], 1e-3);
        assertEquals(0f, totalPan(h)[1], 1e-3);
    }

    /**
     * The pan is the midpoint's travel, so two fingers that keep their
     * separation while both moving are still a pinch — the ratio decided that —
     * and every pixel they travel is a pan.
     */
    @Test
    public void anEngagedPinchThatTravelsPans() {
        final Harness h = Harness.improved().withRelativePointer();
        h.down(0, 1000, 500).down(1, 1400, 500);
        // Separate first, so the gesture latches as a zoom ...
        for (int i = 1; i <= 4; i++) {
            h.move(0, 1000 - 25 * i, 500, 1, 1400 + 25 * i, 500);
        }
        assertEquals(GestureRecognizer.Mode.ZOOM, h.gestures.mode());
        final float originX = h.viewport.originX();
        final float scale = h.viewport.getScale();

        // ... then travel with the separation held, which is pan and nothing else.
        for (int i = 1; i <= 4; i++) {
            h.move(0, 900 + 20 * i, 500, 1, 1500 + 20 * i, 500);
        }
        h.up(1).up(0).advance(300);

        // One finger at a time, since that is how a touch stream arrives: each
        // event moves the midpoint by half of its own finger's delta.
        assertEquals("the pan is the midpoint's travel", 80f, totalPan(h)[0], 1e-3);
        assertEquals(0f, totalPan(h)[1], 1e-3);
        assertEquals("the scale is unchanged by a pan", scale, h.viewport.getScale(), 1e-4);
        // Within a pixel or two: the fingers arrive one event at a time, so
        // between them the separation — and with it the scale a pan is divided
        // by — wobbles by one finger's step.
        assertEquals("the picture followed the fingers",
                originX + 80, h.viewport.originX(), 2.0);
        assertEquals("a pan tells the far end nothing", 0, h.pointer.size());
    }

    // ---- a far end that owns the cursor -----------------------------------

    /**
     * That end accelerates whatever arrives, so nothing here shapes it first:
     * the finger's own deltas go out, and a flick leaves no glide behind to
     * keep sending more of them.
     */
    @Test
    public void motionGoesOutUnshapedWhereTheFarEndOwnsTheCursor() {
        final Harness h = flick(Harness.improved().withRelativePointer());

        final List<String> moves = Harness.only(h.mouse, "move ");
        assertTrue("the gesture must have emitted something", moves.size() >= 4);
        for (String m : moves) {
            assertEquals("every delta is the finger's own", "move 120.00,0.00", m);
        }
        assertEquals("and nothing glides afterwards", 0.0, h.gestures.glideSpeed(), 0.0);
    }

    /** The same finger, with the cursor ours: the whole stack applies. */
    @Test
    public void theSameFingerIsShapedWhereTheCursorIsOurs() {
        final Harness h = flick(Harness.improved());
        assertTrue("the curve should have amplified something",
                Harness.only(h.mouse, "move ").stream().anyMatch(m -> !m.equals("move 120.00,0.00")));
        assertTrue("and the flick should be gliding", h.gestures.glideSpeed() > 0);
    }

    /** And the switch is a switch: off, a relative session is shaped like any other. */
    @Test
    public void theShapingCanBeKeptInRelativeModeToo() {
        final Harness h = Harness.improved();
        h.cfg.rawMotionWhenRelative = false;
        flick(h.withRelativePointer());
        assertTrue(Harness.only(h.mouse, "move ").stream()
                .anyMatch(m -> !m.equals("move 120.00,0.00")));
        assertTrue(h.gestures.glideSpeed() > 0);
    }

    /** A fast straight drag, fast enough to flick, in 120 px steps every 10 ms. */
    private static Harness flick(Harness h) {
        h.step = 10;
        h.down(0, 500, 500);
        for (int i = 1; i <= 6; i++) {
            h.move(0, 500 + i * 120f, 500);
        }
        h.up(0).advance(h.cfg.inertiaStartDelayMs + h.cfg.inertiaTickMs * 3);
        return h;
    }

    /** Everything the recognizer asked to be panned by, summed, x then y. */
    private static float[] totalPan(Harness h) {
        final float[] sum = new float[2];
        for (String line : Harness.only(h.zoom, "zoomPanned")) {
            final String[] d = line.substring("zoomPanned ".length()).split(",");
            sum[0] += Float.parseFloat(d[0]);
            sum[1] += Float.parseFloat(d[1]);
        }
        return sum;
    }

    /** With the cursor ours, the same gesture is the pinch it has always been. */
    @Test
    public void anEngagedPinchThatTravelsDoesNotPanTheCursorsOwnViewport() {
        final Harness h = Harness.improved();
        h.down(0, 1000, 500).down(1, 1400, 500);
        for (int i = 1; i <= 4; i++) {
            h.move(0, 1000 - 25 * i, 500, 1, 1400 + 25 * i, 500);
        }
        final float originX = h.viewport.originX();
        for (int i = 1; i <= 4; i++) {
            h.move(0, 900 + 20 * i, 500, 1, 1500 + 20 * i, 500);
        }
        h.up(1).up(0).advance(300);

        assertEquals(originX, h.viewport.originX(), 0.5);
    }

    @Test
    public void parallelFingersScrollTheWheel() {
        final Harness h = Harness.improved();
        h.down(0, 1000, 500).down(1, 1200, 500);
        for (int i = 1; i <= 4; i++) {
            h.move(0, 1000, 500 + 10 * i, 1, 1200, 500 + 10 * i);
        }
        h.up(1).up(0).advance(300);

        assertEquals("downward two-finger drag scrolls down",
                5, Harness.count(h.mouse, "down WHEEL_DOWN"));
        assertEquals(0, Harness.count(h.mouse, "down WHEEL_UP"));
        assertEquals("a scroll must not click", 0, Harness.count(h.mouse, "down LEFT"));
    }

    @Test
    public void naturalScrollingInvertsTheWheel() {
        final Harness h = Harness.improved();
        h.cfg.naturalScrolling = true;
        h.down(0, 1000, 500).down(1, 1200, 500);
        for (int i = 1; i <= 4; i++) {
            h.move(0, 1000, 500 + 10 * i, 1, 1200, 500 + 10 * i);
        }
        h.up(1).up(0).advance(300);

        assertEquals(0, Harness.count(h.mouse, "down WHEEL_DOWN"));
        assertEquals(5, Harness.count(h.mouse, "down WHEEL_UP"));
    }

    /** Two fingers moving sideways send horizontal wheel clicks, not motion. */
    @Test
    public void twoFingerDragDoesNotMoveTheCursor() {
        final Harness h = Harness.improved();
        h.down(0, 1000, 500).down(1, 1200, 500);
        for (int i = 1; i <= 4; i++) {
            h.move(0, 1000 + 10 * i, 500, 1, 1200 + 10 * i, 500);
        }
        h.up(1).up(0).advance(300);

        assertEquals(0, Harness.count(h.mouse, "move "));
        assertEquals(5, Harness.count(h.mouse, "down WHEEL_RIGHT"));
    }

    // ---- the fixed one-finger-after-two-fingers bug -----------------------

    /**
     * Lifting the <em>first</em> of two fingers and dragging with the second:
     * the original reads slot 0's moved flag and the drag is silently dead
     * ({@code ARCHITECTURE.md} §2.3). Ours uses whichever slot is still down.
     */
    @Test
    public void dragContinuesAfterLiftingTheFirstFinger() {
        final Harness h = Harness.improved();
        h.down(0, 500, 500).down(1, 900, 500);
        h.move(0, 500, 500, 1, 900 + FAR, 500);
        h.up(0);
        h.move(1, 900 + 2 * FAR, 500);
        h.move(1, 900 + 3 * FAR, 500);
        h.up(1).advance(300);

        assertTrue("the second finger must still drive the cursor",
                Harness.count(h.mouse, "move ") > 0);
    }

    private static long cfg(Harness h) {
        return h.cfg.clickHoldMs;
    }
}
