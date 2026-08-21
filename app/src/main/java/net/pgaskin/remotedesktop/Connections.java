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
 * A list somebody scrolls through on a phone is a few dozen entries at the
 * outside, so anything cleverer would be paying for a problem nobody has.
 *
 * <p>What a connection's options <em>mean</em> is {@link Options}': this holds
 * the record, and the ladder that turns one into the map a backend is created
 * with is a question about every layer rather than about this file.
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
        // order is the order things were added, unless somebody has said
        // otherwise with {@link #reorder}, and either way that is what they have
        // learned the shape of.
        //
        // Every write goes through this list, so the file comes back out of a
        // write already in this order. Which is what settles the two questions
        // pinning would otherwise raise: a newly pinned connection is behind
        // every pinned one in the file and therefore lands at the end of them,
        // and an unpinned one is ahead of every unpinned one and lands at the
        // start of those. Both are where somebody looking for what they just
        // did would look.
        out.sort((x, y) -> Boolean.compare(y.pinned(), x.pinned()));
        return out;
    }

    /**
     * Put these connections in this order, leaving every other one where it is.
     *
     * <p>What moves is the occupants of the places they already had between
     * them, rather than the places: the ids given are shuffled among their own
     * slots, so reordering the pinned ones cannot disturb the unpinned list
     * underneath — the pins are a contiguous run at the front by the time
     * {@link #all} has sorted, but nothing here needs them to be.
     *
     * <p>Ignored if the list is no longer the one that was ordered — a
     * connection deleted from another window while the sheet was open — since
     * the alternative is applying half of somebody's arrangement.
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
            previewChanged(id);
        }
    }

    /**
     * Every connection, gone, and its preview with it — how many there were is
     * the answer, since a screen that destroys something has to be able to say
     * what it destroyed.
     *
     * <p>Not {@link #delete} over all the ids: this also takes the previews of
     * connections that are no longer in the list, which is where the pictures of
     * a machine somebody deleted on an older build would otherwise still be.
     * "Delete everything" means the directory rather than the records it can
     * account for.
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
        // Every add, edit, pin, import and delete comes through here, which is
        // why the launcher is told from here rather than from each of them: a
        // shortcut menu and a home screen icon are the list seen from outside
        // the app, and there is no such thing as a change to the list that they
        // are not a change to.
        Shortcuts.publish(ctx);
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

    /**
     * Which version of a connection's preview is on disk, counted from the
     * moment this process started.
     *
     * <p>What a cache of decoded previews compares against. A preview costs a
     * keystore round trip and a PNG decode, so it is worth keeping — and the
     * only thing that can have changed one while a list was away is a session
     * that has just left, which is <em>one</em> connection. Dropping the whole
     * cache on every resume made returning from a session re-decode every card
     * on the screen to pick up one of them.
     *
     * <p>Not a file timestamp: that is a stat call per card per bind, and the
     * question is not when the picture was taken but whether it is the one
     * already in hand.
     */
    public static int previewVersion(String id) {
        return previewEpoch + previewVersions.getOrDefault(id, 0);
    }

    /** Every preview at once, for the two things that take all of them. */
    private static volatile int previewEpoch;
    private static final Map<String, Integer> previewVersions = new ConcurrentHashMap<>();

    private static void previewChanged(String id) {
        previewVersions.merge(id, 1, Integer::sum);
    }

    /**
     * The preview, decoded off the main thread and handed back on it.
     *
     * <p>On the same one thread the writes go through, which is what makes
     * "the version asked for is the version that comes back" true with no lock
     * of its own: a session leaving bumps the number and queues its write
     * before this screen is resumed, so a read is behind that write rather
     * than racing it.
     */
    public static void readThumbnail(Context ctx, String id, Consumer<Bitmap> then) {
        final Context app = ctx.getApplicationContext();
        previewIo.execute(() -> {
            final Bitmap bmp = thumbnail(app, id);
            main.post(() -> then.accept(bmp));
        });
    }

    private static final Handler main = new Handler(Looper.getMainLooper());

    /** Blocking, and so private: {@link #readThumbnail} is the way in. */
    private static Bitmap thumbnail(Context ctx, String id) {
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
        previewEpoch++;
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
        // Before the write is queued rather than after it completes: a list
        // resuming behind this must see a number it does not have a picture
        // for, and then read behind the write on the same thread.
        previewChanged(id);
        previewIo.execute(() -> writeThumbnail(app, id, bmp));
    }

    /**
     * One thread for the preview directory, reads and writes alike. Single
     * because two leaves in quick succession must not write the same file at
     * once, and because a grid of cards decoding all at the same moment is a
     * burst of allocations for pictures that are drawn one after another.
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
