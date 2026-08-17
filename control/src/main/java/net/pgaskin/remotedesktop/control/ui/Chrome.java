// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.ui;

import android.graphics.Canvas;
import android.graphics.Matrix;
import android.graphics.Paint;
import android.graphics.Path;
import android.graphics.Rect;
import android.graphics.RectF;

import net.pgaskin.remotedesktop.control.input.Config;
import net.pgaskin.remotedesktop.control.input.ExtensionKeyboard;
import net.pgaskin.remotedesktop.control.input.MouseOverlay;
import net.pgaskin.remotedesktop.control.input.TapRegions;

import java.util.function.Function;

/**
 * Draws the on-screen controls — the mouse overlay, the extension keyboard and
 * its info bar — from the geometry those models compute.
 *
 * <p>Those are models, not views: they decide where every part is and what
 * state it is in, and know nothing about a {@code Canvas}, which is what lets
 * them run under the JVM test harness. Something still has to put ink on the
 * screen, and two screens need the same ink — the playground and a real
 * session — so it lives here rather than in either.
 */
public final class Chrome {

    private static final float RIPPLE_NANOS = 260e6f; // a key's ripple, start to finish
    private static final float BAR_FADE_DP = 40f;     // where the cursor starts pushing the bar out
    private static final float BAR_FADE_MIN = 0.1f;   // what is left of it: a hint, not a panel

    private final Config cfg;

    private final Paint panel = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint marker = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint text = new Paint(Paint.ANTI_ALIAS_FLAG);
    // keyPaint is also the label measurer, so its size must not change elsewhere.
    private final Paint keyPaint = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint barPaint = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint iconPaint = new Paint(Paint.ANTI_ALIAS_FLAG);

    private final RectF box = new RectF();
    private final Path path = new Path();       // the overlay's per-corner rounding
    private final float[] corners = new float[8];
    private final Rect bounds = new Rect();
    private final Matrix matrix = new Matrix();

    // The key ripple: which key, from where, and when. The keyboard claims its
    // own pointers, so no view ever sees the touch and has to be told.
    private ExtensionKeyboard.Key lastPressed;
    private ExtensionKeyboard.Bounds rippleBounds;
    private long rippleStartNanos;
    private float rippleX, rippleY;

    public Chrome(Config cfg) {
        this.cfg = cfg;
        marker.setStyle(Paint.Style.STROKE);
        marker.setStrokeWidth(1f);
        marker.setColor(0x66ffffff);
        text.setColor(0xffe8f0ff);
        keyPaint.setTextSize(cfg.dp(12.5f));
        keyPaint.setTextAlign(Paint.Align.CENTER);
        barPaint.setTextAlign(Paint.Align.CENTER);
    }

    /** The keyboard measures its labels with our font, so it must be ours. */
    public void attach(ExtensionKeyboard keyboard) {
        keyboard.setLabelWidth(keyPaint::measureText);
    }

    /**
     * Call from the keyboard's own change notification: a new press starts a
     * ripple, and the press is the only moment its origin is known.
     */
    public void keyboardChanged(ExtensionKeyboard keyboard) {
        final ExtensionKeyboard.Key pressed = keyboard.pressedKey();
        if (pressed == lastPressed) {
            return;
        }
        lastPressed = pressed;
        if (pressed == null) {
            return;
        }
        rippleStartNanos = System.nanoTime();
        rippleX = keyboard.pressedX();
        rippleY = keyboard.pressedY();
        rippleBounds = null;
        for (ExtensionKeyboard.Bounds b : keyboard.keys()) {
            if (b.key() == pressed) {
                rippleBounds = b;
                break;
            }
        }
    }

    // ---- the mouse overlay -------------------------------------------------

