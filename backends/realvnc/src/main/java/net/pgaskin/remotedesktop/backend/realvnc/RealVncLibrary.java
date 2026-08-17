// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend.realvnc;

import android.annotation.SuppressLint;
import android.content.Context;

import com.realvnc.vncviewer.jni.Library;

import net.pgaskin.remotedesktop.backend.Backends;
import net.pgaskin.remotedesktop.plugin.Libraries;

import java.io.File;
import java.io.IOException;
import java.util.Map;

/**
 * Where {@code libvncviewer.so} comes from, which is nowhere in this build.
 *
 * <p>It is in no APK: the add-on this module ships inside acquires a copy on the
 * device, from RealVNC, and holds it in its own data directory. What runs here
 * runs as the app, so a copy crosses once and is loaded out of the app's own
 * files — verified against {@link Library} on the way, since a hash of somebody
 * else's binary is the whole of what makes loading it safe.
 */
final class RealVncLibrary {

    private RealVncLibrary() {
    }

    /**
     * The add-on this class came from. Null would mean these classes are in the
     * app itself, which is the arrangement this replaced.
     */
    static String pluginPackage() {
        final String pkg = Backends.packageOf(RealVncLibrary.class.getClassLoader());
        if (pkg == null) {
            throw new IllegalStateException("not loaded from an add-on");
        }
        return pkg;
    }

    static boolean isSetup(Context context) {
        final Map<String, String> wanted = Library.wanted();
        // Not "ready": an empty set of requirements is met by an empty store,
        // and a device with no pinned build is one that can never be set up
        // rather than one that already is.
        return !wanted.isEmpty() && Libraries.ready(context, pluginPackage(), wanted);
    }

    /** Both libraries, in order, from the paths that were just verified. */
    // loadLibrary cannot be what this uses: the library is not in the APK. It is
    // a file an add-on handed over, copied into this app's own storage and
    // checked against a hash this build carries before it is opened.
    @SuppressLint("UnsafeDynamicallyLoadedCode")
    static void load(Context app) throws IOException {
        final Map<String, String> wanted = Library.wanted();
        if (wanted.isEmpty()) {
            throw new IOException("There is no build of this backend's library for this phone.");
        }
        for (File file : Libraries.load(app, pluginPackage(), wanted)) {
            System.load(file.getAbsolutePath());
        }
    }
}
