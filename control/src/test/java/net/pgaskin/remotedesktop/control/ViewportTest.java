// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

import org.junit.Before;
import org.junit.Test;

/**
 * Centre-then-clamp and scale snapping. These are pure functions of
 * (framebuffer, view, scale) and they are the whole reason the cursor appears
 * to stay in the middle of the screen, so they are worth pinning down before
 * anyone tunes them.
 */
public class ViewportTest {

    private static final float DENSITY = 2.625f;
    private static final int FB_W = 4000, FB_H = 3000;
    private static final int VIEW_W = 1000, VIEW_H = 800;

    private Viewport v;

    @Before
    public void setUp() {
        v = new Viewport(DENSITY);
        v.setDesktopSize(FB_W, FB_H);
        v.setViewSize(VIEW_W, VIEW_H);
    }

    @Test
    public void inTheInteriorTheFocusPointSitsAtTheViewportCentre() {
        v.centreOn(2000, 1500, 1.0f);
        assertEquals(VIEW_W / 2f, v.toScreenX(2000), 1e-3);
        assertEquals(VIEW_H / 2f, v.toScreenY(1500), 1e-3);
        assertEquals(-1500f, v.originX(), 1e-3);
        assertEquals(-1100f, v.originY(), 1e-3);
    }

    @Test
    public void nearAnEdgeTheClampTakesOverAndTheCursorLeavesTheCentre() {
        v.centreOn(50, 50, 1.0f);
        assertEquals("origin cannot go positive", 0f, v.originX(), 1e-3);
        assertEquals(0f, v.originY(), 1e-3);
        assertEquals("so the cursor is drawn where it actually is", 50f, v.toScreenX(50), 1e-3);
        assertEquals(50f, v.toScreenY(50), 1e-3);

        v.centreOn(FB_W - 10, FB_H - 10, 1.0f);
        assertEquals(VIEW_W - FB_W, v.originX(), 1e-3);
        assertEquals(VIEW_H - FB_H, v.originY(), 1e-3);
        assertEquals(VIEW_W - 10, v.toScreenX(FB_W - 10), 1e-3);
    }

    @Test
    public void aDesktopSmallerThanTheViewIsCentred() {
        v.setDesktopSize(200, 100);
        v.centreOn(100, 50, 1.0f);
        assertEquals((VIEW_W - 200) / 2f, v.originX(), 1e-3);
        assertEquals((VIEW_H - 100) / 2f, v.originY(), 1e-3);
    }

    @Test
    public void scaleAppliesAboutTheFocusPoint() {
        v.centreOn(2000, 1500, 2.0f);
        assertEquals(VIEW_W / 2f, v.toScreenX(2000), 1e-3);
        assertEquals(VIEW_H / 2f, v.toScreenY(1500), 1e-3);
    }

    @Test
    public void screenAndDesktopCoordinatesRoundTrip() {
        v.centreOn(1234, 987, 1.5f);
        assertEquals(1234f, v.toDesktopX(v.toScreenX(1234)), 1e-2);
        assertEquals(987f, v.toDesktopY(v.toScreenY(987)), 1e-2);
    }

    @Test
    public void centringStoresTheViewportCentreAsTheNewFocus() {
        v.centreOn(2000, 1500, 1.0f);
        assertEquals(2000f, v.focusX(), 1e-3);
        assertEquals(1500f, v.focusY(), 1e-3);

        // Clamped: the focus is the centre of the view, not the requested point.
        v.centreOn(0, 0, 1.0f);
        assertEquals(VIEW_W / 2f, v.focusX(), 1e-3);
        assertEquals(VIEW_H / 2f, v.focusY(), 1e-3);
    }

    // ---- panning ----------------------------------------------------------

    @Test
    public void aPanMovesThePictureByTheScreenDeltaWhateverTheScale() {
        v.centreOn(2000, 1500, 2.0f);
        final float ox = v.originX(), oy = v.originY();
        v.panBy(30, -20);
        assertEquals(ox + 30, v.originX(), 1e-3);
        assertEquals(oy - 20, v.originY(), 1e-3);
        assertEquals("which at 2x is half as many desktop pixels", 1985f, v.focusX(), 1e-3);
        assertEquals(1510f, v.focusY(), 1e-3);
    }

