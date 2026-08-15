// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.graphics.Rect;

import net.pgaskin.remotedesktop.backend.Backend;

import java.util.Arrays;

/**
 * The desktop as last seen, held as a grid of tiles.
 *
 * <p>It replaces one desktop-sized bitmap, for a measured reason: a bitmap whose
 * pixels have changed gets a new generation id and hwui re-uploads <em>all</em>
 * of it, 0.96 ms of render thread per megapixel on this device, on every frame
 * with any damage at all however small. That put the size of the desktop into
 * the cost of a frame — 26% janky frames at 1080&times;2400 against 85% at
 * 3840&times;2400 for the same workload. Two more things fall out of the same
 * grid, which is why it is one mechanism rather than three:
 *
 * <ul>
 *   <li><b>Damage off screen is not fetched.</b> A tile outside the viewport
 *       stays dirty and is read when it comes back into view.
 *   <li><b>Damage rectangles stop being the unit of work.</b> They only mark
 *       tiles, so a repaint arriving as 34 ZRLE strips costs at most one read
 *       per tile rather than 34.
 * </ul>
 *
 * <p>Each dirty tile is read once a frame, and what is read is the union of the
 * damage that landed in it — clipped to the tile, so what changed next door
 * cannot enlarge this one's read — landing at its own place inside the tile,
 * because {@link Backend#readRegion} takes a destination offset and leaves the
 * rest of the bitmap alone. The exact union is therefore both the smallest read
 * and the cheapest one, with nothing to trade off.
 *
 * <p>{@link #damaged} arrives on the protocol's thread and does nothing but set
 * bits. Everything else is the drawing thread's. A tile's dirty bit is cleared
 * <em>before</em> it is read, so damage arriving during a read leaves it dirty
 * and it is read again next frame — over-fetching rather than missing a change.
 */
final class Mirror {

    /**
     * Roughly this big, in desktop pixels, unless the session says otherwise —
     * roughly, because the grid is then made to divide the desktop exactly
     * ({@link #tileW}).
     *
     * <p>The whole trade is in this number: bigger tiles upload more than
     * changed and draw in fewer calls, smaller ones upload less and cost a
     * longer per-frame loop — the loop that costs the original 10.5 ms a frame
     * at 128, with ~180 tiles on a phone screen. A sweep over a small-change
     * workload and a full-screen-repaint one settled on 512: at 128 the render
     * thread costs 39.2% of a core where 512 costs 25.4%.
     */
    static final int DEFAULT_TILE = 512;

    /**
     * Desktop pixels of the neighbouring tile kept around each tile's own, so
     * that filtering at a tile's edge has something true to sample.
     *
     * <p>Without it the picture grows a faint grid. Scaled, a tile boundary that
     * lands between two device pixels has to blend the last row of one tile with
     * the first row of the next — and a bitmap cannot be sampled past its own
     * edge, so each side clamps to itself and the boundary row comes out
     * unblended, measured at up to 127 levels of difference on a one-pixel line
     * every {@code tile × scale} pixels. One pixel is all bilinear sampling ever
     * reaches for, and it costs 0.8% of the grid at 512.
     */
    private static final int APRON = 1;

    private final int desktopW, desktopH;

    /**
     * A tile's size, chosen so that the grid covers the desktop and no more.
     *
     * <p>Whole tiles of the requested size would leave the right column and the
     * bottom row mostly padding — 3&times;5 tiles of 512 over a 1080&times;2400
     * desktop is 3.96 MP of texture for 2.59 MP of picture, and hwui uploads
     * whatever the bitmap is rather than whatever of it means anything, which
     * measured as the difference between beating the old whole-bitmap mirror on
     * a full-screen repaint and losing to it. Dividing instead of truncating —
     * three columns of 360 rather than two of 512 and one of 56 — makes the
     * padding nothing and keeps every tile the <em>same</em> size, which is what
     * lets the RealVNC backend allocate its native scale buffer once.
     */
    private final int tileW, tileH;
    private final int cols, rows;

    private final Bitmap[] tiles;
    private final boolean[] ready; // ever read? nothing is drawn from a tile that has not been

