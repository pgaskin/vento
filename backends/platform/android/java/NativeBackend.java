// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend;

import android.content.Context;
import android.graphics.Bitmap;
import android.os.Build;
import android.os.Handler;
import android.os.Looper;
import android.util.Log;

import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.function.Consumer;

/**
 * A {@link Backend} on any backend but RealVNC's: the backend's
 * {@code api::Backend} adapter, which the Linux client holds too, reached
 * through the one JNI layer in {@code backends/platform/android}. For the four
 * written in Rust that adapter is the protocol crate's own; for the three C
 * ones it is the small crate beside the C library, over the one vtable in
 * {@code vento/backend.h}, linked into the same library.
 *
 * <p>One class serves every native backend: they share the handle, its
 * retirement and the posting to the main thread, and differ only in their
 * natives and a few things that depend on the protocol. The natives are one
 * surface ({@link Natives}), and the protocol-specific parts are
 * {@link Hooks}.
 *
 * <p>A module supplies its natives class (generated, one per module, because
 * each library registers its functions on a class of its own) and its hooks.
 * Everything else is here:
 *
 * <ul>
 *   <li><b>No session thread.</b> RealVNC's core registers its socket on the
 *       calling thread's {@code ALooper}, so every call into it has to be
 *       marshalled onto one {@code Looper} thread. These own their threads, so
 *       they can be called from any thread directly.
 *   <li><b>No connect timeout here.</b> It is in each client, on the socket,
 *       and only over the handshake: a deadline that covers the server but not
 *       the user typing a password.
 *   <li><b>No pixel lock against a resize.</b> Each framebuffer is behind a
 *       reader-writer lock inside its client rather than being a scratch buffer
 *       an unlocked allocator can move under a copy in flight.
 * </ul>
 *
 * <p>What survives is {@link #handle}'s retirement: the drawing thread reads
 * pixels through this object, and {@link Natives#destroy} frees the session,
 * so the handle is taken out of circulation under {@link #pixels} before it is
 * freed.
 *
 * <p>Decisions are not here either, only what is shown: whether a far end's
 * identity matches its pin, whether a first sight of one is pinned without
 * asking, and what an RDP prompt's name starts with are decided by
 * {@code api::trust} and {@code api::credentials}, which the Linux client
 * uses too, applied in the JNI layer. A question arrives here already decided,
 * to be shown; the pins themselves stay in {@link KnownHosts}, which that
 * layer reads and writes through {@link #knownHostsPins} and
 * {@link #knownHostsPin}.
 *
 * <p>Events arrive through the {@code on…} methods below, one at a time and in
 * the session's order, on the thread that raised them: nearly always the
 * protocol thread, sometimes a call into the session (an answer, a close).
 * Damage arrives once per frame, as one array of rectangles handed to the
 * listener one at a time from here: one JNI crossing per frame, not per
 * rectangle.
 */
public final class NativeBackend implements Backend {

    /**
     * The natives every backend's library registers, on a class of the
     * module's own that {@code vento-codegen natives} writes from the JNI table
     * in {@code backends/platform/android/src/surface.rs}.
     *
     * <h2>Threads</h2>
     * <ul>
     *   <li>{@link #readRegion} and {@link #readThumbnail} are called from
     *       whichever thread is drawing, concurrently with everything else.
     *   <li>Everything else may be called from anywhere; a client encodes input
     *       on the calling thread and writes it on a thread of its own, so a
     *       stalled socket cannot reach the main thread.
     * </ul>
     *
     * <h2>The handle</h2>
     * {@link #destroy} frees the session and joins its thread, so nothing may
     * hold a handle across it; {@link NativeBackend} retires the handle under
     * its own lock first. Every other call takes a zero handle as "already
     * gone" and does nothing.
     */
    public interface Natives {

        /** The library's version: the one call that proves the whole path works. */
        String version();