    @Test
    public void aPanCannotPushTheDesktopOffItsWindow() {
        v.centreOn(50, 50, 1.0f); // top left, where the origin is already pinned
        v.panBy(100, 100);
        assertEquals(0f, v.originX(), 1e-3);
        assertEquals(0f, v.originY(), 1e-3);

        v.centreOn(FB_W - 10, FB_H - 10, 1.0f);
        v.panBy(-100, -100);
        assertEquals(VIEW_W - FB_W, v.originX(), 1e-3);
        assertEquals(VIEW_H - FB_H, v.originY(), 1e-3);
    }

    /** At fit there is nothing off-screen to bring on, on either axis. */
    @Test
    public void anAxisWithNothingToPanDoesNotPan() {
        v.zoomToFit();
        final float ox = v.originX(), oy = v.originY();
        v.panBy(80, 80);
        assertEquals("the axis that exactly fills the window", ox, v.originX(), 1e-3);
        assertEquals("and the one the clamp centres", oy, v.originY(), 1e-3);
    }

    @Test
    public void aPanSurvivesTheNextReCentring() {
        v.centreOn(2000, 1500, 1.0f);
        v.panBy(40, 0);
        final float ox = v.originX();
        // Any of the things that recompute the origin: an inset change, an
        // overlay appearing, a desktop resize. A pan written to the origin
        // would be gone by here.
        v.setScale(v.getScale());
        assertEquals(ox, v.originX(), 1e-3);
    }

    // ---- pan margins -------------------------------------------------------

    @Test
    public void aPanMarginLetsTheDesktopSlidePastItsOwnEdges() {
        v.setPanMargins(20, 30, 40, 50);
        v.centreOn(50, 50, 1.0f); // top left, where the origin was pinned at 0
        assertEquals("blank to the left of the desktop", 20f, v.originX(), 1e-3);
        assertEquals(30f, v.originY(), 1e-3);

        v.centreOn(FB_W - 10, FB_H - 10, 1.0f);
        assertEquals(VIEW_W - FB_W - 40f, v.originX(), 1e-3);
        assertEquals(VIEW_H - FB_H - 50f, v.originY(), 1e-3);
    }

    /**
     * A desktop that does not fill its window is centred, margin or no margin:
     * every pixel of it is already on screen, so the margin would be blank in
     * exchange for nothing.
     */
    @Test
    public void anAxisWithBlankOnBothSidesAlreadyIgnoresItsMargin() {
        v.setPanMargins(20, 30, 40, 50);
        v.zoomToFit(); // fits the width exactly; the height has blank above and below
        v.centreOn(FB_W / 2f, FB_H / 2f);
        final float oy = v.originY();
        assertEquals(0f, v.originX(), 1e-3);
        v.panBy(80, 80);
        assertEquals("the axis that exactly fills the window still has an edge to bring in",
                20f, v.originX(), 1e-3);
        assertEquals("the axis with blank on both sides of it does not move",
                oy, v.originY(), 1e-3);
    }

    /** Insets shrink what is derived from the window; a margin does not. */
    @Test
    public void aMarginIsNotAnInset() {
        final float fit = v.minScale();
        final int ladder = v.zoomLadder().length;
        v.setPanMargins(100, 100, 100, 100);
        assertEquals(fit, v.minScale(), 1e-6);
        assertEquals(VIEW_W, v.contentWidth());
        assertEquals(VIEW_H, v.contentHeight());
        assertEquals(ladder, v.zoomLadder().length);
    }

    @Test
    public void aMarginCannotLeaveMoreBlankThanDesktop() {
        v.setPanMargins(10000, 10000, 10000, 10000);
        v.centreOn(0, 0, 1.0f);
        assertEquals("half the content, and no further", VIEW_W / 2f, v.originX(), 1e-3);
        assertEquals(VIEW_H / 2f, v.originY(), 1e-3);
    }

    @Test
    public void scaleLimits() {
        assertEquals(Math.min(VIEW_W / (float) FB_W, VIEW_H / (float) FB_H), v.minScale(), 1e-6);
        assertEquals("2 * floor(density)", 4.0f, v.maxScale(), 1e-6);
    }

