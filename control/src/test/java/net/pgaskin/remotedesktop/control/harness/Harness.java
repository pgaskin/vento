// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.harness;

import net.pgaskin.remotedesktop.control.input.Button;
import net.pgaskin.remotedesktop.control.input.Config;
import net.pgaskin.remotedesktop.control.input.ExtensionKeyboard;
import net.pgaskin.remotedesktop.control.input.GestureRecognizer;
import net.pgaskin.remotedesktop.control.input.KeySink;
import net.pgaskin.remotedesktop.control.input.PhysicalKeyboard;
import net.pgaskin.remotedesktop.control.input.PhysicalMouse;
import net.pgaskin.remotedesktop.control.input.Keysym;
import net.pgaskin.remotedesktop.control.input.MouseOverlay;
import net.pgaskin.remotedesktop.control.input.MouseSink;
import net.pgaskin.remotedesktop.control.input.RegionSink;
import net.pgaskin.remotedesktop.control.input.TapRegions;
import net.pgaskin.remotedesktop.control.input.Toolbar;
import net.pgaskin.remotedesktop.control.input.TouchFrame;
import net.pgaskin.remotedesktop.control.input.TouchRouter;
import net.pgaskin.remotedesktop.control.input.ZoomSink;
import net.pgaskin.remotedesktop.control.CursorController;
import net.pgaskin.remotedesktop.control.Viewport;

import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;

/**
 * The whole input stack, on a virtual clock, with every output recorded.
 *
 * <pre>
 *   synthetic/recorded TouchFrames → TouchRouter → GestureRecognizer
 *        → (mouse log) → CursorController → (pointer log, i.e. what the remote sees)
 * </pre>
 *
 * <p>Tests either drive it by hand ({@link #down}/{@link #move}/{@link #up},
 * each of which also advances the clock) or replay a recorded
 * {@code .touch} fixture through {@link #play}.
 */
public final class Harness implements ZoomSink, CursorController.PointerSink, MouseSink, RegionSink {

    public static final float DENSITY = 2.625f; // 12 dp = 31.5 px, 24 dp = 63 px
    public static final int VIEW_W = 2400, VIEW_H = 1080;
    public static final int DESKTOP_W = 2560, DESKTOP_H = 1440;

    public final FakeScheduler clock = new FakeScheduler();
    public final Config cfg;
    public final Viewport viewport;
    public final CursorController cursor;
    public final GestureRecognizer gestures;
    public final TouchRouter router;
    // Each null until the matching with*() wires one in.
    public MouseOverlay overlay;
    public ExtensionKeyboard keyboard;
    public Toolbar toolbar;
    public PhysicalMouse physicalMouse;
    public PhysicalKeyboard physicalKeys;

    public final int viewW, viewH;

    // The logs. Empty unless the widget that writes to one was wired in.
    public final List<String> all = new ArrayList<>();        // everything, timestamped: the golden form
    public final List<String> mouse = new ArrayList<>();      // move dx,dy / down LEFT / up LEFT
    public final List<String> pointer = new ArrayList<>();    // ptr x,y BUTTONS: what the fake remote sees
    public final List<String> zoom = new ArrayList<>();
    public final List<String> regionTaps = new ArrayList<>(); // region <name> x,y
    public final List<String> keys = new ArrayList<>();       // key down Ctrl
    public final List<String> keyActions = new ArrayList<>(); // action keys, by name
    public final List<String> toolbarTaps = new ArrayList<>();// toolbar <name>, and moved <f>
    public int keyFeedbacks;                                  // haptics the row asked for
    // How far the picture was actually moved by a pinch's pan, summed step by
    // step: the clamp means the sum of what was asked for is not the same thing.
    public double pannedPx;
    public final Map<Integer, Integer> held = new LinkedHashMap<>(); // key id → the keysym it went down with

    private final TouchFrame frame = new TouchFrame();
    private final List<int[]> ids = new ArrayList<>();     // [id]
    private final List<float[]> pos = new ArrayList<>();   // [x, y]

    private boolean paused;

