// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.playground;

import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.Paint;
import android.graphics.Path;
import android.graphics.RectF;

import net.pgaskin.remotedesktop.control.input.Button;
import net.pgaskin.remotedesktop.control.input.KeySink;
import net.pgaskin.remotedesktop.control.input.Keysym;
import net.pgaskin.remotedesktop.control.CursorController;

import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * Stands in for the remote machine. It consumes <em>synthetic mouse events</em>
 * — absolute position plus a button mask — exactly as a real remote desktop
 * would, which is the point of it: the squares never see an Android
 * {@code MotionEvent}. It consumes keysyms the same way, and
 * echoes them into a text field, so "did the remote get what I typed?" is
 * answerable by looking at the desktop rather than at a log.
 */
public final class FakeDesktop implements CursorController.PointerSink, KeySink {

    /**
     * The configuration switches the playground exposes as clickable squares.
     * The enum constant's name is what is drawn on the square; the current
     * <em>value</em> comes from {@link ToggleHandler#label}, since the
     * configuration lives in {@code Config}, not here.
     *
     * <p>Every one of them is drawn except the two recorders, which are there
     * only when the host asked for them — see the constructor.
     */
    public enum Toggle {
        PRESET, ACCEL, AXISLOCK, MOMENTUM, CURSOR, NATSCROLL, HUD, RECORD,
        /** The key trace: one line per key event, and what became of it. */
        KEYTRACE,
        /** Step the zoom ladder down / up, and fit-vs-fill. */
        ZOOMOUT, ZOOMIN, ZOOMFIT,
        /** Fake layout insets, to exercise the inset path in the clamp. */
        INSETS,
        /** Hand the cursor to the far end, which is this class. */
        RELATIVE,
        /** The toolbar tap regions, drawn while they are on. */
        REGIONS,
        /** The mouse button / wheel overlay; the {@code mouse} region does this too. */
        MOUSE,
        /** The extension keyboard and the system IME; the {@code keyboard} region too. */
        KEYBOARD,
        /** Which key list the row is drawn from: one line of keys, or two. */
        TWOLINE,
        /** The hover detent, and the round trip its news is pretended to take. */
        HOVER, LAG
    }

    public interface ToggleHandler {
        /** A toggle square was clicked. */
        void onToggle(Toggle toggle);

        /** Current value to draw on a toggle square. */
        String label(Toggle toggle);
    }

    public static final class Square {
        public final String title;
        public final int color;
        public final boolean draggable;
        public final Toggle toggle;
        public float x, y, w, h;
        public Button lastButton;
        public boolean pressed;

        Square(String title, int color, float x, float y, float w, float h,
               boolean draggable, Toggle toggle) {
            this.title = title;
            this.color = color;
            this.x = x;
            this.y = y;
            this.w = w;
            this.h = h;
            this.draggable = draggable;
            this.toggle = toggle;
        }

        boolean contains(float px, float py) {
            return px >= x && px <= x + w && py >= y && py <= y + h;
        }
    }

    public final int width, height;
    public final Bitmap wallpaper;

    private final List<Square> squares = new ArrayList<>();
    /**
     * Targets for the hover detent, {@code x, y, radius} each — small round
     * things with a cursor of their own, which a desktop is full of and this
     * one had none of: every object on it is 180×120 and nothing needs aiming
     * at. The smallest is under a screen pixel across once the desktop is
     * fitted to a phone, which is the case the assist exists for.
     */
    private final float[][] targets;
    private boolean targetsShown;
    /** What anything on this desktop changes the cursor to, the way a link does. */
    private final Art.Cursor objectCursor = Art.handCursor();
    private final ToggleHandler toggles;

    private int lastButtons;
    private Square dragging;
    private float dragOffX, dragOffY;

    // The cursor, in the mode where it belongs to this end. See setRelative.
    private boolean relative;
    private float relX, relY;
    private final Art.Cursor ownCursor = Art.arrowCursor();

    public int wheelV, wheelH;   // net clicks received, so scrolling is visible
    public String lastEvent = "";
    public String lastKey = "-";  // the last keysym edge, for the HUD
    /** What has been typed into the "remote", the way a text field would echo it. */
    private final StringBuilder typed = new StringBuilder();
    /**
     * What the remote is currently holding, keyed by <em>key id</em> and
     * remembering the keysym each was pressed with — which is exactly what
     * RFB's own {@code CKeyboard} keeps, and what makes a release able to name
     * a key rather than a keysym. Insertion
     * ordered, so the readout lists them in the order they went down.
     */
    private final Map<Integer, Integer> heldKeys = new LinkedHashMap<>();

