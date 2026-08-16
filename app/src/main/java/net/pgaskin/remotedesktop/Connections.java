// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.content.Context;
import android.content.SharedPreferences;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.util.Log;

import net.pgaskin.remotedesktop.backend.BackendOption;
import net.pgaskin.remotedesktop.backend.Backends;

import org.json.JSONArray;
import org.json.JSONObject;

import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.util.ArrayList;
import java.util.Collection;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * The saved connections, and their desktop previews.
 *
 * <p>One JSON array in one preference file, rewritten whole on every change.
 * A list somebody scrolls through on a phone is a few dozen entries at the
 * outside, so anything cleverer would be paying for a problem nobody has.
 *
 * <p>The other half of this class is {@link #effectiveOptions}: the map a
 * backend is actually created with, which is the backend's defaults, then that
 * backend's global settings, then this connection's own. Three layers with the
 * most specific last, and the layering lives here rather than in the editor
 * because the session is what needs the answer.
 */
public final class Connections {

    private static final String TAG = "Connections";
    private static final String FILE = "connections";
    private static final String KEY = "list";

    private Connections() {
    }

    private static SharedPreferences prefs(Context ctx) {
        return ctx.getApplicationContext().getSharedPreferences(FILE, Context.MODE_PRIVATE);
    }

    /** In display order: pinned first, and each group in the order it was saved. */
    public static List<Connection> all(Context ctx) {
        final List<Connection> out = new ArrayList<>();
        try {
            final JSONArray a = new JSONArray(prefs(ctx).getString(KEY, "[]"));
            for (int i = 0; i < a.length(); i++) {
                final JSONObject o = a.optJSONObject(i);
                if (o != null) {
                    out.add(Connection.fromJson(o));
                }
            }
        } catch (Exception e) {
            Log.w(TAG, "reading connections", e);
        }
        // Stable, so pinning one card does not reshuffle the rest — the file's
        // order is the order things were added, and that is what somebody has
        // learned the shape of.
        out.sort((x, y) -> Boolean.compare(y.pinned(), x.pinned()));
        return out;
    }

    /** Pin or unpin several at once, which is what the selection acts on. */
    public static void setPinned(Context ctx, Collection<String> ids, boolean pinned) {
        final List<Connection> list = all(ctx);
        for (int i = 0; i < list.size(); i++) {
            if (ids.contains(list.get(i).id())) {
                list.set(i, list.get(i).withPinned(pinned));
            }
        }
        write(ctx, list);
    }

    public static Connection byId(Context ctx, String id) {
        for (Connection c : all(ctx)) {
            if (c.id().equals(id)) {
                return c;
            }
        }
        return null;
    }

    /** Add if new, replace in place if it is one we already have. */
    public static void save(Context ctx, Connection conn) {
        final List<Connection> list = all(ctx);
        boolean replaced = false;
        for (int i = 0; i < list.size(); i++) {
            if (list.get(i).id().equals(conn.id())) {
                list.set(i, conn);
                replaced = true;
                break;
            }
        }
        if (!replaced) {
            list.add(conn);
        }
        write(ctx, list);
    }

    /**
     * Add several at the end, in one write. An import is the only bulk add
     * there is, and doing it a record at a time would rewrite the whole file
     * once per connection.
     */
    public static void addAll(Context ctx, List<Connection> added) {
        if (added.isEmpty()) {
            return;
        }
        final List<Connection> list = all(ctx);
        list.addAll(added);
        write(ctx, list);
    }

    public static void delete(Context ctx, Collection<String> ids) {
        final List<Connection> list = all(ctx);
        list.removeIf(c -> ids.contains(c.id()));
        write(ctx, list);
        for (String id : ids) {
            for (File f : new File[]{thumbnailFile(ctx, id), legacyThumbnailFile(ctx, id)}) {
                if (f.exists() && !f.delete()) {
                    Log.w(TAG, "could not delete preview for " + id);
                }
            }
        }
    }

    private static void write(Context ctx, List<Connection> list) {
        final JSONArray a = new JSONArray();
        try {
            for (Connection c : list) {
                a.put(c.toJson());
            }
        } catch (Exception e) {
            Log.w(TAG, "writing connections", e);
            return;
        }
        prefs(ctx).edit().putString(KEY, a.toString()).apply();
        // Every add, edit, pin, import and delete comes through here, which is
        // why the launcher is told from here rather than from each of them: a
        // shortcut menu and a home screen icon are the list seen from outside
        // the app, and there is no such thing as a change to the list that they
        // are not a change to.
        Shortcuts.publish(ctx);
    }

    // ---- options -----------------------------------------------------------

    /**
     * What to hand {@link Backends#create}: schema defaults, then the backend's
     * global settings, then the connection's own overrides.
     *
     * <p>{@link BackendOption.Scope#GLOBAL} options are read from the backend's
     * preference file and {@link BackendOption.Scope#CONNECTION} ones from the
     * record, so a setting cannot be answered from the wrong place because
     * someone once edited it in the other screen. A
     * {@link BackendOption.Scope#LAYERED} one is read from both, the record
     * first.
     */
    public static Map<String, String> effectiveOptions(Context ctx, Connection conn) {
        final Map<String, String> out = new LinkedHashMap<>();
        final SharedPreferences global = backendPrefs(ctx, conn.backendId());
        for (BackendOption o : Backends.options(conn.backendId())) {
            String value = null;
            if (o.scope() != BackendOption.Scope.GLOBAL) {
                final String own = conn.options().get(o.key());
                if (own != null && !own.isEmpty()) {
                    value = own;
                }
            }
            if (value == null && o.scope() != BackendOption.Scope.CONNECTION) {
                value = global.getString(o.key(), null);
            }
            // Only what somebody has actually chosen — which is what a stored
            // value is, since both screens offer an explicit unanswered state
            // and store nothing for it. Filling in every default as well looks
            // harmless and is not: a backend then cannot tell "this connection
            // asked for full colour" from "nobody has ever said", and one that
            // wants to move a setting on another's behalf has no way to know
            // whether it may. That cost the RealVNC quality control, which
            // re-wrote ColorLevel at whatever it already was, so applyOptions
            // re-applied the same pixel format and the picture never changed.
            // Backends fill their own defaults in, which is where a default
            // belongs.
            if (value != null && !value.isEmpty()) {
                out.put(o.key(), value);
            }
        }
        return out;
    }

    /**
     * The backend's own half of the same thing, for a session that has no record
     * behind it — everything set in its settings screen, and nothing else.
     */
    public static Map<String, String> backendOptions(Context ctx, String backendId) {
        final Map<String, String> out = new LinkedHashMap<>();
        final SharedPreferences global = backendPrefs(ctx, backendId);
        for (BackendOption o : Backends.options(backendId)) {
            if (o.scope() == BackendOption.Scope.CONNECTION) {
                continue;
            }
            final String value = global.getString(o.key(), null);
            if (value != null && !value.isEmpty()) {
                out.put(o.key(), value);
            }
        }
        return out;
    }

    /** Where a backend's {@link BackendOption.Scope#GLOBAL} settings live. */
    public static SharedPreferences backendPrefs(Context ctx, String backendId) {
        return ctx.getApplicationContext()
                .getSharedPreferences("backend_" + backendId, Context.MODE_PRIVATE);
    }

    // ---- thumbnails ---------------------------------------------------------

    /**
     * The desktop preview on the home card. Written when a session goes away,
     * read when the list is drawn — one file per connection in the app's files
     * directory, because a bitmap does not belong in a preference file and the
     * one thing RealVNC's connection store was needed for was exactly this.
     *
     * <p>A PNG sealed with {@link Secrets}, not a PNG. It is a photograph of
     * whatever was on that machine's screen when the session ended, written by
     * us without anyone asking for it, and it is the most revealing thing this
     * app puts on disk — more so than the password beside it. The extension
     * says what it is.
     */
    public static File thumbnailFile(Context ctx, String id) {
        return new File(thumbnailDir(ctx), id + ".sealed");
    }

    /**
     * Where the previews were kept before they were sealed. Read once, so a
     * card does not go blank on upgrade, and then removed — leaving a readable
     * copy behind would make the sealing decorative.
     */
    private static File legacyThumbnailFile(Context ctx, String id) {
        return new File(thumbnailDir(ctx), id + ".png");
    }

    private static File thumbnailDir(Context ctx) {
        final File dir = new File(ctx.getApplicationContext().getFilesDir(), "thumbnails");
        //noinspection ResultOfMethodCallIgnored
        dir.mkdirs();
        return dir;
    }

    public static Bitmap thumbnail(Context ctx, String id) {
        final File sealed = thumbnailFile(ctx, id);
        if (sealed.exists()) {
            try {
                final byte[] png = Secrets.openBytes(readAll(sealed));
                // Null means this device has no key for it any more — a restored
                // backup or a reinstall. Not an error: the preview is redrawn
                // the next time this connection is used.
                return png == null ? null : BitmapFactory.decodeByteArray(png, 0, png.length);
            } catch (Exception e) {
                Log.w(TAG, "reading preview", e);
                return null;
            }
        }
        final File legacy = legacyThumbnailFile(ctx, id);
        if (!legacy.exists()) {
            return null;
        }
        final Bitmap bmp = BitmapFactory.decodeFile(legacy.getAbsolutePath());
        saveThumbnail(ctx, id, bmp);
        //noinspection ResultOfMethodCallIgnored
        legacy.delete();
        return bmp;
    }

    /**
     * Forget every preview there is.
     *
     * <p>What the preference turns off is the writing; this is what makes
     * turning it off mean something, since the pictures already taken are the
     * ones somebody is switching it off about.
     */
    public static void clearThumbnails(Context ctx) {
        final File[] files = thumbnailDir(ctx).listFiles();
        if (files == null) {
            return;
        }
        for (File f : files) {
            if (!f.delete()) {
                Log.w(TAG, "could not delete " + f.getName());
            }
        }
    }

    /**
     * Encode, seal and write the desktop's last screenful.
     *
     * <p>On a worker, because the caller is {@code onPause} and this is a PNG
     * encode, a keystore round trip and a file write — in the callback the
     * system is timing the transition animation with. The single thread is what
     * keeps two leaves in quick succession from writing the same file at once;
     * the bitmap has to be fetched by the caller, since by the time this runs
     * the session may be gone.
     */
    public static void saveThumbnail(Context ctx, String id, Bitmap bmp) {
        if (bmp == null || !AppSettings.previews(ctx)) {
            return;
        }
        final Context app = ctx.getApplicationContext();
        previewWriter.execute(() -> writeThumbnail(app, id, bmp));
    }

    private static final java.util.concurrent.ExecutorService previewWriter =
            java.util.concurrent.Executors.newSingleThreadExecutor(r -> {
                final Thread t = new Thread(r, "preview-writer");
                t.setDaemon(true);
                return t;
            });

    private static void writeThumbnail(Context ctx, String id, Bitmap bmp) {
        final ByteArrayOutputStream png = new ByteArrayOutputStream();
        bmp.compress(Bitmap.CompressFormat.PNG, 100, png);
        final byte[] sealed = Secrets.sealBytes(png.toByteArray());
        if (sealed == null) {
            // Nothing is written rather than a plain PNG as a consolation, the
            // same rule Secrets.protect follows for a password.
            Log.w(TAG, "no key for the preview; it will not be saved");
            return;
        }
        try (FileOutputStream out = new FileOutputStream(thumbnailFile(ctx, id))) {
            out.write(sealed);
        } catch (Exception e) {
            Log.w(TAG, "writing preview", e);
        }
    }

    private static byte[] readAll(File f) throws Exception {
        try (FileInputStream in = new FileInputStream(f)) {
            return in.readAllBytes();
        }
    }
}
