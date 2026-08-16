// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.input;

/**
 * Every tunable in the input stack, in one place.
 *
 * <p>The defaults are the values reverse-engineered from RealVNC Viewer
 * 4.9.1.60165; {@code ARCHITECTURE.md} §1.15 lists them in one table. Two
 * presets are provided: {@link #faithful} reproduces the original bug-for-bug,
 * {@link #improved} applies the deliberate changes (§2 and §3 there).
 * Flipping between them at runtime is the point — it is how the "feel" gets
 * A/B'd.
 */
public final class Config {

    public final float density;

    public boolean faithfulPreset; // display only

    // ---- tap vs. drag -----------------------------------------------------

    public float moveThresholdPx;    // mouse_cursor_movement_threshold, 12 dp, per-axis
    /**
     * Accumulated path length that promotes a touch to a drag even if it never
     * gets 12 dp away from where it started. Replaces the original's
     * "more than 10 ACTION_MOVE events" test, which a high-report-rate screen
     * trips while the finger is stationary. 0 disables.
     */
    public float tapPathThresholdPx;
    public boolean moveCountTest;    // the test above, as the original has it: a real bug
    public int moveCountLimit = 10;
    public long tapMaxDurationMs;    // longer is a drag, not a tap; 0 disables

    // ---- clicks -----------------------------------------------------------

    /**
     * Auto-release delay for a tap-generated button press. Doubles as the
     * double-click / tap-then-drag window: a touch-down inside it cancels the
     * release, leaving the button held.
     */
    public long clickHoldMs = 250;
    public float flickMinDistPx = 2.0f; // between the last two positions, to glide at all

    // ---- bump scroll (edge auto-repeat during a tap-then-drag) -------------

    public boolean bumpScrollEnabled = true;
    public float bumpBorderPx;              // bump_scroll_border_size, 24 dp
    public float bumpDeltaPx;               // bump_scroll_event_delta, 12 dp
    public long bumpPeriodMs = 100;
    public boolean bumpStopOutsideBorder = true; // original keeps firing (0,0) once armed

    // ---- two-finger -------------------------------------------------------

    public float zoomRatio = 0.7f;      // |Δseparation| / totalTravel above this ⇒ pinch
    public float scrollRatio = 0.2f;    // ... below this ⇒ wheel; between them, undecided
    public float wheelStepPx = 8.0f;    // midpoint travel per wheel click, raw screen px
    public boolean naturalScrolling = false;

    // ---- acceleration ------------------------------------------------------

    public boolean accelEnabled = true;
    public boolean accelDrainHistory = true; // original drains its 3 samples: every 3rd event scales
    public boolean accelResetOnDown = true;  // original never clears them between gestures
    public float accelBase = (float) Math.sqrt(1.1);
    public float accelExponent = 2.5999999f;
    public float accelMin = 1.1f;
    public float accelMax = 5.0f;

    // ---- adaptive acceleration (ours; the original has none of this) ------

    /**
     * Fade the amplification out at low speed. The curve is jerk-based, so a
     * <em>steady</em> slow drag already sits near the {@link #accelMin} floor,
     * but a slow drag with jitter in it is all jerk and gets amplified exactly
     * where precision was wanted. Gating only the part of the factor above the
     * floor leaves the flick response intact.
     */
    public boolean accelAdaptive;
    public float accelSlowSpeedPx;  // below this the factor is pinned to accelMin, px/ms
    public float accelFullSpeedPx;  // at and above it the jerk curve applies unmodified

    /**
     * Zero the minor axis of a slow, near-axial movement, so a horizontal or
     * vertical drag does not wander — text selection is what it is for.
     */
    public boolean axisLockEnabled;
    public float axisLockMaxSpeedPx;         // only engages below this, px/ms
    public float axisLockEnterRatio = 0.30f; // smoothed minor/major below this locks
    public float axisLockExitRatio = 0.55f;  // ... and above this releases: exit > enter

    /**
     * Send a finger's motion unshaped where the far end owns the cursor: no
     * acceleration, no adaptive gate, no axis lock, and no glide after a flick.
     *
     * <p>All four are ways of deciding how far a cursor goes for a given
     * finger, and in that mode the far end decides that — it applies its own
     * acceleration to whatever deltas arrive, which is the acceleration that
     * machine's own user lives with. Ours then compounds it, and a curve on top
     * of a curve is nobody's.
     *
     * <p>What it does <em>not</em> turn off is dividing by the viewport scale,
     * which is not shaping: a finger crossing a zoomed-in screen still covers
     * fewer desktop pixels, and the picture is this end's fact whoever owns the
     * pointer. Nor does it touch a physical mouse, whose deltas never went
     * through any of this.
     *
     * <p>Neither preset sets it, because the original has no such mode to be
     * faithful or unfaithful to.
     */
    public boolean rawMotionWhenRelative = true;
    /**
     * The turn test, without which locking on the ratio alone flattens a third
     * of every circle drawn: the direction is smoothed over
     * {@code axisLockTurnSpanPx} of path and the angle between two such spans is
     * the turn. Longer sees gentler curves, shorter re-locks sooner afterwards.
     * A turn past {@code axisLockCornerDeg} is a corner rather than a curve, so
     * the history is dropped outright and the next straight leg — the across of
     * an L-shaped text selection — locks a few events later instead of a whole
     * span later.
     */
    public float axisLockTurnSpanPx;
    public float axisLockMaxTurnDeg = 10.0f;
    public float axisLockCornerDeg = 45.0f;

