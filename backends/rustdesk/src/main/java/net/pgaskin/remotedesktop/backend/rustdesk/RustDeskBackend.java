// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend.rustdesk;

import android.content.Context;
import android.graphics.Bitmap;
import android.os.Build;
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
import java.util.Arrays;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.function.Consumer;

/**
 * A {@link Backend} on the RustDesk client in {@code src/main/rust}.
 *
 * <p>It is {@code RfbBackend}'s shape, because the seam is what both answer,
 * and three things are different in a way that is about the protocol rather
 * than about this class:
 *
 * <ul>
 *   <li><b>No user name.</b> A peer's permanent password is the whole of what
 *       it asks for.
 *   <li><b>The password is asked for after it is refused</b>, not before. Their
 *       peer answers a login with "Wrong Password" or "Empty Password" and goes
 *       on waiting, so a client asks a person and sends another login on the
 *       same connection — which means a stored password that works is never a
 *       prompt, and one that does not is one prompt rather than a failed
 *       session.
 *   <li><b>Two ways to a machine, and they are not one transport with a
 *       switch on it.</b> By id, a rendezvous server introduces the two ends
 *       and vouches for the peer's key, and the session is encrypted because of
 *       how it got there. By address, the far end has direct access switched on
 *       and the session is plaintext in both directions — which the panel's
 *       security row says in as many words.
 *   <li><b>The pin is silent the first time.</b> A key the rendezvous server
 *       signed for the id that was dialled has already answered the question a
 *       first-sight prompt would ask, so it is stored without one; what is worth
 *       a person's attention is that key changing afterwards.
 * </ul>
 */
public final class RustDeskBackend implements Backend, RustDeskNative.Callbacks {

    static final String TAG = "RustDesk";

    /**
     * Long enough for a peer to answer, short enough that a mistyped address is
     * not a spinner that never stops. It covers the socket and the login
     * exchange, not the person typing a password.
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

    public RustDeskBackend(Context context, String address, String userName, String password,
                           Map<String, String> extraOptions) {
        this.context = context.getApplicationContext();
        this.address = address;
        // Empty is not a password: something that looks like a stored secret
        // and is not one fails the connection instead of asking for the real
        // one.
        this.password = password == null || password.isEmpty() ? null : password;
        this.options = new ConcurrentHashMap<>();
        for (var option : RustDeskProvider.OPTIONS) {
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
        final long h = RustDeskNative.nativeCreate(this, address, byId(),
                options.get(RustDeskProvider.RENDEZVOUS),
                options.get(RustDeskProvider.RENDEZVOUS_KEY),
                password,
                // What the far end shows and logs while this session is up. The
                // phone rather than the app: somebody looking at their own
                // desktop wants to know which device is on it.
                Build.MODEL,
                options.get(RustDeskProvider.QUALITY),
                RustDeskProvider.fps(options.get(RustDeskProvider.FPS)),
                options.get(RustDeskProvider.CODEC),
                bool(RustDeskProvider.LOCK_AFTER),
                CONNECT_TIMEOUT_MS);
        if (h == 0) {
            closed("Could not start the connection");
            return;
        }
        synchronized (pixels) {
            handle = h;
        }
        RustDeskNative.nativeViewOnly(h, bool(RustDeskProvider.VIEW_ONLY));
    }

    @Override
    public void disconnect() {
        final long h = handle;
        if (h != 0) {
            RustDeskNative.nativeDisconnect(h);
        } else {
            closed("Disconnected");
        }
    }

    @Override
    public void destroy() {
        dead = true;
        final long h;
        // Retire it first: a read that has already got past this point holds
        // the framebuffer's lock, and nativeDestroy waits for the protocol
        // thread, not for that.
        synchronized (pixels) {
            h = handle;
            handle = 0;
        }
        if (h != 0) {
            RustDeskNative.nativeDestroy(h);
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
     * The peer's displays, of which it sends one at a time.
     *
     * <p>There is no {@code monitors()} beside this, deliberately: a second
     * display is not part of the picture on screen, so reporting the layout
     * would describe a desktop nobody is looking at. Which one is being sent is
     * the choice {@link #requestDisplay} makes.
     */
    @Override
    public List<Monitor> displays() {
        final long h = handle;
        if (h == 0 || dead || state != State.CONNECTED) {
            return List.of();
        }
        final int[] flat = RustDeskNative.nativeDisplays(h);
        // The current index rides on the end of the same array, so the two
        // cannot disagree about a display that arrived between two calls.
        return flat == null ? List.of() : Monitor.fromFlat(Arrays.copyOf(flat, flat.length - 1));
    }

