// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.input;

import java.util.ArrayList;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Set;

/**
 * A column of buttons over the desktop, on the left edge, that can be dragged up
 * and down: the affordance {@link TapRegions} deliberately is not.
 *
 * <p>The two are alternatives to each other and never meet — the regions are a
 * classification of <em>taps</em> inside the gesture layer, and this claims
 * pointers before the gesture layer sees them — so nothing arbitrates between
 * them and a host may run either, or both. What they do is the same, because
 * both hand the host a name and the host has one place it answers them.
 *
 * <p>This class is the <em>model</em> and draws nothing: the view asks for
 * {@link #items()} and {@link #grip()} and paints them.
 *
 * <p><b>It does not inset the desktop.</b> The picture runs underneath and the
 * pointer can reach the pixels behind it, which is the floating info bar's
 * bargain and the reason a drawer is expected to fade it as the pointer nears.
 * What that costs is the left edge's bump scroll wherever the column has been
 * dragged to, which is the second reason it moves.
 */
public final class Toolbar implements TouchRouter.Claim {

    /**
     * One button. {@code name} is the host's word — this package says nothing
     * about what any of them mean, exactly as it says nothing about what a tap
     * region means — and {@code icon} names a glyph the renderer may know.
     */
    public record Item(String name, String icon) {
    }

    public interface Listener {
        void toolbarChanged();

        /** A button was released on the item it started on. */
        void toolbarAction(String name);

        /**
         * The column was dragged and let go of, at this fraction of the band it
         * may occupy. Called on the release rather than on every move, since
         * what a host does with it is write it down.
         */
        default void toolbarMoved(float fraction) {
        }
    }

    /** One target, in view pixels; {@code item} is null for the grip. */
    public record Bounds(Item item, float left, float top, float right, float bottom) {
        public boolean contains(float x, float y) {
            return x >= left && x < right && y >= top && y < bottom;
        }

        public float width() {
            return right - left;
        }

        public float height() {
            return bottom - top;
        }

        public float centreX() {
            return (left + right) / 2.0f;
        }

        public float centreY() {
            return (top + bottom) / 2.0f;
        }
    }

    private final Config cfg;
    private Listener listener;

    private List<Item> defs = List.of();
    private final List<Bounds> items = new ArrayList<>();
    private final Set<String> active = new LinkedHashSet<>();
    private Bounds grip;

    private int viewW, viewH;
    private float insetLeft, insetTop, insetBottom;
    private boolean visible;

    /** Where the column sits in the band it may occupy, 0 at the top. */
    private float fraction = 0.35f;

    private static final int NONE = Integer.MIN_VALUE;

    private int activeId = NONE;
    private Item activeItem;
    private boolean dragging;
    private float pressY, pressTop;
    private long spare;         // extra fingers: claimed so they cannot reach the pad, then ignored

    public Toolbar(Config cfg) {
        this.cfg = cfg;
    }

    /**
     * The four the tap regions already declare, in the order a column of them
     * reads: what each one does is the host's business.
     */
    public static List<Item> standard() {
        return List.of(
                new Item(TapRegions.DISCONNECT, "disconnect"),
                new Item(TapRegions.INFORMATION, "info"),
                new Item(TapRegions.KEYBOARD, "keyboard"),
                new Item(TapRegions.MOUSE, "mouse"));
    }

    public void setListener(Listener l) {
        this.listener = l;
    }

