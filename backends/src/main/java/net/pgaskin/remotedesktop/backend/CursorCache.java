// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend;

import android.graphics.Bitmap;

import java.util.LinkedHashMap;
import java.util.Map;

/**
 * The remote cursor's shapes, kept rather than rebuilt.
 *
 * <p>What this saves is <em>not</em> a round trip. Every protocol here sends
 * the shape whenever it changes — RDP's own pointer cache is resolved inside
 * its library before we see it, and RFB has no cache at all — so a shape
 * arrives on the wire either way. What it saves is the {@code ARGB_8888} and
 * the {@code setPixels} behind each one, and then the texture the renderer
 * uploads for it: a pointer crossing a window's edge changes shape several
 * times a second, between two or three shapes it has already had.
 *
 * <p>The identity is a hash of the pixels, computed by whichever shim already
 * has them — the seam's four native backends all build the bitmap out of an
 * {@code int[]} they have just filled, so hashing there is a pass over data
 * that is already hot. The consequence is that this class trusts the hash: two
 * shapes that collide are one shape, which is a wrong cursor rather than a
 * crash, and at 64 bits over a few dozen live shapes it will not happen.
 *
 * <p><b>Bounded, which is the whole of the risk.</b> A far end whose cursor is
 * an animation would otherwise fill memory with shapes never seen twice, so
 * there is a count and a byte budget and the least recently used goes first.
 * The pathological case then costs exactly what having no cache costs.
 *
 * <p>Not thread-safe, and does not need to be: a backend's cursor callback
 * arrives on its own protocol thread and nothing else here touches this.
 */
public final class CursorCache {

    /** More shapes than a desktop has. Windows ships about a dozen. */
    private static final int MAX_SHAPES = 16;

    /** A 256×256 cursor is a quarter of this, and is already absurd. */
    private static final int MAX_BYTES = 1024 * 1024;

    private final Map<Long, Bitmap> shapes = new LinkedHashMap<>(8, 0.75f, true);
    private int bytes;

    /**
     * The shape for these pixels, from the cache where it has been seen before
     * and built and kept where it has not.
     *
     * @param hash   of the pixels, from wherever they were filled in
     * @param argb   {@code width * height} pixels; null or empty hides the
     *               cursor, which is not a shape and is not cached
     * @return the bitmap, or null for a hidden cursor. Owned by this cache and
     *         valid until it is evicted, so a caller must not recycle it
     */
    public Bitmap shape(long hash, int[] argb, int width, int height) {
        if (argb == null || width <= 0 || height <= 0 || argb.length < width * height) {
            return null;
        }
        final Bitmap cached = shapes.get(hash);
        if (cached != null) {
            return cached;
        }
        final Bitmap shape = Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888);
        shape.setPixels(argb, 0, width, 0, 0, width, height);
        shapes.put(hash, shape);
        bytes += width * height * 4;
        // Eviction after insertion rather than before, so a single shape larger
        // than the whole budget is still returned once. Nothing recycles what
        // it drops: the renderer may still be drawing the shape it was last
        // handed, and a bitmap nobody holds is collected anyway.
        final var it = shapes.entrySet().iterator();
        while (it.hasNext() && (shapes.size() > MAX_SHAPES || bytes > MAX_BYTES)
                && shapes.size() > 1) {
            final Bitmap evicted = it.next().getValue();
            bytes -= evicted.getWidth() * evicted.getHeight() * 4;
            it.remove();
        }
        return shape;
    }

    public void clear() {
        shapes.clear();
        bytes = 0;
    }

    /**
     * FNV-1a over the pixels, which is what every shim computes natively and
     * what this exists to define in one place. Used by a backend whose library
     * hands over pixels on the Java side.
     */
    public static long hash(int[] argb, int width, int height) {
        long h = 0xcbf29ce484222325L;
        h = (h ^ width) * 0x100000001b3L;
        h = (h ^ height) * 0x100000001b3L;
        for (int p : argb) {
            // Masked, because an opaque pixel is a negative int and sign
            // extension here would not match the shims, where it is a u32.
            h = (h ^ (p & 0xffffffffL)) * 0x100000001b3L;
        }
        return h;
    }
}
