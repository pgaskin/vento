// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend;

import android.graphics.Bitmap;

import java.util.List;
import java.util.function.Consumer;

/**
 * One remote desktop connection, whatever protocol is underneath.
 *
 * <p>Everything above {@code CursorController} is already protocol-free (the
 * gesture stack emits absolute desktop coordinates and an RFB button mask and
 * nothing else), so this interface is narrow on purpose: pixels out, input
 * in, and a small number of events.
 *
 * <h2>Threads</h2>
 * <ul>
 *   <li>Every method here may be called from the main thread, and the
 *       implementation moves the work to whichever thread its library needs.
 *       RealVNC's needs a single {@code Looper} thread other than the main
 *       one, because that is where the core registers its socket and delivers
 *       every callback.
 *   <li>{@link #readRegion} is the exception in the other direction: it is
 *       called from whichever thread is drawing, concurrently with everything
 *       else, and must be safe there, so that pixel fetches do not queue
 *       behind connection work.
 *   <li>{@link Listener} callbacks arrive on the main thread, except
 *       {@link Listener#damaged} and {@link Listener#frameEnd}, which arrive on
 *       the protocol's thread and must be cheap and thread-safe. Those two
 *       come with every update batch, and posting each one would flood the
 *       main thread.
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
         * not fetched until {@link #readRegion} is called.
         */
        void damaged(int x, int y, int width, int height);

        /** Any thread. End of an update batch: a good moment to draw. */
        void frameEnd();

        /**
         * Main thread. The remote cursor's shape, and the hotspot as a
         * positive offset into the bitmap. RFB carries a cursor rectangle whose
         * origin is the <em>negated</em> hotspot, and RealVNC's core passes that
         * origin straight through; a backend undoes the negation here, so a
         * caller draws the shape at {@code position - hotspot} and never sees
         * the negation.
         */
        void cursor(Bitmap shape, int hotX, int hotY);

        /**
         * Main thread. Which end owns the cursor has changed.
         *
         * <p>Relative means the far end does: only {@link #pointerRelative} has
         * an effect, the pointer's position is unknown here, and a client that
         * draws a pointer and follows it with a viewport has to stop doing
         * both. Only RFB and SPICE ever say true, against a server that asks
         * for it (QEMU, in practice).
         */
        void pointerMode(boolean relative);

        /** Main thread. */
        void bell();

        /**
         * Main thread. What the connection is waiting for while it connects
         * (a far end whose user has to accept the session, a retry), as a
         * line under "Connecting to …", or null to clear it. Only meaningful
         * in {@code CONNECTING}: a screen drops it on {@code CONNECTED} and
         * {@code CLOSED}. A default, since only some backends have one.
         */
        default void status(String text) {
        }

        /**
         * Main thread. Whether the far end's sound is playing: from its first
         * sound until its last stream closes, which a backend does a couple of
         * seconds after the sound ends. Whether a sound switch is shown comes
         * from {@link #soundSwitch} instead, which is sent before any sound
         * plays. A default, since only the backends with sound call it.
         */
        default void soundPlaying(boolean playing) {
        }

        /**
         * Main thread. The session's sound switch changed: what it does
         * ({@link #soundSwitch}), whether the far end is asked for its sound
         * ({@link #soundEnabled}), and whether a stream plays. Sent as soon
         * as the far end may send sound, before any plays, and whenever any
         * of it changes. A default, since only the backends with sound call
         * it.
         */
        default void soundSwitch(int soundSwitch, boolean enabled, boolean playing) {
        }

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
     * particular backend has to do to make a change take effect stays inside
     * it: RealVNC's quality group needs an {@code applyOptions} call after the
     * set and nothing else does, and that asymmetry is kept out of this
     * interface.
     */
    void setOption(String key, String value);

    /**
     * Whether this session is watching rather than driving: the one option the
     * <em>app</em> has to act on rather than only pass through.
     *
     * <p>Every backend already drops input for it natively, so nothing would
     * break without this, but the keyboard and mouse overlay would do nothing,
     * which looks the same as a session that has stopped responding. An
     * accessor rather than
     * {@code options.get("ViewOnly")} at the call site, because the key differs
     * per backend and the question does not.
     */
    boolean viewOnly();

    /**
     * Whether the far end's sound is playing right now: {@link
     * Listener#soundPlaying} as of this instant, for a screen that attaches to
     * a session already playing.
     */
    default boolean soundPlaying() {
        return false;
    }

    /** No sound switch: nothing says the far end sends sound. */
    int SOUND_SWITCH_NONE = 0;
    /**
     * A "Mute audio" switch: the far end cannot be asked to stop sending its
     * sound during a session (RDP), so the switch silences it here ({@link
     * #setMuted}).
     */
    int SOUND_SWITCH_MUTE = 1;
    /**
     * An "Enable audio" switch: the far end is asked to stop sending its
     * sound, and to send it again ({@link #setSoundEnabled}): QEMU's audio
     * over TigerVNC, SPICE, RustDesk.
     */
    int SOUND_SWITCH_ENABLE = 2;

    /**
     * What the session's sound switch does, one of {@code SOUND_SWITCH_*}:
     * {@link Listener#soundSwitch} as of this instant. Set from the moment the
     * far end may send sound, whether or not any is playing.
     */
    default int soundSwitch() {
        return SOUND_SWITCH_NONE;
    }

    /**
     * Whether the far end is asked for its sound: the state of an Enable
     * switch, and true for the other kinds.
     */
    default boolean soundEnabled() {
        return true;
    }

    /**
     * The Enable switch: ask the far end to stop sending its sound, or to send
     * it again, on the running session. Per session and not remembered: the
     * next session starts as the connection's own option (Audio) says. Does
     * nothing for other kinds of switch.
     */
    default void setSoundEnabled(boolean enabled) {
    }

    /**
     * A debug overlay line for the session's sound, in the Linux client's
     * format: {@code snd}, then what plays (codec, rate, channels, the queue and
     * the device's buffer, underruns and dropped frames) or {@code nothing
     * playing}, then {@code DISABLED} or {@code MUTED}. Null where there is no
     * sound switch. Cheap enough to call on every overlay refresh.
     */
    default String soundLine() {
        return null;
    }

    /** Whether this session's sound is muted ({@link #setMuted}). */
    default boolean muted() {
        return false;
    }

    /**
     * Mute or unmute the session's sound immediately, on the running session:
     * the mute is applied where the samples go to the device, so queued sound
     * keeps draining under it, and a stream the far end opens later starts
     * muted. Not an option and not remembered: the next session starts with its
     * sound on, and the connection's own option (Audio, Sound) is what keeps a
     * far end quiet permanently.
     */
    default void setMuted(boolean muted) {
    }

    /**
     * What the backend can report about the live connection, in display order:
     * desktop name, protocol, connection, security, encoding, line speed, then
     * diagnostics. Shown in the connection panel.
     *
     * <p>Asynchronous because gathering this means asking the library on the
     * thread it requires, and RealVNC's is the one session thread, which can be
     * inside a connect or blocked behind an unanswered password prompt.
     *
     * @param callback called once, on the main thread, with the facts in display
     *                 order, or an empty list when nothing is connected. May
     *                 never be called if the session is wedged, so nothing on
     *                 screen may depend on it arriving.
     */
    void connectionInfo(Consumer<List<ConnectionFact>> callback);

    /** Zero until the first {@link Listener#desktopSize}. */
    int desktopWidth();

    int desktopHeight();

    /**
     * Everything about a live session that has to be <em>asked for</em> rather
     * than waited for, in one answer.
     *
     * <p>None of these has an event to attach a callback to. An RFB server
     * states its monitor layout in a rectangle that arrives after the
     * connection if it arrives at all; a desktop gains a screen when one is
     * plugged into it and reports it the same way; and whether the far end
     * will take a size becomes true some time after the connection, so a
     * one-shot callback that fired too early would never fire again. A caller
     * must expect every one of these to change while a session runs, and to
     * differ between two sessions on the same backend.
     *
     * <p>One record rather than an accessor each, for three reasons: a poll is
     * one crossing into the library instead of one per field, "has anything
     * moved" is one {@code equals} instead of a remembered field per caller,
     * and a backend answers in one place, which also keeps two of these from
     * disagreeing, since they are read from the far end's state at one instant.
     *
     * @param monitors  how the far end's <em>one</em> desktop is divided, or
     *                  empty where it has not said (which is not the same as
     *                  one monitor, and is the answer for a library that never
     *                  exposes the layout)
     * @param displays  the pictures the far end is <em>not</em> sending and
     *                  will send if asked; empty for every protocol but one.
     *                  Not {@link #monitors}, and the difference is which
     *                  picture is on screen: a monitor is a region of the
     *                  framebuffer served by everything else here, so a caller
     *                  with the layout can jump the viewport between heads and
     *                  the pixels are already there. Choosing a display is a
     *                  message, a new size and a new framebuffer, and its
     *                  rectangles are in the far end's own coordinates only so
     *                  that a person can tell which screen is which.
     * @param display   which of {@link #displays} is on screen, or -1 where
     *                  there is no choice
     * @param canResize whether the far end will take a new desktop size right
     *                  now. Deliberately not a {@link BackendOption}: an option
     *                  is a static description a backend gives of itself before
     *                  anything is connected, and this is a fact about one live
     *                  session.
     * @param viewOnly  {@link #viewOnly()}, which is also its own accessor
     *                  (the one a screen reacting to the panel calls); this is
     *                  the same answer as of this instant
     * @param pointerRelative whether the far end owns the cursor. Pushed as
     *                  {@link Listener#pointerMode} while a screen is attached,
     *                  and here for a screen that attaches to a session that is
     *                  already running
     */
    record Facts(int desktopWidth, int desktopHeight, List<Monitor> monitors,
                 List<Monitor> displays, int display, boolean canResize,
                 boolean viewOnly, boolean pointerRelative) {

        public Facts {
            monitors = List.copyOf(monitors);
            displays = List.copyOf(displays);
        }

        /** Nothing connected: the answer for a session that is over. */
        public static final Facts NONE =
                new Facts(0, 0, List.of(), List.of(), -1, false, false, false);
    }

    /** @see Facts */
    Facts facts();

    /**
     * Ask the far end to send another of its displays.
     *
     * <p>A request, like {@link #requestDesktopSize}: the far end may answer
     * with a different one or with nothing, and what actually happened arrives
     * as {@link Listener#desktopSize} and in the next {@link Facts#display}.
     */
    default void requestDisplay(int index) {
    }

    /**
     * Ask the far end to make the desktop this size.
     *
     * <p>A request, and the far end decides: it may refuse, or grant a
     * different size than asked. What actually happened arrives as
     * {@link Listener#desktopSize}, or as nothing at all when the answer is no,
     * so a caller reports the size it has rather than the size it asked for.
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
     * <p>Only meaningful while {@link Facts#pointerRelative}; a backend without
     * a relative pointer ignores it. No caller may choose between the two:
     * which one carries the pointer is the far end's decision, announced
     * through {@link Listener#pointerMode}.
     *
     * <p>A default, unlike almost everything else here, because for every
     * protocol but RFB and SPICE the cursor is ours. A backend that does not
     * override it is absolute.
     */
    default void pointerRelative(int dx, int dy, int buttonMask) {
    }

    /**
     * @param keysym an X11 keysym ({@code control.input.Keysym})
     * @param keyId  a stable identity for the physical key: any value, as long
     *               as the same key uses the same one. A library keeping a map
     *               of held keys keys it on this and releases the keysym
     *               recorded at press time, so it ties a release to its press.
     */
    void keyDown(int keysym, int keyId);

    void keyUp(int keyId);

    /**
     * Let go of every key this session still has down at the far end.
     *
     * <p>A sweep rather than the ordinary path: the caller releases each key it
     * knows about by id, and this is for the ones it has lost track of (a
     * screen taken away mid-chord, an event stream that stopped between a
     * press and its release). "Held" means held at the far end, until a release
     * arrives, so without this a Ctrl can stay down for the rest of the
     * session.
     *
     * <p>Every backend keeps the map this needs, because a release names a key
     * and the protocols carry a keysym. The native backends expose it and
     * RealVNC's core does not, so that backend keeps its own copy of the ids.
     */
    void releaseAllKeys();

    /**
     * Whether the session is in the foreground. Backends may pause updates when
     * it is not, so a backgrounded session stops costing traffic:
     * RealVNC's core stops asking for framebuffer updates at the end of the one
     * in flight, and remembers a request that fell due meanwhile.
     *
     * <p>Deliberately <em>not</em> where keys are let go: whether losing the
     * screen should release them is a question about this phone rather than
     * about a protocol, so it is one app setting and one
     * {@link #releaseAllKeys} call rather than the same decision made in every
     * backend.
     */
    void focus(boolean focused);

    void clipboardToRemote(String text);

    // ---- pixels -----------------------------------------------------------

    /**
     * Copy the desktop rectangle {@code (x, y, width, height)} into {@code dst}
     * at {@code (dstX, dstY)}, 1:1. <b>The rest of {@code dst} is left as it
     * was</b>, so a caller can hold a picture bigger than any one read and
     * refresh only the parts that changed.
     *
     * <p>That is the reason for the offset. Without it a caller reading less
     * than a whole bitmap has to read into a scratch bitmap and blit the result
     * into place: a copy of everything that changed, on the drawing thread.
     * With it, a backend that owns its framebuffer writes the rows straight
     * into the destination.
     *
     * <p>Callable from the drawing thread at any time. Returns false, leaving
     * {@code dst} untouched, if there is nothing to read yet or the rectangle
     * does not fit.
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
     * {@code maxHeight}, as a thumbnail for the home screen.
     *
     * <p>Returns a new bitmap, whose exact size the backend chooses within the
     * bounds given, or null if there is nothing to show yet. Backends may
     * quantise the scale, since this is a preview; RealVNC's can only
     * downscale by powers of two.
     */
    Bitmap thumbnail(int maxWidth, int maxHeight);
}
