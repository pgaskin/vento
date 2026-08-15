// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.input;

/**
 * The timing services the input stack needs, abstracted so the whole stack can
 * run off a virtual clock in a JVM unit test.
 *
 * <p>The original uses {@code Handler(Looper.getMainLooper())} for the click
 * release / bump scroll / momentum timers, and this stack adds a
 * {@code Choreographer} frame callback for pointer-event coalescing. Both are
 * behind this interface; {@link AndroidScheduler} is the real one.
 *
 * <p>Everything is called on one thread and callbacks must run on that same
 * thread — the state machines are not synchronised.
 */
public interface Scheduler {

    /** Run {@code r} after {@code delayMs}. The same instance may be re-posted. */
    void postDelayed(Runnable r, long delayMs);

    /** Cancel every pending post of {@code r}. */
    void removeCallbacks(Runnable r);

    /**
     * Run {@code r} at the next display frame. At most one post is outstanding
     * per instance: posting one that is already queued does nothing, since a
     * frame callback that ran twice for one frame would be a redraw the caller
     * did not ask for.
     */
    void postFrame(Runnable r);

    /** Cancel a pending frame callback. */
    void removeFrame(Runnable r);
}
