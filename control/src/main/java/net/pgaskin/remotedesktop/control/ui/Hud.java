// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.ui;

import android.graphics.Canvas;
import android.graphics.Paint;
import android.graphics.RectF;

import net.pgaskin.remotedesktop.control.input.Config;

/**
 * The debug readout: a translucent box of monospaced-ish lines in the bottom
 * left, and the only reason it is a class rather than eight lines in a view.
 *
 * <p>What is shared is the <em>box</em>, not the content. Every screen that
 * drives the control stack wants to see what the stack is doing, and getting
 * that on screen means the same four fiddly things each time: shrink the text
 * until the widest line fits a portrait phone, sit clear of whichever overlay is
 * along the bottom, and draw a panel behind it so white text stays legible over
 * a white window. The lines themselves are the screen's business — the
 * playground's talk about a fake desktop, a session's about a connection.
 *
 * <p>Same rule as the rest of {@code control.ui}: {@code android.graphics} only.
 */
public final class Hud {

    /** Nominal text size; the fit may shrink it as far as {@link #MIN_DP}. */
    private static final float SIZE_DP = 11f;
    private static final float MIN_DP = 6f;

    /**
     * A per-second rate off a monotonically increasing counter, averaged over a
     * window so it does not flicker. Every HUD wants one of these — events a
     * second, frames a second, rectangles a second — and none of them wants to
     * keep three fields to get it.
     */
    public static final class Rate {

        private static final long WINDOW_NANOS = 500_000_000L;

        private long windowStart;
        private long windowBase;
        private int rate;

        /** Call once a frame with the running total; returns the current rate. */
        public int sample(long total, long nowNanos) {
            if (windowStart == 0) {
                windowStart = nowNanos;
                windowBase = total;
            } else if (nowNanos - windowStart >= WINDOW_NANOS) {
                rate = (int) ((total - windowBase) * 1_000_000_000L / (nowNanos - windowStart));
                windowStart = nowNanos;
                windowBase = total;
            }
            return rate;
        }
    }

    private final Config cfg;
    private final Paint text = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint panel = new Paint();
    private final RectF box = new RectF();

    public Hud(Config cfg) {
        this.cfg = cfg;
        text.setColor(0xffe8f0ff);
        panel.setColor(0xb0101418);
    }

    /**
     * @param bottomInsetPx what already occupies the bottom of the view — an
     *                      overlay, a keyboard, the IME — so the box sits above
     *                      it rather than behind it
     */
    public void draw(Canvas c, String[] lines, int viewW, int viewH, float bottomInsetPx) {
        if (lines.length == 0) {
            return;
        }
        final float pad = cfg.dp(6);

        // The lines are written for a landscape screen; in portrait they are
        // wider than the display, so shrink until the widest one fits.
        text.setTextSize(cfg.dp(SIZE_DP));
        float wMax = widest(lines);
        final float avail = viewW - pad * 4;
        if (wMax > avail && avail > 0) {
            text.setTextSize(Math.max(cfg.dp(SIZE_DP) * avail / wMax, cfg.dp(MIN_DP)));
            wMax = widest(lines);
        }

        final float lh = text.getTextSize() * 1.45f;
        final float boxH = pad * 2 + lh * lines.length;
        final float bottom = viewH - pad - bottomInsetPx;
        box.set(pad, bottom - boxH, pad * 3 + wMax, bottom);
        c.drawRoundRect(box, cfg.dp(4), cfg.dp(4), panel);

        float y = box.top + pad + text.getTextSize();
        for (String l : lines) {
            c.drawText(l, pad * 2, y, text);
            y += lh;
        }
    }

    private float widest(String[] lines) {
        float w = 0;
        for (String l : lines) {
            w = Math.max(w, text.measureText(l));
        }
        return w;
    }
}
