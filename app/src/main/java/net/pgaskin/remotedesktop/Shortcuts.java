// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.content.Context;
import android.content.pm.ShortcutInfo;
import android.content.pm.ShortcutManager;
import android.graphics.drawable.Icon;
import android.util.Log;

import java.util.ArrayList;
import java.util.HashSet;
import java.util.List;
import java.util.Set;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

/**
 * The connections, as the launcher knows them: the few in the menu behind a
 * long press on the app icon, and however many somebody has put on their home
 * screen.
 *
 * <p>Both are the same shortcut. One id per connection means a rename reaches
 * an icon on the home screen and a menu entry with one call, and it means the
 * icon somebody pinned last month is the one this class goes on maintaining
 * rather than a copy of it that drifts. {@link #publish} is therefore the whole
 * of the API surface: it is told nothing about what changed, reads the list,
 * and makes the launcher agree with it — which is also what makes it right
 * after an import, a restore, or an upgrade from a version that had none of
 * this.
 *
 * <p>What it does <em>not</em> do is give a shortcut the connection's desktop
 * preview. That would be the obvious icon and it is the wrong one: the preview
 * is a photograph of whatever was on that machine's screen, which is why
 * {@link Connections#thumbnail} keeps it encrypted — and a launcher's icon is
 * cached by the launcher, drawn on a screen anyone standing there can see, and
 * outlives the app's own decisions about it. The app's mark with a screen on it
 * says which app the shortcut is for, and nothing about the machine.
 *
 * <p>Off the main thread, because every call here is a binder round trip and
 * they hang off writing the connection list, which happens under a finger. One
 * thread rather than a pool: two publishes racing would be two answers to "what
 * is the list", and the launcher would keep whichever landed second.
 */
public final class Shortcuts {

    private static final String TAG = "Shortcuts";

    /**
     * What a connection's shortcut is called. Prefixed rather than the bare id
     * so that {@code dumpsys shortcut} reads as something, and so that a
     * shortcut this class did not create — a future one about a screen rather
     * than a connection — is not swept up by {@link #publish} as an orphan.
     */
    private static final String PREFIX = "connection:";

    private static final ExecutorService worker =
            Executors.newSingleThreadExecutor(r -> {
                final Thread t = new Thread(r, "shortcuts");
                t.setDaemon(true);
                return t;
            });

    private Shortcuts() {
    }

    /** Make the launcher agree with the saved connections, presently. */
    public static void publish(Context ctx) {
        final Context app = ctx.getApplicationContext();
        worker.execute(() -> republish(app));
    }

    /**
     * This connection was just opened, however it was opened.
     *
     * <p>What it buys is the launcher's own ranking — the menu behind the app
     * icon is short, and a phone that has learnt which machine somebody
     * actually connects to can put it first. Reported for every way in and not
     * only for a shortcut, since a connection opened from the home screen every
     * day is exactly the one that should be one press away.
     */
    public static void used(Context ctx, String connectionId) {
        if (connectionId == null || connectionId.isEmpty()) {
            return;
        }
        final Context app = ctx.getApplicationContext();
        worker.execute(() -> {
            final ShortcutManager mgr = app.getSystemService(ShortcutManager.class);
            if (mgr != null) {
                try {
                    mgr.reportShortcutUsed(PREFIX + connectionId);
                } catch (Exception e) {
                    Log.w(TAG, "reporting use", e);
                }
            }
        });
    }

    /** Whether the launcher on this phone takes shortcuts at all. */
    public static boolean canPin(Context ctx) {
        final ShortcutManager mgr = ctx.getSystemService(ShortcutManager.class);
        return mgr != null && mgr.isRequestPinShortcutSupported();
    }

