// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.playground;

import android.content.Context;
import android.util.Log;

import java.io.File;

/**
 * Where the playground's two recorders write, and the one way to be rid of what
 * they have written.
 *
 * <p>The directories are named here rather than at each recorder because
 * something outside the playground has to be able to delete them: a fixture is
 * recorded to be pulled off the phone and committed, and what is left afterwards
 * is a folder of raw touch and key streams from whatever was being tested. Two
 * places name a directory and a third deletes it is how one of them gets missed.
 *
 * <p>External files, both of them — {@code adb pull} without {@code run-as} is
 * the whole point of a fixture — so a phone with no external storage has nowhere
 * to put them and every call here says so by doing nothing.
 */
public final class Recordings {

    private static final String TAG = "Recordings";

    /** Touch streams, one file per gesture: {@link TouchRecorder}. */
    static final String TOUCH = "touchlogs";

    /** Key events, one file per arming: {@link KeyTrace}. */
    static final String KEYS = "keytrace";

    private Recordings() {
    }

    /** The directory itself, not made; null where there is nowhere to make it. */
    static File dir(Context ctx, String name) {
        final File files = ctx.getExternalFilesDir(null);
        return files == null ? null : new File(files, name);
    }

    /**
     * Whether either recorder has left anything behind, for a host deciding
     * whether to offer the deletion at all. A directory listing rather than a
     * count, because the question is only ever "any": both directories are
     * empty on every phone that has not recorded, and a recorder makes its own
     * so neither need exist.
     */
    public static boolean any(Context ctx) {
        for (String name : new String[]{TOUCH, KEYS}) {
            final File dir = dir(ctx, name);
            final String[] files = dir == null ? null : dir.list();
            if (files != null && files.length > 0) {
                return true;
            }
        }
        return false;
    }

    /**
     * Delete every recording, and say how many went. The directories stay: a
     * recorder makes its own, and an empty one is not worth a special case.
     */
    public static int clear(Context ctx) {
        int gone = 0;
        for (String name : new String[]{TOUCH, KEYS}) {
            final File dir = dir(ctx, name);
            final File[] files = dir == null ? null : dir.listFiles();
            if (files == null) {
                continue;
            }
            for (File f : files) {
                if (f.delete()) {
                    gone++;
                } else {
                    Log.w(TAG, "could not delete " + f);
                }
            }
        }
        return gone;
    }
}
