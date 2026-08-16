// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.playground;

import android.annotation.SuppressLint;
import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.graphics.RectF;
import android.os.VibrationEffect;
import android.os.Vibrator;
import android.os.VibratorManager;
import android.text.InputType;
import android.view.HapticFeedbackConstants;
import android.view.MotionEvent;
import android.view.View;
import android.view.WindowInsets;
import android.view.inputmethod.EditorInfo;
import android.view.inputmethod.InputConnection;
import android.view.inputmethod.InputMethodManager;

import net.pgaskin.remotedesktop.control.CursorController;
import net.pgaskin.remotedesktop.control.Viewport;
import net.pgaskin.remotedesktop.control.input.AndroidScheduler;
import net.pgaskin.remotedesktop.control.input.Button;
import net.pgaskin.remotedesktop.control.input.Config;
import net.pgaskin.remotedesktop.control.input.ExtensionKeyboard;
import net.pgaskin.remotedesktop.control.input.GestureRecognizer;
import net.pgaskin.remotedesktop.control.input.KeySink;
import net.pgaskin.remotedesktop.control.input.Keysym;
import net.pgaskin.remotedesktop.control.input.MouseOverlay;
import net.pgaskin.remotedesktop.control.input.PhysicalKeyboard;
import net.pgaskin.remotedesktop.control.input.PhysicalMouse;
import net.pgaskin.remotedesktop.control.input.RegionSink;
import net.pgaskin.remotedesktop.control.input.TapRegions;
import net.pgaskin.remotedesktop.control.input.TouchRouter;
import net.pgaskin.remotedesktop.control.input.ZoomSink;
import net.pgaskin.remotedesktop.control.ui.Chrome;
import net.pgaskin.remotedesktop.control.ui.Hud;
import net.pgaskin.remotedesktop.control.ui.TextInput;

import java.util.function.Consumer;

/**
 * Wires the whole stack together and draws it:
 *
 * <pre>
 *   MotionEvent → TouchRouter → GestureRecognizer → CursorController → FakeDesktop
 *                                     │                    │
 *                                     └── ZoomSink ──→ Viewport ←┘
 * </pre>
 */
