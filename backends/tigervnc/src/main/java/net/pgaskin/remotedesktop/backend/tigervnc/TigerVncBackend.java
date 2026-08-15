// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend.tigervnc;

import android.content.Context;
import android.graphics.Bitmap;
import android.os.Handler;
import android.os.Looper;
import android.util.Log;

import net.pgaskin.remotedesktop.backend.Backend;
import net.pgaskin.remotedesktop.backend.ConnectionFact;
import net.pgaskin.remotedesktop.backend.CursorCache;
import net.pgaskin.remotedesktop.backend.Monitor;
import net.pgaskin.remotedesktop.backend.Prompt;

import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.function.Consumer;

/**
 * A {@link Backend} on TigerVNC's own client code.
 *
 * <p>The fourth implementation of the seam and the third RFB one: the client
 * the servers everything here has been measured against were written alongside.
 * What it does differently from the other two:
 *
 * <ul>
 *   <li><b>The encoding is a preference.</b> The client offers everything it
 *       can decode and the server picks, so the panel reports the encoding the
 *       server <em>used</em> rather than the one that was asked for.
 *   <li><b>A key's identity is the library's.</b> The seam's {@code keyId} is
 *       TigerVNC's {@code systemKeyCode}: it keeps the map of held keys itself,
 *       so a release naming the wrong keysym is not a mistake available here.
 *   <li><b>Buttons eight and nine reach the far end</b> where the plain RFB
 *       pointer message has no room for the ninth, because this client speaks
 *       the extended pointer message when the server offers it.
 *   <li><b>The far end's identity is the library's business.</b> It checks the
 *       certificate, keeps its own known-hosts file beside the app's, and asks
 *       in its own words; what the seam carries is the question and the answer.
 *       So {@code KnownHosts} is not consulted for this backend, and forgetting
 *       a host there does not reach it.
 * </ul>
 */
public final class TigerVncBackend implements Backend, TigerVncNative.Callbacks {

    static final String TAG = "TigerVnc";

    /** The same deadline the other two use, and over the handshake only. */
    private static final int CONNECT_TIMEOUT_MS = 20_000;

    private final Handler main = new Handler(Looper.getMainLooper());

    /**
     * The remote cursor's shapes. Touched only by {@link #onCursor}, which is
     * the protocol thread's, so it needs no lock of its own.
     */
    private final CursorCache cursors = new CursorCache();
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