    /** @return whether another frame is wanted (never; the overlay is static) */
    public boolean drawOverlay(Canvas c, MouseOverlay overlay) {
        for (MouseOverlay.Bounds b : overlay.parts()) {
            box.set(b.left(), b.top(), b.right(), b.bottom());
            final boolean down = overlay.pressed(b.part());
            // Faint on purpose: the overlay shares the touch surface with the
            // desktop rather than covering it, and a zoomed-in picture runs
            // underneath these. The fills carry the state — pressed is the same
            // colour half again as opaque — and the outline carries the shape,
            // which is why it is the one part that is not faint.
            panel.setColor(b.part() == MouseOverlay.Part.DISMISS
                    ? (down ? 0x99404a58 : 0x5005090e)
                    : (down ? 0x994a5a70 : 0x60151a20));
            edged(box, cfg.dp(3), overlay);
            c.drawPath(path, panel);
            marker.setColor(0x70ffffff);
            c.drawPath(path, marker);

            // The three buttons are unlabelled: they are in the order a mouse
            // has them, left to right, which is the only thing a label could
            // have said. The dismiss keeps its glyph, being the one part whose
            // position does not say what it does.
            if (b.part() == MouseOverlay.Part.DISMISS) {
                text.setTextSize(cfg.dp(14));
                text.setTextAlign(Paint.Align.CENTER);
                text.setColor(0xffe8f0ff);
                c.drawText("✕", (b.left() + b.right()) / 2f,
                        b.centreY() + text.getTextSize() / 3f, text);
                text.setTextAlign(Paint.Align.LEFT);
            }
        }

        // The wheel strip's handle: where it sits is the scroll rate.
        final MouseOverlay.Bounds s = overlay.bounds(MouseOverlay.Part.SCROLL);
        if (s != null) {
            box.set(s.left() + cfg.dp(6), overlay.handleTop(),
                    s.right() - cfg.dp(6), overlay.handleBottom());
            panel.setColor(overlay.scrollRate() != 0 ? 0xaa6a7a90 : 0x772a3240);
            c.drawRoundRect(box, cfg.dp(6), cfg.dp(6), panel);
            marker.setColor(0x90ffffff);
            c.drawRoundRect(box, cfg.dp(6), cfg.dp(6), marker);
            for (int i = -1; i <= 1; i++) {
                final float gy = box.centerY() + i * cfg.dp(9);
                c.drawLine(box.left + cfg.dp(8), gy, box.right - cfg.dp(8), gy, marker);
            }
        }
        marker.setColor(0x66ffffff);
        return false;
    }

    /**
     * Round {@link #path} into a rounded rectangle whose corners against the
     * edge of the window are square.
     *
     * <p>The button row is flush with the bottom of the screen and the wheel
     * strip with the right of it, and a 3 dp arc against a screen edge reads as
     * a slip rather than as a corner — there is a sliver of desktop in it. The
     * inner corners keep the radius, so the parts still read as separate
     * targets. The ✕ and the wheel's handle are inset from everything and so
     * come out fully round, which is the same rule and not an exception to it.
     */
    private void edged(RectF r, float radius, MouseOverlay overlay) {
        final boolean l = r.left <= 0, t = r.top <= 0;
        final boolean b = r.bottom >= overlay.viewHeight();
        final boolean g = r.right >= overlay.viewWidth();
        final float tl = (l || t) ? 0 : radius;
        final float tr = (g || t) ? 0 : radius;
        final float br = (g || b) ? 0 : radius;
        final float bl = (l || b) ? 0 : radius;
        corners[0] = corners[1] = tl;
        corners[2] = corners[3] = tr;
        corners[4] = corners[5] = br;
        corners[6] = corners[7] = bl;
        path.rewind();
        path.addRoundRect(r, corners, Path.Direction.CW);
    }

    // ---- the extension keyboard --------------------------------------------