    @Test
    public void snapScaleQuantisesAndSnapsToNiceWidths() {
        assertEquals("within 4% of 1:1 snaps to 1:1", 1.0f, v.snapScale(0.99f), 1e-6);
        assertEquals("and to 2:1", 2.0f, v.snapScale(1.98f), 1e-6);
        assertEquals("and to fit-width", VIEW_W / (float) FB_W, v.snapScale(0.245f), 1e-6);
        assertEquals("and to fit-height", VIEW_H / (float) FB_H, v.snapScale(0.27f), 1e-6);
        assertEquals("otherwise quantised to 1/128",
                (int) (0.5f * 128) / 128.0f, v.snapScale(0.5f), 1e-6);
    }

    /**
     * The candidates are tried in order against the <em>unsnapped</em> width,
     * so where the fit-width and fit-height bands overlap the last one tried
     * (fit-height) wins. That is what the original does too.
     */
    @Test
    public void whereTwoSnapCandidatesOverlapTheLastOneWins() {
        assertEquals(VIEW_H / (float) FB_H, v.snapScale(0.26f), 1e-6);
    }

    @Test
    public void snapScaleClampsToTheLimits() {
        assertEquals(v.maxScale(), v.snapScale(10f), 1e-6);
        assertEquals(v.minScale(), v.snapScale(0.01f), 1e-6);
    }

    @Test
    public void setScaleReCentresOnTheStoredFocus() {
        v.centreOn(2000, 1500, 1.0f);
        v.setScale(2.0f);
        assertEquals(2.0f, v.getScale(), 1e-6);
        assertEquals("the focus point stays put on screen",
                VIEW_W / 2f, v.toScreenX(2000), 1e-3);
    }

    /**
     * Rotation, as {@code PlaygroundView.onSizeChanged} performs it. The remote
     * desktop does not rotate with the phone, so the cursor keeps its desktop
     * position and only the window onto the desktop changes.
     */
    @Test
    public void rotatingKeepsTheCursorOnTheSameDesktopPoint() {
        v.centreOn(2000, 1500, 1.0f);
        v.setViewSize(VIEW_H, VIEW_W); // portrait
        v.centreOn(2000, 1500, v.snapScale(v.getScale()));
        assertEquals(1.0f, v.getScale(), 1e-6);
        assertEquals(VIEW_H / 2f, v.toScreenX(2000), 1e-3);
        assertEquals(VIEW_W / 2f, v.toScreenY(1500), 1e-3);
    }

    /**
     * The fit-the-desktop minimum moves with the aspect ratio, so a scale that
     * was legal in portrait can be below the minimum in landscape. Re-snapping
     * on resize is what stops the desktop coming back smaller than the window.
     */
    @Test
    public void rotatingReDerivesTheScaleFloor() {
        v.setViewSize(VIEW_H, VIEW_W); // portrait: fit is limited by width
        final float portraitMin = v.minScale();
        v.centreOn(2000, 1500, v.snapScale(portraitMin));
        assertEquals(portraitMin, v.getScale(), 1e-6);

        v.setViewSize(VIEW_W, VIEW_H); // back to landscape
        assertTrue("the old scale no longer fills the window",
                v.getScale() < v.minScale());
        v.centreOn(2000, 1500, v.snapScale(v.getScale()));
        assertEquals(v.minScale(), v.getScale(), 1e-6);
    }

    @Test
    public void withNoViewSizeNothingIsClamped() {
        final Viewport u = new Viewport(DENSITY);
        u.setDesktopSize(FB_W, FB_H);
        u.centreOn(100, 100, 2.0f);
        assertEquals(2.0f, u.getScale(), 1e-6);
        assertEquals(0f, u.originX(), 1e-6);
    }

    // ---- the zoom ladder ---------------------------------------------------

    @Test
    public void theLadderIsAscendingDistinctAndRunsFromFitToTwo() {
        final float[] l = v.zoomLadder();
        assertTrue("built once the window is known", l.length >= 4);
        assertEquals("the bottom rung is fit-the-desktop", v.minScale(), l[0], 1e-6);
        assertEquals("the top rung is 2:1", 2.0f, l[l.length - 1], 1e-6);
        for (int i = 1; i < l.length; i++) {
            assertTrue("strictly ascending at " + i, l[i] > l[i - 1]);
        }
        for (float s : l) {
            assertEquals("every rung is already snapped, so stepping lands on it",
                    s, v.snapScale(s), 1e-6);
        }
    }