    public TigerVncBackend(Context context, String address, String userName, String password,
                           Map<String, String> extraOptions) {
        TigerVncNative.nativeSetStateDir(context.getFilesDir().getAbsolutePath());
        this.address = address;
        this.userName = userName == null ? "" : userName;
        // Empty is not a password: something that looks like a stored secret
        // and is not one fails the connection instead of asking for the real
        // one.
        this.password = password == null || password.isEmpty() ? null : password;
        this.options = new ConcurrentHashMap<>();
        for (var option : TigerVncProvider.OPTIONS) {
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
        final long h = TigerVncNative.nativeCreate(this, address, userName, password,
                bool(TigerVncProvider.SHARED),
                TigerVncProvider.encoding(options.get(TigerVncProvider.ENCODING)),
                TigerVncProvider.level(options.get(TigerVncProvider.COMPRESSION)),
                TigerVncProvider.quality(options.get(TigerVncProvider.QUALITY)),
                TigerVncProvider.colorLevel(options.get(TigerVncProvider.COLOUR)),
                bool(TigerVncProvider.H264),
                CONNECT_TIMEOUT_MS);
        if (h == 0) {
            closed("Could not start the connection");
            return;
        }
        synchronized (pixels) {
            handle = h;
        }
        TigerVncNative.nativeViewOnly(h, bool(TigerVncProvider.VIEW_ONLY));
    }

    @Override
    public void disconnect() {
        final long h = handle;
        if (h != 0) {
            TigerVncNative.nativeDisconnect(h);
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
            TigerVncNative.nativeDestroy(h);
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
        return h != 0 && !dead && state == State.CONNECTED && TigerVncNative.nativeCanResize(h);
    }

    @Override
    public List<Monitor> monitors() {
        final long h = handle;
        return h != 0 && !dead ? Monitor.fromFlat(TigerVncNative.nativeMonitors(h)) : List.of();
    }

    @Override
    public void requestDesktopSize(int width, int height) {
        final long h = handle;
        if (h != 0 && !dead) {
            TigerVncNative.nativeRequestDesktopSize(h, width, height);
        }
    }

    @Override
    public void connectionInfo(Consumer<List<ConnectionFact>> callback) {
        final long h = handle;
        if (h == 0 || dead || state != State.CONNECTED) {
            main.post(() -> callback.accept(List.of()));
            return;
        }
        final String[] info = TigerVncNative.nativeInfo(h);
        final List<ConnectionFact> facts = new ArrayList<>();
        if (info != null && info.length >= 8) {
            facts.add(ConnectionFact.of(ConnectionFact.Field.DESKTOP_NAME, "Desktop", info[0]));
            facts.add(ConnectionFact.of(ConnectionFact.Field.PROTOCOL, "Protocol", info[1]));
            facts.add(ConnectionFact.of(ConnectionFact.Field.CONNECTION, "Connection", info[2]));
            facts.add(ConnectionFact.of(ConnectionFact.Field.SECURITY, "Security", info[3]));
            facts.add(ConnectionFact.of(ConnectionFact.Field.ENCODING, "Encoding in use", info[4]));
            facts.add(ConnectionFact.of(ConnectionFact.Field.LINE_SPEED, "Line speed", info[5]));
            final long[] traffic = TigerVncNative.nativeTraffic(h);
            if (traffic != null && traffic.length == 2 && traffic[0] >= 0) {
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
        return bool(TigerVncProvider.VIEW_ONLY);
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
            case TigerVncProvider.VIEW_ONLY ->
                    TigerVncNative.nativeViewOnly(h, Boolean.parseBoolean(value));
            case TigerVncProvider.ENCODING, TigerVncProvider.COMPRESSION,
                 TigerVncProvider.QUALITY, TigerVncProvider.COLOUR, TigerVncProvider.H264 ->
                    TigerVncNative.nativeSetPicture(h,
                            TigerVncProvider.encoding(options.get(TigerVncProvider.ENCODING)),
                            TigerVncProvider.level(options.get(TigerVncProvider.COMPRESSION)),
                            TigerVncProvider.quality(options.get(TigerVncProvider.QUALITY)),
                            TigerVncProvider.colorLevel(options.get(TigerVncProvider.COLOUR)),
                            bool(TigerVncProvider.H264));
            default -> {
                // Shared is decided at ClientInit and the bell is read where the
                // bell arrives; neither is a call.
            }
        }
    }

    // ---- input -------------------------------------------------------------

    @Override
    public void pointer(int x, int y, int buttonMask) {
        final long h = handle;
        if (h != 0) {
            TigerVncNative.nativePointer(h, x, y, buttonMask);
        }
    }

    @Override
    public void keyDown(int keysym, int keyId) {
        final long h = handle;
        if (h != 0) {
            TigerVncNative.nativeKeyDown(h, keysym, keyId);
        }
    }

    @Override
    public void keyUp(int keyId) {
        final long h = handle;
        if (h != 0) {
            TigerVncNative.nativeKeyUp(h, keyId);
        }
    }

    @Override
    public void releaseAllKeys() {
        final long h = handle;
        if (h != 0) {
            TigerVncNative.nativeReleaseAllKeys(h);
        }
    }

    @Override
    public void focus(boolean focused) {
        final long h = handle;
        if (h != 0) {
            TigerVncNative.nativeFocus(h, focused);
        }
    }

    @Override
    public void clipboardToRemote(String text) {
        final long h = handle;
        if (h != 0 && text != null) {
            TigerVncNative.nativeClipboard(h, text);
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
            return handle != 0 && TigerVncNative.nativeReadRegion(
                    handle, x, y, width, height, dst, dstX, dstY);
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
            ok = handle != 0 && TigerVncNative.nativeReadThumbnail(handle, step, out);
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
        if (!bool(TigerVncProvider.BELL)) {
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
                        TigerVncNative.nativeAnswerCredentials(session,
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

    @Override
    public void onQuestion(String title, String text) {
        Log.i(TAG, "the client wants an answer: " + title);
        main.post(() -> {
            final Prompt.Handler h = prompts;
            final Prompt.Message prompt = new Prompt.Message(address,
                    title + "\n\n" + text, Prompt.Message.Severity.WARNING, true, "Connect") {
                @Override
                protected void deliver(boolean ok) {
                    final long session = handle;
                    if (session != 0) {
                        TigerVncNative.nativeAnswerQuestion(session, ok);
                    }
                }
            };
            if (h == null) {
                prompt.cancel();
            } else {
                h.message(prompt);
            }
        });
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
