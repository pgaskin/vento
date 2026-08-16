// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.realvnc;

import com.realvnc.vncviewer.jni.Library;

import net.pgaskin.remotedesktop.plugin.Download;

import java.util.List;

/**
 * Where a copy of RealVNC's viewer can be got, which is this add-on's own
 * business and nothing the app knows.
 *
 * <p>What is checked is never any of this: {@link Library} says which build the
 * declarations bind to and what each library in it hashes to, and that is what
 * decides whether a copy is kept. A mirror repacks, so an archive is a copy and
 * a library is a build.
 */
final class Viewer {

    /** Their own viewer, whose copy of the library is as good as any. */
    static final String PACKAGE = "com.realvnc.viewer.android";

    /** The release {@link Library} was read from, as the store knows it. */
    static final int VERSION_CODE = 2070175;

    private Viewer() {
    }

    /**
     * Tried in order. Aptoide answers an API for an exact version code, with
     * nothing to scrape; behind the second is a snapshot of the file that API
     * pointed at when this was written. APKMirror is left out on purpose: it
     * answers a script with 403, so it would cost a cookie jar and a page of
     * HTML.
     */
    static List<Download.Source> sources() {
        return List.of(
                new Download.Source(Download.Kind.APTOIDE,
                        "https://ws75.aptoide.com/api/7/app/getMeta"
                                + "?package_name=" + PACKAGE + "&vercode=" + VERSION_CODE),
                new Download.Source(Download.Kind.DIRECT,
                        "https://web.archive.org/web/20260811195414id_/"
                                + "https://pool.apk.aptoide.com/appupdater/"
                                + "com-realvnc-viewer-android-2070175-75463342-"
                                + "f98743311290dcd93f886016879cadfb.apk"));
    }
}