    /**
     * Which buttons there are. A shorter list is how a session that cannot do
     * something offers three buttons rather than four greyed ones.
     */
    public void setItems(List<Item> defs) {
        if (this.defs.equals(defs)) {
            return;   // a press under the finger is not to be cancelled for nothing
        }
        this.defs = List.copyOf(defs);
        release();
        layout();
        changed();
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

    /**
     * What the column may not sit in: the left edge's own inset, whatever is at
     * the top of the window, and whatever covers the bottom of it — the
     * extension keyboard, in practice, which is a thing drawn over the desktop
     * rather than an inset of it.
     */
    public void setInsets(float left, float top, float bottom) {
        if (left == insetLeft && top == insetTop && bottom == insetBottom) {
            return;
        }
        insetLeft = left;
        insetTop = top;
        insetBottom = bottom;
        layout();
        changed();
    }

    public boolean visible() {
        return visible;
    }

    public void setVisible(boolean v) {
        if (visible == v) {
            return;
        }
        visible = v;
        if (!visible) {
            release();
        }
        changed();
    }

    /**
     * Mark a button as showing something that is on right now — the keyboard is
     * up, the mouse overlay is out — so that it can be drawn the way the info
     * bar draws an armed modifier. Which of them have a state at all is the
     * host's business, as their names are.
     */
    public void setActive(String name, boolean on) {
        if (on ? active.add(name) : active.remove(name)) {
            changed();
        }
    }

    public boolean active(Item item) {
        return item != null && active.contains(item.name());
    }

    /** Where the column sits, for a host that remembers it between sessions. */
    public float position() {
        return fraction;
    }

    public void setPosition(float f) {
        fraction = clamp(f, 0, 1);
        layout();
        changed();
    }

    // ---- what the view draws ----------------------------------------------

    public List<Bounds> items() {
        return items;
    }

    /** The handle under the buttons, away from the one with no undo. */
    public Bounds grip() {
        return grip;
    }

    /** The item under a finger right now, for drawing it pressed. */
    public Item pressedItem() {
        return dragging ? null : activeItem;
    }

    /** True while a finger is on the column, dragging it or not. */
    public boolean touched() {
        return activeId != NONE;
    }

    public boolean dragging() {
        return dragging;
    }

    public float left() {
        return insetLeft;
    }

    public float right() {
        return insetLeft + cfg.toolbarButtonPx;
    }

    public float top() {
        return insetTop + fraction * band();
    }

    public float bottom() {
        return top() + height();
    }

    /** The whole column, buttons and grip. Zero while there is nothing in it. */
    public float height() {
        return defs.isEmpty() ? 0 : defs.size() * cfg.toolbarButtonPx + cfg.toolbarGripPx;
    }

    // ---- layout ------------------------------------------------------------

    /** How much room there is to move the column in, which may be none. */
    private float band() {
        return Math.max(0, viewH - insetTop - insetBottom - height());
    }

    private void layout() {
        items.clear();
        grip = null;
        if (viewW <= 0 || viewH <= 0 || defs.isEmpty()) {
            return;
        }
        final float l = left(), r = right();
        float y = top();
        for (Item d : defs) {
            items.add(new Bounds(d, l, y, r, y + cfg.toolbarButtonPx));
            y += cfg.toolbarButtonPx;
        }
        grip = new Bounds(null, l, y, r, y + cfg.toolbarGripPx);
    }

    // ---- TouchRouter.Claim -------------------------------------------------

    @Override
    public boolean claimTouch(int id, float x, float y, long t) {
        if (!visible || defs.isEmpty()
                || x < left() || x >= right() || y < top() || y >= bottom()) {
            return false;
        }
        if (activeId != NONE) {
            // A second finger on the column: claimed so it cannot reach the
            // touchpad behind it, and then ignored.
            spare |= bit(id);
            return true;
        }
        activeId = id;
        activeItem = itemAt(x, y);
        // The grip is a hint rather than a requirement — a pointer that lands on
        // a button and travels drags too (see claimMoved) — but one that lands
        // on the grip is dragging from the first pixel, since there is nothing
        // else it could be doing.
        dragging = activeItem == null;
        pressY = y;
        pressTop = top();
        changed();
        return true;
    }

    @Override
    public void claimMoved(int id, float x, float y, long t) {
        if (id != activeId) {
            return;
        }
        if (!dragging && Math.abs(y - pressY) > cfg.toolbarDragSlopPx) {
            dragging = true;
            activeItem = null;  // it is a drag now, and the button does not fire
        }
        if (dragging) {
            final float b = band();
            fraction = b <= 0 ? 0
                    : clamp((pressTop + (y - pressY) - insetTop) / b, 0, 1);
            layout();
            changed();
        }
    }

    @Override
    public void claimEnded(int id, float x, float y, long t) {
        if (spare != 0 && (spare & bit(id)) != 0) {
            spare &= ~bit(id);
            return;
        }
        if (id != activeId) {
            return;
        }
        final Item item = activeItem;
        final boolean wasDragging = dragging;
        activeId = NONE;
        activeItem = null;
        dragging = false;

        if (wasDragging) {
            if (listener != null) {
                listener.toolbarMoved(fraction);
            }
        } else if (item != null && item == itemAt(x, y) && listener != null) {
            // Still on the button it started on, which is the extension row's
            // rule and an ordinary click's: lifting somewhere else is how a
            // mis-aimed button is abandoned.
            listener.toolbarAction(item.name());
        }
        changed();
    }

    @Override
    public void claimCancelled(long t) {
        release();
        changed();
    }

    private void release() {
        activeId = NONE;
        activeItem = null;
        dragging = false;
        spare = 0;
    }

    private Item itemAt(float x, float y) {
        for (Bounds b : items) {
            if (b.contains(x, y)) {
                return b.item();
            }
        }
        return null;
    }

    private static long bit(int id) {
        return id >= 0 && id < 64 ? 1L << id : 0;
    }

    private void changed() {
        if (listener != null) {
            listener.toolbarChanged();
        }
    }

    private static float clamp(float v, float lo, float hi) {
        return v < lo ? lo : (v > hi ? hi : v);
    }
}
