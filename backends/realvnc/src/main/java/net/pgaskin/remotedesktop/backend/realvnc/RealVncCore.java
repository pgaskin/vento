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
 * The process-wide half of the RealVNC backend: load the library, start the one
 * thread that is allowed to talk to it, and initialise it. Once.
 *
 * <p>The single thread is not a style choice. The core registers its socket on
 * whichever thread called {@code createSession} and delivers every callback
 * there, so that thread must run a {@code Looper} and must not be the main one;
 * and the core's own Android layer funnels every session call through one such
 * thread, created at {@code Application.onCreate} and never torn down, which is
 * the arrangement its locking was written against. The single documented
 * exception — {@code copyScaledRegion} straight from the renderer — is in
 * {@link RealVncBackend}, not here.
 */
public final class RealVncCore {

    static final String TAG = "RealVnc";

    /** Our own namespace, not theirs: theirs would inherit any policy file already on the device. */
    private static final String APP_NAME = "net.pgaskin.remotedesktop";

    private static Handler handler;
    private static boolean initialised;

    private RealVncCore() {
    }

    /**
     * The core's own directory, which is the path {@link #start} gives
     * {@code initApp} with the app name under it — the core makes it, and what
     * it puts there is its business rather than this backend's.
     *
     * <p>Public here because forgetting a server means deleting something out of
     * it, and nothing else can work out where it is.
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

        // Their library, out of the add-on, by absolute path. This call is what
        // sets the classloader the core's own lookups are answered from, so it
        // has to be made from a class of the add-on's — which this is.
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

        // Blocking here rather than posting-and-forgetting: everything else
        // this class exists for is illegal until initApp has returned, and the
        // caller is on the main thread at startup where a few ms is invisible.
        try {
            runBlocking(() -> {
                final File logDir = new File(app.getFilesDir(), "vnclog");
                //noinspection ResultOfMethodCallIgnored
                logDir.mkdirs();

                final Map<String, String> global = Map.of(
                        // The cursor is ours to draw, at our own scale, over a
                        // desktop the viewport may be magnifying — which is the
                        // whole premise of the control stack, and so is not
                        // offered as a setting the way the other backends'
                        // cursor rows are.
                        "UseLocalCursor", "true",
                        "Locale", "en_US",
                        "SctpMaxMtu", "1200");

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

                // The other half of the bootstrap. Everything it wants is about
                // RealVNC accounts, which we do not use; the bindings answer all
                // of it themselves, so there is nothing to pass and nothing to
                // ignore.
                Bindings.initViewer();

                RealVncPrompts.registerFactories();
                Log.i(TAG, "core initialised on " + Thread.currentThread().getName());
            });
        } catch (Throwable t) {
            // A bootstrap that failed leaves nothing worth keeping: the thread
            // goes, so the next attempt is a whole one rather than a second
            // initApp on a looper that is half set up.
            thread.quitSafely();
            handler = null;
            throw t;
        }
        // Last, so that a bootstrap which threw is not a core the next caller
        // is told is ready: everything below is illegal until initApp has
        // returned, and this flag is the whole of what says it has.
        initialised = true;
        return handler;
    }

    /** Run on the session thread, whether or not we are already on it. */
    static void post(Runnable r) {
        if (Thread.currentThread() == handler.getLooper().getThread()) {
            r.run();
        } else {
            handler.post(r);
        }
    }

    static void postDelayed(Runnable r, long delayMs) {
        handler.postDelayed(r, delayMs);
    }

    static void cancel(Runnable r) {
        handler.removeCallbacks(r);
    }

    /**
     * A throw comes back to the caller rather than up this thread. There is no
     * handler above a {@code HandlerThread}, so one let out here is the process
     * — and the caller is the one place that can tell a screen what happened.
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
     * A global parameter, applied the way the original applies its "expert
     * options" string. Anything in the core's parameter registry is reachable
     * this way, including parameters their UI never exposes.
     */
    public static void setParameter(String name, String value) {
        post(() -> ConfigurationBindings.set(name, value, 10, false));
    }
}
