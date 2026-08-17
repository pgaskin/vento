// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend;

import android.graphics.Bitmap;

/**
 * One remote desktop connection, whatever protocol is underneath.
 *
 * <p>Everything above {@code CursorController} is already protocol-free — the
 * gesture stack emits absolute desktop coordinates and an RFB button mask and
 * knows nothing else — so this interface is narrow on purpose: pixels out,
 * input in, and a small number of events.
 *
 * <h2>Threads</h2>
 * <ul>
 *   <li>Every method here may be called from the main thread, and the
 *       implementation is responsible for getting the work to wherever its
 *       library wants it. RealVNC's wants a single {@code Looper} thread that
 *       is not the main one, because that is where the core registers its
 *       socket and delivers every callback.
 *   <li>{@link #readRegion} is the exception in the other direction: it is
 *       called from whichever thread is drawing, concurrently with everything
 *       else, and must be safe there. That is deliberate — pixel fetches must
 *       not queue behind connection work, which is the one place the original
 *       steps outside its own JNI thread too.
 *   <li>{@link Listener} callbacks arrive on the main thread, except
 *       {@link Listener#damaged} and {@link Listener#frameEnd}, which arrive on
 *       whatever thread the protocol runs on and must be cheap and
 *       thread-safe. Those two are per-update-batch traffic; posting each one
 *       would be a flood.
 * </ul>
 */
public interface Backend {

    enum State {
        IDLE,       // created, not started
        CONNECTING, // connect() called, nothing usable yet
        CONNECTED,  // pixels and input are flowing
        CLOSED      // over, for any reason; detail says which, and it is terminal
    }

    interface Listener {

        /** Main thread. {@code detail} is human-readable, and may be null. */
        void state(State state, String detail);

        /** Main thread. The framebuffer's size, initially and on resize. */
        void desktopSize(int width, int height);

        /**
         * Any thread. This rectangle of the desktop has changed; the pixels are
         * not fetched until someone calls {@link #readRegion}.
         */
        void damaged(int x, int y, int width, int height);

        /** Any thread. End of an update batch — a good moment to draw. */
        void frameEnd();

        /**
         * Main thread. The remote cursor's shape, and the hotspot as a
         * positive offset into the bitmap. RFB carries a cursor rectangle whose
         * origin is the <em>negated</em> hotspot, and RealVNC's core passes that
         * origin straight through; a backend undoes the negation here, so a
         * caller draws the shape at {@code position - hotspot} and never meets
         * the sign.
         */
        void cursor(Bitmap shape, int hotX, int hotY);

        /**
         * Main thread. Which end owns the cursor has changed.
         *
         * <p>Relative means the far end does: {@link #pointerRelative} is the
         * call that means anything, nobody here knows where the pointer is, and
         * a client that draws one and follows it with a viewport has to stop
         * doing both. Only RFB against a server that asks for it — QEMU's, in
         * practice — ever says true.
         */
        void pointerMode(boolean relative);

        /** Main thread. */
        void bell();

        /** Main thread. The remote's clipboard changed. */
        void clipboardFromRemote(String text);

        /**
         * Any thread. The remote is asking for ours; return null for nothing.
         * Called inline, so it must not block on the main thread.
         */
        String clipboardForRemote();
    }

    void setListener(Listener listener);

    /** How to answer the connection's questions. Set before {@link #connect}. */
    void setPromptHandler(Prompt.Handler handler);

    /**
     * Start connecting. Returns immediately; progress arrives as
     * {@link Listener#state}.
     */
    void connect();

    /** Ask for a graceful disconnect. {@link State#CLOSED} follows. */
    void disconnect();

    /**
     * Release everything, including whatever the native side is holding. The
     * backend is dead afterwards and must not be reused.
     */
    void destroy();

    State state();

    /**
     * Change one of the settings this backend describes ({@link BackendOption})
     * on the running session.
     *
     * <p>Only meaningful for an option whose {@link BackendOption#live} is set;
     * anything else is remembered for the next connection at best. Whatever a
     * particular backend has to do to make a change take effect is its own
     * business — RealVNC's quality group needs an {@code applyOptions} call
     * after the set and nothing else does, and that asymmetry has no place in
     * this interface.
     */
    void setOption(String key, String value);