        /**
         * Start connecting. Returns a handle at once, or zero where the session
         * could not even start; everything after this arrives on {@code events}.
         *
         * @param tag        what the library logs under
         * @param password   null to be asked; empty is not a password
         * @param options    key, value, key, value, with the catalog's defaults
         *                   filled in
         * @param screenWidth the window a session gets on this phone, or zero:
         *                   what an RDP {@code DesktopSize=device} asks for
         * @param lastWidth  the size this address's desktop was left at, or zero:
         *                   what an RDP {@code DesktopSize=remote} asks for
         * @param clientName what the far end lists this phone as
         * @param filesDir   where a library may keep what outlives a session
         * @param forceSocketpair relay each connection through a socketpair
         *                   ({@link Backends#forceSocketpair()})
         */
        long create(NativeBackend events, String tag, String address, String userName,
                    String password, String[] options, int screenWidth, int screenHeight,
                    int lastWidth, int lastHeight, String clientName, String filesDir,
                    boolean forceSocketpair);

        /**
         * The answer to {@link #onCredentialsNeeded}: what was typed, with a
         * null password for a cancel, which ends the session. An empty user
         * name is the session's own; an RDP one is split into a domain there.
         */
        void answerCredentials(long handle, String userName, String password);

        /**
         * The answer to {@link #onUnverified}, {@link #onQuestion},
         * {@link #onMessage} or {@link #onSecurityNeeded}.
         */
        void answerTrust(long handle, boolean accept);

        /**
         * The answer to {@link #onTrustNeeded}: the session is answered
         * {@code accept}, and {@code fingerprint} pinned where both are true.
         */
        void answerPin(long handle, String fingerprint, boolean accept, boolean remember);

        void disconnect(long handle);

        /** Ends the session, joins its thread and frees the handle. */
        void destroy(long handle);

        void pointer(long handle, int x, int y, int buttonMask);

        void pointerRelative(long handle, int dx, int dy, int buttonMask);

        void keyDown(long handle, int keysym, long keyId);

        void keyUp(long handle, long keyId);

        void releaseAllKeys(long handle);

        void focus(long handle, boolean focused);

        void clipboard(long handle, String text);

        /** One option on the running session; the backend decides which are live. */
        void setOption(long handle, String key, String value);

        void requestDesktopSize(long handle, int width, int height);

        void requestDisplay(long handle, int index);

        /** Mutes or unmutes the session's sound at once; see {@link Backend#setMuted}. */
        void setMuted(long handle, boolean muted);

        /** Asks the far end for its sound or to stop it; see {@link Backend#setSoundEnabled}. */
        void setSoundEnabled(long handle, boolean enabled);

        /** The debug overlay's sound line, or null; see {@link Backend#soundLine}. */
        String soundLine(long handle);

        /**
         * {@link Facts} in one crossing: width, height, display, flags (1 can
         * resize, 2 view only, 4 pointer relative), the monitor count, then
         * four ints per monitor and four per display. Null once gone.
         */
        int[] facts(long handle);

        /**
         * The connection panel's rows, four strings each: the
         * {@link ConnectionFact.Field}'s name, the label, the value, and
         * {@code "d"} for a diagnostic. Empty until connected.
         */
        String[] info(long handle);

        /** Received and sent, -1 for a direction that is not counted; null for none. */
        long[] traffic(long handle);

        boolean readRegion(long handle, int x, int y, int width, int height,
                           Bitmap dst, int dstX, int dstY);

        /** The whole desktop at {@code 1/step}, nearest neighbour. */
        boolean readThumbnail(long handle, int step, Bitmap dst);
    }

    /**
     * A backend's Java-side differences: the few places where the right
     * behaviour depends on the protocol and the decision is still the app's
     * rather than the session's. Each has the common answer as its default, so
     * a backend says only where it differs.
     */
    public interface Hooks {

        /** The common answer to every question. */
        Hooks NONE = new Hooks() {
        };

        /**
         * {@code {width, height}} the far end was left at last time, for a
         * protocol that has to ask for a size and has no way to ask for
         * "whatever it already is"; null for none.
         */
        default int[] lastDesktopSize() {
            return null;
        }

        /**
         * The far end's desktop is this size now. Protocol thread, as
         * {@link Listener#damaged} is: whatever is done here must be cheap.
         */
        default void desktopSize(int width, int height) {
        }

