// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.input;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import net.pgaskin.remotedesktop.control.harness.Harness;

import org.junit.Test;

import java.util.List;

/**
 * The toolbar: four buttons and a grip on the left edge of the harness's
 * 2400×1080 view at density 2.625, so a button is 115.5 px (44 dp) square, the
 * grip is 63 px (24 dp), and the whole column is 525 px — the 200 dp the
 * platform will give back per edge.
 */
public class ToolbarTest {

    private static Harness bar() {
        return Harness.improved().withToolbar().reset();
    }

    /** The middle of the {@code i}th button. */
    private static float[] button(Harness h, int i) {
        final Toolbar.Bounds b = h.toolbar.items().get(i);
        return new float[]{b.centreX(), b.centreY()};
    }

    // ---- the buttons -------------------------------------------------------

    @Test
    public void theColumnIsFourButtonsAndAGripOnTheLeftEdge() {
        final Harness h = bar();
        assertEquals(4, h.toolbar.items().size());
        assertEquals(0f, h.toolbar.left(), 0.01f);
        assertEquals(h.cfg.dp(44), h.toolbar.right(), 0.01f);
        assertEquals(h.cfg.dp(200), h.toolbar.bottom() - h.toolbar.top(), 0.01f);
        for (int i = 0; i < 4; i++) {
            final Toolbar.Bounds b = h.toolbar.items().get(i);
            assertEquals(h.cfg.dp(44), b.height(), 0.01f);
            assertEquals(h.cfg.dp(44), b.width(), 0.01f);
        }
        final Toolbar.Bounds grip = h.toolbar.grip();
        assertNotNull(grip);
        assertEquals("the grip is under the buttons, away from disconnect",
                h.toolbar.items().get(3).bottom(), grip.top(), 0.01f);
    }

    /**
     * A button fires on release, as the extension row's keys do: a press that
     * turns out to be a drag has to be able to change its mind.
     */
    @Test
    public void aButtonFiresOnRelease() {
        final Harness h = bar();
        final float[] p = button(h, 0);
        h.down(0, p[0], p[1]);
        assertEquals(List.of(), h.toolbarTaps);
        h.up(0);
        assertEquals(List.of("toolbar disconnect"), h.toolbarTaps);
    }

    /** ... and only if the lift is still on the button it started on. */
    @Test
    public void aLiftSomewhereElseAbandonsThePress() {
        final Harness h = bar();
        final float[] a = button(h, 0);
        final float[] b = button(h, 1);
        h.down(0, a[0], a[1]);
        h.up(0, b[0], b[1]);
        assertEquals(List.of(), h.toolbarTaps);
    }

    /** A pointer on the column never reaches the gesture layer behind it. */
    @Test
    public void aTouchOnTheColumnIsClaimed() {
        final Harness h = bar();
        final float[] p = button(h, 2);
        h.down(0, p[0], p[1]).up(0);
        assertEquals(List.of(), h.mouse);
        assertEquals(List.of("toolbar keyboard"), h.toolbarTaps);
    }

    /** Beside it is the desktop, and a tap there is a click as it always was. */
    @Test
    public void aTouchBesideTheColumnIsNot() {
        final Harness h = bar();
        final float[] p = button(h, 0);
        h.down(0, h.toolbar.right() + 10, p[1]).up(0);
        assertEquals(List.of(), h.toolbarTaps);
        assertFalse(h.mouse.isEmpty());
    }

    /** A second finger on it is claimed so it cannot reach the pad, then ignored. */
    @Test
    public void aSecondFingerIsClaimedAndIgnored() {
        final Harness h = bar();
        final float[] a = button(h, 0);
        final float[] b = button(h, 1);
        h.down(0, a[0], a[1]);
        h.down(1, b[0], b[1]);
        h.up(1);
        h.up(0);
        assertEquals(List.of("toolbar disconnect"), h.toolbarTaps);
        assertEquals(List.of(), h.mouse);
    }

    @Test
    public void aCancelLetsGoOfEverything() {
        final Harness h = bar();
        final float[] p = button(h, 0);
        h.down(0, p[0], p[1]);
        h.cancel();
        assertFalse(h.toolbar.touched());
        assertNull(h.toolbar.pressedItem());
        assertEquals(List.of(), h.toolbarTaps);
    }

    // ---- the drag ----------------------------------------------------------

