// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.app.Application;

import com.google.android.material.color.DynamicColors;

import net.pgaskin.remotedesktop.backend.Backends;

/**
 * Applies Material You: every activity gets the system palette.
 *
 * <p>minSdk 31 is the API 31 minimum for
 * {@code android.R.color.system_accent1_*}, so there is no fallback path. The
 * session screen does not use the palette: it draws over a remote desktop,
 * where the phone's accent colour does not belong.
 */
public final class RemoteDesktopApp extends Application {

    @Override
    public void onCreate() {
        super.onCreate();
        DynamicColors.applyToActivitiesIfAvailable(this);

        // Before anything queries the available backends, and here rather than
        // in an activity because the service is an entry point too.
        Backends.discover(this);

        // The launcher's copy of the connection list, which is otherwise only
        // rewritten when the list is. It can also go stale while this app is
        // not running: a restored backup, or an upgrade from a version that
        // published none. Off the main thread, and cheap enough to do on every
        // start rather than tracking whether it is needed.
        Shortcuts.publish(this);
    }
}
