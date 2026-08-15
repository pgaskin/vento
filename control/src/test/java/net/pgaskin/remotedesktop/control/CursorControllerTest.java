// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

import net.pgaskin.remotedesktop.control.harness.FakeScheduler;
import net.pgaskin.remotedesktop.control.harness.Harness;
import net.pgaskin.remotedesktop.control.input.Button;
import net.pgaskin.remotedesktop.control.input.Config;

import org.junit.Before;
import org.junit.Test;

import java.util.ArrayList;
import java.util.List;

/**
 * Screen deltas in, absolute desktop pointer events out — plus the per-frame
 * coalescing, which the original does not have.
 */
public class CursorControllerTest {

    private static final int FB_W = 4000, FB_H = 3000;
    private static final int VIEW_W = 1000, VIEW_H = 800;

    private static final class Sink implements CursorController.PointerSink {
        final List<float[]> events = new ArrayList<>();
        /** The relative half, kept apart so a test cannot pass on the wrong one. */
        final List<int[]> deltas = new ArrayList<>();

        @Override
        public void pointerEvent(float x, float y, int buttons) {
            events.add(new float[]{x, y, buttons});
        }

        @Override
        public void pointerEventRelative(int dx, int dy, int buttons) {
            deltas.add(new int[]{dx, dy, buttons});
        }
    }

    private Config cfg;
    private FakeScheduler clock;
    private Viewport viewport;
    private Sink out;
    private CursorController cursor;

    @Before
    public void setUp() {
        cfg = Config.improved(Harness.DENSITY);
        clock = new FakeScheduler();
        viewport = new Viewport(cfg.density);
        viewport.setDesktopSize(FB_W, FB_H);
        viewport.setViewSize(VIEW_W, VIEW_H);
        out = new Sink();
        cursor = new CursorController(cfg, viewport, out, clock);
        cursor.setPosition(FB_W / 2f, FB_H / 2f);
        out.events.clear();
    }

    @Test
    public void theCursorStaysAtTheViewportCentreWhileTheDesktopSlides() {
        final float ox = viewport.originX();
        cursor.mouseMove(100, 0);
        assertEquals(FB_W / 2f + 100, cursor.x(), 1e-3);
        assertEquals("still dead centre", VIEW_W / 2f, cursor.screenX(), 1e-3);
        assertEquals("because the desktop moved instead", ox - 100, viewport.originX(), 1e-3);
    }

    @Test
    public void atTheEdgeTheCursorLeavesTheCentre() {
        cursor.setPosition(20, 20);
        cursor.mouseMove(-10, -10);
        assertEquals(10f, cursor.x(), 1e-3);
        assertEquals("the viewport is clamped, so the cursor really moves", 10f,
                cursor.screenX(), 1e-3);
    }

    @Test
    public void motionIsInDesktopPixelsSoZoomingSlowsTheCursorDown() {
        viewport.centreOn(FB_W / 2f, FB_H / 2f, 2.0f);
        final float before = cursor.x();
        cursor.mouseMove(100, 0);
        assertEquals("100 screen px at 2x is 50 desktop px", before + 50, cursor.x(), 1e-3);
    }

    @Test
    public void thePositionIsClampedToTheFramebuffer() {
        cursor.mouseMove(1e6f, 1e6f);
        assertEquals(FB_W - 1, cursor.x(), 1e-3);
        assertEquals(FB_H - 1, cursor.y(), 1e-3);
        cursor.mouseMove(-1e6f, -1e6f);
        assertEquals(0f, cursor.x(), 1e-3);
        assertEquals(0f, cursor.y(), 1e-3);
    }

    @Test
    public void motionIsCoalescedToOneEventPerFrame() {
        cursor.mouseMove(10, 0);
        cursor.mouseMove(10, 0);
        cursor.mouseMove(10, 0);
        assertEquals("nothing sent yet", 0, out.events.size());

        clock.advance(clock.frameMs);
        assertEquals(1, out.events.size());
        assertEquals("and it carries the accumulated position",
                FB_W / 2f + 30, out.events.get(0)[0], 1e-3);

        clock.advance(clock.frameMs * 4);
        assertEquals("no repeats once it is idle", 1, out.events.size());
    }

    @Test
    public void withoutCoalescingEveryMoveIsSent() {
        cfg.coalescePointerEvents = false;
        cursor.mouseMove(10, 0);
        cursor.mouseMove(10, 0);
        cursor.mouseMove(10, 0);
        assertEquals(3, out.events.size());
    }