    // ---- mouse button overlay (ui.MouseButtons / ui.ScrollButton) ---------

    public float overlayStripWidthPx;   // wheel strip up the right edge, 60 dp
    public float overlayRowHeightPx;    // button row across the bottom, 72 dp
    public float overlayDismissPx;      // the square dismiss button, and its corner margin
    public float overlayDismissMarginPx;
    public float overlayMiddleMinPx;    // the middle button never narrows past this, 100 dp
    public long overlayWheelTickMs = 40;
    public float overlayWheelMaxRate = 4.0f;       // clicks per tick at the ends of the strip
    public float overlayWheelTicksPerClick = 3.0f; // the strip's gearing at rate 1
    public int overlayWheelStartDelayTicks = 8;    // before the first repeat, so a tap is one click

    // ---- extension keyboard (ui.ExtensionKeyboard / ui.InfoBar) -----------

    public float keyboardKeyHeightPx;   // extension_keyboard_height, 46 dp
    public float keyboardInfoHeightPx;  // the info bar above the keys, 30 dp
    public float keyboardKeyPadPx;      // key_horizontal_margin
    public float keyboardKeyPadWidePx;  // key_horizontal_margin_wide
    /**
     * The gap between key groups. The original's is 16 dp either side of a
     * divider it draws and we do not; without the divider that much space reads
     * as the row having ended.
     */
    public float keyboardGroupGapPx;
    public float keyboardIconWidthPx;    // width allowed for a key drawn as an icon
    public float keyboardMinKeyWidthPx;  // however short the label
    public float keyboardScrollSlopPx;   // movement that turns a key press into a scroll
    public long keyboardFlingTickMs = 16;
    public float keyboardFlingDecay = 0.94f;
    public float keyboardFlingMinPx;     // release speed below this does not fling, px/ms
    public float keyboardFlingStopPx;    // ... and a glide slower than this has stopped
    public long keyDoubleTapMs = 300;    // second tap on a modifier locks it; the platform's timeout
    public long keyLongPressMs = 500;    // hold to lock a modifier, or to start repeating
    public long keyRepeatMs = 75;
    /**
     * Whether the key row buzzes at all: the system's own keyboard setting does
     * not reach a view that draws its own keys. Checked here rather than in the
     * view so that the model simply does not ask.
     */
    public boolean keyboardHaptics = true;
    public int keyboardInfoMaxChars = 256; // of the typed line kept; the bar shows the tail
    /**
     * Whether the info bar is part of the keyboard or a pane floating over the
     * desktop. Floating is the original's: the desktop runs on underneath, and
     * the bar thins as the cursor comes near so that a pointer pushed to the
     * bottom edge is not swallowed by it. Solid insets the desktop by the bar as
     * well, which costs 30 dp of picture and buys a readout that never dims and
     * never has desktop showing through it.
     */
    public boolean keyboardInfoSolid = false;

    // ---- momentum ----------------------------------------------------------

    public boolean inertiaEnabled = true;
    public long inertiaStartDelayMs = 50;
    public long inertiaTickMs = 10;
    public double inertiaSpeedScale = 25.0;
    public double inertiaSpeedMax = 200.0;
    public double inertiaDecay = 0.85;
    public double inertiaStopSpeed = 3.0;
    public boolean inertiaCancelOnDown = true; // original lets a glide run through a new touch
    public boolean inertiaResetOnDown = true;  // original reuses the previous gesture's samples

    // ---- cursor / viewport ------------------------------------------------

