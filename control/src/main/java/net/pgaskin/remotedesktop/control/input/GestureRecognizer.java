// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.input;

/**
 * The touchpad gesture state machine, ported from RealVNC Viewer's;
 * {@code ARCHITECTURE.md} §1.2–§1.7 is the specification it was written
 * from.
 *
 * <p>Only two pointers are ever tracked in detail; a high-water mark of the
 * number of fingers down ({@link #maxDown}) is what picks the mouse button.
 * The three mechanisms worth knowing:
 *
 * <ol>
 *   <li><b>Taps are recognised on touch-up.</b> The button is pressed when the
 *       last finger lifts and auto-released {@link Config#clickHoldMs} later.
 *   <li><b>A touch-down inside that release window cancels the release</b>, so
 *       the button stays held. That single rule produces double-click,
 *       double-tap-drag <i>and</i> two-finger-tap-then-drag as right-drag —
 *       there is no double-tap timer or tap counter anywhere.
 *   <li><b>Two-finger mode is decided once per gesture</b> from
 *       {@code |Δseparation| / totalTravel}: above 0.7 it is a pinch, below 0.2
 *       it is a scroll wheel, in between it stays undecided and is re-tested on
 *       the next move.
 * </ol>
 *
 * <p>Deviations from the original are all behind {@link Config} flags; the
 * structural ones (always on) are float coordinates and pointer-id-keyed slots.
 */
public final class GestureRecognizer implements TouchRouter.Listener {

    /** What a multi-finger gesture has been decided to be. */
    public enum Mode {
        /** One finger, or two that have not yet declared themselves. */
        NONE("-"),
        ZOOM("ZOOM"),
        SCROLL("SCROLL"),
        /** Three or more fingers: no gesture, but not a tap either. */
        MULTI("MULTI");

        private final String label;

        Mode(String label) {
            this.label = label;
        }

        @Override
        public String toString() {
            return label;
        }
    }

    private final Config cfg;
    private final MouseSink sink;
    private final ZoomSink zoomSink;
    private final Scheduler handler;

    private final PointerAccel accel;
    private final PointerInertia inertia;

    // --- the two tracked pointer slots ------------------------------------
    private final int[] slotId = {-1, -1};
    private final float[] startX = new float[2], startY = new float[2];
    private final float[] curX = new float[2], curY = new float[2];
    private final boolean[] slotMoved = new boolean[2];
    private final int[] slotMoves = new int[2];
    private final float[] slotPath = new float[2];
    private final long[] slotDownTime = new long[2];

    private int down;      // fingers currently down
    private int maxDown;   // high-water mark for this gesture
    private Mode mode = Mode.NONE;
    private boolean relative; // the far end owns the cursor; see setRelative

    private float pinchStartDist;
    private float midX, midY;
    private final WheelSteps wheel;

    private Button heldButton;     // null when nothing is held
    private Runnable pendingRelease;

    // --- tap regions (ours; the original has a floating toolbar instead) ----
    private TapRegions regions;
    private RegionSink regionSink;

    // --- bump scroll -------------------------------------------------------
    private boolean bumpArmed;
    private boolean externalHold;
    private float bumpDx, bumpDy;
    private boolean bumpRunning;
    private final Runnable bumpTask = new Runnable() {
        @Override
        public void run() {
            // The border test lives on the one-finger path, so with a second
            // finger down nothing is left to stop this: it would go on
            // stepping 12 dp every 100 ms for the rest of the gesture, fingers
            // still and well away from any border. Armed stays armed, so
            // lifting back to one finger resumes.
            if (down != 1) {
                stopBumpScroll();
                return;
            }
            sink.mouseMove(bumpDx, bumpDy);
            handler.postDelayed(this, cfg.bumpPeriodMs);
        }
    };

    // --- flick detection: the last two single-finger positions -------------
    private final float[] flickX = new float[2], flickY = new float[2];
    private int flickCount;

    private int viewW, viewH;

