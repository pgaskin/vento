// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.content.Context;
import android.content.SharedPreferences;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.os.Handler;
import android.os.Looper;
import android.util.Log;


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
import java.util.concurrent.ConcurrentHashMap;
import java.util.function.Consumer;

/**
 * The saved connections, and their desktop previews.
 *
 * <p>One JSON array in one preference file, rewritten whole on every change.
 * The list is a few dozen entries at most, so nothing more elaborate is needed.
 *
 * <p>Resolving a connection's options is {@link Options}' job: this holds the
 * record, and turning it into the map a backend is created with involves every
 * settings layer, not just this file.
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
        // Stable, so pinning one card does not reshuffle the rest: the file's
        // order is the order connections were added, or the order set with
        // reorder, and the user expects it to stay that way.
        //
        // Every write goes through this list, so the file is written back in
        // this order. That decides where pinning moves a connection: a newly
        // pinned one is behind every pinned one in the file and so lands at the
        // end of them, and a newly unpinned one is ahead of every unpinned one
        // and lands at their start. Both are where the user would look for it.
        out.sort((x, y) -> Boolean.compare(y.pinned(), x.pinned()));
        return out;
    }

    /**
     * Put these connections in this order, leaving every other one where it is.
     *
     * <p>The given ids are rearranged among the positions they already occupy,
     * so reordering the pinned ones cannot disturb the unpinned ones. The pinned
     * ones are a contiguous run at the front after {@link #all} sorts, but this
     * does not depend on it.
     *
     * <p>Ignored if the list has changed since (a connection deleted from
     * another window while the sheet was open), rather than applying half of
     * the new order.
     */
    public static void reorder(Context ctx, List<String> idsInOrder) {
        final List<Connection> list = all(ctx);
        final Map<String, Connection> moving = new LinkedHashMap<>();
        for (Connection c : list) {
            if (idsInOrder.contains(c.id())) {
                moving.put(c.id(), c);
            }
        }
        if (moving.size() != idsInOrder.size()) {
            Log.w(TAG, "not reordering: the list has changed underneath");
            return;
        }
        int next = 0;
        for (int i = 0; i < list.size(); i++) {
            if (moving.containsKey(list.get(i).id())) {
                list.set(i, moving.get(idsInOrder.get(next++)));
            }
        }
        write(ctx, list);
    }

    /** Pins or unpins the given connections in one write. */
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

    /** Adds the connection if new, or replaces the existing one in place. */
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
     * Adds several at the end in one write, for an import; adding one at a time
     * would rewrite the whole file once per connection.
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
            previewChanged(id);
        }
    }

    /**
     * Deletes every connection and its preview, and returns how many there were
     * so the settings screen can report it.
     *
     * <p>Not {@link #delete} over all the ids: this also removes previews of
     * connections no longer in the list, such as those deleted on an older
     * build. It clears the whole preview directory, not only the files the
     * records account for.
     */
    public static int deleteAll(Context ctx) {
        final int had = all(ctx).size();
        write(ctx, List.of());
        clearThumbnails(ctx);
        return had;
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
        // Every add, edit, pin, import and delete comes through here, so the
        // launcher shortcuts are updated here rather than from each caller:
        // they mirror the list, so every change to it is a change to them.
        Shortcuts.publish(ctx);
    }

    // ---- thumbnails ---------------------------------------------------------

    /**
     * The desktop preview on the home card. Written when a session goes away
     * and read when the list is drawn. One file per connection in the app's
     * files directory, because a bitmap does not belong in a preference file.
     *
     * <p>A PNG encrypted with {@link Secrets}, hence the extension. It shows
     * whatever was on that machine's screen when the session ended, is written
     * without the user asking, and is the most revealing thing this app puts
     * on disk, more so than the password.
     */
    public static File thumbnailFile(Context ctx, String id) {
        return new File(thumbnailDir(ctx), id + ".sealed");
    }

    /**
     * Where previews were kept before they were encrypted. Read once, so a card
     * does not go blank on upgrade, and then removed, since leaving a readable
     * copy would defeat the encryption.
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

    /**
     * Which version of a connection's preview is on disk, counted from when this
     * process started.
     *
     * <p>A cache of decoded previews compares against this. A preview costs a
     * keystore round trip and a PNG decode, so it is worth caching, and while
     * the list is not visible only a session ending can change a preview, for
     * <em>one</em> connection. Dropping the whole cache on every resume would
     * re-decode every card to update one of them.
     *
     * <p>Not a file timestamp: that is a stat call per card per bind, and what
     * matters is whether the cached picture is current, not when it was taken.
     */
    public static int previewVersion(String id) {
        return previewEpoch + previewVersions.getOrDefault(id, 0);
    }

    /** Invalidates every preview at once, for {@link #clearThumbnails}. */
    private static volatile int previewEpoch;
    private static final Map<String, Integer> previewVersions = new ConcurrentHashMap<>();

    private static void previewChanged(String id) {
        previewVersions.merge(id, 1, Integer::sum);
    }

    /**
     * The preview, decoded off the main thread and delivered on it.
     *
     * <p>On the same single thread as the writes, so the version requested is
     * the version returned without a lock: a session ending bumps the version
     * and queues its write before this screen resumes, so a read is queued
     * behind that write rather than racing it.
     */
    public static void readThumbnail(Context ctx, String id, Consumer<Bitmap> then) {
        final Context app = ctx.getApplicationContext();
        previewIo.execute(() -> {
            final Bitmap bmp = thumbnail(app, id);
            main.post(() -> then.accept(bmp));
        });
    }

    private static final Handler main = new Handler(Looper.getMainLooper());

    /** Blocking, and so private: callers use {@link #readThumbnail}. */
    private static Bitmap thumbnail(Context ctx, String id) {
        final File sealed = thumbnailFile(ctx, id);
        if (sealed.exists()) {
            try {
                final byte[] png = Secrets.openBytes(readAll(sealed));
                // Null means this device no longer has the key (a restored
                // backup or a reinstall). Not an error: the preview is saved
                // again the next time this connection is used.
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
     * Deletes every preview.
     *
     * <p>The preference only stops new previews being written; this deletes the
     * existing ones when it is turned off.
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
        previewEpoch++;
    }

    /**
     * Encodes, encrypts and writes the desktop's last frame.
     *
     * <p>On a worker thread, because the caller is {@code onPause} and this is a
     * PNG encode, a keystore round trip and a file write, in the callback the
     * system times the transition animation against. The single thread keeps
     * two sessions ending in quick succession from writing the same file at
     * once. The caller has to fetch the bitmap, since by the time this runs the
     * session may be gone.
     */
    public static void saveThumbnail(Context ctx, String id, Bitmap bmp) {
        if (bmp == null || !AppSettings.previews(ctx)) {
            return;
        }
        final Context app = ctx.getApplicationContext();
        // Before the write is queued rather than after it completes: a list
        // resuming behind this must see a version it has no picture for, and
        // then read after the write on the same thread.
        previewChanged(id);
        previewIo.execute(() -> writeThumbnail(app, id, bmp));
    }

    /**
     * One thread for the preview directory, for reads and writes. Single
     * because two sessions ending in quick succession must not write the same
     * file at once, and because a grid of cards decoding at the same moment
     * would be a burst of allocations for pictures drawn one after another.
     */
    private static final java.util.concurrent.ExecutorService previewIo =
            java.util.concurrent.Executors.newSingleThreadExecutor(r -> {
                final Thread t = new Thread(r, "previews");
                t.setDaemon(true);
                return t;
            });

    private static void writeThumbnail(Context ctx, String id, Bitmap bmp) {
        final ByteArrayOutputStream png = new ByteArrayOutputStream();
        bmp.compress(Bitmap.CompressFormat.PNG, 100, png);
        final byte[] sealed = Secrets.sealBytes(png.toByteArray());
        if (sealed == null) {
            // Nothing is written rather than an unencrypted PNG, the same rule
            // Secrets.protect follows for a password.
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
            return Streams.readAll(in);
        }
    }
}
