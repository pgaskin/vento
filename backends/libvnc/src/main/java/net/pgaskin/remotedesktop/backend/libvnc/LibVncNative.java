// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend.libvnc;

import android.graphics.Bitmap;

/**
 * The JNI surface of {@code libremotedesktop_libvnc.so} — LibVNCServer's
 * {@code libvncclient} and the shim in {@code src/main/cpp} that adapts it.
 *
 * <p>Deliberately the same surface {@code RfbNative} describes, minus what
 * libvncclient does not have, so that everything above it is the same code
 * against either client and a comparison is between the clients. What is
 * missing, and why:
 *
 * <ul>
 *   <li><b>No {@code nativePointerRelative} and no pointer mode.</b> QEMU's
 *       {@code PointerTypeChange} pseudo-encoding is not among the encodings
 *       libvncclient knows, so a server that wants to own the cursor cannot say
 *       so and this client is always absolute.
 *   <li><b>{@code nativeAnswerTrust} is answered from a fingerprint the library
 *       computes</b>, rather than one this side takes from a certificate: what
 *       arrives once verification has failed is a subject, a validity and a
 *       SHA-256, and the shim formats the last of those the way the other
 *       backends pin it.
 * </ul>
 *
 * <h2>Threads</h2>
 * <ul>
 *   <li>Every {@link Callbacks} method arrives on the protocol thread.
 *   <li>{@link #nativeReadRegion} and {@link #nativeReadThumbnail} are called
 *       from whichever thread is drawing. They take a reader lock over a copy
 *       of the framebuffer the shim keeps, because libvncclient's own has no
 *       lock and its decoder cannot be interrupted.
 *   <li>Everything else may be called from anywhere and is queued: nothing in
 *       libvncclient is safe to call from a second thread, so one thread owns
 *       the session and drains a queue between messages.
 * </ul>
 */
final class LibVncNative {

    private LibVncNative() {
    }

    static {
        System.loadLibrary("remotedesktop_libvnc");
    }

    /** Everything the session says. Protocol thread, always. */
    interface Callbacks {

        void onConnected(int width, int height);

        void onDesktopSize(int width, int height);

        void onDamage(int x, int y, int width, int height);

        void onFrameEnd();

        /**
         * @param argb {@code width * height} pixels, {@code Color}-packed,
         *             alpha from the cursor's mask
         * @param hotX the hotspot, positively
         * @param hash of the pixels, computed where they were filled in: the
         *             identity {@code CursorCache} keeps the built bitmap
         *             under, so an unchanged shape costs no bitmap at all
         */
        void onCursor(int[] argb, int width, int height, int hotX, int hotY, long hash);

        void onBell();

        void onClipboard(String text);

        /**
         * The server wants credentials and none were stored. The session is
         * stopped in the handshake until {@link #nativeAnswerCredentials}.
         *
         * <p>Unlike the client we wrote, this one asks <em>once</em>: a stored
         * password that the server refuses ends the connection rather than
         * bringing the question back, because libvncclient's
         * {@code GetPassword} is not called again after an authentication
         * failure.
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

        /** Terminal. Empty {@code detail} for an ordinary disconnect. */
        void onClosed(String detail);
    }

    /** The library's version — the one call that proves the whole path. */
    static native String nativeVersion();

    /**
     * Start connecting. Returns a handle immediately; everything after this
     * arrives through {@code listener}.
     *
     * @param encoding      a libvncclient encodings string, best first
     * @param compressLevel 0–9, or −1 for the server's own default
     * @param qualityLevel  0–9 for Tight's JPEG, or −1 for lossless — which is
     *                      the absence of the quality pseudo-encoding rather
     *                      than a value of it
     * @param colorLevel    0 for the 32-bit format that makes a region read a
     *                      row copy, 1 for 64 colours, 2 for 8. Connection-time:
     *                      libvncclient has nowhere safe to change a pixel
     *                      format mid-stream
     */
    static native long nativeCreate(Callbacks listener, String address, String userName,
                                    String password, boolean shared, String encoding,
                                    int compressLevel, int qualityLevel, int colorLevel,
                                    int connectTimeoutMs);

    /** A {@code null} password cancels, which ends the session. */
    static native void nativeAnswerCredentials(long handle, String userName, String password);

    /** Whether the certificate {@link Callbacks#onTrustNeeded} named is trusted. */
    static native void nativeAnswerTrust(long handle, boolean accept);

    static native void nativeDisconnect(long handle);

    /** Frees the handle and joins the protocol thread. Nothing may use it after. */
    static native void nativeDestroy(long handle);

    static native void nativePointer(long handle, int x, int y, int buttonMask);

    static native void nativeKeyDown(long handle, int keysym, int keyId);

    static native void nativeKeyUp(long handle, int keyId);

    /** Let go of every key still held — a screen going away mid-chord. */
    static native void nativeReleaseAllKeys(long handle);

    /**
     * Whether to keep reading. libvncclient asks for the next update at the end
     * of each one it handles, so leaving the last one unread is the whole of the
     * pause: one update sits in the socket and the server then waits.
     */
    static native void nativeFocus(long handle, boolean focused);

    static native void nativeViewOnly(long handle, boolean viewOnly);

    /**
     * Whether an {@code ExtDesktopSize} rectangle has arrived, which is all
     * libvncclient leaves behind of one and the only sign that this server takes
     * a size from the client.
     */
    static native boolean nativeCanResize(long handle);

    /**
     * The screen layout out of the same rectangle, flattened four ints per
     * screen — x, y, width, height. Empty when none has arrived.
     */
    static native int[] nativeMonitors(long handle);

    /**
     * Ask for a desktop this size. The answer arrives as
     * {@link Callbacks#onDesktopSize} where it is granted and as nothing at all
     * where it is not — libvncclient reads the result code out of the rectangle
     * and discards it.
     */
    static native void nativeRequestDesktopSize(long handle, int width, int height);

    static native void nativeClipboard(long handle, String text);

    /** Live: {@code SetEncodings} may be sent at any point in a session. */
    static native void nativeSetEncodings(long handle, String encoding, int compressLevel,
                                          int qualityLevel);

    /** Desktop name, protocol, connection, security, encoding, server and viewer pixels. */
    static native String[] nativeInfo(long handle);

    /**
     * Received and sent since the socket was opened, or -1 for both while
     * there is no client. Null if the session has gone.
     */
    static native long[] nativeTraffic(long handle);

    /**
     * Copy the desktop rectangle into {@code dst} at {@code (dstX, dstY)}, 1:1.
     * The rows come out of the shim's copy of the framebuffer, under its reader
     * lock, straight into the destination's own rows.
     */
    static native boolean nativeReadRegion(long handle, int x, int y, int width, int height,
                                           Bitmap dst, int dstX, int dstY);

    /**
     * The whole desktop at {@code 1/step}, nearest neighbour, into a bitmap the
     * caller has already sized to {@code ceil(w/step) × ceil(h/step)}.
     */
    static native boolean nativeReadThumbnail(long handle, int step, Bitmap dst);
}
