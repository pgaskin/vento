// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.app.AlarmManager;
import android.app.PendingIntent;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.graphics.Bitmap;
import android.os.Looper;
import android.os.SystemClock;
import android.util.Log;

import net.pgaskin.remotedesktop.backend.Backend;
import net.pgaskin.remotedesktop.backend.Prompt;
import net.pgaskin.remotedesktop.session.SessionView;

import java.util.ArrayDeque;
import java.util.LinkedHashMap;
import java.util.Map;

/**
 * A live connection, separate from the screen showing it.
 *
 * <p>The activity cannot own the backend: <b>about five seconds in the
 * background drops the connection</b> (measured), because Android freezes a
 * cached process along with its sockets. So the session lives in a process kept
 * in the foreground by {@link SessionService}, and the screen
 * <em>attaches</em> to it.
 *
 * <p>Because of that split, this class:
 *
 * <ul>
 *   <li><b>Is the backend's listener</b>, and forwards to the attached view, or
 *       to nothing when none is attached.
 *   <li><b>Remembers what a new view needs to catch up</b>: the desktop size,
 *       the cursor, the connection state. A view attaching to a session that has
 *       been running for an hour missed all of those, and without the size it
 *       has no framebuffer to draw into.
 *   <li><b>Queues prompts.</b> A password dialog needs an activity, the
 *       connection is stalled until it is answered, and it can arrive while
 *       nothing is on screen. It is held until a screen attaches.
 * </ul>
 *
 * <p>Several sessions can run at once: {@link Sessions} holds them all, and each
 * has its own window, notification and line in the service notification. The
 * key prevents two sessions for the same target.
 */
public final class Session implements Backend.Listener, Prompt.Handler {

    /**
     * Take over {@code backend} (built, not yet connected) and keep it running
     * under a foreground service.
     *
     * @param key      the session target: a saved connection's id, or an
     *                 address. {@link Sessions#byKey} looks it up, so a screen
     *                 opened for the same target re-attaches instead of
     *                 reconnecting, and opening a new session for the same
     *                 target replaces the existing one.
     * @param reopen   how to get back to the screen from the notification
     * @param options  the options the backend was built with, kept so the panel
     *                 can show the session's actual settings, which differ from
     *                 the saved connection's once either has been edited
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
        // A backend may be third-party code loaded from an add-on, and a throw
        // from `connect` must end this session rather than the process. The
        // listener is already set, so the error shows in the open window.
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

    /** Never reset: a notification id can outlive the session it was posted for. */
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
     * The listener used when no screen is attached, so the forwarding methods
     * need no null checks.
     *
     * <p>Only the clipboard request returns a value: null, since the clipboard
     * cache is the view's.
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
    /** The backend's status line while connecting, or null. */
    private String statusLine;
    private int desktopW, desktopH;
    private Bitmap cursor;
    private int cursorHotX, cursorHotY;
    private boolean leaving;
    private boolean closed;

    /** Whether a screen is showing this session, and when one last did. */
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

    /** The options this session was opened with. */
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
     * The session's status message, computed in one place. The notification,
     * the status panel over the desktop and the connection panel's first line
     * all show it, so they cannot disagree.
     *
     * @param text    the message; empty while the session is running normally
     * @param ended   whether the connection has ended
     * @param working whether it is in progress, for a spinner
     */
    public record Status(String text, boolean ended, boolean working) {

        /**
         * The status for a state and its detail.
         *
         * <p>A detail describes its state, so it cannot override it: a
         * connected session shows only its desktop, whatever the last message
         * was. Without this, a screen re-attaching to a session whose stored
         * detail was still the "Connecting to …" sent while connecting showed
         * it over a desktop that had been up for an hour, as happened after a
         * phone woke from a long sleep.
         *
         * <p>{@code IDLE} shows as {@code CONNECTING} rather than nothing: the
         * backend has not reported anything yet, and a blank window explains
         * that worse than the word does.
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

    /** The session target, which {@link Sessions} keys it by. */
    public String key() {
        return key;
    }

    /** Whether this open session is for the target {@code key}. */
    public boolean matches(String key) {
        return !closed && this.key != null && this.key.equals(key);
    }

