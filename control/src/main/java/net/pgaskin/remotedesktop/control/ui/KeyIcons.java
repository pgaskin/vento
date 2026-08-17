// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.ui;

import android.graphics.Matrix;
import android.graphics.Path;
import android.graphics.RectF;

import java.util.HashMap;
import java.util.Map;

/**
 * The keycap glyphs, and the toolbar's, as vectors.
 *
 * <p>RealVNC ships these as PNGs — `key_up`, `key_shift`, `key_backspace`,
 * `key_return`, `key_option`, `key_super`, `key_windows` and the arrows — and
 * draws text only for the keys it has no icon for. We copy
 * the choice of which keys get an icon, and redraw the icons, for the same
 * reason {@code Art} generates the wallpaper: this is a clean reimplementation,
 * and lifting somebody else's artwork into the repo would not be.
 *
 * <p>Paths are built in a **unit box** — {@code (0,0)} to {@code (1,1)}, y down
 * — so a caller scales to whatever size the key face allows. Stroked icons carry
 * their line width as a fraction of that box, so the weight scales with them;
 * {@link Icon#stroke()} of zero means fill instead.
 *
 * <p>Unicode was the first attempt and was wrong: {@code ↑ ↓ ← →} render at the
 * surrounding text's weight and metrics, which next to the word-labelled keys
 * looks like a font substitution rather than a keycap.
 *
 * <p>The toolbar's five are here rather than in the app's icon set, which is
 * Material Symbols, and it is not a licence question — that set is already in
 * the tree and accounted for. It is that these are drawn <em>on the canvas</em>,
 * and the nearest other glyphs to them are the ⇧ and ⌫ three rows below: a
 * filled outline at Material's weight beside a 0.09-stroked ⇧ is two icon sets
 * on one screen, which is the mistake this file was written to avoid.
 */
final class KeyIcons {

    /** A unit-box glyph. {@code stroke} is a fraction of the box; 0 means fill. */
    record Icon(Path path, float stroke) {
    }

    // Key-face icon ids, as carried by ExtensionKeyboard.Key.icon().
    static final String ARROW_UP = "arrow_up";
    static final String ARROW_DOWN = "arrow_down";
    static final String ARROW_LEFT = "arrow_left";
    static final String ARROW_RIGHT = "arrow_right";
    static final String SHIFT = "shift";
    static final String BACKSPACE = "backspace";
    static final String RETURN = "return";
    static final String OPTION = "option";
    static final String COMMAND = "command";
    static final String WINDOWS = "windows";
    static final String PASTE = "paste";
    static final String HOME = "home";
    static final String END = "end";

    // The toolbar's, which are not keycaps — see the note at the top.
    static final String DISCONNECT = "disconnect";
    static final String INFO = "info";
    static final String KEYBOARD = "keyboard";
    static final String MOUSE = "mouse";
    static final String GRIP = "grip";

    private static final Map<String, Icon> CACHE = new HashMap<>();

    private KeyIcons() {
    }

    /** The glyph for {@code id}, or {@code null} if there is none. */
    static Icon of(String id) {
        if (id == null) {
            return null;
        }
        Icon i = CACHE.get(id);
        if (i == null && !CACHE.containsKey(id)) {
            i = build(id);
            CACHE.put(id, i);
        }
        return i;
    }

    private static Icon build(String id) {
        return switch (id) {
            case ARROW_UP -> new Icon(arrow(0), 0.10f);
            case ARROW_RIGHT -> new Icon(arrow(90), 0.10f);
            case ARROW_DOWN -> new Icon(arrow(180), 0.10f);
            case ARROW_LEFT -> new Icon(arrow(270), 0.10f);
            case SHIFT -> new Icon(shift(), 0.09f);
            case BACKSPACE -> new Icon(backspace(), 0.09f);
            case RETURN -> new Icon(returnKey(), 0.09f);
            case OPTION -> new Icon(option(), 0.09f);
            case COMMAND -> new Icon(command(), 0.08f);
            case WINDOWS -> new Icon(windows(), 0);
            case PASTE -> new Icon(paste(), 0.09f);
            case HOME -> new Icon(toBar(false), 0.10f);
            case END -> new Icon(toBar(true), 0.10f);
            case DISCONNECT -> new Icon(disconnect(), 0.09f);
            case INFO -> new Icon(info(), 0.09f);
            case KEYBOARD -> new Icon(keyboard(), 0.09f);
            case MOUSE -> new Icon(mouse(), 0.09f);
            case GRIP -> new Icon(grip(), 0.09f);
            default -> null;
        };
    }

