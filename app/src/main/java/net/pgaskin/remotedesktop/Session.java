// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.app.AlarmManager;
import android.app.PendingIntent;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.graphics.Bitmap;
import android.os.SystemClock;
import android.util.Log;

import net.pgaskin.remotedesktop.backend.Backend;
import net.pgaskin.remotedesktop.backend.Prompt;
import net.pgaskin.remotedesktop.session.SessionView;

import java.util.ArrayDeque;
import java.util.LinkedHashMap;
import java.util.Map;

/**
 * The live connection, which is not the same thing as the screen showing it.
 *
 * <p>The activity cannot own the backend, for a measured reason: <b>about five
 * seconds in the background costs the connection</b>, because Android freezes a
 * cached process and takes its sockets with it. So the session lives in a
 * process kept alive and visible by {@link SessionService}, and the screen
 * becomes something that <em>attaches</em> to it.
 *
 * <p>That split is what the rest of this class is about:
 *
 * <ul>
 *   <li><b>It is the backend's listener</b>, and forwards to whatever view is
 *       attached — or to nothing, harmlessly, when none is.
 *   <li><b>It remembers what a new view needs to catch up</b>: the desktop size,
 *       the cursor, the connection state. A view attaching to a session that has
 *       been running for an hour has missed every one of those, and without them
 *       it has no framebuffer to draw into.
 *   <li><b>It queues prompts.</b> A password dialog needs an activity, the
 *       connection is stalled behind it either way, and one can perfectly well
 *       arrive while nothing is on screen. Held until someone can ask.
 * </ul>
 *
 * <p>Several at once: {@link Sessions} holds them all, and each one has its own
 * window, its own notification and its own line in the service. What one
 * session may not have is two of itself, which is what the key is for.
 */
public final class Session implements Backend.Listener, Prompt.Handler {

    /**
     * Take over {@code backend} — already built, not yet connected — and put it
     * behind a foreground service.
     *
     * @param key      what this session is of: a saved connection's id, or an
     *                 address. {@link Sessions#byKey} compares it, so a screen
     *                 opened for the same thing re-attaches instead of
     *                 reconnecting, and a second one for the same machine
     *                 replaces rather than joins.
     * @param reopen   how to get back to the screen from the notification
     * @param options  what the backend was built with, kept so the panel can
     *                 show what this session is actually on — which is not the
     *                 saved connection's answer once one of them has been edited
     */
    public static Session open(Context ctx, String key, String title, String subtitle,
                               Intent reopen, Backend backend, Map<String, String> options) {
        final Session was = Sessions.byKey(key);
        if (was != null) {
            was.close();
        }
        final Session s = new Session(ctx.getApplicationContext(), key, title, subtitle,
                reopen, backend, options);
        backend.setListener(s);
        backend.setPromptHandler(s);
        Sessions.add(s);
        SessionService.start(s.context);
        // A backend may be somebody else's code, loaded out of an add-on, and a
        // throw out of `connect` must end this session rather than the process.
        // The listener is already set, so the session says what happened in the
        // window that is already open.
        try {
            backend.connect();
        } catch (Throwable t) {
            Log.e("Session", "connecting with " + backend.getClass().getName(), t);
            final String said = t.getMessage();
            s.state(Backend.State.CLOSED,
                    said == null || said.isEmpty() ? t.getClass().getSimpleName() : said);
        }
        return s;
    }

    /** Never reset: a notification id outlives the session it was posted for. */
    private static int nextNotification = 1;

    /** Which session an alarm is for, on the intent {@link Timeout} receives. */
    private static final String EXTRA_SESSION = "session";

    private final Context context;
    private final String key;
    private final String title;
    private final String subtitle;
    private final Intent reopen;
    private final Backend backend;
    private final Map<String, String> openedWith;
    private final int notification = nextNotification++;