    /** Guards {@link #dirty}, {@link #owed} and {@link #damageRects} only. */
    private final Object lock = new Object();
    private final boolean[] dirty;
    /**
     * What of each dirty tile changed, as {@code x0, y0, x1, y1} in desktop
     * coordinates — the union of the damage that landed in it.
     *
     * <p>A tile could be read whole, as the original reads its 128×128 ones.
     * Measured, that amplification is free when damage is large and expensive
     * when it is small and frequent: a remote cursor moving at 60 Hz damages
     * some tens of pixels and was costing a 480×480 read every frame, doubling
     * the main thread's share during a pan.
     */
    private final int[] owed;
    private long damageRects;

    private int c0, r0, c1, r1; // the visible tile range, as of the last update()

    private final Rect src = new Rect();
    private final Rect dst = new Rect();

    private int allocated;
    private int lastRead;

    Mirror(int desktopW, int desktopH, int tileSize) {
        this.desktopW = desktopW;
        this.desktopH = desktopH;
        cols = Math.max(1, (desktopW + tileSize - 1) / tileSize);
        rows = Math.max(1, (desktopH + tileSize - 1) / tileSize);
        tileW = (desktopW + cols - 1) / cols;
        tileH = (desktopH + rows - 1) / rows;
        final int n = Math.max(1, cols * rows);
        tiles = new Bitmap[n];
        ready = new boolean[n];
        dirty = new boolean[n];
        owed = new int[4 * n];
        // Nothing has been read, so all of everything is owed: the session's own
        // damage-everything on the way in is then a belt rather than braces.
        Arrays.fill(dirty, true);
        for (int r = 0; r < rows; r++) {
            for (int c = 0; c < cols; c++) {
                setOwed(r * cols + c, left(c), top(r), right(c), bottom(r));
            }
        }
    }

    // A tile's extent in desktop coordinates, apron included: what its bitmap
    // holds, what damage is clipped to, and what a whole-tile read reads.
    private int left(int c) {
        return Math.max(0, c * tileW - APRON);
    }

    private int top(int r) {
        return Math.max(0, r * tileH - APRON);
    }

    private int right(int c) {
        return Math.min(desktopW, (c + 1) * tileW + APRON);
    }

    private int bottom(int r) {
        return Math.min(desktopH, (r + 1) * tileH + APRON);
    }

    int desktopWidth() {
        return desktopW;
    }

    int desktopHeight() {
        return desktopH;
    }

    int tileWidth() {
        return tileW;
    }

    int tileHeight() {
        return tileH;
    }

    /** Any thread. This rectangle of the desktop changed. */
    void damaged(int x, int y, int w, int h) {
        // Grown by the apron first, because a tile's copy of its neighbour's edge
        // pixel is as stale as the pixel it copies.
        final int x0 = Math.max(0, x - APRON), y0 = Math.max(0, y - APRON);
        final int x1 = Math.min(desktopW, x + w + APRON);
        final int y1 = Math.min(desktopH, y + h + APRON);
        if (x1 <= x0 || y1 <= y0) {
            return;
        }
        final int fc = clamp(x0 / tileW, 0, cols - 1), lc = clamp((x1 - 1) / tileW, 0, cols - 1);
        final int fr = clamp(y0 / tileH, 0, rows - 1), lr = clamp((y1 - 1) / tileH, 0, rows - 1);
        synchronized (lock) {
            damageRects++;
            for (int r = fr; r <= lr; r++) {
                final int base = r * cols;
                for (int c = fc; c <= lc; c++) {
                    final int i = base + c;
                    // Clipped to the tile, so the union of two rectangles in
                    // different tiles cannot grow either of them.
                    final int tx0 = Math.max(x0, left(c)), ty0 = Math.max(y0, top(r));
                    final int tx1 = Math.min(x1, right(c)), ty1 = Math.min(y1, bottom(r));
                    if (tx1 <= tx0 || ty1 <= ty0) {
                        continue;
                    }
                    if (dirty[i]) {
                        setOwed(i, Math.min(owed[4 * i], tx0), Math.min(owed[4 * i + 1], ty0),
                                Math.max(owed[4 * i + 2], tx1), Math.max(owed[4 * i + 3], ty1));
                    } else {
                        dirty[i] = true;
                        setOwed(i, tx0, ty0, tx1, ty1);
                    }
                }
            }
        }
    }

    private void setOwed(int i, int x0, int y0, int x1, int y1) {
        owed[4 * i] = x0;
        owed[4 * i + 1] = y0;
        owed[4 * i + 2] = x1;
        owed[4 * i + 3] = y1;
    }