    /**
     * A line arrow pointing up, rotated by {@code deg} about the centre. Drawn
     * as a shaft and a chevron rather than the original's solid wedge: at 18 dp
     * a filled arrow is a blob, and it is much heavier than the outlined ⇧ and
     * ⌫ sitting three keys away.
     */
    private static Path arrow(float deg) {
        final Path p = new Path();
        p.moveTo(0.50f, 0.90f);
        p.lineTo(0.50f, 0.15f);
        p.moveTo(0.22f, 0.44f);
        p.lineTo(0.50f, 0.15f);
        p.lineTo(0.78f, 0.44f);
        if (deg != 0) {
            final Matrix m = new Matrix();
            m.setRotate(deg, 0.5f, 0.5f);
            p.transform(m);
        }
        return p;
    }

    /**
     * ⇤ / ⇥ — the arrow of {@link #arrow}, sideways, stopped by a bar at the end
     * it points at: the two keys that mean "as far that way as this line goes".
     * Home and End are words on the one-line row, where there is room for them
     * and no arrow beside them; on the two-line row they sit directly above Left
     * and Right, where the same shape at the same weight is what says so.
     */
    private static Path toBar(boolean right) {
        final Path p = new Path();
        p.moveTo(0.12f, 0.16f);
        p.lineTo(0.12f, 0.84f);
        p.moveTo(0.24f, 0.50f);
        p.lineTo(0.94f, 0.50f);
        p.moveTo(0.50f, 0.24f);
        p.lineTo(0.24f, 0.50f);
        p.lineTo(0.50f, 0.76f);
        if (right) {
            final Matrix m = new Matrix();
            m.setScale(-1, 1, 0.5f, 0.5f);
            p.transform(m);
        }
        return p;
    }

    /** ⇧ — the same outline as the arrow, hollow, with a flat foot. */
    private static Path shift() {
        final Path p = new Path();
        p.moveTo(0.50f, 0.07f);
        p.lineTo(0.93f, 0.50f);
        p.lineTo(0.70f, 0.50f);
        p.lineTo(0.70f, 0.93f);
        p.lineTo(0.30f, 0.93f);
        p.lineTo(0.30f, 0.50f);
        p.lineTo(0.07f, 0.50f);
        p.close();
        return p;
    }

    /** ⌫ — a pentagon pointing left, with a cross in it. */
    private static Path backspace() {
        final Path p = new Path();
        p.moveTo(0.04f, 0.50f);
        p.lineTo(0.34f, 0.16f);
        p.lineTo(0.96f, 0.16f);
        p.lineTo(0.96f, 0.84f);
        p.lineTo(0.34f, 0.84f);
        p.close();
        p.moveTo(0.50f, 0.36f);
        p.lineTo(0.76f, 0.64f);
        p.moveTo(0.76f, 0.36f);
        p.lineTo(0.50f, 0.64f);
        return p;
    }

    /** ↵ — a left arrow whose shaft turns up at the far end. */
    private static Path returnKey() {
        final Path p = new Path();
        p.moveTo(0.30f, 0.40f);
        p.lineTo(0.08f, 0.62f);
        p.lineTo(0.30f, 0.84f);
        p.moveTo(0.08f, 0.62f);
        p.lineTo(0.84f, 0.62f);
        p.lineTo(0.84f, 0.18f);
        return p;
    }

    /** ⌥ — a shelf, a diagonal, a shelf, and a separate bar over the top right. */
    private static Path option() {
        final Path p = new Path();
        p.moveTo(0.04f, 0.26f);
        p.lineTo(0.30f, 0.26f);
        p.lineTo(0.70f, 0.78f);
        p.lineTo(0.96f, 0.78f);
        p.moveTo(0.62f, 0.26f);
        p.lineTo(0.96f, 0.26f);
        return p;
    }

    /** ⌘ — a square with a loop through each corner. */
    private static Path command() {
        final Path p = new Path();
        final float lo = 0.31f, hi = 0.69f, r = 0.185f;
        p.addRect(lo, lo, hi, hi, Path.Direction.CW);
        final RectF oval = new RectF();
        // Each loop is three quarters of a circle passing through its corner, so
        // the remaining quarter is the square's own corner: that is what makes
        // the four loops read as interlocked rather than as beads.
        // Centred r/√2 diagonally outward from the corner, so the circle passes
        // exactly through it, and started so the missing quarter is the one
        // facing the square.
        final float d = (float) (r / Math.sqrt(2));
        final float[][] corners = {{lo, lo, -d, -d, 90}, {hi, lo, d, -d, 180},
                {hi, hi, d, d, 270}, {lo, hi, -d, d, 0}};
        for (float[] k : corners) {
            oval.set(k[0] - r, k[1] - r, k[0] + r, k[1] + r);
            oval.offset(k[2], k[3]);
            p.addArc(oval, k[4], 270);
        }
        return p;
    }

