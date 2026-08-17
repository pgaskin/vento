// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.input;

/**
 * A detent on the pointer where the far end changed the cursor's shape.
 *
 * <p>A shape change says one thing: whatever is under the pointer is different
 * now. It says nothing about where the boundary was, how big the thing is or
 * what it is, and the whole design follows from that being all it says — the
 * pointer loses a little distance <em>after</em> the news, over a bounded span
 * of finger travel, so that a small target is easier to stop on and harder to
 * slide off. Nothing is pulled anywhere and nothing teleports.
 *
 * <p><b>The news is a reply, and it is late.</b> It was caused by a pointer
 * position sent a round trip ago, so the pointer is already past the boundary
 * when it arrives: measured, ten to thirty milliseconds against most far ends
 * and a hundred and forty against one that polls its own screen. That is why
 * the detent is applied ahead rather than centred on anything, why the span has
 * to be longer than the lag's travel, and why {@link #changed} refuses to arm
 * for a far end measured too late to be helping with the boundary it named.
 *
 * <p>The budget is in <em>finger</em> travel rather than desktop pixels, which
 * is a decision: it costs a constant amount of movement and buys
 * {@code span * (1 - gain) / 2 / scale} of desktop stickiness — full strength
 * zoomed out, where a link is four screen pixels tall and the help is wanted,
 * and negligible zoomed in, where it would cost real travel for nothing.
 */
final class HoverAssist {

    /**
     * How long after a gesture's last move a shape change still measures the
     * far end's lateness. Beyond this it is the far end's own business — a
     * spinner, a window opening — and not a reply to anything we sent.
     */
    private static final long LAG_WINDOW_MS = 600;

    /** Enough that two slow sessions in a row move the estimate, not one shape. */
    private static final float LAG_ALPHA = 0.3f;

    private final Config cfg;

    private float budget;       // finger travel left in the detent, px
    private float armX, armY;   // the direction it armed in; a reversal drops the rest
    private float lastDx, lastDy;
    private float travel;       // since the last change that armed
    private long lastMoveTime = -1;
    private float gain = 1.0f;

    /**
     * The last move of a gesture that has ended, or -1 while one is running.
     * A change arriving after it is the only kind whose lateness can be
     * measured: nothing has moved since, so the event that caused it is known.
     */
    private long gestureEnd = -1;
    private float lag;

    // A shape changing on a timer would otherwise re-arm for ever, which is
    // what these three count.
    private long burstStart = -1;
    private int burst;
    private long lockoutUntil = -1;

    HoverAssist(Config cfg) {
        this.cfg = cfg;
    }

    void reset() {
        budget = 0;
        travel = 0;
        lastMoveTime = -1;
        gestureEnd = -1;
        gain = 1.0f;
        burstStart = -1;
        burst = 0;
        lockoutUntil = -1;
        // The lag estimate survives: it is a fact about the far end, and a
        // session's worth of it is thrown away by a finger going down.
    }

    /** For the HUD: 1 outside a detent, {@link Config#hoverAssistGain} at its catch. */
    float gain() {
        return gain;
    }

    /** The far end's measured lateness, ms, or 0 before anything has measured it. */
    float lagMs() {
        return lag;
    }

    boolean armed() {
        return budget > 0;
    }

    /**
     * Whether a burst of changes has shut the mechanism off for the moment,
     * which is worth a readout: it is the one state in which nothing happens
     * for a reason that is not "the far end said nothing".
     */
    boolean lockedOut(long now) {
        return now < lockoutUntil;
    }

    /**
     * The factor this move is multiplied by, and the raw distance charged
     * against the budget. Fed the delta before the axis lock has been at it,
     * since what is being spent is finger travel rather than what came out.
     */
    float gain(float dx, float dy, long t) {
        lastMoveTime = t;
        gestureEnd = -1;
        lastDx = dx;
        lastDy = dy;
        final float dist = (float) Math.sqrt(dx * dx + dy * dy);
        travel += dist;
        if (budget <= 0) {
            return gain = 1.0f;
        }
        if (dx * armX + dy * armY < 0) {
            // Somebody reversing is somebody coming back to what they
            // overshot, and slowing that down is the opposite of help.
            budget = 0;
            return gain = 1.0f;
        }
        final float span = Math.max(cfg.hoverAssistSpanPx, 1e-3f);
        float u = (span - budget) / span; // 0 at the change, 1 where the budget runs out
        u = u < 0 ? 0 : (u > 1 ? 1 : u);
        budget -= dist;
        final float s = u * u * (3 - 2 * u); // smoothstep: no kink at either end
        return gain = cfg.hoverAssistGain + (1 - cfg.hoverAssistGain) * s;
    }

    /** No finger left on the screen, so the next change can be timed. */
    void gestureEnded() {
        if (lastMoveTime >= 0) {
            gestureEnd = lastMoveTime;
        }
    }

    /**
     * The far end has changed the cursor's shape. Five things have to hold
     * before that opens a detent, and four of them are the defence against a
     * shape that changes for reasons that are not a boundary.
     *
     * @param speed   the smoothed finger speed {@link PointerAccel} keeps, px/ms
     * @param gliding whether a flick's momentum is still moving the pointer,
     *                which makes every change it causes untimeable: the pointer
     *                is moving with no finger behind it, so neither the arming
     *                tests nor the lag estimate have an event to measure from
     */
    void changed(float speed, long t, boolean gliding) {
        if (!cfg.hoverAssistEnabled || gliding) {
            return;
        }
        if (gestureEnd >= 0 && t - gestureEnd <= LAG_WINDOW_MS) {
            // How late this far end is, from the one kind of change whose cause
            // is known. A lower bound rather than the interval itself — the
            // change may have been caused by an earlier event of the same
            // gesture — and the threshold it is compared against is set knowing
            // that. It cannot tell a late reply from a change the far end made
            // for itself, and does not have to: both are arguments against
            // arming.
            lag += ((t - gestureEnd) - lag) * LAG_ALPHA;
        }
        if (t < lockoutUntil) {
            return;
        }
        if (speed >= cfg.hoverAssistMaxSpeedPx) {
            return; // a purposeful drag is not aiming at anything
        }
        if (lastMoveTime < 0 || t - lastMoveTime > cfg.hoverAssistIdleMs) {
            return; // nothing was moving, so this was not caused by us
        }
        if (travel < cfg.hoverAssistMinTravelPx) {
            return; // two shapes in the same place are frames, not boundaries
        }
        if (lag > cfg.hoverAssistMaxLagMs) {
            return; // the news is too old to be about where the pointer is
        }
        // The burst is counted over the changes that would otherwise arm,
        // rather than over everything that arrives: a hand crossing a desktop
        // full of small objects produces a dozen changes on the way, and
        // locking out because of them is locking out the aim that follows.
        if (burstStart < 0 || t - burstStart > cfg.hoverAssistBurstMs) {
            burstStart = t;
            burst = 0;
        }
        if (++burst > cfg.hoverAssistBurstCount) {
            // An animated cursor running while the finger moves slowly is the
            // one case the other tests all pass.
            lockoutUntil = t + cfg.hoverAssistLockoutMs;
            budget = 0;
            return;
        }
        final float len = (float) Math.sqrt(lastDx * lastDx + lastDy * lastDy);
        if (len <= 0) {
            return;
        }
        armX = lastDx / len;
        armY = lastDy / len;
        budget = cfg.hoverAssistSpanPx;
        travel = 0;
    }
}
