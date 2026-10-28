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
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.function.Consumer;

/**
 * Plugin state as the app presents it (what each plugin is waiting for or what
 * is wrong with it), and the calls into plugin code made at the user's request.
 *
 * <p>{@link Backends} decides which plugins exist; this class decides what to
 * say about them. It is separate from the home screen because four screens make
 * the same two calls ({@link #setup} and {@link #failed}), and a failure in any
 * of them has to end up as a card on the home screen, where the plugin can be
 * uninstalled.
 */
final class Plugins {

    private static final String TAG = "Plugins";

    /** The card type, which decides its message and what it offers to do. */
    enum Kind {
        /** Installed and loaded, but not set up yet. */
        SETUP,
        /** Built for another version of the app, or declares no backend. */
        INCOMPATIBLE,
        /** It threw, at discovery or the first time a screen used it. */
        FAILED,
        /** The installed plugins differ from the loaded ones; only a restart fixes this. */
        RESTART,
    }

    /**
     * One card. The package is null for {@link Kind#RESTART}, which concerns the
     * set of plugins rather than one of them. The backend is the one waiting to
     * be set up, for the setup button. The detail is the plugin's own failure
     * message, and the update URL is where a build of the plugin for this build
     * of the app can be downloaded, if there is one ({@link #updateUrl}).
     */
    record Card(Kind kind, String packageName, String backendId, String title, String message,
                String detail, String updateUrl) {
    }

    private Plugins() {
    }

    /**
     * The cards the home screen shows above the connections, recomputed each
     * time it resumes: any of them can change while the app is in the
     * background, usually by uninstalling from a card or by installing an
     * add-on. An add-on installed since is loaded first ({@link Backends#refresh}),
     * so it gets its setup card without a restart.
     *
     * <p>The result goes to a callback because asking a plugin whether it is set
     * up does not belong on the main thread ({@link Backends#isSetup}). The
     * callback runs on the main thread, before this returns if every answer is
     * already known.
     */
    static void cards(Context context, Consumer<List<Card>> then) {
        final boolean restart = Backends.refresh(context);
        final List<String> ids = new ArrayList<>();
        for (String id : Backends.ids()) {
            if (Backends.packageOf(id) != null) {
                ids.add(id);
            }
        }
        if (ids.isEmpty()) {
            then.accept(build(context, Map.of(), restart));
            return;
        }
        // Every answer arrives on the main thread, so the map and the counter
        // need no synchronization.
        final Map<String, List<String>> waiting = new LinkedHashMap<>();
        final int[] left = {ids.size()};
        for (String id : ids) {
            Backends.isSetup(context, id, ok -> {
                if (!ok) {
                    waiting.computeIfAbsent(Backends.packageOf(id), k -> new ArrayList<>()).add(id);
                }
                if (--left[0] == 0) {
                    then.accept(build(context, waiting, restart));
                }
            });
        }
    }

    /**
     * Builds the cards, given the backends that reported they are not set up,
     * and whether an add-on whose code this process has run was updated or
     * uninstalled since.
     */
    private static List<Card> build(Context context, Map<String, List<String>> waiting,
                                    boolean restart) {
        final PackageManager pm = context.getPackageManager();
        final Set<String> installed = Backends.installedPlugins(context);
        final List<Card> cards = new ArrayList<>();
        for (Backends.Plugin p : Backends.plugins()) {
            if (!installed.contains(p.packageName())) {
                continue; // uninstalled since, which `restart` covers
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
                // The only card with an update: the problem is the version, so a
                // build for this version of the app fixes it. Reinstalling the
                // same version does not fix a plugin that threw.
                case INCOMPATIBLE -> cards.add(new Card(Kind.INCOMPATIBLE, p.packageName(), null,
                        label, context.getString(R.string.plugin_incompatible), p.detail(),
                        updateUrl(context, p.packageName())));
                case FAILED -> cards.add(new Card(Kind.FAILED, p.packageName(), null,
                        label, context.getString(R.string.plugin_failed), p.detail(), null));
            }
        }
        if (restart) {
            cards.add(new Card(Kind.RESTART, null, null,
                    context.getString(R.string.plugin_restart_title),
                    context.getString(R.string.plugin_restart), null, null));
        }
        return cards;
    }

