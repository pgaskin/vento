// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.plugin;

import android.content.Context;

import java.io.File;
import java.io.IOException;
import java.io.InputStream;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.zip.ZipEntry;
import java.util.zip.ZipFile;

/**
 * Taking pinned libraries out of somebody else's APK.
 *
 * <p>Both routes an add-on offers end here, with an archive and a list of what
 * has to be in it. A name is looked for at {@code lib/<abi>/<name>} and kept
 * only if it hashes to what was asked for, so where the archive came from — a
 * download, or a file somebody picked — decides nothing.
 */
public final class Apks {

    private Apks() {
    }

    /**
     * Reads every wanted library out of the archives, in order, and stores the
     * ones that verify.
     *
     * <p>More than one archive because an installer may have split an app by
     * ABI, in which case the libraries are in a split and not in the base.
     *
     * @return what is still missing afterwards, empty if it all arrived
     */
    public static Map<String, String> take(Context context, List<File> apks, String abi,
                                           Map<String, String> wanted) throws IOException {
        final Map<String, String> missing = new LinkedHashMap<>(wanted);
        IOException last = null;
        for (File apk : apks) {
            if (missing.isEmpty()) {
                break;
            }
            final ZipFile zip;
            try {
                zip = new ZipFile(apk);
            } catch (IOException x) {
                last = x; // a split that is not a zip is not the end of it
                continue;
            }
            // Whereas a library that is there and does not verify is: it is
            // the archive answering the question, and the entry after it is
            // out of the same archive. Its message is also the only useful
            // thing anybody is going to be told.
            try (zip) {
                for (Map.Entry<String, String> e : new ArrayList<>(missing.entrySet())) {
                    final ZipEntry entry = zip.getEntry("lib/" + abi + "/" + e.getKey());
                    if (entry == null) {
                        continue;
                    }
                    try (InputStream in = zip.getInputStream(entry)) {
                        LibraryStore.store(context, e.getKey(), e.getValue(), in);
                    }
                    missing.remove(e.getKey());
                }
            }
        }
        if (!missing.isEmpty() && last != null) {
            throw last;
        }
        return missing;
    }

}