    @Test
    public void buttonChangesFlushImmediately() {
        cursor.mouseMove(10, 0);
        assertEquals(0, out.events.size());
        cursor.mouseDown(Button.LEFT.mask());
        assertEquals("the pending motion is flushed with the press, not after it",
                1, out.events.size());
        assertEquals(FB_W / 2f + 10, out.events.get(0)[0], 1e-3);
        assertEquals(Button.LEFT.mask(), (int) out.events.get(0)[2]);

        clock.advance(clock.frameMs * 4);
        assertEquals("and no duplicate frame callback survives", 1, out.events.size());
    }

    @Test
    public void aWheelClickIsNeverCollapsed() {
        cursor.mouseDown(Button.WHEEL_DOWN.mask());
        cursor.mouseUp(Button.WHEEL_DOWN.mask());
        assertEquals(2, out.events.size());
        assertEquals(Button.WHEEL_DOWN.mask(), (int) out.events.get(0)[2]);
        assertEquals(0, (int) out.events.get(1)[2]);
    }

    /**
     * A finger held still keeps producing ACTION_MOVEs, and a cursor pinned at a
     * desktop edge clamps every delta away — both arrive here as a repeat of the
     * position the remote already has.
     */
    @Test
    public void anUnchangedPositionIsNotSentTwice() {
        cursor.mouseMove(0, 0);
        clock.advance(clock.frameMs);
        assertEquals(1, out.events.size());

        for (int i = 0; i < 10; i++) {
            cursor.mouseMove(0, 0);
            clock.advance(clock.frameMs);
        }
        assertEquals("nothing changed, so nothing was sent", 1, out.events.size());
        assertEquals(10, cursor.suppressedCount());

        cursor.mouseMove(1, 0);
        clock.advance(clock.frameMs);
        assertEquals("but a real move still goes", 2, out.events.size());
    }

    @Test
    public void aButtonChangeAtTheSamePositionAlwaysGoes() {
        cursor.mouseMove(0, 0);
        clock.advance(clock.frameMs);
        final int n = out.events.size();

        cursor.mouseDown(Button.LEFT.mask());
        cursor.mouseUp(Button.LEFT.mask());
        assertEquals("press and release are never deduped", n + 2, out.events.size());
        assertEquals(0, cursor.suppressedCount());
    }

    @Test
    public void withDedupeOffEveryEventGoes() {
        cfg.dedupePointerEvents = false;
        for (int i = 0; i < 5; i++) {
            cursor.mouseMove(0, 0);
            clock.advance(clock.frameMs);
        }
        assertEquals(5, out.events.size());
        assertEquals(0, cursor.suppressedCount());
    }

    @Test
    public void centreCursorPutsItBackInTheMiddleOfTheView() {
        viewport.centreOn(FB_W / 2f, FB_H / 2f, 1.0f);
        cursor.setPosition(FB_W / 2f + 300, FB_H / 2f);
        cursor.centreCursor(true);
        clock.advance(clock.frameMs);
        assertTrue("centring tells the remote where the cursor went", cursor.eventCount() > 0);
        assertEquals(VIEW_W / 2f, cursor.screenX(), 1e-3);
        assertEquals(VIEW_H / 2f, cursor.screenY(), 1e-3);
    }

    /**
     * "The middle" is the middle of the content rect. With a bottom inset —
     * a soft keyboard, or the extension keyboard — the cursor must
     * settle above it, not behind it.
     */
    @Test
    public void centreCursorHonoursTheLayoutInsets() {
        viewport.setInsets(0, 0, 0, 200);
        viewport.centreOn(FB_W / 2f, FB_H / 2f, 1.0f);
        cursor.setPosition(FB_W / 2f + 300, FB_H / 2f);
        cursor.centreCursor(true);
        assertEquals(VIEW_W / 2f, cursor.screenX(), 1e-3);
        assertEquals((VIEW_H - 200) / 2f, cursor.screenY(), 1e-3);
    }

    /** And the same inset keeps the desktop out from under it while panning. */
    @Test
    public void theViewportClampRespectsTheInsets() {
        viewport.setInsets(0, 0, 0, 200);
        cursor.setPosition(FB_W / 2f, FB_H / 2f);
        cursor.mouseMove(0, -100000); // hard against the top edge
        assertEquals(0f, cursor.y(), 1e-3);
        assertEquals("the desktop top lines up with the content top",
                0f, viewport.originY(), 1e-3);
        assertEquals(0f, cursor.screenY(), 1e-3);
    }

