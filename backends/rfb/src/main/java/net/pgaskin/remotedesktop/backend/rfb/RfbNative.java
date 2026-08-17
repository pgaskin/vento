// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend.rfb;

import android.graphics.Bitmap;

/**
 * The JNI surface of {@code libremotedesktop_rfb.so} — the RFB client in
 * {@code src/main/rust}, bound in its {@code src/bindings} and compiled by this
 * module's {@code cargoBuild*Native}.
 *
 * <p>Both halves of this one are in this repository and change together, so it
 * is shaped for this app rather than described from a binary: a handle, a
 * listener, and the two pixel calls the drawing thread makes.
 *
 * <h2>Threads</h2>
 * <ul>
 *   <li>Every {@link Callbacks} method arrives on the protocol thread, which is
 *       the Rust side's own and never the main one.
 *   <li>{@link #nativeReadRegion} and {@link #nativeReadThumbnail} are called
 *       from whichever thread is drawing, concurrently with everything else.
 *       They take the framebuffer's read lock and copy rows straight into the
 *       bitmap.
 *   <li>Everything else may be called from anywhere; input is encoded on the
 *       calling thread and written on a thread of the binding's own, so a
 *       stalled socket cannot reach the main thread.
 * </ul>
 *
 * <p>{@code nativeDestroy} frees the session and joins its thread, so nothing
 * may hold a handle across it: {@link RfbBackend} retires the handle under its
 * own lock first, and the join is what makes the callbacks provably finished.
 */
final class RfbNative {

    private RfbNative() {
    }

    static {
        System.loadLibrary("remotedesktop_rfb");
    }

    /** Everything the session says. Protocol thread, always. */
    interface Callbacks {

        void onConnected(int width, int height);

        void onDesktopSize(int width, int height);

        void onDamage(int x, int y, int width, int height);

        void onFrameEnd();

        /**
         * @param argb   {@code width * height} pixels, {@code Color}-packed,
         *               alpha from the cursor's mask; empty when the server has
         *               hidden the pointer
         * @param hotX   the hotspot, positively. The negated one RealVNC's
         *               API hands out is their own rewriting of the rectangle,
         *               not the protocol's
         * @param hash   of the pixels, computed where they were filled in: the
         *               identity {@code CursorCache} keeps the built bitmap
         *               under, so an unchanged shape costs no bitmap at all
         */
        void onCursor(int[] argb, int width, int height, int hotX, int hotY, long hash);

        /**
         * The server has said which end owns the cursor — absolute at first,
         * and relative if it ever asks. Arrives on the protocol thread,
         * unprompted, shortly after the connection.
         */
        void onPointerMode(boolean relative);

        void onBell();

        void onClipboard(String text);

        /**
         * The server wants credentials and none were stored, or the stored ones
         * were refused. The session is stopped in the handshake until
         * {@link #nativeAnswerCredentials} is called — that is the design, since
         * on this side of it is a dialog and a person.
         *
         * @param needsUserName false for VncAuth, which has no notion of one
         */
        void onCredentialsNeeded(boolean needsUserName);

        /**
         * The server's certificate, before anything secret is sent over the
         * tunnel it belongs to. Nothing vouches for it — a VNC server signs its
         * own — so the fingerprint is the identity, and the answer comes from
         * the pin store and, where that has nothing to say, from a person
         * ({@link #nativeAnswerTrust}).
         *
         * @param fingerprint SHA-256 of the certificate, as
         *                    {@code openssl x509 -fingerprint -sha256} prints it
         */
        void onTrustNeeded(String fingerprint);

        /**
         * Never called: this client refuses the security types that have no
         * server identity in them, rather than asking whether to go on without
         * one. Declared because the interface is the one every backend answers.
         */
        void onUnverified(String why);

        /** Terminal. Empty {@code detail} for an ordinary disconnect. */
        void onClosed(String detail);
    }

    /** The RFB version the client speaks: the one call that proves the whole path. */
    static native String nativeVersion();