    public GestureRecognizer(Config cfg, MouseSink sink, ZoomSink zoomSink, Scheduler scheduler) {
        this.cfg = cfg;
        this.sink = sink;
        this.wheel = new WheelSteps(sink);
        this.zoomSink = zoomSink;
        this.handler = scheduler;
        this.accel = new PointerAccel(cfg, sink);
        this.inertia = new PointerInertia(cfg, sink, scheduler);
    }

    public void setViewSize(int w, int h) {
        viewW = w;
        viewH = h;
    }

    /**
     * Route single-finger taps landing in one of {@code regions} to {@code sink}
     * instead of clicking. Pass {@code null} for either to turn them off, which
     * is the default; see {@link TapRegions} for what the hook does and does not
     * intercept.
     */
    public void setRegions(TapRegions regions, RegionSink sink) {
        this.regions = regions;
        this.regionSink = sink;
    }

    /**
     * Which end owns the cursor, which the host learns from the far end and
     * tells the cursor controller at the same time.
     *
     * <p>The state machine itself does not care — a finger is a finger — but
     * everything that decides <em>how far</em> a cursor goes for a given finger
     * belongs to whichever end owns it, and that is
     * {@link Config#rawMotionWhenRelative}.
     */
    public void setRelative(boolean relative) {
        this.relative = relative;
    }

    /**
     * The far end has changed the shape of its cursor, which is the one thing
     * it ever says about what is under the pointer — see
     * {@code ARCHITECTURE.md} §3.20. Ignored where the far end owns the cursor:
     * nothing here is shaping that motion, and a picture with a pointer drawn
     * into it has no shape to arrive anyway.
     *
     * @param t when it arrived, not when the crossing it reports happened
     */
    public void remoteCursorChanged(long t) {
        if (relative && cfg.rawMotionWhenRelative) {
            return;
        }
        accel.remoteCursorChanged(t, inertia.speed() > 0);
    }

    /**
     * Tell the state machine that something outside it — the mouse overlay — is
     * holding a mouse button down. The only thing it changes is that bump
     * scroll (§1.6) arms for a drag started while that button is held, which is
     * the case the original cannot have: its only way to hold a button is the
     * 250 ms window.
     */
    public void setExternalButtonHeld(boolean held) {
        this.externalHold = held;
    }

    // ---- debug accessors --------------------------------------------------

    public int downCount() {
        return down;
    }

    public int maxDownCount() {
        return maxDown;
    }

    public Mode mode() {
        return mode;
    }

    /** The button a tap left held, or {@code null}. */
    public Button heldButton() {
        return heldButton;
    }

    public boolean moving() {
        return isMoving();
    }

    public float accelFactor() {
        return accel.lastFactor();
    }

    /** Smoothed finger speed, px/ms — what the adaptive gate and axis lock read. */
    public float accelSpeed() {
        return accel.speed();
    }

    public PointerAccel.Axis axisLock() {
        return accel.axisLock();
    }

    /** How far the direction has turned recently — the axis lock's curve test. */
    public float turnDegrees() {
        return accel.turnDegrees();
    }

    /** The hover detent's factor, 1 outside one. */
    public float hoverGain() {
        return accel.hoverGain();
    }

    /** How late the far end's shape changes are running, ms. */
    public float hoverLagMs() {
        return accel.hoverLagMs();
    }

    /** Whether a burst of shape changes has shut the detent off for the moment. */
    public boolean hoverLockedOut(long now) {
        return accel.hoverLockedOut(now);
    }

    public double glideSpeed() {
        return inertia.speed();
    }

    // ---- TouchRouter.Listener --------------------------------------------

