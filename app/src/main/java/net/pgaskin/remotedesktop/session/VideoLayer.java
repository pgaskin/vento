// SPDX-FileCopyrightText: 2026 Nick Haghiri
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.session;

import android.content.Context;
import android.graphics.Bitmap;
import android.graphics.Path;
import android.graphics.Rect;
import android.opengl.GLES20;
import android.opengl.GLSurfaceView;

import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.FloatBuffer;
import java.util.ArrayList;
import java.util.Iterator;
import java.util.List;

import javax.microedition.khronos.egl.EGLConfig;
import javax.microedition.khronos.opengles.GL10;

/**
 * The remote desktop's video, drawn by the GPU: one textured rectangle per region
 * the server's H.264 has painted, under the desktop's own canvas.
 *
 * <p>A region is a rectangle of the desktop whose latest pixels are a decoded
 * frame rather than the CPU framebuffer. The frame arrives as NV12 planes and is
 * converted to RGB in the fragment shader. The view clips its own tiles out of
 * every region, so the old pixels under a region never show through, and any
 * non-video update that lands on a region removes it, which gives the pixels back
 * to the CPU framebuffer.
 *
 * <p>Threads: frames and damage come from the protocol thread; drawing and every
 * GL call happen on the GL thread. The region list is guarded by {@code this}, and
 * the rectangles the view clips with are a snapshot that changes all at once.
 */
public final class VideoLayer implements GLSurfaceView.Renderer {

    private final GLSurfaceView surface;
    private final List<Region> regions = new ArrayList<>();
    /** Video that damage has covered: what is left of a region, to be repainted from its last frame. */
    private final List<Ghost> ghosts = new ArrayList<>();

    // The view's transform: desktop pixel to view pixel, as the tiles are drawn.
    private volatile float scale = 1f;
    private volatile float originX;
    private volatile float originY;
    private volatile int viewWidth = 1;
    private volatile int viewHeight = 1;

    private int program;
    private int positionLocation;
    private int textureLocation;
    private int bt709Location;
    private int fullRangeLocation;
    private final FloatBuffer quad = ByteBuffer.allocateDirect(16 * 4).order(ByteOrder.nativeOrder()).asFloatBuffer();

    /** One rectangle of the desktop, and the textures that hold its latest frame. */
    private static final class Region {
        final int x, y, w, h;
        ByteBuffer pending; // NV12, waiting for the GL thread
        byte[] nv12; // the latest frame, kept for the CPU reads: thumbnails and the mirror
        int mode; // bit 0: BT.709, bit 1: full range; set with pending, read with it
        int yTexture;
        int uvTexture;
        boolean textured;

        Region(int x, int y, int w, int h) {
            this.x = x;
            this.y = y;
            this.w = w;
            this.h = h;
        }

        boolean overlaps(int ox, int oy, int ow, int oh) {
            return x < ox + ow && ox < x + w && y < oy + oh && oy < y + h;
        }
    }

    public VideoLayer(Context context) {
        surface = new GLSurfaceView(context);
        surface.setEGLContextClientVersion(2);
        surface.setRenderer(this);
        surface.setRenderMode(GLSurfaceView.RENDERMODE_WHEN_DIRTY);
    }

    /** The view to put behind the desktop's own view. */
    public GLSurfaceView surface() {
        return surface;
    }

    /**
     * A frame for a rectangle, from any thread. Replaces the region's last frame.
     * The frame is {@code frameW} by {@code frameH} and may be larger than the
     * rectangle; the rectangle's top-left part is what is kept.
     */
    public void submit(int x, int y, int w, int h, int frameW, int frameH, int colorMode, byte[] nv12) {
        if (w <= 0 || h <= 0 || frameW < w || frameH < h) {
            return;
        }
        final byte[] picture = crop(nv12, frameW, frameH, w, h);
        ByteBuffer copy = ByteBuffer.allocateDirect(picture.length).order(ByteOrder.nativeOrder());
        copy.put(picture).position(0);
        synchronized (this) {
            Region region = find(x, y, w, h);
            if (region == null) {
                region = new Region(x, y, w, h);
                regions.add(region);
            }
            region.pending = copy;
            region.nv12 = picture;
            region.mode = colorMode;
            // What video covers again is no longer waiting to be repainted.
            for (Iterator<Ghost> it = ghosts.iterator(); it.hasNext(); ) {
                Ghost ghost = it.next();
                final List<Rect> rest = new ArrayList<>();
                for (Rect r : ghost.rest) {
                    rest.addAll(subtract(r, x, y, x + w, y + h));
                }
                ghost.rest = rest;
                if (rest.isEmpty()) {
                    it.remove();
                }
            }
        }
        surface.requestRender();
    }

