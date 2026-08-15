// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend.freerdp;

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

import java.io.File;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.function.Consumer;

/**
 * A {@link Backend} on FreeRDP, through the shim in {@code src/main/cpp}.
 *
 * <p>The sixth implementation behind this seam and the second of one protocol,
 * which is the point of it: RFB has had three clients to read a disagreement
 * against since stage 23a, and RDP has had one — long enough for a graphics
 * pipeline to be built against that one, fail, and be rolled back with nothing
 * able to say whether the library or the server was wrong.
 *
 * <p>Where it differs from {@code IronRdpBackend}, the library is why:
 *
 * <ul>
 *   <li><b>Credentials are asked for by the library.</b> The other client is
 *       told them once and fails if they are wrong; FreeRDP asks again through
 *       its own callback, so a mistyped password is a second prompt rather than
 *       a closed session.
 *   <li><b>The certificate has two questions.</b> FreeRDP keeps a store of its
 *       own and asks a different one when what is on file disagrees. Neither
 *       answer comes from that store here — {@link KnownHosts} is what decides —
 *       but the second callback has to exist, because leaving it unset is a
 *       refusal.
 *   <li><b>The graphics pipeline is a value rather than an absence.</b>
 * </ul>
 */
public final class FreeRdpBackend implements Backend, FreeRdpNative.Callbacks {

    static final String TAG = "FreeRdp";

    private static final int CONNECT_TIMEOUT_MS = 20_000;

    /**
     * US English, and not a choice — see {@link FreeRdpProvider}: the scancode
     * table the shim sends is a US layout, so the layout the server is told to
     * use has to be the same one.
     */
    private static final int KEYBOARD_LAYOUT = 0x0409;

    private final Handler main = new Handler(Looper.getMainLooper());

    /** The remote cursor's shapes. Touched only by the protocol thread. */
    private final CursorCache cursors = new CursorCache();
    private final Context context;
    private final String address;
    private final String userName;
    private final String domain;
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

    public FreeRdpBackend(Context context, String address, String userName, String password,
                          Map<String, String> extraOptions) {
        this.context = context.getApplicationContext();
        this.address = address;
        final String[] split = splitDomain(userName);
        this.domain = split[0];
        this.userName = split[1];
        // Empty is not a password: something that looks like a stored secret
        // and is not one fails the connection instead of asking for the real one.
        this.password = password == null || password.isEmpty() ? null : password;
        this.options = new ConcurrentHashMap<>();
        for (var option : FreeRdpProvider.OPTIONS) {
            options.put(option.key(), option.defaultValue());
        }
        if (extraOptions != null) {
            options.putAll(extraOptions);
        }
    }

    /**
     * {@code DOMAIN&#92;user}, {@code user@domain} or a bare name, as
     * {@code {domain, user}} — both forms, because both are typed.
     */
    static String[] splitDomain(String value) {
        final String name = value == null ? "" : value.trim();
        final int slash = name.indexOf('\\');
        if (slash >= 0) {
            return new String[]{name.substring(0, slash), name.substring(slash + 1)};
        }
        final int at = name.indexOf('@');
        if (at > 0) {
            return new String[]{name.substring(at + 1), name.substring(0, at)};
        }
        return new String[]{"", name};
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
        final int[] size = FreeRdpProvider.size(context, address,
                options.get(FreeRdpProvider.DESKTOP_SIZE));
        // A directory of ours rather than the library's default, which is under
        // $HOME and does not exist in an app's process: what it keeps there is a
        // certificate store nothing here reads, but one it cannot write is a
        // connection that fails and blames TLS for it.
        final File store = new File(context.getFilesDir(), "freerdp");
        //noinspection ResultOfMethodCallIgnored
        store.mkdirs();
        final long h = FreeRdpNative.nativeCreate(this, address, userName, domain, password,
                options.get(FreeRdpProvider.NLA), bool(FreeRdpProvider.COMPRESSION),
                options.get(FreeRdpProvider.GRAPHICS), options.get(FreeRdpProvider.EXPERIENCE),
                options.get(FreeRdpProvider.SOUND), scale(),
                size[0], size[1], monitorCount(), KEYBOARD_LAYOUT,
                android.os.Build.MODEL, store.getAbsolutePath(), CONNECT_TIMEOUT_MS);
        if (h == 0) {
            closed("Could not start the connection");
            return;
        }
        synchronized (pixels) {
            handle = h;
        }
        FreeRdpNative.nativeViewOnly(h, bool(FreeRdpProvider.VIEW_ONLY));
    }

