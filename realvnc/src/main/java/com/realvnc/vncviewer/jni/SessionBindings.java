// SPDX-License-Identifier: CC0-1.0

package com.realvnc.vncviewer.jni;

import java.nio.ByteBuffer;
import java.util.Map;

/**
 * One connection, and everything that happens to it: opening it, driving it,
 * reading its framebuffer, closing it.
 *
 * <p>Required by fourteen exported symbols
 * ({@code Java_com_realvnc_vncviewer_jni_SessionBindings_*}) and by three
 * classes the core resolves by name: {@code SessionBindings$Session},
 * {@code $SessionCallback} and {@code $DesktopCallback}.
 */
public final class SessionBindings {

    /**
     * The eight callbacks the core makes about the desktop, each resolved by
     * name on the object passed to {@link #createSession}.
     *
     * <p>They arrive on the thread that called {@link #createSession}, which is
     * where the core registers its socket, so that thread must be running a
     * {@code Looper} and must not be given slow work. An implementation that
     * wants them anywhere else marshals; nothing here posts.
     */
    public interface DesktopCallback {

        void bell();

        /** Connected and ready. */
        void connSuccess();

        /** A region changed: x, y, <em>width</em>, <em>height</em> — not a rect. */
        void drawRegion(int x, int y, int width, int height);

        void framebufferUpdateEnd();

        /** Our clipboard, for the server. */
        String getClipboard();

        void setClipboard(String text);

        /**
         * Cursor shape and the <em>negated</em> hotspot: the RFB parser rewrites
         * a cursor rectangle to {@code (-hot, size - hot)} on arrival and this
         * carries that rectangle's origin, so a drawer adds the hotspot back.
         *
         * <p>The bitmap is an {@code android.graphics.Bitmap}. It is typed as
         * {@code Object} because that is what the core constructs and hands
         * over, and naming the type here would not make it any more checked.
         */
        void setCursor(Object bitmap, int negHotX, int negHotY);

        /** Framebuffer size, initially and on every resize. */
        void setDesktopSize(int width, int height);
    }

    public interface SessionCallback {
        /** The session ended, for any reason — remote, error, or our own close. */
        void sessionClosed(Session session);
    }

    /**
     * An opaque handle. {@code token} is the native {@code AndroidSession*},
     * which the core reads back through {@code getToken} — a cached method id —
     * on every call, so the object handed to any native below has to be the one
     * {@link #createSession} returned.
     *
     * <p>Static, and it matters: the constructor the core calls is
     * {@code <init>(J)V}, with no outer instance to pass.
     */
    public static final class Session {

        private long token;

        Session(long token) {
            this.token = token;
        }

        /**
         * Public because it is the only way past this surface: the core keeps
         * everything it knows about a connection behind this pointer, and not
         * all of it is reachable through an exported function.
         */
        public long getToken() {
            return token;
        }

        void setToken(long token) {
            this.token = token;
        }
    }

    private SessionBindings() {
    }

    /**
     * Re-read the parameters that affect the encoder.
     *
     * <p>Needed after changing the quality group, and only then: the core acts
     * on a quality that has <em>changed</em>, and carries the rest of the group
     * with it. {@link #setOption} alone is enough for everything else.
     */
    public static native void applyOptions(Session session);

    /** Graceful disconnect. {@code sessionClosed} follows. */
    public static native void closeSession(Session session);

    /**
     * Scale a desktop rectangle into the session's scratch buffer, at the
     * <em>buffer's</em> row stride.
     *
     * <p>{@code scale} must be ≤ 1 and is quantised to {@code 1/(int)(1/scale)};
     * above 1 the native loop never advances.
     */
    public static native void copyScaledRegion(Session session, int x, int y,
                                               int width, int height, float scale);

    /**
     * Starts a connection and returns in a few milliseconds; the rest of it
     * arrives as callbacks on this thread. A failure reports nothing at all —
     * no callback, no status, no exception — so a caller needs a timeout of its
     * own.
     */
    public static native Session createSession(SessionCallback sessionCallback,
                                               DesktopCallback desktopCallback,
                                               String address, String password,
                                               Map<String, String> options);

    /** Frees the session and removes it from the native registry. */
    public static native void destroySession(Session session);

    /**
     * Whether the session should keep asking the server for updates. On
     * {@code false} the core stops at the end of the update in flight and
     * remembers a request that falls due meanwhile, so there is nothing else to
     * throttle when a viewer goes away.
     */
    public static native void focusEvent(Session session, boolean focused);

    /**
     * The keycode is a client-side identity that ties a release to its press;
     * the keysym is the payload. Throws if the keysym does not fit in 31 bits.
     */
    public static native void keyDownEvent(Session session, long keysym, int keycode);

    /** Releases the keysym recorded for {@code keycode} at press time. */
    public static native void keyUpEvent(Session session, int keycode);

    /** Absolute desktop coordinates, RFB button mask. Clamped natively. */
    public static native void pointerEvent(Session session, int x, int y, int buttonMask);

    public static native void setOption(Session session, String name, String value);

    /**
     * Allocates {@code width * height * 4} bytes natively and returns a direct
     * view of them, for {@link #copyScaledRegion} to write into. The buffer
     * belongs to the session; calling this again re-allocates.
     */
    public static native ByteBuffer setScaleBufferSize(Session session, int width, int height);
}