    public static Harness improved() {
        return new Harness(Config.improved(DENSITY));
    }

    public static Harness faithful() {
        return new Harness(Config.faithful(DENSITY));
    }

    public Harness(Config cfg) {
        this(cfg, VIEW_W, VIEW_H);
    }

    public Harness(Config cfg, int viewW, int viewH) {
        this.cfg = cfg;
        this.viewW = viewW;
        this.viewH = viewH;
        viewport = new Viewport(cfg.density);
        viewport.setDesktopSize(DESKTOP_W, DESKTOP_H);
        viewport.setViewSize(viewW, viewH);
        viewport.setFocus(DESKTOP_W / 2f, DESKTOP_H / 2f);
        viewport.centreOn(DESKTOP_W / 2f, DESKTOP_H / 2f, 1.0f);

        cursor = new CursorController(cfg, viewport, this, clock);
        cursor.setPosition(DESKTOP_W / 2f, DESKTOP_H / 2f);

        gestures = new GestureRecognizer(cfg, this, this, clock);
        gestures.setViewSize(viewW, viewH);
        router = new TouchRouter(gestures);
        reset();
    }

    /** Forget everything recorded so far (e.g. the initial centring event). */
    public Harness reset() {
        all.clear();
        mouse.clear();
        pointer.clear();
        zoom.clear();
        regionTaps.clear();
        keys.clear();
        keyActions.clear();
        keyFeedbacks = 0;
        pannedPx = 0;
        return this;
    }

    /** Whether {@link #regionTapped} claims the taps it is offered. */
    public boolean consumeRegionTaps = true;

    /** Hand the cursor to the far end, the way a server that owns one does. */
    public Harness withRelativePointer() {
        cursor.setRelative(true);
        gestures.setRelative(true);
        return this;
    }

    /** Wire tap regions up to this harness, which logs them to {@link #regionTaps}. */
    public Harness withRegions(TapRegions r) {
        gestures.setRegions(r, this);
        return this;
    }

    /**
     * Add the mouse overlay, visible, claiming touches that land on it. Its
     * presses go to their own {@link CursorController} button source — the same
     * wiring the playground uses — and are logged to {@link #mouse} with an
     * {@code ovl} prefix so they can be told apart from the gesture layer's.
     */
    public Harness withOverlay() {
        final MouseSink source = cursor.newButtonSource();
        overlay = new MouseOverlay(cfg, new MouseSink() {
            @Override
            public void mouseMove(float dx, float dy) {
                record(mouse, fmt("ovl move %.2f,%.2f", dx, dy));
                source.mouseMove(dx, dy);
            }

            @Override
            public void mouseDown(int mask) {
                record(mouse, "ovl down " + Button.maskName(mask));
                source.mouseDown(mask);
            }

            @Override
            public void mouseUp(int mask) {
                record(mouse, "ovl up " + Button.maskName(mask));
                source.mouseUp(mask);
            }

        }, clock);
        overlay.setViewSize(viewW, viewH);
        overlay.setListener(() -> gestures.setExternalButtonHeld(
                (overlay.heldMask() & Button.DRAG_MASK) != 0));
        overlay.setVisible(true);
        router.addClaim(overlay);
        return this;
    }

    /** Overlay events only: {@code ovl down LEFT} … */
    public List<String> overlayEvents() {
        return only(mouse, "ovl ");
    }

    /**
     * Add the extension keyboard, visible, claiming touches on its two bars.
     * Its output is logged to {@link #keys} as {@code key down Ctrl}, the same
     * shape the overlay's is.
     */
    public Harness withKeyboard() {
        return withKeyboard(ExtensionKeyboard.standardKeys());
    }

