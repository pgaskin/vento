// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend.rfb;

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
 * A {@link Backend} on the RFB client in {@code src/main/rust}.
 *
 * <p>The point of it, beyond having one at all in the {@code free} flavour, is
 * that the seam gets a second implementation: everything above
 * {@code CursorController} is protocol-free by construction, and that claim
 * had been tested against exactly one library plus a fake desktop.
 *
 * <p>Much of what {@code RealVncBackend} has to do is absent here, and each
 * absence is a fact about the two libraries rather than about this class:
 *
 * <ul>
 *   <li><b>No session thread.</b> RealVNC's core registers its socket on the
 *       calling thread's {@code ALooper}, so every call into it has to be
 *       marshalled onto one {@code Looper} thread. This one owns its threads,
 *       so a call from anywhere is just a call.
 *   <li><b>No connect timeout here.</b> It is in the client, on the socket,
 *       and only over the handshake — a deadline that covers the server but not
 *       the person typing a password, which is the shape a timeout that once
 *       killed a connection under somebody's password prompt asked for.
 *   <li><b>No pixel lock against a resize.</b> The framebuffer is behind a
 *       reader-writer lock inside the client rather than being a scratch buffer
 *       an unlocked allocator can move under a copy in flight.
 * </ul>
 *
 * <p>What survives is {@link #handle}'s retirement: the drawing thread reads
 * pixels through this object, and {@code nativeDestroy} frees the session, so
 * the handle is taken out of circulation under {@link #pixels} before it is
 * freed. That is the same discipline for a milder reason — there is no
 * third-party code left holding a pointer.
 */
public final class RfbBackend implements Backend, RfbNative.Callbacks {

    static final String TAG = "Rfb";

    /**
     * Long enough for a server to finish a handshake, short enough that a
     * mistyped address is not a spinner that never stops. It covers the
     * handshake only — see the class comment.
     */
    private static final int CONNECT_TIMEOUT_MS = 20_000;

    private final Handler main = new Handler(Looper.getMainLooper());

    /**
     * The remote cursor's shapes. Touched only by {@link #onCursor}, which is
     * the protocol thread's, so it needs no lock of its own.
     */
    private final CursorCache cursors = new CursorCache();
    private final Context context;
    private final String address;
    private final String userName;
    private final String password;
    private final Map<String, String> options;

    private Listener listener;
    private Prompt.Handler prompts;

    /** Guards {@link #handle} against being freed under a read in flight. */
    private final Object pixels = new Object();
    // Volatile as well as guarded: the lock holds off the free while a pixel
    // read is in flight, and every other caller — input, a prompt answered from
    // the protocol thread, the panel — reads it without taking it.
    private volatile long handle;

    private volatile State state = State.IDLE;
    private volatile int desktopWidth;
    private volatile int desktopHeight;
    private volatile boolean dead;

    public RfbBackend(Context context, String address, String userName, String password,
                      Map<String, String> extraOptions) {
        // The application's, not a screen's: this outlives the activity by
        // design, and the only thing it is for is the pin store.
        this.context = context.getApplicationContext();
        this.address = address;
        this.userName = userName == null ? "" : userName;
        // Empty is not a password, for the reason the RealVNC backend found
        // it first: something that looks like a stored secret and is not one
        // fails the connection instead of asking for the real one.
        this.password = password == null || password.isEmpty() ? null : password;
        this.options = new ConcurrentHashMap<>();
        for (var option : RfbProvider.OPTIONS) {
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
        final long h = RfbNative.nativeCreate(this, address, userName, password,
                options.get(RfbProvider.SECURITY), bool(RfbProvider.SHARED),
                options.get(RfbProvider.ENCODING),
                RfbProvider.compressLevel(options.get(RfbProvider.COMPRESSION)),
                CONNECT_TIMEOUT_MS);
        if (h == 0) {
            closed("Could not start the connection");
            return;
        }
        synchronized (pixels) {
            handle = h;
        }
        RfbNative.nativeViewOnly(h, bool(RfbProvider.VIEW_ONLY));
    }

    @Override
    public void disconnect() {
        final long h = handle;
        if (h != 0) {
            RfbNative.nativeDisconnect(h);
        } else {
            closed("Disconnected");
        }
    }

    @Override
    public void destroy() {
        dead = true;
        final long h;
        // Retire it first: a read that has already got past this point is
        // holding the framebuffer's lock, and nativeDestroy waits for the
        // protocol thread, not for that.
        synchronized (pixels) {
            h = handle;
            handle = 0;
        }
        if (h != 0) {
            RfbNative.nativeDestroy(h);
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

    @Override
    public Facts facts() {
        final long h = handle;
        final boolean live = h != 0 && !dead;
        final boolean connected = live && state == State.CONNECTED;
        // Live rather than connected, which is what the layout used to be
        // asked under: a rectangle can arrive before the state does.
        final int[] flat = live ? RfbNative.nativeMonitors(h) : null;
        return new Facts(desktopWidth, desktopHeight,
                flat == null ? List.of() : Monitor.fromFlat(flat),
                List.of(), -1,
                connected && RfbNative.nativeCanResize(h),
                viewOnly(),
                live && RfbNative.nativePointerIsRelative(h));
    }

    @Override
    public void requestDesktopSize(int width, int height) {
        final long h = handle;
        if (h != 0 && !dead) {
            RfbNative.nativeRequestDesktopSize(h, width, height);
        }
    }

    /**
     * The facts, gathered without touching the protocol thread: everything here
     * is a string the client keeps in a mutex beside the connection, so this is
     * a lock and a copy.
     *
     * <p>Asynchronous all the same, because the interface is — and because a
     * backend that answers synchronously today teaches the panel a habit the
     * next one cannot keep.
     */
    @Override
    public void connectionInfo(Consumer<List<ConnectionFact>> callback) {
        final long h = handle;
        if (h == 0 || dead || state != State.CONNECTED) {
            main.post(() -> callback.accept(List.of()));
            return;
        }
        final String[] info = RfbNative.nativeInfo(h);
        final List<ConnectionFact> facts = new ArrayList<>();
        if (info != null && info.length >= 8) {
            facts.add(ConnectionFact.of(ConnectionFact.Field.DESKTOP_NAME, "Desktop", info[0]));
            facts.add(ConnectionFact.of(ConnectionFact.Field.PROTOCOL, "Protocol", info[1]));
            // Worth a row only because there are two transports — a socket,
            // and a socket inside TLS. With one it says nothing the address
            // does not.
            facts.add(ConnectionFact.of(ConnectionFact.Field.CONNECTION, "Connection", info[2]));
            facts.add(ConnectionFact.of(ConnectionFact.Field.SECURITY, "Security", info[3]));
            facts.add(ConnectionFact.of(ConnectionFact.Field.ENCODING, "Encoding", info[4]));
            facts.add(ConnectionFact.of(ConnectionFact.Field.LINE_SPEED, "Line speed", info[5]));
            final long[] traffic = RfbNative.nativeTraffic(h);
            if (traffic != null && traffic.length == 2) {
                facts.add(ConnectionFact.data(traffic[0], traffic[1]));
            }
            facts.add(ConnectionFact.diagnostic(ConnectionFact.Field.SERVER_PIXELS,
                    "Server pixels", info[6]));
            facts.add(ConnectionFact.diagnostic(ConnectionFact.Field.VIEWER_PIXELS,
                    "Viewer pixels", info[7]));
        }
        main.post(() -> callback.accept(facts));
    }

    @Override
    public boolean viewOnly() {
        return bool(RfbProvider.VIEW_ONLY);
    }

    @Override
    public void setOption(String key, String value) {
        // The map is concurrent, so it takes no nulls: an option nobody has
        // answered is an absent key, which is what every reader here expects.
        options.put(key, value == null ? "" : value);
        final long h = handle;
        if (h == 0 || dead) {
            return;
        }
        switch (key) {
            case RfbProvider.VIEW_ONLY -> RfbNative.nativeViewOnly(h, Boolean.parseBoolean(value));
            case RfbProvider.ENCODING, RfbProvider.COMPRESSION -> RfbNative.nativeSetEncodings(h,
                    options.get(RfbProvider.ENCODING),
                    RfbProvider.compressLevel(options.get(RfbProvider.COMPRESSION)));
            default -> {
                // Shared is decided at ClientInit and the clipboard switches are
                // read where the clipboard is handled; neither is a call.
            }
        }
    }

    // ---- input -------------------------------------------------------------

    @Override
    public void pointer(int x, int y, int buttonMask) {
        final long h = handle;
        if (h != 0) {
            RfbNative.nativePointer(h, x, y, buttonMask);
        }
    }

    @Override
    public void pointerRelative(int dx, int dy, int buttonMask) {
        final long h = handle;
        if (h != 0) {
            RfbNative.nativePointerRelative(h, dx, dy, buttonMask);
        }
    }

    @Override
    public void keyDown(int keysym, int keyId) {
        final long h = handle;
        if (h != 0) {
            RfbNative.nativeKeyDown(h, keysym, keyId);
        }
    }

    @Override
    public void keyUp(int keyId) {
        final long h = handle;
        if (h != 0) {
            RfbNative.nativeKeyUp(h, keyId);
        }
    }

    @Override
    public void releaseAllKeys() {
        final long h = handle;
        if (h != 0) {
            RfbNative.nativeReleaseAllKeys(h);
        }
    }

    @Override
    public void focus(boolean focused) {
        final long h = handle;
        if (h != 0) {
            RfbNative.nativeFocus(h, focused);
        }
    }

    @Override
    public void clipboardToRemote(String text) {
        final long h = handle;
        if (h != 0 && text != null) {
            RfbNative.nativeClipboard(h, text);
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
                    && RfbNative.nativeReadRegion(handle, x, y, width, height, dst, dstX, dstY);
        }
    }

    @Override
    public Bitmap thumbnail(int maxWidth, int maxHeight) {
        final int w = desktopWidth, h = desktopHeight;
        if (w <= 0 || h <= 0 || maxWidth <= 0 || maxHeight <= 0) {
            return null;
        }
        // Any integer step. RealVNC's had to be a power of two because their
        // scaler computes its stride as (int)(1/scale) and a float reciprocal
        // does not round-trip; ours takes the step itself and never sees a
        // float.
        final int step = Math.max(1, Math.max(
                (w + maxWidth - 1) / maxWidth, (h + maxHeight - 1) / maxHeight));
        final int tw = Math.max(1, (w + step - 1) / step);
        final int th = Math.max(1, (h + step - 1) / step);
        final Bitmap out = Bitmap.createBitmap(tw, th, Bitmap.Config.ARGB_8888);
        final boolean ok;
        synchronized (pixels) {
            ok = handle != 0 && RfbNative.nativeReadThumbnail(handle, step, out);
        }
        if (!ok) {
            out.recycle();
            return null;
        }
        return out;
    }

    // ---- Callbacks (protocol thread) ---------------------------------------

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
            // Push what is on the phone's clipboard now, so a paste works
            // without leaving the app and coming back. RealVNC's core pulls
            // when it wants one; this client has to be told.
            final Listener l = listener;
            if (l != null) {
                final String ours = l.clipboardForRemote();
                if (ours != null && !ours.isEmpty()) {
                    clipboardToRemote(ours);
                }
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
        if (!bool(RfbProvider.BELL)) {
            return;
        }
        main.post(() -> {
            if (listener != null) {
                listener.bell();
            }
        });
    }

    @Override
    public void onClipboard(String text) {
        if (text == null) {
            return;
        }
        main.post(() -> {
            if (listener != null) {
                listener.clipboardFromRemote(text);
            }
        });
    }

    @Override
    public void onCredentialsNeeded(boolean needsUserName) {
        Log.i(TAG, "the server wants credentials (user name: " + needsUserName + ")");
        main.post(() -> {
            final Prompt.Handler h = prompts;
            final Prompt.Credentials prompt = new Prompt.Credentials(address, "", needsUserName,
                    userName, true, "", "") {
                @Override
                protected void deliver(boolean ok, String user, String secret) {
                    final long session = handle;
                    if (session != 0) {
                        RfbNative.nativeAnswerCredentials(session,
                                user == null || user.isEmpty() ? userName : user,
                                ok ? secret : null);
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

    /** The certificate, against what this address was accepted with before. */
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
     * Never called: the security types with no identity in them are the ones
     * this client's TLS stack cannot speak at all, so there is nothing to ask
     * about. The two borrowed VNC clients offer them and do ask.
     */
    @Override
    public void onUnverified(String why) {
    }

    private void answerTrust(boolean accept) {
        final long h = handle;
        if (h != 0) {
            RfbNative.nativeAnswerTrust(h, accept);
        }
    }

    @Override
    public void onClosed(String detail) {
        Log.i(TAG, "closed: " + (detail == null || detail.isEmpty() ? "disconnected" : detail));
        closed(detail == null || detail.isEmpty() ? null : detail);
    }

    // ---- plumbing ----------------------------------------------------------

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
