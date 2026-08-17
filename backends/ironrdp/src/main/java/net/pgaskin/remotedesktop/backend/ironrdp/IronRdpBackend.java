// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend.ironrdp;

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
 * A {@link Backend} on IronRDP, through the client in {@code src/main/rust}.
 *
 * <p>The third implementation behind this seam, and the first one that is not
 * VNC. That is the point of it: everything above {@code CursorController} is
 * protocol-free by construction, and that claim had been tested against two
 * libraries speaking the same protocol.
 *
 * <p>Where it differs from {@code RfbBackend}, the protocol is why:
 *
 * <ul>
 *   <li><b>Credentials come first.</b> RDP carries them in the connection
 *       sequence, so a connection with none stored asks before it touches the
 *       network — where the RFB one asks when a server gets round to wanting it.
 *   <li><b>The domain is part of the user name.</b> {@code DOMAIN&#92;user} and
 *       {@code user@domain} are what every RDP client takes and every user
 *       types, so the record needs no third field and neither does the prompt.
 *   <li><b>The desktop size is asked for.</b> There is no desktop at the far end
 *       until this connects; RFB's is whatever size it already was.
 *   <li><b>Nothing is live.</b> RFB can be told to change encoding mid-session;
 *       RDP negotiates its codecs once, so the only live option is the one that
 *       is ours rather than the protocol's — view only.
 * </ul>
 */
public final class IronRdpBackend implements Backend, IronRdpNative.Callbacks {

    static final String TAG = "Rdp";

    private static final int CONNECT_TIMEOUT_MS = 20_000;

    /**
     * US English, and not a choice — see {@code IronRdpProvider} for why: the
     * scancode table this client sends is a US layout, so the layout the server
     * is told to use has to be the same one.
     */
    private static final int KEYBOARD_LAYOUT = 0x0409;

    private final Handler main = new Handler(Looper.getMainLooper());

    /**
     * The remote cursor's shapes. Touched only by {@link #onCursor}, which is
     * the protocol thread's, so it needs no lock of its own.
     */
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
    // Volatile as well as guarded: the lock holds off the free while a pixel
    // read is in flight, and every other caller — input, a prompt answered from
    // the protocol thread, the panel — reads it without taking it.
    private volatile long handle;

    private volatile State state = State.IDLE;
    private volatile int desktopWidth;
    private volatile int desktopHeight;
    private volatile boolean dead;

    public IronRdpBackend(Context context, String address, String userName, String password,
                      Map<String, String> extraOptions) {
        this.context = context.getApplicationContext();
        this.address = address;
        final String[] split = splitDomain(userName);
        this.domain = split[0];
        this.userName = split[1];
        // Empty is not a password, for the reason the RealVNC backend found
        // it first: something that looks like a stored secret and is not one
        // fails the connection instead of asking for the real one.
        this.password = password == null || password.isEmpty() ? null : password;
        this.options = new ConcurrentHashMap<>();
        for (var option : IronRdpProvider.OPTIONS) {
            options.put(option.key(), option.defaultValue());
        }
        if (extraOptions != null) {
            options.putAll(extraOptions);
        }
    }

    /**
     * {@code DOMAIN&#92;user}, {@code user@domain} or a bare name, as
     * {@code {domain, user}}.
     *
     * <p>Both forms, because both are typed: the backslash is what Windows
     * shows and the {@code @} is what a UPN looks like. A bare name means the
     * machine's own accounts, which is the ordinary case for anything not in a
     * domain.
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
        final int[] size = IronRdpProvider.size(context, address, options.get(IronRdpProvider.DESKTOP_SIZE));
        final long h = IronRdpNative.nativeCreate(this, address, userName, domain, password,
                options.get(IronRdpProvider.NLA),
                bool(IronRdpProvider.COMPRESSION) ? "rdp61" : null,
                !"bitmap".equals(options.get(IronRdpProvider.GRAPHICS)),
                options.get(IronRdpProvider.EXPERIENCE),
                size[0], size[1], monitorCount(), KEYBOARD_LAYOUT,
                android.os.Build.MODEL, CONNECT_TIMEOUT_MS);
        if (h == 0) {
            closed("Could not start the connection");
            return;
        }
        synchronized (pixels) {
            handle = h;
        }
        IronRdpNative.nativeViewOnly(h, bool(IronRdpProvider.VIEW_ONLY));
    }

    @Override
    public void disconnect() {
        final long h = handle;
        if (h != 0) {
            IronRdpNative.nativeDisconnect(h);
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
            IronRdpNative.nativeDestroy(h);
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
     * The facts. Two of the panel's rows are simply not here — RDP has no
     * desktop name and no encoding anybody can name from outside the codec
     * negotiation — and {@link ConnectionFact}'s rule says to omit a field the
     * protocol has no concept of rather than print "Not set" for a question it
     * does not answer.
     *
     * <p>Omitted <em>always</em>, not when they happen to be empty: the shape
     * of the panel is a fact about the protocol, and a row that appears against
     * one server and not another is the thing that rule exists to prevent.
     */
    @Override
    public void connectionInfo(Consumer<List<ConnectionFact>> callback) {
        final long h = handle;
        if (h == 0 || dead || state != State.CONNECTED) {
            main.post(() -> callback.accept(List.of()));
            return;
        }
        final String[] info = IronRdpNative.nativeInfo(h);
        final List<ConnectionFact> facts = new ArrayList<>();
        if (info != null && info.length >= 6) {
            facts.add(ConnectionFact.of(ConnectionFact.Field.PROTOCOL, "Protocol", info[0]));
            facts.add(ConnectionFact.of(ConnectionFact.Field.CONNECTION, "Connection", info[1]));
            facts.add(ConnectionFact.of(ConnectionFact.Field.SECURITY, "Security", info[2]));
            facts.add(ConnectionFact.of(ConnectionFact.Field.LINE_SPEED, "Line speed", info[3]));
            final long[] traffic = IronRdpNative.nativeTraffic(h);
            if (traffic != null && traffic.length == 2) {
                facts.add(ConnectionFact.data(traffic[0], traffic[1]));
            }
            facts.add(ConnectionFact.diagnostic(ConnectionFact.Field.SERVER_PIXELS,
                    "Server pixels", info[4]));
            facts.add(ConnectionFact.diagnostic(ConnectionFact.Field.VIEWER_PIXELS,
                    "Viewer pixels", info[5]));
        }
        main.post(() -> callback.accept(facts));
    }

    @Override
    public boolean viewOnly() {
        return bool(IronRdpProvider.VIEW_ONLY);
    }

    /**
     * How many monitors this connection asks for, which is a number in a
     * choice list and so is only a number if nothing has been typed into the
     * file by hand.
     */
    private int monitorCount() {
        try {
            return Math.max(1, Math.min(16,
                    Integer.parseInt(options.getOrDefault(IronRdpProvider.MONITORS, "1"))));
        } catch (NumberFormatException e) {
            return 1;
        }
    }

    @Override
    public boolean canResize() {
        final long h = handle;
        return h != 0 && !dead && state == State.CONNECTED && IronRdpNative.nativeCanResize(h);
    }

    @Override
    public List<Monitor> monitors() {
        final long h = handle;
        return h != 0 && !dead ? Monitor.fromFlat(IronRdpNative.nativeMonitors(h)) : List.of();
    }

    @Override
    public void requestDesktopSize(int width, int height) {
        final long h = handle;
        if (h != 0 && !dead) {
            IronRdpNative.nativeRequestDesktopSize(h, width, height);
        }
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
        if (IronRdpProvider.VIEW_ONLY.equals(key)) {
            IronRdpNative.nativeViewOnly(h, Boolean.parseBoolean(value));
        } else if (IronRdpProvider.MONITORS.equals(key)) {
            // Remembered rather than acted on: the count is what the *next*
            // layout asks for, and a desktop somebody is working on does not
            // get rearranged as a side effect of a settings row moving.
            IronRdpNative.nativeSetMonitorCount(h, monitorCount());
        }
        // Everything else is decided in the connection sequence, which has
        // already happened.
    }

    // ---- input -------------------------------------------------------------

    @Override
    public void pointer(int x, int y, int buttonMask) {
        final long h = handle;
        if (h != 0) {
            IronRdpNative.nativePointer(h, x, y, buttonMask);
        }
    }

    @Override
    public void keyDown(int keysym, int keyId) {
        final long h = handle;
        if (h != 0) {
            IronRdpNative.nativeKeyDown(h, keysym, keyId);
        }
    }

    @Override
    public void keyUp(int keyId) {
        final long h = handle;
        if (h != 0) {
            IronRdpNative.nativeKeyUp(h, keyId);
        }
    }

    @Override
    public void releaseAllKeys() {
        final long h = handle;
        if (h != 0) {
            IronRdpNative.nativeReleaseAllKeys(h);
        }
    }

    @Override
    public void focus(boolean focused) {
        final long h = handle;
        if (h != 0) {
            IronRdpNative.nativeFocus(h, focused);
        }
    }

    @Override
    public void clipboardToRemote(String text) {
        final long h = handle;
        if (h != 0 && text != null) {
            IronRdpNative.nativeClipboard(h, text);
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
                    && IronRdpNative.nativeReadRegion(handle, x, y, width, height, dst, dstX, dstY);
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
            ok = handle != 0 && IronRdpNative.nativeReadThumbnail(handle, step, out);
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
        // Never called; see IronRdpNative.Callbacks.
    }

    @Override
    public void onBell() {
        // Never called; see IronRdpNative.Callbacks. The three VNC backends gate
        // theirs on a per-connection switch, and this one has no such row
        // because there is nothing behind it to switch off.
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
                    IronRdpNative.nativeAnswerCredentials(session, split[1], split[0],
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
     * The certificate. The same question about a different protocol, which is
     * the argument for the store being backend-neutral: what is identified is
     * the machine, not what reaches it.
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

    /** Never called: an RDP server always presents a certificate. */
    @Override
    public void onUnverified(String why) {
    }

    private void answerTrust(boolean accept) {
        final long h = handle;
        if (h != 0) {
            IronRdpNative.nativeAnswerTrust(h, accept);
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
