// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend.realvnc;

import android.content.Context;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.Rect;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;
import android.util.Log;

import com.realvnc.vncviewer.jni.ConnectionInfoBindings;
import com.realvnc.vncviewer.jni.SessionBindings;

import net.pgaskin.remotedesktop.backend.Backend;
import net.pgaskin.remotedesktop.backend.Backends;
import net.pgaskin.remotedesktop.backend.ConnectionFact;
import net.pgaskin.remotedesktop.backend.KnownHosts;
import net.pgaskin.remotedesktop.backend.Prompt;

import java.io.File;
import java.nio.ByteBuffer;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.function.Consumer;

/**
 * A {@link Backend} on RealVNC's own viewer core, whose JNI surface is
 * declared in {@code :realvnc-jni}. Most of this class follows from the
 * threading rules that library imposes.
 *
 * <p>Two threads use this object:
 *
 * <ul>
 *   <li><b>The session thread</b> ({@link RealVncCore}) owns every call into
 *       the library, and <em>also delivers every callback</em>, which is not
 *       what the original's posting asymmetry suggests. The core registers its
 *       socket on that thread's {@code ALooper}, which is why the thread must
 *       have a running {@code Looper}, and why long work posted to it delays
 *       the protocol rather than merely queueing behind it.
 *   <li><b>The drawing thread</b> calls {@link #readRegion} directly, as the
 *       original calls {@code copyScaledRegion} straight from its renderer, so
 *       pixel fetches never queue behind connection work.
 * </ul>
 *
 * <p>The app still sees the main thread for everything but damage rectangles:
 * this class guarantees that, not the library.
 *
 * <p>The drawing thread's calls need their own lock, for a more specific reason
 * than shared state: {@code copyScaledRegion} takes a native mutex, but the
 * allocator behind {@code setScaleBufferSize} does not (0x2405e4 rewrites the
 * vector at +0x148 with no lock at all), so a resize racing a copy is a
 * use-after-free inside the library. {@link #pixels} makes the pair mutually
 * exclusive on this side, the only place it can be done.
 */
