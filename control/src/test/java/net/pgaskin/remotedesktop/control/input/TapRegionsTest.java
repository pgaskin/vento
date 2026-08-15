// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.input;

import static java.util.Arrays.asList;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import net.pgaskin.remotedesktop.control.harness.Harness;

import org.junit.Test;

/**
 * The toolbar tap regions: the geometry, and the rule that only a <em>tap</em>
 * activates one.
 *
 * <p>Positions are for the harness's 2400×1080 view, where the top band ends at
 * y = 98 (2/22), the bottom band starts at y = 933 (19/22), and the two vertical
 * splits are x = 480 (1/5) and x = 1920 (4/5).
 */
public class TapRegionsTest {

    private static final float[] DISCONNECT = {200, 50};
    private static final float[] INFORMATION = {1200, 50};
    private static final float[] KEYBOARD = {1000, 1000};
    private static final float[] MOUSE = {2200, 1000};
    private static final float[] MIDDLE = {1200, 540};

    private static Harness withToolbar() {
        return Harness.improved().withRegions(TapRegions.toolbar());
    }

    private static Harness tapAt(Harness h, float[] p) {
        return h.tap(p[0], p[1]);
    }

    // ---- geometry ---------------------------------------------------------

    @Test
    public void toolbarBandsAreWhereTheLayoutSaysTheyAre() {
        final TapRegions t = TapRegions.toolbar();
        assertEquals(TapRegions.DISCONNECT, t.hit(200, 50, 2400, 1080).name());
        assertEquals(TapRegions.INFORMATION, t.hit(1200, 50, 2400, 1080).name());
        assertEquals(TapRegions.KEYBOARD, t.hit(1000, 1000, 2400, 1080).name());
        assertEquals(TapRegions.MOUSE, t.hit(2200, 1000, 2400, 1080).name());

        // The 17/22 middle belongs to nobody — it is the whole point.
        assertNull(t.hit(1200, 540, 2400, 1080));
        assertNull(t.hit(1200, 99, 2400, 1080));
        assertNull(t.hit(1200, 932, 2400, 1080));

        // The vertical splits: 1:4 across the top, 4:1 across the bottom.
        assertEquals(TapRegions.DISCONNECT, t.hit(479, 50, 2400, 1080).name());
        assertEquals(TapRegions.INFORMATION, t.hit(481, 50, 2400, 1080).name());
        assertEquals(TapRegions.KEYBOARD, t.hit(1919, 1000, 2400, 1080).name());
        assertEquals(TapRegions.MOUSE, t.hit(1921, 1000, 2400, 1080).name());
    }

    @Test
    public void theCornersOfTheViewAreInsideARegion() {
        final TapRegions t = TapRegions.toolbar();
        assertEquals(TapRegions.DISCONNECT, t.hit(0, 0, 2400, 1080).name());
        assertEquals(TapRegions.INFORMATION, t.hit(2400, 0, 2400, 1080).name());
        assertEquals(TapRegions.KEYBOARD, t.hit(0, 1080, 2400, 1080).name());
        assertEquals(TapRegions.MOUSE, t.hit(2400, 1080, 2400, 1080).name());
    }

    /** Fractions, so a rotation moves the bands with the view rather than off it. */
    @Test
    public void regionsScaleWithTheView() {
        final TapRegions t = TapRegions.toolbar();
        assertEquals(TapRegions.MOUSE, t.hit(2200, 1000, 2400, 1080).name());
        // The same physical corner of a portrait view.
        assertEquals(TapRegions.MOUSE, t.hit(990, 2220, 1080, 2400).name());
        // ... and that position is nowhere near the bands of the landscape one.
        assertNull(t.hit(990, 2220, 2400, 1080));
    }

    @Test
    public void aViewWithNoSizeYetHitsNothing() {
        assertNull(TapRegions.toolbar().hit(0, 0, 0, 0));
    }

    // ---- what activates one -----------------------------------------------

    @Test
    public void regionsAreOffUntilTheyAreWiredUp() {
        final Harness h = Harness.improved();
        tapAt(h, DISCONNECT);
        assertEquals(asList("down LEFT", "up LEFT"), h.buttonEvents());
        assertEquals(asList(), h.regionTaps);
    }

