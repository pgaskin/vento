// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.playground;

import android.content.Context;
import android.os.Handler;
import android.os.Looper;
import android.util.Log;

import net.pgaskin.remotedesktop.control.input.Config;
import net.pgaskin.remotedesktop.control.input.TouchFrame;
import net.pgaskin.remotedesktop.control.input.TouchLog;
import net.pgaskin.remotedesktop.control.input.TouchRouter;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.OutputStreamWriter;
import java.nio.charset.StandardCharsets;

/**
 * Records raw touch streams to files, one per gesture, for replay in the JVM
 * unit tests ({@code FixtureReplayTest}). Armed from the {@code RECORD} toggle
 * square.
 *
 * <p>A file is closed once the screen has been empty for {@link #idleMs()},
 * <em>not</em> when the last finger lifts: a tap-then-drag is two touches with
 * a gap in the middle, and it is only a right-button drag because the second
 * one lands inside the 250 ms click window. Splitting there would record two
 * unrelated gestures and lose the thing being tested. So the boundary is "the
 * state machine is back to idle", which is the click window plus a margin.
 *
 * <pre>
 *   adb shell run-as net.pgaskin.remotedesktop ls files/touchlogs   # or:
 *   adb pull /sdcard/Android/data/net.pgaskin.remotedesktop/files/touchlogs
 * </pre>
 */
public final class TouchRecorder implements TouchRouter.Tap {

    private static final String TAG = "TouchRecorder";

    private final Context ctx;
    private final Config cfg;
    private final Handler handler = new Handler(Looper.getMainLooper());

    private boolean armed;
    private boolean relative;
    private TouchLog.Writer writer;
    private int written;
    private int viewW, viewH;
    private String lastPath = "";

    private final Runnable flushTask = this::flush;

    public TouchRecorder(Context ctx, Config cfg) {
        this.ctx = ctx;
        this.cfg = cfg;
    }

    /** How long the screen must stay empty before a recording is closed. */
    public long idleMs() {
        return cfg.clickHoldMs + 150;
    }

    public void setViewSize(int w, int h) {
        viewW = w;
        viewH = h;
    }

    /**
     * Which end owns the cursor. Recorded with the touches because it decides
     * what the same fingers do — a pinch pans only in the relative mode — so a
     * replay that did not know would produce a different answer to the one the
     * gesture was recorded to check.
     */
    public void setRelative(boolean on) {
        relative = on;
    }

    public boolean armed() {
        return armed;
    }

    public int count() {
        return written;
    }

    public String lastPath() {
        return lastPath;
    }

    /** Arm or disarm. Disarming writes out whatever has been recorded so far. */
    public void toggle() {
        handler.removeCallbacks(flushTask);
        if (armed) {
            flush();
        }
        armed = !armed;
        writer = null;
    }

    public String label() {
        return armed ? "REC " + written : (written > 0 ? "OFF " + written : "OFF");
    }

    @Override
    public void onFrame(TouchFrame f) {
        if (!armed) {
            return;
        }
        handler.removeCallbacks(flushTask);
        if (writer == null) {
            if (f.action != TouchFrame.Action.DOWN) {
                return; // wait for the start of a gesture
            }
            writer = new TouchLog.Writer()
                    .meta("label", "unlabelled")
                    .meta("density", Float.toString(cfg.density))
                    .meta("preset", cfg.faithfulPreset ? "faithful" : "improved")
                    .meta("view", viewW + " " + viewH)
                    .meta("pointer", relative ? "relative" : "absolute");
        }
        writer.add(f);

        final boolean screenEmpty = (f.action == TouchFrame.Action.UP && f.count <= 1)
                || f.action == TouchFrame.Action.CANCEL;
        if (screenEmpty) {
            handler.postDelayed(flushTask, idleMs());
        }
    }

    private void flush() {
        if (writer != null) {
            save(writer);
            writer = null;
        }
    }

    private void save(TouchLog.Writer w) {
        final File dir = new File(ctx.getExternalFilesDir(null), "touchlogs");
        if (!dir.isDirectory() && !dir.mkdirs()) {
            Log.e(TAG, "cannot create " + dir);
            return;
        }
        File out;
        int n = written;
        do {
            out = new File(dir, String.format("%03d.touch", ++n));
        } while (out.exists());

        try (OutputStreamWriter os = new OutputStreamWriter(
                new FileOutputStream(out), StandardCharsets.UTF_8)) {
            os.write(w.text());
        } catch (IOException e) {
            Log.e(TAG, "write " + out, e);
            return;
        }
        written = n;
        lastPath = out.getName() + " (" + w.frameCount() + " frames)";
        Log.i(TAG, "recorded " + out + " (" + w.frameCount() + " frames)");
    }
}
