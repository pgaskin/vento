// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend.spice;

import android.content.Context;
import android.graphics.Bitmap;
import android.os.Handler;
import android.os.Looper;
import android.util.Log;

import net.pgaskin.remotedesktop.backend.Backend;
import net.pgaskin.remotedesktop.backend.ConnectionFact;
import net.pgaskin.remotedesktop.backend.CursorCache;
import net.pgaskin.remotedesktop.backend.KnownHosts;
import net.pgaskin.remotedesktop.backend.Monitor;
import net.pgaskin.remotedesktop.backend.Prompt;

import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.function.Consumer;

/**
 * A {@link Backend} on the SPICE client in {@code src/main/rust}.
 *
 * <p>It is {@code RfbBackend}'s shape, because the seam is what both answer,
 * and what differs is about the protocol rather than about this class:
 *
 * <ul>
 *   <li><b>The far end is a hypervisor, not a desktop.</b> Everything else here
 *       talks to something running inside a machine; this talks to the thing
 *       drawing that machine's screen, so what it can show includes firmware, a
 *       bootloader and a machine that never finishes starting.
 *   <li><b>No user name, and the password is asked for after it is refused.</b>
 *       A ticket is the whole of what a SPICE server wants, and the key it is
 *       encrypted to is made per connection — so a refused ticket is answered
 *       by dialling again rather than by sending another one.
 *   <li><b>Encryption is which port was typed.</b> A plain SPICE port and a TLS
 *       one are different ports rather than a negotiation, so it is a setting
 *       and the security row says which of the two this session is.
 *   <li><b>Nothing to pin without TLS.</b> The key in a plain session's link
 *       reply is generated per connection and identifies nothing, so
 *       {@link KnownHosts} has an entry only for a TLS session — the first
 *       backend here where that is true.
 *   <li><b>The clipboard and the resize are the guest's, not the protocol's.</b>
 *       Both travel over {@code spice-vdagent} inside the machine, so a guest
 *       without it has neither, and the panel says whether it is running.
 * </ul>
 */
public final class SpiceBackend implements Backend, SpiceNative.Callbacks {

    static final String TAG = "Spice";

    /**
     * Long enough for a hypervisor to answer four link handshakes, short enough
     * that a mistyped address is not a spinner that never stops. It covers the
     * connection and not the person typing a password.
     */
    private static final int CONNECT_TIMEOUT_MS = 20_000;

    private final Handler main = new Handler(Looper.getMainLooper());

    /**
     * The remote cursor's shapes. Touched only by {@link #onCursor}, which is
     * the session thread's, so it needs no lock of its own.
     */
    private final CursorCache cursors = new CursorCache();
    private final Context context;
    private final String address;
    private final String password;
    private final Map<String, String> options;

    private Listener listener;
    private Prompt.Handler prompts;

    /** Guards {@link #handle} against being freed under a read in flight. */
    private final Object pixels = new Object();
    private volatile long handle;

    private volatile State state = State.IDLE;
    private volatile int desktopWidth;
    private volatile int desktopHeight;
    private volatile boolean dead;

    public SpiceBackend(Context context, String address, String userName, String password,
                        Map<String, String> extraOptions) {
        this.context = context.getApplicationContext();
        this.address = address;
        // Empty is not a password: something that looks like a stored secret
        // and is not one fails the connection instead of asking for the real
        // one.
        this.password = password == null || password.isEmpty() ? null : password;
        this.options = new ConcurrentHashMap<>();
        for (var option : SpiceProvider.OPTIONS) {
            options.put(option.key(), option.defaultValue());
        }
        if (extraOptions != null) {
            options.putAll(extraOptions);
        }
    }

    // ---- lifecycle ---------------------------------------------------------

    @Override
    public void setListener(Listener listener) {
        this.listener = listener;
    }

    @Override
    public void setPromptHandler(Prompt.Handler handler) {
        this.prompts = handler;
    }

