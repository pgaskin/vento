// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control;

import java.util.Arrays;

/**
 * Maps remote-desktop coordinates to screen coordinates, ported from RealVNC's
 * {@code ui.scroll.f} (ScaleManager) and {@code ui.scroll.c} (scale limits).
 *
 * <pre>
 *   screen  = desktop * scale + origin
 *   desktop = (screen - origin) / scale
 * </pre>
 *
 * <p>{@link #centreOn} is the whole "touchpad" trick: put the focus point at the
 * viewport centre, then clamp the origin to the desktop bounds. In the interior
 * of the desktop the cursor therefore sits exactly at the centre of the screen
 * and the desktop slides underneath it; near an edge the clamp takes over and
 * the cursor slides away from the centre instead. No code anywhere special-cases
 * "cursor near the edge" — it falls out of the clamp.
 *
 * <p>The view can be <em>inset</em>: everything above works on the content rect
 * (the view minus {@link #setInsets}), so system bars or an on-screen keyboard
 * shrink the window onto the desktop without moving the desktop under the
 * remaining pixels. With zero insets — the default — the content rect is the
 * view and nothing changes.
 */
public final class Viewport {

    private final float density;

    private int fbW = 1, fbH = 1;
    private int viewW, viewH;
    private int insetL, insetT, insetR, insetB;

    private float scale = 1.0f;
    private float originX, originY;
    private float focusX, focusY; // scale focus, in desktop coords

    // The zoom-button ladder, ascending, and its cursor.
    private float[] ladder = {};
    private int ladderIndex;
    private int[] fitSizes = {};

    public Viewport(float density) {
        this.density = Math.max(density, 1.0f);
    }

    public void setDesktopSize(int w, int h) {
        fbW = Math.max(w, 1);
        fbH = Math.max(h, 1);
        rebuildLadder();
    }

    public void setViewSize(int w, int h) {
        viewW = w;
        viewH = h;
        rebuildLadder();
    }

    /**
     * Shrink the usable window onto the desktop by this many pixels on each
     * side. The original applies its insets in some layout modes only; we have
     * one mode and it always applies them.
     */
    public void setInsets(int left, int top, int right, int bottom) {
        insetL = Math.max(left, 0);
        insetT = Math.max(top, 0);
        insetR = Math.max(right, 0);
        insetB = Math.max(bottom, 0);
        rebuildLadder();
    }

    public int insetLeft() {
        return insetL;
    }

    public int insetTop() {
        return insetT;
    }

    public int insetRight() {
        return insetR;
    }

    public int insetBottom() {
        return insetB;
    }

    /** View width minus the horizontal insets — the window the desktop gets. */
    public int contentWidth() {
        return Math.max(viewW - insetL - insetR, 0);
    }

    public int contentHeight() {
        return Math.max(viewH - insetT - insetB, 0);
    }

    /** Screen x the focus point is centred on. */
    public float centreScreenX() {
        return insetL + contentWidth() / 2.0f;
    }

    public float centreScreenY() {
        return insetT + contentHeight() / 2.0f;
    }

    public int desktopWidth() {
        return fbW;
    }

    public int desktopHeight() {
        return fbH;
    }

    public int viewWidth() {
        return viewW;
    }

    public int viewHeight() {
        return viewH;
    }

    public float getScale() {
        return scale;
    }

    public float originX() {
        return originX;
    }

    public float originY() {
        return originY;
    }

    public float toScreenX(float dx) {
        return dx * scale + originX;
    }

    public float toScreenY(float dy) {
        return dy * scale + originY;
    }

    public float toDesktopX(float sx) {
        return (sx - originX) / scale;
    }

    public float toDesktopY(float sy) {
        return (sy - originY) / scale;
    }

    public void setFocus(float dx, float dy) {
        focusX = dx;
        focusY = dy;
    }

    public float focusX() {
        return focusX;
    }

    public float focusY() {
        return focusY;
    }

    /** Fit-the-whole-desktop scale. */
    public float minScale() {
        if (contentWidth() == 0 || contentHeight() == 0) {
            return 1.0f;
        }
        return Math.min((float) contentWidth() / fbW, (float) contentHeight() / fbH);
    }