    /**
     * Whatever was last drawn on the background, in desktop coordinates. It is
     * what makes the shape of a gesture visible after the fact — a circle that
     * came out with four flat sides is the axis lock engaging on a curve, and
     * that is not a thing the HUD can show.
     */
    private final Path inkPath = new Path();
    private boolean drawing;

    private final Paint fill = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint stroke = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint text = new Paint(Paint.ANTI_ALIAS_FLAG);
    /**
     * Deliberately un-antialiased, butt-capped and mitred: the point of the ink
     * is to show exactly where the cursor went, and a soft round brush hides
     * precisely the small steps and flat spots worth looking for. Translucent so
     * a second pass over the same place is visible.
     */
    private final Paint ink = new Paint();
    private final RectF tmp = new RectF();

    /**
     * @param recorders whether the {@code RECORD} and {@code KEYTRACE} squares
     *                  are among the ones drawn. Off, they are not laid out at
     *                  all and the block closes up behind them: a square that is
     *                  present but dead is a thing to explain, and there is
     *                  nothing here to explain it with.
     */
    public FakeDesktop(int width, int height, ToggleHandler toggles, boolean recorders) {
        this.width = width;
        this.height = height;
        this.toggles = toggles;
        this.wallpaper = Art.wallpaper(width, height);

        stroke.setStyle(Paint.Style.STROKE);
        stroke.setStrokeWidth(1.5f);
        ink.setStyle(Paint.Style.STROKE);
        ink.setStrokeWidth(3f);
        ink.setStrokeCap(Paint.Cap.BUTT);
        ink.setStrokeJoin(Paint.Join.MITER);
        ink.setColor(0x99e02828);
        text.setTextAlign(Paint.Align.CENTER);
        text.setFakeBoldText(true);

        final int[] colors = {0xff4a90d9, 0xffe0a33e, 0xff40c4a0, 0xffd95f6b, 0xff9b6bd9, 0xff6bd94a};
        // Sized like real desktop widgets rather than phone-sized buttons: on a
        // 1920x1200 remote a draggable object is ~180x120 px with ~15 px labels,
        // and getting that scale right matters for judging cursor feel — the
        // same cursor travel reads as much faster across oversized objects.
        // Small on the phone is correct: that is what
        // zoom is for, and it is how a real remote looks.
        // Laid out clear of the toggle block (60..720 x 60..254) and the wheel
        // readout (top right).
        final float[][] at = {
                {980, 120}, {1500, 260}, {200, 400}, {700, 620}, {1150, 500}, {1550, 800},
        };
        for (int i = 0; i < at.length; i++) {
            squares.add(new Square("SQ" + (i + 1), colors[i], at[i][0], at[i][1], 180, 120, true, null));
        }

        // Three rows of them, each row a sweep from 4 px across to 44, and the
        // rows offset so that a drag along one crosses a different set of sizes
        // than a drag along the next. Clear of the squares' start positions,
        // which are what a drag along a row would otherwise run into.
        final float[] radii = {2, 4, 6, 9, 13, 18, 22};
        targets = new float[3 * radii.length][];
        for (int row = 0; row < 3; row++) {
            for (int i = 0; i < radii.length; i++) {
                targets[row * radii.length + i] = new float[]{
                        260 + i * 200 + row * 60, 780 + row * 110, radii[i]};
            }
        }

        final List<Toggle> toggleDefs = new ArrayList<>();
        for (Toggle t : Toggle.values()) {
            if (recorders || (t != Toggle.RECORD && t != Toggle.KEYTRACE)) {
                toggleDefs.add(t);
            }
        }
        for (int i = 0; i < toggleDefs.size(); i++) {
            final float tx = 60 + (i % 4) * 170;
            final float ty = 60 + (i / 4) * 70;
            squares.add(new Square(toggleDefs.get(i).name(), 0xff3a3f4b, tx, ty, 150, 54,
                    false, toggleDefs.get(i)));
        }
    }

    public List<Square> squares() {
        return squares;
    }

    // ---- the "remote" input handler ---------------------------------------

    /**
     * Take the cursor over from {@code (x, y)}, or hand it back.
     *
     * <p>A far end that owns the cursor is one that integrates the deltas
     * itself and draws the result into the picture it sends, so this class does
     * both — which is what makes the relative path exercisable with no server
     * in the room. Everything else about it is unchanged, deliberately: the
     * deltas are turned back into a position and go through the same handler,
     * so the squares are still clickable and this square is still reachable to
     * turn the mode off again.
     */
    public void setRelative(boolean on, float x, float y) {
        relative = on;
        relX = clamp(x, 0, width - 1);
        relY = clamp(y, 0, height - 1);
    }