    @Override
    public void connect() {
        if (state != State.IDLE) {
            return;
        }
        state = State.CONNECTING;
        fireState(State.CONNECTING, "Connecting to " + address);
        final long h = SpiceNative.nativeCreate(this, address, tls(), password,
                options.get(SpiceProvider.COMPRESSION),
                bool(SpiceProvider.VIEW_ONLY),
                CONNECT_TIMEOUT_MS);
        if (h == 0) {
            closed("Could not start the connection");
            return;
        }
        synchronized (pixels) {
            handle = h;
        }
    }

    @Override
    public void disconnect() {
        final long h = handle;
        if (h != 0) {
            SpiceNative.nativeDisconnect(h);
        } else {
            closed("Disconnected");
        }
    }

    @Override
    public void destroy() {
        dead = true;
        final long h;
        // Retire it first: a read that has already got past this point holds
        // the framebuffer's lock, and nativeDestroy waits for the session
        // thread, not for that.
        synchronized (pixels) {
            h = handle;
            handle = 0;
        }
        if (h != 0) {
            SpiceNative.nativeDestroy(h);
        }
    }

    @Override
    public State state() {
        return state;
    }

    @Override
    public int desktopWidth() {
        return desktopWidth;
    }

    @Override
    public int desktopHeight() {
        return desktopHeight;
    }

    /**
     * The heads the far end has divided this desktop into, which SPICE
     * <em>announces</em> where every other protocol here leaves it to be
     * inferred from a screen boundary.
     */
    @Override
    public List<Monitor> monitors() {
        final long h = handle;
        if (h == 0 || dead || state != State.CONNECTED) {
            return List.of();
        }
        final int[] flat = SpiceNative.nativeMonitors(h);
        return flat == null ? List.of() : Monitor.fromFlat(flat);
    }

    /**
     * Whether the guest is running the agent and said it does monitors — so it
     * is false for a machine that has not booted, false for a guest without the
     * program, and true a moment after the agent announces itself, which may be
     * long after the picture arrives.
     */
    @Override
    public boolean canResize() {
        final long h = handle;
        return h != 0 && !dead && state == State.CONNECTED && SpiceNative.nativeCanResize(h);
    }

    @Override
    public void requestDesktopSize(int width, int height) {
        final long h = handle;
        if (h != 0 && !dead) {
            SpiceNative.nativeRequestDesktopSize(h, width, height);
        }
    }

    @Override
    public void connectionInfo(Consumer<List<ConnectionFact>> callback) {
        final long h = handle;
        if (h == 0 || dead || state != State.CONNECTED) {
            main.post(() -> callback.accept(List.of()));
            return;
        }
        final String[] info = SpiceNative.nativeInfo(h);
        final List<ConnectionFact> facts = new ArrayList<>();
        if (info != null && info.length >= 8) {
            facts.add(ConnectionFact.of(ConnectionFact.Field.DESKTOP_NAME, "Machine", info[0]));
            facts.add(ConnectionFact.of(ConnectionFact.Field.PROTOCOL, "Protocol", info[1]));
            facts.add(ConnectionFact.of(ConnectionFact.Field.CONNECTION, "Connection", info[2]));
            // The row this backend has to be honest in: on a plain port the
            // ticket is encrypted to a key that arrived in the clear a moment
            // earlier, so it is proof against nothing that was listening.
            facts.add(ConnectionFact.of(ConnectionFact.Field.SECURITY, "Security", info[3]));
            if (!info[4].isEmpty()) {
                facts.add(ConnectionFact.of(ConnectionFact.Field.ENCODING, "Encoding", info[4]));
            }
            final long[] traffic = SpiceNative.nativeTraffic(h);
            if (traffic != null && traffic.length == 2) {
                facts.add(ConnectionFact.data(traffic[0], traffic[1]));
            }
            // A session is several connections and what it can do depends on
            // which of them came up, which is a row nothing else here has.
            facts.add(ConnectionFact.of(ConnectionFact.Field.OTHER, "Channels", info[5]));
            // And the one that decides whether there is a clipboard at all.
            facts.add(ConnectionFact.of(ConnectionFact.Field.OTHER, "Guest agent", info[6]));
            if (!info[7].isEmpty()) {
                facts.add(ConnectionFact.diagnostic(ConnectionFact.Field.OTHER, "Picture", info[7]));
            }
        }
        main.post(() -> callback.accept(facts));
    }