    // ---- the window changing shape ----------------------------------------

    /**
     * An overlay appearing must not move the picture. It is the cursor that
     * gives way, not the desktop — see {@link CursorController#setInsets}.
     */
    @Test
    public void insetsMoveTheCursorAndLeaveTheDesktopStill() {
        final float ox = viewport.originX(), oy = viewport.originY();
        final float x = cursor.x(), y = cursor.y();
        out.events.clear();

        cursor.setInsets(0, 0, 160, 240);

        assertEquals("the desktop has not moved", ox, viewport.originX(), 1e-3);
        assertEquals(oy, viewport.originY(), 1e-3);
        assertEquals("the cursor is at the new centre",
                viewport.centreScreenX(), cursor.screenX(), 1e-3);
        assertEquals(viewport.centreScreenY(), cursor.screenY(), 1e-3);
        // Half the inset, in desktop pixels, which is scale 1 here.
        assertEquals(x - 80, cursor.x(), 1e-3);
        assertEquals(y - 120, cursor.y(), 1e-3);

        clock.advance(32);
        assertEquals("and the far end was told where the pointer went",
                1, out.events.size());
    }

    /** Taking them away again puts it back, with the desktop still still. */
    @Test
    public void removingTheInsetsIsTheSameMoveBackwards() {
        final float ox = viewport.originX(), oy = viewport.originY();
        final float x = cursor.x(), y = cursor.y();
        cursor.setInsets(0, 0, 160, 240);
        cursor.setInsets(0, 0, 0, 0);
        assertEquals(ox, viewport.originX(), 1e-3);
        assertEquals(oy, viewport.originY(), 1e-3);
        assertEquals(x, cursor.x(), 1e-3);
        assertEquals(y, cursor.y(), 1e-3);
    }

    /**
     * Against a desktop edge the clamp owns the origin and the cursor is not at
     * the centre, so there is nothing to preserve: the desktop moves, because
     * its edge has to stay against the content edge, and the pointer keeps the
     * desktop position it was aimed at.
     */
    @Test
    public void atAnEdgeTheCursorKeepsItsPositionAndTheDesktopMoves() {
        cursor.mouseMove(100000, 0);   // hard against the right edge
        final float x = cursor.x(), y = cursor.y();
        final float ox = viewport.originX();

        cursor.setInsets(0, 0, 160, 0);

        assertEquals("the pointer is still on the same pixel", x, cursor.x(), 1e-3);
        assertEquals(y, cursor.y(), 1e-3);
        assertEquals("the desktop edge follows the content edge",
                ox - 160, viewport.originX(), 1e-3);
    }

    /**
     * The other half of the same question, and the one the device found: on an
     * axis where the desktop is <em>smaller</em> than the window there is no
     * clamp and the cursor is not what holds the picture — it is simply placed
     * in the spare space, so re-placing it in a content rect that just shrank
     * moves the whole desktop by half the inset. A 1920×1200 desktop on a
     * portrait phone is letterboxed exactly like this.
     */
    @Test
    public void aLetterboxedDesktopIsNotRecentredByAnOverlay() {
        // Wider than the window and much shorter: the x axis is the ordinary
        // cursor-and-clamp one, the y axis is the letterbox.
        viewport.setDesktopSize(1600, 200);
        cursor.setPosition(800, 100);
        final float ox = viewport.originX(), oy = viewport.originY();
        assertEquals("centred to start with", (VIEW_H - 200) / 2f, oy, 1e-3);

        cursor.setInsets(0, 0, 0, 240);
        assertEquals(ox, viewport.originX(), 1e-3);
        assertEquals(oy, viewport.originY(), 1e-3);

        cursor.setInsets(0, 0, 0, 0);
        assertEquals("and centred again when the window comes back",
                oy, viewport.originY(), 1e-3);
    }

    /** Unless it cannot be: the picture never ends up under the inset. */
    @Test
    public void aLetterboxedDesktopMovesOnlyAsFarAsItMust() {
        viewport.setDesktopSize(1600, 200);
        cursor.setPosition(800, 100);

        cursor.setInsets(0, 0, 0, 500);   // 300 px of content left, 200 of desktop
        assertTrue("pushed up out of the inset, and no further",
                viewport.originY() > 99 && viewport.originY() <= 100.001);
    }

