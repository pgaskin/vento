// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.input;

import java.util.ArrayList;
import java.util.List;

/**
 * The mouse button / wheel overlay: an L of touch targets in the bottom-right
 * corner that presses real mouse buttons while the rest of the touch surface
 * keeps driving the cursor. Ported from RealVNC's {@code ui.MouseButtons} and
 * {@code ui.ScrollButton}; {@code ARCHITECTURE.md} §1.12 is the specification.
 *
 * <p>It exists because the gesture layer can only hold a button for as long as
 * the 250 ms window in {@link GestureRecognizer} allows, and because a
 * three-finger tap is a poor way to middle-click. Hold a button here with one
 * finger and drag with another and the drag lasts as long as you want.
 *
 * <p>This class is the <em>model</em> and draws nothing: the view asks it for
 * {@link #parts()} and {@link #pressed(Part)} and paints them.
 *
 * <p>Touches reach it through {@link TouchRouter.Claim}: a pointer landing on
 * the overlay is claimed for its whole life and never reaches the gesture
 * layer, and every other pointer is untouched. The original arrives at the same
 * place from the other end — its buttons are real {@code View}s, and
 * {@code InterceptingRelativeLayout} has an explicit special case re-routing a
 * second finger on the desktop into the gesture layer while the first is here.
 *
 * <p>The button mask is not owned here: the {@link MouseSink} to hand it is one
 * that unions with the gesture layer's rather than overwriting it (see
 * {@code CursorController.newButtonSource}), or the gesture layer's own
 * press-and-release of {@code LEFT} for a tap would release a {@code LEFT} this
 * overlay is holding.
 */
public final class MouseOverlay implements TouchRouter.Claim {

    /** Redraw hook: geometry, press states or the handle position changed. */
    public interface Listener {
        void overlayChanged();
    }

    /** The overlay's touch targets. */
    public enum Part {
        LEFT(Button.LEFT), MIDDLE(Button.MIDDLE), RIGHT(Button.RIGHT),
        DISMISS(null), // the corner ✕
        SCROLL(null);  // the wheel strip up the right-hand edge

        private final Button button;

        Part(Button button) {
            this.button = button;
        }

        /** The button this part presses, or {@code null} if it presses none. */
        public Button button() {
            return button;
        }
    }

    /** One target, in view pixels. */
    public record Bounds(Part part, float left, float top, float right, float bottom) {
        public boolean contains(float x, float y) {
            return x >= left && x < right && y >= top && y < bottom;
        }

        public float width() {
            return right - left;
        }

        public float height() {
            return bottom - top;
        }

        public float centreY() {
            return (top + bottom) / 2.0f;
        }
    }

    private final Config cfg;
    private final MouseSink sink;
    private final Scheduler scheduler;
    private Listener listener;

    private int viewW, viewH;
    private final List<Bounds> parts = new ArrayList<>();
    private boolean visible;

    // Claimed pointers: which part each one is on. Several at once is the point
    // — a thumb on LEFT and a finger on the strip is a legitimate combination.
    private final int[] ptrId = new int[TouchFrame.MAX_POINTERS];
    private final Part[] ptrPart = new Part[TouchFrame.MAX_POINTERS];
    private int ptrCount;

    /** Buttons this overlay currently holds down. */
    private int heldMask;

    // ---- the wheel strip --------------------------------------------------

    private int scrollPtr = -1; // pointer driving the strip: only the first to land on it
    private float rate;         // signed wheel rate, negative is up, 0 when not scrolling
    private int ticks;          // since the last click; starts negative for the repeat delay
    private boolean ticking;
    private float handleY;      // centre of the drag handle; the strip's middle at rest

    private final Runnable wheelTick = new Runnable() {
        @Override
        public void run() {
            ticking = false;
            if (rate == 0.0f) {
                return;
            }
            ticks++;
            if (ticks >= cfg.overlayWheelTicksPerClick / Math.abs(rate)) {
                click();
                ticks = 0;
            }
            schedule();
        }
    };

    public MouseOverlay(Config cfg, MouseSink sink, Scheduler scheduler) {
        this.cfg = cfg;
        this.sink = sink;
        this.scheduler = scheduler;
    }

    public void setListener(Listener l) {
        this.listener = l;
    }

    public void setViewSize(int w, int h) {
        if (w == viewW && h == viewH) {
            return;
        }
        viewW = w;
        viewH = h;
        layout();
        changed();
    }

    /** The window the parts were laid out in, so a drawer can tell flush from inset. */
    public int viewWidth() {
        return viewW;
    }

    public int viewHeight() {
        return viewH;
    }

    public boolean visible() {
        return visible;
    }

    public void toggle() {
        setVisible(!visible);
    }