    /**
     * Read whatever is dirty and visible. {@code vx0..vy1} is the part of the
     * desktop the viewport can see, in desktop pixels; anything outside it
     * keeps its damage until it is looked at.
     */
    void update(Backend backend, float vx0, float vy0, float vx1, float vy1) {
        c0 = clamp((int) Math.floor(vx0 / tileW), 0, cols - 1);
        r0 = clamp((int) Math.floor(vy0 / tileH), 0, rows - 1);
        c1 = clamp((int) Math.ceil(vx1 / tileW) - 1, c0, cols - 1);
        r1 = clamp((int) Math.ceil(vy1 / tileH) - 1, r0, rows - 1);
        lastRead = 0;
        for (int r = r0; r <= r1; r++) {
            for (int c = c0; c <= c1; c++) {
                final int i = r * cols + c;
                final int dx0, dy0, dx1, dy1;
                synchronized (lock) {
                    if (!dirty[i]) {
                        continue;
                    }
                    dx0 = owed[4 * i];
                    dy0 = owed[4 * i + 1];
                    dx1 = owed[4 * i + 2];
                    dy1 = owed[4 * i + 3];
                    // Cleared before the read, not after: damage that arrives
                    // while we are reading has to survive us.
                    dirty[i] = false;
                }
                if (tiles[i] == null) {
                    tiles[i] = Bitmap.createBitmap(tileW + 2 * APRON, tileH + 2 * APRON,
                            Bitmap.Config.ARGB_8888);
                    allocated++;
                }
                // Exactly what changed, where it belongs inside the tile.
                final int tx0 = left(c), ty0 = top(r);
                final boolean ok = backend.readRegion(dx0, dy0, dx1 - dx0, dy1 - dy0,
                        tiles[i], dx0 - tx0, dy0 - ty0);
                if (ok) {
                    ready[i] = true;
                    lastRead++;
                } else {
                    // Nothing to read yet, or no room to read it into — owed
                    // again, and all of it, not lost.
                    synchronized (lock) {
                        if (dirty[i]) {
                            setOwed(i, Math.min(owed[4 * i], dx0), Math.min(owed[4 * i + 1], dy0),
                                    Math.max(owed[4 * i + 2], dx1),
                                    Math.max(owed[4 * i + 3], dy1));
                        } else {
                            dirty[i] = true;
                            setOwed(i, dx0, dy0, dx1, dy1);
                        }
                    }
                }
            }
        }
    }

    /**
     * Draw the visible tiles. The canvas is already in desktop coordinates —
     * translated to the viewport origin and scaled — so a tile goes at its own
     * position in the desktop.
     */
    void draw(Canvas c, Paint paint) {
        for (int r = r0; r <= r1; r++) {
            for (int col = c0; col <= c1; col++) {
                final int i = r * cols + col;
                if (!ready[i]) {
                    continue;
                }
                final int x = col * tileW, y = r * tileH;
                final int w = Math.min(tileW, desktopW - x);
                final int h = Math.min(tileH, desktopH - y);
                // Where this tile's own pixels start inside its bitmap: after
                // whatever apron there was room for in front of them.
                final int ax = x - left(col), ay = y - top(r);
                src.set(ax, ay, ax + w, ay + h);
                dst.set(x, y, x + w, y + h);
                // The apron is outside src and is meant to be sampled from all
                // the same: hwui draws this with Skia's "fast" source
                // constraint, which allows filtering to reach past the source
                // rectangle rather than clamping to it.
                c.drawBitmap(tiles[i], src, dst, paint);
            }
        }
    }

    // ---- what the HUD reads ------------------------------------------------

    long damageRects() {
        synchronized (lock) {
            return damageRects;
        }
    }

    int dirtyCount() {
        int n = 0;
        synchronized (lock) {
            for (boolean d : dirty) {
                if (d) {
                    n++;
                }
            }
        }
        return n;
    }

    /** Tiles read on the last frame. */
    int lastRead() {
        return lastRead;
    }

    int tileCount() {
        return cols * rows;
    }

    int allocatedTiles() {
        return allocated;
    }

    /** Tiles in the visible range, which is what a frame's cost is set by. */
    int visibleTiles() {
        return (r1 - r0 + 1) * (c1 - c0 + 1);
    }

    void release() {
        for (int i = 0; i < tiles.length; i++) {
            if (tiles[i] != null) {
                tiles[i].recycle();
                tiles[i] = null;
            }
            ready[i] = false;
        }
        allocated = 0;
    }

    private static int clamp(int v, int lo, int hi) {
        return v < lo ? lo : Math.min(v, hi);
    }
}