    /** Fill-the-window scale: the other axis' fit, so the desktop overflows on one. */
    public float fillScale() {
        if (contentWidth() == 0 || contentHeight() == 0) {
            return 1.0f;
        }
        return Math.max((float) contentWidth() / fbW, (float) contentHeight() / fbH);
    }

    /** {@code 2 * floor(density)} — the original's cap. */
    public float maxScale() {
        return 2.0f * (float) Math.floor(density);
    }

    /** Quantise to 1/128, snap to a "nice" desktop width, clamp to the limits. */
    public float snapScale(float s) {
        float q = ((int) (s * 128.0f)) / 128.0f;
        final float w = fbW * q;
        final int steps = (int) Math.floor(density);
        for (int k = 1; k <= steps; k++) {
            q = snapTo(q, w, fbW * k);
        }
        if (contentWidth() > 0 && contentHeight() > 0) {
            q = snapTo(q, w, contentWidth());                       // fit width
            q = snapTo(q, w, fbW * ((float) contentHeight() / fbH)); // fit height
        }
        return clamp(q, minScale(), maxScale());
    }

    private float snapTo(float q, float w, float candidateW) {
        if (candidateW > 0 && Math.abs(w - candidateW) <= 0.04f * candidateW) {
            return candidateW / fbW;
        }
        return q;
    }

    /** Set the scale (snapped) and re-centre on the current focus point. */
    public void setScale(float s) {
        centreOn(focusX, focusY, snapScale(s));
    }

    public void centreOn(float dx, float dy) {
        centreOn(dx, dy, scale);
    }

    /** ScaleManager.g — centre on the focus point, then clamp to the desktop. */
    public void centreOn(float dx, float dy, float s) {
        final int cw = contentWidth(), ch = contentHeight();
        if (cw == 0 || ch == 0) {
            scale = s;
            return;
        }
        final boolean scaleChanged = (scale != s);
        scale = s;

        final float ox = centreScreenX() - dx * s;
        final float oy = centreScreenY() - dy * s;
        final float dw = fbW * s;
        final float dh = fbH * s;

        // A desktop smaller than the window is centred in the *view*, then pushed
        // inside the content rect — not centred in the content rect, which is
        // what makes the original's picture jump by half the inset whenever an
        // overlay appears. Nothing clamps this axis, so the rule has to be one
        // that comes back on its own: this moves only when an inset leaves it
        // nowhere else to be, and moves back when that inset goes.
        originX = (dw < cw)
                ? clamp((viewW - dw) / 2.0f, insetL, insetL + cw - dw)
                : clamp(ox, insetL + cw - dw, insetL);
        originY = (dh < ch)
                ? clamp((viewH - dh) / 2.0f, insetT, insetT + ch - dh)
                : clamp(oy, insetT + ch - dh, insetT);

        // The original stores the viewport centre back as the scale focus.
        focusX = toDesktopX(centreScreenX());
        focusY = toDesktopY(centreScreenY());

        if (scaleChanged) {
            selectNearestLadderEntry(s);
        }
    }

    /**
     * Slide the picture by this many screen pixels, which is what the pan half
     * of a pinch is.
     *
     * <p>It moves the <em>focus</em>, not the origin. The origin is not state:
     * every {@link #centreOn} recomputes it from the focus, so an origin written
     * here would be silently undone by the next thing that re-centres — an inset
     * change, an overlay appearing, a desktop resize. Moving the focus keeps
     * every derivation, and with them both edge rules: a pan cannot push the
     * desktop off its window, and an axis where the desktop is smaller than the
     * window does not pan at all, because the same clamp that centres it there
     * pins it there.
     */
    public void panBy(float screenDx, float screenDy) {
        centreOn(focusX - screenDx / scale, focusY - screenDy / scale);
    }

    // ---- the zoom-button ladder (ScaleManager.z / A / B) ------------------

    /**
     * The discrete scales the zoom buttons step through, ascending: the
     * original's {@code 1.0 × 0.66ⁿ} down to fit, plus 1.5, 2.0, fit and fill.
     *
     * <p>Two corrections to it. Each entry goes through {@link #snapScale}, so
     * stepping to an entry lands exactly on it and the index survives the round
     * trip; entries that collide afterwards are dropped, which the original does
     * not do, leaving it dead steps — fit and fill coincide whenever the desktop
     * and the window share an aspect ratio, and on a desktop smaller than the
     * window everything below fit clamps onto fit.
     */
    public float[] zoomLadder() {
        return ladder.clone();
    }