    /**
     * Hiding releases everything: a button held by a widget that is no longer
     * on the screen is stuck forever, and the wheel would keep repeating.
     */
    public void setVisible(boolean v) {
        if (visible == v) {
            return;
        }
        visible = v;
        if (!visible) {
            releaseAll();
        }
        changed();
    }

    /** The targets, in draw order. Empty until the view has a size. */
    public List<Bounds> parts() {
        return parts;
    }

    public Bounds bounds(Part p) {
        for (Bounds b : parts) {
            if (b.part() == p) {
                return b;
            }
        }
        return null;
    }

    /** Is a finger on this part right now? Buttons draw themselves pressed. */
    public boolean pressed(Part p) {
        for (int i = 0; i < ptrCount; i++) {
            if (ptrPart[i] == p) {
                return true;
            }
        }
        return false;
    }

    /** Buttons this overlay is holding down, for the HUD and for bump scroll. */
    public int heldMask() {
        return heldMask;
    }

    /**
     * What the overlay covers, for the viewport's insets: the desktop should
     * centre and clamp inside the part of the window that is still visible, or
     * the cursor ends up behind the buttons. Zero while hidden.
     *
     * <p>The original insets by a constant — the <em>extension keyboard's</em>
     * height — whichever overlay is up, so its own button row overlaps live
     * desktop by 26 dp and its wheel strip by all 60.
     */
    public float insetRightPx() {
        return visible ? cfg.overlayStripWidthPx : 0;
    }

    public float insetBottomPx() {
        return visible ? cfg.overlayRowHeightPx : 0;
    }

    /** Signed wheel rate, in clicks per {@code 3 × tick}; 0 when idle. */
    public float scrollRate() {
        return rate;
    }

    public float handleTop() {
        return handleY - handleHeight() / 2.0f;
    }

    public float handleBottom() {
        return handleY + handleHeight() / 2.0f;
    }

    /**
     * Big enough to grab without aiming, and it grows with the strip:
     * {@code stripHeight/3 + 40 dp}, the original's formula (minus its rounding
     * of the result to a multiple of 10 px, which is a layout artefact).
     */
    public float handleHeight() {
        final Bounds s = bounds(Part.SCROLL);
        return s == null ? 0 : s.height() / 3.0f + cfg.dp(40);
    }

    // ---- layout -----------------------------------------------------------

    /**
     * The original's layout: a strip up the right edge and a row across the
     * bottom holding left / middle / right in weights 2:1:2 plus the dismiss
     * button. Together they are an L, so the top-left of the screen — where the
     * dragging happens — stays clear.
     */
    private void layout() {
        parts.clear();
        if (viewW <= 0 || viewH <= 0) {
            return;
        }
        final float rowTop = viewH - cfg.overlayRowHeightPx;

        parts.add(new Bounds(Part.SCROLL,
                viewW - cfg.overlayStripWidthPx, 0, viewW, rowTop));

        // The dismiss button and its end margin come off the row first; the
        // three buttons share what is left, 2:1:2.
        final float row = viewW - cfg.overlayDismissPx - cfg.overlayDismissMarginPx;
        final float mid = Math.max(cfg.overlayMiddleMinPx, row / 5.0f);
        final float side = Math.max(0, (row - mid) / 2.0f);
        parts.add(new Bounds(Part.LEFT, 0, rowTop, side, viewH));
        parts.add(new Bounds(Part.MIDDLE, side, rowTop, side + mid, viewH));
        parts.add(new Bounds(Part.RIGHT, side + mid, rowTop, row, viewH));

        final float dc = viewH - cfg.overlayRowHeightPx / 2.0f;
        parts.add(new Bounds(Part.DISMISS, row,
                dc - cfg.overlayDismissPx / 2.0f,
                row + cfg.overlayDismissPx,
                dc + cfg.overlayDismissPx / 2.0f));

        centreHandle();
    }

    private void centreHandle() {
        final Bounds s = bounds(Part.SCROLL);
        handleY = s == null ? 0 : s.centreY();
    }

    // ---- TouchRouter.Claim ------------------------------------------------

    @Override
    public boolean claimTouch(int id, float x, float y, long t) {
        if (!visible || ptrCount == ptrId.length) {
            return false;
        }
        final Part hit = hit(x, y);
        if (hit == null) {
            return false;
        }
        ptrId[ptrCount] = id;
        ptrPart[ptrCount] = hit;
        ptrCount++;

        if (hit.button() != null) {
            press(hit.button());
        } else if (hit == Part.SCROLL && scrollPtr < 0) {
            scrollPtr = id;
            // Rate from where the finger landed, so a tap anywhere on the strip
            // is exactly one wheel click. The original waits for movement, and
            // only from a finger that landed on the handle itself.
            drag(y);
        }
        changed();
        return true;
    }

