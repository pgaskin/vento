// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.playground;

import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.LinearGradient;
import android.graphics.Paint;
import android.graphics.Path;
import android.graphics.RadialGradient;
import android.graphics.RectF;
import android.graphics.Shader;

/**
 * Procedurally generated stand-ins for the remote framebuffer and the remote
 * cursor. Generated rather than shipped as assets so the repo stays free of
 * binary blobs, and because a grid with coordinate labels is far more useful
 * than a photo when checking that screen↔desktop mapping is exact.
 */
public final class Art {

    /** A remote cursor shape: a bitmap plus the hotspot the server declared. */
    public record Cursor(Bitmap bitmap, int hotX, int hotY, String name) {
    }

    private Art() {
    }

    public static Bitmap wallpaper(int w, int h) {
        final Bitmap bmp = Bitmap.createBitmap(w, h, Bitmap.Config.ARGB_8888);
        final Canvas c = new Canvas(bmp);
        final Paint p = new Paint(Paint.ANTI_ALIAS_FLAG);

        p.setShader(new LinearGradient(0, 0, w, h,
                new int[]{0xff10243c, 0xff1d4f5e, 0xff2b6b52, 0xff4a3b6b},
                new float[]{0f, 0.38f, 0.66f, 1f}, Shader.TileMode.CLAMP));
        c.drawRect(0, 0, w, h, p);
        p.setShader(null);

        // A few soft blobs so it reads as a picture rather than graph paper.
        final int[] blobs = {0x66e0a33e, 0x5540c4a0, 0x554a90d9, 0x66d95f6b, 0x4477e0c0};
        for (int i = 0; i < blobs.length; i++) {
            final float bx = w * (0.12f + 0.19f * i);
            final float by = h * (i % 2 == 0 ? 0.28f : 0.72f);
            final float br = Math.min(w, h) * (0.16f + 0.05f * (i % 3));
            p.setShader(new RadialGradient(bx, by, br, blobs[i], 0x00000000, Shader.TileMode.CLAMP));
            c.drawCircle(bx, by, br, p);
        }
        p.setShader(null);

        // Grid: fine every 100 px, heavy every 500 px, labelled at intersections.
        p.setStyle(Paint.Style.STROKE);
        p.setStrokeWidth(1f);
        p.setColor(0x22ffffff);
        for (int x = 0; x <= w; x += 100) c.drawLine(x, 0, x, h, p);
        for (int y = 0; y <= h; y += 100) c.drawLine(0, y, w, y, p);
        p.setStrokeWidth(2f);
        p.setColor(0x55ffffff);
        for (int x = 0; x <= w; x += 500) c.drawLine(x, 0, x, h, p);
        for (int y = 0; y <= h; y += 500) c.drawLine(0, y, w, y, p);

        p.setStyle(Paint.Style.FILL);
        p.setColor(0x99ffffff);
        p.setTextSize(22f);
        for (int x = 0; x <= w; x += 500) {
            for (int y = 0; y <= h; y += 500) {
                c.drawText(x + "," + y, x + 6, y + 24, p);
            }
        }

        // Border so the desktop edge is unmistakable.
        p.setStyle(Paint.Style.STROKE);
        p.setStrokeWidth(8f);
        p.setColor(0xffff5555);
        c.drawRect(4, 4, w - 4, h - 4, p);

        return bmp;
    }