    @Override
    public void disconnect() {
        final long h = handle;
        if (h != 0) {
            FreeRdpNative.nativeDisconnect(h);
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
            FreeRdpNative.nativeDestroy(h);
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
     * The facts. No desktop name, because RDP has none — but an encoding, unlike
     * the other RDP backend: this library says what the picture is arriving as,
     * and a row omitted is omitted always rather than when it happens to be
     * empty.
     */
    @Override
    public void connectionInfo(Consumer<List<ConnectionFact>> callback) {
        final long h = handle;
        if (h == 0 || dead || state != State.CONNECTED) {
            main.post(() -> callback.accept(List.of()));
            return;
        }
        final String[] info = FreeRdpNative.nativeInfo(h);
        final List<ConnectionFact> facts = new ArrayList<>();
        if (info != null && info.length >= 7) {
            facts.add(ConnectionFact.of(ConnectionFact.Field.PROTOCOL, "Protocol", info[0]));
            facts.add(ConnectionFact.of(ConnectionFact.Field.CONNECTION, "Connection", info[1]));
            facts.add(ConnectionFact.of(ConnectionFact.Field.SECURITY, "Security", info[2]));
            facts.add(ConnectionFact.of(ConnectionFact.Field.ENCODING, "Encoding", info[3]));
            facts.add(ConnectionFact.of(ConnectionFact.Field.LINE_SPEED, "Line speed", info[4]));
            final long[] traffic = FreeRdpNative.nativeTraffic(h);
            if (traffic != null && traffic.length == 2) {
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
        return bool(FreeRdpProvider.VIEW_ONLY);
    }

    /** The interface size, as a percentage the shim will accept. */
    private int scale() {
        try {
            return Integer.parseInt(options.getOrDefault(FreeRdpProvider.SCALE, "100"));
        } catch (NumberFormatException e) {
            return 100;
        }
    }

    private int monitorCount() {
        try {
            return Math.max(1, Math.min(16,
                    Integer.parseInt(options.getOrDefault(FreeRdpProvider.MONITORS, "1"))));
        } catch (NumberFormatException e) {
            return 1;
        }
    }

    @Override
    public boolean canResize() {
        final long h = handle;
        return h != 0 && !dead && state == State.CONNECTED && FreeRdpNative.nativeCanResize(h);
    }

    @Override
    public List<Monitor> monitors() {
        final long h = handle;
        return h != 0 && !dead ? Monitor.fromFlat(FreeRdpNative.nativeMonitors(h)) : List.of();
    }

    @Override
    public void requestDesktopSize(int width, int height) {
        final long h = handle;
        if (h != 0 && !dead) {
            FreeRdpNative.nativeRequestDesktopSize(h, width, height);
        }
    }

    @Override
    public void setOption(String key, String value) {
        options.put(key, value == null ? "" : value);
        final long h = handle;
        if (h == 0 || dead) {
            return;
        }
        if (FreeRdpProvider.VIEW_ONLY.equals(key)) {
            FreeRdpNative.nativeViewOnly(h, Boolean.parseBoolean(value));
        } else if (FreeRdpProvider.MONITORS.equals(key)) {
            FreeRdpNative.nativeSetMonitorCount(h, monitorCount());
        }
        // Everything else is decided in the connection sequence, which has
        // already happened.
    }

    // ---- input -------------------------------------------------------------

    @Override
    public void pointer(int x, int y, int buttonMask) {
        final long h = handle;
        if (h != 0) {
            FreeRdpNative.nativePointer(h, x, y, buttonMask);
        }
    }

    @Override
    public void keyDown(int keysym, int keyId) {
        final long h = handle;
        if (h != 0) {
            FreeRdpNative.nativeKeyDown(h, keysym, keyId);
        }
    }

    @Override
    public void keyUp(int keyId) {
        final long h = handle;
        if (h != 0) {
            FreeRdpNative.nativeKeyUp(h, keyId);
        }
    }

    @Override
    public void releaseAllKeys() {
        final long h = handle;
        if (h != 0) {
            FreeRdpNative.nativeReleaseAllKeys(h);
        }
    }

    @Override
    public void focus(boolean focused) {
        final long h = handle;
        if (h != 0) {
            FreeRdpNative.nativeFocus(h, focused);
        }
    }

    @Override
    public void clipboardToRemote(String text) {
        final long h = handle;
        if (h != 0 && text != null) {
            FreeRdpNative.nativeClipboard(h, text);
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
            return handle != 0 && FreeRdpNative.nativeReadRegion(handle, x, y, width, height,
                    dst, dstX, dstY);
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
            ok = handle != 0 && FreeRdpNative.nativeReadThumbnail(handle, step, out);
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
        DesktopSizes.remember(context, address, width, height);
        main.post(() -> {
            if (dead) {
                return;
            }
            state = State.CONNECTED;
            if (listener != null) {
                listener.desktopSize(width, height);
            }
            fireState(State.CONNECTED, null);
            // Offer what the phone is already holding, so a paste over there
            // works without copying something here first.
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
        DesktopSizes.remember(context, address, width, height);
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
        // Never called; see FreeRdpNative.Callbacks.
    }

    @Override
    public void onBell() {
        // Never called; see FreeRdpNative.Callbacks.
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
        Log.i(TAG, "the connection wants credentials");
        main.post(() -> {
            final Prompt.Handler h = prompts;
            final String shown = domain.isEmpty() ? userName : domain + "\\" + userName;
            final Prompt.Credentials prompt = new Prompt.Credentials(address,
                    "Use DOMAIN\\user for a domain account.", needsUserName, shown,
                    true, "", "") {
                @Override
                protected void deliver(boolean ok, String user, String secret) {
                    final long session = handle;
                    if (session == 0) {
                        return;
                    }
                    final String[] split = splitDomain(
                            user == null || user.isEmpty() ? shown : user);
                    FreeRdpNative.nativeAnswerCredentials(session, split[1], split[0],
                            ok ? secret : null);
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
     * The certificate. The same question about the same machine the other four
     * backends ask, which is the argument for the store being backend-neutral:
     * what is identified is the machine, not what reaches it.
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
            FreeRdpNative.nativeAnswerTrust(h, accept);
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