    /**
     * Non-video pixels landed on this rectangle: any region under it gives it back.
     * What is left of a region is kept as a ghost, since the tiles under it are
     * stale, and the returned rectangles are where the caller must repaint them.
     */
    public List<Rect> damaged(int x, int y, int w, int h) {
        final List<Rect> repaint = new ArrayList<>();
        synchronized (this) {
            boolean removed = false;
            for (Iterator<Region> it = regions.iterator(); it.hasNext(); ) {
                Region region = it.next();
                if (region.overlaps(x, y, w, h)) {
                    it.remove();
                    removed = true;
                    final List<Rect> rest = subtract(
                            new Rect(region.x, region.y, region.x + region.w, region.y + region.h),
                            x, y, x + w, y + h);
                    if (!rest.isEmpty()) {
                        ghosts.add(new Ghost(region, rest));
                        repaint.addAll(rest);
                    }
                }
            }
            for (Iterator<Ghost> it = ghosts.iterator(); it.hasNext(); ) {
                Ghost ghost = it.next();
                final List<Rect> rest = new ArrayList<>();
                for (Rect r : ghost.rest) {
                    rest.addAll(subtract(r, x, y, x + w, y + h));
                }
                ghost.rest = rest;
                if (rest.isEmpty()) {
                    it.remove();
                }
            }
            if (!removed) {
                return repaint;
            }
        }
        surface.requestRender();
        return repaint;
    }

    /** Every region is gone: a new desktop, or a reconnect. */
    public void clear() {
        synchronized (this) {
            if (regions.isEmpty() && ghosts.isEmpty()) {
                return;
            }
            regions.clear();
            ghosts.clear();
        }
        surface.requestRender();
    }

    /** Whether any video is on screen or waiting to be repainted. */
    public boolean hasVideo() {
        synchronized (this) {
            return !regions.isEmpty() || !ghosts.isEmpty();
        }
    }

    /**
     * Paints the video under a desktop rectangle into {@code dst}, which holds the
     * desktop from ({@code x}, {@code y}) at ({@code dstX}, {@code dstY}). Anything
     * not under video is left as it was. True if any video was painted.
     */
    public boolean paintRegion(Bitmap dst, int x, int y, int w, int h, int dstX, int dstY) {
        boolean painted = false;
        final int[] row = new int[w];
        for (Source s : sources()) {
            final int left = Math.max(x, s.area.left), top = Math.max(y, s.area.top);
            final int right = Math.min(x + w, s.area.right), bottom = Math.min(y + h, s.area.bottom);
            if (left >= right || top >= bottom) {
                continue;
            }
            painted = true;
            final int n = right - left;
            for (int py = top; py < bottom; py++) {
                for (int px = left; px < right; px++) {
                    row[px - left] = argb(s, px, py);
                }
                dst.setPixels(row, 0, n, dstX + left - x, dstY + py - y, n, 1);
            }
        }
        return painted;
    }

    /**
     * Paints the video into a thumbnail of the whole desktop, {@code dst} being the
     * desktop of {@code desktopW} by {@code desktopH} at whatever size it is.
     */
    public void paintThumbnail(Bitmap dst, int desktopW, int desktopH) {
        final float sx = desktopW / (float) dst.getWidth();
        final float sy = desktopH / (float) dst.getHeight();
        final int[] row = new int[dst.getWidth()];
        for (Source s : sources()) {
            final int x0 = Math.max(0, (int) Math.ceil(s.area.left / sx));
            final int x1 = Math.min(dst.getWidth(), (int) Math.ceil(s.area.right / sx));
            final int y0 = Math.max(0, (int) Math.ceil(s.area.top / sy));
            final int y1 = Math.min(dst.getHeight(), (int) Math.ceil(s.area.bottom / sy));
            if (x0 >= x1 || y0 >= y1) {
                continue;
            }
            final int n = x1 - x0;
            for (int ty = y0; ty < y1; ty++) {
                final int py = Math.min(s.area.bottom - 1, (int) (ty * sy));
                for (int tx = x0; tx < x1; tx++) {
                    final int px = Math.min(s.area.right - 1, (int) (tx * sx));
                    row[tx - x0] = argb(s, px, py);
                }
                dst.setPixels(row, 0, n, x0, ty, n, 1);
            }
        }
    }