    public Harness withKeyboard(List<ExtensionKeyboard.Key> defs) {
        keyboard = new ExtensionKeyboard(cfg, new KeySink() {
            /**
             * The far end's own bookkeeping, which is the point of the key id:
             * a release names a key and lets go of the keysym that key went down
             * with. Modelling it here rather than
             * logging the raw id keeps the assertions readable <em>and</em> makes
             * a mispaired id show up, as {@code key up ?}.
             */
            @Override
            public void keyDown(int keysym, int keyId) {
                held.put(keyId, keysym);
                record(keys, "key down " + Keysym.name(keysym));
            }

            @Override
            public void keyUp(int keyId) {
                final Integer keysym = held.remove(keyId);
                record(keys, "key up " + (keysym == null ? "?" : Keysym.name(keysym)));
            }
        }, clock, defs);
        keyboard.setViewSize(viewW, viewH);
        keyboard.setListener(new ExtensionKeyboard.Listener() {
            @Override
            public void keyboardChanged() {
            }

            @Override
            public void keyFeedback(ExtensionKeyboard.Feedback what) {
                keyFeedbacks++;
            }

            @Override
            public void keyAction(String name) {
                record(keyActions, "action " + name);
            }
        });
        keyboard.setVisible(true);
        router.addClaim(keyboard);
        // The rest of the wiring both real screens do: a click consumes the
        // armed modifiers the way a key does, and the cursor is the only place
        // every source of buttons meets.
        cursor.setListener(new CursorController.Listener() {
            @Override
            public void onCursorChanged() {
            }

            @Override
            public void onButtonsReleased() {
                keyboard.externalClick();
            }
        });
        return this;
    }

    /**
     * Add the toolbar, visible, with the four standard items. Its actions are
     * logged to {@link #toolbarTaps} as {@code toolbar disconnect}, and a drag
     * that ends as {@code toolbar moved 0.42}.
     */
    public Harness withToolbar() {
        return withToolbar(Toolbar.standard());
    }

    public Harness withToolbar(List<Toolbar.Item> items) {
        toolbar = new Toolbar(cfg);
        toolbar.setItems(items);
        toolbar.setListener(new Toolbar.Listener() {
            @Override
            public void toolbarChanged() {
            }

            @Override
            public void toolbarAction(String name) {
                record(toolbarTaps, "toolbar " + name);
            }

            @Override
            public void toolbarMoved(float fraction) {
                record(toolbarTaps, fmt("toolbar moved %.3f", fraction));
            }
        });
        toolbar.setViewSize(viewW, viewH);
        toolbar.setVisible(true);
        router.addClaim(toolbar);
        return this;
    }

    /**
     * Add a physical mouse, on its own {@link CursorController} button source
     * exactly as the two real screens wire it. Its output is logged to
     * {@link #mouse} with a {@code phys} prefix, so a mouse button and a
     * gesture-layer button can be told apart in one list — which is the point of
     * the union: a tap during a mouse-held drag must not release it.
     */
    public Harness withMouse() {
        final MouseSink source = cursor.newButtonSource();
        physicalMouse = new PhysicalMouse(cfg, new MouseSink() {
            @Override
            public void mouseMove(float dx, float dy) {
                record(mouse, fmt("phys move %.2f,%.2f", dx, dy));
                source.mouseMove(dx, dy);
            }

            @Override
            public void mouseDown(int mask) {
                record(mouse, "phys down " + Button.maskName(mask));
                source.mouseDown(mask);
            }

            @Override
            public void mouseUp(int mask) {
                record(mouse, "phys up " + Button.maskName(mask));
                source.mouseUp(mask);
            }

        });
        return this;
    }

    /** Physical-mouse events only: {@code phys down LEFT} … */
    public List<String> physicalMouseEvents() {
        return only(mouse, "phys ");
    }

    /**
     * Add a physical keyboard, sharing {@link #keys} and {@link #held} with the
     * extension row — which is the arrangement being tested as much as it is a
     * convenience: the two keyboards are one keyboard at the far end.
     */
    public Harness withPhysicalKeys() {
        physicalKeys = new PhysicalKeyboard(new KeySink() {
            @Override
            public void keyDown(int keysym, int keyId) {
                held.put(keyId, keysym);
                record(keys, "key down " + Keysym.name(keysym));
            }

            @Override
            public void keyUp(int keyId) {
                final Integer keysym = held.remove(keyId);
                record(keys, "key up " + (keysym == null ? "?" : Keysym.name(keysym)));
            }
        });
        return this;
    }