    public boolean relative() {
        return relative;
    }

    @Override
    public void pointerEventRelative(int dx, int dy, int buttons) {
        relX = clamp(relX + dx, 0, width - 1);
        relY = clamp(relY + dy, 0, height - 1);
        pointerEvent(relX, relY, buttons);
    }

    @Override
    public void pointerEvent(float x, float y, int buttons) {
        final int changed = buttons ^ lastButtons;
        final int pressed = changed & buttons;
        final int released = changed & ~buttons;
        // Before dispatching anything: a toggle handler is free to move the
        // cursor, which re-enters this method with the same button mask. Latch
        // the mask first so the re-entrant call sees no change and stops.
        lastButtons = buttons;

        if (Button.WHEEL_UP.in(pressed)) wheelV--;
        if (Button.WHEEL_DOWN.in(pressed)) wheelV++;
        if (Button.WHEEL_LEFT.in(pressed)) wheelH--;
        if (Button.WHEEL_RIGHT.in(pressed)) wheelH++;

        if ((pressed & Button.DRAG_MASK) != 0) {
            final Square s = hit(x, y);
            if (s != null) {
                s.lastButton = Button.lowest(pressed & Button.DRAG_MASK);
                s.pressed = true;
                if (s.toggle != null) {
                    toggles.onToggle(s.toggle);
                } else if (s.draggable) {
                    dragging = s;
                    dragOffX = s.x - x;
                    dragOffY = s.y - y;
                }
            } else {
                // Pressed on the background: start a new stroke, dropping the
                // last one. A tap therefore clears the canvas.
                inkPath.rewind();
                inkPath.moveTo(x, y);
                drawing = true;
            }
        }

        if (dragging != null && (buttons & Button.DRAG_MASK) != 0) {
            dragging.x = clamp(x + dragOffX, 0, width - dragging.w);
            dragging.y = clamp(y + dragOffY, 0, height - dragging.h);
        }
        if (drawing) {
            if ((buttons & Button.DRAG_MASK) != 0) {
                inkPath.lineTo(x, y);
            } else {
                drawing = false;
            }
        }

        if ((released & Button.DRAG_MASK) != 0 && (buttons & Button.DRAG_MASK) == 0) {
            for (Square s : squares) {
                s.pressed = false;
            }
            dragging = null;
        }

        if (changed != 0) {
            lastEvent = (pressed != 0 ? "down " + Button.lowest(pressed)
                    : "up " + Button.lowest(released))
                    + " @" + (int) x + "," + (int) y;
        }
    }

    // ---- the "remote" keyboard handler ------------------------------------

    /**
     * A remote machine sees only keysym edges, and what a key <em>means</em> is
     * its business. This one behaves like a text field: printable keysyms are
     * appended, Backspace erases, Return breaks the line, and everything else
     * (including every modifier) is shown as held but changes no text — which is
     * enough to see that Shift is really down while the IME's letter arrives.
     */
    @Override
    public void keyDown(int keysym, int keyId) {
        lastKey = "down " + Keysym.name(keysym);
        heldKeys.put(keyId, keysym);
        if (keysym == Keysym.BACKSPACE) {
            if (typed.length() > 0) {
                typed.setLength(typed.length() - 1);
            }
            return;
        }
        if (keysym == Keysym.RETURN) {
            typed.append('⏎');
            trimTyped();
            return;
        }
        final int cp = Keysym.toUnicode(keysym);
        if (cp != 0) {
            // Shift is held by the extension keyboard, not folded into the
            // character, so apply it here — the remote is what owns the layout.
            typed.appendCodePoint(heldKeys.containsValue(Keysym.SHIFT_L)
                    || heldKeys.containsValue(Keysym.SHIFT_R)
                    ? Character.toUpperCase(cp) : cp);
            trimTyped();
        }
    }

    /**
     * A release names the key, not the keysym, and lets go of whatever that key
     * went down with — an id nobody pressed releases nothing, which is the
     * failure a keysym-as-id client cannot show you.
     */
    @Override
    public void keyUp(int keyId) {
        final Integer keysym = heldKeys.remove(keyId);
        lastKey = "up " + (keysym == null ? "?" : Keysym.name(keysym));
    }

    private void trimTyped() {
        if (typed.length() > 48) {
            typed.delete(0, typed.length() - 48);
        }
    }

    /** What the remote's text field holds, and what it is holding down. */
    public String typedText() {
        return typed.toString();
    }

