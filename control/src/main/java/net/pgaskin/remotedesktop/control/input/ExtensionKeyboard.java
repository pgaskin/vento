// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.input;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.HashMap;
import java.util.IdentityHashMap;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;

/**
 * The extension keyboard: the row of keys a physical keyboard has and a soft
 * one does not — modifiers, Esc, Tab, the arrows, the F-keys — plus the info bar
 * above it. Ported from RealVNC's {@code ui.ExtensionKeyboard},
 * {@code ui.KeyboardKey} and {@code ui.InfoBar}, with the mechanics of
 * cmus-android's {@code KeyRow}.
 *
 * <p>It is the companion to the system IME, not a replacement for it: letters
 * come from the soft keyboard, and this row supplies what the soft keyboard has
 * no concept of. That is why {@link #modifiers() modifier} keys are
 * <em>held at the far end</em> rather than folded into a character — the Ctrl
 * of Ctrl+C comes from here and the C comes from the IME, and the only thing
 * that can join them is the remote.
 *
 * <p>This class is the <em>model</em> and draws nothing: the view asks for
 * {@link #keys()}, {@link #sticky} and {@link #infoText()} and paints them.
 * Touches reach it through {@link TouchRouter.Claim}, so a finger on a key never
 * reaches the gesture layer and every other finger keeps driving the cursor.
 *
 * <p><b>The key list is supplied by the caller.</b> {@link #standardKeys()} is
 * RealVNC's, which is an X11 set; protocols differ in which modifiers they even
 * have (there is no Option on an RDP session, and Super and Meta are not
 * everywhere the same key), so the backend that knows picks the list. Nothing
 * here is keyed on a particular keysym.
 */
public final class ExtensionKeyboard implements TouchRouter.Claim {

    /** Things worth feeling. What each one feels like is the view's business. */
    public enum Feedback {
        PRESS,  // a key tapped, or a modifier armed
        LOCK,   // a modifier locked, by a hold or by two taps
        REPEAT  // one repeat of a held key: up to thirteen a second, so the lightest tick there is
    }

    /** Redraw hook, plus the haptics the view is the only one able to play. */
    public interface Listener {
        void keyboardChanged();

        default void keyFeedback(Feedback what) {
        }

        /** A {@link Key#action} key was tapped. The name is the caller's own. */
        default void keyAction(String name) {
        }

        /**
         * A modifier key on this row was tapped, whatever it did to its state.
         * Separate from {@link #keyboardChanged} because the resulting state
         * cannot tell a press apart from a modifier being consumed by the key
         * after it, and a host may want to act on the press.
         */
        default void modifierPressed(Key key) {
        }
    }

    /** The one action {@link #standardKeys()} carries; the host implements it. */
    public static final String ACTION_PASTE = "paste";

    /** Measures a label, since a plain-JVM model cannot. */
    public interface LabelWidth {
        float measure(String label);
    }

    /**
     * One key. {@code group} only separates: keys with different group numbers
     * get a gap between them. {@code row} is which line of the group it sits on,
     * where a group of more than one line is laid out as a grid (§{@link
     * #layout()}). {@code wide} is roomier padding, for keys whose
     * label is short enough to come out a sliver. {@code icon} names a glyph the
     * renderer may know — an arrow, ⇧, ⌘ — and stays a <em>name</em> because
     * this package must not know what a bitmap is; a renderer that does not
     * recognise it falls back to the label, so an unknown icon costs nothing.
     *
     * <p>{@code row} is a layout attribute exactly as {@code group} and
     * {@code wide} are, and the key list stays <em>flat</em> because a key's
     * position in that list is its state slot and its id at the far end: a list
     * of lists would make all three two-dimensional for the sake of one of them.
     *
     * <p>{@code action} makes the key mean something this package has no idea
     * about: instead of a keysym it reports the name to the listener, and the
     * app decides. Same shape as {@link TapRegions}' caller-supplied names, and
     * for the same reason — Paste has to read a clipboard and put a dialog on
     * screen, neither of which belongs in a plain-JVM model.
     */
    public record Key(String label, String icon, int keysym,
                      boolean modifier, boolean repeats, int group, int row, boolean wide,
                      String action) {

        public Key(String label, String icon, int keysym,
                   boolean modifier, boolean repeats, int group, boolean wide) {
            this(label, icon, keysym, modifier, repeats, group, 0, wide, null);
        }

        /**
         * The same key, held-to-repeat. Only where holding means "again, and
         * again": on Esc, Enter, Ins or an F-key a hold that turns into eight
         * presses is a mistake, and there is no undo at the far end.
         */
        public Key repeating() {
            return new Key(label, icon, keysym, modifier, true, group, row, wide, action);
        }

        /** The same key, on line {@code r} of its group. */
        public Key onRow(int r) {
            return new Key(label, icon, keysym, modifier, repeats, group, r, wide, action);
        }

        public static Key normal(String label, int keysym, int group) {
            return new Key(label, null, keysym, false, false, group, false);
        }

        public static Key wide(String label, int keysym, int group) {
            return new Key(label, null, keysym, false, false, group, true);
        }

        public static Key icon(String label, String icon, int keysym, int group) {
            return new Key(label, icon, keysym, false, false, group, true);
        }

        public static Key modifier(String label, String icon, int keysym, int group) {
            return new Key(label, icon, keysym, true, false, group, false);
        }

        /**
         * A key that asks the host to do something rather than sending anything.
         * {@code name} is the host's word, not this package's.
         */
        public static Key action(String label, String icon, String name, int group) {
            return new Key(label, icon, 0, false, false, group, 0, icon != null, name);
        }
    }

