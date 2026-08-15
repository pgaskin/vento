// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.harness;

import net.pgaskin.remotedesktop.control.input.Scheduler;

import java.util.ArrayList;
import java.util.Collections;
import java.util.Comparator;
import java.util.List;

/**
 * A {@link Scheduler} on a virtual clock: nothing runs until the test advances
 * time, and then everything runs deterministically in due order.
 *
 * <p>This is what makes the 250 ms click window, the 100 ms bump-scroll repeat
 * and the 10 ms momentum ticks testable without sleeping.
 */
public final class FakeScheduler implements Scheduler {

    private static final class Task {
        final Runnable r;
        final long at;
        final boolean frame;
        final long seq;

        Task(Runnable r, long at, boolean frame, long seq) {
            this.r = r;
            this.at = at;
            this.frame = frame;
            this.seq = seq;
        }
    }

    /** Display frame period; frame callbacks land on the next multiple of it. */
    public long frameMs = 16;

    private final List<Task> tasks = new ArrayList<>();
    private long now;
    private long seq;

    public long now() {
        return now;
    }

    public int pending() {
        return tasks.size();
    }

    public void advance(long dt) {
        advanceTo(now + dt);
    }

    public void advanceTo(long t) {
        while (true) {
            final Task next = due(t);
            if (next == null) {
                break;
            }
            tasks.remove(next);
            now = next.at;
            next.r.run();
        }
        now = Math.max(now, t);
    }

    private Task due(long limit) {
        final List<Task> ready = new ArrayList<>();
        for (Task t : tasks) {
            if (t.at <= limit) {
                ready.add(t);
            }
        }
        if (ready.isEmpty()) {
            return null;
        }
        Collections.sort(ready, Comparator.comparingLong((Task t) -> t.at)
                .thenComparingLong(t -> t.seq));
        return ready.get(0);
    }

    @Override
    public void postDelayed(Runnable r, long delayMs) {
        tasks.add(new Task(r, now + delayMs, false, seq++));
    }

    @Override
    public void removeCallbacks(Runnable r) {
        tasks.removeIf(t -> !t.frame && t.r == r);
    }

    @Override
    public void postFrame(Runnable r) {
        for (Task t : tasks) {
            if (t.frame && t.r == r) {
                return; // one post per instance, as Scheduler says
            }
        }
        final long at = (now / frameMs + 1) * frameMs;
        tasks.add(new Task(r, at, true, seq++));
    }

    @Override
    public void removeFrame(Runnable r) {
        tasks.removeIf(t -> t.frame && t.r == r);
    }
}