    /**
     * Ask the launcher for a home screen icon of this connection's own.
     *
     * <p>On the calling thread, unlike everything else here, because the answer
     * is the caller's: this is a request the system puts a dialog in front of,
     * and it may only be made by an app somebody is looking at.
     *
     * @return false if it was not asked — no launcher support, or one that
     *         refused — which is the only case worth saying anything about, the
     *         rest of the conversation being the system's own dialog
     */
    public static boolean requestPin(Context ctx, Connection conn) {
        final ShortcutManager mgr = ctx.getSystemService(ShortcutManager.class);
        if (mgr == null || !mgr.isRequestPinShortcutSupported() || conn.title().isEmpty()) {
            return false;
        }
        try {
            // No IntentSender: what comes back is "the launcher accepted it",
            // which the launcher has already said on screen, and a callback
            // held across a dialog is a callback held across a rotation.
            return mgr.requestPinShortcut(build(ctx, conn, 0), null);
        } catch (Exception e) {
            Log.w(TAG, "requesting a pinned shortcut", e);
            return false;
        }
    }

    // ---- the list -----------------------------------------------------------

    private static void republish(Context ctx) {
        final ShortcutManager mgr = ctx.getSystemService(ShortcutManager.class);
        if (mgr == null) {
            return;
        }
        try {
            // In the home screen's own order, which is the only order this app
            // has an opinion about: pinned first, then as they were added. The
            // launcher re-ranks by use on top of it — see #used — so this is
            // the answer for a phone that has not learnt one yet.
            final List<ShortcutInfo> all = new ArrayList<>();
            final Set<String> live = new HashSet<>();
            for (Connection c : Connections.all(ctx)) {
                // A shortcut is its label, and a record with neither name nor
                // address has none. The editor will not save one; an imported
                // file can carry one.
                if (c.title().isEmpty()) {
                    continue;
                }
                all.add(build(ctx, c, all.size()));
                live.add(PREFIX + c.id());
            }

            // The two directions a pinned icon can be out of step with the
            // list, which are one question asked of the launcher rather than
            // something the callers have to remember to say. A connection that
            // came back — an import, a restored backup — gets its icon working
            // again; a deleted one leaves an icon that says so when pressed,
            // rather than one that fails with the app's name on it. Enabling
            // first, so that the update below lands on shortcuts that are live.
            final List<String> back = new ArrayList<>();
            final List<String> gone = new ArrayList<>();
            for (ShortcutInfo s : mgr.getPinnedShortcuts()) {
                if (!s.getId().startsWith(PREFIX)) {
                    continue;
                }
                if (live.contains(s.getId())) {
                    if (!s.isEnabled()) {
                        back.add(s.getId());
                    }
                } else if (s.isEnabled()) {
                    gone.add(s.getId());
                }
            }
            if (!back.isEmpty()) {
                mgr.enableShortcuts(back);
            }

            // As many as the launcher will hold, and the rest only as updates:
            // a shortcut that is pinned but no longer near the top of the list
            // is still an icon on somebody's home screen, and it has to follow
            // a rename like the others.
            final int max = Math.max(0, mgr.getMaxShortcutCountPerActivity());
            mgr.setDynamicShortcuts(all.subList(0, Math.min(max, all.size())));
            if (!all.isEmpty()) {
                mgr.updateShortcuts(all);
            }

            if (!gone.isEmpty()) {
                mgr.disableShortcuts(gone, ctx.getString(R.string.shortcut_deleted));
            }
        } catch (Exception e) {
            // Every call above is rate limited when it comes from an app nobody
            // is looking at, and a refused one is a launcher menu that is one
            // edit out of date until the next write. Not worth a crash on the
            // way out of saving a connection.
            Log.w(TAG, "publishing shortcuts", e);
        }
    }

    private static ShortcutInfo build(Context ctx, Connection conn, int rank) {
        return new ShortcutInfo.Builder(ctx, PREFIX + conn.id())
                .setShortLabel(conn.title())
                // Under the icon there is room for a word; in the menu there is
                // room for the line the home screen card carries.
                .setLongLabel(conn.subtitle().isEmpty() ? conn.title()
                        : ctx.getString(R.string.shortcut_label, conn.title(), conn.subtitle()))
                .setIcon(Icon.createWithResource(ctx, R.mipmap.ic_shortcut_connection))
                .setIntent(SessionActivity.intentFor(ctx, conn.id()))
                // What lets an icon outlive the menu it was dragged out of: a
                // shortcut that is not long lived may be dropped by the system
                // the moment it stops being one of the few published above, and
                // this app has more connections than the launcher has room for.
                .setLongLived(true)
                .setRank(rank)
                .build();
    }
}
