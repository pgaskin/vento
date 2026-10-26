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
 * which only changed tiles are re-read and only visible ones are read at all,
 * and drawn through the viewport transform. It is a grid rather than one
 * desktop-sized bitmap because of a measurement: hwui re-uploads a whole bitmap
 * whenever any of it changes, which made the cost of every frame depend on the
 * desktop size.
 */
// No (Context, AttributeSet) constructor: the view takes its host and backend,
// which an XML attribute cannot provide.
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
         * The connection is established. The activity needs this for itself:
         * credentials entered at a prompt are only known to be correct now,
         * and can be saved.
         */
        void connected();

        /** The connection ended, for any reason. Terminal. */
        void disconnected();

        /**
         * The paste key was pressed and needs confirmation. Run
         * {@code proceed} if confirmed; {@link PasteTyper} decides when to
         * ask.
         *
         * @param heldModifiers the locked ones, "Ctrl + Shift", or empty
         */
        void confirmPaste(int characters, String heldModifiers, Runnable proceed);

        /** The paste key was pressed with an empty clipboard. */
        void nothingToPaste();

        /**
         * The backend's state and detail, as reported: how a session's status
         * is shown is decided by the app, not this view.
         *
         * <p>Reported rather than drawn here because the user may need to
         * <em>act</em> on it (read a failure, reconnect, open the log), which
         * needs real buttons with real touch targets that a canvas lacks.
         */
        void status(Backend.State state, String detail);

        /**
         * What the connection is waiting for while it connects, a line under
         * the status, or null for none: {@link Backend.Listener#status}.
         */
        void statusLine(String text);

        /**
         * The first frame of this session's desktop is on screen.
         *
         * <p>The earliest moment the tap regions can be explained over the
         * desktop: before it there is a black screen, possibly with a password
         * dialog over it.
         */
        void firstFrame();

        /**
         * The session's sound switch changed: offered, withdrawn, toggled, or
         * its sound started or stopped. The switch is on the connection panel,
         * which belongs to the activity and may be open at the time; this view
         * only shows it in the HUD line, which is redrawn every frame anyway.
         */
        void soundChanged();
    }

    /**
     * How often the session is polled for the two things it cannot announce:
     * its monitor layout, and (if this connection follows the window size)
     * whether it accepts a resize.
     */
    private static final long SESSION_POLL_MS = 1000;

    /**
     * Logs pointer moves and cursor shape changes with timestamps, which give
     * the round trip between them: how late a cursor change arrives, and so how
     * far past a boundary the pointer already is when the far end reports it.
     * Silent unless {@code setprop log.tag.HoverLag VERBOSE}. The hover assist
     * threshold was set from these measurements.
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

    // Helpers for the parts of this screen that are not input routing, backend
    // event forwarding or drawing. The fourth, the HUD, is below with its
    // visibility flag.
    private final SoftInput softInput;
    private final PasteTyper paste;
    private final WindowFollower follower;

    private final boolean subtleHaptics;      // whether the vibrator supports tick primitives

    /**
     * How much of each edge of this window (left, top, right, bottom) is covered
     * by the system bars, the display cutout and the rounded corners. The pan
     * margin is measured from this.
     *
     * <p>Not an inset: the desktop is still drawn there, edge to edge. It is how
     * far in from each edge the picture is unobstructed and clickable, and is
     * only used when the pan margin setting is on.
     *
     * <p>Starts at -1 ("not measured") rather than 0, so that a window whose
     * edges really are all 0 still differs from it, and the first insets are
     * applied.
     */
    private int[] windowEdges = {-1, -1, -1, -1};
    /** The toolbar's box, handed back to the system: see {@link #applyGestureExclusion}. */
    private final android.graphics.Rect exclusion = new android.graphics.Rect();
    private final List<android.graphics.Rect> exclusions = new ArrayList<>(1);

    // Volatile because damaged() is called on the protocol thread; the mirror
    // is only replaced on the main thread, which is also where onDraw runs.
    private volatile Mirror mirror;
    private int tileSize; // 0 is Mirror.DEFAULT_TILE

    private Bitmap cursorShape;
    private int cursorHotX, cursorHotY;

    private final Paint bitmapPaint = new Paint(Paint.FILTER_BITMAP_FLAG);

    private float baseScale = 1.0f;
    private boolean placed;
    // Whether the far end has reported its size. Not the same as
    // viewport.desktopWidth() == 0, which is never true: Viewport starts at 1×1
    // so that nothing divides by zero, and placing against that opened every
    // session at maxScale on the desktop's top-left corner.
    private boolean desktopKnown;
    private Backend.State backendState = Backend.State.IDLE;

    /**
     * The far end's monitor layout as of the last poll. Polled rather than
     * announced, as {@link Backend.Facts} explains; kept so the zoom ladder is
     * not rebuilt every second for an unchanged layout.
     */
    private List<Monitor> monitors = List.of();

    // The HUD, off unless enabled in the settings or, for one session, by an
    // intent extra (SessionActivity).
    private final SessionHud hud;
    private boolean hudVisible;

    /**
     * The label of each tap region while the hints are shown, or null. The
     * labels come from the app, because {@code control} only names its regions
     * and does not describe what they do.
     */
    private Map<String, String> regionHints;
    private boolean firstFrameSeen;
    /**
     * The background before the desktop arrives. Once it has, the letterbox
     * around it is black, but an empty window while connecting is an app screen
     * rather than the edges of the remote picture. Black unless the app sets
     * another color.
     */
    private int emptyColor = 0xff000000;
    private String lastRegion = "-";

    /**
     * @param cfg the input stack's settings, which the app builds from its
     *            preferences ({@link InputSettings})
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
        // The three helpers the callbacks below delegate to, built before the
        // overlay and keyboard listeners are set: measuring the row counts as a
        // change to it, and Chrome measures it when attached, so a change
        // notification arrives during this constructor.
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
        // A third button source, for the same reason the overlay has its own: a
        // mouse holding LEFT while a finger taps the touchpad must not have its
        // button released by the tap's 250 ms click window.
        mouse = new PhysicalMouse(cfg, cursor.newButtonSource());
        mouse.setListener(this);
        keys = new PhysicalKeyboard(this);
        keys.setListener(this);
        chrome = new Chrome(cfg);
        chrome.attach(keyboard);
        // Only now: a change notification asks the chrome for a ripple, and the
        // chrome is created just above.
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
     * Apply the current input settings. They are edited on another screen, and
     * a session still connected would otherwise keep the ones it opened with
     * until it reconnected.
     *
     * <p>Copied into the live config rather than replacing it, because every
     * part of the input stack holds <em>this</em> object and reads its fields
     * as needed, so copying the values does nearly all of the work. The rest
     * does not follow from reading a field later: pointer capture is a request
     * to the system rather than a value, and the insets are computed from
     * several settings and cached in the viewport. Both are redone
     * unconditionally rather than based on which settings changed, since such a
     * mapping would be easy to forget to update and nothing would fail.
     *
     * <p>Called between gestures: the settings screen is another activity, so
     * {@code onStop} has already released every touch, key and button this
     * session was holding.
     */
    public void applySettings(Config now) {
        cfg.copyFrom(now);
        syncPointerCapture();
        applyInsets();
    }

    /**
     * The extension row's keys, from the preferences: the single scrolling line
     * or the two-line grouping, either optionally without the two Mac-only
     * modifiers.
     *
     * <p>The filter is here rather than in {@code control}, which knows nothing
     * about platforms: the labels are that package's, but what they mean on the
     * far end is the app's concern.
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
     * Apply the key list from the current preferences. Separate from
     * {@link #applySettings}, which covers the input stack's own preferences.
     *
     * <p>Only when it differs: replacing the keys releases every modifier held
     * at the far end, so doing it after merely opening and closing the settings
     * would drop a chord the user had armed.
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
     * Override {@link Mirror#DEFAULT_TILE} for this session, so tile sizes can
     * be tested from the command line without rebuilding.
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
        // A connection saved as view-only starts that way rather than being
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
        // Every tile starts dirty, so damage sent before the size was known
        // does not need to be replayed.
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
            // A reference comparison, which works because CursorCache returns
            // the same bitmap for a shape it has seen before, so a pointer
            // crossing between two known shapes is two changes without
            // comparing pixels.
            gestures.remoteCursorChanged(SystemClock.uptimeMillis());
        }
        cursorShape = shape;
        cursorHotX = hotX;
        cursorHotY = hotY;
        invalidate();
    }

    /**
     * The far end has taken over the cursor, or given it back. In relative
     * mode the controller stops integrating deltas into a position, and no
     * cursor is drawn here, since its position is unknown and the far end draws
     * it into the picture.
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
    public void status(String text) {
        host.statusLine(text);
    }

    @Override
    public void soundSwitch(int soundSwitch, boolean enabled, boolean playing) {
        host.soundChanged();
    }

    @Override
    public void clipboardFromRemote(String text) {
        clipboard.fromRemote(text);
    }

    /**
     * Called inline on the protocol thread, so it answers from the cache the
     * main thread keeps filled (see {@link SessionClipboard}).
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

    // ---- View -------------------------------------------------------------

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
     * Polls the two things this screen cannot be notified of, on one timer. A
     * monitor layout arrives in a rectangle that may not change the desktop
     * size, so there is no callback for it; and the far end may only accept a
     * resize some time after connecting, so a one-shot request sent too early
     * would never be retried. The connection panel polls the same facts at the
     * same rate.
     */
    private final Runnable sessionTick = new Runnable() {
        @Override
        public void run() {
            // One backend call for both, so they come from the same snapshot.
            final Backend.Facts facts = backend.facts();
            // Compare the layout alone, not the whole snapshot: rebuilding the
            // zoom ladder every second for an unchanged layout would snap the
            // scale during a pinch, and canResize changing does not change the
            // layout.
            if (!facts.monitors().equals(monitors)) {
                monitors = facts.monitors();
                viewport.setFitSizes(fitSizes(monitors));
            }
            follower.tick(activity(), facts.canResize());
            scheduler.postDelayed(this, SESSION_POLL_MS);
        }
    };

    /**
     * One zoom step per distinct monitor <em>size</em>, so two identical
     * monitors give one step rather than two at the same scale. None for a
     * single-monitor desktop.
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
     * The clipboard is only readable while this window has focus, so a copy
     * made in another app is picked up on return rather than when it happened.
     * The core requests the local clipboard as soon as it gets the focus, so
     * this has to run <em>before</em> {@code Backend.focus(true)}, which is why
     * the activity calls it rather than this view's
     * {@code onWindowFocusChanged}.
     */
    public void refreshClipboard() {
        clipboard.read();
    }

    /**
     * The session has left the screen. Everything held is released first,
     * because it is held at the far end: a finger down when the app is switched
     * away never gets an up event, the click window's button and a locked Ctrl
     * are real state at the far end, and a glide would keep moving the cursor
     * on a desktop that is not being viewed. The original does none of this,
     * which is why switching away from it mid-drag leaves the button down.
     */
    public void suspendInput() {
        paste.cancel();
        final long now = SystemClock.uptimeMillis();
        router.cancel(now);
        gestures.cancelAll(now);
        keyboard.clearModifiers();
        // Not touches, but the same problem: a mouse button or key held when
        // the session leaves the screen never gets its release either.
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
        // On any window size change rather than only orientation changes: a
        // split screen being resized needs the same, and this activity handles
        // its own configuration changes, so it is not recreated.
        // Rescheduling the existing tick rather than starting a second timer,
        // so the request is made in one place.
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
     * the view onto the desktop changes; but the fit-the-desktop minimum scale
     * depends on the aspect ratio, so the scale is re-snapped either way.
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

    // ---- physical mouse -----------------------------------------------------

    /** An uncaptured mouse: hovering, and its wheel. */
    @Override
    public boolean onGenericMotionEvent(MotionEvent ev) {
        return mouse.onGenericMotionEvent(ev) || super.onGenericMotionEvent(ev);
    }

    /** A captured mouse, where the coordinates are already deltas. */
    @Override
    public boolean onCapturedPointerEvent(MotionEvent ev) {
        return mouse.onCapturedPointerEvent(ev) || super.onCapturedPointerEvent(ev);
    }

    @Override
    public void onPointerCaptureChange(boolean hasCapture) {
        super.onPointerCaptureChange(hasCapture);
        if (!hasCapture) {
            // The pointer may have returned to the system with a button down,
            // which the far end would keep holding.
            mouse.cancel();
        }
        invalidate();
    }

    /**
     * Pointer capture on focus gain, and releasing held input on focus loss.
     *
     * <p>Capture is requested on every focus gain rather than once: it ends
     * whenever the window loses focus (a sheet, a dialog, the notification
     * shade), and nothing signals that it can be requested again. Requesting it
     * without a mouse costs nothing.
     */
    @Override
    public void onWindowFocusChanged(boolean hasWindowFocus) {
        super.onWindowFocusChanged(hasWindowFocus);
        if (hasWindowFocus) {
            requestFocus();
            syncPointerCapture();
            softInput.sync();
        } else {
            // Held at the far end, as in suspendInput().
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

    // ---- physical keyboard --------------------------------------------------

    /**
     * Every key event the focused session gets. {@link PhysicalKeyboard} refuses
     * the ones this client handles itself (mainly Back, which leaves the
     * session), and those fall through to Android. Here rather than in the
     * activity, which is dispatched first and would also take the keys meant
     * for a sheet open over this screen.
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
     * A key from somewhere other than the extension row (the system IME or a
     * physical keyboard). The row is notified so that a one-shot modifier armed
     * on it is consumed and its info bar is updated.
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
     * A pinch also pans, but only when the far end owns the cursor: there is no
     * centre-follow then, so a desktop bigger than the window could not
     * otherwise be navigated. When the cursor is local the view already follows
     * it, and a pan would drag the pointer across the desktop during a gesture
     * meant only for viewing.
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
     * The extension row treats a click like a key: a one-shot modifier armed on
     * it makes Ctrl+click, and has to be released afterwards or the next click
     * would carry it too.
     */
    @Override
    public void onButtonsReleased() {
        keyboard.externalClick();
    }

    // ---- KeySink -----------------------------------------------------------

    /**
     * Passed straight through: {@link KeySink} and {@link Backend} use the same
     * key event model, where a release is tied to its press by the id rather
     * than the keysym ({@code KeySink} §"The key id").
     */
    @Override
    public void keyDown(int keysym, int keyId) {
        backend.keyDown(keysym, keyId);
    }

    @Override
    public void keyUp(int keyId) {
        backend.keyUp(keyId);
    }

    // ---- system IME ---------------------------------------------------------

    @Override
    public boolean onCheckIsTextEditor() {
        return true;
    }

    /**
     * {@code TYPE_NULL} with no extract UI: there is no text field here, only a
     * remote machine to send keys to, and declaring that makes an IME send key
     * events rather than editing a document.
     *
     * <p>With the private IME preference on, a password field instead.
     * {@code TYPE_NULL} does not stop a keyboard from learning the words,
     * suggesting them, or uploading them to improve itself, and since this
     * field covers the whole remote machine, a password typed into it looks
     * like any other text. Part of it cannot be marked, so all of it is.
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
     * The window's edges, which change on a rotation or a split screen resize,
     * and the system keyboard under the extension row ({@link SoftInput}). Both
     * are checked, then the insets are applied once: checking only the edges
     * would miss IME changes, since neither a rotation nor a split screen
     * moves the IME.
     */
    @Override
    public WindowInsets onApplyWindowInsets(WindowInsets insets) {
        final int[] edges = windowEdges(insets);
        boolean changed = !Arrays.equals(edges, windowEdges);
        windowEdges = edges;
        // Not short-circuiting: the IME is checked whatever the edges did.
        changed |= softInput.imeInsets(insets);
        if (changed) {
            applyInsets();
            invalidate();
        }
        return super.onApplyWindowInsets(insets);
    }

    /**
     * How much of each edge of this window is obstructed. The bars and the
     * cutout are insets; the rounded corners are a radius per corner, and each
     * side takes the larger of its two corners, since a picture that far in
     * from the edge clears the arc at both ends.
     *
     * <p>Ignoring visibility, for the reason
     * {@link WindowFollower#deviceSize} gives: a hidden bar comes back, and
     * limits that changed every time sticky immersive showed one would move the
     * picture with them.
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
     * Zero for a corner this window does not have (a square display, or a
     * split screen half that does not reach it).
     */
    private static int cornerRadius(WindowInsets insets, int position) {
        final RoundedCorner corner = insets.getRoundedCorner(position);
        return corner == null ? 0 : corner.getRadius();
    }

    // ---- MouseOverlay.Listener ---------------------------------------------

    @Override
    public void overlayChanged() {
        // Bump scroll arms for a drag started while the overlay holds a button,
        // as it does inside the 250 ms click window.
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
     * Return the soft keyboard to its letters, as the original does, which
     * makes the two rows feel like one keyboard: a chord is a modifier on the
     * row plus a letter on the IME, and pressing Ctrl while the IME shows
     * symbols would otherwise mean switching back to letters by hand.
     *
     * <p>{@code restartInput} is the only way: there is no API to show the
     * alphabetic page, only one to tell the IME its target has changed, which
     * every IME handles by returning to its default page.
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
     * The row's only action key, which types the clipboard rather than sending
     * Ctrl+V ({@link PasteTyper}).
     */
    @Override
    public void keyAction(String name) {
        if (!ExtensionKeyboard.ACTION_PASTE.equals(name)) {
            return;
        }
        pasteClipboard();
    }

    /**
     * Type the clipboard. Called for the row's Paste key, and for an IME's own
     * clipboard key, which asks the editor to paste rather than committing text
     * ({@link TextInput#performContextMenuAction}).
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
     * Set the area the viewport clamps to, excluding whatever the overlay or the
     * keyboard covers. Through the cursor rather than the viewport directly,
     * because the desktop must not jump when a widget appears over it (see
     * {@link CursorController#setInsets}).
     */
    private void applyInsets() {
        final int right = (int) overlay.insetRightPx();
        int bottom = (int) Math.max(overlay.insetBottomPx(), keyboard.insetBottomPx());
        // The IME on its own, in case it outlives the row that requested it.
        bottom = Math.max(bottom, softInput.imeHeight());
        // Nothing insets the left or the top, so those margins are the full
        // edge size. Set before the insets so that the re-clamp setInsets ends
        // with comes last; a margin that changed alone re-clamps by itself.
        viewport.setPanMargins(panMargin(windowEdges[0], 0), panMargin(windowEdges[1], 0),
                panMargin(windowEdges[2], right), panMargin(windowEdges[3], bottom));
        cursor.setInsets(0, 0, right, bottom);
        // The toolbar floats over the picture, so it is not inset like the
        // desktop. It needs the band it may be dragged in: inside this window's
        // edges, and clear of the keyboard's full height, which is larger than
        // the keyboard's inset.
        // Flush to the left edge regardless of what covers it: the toolbar is
        // a widget over the picture, and a column indented by a corner radius
        // looks detached. Only the top and bottom are kept clear, since that is
        // where the bars and cutouts are.
        toolbar.setInsets(0, windowEdges[1],
                Math.max(windowEdges[3], keyboard.heightPx()));
        baseScale = viewport.getScale();
        invalidate();
    }

    /**
     * How far in from one window edge a pan may bring the desktop's edge: the
     * two settings added together, so either alone gives a margin.
     *
     * <p>The edge's obstructed size, if that setting is on, minus the inset
     * already clearing it, since the extension keyboard sits on the navigation
     * bar and the desktop above the keyboard is already clear of the bar. Plus
     * the fixed margin, if set, which is for comfort rather than obstruction and
     * so is not reduced by an inset.
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
     * The action for a tap region or toolbar button, in one place so that both
     * do the same thing.
     *
     * @return whether it was handled. The two input actions do not exist on a
     * view-only session: they are refused rather than consumed, so a tap in
     * their region is passed on as a click and visibly does nothing, like a tap
     * anywhere else on a view-only desktop ({@code TapRegions} §"the handler
     * decides"). The toolbar ignores the result, since it does not offer those
     * two.
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
     * The left edge has the system back gesture, and claiming the pointer does
     * not prevent it: the platform decides before this view gets the events. So
     * the toolbar's box is excluded from system gestures, and updated every time
     * it moves, which is why this runs on the change notification rather than
     * on layout.
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
     * Apply the controls preference (toolbar, tap regions, or both), and the
     * toolbar's state for the two toggles. Applied to a running session, since
     * the choice is made in the first-frame dialog with the session underneath.
     */
    public void applyControls() {
        final Context ctx = getContext();
        gestures.setRegions(AppSettings.regionsShown(ctx) ? tapRegions : null, this);
        toolbar.setVisible(AppSettings.toolbarShown(ctx));
        syncToolbarState();
        invalidate();
    }

    /**
     * A view-only session gets a shorter toolbar rather than two disabled
     * buttons, since the keyboard and the mouse overlay are not available. The
     * keyboard and mouse buttons show whether their widget is visible, as the
     * info bar shows an armed modifier.
     */
    private void syncToolbarState() {
        toolbar.setItems(backend.viewOnly()
                ? List.of(Toolbar.standard().get(0), Toolbar.standard().get(1))
                : Toolbar.standard());
        toolbar.setActive(TapRegions.MOUSE, overlay.visible());
        toolbar.setActive(TapRegions.KEYBOARD, keyboard.visible());
    }

    /**
     * A connection option this screen depends on changed. Currently only
     * view-only, which is a control on the connection panel and so can be
     * turned on with the keyboard and the overlay open; both are then hidden
     * ({@link SoftInput#hideAll}).
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
            // Read the visible changed tiles on the drawing thread, because a
            // fetch must never queue behind connection work. Off-screen tiles
            // stay dirty until they are visible.
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
            // also sent when the desktop size arrives, before there are any
            // pixels.
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

        // Over the desktop, and under the status panel, which is a separate
        // view above this one.
        if (regionHints != null) {
            chrome.drawRegionHints(c, tapRegions, getWidth(), getHeight(), 1f, regionHints::get);
        }
        if (hudVisible) {
            hud.draw(c, getWidth(), getHeight(), mirror, backendState, lastRegion,
                    softInput.imeHeight(), hasPointerCapture());
            // The rates need a next frame to compare against, and a still
            // desktop produces none.
            postInvalidateOnAnimation();
        }
    }

    /**
     * The remote cursor, capped at 32 logical pixels as in the original, with
     * the hotspot already un-negated by the backend.
     */
    private void drawCursor(Canvas c) {
        if (cursor.isRelative()) {
            // The position is unknown, and the far end already draws the cursor
            // into the picture, so drawing one here would add a second, wrong
            // pointer.
            return;
        }
        chrome.drawCursor(c, cursorShape, cursorHotX, cursorHotY,
                cursor.screenX(), cursor.screenY(), bitmapPaint);
    }

    /** Release the mirror's bitmaps. The backend outlives the view. */
    public void release() {
        if (mirror != null) {
            mirror.release();
            mirror = null;
        }
    }
}
