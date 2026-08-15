// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.content.Context;
import android.content.Intent;
import android.graphics.Bitmap;

import net.pgaskin.remotedesktop.backend.Backend;
import net.pgaskin.remotedesktop.backend.Prompt;

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
        backend.connect();
        return s;
    }

    /** Never reset: a notification id outlives the session it was posted for. */
    private static int nextNotification = 1;

    private final Context context;
    private final String key;
    private final String title;
    private final String subtitle;
    private final Intent reopen;
    private final Backend backend;
    private final Map<String, String> openedWith;
    private final int notification = nextNotification++;

    /** Written on the main thread, read on the protocol's. */
    private volatile Backend.Listener view;
    private Prompt.Handler ui;

    private final ArrayDeque<Prompt> pending = new ArrayDeque<>();

    private Backend.State state = Backend.State.IDLE;
    private String detail;
    private int desktopW, desktopH;
    private Bitmap cursor;
    private int cursorHotX, cursorHotY;
    private boolean leaving;
    private boolean closed;

    private Session(Context context, String key, String title, String subtitle,
                    Intent reopen, Backend backend, Map<String, String> options) {
        this.context = context;
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

    /** What the notification says under the name. */
    public String status() {
        final boolean has = detail != null && !detail.isEmpty();
        return switch (state) {
            case IDLE, CONNECTING -> has ? detail : context.getString(R.string.session_connecting);
            case CONNECTED -> subtitle;
            case CLOSED -> has ? detail : context.getString(R.string.session_disconnected);
        };
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
        final Backend.State live = backend.state();
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
        view = null;
        ui = null;
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
        // report the view is going to get.
        state(Backend.State.CLOSED, detail);
        backend.destroy();
    }

    public boolean isClosed() {
        return closed;
    }

    // ---- Backend.Listener: remember, then forward --------------------------

    @Override
    public void state(Backend.State s, String d) {
        state = s;
        // A detail is what a state says about itself and cannot outlive it: a
        // backend that reports its progress — RealVNC's core says it is
        // generating a key — sends the last of those after the session is up,
        // and a notification that kept it said so for the rest of the hour.
        detail = s == Backend.State.CONNECTED ? null : d;
        final Backend.Listener v = view;
        if (v != null) {
            v.state(s, d);
        }
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
        final Backend.Listener v = view;
        if (v != null) {
            v.desktopSize(width, height);
        }
    }

    @Override
    public void damaged(int x, int y, int width, int height) {
        final Backend.Listener v = view;
        if (v != null) {
            v.damaged(x, y, width, height);
        }
    }

    @Override
    public void frameEnd() {
        final Backend.Listener v = view;
        if (v != null) {
            v.frameEnd();
        }
    }

    @Override
    public void cursor(Bitmap shape, int hotX, int hotY) {
        cursor = shape;
        cursorHotX = hotX;
        cursorHotY = hotY;
        final Backend.Listener v = view;
        if (v != null) {
            v.cursor(shape, hotX, hotY);
        }
    }

    @Override
    public void pointerMode(boolean relative) {
        final Backend.Listener v = view;
        if (v != null) {
            v.pointerMode(relative);
        }
    }

    @Override
    public void bell() {
        final Backend.Listener v = view;
        if (v != null) {
            v.bell();
        }
    }

    @Override
    public void clipboardFromRemote(String text) {
        final Backend.Listener v = view;
        if (v != null) {
            v.clipboardFromRemote(text);
        }
        // With no screen attached there is nowhere to put it: the clipboard is
        // the view's (SessionClipboard), and an app without focus may not write
        // one anyway.
    }

    @Override
    public String clipboardForRemote() {
        final Backend.Listener v = view;
        return v != null ? v.clipboardForRemote() : null;
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
