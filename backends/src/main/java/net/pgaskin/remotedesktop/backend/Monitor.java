// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend;

/**
 * One of the far end's screens, as a rectangle in its coordinates.
 *
 * <p>Two questions use this shape and they are not the same question.
 * {@link Backend#monitors} is a multi-head desktop as every protocol here but
 * one serves it: <em>one</em> framebuffer with the heads laid out inside it, so
 * the rectangles tile the picture, the first is not necessarily at the origin,
 * and nothing about the pixels or the pointer is built on them — it is the far
 * end saying where the joins are, and everything else works in desktop pixels
 * and never asks. {@link Backend#displays} is RustDesk's, where the far end
 * sends one screen at a time: the rectangles are then a menu rather than a map,
 * since only one of them is the picture and the others are nowhere in it.
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
