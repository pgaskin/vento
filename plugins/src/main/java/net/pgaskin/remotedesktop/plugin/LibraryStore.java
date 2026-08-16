// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.plugin;

import android.content.Context;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.util.Map;

/**
 * What an add-on has acquired, in its own data directory.
 *
 * <p>Nothing outside its uid can read this — that is what {@link LibraryProvider}
 * is for — and nothing lands in it unverified: a file arrives under a temporary
 * name, is hashed as it is written, and is renamed into place only if it is the
 * build that was asked for. So a file being here at all is the statement that it
 * is the right one.
 */
public final class LibraryStore {

    private LibraryStore() {
    }

    public static File dir(Context context) {
        final File dir = new File(context.getFilesDir(), "libraries");
        //noinspection ResultOfMethodCallIgnored
        dir.mkdirs();
        return dir;
    }

    public static File file(Context context, String name) {
        return new File(dir(context), name);
    }

    /** Have we got this one, and is it still what it was? */
    public static boolean has(Context context, String name, String sha256) {
        final File file = file(context, name);
        try {
            return file.isFile() && sha256.equals(Hashes.of(file));
        } catch (IOException e) {
            return false;
        }
    }

    public static boolean has(Context context, Map<String, String> wanted) {
        for (Map.Entry<String, String> e : wanted.entrySet()) {
            if (!has(context, e.getKey(), e.getValue())) {
                return false;
            }
        }
        return true;
    }

    /**
     * What one library may be. The largest of the four RealVNC ships is 4.8 MB,
     * so this is an order of magnitude over anything real: it is not a limit
     * anybody is meant to reach, it is the difference between a bad archive
     * costing a refusal and it costing the phone's whole storage.
     */
    private static final long MAX_BYTES = 64L * 1024 * 1024;

    /**
     * Consumes the stream whatever happens; the file is there afterwards only if
     * it hashed to {@code sha256}.
     */
    public static void store(Context context, String name, String sha256, InputStream in)
            throws IOException {
        final File out = file(context, name);
        final File tmp = new File(out.getPath() + ".part");
        final String got;
        try (InputStream source = in; FileOutputStream sink = new FileOutputStream(tmp)) {
            got = Hashes.copy(source, sink, MAX_BYTES);
        } catch (IOException e) {
            //noinspection ResultOfMethodCallIgnored
            tmp.delete();
            throw e;
        }
        if (!sha256.equals(got)) {
            //noinspection ResultOfMethodCallIgnored
            tmp.delete();
            throw new IOException("That is not the build this plugin was written for.");
        }
        //noinspection ResultOfMethodCallIgnored
        out.delete();
        if (!tmp.renameTo(out)) {
            //noinspection ResultOfMethodCallIgnored
            tmp.delete();
            throw new IOException("The library could not be saved.");
        }
    }

    /** Forget everything, which is what an add-on's own reset does. */
    public static void clear(Context context) {
        final File[] files = dir(context).listFiles();
        if (files != null) {
            for (File f : files) {
                //noinspection ResultOfMethodCallIgnored
                f.delete();
            }
        }
    }
}