    @Override
    public void claimMoved(int id, float x, float y, long t) {
        final int i = indexOf(id);
        if (i < 0) {
            return;
        }
        if (ptrPart[i] == Part.SCROLL && id == scrollPtr) {
            drag(y);
            changed();
        }
        // A finger sliding off a button keeps it down, as a Button does: the
        // release is what matters, and a thumb resting on a button drifts.
    }

    @Override
    public void claimEnded(int id, float x, float y, long t) {
        final int i = indexOf(id);
        if (i < 0) {
            return;
        }
        final Part part = ptrPart[i];
        forget(i);

        if (part.button() != null) {
            release(part.button());
        } else if (part == Part.SCROLL && id == scrollPtr) {
            stopScrolling();
        } else if (part == Part.DISMISS) {
            // Only if the finger is still on it, like an ordinary click.
            final Bounds b = bounds(Part.DISMISS);
            if (b != null && b.contains(x, y)) {
                setVisible(false);
            }
        }
        changed();
    }

    @Override
    public void claimCancelled(long t) {
        releaseAll();
        changed();
    }

    private Part hit(float x, float y) {
        for (Bounds b : parts) {
            if (b.contains(x, y)) {
                return b.part();
            }
        }
        return null;
    }

    // ---- buttons ----------------------------------------------------------

    private void press(Button b) {
        if ((heldMask & b.mask()) == 0) {
            heldMask |= b.mask();
            sink.mouseDown(b.mask());
        }
    }

    private void release(Button b) {
        // Another finger may be on the same button; the last one out releases.
        for (int i = 0; i < ptrCount; i++) {
            if (ptrPart[i].button() == b) {
                return;
            }
        }
        if ((heldMask & b.mask()) != 0) {
            heldMask &= ~b.mask();
            sink.mouseUp(b.mask());
        }
    }

    private void releaseAll() {
        ptrCount = 0;
        stopScrolling();
        if (heldMask != 0) {
            final int m = heldMask;
            heldMask = 0;
            sink.mouseUp(m);
        }
    }

    // ---- the wheel strip --------------------------------------------------

    /**
     * Position on the strip <em>is</em> the scroll rate: the middle is stopped,
     * the ends are {@code overlayWheelMaxRate} clicks per
     * {@code overlayWheelTicksPerClick} ticks. The rate is re-read on every
     * move, so it is a throttle rather than a per-pixel wheel — which is why
     * the strip does not need the gesture layer's 8 px-per-click rule.
     */
    private void drag(float y) {
        final Bounds s = bounds(Part.SCROLL);
        if (s == null || s.height() <= 0) {
            return;
        }
        handleY = clamp(y, s.top() + handleHeight() / 2.0f,
                s.bottom() - handleHeight() / 2.0f);
        final float half = s.height() / 2.0f;
        setRate(clamp((y - s.centreY()) / half, -1.0f, 1.0f) * cfg.overlayWheelMaxRate);
    }

    private void setRate(float r) {
        if (rate == 0.0f && r != 0.0f) {
            // First click immediately, then a pause before the repeat starts —
            // key repeat, so a single click is easy to land.
            click(r);
            ticks = -cfg.overlayWheelStartDelayTicks;
            rate = r;
            schedule();
        } else if (r == 0.0f) {
            // The finger is still on the strip — it is at the middle, where the
            // rate is zero. Anything that forgot which pointer owned the strip
            // here would stop reading it for the rest of the gesture, and a
            // slow drag through the centre lands on it exactly.
            pauseScrolling();
        } else {
            rate = r;
        }
    }

    /** The finger has gone. */
    private void stopScrolling() {
        scrollPtr = -1;
        pauseScrolling();
    }

    /** Scrolling stops; the finger stays. */
    private void pauseScrolling() {
        rate = 0;
        ticks = 0;
        if (ticking) {
            ticking = false;
            scheduler.removeCallbacks(wheelTick);
        }
        centreHandle();
    }

    private void schedule() {
        if (!ticking) {
            ticking = true;
            scheduler.postDelayed(wheelTick, cfg.overlayWheelTickMs);
        }
    }

    private void click() {
        click(rate);
    }

    private void click(float r) {
        final Button b = r < 0 ? Button.WHEEL_UP : Button.WHEEL_DOWN;
        sink.mouseDown(b.mask());
        sink.mouseUp(b.mask());
    }

    // ---- pointer bookkeeping ----------------------------------------------

    private int indexOf(int id) {
        for (int i = 0; i < ptrCount; i++) {
            if (ptrId[i] == id) {
                return i;
            }
        }
        return -1;
    }

    private void forget(int i) {
        ptrCount--;
        ptrId[i] = ptrId[ptrCount];
        ptrPart[i] = ptrPart[ptrCount];
    }

    private void changed() {
        if (listener != null) {
            listener.overlayChanged();
        }
    }

    private static float clamp(float v, float lo, float hi) {
        return v < lo ? lo : (v > hi ? hi : v);
    }
}