    /**
     * The grip is a hint rather than a requirement: a pointer that lands on a
     * button and travels drags the column too, and gives up on the button.
     */
    @Test
    public void aDragPastTheSlopAbandonsThePressAndMovesTheColumn() {
        final Harness h = bar();
        final float[] p = button(h, 0);
        final float was = h.toolbar.top();
        h.down(0, p[0], p[1]);
        h.move(0, p[0], p[1] + h.cfg.toolbarDragSlopPx + 5);
        assertTrue(h.toolbar.dragging());
        assertNull(h.toolbar.pressedItem());
        h.move(0, p[0], p[1] + 200);
        h.up(0, p[0], p[1] + 200);
        assertEquals(was + 200, h.toolbar.top(), 1.0f);
        assertEquals("the move is reported, the button is not",
                1, h.toolbarTaps.size());
        assertTrue(h.toolbarTaps.get(0).startsWith("toolbar moved"));
    }

    /** Inside the slop it is still a press. */
    @Test
    public void aTwitchIsStillAPress() {
        final Harness h = bar();
        final float[] p = button(h, 1);
        h.down(0, p[0], p[1]);
        h.move(0, p[0], p[1] + h.cfg.toolbarDragSlopPx - 1);
        h.up(0, p[0], p[1] + h.cfg.toolbarDragSlopPx - 1);
        assertEquals(List.of("toolbar information"), h.toolbarTaps);
    }

    /** A pointer on the grip is dragging from the first pixel. */
    @Test
    public void aTouchOnTheGripDragsAtOnce() {
        final Harness h = bar();
        final Toolbar.Bounds g = h.toolbar.grip();
        h.down(0, g.centreX(), g.centreY());
        assertTrue(h.toolbar.dragging());
        h.up(0);
        assertEquals(1, h.toolbarTaps.size());
    }

    @Test
    public void theColumnClampsAtBothEndsOfItsBand() {
        final Harness h = bar();
        final Toolbar.Bounds g = h.toolbar.grip();
        h.down(0, g.centreX(), g.centreY());
        h.move(0, g.centreX(), -5000);
        assertEquals(0f, h.toolbar.top(), 0.01f);
        assertEquals(0f, h.toolbar.position(), 0.001f);
        h.move(0, g.centreX(), 5000);
        assertEquals(h.viewH, h.toolbar.bottom(), 0.01f);
        assertEquals(1f, h.toolbar.position(), 0.001f);
        h.up(0);
    }

    /**
     * The band it may be dragged in is bounded by what the keyboard
     * <em>occupies</em>, since the two are both drawn over the desktop and the
     * column must not end up under the key row.
     */
    @Test
    public void theKeyboardTakesTheBottomOfTheBand() {
        final Harness h = bar();
        final float keyboard = h.cfg.dp(200);
        h.toolbar.setInsets(0, 0, keyboard);
        h.toolbar.setPosition(1f);
        assertEquals(h.viewH - keyboard, h.toolbar.bottom(), 0.01f);
    }

    /** The position is a fraction of that band, so a rotation keeps it sensible. */
    @Test
    public void thePositionRoundTripsThroughItsFraction() {
        final Harness h = bar();
        h.toolbar.setPosition(0.25f);
        final float top = h.toolbar.top();
        h.toolbar.setViewSize(h.viewH, h.viewW);   // a rotation
        h.toolbar.setViewSize(h.viewW, h.viewH);
        assertEquals(0.25f, h.toolbar.position(), 0.001f);
        assertEquals(top, h.toolbar.top(), 0.01f);
    }

    /** A band with no room in it puts the column at the top rather than nowhere. */
    @Test
    public void aBandWithNoRoomClampsToTheTop() {
        final Harness h = bar();
        h.toolbar.setInsets(0, 0, h.viewH);
        h.toolbar.setPosition(1f);
        assertEquals(0f, h.toolbar.top(), 0.01f);
    }

    // ---- the item list is the caller's -------------------------------------

    /**
     * A shorter list is how a view-only session offers three buttons rather than
     * four greyed ones, and one item is what proves the column is not four
     * things in a trench coat.
     */
    @Test
    public void aListOfOneWorks() {
        final Harness h = Harness.improved()
                .withToolbar(List.of(new Toolbar.Item("disconnect", "disconnect"))).reset();
        assertEquals(1, h.toolbar.items().size());
        assertEquals(h.cfg.dp(44) + h.cfg.dp(24),
                h.toolbar.bottom() - h.toolbar.top(), 0.01f);
        final float[] p = button(h, 0);
        h.down(0, p[0], p[1]).up(0);
        assertEquals(List.of("toolbar disconnect"), h.toolbarTaps);
    }

    /** Hidden, it claims nothing: the touch belongs to the desktop again. */
    @Test
    public void aHiddenColumnClaimsNothing() {
        final Harness h = bar();
        final float[] p = button(h, 0);
        h.toolbar.setVisible(false);
        h.down(0, p[0], p[1]).up(0);
        assertEquals(List.of(), h.toolbarTaps);
        assertFalse(h.mouse.isEmpty());
    }
}
