// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.session;

import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Context;
import android.util.Log;

import net.pgaskin.remotedesktop.AppSettings;

/**
 * The system clipboard, on both sides of a session.
 *
 * <p>The awkwardness this exists for is that the two directions have different
 * threads and different rules:
 *
 * <ul>
 *   <li><b>Ours → the remote</b> is a <em>pull</em>. Nothing in the RealVNC
 *       binding surface pushes a clipboard; the core asks on its own session
 *       thread, inline, so the answer has to be sitting in a field already.
 *   <li><b>The remote → ours</b> is a push, on the main thread, and is a plain
 *       {@code setPrimaryClip}.
 * </ul>
 *
 * <p>The cache is not an optimisation. Since Android 10 an app may only read the
 * clipboard <em>while it has focus</em>, and from Android 12 every read raises a
 * "pasted from clipboard" toast; a background thread asking at an arbitrary
 * moment would get null about as often as not, and would nag. Reading on the
 * clip-changed callback and on focus instead means the read happens exactly when
 * it is allowed to. The original reads inline, and gets away with it because it
 * was written against Android 5.
 */
final class SessionClipboard implements ClipboardManager.OnPrimaryClipChangedListener {

    private static final String TAG = "SessionClipboard";

    private static final int MAX_CHARS = 1 << 20; // longer is a file, not a copy

    /** Told whenever our clipboard changes, for a backend that can push. */
    interface Sink {
        void clipboardToRemote(String text);
    }

    private final Context ctx;
    private final ClipboardManager cm;
    private final Sink sink;

    private volatile String forRemote; // read from the session thread, written from the main one

    /**
     * The last text either side has seen, so a clipboard we just set from the
     * remote is not read back and sent straight to it again. Main thread only.
     */
    private String lastSeen;

    /**
     * Set while a write of our own is on its way to the system clipboard.
     *
     * <p>{@link #lastSeen} is not enough for that, because the change callback
     * our own write provokes arrives <em>before the new value is readable</em>:
     * {@code getPrimaryClip} at that moment returns the <b>previous</b> clip,
     * which is not {@code lastSeen} and so looks exactly like somebody having
     * copied something else. The cache then sits one value behind for ever and
     * pushes the stale text back to the server.
     */
    private boolean written;

    SessionClipboard(Context ctx, Sink sink) {
        this.ctx = ctx.getApplicationContext();
        this.cm = ctx.getSystemService(ClipboardManager.class);
        this.sink = sink;
    }

    void start() {
        if (cm != null) {
            cm.addPrimaryClipChangedListener(this);
            read();
        }
    }

    void stop() {
        if (cm != null) {
            cm.removePrimaryClipChangedListener(this);
        }
    }

    /**
     * Any thread: whatever we last read off the phone's clipboard.
     *
     * <p>Not gated on the sharing switch — the Paste key types this out, and
     * that is somebody asking for the text in front of them rather than a
     * session helping itself.
     */
    String current() {
        return forRemote;
    }

    /** Any thread, called inline by the protocol: what it may be given. */
    String forRemote() {
        return AppSettings.clipboardOut(ctx) ? forRemote : null;
    }

    /** Main thread. The remote copied something; make it ours. */
    void fromRemote(String text) {
        if (cm == null || text == null || text.isEmpty() || text.equals(lastSeen)
                || !AppSettings.clipboardIn(ctx)) {
            return;
        }
        lastSeen = text;
        forRemote = text;
        written = true;
        try {
            cm.setPrimaryClip(ClipData.newPlainText(null, text));
        } catch (RuntimeException e) {
            // A clipboard write can be refused outright (a work profile, a
            // device policy); it is not worth taking the session down for.
            written = false;
            Log.w(TAG, "cannot set the clipboard", e);
        }
    }

    @Override
    public void onPrimaryClipChanged() {
        if (written) {
            // Our own write coming back. There is nothing to learn from it and
            // something to lose: see the field.
            written = false;
            return;
        }
        read();
    }

    /**
     * Main thread. Re-read the system clipboard, if we are allowed to. Called on
     * every change and again whenever the window takes focus — a copy made in
     * another app changes the clip while we are not focused, so the change
     * callback that arrives with it reads null and the focus is the second
     * chance.
     */
    void read() {
        if (cm == null) {
            return;
        }
        final ClipData clip;
        try {
            clip = cm.getPrimaryClip();
        } catch (RuntimeException e) {
            Log.w(TAG, "cannot read the clipboard", e);
            return;
        }
        if (clip == null || clip.getItemCount() == 0) {
            return;
        }
        final CharSequence cs = clip.getItemAt(0).coerceToText(ctx);
        if (cs == null || cs.length() == 0 || cs.length() > MAX_CHARS) {
            return;
        }
        final String text = cs.toString();
        if (text.equals(lastSeen)) {
            return;
        }
        lastSeen = text;
        forRemote = text;
        // Both, because backends differ in which way round they work: RealVNC's
        // core only ever pulls, and one that can push should not have to wait to
        // be asked.
        if (AppSettings.clipboardOut(ctx)) {
            sink.clipboardToRemote(text);
        }
    }

    /** For the HUD: what the remote would be given, in short. */
    String summary() {
        final String s = forRemote();
        return s == null ? "-" : s.length() + "c";
    }
}
