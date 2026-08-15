// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.input;

/**
 * Where {@link TapRegions} hits go. The gesture layer has no idea what a region
 * does — it only knows a tap landed in one and whether anybody wanted it.
 */
public interface RegionSink {
    /**
     * A single-finger tap landed in {@code region}, at screen position
     * {@code (x, y)}.
     *
     * @return {@code true} if the tap was consumed, in which case no mouse
     *         button is clicked; {@code false} to let it click normally.
     */
    boolean regionTapped(TapRegions.Region region, float x, float y);
}
