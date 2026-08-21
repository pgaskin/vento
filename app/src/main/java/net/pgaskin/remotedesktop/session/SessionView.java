// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.session;

import android.annotation.SuppressLint;
import android.app.Activity;
import android.content.Context;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.os.SystemClock;
import android.os.VibrationEffect;
import android.os.Vibrator;
import android.os.VibratorManager;
import android.text.InputType;
import android.util.Log;
import android.view.HapticFeedbackConstants;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.view.RoundedCorner;
import android.view.View;
import android.view.WindowInsets;
import android.view.inputmethod.EditorInfo;
import android.view.inputmethod.InputConnection;
import android.view.inputmethod.InputMethodManager;

import net.pgaskin.remotedesktop.AppSettings;
import net.pgaskin.remotedesktop.InputSettings;
import net.pgaskin.remotedesktop.backend.Backend;
import net.pgaskin.remotedesktop.backend.Monitor;
import net.pgaskin.remotedesktop.control.CursorController;
import net.pgaskin.remotedesktop.control.Viewport;
import net.pgaskin.remotedesktop.control.input.AndroidScheduler;
import net.pgaskin.remotedesktop.control.input.Button;
import net.pgaskin.remotedesktop.control.input.Config;
import net.pgaskin.remotedesktop.control.input.ExtensionKeyboard;
import net.pgaskin.remotedesktop.control.input.GestureRecognizer;
import net.pgaskin.remotedesktop.control.input.KeySink;
import net.pgaskin.remotedesktop.control.input.MouseOverlay;
import net.pgaskin.remotedesktop.control.input.PhysicalKeyboard;
import net.pgaskin.remotedesktop.control.input.PhysicalMouse;
import net.pgaskin.remotedesktop.control.input.RegionSink;
import net.pgaskin.remotedesktop.control.input.TapRegions;
import net.pgaskin.remotedesktop.control.input.Toolbar;
import net.pgaskin.remotedesktop.control.input.TouchRouter;
import net.pgaskin.remotedesktop.control.input.ZoomSink;
import net.pgaskin.remotedesktop.control.ui.Chrome;
import net.pgaskin.remotedesktop.control.ui.TextInput;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.Map;
import java.util.Set;

/**
 * The same stack the playground drives, with a real desktop at the far end:
 *
 * <pre>
 *   MotionEvent → TouchRouter → GestureRecognizer → CursorController → Backend
 *                                     │                    │
 *                                     └── ZoomSink ──→ Viewport ←┘
 * </pre>
 *
 * <p>The framebuffer is mirrored into a grid of tiles ({@link Mirror}), of
 * which only the ones that changed are re-read and only the ones on screen are
 * read at all, and drawn through the viewport transform. Why it is a grid and
 * not the one desktop-sized bitmap it used to be is a measurement rather than a
 * preference: hwui re-uploads a whole bitmap whenever any of it changes, which
 * put the size of the desktop into the cost of every frame.
 */