    /**
     * Modifier state. A tap arms {@link #ONESHOT}, which survives exactly one
     * key and then lets go; a long press {@link #LOCKED}s it until it is tapped
     * again. In both states the modifier is held down at the remote.
     */
    public enum Sticky {OFF, ONESHOT, LOCKED}

    /** A laid-out key, in view pixels; {@code left} already includes the scroll. */
    public record Bounds(Key key, float left, float top, float right, float bottom) {
        public boolean contains(float x, float y) {
            return x >= left && x < right && y >= top && y < bottom;
        }

        public float width() {
            return right - left;
        }

        public float centreX() {
            return (left + right) / 2.0f;
        }

        public float centreY() {
            return (top + bottom) / 2.0f;
        }
    }

    /** The info bar's own touch targets, right to left. */
    public enum Part {
        MASK,   // show the typed text, or mask it as dots
        DISMISS // hide the whole thing
    }

    private final Config cfg;
    private final KeySink sink;
    private final Scheduler scheduler;
    private List<Key> keyList;
    /**
     * Where each key sits in {@link #keyList}, which is its state slot and its
     * id at the far end. A map rather than {@code indexOf}: the row is drawn
     * every frame and each key's state is asked for as it is drawn, and — since
     * the list is the caller's — two keys that happen to be equal would
     * otherwise share one slot and one id.
     */
    private final IdentityHashMap<Key, Integer> keyIndex = new IdentityHashMap<>();
    private final List<Key> modifiers = new ArrayList<>();
    private Sticky[] state;
    private int rows = 1;   // lines of keys: max(Key.row) + 1
    private Listener listener;
    private LabelWidth labelWidth = ExtensionKeyboard::estimateWidth;

    private int viewW, viewH;
    private float bottomOffset; // what already covers the bottom: the system IME
    private boolean visible;
    private boolean masked;

    private final List<Bounds> keys = new ArrayList<>();
    private float contentWidth;
    private float scrollX;

    // Fling: a smoothed drag velocity, and the glide it leaves behind.
    private float dragVelocity;
    private long lastMoveTime;
    private float flingVelocity;

    /** The line being typed, as the original's info bar accumulates it. */
    private final StringBuilder typed = new StringBuilder();

    /** keysym → the key id its press was sent under; see {@link #set}. */
    private final Map<Integer, Integer> heldIds = new HashMap<>();

    // ---- the pointer driving the keyboard, if any -------------------------

    private static final int NONE = Integer.MIN_VALUE;

    private int activeId = NONE;
    private Key activeKey;
    private Part activePart;
    private float pressX, pressY, lastX;
    private boolean scrolling;
    private boolean consumed;   // a long press decided; the lift does nothing
    private long spare;         // extra fingers: claimed so they cannot reach the pad, then ignored
    private Key lastTapKey;     // for the double tap that locks a modifier
    private long lastTapTime;

    private final Runnable longPress = this::onLongPress;
    private final Runnable repeat = new Runnable() {
        @Override
        public void run() {
            if (activeKey == null || !activeKey.repeats()) {
                return;
            }
            // Only now has the hold produced anything, so only now must the
            // lift stop producing one of its own.
            consumed = true;
            fire(activeKey);
            feedback(Feedback.REPEAT);
            scheduler.postDelayed(this, cfg.keyRepeatMs);
        }
    };
    private final Runnable fling = new Runnable() {
        @Override
        public void run() {
            scrollBy(flingVelocity * cfg.keyboardFlingTickMs);
            flingVelocity *= cfg.keyboardFlingDecay;
            if (Math.abs(flingVelocity) < cfg.keyboardFlingStopPx || atScrollLimit()) {
                flingVelocity = 0;
            } else {
                scheduler.postDelayed(this, cfg.keyboardFlingTickMs);
            }
            changed();
        }
    };

    public ExtensionKeyboard(Config cfg, KeySink sink, Scheduler scheduler, List<Key> keys) {
        this.cfg = cfg;
        this.sink = sink;
        this.scheduler = scheduler;
        adopt(keys);
    }

