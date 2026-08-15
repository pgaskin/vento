// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * Every session that has been opened and not yet let go of, keyed by what it is
 * a session <em>of</em>.
 *
 * <p>There is one of these rather than one live session because a window is not
 * the connection: each session has a window of its own in the recents list, two
 * of them side by side in a split screen are both live, and a session with no
 * window at all is the ordinary case the foreground service exists for. What a
 * key names is a machine, so the same connection opened twice attaches to what
 * is already running instead of dialling it again.
 *
 * <p>Main thread only. Everything that reaches it — a screen opening or
 * finishing, a state callback the seam has already posted — is there already.
 */
public final class Sessions {

    /**
     * Told when the set changes or when anything in it changes what it says
     * about itself. One notification for the lot: both listeners rebuild from
     * the whole set anyway, and a per-session callback would mean the service
     * subscribing and unsubscribing as sessions come and go.
     */
    public interface Watcher {
        void sessionsChanged();
    }

    private static final Map<String, Session> live = new LinkedHashMap<>();
    private static final List<Watcher> watchers = new ArrayList<>();

    private Sessions() {
    }

    /**
     * What a session of a saved connection is keyed by, and what a session of a
     * bare address is. Here rather than at either caller because the home
     * screen asks whether a connection is live and the session screen answers
     * to the same question, and two spellings of one key would be two
     * screens quietly disagreeing.
     */
    public static String keyFor(String connectionId) {
        return "id:" + connectionId;
    }

    public static String keyForAddress(String address) {
        return "addr:" + address;
    }

    /** Whatever is registered for {@code key}, alive or ended, or null. */
    public static Session byKey(String key) {
        return key == null ? null : live.get(key);
    }

    /** Every session, in the order they were opened. */
    public static List<Session> all() {
        return List.copyOf(live.values());
    }

    /**
     * The ones still worth a notification: connected, or on their way there. A
     * session that has ended stays registered until its screen lets go of it,
     * so that the screen can go on showing the reason it stopped.
     */
    public static List<Session> alive() {
        final List<Session> out = new ArrayList<>(live.size());
        for (Session s : live.values()) {
            if (s.alive()) {
                out.add(s);
            }
        }
        return out;
    }

    /** Whether {@code key} has a session that has not ended. */
    public static boolean isAlive(String key) {
        final Session s = byKey(key);
        return s != null && s.alive();
    }

    static void add(Session s) {
        live.put(s.key(), s);
        changed();
    }

    static void remove(Session s) {
        // Only if it is still the one registered: closing a session that has
        // already been replaced by a reconnect must not unregister the new one.
        if (live.get(s.key()) == s) {
            live.remove(s.key());
        }
        changed();
    }

    /**
     * The session timeout has been set, imported or reset, so every live
     * session's deadline moves with it — including one already off screen for
     * longer than the new answer, which closes now.
     */
    public static void timeoutChanged() {
        for (Session s : all()) {
            s.rearmTimeout();
        }
    }

    public static void addWatcher(Watcher w) {
        watchers.add(w);
    }

    public static void removeWatcher(Watcher w) {
        watchers.remove(w);
    }

    /** A copy, because a watcher may register or unregister from inside this. */
    static void changed() {
        for (Watcher w : new ArrayList<>(watchers)) {
            w.sessionsChanged();
        }
    }
}
