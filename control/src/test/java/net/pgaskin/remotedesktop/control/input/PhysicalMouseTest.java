// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.input;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

import net.pgaskin.remotedesktop.control.harness.Harness;

import org.junit.Test;

import java.util.List;

/**
 * The physical mouse: the button mask, the wheel, and the two things about it
 * that only exist because it is <em>not</em> a finger.
 */
public class PhysicalMouseTest {

    // MotionEvent button bits.
    private static final int PRIMARY = 1, SECONDARY = 2, TERTIARY = 4;
    private static final int BACK = 8, FORWARD = 16;

    private static Harness mouse() {
        return Harness.improved().withMouse().reset();
    }

    // ---- buttons -----------------------------------------------------------

    @Test
    public void theThreeButtonsAreLeftRightMiddle() {
        // Android's secondary is RFB's *third* bit, which is the one place the
        // two vocabularies disagree about an ordering.
        assertEquals(Button.LEFT.mask(), PhysicalMouse.toRfbMask(PRIMARY));
        assertEquals(Button.RIGHT.mask(), PhysicalMouse.toRfbMask(SECONDARY));
        assertEquals(Button.MIDDLE.mask(), PhysicalMouse.toRfbMask(TERTIARY));
        assertEquals(Button.BACK.mask(), PhysicalMouse.toRfbMask(BACK));
        assertEquals(Button.FORWARD.mask(), PhysicalMouse.toRfbMask(FORWARD));
    }

    @Test
    public void theWholeStateArrivesAtOnceAndOnlyTheChangeIsSent() {
        final Harness h = mouse();
        h.physicalMouse.buttonState(PRIMARY);
        h.physicalMouse.buttonState(PRIMARY);           // no change: nothing
        h.physicalMouse.buttonState(PRIMARY | SECONDARY);
        h.physicalMouse.buttonState(0);
        assertEquals(List.of("phys down LEFT", "phys down RIGHT", "phys up LEFT+RIGHT"),
                h.physicalMouseEvents());
    }

    /**
     * A chord that swaps one button for another releases first. The far end sees
     * a mask, so pressing before releasing would show both held for one event —
     * which on a desktop is a middle-click emulation or a drag that never
     * started.
     */
    @Test
    public void aSwapReleasesBeforeItPresses() {
        final Harness h = mouse();
        h.physicalMouse.buttonState(PRIMARY);
        h.reset();
        h.physicalMouse.buttonState(SECONDARY);
        assertEquals(List.of("phys up LEFT", "phys down RIGHT"), h.physicalMouseEvents());
    }

    /**
     * The bug a third producer of button presses brings. The mouse holds LEFT; a
     * finger taps the touchpad, which presses and releases LEFT of its own 250 ms
     * later — and last-writer-wins on a single mask would drop whatever the mouse
     * was dragging.
     */
    @Test
    public void aTapDuringAMouseHeldDragDoesNotReleaseIt() {
        final Harness h = mouse();
        h.physicalMouse.buttonState(PRIMARY);
        assertTrue(Button.LEFT.in(h.cursor.buttons()));

        h.tap(1200, 500);                      // the gesture layer's own click
        h.advance(cfgClickHold(h) + 50);       // ...and its auto-release
        assertTrue("the mouse is still holding LEFT", Button.LEFT.in(h.cursor.buttons()));

        h.physicalMouse.buttonState(0);
        assertEquals(0, h.cursor.buttons());
    }

    private static long cfgClickHold(Harness h) {
        return h.cfg.clickHoldMs;
    }

    @Test
    public void cancelLetsGoOfWhatIsHeldThere() {
        final Harness h = mouse();
        h.physicalMouse.buttonState(PRIMARY | TERTIARY);
        h.reset();
        h.physicalMouse.cancel();
        assertEquals(List.of("phys up LEFT+MIDDLE"), h.physicalMouseEvents());
        assertEquals(0, h.cursor.buttons());
        // And a second cancel is not a second release.
        h.reset();
        h.physicalMouse.cancel();
        assertEquals(List.of(), h.physicalMouseEvents());
    }

    // ---- motion ------------------------------------------------------------