    /**
     * Swap the key list of a keyboard that is already running, so that a host
     * offering a choice of layouts does not have to reconnect to apply one.
     *
     * <p><b>Every held modifier is let go of first, through the old list.</b> A
     * key id is a position in that list, so re-indexing before releasing would
     * send a key-up for an id the far end never saw a key-down for — and leave a
     * modifier held down on somebody's machine for the rest of the session. The
     * active touch, the timers and the fling go with it, since each of them
     * holds a {@link Key} that may not be in the new list.
     */
    public void setKeys(List<Key> keys) {
        clearModifiers();
        release();
        heldIds.clear();
        adopt(keys);
        layout();
        changed();
    }

    /** Take the list as it is: state, index, modifiers and the line count. */
    private void adopt(List<Key> keys) {
        this.keyList = List.copyOf(keys);
        this.state = new Sticky[this.keyList.size()];
        this.rows = 1;
        keyIndex.clear();
        modifiers.clear();
        for (int i = 0; i < this.keyList.size(); i++) {
            keyIndex.put(this.keyList.get(i), i);
        }
        for (int i = 0; i < state.length; i++) {
            state[i] = Sticky.OFF;
            final Key k = this.keyList.get(i);
            if (k.modifier()) {
                modifiers.add(k);
            }
            rows = Math.max(rows, k.row() + 1);
        }
    }

    /**
     * RealVNC's key set, in its display grouping. Windows and CMD are both
     * {@code XK_Super_L}: the two labels are for two remote operating systems,
     * and the original ships both and lets you pick by eye.
     */
    public static List<Key> standardKeys() {
        final List<Key> k = new ArrayList<>();
        // Labels, icons and grouping are the original's, key for key.
        k.add(Key.modifier("Shift", "shift", Keysym.SHIFT_L, 0));
        k.add(Key.modifier("Ctrl", null, Keysym.CONTROL_L, 0));
        k.add(Key.modifier("Alt", null, Keysym.ALT_L, 0));
        k.add(Key.modifier("Windows", "windows", Keysym.SUPER_L, 0));
        k.add(Key.modifier("Option", "option", Keysym.ISO_LEVEL3_SHIFT, 0));
        k.add(Key.modifier("CMD", "command", Keysym.SUPER_L, 0));

        k.add(Key.icon("Backspace", "backspace", Keysym.BACKSPACE, 1).repeating());
        k.add(Key.normal("Del", Keysym.DELETE, 1).repeating());
        k.add(Key.normal("Esc", Keysym.ESCAPE, 1));
        k.add(Key.normal("Tab", Keysym.TAB, 1).repeating());
        k.add(Key.normal("Ins", Keysym.INSERT, 1));

        k.add(Key.icon("Enter", "return", Keysym.RETURN, 2));

        k.add(Key.icon("Up", "arrow_up", Keysym.UP, 3).repeating());
        k.add(Key.icon("Down", "arrow_down", Keysym.DOWN, 3).repeating());
        k.add(Key.icon("Left", "arrow_left", Keysym.LEFT, 3).repeating());
        k.add(Key.icon("Right", "arrow_right", Keysym.RIGHT, 3).repeating());

        // Ours: the host types the phone's clipboard out, which works against a
        // remote whose own clipboard we cannot reach. A group to itself, so it
        // has a gap either side — it is the one key here the row cannot undo.
        k.add(Key.action("Paste", "paste", ACTION_PASTE, 4));

        k.add(Key.normal("Home", Keysym.HOME, 5));
        k.add(Key.normal("End", Keysym.END, 5));
        k.add(Key.normal("Page Up", Keysym.PAGE_UP, 5).repeating());
        k.add(Key.normal("Page Down", Keysym.PAGE_DOWN, 5).repeating());

        for (int i = 1; i <= 12; i++) {
            k.add(Key.wide("F" + i, Keysym.f(i), 6));
        }
        return k;
    }