        /**
         * The line shown after the reason the far end could not be verified:
         * the second half of {@link #onUnverified}'s question. The common one
         * says the connection is not encrypted; a VNC TLS without a
         * certificate is still encrypted, so it says only that the server
         * could not be verified.
         */
        default String unverifiedConsequence() {
            return "The connection is not encrypted.";
        }

        /** Words under a credentials prompt, or empty. */
        default String credentialsHint() {
            return "";
        }
    }

    private final String tag;
    private final Natives natives;
    /** The last credentials question, for the session to withdraw. Main thread. */
    private Prompt.Credentials asking;
    private final Hooks hooks;
    private final Handler main = new Handler(Looper.getMainLooper());

    /**
     * The remote cursor's shapes. Touched only by {@link #onCursor}, which is
     * called one event at a time, so it needs no lock of its own.
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
    // read is in flight, and every other caller (input, a prompt answered from
    // the protocol thread, the panel) reads it without taking it.
    private volatile long handle;

    private volatile State state = State.IDLE;
    private volatile int desktopWidth;
    private volatile int desktopHeight;
    private volatile boolean dead;
    /** What {@link #onSound} last said, and what {@link #setMuted} was told. */
    private volatile boolean soundPlaying;
    private volatile int soundSwitch = SOUND_SWITCH_NONE;
    private volatile boolean soundEnabled = true;
    private volatile boolean muted;

    /**
     * @param tag     what this backend logs under, here and in its library
     * @param catalog the backend's options, whose defaults fill in what the
     *                connection's own options leave out
     */
    public NativeBackend(Context context, String tag, List<BackendOption> catalog,
                         Natives natives, Hooks hooks, String address, String userName,
                         String password, Map<String, String> extraOptions) {
        // The application's, not a screen's: this outlives the activity by
        // design, and it is used for the pin store, the window size and the
        // files directory.
        this.context = context.getApplicationContext();
        this.tag = tag;
        this.natives = natives;
        this.hooks = hooks == null ? Hooks.NONE : hooks;
        this.address = address;
        this.userName = userName == null ? "" : userName;
        // Empty is not a password, as in the RealVNC backend: something that
        // looks like a stored secret and is not one fails the connection
        // instead of asking for the real one.
        this.password = password == null || password.isEmpty() ? null : password;
        this.options = new ConcurrentHashMap<>();
        for (var option : catalog) {
            options.put(option.key(), option.defaultValue());
        }
        if (extraOptions != null) {
            // The map is concurrent, so it takes no nulls: an unset option is
            // an absent key, which is what every reader here expects.
            extraOptions.forEach((k, v) -> {
                if (k != null && v != null) {
                    options.put(k, v);
                }
            });
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
        final String[] flat = new String[options.size() * 2];
        int i = 0;
        for (var e : options.entrySet()) {
            flat[i++] = e.getKey();
            flat[i++] = e.getValue();
        }
        final int[] screen = DesktopSizes.device(context);
        final int[] last = hooks.lastDesktopSize();
        final long h = natives.create(this, tag, address, userName, password, flat,
                screen == null ? 0 : screen[0], screen == null ? 0 : screen[1],
                last == null ? 0 : last[0], last == null ? 0 : last[1],
                // What the far end shows and logs while this session is up.
                // The phone rather than the app: the desktop's owner wants to
                // know which device is connected.
                Build.MODEL, context.getFilesDir().getPath(), Backends.forceSocketpair());
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
            natives.disconnect(h);
        } else {
            closed("Disconnected");
        }
    }