    /**
     * Where a callback goes when no screen is attached, so that "nobody is
     * looking" is a state this class can be in rather than a condition each of
     * nine forwarding methods has to remember to test.
     *
     * <p>Nothing here is owed an answer except the clipboard, which is a pull
     * rather than a push: null is all a session with no screen has to give,
     * since the cache the answer comes out of is the view's.
     */
    private static final Backend.Listener NOBODY = new Backend.Listener() {
        @Override
        public void state(Backend.State state, String detail) {
        }

        @Override
        public void desktopSize(int width, int height) {
        }

        @Override
        public void damaged(int x, int y, int width, int height) {
        }

        @Override
        public void frameEnd() {
        }

        @Override
        public void cursor(Bitmap shape, int hotX, int hotY) {
        }

        @Override
        public void pointerMode(boolean relative) {
        }

        @Override
        public void bell() {
        }

        @Override
        public void clipboardFromRemote(String text) {
        }

        @Override
        public String clipboardForRemote() {
            return null;
        }
    };

    /** Written on the main thread, read on the protocol's. */
    private volatile Backend.Listener view = NOBODY;
    private Prompt.Handler ui;

    private final ArrayDeque<Prompt> pending = new ArrayDeque<>();

    private Backend.State state = Backend.State.IDLE;
    private String detail;
    private int desktopW, desktopH;
    private Bitmap cursor;
    private int cursorHotX, cursorHotY;
    private boolean leaving;
    private boolean closed;

    /** Whether a screen is showing this, and when one last was. */
    private boolean onScreen;
    private long lastOnScreenAt;

    private Session(Context context, String key, String title, String subtitle,
                    Intent reopen, Backend backend, Map<String, String> options) {
        this.context = context;
        this.lastOnScreenAt = SystemClock.elapsedRealtime();
        this.key = key;
        this.title = title;
        this.subtitle = subtitle;
        this.reopen = reopen;
        this.backend = backend;
        this.openedWith = options == null ? Map.of() : Map.copyOf(options);
    }

    /** The options this session was opened with, whatever they came from. */
    public Map<String, String> openedWith() {
        return openedWith;
    }

    public Backend backend() {
        return backend;
    }

    public String title() {
        return title;
    }

    public String subtitle() {
        return subtitle;
    }

    public Intent reopenIntent() {
        return reopen;
    }

    public Backend.State state() {
        return state;
    }

    /**
     * What this session has to say for itself, in the one place that decides
     * it. The notification, the status panel over the desktop and the
     * connection panel's first line all show this, and three renderings of the
     * same pair are three chances for them to disagree about what a session is
     * doing.
     *
     * @param text    what to say; empty while a session is running normally,
     *                which is the whole of "there is nothing to report"
     * @param ended   whether there is a connection left to act on
     * @param working whether it is on its way somewhere, so a spinner turns
     */
    public record Status(String text, boolean ended, boolean working) {

        /**
         * One pair, rendered.
         *
         * <p>A detail is what a state says <em>about itself</em>, so it cannot
         * outrank the state: a connected session shows its desktop and nothing
         * over it, whatever the last message was. Without that rule a screen
         * re-attaching to a session whose remembered detail is still
         * "Connecting to …" — the one the backend sent on the way in — puts
         * that over an hour-old desktop, which is what a phone coming back
         * from a long sleep did.
         *
         * <p>{@code IDLE} reads as {@code CONNECTING} rather than as silence:
         * a backend that has not said anything yet is one nothing has come
         * back from, and a blank window is a worse account of that than the
         * word.
         */
        public static Status of(Context ctx, Backend.State state, String detail) {
            final boolean has = detail != null && !detail.isEmpty();
            return switch (state) {
                case IDLE, CONNECTING -> new Status(
                        has ? detail : ctx.getString(R.string.session_connecting), false, true);
                case CONNECTED -> new Status("", false, false);
                case CLOSED -> new Status(
                        has ? detail : ctx.getString(R.string.session_disconnected), true, false);
            };
        }
    }

    /** @see Status */
    public Status status() {
        return Status.of(context, state, detail);
    }

    /** What this session is of, which is what {@link Sessions} keys it by. */
    public String key() {
        return key;
    }

    /** Is this a session of the same thing a screen is being opened for? */
    public boolean matches(String key) {
        return !closed && this.key != null && this.key.equals(key);
    }

    /**
     * What the service posts this session's notification under, and cancels it
     * by — and, since a {@code PendingIntent}'s identity does not include its
     * extras, also the request code that keeps one session's Disconnect button
     * from ending another's.
     */
    int notificationId() {
        return notification;
    }