    @Test
    public void theLadderIsTheGeometricSeriesPlusTheFixedRungs() {
        // 1.0 * 0.66^n down to fit, then 1.5, 2.0, fit and fill.
        assertArrayEquals(new float[]{
                0.25f,        // fit  = 1000/4000
                0.26666668f,  // fill =  800/3000
                0.28125f,     // 0.66^3 quantised to 1/128
                0.4296875f,   // 0.66^2
                0.65625f,     // 0.66
                1.0f, 1.5f, 2.0f,
        }, v.zoomLadder(), 1e-6f);
    }

    /**
     * A region worth fitting gets a rung of its own, and one whose scale the
     * ladder already stops on does not become a second rung on the same scale
     * — which is the ordinary case rather than the exotic one, since two
     * monitors side by side fill the window at exactly the fill scale.
     */
    @Test
    public void aFitSizeIsOneMoreRungAndACollidingOneIsNone() {
        final int before = v.zoomLadder().length;
        v.setFitSizes(FB_W / 2, FB_H / 2);
        final float[] l = v.zoomLadder();
        assertEquals(before + 1, l.length);
        assertEquals("a quarter of the desktop, fitted to the window",
                v.snapScale(Math.min(VIEW_W / (FB_W / 2f), VIEW_H / (FB_H / 2f))),
                l[4], 1e-6);

        v.setFitSizes(FB_W / 2, FB_H); // half of a two-monitor desktop: the fill scale
        assertEquals(before, v.zoomLadder().length);
        v.setFitSizes(FB_W, FB_H);
        assertEquals(before, v.zoomLadder().length);
        v.setFitSizes(0, 0, FB_W * 2, FB_H);
        assertEquals(before, v.zoomLadder().length);
    }

    @Test
    public void theZoomButtonsStepOneRungAtATime() {
        final float[] l = v.zoomLadder();
        v.setScale(l[0]);
        assertEquals(0, v.zoomIndex());
        assertTrue(!v.canZoomOut());
        assertTrue(v.canZoomIn());

        v.zoomIn();
        assertEquals(l[1], v.getScale(), 1e-6);
        assertEquals(1, v.zoomIndex());

        v.zoomOut();
        assertEquals(l[0], v.getScale(), 1e-6);

        v.zoomOut();
        assertEquals("at the bottom it is a no-op", l[0], v.getScale(), 1e-6);

        for (int i = 0; i < l.length; i++) {
            v.zoomIn();
        }
        assertEquals(l.length - 1, v.zoomIndex());
        assertEquals(2.0f, v.getScale(), 1e-6);
        assertTrue(!v.canZoomIn());
    }

    /**
     * A pinch leaves the scale between two rungs. The next button press must
     * step relative to where the scale actually is, which is why the index is
     * re-derived from the scale rather than only being incremented.
     */
    @Test
    public void aButtonAfterAPinchStepsFromTheNearestRung() {
        v.setScale(0.9f); // pinched: snaps to 0.8984, nearest rung is 1.0
        assertEquals(1.0f, v.zoomLadder()[v.zoomIndex()], 1e-6);
        v.zoomIn();
        assertEquals(1.5f, v.getScale(), 1e-6);

        v.setScale(0.9f);
        v.zoomOut();
        assertEquals(0.65625f, v.getScale(), 1e-6);
    }

    @Test
    public void zoomToFitAndZoomToFill() {
        v.setScale(2.0f);
        v.zoomToFit();
        assertEquals(v.minScale(), v.getScale(), 1e-6);
        v.zoomToFill();
        assertEquals("fill overflows the other axis", v.fillScale(), v.getScale(), 1e-6);
        assertTrue(v.fillScale() > v.minScale());
    }

    /**
     * {@code ScaleManager.u} caps fill at {@code floor(density)} so a desktop
     * far narrower than the window cannot jump to an enormous scale.
     */
    @Test
    public void zoomToFillIsCappedAtHalfTheMaximum() {
        v.setDesktopSize(100, 3000);
        assertTrue(v.fillScale() > Math.floor(DENSITY));
        v.zoomToFill();
        assertEquals((float) Math.floor(DENSITY), v.getScale(), 1e-6);
    }

