// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.input;

/**
 * Pointer acceleration, ported from RealVNC Viewer's.
 *
 * <p>The input is <em>jerk</em> — the change in per-event displacement — not
 * speed, so moving at a constant velocity gives a factor of ~1.1 no matter how
 * fast, and only changes in speed get amplified. That is why the original feels
 * snappy on flicks but precise on slow movement without any explicit precision
 * mode.
 *
 * <pre>
 *   factor = clamp((sqrt(1.1) + jerk/dt) ^ 2.6, 1.1, 5.0)
 * </pre>
 *
 * <p>The original's 3-sample history is a FIFO that gets fully <em>drained</em>
 * whenever it fills, so a real factor is only computed on every 3rd move event
 * and the other two get the 1.1 floor — the emitted motion is a
 * {@code 1.1, 1.1, F, 1.1, 1.1, F} sawtooth. Almost certainly unintentional, but
 * it is what the original feels like, so it is reproduced here and
 * {@link Config#accelDrainHistory} switches to a proper sliding window.
 *
 * <p>Two mechanisms of our own hang off the same path ({@code ARCHITECTURE.md}
 * §3.1 and §3.2), both on in {@link Config#improved}: an adaptive speed gate on
 * the factor and axis locking. Both read the speed estimate below.
 */
public final class PointerAccel {

    // Weight of the newest sample in the speed and per-axis magnitude estimates.
    // 0.5 settles within a few percent in three events, which is as long as a
    // gate can take without being felt.
    private static final float EMA_ALPHA = 0.5f;

    private static final float NOMINAL_GAP_MS = 8.0f; // for the first event of a gesture

    // The short direction estimate's span, as a fraction of the long one's. The
    // angle between the two measures how hard the path is turning, so the
    // difference in their lag is the sensitivity.
    private static final float TURN_FAST_FRACTION = 0.25f;

    private final Config cfg;
    private final MouseSink sink;

    private final float[] dx = new float[3];
    private final float[] dy = new float[3];
    private final long[] time = new long[3];
    private int size;

    private float lastDt = 1.0f;
    private float lastFactor = 1.0f;

    // --- running estimates, updated on every event regardless of the window --
    private long lastTime = -1;
    private float lastGapMs = NOMINAL_GAP_MS;
    private float speed;             // px/ms
    private float magX, magY;        // smoothed |dx|, |dy|
    private Axis lock = Axis.NONE;

    // --- direction, smoothed over two different path lengths ----------------
    private float fastX, fastY, slowX, slowY;

    /** Which axis, if either, the movement is currently locked to. */
    public enum Axis {
        NONE("-"), X("X"), Y("Y");

        private final String label;

        Axis(String label) {
            this.label = label;
        }

        @Override
        public String toString() {
            return label;
        }
    }

    public PointerAccel(Config cfg, MouseSink sink) {
        this.cfg = cfg;
        this.sink = sink;
    }

    /** Drop the sample history (the original never does this between gestures). */
    public void reset() {
        size = 0;
        lastTime = -1;
        lastGapMs = NOMINAL_GAP_MS;
        lastDt = 1.0f;
        lastFactor = 1.0f;
        speed = 0;
        magX = magY = 0;
        lock = Axis.NONE;
        fastX = fastY = slowX = slowY = 0;
    }

    public float lastFactor() {
        return lastFactor;
    }

    /** Smoothed finger speed, px/ms — what the adaptive gate reads. */
    public float speed() {
        return speed;
    }

    public Axis axisLock() {
        return lock;
    }

    /**
     * How far the direction has turned over the last
     * {@link Config#axisLockTurnSpanPx} of path, in degrees. ~0 on a straight
     * drag however noisy, a steady non-zero on a curve, and a spike at a corner.
     */
    public float turnDegrees() {
        final float cross = fastX * slowY - fastY * slowX;
        final float dot = fastX * slowX + fastY * slowY;
        if (cross == 0 && dot == 0) {
            return 0;
        }
        return (float) Math.abs(Math.toDegrees(Math.atan2(cross, dot)));
    }

    public void move(float ax, float ay, long t) {
        track(ax, ay, t);

        if (cfg.axisLockEnabled) {
            lock = nextLock();
            if (lock == Axis.X) {
                ay = 0;
            } else if (lock == Axis.Y) {
                ax = 0;
            }
        } else {
            lock = Axis.NONE;
        }

        float factor = 1.0f;
        if (cfg.accelEnabled) {
            factor = cfg.accelMin;
            push(ax, ay, t);
            if (size == 3) {
                final float jx = dx[1] - dx[0];
                final float jy = dy[1] - dy[0];
                final float jerk = (float) Math.sqrt(jx * jx + jy * jy);

                float dt = Math.abs(time[2] - time[0]);
                if (dt == 0.0f) {
                    dt = lastDt;
                } else {
                    lastDt = dt;
                }

                final float f = (float) Math.pow(cfg.accelBase + jerk / dt, cfg.accelExponent);
                if (f >= cfg.accelMin) {
                    factor = Math.min(f, cfg.accelMax);
                }
                if (cfg.accelAdaptive) {
                    factor = gate(factor);
                }

                if (cfg.accelDrainHistory) {
                    size = 0;
                } else {
                    System.arraycopy(dx, 1, dx, 0, 2);
                    System.arraycopy(dy, 1, dy, 0, 2);
                    System.arraycopy(time, 1, time, 0, 2);
                    size = 2;
                }
            }
        }
        lastFactor = factor;
        sink.mouseMove(ax * factor, ay * factor);
    }