    @Override
    public boolean viewOnly() {
        return bool(SpiceProvider.VIEW_ONLY);
    }

    @Override
    public void setOption(String key, String value) {
        options.put(key, value == null ? "" : value);
        final long h = handle;
        if (h == 0 || dead) {
            return;
        }
        switch (key) {
            case SpiceProvider.VIEW_ONLY ->
                    SpiceNative.nativeViewOnly(h, Boolean.parseBoolean(value));
            case SpiceProvider.COMPRESSION -> SpiceNative.nativeSetCompression(h, value);
            default -> {
            }
        }
    }

    // ---- input -------------------------------------------------------------

    @Override
    public void pointer(int x, int y, int buttonMask) {
        final long h = handle;
        if (h != 0) {
            SpiceNative.nativePointer(h, x, y, buttonMask);
        }
    }

    @Override
    public void pointerRelative(int dx, int dy, int buttonMask) {
        final long h = handle;
        if (h != 0) {
            SpiceNative.nativePointerRelative(h, dx, dy, buttonMask);
        }
    }

    @Override
    public boolean pointerIsRelative() {
        final long h = handle;
        return h != 0 && SpiceNative.nativePointerIsRelative(h);
    }

    @Override
    public void keyDown(int keysym, int keyId) {
        final long h = handle;
        if (h != 0) {
            SpiceNative.nativeKeyDown(h, keysym, keyId);
        }
    }

    @Override
    public void keyUp(int keyId) {
        final long h = handle;
        if (h != 0) {
            SpiceNative.nativeKeyUp(h, keyId);
        }
    }

    @Override
    public void releaseAllKeys() {
        final long h = handle;
        if (h != 0) {
            SpiceNative.nativeReleaseAllKeys(h);
        }
    }

    @Override
    public void focus(boolean focused) {
        final long h = handle;
        if (h != 0) {
            SpiceNative.nativeFocus(h, focused);
        }
    }

    /**
     * Offered rather than sent: the agent takes an announcement that this end
     * has text and asks for it when something over there pastes. A guest with
     * no agent takes neither, and the panel says so.
     */
    @Override
    public void clipboardToRemote(String text) {
        final long h = handle;
        if (h != 0 && text != null && !text.isEmpty()) {
            SpiceNative.nativeClipboard(h, text);
        }
    }

    // ---- pixels ------------------------------------------------------------

    @Override
    public boolean readRegion(int x, int y, int width, int height,
                              Bitmap dst, int dstX, int dstY) {
        if (dst == null || dst.getConfig() != Bitmap.Config.ARGB_8888) {
            return false;
        }
        synchronized (pixels) {
            return handle != 0
                    && SpiceNative.nativeReadRegion(handle, x, y, width, height, dst, dstX, dstY);
        }
    }

    @Override
    public Bitmap thumbnail(int maxWidth, int maxHeight) {
        final int w = desktopWidth, h = desktopHeight;
        if (w <= 0 || h <= 0 || maxWidth <= 0 || maxHeight <= 0) {
            return null;
        }
        final int step = Math.max(1, Math.max(
                (w + maxWidth - 1) / maxWidth, (h + maxHeight - 1) / maxHeight));
        final int tw = Math.max(1, (w + step - 1) / step);
        final int th = Math.max(1, (h + step - 1) / step);
        final Bitmap out = Bitmap.createBitmap(tw, th, Bitmap.Config.ARGB_8888);
        final boolean ok;
        synchronized (pixels) {
            ok = handle != 0 && SpiceNative.nativeReadThumbnail(handle, step, out);
        }
        if (!ok) {
            out.recycle();
            return null;
        }
        return out;
    }

    // ---- Callbacks (session thread) ----------------------------------------

    @Override
    public void onConnected(int width, int height) {
        Log.i(TAG, "connected: " + width + "x" + height);
        desktopWidth = width;
        desktopHeight = height;
        main.post(() -> {
            if (dead) {
                return;
            }
            state = State.CONNECTED;
            if (listener != null) {
                listener.desktopSize(width, height);
            }
            fireState(State.CONNECTED, null);
            // Offer what is on the phone's clipboard now, so a paste works
            // without leaving the app and coming back. The agent may not have
            // said it takes one yet; the offer waits for that rather than being
            // dropped.
            final Listener l = listener;
            if (l != null) {
                clipboardToRemote(l.clipboardForRemote());
            }
        });
    }