    /**
     * The same keys in two lines, grouped the way a keyboard groups them:
     *
     * <pre>
     *   bksp del esc tab ins     home  up   end      pgup    paste    f1-f6
     *   modifiers                left  down right    pgdn    return   f7-f12
     * </pre>
     *
     * <p>The arrows are why the columns of a group are shared between its lines
     * rather than each line being laid out on its own: sharing them puts
     * {@code home up end} over {@code left down right} as an inverted T, with
     * Home above Left and End above Right — the two keys that mean "the far end
     * of this line, that way" on the axis they mean. The modifiers are the one
     * group whose columns do not correspond, five over six, and that costs
     * nothing.
     *
     * <p>It is a line of somebody else's screen dearer than {@link
     * #standardKeys()} and reaches the F-keys without scrolling for them, which
     * is a trade only the person using it can make.
     */
    public static List<Key> twoLineKeys() {
        final List<Key> k = new ArrayList<>();
        k.add(Key.modifier("Shift", "shift", Keysym.SHIFT_L, 0).onRow(1));
        k.add(Key.modifier("Ctrl", null, Keysym.CONTROL_L, 0).onRow(1));
        k.add(Key.modifier("Alt", null, Keysym.ALT_L, 0).onRow(1));
        k.add(Key.modifier("Windows", "windows", Keysym.SUPER_L, 0).onRow(1));
        k.add(Key.modifier("Option", "option", Keysym.ISO_LEVEL3_SHIFT, 0).onRow(1));
        k.add(Key.modifier("CMD", "command", Keysym.SUPER_L, 0).onRow(1));
        k.add(Key.icon("Backspace", "backspace", Keysym.BACKSPACE, 0).repeating());
        k.add(Key.normal("Del", Keysym.DELETE, 0).repeating());
        k.add(Key.normal("Esc", Keysym.ESCAPE, 0));
        k.add(Key.normal("Tab", Keysym.TAB, 0).repeating());
        k.add(Key.normal("Ins", Keysym.INSERT, 0));

        // Home and End are drawn as arrows here, where they sit over Left and
        // Right: at that width a word beside three arrow glyphs is what breaks
        // the cluster up, and the shape is the thing that says which is which.
        k.add(Key.icon("Home", "home", Keysym.HOME, 1));
        k.add(Key.icon("Up", "arrow_up", Keysym.UP, 1).repeating());
        k.add(Key.icon("End", "end", Keysym.END, 1));
        k.add(Key.icon("Left", "arrow_left", Keysym.LEFT, 1).repeating().onRow(1));
        k.add(Key.icon("Down", "arrow_down", Keysym.DOWN, 1).repeating().onRow(1));
        k.add(Key.icon("Right", "arrow_right", Keysym.RIGHT, 1).repeating().onRow(1));

        // Abbreviated, since the pair is a column of its own and "Page Down" is
        // the widest label on the row by half again.
        k.add(Key.normal("PgUp", Keysym.PAGE_UP, 2).repeating());
        k.add(Key.normal("PgDn", Keysym.PAGE_DOWN, 2).repeating().onRow(1));

        k.add(Key.action("Paste", "paste", ACTION_PASTE, 3));
        k.add(Key.icon("Enter", "return", Keysym.RETURN, 3).onRow(1));

        for (int i = 1; i <= 12; i++) {
            k.add(Key.wide("F" + i, Keysym.f(i), 4).onRow(i <= 6 ? 0 : 1));
        }
        return k;
    }

    public void setListener(Listener l) {
        this.listener = l;
    }