    @Test
    public void aTapInEachRegionFiresItAndClicksNothing() {
        final Harness h = withToolbar();
        tapAt(h, DISCONNECT);
        tapAt(h, INFORMATION);
        tapAt(h, KEYBOARD);
        tapAt(h, MOUSE);
        assertEquals(asList(
                        "region disconnect 200,50",
                        "region information 1200,50",
                        "region keyboard 1000,1000",
                        "region mouse 2200,1000"),
                h.regionTaps);
        assertEquals("a consumed region tap must not also click",
                asList(), h.buttonEvents());
    }

    @Test
    public void aTapOutsideEveryRegionIsAnOrdinaryClick() {
        final Harness h = withToolbar();
        tapAt(h, MIDDLE);
        assertEquals(asList("down LEFT", "up LEFT"), h.buttonEvents());
        assertEquals(asList(), h.regionTaps);
    }

    /**
     * The reason this hangs off the tap path rather than being a view on top of
     * the desktop, as vncpatch's is: a drag that merely <em>starts</em> in a band
     * has to reach the cursor. vncpatch's region views swallow it.
     */
    @Test
    public void aDragStartingInARegionMovesTheCursorAndFiresNothing() {
        final Harness h = withToolbar();
        h.drag(0, DISCONNECT[0], DISCONNECT[1], 0, 400, 10).advance(300);
        assertEquals(asList(), h.regionTaps);
        assertEquals(asList(), h.buttonEvents());
        assertTrue("the drag has to reach the cursor",
                Harness.count(h.mouse, "move ") > 0);
    }

    /** Bump scroll lives in the same bands, and is a drag, so it is unaffected. */
    @Test
    public void bumpScrollStillRunsInTheBottomBand() {
        final Harness h = withToolbar();
        h.down(0, MIDDLE[0], MIDDLE[1]).up(0).advance(100);
        h.reset();

        // Touching down again inside the click-hold window cancels the pending
        // release and arms bump scroll; the drag then pushes into the bottom
        // 24 dp (y > 1017), which is inside the keyboard region.
        h.down(0, MIDDLE[0], MIDDLE[1]);
        for (int i = 1; i <= 12; i++) {
            h.move(0, MIDDLE[0], MIDDLE[1] + (1040 - MIDDLE[1]) * i / 12);
        }
        assertEquals("a bump-scrolling drag is not a region tap", asList(), h.regionTaps);
        assertTrue("the edge step should have fired",
                Harness.count(h.mouse, "move 0.00,31.50") > 0);
        h.up(0).advance(300);
    }

    /**
     * Regions are a one-finger affordance. A two-finger tap is a right click
     * everywhere on the screen, including inside a band — otherwise the bands
     * would be dead zones for the other buttons.
     */
    @Test
    public void aTwoFingerTapInARegionIsStillARightClick() {
        final Harness h = withToolbar();
        h.down(0, 900, 1000).down(1, 1100, 1000).up(1).up(0).advance(300);
        assertEquals(asList("down RIGHT", "up RIGHT"), h.buttonEvents());
        assertEquals(asList(), h.regionTaps);
    }

    /** An unclaimed tap clicks, so a temporarily inactive region needs no swap. */
    @Test
    public void aRegionTapTheHandlerDeclinesStillClicks() {
        final Harness h = withToolbar();
        h.consumeRegionTaps = false;
        tapAt(h, KEYBOARD);
        assertEquals(asList("region keyboard 1000,1000"), h.regionTaps);
        assertEquals(asList("down LEFT", "up LEFT"), h.buttonEvents());
    }

    /**
     * The region is chosen by where the finger landed, not where it lifted: a
     * tap that drifts a few pixels out of the band is still that band's tap.
     */
    @Test
    public void theTapIsPlacedWhereTheFingerLanded() {
        final Harness h = withToolbar();
        h.down(0, 200, 90).move(0, 200, 110).up(0).advance(300);
        assertEquals(asList("region disconnect 200,90"), h.regionTaps);
        assertEquals(asList(), h.buttonEvents());
    }

    /** A held button from a previous tap is released before the region fires. */
    @Test
    public void aRegionTapInsideTheClickWindowReleasesTheHeldButton() {
        final Harness h = withToolbar();
        h.down(0, MIDDLE[0], MIDDLE[1]).up(0).advance(100);
        assertEquals(asList("down LEFT"), h.buttonEvents());

        tapAt(h, MOUSE);
        assertEquals("the pending release still has to happen",
                asList("down LEFT", "up LEFT"), h.buttonEvents());
        assertEquals(asList("region mouse 2200,1000"), h.regionTaps);
    }
}