    // ---- driving ----------------------------------------------------------

    /** Advance the virtual clock, running anything that comes due. */
    public Harness advance(long ms) {
        clock.advance(ms);
        return this;
    }

    /** Default gap between synthesised events, ms. */
    public long step = 8;

    public Harness down(int id, float x, float y) {
        clock.advance(step);
        final int i = find(id);
        if (i >= 0) {
            throw new IllegalStateException("pointer " + id + " already down");
        }
        ids.add(new int[]{id});
        pos.add(new float[]{x, y});
        return dispatch(TouchFrame.Action.DOWN, ids.size() - 1);
    }

    /** Move one pointer; the others keep their positions, as MotionEvent does. */
    public Harness move(int id, float x, float y) {
        clock.advance(step);
        set(id, x, y);
        return dispatch(TouchFrame.Action.MOVE, 0);
    }

    /** Move two pointers in a single event, which is what really happens. */
    public Harness move(int id0, float x0, float y0, int id1, float x1, float y1) {
        clock.advance(step);
        set(id0, x0, y0);
        set(id1, x1, y1);
        return dispatch(TouchFrame.Action.MOVE, 0);
    }

    public Harness up(int id) {
        clock.advance(step);
        final int i = find(id);
        if (i < 0) {
            throw new IllegalStateException("pointer " + id + " is not down");
        }
        dispatch(TouchFrame.Action.UP, i);
        ids.remove(i);
        pos.remove(i);
        return this;
    }

    /**
     * Lift somewhere other than where the pointer last was, in one event, which
     * is what a finger sliding off a target as it goes really produces.
     */
    public Harness up(int id, float x, float y) {
        set(id, x, y);
        return up(id);
    }

    public Harness cancel() {
        clock.advance(step);
        dispatch(TouchFrame.Action.CANCEL, 0);
        ids.clear();
        pos.clear();
        return this;
    }

    /**
     * The session leaving the screen: the three calls a host makes on its way
     * out, in the same order. Kept here so
     * the sequence is exercised as a whole rather than one class at a time —
     * what it is for is "the remote is holding nothing", and no single class can
     * answer that.
     */
    public Harness suspend() {
        clock.advance(step);
        router.cancel(clock.now());
        gestures.cancelAll(clock.now());
        if (keyboard != null) {
            keyboard.clearModifiers();
        }
        ids.clear();
        pos.clear();
        return this;
    }

    /** Tap: down, up, then wait out the click-release window. */
    public Harness tap(float x, float y) {
        return down(0, x, y).up(0).advance(cfg.clickHoldMs + 10);
    }

    /** A straight drag from (x,y) by (dx,dy) in {@code steps} events. */
    public Harness drag(int id, float x, float y, float dx, float dy, int steps) {
        down(id, x, y);
        for (int i = 1; i <= steps; i++) {
            move(id, x + dx * i / steps, y + dy * i / steps);
        }
        return up(id);
    }

    /** Replay a recorded fixture, keeping the clock in step with its times. */
    public Harness play(List<TouchFrame> frames) {
        if (frames.isEmpty()) {
            return this;
        }
        final long base = frames.get(0).time;
        for (TouchFrame f : frames) {
            clock.advanceTo(f.time - base);
            final TouchFrame g = f.copy();
            g.time = clock.now();
            router.handle(g);
        }
        return this;
    }

    private Harness dispatch(TouchFrame.Action action, int index) {
        frame.set(action, index, clock.now());
        for (int i = 0; i < ids.size(); i++) {
            frame.add(ids.get(i)[0], pos.get(i)[0], pos.get(i)[1]);
        }
        router.handle(frame);
        return this;
    }

    private int find(int id) {
        for (int i = 0; i < ids.size(); i++) {
            if (ids.get(i)[0] == id) {
                return i;
            }
        }
        return -1;
    }

    private void set(int id, float x, float y) {
        final int i = find(id);
        if (i < 0) {
            throw new IllegalStateException("pointer " + id + " is not down");
        }
        pos.get(i)[0] = x;
        pos.get(i)[1] = y;
    }