    @Override
    public void onDesktopSize(int width, int height) {
        Log.i(TAG, "desktop size: " + width + "x" + height);
        desktopWidth = width;
        desktopHeight = height;
        main.post(() -> {
            if (listener != null) {
                listener.desktopSize(width, height);
            }
        });
    }

    @Override
    public void onDamage(int x, int y, int width, int height) {
        final Listener l = listener;
        if (l != null) {
            l.damaged(x, y, width, height);
        }
    }

    @Override
    public void onFrameEnd() {
        final Listener l = listener;
        if (l != null) {
            l.frameEnd();
        }
    }

    @Override
    public void onCursor(int[] argb, int width, int height, int hotX, int hotY, long hash) {
        final Bitmap shape = cursors.shape(hash, argb, width, height);
        main.post(() -> {
            if (listener != null) {
                listener.cursor(shape, hotX, hotY);
            }
        });
    }

    @Override
    public void onPointerMode(boolean relative) {
        main.post(() -> {
            if (listener != null) {
                listener.pointerMode(relative);
            }
        });
    }

    @Override
    public void onBell() {
    }

    @Override
    public void onClipboard(String text) {
        main.post(() -> {
            if (listener != null) {
                listener.clipboardFromRemote(text);
            }
        });
    }

    @Override
    public void onCredentialsNeeded(boolean needsUserName) {
        Log.i(TAG, "the server wants a ticket");
        main.post(() -> {
            final Prompt.Handler h = prompts;
            // No user name and no domain: a ticket is the whole of what a SPICE
            // server asks for.
            final Prompt.Credentials prompt = new Prompt.Credentials(address, "", false,
                    "", true, "", "") {
                @Override
                protected void deliver(boolean ok, String user, String secret) {
                    final long session = handle;
                    if (session != 0) {
                        SpiceNative.nativeAnswerPassword(session, ok ? secret : null);
                    }
                }
            };
            if (h == null) {
                prompt.cancel();
            } else {
                h.credentials(prompt);
            }
        });
    }

    @Override
    public void onTrustNeeded(String fingerprint) {
        if (KnownHosts.matches(context, address, fingerprint)) {
            Log.i(TAG, "certificate matches the pin for " + address);
            answerTrust(true);
            return;
        }
        main.post(() -> KnownHosts.ask(context, address, fingerprint, prompts, this::answerTrust));
    }

    /**
     * Never called. A TLS session has a certificate to ask about and a plain one
     * has nothing at all — no identity that could be checked, and none that
     * could be missing — so there is no third case where a far end is offered
     * and cannot be verified.
     */
    @Override
    public void onUnverified(String why) {
    }

    private void answerTrust(boolean accept) {
        final long h = handle;
        if (h != 0) {
            SpiceNative.nativeAnswerTrust(h, accept);
        }
    }

    @Override
    public void onClosed(String detail) {
        Log.i(TAG, "closed: " + (detail == null || detail.isEmpty() ? "disconnected" : detail));
        closed(detail == null || detail.isEmpty() ? null : detail);
    }

    // ---- plumbing ----------------------------------------------------------

    private boolean tls() {
        return SpiceProvider.TLS.equals(options.get(SpiceProvider.SECURITY));
    }

    private boolean bool(String key) {
        return Boolean.parseBoolean(options.get(key));
    }

    private void closed(String detail) {
        onMain(() -> {
            if (state == State.CLOSED) {
                return;
            }
            state = State.CLOSED;
            fireState(State.CLOSED, detail);
        });
    }

    private void fireState(State s, String detail) {
        onMain(() -> {
            if (listener != null) {
                listener.state(s, detail);
            }
        });
    }

    private void onMain(Runnable r) {
        if (Looper.myLooper() == Looper.getMainLooper()) {
            r.run();
        } else {
            main.post(r);
        }
    }
}
