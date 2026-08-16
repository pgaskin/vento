// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.plugin;

import android.net.Uri;

/**
 * What both sides of an add-on agree on.
 *
 * <p>Two processes and two uids: the add-on's own, where a library is acquired
 * and checked, and the app's, where the add-on's dex is loaded and the library
 * is used. Nothing crosses between them except a file descriptor and an intent,
 * and both are named here.
 */
public final class Plugin {

    /**
     * What the app starts to have an add-on set itself up. Answered by an
     * activity in the add-on's own package, so that its permissions and the
     * packages it can see are its own.
     */
    public static final String ACTION_SETUP = "net.pgaskin.remotedesktop.action.SETUP_PLUGIN";

    /**
     * The add-on's provider, as {@code <its package>.libraries}. Derived rather
     * than declared so that a debug build's suffix carries through: an authority
     * is unique across the device, and two builds of one add-on installed side
     * by side must not collide.
     */
    public static Uri libraries(String pluginPackage) {
        return new Uri.Builder().scheme("content").authority(pluginPackage + ".libraries").build();
    }

    /** One file it serves, by the name the pin gives it. */
    public static Uri library(String pluginPackage, String name) {
        return libraries(pluginPackage).buildUpon().appendPath(name).build();
    }

    /** {@link android.content.ContentProvider#call} on the above: has it got them all yet? */
    public static final String CALL_READY = "ready";

    public static final String EXTRA_READY = "ready";

    private Plugin() {
    }
}
