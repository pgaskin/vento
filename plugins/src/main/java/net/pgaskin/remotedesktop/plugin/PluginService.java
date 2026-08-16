// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.plugin;

import android.app.Service;
import android.content.Intent;
import android.os.IBinder;

/**
 * The marker the app discovers this package by. It is never started and never
 * bound: what matters is the intent filter in the manifest, which is what a
 * {@code <queries>} entry can match without either side naming the other's
 * package.
 */
public final class PluginService extends Service {

    @Override
    public IBinder onBind(Intent intent) {
        return null;
    }

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        stopSelf(startId);
        return START_NOT_STICKY;
    }
}
