// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.input;

import android.os.Handler;
import android.os.Looper;
import android.view.Choreographer;

import java.util.HashMap;
import java.util.Map;

/** The real {@link Scheduler}: main-thread {@link Handler} + {@link Choreographer}. */
public final class AndroidScheduler implements Scheduler {

    private final Handler handler = new Handler(Looper.getMainLooper());
    private final Choreographer choreographer = Choreographer.getInstance();
    private final Map<Runnable, Choreographer.FrameCallback> frames = new HashMap<>();

    @Override
    public void postDelayed(Runnable r, long delayMs) {
        handler.postDelayed(r, delayMs);
    }

    @Override
    public void removeCallbacks(Runnable r) {
        handler.removeCallbacks(r);
    }

    @Override
    public void postFrame(Runnable r) {
        Choreographer.FrameCallback cb = frames.get(r);
        if (cb != null) {
            // Already queued for the next frame. Posting the same callback
            // again would run it twice, which is not what "post this for the
            // next frame" can be taken to mean.
            return;
        }
        cb = frameTimeNanos -> {
            frames.remove(r);
            r.run();
        };
        frames.put(r, cb);
        choreographer.postFrameCallback(cb);
    }

    @Override
    public void removeFrame(Runnable r) {
        final Choreographer.FrameCallback cb = frames.remove(r);
        if (cb != null) {
            choreographer.removeFrameCallback(cb);
        }
    }
}