public final class RealVncBackend implements Backend,
        SessionBindings.DesktopCallback, SessionBindings.SessionCallback {

    private static final String TAG = RealVncCore.TAG;

    private final Handler main = new Handler(Looper.getMainLooper());
    private final Context context;
    private final String address;
    private final String password;
    private final Map<String, String> options;

    private Listener listener;
    private Prompt.Handler prompts;

    private volatile SessionBindings.Session session; // written on the session thread, read everywhere
    private volatile State state = State.IDLE;
    private volatile int desktopWidth;
    private volatile int desktopHeight;

    /**
     * The received count, and how often it is sampled. Session thread only,
     * where both of its callers run.
     */
    private final RealVncTraffic traffic = new RealVncTraffic();
    private static final long SAMPLE_INTERVAL_MS = 1000;
    private long lastSample;

    /** Guards the native scratch buffer against a concurrent resize. */
    private final Object pixels = new Object();
    private ByteBuffer buffer;
    private int bufferWidth;
    private int bufferHeight;

    private volatile boolean dead; // destroy() has run: every native call is a no-op after

    /** {@link RealVncNet#attach}'s, set and detached on the session thread. */
    private volatile int route;
    /** What carries the core's connections, for the Tunnel row; null where they are its own. */
    private volatile String tunnel;

    /** {@link #connect} has started the core, so the session thread exists. */
    private volatile boolean coreStarted;

    /**
     * How long to wait for {@code connSuccess} before giving up. Required: a
     * connect that fails reports <em>nothing</em>. A refused port logs
     * {@code CConnection: close} internally and then the session stays as it
     * is, with no {@code sessionClosed}, no {@code MsgBox}, no status change and
     * no retry, so without a timeout a mistyped address shows a spinner that
     * never stops.
     */
    private static final long CONNECT_TIMEOUT_MS = 20_000;

    private final Runnable connectTimeout = this::onConnectTimeout;

    /**
     * The connect gate. The core faults when two sessions are in their connect
     * phase at once: it brings each session's modules up against process-wide
     * singletons, and a second {@code createSession} reaching that code while
     * the first session is still connecting leaves a freed module in the first
     * session's list, so the next walk of that list calls through a dangling
     * pointer and the process dies inside the core. The core is closed, so this
     * keeps the connect phases from overlapping: at most one session runs
     * {@code createSession} and the connect that follows; the rest wait and
     * begin in turn, oldest first. A reconnect (the core's own, after
     * {@code connSuccess}) does not go through {@code createSession}, so it is
     * not gated. All three fields are touched only on the session thread (every
     * connect body, {@code connSuccess} and {@code closed} runs there), so no
     * lock guards them.
     */
    private static RealVncBackend connecting;
    private static final java.util.ArrayDeque<RealVncBackend> connectWaiters = new java.util.ArrayDeque<>();
    private boolean waitingToConnect;
    /** Read at {@link #connect}, as the other backends read it when they start. */
    private boolean forceSocketpair;

    /**
     * Prompts on screen. The timeout is for a session that has gone silent, and
     * a session waiting for a password has not: it is waiting for the user, who
     * may take longer than twenty seconds to find one. Counted
     * rather than a flag because the security prompt and the password prompt
     * both arrive before {@code connSuccess}, and the second can open while the
     * first is still up. Session thread only.
     */
    private int outstandingPrompts;

    private void onConnectTimeout() {
        if (state == State.CONNECTING) {
            Log.w(TAG, "no connSuccess within " + CONNECT_TIMEOUT_MS + " ms");
            reap(true);
            closed("Could not connect to " + address);
        }
    }

    /** Pauses the connect timeout while a prompt waits for the user. */
    private void promptShown() {
        RealVncCore.post(() -> {
            outstandingPrompts++;
            RealVncCore.cancel(connectTimeout);
        });
    }

    /** Restarts the connect timeout from the beginning once the last prompt is answered. */
    void promptAnswered() {
        RealVncCore.post(() -> {
            outstandingPrompts = Math.max(0, outstandingPrompts - 1);
            if (outstandingPrompts == 0 && state == State.CONNECTING && !dead) {
                RealVncCore.postDelayed(connectTimeout, CONNECT_TIMEOUT_MS);
            }
        });
    }

    public RealVncBackend(Context context, String address, String userName, String password,
                          Map<String, String> extraOptions) {
        this.context = context.getApplicationContext();
        RealVncNet.context(this.context);
        this.address = address;
        // Empty is not a password: the core authenticates with whatever it is
        // given, so "" fails the connection outright instead of asking. Null
        // tells it there is no stored password.
        this.password = password == null || password.isEmpty() ? null : password;
        this.options = new LinkedHashMap<>();

        // Every offered parameter at its default, then the caller's stored
        // values over them. The list, the defaults and the wording all live
        // in RealVncProvider, so the settings screen and the connect path cannot
        // drift apart.
        options.putAll(RealVncProvider.defaults());
        if (userName != null && !userName.isEmpty()) {
            options.put("UserName", userName);
        }
        if (extraOptions != null) {
            options.putAll(extraOptions);
        }
        // The identity this server was last accepted with. The core's own
        // security prompt is silent for an identity it recognises, the same
        // "accept once, warn if it changes" rule the other backends get from
        // api::trust over KnownHosts; the difference is that this is an opaque
        // blob the core makes rather than a fingerprint other code could
        // compare.
        final String identity = identities(context).getString(address == null ? "" : address, null);
        if (identity != null && !options.containsKey("Identity")) {
            options.put("Identity", identity);
        }
        // Whether the colour depth was chosen for this connection. The store
        // keeps only values that differ from the default, so the key's presence
        // means it was set, and a depth that was set must not be changed by the
        // quality control.
        this.colorLevelPinned = extraOptions != null && extraOptions.containsKey("ColorLevel");
        if (!colorLevelPinned) {
            options.put("ColorLevel",
                    RealVncProvider.colorLevelFor(options.get("Quality")));
        }
    }

    /** See the constructor. */
    private boolean colorLevelPinned;

    /** For the prompts, which name the machine a question is about. */
    String address() {
        return address;
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
        // Read now, as the other backends read it when they start.
        forceSocketpair = Backends.forceSocketpair();
        RealVncCore.start(context);
        coreStarted = true;
        onMain(() -> fireState(State.CONNECTING, connectingTo()));
        RealVncCore.post(() -> {
            if (dead) {
                return;
            }
            // One session's connect at a time: begin now, or wait for the one
            // in flight to finish and be started from finishConnect().
            if (connecting != null && connecting != this) {
                connectWaiters.add(this);
                waitingToConnect = true;
                Log.i(TAG, "waiting for another RealVNC session to finish connecting");
                return;
            }
            connecting = this;
            beginConnect();
        });
    }

    /**
     * The connect itself, on the session thread, once this session holds the
     * gate (see {@link #connecting}): the timeout armed, the route attached,
     * and {@code createSession} called.
     */
    private void beginConnect() {
        RealVncCore.postDelayed(connectTimeout, CONNECT_TIMEOUT_MS);
        // Before createSession, after which the core's first connection
        // follows at once.
        final Map<String, String> sessionOptions = new HashMap<>(options);
        final boolean udp = Boolean.parseBoolean(sessionOptions.getOrDefault("EnableUdpRfb", "true"));
        try {
            route = RealVncNet.attach(address, (int) CONNECT_TIMEOUT_MS, forceSocketpair);
        } catch (java.io.IOException e) {
            // Never dialled directly, or for another session, instead.
            Log.w(TAG, "not started: " + e.getMessage());
            RealVncCore.cancel(connectTimeout);
            closed("RealVNC cannot connect through this connection's tunnel: " + e.getMessage());
            return;
        }
        if (route > 0) {
            tunnel = RealVncNet.tunnel(forceSocketpair);
            Log.i(TAG, "the core's connections go through the session's network"
                    + (udp ? "" : " (UDP off)"));
        }
        final long t0 = System.nanoTime();
        RealVncPrompts.connecting(this);
        SessionBindings.Session s = null;
        try {
            s = SessionBindings.createSession(this, this, address, password,
                    sessionOptions);
        } catch (Throwable t) {
            Log.e(TAG, "createSession failed", t);
        } finally {
            RealVncPrompts.connected(this);
        }
        // It returns in a few ms and the connect proceeds as callbacks on
        // this same thread, so what follows runs before the connection is
        // up. Logged rather than assumed, because everything below depends
        // on it.
        Log.i(TAG, "createSession returned after "
                + (System.nanoTime() - t0) / 1_000_000 + " ms on "
                + thread() + " -> " + s);
        if (s == null) {
            closed("Could not start the connection");
            return;
        }
        session = s;
        RealVncPrompts.attach(s, this);
        synchronized (pixels) {
            resizeBuffer(256, 256);
        }
        SessionBindings.focusEvent(s, true);
    }

    /**
     * Releases the connect gate once this session's connect has settled
     * (succeeded in {@link #connSuccess}, or ended in {@link #closed}) and
     * starts the next waiter. A no-op for a session that was not the one
     * connecting: one already connected and ending later, or one closed
     * before its connect reached the session thread. A waiter torn down before
     * it began drops itself from the queue here. Called again for a session
     * already settled (closed, then destroyed), it does nothing.
     *
     * <p>The gate state is touched only on the session thread, so the work is
     * run there: {@link #connSuccess} and most {@link #closed} callers are
     * already on it (the body then runs inline), but {@code disconnect} calls
     * {@code closed} on its own thread. A session that never started the core
     * never entered the gate, so there is nothing to release.
     */
    private void finishConnect() {
        if (!coreStarted) {
            return;
        }
        RealVncCore.post(() -> {
            if (waitingToConnect) {
                connectWaiters.remove(this);
                waitingToConnect = false;
                return;
            }
            if (connecting != this) {
                return;
            }
            connecting = null;
            // Start the next waiter in a later turn of the session thread's
            // loop, not here: begun inline it would run inside this session's
            // connSuccess or closed callback chain, while the core is still
            // unwinding the first session's modules, which is the overlap it
            // faults on. A waiter closed or destroyed before this runs has
            // removed itself or is skipped here, so only a live session begins.
            RealVncCore.postToLoop(() -> {
                if (connecting != null) {
                    return; // a session started in the meantime holds the gate
                }
                RealVncBackend next;
                do {
                    next = connectWaiters.poll();
                } while (next != null && next.dead);
                if (next == null) {
                    return;
                }
                next.waitingToConnect = false;
                connecting = next;
                next.beginConnect();
            });
        });
    }

    /**
     * Closes the session <em>and</em> retires the handle, which prevents a
     * second close.
     *
     * <p>The handle is usually retired by the core's own {@code sessionClosed},
     * which arrives on the session thread some time later, so a screen that
     * leaves immediately after asking to disconnect (as the Back gesture does)
     * reaches {@link #destroy} while the handle is still live and closes it
     * again. A second {@code closeSession} is a <b>SIGSEGV inside the core</b>
     * rather than a reported error, and it takes the process with it, so the
     * app disappears just as the connection list comes up.
     */
    @Override
    public void disconnect() {
        // First, from this thread: a dial through the network in progress
        // ends at once, its connect left in progress, as the core leaves a
        // direct one it closes.
        RealVncNet.cancel(route);
        if (session != null) {
            reap(true);
        } else {
            closed("Disconnected");
        }
    }

    @Override
    public void destroy() {
        dead = true;
        RealVncNet.cancel(route);
        reap(true);
        // After the close reap posted, on the same thread, where the route
        // is attached: posted whenever connect() has started the core, since
        // its attach may still be queued.
        if (coreStarted) {
            RealVncCore.post(() -> {
                RealVncNet.detach(route);
                route = 0;
            });
        }
        // The app destroys a session without disconnecting it first, and
        // nothing calls closed() for one waiting to connect (it has no core
        // session to close), so it leaves the connect gate here, as a timed
        // out connect does in closed(). Left queued, it would be begun later
        // with no window, connect, and hold the gate.
        finishConnect();
    }

    /**
     * Retires the handle: nothing new can be posted for it, and what is already
     * queued is skipped by {@link #onSession}'s identity check.
     *
     * <p><b>Nothing is ever freed.</b> {@code destroySession} exists, and calling
     * it on a session the core has closed is a SIGSEGV inside the core: the
     * same fault, at the same offset, as closing one twice. The core tears its
     * session down in {@code sessionClosed} and what is left is a handle that
     * may only be dropped, which is exactly what the original does: it never
     * calls {@code destroySession} at all and lets the native registry keep
     * every session the process ever opened. That is a leak in the core; the
     * alternative is a crash at the end of every connection.
     */
    private void reap(boolean close) {
        RealVncCore.cancel(connectTimeout);
        final SessionBindings.Session s = session;
        if (s == null) {
            return;
        }
        session = null;
        RealVncPrompts.detach(s);
        final boolean doClose = close;
        RealVncCore.post(() -> {
            // Holds the pixel lock as well as running on the session thread: a
            // read in progress on the drawing thread must not be using the
            // buffer this frees, and the native allocator does not lock (see
            // the class comment).
            synchronized (pixels) {
                buffer = null;
                bufferWidth = 0;
                bufferHeight = 0;
                if (scratch != null) {
                    scratch.recycle();
                    scratch = null;
                    scratchCanvas = null;
                }
                try {
                    // Not after the core has reported it closed: it tears the
                    // session down in `sessionClosed`, and a close after that
                    // faults in the same place a second one does.
                    if (doClose && state != State.CLOSED) {
                        SessionBindings.closeSession(s);
                    }
                } catch (Throwable t) {
                    Log.w(TAG, "closing session", t);
                }
            }
        });
    }

    /**
     * Runs something on the session thread with the live handle, or returns false
     * if there is none. The handle is captured now and re-checked then, so a
     * call posted before {@link #reap} runs after it is dropped rather than
     * reaching a freed session.
     */
    private boolean onSession(java.util.function.Consumer<SessionBindings.Session> what) {
        final SessionBindings.Session s = session;
        if (s == null || dead) {
            return false;
        }
        RealVncCore.post(() -> {
            if (session == s) {
                what.accept(s);
            }
        });
        return true;
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
     * Only the size and the view-only state: this library exposes no monitor
     * layout, offers no second display, and its {@code ServerResizeSupported} is
     * not reachable through this backend's bindings. Reported anyway rather than
     * left to a default, because facts that disagreed with {@link #viewOnly()}
     * would be worse than facts that say little.
     */
    @Override
    public Facts facts() {
        return new Facts(desktopWidth, desktopHeight, List.of(), List.of(), -1,
                false, viewOnly(), false);
    }

    /**
     * Everything {@code ConnectionInfoBindings} reports, gathered on the session
     * thread and returned on the main one.
     *
     * <p>Each is a string the library formats itself, and two of them change
     * over time: {@code getLastUsedEncoding} follows the quality group and
     * {@code getLineSpeed} is a running measurement. So the panel polls rather
     * than taking one snapshot.
     */
    @Override
    public void connectionInfo(Consumer<List<ConnectionFact>> callback) {
        final SessionBindings.Session s = session;
        if (s == null || dead || state != State.CONNECTED) {
            main.post(() -> callback.accept(List.of()));
            return;
        }
        RealVncCore.post(() -> {
            final List<ConnectionFact> facts = new ArrayList<>();
            if (!dead && session == s) {
                // Sampled here as well as on every update, since an idle
                // desktop can be seconds past the last one.
                traffic.sample(s);
                facts.add(ConnectionFact.of(ConnectionFact.Field.DESKTOP_NAME, "Desktop",
                        ConnectionInfoBindings.getDesktopName(s)));
                facts.add(ConnectionFact.of(ConnectionFact.Field.PROTOCOL, "Protocol",
                        ConnectionInfoBindings.getProtoVersion(s)));
                facts.add(ConnectionFact.of(ConnectionFact.Field.CONNECTION, "Connection",
                        connectionRow(ConnectionInfoBindings.getConnectionType(s), route > 0)));
                facts.add(ConnectionFact.of(ConnectionFact.Field.SECURITY, "Security",
                        ConnectionInfoBindings.getSecurityDesc(s)));
                facts.add(ConnectionFact.of(ConnectionFact.Field.ENCODING, "Encoding",
                        ConnectionInfoBindings.getLastUsedEncoding(s)));
                facts.add(ConnectionFact.of(ConnectionFact.Field.LINE_SPEED, "Line speed",
                        ConnectionInfoBindings.getLineSpeed(s)));
                // Received only, and no row at all without a trustworthy
                // number: the core counts one direction and has no count of
                // the other, and reporting zero would claim nothing was sent.
                final long received = traffic.received();
                if (received >= 0) {
                    facts.add(ConnectionFact.data(received, -1));
                }
                facts.add(ConnectionFact.diagnostic(ConnectionFact.Field.SERVER_PIXELS,
                        "Server pixels", ConnectionInfoBindings.getServerPixelFormat(s)));
                facts.add(ConnectionFact.diagnostic(ConnectionFact.Field.VIEWER_PIXELS,
                        "Viewer pixels", ConnectionInfoBindings.getViewerPixelFormat(s)));
                // Last, as the native layer adds it for the other backends.
                final String carried = tunnel;
                if (carried != null) {
                    facts.add(ConnectionFact.of(ConnectionFact.Field.TUNNEL, "Tunnel", carried));
                }
            }
            main.post(() -> callback.accept(facts));
        });
    }

    /**
     * The core's "Connection" row for a session whose connections go through
     * a network that is not direct: the core reports its own socket ("Direct
     * TCP"), which the network carries, so "Direct" is dropped and the Tunnel
     * row says what carries it. As {@code connectionRow} in
     * {@code realvnc_session.cxx}.
     */
    static String connectionRow(String core, boolean relayed) {
        if (!relayed || core == null || !core.startsWith("Direct ")) {
            return core;
        }
        return core.substring("Direct ".length()) + " (tunnel)";
    }

    @Override
    public boolean viewOnly() {
        return Boolean.parseBoolean(options.get("ViewOnly"));
    }

    /**
     * A live option change. {@link RealVncProvider#needsApply} decides which
     * keys need the second call; the interface does not carry it, because
     * "re-read the parameters that affect the encoder" is a fact about this
     * library and not about remote desktops.
     *
     * <p>With no session yet, the value joins the map {@link #connect} passes to
     * {@code createSession}.
     */
    @Override
    public void setOption(String key, String value) {
        final SessionBindings.Session s = session;
        if (s == null) {
            options.put(key, value);
            return;
        }
        options.put(key, value);
        // Same rule as the constructor's: a depth set explicitly, before the
        // session or during it, is kept.
        colorLevelPinned |= "ColorLevel".equals(key);
        final boolean group = RealVncProvider.needsApply(key);
        // The quality group is always sent together: the core re-reads the
        // three on applyOptions, and together they decide which encodings and
        // which pixel format to request. Colour depth first, quality last, the
        // order found to work.
        final Map<String, String> alsoSet = new LinkedHashMap<>();
        if (group) {
            // The depth follows the quality unless this connection chose one
            // (RealVncProvider.colorLevelFor). Only a quality change moves it,
            // so a depth set in the panel is the one sent.
            if (!colorLevelPinned && "Quality".equals(key)) {
                options.put("ColorLevel", RealVncProvider.colorLevelFor(value));
            }
            alsoSet.put("ColorLevel", options.getOrDefault("ColorLevel", "full"));
            alsoSet.put("PreferredEncoding", options.getOrDefault("PreferredEncoding", "ZRLE2"));
            alsoSet.put("Quality", options.getOrDefault("Quality", "Auto"));
        } else {
            alsoSet.put(key, value);
        }
        RealVncCore.post(() -> {
            if (dead || session != s) {
                return;
            }
            alsoSet.forEach((k, v) -> SessionBindings.setOption(s, k, v));
            if (group) {
                SessionBindings.applyOptions(s);
            }
        });
    }

    // ---- input -------------------------------------------------------------

    @Override
    public void pointer(int x, int y, int buttonMask) {
        onSession(s -> SessionBindings.pointerEvent(s, x, y, buttonMask));
    }

    /**
     * The keys held down at the far end.
     *
     * <p>The core keeps this map too (it makes a release name the same key as
     * its press), but its JNI surface has no "release everything" call, and the
     * map inside the library cannot be enumerated from here. So the ids are
     * tracked on this side as well, so that {@link #releaseAllKeys} works as in
     * every other backend. Session thread only, where every one of these calls
     * runs.
     */
    private final java.util.Set<Integer> held = new java.util.LinkedHashSet<>();

    @Override
    public void keyDown(int keysym, int keyId) {
        onSession(s -> {
            held.add(keyId);
            SessionBindings.keyDownEvent(s, keysym, keyId);
        });
    }

    @Override
    public void keyUp(int keyId) {
        onSession(s -> {
            held.remove(keyId);
            SessionBindings.keyUpEvent(s, keyId);
        });
    }

    @Override
    public void releaseAllKeys() {
        onSession(s -> {
            for (var keyId : held) {
                SessionBindings.keyUpEvent(s, keyId);
            }
            held.clear();
        });
    }

    @Override
    public void focus(boolean focused) {
        onSession(s -> SessionBindings.focusEvent(s, focused));
    }

    @Override
    public void clipboardToRemote(String text) {
        // The core pulls the local clipboard through getClipboard() rather than
        // having it pushed, so there is nothing to send: it is stored and
        // returned when asked for.
        clipboard = text;
    }

    private volatile String clipboard;

    // ---- pixels ------------------------------------------------------------

    /**
     * The only method here that cannot do what {@link Backend} asks for
     * directly.
     *
     * <p>All pixels from this library come through {@code copyScaledRegion},
     * which writes to the front of a buffer <em>the library</em> owns, and
     * {@code Bitmap.copyPixelsFromBuffer}, which fills the whole destination
     * from the front of that buffer. Neither takes an offset, and the second one
     * overwrites every pixel of the bitmap it is given, so reading part of a
     * tile straight into that tile would blank the rest of it.
     *
     * <p>So the offset is emulated here, in the only backend that needs it: the
     * region is read into a scratch bitmap and blitted into place. The scratch
     * bitmap is the <em>destination's</em> size and not the region's, which
     * looks wasteful but is not: the scaler writes at the buffer's row stride, so
     * a buffer whose width changed from read to read would be reallocated
     * natively every time, and the per-call {@code memset} covers the whole
     * buffer regardless.
     */
    @Override
    public boolean readRegion(int x, int y, int width, int height,
                              Bitmap dst, int dstX, int dstY) {
        if (dst == null || dst.getConfig() != Bitmap.Config.ARGB_8888
                || dstX < 0 || dstY < 0) {
            return false;
        }
        if (dstX == 0 && dstY == 0
                && width == dst.getWidth() && height == dst.getHeight()) {
            // Nothing outside the rectangle to preserve, so the copy lands in
            // the caller's bitmap and the scratch is not involved at all. The
            // mirror's interior tiles are read this way whenever all of one
            // changed, which is most of a full-screen repaint.
            return read(x, y, width, height, 1f, dst);
        }
        if (dstX + width > dst.getWidth() || dstY + height > dst.getHeight()) {
            return false;
        }
        synchronized (pixels) {
            if (scratch == null
                    || scratch.getWidth() != dst.getWidth()
                    || scratch.getHeight() != dst.getHeight()) {
                if (scratch != null) {
                    scratch.recycle();
                }
                scratch = Bitmap.createBitmap(dst.getWidth(), dst.getHeight(),
                        Bitmap.Config.ARGB_8888);
                scratchCanvas = new Canvas();
            }
            if (!read(x, y, width, height, 1f, scratch)) {
                return false;
            }
            scratchSrc.set(0, 0, width, height);
            scratchDst.set(dstX, dstY, dstX + width, dstY + height);
            scratchCanvas.setBitmap(dst);
            scratchCanvas.drawBitmap(scratch, scratchSrc, scratchDst, null);
            // Releases the caller's bitmap: a Canvas keeps its bitmap alive, and
            // the caller's bitmaps are the largest objects in the app.
            scratchCanvas.setBitmap(null);
            return true;
        }
    }

    private Bitmap scratch; // guarded by pixels, like the buffer it is read through
    private Canvas scratchCanvas;
    private final Rect scratchSrc = new Rect();
    private final Rect scratchDst = new Rect();

    @Override
    public Bitmap thumbnail(int maxWidth, int maxHeight) {
        final int w = desktopWidth, h = desktopHeight;
        if (w <= 0 || h <= 0 || maxWidth <= 0 || maxHeight <= 0) {
            return null;
        }
        // Powers of two only, and not for quality reasons. The native step is
        // (int)(1/scale), so the scale passed has to survive a round trip
        // through a float reciprocal exactly: 1f/3 is 0.33333334, whose
        // reciprocal is 2.99999994, which truncates to a step of *2*. The scaler
        // then walks the source in half the intended stride and writes 960 pixels
        // a row into a 640-wide buffer, which crashes in native code at the end
        // of every session. 1/2, 1/4 and 1/8 are exact in binary; the original's
        // own tile pyramid uses powers of two, presumably for the same reason.
        // Tested against what is allocated below rather than against the floor
        // of the same division: at w = 1001 and maxWidth = 500, 1001 / 2 is not
        // greater than 500 and the bitmap is 501 wide, which
        // `Backend.thumbnail`'s "no bigger than" does not allow.
        int step = 1;
        while ((w + step - 1) / step > maxWidth || (h + step - 1) / step > maxHeight) {
            step <<= 1;
        }
        // Rounded up, so a desktop whose size is not a multiple of the step has
        // a destination at least as big as whatever the scaler decides to write.
        final int tw = Math.max(1, (w + step - 1) / step);
        final int th = Math.max(1, (h + step - 1) / step);
        final Bitmap out = Bitmap.createBitmap(tw, th, Bitmap.Config.ARGB_8888);
        if (!read(0, 0, w, h, 1f / step, out)) {
            out.recycle();
            return null;
        }
        return out;
    }

    /**
     * The only source of pixels. Deliberately built on
     * {@code copyScaledRegion} rather than {@code getScaledBitmap}, even for
     * the whole framebuffer: the scaler memsets {@code scaleBufH × callerStride}
     * bytes of the destination on every call, which for {@code getScaledBitmap}
     * means clearing past the end of a bitmap smaller than the scratch buffer.
     * Going through a buffer sized by this class keeps that arithmetic here.
     */
    private boolean read(int x, int y, int width, int height, float scale, Bitmap dst) {
        if (dst == null || dst.getConfig() != Bitmap.Config.ARGB_8888) {
            return false;
        }
        synchronized (pixels) {
            final SessionBindings.Session s = session;
            if (s == null || dead || desktopWidth <= 0) {
                return false;
            }
            final int dw = dst.getWidth(), dh = dst.getHeight();
            // The scaler writes at the *buffer's* row stride, so the buffer has
            // to be exactly the destination's width or the rows shear.
            if (buffer == null || bufferWidth != dw || bufferHeight < dh) {
                if (!resizeBuffer(dw, dh)) {
                    return false;
                }
            }
            try {
                SessionBindings.copyScaledRegion(s, x, y, width, height, scale);
            } catch (Throwable t) {
                Log.w(TAG, "copyScaledRegion", t);
                return false;
            }
            buffer.rewind();
            dst.copyPixelsFromBuffer(buffer);
            return true;
        }
    }

    /** Caller holds {@link #pixels}. */
    private boolean resizeBuffer(int width, int height) {
        final SessionBindings.Session s = session;
        if (s == null || dead) {
            return false;
        }
        try {
            buffer = SessionBindings.setScaleBufferSize(s, width, height);
        } catch (Throwable t) {
            Log.w(TAG, "setScaleBufferSize", t);
            buffer = null;
        }
        if (buffer == null) {
            bufferWidth = bufferHeight = 0;
            return false;
        }
        bufferWidth = width;
        bufferHeight = height;
        return true;
    }

    // ---- DesktopCallback (native session thread) ---------------------------

    @Override
    public void connSuccess() {
        Log.i(TAG, "connSuccess on " + Thread.currentThread().getName());
        // The connect is over: let the next waiting session begin.
        finishConnect();
        RealVncCore.cancel(connectTimeout);
        onMain(() -> {
            state = State.CONNECTED;
            fireState(State.CONNECTED, null);
        });
    }

    @Override
    public void setDesktopSize(int width, int height) {
        Log.i(TAG, "setDesktopSize " + width + "x" + height
                + " on " + Thread.currentThread().getName());
        desktopWidth = width;
        desktopHeight = height;
        onMain(() -> {
            if (listener != null) {
                listener.desktopSize(width, height);
            }
        });
    }

    /**
     * The current thread, for the one-off logs recording where the
     * high-traffic callbacks arrive: the library does not document it, and one
     * log line settles it. The name alone does not identify a thread (two
     * threads may share one).
     */
    @SuppressWarnings("deprecation") // Thread.threadId() is Java 19; Android 14 is 17
    private static String thread() {
        final Thread t = Thread.currentThread();
        return t.getName() + " #" + t.getId();
    }

    private boolean loggedDrawThread;

    @Override
    public void drawRegion(int x, int y, int width, int height) {
        if (!loggedDrawThread) {
            loggedDrawThread = true;
            Log.i(TAG, "drawRegion on " + thread());
        }
        final Listener l = listener;
        if (l != null) {
            l.damaged(x, y, width, height);
        }
    }

    private boolean loggedEndThread;

    @Override
    public void framebufferUpdateEnd() {
        if (!loggedEndThread) {
            loggedEndThread = true;
            Log.i(TAG, "framebufferUpdateEnd on " + thread());
        }
        // The received count is sampled here, because this is the edge that
        // moves it: the estimator's window is the update this call ends. Once a
        // second is enough: the mark is cumulative, so a skipped update loses
        // nothing.
        final long now = SystemClock.uptimeMillis();
        if (now - lastSample >= SAMPLE_INTERVAL_MS) {
            lastSample = now;
            traffic.sample(session);
        }
        final Listener l = listener;
        if (l != null) {
            l.frameEnd();
        }
    }

    @Override
    public void setCursor(Object bitmap, int negHotX, int negHotY) {
        // The two ints are the negated hotspot: the core's RFB parser rewrites a
        // cursor rectangle to Rect(-hot, size - hot) on arrival and this
        // callback forwards that rectangle's origin. There is no hotspot
        // variable anywhere in the library to compare it against.
        final Bitmap shape = bitmap instanceof Bitmap b ? b : null;
        onMain(() -> {
            if (listener != null) {
                listener.cursor(shape, -negHotX, -negHotY);
            }
        });
    }

    @Override
    public void bell() {
        onMain(() -> {
            if (listener != null) {
                listener.bell();
            }
        });
    }

    @Override
    public String getClipboard() {
        // Answered inline, on the core's thread, as the original does.
        final Listener l = listener;
        final String fromApp = l != null ? l.clipboardForRemote() : null;
        final String out = fromApp != null ? fromApp : clipboard;
        Log.i(TAG, "getClipboard: " + (out == null ? "none" : out.length() + " chars"));
        return out;
    }

    @Override
    public void setClipboard(String text) {
        Log.i(TAG, "setClipboard: " + (text == null ? "none" : text.length() + " chars"));
        onMain(() -> {
            if (listener != null) {
                listener.clipboardFromRemote(text);
            }
        });
    }

    // ---- SessionCallback ---------------------------------------------------

    @Override
    public void sessionClosed(SessionBindings.Session ended) {
        Log.i(TAG, "sessionClosed on " + Thread.currentThread().getName());
        if (ended != null && session != null && ended != session) {
            return; // another session: the identity check the rest of this class relies on
        }
        // Already closed, so it must not be closed again.
        reap(false);
        closed(null);
    }

    // ---- called by RealVncPrompts ------------------------------------------

    void ask(Prompt prompt) {
        promptShown();
        onMain(() -> {
            final Prompt.Handler h = prompts;
            if (h == null) {
                prompt.cancel();
                return;
            }
            switch (prompt) {
                case Prompt.Credentials c -> h.credentials(c);
                case Prompt.Trust t -> h.trust(t);
                case Prompt.Message m -> h.message(m);
            }
        });
    }

    /**
     * The core's status line ("Authenticating user…", "Attempting to
     * reconnect to VNC Server…"), under "Connecting to" rather than in its
     * place, so that the address stays on screen. The screen shows it only
     * while connecting. Its first, "Connecting...", says what the line above
     * it already does, and is shown as no status.
     */
    void status(String text) {
        Log.i(TAG, "status: " + text);
        final String shown = "Connecting...".equals(text) ? null : text;
        onMain(() -> fireStatus(shown));
    }

    /**
     * The core's reconnect banner: connecting again, with its message as the
     * status line.
     */
    void reconnecting(String message, String detail) {
        Log.i(TAG, "reconnecting: " + message + (detail == null ? "" : " (" + detail + ")"));
        onMain(() -> {
            fireState(State.CONNECTING, connectingTo());
            fireStatus(message);
        });
    }

    /**
     * The core's reconnect banner being hidden again; the core shows it briefly
     * on a session it then keeps. Only the banner reported the connection
     * dropping; {@link #state} never changed, so it is reported again here.
     * Without this, a recovered session would show "Connecting…" forever.
     */
    void reconnected() {
        onMain(() -> {
            fireState(state, state == State.CONNECTING ? connectingTo() : null);
            fireStatus(null);
        });
    }

    void saveCredentials(String userName, String password) {
        // These belong in the connection store, which this class cannot reach;
        // logging the request is better than dropping it silently.
        Log.i(TAG, "server asked to save credentials for " + userName);
    }

    void saveIdentity(String identity) {
        // The core sends this when the prompt is answered with "remember";
        // storing it skips the prompt on the next connection.
        identities(context).edit().putString(address == null ? "" : address, identity).apply();
    }

    /** Keyed on the address as written, as {@link KnownHosts} is. */
    private static android.content.SharedPreferences identities(Context context) {
        return context.getSharedPreferences("realvnc_identities", Context.MODE_PRIVATE);
    }

    /**
     * Forgets every identity this core has been told to remember, so that the
     * next connection to each server prompts again.
     *
     * <p>The RealVNC part of clearing {@link KnownHosts}, which does not reach
     * either of these stores: what is stored is the core's own blob rather than
     * a fingerprint, and only the core can compare one.
     *
     * <p>Both stores are cleared. The one above holds what this class passes
     * back as {@code Identity} on the next connection; the core keeps its own
     * beside its keys, under the directory it was given, and does not prompt
     * for a server it still recognises there.
     */
    static void forgetIdentities(Context context) {
        identities(context).edit().clear().apply();
        final File own = new File(new File(RealVncCore.dataDir(context), ".vnc"), "identities");
        if (own.exists() && !own.delete()) {
            Log.w(TAG, "could not delete " + own);
        }
    }

    // ---- plumbing ----------------------------------------------------------

    private void closed(String detail) {
        // The connect is over (it may never have begun, or ended before
        // connSuccess): release the gate and start the next waiter.
        finishConnect();
        // Nothing the core dials from here on is this session's.
        RealVncNet.closed(route);
        onMain(() -> {
            if (state == State.CLOSED) {
                return;
            }
            state = State.CLOSED;
            fireState(State.CLOSED, detail);
        });
    }

    private void fireState(State s, String detail) {
        if (listener != null) {
            listener.state(s, detail);
        }
    }

    private void fireStatus(String text) {
        if (listener != null) {
            listener.status(text);
        }
    }

    private String connectingTo() {
        return "Connecting to " + address;
    }

    private void onMain(Runnable r) {
        if (Looper.myLooper() == Looper.getMainLooper()) {
            r.run();
        } else {
            main.post(r);
        }
    }
}
