// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.app.Activity;
import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.net.Uri;
import android.util.Log;

import com.google.android.material.dialog.MaterialAlertDialogBuilder;

import net.pgaskin.remotedesktop.backend.Backends;

import java.io.File;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.function.Consumer;

/**
 * The plugins as the app has to deal with them: what each is waiting for or
 * wrong about, and the calls into somebody else's code that a person asks for.
 *
 * <p>Which plugins there are is {@link Backends}'s answer; what to say about one
 * is the app's, and it is here rather than on the home screen because four
 * screens make the same two calls and a failure in any of them has to end up in
 * the same place — a card on the home screen, which is where the thing itself
 * can be uninstalled.
 */
final class Plugins {

    private static final String TAG = "Plugins";

    /** What a card says, and so what it offers to do about it. */
    enum Kind {
        /** Installed and loaded, and waiting for something it has to be given. */
        SETUP,
        /** Not this build's companion, or it declares no backend. */
        INCOMPATIBLE,
        /** It threw, at discovery or the first time a screen used it. */
        FAILED,
        /** What is installed is not what is loaded, which only ending the process fixes. */
        RESTART,
    }

    /**
     * One card. The package is null for {@link Kind#RESTART}, which is about the
     * set rather than about any one of them; the backend is the one waiting to
     * be set up, for the button that asks; and the detail is whatever the
     * plugin's own failure said.
     */
    record Card(Kind kind, String packageName, String backendId, String title, String message,
                String detail) {
    }

    private Plugins() {
    }

    /**
     * What the home screen puts above the connections, worked out fresh each
     * time it resumes: every one of these can change while the app is in the
     * background, and uninstalling from a card is how one of them usually does.
     *
     * <p>The answer arrives rather than being returned, because asking a
     * plugin whether it is set up is not a question for the main thread
     * ({@link Backends#isSetup}). It arrives on that thread, and at once where
     * every answer is already known.
     */
    static void cards(Context context, Consumer<List<Card>> then) {
        final List<String> ids = new ArrayList<>();
        for (String id : Backends.ids()) {
            if (Backends.packageOf(id) != null) {
                ids.add(id);
            }
        }
        if (ids.isEmpty()) {
            then.accept(build(context, Map.of()));
            return;
        }
        // Counted on the main thread, which every answer comes back on, so
        // neither the map nor the counter needs anything of its own.
        final Map<String, List<String>> waiting = new LinkedHashMap<>();
        final int[] left = {ids.size()};
        for (String id : ids) {
            Backends.isSetup(context, id, ok -> {
                if (!ok) {
                    waiting.computeIfAbsent(Backends.packageOf(id), k -> new ArrayList<>()).add(id);
                }
                if (--left[0] == 0) {
                    then.accept(build(context, waiting));
                }
            });
        }
    }

    /** The cards themselves, given which backends said they are not set up. */
    private static List<Card> build(Context context, Map<String, List<String>> waiting) {
        final PackageManager pm = context.getPackageManager();
        final Set<String> installed = Backends.installedPlugins(context);
        final List<Card> cards = new ArrayList<>();
        final Set<String> known = new LinkedHashSet<>();
        boolean stale = false;
        for (Backends.Plugin p : Backends.plugins()) {
            known.add(p.packageName());
            if (!installed.contains(p.packageName())) {
                // Gone since the app started. Only one that was loaded leaves
                // anything behind: its dex and its backends are in this process
                // until the process ends.
                stale |= p.state() == Backends.PluginState.LOADED;
                continue;
            }
            final String label = label(pm, p.packageName());
            switch (p.state()) {
                case LOADED -> {
                    final List<String> ids = waiting.get(p.packageName());
                    if (ids != null) {
                        final List<String> names = ids.stream().map(Backends::name).toList();
                        cards.add(new Card(Kind.SETUP, p.packageName(), ids.get(0), label,
                                context.getString(R.string.plugin_needs_setup,
                                        String.join(", ", names)), null));
                    }
                }
                case INCOMPATIBLE -> cards.add(new Card(Kind.INCOMPATIBLE, p.packageName(), null,
                        label, context.getString(R.string.plugin_incompatible), p.detail()));
                case FAILED -> cards.add(new Card(Kind.FAILED, p.packageName(), null,
                        label, context.getString(R.string.plugin_failed), p.detail()));
            }
        }
        // One installed while the app was running, which is the ordinary first
        // run rather than a fault: nothing of it can be loaded until this
        // process ends, and there is nothing else to offer.
        for (String pkg : installed) {
            stale |= !known.contains(pkg);
        }
        if (stale) {
            cards.add(new Card(Kind.RESTART, null, null,
                    context.getString(R.string.plugin_restart_title),
                    context.getString(R.string.plugin_restart), null));
        }
        return cards;
    }

