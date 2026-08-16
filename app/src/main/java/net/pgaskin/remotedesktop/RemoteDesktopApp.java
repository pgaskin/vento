// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.app.Application;

import com.google.android.material.color.DynamicColors;

import net.pgaskin.remotedesktop.backend.Backends;

/**
 * Material You, in one line: every activity gets the system palette.
 *
 * <p>minSdk 34 is well past the API 31 floor for
 * {@code android.R.color.system_accent1_*}, so there is no fallback path to
 * write. The session screen takes none of this — it draws
 * itself, over somebody else's desktop, where a phone's accent colour has no
 * business.
 */
public final class RemoteDesktopApp extends Application {

    @Override
    public void onCreate() {
        super.onCreate();
        DynamicColors.applyToActivitiesIfAvailable(this);

        // Before anything can ask what this app can connect with, and here
        // rather than in an activity because the service is an entry point too.
        Backends.discover(this);
    }
}
