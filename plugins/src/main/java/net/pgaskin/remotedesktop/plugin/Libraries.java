// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.plugin;

import android.content.Context;
import android.os.Bundle;
import android.util.Log;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;

/**
 * The add-on's libraries, in the app's process, ready to be loaded.
 *
 * <p>This runs as the *app* — it is the add-on's dex on the app's classloader —
 * so the add-on's own data directory is another uid and unreadable here. A copy
 * comes across the provider once and lives in the app's {@code filesDir}
 * thereafter, which is also the only place it can be {@code dlopen}ed from.
 *
 * <p>The copy is hashed as it is written and hashed again on every later start.
 * That is not belt and braces: the file that gets loaded is this one rather than
 * the one the add-on checked, so this one is what has to be right, and hashing
 * ten megabytes is nothing beside what happens next.
 */
public final class Libraries {

    private static final String TAG = "Libraries";

    private Libraries() {
    }

    private static File dir(Context app, String pluginPackage) {
        final File dir = new File(new File(app.getFilesDir(), "plugins"), pluginPackage);
        //noinspection ResultOfMethodCallIgnored
        dir.mkdirs();
        return dir;
    }

    /**
     * Whether {@link #load} would work without asking anybody: either this app
     * already holds good copies, or the add-on does.
     */
    public static boolean ready(Context app, String pluginPackage, Map<String, String> wanted) {
        if (present(app, pluginPackage, wanted)) {
            return true;
        }
        final Bundle ask = new Bundle();
        for (Map.Entry<String, String> e : wanted.entrySet()) {
            ask.putString(e.getKey(), e.getValue());
        }
        try {
            final Bundle answer = app.getContentResolver().call(
                    Plugin.libraries(pluginPackage), Plugin.CALL_READY, null, ask);
            return answer != null && answer.getBoolean(Plugin.EXTRA_READY);
        } catch (Exception e) {
            Log.w(TAG, "asking " + pluginPackage, e);
            return false;
        }
    }

    private static boolean present(Context app, String pluginPackage, Map<String, String> wanted) {
        final File dir = dir(app, pluginPackage);
        for (Map.Entry<String, String> e : wanted.entrySet()) {
            final File file = new File(dir, e.getKey());
            try {
                if (!file.isFile() || !e.getValue().equals(Hashes.of(file))) {
                    return false;
                }
            } catch (IOException x) {
                return false;
            }
        }
        return true;
    }

    /**
     * Every library in {@code wanted}, verified, in the order they were given —
     * which for a set of libraries that depend on each other is the order they
     * must be loaded in.
     */
    public static List<File> load(Context app, String pluginPackage, Map<String, String> wanted)
            throws IOException {
        final File dir = dir(app, pluginPackage);
        final List<File> files = new ArrayList<>(wanted.size());
        for (Map.Entry<String, String> e : wanted.entrySet()) {
            files.add(copy(app, pluginPackage, dir, e.getKey(), e.getValue()));
        }
        sweep(dir, wanted);
        return files;
    }

    /**
     * Anything here the pin no longer names, gone. A library that was wanted by
     * an older add-on is not wanted by this one, and leaving it is leaving an
     * unowned copy of somebody else's binary in the app's files for ever.
     */
    private static void sweep(File dir, Map<String, String> wanted) {
        final File[] files = dir.listFiles();
        if (files == null) {
            return;
        }
        for (File f : files) {
            if (!wanted.containsKey(f.getName())) {
                //noinspection ResultOfMethodCallIgnored
                f.delete();
            }
        }
    }

    private static File copy(Context app, String pluginPackage, File dir, String name,
                             String sha256) throws IOException {
        final File out = new File(dir, name);
        // Every start, not only the first: what is on disk is what gets loaded,
        // and a file left over from an older pin has the right name and the
        // wrong contents. A mismatch is fetched again rather than complained
        // about, since the add-on holds the copy that decides.
        if (out.isFile() && sha256.equals(Hashes.of(out))) {
            return out;
        }
        final File tmp = new File(out.getPath() + ".part");
        final String got;
        try (InputStream in = app.getContentResolver()
                .openInputStream(Plugin.library(pluginPackage, name))) {
            if (in == null) {
                throw new IOException("The plugin does not have this backend's library.");
            }
            try (FileOutputStream sink = new FileOutputStream(tmp)) {
                got = Hashes.copy(in, sink);
            }
        } catch (IOException | RuntimeException x) {
            //noinspection ResultOfMethodCallIgnored
            tmp.delete();
            throw x instanceof IOException io ? io : new IOException(x);
        }
        if (!sha256.equals(got)) {
            //noinspection ResultOfMethodCallIgnored
            tmp.delete();
            throw new IOException("The plugin's library is not the build this app was written for.");
        }
        // Read-only before it has the name anything looks for, and never
        // writable under that name: Android already refuses to load dex out of a
        // writable file, dlopen is the obvious next one, and a file that is
        // read-only from the moment it exists cannot be caught by that later.
        //noinspection ResultOfMethodCallIgnored
        tmp.setReadOnly();
        //noinspection ResultOfMethodCallIgnored
        out.delete();
        if (!tmp.renameTo(out)) {
            //noinspection ResultOfMethodCallIgnored
            tmp.delete();
            throw new IOException("The library could not be saved.");
        }
        Log.i(TAG, "copied " + name + " from " + pluginPackage);
        return out;
    }
}