    @Override
    public void destroy() {
        dead = true;
        final long h;
        // Retire it first: a read that has already got past this point is
        // holding the framebuffer's lock, and destroy waits for the protocol
        // thread, not for that.
        synchronized (pixels) {
            h = handle;
            handle = 0;
        }
        if (h != 0) {
            natives.destroy(h);
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
        final int[] f = h != 0 && !dead ? natives.facts(h) : null;
        if (f == null || f.length < 5) {
            return new Facts(desktopWidth, desktopHeight, List.of(), List.of(), -1, false,
                    viewOnly(), false);
        }
        final int monitors = Math.max(0, Math.min(f[4], (f.length - 5) / 4));
        final int split = 5 + monitors * 4;
        return new Facts(f[0], f[1],
                Monitor.fromFlat(java.util.Arrays.copyOfRange(f, 5, split)),
                Monitor.fromFlat(java.util.Arrays.copyOfRange(f, split, f.length)),
                f[2], (f[3] & 1) != 0, (f[3] & 2) != 0, (f[3] & 4) != 0);
    }

    @Override
    public void requestDesktopSize(int width, int height) {
        final long h = handle;
        if (h != 0 && !dead) {
            natives.requestDesktopSize(h, width, height);
        }
    }

    @Override
    public void requestDisplay(int index) {
        final long h = handle;
        if (h != 0 && !dead) {
            natives.requestDisplay(h, index);
        }
    }

    /**
     * The facts, gathered without touching the protocol thread: every client
     * keeps them in a mutex beside the connection, so this is a lock and a
     * copy. Asynchronous all the same, because the interface is.
     *
     * <p>The rows and their words are the backend's; the one row whose words
     * are the app's, {@link ConnectionFact.Field#DATA}, is made here from the
     * raw counts, in the phone's own number format.
     */
    @Override
    public void connectionInfo(Consumer<List<ConnectionFact>> callback) {
        final long h = handle;
        if (h == 0 || dead || state != State.CONNECTED) {
            main.post(() -> callback.accept(List.of()));
            return;
        }
        final String[] info = natives.info(h);
        final List<ConnectionFact> facts = new ArrayList<>();
        for (int i = 0; info != null && i + 3 < info.length; i += 4) {
            final ConnectionFact.Field field;
            try {
                field = ConnectionFact.Field.valueOf(info[i]);
            } catch (IllegalArgumentException e) {
                continue;
            }
            if (field == ConnectionFact.Field.DATA) {
                final long[] traffic = natives.traffic(h);
                if (traffic != null && traffic.length == 2) {
                    facts.add(ConnectionFact.data(traffic[0], traffic[1]));
                }
            } else {
                facts.add(new ConnectionFact(field, info[i + 1], info[i + 2],
                        "d".equals(info[i + 3])));
            }
        }
        main.post(() -> callback.accept(facts));
    }

    @Override
    public boolean viewOnly() {
        return Boolean.parseBoolean(options.get("ViewOnly"));
    }

    @Override
    public boolean soundPlaying() {
        return soundPlaying;
    }

    @Override
    public int soundSwitch() {
        return soundSwitch;
    }

    @Override
    public boolean soundEnabled() {
        return soundEnabled;
    }

    @Override
    public void setSoundEnabled(boolean enabled) {
        final long h = handle;
        if (h != 0 && !dead) {
            // Stored at once, as the session's controls do: the switch's state
            // is the request, and a screen that asks straight after turning it
            // must not see it turned back before the far end's answer arrives.
            // onSound reports the same a moment later.
            if (soundSwitch == SOUND_SWITCH_ENABLE) {
                soundEnabled = enabled;
            }
            natives.setSoundEnabled(h, enabled);
        }
    }

    @Override
    public String soundLine() {
        final long h = handle;
        return h != 0 && !dead ? natives.soundLine(h) : null;
    }

    @Override
    public boolean muted() {
        return muted;
    }

    @Override
    public void setMuted(boolean muted) {
        this.muted = muted;
        final long h = handle;
        if (h != 0 && !dead) {
            natives.setMuted(h, muted);
        }
    }

    @Override
    public void setOption(String key, String value) {
        options.put(key, value == null ? "" : value);
        final long h = handle;
        if (h != 0 && !dead) {
            natives.setOption(h, key, value == null ? "" : value);
        }
    }

    // ---- input -------------------------------------------------------------

    @Override
    public void pointer(int x, int y, int buttonMask) {
        final long h = handle;
        if (h != 0) {
            natives.pointer(h, x, y, buttonMask);
        }
    }

    @Override
    public void pointerRelative(int dx, int dy, int buttonMask) {
        final long h = handle;
        if (h != 0) {
            natives.pointerRelative(h, dx, dy, buttonMask);
        }
    }

    @Override
    public void keyDown(int keysym, int keyId) {
        final long h = handle;
        if (h != 0) {
            natives.keyDown(h, keysym, keyId);
        }
    }

    @Override
    public void keyUp(int keyId) {
        final long h = handle;
        if (h != 0) {
            natives.keyUp(h, keyId);
        }
    }

    @Override
    public void releaseAllKeys() {
        final long h = handle;
        if (h != 0) {
            natives.releaseAllKeys(h);
        }
    }

    @Override
    public void focus(boolean focused) {
        final long h = handle;
        if (h != 0) {
            natives.focus(h, focused);
        }
    }

    @Override
    public void clipboardToRemote(String text) {
        final long h = handle;
        if (h != 0 && text != null && !text.isEmpty()) {
            natives.clipboard(h, text);
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
                    && natives.readRegion(handle, x, y, width, height, dst, dstX, dstY);
        }
    }

    @Override
    public Bitmap thumbnail(int maxWidth, int maxHeight) {
        final int w = desktopWidth, h = desktopHeight;
        if (w <= 0 || h <= 0 || maxWidth <= 0 || maxHeight <= 0) {
            return null;
        }
        // Any integer step. RealVNC's has to be a power of two because its
        // scaler computes its stride as (int)(1/scale) and a float reciprocal
        // does not round-trip; these take the step itself and never see a
        // float.
        final int step = Math.max(1, Math.max(
                (w + maxWidth - 1) / maxWidth, (h + maxHeight - 1) / maxHeight));
        final int tw = Math.max(1, (w + step - 1) / step);
        final int th = Math.max(1, (h + step - 1) / step);
        final Bitmap out = Bitmap.createBitmap(tw, th, Bitmap.Config.ARGB_8888);
        final boolean ok;
        synchronized (pixels) {
            ok = handle != 0 && natives.readThumbnail(handle, step, out);
        }
        if (!ok) {
            out.recycle();
            return null;
        }
        return out;
    }

    // ---- events (one at a time, nearly always the protocol thread's) -------

    void onConnected(int width, int height) {
        Log.i(tag, "connected: " + width + "x" + height);
        desktopWidth = width;
        desktopHeight = height;
        hooks.desktopSize(width, height);
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
            // without leaving the app and coming back. RealVNC's core pulls
            // when it wants one; these clients have to be told.
            final Listener l = listener;
            if (l != null) {
                clipboardToRemote(l.clipboardForRemote());
            }
        });
    }

    void onDesktopSize(int width, int height) {
        Log.i(tag, "desktop size: " + width + "x" + height);
        desktopWidth = width;
        desktopHeight = height;
        hooks.desktopSize(width, height);
        main.post(() -> {
            if (listener != null) {
                listener.desktopSize(width, height);
            }
        });
    }

    /** One frame: every rectangle that changed in it, four ints each. */
    void onFrame(int[] rects) {
        final Listener l = listener;
        if (l == null) {
            return;
        }
        for (int i = 0; i + 3 < rects.length; i += 4) {
            l.damaged(rects[i], rects[i + 1], rects[i + 2], rects[i + 3]);
        }
        l.frameEnd();
    }

    /**
     * @param argb {@code width * height} pixels, {@code Color}-packed; empty
     *             when the server has hidden the pointer
     * @param hash of the pixels, computed where they were filled in: the
     *             identity {@code CursorCache} keeps the built bitmap under,
     *             so an unchanged shape costs no bitmap at all
     */
    void onCursor(int[] argb, int width, int height, int hotX, int hotY, long hash) {
        final Bitmap shape = cursors.shape(hash, argb, width, height);
        main.post(() -> {
            if (listener != null) {
                listener.cursor(shape, hotX, hotY);
            }
        });
    }

    void onPointerMode(boolean relative) {
        main.post(() -> {
            if (listener != null) {
                listener.pointerMode(relative);
            }
        });
    }

    /**
     * The session's sound: its switch ({@code SOUND_SWITCH_*}), whether the
     * far end is asked for its sound, whether it is muted here (which {@link
     * #setMuted} set), and whether a stream plays (from the first stream
     * written to until the last one closes). Kept here as well, for a screen
     * that attaches later.
     */
    void onSound(int soundSwitch, boolean enabled, boolean muted, boolean playing) {
        final boolean started = playing != soundPlaying;
        this.soundSwitch = soundSwitch;
        this.soundEnabled = enabled;
        this.soundPlaying = playing;
        main.post(() -> {
            if (listener != null) {
                listener.soundSwitch(soundSwitch, enabled, playing);
                if (started) {
                    listener.soundPlaying(playing);
                }
            }
        });
    }

    /** Already filtered by {@code AcceptBell}, which the session applies. */
    void onBell() {
        main.post(() -> {
            if (listener != null) {
                listener.bell();
            }
        });
    }

    /** The connecting screen's status line; null clears it. */
    void onStatus(String text) {
        if (text != null) {
            Log.i(tag, "status: " + text);
        }
        main.post(() -> {
            if (listener != null && !dead) {
                listener.status(text);
            }
        });
    }

    void onClipboard(String text) {
        if (text == null) {
            return;
        }
        main.post(() -> {
            if (listener != null) {
                listener.clipboardFromRemote(text);
            }
        });
    }

    /**
     * Credentials are wanted. Answered through {@link Natives#answerCredentials}
     * with what was typed; the session decides what a blank or a domain means
     * for the protocol.
     *
     * @param user what the box starts with: the record's name, with an RDP
     *             domain written into it
     */
    void onCredentialsNeeded(boolean needsUserName, String user) {
        Log.i(tag, "credentials wanted (user name: " + needsUserName + ")");
        main.post(() -> {
            asking = null;
            final Prompt.Handler h = prompts;
            final Prompt.Credentials prompt = new Prompt.Credentials(address,
                    hooks.credentialsHint(), needsUserName,
                    needsUserName ? user : "", true, "", "") {
                @Override
                protected void deliver(boolean ok, String user, String secret) {
                    final long session = handle;
                    if (session != 0) {
                        natives.answerCredentials(session, user == null ? "" : user,
                                ok ? secret : null);
                    }
                }
            };
            asking = prompt;
            if (h == null) {
                prompt.cancel();
            } else {
                h.credentials(prompt);
            }
        });
    }

    /**
     * The session took back its credentials question: the prompt is closed
     * without an answer. Posted, like the question, so it follows it.
     */
    void onPromptWithdrawn() {
        Log.i(tag, "the credentials prompt is withdrawn");
        main.post(() -> {
            final Prompt.Credentials prompt = asking;
            asking = null;
            if (prompt != null) {
                prompt.withdraw();
            }
        });
    }

    /**
     * The far end's identity is not pinned or does not match its pin, and the
     * user has to decide whether to continue: what the JNI layer decided, with
     * the reason. A match, and a first sight that the backend vouches for,
     * never come here: the JNI layer answers them.
     *
     * <p>The important distinction is between "never seen" and "seen, and
     * different now", which {@code identity} carries; a screen warns loudly
     * about the second. Answered through {@link Natives#answerPin}, which pins
     * it when it was accepted with <em>remember</em> ticked.
     *
     * @param key             what it is pinned under: the address as written,
     *                        or a RustDesk id with its network
     * @param identity        a {@link Prompt.Trust.Identity}'s name
     * @param matchingAddress the key it is pinned under elsewhere, or empty
     */
    void onTrustNeeded(String key, String fingerprint, String identity, String matchingAddress) {
        main.post(() -> {
            final Prompt.Handler h = prompts;
            final Prompt.Trust prompt = new Prompt.Trust(key,
                    Prompt.Trust.Encryption.ENCRYPTED, Prompt.Trust.Identity.valueOf(identity),
                    fingerprint, "", matchingAddress.isEmpty() ? null : matchingAddress) {
                @Override
                protected void deliver(boolean accept, boolean remember) {
                    final long session = handle;
                    if (session != 0) {
                        natives.answerPin(session, fingerprint, accept, remember);
                    }
                }
            };
            if (h == null) {
                prompt.cancel();
            } else {
                h.trust(prompt);
            }
        });
    }

    /** The pins, key then fingerprint, for the JNI layer to decide against. */
    String[] knownHostsPins() {
        return KnownHosts.pins(context);
    }

    /** A pin the JNI layer decided on: a first sight, or an answer. */
    void knownHostsPin(String key, String fingerprint) {
        KnownHosts.put(context, key, fingerprint);
    }

    /**
     * The machine could not be verified at all, which is a different question
     * from whether an identity is the right one: there is nothing to pin,
     * nothing to compare it against, and continuing usually means continuing in
     * the clear ({@link Hooks#unverifiedConsequence}).
     *
     * <p>So it is a question rather than a refusal, and it is
     * {@link Prompt.Message} rather than {@link Prompt.Trust}: a trust prompt
     * offers to remember an identity, and in this case there is none. Nothing
     * is remembered and it is asked every time. RustDesk asks it of a peer with
     * no key, and the two third-party VNC clients of a TLS with no certificate;
     * the others have an identity or nothing to ask.
     *
     * @param key what an identity would have been pinned under
     */
    void onUnverified(String key, String why) {
        Log.w(tag, "cannot verify " + key + ": " + why);
        main.post(() -> {
            final Prompt.Handler h = prompts;
            final Prompt.Message prompt = new Prompt.Message(key,
                    why + " " + hooks.unverifiedConsequence(),
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

    /**
     * A question in the library's own words, which only TigerVNC asks: an
     * RSA-AES server key, which its viewer asks about every time without
     * storing it. So it is a {@link Prompt.Message}, not a pin.
     */
    void onQuestion(String title, String text) {
        Log.i(tag, "the library wants an answer: " + title);
        message(title + "\n\n" + text, Prompt.Message.Severity.WARNING, true, "Connect");
    }

    /**
     * Something the library wants shown, with an OK or a yes and a no. A no to
     * a question ends the session; to a plain message it is the only answer.
     *
     * @param severity a {@code VENTO_SEVERITY_*}, which is
     *                 {@link Prompt.Message.Severity}'s order
     */
    void onMessage(String text, int severity, boolean question, String confirmLabel) {
        final var severities = Prompt.Message.Severity.values();
        message(text, severities[Math.max(0, Math.min(severity, severities.length - 1))],
                question, confirmLabel == null ? "" : confirmLabel);
    }

    /**
     * RealVNC's security question, which is asked of its own Java and never
     * comes this way. Refused rather than dropped, so that a backend that did
     * ask it would close instead of waiting for an answer that never comes.
     * Posted, so the handle is there to answer through.
     */
    void onSecurityNeeded(String signature) {
        Log.w(tag, "refusing a security question this layer does not ask: " + signature);
        main.post(() -> answerTrust(false));
    }

    /** Terminal. Empty {@code detail} for an ordinary disconnect. */
    void onClosed(String detail) {
        Log.i(tag, "closed: " + (detail == null || detail.isEmpty() ? "disconnected" : detail));
        closed(detail == null || detail.isEmpty() ? null : detail);
    }

    // ---- plumbing ----------------------------------------------------------

    private void message(String text, Prompt.Message.Severity severity, boolean question,
                         String confirmLabel) {
        main.post(() -> {
            final Prompt.Handler h = prompts;
            final Prompt.Message prompt = new Prompt.Message(address, text, severity, question,
                    confirmLabel) {
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
            natives.answerTrust(h, accept);
        }
    }

    /**
     * Posted even from the main thread, never run in place: the event can be
     * delivered inline, on the thread of a call into the session that made
     * it, and a listener that destroys the session on hearing it would free
     * the handle under that call.
     */
    private void closed(String detail) {
        main.post(() -> {
            if (state == State.CLOSED) {
                return;
            }
            state = State.CLOSED;
            fireState(State.CLOSED, detail);
        });
    }

    private void fireState(State s, String detail) {
        runOnMain(() -> {
            if (listener != null) {
                listener.state(s, detail);
            }
        });
    }

    private void runOnMain(Runnable r) {
        if (Looper.myLooper() == Looper.getMainLooper()) {
            r.run();
        } else {
            main.post(r);
        }
    }
}