    /**
     * Where a build of an add-on for this build of the app can be downloaded, or
     * null if there is no trustworthy answer.
     *
     * <p>An add-on declares a URL template with a placeholder for the version
     * code, and the signing certificate of the APK there
     * ({@link Backends#PLUGIN_UPDATE_URL}). The placeholder is filled with this
     * app's version code, since the wanted build is the one matching the
     * installed app rather than the newest one.
     *
     * <p>The declared certificate has to be the one <em>this app</em> is signed
     * with: a file signed with anything else cannot install over the add-on it
     * is meant to replace, so the link would only lead to an installer error.
     * The add-on's own certificate is not checked here because an add-on signed
     * differently from the app is never listed
     * ({@link Backends#installedPlugins}).
     *
     * <p>This does not make the download safe; the installer checks the file
     * itself against the same key. It only keeps the app from offering a link
     * that cannot lead to an installable file, which for an unofficial build of
     * the app is every link an add-on could declare.
     *
     * <p>The installer also refuses a downgrade, so an add-on newer than the app
     * is offered nothing: the app is the one behind, and this card cannot fix
     * that. It is the less common case (an add-on is downloaded once and the app
     * updates itself), and it resolves once the app is updated.
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
            return null; // uninstalled while the cards were being built
        }
        if (template == null || !template.startsWith("https://")
                || !template.contains(Backends.PLUGIN_UPDATE_VERSION)) {
            return null;
        }
        return template.replace(Backends.PLUGIN_UPDATE_VERSION, Long.toString(version));
    }

    /**
     * Whether this app is signed with the certificate whose SHA-256 is written
     * out, in the format {@code apksigner verify --print-certs} prints. False
     * for a missing or malformed fingerprint.
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
        // The current APK contents signer rather than the signing lineage, since
        // the download has to be signed the same way. A single fingerprint
        // cannot describe multiple signers, so those are rejected.
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
     * Start a plugin's setup, and return. There is no callback: this starts a
     * screen in another process, and this process can be killed in the
     * meantime, so the caller asks {@link Backends#isSetup} again when it
     * resumes.
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
     * <p>A loaded plugin stays loaded for the life of a process (open dex files
     * and shared libraries cannot be swapped under a live session), so this is
     * the only way to pick up an update or removal of one. Sessions are
     * disconnected first: a server that is disconnected cleanly knows at once,
     * while one whose socket vanishes waits for a timeout. While a session is
     * connected or connecting, this asks first; the card stays if the user
     * declines.
     */
    static void restart(Activity host) {
        if (Sessions.alive().isEmpty()) {
            exit(host);
            return;
        }
        new MaterialAlertDialogBuilder(host, R.style.ThemeOverlay_RemoteDesktop_Dialog)
                .setTitle(R.string.plugin_restart_confirm)
                .setMessage(R.string.plugin_restart_sessions)
                .setNegativeButton(android.R.string.cancel, null)
                .setPositiveButton(R.string.plugin_restart_action, (d, w) -> exit(host))
                .show();
    }

    private static void exit(Activity host) {
        for (Session s : Sessions.all()) {
            s.disconnect();
        }
        host.startActivity(new Intent(host, HomeActivity.class)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_CLEAR_TASK));
        host.finish();
        Runtime.getRuntime().exit(0);
    }

    /**
     * Plugin code threw when this screen called it: show it once, and record it
     * so the home screen shows a card with an uninstall button. The message is
     * the plugin's own if it has one, since some plugin errors (such as a
     * library that is not the expected build) reach the user only this way.
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

    /**
     * Where the app keeps its copies of add-on libraries.
     *
     * <p>Written by {@code net.pgaskin.remotedesktop.plugin.Libraries}, which
     * runs in this process but is in a module the app does not compile against
     * (it is loaded from the add-on's dex). So the directory is named in two
     * places, and this is the second. A copy is verified against the add-on's
     * before it is loaded, so if the two drift apart, the button deletes nothing
     * rather than the wrong thing.
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
     * Delete every copy, and return how many files were deleted.
     *
     * <p>Libraries already loaded stay mapped after their files are deleted, so
     * a live session is not a reason to refuse this. The next load of an
     * add-on's backend copies the library again, which makes this a cache: it
     * is cleared to reclaim space, or to make an add-on that has been set up
     * again hand over its new files.
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
            // The package's directory too, so an add-on uninstalled long ago
            // leaves nothing behind.
            //noinspection ResultOfMethodCallIgnored
            dir.delete();
        }
        // Every cached "set up" answer referred to the files just deleted, so
        // the add-on has to be asked again. It usually is still set up, since it
        // has its own copy to hand over; if not, this puts the card back on the
        // home screen instead of leaving a session to fail.
        Backends.setupChanged();
        return gone;
    }

    /**
     * Open the download link in the browser: the browser fetches the APK and the
     * system installer handles it, so the app never holds the file or needs the
     * install permission.
     *
     * <p>As with the source link in the settings, the exception is caught
     * instead of resolving the intent first, because resolving it needs a
     * package-visibility declaration.
     */
    static void update(Activity host, String url) {
        try {
            host.startActivity(new Intent(Intent.ACTION_VIEW, Uri.parse(url)));
        } catch (ActivityNotFoundException e) {
            Log.w(TAG, "no browser for " + url, e);
        }
    }

    /** Uses the system uninstall confirmation, so no permission is needed. */
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