    @Override
    public void touchBegan(int id, float x, float y, long t) {
        final boolean secondFinger = (down == 1);

        if (down < 2) {
            final int s = (slotId[0] < 0) ? 0 : 1;
            slotId[s] = id;
            startX[s] = curX[s] = x;
            startY[s] = curY[s] = y;
            slotMoves[s] = 0;
            slotPath[s] = 0;
            slotDownTime[s] = t;
        }

        if (secondFinger) {
            // Second finger down: arm the pinch/scroll gesture.
            setMode(Mode.NONE);
            pinchStartDist = dist(curX[0], curY[0], curX[1], curY[1]);
            midX = (curX[0] + curX[1]) / 2.0f;
            midY = (curY[0] + curY[1]) / 2.0f;
            if (zoomSink != null) {
                zoomSink.scaleCentre(midX, midY);
            }
        } else if (down == 0) {
            // First finger of a new gesture. If a click release is still
            // pending, cancel it — the button stays held, which is what turns
            // tap+drag into a button drag and tap+tap into a double click.
            final boolean resumed = pendingRelease != null;
            if (resumed) {
                handler.removeCallbacks(pendingRelease);
                pendingRelease = null;
            }
            // Armed while a button is held, whoever is holding it: a button held
            // by the mouse overlay is a drag too, and the original knows only
            // about its own 250 ms window.
            bumpArmed = cfg.bumpScrollEnabled && (resumed || externalHold);
            if (cfg.accelResetOnDown) {
                accel.reset();
            }
            if (cfg.inertiaCancelOnDown) {
                inertia.cancel();
            }
        }

        down++;
        // The original clears both "has moved" flags on every touch-down.
        slotMoved[0] = false;
        slotMoved[1] = false;
        maxDown = Math.max(maxDown, down);
    }

    @Override
    public void touchMoved(int id, float prevX, float prevY, float x, float y, long t) {
        if (maxDown == 1) {
            pushFlick(x, y);
        }

        final int s = slotOf(id);
        if (s >= 0) {
            curX[s] = x;
            curY[s] = y;
            slotMoves[s]++;
            slotPath[s] += dist(prevX, prevY, x, y);
            if (!slotMoved[s]) {
                slotMoved[s] = hasMoved(s, t);
            }
        }

        if (!isMoving()) {
            return;
        }

        if (down == 1) {
            if (s < 0) {
                return; // the moving finger isn't one of the tracked slots
            }
            updateBumpScroll(x, y);
            final float dx = x - prevX;
            final float dy = y - prevY;
            if (relative && cfg.rawMotionWhenRelative) {
                // Straight out, and no samples kept: a glide is one more way of
                // deciding how far the cursor goes, and the far end is deciding
                // that. The accelerator is left untouched rather than driven
                // and ignored, so its readouts report the last gesture that
                // went through it.
                sink.mouseMove(dx, dy);
            } else {
                accel.move(dx, dy, t);   // emits the (accelerated) mouse motion
                inertia.sample(dx, dy, t); // momentum samples the raw delta
            }
        } else if (down == 2) {
            twoFingerMoved();
        } else {
            setMode(Mode.MULTI);
        }
    }

    @Override
    public void touchEnded(int id, float prevX, float prevY, float x, float y, long t) {
        final int s = slotOf(id);
        if (s >= 0) {
            curX[s] = x;
            curY[s] = y;
            slotId[s] = -1;
        }
        release(false);
    }

    @Override
    public void touchCancelled(long t) {
        while (down > 0) {
            release(true);
        }
    }

    /**
     * Everything this recognizer can still be doing with no finger on the
     * screen, stopped — for a session going away while the remote is left
     * holding whatever we last sent it.
     *
     * <p>Deliberately more than {@link #touchCancelled}, which is the touch
     * stream ending and nothing else. Three things outlive a gesture by design —
     * the button a tap holds for {@link Config#clickHoldMs}, a glide, and bump
     * scroll's timer — and all three are wrong once nobody is looking at the
     * screen, because a button held here is held at the far end for as long as
     * the session lives.
     */
    public void cancelAll(long t) {
        touchCancelled(t);
        stopBumpScroll();
        bumpArmed = false;
        inertia.cancel();
        if (pendingRelease != null) {
            handler.removeCallbacks(pendingRelease);
            pendingRelease = null;
        }
        if (heldButton != null) {
            sink.mouseUp(heldButton.mask());
            heldButton = null;
        }
        reset();
    }

