// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.input;

/**
 * Everything the gesture layer can do to a pointer. Deltas are in <em>screen</em>
 * pixels; it is up to the consumer to convert to remote-desktop coordinates.
 *
 * <p>The same four calls the original's gesture layer makes: move, press,
 * release, and cancel an auto-scroll.
 */
public interface MouseSink {
    /** Relative pointer motion, in screen px, already accelerated. */
    void mouseMove(float dx, float dy);

    /**
     * Press the buttons in {@code mask} (see {@link Button}).
     *
     * <p>A <em>mask</em>, not a {@link Button}, because the gesture layer is not
     * the only producer: a physical mouse reports its whole button state at once
     * and a chorded press arrives as one event, and the on-screen overlay can
     * hold more than one button down.
     */
    void mouseDown(int mask);

    /** Release the buttons in {@code mask}. */
    void mouseUp(int mask);

}
