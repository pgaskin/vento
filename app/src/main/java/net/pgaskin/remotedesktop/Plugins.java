// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.app.Activity;
import android.content.ActivityNotFoundException;
import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.content.pm.Signature;
import android.content.pm.SigningInfo;
import android.net.Uri;
import android.os.Bundle;
import android.util.Log;

import com.google.android.material.dialog.MaterialAlertDialogBuilder;

import net.pgaskin.remotedesktop.backend.Backends;

import java.io.File;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.ArrayList;
import java.util.HexFormat;
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
     * be set up, for the button that asks; the detail is whatever the plugin's
     * own failure said; and the update is where a build of it for this build of
     * the app can be got, where there is one to be had ({@link #updateUrl}).
     */
    record Card(Kind kind, String packageName, String backendId, String title, String message,
                String detail, String updateUrl) {
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
                                        String.join(", ", names)), null, null));
                    }
                }
                // The one card an update is offered on: what is wrong with this
                // package is its version and nothing else, so a build of it for
                // this one is the whole of the fix. A plugin that threw is not
                // fixed by reinstalling the same version of it.
                case INCOMPATIBLE -> cards.add(new Card(Kind.INCOMPATIBLE, p.packageName(), null,
                        label, context.getString(R.string.plugin_incompatible), p.detail(),
                        updateUrl(context, p.packageName())));
                case FAILED -> cards.add(new Card(Kind.FAILED, p.packageName(), null,
                        label, context.getString(R.string.plugin_failed), p.detail(), null));
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
                    context.getString(R.string.plugin_restart), null, null));
        }
        return cards;
    }

    // ---- the build of an add-on this build of the app can use ---------------

    /**
     * Where a build of an add-on for this build of the app can be got, or null
     * where the question has no answer that can be trusted.
     *
     * <p>An add-on names a URL with a version code missing from it and says what
     * the APK there is signed with ({@link Backends#PLUGIN_UPDATE_URL}); the
     * version code filled in is this app's, since what is wanted is the build
     * that goes with what is installed rather than the newest of anything.
     *
     * <p>The add-on being <em>able</em> to say this is not the app believing it.
     * What it says the download is signed with has to be the key <em>this
     * app</em> is signed with, which is the only version of that question worth
     * asking: a file signed with anything else cannot install over the add-on it
     * is meant to replace, so sending somebody to it spends a download to arrive
     * at the installer's flattest refusal. The add-on's own key is not asked
     * about, being already answered — a package signed differently from the app
     * is not in the list at all ({@link Backends#installedPlugins}).
     *
     * <p>Nothing here makes the download safe. That is the installer's job and
     * it does it with the same key, on the file itself rather than on a claim
     * about it. What this does is keep the app from offering a link it has no
     * reason to expect a usable file at the end of — which, for a build signed
     * by somebody who did not sign what the add-on points at, is every link an
     * add-on could name.
     *
     * <p>The other file the installer would refuse is an older one, so an add-on
     * ahead of the app is offered nothing: what is behind there is the app, and
     * it is not this card's to fix. That is the uncommon direction — an add-on
     * is a file somebody fetched once and the app updates itself — and it is the
     * one where the phone has a working pair as soon as the app catches up.
     */
    private static String updateUrl(Context context, String packageName) {
        final PackageManager pm = context.getPackageManager();
        final String template;
        final long version;
        try {
            final Bundle meta = pm.getApplicationInfo(packageName,
                    PackageManager.ApplicationInfoFlags.of(PackageManager.GET_META_DATA)).metaData;
            if (meta == null) {
                return null;
            }
            if (!signedWith(context, meta.getString(Backends.PLUGIN_UPDATE_SIGNATURE))) {
                return null;
            }
            template = meta.getString(Backends.PLUGIN_UPDATE_URL);
            version = pm.getPackageInfo(context.getPackageName(),
                    PackageManager.PackageInfoFlags.of(0)).getLongVersionCode();
            if (pm.getPackageInfo(packageName,
                    PackageManager.PackageInfoFlags.of(0)).getLongVersionCode() > version) {
                return null;
            }
        } catch (PackageManager.NameNotFoundException e) {
            return null; // uninstalled while the home screen was working it out
        }
        if (template == null || !template.startsWith("https://")
                || !template.contains(Backends.PLUGIN_UPDATE_VERSION)) {
            return null;
        }
        return template.replace(Backends.PLUGIN_UPDATE_VERSION, Long.toString(version));
    }

    /**
     * Whether this app is signed with the certificate whose SHA-256 is written
     * out, as {@code apksigner verify --print-certs} prints it. False for a
     * fingerprint that is missing or is not one.
     */
    private static boolean signedWith(Context context, String sha256) {
        if (sha256 == null || sha256.isEmpty()) {
            return false;
        }
        final SigningInfo signing;
        try {
            signing = context.getPackageManager().getPackageInfo(context.getPackageName(),
                    PackageManager.PackageInfoFlags.of(PackageManager.GET_SIGNING_CERTIFICATES))
                    .signingInfo;
        } catch (PackageManager.NameNotFoundException e) {
            throw new IllegalStateException(e); // we are installed
        }
        // What the APK's contents are signed with rather than anything a lineage
        // remembers, since it is a file signed the same way that has to come out
        // of the download; and one signer, because a fingerprint is an answer to
        // "who signed this" and a set of them is not.
        if (signing == null || signing.hasMultipleSigners()) {
            return false;
        }
        final MessageDigest digest;
        try {
            digest = MessageDigest.getInstance("SHA-256");
        } catch (NoSuchAlgorithmException e) {
            throw new IllegalStateException(e); // every Android has it
        }
        for (Signature s : signing.getApkContentsSigners()) {
            if (sha256.equalsIgnoreCase(
                    HexFormat.of().formatHex(digest.digest(s.toByteArray())))) {
                return true;
            }
        }
        return false;
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

    /**
     * Hand the download to whatever opens links, and have nothing more to do
     * with it: the browser fetches the APK, the system's own installer asks
     * about it, and the app neither holds the file nor asks to install anything.
     *
     * <p>Caught rather than guarded, as the source link in the settings is: a
     * phone with nothing to open an https link with cannot be asked about one
     * either, since resolving it needs a package-visibility declaration to get
     * an answer at all.
     */
    static void update(Activity host, String url) {
        try {
            host.startActivity(new Intent(Intent.ACTION_VIEW, Uri.parse(url)));
        } catch (ActivityNotFoundException e) {
            Log.w(TAG, "no browser for " + url, e);
        }
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
