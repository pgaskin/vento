// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend;

/**
 * One of the far end's monitors, as a rectangle of the desktop.
 *
 * <p>A multi-head desktop is one framebuffer with the heads laid out inside it,
 * and that is true of every protocol here — there is no per-monitor mode, no
 * chooser and no second connection. So this is not a thing the picture or the
 * pointer is built on: it is the far end describing where the joins are, for the
 * two places a join is worth knowing about, and everything else works in desktop
 * pixels and never asks.
 *
 * <p>Coordinates are the desktop's own, so the rectangles tile the framebuffer
 * and the first one is not necessarily at the origin.
 */
public record Monitor(int x, int y, int width, int height) {

    /**
     * The shape every native backend hands its layout over in: four ints per
     * monitor, x, y, width, height. One reader for all of them, because a
     * screen list is the same list whichever library read it off the wire, and
     * the empty and null cases have to be the same answer in every one.
     */
    public static java.util.List<Monitor> fromFlat(int[] flat) {
        if (flat == null || flat.length < 4) {
            return java.util.List.of();
        }
        final java.util.List<Monitor> monitors = new java.util.ArrayList<>(flat.length / 4);
        for (int i = 0; i + 3 < flat.length; i += 4) {
            if (flat[i + 2] > 0 && flat[i + 3] > 0) {
                monitors.add(new Monitor(flat[i], flat[i + 1], flat[i + 2], flat[i + 3]));
            }
        }
        return java.util.List.copyOf(monitors);
    }

    public int right() {
        return x + width;
    }

    public int bottom() {
        return y + height;
    }

    public boolean contains(int px, int py) {
        return px >= x && py >= y && px < right() && py < bottom();
    }
}