    /**
     * The key row and its info bar.
     *
     * @param cursorY where the cursor is on screen; the info bar gets out of
     *                its way (see {@link #barFade})
     * @return whether another frame is wanted — a ripple is mid-flight
     */
    public boolean drawKeyboard(Canvas c, ExtensionKeyboard keyboard, int viewWidth,
                                float cursorY) {
        final float barTop = keyboard.infoBarTop();
        final float rowTop = keyboard.keyRowTop();
        final float rowBottom = keyboard.keyRowBottom();
        final float barMid = (barTop + rowTop) / 2;
        // Solid means the desktop is inset by the bar, so there is no cursor
        // under it to get out of the way of and nothing to see through it.
        final float fade = cfg.keyboardInfoSolid ? 1f : barFade(barTop, cursorY);

        // ---- the info bar --------------------------------------------------
        // A step lighter than the key row below it, which is black: the two are
        // one widget and the readout is the part that is not a key.
        box.set(0, barTop, viewWidth, rowTop);
        panel.setColor(cfg.keyboardInfoSolid ? 0xff101418 : alpha(0xe6101418, fade));
        c.drawRect(box, panel);

        // Status lights, one per modifier: dim is off, bright is armed, an
        // underline means locked.
        final float cell = keyboard.lightCell();
        final float ink = cfg.dp(13);
        barPaint.setTextSize(cfg.dp(14));
        float x = keyboard.lightsLeft();
        for (ExtensionKeyboard.Key m : keyboard.modifiers()) {
            final ExtensionKeyboard.Sticky s = keyboard.sticky(m);
            final int colour = alpha(s == ExtensionKeyboard.Sticky.OFF ? 0x4dffffff : 0xffe8f0ff,
                    fade);
            barPaint.setColor(colour);
            final KeyIcons.Icon icon = KeyIcons.of(m.icon());
            if (icon != null) {
                drawIcon(c, icon, x + (cell - ink) / 2, barMid - ink / 2, ink, colour);
            } else {
                // The original's info bar draws Ctrl and Alt as a bare C and A —
                // the initial is all the room there is, and all that is needed.
                final String initial = m.label().substring(0, 1);
                barPaint.getTextBounds(initial, 0, initial.length(), bounds);
                c.drawText(initial, x + cell / 2, barMid - bounds.exactCenterY(), barPaint);
            }
            if (s == ExtensionKeyboard.Sticky.LOCKED) {
                final float uy = barMid + ink / 2 + cfg.dp(2.5f);
                panel.setColor(colour);
                c.drawRect(x + (cell - ink) / 2, uy,
                        x + (cell + ink) / 2, uy + cfg.dp(1.2f), panel);
            }
            x += cell;
        }

        // The line being typed, ellipsized from the start: the tail is the part
        // you are still working on.
        barPaint.setTextAlign(Paint.Align.LEFT);
        barPaint.setTextSize(keyboard.masked() ? cfg.dp(10) : cfg.dp(11));
        barPaint.setColor(alpha(0x99ffffff, fade));
        final String info = keyboard.infoText();
        final float avail = keyboard.textRight() - keyboard.textLeft();
        int from = 0;
        while (from < info.length() && barPaint.measureText(info, from, info.length()) > avail) {
            from++;
        }
        c.drawText(info.substring(from), keyboard.textLeft(), barMid + cfg.dp(4), barPaint);
        barPaint.setTextAlign(Paint.Align.CENTER);

        barPaint.setTextSize(cfg.dp(12));
        barPaint.setColor(alpha(0x99ffffff, fade));
        for (ExtensionKeyboard.Part p : ExtensionKeyboard.Part.values()) {
            final ExtensionKeyboard.Bounds b = keyboard.part(p);
            c.drawText(p == ExtensionKeyboard.Part.DISMISS
                            ? "✕" : (keyboard.masked() ? "•••" : "abc"),
                    b.centreX(), b.centreY() + cfg.dp(4), barPaint);
        }

        // ---- the key row ---------------------------------------------------
        // Solid, unlike the info bar above it. The eye reads a key's face against
        // its background, and desktop content showing through turns forty faint
        // glyphs into forty ambiguities — worst over a light window, where the
        // labels vanish altogether. The bar above is a readout with nothing to
        // aim at, so letting the desktop through there costs nothing.
        box.set(0, rowTop, viewWidth, rowBottom);
        panel.setColor(0xff000000);
        c.drawRect(box, panel);

        boolean animating = false;
        for (ExtensionKeyboard.Bounds b : keyboard.keys()) {
            if (b.right() < 0 || b.left() > viewWidth) {
                continue;
            }
            final boolean down = keyboard.pressedKey() == b.key();
            final ExtensionKeyboard.Sticky s = keyboard.sticky(b.key());
            // The key's own bounds, not the row's: a list of more than one line
            // puts them on different lines, and the two are the same rectangle
            // only while there is one.
            box.set(b.left(), b.top(), b.right(), b.bottom());

            // Flush rectangles with no chrome of their own, as the original draws
            // them: borders and margins turn forty keys into forty slabs, and
            // the group gaps are the only division the row needs.
            if (s != ExtensionKeyboard.Sticky.OFF) {
                panel.setColor(s == ExtensionKeyboard.Sticky.LOCKED ? 0x38ffffff : 0x20ffffff);
                c.drawRect(box, panel);
            }
            if (down) {
                panel.setColor(0x1affffff);
                c.drawRect(box, panel);
            }

            animating |= drawKeyRipple(c, b);

            final int colour = s == ExtensionKeyboard.Sticky.OFF ? 0xa3ffffff : 0xf2ffffff;
            final KeyIcons.Icon icon = KeyIcons.of(b.key().icon());
            final float underlineHalf;
            if (icon != null) {
                final float size = cfg.keyboardIconWidthPx;
                drawIcon(c, icon, b.centreX() - size / 2, b.centreY() - size / 2, size, colour);
                underlineHalf = size / 2;
            } else {
                keyPaint.setColor(colour);
                c.drawText(b.key().label(), b.centreX(), b.centreY() + cfg.dp(4.5f), keyPaint);
                underlineHalf = keyPaint.measureText(b.key().label()) / 2;
            }
            if (s == ExtensionKeyboard.Sticky.LOCKED) {
                panel.setColor(colour);
                c.drawRect(b.centreX() - underlineHalf, b.centreY() + cfg.dp(8),
                        b.centreX() + underlineHalf, b.centreY() + cfg.dp(9.2f), panel);
            }
        }
        return animating;
    }