    /**
     * The id of this session's notification. Since a {@code PendingIntent}'s
     * identity does not include its extras, it is also the request code that
     * keeps one session's Disconnect button from ending another's.
     */
    int notificationId() {
        return notification;
    }

    /**
     * Whether the process still needs to stay in the foreground for this
     * session. False as soon as the connection ends, before the session is
     * released: the screen keeps showing why it stopped, but there is nothing
     * left to keep alive.
     */
    public boolean alive() {
        return !closed && state != Backend.State.CLOSED;
    }

    /** True when the user asked to disconnect, so the screen should close too. */
    public boolean leaving() {
        return leaving;
    }

    // ---- attaching a screen ------------------------------------------------

    /**
     * Attach a screen, and bring it up to date. A new view has no framebuffer
     * until it gets the desktop size, no cursor until it gets the shape, and no
     * status until it gets the state; a session that connected earlier sent all
     * three to no view.
     */
    public void attach(SessionView v, Prompt.Handler handler) {
        view = v;
        ui = handler;
        if (desktopW > 0 && desktopH > 0) {
            // SessionView allocates a new mirror, which starts fully dirty, so
            // the picture is read again too.
            v.desktopSize(desktopW, desktopH);
        }
        if (cursor != null) {
            v.cursor(cursor, cursorHotX, cursorHotY);
        }
        // Read from the backend rather than stored from the callback, so there
        // is one source for which end owns the cursor. Sent even when false,
        // since a new screen's controller is only absolute by default.
        v.pointerMode(backend.facts().pointerRelative());
        // The backend's current state, for the same reason: a screen returning
        // to a session running for an hour showed "Connecting…" over a live
        // desktop, because it was given the state from the last callback rather
        // than the current one. The stored detail belongs to the stored state
        // and is dropped when they differ ("Connecting to …" under CONNECTED is
        // worse than nothing).
        // An ended session is not re-read: a backend that threw while
        // connecting never reached a state of its own, so reading it would
        // replace the reason this session ended with "Connecting…" on a window
        // that will never connect. CLOSED is terminal here whatever the backend
        // reports.
        final Backend.State live = state == Backend.State.CLOSED ? state : backend.state();
        if (live != state) {
            state = live;
            detail = null;
        }
        v.state(state, detail);
        if (statusLine != null) {
            v.status(statusLine);
        }
        flushPrompts();
    }

    public void detach(SessionView v) {
        if (view != v) {
            // A newer screen has already attached; its view and prompt handler
            // are the current ones, and the two are set together.
            return;
        }
        view = NOBODY;
        ui = null;
    }

    // ---- timeout while off screen ------------------------------------------

    /**
     * Called by the screen when it becomes visible and when it stops being
     * visible, the same two points the backend's pause uses, so the app has one
     * definition of "on screen".
     *
     * <p>Visibility rather than window focus, as with the pause: a dialog over
     * the session takes the focus without hiding it. And not
     * {@link #attach}/{@link #detach}, which track the activity's existence: a
     * session backgrounded with its window intact is the usual case for the
     * timeout, and would never arm it.
     */
    public void onScreen(boolean visible) {
        onScreen = visible;
        if (!visible) {
            lastOnScreenAt = SystemClock.elapsedRealtime();
        }
        rearmTimeout();
    }

    /**
     * Set, move or cancel the alarm that closes this session, from the current
     * setting and when a screen last showed it.
     *
     * <p>The deadline is computed from {@link #lastOnScreenAt} rather than
     * counted down from now, which handles three cases without extra code: a
     * rotation re-arms against the same instant and changes nothing; shortening
     * the setting below the time already spent off screen sets a deadline in
     * the past, which {@code AlarmManager} delivers at once; and lengthening it
     * moves the deadline out with no stored count to adjust.
     *
     * <p>{@code ELAPSED_REALTIME} because {@code uptimeMillis} does not advance
     * in deep sleep, so a {@link android.os.Handler} would fire an eight-hour
     * timeout after eight hours <em>awake</em>, which for a phone in a pocket is
     * never. Inexact and non-waking on purpose: dropping a socket does not need
     * to wake the phone, and it avoids needing {@code SCHEDULE_EXACT_ALARM}.
     */
    void rearmTimeout() {
        final AlarmManager alarms = context.getSystemService(AlarmManager.class);
        final PendingIntent when = alarm();
        alarms.cancel(when);
        final int minutes = AppSettings.sessionTimeout(context);
        // An ended session is left as it is: its window shows why it ended, and
        // a timeout must not replace that reason.
        if (!alive() || onScreen || minutes <= 0) {
            return;
        }
        alarms.set(AlarmManager.ELAPSED_REALTIME,
                lastOnScreenAt + minutes * 60_000L, when);
    }

