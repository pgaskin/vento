// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend.realvnc;

import android.content.Context;
import android.os.Handler;
import android.os.HandlerThread;
import android.util.Log;

import com.realvnc.vncviewer.jni.AuthkeyStoreAndroid;
import com.realvnc.vncviewer.jni.Bindings;
import com.realvnc.vncviewer.jni.ConfigurationBindings;

import java.io.File;
import java.io.IOException;
import java.util.Map;
import java.util.Set;

/**
 * The process-wide part of the RealVNC backend: loads the library, starts the
 * one thread allowed to call into it, and initialises it, once.
 *
 * <p>The single thread is not a style choice. The core registers its socket on
 * whichever thread called {@code createSession} and delivers every callback
 * there, so that thread must run a {@code Looper} and must not be the main one;
 * and the core's own Android layer funnels every session call through one such
 * thread, created at {@code Application.onCreate} and never torn down, which is
 * the arrangement its locking was written against. The single documented
 * exception ({@code copyScaledRegion} called directly from the renderer) is in
 * {@link RealVncBackend}, not here.
 */
public final class RealVncCore {

    static final String TAG = "RealVnc";

    /** This app's namespace, not RealVNC's, which would pick up any policy file already on the device. */
    private static final String APP_NAME = "net.pgaskin.remotedesktop";

    /**
     * The core's own {@code Log} parameter. {@link Bindings.LogMode#LOG_DEBUG}
     * sets it to {@code *:android:110}: every category, to logcat, down to the
     * core's level 110, which it writes at VERBOSE. Three categories write a
     * line there per event: {@code CKeyboard} and {@code CConn} one per key
     * press and release, naming the keysym (the typed key, which no log should
     * hold), and {@code LineSpeedEstimator} one per estimate, a run of them
     * with every burst of updates. Those three are held at level 100, which
     * drops their VERBOSE lines: what each writes at DEBUG and above ({@code
     * CConn}'s "sending client cut text", any warning or error) still reaches
     * logcat, and every other category is unchanged. The parameters given to
     * {@code initApp} are set after the log mode's, at the same priority, so
     * this one takes effect. The Linux session's {@code CORE_LOG} is the same
     * string.
     */
    private static final String CORE_LOG =
            "*:android:110,CKeyboard:android:100,CConn:android:100,LineSpeedEstimator:android:100";

    private static Handler handler;
    private static boolean initialised;

    private RealVncCore() {
    }

    /**
     * The core's own directory: the path {@link #start} gives {@code initApp},
     * with the app name under it. The core creates it and manages its contents.
     *
     * <p>Not private because forgetting a server means deleting something from
     * it, and only this class knows where it is.
     */
    static File dataDir(Context context) {
        return new File(context.getApplicationContext().getFilesDir(), APP_NAME);
    }

    /**
     * Idempotent. Returns the handler for the session thread, which is also
     * what {@link #post} uses.
     */
    public static synchronized Handler start(Context context) {
        if (initialised) {
            return handler;
        }
        final Context app = context.getApplicationContext();

        // RealVNC's library, from the add-on, by absolute path. This call sets
        // the classloader the core's own lookups resolve against, so it has to
        // be made from a class in the add-on, as this one is.
        try {
            RealVncLibrary.load(app);
        } catch (IOException e) {
            throw new IllegalStateException(e.getMessage(), e);
        }
        // No client certificates: nothing here enrols one, so the core's
        // identity authentication has an empty list to choose from and falls
        // back to asking for a password.
        AuthkeyStoreAndroid.configure(app, Set.of());

        final HandlerThread thread = new HandlerThread("VncCore");
        thread.start();
        handler = new Handler(thread.getLooper());

        // Blocks here rather than posting without waiting: everything else
        // this class does is illegal until initApp has returned, and the caller
        // is on the main thread at startup, where a few ms is not noticeable.
        try {
            runBlocking(() -> {
                final File logDir = new File(app.getFilesDir(), "vnclog");
                //noinspection ResultOfMethodCallIgnored
                logDir.mkdirs();

                final Map<String, String> global = Map.of(
                        // The app draws the cursor, at its own scale, over a
                        // desktop the viewport may be magnifying. The control
                        // stack depends on that, so it is not offered as a
                        // setting as the other backends' cursor rows are.
                        "UseLocalCursor", "true",
                        "Locale", "en_US",
                        "SctpMaxMtu", "1200",
                        "Log", CORE_LOG);

                Bindings.initApp(
                        APP_NAME,
                        "1",
                        app.getFilesDir().getAbsolutePath(),
                        logDir.getAbsolutePath(),
                        "vncviewer",
                        app,
                        global,
                        true,
                        Bindings.LogMode.LOG_DEBUG);

                // The second part of the bootstrap. Everything it takes concerns
                // RealVNC accounts, which this app does not use; the bindings
                // answer all of it themselves, so there is nothing to pass.
                Bindings.initViewer();

                RealVncPrompts.registerFactories();
                Log.i(TAG, "core initialised on " + Thread.currentThread().getName());
            });
        } catch (Throwable t) {
            // A failed bootstrap leaves nothing worth keeping: the thread is
            // stopped, so the next attempt starts from scratch rather than
            // calling initApp again on a half set up looper.
            thread.quitSafely();
            handler = null;
            throw t;
        }
        // Last, so that the next caller is not told the core is ready after
        // its bootstrap threw: everything else in this class is illegal until
        // initApp has returned, and this flag is the only record that it has.
        initialised = true;
        return handler;
    }

    /** Runs on the session thread, inline if already on it. */
    static void post(Runnable r) {
        if (Thread.currentThread() == handler.getLooper().getThread()) {
            r.run();
        } else {
            handler.post(r);
        }
    }

    /**
     * Queues {@code r} to run in a later turn of the session thread's loop,
     * never inline, even when the caller is already on that thread (unlike
     * {@link #post}). For work that must not run inside the callback chain
     * it is posted from.
     */
    static void postToLoop(Runnable r) {
        handler.post(r);
    }

    static void postDelayed(Runnable r, long delayMs) {
        handler.postDelayed(r, delayMs);
    }

    static void cancel(Runnable r) {
        handler.removeCallbacks(r);
    }

    /**
     * An exception is rethrown to the caller rather than up this thread. There
     * is no handler above a {@code HandlerThread}, so one escaping there kills
     * the process, and the caller is the only place that can tell a screen what
     * happened.
     */
    static void runBlocking(Runnable r) {
        if (Thread.currentThread() == handler.getLooper().getThread()) {
            r.run();
            return;
        }
        final Object lock = new Object();
        final boolean[] done = {false};
        final Throwable[] threw = {null};
        handler.post(() -> {
            try {
                r.run();
            } catch (Throwable t) {
                threw[0] = t;
            } finally {
                synchronized (lock) {
                    done[0] = true;
                    lock.notifyAll();
                }
            }
        });
        final Throwable failed;
        synchronized (lock) {
            while (!done[0]) {
                try {
                    lock.wait();
                } catch (InterruptedException e) {
                    Thread.currentThread().interrupt();
                    return;
                }
            }
            failed = threw[0];
        }
        if (failed instanceof RuntimeException e) {
            throw e;
        } else if (failed instanceof Error x) {
            throw x;
        } else if (failed != null) {
            throw new RuntimeException(failed);
        }
    }

    /**
     * Sets a global parameter, as the original applies its "expert options"
     * string. Anything in the core's parameter registry can be set this way,
     * including parameters RealVNC's UI never exposes.
     */
    public static void setParameter(String name, String value) {
        post(() -> ConfigurationBindings.set(name, value, 10, false));
    }
}