    /**
     * Still worth keeping a process alive for. False from the moment the far
     * end goes, which is before anybody lets go of the session: the screen goes
     * on showing why it stopped, and there is nothing left to be foreground for.
     */
    public boolean alive() {
        return !closed && state != Backend.State.CLOSED;
    }

    /** True when the disconnect was asked for, so the screen should go too. */
    public boolean leaving() {
        return leaving;
    }

    // ---- the screen coming and going ---------------------------------------

    /**
     * Point the session at a screen, and bring that screen up to date — which is
     * the whole difference between a listener and an attachment. A new view has
     * no framebuffer until it is told the desktop size, no cursor until it is
     * told the shape, and nothing to explain itself with until it is told the
     * state; a session that connected first has sent all three to nobody.
     */
    public void attach(SessionView v, Prompt.Handler handler) {
        view = v;
        ui = handler;
        if (desktopW > 0 && desktopH > 0) {
            // SessionView answers this by allocating the mirror and damaging all
            // of it, so the picture comes back with it.
            v.desktopSize(desktopW, desktopH);
        }
        if (cursor != null) {
            v.cursor(cursor, cursorHotX, cursorHotY);
        }
        // Asked of the backend rather than remembered from the callback, so there
        // is one answer to "who owns the cursor". Told even when it is false: a
        // new screen's controller is absolute only by default.
        v.pointerMode(backend.pointerIsRelative());
        // The backend's own answer, for the same reason and a sharper one: a
        // screen coming back to a session that has been running for an hour was
        // showing "Connecting…" over a desktop that had been up the whole time,
        // because what it was told is what the last callback said rather than
        // what is true now. The remembered detail goes with the remembered
        // state and is dropped when they disagree — "Connecting to …" under
        // CONNECTED is worse than nothing.
        // A session that has already ended is not re-asked, which is the other
        // half of the same rule: a backend that threw on the way up never
        // reached a state of its own, so asking it turns the reason this
        // session ended back into "Connecting…" on a window that will never
        // connect. CLOSED is terminal here whatever the backend says.
        final Backend.State live = state == Backend.State.CLOSED ? state : backend.state();
        if (live != state) {
            state = live;
            detail = null;
        }
        v.state(state, detail);
        flushPrompts();
    }

    public void detach(SessionView v) {
        if (view != v) {
            // A newer screen has already attached — its view and its prompt
            // handler are the live ones, and the two are set together.
            return;
        }
        view = NOBODY;
        ui = null;
    }

    // ---- closing itself after a while off screen ----------------------------

    /**
     * Told by the screen at the two moments it becomes and stops being visible,
     * which are the same pair the backend's pause hangs off — one definition of
     * "on screen" in the app.
     *
     * <p>Visibility rather than window focus, for the reason the pause has: a
     * dialog over the session takes the focus without hiding anything. And
     * rather than {@link #attach}/{@link #detach}, which are about the activity
     * existing: a session backgrounded with its window intact is the ordinary
     * case this is for, and would never arm.
     */
    public void onScreen(boolean visible) {
        onScreen = visible;
        if (!visible) {
            lastOnScreenAt = SystemClock.elapsedRealtime();
        }
        rearmTimeout();
    }

    /**
     * Set, move or drop the alarm that closes this session, from what the
     * setting says now and when a screen last had it.
     *
     * <p>The deadline is computed from {@link #lastOnScreenAt} rather than
     * counted down from here, which is what makes three cases fall out for
     * free: a rotation re-arms against the same instant and changes nothing, a
     * setting shortened past what a session has already spent off screen sets a
     * deadline in the past — which {@code AlarmManager} delivers at once, being
     * what somebody choosing a shorter one means — and lengthening it moves the
     * deadline out with no stored count to adjust.
     *
     * <p>{@code ELAPSED_REALTIME} because {@code uptimeMillis} does not advance
     * in deep sleep, so a {@link android.os.Handler} would fire an eight-hour
     * timeout after eight hours <em>awake</em>, which on a phone in a pocket is
     * never. Inexact and non-waking on purpose: nothing here needs the phone
     * woken to drop a socket, and it keeps the feature clear of
     * {@code SCHEDULE_EXACT_ALARM}, which this app would have to justify.
     */
    void rearmTimeout() {
        final AlarmManager alarms = context.getSystemService(AlarmManager.class);
        final PendingIntent when = alarm();
        alarms.cancel(when);
        final int minutes = AppSettings.sessionTimeout(context);
        // A session that has already ended is left exactly as it is: its window
        // is showing why, and that reason is worth more than this one.
        if (!alive() || onScreen || minutes <= 0) {
            return;
        }
        alarms.set(AlarmManager.ELAPSED_REALTIME,
                lastOnScreenAt + minutes * 60_000L, when);
    }

