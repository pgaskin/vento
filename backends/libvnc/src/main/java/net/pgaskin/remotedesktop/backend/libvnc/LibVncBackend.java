// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend.libvnc;

import android.content.Context;
import android.graphics.Bitmap;
import android.os.Handler;
import android.os.Looper;
import android.util.Log;

import net.pgaskin.remotedesktop.backend.Backend;
import net.pgaskin.remotedesktop.backend.ConnectionFact;
import net.pgaskin.remotedesktop.backend.CursorCache;
import net.pgaskin.remotedesktop.backend.Monitor;
import net.pgaskin.remotedesktop.backend.KnownHosts;
import net.pgaskin.remotedesktop.backend.Prompt;

import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.function.Consumer;

/**
 * A {@link Backend} on LibVNCServer's {@code libvncclient}.
 *
 * <p>The third implementation of the seam, and the first written by somebody
 * else: everything the session screen does is unchanged, and what this class
 * has to do differently is a fact about that library rather than about
 * backends.
 *
 * <p>Beside {@code RfbBackend}, which is the same protocol and a client we
 * wrote, the differences that reach this far are all absences:
 *
 * <ul>
 *   <li><b>No pointer mode.</b> libvncclient does not know QEMU's
 *       {@code PointerTypeChange}, so a server that wants to own the cursor
 *       cannot say so and the two default methods of the seam are enough.
 *   <li><b>The certificate prompt is the same one</b>, and pins to the same
 *       store: the library hands the shim a subject, a validity and a SHA-256
 *       once verification has failed against a trust store this build does not
 *       have, and what it does with that is the RFB backend's own answer.
 *   <li><b>One credentials prompt, not a loop.</b> A stored password the server
 *       refuses ends the connection with "Authentication failed"; libvncclient
 *       does not ask again.
 * </ul>
 */
public final class LibVncBackend implements Backend, LibVncNative.Callbacks {

    static final String TAG = "LibVnc";

    /** The same deadline the RFB backend uses, and over the handshake only. */
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