    @Override
    public int display() {
        final long h = handle;
        if (h == 0 || dead || state != State.CONNECTED) {
            return -1;
        }
        final int[] flat = RustDeskNative.nativeDisplays(h);
        return flat == null || flat.length < 5 ? -1 : flat[flat.length - 1];
    }

    @Override
    public void requestDisplay(int index) {
        final long h = handle;
        if (h != 0 && !dead) {
            RustDeskNative.nativeRequestDisplay(h, index);
        }
    }

    /**
     * Whether the peer offered a list of sizes.
     */
    @Override
    public boolean canResize() {
        final long h = handle;
        return h != 0 && !dead && state == State.CONNECTED && RustDeskNative.nativeCanResize(h);
    }

    @Override
    public void requestDesktopSize(int width, int height) {
        final long h = handle;
        if (h != 0 && !dead) {
            RustDeskNative.nativeRequestDesktopSize(h, width, height);
        }
    }

    @Override
    public void connectionInfo(Consumer<List<ConnectionFact>> callback) {
        final long h = handle;
        if (h == 0 || dead || state != State.CONNECTED) {
            main.post(() -> callback.accept(List.of()));
            return;
        }
        final String[] info = RustDeskNative.nativeInfo(h);
        final List<ConnectionFact> facts = new ArrayList<>();
        if (info != null && info.length >= 8) {
            facts.add(ConnectionFact.of(ConnectionFact.Field.DESKTOP_NAME, "Desktop", info[0]));
            facts.add(ConnectionFact.of(ConnectionFact.Field.PROTOCOL, "Protocol", info[1]));
            facts.add(ConnectionFact.of(ConnectionFact.Field.CONNECTION, "Connection", info[2]));
            // The one row this backend exists to be honest in: there is no
            // encryption in this mode and no option to turn any on.
            facts.add(ConnectionFact.of(ConnectionFact.Field.SECURITY, "Security", info[3]));
            facts.add(ConnectionFact.of(ConnectionFact.Field.ENCODING, "Encoding", info[4]));
            // Their timing message is a round trip rather than a rate, so it is
            // not the line speed row: a number in the wrong row is worse than a
            // row nothing fills.
            facts.add(ConnectionFact.of(ConnectionFact.Field.OTHER, "Round trip", info[5]));
            final long[] traffic = RustDeskNative.nativeTraffic(h);
            if (traffic != null && traffic.length == 2) {
                facts.add(ConnectionFact.data(traffic[0], traffic[1]));
            }
            facts.add(ConnectionFact.diagnostic(ConnectionFact.Field.OTHER, "Peer", info[6]));
            if (!info[7].isEmpty()) {
                facts.add(ConnectionFact.diagnostic(ConnectionFact.Field.OTHER, "Display", info[7]));
            }
        }
        main.post(() -> callback.accept(facts));
    }

    @Override
    public boolean viewOnly() {
        return bool(RustDeskProvider.VIEW_ONLY);
    }

    @Override
    public void setOption(String key, String value) {
        options.put(key, value == null ? "" : value);
        final long h = handle;
        if (h == 0 || dead) {
            return;
        }
        switch (key) {
            case RustDeskProvider.VIEW_ONLY ->
                    RustDeskNative.nativeViewOnly(h, Boolean.parseBoolean(value));
            case RustDeskProvider.QUALITY, RustDeskProvider.FPS, RustDeskProvider.CODEC ->
                    RustDeskNative.nativeSetOptions(h,
                            options.get(RustDeskProvider.QUALITY),
                            RustDeskProvider.fps(options.get(RustDeskProvider.FPS)),
                            options.get(RustDeskProvider.CODEC));
            default -> {
            }
        }
    }

    // ---- input -------------------------------------------------------------

    @Override
    public void pointer(int x, int y, int buttonMask) {
        final long h = handle;
        if (h != 0) {
            RustDeskNative.nativePointer(h, x, y, buttonMask);
        }
    }

    @Override
    public void keyDown(int keysym, int keyId) {
        final long h = handle;
        if (h != 0) {
            RustDeskNative.nativeKeyDown(h, keysym, keyId);
        }
    }

    @Override
    public void keyUp(int keyId) {
        final long h = handle;
        if (h != 0) {
            RustDeskNative.nativeKeyUp(h, keyId);
        }
    }

    @Override
    public void releaseAllKeys() {
        final long h = handle;
        if (h != 0) {
            RustDeskNative.nativeReleaseAllKeys(h);
        }
    }

