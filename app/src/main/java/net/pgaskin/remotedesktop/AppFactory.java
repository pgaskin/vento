// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.annotation.SuppressLint;
import androidx.core.app.CoreComponentFactory;
import android.content.pm.ApplicationInfo;

import net.pgaskin.remotedesktop.backend.Backends;

/**
 * The one hook that runs before the app's classloader is settled, which is what
 * an add-on's classes have to be reachable from.
 *
 * <p>Nothing else about component instantiation is changed.
 *
 * <p>The base class is androidx-internal, and extending it is the point: androidx
 * declares this factory in its own manifest, so a factory of ours either
 * continues theirs or silently replaces it.
 */
@SuppressLint("RestrictedApi")
public final class AppFactory extends CoreComponentFactory {

    @Override
    public ClassLoader instantiateClassLoader(ClassLoader cl, ApplicationInfo aInfo) {
        return Backends.classLoader(cl);
    }
}