    // ---- internals --------------------------------------------------------

    private int slotOf(int id) {
        if (slotId[0] == id) return 0;
        if (slotId[1] == id) return 1;
        return -1;
    }

    /**
     * The original reads slot 0's flag whenever one finger is down, which
     * silently breaks a one-finger drag continued after lifting the
     * <em>first</em> of two fingers. Use whichever slot is actually still down.
     */
    private boolean isMoving() {
        if (down == 1) {
            if (slotId[0] >= 0) return slotMoved[0];
            if (slotId[1] >= 0) return slotMoved[1];
            return false;
        }
        return slotMoved[0] && slotMoved[1];
    }

    private boolean hasMoved(int s, long t) {
        if (Math.abs(curX[s] - startX[s]) >= cfg.moveThresholdPx
                || Math.abs(curY[s] - startY[s]) >= cfg.moveThresholdPx) {
            return true;
        }
        if (cfg.tapPathThresholdPx > 0 && slotPath[s] >= cfg.tapPathThresholdPx) {
            return true;
        }
        if (cfg.moveCountTest && slotMoves[s] > cfg.moveCountLimit) {
            return true; // the vncpatch#1 bug, kept for A/B
        }
        return cfg.tapMaxDurationMs > 0 && (t - slotDownTime[s]) > cfg.tapMaxDurationMs;
    }

    private void setMode(Mode m) {
        if (mode == m) {
            return;
        }
        if (mode == Mode.ZOOM && zoomSink != null) {
            zoomSink.zoomEnded();
        }
        mode = m;
        if (mode == Mode.ZOOM && zoomSink != null) {
            zoomSink.zoomBegan();
        }
    }

    private void twoFingerMoved() {
        final float travel = dist(startX[0], startY[0], curX[0], curY[0])
                + dist(startX[1], startY[1], curX[1], curY[1]);
        final float spread = Math.abs(dist(startX[0], startY[0], startX[1], startY[1])
                - dist(curX[0], curY[0], curX[1], curY[1]));
        if (travel == 0.0f) {
            return;
        }
        if (mode == Mode.NONE) {
            final float ratio = spread / travel;
            if (ratio > cfg.zoomRatio) {
                setMode(Mode.ZOOM);
            } else if (ratio < cfg.scrollRatio) {
                setMode(Mode.SCROLL);
            }
        }
        if (mode == Mode.ZOOM) {
            // The midpoint is the other half of a pinch, and the original uses
            // only the separation. It is read here rather than every two-finger
            // frame so that the mode still consumes the travel it was decided
            // by: the first delta of either mode covers everything since the
            // second finger landed.
            final float mx = (curX[0] + curX[1]) / 2.0f;
            final float my = (curY[0] + curY[1]) / 2.0f;
            if (zoomSink != null) {
                if (pinchStartDist > 0) {
                    zoomSink.zoomChanged(dist(curX[0], curY[0], curX[1], curY[1]) / pinchStartDist);
                }
                zoomSink.zoomPanned(mx - midX, my - midY);
            }
            midX = mx;
            midY = my;
        } else if (mode == Mode.SCROLL) {
            final float mx = (curX[0] + curX[1]) / 2.0f;
            final float my = (curY[0] + curY[1]) / 2.0f;
            emitWheel(mx - midX, my - midY);
            midX = mx;
            midY = my;
        }
    }

    /** One wheel click per {@link Config#wheelStepPx} of raw travel, per axis. */
    private void emitWheel(float dx, float dy) {
        final float sign = cfg.naturalScrolling ? -1.0f : 1.0f;
        wheel.add(dx * sign, dy * sign, cfg.wheelStepPx);
    }

    private void click(Button button) {
        sink.mouseDown(button.mask());
        sink.mouseUp(button.mask());
    }

