// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend.rdp;

import android.content.Context;
import android.content.SharedPreferences;

import java.util.Locale;

/**
 * What each machine's desktop turned out to be, so that connecting again can ask
 * for the same thing.
 *
 * <p>RDP has no way to ask for "whatever it already is": the size is in the
 * client's connection data and something has to go there. So the nearest true
 * answer is the size the last session ended up at, which is what this
 * remembers — and it is the size the <em>server</em> reported rather than the
 * one that was asked for, since those differ (a server is free to round the
 * width, and does).
 *
 * <p>Keyed on the address as written, for {@code KnownHosts}' reason: the
 * desktop belongs to the machine and not to whichever saved record reached it.
 */
final class DesktopSizes {

    private static final String FILE = "rdp_desktop_sizes";

    private DesktopSizes() {
    }

    private static SharedPreferences prefs(Context ctx) {
        return ctx.getApplicationContext().getSharedPreferences(FILE, Context.MODE_PRIVATE);
    }

    private static String key(String address) {
        return address == null ? "" : address.trim().toLowerCase(Locale.ROOT);
    }

    /** {@code {width, height}}, or null if this address has never connected. */
    static int[] remembered(Context ctx, String address) {
        final String value = prefs(ctx).getString(key(address), null);
        return value == null ? null : RdpProvider.parse(value);
    }

    static void remember(Context ctx, String address, int width, int height) {
        if (width <= 0 || height <= 0) {
            return;
        }
        prefs(ctx).edit().putString(key(address), width + "x" + height).apply();
    }
}