    /**
     * How opaque the info bar should be, given where the cursor is.
     *
     * <p>The desktop insets above the whole keyboard, so the bar covers no
     * remote pixels — but it does cover the <em>cursor</em>: the viewport clamps
     * the desktop into the content rect, so a cursor at the bottom edge sits on
     * the bar's top edge and an arrow drawn from its hotspot extends into it.
     * The chrome is drawn after the cursor, so the bar would swallow it exactly
     * while the pointer was being pushed downwards. Fading rather than
     * reordering, because only one of them can be on top; ramped rather than
     * switched, so it does not blink as the cursor grazes the boundary.
     */
    private float barFade(float barTop, float cursorY) {
        final float d = barTop - cursorY;
        if (d <= 0) {
            return BAR_FADE_MIN;
        }
        final float ramp = cfg.dp(BAR_FADE_DP);
        if (d >= ramp) {
            return 1f;
        }
        return BAR_FADE_MIN + (1f - BAR_FADE_MIN) * (d / ramp);
    }

    /** {@code argb} with its alpha scaled by {@code f}. */
    private static int alpha(int argb, float f) {
        final int a = Math.round(((argb >>> 24) & 0xff) * f);
        return (a << 24) | (argb & 0xffffff);
    }

    /**
     * A material-style ripple from where the finger landed, on the one key it
     * belongs to. Hand-drawn because the keyboard is not a view per key — which
     * is the whole point of the claim-based routing — so there is no
     * {@code Drawable} to hang a {@code RippleDrawable} on.
     */
    private boolean drawKeyRipple(Canvas c, ExtensionKeyboard.Bounds b) {
        if (rippleStartNanos == 0 || rippleBounds == null || rippleBounds.key() != b.key()) {
            return false;
        }
        final float t = (System.nanoTime() - rippleStartNanos) / RIPPLE_NANOS;
        if (t >= 1f) {
            rippleStartNanos = 0;
            return false;
        }
        // Fast out, slow in on the radius; linear fade, so a quick tap still
        // leaves a visible mark and a long hold does not glow for ever.
        final float grow = 1f - (1f - t) * (1f - t);
        final float max = Math.max(b.width(), b.bottom() - b.top());
        final int save = c.save();
        box.set(b.left(), b.top(), b.right(), b.bottom());
        c.clipRect(box);
        panel.setColor((int) (0x2e * (1f - t)) << 24 | 0xffffff);
        c.drawCircle(rippleX, rippleY, max * (0.35f + 0.65f * grow), panel);
        c.restoreToCount(save);
        return true;
    }

    /**
     * A {@link KeyIcons} glyph, scaled from its unit box into a {@code size}
     * square at {@code (left, top)}. Stroked icons keep their weight relative to
     * the box, so ⇧ at 13 px and ⇧ at 18 px look like the same glyph.
     */
    private void drawIcon(Canvas c, KeyIcons.Icon icon, float left, float top,
                          float size, int colour) {
        final int save = c.save();
        c.translate(left, top);
        c.scale(size, size);
        if (icon.stroke() > 0) {
            iconPaint.setStyle(Paint.Style.STROKE);
            iconPaint.setStrokeWidth(icon.stroke());
            // Round ends and corners: these are line drawings at 13–18 dp, where
            // a mitred corner on a 2 px stroke is a spike.
            iconPaint.setStrokeCap(Paint.Cap.ROUND);
            iconPaint.setStrokeJoin(Paint.Join.ROUND);
        } else {
            iconPaint.setStyle(Paint.Style.FILL);
        }
        iconPaint.setColor(colour);
        c.drawPath(icon.path(), iconPaint);
        c.restoreToCount(save);
    }