    // ---- recording --------------------------------------------------------

    /** Stop recording (used to ignore setup noise while still running the stack). */
    public Harness pause(boolean p) {
        paused = p;
        return this;
    }

    private void record(List<String> bucket, String line) {
        if (paused) {
            return;
        }
        bucket.add(line);
        all.add(clock.now() + " " + line);
    }

    // ---- MouseSink (the gesture layer's output) ---------------------------

    @Override
    public void mouseMove(float dx, float dy) {
        record(mouse, fmt("move %.2f,%.2f", dx, dy));
        cursor.mouseMove(dx, dy);
    }

    @Override
    public void mouseDown(int mask) {
        record(mouse, "down " + Button.maskName(mask));
        cursor.mouseDown(mask);
    }

    @Override
    public void mouseUp(int mask) {
        record(mouse, "up " + Button.maskName(mask));
        cursor.mouseUp(mask);
    }

    // ---- RegionSink -------------------------------------------------------

    @Override
    public boolean regionTapped(TapRegions.Region region, float x, float y) {
        record(regionTaps, fmt("region %s %.0f,%.0f", region.name(), x, y));
        return consumeRegionTaps;
    }

    // ---- PointerSink (what the remote machine sees) ----------------------

    @Override
    public void pointerEvent(float x, float y, int buttons) {
        record(pointer, fmt("ptr %.1f,%.1f %s", x, y, Button.maskName(buttons)));
    }

    @Override
    public void pointerEventRelative(int dx, int dy, int buttons) {
        record(pointer, fmt("rel %d,%d %s", dx, dy, Button.maskName(buttons)));
    }

    // ---- ZoomSink ---------------------------------------------------------

    @Override
    public void zoomBegan() {
        record(zoom, "zoomBegan");
        baseScale = viewport.getScale();
    }

    @Override
    public void zoomChanged(float factor) {
        record(zoom, fmt("zoomChanged %.4f", factor));
        viewport.setScale(baseScale * factor);
        if (cfg.recentreCursorOnZoom) {
            cursor.centreCursor(true);
        }
    }

    /**
     * Recorded whatever the mode, since it is what the recognizer emitted, but
     * acted on only where a host would act on it — see {@link ZoomSink}.
     */
    @Override
    public void zoomPanned(float screenDx, float screenDy) {
        record(zoom, fmt("zoomPanned %.2f,%.2f", screenDx, screenDy));
        if (cursor.isRelative()) {
            final float ox = viewport.originX(), oy = viewport.originY();
            viewport.panBy(screenDx, screenDy);
            pannedPx += Math.hypot(viewport.originX() - ox, viewport.originY() - oy);
        }
    }

    @Override
    public void zoomEnded() {
        record(zoom, "zoomEnded");
        baseScale = viewport.getScale();
    }

    @Override
    public void scaleCentre(float screenX, float screenY) {
        record(zoom, fmt("scaleCentre %.1f,%.1f", screenX, screenY));
        viewport.setFocus(viewport.toDesktopX(screenX), viewport.toDesktopY(screenY));
    }

    private float baseScale = 1.0f;

    // ---- helpers ----------------------------------------------------------

    /** Count of log lines starting with {@code prefix}. */
    public static int count(List<String> lines, String prefix) {
        int n = 0;
        for (String l : lines) {
            if (l.startsWith(prefix)) {
                n++;
            }
        }
        return n;
    }

    /** The lines starting with any of {@code prefixes}, in order. */
    public static List<String> only(List<String> lines, String... prefixes) {
        final List<String> out = new ArrayList<>();
        for (String l : lines) {
            for (String p : prefixes) {
                if (l.startsWith(p)) {
                    out.add(l);
                    break;
                }
            }
        }
        return out;
    }

    public List<String> buttonEvents() {
        return only(mouse, "down ", "up ");
    }

    private static String fmt(String f, Object... args) {
        return String.format(Locale.ROOT, f, args);
    }
}