    /** How the view measures labels; without one, a monospace guess is used. */
    public void setLabelWidth(LabelWidth m) {
        this.labelWidth = m == null ? ExtensionKeyboard::estimateWidth : m;
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
     * How much of the bottom of the view is already covered — the system IME's
     * height. The row sits directly on top of it: the two are one keyboard as
     * far as anyone using them is concerned.
     */
    public void setBottomOffset(float px) {
        if (px == bottomOffset) {
            return;
        }
        bottomOffset = px;
        layout();
        changed();
    }

    public List<Key> allKeys() {
        return keyList;
    }

    /** The modifier keys, in declaration order: the info bar's status lights. */
    public List<Key> modifiers() {
        return modifiers;
    }

    /**
     * The keysyms of the modifiers held down at the far end right now, one-shot
     * and locked alike. A soft keyboard cannot see these, so a character typed
     * on one has to be put in the case they imply
     * ({@link Keysym#forCharacter(int, boolean, boolean)}).
     */
    public Set<Integer> heldModifiers() {
        final Set<Integer> held = new LinkedHashSet<>();
        for (Key m : modifiers) {
            if (sticky(m) != Sticky.OFF) {
                held.add(m.keysym());
            }
        }
        return held;
    }

    public Sticky sticky(Key k) {
        final Integer i = keyIndex.get(k);
        return i == null ? Sticky.OFF : state[i];
    }

    public boolean visible() {
        return visible;
    }

    public void toggle() {
        setVisible(!visible);
    }

    /**
     * Hiding clears every modifier, locked ones included: one you cannot see
     * must never eat the next key, and one held at the far end by a widget that
     * is gone can never be let go. The original does the same.
     */
    public void setVisible(boolean v) {
        if (visible == v) {
            return;
        }
        visible = v;
        if (!visible) {
            release();
            clearModifiers();
            typed.setLength(0);
        }
        changed();
    }

    // ---- what the view draws ----------------------------------------------

    /** The laid-out keys. Only those at least partly on screen. */
    public List<Bounds> keys() {
        return keys;
    }

    /** The key under the finger right now, for drawing it pressed. */
    public Key pressedKey() {
        return scrolling ? null : activeKey;
    }

    /**
     * Where the finger came down, for a ripple that starts there rather than at
     * the middle of the key. A claimed pointer never reaches the view, so this
     * is the only place the position exists.
     */
    public float pressedX() {
        return pressX;
    }

    public float pressedY() {
        return pressY;
    }

    public float infoBarTop() {
        return keyRowTop() - cfg.keyboardInfoHeightPx;
    }

    public float keyRowTop() {
        return viewH - bottomOffset - rows * lineHeight();
    }

    /** How many lines of keys the current list asks for. */
    public int rows() {
        return rows;
    }

    /** How wide the keys come out, which is what there is to scroll through. */
    public float contentWidth() {
        return contentWidth;
    }

    /** How tall one line of keys is, which is not the same for one line as for two. */
    public float lineHeight() {
        return rows > 1 ? cfg.keyboardKeyHeightMultiPx : cfg.keyboardKeyHeightPx;
    }

    public float keyRowBottom() {
        return viewH - bottomOffset;
    }

    /** The mask ⇄ show toggle, and the dismiss button, at the bar's right end. */
    public Bounds part(Part p) {
        final float top = infoBarTop(), bottom = keyRowTop();
        final float side = cfg.keyboardInfoHeightPx;
        final float right = p == Part.DISMISS ? viewW : viewW - side;
        return new Bounds(null, right - side, top, right, bottom);
    }

    /** The status lights: one cell per modifier, starting here. */
    public float lightsLeft() {
        return cfg.dp(6);
    }

    /**
     * Barely wider than a glyph. These are an indicator strip, not buttons —
     * nothing is tappable here — so they want to read as one dense cluster,
     * and generous cells only push the readout off centre.
     */
    public float lightCell() {
        return cfg.keyboardInfoHeightPx * 0.55f;
    }

    /** Where the typed-text readout may draw: between the lights and the buttons. */
    public float textLeft() {
        return lightsLeft() + modifiers.size() * lightCell() + cfg.dp(8);
    }

    public float textRight() {
        return viewW - 2 * cfg.keyboardInfoHeightPx - cfg.dp(6);
    }

    public boolean masked() {
        return masked;
    }

    /** The line typed so far, masked if the privacy toggle is on. */
    public String infoText() {
        if (!masked) {
            return typed.toString();
        }
        return "●".repeat(typed.length());
    }

    /**
     * What the keyboard covers, for the viewport's insets: the key row and
     * whatever the IME under it covers — and the info bar only when that is
     * solid.
     *
     * <p>The row is opaque, it is where fingers land, and a cursor able to
     * travel beneath it would be unreachable. The info bar is none of those, so
     * by default it is not inset for: the desktop runs on underneath and the bar
     * floats over the last 30 dp, thinning as the cursor approaches (see
     * {@code control.ui.Chrome}). Inset by it *and* faded would reveal only
     * black, which is why the two go together — {@code keyboardInfoSolid}
     * chooses both at once.
     */
    public float insetBottomPx() {
        if (!visible) {
            return 0;
        }
        return bottomOffset + rows * lineHeight()
                + (cfg.keyboardInfoSolid ? cfg.keyboardInfoHeightPx : 0);
    }

    /**
     * How much of the bottom of the view the widget <em>occupies</em>: the row,
     * the IME and the info bar, whichever way the bar is drawn. The viewport
     * uses the inset, since a floating bar is one the desktop runs under;
     * anything drawn over the desktop that must not collide with the bar (the
     * HUD) uses this.
     */
    public float heightPx() {
        if (!visible) {
            return 0;
        }
        return bottomOffset + rows * lineHeight() + cfg.keyboardInfoHeightPx;
    }

    // ---- layout ------------------------------------------------------------

    /**
     * The keys, scrolled horizontally: centred when they fit, from the left when
     * they do not.
     *
     * <p>A group is a <b>grid</b> whose columns are shared between its lines —
     * the <i>n</i>th key of one line sits in the same column as the <i>n</i>th
     * key of the next, and the column is as wide as the widest of them. That is
     * what makes the arrows an inverted T ({@link #twoLineKeys()}), and it has
     * the useful side effect that every line is the same width, so the scroll,
     * the clamp and the fling stay one number. A one-line list is the same rule
     * with one row in it, laid out exactly as it always was.
     */
    private void layout() {
        keys.clear();
        if (viewW <= 0 || viewH <= 0) {
            return;
        }
        final int n = keyList.size();
        final List<int[]> runs = new ArrayList<>();    // {from, to} of each group
        final List<float[]> cols = new ArrayList<>();  // its column widths

        contentWidth = 0;
        for (int i = 0; i < n; ) {
            final int group = keyList.get(i).group();
            int j = i;
            while (j < n && keyList.get(j).group() == group) {
                j++;
            }
            final float[] w = columnWidths(i, j);
            if (i > 0) {
                contentWidth += cfg.keyboardGroupGapPx;
            }
            for (float cw : w) {
                contentWidth += cw;
            }
            runs.add(new int[]{i, j});
            cols.add(w);
            i = j;
        }

        final float origin;
        if (contentWidth <= viewW) {
            scrollX = 0;
            origin = (viewW - contentWidth) / 2.0f;
        } else {
            scrollX = clamp(scrollX, 0, contentWidth - viewW);
            origin = -scrollX;
        }

        final float top = keyRowTop();
        final float height = lineHeight();
        float x = origin;
        for (int g = 0; g < runs.size(); g++) {
            if (g > 0) {
                x += cfg.keyboardGroupGapPx;
            }
            final int[] run = runs.get(g);
            final float[] w = cols.get(g);
            final int[] next = new int[rows];
            for (int i = run[0]; i < run[1]; i++) {
                final Key k = keyList.get(i);
                final int c = next[k.row()]++;
                float left = x;
                for (int q = 0; q < c; q++) {
                    left += w[q];
                }
                final float keyTop = top + k.row() * height;
                keys.add(new Bounds(k, left, keyTop, left + w[c], keyTop + height));
            }
            for (float cw : w) {
                x += cw;
            }
        }
    }

    /** The column widths of the group of keys {@code [from, to)}. */
    private float[] columnWidths(int from, int to) {
        final float[] w = new float[to - from];
        final int[] next = new int[rows];
        int columns = 0;
        for (int i = from; i < to; i++) {
            final Key k = keyList.get(i);
            final int c = next[k.row()]++;
            w[c] = Math.max(w[c], widthOf(k));
            columns = Math.max(columns, c + 1);
        }
        return Arrays.copyOf(w, columns);
    }

    private float widthOf(Key k) {
        final float pad = k.wide() ? cfg.keyboardKeyPadWidePx : cfg.keyboardKeyPadPx;
        // An icon key is its icon wide, whatever its label says: the label is a
        // fallback for a renderer that cannot draw the icon.
        final float content = k.icon() != null
                ? cfg.keyboardIconWidthPx : labelWidth.measure(k.label());
        return Math.max(cfg.keyboardMinKeyWidthPx, content + 2 * pad);
    }

    /** Without a real measurer: a monospace guess, so tests need no view. */
    private static float estimateWidth(String label) {
        return label.length() * 12.0f;
    }

    private void scrollBy(float dx) {
        if (contentWidth <= viewW) {
            return;
        }
        final float before = scrollX;
        scrollX = clamp(scrollX - dx, 0, contentWidth - viewW);
        if (scrollX != before) {
            layout();
        }
    }

    // ---- TouchRouter.Claim -------------------------------------------------

    @Override
    public boolean claimTouch(int id, float x, float y, long t) {
        if (!visible || y < infoBarTop() || y >= keyRowBottom() || x < 0 || x >= viewW) {
            return false;
        }
        if (activeId != NONE) {
            // A second finger on the keyboard: claimed so it cannot reach the
            // touchpad behind, but the row is one finger wide in practice.
            spare |= bit(id);
            return true;
        }
        stopFling();
        activeId = id;
        activeKey = null;
        activePart = null;
        pressX = lastX = x;
        pressY = y;
        lastMoveTime = t;
        dragVelocity = 0;
        scrolling = false;
        consumed = false;

        if (inInfoBar(y)) {
            activePart = partAt(x);
        } else {
            activeKey = keyAt(x, y);
            if (activeKey != null) {
                scheduler.postDelayed(longPress, cfg.keyLongPressMs);
            }
        }
        changed();
        return true;
    }

    @Override
    public void claimMoved(int id, float x, float y, long t) {
        if (id != activeId) {
            return;
        }
        if (!scrolling && activePart == null && Math.abs(x - pressX) > cfg.keyboardScrollSlopPx) {
            // Dragging, not tapping: give up on the key and scroll the row under
            // it. This is why keys fire on release rather than on press.
            scrolling = true;
            activeKey = null;
            cancelTimers();
        }
        if (scrolling) {
            scrollBy(x - lastX);
            // Smoothed: a fling is judged from the last delta before the finger
            // left, and a single frame's delta is noisy.
            final long dt = Math.max(1, t - lastMoveTime);
            final float sample = (x - lastX) / dt;
            dragVelocity = dragVelocity == 0 ? sample : dragVelocity * 0.6f + sample * 0.4f;
            changed();
        }
        lastX = x;
        lastMoveTime = t;
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
        cancelTimers();
        final Key key = activeKey;
        final Part part = activePart;
        final boolean act = !scrolling && !consumed;
        final boolean wasScrolling = scrolling;
        activeId = NONE;
        activeKey = null;
        activePart = null;
        scrolling = false;

        if (wasScrolling && Math.abs(dragVelocity) >= cfg.keyboardFlingMinPx) {
            flingVelocity = dragVelocity;
            scheduler.postDelayed(fling, cfg.keyboardFlingTickMs);
        }
        if (act && key != null && key.equals(keyAt(x, y))) {
            // Still on the key it started on, which is the rule the info bar's
            // parts below already follow: lifting somewhere else is how a
            // mis-aimed key is abandoned, and only sideways movement — the
            // scroll slop — used to do that.
            tap(key, t);
        } else if (act && part != null && inInfoBar(y) && partAt(x) == part) {
            // Still on what it started on, like an ordinary click.
            if (part == Part.DISMISS) {
                setVisible(false);
            } else {
                masked = !masked;
            }
        }
        changed();
    }

    @Override
    public void claimCancelled(long t) {
        release();
        changed();
    }

    private void release() {
        cancelTimers();
        stopFling();
        activeId = NONE;
        activeKey = null;
        activePart = null;
        scrolling = false;
        consumed = false;
        spare = 0;
        // The double-tap memory goes too. Hiding the row clears every
        // modifier's state, so a remembered tap that outlived it would meet a
        // key that is off and read as the second of a pair — locking on what is
        // somebody's first tap since the row came back.
        lastTapKey = null;
        lastTapTime = 0;
    }

    private void cancelTimers() {
        scheduler.removeCallbacks(longPress);
        scheduler.removeCallbacks(repeat);
    }

    private void stopFling() {
        if (flingVelocity != 0) {
            flingVelocity = 0;
            scheduler.removeCallbacks(fling);
        }
    }

    /** Is the row scrolled as far as it goes? Then a fling has nowhere to run. */
    private boolean atScrollLimit() {
        return contentWidth <= viewW || scrollX <= 0 || scrollX >= contentWidth - viewW;
    }

    /** True while the row is still gliding after a flick. */
    public boolean flinging() {
        return flingVelocity != 0;
    }

    private static long bit(int id) {
        return id >= 0 && id < 64 ? 1L << id : 0;
    }

    private Key keyAt(float x, float y) {
        for (Bounds b : keys) {
            if (b.contains(x, y)) {
                return b.key();
            }
        }
        return null;
    }

    private boolean inInfoBar(float y) {
        return y >= infoBarTop() && y < keyRowTop();
    }

    private Part partAt(float x) {
        for (Part p : Part.values()) {
            final Bounds b = part(p);
            if (x >= b.left() && x < b.right()) {
                return p;
            }
        }
        // The original's whole info bar toggles the mask; only the two buttons
        // at the right end do anything else.
        return Part.MASK;
    }

    // ---- keys --------------------------------------------------------------

    /**
     * The long press means one thing per key type: a modifier <b>locks</b>, a
     * {@link Key#repeating()} key <b>starts repeating</b>, anything else does
     * nothing and still sends its one press on release.
     *
     * <p>Note what the ordinary key does <em>not</em> do: emit a press at the
     * threshold. The single press of a key is its release, always, however long
     * it was held. Firing at the threshold as well would make 490 ms and 510 ms
     * holds send one key at visibly different moments, and would put a keystroke
     * at exactly the instant a modifier would have locked.
     */
    private void onLongPress() {
        if (activeKey == null) {
            return;
        }
        if (activeKey.modifier()) {
            consumed = true;
            set(activeKey, sticky(activeKey) == Sticky.LOCKED ? Sticky.OFF : Sticky.LOCKED);
            feedback(Feedback.LOCK);
        } else if (activeKey.repeats()) {
            scheduler.postDelayed(repeat, cfg.keyRepeatMs);
        }
        changed();
    }

    /**
     * A tap arms a modifier, a second tap within {@link Config#keyDoubleTapMs}
     * locks it — the original's gesture, alongside the hold in
     * {@link #onLongPress()}.
     *
     * <p>The double tap is consulted <em>only for modifiers</em>. RealVNC routes
     * every key through one double-tap detector and then ignores the second tap
     * for keys that are not modifiers, so pressing Backspace twice quickly
     * deletes one character. Here a non-modifier never asks the question.
     */
    private void tap(Key k, long t) {
        final boolean second = k.equals(lastTapKey) && t - lastTapTime <= cfg.keyDoubleTapMs;
        lastTapKey = k;
        lastTapTime = t;
        if (!k.modifier()) {
            fire(k);
        } else if (second) {
            set(k, sticky(k) == Sticky.LOCKED ? Sticky.OFF : Sticky.LOCKED);
        } else {
            set(k, sticky(k) == Sticky.OFF ? Sticky.ONESHOT : Sticky.OFF);
        }
        if (k.modifier() && listener != null) {
            listener.modifierPressed(k);
        }
        feedback(second && k.modifier() ? Feedback.LOCK : Feedback.PRESS);
    }

    /**
     * Press and release a key, then let go of whatever was armed for it. An
     * action key ({@link Key#action}) reports its name instead, and still
     * consumes the armed modifiers.
     */
    private void fire(Key k) {
        if (k.action() != null) {
            if (listener != null) {
                listener.keyAction(k.action());
            }
            releaseOneShotModifiers();
            return;
        }
        final int id = keyId(k);
        sink.keyDown(k.keysym(), id);
        sink.keyUp(id);
        note(k.keysym());
        releaseOneShotModifiers();
    }

    /**
     * This key's identity at the far end ({@link KeySink} §"the key id"): its
     * place in the row, stable because the list is immutable. Two keys sharing a
     * keysym — Windows and CMD are both {@code Super_L} — therefore have
     * different ids, which is the point.
     */
    private int keyId(Key k) {
        final Integer i = keyIndex.get(k);
        return KeySink.ID_EXTENSION_KEYBOARD + (i == null ? -1 : i);
    }

    /**
     * A key was sent by somebody else — the system IME committing a character,
     * in practice. Feeds the info bar and consumes the armed modifiers, as one
     * of our own keys would.
     */
    public void externalKey(int keysym) {
        note(keysym);
        releaseOneShotModifiers();
        changed();
    }

    /**
     * A click finished somewhere else — the touchpad, the mouse overlay, a real
     * mouse. Consumes the armed modifiers, because Ctrl+click is a chord in
     * exactly the way Ctrl+C is and this row is the only half of either that can
     * be armed.
     *
     * <p>What the caller reports is the <em>release</em> of the last button, not
     * the press: a modifier let go of at the press would be gone for the drag
     * that press turns into, and Shift+drag and Ctrl+drag are most of what this
     * is for.
     */
    public void externalClick() {
        releaseOneShotModifiers();
        changed();
    }

    /** Let go of every one-shot modifier; locked ones stay down. */
    public void releaseOneShotModifiers() {
        for (Key m : modifiers) {
            if (sticky(m) == Sticky.ONESHOT) {
                set(m, Sticky.OFF);
            }
        }
    }

    public void clearModifiers() {
        for (Key m : modifiers) {
            set(m, Sticky.OFF);
        }
    }

    /** Modifiers held down at the remote right now, for the HUD. */
    public int heldModifierCount() {
        int n = 0;
        for (Key m : modifiers) {
            if (sticky(m) != Sticky.OFF) {
                n++;
            }
        }
        return n;
    }

    private void set(Key k, Sticky to) {
        final Integer idx = keyIndex.get(k);
        final int i = idx == null ? -1 : idx;
        if (i < 0 || state[i] == to) {
            return;
        }
        final boolean was = state[i] != Sticky.OFF;
        state[i] = to;
        final boolean now = to != Sticky.OFF;
        if (was == now) {
            return; // ONESHOT ⇄ LOCKED: still held, nothing to send.
        }
        // Two keys can share a keysym — Windows and CMD are both Super_L — so the
        // remote hears only the first press and the last release, and the release
        // has to name the id the *press* used: press Windows then CMD and CMD's
        // release ends a hold the far end recorded under Windows.
        if (othersHold(i, k.keysym())) {
            return;
        }
        if (now) {
            heldIds.put(k.keysym(), keyId(k));
            sink.keyDown(k.keysym(), keyId(k));
        } else {
            final Integer id = heldIds.remove(k.keysym());
            sink.keyUp(id != null ? id : keyId(k));
        }
    }

    private boolean othersHold(int except, int keysym) {
        for (int i = 0; i < state.length; i++) {
            if (i != except && state[i] != Sticky.OFF && keyList.get(i).keysym() == keysym) {
                return true;
            }
        }
        return false;
    }

    /**
     * The info bar's readout: the line being typed. Backspace erases, Return
     * clears the whole buffer, anything with a character appends. The original's
     * info bar exactly.
     */
    private void note(int keysym) {
        if (keysym == Keysym.BACKSPACE) {
            if (typed.length() > 0) {
                typed.setLength(typed.length() - 1);
            }
            return;
        }
        if (keysym == Keysym.RETURN) {
            typed.setLength(0);
            return;
        }
        final int cp = Keysym.toUnicode(keysym);
        if (cp != 0) {
            typed.appendCodePoint(cp);
            if (typed.length() > cfg.keyboardInfoMaxChars) {
                typed.delete(0, typed.length() - cfg.keyboardInfoMaxChars);
            }
        }
    }

    private void feedback(Feedback what) {
        if (listener != null && cfg.keyboardHaptics) {
            listener.keyFeedback(what);
        }
    }

    private void changed() {
        if (listener != null) {
            listener.keyboardChanged();
        }
    }

    private static float clamp(float v, float lo, float hi) {
        return v < lo ? lo : (v > hi ? hi : v);
    }
}