    /**
     * The alarm, which is one per session and identified by the same number the
     * notification is: a {@code PendingIntent}'s identity is its request code
     * and its intent bar the extras, so without a distinct code one session's
     * deadline would replace another's.
     */
    private PendingIntent alarm() {
        return PendingIntent.getBroadcast(context, notification,
                new Intent(context, Timeout.class).putExtra(EXTRA_SESSION, key),
                PendingIntent.FLAG_UPDATE_CURRENT | PendingIntent.FLAG_IMMUTABLE);
    }

    /**
     * Where the alarm lands. A receiver rather than {@link SessionService},
     * which is where the notification's Disconnect goes: the sessions are a map
     * in this process, so there is nothing to start and nothing to be
     * foreground for — and an alarm that outlived the process it was set from
     * finds no session here and does nothing at all, where a service start
     * would have owed a notification for a connection that no longer exists.
     */
    public static final class Timeout extends BroadcastReceiver {
        @Override
        public void onReceive(Context ctx, Intent intent) {
            final Session s = Sessions.byKey(intent.getStringExtra(EXTRA_SESSION));
            if (s != null) {
                s.timedOut();
            }
        }
    }

    /**
     * The alarm went off: end the connection, and leave the window saying so.
     *
     * <p>Not {@link #disconnect}, which sets {@link #leaving} and so takes the
     * screen with it. Nobody asked for this one, so the window stays in the
     * CLOSED state it already knows how to draw — a window that had quietly
     * vanished would leave nothing to connect the setting to — and the reason
     * names the length of time, since "Disconnected" on its own is what a far
     * end going away says and would read as a fault in the connection.
     */
    void timedOut() {
        // An alarm already in flight is not cancellable, so everything that
        // would have cancelled it is checked again here: a screen coming back
        // in that moment, the row being set to Never, and the far end having
        // gone by itself in the meantime.
        final int minutes = AppSettings.sessionTimeout(context);
        if (!alive() || onScreen || minutes <= 0) {
            return;
        }
        final String reason = context.getString(R.string.session_timed_out,
                AppSettings.timeoutLabel(context, minutes));
        backend.disconnect();
        // After the far end has been told, so that whatever it says on the way
        // out is not the last word: close() reports this one to the screen.
        detail = reason;
        close();
    }

    // ---- options changed while it runs --------------------------------------

    /**
     * What the connection panel has changed on this session, over whatever it
     * connected with.
     *
     * <p>Here rather than on the screen for the same reason as everything else
     * in this class: a quality lowered ten minutes ago is a fact about the
     * connection, not about the window. The panel layers this over the saved
     * connection's options and the backend's defaults to get a value to show.
     *
     * <p>Session-scoped on purpose, and never written back: the editor is where
     * a stored connection is changed, and a control that quietly rewrote one
     * would be a second editor.
     */
    private final Map<String, String> liveOptions = new LinkedHashMap<>();

    public void setLiveOption(String key, String value) {
        liveOptions.put(key, value);
        backend.setOption(key, value);
    }

    /** What this session has been told since it came up, or null. */
    public String liveOption(String key) {
        return liveOptions.get(key);
    }

    // ---- ending it ---------------------------------------------------------

    /**
     * Ask the far end to go away, on purpose — the notification's button, the
     * disconnect region, the back gesture's dialog. Recorded as
     * {@link #leaving}, because a session told to end needs no message on
     * screen, while one that ends by itself does.
     */
    public void disconnect() {
        leaving = true;
        backend.disconnect();
        close();
    }

