// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.input;

import java.util.List;

/**
 * Rectangles of the touch surface that a <em>tap</em> activates instead of
 * clicking, expressed as fractions of the view so they survive rotation and
 * say nothing about pixels.
 *
 * <p>This replaces the original's floating toolbar, which is large, in the way,
 * and has to be dismissed. The layout in {@link #toolbar()} is vncpatch's, but
 * the mechanism is not: vncpatch puts real {@code View}s over the desktop, so
 * <em>any</em> touch in a band is swallowed — including a drag that merely
 * starts near an edge, and including the bump scroll that deliberately lives in
 * those same bands. Here the regions hang off {@link GestureRecognizer}'s tap
 * path, so only a gesture already classified as a tap can trigger one.
 *
 * <p>Two rules follow from that placement. <b>One finger only</b>: a two- or
 * three-finger tap is a right or middle click everywhere on the screen, bands
 * included. And <b>the handler decides</b> — {@link RegionSink#regionTapped}
 * returns whether it consumed the tap, so a region that is inactive right now
 * needs no set-swapping.
 *
 * <p>Names are the caller's: which one means what is the app's business.
 */
public final class TapRegions {

    /**
     * One rectangle, in fractions of the view: {@code (0,0)} is the top-left
     * corner and {@code (1,1)} the bottom-right. Bounds are inclusive, and
     * {@link TapRegions#hit} returns the first match, so regions that share an
     * edge resolve to whichever was declared first.
     */
    public record Region(String name, float left, float top, float right, float bottom) {
        public boolean contains(float fx, float fy) {
            return fx >= left && fx <= right && fy >= top && fy <= bottom;
        }
    }

    /** Names of the {@link #toolbar()} regions. */
    public static final String DISCONNECT = "disconnect";
    public static final String INFORMATION = "information";
    public static final String KEYBOARD = "keyboard";
    public static final String MOUSE = "mouse";

    private final List<Region> regions;

    public TapRegions(Region... regions) {
        this.regions = List.of(regions);
    }

    public List<Region> regions() {
        return regions;
    }

    /**
     * The region containing screen position {@code (x, y)} in a {@code viewW ×
     * viewH} view, or {@code null} — including when the view has no size yet.
     */
    public Region hit(float x, float y, int viewW, int viewH) {
        if (viewW <= 0 || viewH <= 0) {
            return null;
        }
        final float fx = x / viewW, fy = y / viewH;
        for (Region r : regions) {
            if (r.contains(fx, fy)) {
                return r;
            }
        }
        return null;
    }

    /**
     * vncpatch's replacement for the floating toolbar. The bottom-right corner
     * is where the mouse overlay's own dismiss button sits, so that region
     * toggles it.
     *
     * <pre>
     *   ┌────────────┬──────────────────────────┐
     *   │ disconnect │       information        │  2/22
     *   ├────────────┴──────────────────────────┤
     *   │                                       │
     *   │              (no region)              │  17/22
     *   │                                       │
     *   ├───────────────────────────────┬───────┤
     *   │           keyboard            │ mouse │  3/22
     *   └───────────────────────────────┴───────┘
     *          4/5                         1/5
     * </pre>
     */
    public static TapRegions toolbar() {
        final float top = 2f / 22f;
        final float bottom = 19f / 22f;
        return new TapRegions(
                new Region(DISCONNECT, 0f, 0f, 1f / 5f, top),
                new Region(INFORMATION, 1f / 5f, 0f, 1f, top),
                new Region(KEYBOARD, 0f, bottom, 4f / 5f, 1f),
                new Region(MOUSE, 4f / 5f, bottom, 1f, 1f));
    }
}