    /**
     * Whether this session is watching rather than driving — the one option the
     * <em>app</em> has to know about rather than merely pass through.
     *
     * <p>Every backend already drops input for it natively, so nothing would
     * break without this; what would remain is a keyboard and a mouse overlay
     * that quietly do nothing, which is indistinguishable from a session that
     * has stopped responding. An accessor rather than
     * {@code options.get("ViewOnly")} at the call site, because the key differs
     * per backend and the question does not.
     */
    boolean viewOnly();

    /**
     * Whatever the backend can say about the live connection, in display order:
     * desktop name, protocol, connection, security, encoding, line speed, and
     * then whatever is diagnostic. What the "connection information" tap region
     * opens.
     *
     * <p>Asynchronous, and that is not politeness: gathering this means asking
     * the library on whichever thread it insists on, and RealVNC's is the one
     * session thread, which can be inside a connect or blocked behind an
     * unanswered password prompt.
     *
     * @param callback called once, on the main thread, with the facts in display
     *                 order — or an empty list when there is nothing connected.
     *                 May never be called if the session is wedged, which is why
     *                 nothing on screen may depend on it arriving.
     */
    void connectionInfo(java.util.function.Consumer<java.util.List<ConnectionFact>> callback);

    /** Zero until the first {@link Listener#desktopSize}. */
    int desktopWidth();

    int desktopHeight();

    /**
     * How the far end's desktop is divided into monitors, or empty when it has
     * not said — which is not the same as one monitor, and is the answer for a
     * backend whose library never exposes the layout.
     *
     * <p>Polled rather than announced, and for the same reason
     * {@link #canResize} is: an RFB server states its layout in a rectangle that
     * arrives after the connection if it arrives at all, and a desktop with a
     * monitor plugged into it says so again.
     */
    default java.util.List<Monitor> monitors() {
        return java.util.List.of();
    }

    /**
     * The far end's displays, where it sends <em>one of them at a time</em> and
     * will send another if asked — empty for every protocol that does not, which
     * is all but one of them.
     *
     * <p>Not {@link #monitors}, and the difference is which picture is on
     * screen. A monitor is a region of the one framebuffer everything else here
     * serves, so a caller with the layout can jump the viewport between heads
     * and the pixels are already there. A display is a picture the far end is
     * <em>not</em> sending: choosing another is a message, a new size and a new
     * framebuffer, and the rectangles are only in the far end's own coordinates
     * so that a person can tell which screen is which.
     *
     * <p>Polled, and for {@link #canResize}'s reason: it is a fact about the
     * live session, arrives after the connection and changes when a screen is
     * plugged in over there.
     */
    default java.util.List<Monitor> displays() {
        return java.util.List.of();
    }

    /** Which of {@link #displays} is on screen, or -1 where there is no choice. */
    default int display() {
        return -1;
    }

    /**
     * Ask the far end to send another of its displays.
     *
     * <p>A request, like {@link #requestDesktopSize}: the far end may answer
     * with a different one or with nothing, and what actually happened arrives
     * as {@link Listener#desktopSize} and in the next {@link #display}.
     */
    default void requestDisplay(int index) {
    }

    /**
     * Whether the far end will take a new desktop size right now.
     *
     * <p>Deliberately not a {@link BackendOption}: an option is a static
     * description a backend gives of itself before anything is connected, and
     * this is a fact about one live session — an RFB server says it by sending
     * an {@code ExtendedDesktopSize} rectangle, which may not arrive at all.
     * So it is asked repeatedly rather than announced, and a caller must expect
     * the answer to change from false to true shortly after a connection and to
     * differ between two sessions with the same backend.
     */
    default boolean canResize() {
        return false;
    }

    /**
     * Ask the far end to make the desktop this size.
     *
     * <p>A request, and the far end decides: it may refuse, or grant a
     * different size than asked. What actually happened arrives as
     * {@link Listener#desktopSize}, or as nothing at all when the answer is no
     * — so a caller reports the size it has rather than the size it asked for.
     */
    default void requestDesktopSize(int width, int height) {
    }

    // ---- input ------------------------------------------------------------

    /** Absolute desktop coordinates and an RFB button mask (see {@code Button}). */
    void pointer(int x, int y, int buttonMask);