    /** One picture and the part of the desktop it is still the picture for. */
    private static final class Source {
        final byte[] nv12;
        final int mode, rx, ry, rw, rh;
        final Rect area;

        Source(byte[] nv12, int mode, int rx, int ry, int rw, int rh, Rect area) {
            this.nv12 = nv12;
            this.mode = mode;
            this.rx = rx;
            this.ry = ry;
            this.rw = rw;
            this.rh = rh;
            this.area = area;
        }
    }

    /** Every live region, and every ghost's remainder, as sources to paint from. */
    private List<Source> sources() {
        final List<Source> out = new ArrayList<>();
        synchronized (this) {
            for (Region r : regions) {
                if (r.nv12 != null) {
                    out.add(new Source(r.nv12, r.mode, r.x, r.y, r.w, r.h,
                            new Rect(r.x, r.y, r.x + r.w, r.y + r.h)));
                }
            }
            for (Ghost g : ghosts) {
                for (Rect rest : g.rest) {
                    out.add(new Source(g.region.nv12, g.region.mode, g.region.x, g.region.y,
                            g.region.w, g.region.h, rest));
                }
            }
        }
        return out;
    }

    /** A region that video no longer covers, and the part of it still to be repainted. */
    private static final class Ghost {
        final Region region;
        List<Rect> rest;

        Ghost(Region region, List<Rect> rest) {
            this.region = region;
            this.rest = rest;
        }
    }

    /** The parts of {@code r} that are not inside the rectangle (x0, y0)-(x1, y1). */
    private static List<Rect> subtract(Rect r, int x0, int y0, int x1, int y1) {
        final List<Rect> out = new ArrayList<>();
        if (r.right <= x0 || r.left >= x1 || r.bottom <= y0 || r.top >= y1) {
            out.add(r);
            return out;
        }
        if (r.top < y0) {
            out.add(new Rect(r.left, r.top, r.right, y0));
        }
        if (r.bottom > y1) {
            out.add(new Rect(r.left, y1, r.right, r.bottom));
        }
        final int top = Math.max(r.top, y0), bottom = Math.min(r.bottom, y1);
        if (r.left < x0) {
            out.add(new Rect(r.left, top, x0, bottom));
        }
        if (r.right > x1) {
            out.add(new Rect(x1, top, r.right, bottom));
        }
        return out;
    }

    /** A picture's sample at a desktop position, as ARGB, from its NV12 planes. */
    private static int argb(Source s, int px, int py) {
        final int lx = px - s.rx, ly = py - s.ry;
        final int cw = (s.rw + 1) / 2;
        final float y = (s.nv12[ly * s.rw + lx] & 0xff);
        final int uv = s.rw * s.rh + (ly / 2) * cw * 2 + (lx / 2) * 2;
        float u = (s.nv12[uv] & 0xff) - 128f;
        float v = (s.nv12[uv + 1] & 0xff) - 128f;
        final boolean bt709 = (s.mode & 1) != 0;
        final boolean full = (s.mode & 2) != 0;
        float luma = y;
        if (!full) {
            luma = (y - 16f) * 255f / 219f;
            u = u * 255f / 224f;
            v = v * 255f / 224f;
        }
        final float r = luma + (bt709 ? 1.5748f : 1.402f) * v;
        final float g = luma - (bt709 ? 0.1873f : 0.3441f) * u - (bt709 ? 0.4681f : 0.7141f) * v;
        final float b = luma + (bt709 ? 1.8556f : 1.772f) * u;
        return 0xff000000 | (clamp8(r) << 16) | (clamp8(g) << 8) | clamp8(b);
    }

    private static int clamp8(float v) {
        return v <= 0f ? 0 : v >= 255f ? 255 : Math.round(v);
    }

    /** The top-left {@code w} by {@code h} of an NV12 picture of {@code frameW} by {@code frameH}. */
    private static byte[] crop(byte[] nv12, int frameW, int frameH, int w, int h) {
        if (frameW == w && frameH == h) {
            return nv12;
        }
        final int cw = (w + 1) / 2, fcw = (frameW + 1) / 2;
        final byte[] out = new byte[w * h + 2 * cw * ((h + 1) / 2)];
        for (int row = 0; row < h; row++) {
            System.arraycopy(nv12, row * frameW, out, row * w, w);
        }
        final int uvBase = frameW * frameH, outUv = w * h;
        for (int row = 0; row < (h + 1) / 2; row++) {
            System.arraycopy(nv12, uvBase + row * fcw * 2, out, outUv + row * cw * 2, cw * 2);
        }
        return out;
    }

