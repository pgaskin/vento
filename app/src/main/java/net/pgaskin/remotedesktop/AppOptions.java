// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.content.Context;

import net.pgaskin.remotedesktop.backend.BackendOption;

import java.util.List;
import java.util.Map;

/**
 * Per-connection settings the <em>app</em> owns.
 *
 * <p>A {@link BackendOption} is a backend describing itself, and these are not
 * that: they are questions about this phone that happen to want a different
 * answer per machine, which is the one thing the app's settings tree cannot
 * express. They ride in the same map as a backend's own options and are shown
 * by the same editor rows, so a person meets one kind of setting rather than
 * two — and the keys carry a prefix nothing else here uses, so a backend never
 * sees one.
 *
 * <p>The other half of the rule: nothing goes here that a backend could answer
 * for itself. Whether the far end <em>can</em> resize is a fact about a live
 * session, whether it may be <em>asked</em> to is a question about this phone,
 * and only the second one is here.
 */
public final class AppOptions {

    /** Namespaced, because these share a map with every backend's own keys. */
    public static final String FOLLOW_WINDOW = "app.FollowWindow";

    private AppOptions() {
    }

    public static List<BackendOption> options(Context ctx) {
        return List.of(BackendOption.bool(FOLLOW_WINDOW,
                ctx.getString(R.string.option_follow_window),
                ctx.getString(R.string.option_follow_window_summary),
                false, BackendOption.Scope.CONNECTION, false));
    }

    public static boolean followWindow(Map<String, String> options) {
        return Boolean.parseBoolean(options.getOrDefault(FOLLOW_WINDOW, "false"));
    }
}