    /**
     * A clipboard: a board whose top edge stops either side of the clip, so the
     * two outlines meet instead of crossing. Ours — the original has no such
     * key.
     */
    private static Path paste() {
        final Path p = new Path();
        p.moveTo(0.34f, 0.14f);
        p.lineTo(0.12f, 0.14f);
        p.lineTo(0.12f, 0.96f);
        p.lineTo(0.88f, 0.96f);
        p.lineTo(0.88f, 0.14f);
        p.lineTo(0.66f, 0.14f);
        p.moveTo(0.34f, 0.24f);
        p.lineTo(0.34f, 0.04f);
        p.lineTo(0.66f, 0.04f);
        p.lineTo(0.66f, 0.24f);
        p.close();
        return p;
    }

    /**
     * Leaving: a frame open on one side with the arrow of {@link #arrow} coming
     * out of it. It has to read as <em>this connection ends</em> rather than as
     * <em>that machine turns off</em>, which is what anything resembling a power
     * symbol would have said.
     *
     * <p>A snapped chain was the first attempt and is the better metaphor at any
     * size but this one: two half-links with a gap between them is a 4 px gap in
     * a 2 px stroke at 22 dp, which closes up into a ring.
     */
    private static Path disconnect() {
        final Path p = new Path();
        p.moveTo(0.46f, 0.08f);
        p.lineTo(0.08f, 0.08f);
        p.lineTo(0.08f, 0.92f);
        p.lineTo(0.46f, 0.92f);
        p.moveTo(0.36f, 0.50f);
        p.lineTo(0.94f, 0.50f);
        p.moveTo(0.70f, 0.28f);
        p.lineTo(0.94f, 0.50f);
        p.lineTo(0.70f, 0.72f);
        return p;
    }

    /** The letter i in a ring, drawn rather than set: a glyph would not match. */
    private static Path info() {
        final Path p = new Path();
        p.addCircle(0.50f, 0.50f, 0.44f, Path.Direction.CW);
        // A round cap makes a zero-length segment a dot, so the tittle is the
        // same weight as the stem by construction.
        p.moveTo(0.50f, 0.27f);
        p.lineTo(0.50f, 0.28f);
        p.moveTo(0.50f, 0.42f);
        p.lineTo(0.50f, 0.73f);
        return p;
    }

    /** A keyboard: an outline, a row of keys and a space bar. */
    private static Path keyboard() {
        final Path p = new Path();
        final RectF r = new RectF(0.04f, 0.22f, 0.96f, 0.78f);
        p.addRoundRect(r, 0.08f, 0.08f, Path.Direction.CW);
        for (float x = 0.18f; x < 0.80f; x += 0.24f) {
            p.moveTo(x, 0.40f);
            p.lineTo(x + 0.12f, 0.40f);
        }
        p.moveTo(0.30f, 0.62f);
        p.lineTo(0.70f, 0.62f);
        return p;
    }

    /** A mouse: an outline with a wheel in the top of it. */
    private static Path mouse() {
        final Path p = new Path();
        final RectF r = new RectF(0.28f, 0.06f, 0.72f, 0.94f);
        p.addRoundRect(r, 0.22f, 0.22f, Path.Direction.CW);
        p.moveTo(0.50f, 0.22f);
        p.lineTo(0.50f, 0.38f);
        return p;
    }

    /** Two bars: the handle the column is dragged by. */
    private static Path grip() {
        final Path p = new Path();
        p.moveTo(0.26f, 0.40f);
        p.lineTo(0.74f, 0.40f);
        p.moveTo(0.26f, 0.60f);
        p.lineTo(0.74f, 0.60f);
        return p;
    }

    /** The four-pane flag, its outer edges tilted the way the real one is. */
    private static Path windows() {
        final Path p = new Path();
        p.moveTo(0.04f, 0.16f);
        p.lineTo(0.46f, 0.10f);
        p.lineTo(0.46f, 0.47f);
        p.lineTo(0.04f, 0.47f);
        p.close();
        p.moveTo(0.54f, 0.09f);
        p.lineTo(0.96f, 0.03f);
        p.lineTo(0.96f, 0.47f);
        p.lineTo(0.54f, 0.47f);
        p.close();
        p.moveTo(0.04f, 0.53f);
        p.lineTo(0.46f, 0.53f);
        p.lineTo(0.46f, 0.90f);
        p.lineTo(0.04f, 0.84f);
        p.close();
        p.moveTo(0.54f, 0.53f);
        p.lineTo(0.96f, 0.53f);
        p.lineTo(0.96f, 0.97f);
        p.lineTo(0.54f, 0.91f);
        p.close();
        return p;
    }
}