    // ---- the tap regions ---------------------------------------------------

    /**
     * The tap regions, which are invisible in earnest — drawn only so that "did
     * my finger land in the band?" is answerable while testing. Screen
     * coordinates, so this goes outside the viewport transform.
     */
    public void drawRegions(Canvas c, TapRegions regions, int viewWidth, int viewHeight) {
        text.setTextSize(cfg.dp(10));
        text.setColor(0xffe8f0ff);
        for (TapRegions.Region r : regions.regions()) {
            box.set(r.left() * viewWidth, r.top() * viewHeight,
                    r.right() * viewWidth, r.bottom() * viewHeight);
            c.drawRect(box, marker);
            // Labelled at the top of the band, not the middle: the bottom band
            // is where a HUD tends to live.
            c.drawText(r.name(), box.left + cfg.dp(6),
                    box.top + text.getTextSize() + cfg.dp(4), text);
        }
    }

    /**
     * The tap regions shown to somebody who does not know they are there.
     *
     * <p>The bands are the whole affordance this surface has: there is no
     * toolbar to notice and no button to find, which is the point of them and
     * also the one thing wrong with them. So they are drawn once, over a
     * desktop that is already up, with each band saying what it does.
     *
     * <p>Names are the region's, which are the caller's; {@code label} turns one
     * into the words a person reads, and a region it has nothing to say about is
     * outlined without one.
     *
     * @param alpha 0 to 1, so the caller owns the fade
     */
    public void drawRegionHints(Canvas c, TapRegions regions, int viewWidth, int viewHeight,
                                float alpha, Function<String, String> label) {
        final int a = Math.round(Math.max(0f, Math.min(1f, alpha)) * 255f);
        if (a == 0) {
            return;
        }
        final float inset = cfg.dp(3);
        final float radius = cfg.dp(8);
        text.setTextSize(cfg.dp(15));
        text.setTextAlign(Paint.Align.CENTER);
        for (TapRegions.Region r : regions.regions()) {
            box.set(r.left() * viewWidth + inset, r.top() * viewHeight + inset,
                    r.right() * viewWidth - inset, r.bottom() * viewHeight - inset);
            panel.setColor(0x151a20 | (a * 0xaa / 255) << 24);
            c.drawRoundRect(box, radius, radius, panel);
            marker.setColor(0xffffff | (a * 0x80 / 255) << 24);
            c.drawRoundRect(box, radius, radius, marker);

            final String words = label == null ? null : label.apply(r.name());
            if (words != null && !words.isEmpty()) {
                text.setColor(0xe8f0ff | a << 24);
                c.drawText(words, box.centerX(), box.centerY() + text.getTextSize() / 3f, text);
            }
        }
        marker.setColor(0x66ffffff);
        text.setTextAlign(Paint.Align.LEFT);
    }

    // ---- the cursor --------------------------------------------------------

    /**
     * {@code CursorView.b} — cap the bitmap at 32 logical px and apply the
     * hotspot, which arrives here as a positive offset into the bitmap. Callers
     * that get it from a protocol may have to negate it: RFB carries a cursor
     * as a rectangle whose origin is already {@code -hot}.
     */
    public void drawCursor(Canvas c, android.graphics.Bitmap shape, float hotX, float hotY,
                           float screenX, float screenY, Paint bitmapPaint) {
        if (shape == null || shape.isRecycled()) {
            return;
        }
        float k = Math.max(cfg.density, 1.0f);
        final int longest = Math.max(shape.getWidth(), shape.getHeight());
        if (longest > 32) {
            k *= 32.0f / longest;
        }
        matrix.reset();
        matrix.preScale(k, k);
        matrix.postTranslate(screenX - hotX * k, screenY - hotY * k);
        c.drawBitmap(shape, matrix, bitmapPaint);
    }
}