public final class PlaygroundView extends View
        implements ZoomSink, CursorController.Listener, FakeDesktop.ToggleHandler, RegionSink,
        MouseOverlay.Listener, ExtensionKeyboard.Listener, TextInput.Watcher,
        PhysicalMouse.Listener, PhysicalKeyboard.Listener {

    // A common laptop/remote size, so this can be compared
    // side by side with a real viewer on the same desktop.
    private static final int DESKTOP_W = 1920;
    private static final int DESKTOP_H = 1200;

    private final AndroidScheduler scheduler = new AndroidScheduler();
    private final Config cfg;
    private final Viewport viewport;
    private final FakeDesktop desktop;
    private final CursorController cursor;
    private final GestureRecognizer gestures;
    private final TouchRouter router;
    private final TouchRecorder recorder;
    private final MouseOverlay overlay;
    private final ExtensionKeyboard keyboard;
    private final PhysicalMouse mouse;
    private final PhysicalKeyboard keys;
    private final KeyTrace keyTrace;
    private final Chrome chrome;

    private final Art.Cursor arrow = Art.arrowCursor();
    private final Art.Cursor cross = Art.crossCursor();
    private Art.Cursor cursorShape = arrow;

    private final Paint bitmapPaint = new Paint(Paint.FILTER_BITMAP_FLAG);
    private final Paint marker = new Paint(Paint.ANTI_ALIAS_FLAG);
    /** Scratch for the inset outline. */
    private final RectF insetRect = new RectF();

    private float baseScale = 1.0f;
    private boolean laidOut;
    private boolean hudVisible = true;
    private boolean fakeInsets;
    private boolean overlayShown;
    private boolean keyboardShown;
    private boolean overlayHiddenByKeyboard; // put it back when the keyboard goes
    private int imeHeight;                   // the system IME's, from the window insets
    private boolean imeUp;                   // ... which is not the same as it being up
    private final boolean subtleHaptics;     // or only a buzz

    private final TapRegions tapRegions = TapRegions.toolbar();
    private boolean regionsOn = true;
    private String lastRegion = "-";          // for the HUD; what a region means is the host's

    private final Hud hud;
    private final Hud.Rate eventRate = new Hud.Rate();

    public PlaygroundView(Context ctx) {
        this(ctx, Config.improved(ctx.getResources().getDisplayMetrics().density));
    }

    public PlaygroundView(Context ctx, Config cfg) {
        this(ctx, cfg, true);
    }

    /**
     * @param cfg the settings to start from. The app hands its stored ones in,
     *            so what is being exercised here is what a session would run —
     *            the PRESET square still swaps presets on top of them, which is
     *            what makes this a playground rather than a preview.
     * @param recorders whether the fixture recorder and the key trace are
     *            offered as squares. They write raw streams to files meant to be
     *            pulled off the phone, which is a thing a library's own demo and
     *            a host that has asked for it both want and an app's test
     *            surface does not — hence the parameter, and hence the
     *            constructors above it leaving them on. {@link #setRecording}
     *            and {@link #setKeyTrace} still work either way: a host that
     *            hides the squares can still arm one from a script.
     */
    public PlaygroundView(Context ctx, Config cfg, boolean recorders) {
        super(ctx);
        this.cfg = cfg;
        viewport = new Viewport(cfg.density);
        viewport.setDesktopSize(DESKTOP_W, DESKTOP_H);
        desktop = new FakeDesktop(DESKTOP_W, DESKTOP_H, this, recorders);
        cursor = new CursorController(cfg, viewport, desktop, scheduler);
        cursor.setListener(this);
        gestures = new GestureRecognizer(cfg, cursor, this, scheduler);
        gestures.setRegions(tapRegions, this);
        router = new TouchRouter(gestures);
        recorder = new TouchRecorder(ctx, cfg);
        router.setTap(recorder);
        // Its own button source, so a tap on the touchpad during an overlay-held
        // drag cannot release what the overlay is holding (CursorController).
        overlay = new MouseOverlay(cfg, cursor.newButtonSource(), scheduler);
        overlay.setListener(this);
        router.addClaim(overlay);
        // The keyboard's keysyms go to the same fake remote the mouse events do,
        // so "what did the far end actually receive" is one readout.
        keyboard = new ExtensionKeyboard(cfg, desktop, scheduler,
                ExtensionKeyboard.standardKeys());
        keyboard.setListener(this);
        router.addClaim(keyboard);
        // The physical pair, against the one desktop whose reaction to them can
        // be checked without a server.
        mouse = new PhysicalMouse(cfg, cursor.newButtonSource());
        mouse.setListener(this);
        // The trace stands between the keyboard and the desktop, so what it
        // writes down is what was sent rather than what would have been.
        keyTrace = new KeyTrace(ctx, desktop);
        keys = new PhysicalKeyboard(keyTrace);
        keys.setListener(this);

        hud = new Hud(cfg);
        marker.setStyle(Paint.Style.STROKE);
        marker.setStrokeWidth(1f);
        marker.setColor(0x66ffffff);
        chrome = new Chrome(cfg);
        chrome.attach(keyboard);
        subtleHaptics = canTick(ctx);
        setFocusable(true);
        // The IME will not open against a view that cannot take focus by touch.
        setFocusableInTouchMode(true);
    }

    /** Arm the fixture recorder without having to click the toggle square. */
    public void setRecording(boolean on) {
        if (recorder.armed() != on) {
            recorder.toggle();
        }
    }

    /** The same for the key trace, which is driven from a script rather than by hand. */
    public void setKeyTrace(boolean on) {
        keyTrace.setArmed(on);
    }

    /** And for the mode the RELATIVE square toggles: the far end owns the cursor. */
    public void setRelativePointer(boolean on) {
        if (cursor.isRelative() != on) {
            toggleRelativePointer();
        }
    }

    @Override
    protected void onSizeChanged(int w, int h, int oldw, int oldh) {
        super.onSizeChanged(w, h, oldw, oldh);
        viewport.setViewSize(w, h);
        gestures.setViewSize(w, h);
        recorder.setViewSize(w, h);
        overlay.setViewSize(w, h);
        keyboard.setViewSize(w, h);
        if (!laidOut) {
            laidOut = true;
            viewport.setFocus(DESKTOP_W / 2f, DESKTOP_H / 2f);
            viewport.centreOn(DESKTOP_W / 2f, DESKTOP_H / 2f, viewport.snapScale(1.0f));
            cursor.setPosition(DESKTOP_W / 2f, DESKTOP_H / 2f);
            // A mode asked for before there was a window to place it in.
            desktop.setRelative(cursor.isRelative(), cursor.x(), cursor.y());
        } else {
            // Rotation, or any other resize. The remote desktop does not rotate
            // with the phone, so the cursor keeps its desktop position and only
            // the window onto the desktop changes. Re-snap the scale, because
            // the fit-the-desktop minimum moves with the aspect ratio: a scale
            // that was legal in landscape can be below the minimum in portrait.
            viewport.centreOn(cursor.x(), cursor.y(), viewport.snapScale(viewport.getScale()));
        }
        baseScale = viewport.getScale();
    }

    @SuppressLint("ClickableViewAccessibility")
    @Override
    public boolean onTouchEvent(MotionEvent ev) {
        if (PhysicalMouse.isMouse(ev) && mouse.onTouchEvent(ev)) {
            return true;
        }
        return router.onTouchEvent(ev);
    }

    // ---- the physical mouse and keyboard, wired as a host would ------------

    @Override
    public boolean onGenericMotionEvent(MotionEvent ev) {
        return mouse.onGenericMotionEvent(ev) || super.onGenericMotionEvent(ev);
    }

    @Override
    public boolean onCapturedPointerEvent(MotionEvent ev) {
        return mouse.onCapturedPointerEvent(ev) || super.onCapturedPointerEvent(ev);
    }

    @Override
    public void onPointerCaptureChange(boolean hasCapture) {
        super.onPointerCaptureChange(hasCapture);
        if (!hasCapture) {
            mouse.cancel();
        }
        invalidate();
    }

    @Override
    public void onWindowFocusChanged(boolean hasWindowFocus) {
        super.onWindowFocusChanged(hasWindowFocus);
        if (hasWindowFocus) {
            requestFocus();
            if (cfg.mouseCapture && isFocused()) {
                requestPointerCapture();
            }
            // Both directions: the row and the soft keyboard are one keyboard,
            // and only this window can say so to the IMM.
            syncKeyboardChrome();
        } else {
            mouse.cancel();
            keys.releaseAll();
        }
    }

    @Override
    public boolean dispatchKeyEvent(android.view.KeyEvent ev) {
        if (keyTrace.record(ev, () -> keys.onKeyEvent(ev, this::sent))) {
            return true;
        }
        return super.dispatchKeyEvent(ev);
    }

    @Override
    public void mouseActivity() {
        invalidate();
    }

    @Override
    public void keyboardActivity() {
        invalidate();
    }

    // ---- the system IME ---------------------------------------------------

    @Override
    public boolean onCheckIsTextEditor() {
        return true;
    }

    /**
     * {@code TYPE_NULL} with no extract UI: there is no text field here to edit,
     * only a remote machine to send keys to, and declaring that makes an IME
     * send key events rather than trying to manage a document. {@link TextInput}
     * handles committed text as well, since prediction and emoji arrive that way
     * whatever the input type says.
     */
    @Override
    public InputConnection onCreateInputConnection(EditorInfo out) {
        out.inputType = InputType.TYPE_NULL;
        out.imeOptions = EditorInfo.IME_FLAG_NO_EXTRACT_UI
                | EditorInfo.IME_FLAG_NO_FULLSCREEN
                | EditorInfo.IME_ACTION_NONE;
        return new TextInput(this, desktop, this);
    }

    /**
     * {@link TextInput.Watcher}: the IME asked the editor to paste, which is
     * what an IME with a clipboard key of its own does instead of committing
     * text. Same thing the row's Paste key does.
     */
    @Override
    public void pasteRequested() {
        keyAction(ExtensionKeyboard.ACTION_PASTE);
    }

    /** {@link TextInput.Watcher}: an IME key counts as a key for the modifiers. */
    @Override
    public void sent(int keysym) {
        keyboard.externalKey(keysym);
        invalidate();
    }

    /** {@link TextInput.Watcher}: what the row is holding decides a typed character's case. */
    @Override
    public java.util.Set<Integer> heldModifiers() {
        return keyboard.heldModifiers();
    }

    /**
     * The IME's height, so the extension keyboard sits on top of it rather than
     * behind it and the desktop insets by the pair. A soft keyboard dismissed
     * with the back gesture takes the row with it; guarded on one having been
     * up, so a window that is never told of an IME does not hide the row the
     * moment it is shown.
     */
    @Override
    public WindowInsets onApplyWindowInsets(WindowInsets insets) {
        final int ime = insets.getInsets(WindowInsets.Type.ime()).bottom;
        // Whether one is up, asked separately from how much of this window it
        // covers. In multi-window the two part company: the window is resized
        // around the keyboard rather than overlapped by it, so the inset stays
        // 0 the whole time one is showing — and a height that never rises is a
        // height that never falls, which left the row up after a back gesture
        // had taken the keyboard under it.
        final boolean up = insets.isVisible(WindowInsets.Type.ime());
        if (ime != imeHeight || up != imeUp) {
            // While this window has focus only: an IME also closes when the
            // app is switched away from, and reading that as a dismissal hides
            // the row on the way out.
            final boolean closed = imeUp && !up && hasWindowFocus();
            imeHeight = ime;
            imeUp = up;
            keyboard.setBottomOffset(ime);
            if (closed && keyboard.visible()) {
                setKeyboardVisible(false);
            }
            applyInsets();
            invalidate();
        }
        return super.onApplyWindowInsets(insets);
    }

    /**
     * Showing one overlay hides the other, and a mouse overlay displaced by the
     * keyboard comes back when the keyboard goes away — the one nicety worth
     * keeping from the original's four-state machine.
     */
    private void setKeyboardVisible(boolean show) {
        if (show == keyboard.visible()) {
            return;
        }
        if (show) {
            overlayHiddenByKeyboard = overlay.visible();
            overlay.setVisible(false);
        }
        // Everything else follows from the model changing, in keyboardChanged()
        // — because the model can also hide itself, from its own ✕, and the IME
        // has to come down with it either way.
        keyboard.setVisible(show);
    }

    /**
     * The IME follows the extension row, whichever was asked first: a soft
     * keyboard left behind by a dismissed row covers the very tap region that
     * would bring the row back.
     */
    private void syncKeyboardChrome() {
        final InputMethodManager imm = getContext().getSystemService(InputMethodManager.class);
        if (keyboard.visible()) {
            requestFocus();
            imm.showSoftInput(this, 0);
        } else {
            imm.hideSoftInputFromWindow(getWindowToken(), 0);
            if (overlayHiddenByKeyboard) {
                overlayHiddenByKeyboard = false;
                overlay.setVisible(true);
            }
        }
    }

    // ---- ZoomSink ---------------------------------------------------------

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
     * somebody's desktop for a gesture that is about looking ({@code ARCHITECTURE.md} §3.18).
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

    // ---- CursorController.Listener ---------------------------------------

    @Override
    public void onCursorChanged() {
        invalidate();
    }

    /** A click consumes the armed modifiers, exactly as a key does. */
    @Override
    public void onButtonsReleased() {
        keyboard.externalClick();
    }

    // ---- FakeDesktop.ToggleHandler ---------------------------------------

    @Override
    public void onToggle(FakeDesktop.Toggle toggle) {
        switch (toggle) {
            case PRESET -> cfg.copyFrom(cfg.faithfulPreset
                    ? Config.improved(cfg.density)
                    : Config.faithful(cfg.density));
            // SAWTOOTH (the original) → SMOOTH (sliding window) → ADAPTIVE
            // (original curve, speed-gated) → OFF.
            case ACCEL -> {
                if (!cfg.accelEnabled) {
                    cfg.accelEnabled = true;
                    cfg.accelDrainHistory = true;
                    cfg.accelAdaptive = false;
                } else if (cfg.accelDrainHistory && !cfg.accelAdaptive) {
                    cfg.accelDrainHistory = false;
                } else if (!cfg.accelDrainHistory) {
                    cfg.accelDrainHistory = true;
                    cfg.accelAdaptive = true;
                } else {
                    cfg.accelEnabled = false;
                    cfg.accelAdaptive = false;
                }
            }
            case AXISLOCK -> cfg.axisLockEnabled = !cfg.axisLockEnabled;
            case MOMENTUM -> cfg.inertiaEnabled = !cfg.inertiaEnabled;
            case CURSOR -> cursorShape = (cursorShape == arrow) ? cross : arrow;
            case NATSCROLL -> cfg.naturalScrolling = !cfg.naturalScrolling;
            case HUD -> hudVisible = !hudVisible;
            case RECORD -> recorder.toggle();
            case KEYTRACE -> keyTrace.toggle();
            case ZOOMIN -> zoom(Viewport::zoomIn);
            case ZOOMOUT -> zoom(Viewport::zoomOut);
            case ZOOMFIT -> zoom(viewport.getScale() > viewport.minScale() + 1e-4f
                    ? Viewport::zoomToFit
                    : Viewport::zoomToFill);
            case INSETS -> {
                fakeInsets = !fakeInsets;
                applyInsets();
            }
            case RELATIVE -> toggleRelativePointer();
            case REGIONS -> {
                regionsOn = !regionsOn;
                gestures.setRegions(regionsOn ? tapRegions : null, this);
            }
            // Same thing the `mouse` and `keyboard` tap regions do; here too so
            // both are reachable with the regions off, and from a script.
            case MOUSE -> toggleOverlay();
            case KEYBOARD -> setKeyboardVisible(!keyboard.visible());
        }
        invalidate();
    }

    // ---- RegionSink -------------------------------------------------------

    /**
     * The {@code mouse} region toggles the overlay — which is why that region is
     * the bottom-right corner, where the overlay's own dismiss button sits, so
     * the two read as one control — and the {@code keyboard} region, the rest of
     * the bottom band, toggles the extension keyboard and the IME with it. The
     * remaining two are stubbed to a HUD readout here: disconnect and
     * connection info are questions only a host with a session can answer.
     */
    @Override
    public boolean regionTapped(TapRegions.Region region, float x, float y) {
        lastRegion = region.name() + " @" + (int) x + "," + (int) y;
        if (TapRegions.MOUSE.equals(region.name())) {
            toggleOverlay();
        } else if (TapRegions.KEYBOARD.equals(region.name())) {
            setKeyboardVisible(!keyboard.visible());
        }
        invalidate();
        return true;
    }

    /**
     * Hand the cursor to the far end, or take it back. It is handed over where
     * our cursor already is, so nothing jumps; a real one is announced by the
     * server mid-session and lands wherever that machine had left it.
     */
    private void toggleRelativePointer() {
        final boolean on = !cursor.isRelative();
        desktop.setRelative(on, cursor.x(), cursor.y());
        cursor.setRelative(on);
        gestures.setRelative(on);
        recorder.setRelative(on);
        invalidate();
    }

    /** Showing the overlay puts the keyboard away, and vice versa. */
    private void toggleOverlay() {
        if (overlay.visible()) {
            overlay.setVisible(false);
        } else {
            setKeyboardVisible(false);
            overlay.setVisible(true);
        }
    }

    // ---- MouseOverlay.Listener -------------------------------------------

    @Override
    public void overlayChanged() {
        // Bump scroll arms for a drag started while the overlay holds a button,
        // the same as it does inside the 250 ms click window.
        gestures.setExternalButtonHeld((overlay.heldMask() & Button.DRAG_MASK) != 0);
        // Appearing and disappearing changes what the window shows, so it moves
        // the viewport; being pressed does not. This fires on every press.
        if (overlayShown != overlay.visible()) {
            overlayShown = overlay.visible();
            applyInsets();
        }
        invalidate();
    }

    // ---- ExtensionKeyboard.Listener --------------------------------------

    @Override
    public void keyboardChanged() {
        if (keyboardShown != keyboard.visible()) {
            keyboardShown = keyboard.visible();
            syncKeyboardChrome();
            applyInsets();
        }
        chrome.keyboardChanged(keyboard);
        invalidate();
    }

    /**
     * The row's Paste key, against the fake desktop's text field: the same thing
     * the session screen does — the clipboard's characters typed one at a time —
     * minus the confirmation, since here the far end is a rectangle we drew and
     * a long paste costs nothing but time.
     */
    @Override
    public void keyAction(String name) {
        if (!ExtensionKeyboard.ACTION_PASTE.equals(name)) {
            return;
        }
        final ClipboardManager cm = getContext().getSystemService(ClipboardManager.class);
        final ClipData clip = cm == null ? null : cm.getPrimaryClip();
        if (clip == null || clip.getItemCount() == 0) {
            return;
        }
        final CharSequence cs = clip.getItemAt(0).coerceToText(getContext());
        final String text = cs == null ? "" : cs.toString();
        for (int i = 0; i < text.length(); ) {
            final int cp = text.codePointAt(i);
            i += Character.charCount(cp);
            final int keysym = Keysym.forCharacter(cp);
            if (keysym != 0) {
                desktop.keyDown(keysym, KeySink.ID_TEXT);
                desktop.keyUp(KeySink.ID_TEXT);
            }
        }
        invalidate();
    }

    /**
     * A repeat wants the lightest thing the device can do — the tick a text
     * cursor makes stepping through a word — and only if it really can do light
     * things. A motor that can only manage a blunt buzz would turn a held arrow
     * key into a drill, so gate on the actuator actually supporting a tick
     * primitive rather than on the API level.
     */
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
     * Run a zoom off the current dispatch. Toggle squares are clicked by the
     * emulated cursor, so this arrives from inside {@code pointerEvent}; zooming
     * re-centres the cursor and sends another, and doing that re-entrantly makes
     * the event ordering hard to reason about for no gain.
     */
    private void zoom(Consumer<Viewport> step) {
        post(() -> {
            // The cursor is the scale focus, and it is put back at the
            // centre of the window afterwards.
            viewport.setFocus(cursor.x(), cursor.y());
            step.accept(viewport);
            baseScale = viewport.getScale();
            if (cfg.recentreCursorOnZoom) {
                cursor.centreCursor(true);
            }
            invalidate();
        });
    }

    /**
     * The insets the viewport clamps inside: the {@code INSETS} square's fake
     * ones, standing in for system bars, plus whatever the mouse overlay is
     * covering. Applied through the cursor, which is what keeps the desktop
     * still while the window around it changes shape
     * ({@link net.pgaskin.remotedesktop.control.CursorController#setInsets}).
     */
    private void applyInsets() {
        int left = 0, top = 0, right = 0, bottom = 0;
        if (fakeInsets) {
            left = (int) cfg.dp(16);
            top = (int) cfg.dp(48);
            right = (int) cfg.dp(16);
            bottom = (int) cfg.dp(120);
        }
        right = Math.max(right, (int) overlay.insetRightPx());
        bottom = Math.max(bottom, (int) overlay.insetBottomPx());
        bottom = Math.max(bottom, (int) keyboard.insetBottomPx());
        // The IME on its own, in case it outlives the row that asked for it.
        bottom = Math.max(bottom, imeHeight);
        cursor.setInsets(left, top, right, bottom);
        baseScale = viewport.getScale();
    }

    @Override
    public String label(FakeDesktop.Toggle toggle) {
        return switch (toggle) {
            case PRESET -> cfg.faithfulPreset ? "FAITHFUL" : "IMPROVED";
            case ACCEL -> !cfg.accelEnabled ? "OFF"
                    : cfg.accelAdaptive ? "ADAPTIVE"
                    : cfg.accelDrainHistory ? "SAWTOOTH" : "SMOOTH";
            case AXISLOCK -> cfg.axisLockEnabled ? "ON" : "OFF";
            case MOMENTUM -> cfg.inertiaEnabled ? "ON" : "OFF";
            case CURSOR -> cursorShape.name();
            case NATSCROLL -> cfg.naturalScrolling ? "ON" : "OFF";
            case HUD -> hudVisible ? "ON" : "OFF";
            case RECORD -> recorder.label();
            case KEYTRACE -> keyTrace.label();
            case ZOOMIN -> viewport.canZoomIn()
                    ? String.format("%.2f", viewport.nextZoomIn()) : "MAX";
            case ZOOMOUT -> viewport.canZoomOut()
                    ? String.format("%.2f", viewport.nextZoomOut()) : "MIN";
            case ZOOMFIT -> viewport.getScale() > viewport.minScale() + 1e-4f ? "FIT" : "FILL";
            case INSETS -> fakeInsets ? "ON" : "OFF";
            case RELATIVE -> cursor.isRelative() ? "THEIRS" : "OURS";
            case REGIONS -> regionsOn ? "ON" : "OFF";
            case MOUSE -> overlay.visible() ? "ON" : "OFF";
            case KEYBOARD -> keyboard.visible() ? "ON" : "OFF";
        };
    }

    // ---- drawing ----------------------------------------------------------

    @Override
    protected void onDraw(Canvas c) {
        c.drawColor(0xff000000);

        final int save = c.save();
        c.translate(viewport.originX(), viewport.originY());
        c.scale(viewport.getScale(), viewport.getScale());
        desktop.draw(c);
        c.restoreToCount(save);

        if (hudVisible) {
            // Viewport centre marks: the cursor sits exactly here except at the
            // desktop edges. With insets on this is the centre of the *content*
            // rect, not of the view, which is the whole visible difference.
            final float cx = viewport.centreScreenX(), cy = viewport.centreScreenY();
            c.drawLine(cx - cfg.dp(8), cy, cx + cfg.dp(8), cy, marker);
            c.drawLine(cx, cy - cfg.dp(8), cx, cy + cfg.dp(8), marker);
            if (viewport.insetLeft() + viewport.insetTop()
                    + viewport.insetRight() + viewport.insetBottom() > 0) {
                insetRect.set(viewport.insetLeft(), viewport.insetTop(),
                        getWidth() - viewport.insetRight(),
                        getHeight() - viewport.insetBottom());
                c.drawRect(insetRect, marker);
            }
            if (regionsOn) {
                chrome.drawRegions(c, tapRegions, getWidth(), getHeight());
            }
        }

        drawCursor(c);

        if (overlay.visible()) {
            chrome.drawOverlay(c, overlay);
        }

        if (keyboard.visible() && chrome.drawKeyboard(c, keyboard, getWidth(), cursor.screenY())) {
            postInvalidateOnAnimation();
        }

        if (hudVisible) {
            drawHud(c);
        }
    }

    /**
     * CursorView.b — cap the bitmap at 32 logical px and apply the hotspot.
     *
     * <p>The original translates by {@code pos + hot*scale}, where its {@code hot}
     * is the <em>negated</em> hotspot: the native parser rewrites the cursor's
     * rect to {@code Rect(-hot, size - hot)} and passes its origin up.
     * {@link Art.Cursor} keeps the positive hotspot, so subtract instead — same
     * result.
     */
    private void drawCursor(Canvas c) {
        if (cursor.isRelative()) {
            return; // the far end drew its own into the picture
        }
        final Art.Cursor s = cursorShape;
        chrome.drawCursor(c, s.bitmap(), s.hotX(), s.hotY(),
                cursor.screenX(), cursor.screenY(), bitmapPaint);
    }

    private void drawHud(Canvas c) {
        final int eventsPerSecond = eventRate.sample(cursor.eventCount(), System.nanoTime());

        final String[] lines = {
                "down " + gestures.downCount() + "  max " + gestures.maxDownCount()
                        + "  mode " + gestures.mode()
                        + "  moving " + (gestures.moving() ? "Y" : "N")
                        + "  held " + (gestures.heldButton() == null ? "-" : gestures.heldButton()),
                "cursor " + (cursor.isRelative() ? "theirs"
                        : (int) cursor.x() + "," + (int) cursor.y())
                        + "  btn " + cursor.buttonsName()
                        + "  scale " + String.format("%.3f", viewport.getScale())
                        + " [" + (viewport.zoomIndex() + 1) + "/" + viewport.zoomLadder().length + "]"
                        + "  origin " + (int) viewport.originX() + "," + (int) viewport.originY()
                        + (fakeInsets || overlayShown ? "  inset " + viewport.contentWidth()
                        + "x" + viewport.contentHeight() : ""),
                "accel x" + String.format("%.2f", gestures.accelFactor())
                        // dp/ms, so it can be read against the Config thresholds
                        + "  spd " + String.format("%.2f", gestures.accelSpeed() / cfg.density)
                        + "  lock " + gestures.axisLock()
                        + " " + String.format("%.0f", gestures.turnDegrees()) + "\u00b0"
                        + "  glide " + String.format("%.1f", gestures.glideSpeed())
                        + "  events " + cursor.eventCount() + " (" + eventsPerSecond + "/s)"
                        + "  dup " + cursor.suppressedCount(),
                "cfg " + (cfg.faithfulPreset ? "FAITHFUL" : "IMPROVED")
                        + "  accel " + label(FakeDesktop.Toggle.ACCEL)
                        + "  axlock " + label(FakeDesktop.Toggle.AXISLOCK)
                        + "  coalesce " + (cfg.coalescePointerEvents ? "Y" : "N")
                        + "  dedupe " + (cfg.dedupePointerEvents ? "Y" : "N")
                        + "  countTest " + (cfg.moveCountTest ? "Y" : "N"),
                "ovl " + (overlay.visible()
                        ? Button.maskName(overlay.heldMask())
                        + " rate " + String.format("%.1f", overlay.scrollRate())
                        : "off")
                        + "   kbd " + (keyboard.visible()
                        ? "on ime " + imeHeight + " mod " + keyboard.heldModifierCount()
                        + (subtleHaptics ? " tick" : " buzz")
                        : "off")
                        + "   key " + desktop.lastKey
                        + "   region " + (regionsOn ? lastRegion : "off")
                        + "   last " + desktop.lastEvent
                        + (recorder.armed() || recorder.count() > 0
                        ? "   rec " + (recorder.armed() ? "ON" : "off")
                        + " " + recorder.lastPath() : ""),
                "mouse " + (hasPointerCapture() ? "captured" : mouse.seen() ? "hover" : "-")
                        + " btn " + Button.maskName(mouse.heldMask())
                        + "   keys " + keys.heldCount() + " held"
                        + (keyTrace.armed() ? "   trace " + keyTrace.label()
                        + " " + keyTrace.lastPath() : ""),
        };

        // Clear of whichever overlay is up: both live along the bottom edge.
        // The keyboard's heightPx() rather than its inset, because the info bar
        // is a readout the HUD must not sit on even though the desktop may.
        hud.draw(c, lines, getWidth(), getHeight(),
                Math.max(Math.max(overlay.visible() ? cfg.overlayRowHeightPx : 0, imeHeight),
                        keyboard.heightPx()));
    }
}