    private void updateBumpScroll(float x, float y) {
        if (!bumpArmed) {
            stopBumpScroll();
            return;
        }
        bumpDx = 0;
        bumpDy = 0;
        if (y < cfg.bumpBorderPx) bumpDy = -cfg.bumpDeltaPx;
        if (y > viewH - cfg.bumpBorderPx) bumpDy = cfg.bumpDeltaPx;
        if (x < cfg.bumpBorderPx) bumpDx = -cfg.bumpDeltaPx;
        if (x > viewW - cfg.bumpBorderPx) bumpDx = cfg.bumpDeltaPx;

        if (bumpDx == 0 && bumpDy == 0 && cfg.bumpStopOutsideBorder) {
            stopBumpScroll();
            return;
        }
        sink.mouseMove(bumpDx, bumpDy);
        if (!bumpRunning) {
            bumpRunning = true;
            handler.postDelayed(bumpTask, cfg.bumpPeriodMs);
        }
    }

    private void stopBumpScroll() {
        if (bumpRunning) {
            bumpRunning = false;
            handler.removeCallbacks(bumpTask);
        }
    }

    /** A finger lifted. Only does anything once they are all up. */
    private void release(boolean cancelled) {
        if (down == 0) {
            return;
        }
        if (--down != 0) {
            return;
        }

        bumpArmed = false;
        stopBumpScroll();
        accel.gestureEnded();

        if (heldButton != null) {
            sink.mouseUp(heldButton.mask());
            heldButton = null;
        }

        if (slotMoved[0] || slotMoved[1] || cancelled) {
            // It was a drag: no click, but maybe a flick. Not where the motion
            // went out raw — nothing sampled it, and the samples still in there
            // are an older gesture's.
            if (!cancelled && !(relative && cfg.rawMotionWhenRelative) && flickCount >= 2
                    && dist(flickX[0], flickY[0], flickX[1], flickY[1]) > cfg.flickMinDistPx) {
                inertia.flick();
            }
        } else if (!regionTapped()) {
            final Button button = switch (maxDown) {
                case 1 -> Button.LEFT;
                case 2 -> Button.RIGHT;
                case 3 -> Button.MIDDLE;
                default -> null; // 4+ fingers: nothing
            };
            if (button != null) {
                press(button);
            }
        }
        reset();
    }

    /**
     * Offer a one-finger tap to the tap regions. The position tested is where
     * the finger <em>landed</em>: a tap has by definition not travelled far, and
     * the touch-down is what the user aimed.
     */
    private boolean regionTapped() {
        if (regions == null || regionSink == null || maxDown != 1) {
            return false;
        }
        final TapRegions.Region r = regions.hit(startX[0], startY[0], viewW, viewH);
        return r != null && regionSink.regionTapped(r, startX[0], startY[0]);
    }

    /** Press now, release {@link Config#clickHoldMs} later. */
    private void press(Button button) {
        sink.mouseDown(button.mask());
        heldButton = button;
        pendingRelease = () -> {
            pendingRelease = null;
            if (heldButton != null) {
                sink.mouseUp(heldButton.mask());
                heldButton = null;
            }
        };
        handler.postDelayed(pendingRelease, cfg.clickHoldMs);
    }

    /** End of gesture. Note {@code heldButton} deliberately survives. */
    private void reset() {
        down = 0;
        maxDown = 0;
        slotId[0] = slotId[1] = -1;
        slotMoved[0] = slotMoved[1] = false;
        slotMoves[0] = slotMoves[1] = 0;
        slotPath[0] = slotPath[1] = 0;
        wheel.reset();
        flickCount = 0;
        setMode(Mode.NONE);
    }

    private void pushFlick(float x, float y) {
        if (flickCount < 2) {
            flickX[flickCount] = x;
            flickY[flickCount] = y;
            flickCount++;
        } else {
            flickX[0] = flickX[1];
            flickY[0] = flickY[1];
            flickX[1] = x;
            flickY[1] = y;
        }
    }

    private static float dist(float ax, float ay, float bx, float by) {
        final float dx = bx - ax, dy = by - ay;
        return (float) Math.sqrt(dx * dx + dy * dy);
    }
}