    /**
     * Sizes worth having a zoom-button rung for: the scale at which a region of
     * the desktop this big fits the window, one rung each.
     *
     * <p>Caller-supplied because what a *region* is depends on what is at the
     * other end and this class has no business knowing — the same shape as the
     * caller-supplied names on the tap regions. A size that lands on a rung the
     * ladder already has is dropped like any other collision, so handing over a
     * region the size of the desktop changes nothing.
     *
     * @param sizes width and height in desktop pixels, in pairs; anything
     *              non-positive or longer than the desktop is ignored
     */
    public void setFitSizes(int... sizes) {
        fitSizes = sizes == null ? new int[0] : sizes.clone();
        rebuildLadder();
    }

    /** Where the current scale sits in {@link #zoomLadder}. */
    public int zoomIndex() {
        return ladderIndex;
    }

    public boolean canZoomIn() {
        return ladderIndex < ladder.length - 1;
    }

    public boolean canZoomOut() {
        return ladderIndex > 0;
    }

    /** Next ladder entry up, or the current scale if there is none. */
    public float nextZoomIn() {
        return canZoomIn() ? ladder[ladderIndex + 1] : scale;
    }

    public float nextZoomOut() {
        return canZoomOut() ? ladder[ladderIndex - 1] : scale;
    }

    /**
     * Step one entry up the ladder, re-centring on the current focus. Callers
     * that want the original's behaviour set the focus to the cursor first and
     * re-centre the cursor afterwards.
     */
    public void zoomIn() {
        if (canZoomIn()) {
            setScale(ladder[ladderIndex + 1]);
        }
    }

    public void zoomOut() {
        if (canZoomOut()) {
            setScale(ladder[ladderIndex - 1]);
        }
    }

    /** Fit the whole desktop in the window. */
    public void zoomToFit() {
        setScale(minScale());
    }

    /**
     * Fill the window, overflowing on the longer axis — {@code ScaleManager.u},
     * which caps it at {@code floor(density)} (half of {@link #maxScale}) so a
     * tall desktop on a wide phone cannot jump straight to a huge scale.
     */
    public void zoomToFill() {
        setScale(Math.min(fillScale(), (float) Math.floor(density)));
    }

    private void rebuildLadder() {
        if (contentWidth() == 0 || contentHeight() == 0) {
            ladder = new float[0];
            ladderIndex = 0;
            return;
        }
        final float fit = minScale();
        final float[] raw = new float[8 + 32 + fitSizes.length / 2];
        int n = 0;
        for (float f = 1.0f; f > fit && n < 32; f *= 0.66f) {
            raw[n++] = snapScale(f);
        }
        raw[n++] = snapScale(1.5f);
        raw[n++] = snapScale(2.0f);
        raw[n++] = snapScale(fit);
        raw[n++] = snapScale(fillScale());
        for (int i = 0; i + 1 < fitSizes.length; i += 2) {
            final int w = fitSizes[i], h = fitSizes[i + 1];
            if (w > 0 && h > 0 && w <= fbW && h <= fbH) {
                raw[n++] = snapScale(Math.min((float) contentWidth() / w,
                        (float) contentHeight() / h));
            }
        }

        Arrays.sort(raw, 0, n);
        final float[] out = new float[n];
        int m = 0;
        for (int i = 0; i < n; i++) {
            if (m == 0 || raw[i] - out[m - 1] > 1e-4f) {
                out[m++] = raw[i];
            }
        }
        ladder = Arrays.copyOf(out, m);
        selectNearestLadderEntry(scale);
    }

    /** ScaleManager.y — put the index on whichever entry is closest to {@code s}. */
    private void selectNearestLadderEntry(float s) {
        int best = -1;
        float bestErr = 0;
        for (int i = 0; i < ladder.length; i++) {
            final float err = Math.abs(s - ladder[i]);
            if (best < 0 || err < bestErr) {
                best = i;
                bestErr = err;
            }
        }
        ladderIndex = Math.max(best, 0);
    }

    public static float clamp(float v, float lo, float hi) {
        return v < lo ? lo : (v > hi ? hi : v);
    }
}