    /**
     * The alarm, one per session, with the notification id as its request code:
     * a {@code PendingIntent}'s identity is its request code and its intent
     * excluding the extras, so without a distinct code one session's deadline
     * would replace another's.
     */
    private PendingIntent alarm() {
        return PendingIntent.getBroadcast(context, notification,
                new Intent(context, Timeout.class).putExtra(EXTRA_SESSION, key),
                PendingIntent.FLAG_UPDATE_CURRENT | PendingIntent.FLAG_IMMUTABLE);
    }

    /**
     * Receives the alarm. A receiver rather than {@link SessionService} (which
     * handles the notification's Disconnect): the sessions are a map in this
     * process, so there is nothing to start. An alarm that outlived the process
     * that set it finds no session and does nothing, whereas a service start
     * would have required posting a notification for a connection that no
     * longer exists.
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
     * The alarm went off: end the connection, and leave the window showing why.
     *
     * <p>Not {@link #disconnect}, which sets {@link #leaving} and so closes the
     * screen. The user did not ask for this disconnect, so the window stays in
     * the CLOSED state (a window that disappeared would give no hint of the
     * setting), and the reason names the timeout length, since "Disconnected"
     * alone is what a dropped connection shows and would look like a fault.
     */
    void timedOut() {
        // An alarm already being delivered cannot be cancelled, so everything
        // that would have cancelled it is checked again here: a screen coming
        // back, the setting changed to Never, and the connection having ended
        // on its own in the meantime.
        final int minutes = AppSettings.sessionTimeout(context);
        if (!alive() || onScreen || minutes <= 0) {
            return;
        }
        final String reason = context.getString(R.string.session_timed_out,
                AppSettings.timeoutLabel(context, minutes));
        backend.disconnect();
        // Set after the disconnect so that a detail the backend reports while
        // disconnecting does not replace it: close() reports this one.
        detail = reason;
        close();
    }

    // ---- options changed during the session --------------------------------

    /**
     * Options the connection panel has changed on this session, overriding the
     * ones it connected with.
     *
     * <p>Kept here rather than on the screen because, like the rest of this
     * class, they belong to the connection, not the window. The panel layers
     * these over the saved connection's options and the backend's defaults to
     * get the value to show.
     *
     * <p>Session-scoped on purpose, and never saved: stored connections are
     * changed in the editor, and a panel control that rewrote them would be a
     * second editor.
     */
    private final Map<String, String> liveOptions = new LinkedHashMap<>();

    public void setLiveOption(String key, String value) {
        liveOptions.put(key, value);
        backend.setOption(key, value);
    }

    /** The value set by {@link #setLiveOption} since the session opened, or null. */
    public String liveOption(String key) {
        return liveOptions.get(key);
    }

    // ---- disconnecting -----------------------------------------------------

    /**
     * Disconnect at the user's request (the notification's button, the
     * disconnect region, the back gesture's dialog). Recorded as
     * {@link #leaving}, because a session ended by the user needs no message on
     * screen, while one that ends on its own does.
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
        // Cancels the alarm, now that closed is set. An alarm outlives the
        // process that set it, so one left behind would fire days later for a
        // connection that no longer exists.
        rearmTimeout();
        Sessions.remove(this);
        // Every queued prompt must be answered or cancelled exactly once, and no
        // screen will answer these now. The protocol thread may be blocked
        // waiting on one.
        while (!pending.isEmpty()) {
            pending.poll().cancel();
        }
        // The screen gets CLOSED from here rather than from the backend. A
        // disconnect from the notification releases the session before the
        // backend's own CLOSED arrives, and state() ignores callbacks once
        // closed is set, so this is the only report the view gets.
        report(Backend.State.CLOSED, detail);
        backend.destroy();
    }

    public boolean isClosed() {
        return closed;
    }

    // ---- Backend.Listener: store, then forward ------------------------------

    /**
     * Checks the "main thread" part of {@link Backend.Listener}'s contract.
     *
     * <p>Every backend's callbacks arrive here, and breaking the contract does
     * not fail here: a view invalidated from a native thread, a mirror replaced
     * while {@code onDraw} is reading it, a status set on a widget from another
     * thread each fail later, elsewhere, with no indication of which backend
     * caused it. With plugins, that includes code outside this repository.
     *
     * <p>Debug builds only, not because of the cost (a flag and a branch) but
     * because a release build should keep drawing the desktop rather than end
     * the session over a backend that breaks the contract.
     */
    private static void assertMain(String what) {
        if (BuildConfig.DEBUG && Looper.myLooper() != Looper.getMainLooper()) {
            throw new IllegalStateException("Backend.Listener." + what
                    + " off the main thread, on " + Thread.currentThread().getName());
        }
    }