    /**
     * Speed and per-axis magnitude estimates, always from the <em>raw</em>
     * delta: feeding the locked delta back in would drive the minor axis's
     * estimate to zero and latch the lock permanently.
     */
    private void track(float ax, float ay, long t) {
        float gap = (lastTime < 0) ? 0 : (float) (t - lastTime);
        lastTime = t;
        if (gap <= 0) {
            gap = lastGapMs;
        } else {
            lastGapMs = gap;
        }
        final float dist = (float) Math.sqrt(ax * ax + ay * ay);
        final float inst = dist / gap;
        speed += (inst - speed) * EMA_ALPHA;
        magX += (Math.abs(ax) - magX) * EMA_ALPHA;
        magY += (Math.abs(ay) - magY) * EMA_ALPHA;

        // Direction, low-passed over two path lengths rather than two event
        // counts: the lock's speed band spans more than a decade, so an
        // event-weighted estimate would be a curvature threshold that moves.
        final float span = Math.max(cfg.axisLockTurnSpanPx, 1e-3f);
        final float wSlow = Math.min(1.0f, dist / span);
        final float wFast = Math.min(1.0f, dist / (span * TURN_FAST_FRACTION));
        slowX += (ax - slowX) * wSlow;
        slowY += (ay - slowY) * wSlow;
        fastX += (ax - fastX) * wFast;
        fastY += (ay - fastY) * wFast;
        if (turnDegrees() > cfg.axisLockCornerDeg) {
            // Not a curve — a corner. Forgetting the direction outright is what
            // locks "up, then across" a few events into the across rather than a
            // whole span into it.
            slowX = fastX = ax;
            slowY = fastY = ay;
        }
    }

    /**
     * Fade the amplified part of the factor in between
     * {@link Config#accelSlowSpeedPx} and {@link Config#accelFullSpeedPx}. The
     * {@link Config#accelMin} floor is never touched, so this can only ever slow
     * the cursor down relative to the original, never speed it up, and a gesture
     * fast enough to matter gets the original's factor exactly.
     */
    private float gate(float factor) {
        if (factor <= cfg.accelMin) {
            return factor;
        }
        final float lo = cfg.accelSlowSpeedPx, hi = cfg.accelFullSpeedPx;
        float g = (hi > lo) ? (speed - lo) / (hi - lo) : 1.0f;
        if (g <= 0) {
            return cfg.accelMin;
        }
        if (g < 1) {
            g = g * g * (3 - 2 * g); // smoothstep: no kink at either end
        } else {
            g = 1;
        }
        return cfg.accelMin + (factor - cfg.accelMin) * g;
    }

    /**
     * The lock decision, from the smoothed per-axis magnitudes rather than the
     * current event: at the speeds this engages at, a single delta is one or two
     * quantised pixels and its ratio is pure noise.
     *
     * <p>Three things have to hold: the movement is slow, it is near-axial, and
     * its <em>direction</em> is not turning. A circle is near-axial four times
     * per revolution, so the ratio test alone locks four flat spots into it;
     * what tells it apart from a drag along an axis is that the direction keeps
     * rotating while the ratio test is happy.
     */
    private Axis nextLock() {
        if (speed >= cfg.axisLockMaxSpeedPx) {
            return Axis.NONE;
        }
        if (turnDegrees() > cfg.axisLockMaxTurnDeg) {
            return Axis.NONE;
        }
        final float major = Math.max(magX, magY);
        if (major <= 0) {
            return lock;
        }
        final float ratio = Math.min(magX, magY) / major;
        final boolean locked = (lock == Axis.NONE)
                ? ratio <= cfg.axisLockEnterRatio
                : ratio <= cfg.axisLockExitRatio;
        if (!locked) {
            return Axis.NONE;
        }
        return (magX >= magY) ? Axis.X : Axis.Y;
    }

    private void push(float ax, float ay, long t) {
        if (size == 3) {
            System.arraycopy(dx, 1, dx, 0, 2);
            System.arraycopy(dy, 1, dy, 0, 2);
            System.arraycopy(time, 1, time, 0, 2);
            size = 2;
        }
        dx[size] = ax;
        dy[size] = ay;
        time[size] = t;
        size++;
    }
}
