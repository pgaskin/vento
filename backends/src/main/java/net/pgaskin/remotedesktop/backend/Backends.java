// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend;

import android.content.Context;

import java.util.ArrayList;
import java.util.Comparator;
import java.util.List;
import java.util.Map;
import java.util.ServiceLoader;

/**
 * Which backends this build has.
 *
 * <p>Nobody writes the list: it is every {@link BackendProvider} on the
 * classpath, which is every protocol module the build depends on. That is what
 * makes "does this APK contain RealVNC's core?" a question about one dependency
 * line rather than about a class somebody has to remember to edit — and it is
 * why the app has no per-flavour source at all.
 *
 * <p>An id this build does not have is not an error anywhere except
 * {@link #create}: a connection record naming one still has to draw on the home
 * screen and open in the editor, since the record outlives the build that made
 * it.
 */
public final class Backends {

    private static volatile List<BackendProvider> providers;

    private Backends() {
    }

    private static synchronized List<BackendProvider> providers() {
        List<BackendProvider> found = providers;
        if (found == null) {
            final List<BackendProvider> loaded = new ArrayList<>();
            // The classloader is named rather than defaulted: the thread
            // context one is the app's here, but it is not ours to assume.
            // (A minified build would need these kept — nothing here is
            // referenced by name.)
            for (BackendProvider p : ServiceLoader.load(
                    BackendProvider.class, Backends.class.getClassLoader())) {
                loaded.add(p);
            }
            loaded.sort(Comparator.comparingInt(BackendProvider::order)
                    .thenComparing(BackendProvider::id));
            found = List.copyOf(loaded);
            providers = found;
        }
        return found;
    }

    private static BackendProvider provider(String id) {
        for (BackendProvider p : providers()) {
            if (p.id().equals(id)) {
                return p;
            }
        }
        return null;
    }

    public static List<String> ids() {
        return providers().stream().map(BackendProvider::id).toList();
    }

    public static String name(String id) {
        final BackendProvider p = provider(id);
        return p == null ? id : p.name();
    }

    /**
     * Empty for a backend this build does not have — a card for one still has
     * to draw, and naming something that cannot be opened is worse than saying
     * nothing.
     */
    public static String shortName(String id) {
        final BackendProvider p = provider(id);
        return p == null ? "" : p.shortName();
    }

    /** Empty for a backend this build does not have, as {@link #shortName}. */
    public static String description(String id) {
        final BackendProvider p = provider(id);
        return p == null ? "" : p.description();
    }

    public static List<BackendOption> options(String id) {
        final BackendProvider p = provider(id);
        return p == null ? List.of() : p.options();
    }

    public static Backend create(Context context, String id, String address, String userName,
                                 String password, Map<String, String> options) {
        final BackendProvider p = provider(id);
        if (p == null) {
            throw new IllegalArgumentException(
                    "no such backend in this build: " + id + " (it has " + ids() + ")");
        }
        return p.create(context, address, userName, password, options);
    }
}