    public LibVncBackend(Context context, String address, String userName, String password,
                         Map<String, String> extraOptions) {
        this.context = context.getApplicationContext();
        this.address = address;
        this.userName = userName == null ? "" : userName;
        // Empty is not a password: something that looks like a stored secret
        // and is not one fails the connection instead of asking for the real
        // one.
        this.password = password == null || password.isEmpty() ? null : password;
        this.options = new ConcurrentHashMap<>();
        for (var option : LibVncProvider.OPTIONS) {
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
        final long h = LibVncNative.nativeCreate(this, address, userName, password,
                bool(LibVncProvider.SHARED),
                LibVncProvider.encodings(options.get(LibVncProvider.ENCODING)),
                LibVncProvider.level(options.get(LibVncProvider.COMPRESSION)),
                LibVncProvider.level(options.get(LibVncProvider.QUALITY)),
                LibVncProvider.colorLevel(options.get(LibVncProvider.COLOUR)),
                CONNECT_TIMEOUT_MS);
        if (h == 0) {
            closed("Could not start the connection");
            return;
        }
        synchronized (pixels) {
            handle = h;
        }
        LibVncNative.nativeViewOnly(h, bool(LibVncProvider.VIEW_ONLY));
    }

    @Override
    public void disconnect() {
        final long h = handle;
        if (h != 0) {
            LibVncNative.nativeDisconnect(h);
        } else {
            closed("Disconnected");
        }
    }

    @Override
    public void destroy() {
        dead = true;
        final long h;
        // Retire it first: a read that has already got past this point holds
        // the framebuffer's reader lock, and nativeDestroy waits for the
        // protocol thread, not for that.
        synchronized (pixels) {
            h = handle;
            handle = 0;
        }
        if (h != 0) {
            LibVncNative.nativeDestroy(h);
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
    public boolean canResize() {
        final long h = handle;
        return h != 0 && !dead && state == State.CONNECTED && LibVncNative.nativeCanResize(h);
    }

    @Override
    public List<Monitor> monitors() {
        final long h = handle;
        return h != 0 && !dead ? Monitor.fromFlat(LibVncNative.nativeMonitors(h)) : List.of();
    }

    @Override
    public void requestDesktopSize(int width, int height) {
        final long h = handle;
        if (h != 0 && !dead) {
            LibVncNative.nativeRequestDesktopSize(h, width, height);
        }
    }

    /**
     * The facts, with one row that means something slightly different here.
     *
     * <p>The other backends' {@code ENCODING} is the encoding the server
     * <em>used</em>, which moves when quality moves; libvncclient never says
     * which encoding decoded a rectangle, so what can be shown is the list it
     * was asked for. The field's identity is ours and the label is the
     * backend's, which is what the label is for — the row lands in the same
     * place and does not claim to be the other thing.
     *
     * <p>There is no line speed because the library measures none.
     */
    @Override
    public void connectionInfo(Consumer<List<ConnectionFact>> callback) {
        final long h = handle;
        if (h == 0 || dead || state != State.CONNECTED) {
            main.post(() -> callback.accept(List.of()));
            return;
        }
        final String[] info = LibVncNative.nativeInfo(h);
        final List<ConnectionFact> facts = new ArrayList<>();
        if (info != null && info.length >= 7) {
            facts.add(ConnectionFact.of(ConnectionFact.Field.DESKTOP_NAME, "Desktop", info[0]));
            facts.add(ConnectionFact.of(ConnectionFact.Field.PROTOCOL, "Protocol", info[1]));
            facts.add(ConnectionFact.of(ConnectionFact.Field.CONNECTION, "Connection", info[2]));
            facts.add(ConnectionFact.of(ConnectionFact.Field.SECURITY, "Security", info[3]));
            facts.add(ConnectionFact.of(ConnectionFact.Field.ENCODING, "Encodings offered",
                    info[4]));
            final long[] traffic = LibVncNative.nativeTraffic(h);
            if (traffic != null && traffic.length == 2 && traffic[0] >= 0) {
                facts.add(ConnectionFact.data(traffic[0], traffic[1]));
            }
            facts.add(ConnectionFact.diagnostic(ConnectionFact.Field.SERVER_PIXELS,
                    "Server pixels", info[5]));
            facts.add(ConnectionFact.diagnostic(ConnectionFact.Field.VIEWER_PIXELS,
                    "Viewer pixels", info[6]));
        }
        main.post(() -> callback.accept(facts));
    }

    @Override
    public boolean viewOnly() {
        return bool(LibVncProvider.VIEW_ONLY);
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
            case LibVncProvider.VIEW_ONLY ->
                    LibVncNative.nativeViewOnly(h, Boolean.parseBoolean(value));
            case LibVncProvider.ENCODING, LibVncProvider.COMPRESSION, LibVncProvider.QUALITY ->
                    LibVncNative.nativeSetEncodings(h,
                            LibVncProvider.encodings(options.get(LibVncProvider.ENCODING)),
                            LibVncProvider.level(options.get(LibVncProvider.COMPRESSION)),
                            LibVncProvider.level(options.get(LibVncProvider.QUALITY)));
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
            LibVncNative.nativePointer(h, x, y, buttonMask);
        }
    }

    @Override
    public void keyDown(int keysym, int keyId) {
        final long h = handle;
        if (h != 0) {
            LibVncNative.nativeKeyDown(h, keysym, keyId);
        }
    }

    @Override
    public void keyUp(int keyId) {
        final long h = handle;
        if (h != 0) {
            LibVncNative.nativeKeyUp(h, keyId);
        }
    }

    @Override
    public void releaseAllKeys() {
        final long h = handle;
        if (h != 0) {
            LibVncNative.nativeReleaseAllKeys(h);
        }
    }

    @Override
    public void focus(boolean focused) {
        final long h = handle;
        if (h != 0) {
            LibVncNative.nativeFocus(h, focused);
        }
    }

    @Override
    public void clipboardToRemote(String text) {
        final long h = handle;
        if (h != 0 && text != null) {
            LibVncNative.nativeClipboard(h, text);
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
                    && LibVncNative.nativeReadRegion(handle, x, y, width, height, dst, dstX, dstY);
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
            ok = handle != 0 && LibVncNative.nativeReadThumbnail(handle, step, out);
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
    public void onBell() {
        if (!bool(LibVncProvider.BELL)) {
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
                        LibVncNative.nativeAnswerCredentials(session,
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

    /**
     * The certificate, against the same store every other backend pins into, so
     * a host reached through any of them is one entry.
     */
    @Override
    public void onTrustNeeded(String fingerprint) {
        if (KnownHosts.matches(context, address, fingerprint)) {
            Log.i(TAG, "certificate matches the pin for " + address);
            answerTrust(true);
            return;
        }
        main.post(() -> KnownHosts.ask(context, address, fingerprint, prompts, this::answerTrust));
    }

    private void answerTrust(boolean accept) {
        final long h = handle;
        if (h != 0) {
            LibVncNative.nativeAnswerTrust(h, accept);
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