// No (Context, AttributeSet) constructor: a session view is given its host and
// its backend, and there is no state of either an XML attribute could carry.
@SuppressLint("ViewConstructor")
public final class SessionView extends View implements ZoomSink, CursorController.Listener,
        CursorController.PointerSink, RegionSink, Backend.Listener, KeySink,
        MouseOverlay.Listener, ExtensionKeyboard.Listener, Toolbar.Listener,
        PhysicalMouse.Listener, PhysicalKeyboard.Listener {

    public interface Host {
        /** A tap in the {@code disconnect} region. */
        void disconnectRequested();

        /** A tap in the {@code information} region. */
        void informationRequested();

        /**
         * The connection came up. The one piece of backend state the activity
         * needs for itself: credentials typed into a prompt are only known to
         * be right at this moment, and that is when they may be saved.
         */
        void connected();

        /** ... and it is over, for whatever reason. Terminal. */
        void disconnected();

        /**
         * The paste key, with something worth asking about first. Run
         * {@code proceed} if the answer is yes; what makes it worth asking is
         * {@link PasteTyper}'s question rather than this screen's.
         *
         * @param heldModifiers the locked ones, "Ctrl + Shift", or empty
         */
        void confirmPaste(int characters, String heldModifiers, Runnable proceed);

        /** The paste key, with an empty clipboard. Worth saying so. */
        void nothingToPaste();

        /**
         * The pair the backend reports itself with, unrendered: what a session
         * has to say for itself is one policy and it is the app's, not this
         * view's.
         *
         * <p>Reported rather than drawn here because it is the one thing on this
         * screen a person may need to <em>act</em> on — read a failure, try
         * again, look at the log — and an action needs a real button with a real
         * touch target, which a canvas does not have.
         */
        void status(Backend.State state, String detail);

        /**
         * The first frame of this session's desktop is on screen.
         *
         * <p>Which is the earliest moment there is anything to point at, and so
         * the moment the tap regions can be explained: before it there is a
         * black screen, and possibly a password dialog over it.
         */
        void firstFrame();
    }

    /**
     * How often the live session is asked the two things it cannot announce:
     * how its desktop is divided, and — where this connection has asked for it
     * — whether it will now take the shape of this window.
     */
    private static final long SESSION_POLL_MS = 1000;

    /**
     * Where the pointer went and when a shape came back, which together are the
     * round trip between the two — the interval a cursor change is late by, and
     * so how far past a boundary the pointer already is when the far end says
     * there was one. Silent unless {@code setprop log.tag.HoverLag VERBOSE},
     * and what set the threshold hover assist arms below.
     */
    private static final String LAG_TAG = "HoverLag";

    private final AndroidScheduler scheduler = new AndroidScheduler();
    private final Config cfg;
    private final Viewport viewport;
    private final CursorController cursor;
    private final GestureRecognizer gestures;
    private final TouchRouter router;
    private final TapRegions tapRegions = TapRegions.standard();
    private final MouseOverlay overlay;
    private final ExtensionKeyboard keyboard;
    private final Toolbar toolbar;
    private final PhysicalMouse mouse;
    private final PhysicalKeyboard keys;
    private final Chrome chrome;
    private final Backend backend;
    private final SessionClipboard clipboard;
    private final Host host;

    // The pieces of this screen that are not routing input, forwarding backend
    // events or drawing, and are small machines of their own. The fourth, the
    // HUD, is below with the switch that turns it on.
    private final SoftInput softInput;
    private final PasteTyper paste;
    private final WindowFollower follower;

    private final boolean subtleHaptics;      // or only a buzz

    /**
     * What each edge of this window costs the desktop drawn under it, left, top,
     * right, bottom — the system bars, the display cutout, and the rounded
     * corners, which is what the pan margin is measured from.
     *
     * <p>Not an inset: none of it stops the desktop being drawn there, and a
     * session is meant to run edge to edge. It is the answer to "how far in from
     * this edge is the picture actually square, unclipped and clickable", which
     * is a different number and is only used when the margin is asked for.
     *
     * <p>Starts at "not measured", which costs nothing and is not a measurement
     * either — a window whose edges really do cost nothing must still differ
     * from this, or the first insets would look like no news and the margin
     * would never be set at all.
     */
    private int[] windowEdges = {-1, -1, -1, -1};
    /** The toolbar's box, handed back to the system: see {@link #applyGestureExclusion}. */
    private final android.graphics.Rect exclusion = new android.graphics.Rect();
    private final List<android.graphics.Rect> exclusions = new ArrayList<>(1);

    // Volatile because damaged() is the one thing here that arrives on the
    // protocol's thread; everything that replaces the mirror is the main
    // thread's, which is also the thread onDraw runs on.
    private volatile Mirror mirror;
    private int tileSize; // 0 is Mirror.DEFAULT_TILE

    private Bitmap cursorShape;
    private int cursorHotX, cursorHotY;

    private final Paint bitmapPaint = new Paint(Paint.FILTER_BITMAP_FLAG);

    private float baseScale = 1.0f;
    private boolean placed;
    // Whether the far end has said how big it is. Not the same question as
    // viewport.desktopWidth() == 0, which is never true: Viewport starts at 1×1
    // so that nothing divides by zero, and placing against that sentinel opens
    // every session at maxScale on the desktop's top-left corner.
    private boolean desktopKnown;
    private Backend.State backendState = Backend.State.IDLE;

    /**
     * How the far end says its desktop is divided, last time it was asked.
     * Polled rather than announced, because that is what the seam offers and
     * why is written there; kept so the ladder is not rebuilt every second for
     * a layout that has not moved.
     */
    private List<Monitor> monitors = List.of();

    // The HUD, off unless asked for: the switch is in the settings tree, and an
    // intent extra overrides it for one session (SessionActivity).
    private final SessionHud hud;
    private boolean hudVisible;

    /**
     * What each tap region is called, while the hints are up; null when they
     * are not. The words are the app's, because {@code control} names its
     * regions and says nothing about what they are for.
     */
    private Map<String, String> regionHints;
    private boolean firstFrameSeen;
    /**
     * What is behind a desktop that has not arrived yet. Black is the letterbox
     * around one that has, and the two are different questions: an empty window
     * during a connection is a screen of this app rather than the edges of
     * somebody else's picture. Black until the app says otherwise.
     */
    private int emptyColor = 0xff000000;
    private String lastRegion = "-";

    /**
     * @param cfg the input stack's settings, which the app builds from its
     *            preferences ({@link InputSettings}) rather than this view
     *            deciding anything about the feel.
     */
    public SessionView(Context ctx, Backend backend, Host host, Config cfg) {
        super(ctx);
        this.backend = backend;
        this.host = host;
        this.cfg = cfg;
        viewport = new Viewport(cfg.density);
        cursor = new CursorController(cfg, viewport, this, scheduler);
        cursor.setListener(this);
        gestures = new GestureRecognizer(cfg, cursor, this, scheduler);
        gestures.setRegions(tapRegions, this);
        router = new TouchRouter(gestures);
        // Its own button source, so a tap on the touchpad during an
        // overlay-held drag cannot release what the overlay is holding.
        overlay = new MouseOverlay(cfg, cursor.newButtonSource(), scheduler);
        router.addClaim(overlay);
        keyboard = new ExtensionKeyboard(cfg, this, scheduler, keyList(ctx));
        router.addClaim(keyboard);
        // The three small machines the callbacks below hand their work to,
        // built before either of those two is listened to: measuring the row
        // counts as a change to it, and Chrome does that on the way in — so a
        // notification really does arrive during this constructor.
        softInput = new SoftInput(this, keyboard, overlay, () -> {
            syncToolbarState();
            applyInsets();
        });
        paste = new PasteTyper(this, scheduler, host::confirmPaste, host::nothingToPaste);
        follower = new WindowFollower(backend);
        overlay.setListener(this);
        keyboard.setListener(this);
        toolbar = new Toolbar(cfg);
        toolbar.setItems(Toolbar.standard());
        toolbar.setPosition(AppSettings.toolbarPosition(ctx));
        router.addClaim(toolbar);
        // A third button source, for the same reason the overlay has the second:
        // a mouse holding LEFT while a finger taps the touchpad must not have
        // its button released by the tap's own 250 ms window.
        mouse = new PhysicalMouse(cfg, cursor.newButtonSource());
        mouse.setListener(this);
        keys = new PhysicalKeyboard(this);
        keys.setListener(this);
        chrome = new Chrome(cfg);
        chrome.attach(keyboard);
        // Listening only now: a change notification asks the chrome for a
        // ripple, and the chrome does not exist above this line.
        toolbar.setListener(this);
        applyControls();
        clipboard = new SessionClipboard(ctx, backend::clipboardToRemote);
        hud = new SessionHud(cfg, backend, clipboard, cursor, viewport, gestures,
                overlay, keyboard, mouse, keys);
        subtleHaptics = canTick(ctx);
        setFocusable(true);
        setFocusableInTouchMode(true);
    }

    public Config config() {
        return cfg;
    }

    /**
     * The input stack's settings, as they are now: they are edited on a screen
     * of their own, and a session still connected underneath it would otherwise
     * go on running on the ones it opened with until it was reconnected.
     *
     * <p>Into the live config rather than in place of it, because every piece
     * of this stack holds <em>this</em> object and reads its fields as it goes
     * — which is what makes a swap possible at all, and is why the values are
     * nearly all of the work. What is left is whatever does not follow from a
     * field being read later: a capture is a request to the system rather than
     * a value, and the insets are computed from several of these and cached in
     * the viewport. Both are redone unconditionally rather than against a list
     * of which setting implies which, since such a list is one somebody has to
     * remember to add to and nothing fails when they do not.
     *
     * <p>Called between gestures — the settings screen is another activity, so
     * {@code onStop} has already let go of every touch, key and button this
     * session was holding.
     */
    public void applySettings(Config now) {
        cfg.copyFrom(now);
        syncPointerCapture();
        applyInsets();
    }

    /**
     * Which keys the extension row offers, from this phone's preferences: the
     * one scrolling line this app has always had, or the two-line grouping, and
     * either of them without the two modifiers that are only for a Mac.
     *
     * <p>The filter is here rather than in {@code control}, which has no idea
     * what a Mac is: the labels are that package's own, and what they mean to
     * somebody's far end is the app's question.
     */
    private static List<ExtensionKeyboard.Key> keyList(Context ctx) {
        final List<ExtensionKeyboard.Key> keys = new ArrayList<>(AppSettings.twoLineKeys(ctx)
                ? ExtensionKeyboard.twoLineKeys()
                : ExtensionKeyboard.standardKeys());
        if (!AppSettings.macKeys(ctx)) {
            keys.removeIf(k -> "Option".equals(k.label()) || "CMD".equals(k.label()));
        }
        return keys;
    }

    /**
     * The key list, as the preferences have it now. Separate from
     * {@link #applySettings}, which is about the input stack's own file.
     *
     * <p>Only when it differs: a swap lets go of every modifier held at the far
     * end, so making one for a settings screen that was opened and closed again
     * would drop a chord somebody had armed.
     */
    public void applyKeyList() {
        final List<ExtensionKeyboard.Key> want = keyList(getContext());
        if (want.equals(keyboard.allKeys())) {
            return;
        }
        keyboard.setKeys(want);
        applyInsets();
        invalidate();
    }

    public void setHudVisible(boolean show) {
        hudVisible = show;
        invalidate();
    }

    /** @see #emptyColor */
    public void setEmptyColor(int argb) {
        emptyColor = argb;
        invalidate();
    }

    /** Show the tap regions with these labels, or {@code null} to stop. */
    public void setRegionHints(Map<String, String> labels) {
        regionHints = labels;
        invalidate();
    }

    /**
     * Override {@link Mirror#DEFAULT_TILE} for this session, so the tile size can
     * be swept from the command line rather than by rebuilding.
     */
    public void setTileSize(int px) {
        tileSize = px;
    }

    // ---- Backend.Listener --------------------------------------------------

    @Override
    public void state(Backend.State state, String detail) {
        if (state == Backend.State.CONNECTED && backendState != Backend.State.CONNECTED) {
            host.connected();
        }
        final boolean wasClosed = backendState == Backend.State.CLOSED;
        backendState = state;
        host.status(state, detail);
        if (state == Backend.State.CLOSED && !wasClosed) {
            host.disconnected();
        }
        // A connection saved view-only arrives that way rather than being
        // switched to it, and a screen re-attaching to one has to catch up.
        if (state == Backend.State.CONNECTED) {
            optionsChanged();
        }
        invalidate();
    }

    @Override
    public void desktopSize(int width, int height) {
        if (mirror != null && mirror.desktopWidth() == width && mirror.desktopHeight() == height) {
            return;
        }
        if (mirror != null) {
            mirror.release();
        }
        // Every tile starts dirty, so the far end having been drawing since
        // before we knew how big it was needs no damage of its own here.
        mirror = new Mirror(width, height,
                tileSize > 0 ? tileSize : Mirror.DEFAULT_TILE);
        viewport.setDesktopSize(width, height);
        desktopKnown = true;
        place();
        frameEnd();
    }

    @Override
    public void damaged(int x, int y, int width, int height) {
        final Mirror m = mirror;
        if (m != null) {
            m.damaged(x, y, width, height);
        }
    }

    @Override
    public void frameEnd() {
        postInvalidateOnAnimation();
    }

    @Override
    public void cursor(Bitmap shape, int hotX, int hotY) {
        if (shape != cursorShape) {
            if (Log.isLoggable(LAG_TAG, Log.VERBOSE)) {
                Log.v(LAG_TAG, "cursor " + SystemClock.uptimeMillis()
                        + (shape == null ? " hidden" : " " + shape.getWidth() + "x" + shape.getHeight()));
            }
            // A reference comparison, and it is one because CursorCache hands
            // back the same bitmap for a shape it has seen before — so a
            // pointer crossing a window's edge between two shapes it already
            // had is two changes rather than a pixel comparison each way.
            gestures.remoteCursorChanged(SystemClock.uptimeMillis());
        }
        cursorShape = shape;
        cursorHotX = hotX;
        cursorHotY = hotY;
        invalidate();
    }

    /**
     * The far end has taken the cursor over, or given it back. Both halves stop
     * at once: the controller stops integrating deltas into a position, and
     * nothing is drawn here for a pointer whose position is unknown — the
     * picture arriving from the far end has one drawn into it.
     */
    @Override
    public void pointerMode(boolean relative) {
        cursor.setRelative(relative);
        gestures.setRelative(relative);
        invalidate();
    }

    @Override
    public void bell() {
        performHapticFeedback(HapticFeedbackConstants.LONG_PRESS);
    }

    @Override
    public void clipboardFromRemote(String text) {
        clipboard.fromRemote(text);
    }

    /**
     * Called inline on the protocol's thread, so it answers from the cache the
     * main thread keeps filled — see {@link SessionClipboard}.
     */
    @Override
    public String clipboardForRemote() {
        return clipboard.forRemote();
    }

    // ---- CursorController.PointerSink --------------------------------------

    @Override
    public void pointerEvent(float x, float y, int buttons) {
        if (Log.isLoggable(LAG_TAG, Log.VERBOSE)) {
            Log.v(LAG_TAG, "pointer " + Math.round(x) + " " + Math.round(y)
                    + " " + SystemClock.uptimeMillis());
        }
        backend.pointer(Math.round(x), Math.round(y), buttons);
    }

    @Override
    public void pointerEventRelative(int dx, int dy, int buttons) {
        backend.pointerRelative(dx, dy, buttons);
    }

    // ---- the view ----------------------------------------------------------

    @Override
    protected void onAttachedToWindow() {
        super.onAttachedToWindow();
        clipboard.start();
        scheduler.postDelayed(sessionTick, 0);
    }

    @Override
    protected void onDetachedFromWindow() {
        super.onDetachedFromWindow();
        clipboard.stop();
        scheduler.removeCallbacks(sessionTick);
    }

    /**
     * The two things on this screen that have to be asked for rather than
     * waited for, on one timer because they are the same kind of question. A
     * monitor layout arrives in a rectangle that may carry no size change with
     * it, so there is no callback to hang it on; and whether the far end will
     * take a size becomes true some time after the connection, so a one-shot
     * that fired too early would never fire again. The connection panel polls
     * the same class of fact at the same rate.
     */
    private final Runnable sessionTick = new Runnable() {
        @Override
        public void run() {
            // One crossing for both questions, and one answer they agree on.
            final Backend.Facts facts = backend.facts();
            // The layout on its own, not the whole snapshot: a zoom ladder
            // rebuilt every second for a desktop that has not moved would snap
            // the scale under a pinch, and canResize turning true is not news
            // about how the far end is divided.
            if (!facts.monitors().equals(monitors)) {
                monitors = facts.monitors();
                viewport.setFitSizes(fitSizes(monitors));
            }
            follower.tick(activity(), facts.canResize());
            scheduler.postDelayed(this, SESSION_POLL_MS);
        }
    };

    /**
     * One zoom rung per monitor <em>size</em>, so a pair of matched screens is
     * one step rather than two that stop in the same place. Nothing at all for
     * a desktop that is one screen, which is not a layout.
     */
    private static int[] fitSizes(List<Monitor> monitors) {
        if (monitors.size() < 2) {
            return new int[0];
        }
        final List<Integer> flat = new ArrayList<>();
        for (Monitor m : monitors) {
            boolean seen = false;
            for (int i = 0; i + 1 < flat.size(); i += 2) {
                seen |= flat.get(i) == m.width() && flat.get(i + 1) == m.height();
            }
            if (!seen) {
                flat.add(m.width());
                flat.add(m.height());
            }
        }
        final int[] sizes = new int[flat.size()];
        for (int i = 0; i < sizes.length; i++) {
            sizes[i] = flat.get(i);
        }
        return sizes;
    }

    /**
     * The clipboard is only readable while we have focus, so a copy made in
     * another app is picked up on the way back rather than when it happened —
     * and the core asks for ours the moment it is told about the focus, so this
     * has to run <em>before</em> {@code Backend.focus(true)}. That ordering is
     * why the activity calls it rather than this view overriding
     * {@code onWindowFocusChanged}.
     */
    public void refreshClipboard() {
        clipboard.read();
    }

    /**
     * The session has left the screen. Everything we are holding the remote to
     * is let go of first, because "held" here means held <em>there</em>: a
     * finger down when the app is switched away never gets an up, the click
     * window's button and a locked Ctrl are real state at the far end, and a
     * glide would keep moving somebody's cursor around a desktop they are no
     * longer looking at. The original does none of this, which is why switching
     * away from it mid-drag leaves the button down.
     */
    public void suspendInput() {
        paste.cancel();
        final long now = SystemClock.uptimeMillis();
        router.cancel(now);
        gestures.cancelAll(now);
        keyboard.clearModifiers();
        // The two that are not touches: a mouse button and a key held when the
        // session leaves the screen never get their edge either.
        mouse.cancel();
        keys.releaseAll();
    }

    @Override
    protected void onSizeChanged(int w, int h, int oldw, int oldh) {
        super.onSizeChanged(w, h, oldw, oldh);
        viewport.setViewSize(w, h);
        gestures.setViewSize(w, h);
        overlay.setViewSize(w, h);
        keyboard.setViewSize(w, h);
        toolbar.setViewSize(w, h);
        place();
        // The window changing shape rather than the orientation changing: a
        // split screen dragged about asks the same question, and this activity
        // handles its own configuration changes so there is no recreation to
        // hang it on either.
        // Bringing the tick forward rather than starting a second timer, so
        // there is one place the question is asked.
        if (follower.enabled()) {
            scheduler.removeCallbacks(sessionTick);
            scheduler.postDelayed(sessionTick, WindowFollower.DEBOUNCE_MS);
        }
    }

    /** @see WindowFollower */
    public void setFollowWindow(boolean follow) {
        follower.setEnabled(follow);
    }

    private Activity activity() {
        return getContext() instanceof Activity a ? a : null;
    }

    /**
     * First fit, or a re-fit after a rotation. The remote desktop does not
     * rotate with the phone, so the cursor keeps its desktop position and only
     * the window onto the desktop changes — but the fit-the-desktop minimum
     * moves with the aspect ratio, so the scale is re-snapped either way.
     */
    private void place() {
        if (getWidth() == 0 || !desktopKnown) {
            return;
        }
        if (!placed) {
            placed = true;
            final float w = viewport.desktopWidth(), h = viewport.desktopHeight();
            viewport.setFocus(w / 2, h / 2);
            viewport.centreOn(w / 2, h / 2, viewport.snapScale(1.0f));
            cursor.setPosition(w / 2, h / 2);
        } else {
            viewport.centreOn(cursor.x(), cursor.y(), viewport.snapScale(viewport.getScale()));
        }
        baseScale = viewport.getScale();
        invalidate();
    }

    @SuppressLint("ClickableViewAccessibility")
    @Override
    public boolean onTouchEvent(MotionEvent ev) {
        // An uncaptured mouse with a button down arrives as touch, and must not
        // reach the gesture layer: its buttons are already unambiguous.
        if (PhysicalMouse.isMouse(ev) && mouse.onTouchEvent(ev)) {
            return true;
        }
        return router.onTouchEvent(ev);
    }

    // ---- the physical mouse ------------------------------------------------

    /** An uncaptured mouse: hovering, and its wheel. */
    @Override
    public boolean onGenericMotionEvent(MotionEvent ev) {
        return mouse.onGenericMotionEvent(ev) || super.onGenericMotionEvent(ev);
    }

    /** A captured one, where the coordinates are already the deltas. */
    @Override
    public boolean onCapturedPointerEvent(MotionEvent ev) {
        return mouse.onCapturedPointerEvent(ev) || super.onCapturedPointerEvent(ev);
    }

    @Override
    public void onPointerCaptureChange(boolean hasCapture) {
        super.onPointerCaptureChange(hasCapture);
        if (!hasCapture) {
            // The pointer went back to the system with a button held down, and
            // the far end is the one holding it.
            mouse.cancel();
        }
        invalidate();
    }

    /**
     * Capture, and the two things that go with losing focus.
     *
     * <p>Asked on every focus gain rather than once: capture ends whenever the
     * window loses focus — a sheet, a dialog, the notification shade — and
     * nothing says "and now you may have it back". Asking when there is no
     * mouse costs nothing.
     */
    @Override
    public void onWindowFocusChanged(boolean hasWindowFocus) {
        super.onWindowFocusChanged(hasWindowFocus);
        if (hasWindowFocus) {
            requestFocus();
            syncPointerCapture();
            softInput.sync();
        } else {
            // Held there, not here — the same argument as suspendInput().
            mouse.cancel();
            keys.releaseAll();
        }
    }

    private void syncPointerCapture() {
        if (cfg.mouseCapture && hasWindowFocus() && isFocused()) {
            requestPointerCapture();
        } else if (hasPointerCapture()) {
            releasePointerCapture();
        }
    }

    @Override
    public void mouseActivity() {
        invalidate();
    }

    // ---- the physical keyboard ---------------------------------------------

    /**
     * Every key event the focused session sees. {@link PhysicalKeyboard} refuses
     * the ones this client keeps for itself — Back above all, since it is how a
     * session is left — and those fall through to Android. Here rather than on
     * the activity, which is dispatched first and would take the keys belonging
     * to whatever sheet is open over this screen.
     */
    @Override
    public boolean dispatchKeyEvent(KeyEvent ev) {
        if (keys.onKeyEvent(ev, this::externalKeyTyped)) {
            return true;
        }
        return super.dispatchKeyEvent(ev);
    }

    @Override
    public void keyboardActivity() {
        invalidate();
    }

    /**
     * A key from somewhere other than the extension row — the system IME or a
     * real keyboard. The row is told so that a one-shot modifier armed on it is
     * consumed by it and its info bar has something to read.
     */
    private void externalKeyTyped(int keysym) {
        keyboard.externalKey(keysym);
        invalidate();
    }

    // ---- ZoomSink ----------------------------------------------------------

    @Override
    public void zoomBegan() {
        baseScale = viewport.getScale();
    }

    @Override
    public void zoomChanged(float factor) {
        viewport.setScale(baseScale * factor);
        if (cfg.recentreCursorOnZoom) {
            cursor.centreCursor(true);
        }
        invalidate();
    }

    /**
     * A pinch pans as well, and only where the far end owns the cursor: there
     * is no centre-follow there, so a desktop bigger than the window is
     * otherwise navigated blind. With the cursor ours the desktop is already
     * wherever the cursor is, and a pan there would drag the pointer across
     * somebody's desktop for a gesture that is about looking.
     */
    @Override
    public void zoomPanned(float screenDx, float screenDy) {
        if (cursor.isRelative()) {
            viewport.panBy(screenDx, screenDy);
            invalidate();
        }
    }

    @Override
    public void zoomEnded() {
        baseScale = viewport.getScale();
    }

    @Override
    public void scaleCentre(float screenX, float screenY) {
        viewport.setFocus(viewport.toDesktopX(screenX), viewport.toDesktopY(screenY));
    }

    @Override
    public void onCursorChanged() {
        invalidate();
    }

    /**
     * A click is a key as far as the extension row is concerned: a one-shot
     * modifier armed on it is what makes Ctrl+click, and it has to let go
     * afterwards or the next click carries it too.
     */
    @Override
    public void onButtonsReleased() {
        keyboard.externalClick();
    }

    // ---- KeySink -----------------------------------------------------------

    /**
     * Straight through: {@link KeySink} and {@link Backend} agree on what a key
     * event is, and for the same reason — a release is tied to its press by the
     * id rather than by the keysym ({@code KeySink} §"the key id").
     */
    @Override
    public void keyDown(int keysym, int keyId) {
        backend.keyDown(keysym, keyId);
    }

    @Override
    public void keyUp(int keyId) {
        backend.keyUp(keyId);
    }

    // ---- the system IME ----------------------------------------------------

    @Override
    public boolean onCheckIsTextEditor() {
        return true;
    }

    /**
     * {@code TYPE_NULL} with no extract UI: there is no text field here, only a
     * remote machine to send keys to, and declaring that makes an IME send key
     * events rather than trying to manage a document.
     *
     * <p>And, unless the preference says otherwise, a password field as well.
     * {@code TYPE_NULL} does not stop a keyboard learning the words, offering
     * them back as suggestions, or shipping them off to improve itself — and
     * this field is a whole remote machine, so a password typed at it looks like
     * everything else. There is no way to mark only part of it, which is the
     * argument for marking all of it.
     */
    @Override
    public InputConnection onCreateInputConnection(EditorInfo out) {
        final boolean privateIme = AppSettings.privateIme(getContext());
        out.inputType = privateIme
                ? InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_PASSWORD
                : InputType.TYPE_NULL;
        out.imeOptions = EditorInfo.IME_FLAG_NO_EXTRACT_UI
                | EditorInfo.IME_FLAG_NO_FULLSCREEN
                | EditorInfo.IME_ACTION_NONE
                | (privateIme ? EditorInfo.IME_FLAG_NO_PERSONALIZED_LEARNING : 0);
        return new TextInput(this, this, new TextInput.Watcher() {
            @Override
            public void sent(int keysym) {
                externalKeyTyped(keysym);
            }

            @Override
            public void pasteRequested() {
                pasteClipboard();
            }

            @Override
            public Set<Integer> heldModifiers() {
                return keyboard.heldModifiers();
            }
        });
    }

    /**
     * This window's own shape, which changes for a rotation or a split screen
     * rather than for anything anybody did here, and then whatever the system
     * keyboard is doing under the extension row ({@link SoftInput}). Both, and
     * then one re-inset: the first would not catch the second, since neither a
     * rotation nor a split screen moves the IME.
     */
    @Override
    public WindowInsets onApplyWindowInsets(WindowInsets insets) {
        final int[] edges = windowEdges(insets);
        boolean changed = !Arrays.equals(edges, windowEdges);
        windowEdges = edges;
        // Not short-circuiting: the IME is asked whatever the edges said.
        changed |= softInput.imeInsets(insets);
        if (changed) {
            applyInsets();
            invalidate();
        }
        return super.onApplyWindowInsets(insets);
    }

    /**
     * What the far edges of this window are lost to. The bars and the cutout are
     * an inset and arrive as one; the rounded corners are not an inset at all —
     * they are a radius per corner, of which each side takes the larger of the
     * two it has, since a picture pulled that far in from an edge clears the arc
     * at both of its ends.
     *
     * <p>Ignoring visibility, for the reason {@link #deviceSize} gives: a bar
     * hidden this instant comes back, and limits that moved every time sticky
     * immersive flashed one up would drag the picture about with them.
     */
    private static int[] windowEdges(WindowInsets insets) {
        final android.graphics.Insets bars = insets.getInsetsIgnoringVisibility(
                WindowInsets.Type.systemBars() | WindowInsets.Type.displayCutout());
        final int tl = cornerRadius(insets, RoundedCorner.POSITION_TOP_LEFT);
        final int tr = cornerRadius(insets, RoundedCorner.POSITION_TOP_RIGHT);
        final int br = cornerRadius(insets, RoundedCorner.POSITION_BOTTOM_RIGHT);
        final int bl = cornerRadius(insets, RoundedCorner.POSITION_BOTTOM_LEFT);
        return new int[]{
                Math.max(bars.left, Math.max(tl, bl)),
                Math.max(bars.top, Math.max(tl, tr)),
                Math.max(bars.right, Math.max(tr, br)),
                Math.max(bars.bottom, Math.max(bl, br)),
        };
    }

    /**
     * A corner this window has not got — a square display, or the half of a
     * split screen that does not reach it — costs it nothing.
     */
    private static int cornerRadius(WindowInsets insets, int position) {
        final RoundedCorner corner = insets.getRoundedCorner(position);
        return corner == null ? 0 : corner.getRadius();
    }

    // ---- MouseOverlay.Listener ---------------------------------------------

    @Override
    public void overlayChanged() {
        // Bump scroll arms for a drag started while the overlay holds a button,
        // the same as it does inside the 250 ms click window.
        gestures.setExternalButtonHeld((overlay.heldMask() & Button.DRAG_MASK) != 0);
        softInput.overlayChanged();
        invalidate();
    }

    // ---- ExtensionKeyboard.Listener ----------------------------------------

    @Override
    public void keyboardChanged() {
        softInput.keyboardChanged();
        chrome.keyboardChanged(keyboard);
        invalidate();
    }

    /**
     * Send the soft keyboard back to its letters, which is what the original
     * does and what makes the two rows feel like one keyboard: a chord is a
     * modifier here and a letter down there, and reaching for Ctrl while the
     * IME is showing symbols otherwise means going back for the letters by hand.
     *
     * <p>{@code restartInput} is the whole of it: there is no API for "show the
     * alphabetic page", only a way to tell the IME its target has changed, which
     * every IME answers by starting again at its default page.
     */
    @Override
    public void modifierPressed(ExtensionKeyboard.Key key) {
        if (!AppSettings.modifierResetsIme(getContext())) {
            return;
        }
        final InputMethodManager imm = getContext().getSystemService(InputMethodManager.class);
        if (imm != null && imm.isActive(this)) {
            imm.restartInput(this);
        }
    }

    @Override
    public void keyFeedback(ExtensionKeyboard.Feedback what) {
        performHapticFeedback(switch (what) {
            case LOCK -> HapticFeedbackConstants.LONG_PRESS;
            case PRESS -> HapticFeedbackConstants.KEYBOARD_TAP;
            case REPEAT -> subtleHaptics
                    ? HapticFeedbackConstants.SEGMENT_FREQUENT_TICK
                    : HapticFeedbackConstants.NO_HAPTICS;
        });
    }

    /**
     * The row's one action key, which types the clipboard out rather than
     * sending a Ctrl+V ({@link PasteTyper}).
     */
    @Override
    public void keyAction(String name) {
        if (!ExtensionKeyboard.ACTION_PASTE.equals(name)) {
            return;
        }
        pasteClipboard();
    }

    /**
     * Type the clipboard out. Two things ask for this: the row's own Paste key,
     * and an IME with a clipboard key of its own, which asks the editor to paste
     * rather than committing any text ({@link TextInput#performContextMenuAction}).
     */
    private void pasteClipboard() {
        clipboard.read();
        paste.paste(clipboard.current(), PasteTyper.lockedModifiers(keyboard));
    }

    private static boolean canTick(Context ctx) {
        final VibratorManager vm = ctx.getSystemService(VibratorManager.class);
        if (vm == null) {
            return false;
        }
        final Vibrator v = vm.getDefaultVibrator();
        return v != null && v.hasVibrator()
                && v.areAllPrimitivesSupported(VibrationEffect.Composition.PRIMITIVE_TICK);
    }

    /**
     * What the viewport clamps inside: whatever the overlay or the keyboard is
     * covering. Through the cursor rather than the viewport directly, because
     * the desktop must not jump when a widget appears over it — see
     * {@link CursorController#setInsets}.
     */
    private void applyInsets() {
        final int right = (int) overlay.insetRightPx();
        int bottom = (int) Math.max(overlay.insetBottomPx(), keyboard.insetBottomPx());
        // The IME on its own, in case it outlives the row that asked for it.
        bottom = Math.max(bottom, softInput.imeHeight());
        // Nothing insets the left or the top, so there those margins are the
        // whole of what the edge costs. Before the insets rather than after, so
        // that the re-clamp the insets end with is the one that has the last
        // word; a margin that changed alone re-clamps itself.
        viewport.setPanMargins(panMargin(windowEdges[0], 0), panMargin(windowEdges[1], 0),
                panMargin(windowEdges[2], right), panMargin(windowEdges[3], bottom));
        cursor.setInsets(0, 0, right, bottom);
        // Not one of those: the toolbar floats over the picture. What it needs
        // is the band it may be dragged in — inside this window's own edges, and
        // clear of what the keyboard *occupies*, which is a bigger number than
        // what the keyboard insets by.
        // Flush to the left edge, whatever that edge costs: this is a widget
        // over the picture rather than part of it, and a column indented by a
        // corner radius reads as a panel that has come loose. Only the vertical
        // ends are held off, since those are where a bar or a cutout is.
        toolbar.setInsets(0, windowEdges[1],
                Math.max(windowEdges[3], keyboard.heightPx()));
        baseScale = viewport.getScale();
        invalidate();
    }

    /**
     * How far in from one edge of the window a pan may bring the desktop's edge:
     * the two settings, which are two reasons to want the same thing and are
     * added rather than gated on each other, so either alone is a margin.
     *
     * <p>What this edge costs, if that setting is on — less whatever an inset
     * has already moved the picture clear of it, since the extension keyboard
     * stands on the navigation bar and the desktop above the keyboard is already
     * above the bar. Plus the flat margin, if there is one, which is about where
     * an edge is comfortable rather than what is over it and so is not something
     * an inset answers.
     */
    private int panMargin(int edge, int inset) {
        return (cfg.panMarginInsets ? Math.max(edge - inset, 0) : 0) + (int) cfg.panMarginPx;
    }

    // ---- RegionSink --------------------------------------------------------

    @Override
    public boolean regionTapped(TapRegions.Region region, float x, float y) {
        lastRegion = region.name();
        return controlAction(region.name());
    }

    /**
     * What either affordance does, in the one place both go through, so the two
     * cannot drift apart in what they do — only in how they are reached.
     *
     * @return whether it was done. The two input actions are not there at all on
     * a view-only session: refused rather than consumed, so a tap in a region
     * clicks as usual and does visibly nothing, like a tap anywhere else on a
     * view-only desktop ({@code TapRegions} §"the handler decides"). The toolbar
     * does not need the answer, since it simply does not offer those two.
     */
    private boolean controlAction(String name) {
        switch (name) {
            case TapRegions.DISCONNECT -> host.disconnectRequested();
            case TapRegions.INFORMATION -> host.informationRequested();
            case TapRegions.MOUSE -> {
                if (backend.viewOnly()) {
                    return false;
                }
                softInput.toggleOverlay();
            }
            case TapRegions.KEYBOARD -> {
                if (backend.viewOnly()) {
                    return false;
                }
                softInput.toggleKeyboard();
            }
            default -> {
                return false;
            }
        }
        return true;
    }

    // ---- Toolbar.Listener --------------------------------------------------

    @Override
    public void toolbarChanged() {
        chrome.toolbarChanged(toolbar);
        applyGestureExclusion();
        invalidate();
    }

    @Override
    public void toolbarAction(String name) {
        lastRegion = name;
        controlAction(name);
    }

    @Override
    public void toolbarMoved(float fraction) {
        AppSettings.setToolbarPosition(getContext(), fraction);
    }

    /**
     * The left edge is where the system's back gesture is, and a claimed pointer
     * does not change that: the platform decides before this view sees the
     * stream. So the box the toolbar occupies is handed back through the one
     * mechanism there is for it, and handed back again every time it moves —
     * which is why this hangs off the change notification rather than the
     * layout.
     */
    private void applyGestureExclusion() {
        exclusions.clear();
        if (toolbar.visible()) {
            exclusion.set((int) toolbar.left(), (int) toolbar.top(),
                    (int) toolbar.right(), (int) toolbar.bottom());
            exclusions.add(exclusion);
        }
        setSystemGestureExclusionRects(exclusions);
    }

    /**
     * Which affordance this session offers, from this phone's preferences, and
     * what the toolbar has to say about the two widgets that have a state.
     * Applied to a running session, since it is answered on the first frame's
     * dialog with the session already underneath it.
     */
    public void applyControls() {
        final Context ctx = getContext();
        gestures.setRegions(AppSettings.regionsShown(ctx) ? tapRegions : null, this);
        toolbar.setVisible(AppSettings.toolbarShown(ctx));
        syncToolbarState();
        invalidate();
    }

    /**
     * A view-only session gets a shorter column rather than two dead buttons:
     * the keyboard and the mouse overlay are not inactive there, they are
     * absent. And the two that remain report what they are showing, the way the
     * info bar reports an armed modifier.
     */
    private void syncToolbarState() {
        toolbar.setItems(backend.viewOnly()
                ? List.of(Toolbar.standard().get(0), Toolbar.standard().get(1))
                : Toolbar.standard());
        toolbar.setActive(TapRegions.MOUSE, overlay.visible());
        toolbar.setActive(TapRegions.KEYBOARD, keyboard.visible());
    }

    /**
     * Something about the connection changed that this screen shows — today,
     * only whether it is view-only, which is a control on the connection panel
     * and so can be turned on with the keyboard and the overlay already open.
     * Both are put away rather than left inert: a row of keys that types nothing
     * is indistinguishable from a session that has stopped answering.
     */
    public void optionsChanged() {
        if (backend.viewOnly()) {
            softInput.hideAll();
        }
        syncToolbarState();
        invalidate();
    }

    // ---- drawing -----------------------------------------------------------

    @Override
    protected void onDraw(Canvas c) {
        c.drawColor(mirror != null ? 0xff000000 : emptyColor);

        if (mirror != null) {
            // Pull whatever changed where we are looking, on the drawing thread,
            // because a fetch must never queue behind connection work. What is
            // off screen keeps its damage until it is looked at.
            mirror.update(backend,
                    viewport.toDesktopX(0), viewport.toDesktopY(0),
                    viewport.toDesktopX(getWidth()), viewport.toDesktopY(getHeight()));
            final int save = c.save();
            c.translate(viewport.originX(), viewport.originY());
            c.scale(viewport.getScale(), viewport.getScale());
            mirror.draw(c, bitmapPaint);
            c.restoreToCount(save);
            drawCursor(c);
            // Here rather than on the backend's frame notification, which is
            // also sent for the desktop size arriving — before there are any
            // pixels. "On screen" is this line having run.
            if (!firstFrameSeen) {
                firstFrameSeen = true;
                post(host::firstFrame);
            }
        }

        if (overlay.visible()) {
            chrome.drawOverlay(c, overlay);
        }
        if (keyboard.visible() && chrome.drawKeyboard(c, keyboard, getWidth(), cursor.screenY())) {
            postInvalidateOnAnimation();
        }
        if (chrome.drawToolbar(c, toolbar, getWidth(), getHeight(),
                cursor.screenX(), cursor.screenY())) {
            postInvalidateOnAnimation();
        }

        // Over the desktop, and under the status panel, which is a view rather
        // than ink and is above everything here.
        if (regionHints != null) {
            chrome.drawRegionHints(c, tapRegions, getWidth(), getHeight(), 1f, regionHints::get);
        }
        if (hudVisible) {
            hud.draw(c, getWidth(), getHeight(), mirror, backendState, lastRegion,
                    softInput.imeHeight(), hasPointerCapture());
            // The rates are only meaningful if there is a next frame to compare
            // against, and a still desktop produces none.
            postInvalidateOnAnimation();
        }
    }

    /**
     * The remote cursor, capped at 32 logical pixels as the original caps it,
     * with the hotspot already un-negated by the backend.
     */
    private void drawCursor(Canvas c) {
        if (cursor.isRelative()) {
            // Drawing a shape at a position nobody knows would put a second,
            // wrong pointer on a picture that already has the real one in it.
            return;
        }
        chrome.drawCursor(c, cursorShape, cursorHotX, cursorHotY,
                cursor.screenX(), cursor.screenY(), bitmapPaint);
    }

    /** Let go of the bitmaps; the backend outlives the view only briefly. */
    public void release() {
        if (mirror != null) {
            mirror.release();
            mirror = null;
        }
    }
}