    @Override
    public void state(Backend.State s, String d) {
        assertMain("state");
        // A released backend may still be finishing: its own CLOSED arrives
        // after close() has already reported one, and the session's report has
        // to be the last. Without this, a timeout's reason on the window was
        // replaced by "Disconnected" a moment later.
        if (closed) {
            return;
        }
        report(s, d);
    }

    private void report(Backend.State s, String d) {
        state = s;
        // Cleared rather than only hidden (which Status already does): close()
        // reports CLOSED with this detail, and the backend's last message
        // before connecting (RealVNC's core reports generating a key) is not
        // why the session ended an hour later.
        detail = s == Backend.State.CONNECTED ? null : d;
        // Only while connecting; the screen drops its copy itself.
        if (s == Backend.State.CONNECTED || s == Backend.State.CLOSED) {
            statusLine = null;
        }
        view.state(s, d);
        // The service watches the set of sessions and counts only alive ones,
        // so the last session ending stops it. Deliberately not close(): the
        // screen, if any, still shows the last frame and the reason it
        // stopped.
        Sessions.changed();
    }

    @Override
    public void desktopSize(int width, int height) {
        assertMain("desktopSize");
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
        assertMain("cursor");
        cursor = shape;
        cursorHotX = hotX;
        cursorHotY = hotY;
        view.cursor(shape, hotX, hotY);
    }

    @Override
    public void pointerMode(boolean relative) {
        assertMain("pointerMode");
        view.pointerMode(relative);
    }

    @Override
    public void bell() {
        assertMain("bell");
        view.bell();
    }

    /** Kept for a screen that attaches while it is up. */
    @Override
    public void status(String text) {
        assertMain("status");
        if (closed || state == Backend.State.CONNECTED || state == Backend.State.CLOSED) {
            return;
        }
        statusLine = text == null || text.isEmpty() ? null : text;
        view.status(statusLine);
    }

    @Override
    public void soundSwitch(int soundSwitch, boolean enabled, boolean playing) {
        assertMain("soundSwitch");
        view.soundSwitch(soundSwitch, enabled, playing);
    }

    @Override
    public void clipboardFromRemote(String text) {
        assertMain("clipboardFromRemote");
        view.clipboardFromRemote(text);
    }

    @Override
    public String clipboardForRemote() {
        return view.clipboardForRemote();
    }

    // ---- Prompt.Handler: queue, then show ----------------------------------

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
        // Until a screen shows it, which then decides what withdrawing does.
        p.whenWithdrawn(() -> {
            pending.remove(p);
            Sessions.changed();
        });
        flushPrompts();
    }

    /**
     * Show every queued prompt once a screen is attached: a connection can
     * reach its password prompt while the phone is in a pocket, and the
     * protocol thread is blocked on the answer until then.
     */
    private void flushPrompts() {
        while (ui != null && !pending.isEmpty()) {
            // No default: Prompt is sealed, so a new subclass is a compile error
            // here rather than a prompt that is silently never shown.
            switch (pending.poll()) {
                case Prompt.Credentials c -> ui.credentials(c);
                case Prompt.Trust t -> ui.trust(t);
                case Prompt.Message m -> ui.message(m);
            }
        }
    }

    /** Whether a prompt is waiting for a screen, for the notification. */
    public boolean hasPendingPrompt() {
        return !pending.isEmpty();
    }
}
