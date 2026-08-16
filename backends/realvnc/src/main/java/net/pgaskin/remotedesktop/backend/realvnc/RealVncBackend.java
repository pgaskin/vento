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
 * declared in {@code :realvnc-jni}. What is interesting here is the thread
 * discipline that library imposes.
 *
 * <p>Two threads touch this object, not the three that were expected:
 *
 * <ul>
 *   <li><b>The session thread</b> ({@link RealVncCore}) owns every call into
 *       the library — and <em>also delivers every callback</em>, which is not
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
 * that is this class's promise, not the library's.
 *
 * <p>That last one needs its own lock, for a sharper reason than "shared
 * state": {@code copyScaledRegion} takes a native mutex, but the allocator
 * behind {@code setScaleBufferSize} does not (0x2405e4 rewrites the vector at
 * +0x148 with no lock at all), so a resize racing a copy is a use-after-free
 * inside the library. {@link #pixels} makes the pair mutually exclusive on our
 * side, which is the only place it can be done.
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
     * The received count, and how often it is taken. Session thread only, which
     * is where both of its callers already are.
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

    /**
     * How long to wait for {@code connSuccess} before giving up. Load-bearing:
     * a connect that fails reports <em>nothing</em> — a refused port logs
     * {@code CConnection: close} internally and then the session sits there,
     * with no {@code sessionClosed}, no {@code MsgBox}, no status change and no
     * retry, so a mistyped address is a spinner that never stops.
     */
    private static final long CONNECT_TIMEOUT_MS = 20_000;

    private final Runnable connectTimeout = this::onConnectTimeout;

    /**
     * Prompts on screen. The timeout is about a session that has gone silent,
     * and a session waiting for a password has not: it is waiting for a person,
     * who is entitled to take longer than twenty seconds to find one. Counted
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

    /** Stop the clock: something is being asked of the person holding the phone. */
    private void promptShown() {
        RealVncCore.post(() -> {
            outstandingPrompts++;
            RealVncCore.cancel(connectTimeout);
        });
    }

    /** Start it again, from the top, once the last dialog is gone. */
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
        this.address = address;
        // Empty is not a password: the core authenticates with whatever it is
        // given, so "" fails the connection outright instead of asking. Null is
        // how it is told there is nothing stored.
        this.password = password == null || password.isEmpty() ? null : password;
        this.options = new LinkedHashMap<>();

        // Every offered parameter at its default, then whatever the caller has
        // stored over the top. The list, the defaults and the wording all live
        // in RealVncProvider, so the settings screen and the connect path cannot
        // drift apart.
        options.putAll(RealVncProvider.defaults());
        if (userName != null && !userName.isEmpty()) {
            options.put("UserName", userName);
        }
        if (extraOptions != null) {
            options.putAll(extraOptions);
        }
        // What this server was last accepted as. The core's own security
        // prompt is silent for an identity it recognises, which is the same
        // "accepted once, loud if it changes" rule the other four get from
        // KnownHosts — the difference being that this one is an opaque blob the
        // core makes rather than a fingerprint anybody else could compare.
        final String identity = identities(context).getString(address == null ? "" : address, null);
        if (identity != null && !options.containsKey("Identity")) {
            options.put("Identity", identity);
        }
        // Whether the colour depth is this connection's own choice. The store
        // keeps only what differs from the default, so the key being present *is*
        // the statement that somebody set it — and a depth somebody set must not
        // be moved out from under them by the quality control.
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
        RealVncCore.start(context);
        RealVncCore.postDelayed(connectTimeout, CONNECT_TIMEOUT_MS);
        onMain(() -> fireState(State.CONNECTING, "Connecting to " + address));
        RealVncCore.post(() -> {
            if (dead) {
                return;
            }
            final long t0 = System.nanoTime();
            RealVncPrompts.connecting(this);
            SessionBindings.Session s = null;
            try {
                s = SessionBindings.createSession(this, this, address, password,
                        new HashMap<>(options));
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
        });
    }

    /**
     * Close it <em>and</em> retire the handle, which is what stops the close
     * happening twice.
     *
     * <p>The core's own {@code sessionClosed} is what usually retires it, and it
     * arrives on the session thread some time later — so a screen that leaves
     * immediately after asking to disconnect, which is exactly what the Back
     * gesture does, reaches {@link #destroy} while the handle is still live and
     * closes it again. A second {@code closeSession} is a <b>SIGSEGV inside the
     * core</b> rather than an error it reports, and it takes the process with
     * it, so the app disappears just as the connection list comes up.
     */
    @Override
    public void disconnect() {
        if (session != null) {
            reap(true);
        } else {
            closed("Disconnected");
        }
    }

    @Override
    public void destroy() {
        dead = true;
        reap(true);
    }

    /**
     * Retire the handle: nothing new can be posted for it, and what is already
     * queued is skipped by {@link #onSession}'s identity check.
     *
     * <p><b>Nothing is ever freed.</b> {@code destroySession} exists, and calling
     * it on a session the core has closed is a SIGSEGV inside the core — the
     * same fault, at the same offset, as closing one twice. The core tears its
     * session down in {@code sessionClosed} and what is left is a handle that
     * may only be dropped, which is exactly what the original does: it never
     * calls {@code destroySession} at all and lets the native registry keep
     * every session the process ever opened. That is a leak, and it is theirs;
     * the alternative is a crash on the way out of every connection.
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
            // The pixel lock as well as the session thread: a fetch in flight
            // on the drawing thread must not be looking at the buffer this
            // frees, and the native allocator does not lock (see the class
            // comment).
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
                    // Not once the core has said it is closed: it tears the
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
     * Run something on the session thread with the live handle, or return false
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
     * Everything {@code ConnectionInfoBindings} knows, gathered on the session
     * thread and handed back on the main one.
     *
     * <p>Every one of these is a string the library formats itself, and two of
     * them move on their own: {@code getLastUsedEncoding} follows the quality
     * group and {@code getLineSpeed} is a running measurement. That is what
     * makes the panel's poll worth having rather than a snapshot.
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
                        ConnectionInfoBindings.getConnectionType(s)));
                facts.add(ConnectionFact.of(ConnectionFact.Field.SECURITY, "Security",
                        ConnectionInfoBindings.getSecurityDesc(s)));
                facts.add(ConnectionFact.of(ConnectionFact.Field.ENCODING, "Encoding",
                        ConnectionInfoBindings.getLastUsedEncoding(s)));
                facts.add(ConnectionFact.of(ConnectionFact.Field.LINE_SPEED, "Line speed",
                        ConnectionInfoBindings.getLineSpeed(s)));
                // Received only, and no row at all where there is no
                // trustworthy number: the core counts one direction and has no
                // count of the other anywhere, and a zero would be a claim.
                final long received = traffic.received();
                if (received >= 0) {
                    facts.add(ConnectionFact.data(received, -1));
                }
                facts.add(ConnectionFact.diagnostic(ConnectionFact.Field.SERVER_PIXELS,
                        "Server pixels", ConnectionInfoBindings.getServerPixelFormat(s)));
                facts.add(ConnectionFact.diagnostic(ConnectionFact.Field.VIEWER_PIXELS,
                        "Viewer pixels", ConnectionInfoBindings.getViewerPixelFormat(s)));
            }
            main.post(() -> callback.accept(facts));
        });
    }

    @Override
    public boolean viewOnly() {
        return Boolean.parseBoolean(options.get("ViewOnly"));
    }

    /**
     * A live option change. Which keys need the second call is
     * {@link RealVncProvider#needsApply}'s business — the interface does not
     * carry it, because "re-read the parameters that affect the encoder" is a
     * fact about this library and not about remote desktops.
     *
     * <p>With no session yet, the value simply joins the map {@link #connect}
     * will hand to {@code createSession}.
     */
    @Override
    public void setOption(String key, String value) {
        final SessionBindings.Session s = session;
        if (s == null) {
            options.put(key, value);
            return;
        }
        options.put(key, value);
        // Same rule as the constructor's: a depth somebody has set is theirs,
        // whether they set it before the session or during it.
        colorLevelPinned |= "ColorLevel".equals(key);
        final boolean group = RealVncProvider.needsApply(key);
        // The quality group goes over as a group, always: the core re-reads the
        // three together on applyOptions, and the three decide one thing
        // between them — which encodings to ask for and which pixel format.
        // Colour depth first, quality last, which is the order the pair was
        // seen to work in.
        final Map<String, String> alsoSet = new LinkedHashMap<>();
        if (group) {
            // And the depth is the quality's, unless this connection chose one
            // (RealVncProvider.colorLevelFor). Only a quality change moves it,
            // so a depth set in the panel is the one that goes over.
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
     * Which keys are down at the far end.
     *
     * <p>The core keeps this map too — it is what makes a release name the key
     * its press named — but its JNI surface has no "let go of everything", and
     * nothing here can enumerate a map that lives in the library. So the ids
     * are kept on this side as well, purely so that
     * {@link #focus focus(false)} can do what every other backend does.
     * Session thread only, which is where every one of these calls runs.
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
        // The core pulls ours through getClipboard() rather than being pushed,
        // so there is nothing to send: remember it and answer when asked.
        clipboard = text;
    }

    private volatile String clipboard;

    // ---- pixels ------------------------------------------------------------

    /**
     * The one method here that cannot do what the seam asks for directly.
     *
     * <p>Everything this library gives us goes through {@code copyScaledRegion},
     * which writes to the front of a buffer <em>it</em> owns, and
     * {@code Bitmap.copyPixelsFromBuffer}, which fills the whole destination
     * from the front of that buffer. Neither takes an offset, and the second one
     * overwrites every pixel of the bitmap it is given — so a read of part of a
     * tile straight into that tile would blank the rest of it.
     *
     * <p>So the offset is emulated here, in the one backend that needs it: a
     * scratch bitmap read into and blitted into place. It is the
     * <em>destination's</em> size and not the region's, which looks wasteful and
     * is not — the scaler writes at the buffer's row stride, so a buffer whose
     * width changed from read to read would be re-allocated natively every time,
     * and the per-call {@code memset} covers the whole buffer regardless.
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
            // Not left holding the caller's bitmap: a Canvas keeps its bitmap
            // alive, and the caller's are the biggest things in the app.
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
        // Powers of two only, and that is not a quality decision. The native step
        // is (int)(1/scale), so the scale we pass has to survive a round trip
        // through a float reciprocal exactly: 1f/3 is 0.33333334, whose
        // reciprocal is 2.99999994, which truncates to a step of *2* — the scaler
        // then walks the source in half the intended stride and writes 960 pixels
        // a row into a 640-wide buffer, which is a native crash on the way out of
        // every session. 1/2, 1/4 and 1/8 are exact in binary; the original's own
        // tile pyramid is powers of two for presumably the same reason.
        // Tested against what is allocated below rather than against the floor
        // of the same division: at w = 1001 and maxWidth = 500, 1001 / 2 is not
        // greater than 500 and the bitmap is 501 wide, which the seam's "no
        // bigger than" does not allow.
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
     * The one place pixels come from. Deliberately built on
     * {@code copyScaledRegion} rather than {@code getScaledBitmap}, even for
     * the whole framebuffer: the scaler memsets {@code scaleBufH × callerStride}
     * bytes of the destination on every call, which for {@code getScaledBitmap}
     * means clearing past the end of a bitmap smaller than the scratch buffer.
     * Going through the buffer we own makes that arithmetic ours.
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
     * Which thread, for the one-off logs that record where the high-traffic
     * callbacks arrive: nothing in the library says, and one log line settles
     * it. The name alone is not identity — two threads may share one.
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
        // Where the received count is sampled, because this is the edge that
        // moves it: the estimator's window is the update this call ends. Once a
        // second is enough — the mark is cumulative, so a skipped update costs
        // nothing at all.
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
        // The two ints are the negated hotspot: their RFB parser rewrites a
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
            return; // not ours: the identity check the rest of this class is built on
        }
        // Already closed by definition, so do not close it again.
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

    void status(String text) {
        Log.i(TAG, "status: " + text);
        onMain(() -> fireState(state, text));
    }

    void reconnecting(String message, String detail) {
        onMain(() -> fireState(State.CONNECTING, message));
    }

    /**
     * The core's reconnect banner coming down again, which it raises for a
     * moment on a session it then keeps. Only the banner said the connection
     * was going; {@link #state} never moved, so this is what it still is —
     * without it a session that recovered says "Connecting…" for ever.
     */
    void reconnected() {
        onMain(() -> fireState(state, null));
    }

    void saveCredentials(String userName, String password) {
        // The connection store is where these belong, and nothing here has a
        // way to reach it: saying so is better than silently dropping them.
        Log.i(TAG, "server asked to save credentials for " + userName);
    }

    void saveIdentity(String identity) {
        // Answering the prompt with "remember" is what makes the core send
        // this; storing it is what makes the next connection silent.
        identities(context).edit().putString(address == null ? "" : address, identity).apply();
    }

    /** Keyed on the address as written, as {@link KnownHosts} is. */
    private static android.content.SharedPreferences identities(Context context) {
        return context.getSharedPreferences("realvnc_identities", Context.MODE_PRIVATE);
    }

    /**
     * Every identity this core has been told to remember, forgotten, so that the
     * next connection to each is asked about again.
     *
     * <p>The other half of what clearing {@link KnownHosts} means, which does
     * not reach either of these: what is stored is the core's own blob rather
     * than a fingerprint, and only the core can compare one.
     *
     * <p>Two stores because there are two. The one above is what this class
     * hands back as {@code Identity} on the next connection; the core keeps its
     * own beside its keys, under the directory it was given, and a server it
     * still recognises there is a server it does not ask about.
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

    private void onMain(Runnable r) {
        if (Looper.myLooper() == Looper.getMainLooper()) {
            r.run();
        } else {
            main.post(r);
        }
    }
}