    @Test
    public void resizingRebuildsTheLadderAndKeepsTheIndexOnTheCurrentScale() {
        v.setScale(1.0f);
        final int before = v.zoomIndex();
        v.setViewSize(VIEW_H, VIEW_W); // portrait
        assertEquals("the bottom rung follows the new fit scale",
                v.minScale(), v.zoomLadder()[0], 1e-6);
        assertEquals("and 1.0 is still 1.0", 1.0f, v.zoomLadder()[v.zoomIndex()], 1e-6);
        assertTrue(before >= 0);
    }

    // ---- layout insets -----------------------------------------------------

    @Test
    public void insetsShrinkTheWindowAndMoveItsCentre() {
        v.setInsets(100, 50, 20, 30);
        assertEquals(VIEW_W - 120, v.contentWidth());
        assertEquals(VIEW_H - 80, v.contentHeight());
        assertEquals(100 + (VIEW_W - 120) / 2f, v.centreScreenX(), 1e-3);
        assertEquals(50 + (VIEW_H - 80) / 2f, v.centreScreenY(), 1e-3);
        assertEquals("fit is against the content rect",
                Math.min((VIEW_W - 120) / (float) FB_W, (VIEW_H - 80) / (float) FB_H),
                v.minScale(), 1e-6);
    }

    @Test
    public void theFocusPointSitsAtTheCentreOfTheContentRect() {
        v.setInsets(100, 50, 20, 30);
        v.centreOn(2000, 1500, 1.0f);
        assertEquals(v.centreScreenX(), v.toScreenX(2000), 1e-3);
        assertEquals(v.centreScreenY(), v.toScreenY(1500), 1e-3);
    }

    @Test
    public void theClampStopsAtTheInsetEdgesNotTheViewEdges() {
        v.setInsets(100, 50, 20, 30);
        v.centreOn(0, 0, 1.0f);
        assertEquals("desktop origin cannot come inside the left inset",
                100f, v.originX(), 1e-3);
        assertEquals(50f, v.originY(), 1e-3);

        v.centreOn(FB_W, FB_H, 1.0f);
        assertEquals("nor can its far edge come inside the right inset",
                100 + v.contentWidth() - FB_W, v.originX(), 1e-3);
        assertEquals(50 + v.contentHeight() - FB_H, v.originY(), 1e-3);
    }

    /**
     * A desktop smaller than the window is centred in the <em>view</em> and then
     * pushed inside the content rect — not centred in the content rect, which is
     * what the original does. On this axis there is no clamp to hold the picture
     * still, so centring in the content rect moves the whole desktop by half of
     * every inset that appears, which is the mouse overlay jumping the desktop.
     */
    @Test
    public void aDesktopSmallerThanTheWindowIsCentredInTheViewAndPushedInside() {
        v.setDesktopSize(200, 100);
        v.setInsets(0, 0, 0, 0);
        v.centreOn(100, 50, 1.0f);
        final float ox = v.originX(), oy = v.originY();
        assertEquals((VIEW_W - 200) / 2f, ox, 1e-3);
        assertEquals((VIEW_H - 100) / 2f, oy, 1e-3);

        // An overlay with room to spare under the picture: nothing moves.
        v.setInsets(0, 0, 60, 120);
        v.centreOn(100, 50, 1.0f);
        assertEquals(ox, v.originX(), 1e-3);
        assertEquals(oy, v.originY(), 1e-3);

        // And one that leaves it nowhere else to be: as far as it must, no
        // further, and never under the inset.
        v.setInsets(100, 50, 20, VIEW_H - 200);
        v.centreOn(100, 50, 1.0f);
        assertEquals("pushed up until its bottom edge is the content's",
                50 + v.contentHeight() - 100f, v.originY(), 1e-3);
        assertEquals("and still where it was across the view", ox, v.originX(), 1e-3);

        // Back again when it goes.
        v.setInsets(0, 0, 0, 0);
        v.centreOn(100, 50, 1.0f);
        assertEquals(oy, v.originY(), 1e-3);
    }

    @Test
    public void zeroInsetsAreExactlyTheOldBehaviour() {
        v.setInsets(0, 0, 0, 0);
        assertEquals(VIEW_W, v.contentWidth());
        assertEquals(VIEW_W / 2f, v.centreScreenX(), 1e-6);
        v.centreOn(2000, 1500, 1.0f);
        assertEquals(-1500f, v.originX(), 1e-3);
        assertEquals(-1100f, v.originY(), 1e-3);
    }
}