    @Override
    public void focus(boolean focused) {
        final long h = handle;
        if (h != 0) {
            RustDeskNative.nativeFocus(h, focused);
        }
    }

    @Override
    public void clipboardToRemote(String text) {
        final long h = handle;
        if (h != 0 && text != null) {
            RustDeskNative.nativeClipboard(h, text);
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
                    && RustDeskNative.nativeReadRegion(handle, x, y, width, height, dst, dstX, dstY);
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
            ok = handle != 0 && RustDeskNative.nativeReadThumbnail(handle, step, out);
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
            // without leaving the app and coming back.
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
    }

    @Override
    public void onBell() {
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
        Log.i(TAG, "the peer wants a password");
        main.post(() -> {
            final Prompt.Handler h = prompts;
            // No user name, and no domain: a peer's permanent password is the
            // whole of what it asks for.
            final Prompt.Credentials prompt = new Prompt.Credentials(address, "", false,
                    "", true, "", "") {
                @Override
                protected void deliver(boolean ok, String user, String secret) {
                    final long session = handle;
                    if (session != 0) {
                        RustDeskNative.nativeAnswerPassword(session, ok ? secret : null);
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
     * The peer's long-term key, which only the id path has.
     *
     * <p>A first sight is pinned <em>silently</em>, which is the one place this
     * app does not ask. Everywhere else a fingerprint arrives with nothing
     * behind it — a server's certificate says only that whoever made it made it
     * — so somebody has to be asked once. Here the rendezvous server has signed
     * that this key belongs to the id that was dialled, and the peer has proved
     * it holds the key, so a prompt would be asking a question that has an
     * answer and offering no way to check it. What is worth interrupting for is
     * the key changing under an id that has not, which is what this store is
     * for and which arrives as the loud prompt.
     */
    @Override
    public void onTrustNeeded(String fingerprint) {
        final String peer = pinnedAs();
        if (KnownHosts.matches(context, peer, fingerprint)) {
            answerTrust(true);
            return;
        }
        if (KnownHosts.pinned(context, peer) == null) {
            KnownHosts.pin(context, peer, fingerprint);
            Log.i(TAG, "pinned " + peer);
            answerTrust(true);
            return;
        }
        Log.w(TAG, "the key for " + peer + " has changed");
        main.post(() -> KnownHosts.ask(context, peer, fingerprint, prompts, this::answerTrust));
    }

    /**
     * The machine could not be verified at all, which is a different question
     * from whether an identity is the right one: there is nothing to pin,
     * nothing to compare it against, and going on means going on in the clear.
     *
     * <p>So it is a question rather than a refusal, and it is
     * {@link Prompt.Message} rather than {@link Prompt.Trust}: a trust prompt
     * offers to remember an identity, and the whole of this case is that there
     * is not one. Nothing is remembered and it is asked every time — which is
     * not the prompt-nobody-reads case, because a machine that stops being
     * verifiable is a thing that should not happen twice quietly.
     */
    @Override
    public void onUnverified(String why) {
        Log.w(TAG, "cannot verify " + pinnedAs() + ": " + why);
        main.post(() -> {
            final Prompt.Handler h = prompts;
            final Prompt.Message prompt = new Prompt.Message(pinnedAs(),
                    why + " Connecting anyway leaves the session unencrypted.",
                    Prompt.Message.Severity.WARNING, true, "Connect anyway") {
                @Override
                protected void deliver(boolean ok) {
                    answerTrust(ok);
                }
            };
            if (h == null) {
                prompt.cancel();
            } else {
                h.message(prompt);
            }
        });
    }

    private void answerTrust(boolean accept) {
        final long h = handle;
        if (h != 0) {
            RustDeskNative.nativeAnswerTrust(h, accept);
        }
    }

    /**
     * What the pin is keyed on: the id and the network it means something on.
     *
     * <p>An id is nine digits handed out by a rendezvous server and re-issued
     * when a machine's configuration is wiped, so the same digits on two
     * networks are two machines — and one entry under the bare id would have a
     * self-hosted server's peer and the public network's peer overwriting each
     * other's identity.
     */
    private String pinnedAs() {
        final String server = options.get(RustDeskProvider.RENDEZVOUS);
        return address + "@" + (server == null || server.isBlank()
                ? RustDeskProvider.PUBLIC_NETWORK : server.trim());
    }

    private boolean byId() {
        return RustDeskProvider.BY_ID.equals(options.get(RustDeskProvider.CONNECT_BY));
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