    /**
     * Ask a plugin for whatever it is waiting for, and return. There is no
     * callback: what this starts is a screen in another process, which can be
     * killed behind a dialog, so the only answer that survives is the caller
     * asking {@link Backends#isSetup} again when it resumes.
     */
    static void setup(Activity host, String id) {
        try {
            Backends.setup(host, id);
        } catch (Throwable t) {
            failed(host, id, t);
        }
    }

    /**
     * End this process and come back to an empty home screen.
     *
     * <p>The loaded set is fixed for the life of a process — dex and a shared
     * library that are open cannot be swapped underneath a live session — so
     * this is the only way to pick up a plugin that has arrived since. The
     * sessions are told rather than dropped: a far end that is disconnected
     * knows, where one whose socket vanishes waits for a timeout.
     */
    static void restart(Activity host) {
        for (Session s : Sessions.all()) {
            s.disconnect();
        }
        host.startActivity(new Intent(host, HomeActivity.class)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_CLEAR_TASK));
        host.finish();
        Runtime.getRuntime().exit(0);
    }

    /**
     * Somebody else's code threw where this screen called it: say so once, and
     * record it so the home screen grows a card with an uninstall on it. The
     * message is the plugin's own where it has one, since "not the build this
     * was written for" reaches a person through exactly this path.
     */
    static void failed(Activity host, String id, Throwable t) {
        Backends.failed(id, t);
        final String said = t.getMessage();
        new MaterialAlertDialogBuilder(host, R.style.ThemeOverlay_RemoteDesktop_Dialog)
                .setTitle(Backends.name(id))
                .setMessage(said == null || said.isEmpty()
                        ? host.getString(R.string.plugin_failed) : said)
                .setPositiveButton(android.R.string.ok, null)
                .show();
    }

    // ---- the libraries an add-on has handed over --------------------------

    /**
     * Where an add-on's libraries are kept once they are the app's copies.
     *
     * <p>Written by {@code net.pgaskin.remotedesktop.plugin.Libraries}, which
     * runs in this process as the app but is in a module the app does not
     * compile against — it comes across on the add-on's dex. So the directory is
     * named in two places, and this is the second: a copy is verified against
     * the add-on's before it is loaded either way, so the cost of the two
     * drifting apart is a button that deletes nothing rather than one that
     * deletes the wrong thing.
     */
    private static File libraryCache(Context ctx) {
        return new File(ctx.getApplicationContext().getFilesDir(), "plugins");
    }

    /** Whether there is anything for {@link #clearLibraries} to do. */
    static boolean hasLibraries(Context ctx) {
        final File[] dirs = libraryCache(ctx).listFiles();
        if (dirs == null) {
            return false;
        }
        for (File dir : dirs) {
            final File[] files = dir.listFiles();
            if (files == null || files.length > 0) {
                return true;
            }
        }
        return false;
    }

    /**
     * Forget every copy, and say how many went.
     *
     * <p>What is already loaded stays loaded — a library this process has opened
     * is mapped whether the file is still named or not, so a live session is not
     * a reason to refuse this. The next one to load a backend of an add-on's
     * copies it across again, which is what makes this a cache rather than a
     * store: it is deleted to reclaim the space, or to make an add-on that has
     * been set up again hand over what it now has.
     */
    static int clearLibraries(Context ctx) {
        final File[] dirs = libraryCache(ctx).listFiles();
        if (dirs == null) {
            return 0;
        }
        int gone = 0;
        for (File dir : dirs) {
            final File[] files = dir.listFiles();
            if (files != null) {
                for (File f : files) {
                    if (f.delete()) {
                        gone++;
                    } else {
                        Log.w(TAG, "could not delete " + f);
                    }
                }
            }
            // The package's own directory too, so that an add-on uninstalled
            // long ago leaves nothing behind with its name on it.
            //noinspection ResultOfMethodCallIgnored
            dir.delete();
        }
        // Every "this backend is ready" answer given so far was about the files
        // that have just gone. Whether it is still true is now the add-on's to
        // say again — it usually is, since it has its own copy to hand over,
        // and where it has not this is what puts the card back on the home
        // screen instead of leaving a session to fail.
        Backends.setupChanged();
        return gone;
    }

    /** The system's own confirmation, which is why this needs no permission. */
    static void uninstall(Activity host, String packageName) {
        host.startActivity(new Intent(Intent.ACTION_DELETE,
                Uri.fromParts("package", packageName, null)));
    }

    private static String label(PackageManager pm, String pkg) {
        try {
            return pm.getApplicationLabel(pm.getApplicationInfo(pkg, 0)).toString();
        } catch (PackageManager.NameNotFoundException e) {
            return pkg; // uninstalled between two calls
        }
    }
}
