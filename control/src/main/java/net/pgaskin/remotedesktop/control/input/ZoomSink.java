// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.input;

/**
 * The pinch-zoom half of the gesture layer's output: the original's four
 * callbacks, plus the travel it throws away.
 */
public interface ZoomSink {
    /** A pinch has been recognised; capture the current scale as the base. */
    void zoomBegan();

    /** {@code factor} is relative to the finger separation at gesture start. */
    void zoomChanged(float factor);

    /**
     * How far the two-finger midpoint has travelled since the last call, in
     * screen pixels, while the pinch is engaged — a pinch that also pans.
     *
     * <p>Whether it pans anything is the host's: a viewport that follows a
     * cursor of its own is already where it should be, and only a far end that
     * owns the cursor leaves a zoomed-in desktop with no other way to be looked
     * around. {@link net.pgaskin.remotedesktop.control.Viewport#panBy} is what
     * a host that does pan calls.
     */
    void zoomPanned(float screenDx, float screenDy);

    /** The pinch ended (or turned into something else); re-capture the scale. */
    void zoomEnded();

    /** Fix the scale focus at this screen point (the two-finger midpoint). */
    void scaleCentre(float screenX, float screenY);
}