    /** Classic arrow, hotspot at the tip. */
    public static Cursor arrowCursor() {
        final int w = 14, h = 22;
        final Bitmap bmp = Bitmap.createBitmap(w, h, Bitmap.Config.ARGB_8888);
        final Canvas c = new Canvas(bmp);
        final Path path = new Path();
        path.moveTo(0.5f, 0.5f);
        path.lineTo(0.5f, 16.5f);
        path.lineTo(4.5f, 12.5f);
        path.lineTo(7.0f, 19.5f);
        path.lineTo(10.0f, 18.0f);
        path.lineTo(7.5f, 11.5f);
        path.lineTo(12.5f, 11.5f);
        path.close();

        final Paint p = new Paint(Paint.ANTI_ALIAS_FLAG);
        p.setStyle(Paint.Style.FILL);
        p.setColor(Color.BLACK);
        c.drawPath(path, p);
        p.setStyle(Paint.Style.STROKE);
        p.setStrokeWidth(1f);
        p.setColor(Color.WHITE);
        c.drawPath(path, p);
        return new Cursor(bmp, 0, 0, "ARROW");
    }

    /** The link hand, hotspot at the fingertip — what a page changes to. */
    public static Cursor handCursor() {
        final int w = 22, h = 28;
        final Bitmap bmp = Bitmap.createBitmap(w, h, Bitmap.Config.ARGB_8888);
        final Canvas c = new Canvas(bmp);
        // Six rounded boxes unioned: the index finger, three curled ones, the
        // fist and a thumb bulging out to the left of it. Drawn to the same
        // proportions as the desktop pointer it stands in for, since a hand
        // that is not that shape reads as a blob at this size.
        final float[][] parts = {
                {6.5f, 2.5f, 10.5f, 16f, 2.0f},    // index
                {10.2f, 9f, 13.6f, 16f, 1.7f},
                {13.3f, 10.2f, 16.6f, 16f, 1.7f},
                {16.3f, 11.5f, 19.3f, 17f, 1.5f},  // little
                {4.8f, 14f, 19.3f, 25.5f, 4.6f},   // fist
                {1.8f, 16.5f, 7.0f, 23.0f, 2.6f},  // thumb
        };
        final Path path = new Path();
        for (float[] r : parts) {
            path.addRoundRect(new RectF(r[0], r[1], r[2], r[3]), r[4], r[4], Path.Direction.CW);
        }

        final Paint p = new Paint(Paint.ANTI_ALIAS_FLAG);
        // Outline first and fill over it, so the white is a halo outside the
        // hand rather than a line through every box that made it.
        p.setStyle(Paint.Style.STROKE);
        p.setStrokeWidth(1.6f);
        p.setStrokeJoin(Paint.Join.ROUND);
        p.setColor(Color.WHITE);
        c.drawPath(path, p);
        p.setStyle(Paint.Style.FILL);
        p.setColor(Color.BLACK);
        c.drawPath(path, p);

        // ... and then the gaps between the fingers, which the fill covered.
        p.setStyle(Paint.Style.STROKE);
        p.setStrokeWidth(0.9f);
        p.setColor(Color.WHITE);
        c.drawLine(10.35f, 10.2f, 10.35f, 15f, p);
        c.drawLine(13.45f, 11.4f, 13.45f, 15f, p);
        c.drawLine(16.45f, 12.7f, 16.45f, 15f, p);
        return new Cursor(bmp, 8, 1, "HAND");
    }

    /** Crosshair, hotspot in the middle — the interesting case for hotspots. */
    public static Cursor crossCursor() {
        final int n = 21, m = n / 2;
        final Bitmap bmp = Bitmap.createBitmap(n, n, Bitmap.Config.ARGB_8888);
        final Canvas c = new Canvas(bmp);
        final Paint p = new Paint();
        p.setStyle(Paint.Style.STROKE);
        p.setStrokeWidth(3f);
        p.setColor(Color.WHITE);
        c.drawLine(0, m + 0.5f, n, m + 0.5f, p);
        c.drawLine(m + 0.5f, 0, m + 0.5f, n, p);
        p.setStrokeWidth(1f);
        p.setColor(Color.BLACK);
        c.drawLine(0, m + 0.5f, n, m + 0.5f, p);
        c.drawLine(m + 0.5f, 0, m + 0.5f, n, p);
        return new Cursor(bmp, m, m, "CROSS");
    }
}