    public boolean recentreCursorOnZoom = true;  // on every pinch step, as the original does
    public boolean coalescePointerEvents = true; // one event per frame; the original does not throttle
    /**
     * Drop a pointer event whose position <em>and</em> button mask are identical
     * to the last one sent. A finger held still keeps producing ACTION_MOVEs
     * (the digitizer jitters below a pixel), and pushing the cursor against a
     * desktop edge clamps every delta away, so without this the stack streams
     * duplicate positions for as long as a finger is down.
     *
     * <p>Safe only because the protocol is absolute: a dropped duplicate cannot
     * lose distance the way a dropped relative delta would.
     */
    public boolean dedupePointerEvents = true;
    /**
     * Keep the desktop's edges clear of the shape of the window
     * ({@link net.pgaskin.remotedesktop.control.Viewport#setPanMargins}): a pan
     * may carry each edge in by whatever the caller measures that edge to cost,
     * leaving blank beside it. Off is the original: an edge of the desktop stops
     * at the edge of the window, which on a phone is not a rectangle — a rounded
     * corner, a camera cutout and a system bar all sit over the picture, and the
     * last row of pixels against one of them cannot be looked at squarely or
     * clicked comfortably.
     *
     * <p>Off by default, because it is blank space in exchange for reach, and
     * whether that is worth it depends on the phone.
     */
    public boolean panMarginInsets = false;
    /**
     * A margin on every edge, the same on each and answering to nothing about
     * the window: room to bring an edge of the desktop in from the edge of the
     * screen because that is where it is easier to look at, whatever is or is
     * not over it there.
     *
     * <p>Independent of {@link #panMarginInsets}, which is a different reason
     * to want the same thing — either alone is a margin, and both is the sum.
     * Zero by default, like the other off.
     */
    public float panMarginPx;

    // ---- physical mouse and keyboard --------------------------------------

    /**
     * Ask for the pointer when the session has focus, so a real mouse reports
     * relative motion and the local cursor cannot leave the window. Off falls
     * back to deriving deltas from the local pointer's position, which stops
     * dead at the screen edge — which is why the original, which never captures,
     * needs an edge auto-scroll at all.
     */
    public boolean mouseCapture = true;
    public float mouseSpeed = 1.0f;     // 1.0 is the pointer profile the phone already applied
    public float mouseWheelStep = 1.0f; // clicks per detent; not wheelStepPx, which is finger travel

    private Config(float density) {
        this.density = density;
        this.moveThresholdPx = dp(12);
        this.bumpBorderPx = dp(24);
        this.bumpDeltaPx = dp(12);
        // Speeds are dp/ms. The recorded precision gestures and the purposeful
        // ones sit two decades apart, so every threshold below falls in the gap
        // between them: 0.15 dp/ms is placing the cursor, 0.6 is a real drag.
        this.accelSlowSpeedPx = dp(0.15f);
        this.accelFullSpeedPx = dp(0.60f);
        this.axisLockMaxSpeedPx = dp(0.25f);
        this.axisLockTurnSpanPx = dp(36);
        this.overlayStripWidthPx = dp(60);
        this.overlayRowHeightPx = dp(72);
        this.overlayDismissPx = dp(40);
        this.overlayDismissMarginPx = dp(17.5f);
        this.overlayMiddleMinPx = dp(100);
        this.keyboardKeyHeightPx = dp(46);
        this.keyboardInfoHeightPx = dp(30);
        this.keyboardKeyPadPx = dp(8);
        this.keyboardKeyPadWidePx = dp(12);
        this.keyboardGroupGapPx = dp(22); // enough to group, not enough to look like a break
        this.keyboardIconWidthPx = dp(18);
        this.keyboardMinKeyWidthPx = dp(32);
        this.keyboardScrollSlopPx = dp(8);
        this.keyboardFlingMinPx = dp(0.3f);
        this.keyboardFlingStopPx = dp(0.02f);
    }

    public float dp(float v) {
        return v * density;
    }