    /** The clip for the desktop's own tiles: every region, in desktop pixels. */
    public Path holes() {
        Path path = new Path();
        synchronized (this) {
            for (Region r : regions) {
                path.addRect(r.x, r.y, r.x + r.w, r.y + r.h, Path.Direction.CW);
            }
        }
        return path;
    }

    /** Where the desktop is drawn in the view, as the view draws it. */
    public void setView(float scale, float originX, float originY, int width, int height) {
        this.scale = scale;
        this.originX = originX;
        this.originY = originY;
        this.viewWidth = Math.max(1, width);
        this.viewHeight = Math.max(1, height);
        surface.requestRender();
    }

    private Region find(int x, int y, int w, int h) {
        for (Region region : regions) {
            if (region.x == x && region.y == y && region.w == w && region.h == h) {
                return region;
            }
        }
        return null;
    }

    private static final String VERTEX =
            "attribute vec2 p;\n"
            + "attribute vec2 t;\n"
            + "varying vec2 v;\n"
            + "void main() {\n"
            + "  v = t;\n"
            + "  gl_Position = vec4(p, 0.0, 1.0);\n"
            + "}\n";

    // YUV to RGB for the frame's own colour mode: BT.601 or BT.709 matrix, limited or
    // full range. Limited range is expanded from 16..235 and 16..240 to 0..1.
    private static final String FRAGMENT =
            "precision mediump float;\n"
            + "varying vec2 v;\n"
            + "uniform sampler2D yTex;\n"
            + "uniform sampler2D uvTex;\n"
            + "uniform float bt709;\n"
            + "uniform float fullRange;\n"
            + "void main() {\n"
            + "  float y = texture2D(yTex, v).r;\n"
            + "  vec2 uv = texture2D(uvTex, v).ra - vec2(0.5019608);\n"
            + "  if (fullRange < 0.5) {\n"
            + "    y = (y - 0.0627451) * 1.1643836;\n"
            + "    uv *= 1.1383929;\n"
            + "  }\n"
            + "  float kr = mix(1.402, 1.5748, bt709);\n"
            + "  float kgu = mix(0.344136, 0.187324, bt709);\n"
            + "  float kgv = mix(0.714136, 0.468124, bt709);\n"
            + "  float kb = mix(1.772, 1.8556, bt709);\n"
            + "  gl_FragColor = vec4(y + kr * uv.y,\n"
            + "                      y - kgu * uv.x - kgv * uv.y,\n"
            + "                      y + kb * uv.x, 1.0);\n"
            + "}\n";

    @Override
    public void onSurfaceCreated(GL10 gl, EGLConfig config) {
        program = link(VERTEX, FRAGMENT);
        GLES20.glUseProgram(program);
        positionLocation = GLES20.glGetAttribLocation(program, "p");
        textureLocation = GLES20.glGetAttribLocation(program, "t");
        GLES20.glUniform1i(GLES20.glGetUniformLocation(program, "yTex"), 0);
        GLES20.glUniform1i(GLES20.glGetUniformLocation(program, "uvTex"), 1);
        bt709Location = GLES20.glGetUniformLocation(program, "bt709");
        fullRangeLocation = GLES20.glGetUniformLocation(program, "fullRange");
        GLES20.glPixelStorei(GLES20.GL_UNPACK_ALIGNMENT, 1);
    }

    @Override
    public void onSurfaceChanged(GL10 gl, int width, int height) {
        GLES20.glViewport(0, 0, width, height);
    }

    @Override
    public void onDrawFrame(GL10 gl) {
        // Snapshot the regions, and upload what is pending, on this thread only.
        List<Region> now;
        synchronized (this) {
            now = new ArrayList<>(regions);
        }
        GLES20.glClearColor(0f, 0f, 0f, 1f);
        GLES20.glClear(GLES20.GL_COLOR_BUFFER_BIT);
        GLES20.glUseProgram(program);
        for (Region region : now) {
            ByteBuffer frame;
            int mode;
            synchronized (this) {
                frame = region.pending;
                region.pending = null;
                mode = region.mode;
            }
            if (frame != null) {
                upload(region, frame);
            }
            if (region.textured) {
                GLES20.glUniform1f(bt709Location, mode & 1);
                GLES20.glUniform1f(fullRangeLocation, (mode >> 1) & 1);
                draw(region);
            }
        }
    }

