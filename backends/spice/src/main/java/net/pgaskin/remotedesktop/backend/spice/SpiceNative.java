// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend.spice;

import android.graphics.Bitmap;

/**
 * The JNI surface of {@code libremotedesktop_spice.so} — the SPICE client in
 * {@code src/main/rust}, bound in its {@code src/bindings} and compiled by this
 * module's {@code cargoBuild*Native}.
 *
 * <p>Both halves of this one are in this repository and change together, so it
 * is shaped for this app rather than described from a binary: a handle, a
 * listener, and the two pixel calls the drawing thread makes.
 *
 * <h2>Threads</h2>
 * <ul>
 *   <li>Every {@link Callbacks} method arrives on the session thread, which is
 *       the Rust side's own — one thread with one runtime on it, whatever a
 *       session's four connections are doing — and never the main one.
 *   <li>{@link #nativeReadRegion} and {@link #nativeReadThumbnail} are called
 *       from whichever thread is drawing, concurrently with everything else.
 *       They take the framebuffer's read lock, which is only ever held against
 *       them for one blit.
 *   <li>Everything else may be called from anywhere: input is put on a queue
 *       and the calling thread returns, so a stalled socket cannot reach the
 *       main thread.
 * </ul>
 *
 * <p>{@code nativeDestroy} frees the session and joins its thread, so nothing
 * may hold a handle across it: {@link SpiceBackend} retires the handle under
 * its own lock first, and the join is what makes the callbacks provably
 * finished.
 */
final class SpiceNative {

    private SpiceNative() {
    }

    static {
        System.loadLibrary("remotedesktop_spice");
    }

    /**
     * Everything the session says. Session thread, always.
     *
     * <p>Two of these are never called here and are declared because the
     * interface is the same one every backend answers: this protocol has no
     * bell, and a server whose identity cannot be checked at all is not a case
     * it has — a plain port asserts no identity and a TLS one presents a
     * certificate. The Rust side resolves every method at create time, so a
     * shorter interface would be a different one.
     */
    interface Callbacks {

        void onConnected(int width, int height);

        void onDesktopSize(int width, int height);

        void onDamage(int x, int y, int width, int height);

        void onFrameEnd();

        /**
         * @param argb {@code width * height} pixels, {@code Color}-packed, or
         *             empty where the guest has hidden the cursor
         * @param hash of the pixels, which is the identity {@code CursorCache}
         *             keeps the built bitmap under
         */
        void onCursor(int[] argb, int width, int height, int hotX, int hotY, long hash);

        /**
         * Which end owns the pointer. SPICE says so in the main channel and can
         * change it mid-session: a guest with a tablet attached lets the client
         * choose and one without owns the cursor itself.
         */
        void onPointerMode(boolean relative);

        /** Never called: there is no bell in this protocol. */
        void onBell();

        /**
         * The guest's clipboard, which arrives only where the guest is running
         * the agent — the protocol itself carries no such thing.
         */
        void onClipboard(String text);

        /**
         * The ticket was refused, or the server wants one and was given none.
         * The session is stopped until {@link #nativeAnswerPassword} — and then
         * it dials again, because the key a ticket is encrypted to is made per
         * connection and the one this ticket was refused on is spent.
         *
         * @param needsUserName always false: SPICE has no user name
         */
        void onCredentialsNeeded(boolean needsUserName);

        /**
         * The TLS certificate, by fingerprint, after the handshake and before
         * the ticket goes out. The session waits for
         * {@link #nativeAnswerTrust}.
         *
         * <p>A plain session never asks: the key in its link reply is generated
         * per connection to encrypt one ticket and identifies nothing, so there
         * is nothing to put the question about.
         */
        void onTrustNeeded(String fingerprint);

        /** Never called: see {@link #onTrustNeeded}. */
        void onUnverified(String why);

        /** Terminal. Empty {@code detail} for an ordinary disconnect. */
        void onClosed(String detail);
    }

    /** The protocol version this client speaks, and the proof of the path. */
    static native String nativeVersion();

    /**
     * Start connecting. Returns a handle immediately; everything after this
     * arrives through {@code listener}.
     *
     * @param address     {@code host:port}
     * @param tls         whether that port is a TLS port, which is a different
     *                    port rather than a negotiation
     * @param compression which image compression to ask the server for, or
     *                    empty for whatever it chooses
     */
    static native long nativeCreate(Callbacks listener, String address, boolean tls,
                                    String password, String compression, boolean viewOnly,
                                    int connectTimeoutMs);

    /** A {@code null} password cancels, which ends the session. */
    static native void nativeAnswerPassword(long handle, String password);

    /** Whether the certificate {@link Callbacks#onTrustNeeded} named is trusted. */
    static native void nativeAnswerTrust(long handle, boolean accept);

    static native void nativeDisconnect(long handle);

    /** Frees the handle and joins the session thread. Nothing may use it after. */
    static native void nativeDestroy(long handle);

    static native void nativePointer(long handle, int x, int y, int buttonMask);

    /** Only meaningful while {@link #nativePointerIsRelative}. */
    static native void nativePointerRelative(long handle, int dx, int dy, int buttonMask);

    static native boolean nativePointerIsRelative(long handle);

    static native void nativeKeyDown(long handle, int keysym, long keyId);

    static native void nativeKeyUp(long handle, long keyId);

    /** Let go of every key still held — a screen going away mid-chord. */
    static native void nativeReleaseAllKeys(long handle);

    /**
     * Whether the session is on screen. There is no pause message in this
     * protocol, so an unfocused session stops answering the server's ack
     * window, which is the server's own reason to stop sending.
     */
    static native void nativeFocus(long handle, boolean focused);

    static native void nativeViewOnly(long handle, boolean viewOnly);

    /** Live: the next image the server encodes uses it. */
    static native void nativeSetCompression(long handle, String compression);

    /**
     * What this phone has on its clipboard, offered to the guest's agent. The
     * text crosses when the guest asks for it, which is the agent's shape and
     * not this seam's; a guest with no agent takes nothing.
     */
    static native void nativeClipboard(long handle, String text);

    /**
     * Whether the guest will take a desktop size right now — which is whether
     * the agent is running in it and said it does monitors, and can turn true
     * seconds after the picture arrives.
     */
    static native boolean nativeCanResize(long handle);

    /** A request: the agent hands it to the guest's own display machinery. */
    static native void nativeRequestDesktopSize(long handle, int width, int height);

    /**
     * The far end's monitors as {@code x, y, width, height} each, or null where
     * it has not published a layout — which is announced here rather than
     * inferred from a screen boundary, as it is everywhere else.
     */
    static native int[] nativeMonitors(long handle);

    /** Received and sent since the sockets were opened, or null if they have gone. */
    static native long[] nativeTraffic(long handle);

    /**
     * Desktop name, protocol, connection, security, encoding, the channels that
     * linked, whether the guest agent is running, and what the picture is made
     * of.
     */
    static native String[] nativeInfo(long handle);

    /**
     * Copy the desktop rectangle into {@code dst} at {@code (dstX, dstY)}, 1:1,
     * with no intermediate buffer.
     */
    static native boolean nativeReadRegion(long handle, int x, int y, int width, int height,
                                           Bitmap dst, int dstX, int dstY);

    /**
     * The whole desktop at {@code 1/step}, nearest neighbour, into a bitmap the
     * caller has already sized to {@code ceil(w/step) × ceil(h/step)}.
     */
    static native boolean nativeReadThumbnail(long handle, int step, Bitmap dst);
}