    public String heldKeysText() {
        if (heldKeys.isEmpty()) {
            return "-";
        }
        final StringBuilder sb = new StringBuilder();
        for (int k : heldKeys.values()) {
            if (sb.length() > 0) {
                sb.append('+');
            }
            sb.append(Keysym.name(k));
        }
        return sb.toString();
    }

    /**
     * The cursor a real desktop would be showing at this point, or null over
     * the wallpaper, where it is whatever the host is drawing anyway. What it
     * is worth is that the boundary's position is known exactly, which nothing
     * at the far end of a protocol ever is — so the ink trail answers "did the
     * detent land where the edge was" rather than merely "did something
     * happen".
     */
    public Art.Cursor shapeAt(float x, float y) {
        if (hit(x, y) != null) {
            return objectCursor;
        }
        if (targetsShown) {
            for (float[] t : targets) {
                final float dx = x - t[0], dy = y - t[1];
                if (dx * dx + dy * dy <= t[2] * t[2]) {
                    return objectCursor;
                }
            }
        }
        return null;
    }

    /**
     * Whether the small round targets are on the desktop at all. They are the
     * hover detent's reason to exist, so they appear with it: an object whose
     * cursor changes and which nothing else here provides.
     */
    public void setTargets(boolean shown) {
        targetsShown = shown;
    }

    private Square hit(float x, float y) {
        for (int i = squares.size() - 1; i >= 0; i--) {
            if (squares.get(i).contains(x, y)) {
                return squares.get(i);
            }
        }
        return null;
    }

    // ---- rendering (in desktop coordinates) -------------------------------

    public void draw(Canvas c) {
        c.drawBitmap(wallpaper, 0, 0, null);
        // Under the ink, so a trail across one still shows where it went.
        if (targetsShown) {
            for (float[] t : targets) {
                fill.setColor(0x55ffffff);
                c.drawCircle(t[0], t[1], t[2], fill);
                stroke.setColor(0x88ffffff);
                c.drawCircle(t[0], t[1], t[2], stroke);
            }
        }
        c.drawPath(inkPath, ink);

        for (Square s : squares) {
            tmp.set(s.x, s.y, s.x + s.w, s.y + s.h);
            fill.setColor(s.pressed ? darken(s.color) : s.color);
            c.drawRoundRect(tmp, 6, 6, fill);
            stroke.setColor(s.pressed ? 0xffffffff : 0x66000000);
            c.drawRoundRect(tmp, 6, 6, stroke);

            text.setColor(0xddffffff);
            text.setTextSize(15);
            c.drawText(s.title, s.x + s.w / 2, s.y + 20, text);

            final String sub = s.toggle != null
                    ? toggles.label(s.toggle)
                    : (s.lastButton == null ? "-" : s.lastButton.toString());
            text.setColor(0xffffffff);
            text.setTextSize(s.toggle != null ? 17 : 22);
            c.drawText(sub, s.x + s.w / 2, s.y + s.h / 2 + 12, text);
        }

        // Wheel readout, so two-finger scroll is verifiable at a glance.
        tmp.set(width - 300, 60, width - 40, 124);
        fill.setColor(0xcc202531);
        c.drawRoundRect(tmp, 6, 6, fill);
        text.setColor(0xffffffff);
        text.setTextSize(20);
        c.drawText("WHEEL  V " + wheelV + "   H " + wheelH, tmp.centerX(), tmp.centerY() + 7, text);

        // The remote's "text field": what the keyboard actually delivered.
        tmp.set(60, height - 160, width - 60, height - 60);
        fill.setColor(0xcc202531);
        c.drawRoundRect(tmp, 6, 6, fill);
        text.setColor(0xff9fb4d0);
        text.setTextSize(15);
        c.drawText("KEYS   held " + heldKeysText() + "   last " + lastKey,
                tmp.centerX(), tmp.top + 26, text);
        text.setColor(0xffffffff);
        text.setTextSize(26);
        c.drawText(typed.length() == 0 ? "(nothing typed)" : typed.toString(),
                tmp.centerX(), tmp.bottom - 22, text);

        // Last, and in desktop pixels: a cursor this end owns is one drawn into
        // the picture, at the size the desktop draws it and zooming with it.
        if (relative) {
            c.drawBitmap(ownCursor.bitmap(),
                    relX - ownCursor.hotX(), relY - ownCursor.hotY(), null);
        }
    }

    private static int darken(int color) {
        return Color.argb(Color.alpha(color),
                (int) (Color.red(color) * 0.5f),
                (int) (Color.green(color) * 0.5f),
                (int) (Color.blue(color) * 0.5f));
    }

    private static float clamp(float v, float lo, float hi) {
        return v < lo ? lo : (v > hi ? hi : v);
    }
}
