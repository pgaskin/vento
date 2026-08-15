// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.input;

/**
 * Flick momentum, ported from RealVNC Viewer's.
 *
 * <p>Samples the raw (un-accelerated) per-event deltas. On touch-up, if the
 * finger was still moving, a glide is scheduled 50 ms later; any further move
 * cancels it, so momentum only ever starts if the finger really left the
 * screen. The glide direction and magnitude come from the <em>two oldest</em>
 * of the last three samples over the time span of all three — more lag than
 * using the newest samples, which combined with the 50 ms delay means
 * deliberately slowing down at the end of a flick kills the momentum. That is
 * probably exactly why it feels good, so it is preserved.
 *
 * <pre>
 *   speed = min(|d0 + d1| * 25 / (t2 - t0), 200)   px per 10 ms tick
 *   speed *= 0.85 every tick, stop below 3
 * </pre>
 */
public final class PointerInertia {

    private static final int IDLE = 0, SAMPLING = 1, GLIDING = 2;

    private final Config cfg;
    private final MouseSink sink;
    private final Scheduler handler;

    private final float[] dx = new float[3];
    private final float[] dy = new float[3];
    private final long[] time = new long[3];
    private int size;

    private int state = IDLE;
    private double speed;
    private float dirX, dirY;

    private final Runnable startTask = this::startGlide;
    private final Runnable tickTask = this::tick;

    public PointerInertia(Config cfg, MouseSink sink, Scheduler scheduler) {
        this.cfg = cfg;
        this.sink = sink;
        this.handler = scheduler;
    }

    public boolean isGliding() {
        return state == GLIDING;
    }

    public double speed() {
        return state == GLIDING ? speed : 0.0;
    }

    /** Feed a raw move delta. */
    public void sample(float ax, float ay, long t) {
        handler.removeCallbacks(startTask);
        if (state != SAMPLING) {
            handler.removeCallbacks(tickTask);
            state = SAMPLING;
            if (cfg.inertiaResetOnDown) {
                size = 0;
            }
            // The original's recursive re-entry pushes a (0, 0, 0) sentinel
            // here, which matters only for a gesture of one or two move events:
            // with three the sentinel has already fallen out of the window. Not
            // reproduced, because its *timestamp* is absolute — on a phone,
            // where a clock reads in the millions, the span it makes is
            // enormous and the speed is nil, so the original does not glide
            // either; under a test clock that starts at zero the same sentinel
            // makes the span one frame and the glide full speed. Leaving it out
            // is what makes the two agree, and it says the same thing: fewer
            // than three samples do not glide.
        }
        push(ax, ay, t);
    }

    /** Called on touch-up when the finger was still moving. */
    public void flick() {
        if (!cfg.inertiaEnabled) {
            return;
        }
        handler.removeCallbacks(startTask);
        handler.postDelayed(startTask, cfg.inertiaStartDelayMs);
    }

    /** Stop everything (new touch down, or the consumer took over). */
    public void cancel() {
        handler.removeCallbacks(startTask);
        handler.removeCallbacks(tickTask);
        state = IDLE;
        speed = 0;
    }

    private void startGlide() {
        if (size != 3) {
            return;
        }
        state = GLIDING;

        final float vx = dx[0] + dx[1];
        final float vy = dy[0] + dy[1];
        final double mag = Math.sqrt(vx * vx + vy * vy);
        if (mag != 0.0) {
            dirX = (float) (vx / mag);
            dirY = (float) (vy / mag);
        }
        if (time[0] == time[2]) {
            speed = 0;
        } else {
            speed = Math.min(mag * cfg.inertiaSpeedScale / (time[2] - time[0]), cfg.inertiaSpeedMax);
        }

        if (speed > 0) {
            handler.postDelayed(tickTask, cfg.inertiaTickMs);
        } else {
            state = IDLE;
        }
    }

    private void tick() {
        sink.mouseMove((float) (dirX * speed), (float) (dirY * speed));
        speed *= cfg.inertiaDecay;
        if (speed > cfg.inertiaStopSpeed) {
            handler.postDelayed(tickTask, cfg.inertiaTickMs);
        } else {
            speed = 0;
            state = IDLE;
        }
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
