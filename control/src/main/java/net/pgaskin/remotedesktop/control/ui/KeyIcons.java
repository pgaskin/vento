// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.ui;

import android.graphics.Matrix;
import android.graphics.Path;
import android.graphics.RectF;

import java.util.HashMap;
import java.util.Map;

/**
 * The keycap glyphs, as vectors.
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