    /**
     * The same event where the far end owns the cursor: how far it moved, in
     * desktop pixels, rather than where it now is.
     *
     * <p>Only meaningful while {@link #pointerIsRelative} — every other backend
     * ignores it, and no caller may choose between the two: which one carries
     * the pointer is the far end's decision, announced through
     * {@link Listener#pointerMode}.
     *
     * <p>These two have defaults where nothing else here does, and the default
     * is the answer for every protocol but one: the cursor is ours. A backend
     * that says nothing is absolute.
     */
    default void pointerRelative(int dx, int dy, int buttonMask) {
    }

    /**
     * Whether the far end owns the cursor. False for everything but an RFB
     * session whose server has asked for relative motion.
     *
     * <p>Asked as well as announced, because a screen attaching to a session
     * that is already running has missed the announcement.
     */
    default boolean pointerIsRelative() {
        return false;
    }

    /**
     * @param keysym an X11 keysym ({@code control.input.Keysym})
     * @param keyId  a stable identity for the physical key — any value, as long
     *               as the same key uses the same one. A library keeping a map
     *               of held keys keys it on this and releases the keysym
     *               recorded at press time, so it is what ties an up to its down.
     */
    void keyDown(int keysym, int keyId);

    void keyUp(int keyId);

    /**
     * Let go of every key this session still has down at the far end.
     *
     * <p>A sweep rather than the ordinary path: the caller releases each key it
     * knows about by id, and this is for the ones it no longer knows about —
     * a screen taken away mid-chord, an event stream that stopped between a
     * press and its release. "Held" means held over there, and it stays that
     * way until something says otherwise, so the alternative is a Ctrl that is
     * down for the rest of the session.
     *
     * <p>Every backend keeps the map this needs, because a release names a key
     * and the protocols carry a keysym; four of them expose it and RealVNC's
     * core does not, so that one keeps its own copy of the ids.
     */
    void releaseAllKeys();

    /**
     * Whether the session is in the foreground. Backends may pause updates when
     * it is not, which is where a backgrounded session stops costing traffic:
     * RealVNC's core stops asking for framebuffer updates at the end of the one
     * in flight, and remembers a request that fell due meanwhile.
     *
     * <p>Deliberately <em>not</em> where keys are let go: whether losing the
     * screen should release them is a question about this phone rather than
     * about a protocol, so it is one app setting and one
     * {@link #releaseAllKeys} call rather than the same decision taken five
     * times below this line.
     */
    void focus(boolean focused);

    void clipboardToRemote(String text);

    // ---- pixels -----------------------------------------------------------

    /**
     * Copy the desktop rectangle {@code (x, y, width, height)} into {@code dst}
     * at {@code (dstX, dstY)}, 1:1. <b>The rest of {@code dst} is left as it
     * was</b>, which is what lets a caller hold a picture bigger than any one
     * read and refresh the parts of it that changed.
     *
     * <p>That promise is the whole reason the offset is here. Without it a
     * caller reading less than a whole bitmap has to go through a scratch bitmap
     * and blit the result into place — a copy of everything that changed, on the
     * drawing thread. With it, a backend that owns its framebuffer writes the
     * rows straight into the destination.
     *
     * <p>Callable from the drawing thread at any time. Returns false if there is
     * nothing to read yet, or if the rectangle does not fit — in which case
     * {@code dst} is untouched.
     *
     * @param dst  {@link Bitmap.Config#ARGB_8888}, at least
     *             {@code dstX + width} by {@code dstY + height}
     * @param dstX where in {@code dst} the rectangle's left edge goes
     * @param dstY where in {@code dst} the rectangle's top edge goes
     */
    boolean readRegion(int x, int y, int width, int height,
                       Bitmap dst, int dstX, int dstY);

    /**
     * The whole framebuffer, no bigger than {@code maxWidth} by
     * {@code maxHeight} — a thumbnail for the home screen.
     *
     * <p>Returns a new bitmap, whose exact size the backend chooses within the
     * bounds given, or null if there is nothing to show yet. Backends may
     * quantise the scale — this is a preview, and the RealVNC one can only
     * downscale by whole integers.
     */
    Bitmap thumbnail(int maxWidth, int maxHeight);
}