    /** Adopt every setting of {@code o}, so a preset can be swapped under a running stack. */
    public void copyFrom(Config o) {
        faithfulPreset = o.faithfulPreset;
        moveThresholdPx = o.moveThresholdPx;
        tapPathThresholdPx = o.tapPathThresholdPx;
        moveCountTest = o.moveCountTest;
        moveCountLimit = o.moveCountLimit;
        tapMaxDurationMs = o.tapMaxDurationMs;
        clickHoldMs = o.clickHoldMs;
        flickMinDistPx = o.flickMinDistPx;
        bumpScrollEnabled = o.bumpScrollEnabled;
        bumpBorderPx = o.bumpBorderPx;
        bumpDeltaPx = o.bumpDeltaPx;
        bumpPeriodMs = o.bumpPeriodMs;
        bumpStopOutsideBorder = o.bumpStopOutsideBorder;
        zoomRatio = o.zoomRatio;
        scrollRatio = o.scrollRatio;
        wheelStepPx = o.wheelStepPx;
        naturalScrolling = o.naturalScrolling;
        accelEnabled = o.accelEnabled;
        accelDrainHistory = o.accelDrainHistory;
        accelResetOnDown = o.accelResetOnDown;
        accelBase = o.accelBase;
        accelExponent = o.accelExponent;
        accelMin = o.accelMin;
        accelMax = o.accelMax;
        accelAdaptive = o.accelAdaptive;
        accelSlowSpeedPx = o.accelSlowSpeedPx;
        accelFullSpeedPx = o.accelFullSpeedPx;
        axisLockEnabled = o.axisLockEnabled;
        axisLockMaxSpeedPx = o.axisLockMaxSpeedPx;
        axisLockEnterRatio = o.axisLockEnterRatio;
        axisLockExitRatio = o.axisLockExitRatio;
        axisLockTurnSpanPx = o.axisLockTurnSpanPx;
        axisLockMaxTurnDeg = o.axisLockMaxTurnDeg;
        axisLockCornerDeg = o.axisLockCornerDeg;
        rawMotionWhenRelative = o.rawMotionWhenRelative;
        overlayStripWidthPx = o.overlayStripWidthPx;
        overlayRowHeightPx = o.overlayRowHeightPx;
        overlayDismissPx = o.overlayDismissPx;
        overlayDismissMarginPx = o.overlayDismissMarginPx;
        overlayMiddleMinPx = o.overlayMiddleMinPx;
        overlayWheelTickMs = o.overlayWheelTickMs;
        overlayWheelMaxRate = o.overlayWheelMaxRate;
        overlayWheelTicksPerClick = o.overlayWheelTicksPerClick;
        overlayWheelStartDelayTicks = o.overlayWheelStartDelayTicks;
        keyboardKeyHeightPx = o.keyboardKeyHeightPx;
        keyboardInfoHeightPx = o.keyboardInfoHeightPx;
        keyboardKeyPadPx = o.keyboardKeyPadPx;
        keyboardKeyPadWidePx = o.keyboardKeyPadWidePx;
        keyboardGroupGapPx = o.keyboardGroupGapPx;
        keyboardIconWidthPx = o.keyboardIconWidthPx;
        keyboardMinKeyWidthPx = o.keyboardMinKeyWidthPx;
        keyboardScrollSlopPx = o.keyboardScrollSlopPx;
        keyboardFlingTickMs = o.keyboardFlingTickMs;
        keyboardFlingDecay = o.keyboardFlingDecay;
        keyboardFlingMinPx = o.keyboardFlingMinPx;
        keyboardFlingStopPx = o.keyboardFlingStopPx;
        keyDoubleTapMs = o.keyDoubleTapMs;
        keyLongPressMs = o.keyLongPressMs;
        keyRepeatMs = o.keyRepeatMs;
        keyboardHaptics = o.keyboardHaptics;
        keyboardInfoMaxChars = o.keyboardInfoMaxChars;
        keyboardInfoSolid = o.keyboardInfoSolid;
        inertiaEnabled = o.inertiaEnabled;
        inertiaStartDelayMs = o.inertiaStartDelayMs;
        inertiaTickMs = o.inertiaTickMs;
        inertiaSpeedScale = o.inertiaSpeedScale;
        inertiaSpeedMax = o.inertiaSpeedMax;
        inertiaDecay = o.inertiaDecay;
        inertiaStopSpeed = o.inertiaStopSpeed;
        inertiaCancelOnDown = o.inertiaCancelOnDown;
        inertiaResetOnDown = o.inertiaResetOnDown;
        recentreCursorOnZoom = o.recentreCursorOnZoom;
        coalescePointerEvents = o.coalescePointerEvents;
        dedupePointerEvents = o.dedupePointerEvents;
        panMarginInsets = o.panMarginInsets;
        panMarginPx = o.panMarginPx;
        mouseCapture = o.mouseCapture;
        mouseSpeed = o.mouseSpeed;
        mouseWheelStep = o.mouseWheelStep;
    }

    /** Bug-for-bug reproduction of RealVNC Viewer 4.9.1. */
    public static Config faithful(float density) {
        Config c = new Config(density);
        c.faithfulPreset = true;
        c.moveCountTest = true;
        c.tapPathThresholdPx = 0;
        c.tapMaxDurationMs = 0;
        c.bumpStopOutsideBorder = false;
        c.accelDrainHistory = true;
        c.accelResetOnDown = false;
        c.inertiaCancelOnDown = false;
        c.inertiaResetOnDown = false;
        c.coalescePointerEvents = false;
        c.dedupePointerEvents = false;
        return c;
    }

    /** The version we actually want to ship. */
    public static Config improved(float density) {
        Config c = new Config(density);
        c.faithfulPreset = false;
        c.moveCountTest = false;
        c.tapPathThresholdPx = c.dp(24);
        c.tapMaxDurationMs = 0;
        c.bumpStopOutsideBorder = true;
        c.accelDrainHistory = true; // still the original curve; toggle separately
        c.accelAdaptive = true;
        c.axisLockEnabled = true;
        c.accelResetOnDown = true;
        c.inertiaCancelOnDown = true;
        c.inertiaResetOnDown = true;
        c.coalescePointerEvents = true;
        c.dedupePointerEvents = true;
        return c;
    }
}