    /** Let go of everything. The session is dead afterwards. */
    public void close() {
        if (closed) {
            return;
        }
        closed = true;
        // Which cancels, now that closed is set. An alarm outlives the process
        // that set it, so one left behind for an ended session is a wake-up
        // days later for a connection nobody has any memory of.
        rearmTimeout();
        Sessions.remove(this);
        // Whatever is still queued is owed an answer, and there will never be
        // anyone to give it: the core blocks its session thread until each one
        // is dealt with, and destroy() below is what unblocks it.
        while (!pending.isEmpty()) {
            pending.poll().cancel();
        }
        // The screen hears it from here rather than from the backend. A
        // disconnect asked for on the notification frees the session before the
        // far end's own "closed" can come back through it, so this is the only
        // report the view is going to get — and, since the guard above is now
        // shut, the only one it can get.
        report(Backend.State.CLOSED, detail);
        backend.destroy();
    }

    public boolean isClosed() {
        return closed;
    }

    // ---- Backend.Listener: remember, then forward --------------------------

    @Override
    public void state(Backend.State s, String d) {
        // A backend that has been let go of is still finishing: its own "closed"
        // arrives after close() has already reported one, and the last word has
        // to be the session's. Without this a timeout says why on the window and
        // then has it replaced by "Disconnected" a moment later.
        if (closed) {
            return;
        }
        report(s, d);
    }

    private void report(Backend.State s, String d) {
        state = s;
        // Dropped rather than merely not shown, which Status already does:
        // this is the detail close() reports CLOSED with, and a backend's last
        // word before it came up — RealVNC's core says it is generating a key
        // — is not why the session ended an hour later.
        detail = s == Backend.State.CONNECTED ? null : d;
        view.state(s, d);
        // The service is watching the set rather than this session, and an
        // ended session is no longer in what it counts — so a last session
        // ending is what stops it. Deliberately not close(): the screen, if
        // there is one, is still showing the last frame and the reason it
        // stopped.
        Sessions.changed();
    }

    @Override
    public void desktopSize(int width, int height) {
        desktopW = width;
        desktopH = height;
        view.desktopSize(width, height);
    }

    @Override
    public void damaged(int x, int y, int width, int height) {
        view.damaged(x, y, width, height);
    }

    @Override
    public void frameEnd() {
        view.frameEnd();
    }

    @Override
    public void cursor(Bitmap shape, int hotX, int hotY) {
        cursor = shape;
        cursorHotX = hotX;
        cursorHotY = hotY;
        view.cursor(shape, hotX, hotY);
    }

    @Override
    public void pointerMode(boolean relative) {
        view.pointerMode(relative);
    }

    @Override
    public void bell() {
        view.bell();
    }

    @Override
    public void clipboardFromRemote(String text) {
        view.clipboardFromRemote(text);
    }

    @Override
    public String clipboardForRemote() {
        return view.clipboardForRemote();
    }

    // ---- Prompt.Handler: queue, then ask -----------------------------------

    @Override
    public void credentials(Prompt.Credentials prompt) {
        queue(prompt);
    }

    @Override
    public void trust(Prompt.Trust prompt) {
        queue(prompt);
    }

    @Override
    public void message(Prompt.Message prompt) {
        queue(prompt);
    }

    private void queue(Prompt p) {
        pending.add(p);
        flushPrompts();
    }

    /**
     * Ask everything that has piled up, as soon as there is somebody to ask: a
     * connection can reach its password prompt while the phone is in a pocket,
     * and the protocol thread is blocked on the answer either way.
     */
    private void flushPrompts() {
        while (ui != null && !pending.isEmpty()) {
            // No default: Prompt is sealed, so a fourth kind of question is a
            // compile error here rather than one that is silently never asked.
            switch (pending.poll()) {
                case Prompt.Credentials c -> ui.credentials(c);
                case Prompt.Trust t -> ui.trust(t);
                case Prompt.Message m -> ui.message(m);
            }
        }
    }

    /** Whether a prompt is waiting for a screen to appear, for the notification. */
    public boolean hasPendingPrompt() {
        return !pending.isEmpty();
    }
}