    /**
     * Start connecting. Returns a handle immediately; everything after this
     * arrives through {@code listener}.
     *
     * @param security      {@code prefer}, {@code require} or {@code plain} —
     *                      whether TLS is taken where it is offered, insisted
     *                      on, or refused
     * @param encoding      {@code auto}, {@code zrle}, {@code hextile},
     *                      {@code rre} or {@code raw}
     * @param compressLevel 0–9, or anything outside that for the server's own
     *                      default
     */
    static native long nativeCreate(Callbacks listener, String address, String userName,
                                    String password, String security, boolean shared,
                                    String encoding, int compressLevel, int connectTimeoutMs);

    /** A {@code null} password cancels, which ends the session. */
    static native void nativeAnswerCredentials(long handle, String userName, String password);

    /** Whether the certificate {@link Callbacks#onTrustNeeded} named is trusted. */
    static native void nativeAnswerTrust(long handle, boolean accept);

    static native void nativeDisconnect(long handle);

    /** Frees the handle and joins the protocol thread. Nothing may use it after. */
    static native void nativeDestroy(long handle);

    static native void nativePointer(long handle, int x, int y, int buttonMask);

    /**
     * A delta rather than a place, for a server that has asked for one
     * ({@link Callbacks#onPointerMode}). Same message on the wire, coordinates
     * biased by {@code 0x7FFF}.
     */
    static native void nativePointerRelative(long handle, int dx, int dy, int buttonMask);

    static native boolean nativePointerIsRelative(long handle);

    /**
     * Whether the server has sent an {@code ExtendedDesktopSize} rectangle,
     * which is the only announcement there is that it takes a size from this
     * end. False for a view-only session, whose promise not to touch the far
     * end covers the shape of it too.
     */
    static native boolean nativeCanResize(long handle);

    /**
     * The screen layout the same rectangle carries, flattened four ints per
     * screen — x, y, width, height. Empty when none has arrived.
     */
    static native int[] nativeMonitors(long handle);

    /**
     * Ask for a desktop this size, as one screen covering it. The answer is
     * {@link Callbacks#onDesktopSize} or nothing at all.
     */
    static native void nativeRequestDesktopSize(long handle, int width, int height);

    static native void nativeKeyDown(long handle, int keysym, long keyId);

    static native void nativeKeyUp(long handle, long keyId);

    /** Let go of every key still held — a screen going away mid-chord. */
    static native void nativeReleaseAllKeys(long handle);

    /**
     * Whether to keep asking for framebuffer updates. An RFB server sends
     * nothing that was not asked for, so not asking is the whole of the pause.
     */
    static native void nativeFocus(long handle, boolean focused);

    static native void nativeViewOnly(long handle, boolean viewOnly);

    static native void nativeClipboard(long handle, String text);

    /** Live: {@code SetEncodings} may be sent at any point in a session. */
    static native void nativeSetEncodings(long handle, String encoding, int compressLevel);

    /**
     * Desktop name, protocol, connection, security, encoding, line speed,
     * server and viewer pixels.
     */
    static native String[] nativeInfo(long handle);

    /** Received and sent since the socket was opened, or null if the session has gone. */
    static native long[] nativeTraffic(long handle);

    /**
     * Copy the desktop rectangle into {@code dst} at {@code (dstX, dstY)}, 1:1,
     * with no intermediate buffer: the pixels are locked with
     * {@code jnigraphics} and the rows copied out of the framebuffer under its
     * read lock, straight into the destination's own rows.
     */
    static native boolean nativeReadRegion(long handle, int x, int y, int width, int height,
                                           Bitmap dst, int dstX, int dstY);

    /**
     * The whole desktop at {@code 1/step}, nearest neighbour, into a bitmap the
     * caller has already sized to {@code ceil(w/step) × ceil(h/step)}.
     *
     * <p>Any integer step, unlike RealVNC's, whose powers-of-two constraint
     * was a consequence of {@code (int)(1/scale)} inside their scaler: there is
     * no float here at all.
     */
    static native boolean nativeReadThumbnail(long handle, int step, Bitmap dst);
}