    private void upload(Region region, ByteBuffer frame) {
        int cw = (region.w + 1) / 2;
        int ch = (region.h + 1) / 2;
        if (!region.textured) {
            region.yTexture = texture(GLES20.GL_LUMINANCE);
            region.uvTexture = texture(GLES20.GL_LUMINANCE_ALPHA);
        }
        frame.position(0);
        GLES20.glActiveTexture(GLES20.GL_TEXTURE0);
        GLES20.glBindTexture(GLES20.GL_TEXTURE_2D, region.yTexture);
        // The Y plane is the first w by h bytes of the frame.
        GLES20.glTexImage2D(GLES20.GL_TEXTURE_2D, 0, GLES20.GL_LUMINANCE, region.w, region.h, 0,
                GLES20.GL_LUMINANCE, GLES20.GL_UNSIGNED_BYTE, frame);
        frame.position(region.w * region.h);
        GLES20.glActiveTexture(GLES20.GL_TEXTURE1);
        GLES20.glBindTexture(GLES20.GL_TEXTURE_2D, region.uvTexture);
        GLES20.glTexImage2D(GLES20.GL_TEXTURE_2D, 0, GLES20.GL_LUMINANCE_ALPHA, cw, ch, 0,
                GLES20.GL_LUMINANCE_ALPHA, GLES20.GL_UNSIGNED_BYTE, frame);
        region.textured = true;
    }

    private void draw(Region region) {
        float s = scale;
        float left = region.x * s + originX;
        float top = region.y * s + originY;
        float right = left + region.w * s;
        float bottom = top + region.h * s;
        float vw = viewWidth;
        float vh = viewHeight;
        // Two triangles as a strip: position in clip space, then texture coordinates.
        float[] v = {
                toClipX(left, vw), toClipY(top, vh), 0f, 0f,
                toClipX(right, vw), toClipY(top, vh), 1f, 0f,
                toClipX(left, vw), toClipY(bottom, vh), 0f, 1f,
                toClipX(right, vw), toClipY(bottom, vh), 1f, 1f,
        };
        quad.position(0);
        quad.put(v).position(0);
        GLES20.glEnableVertexAttribArray(positionLocation);
        GLES20.glEnableVertexAttribArray(textureLocation);
        quad.position(0);
        GLES20.glVertexAttribPointer(positionLocation, 2, GLES20.GL_FLOAT, false, 16, quad);
        quad.position(2);
        GLES20.glVertexAttribPointer(textureLocation, 2, GLES20.GL_FLOAT, false, 16, quad);
        GLES20.glActiveTexture(GLES20.GL_TEXTURE0);
        GLES20.glBindTexture(GLES20.GL_TEXTURE_2D, region.yTexture);
        GLES20.glActiveTexture(GLES20.GL_TEXTURE1);
        GLES20.glBindTexture(GLES20.GL_TEXTURE_2D, region.uvTexture);
        GLES20.glDrawArrays(GLES20.GL_TRIANGLE_STRIP, 0, 4);
    }

    private static float toClipX(float px, float width) {
        return px / width * 2f - 1f;
    }

    private static float toClipY(float py, float height) {
        return 1f - py / height * 2f;
    }

    private static int texture(int format) {
        int[] ids = new int[1];
        GLES20.glGenTextures(1, ids, 0);
        GLES20.glBindTexture(GLES20.GL_TEXTURE_2D, ids[0]);
        GLES20.glTexParameteri(GLES20.GL_TEXTURE_2D, GLES20.GL_TEXTURE_MIN_FILTER, GLES20.GL_LINEAR);
        GLES20.glTexParameteri(GLES20.GL_TEXTURE_2D, GLES20.GL_TEXTURE_MAG_FILTER, GLES20.GL_LINEAR);
        GLES20.glTexParameteri(GLES20.GL_TEXTURE_2D, GLES20.GL_TEXTURE_WRAP_S, GLES20.GL_CLAMP_TO_EDGE);
        GLES20.glTexParameteri(GLES20.GL_TEXTURE_2D, GLES20.GL_TEXTURE_WRAP_T, GLES20.GL_CLAMP_TO_EDGE);
        return ids[0];
    }

    private static int link(String vertex, String fragment) {
        int vs = compile(GLES20.GL_VERTEX_SHADER, vertex);
        int fs = compile(GLES20.GL_FRAGMENT_SHADER, fragment);
        int p = GLES20.glCreateProgram();
        GLES20.glAttachShader(p, vs);
        GLES20.glAttachShader(p, fs);
        GLES20.glLinkProgram(p);
        return p;
    }

    private static int compile(int type, String source) {
        int s = GLES20.glCreateShader(type);
        GLES20.glShaderSource(s, source);
        GLES20.glCompileShader(s);
        return s;
    }
}