    @Test
    public void motionGoesThroughUnacceleratedAndDrivesTheSameViewport() {
        final Harness h = mouse();
        final float x0 = h.cursor.x();
        h.physicalMouse.motion(40, 0);
        // 1:1 at scale 1, with no jerk curve anywhere in the path — the whole
        // point of not going through GestureRecognizer.
        assertEquals(x0 + 40, h.cursor.x(), 0.001f);
        assertEquals(List.of("phys move 40.00,0.00"), h.physicalMouseEvents());
    }

    @Test
    public void speedIsTheOnlyDial() {
        final Harness h = mouse();
        h.cfg.mouseSpeed = 2.0f;
        final float x0 = h.cursor.x();
        h.physicalMouse.motion(10, 0);
        assertEquals(x0 + 20, h.cursor.x(), 0.001f);
    }

    @Test
    public void aStationaryMouseSendsNothing() {
        final Harness h = mouse();
        h.physicalMouse.motion(0, 0);
        assertEquals(List.of(), h.physicalMouseEvents());
    }

    // ---- the wheel ---------------------------------------------------------

    @Test
    public void oneNotchIsOneClickAndTheAxisIsInverted() {
        final Harness h = mouse();
        // Android's VSCROLL is positive *up*; the wheel buttons count down.
        h.physicalMouse.scroll(0, -1);
        assertEquals(List.of("phys down WHEEL_DOWN", "phys up WHEEL_DOWN"),
                h.physicalMouseEvents());
        h.reset();
        h.physicalMouse.scroll(0, 1);
        assertEquals(List.of("phys down WHEEL_UP", "phys up WHEEL_UP"),
                h.physicalMouseEvents());
    }

    /**
     * A high-resolution wheel sends fractions of a notch. Rounding each event
     * would give either a click per event or none at all; accumulating gives the
     * same number of clicks per turn as a detented wheel.
     */
    @Test
    public void fractionsOfANotchAccumulateIntoWholeClicks() {
        final Harness h = mouse();
        for (int i = 0; i < 3; i++) {
            h.physicalMouse.scroll(0, -0.25f);
        }
        assertEquals(List.of(), h.physicalMouseEvents());
        h.physicalMouse.scroll(0, -0.25f);
        assertEquals(List.of("phys down WHEEL_DOWN", "phys up WHEEL_DOWN"),
                h.physicalMouseEvents());
    }

    @Test
    public void aFastFlickOfTheWheelIsSeveralClicks() {
        final Harness h = mouse();
        h.physicalMouse.scroll(0, -3);
        assertEquals(6, h.physicalMouseEvents().size());   // three down/up pairs
    }

    @Test
    public void gearingIsNotchesPerClick() {
        final Harness h = mouse();
        h.cfg.mouseWheelStep = 2.0f;
        h.physicalMouse.scroll(0, -1);
        assertEquals(List.of(), h.physicalMouseEvents());
        h.physicalMouse.scroll(0, -1);
        assertEquals(List.of("phys down WHEEL_DOWN", "phys up WHEEL_DOWN"),
                h.physicalMouseEvents());
    }

    @Test
    public void theHorizontalAxisIsNotInverted() {
        final Harness h = mouse();
        h.physicalMouse.scroll(1, 0);
        assertEquals(List.of("phys down WHEEL_RIGHT", "phys up WHEEL_RIGHT"),
                h.physicalMouseEvents());
    }

    @Test
    public void naturalScrollingFlipsBoth() {
        final Harness h = mouse();
        h.cfg.naturalScrolling = true;
        h.physicalMouse.scroll(0, -1);
        assertEquals(List.of("phys down WHEEL_UP", "phys up WHEEL_UP"),
                h.physicalMouseEvents());
    }

    @Test
    public void cancelForgetsAPartialNotch() {
        final Harness h = mouse();
        h.physicalMouse.scroll(0, -0.5f);
        h.physicalMouse.cancel();
        h.reset();
        h.physicalMouse.scroll(0, -0.5f);
        assertEquals("half a notch either side of a cancel is not a click",
                List.of(), h.physicalMouseEvents());
    }
}