    // ---- relative mode: the far end owns the cursor -------------------------

    /**
     * The four lines that are the whole centre-follow behaviour stop running:
     * the delta goes out as a delta, the position stands still, and so does the
     * desktop — because nothing here knows where the pointer is any more.
     */
    @Test
    public void inRelativeModeTheDeltaGoesOutAndNothingHereMoves() {
        cursor.setRelative(true);
        final float ox = viewport.originX();
        final float x = cursor.x(), y = cursor.y();

        cursor.mouseMove(30, -20);
        clock.advance(clock.frameMs);

        assertEquals("no absolute events at all", 0, out.events.size());
        assertEquals(1, out.deltas.size());
        assertArrayEquals(new int[]{30, -20, 0}, out.deltas.get(0));
        assertEquals("our idea of the cursor is frozen", x, cursor.x(), 1e-3);
        assertEquals(y, cursor.y(), 1e-3);
        assertEquals("and the desktop does not slide under it", ox, viewport.originX(), 1e-3);
    }

    /** A zoomed screen means fewer desktop pixels, for a delta as for a position. */
    @Test
    public void aRelativeDeltaIsStillInDesktopPixels() {
        viewport.centreOn(FB_W / 2f, FB_H / 2f, 2.0f);
        cursor.setRelative(true);

        cursor.mouseMove(100, 0);
        clock.advance(clock.frameMs);

        assertArrayEquals(new int[]{50, 0, 0}, out.deltas.get(0));
    }

    /**
     * The wire carries whole pixels, and a careful drag is a stream of deltas
     * each smaller than one. Rounding each on its own would send nothing at
     * all — so the fraction is owed, not lost.
     */
    @Test
    public void aSlowDragOwesItsFractionsRatherThanLosingThem() {
        cursor.setRelative(true);
        for (int i = 0; i < 10; i++) {
            cursor.mouseMove(0.4f, 0);
            clock.advance(clock.frameMs);
        }
        int total = 0;
        for (int[] d : out.deltas) {
            total += d[0];
        }
        assertEquals("ten quarters of a pixel is four pixels, not zero", 4, total);
    }

    /**
     * The dedupe is the absolute one's mirror image: a second delta identical
     * to the first is a second real movement, and only a frame with nothing
     * owed and no button change is worth swallowing.
     */
    @Test
    public void anIdenticalRelativeDeltaIsNotADuplicate() {
        cursor.setRelative(true);
        for (int i = 0; i < 3; i++) {
            cursor.mouseMove(5, 0);
            clock.advance(clock.frameMs);
        }
        assertEquals(3, out.deltas.size());

        final int idle = out.deltas.size();
        clock.advance(clock.frameMs * 4);
        assertEquals("but a still finger sends nothing", idle, out.deltas.size());
    }

    /** A button change is a message even with nowhere to move. */
    @Test
    public void aRelativeButtonPressCarriesNoMotion() {
        cursor.setRelative(true);
        cursor.mouseDown(Button.LEFT.mask());
        assertEquals(1, out.deltas.size());
        assertArrayEquals(new int[]{0, 0, Button.LEFT.mask()}, out.deltas.get(0));

        cursor.mouseUp(Button.LEFT.mask());
        assertArrayEquals(new int[]{0, 0, 0}, out.deltas.get(1));
    }

    /**
     * A host draws on this notification, so a press with no movement has to
     * carry one: without it a button-only change is invisible to anything whose
     * "remote" is in the same process and sends no frame back.
     */
    @Test
    public void aButtonWithNoMovementStillTellsTheListener() {
        final int[] notified = {0};
        cursor.setListener(() -> notified[0]++);

        cursor.mouseDown(Button.LEFT.mask());
        assertEquals(1, notified[0]);
        cursor.mouseUp(Button.LEFT.mask());
        assertEquals(2, notified[0]);
    }

    /**
     * A window that grows can leave the picture smaller than the space for it,
     * which is the one case where the scale has to change.
     */
    @Test
    public void aGrowingWindowRaisesAScaleBelowTheNewFit() {
        viewport.setInsets(0, 0, 0, 600);
        viewport.zoomToFit();
        final float fitted = viewport.getScale();

        cursor.setInsets(0, 0, 0, 0);
        assertTrue("the fit scale went up with the window",
                viewport.getScale() > fitted);
        assertEquals(viewport.minScale(), viewport.getScale(), 1e-3);
    }
}
