// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend.freerdp;

import android.graphics.Bitmap;

/**
 * The JNI surface of {@code libremotedesktop_freerdp.so} — FreeRDP's own client
 * behind the shim in {@code src/main/cpp}.
 *
 * <p>{@code RdpNative}'s twin method for method, because that is what makes the
 * two RDP clients comparable: anything the app can ask one it can ask the other,
 * and a difference in what comes back is a difference between the libraries.
 * {@link #nativeCreate}'s arguments are where they diverge, and every divergence
 * is something the other client cannot do at all — the graphics pipeline, sound,
 * the interface size the far end draws at, and a directory for the store FreeRDP
 * keeps whether or not anything reads it.
 *
 * <h2>Threads</h2>
 * <ul>
 *   <li>Every {@link Callbacks} method arrives on the protocol thread.
 *   <li>{@link #nativeReadRegion} and {@link #nativeReadThumbnail} are called
 *       from whichever thread is drawing, concurrently with everything else.
 *   <li>Everything else may be called from anywhere: it is queued for the one
 *       thread that owns the connection, so a stalled socket never reaches the
 *       main thread.
 * </ul>
 *
 * <h2>The handle</h2>
 * {@code nativeDestroy} frees the session and joins its thread, so nothing may
 * hold a handle across it. {@link FreeRdpBackend} retires the handle under its
 * own lock first.
 */
final class FreeRdpNative {

    private FreeRdpNative() {
    }

    static {
        System.loadLibrary("remotedesktop_freerdp");
    }

    /** Everything the session says. Protocol thread, always. */
    interface Callbacks {

        void onConnected(int width, int height);

        void onDesktopSize(int width, int height);

        void onDamage(int x, int y, int width, int height);

        void onFrameEnd();

        /**
         * @param argb {@code width * height} pixels, {@code Color}-packed;
         *             null when the server has hidden the pointer or asked for
         *             the default one, both of which mean "draw your own"
         * @param hash of the pixels, computed where they were converted: the
         *             identity {@code CursorCache} keeps the built bitmap under
         */
        void onCursor(int[] argb, int width, int height, int hotX, int hotY, long hash);

        /** Never called: RDP has no relative pointer. */
        void onPointerMode(boolean relative);

        /** Never called: nothing in this client rings one. */
        void onBell();

        void onClipboard(String text);

        /**
         * The connection needs credentials, either because none were stored or
         * because the ones that were did not work — FreeRDP asks again itself,
         * which the other RDP client does not.
         */
        void onCredentialsNeeded(boolean needsUserName);

        /**
         * The server's certificate, after the TLS handshake and before anything
         * secret is sent.
         *
         * @param fingerprint SHA-256, as {@code openssl x509 -fingerprint} prints it
         */
        void onTrustNeeded(String fingerprint);

        /** Terminal. Empty {@code detail} for an ordinary disconnect. */
        void onClosed(String detail);
    }

    /** What the client is — the one call that proves the whole path. */
    static native String nativeVersion();

    /**
     * Start connecting. Returns a handle immediately; everything after this
     * arrives through {@code listener}.
     *
     * @param nla        {@code prefer}, {@code require} or {@code off}
     * @param graphics   {@code gfx}, {@code gfx-novideo}, {@code rfx} or
     *                   {@code bitmap}
     * @param experience {@code full}, {@code balanced} or {@code plain}
     * @param sound      {@code off}, {@code local} or {@code remote}
     * @param scale      the percentage the far end is asked to draw its own
     *                   interface at; 100, 140 and 180 are the only values the
     *                   protocol allows, and anything else is taken as 100
     * @param configPath a writable directory: the library keeps a certificate
     *                   store there, and one it cannot write is a connection
     *                   that fails as a TLS error
     */
    static native long nativeCreate(Callbacks listener, String address, String userName,
                                    String domain, String password, String nla,
                                    boolean compression, String graphics, String experience,
                                    String sound, int scale, int width, int height, int monitors,
                                    int keyboardLayout, String clientName, String configPath,
                                    int connectTimeoutMs);

    /**
     * Whether the display control channel is open, which is the only way an RDP
     * desktop is reshaped after connect.
     */
    static native boolean nativeCanResize(long handle);

    /** Ask for a desktop of this size per monitor. */
    static native void nativeRequestDesktopSize(long handle, int width, int height);

    /** How many monitors the next layout asks for. */
    static native void nativeSetMonitorCount(long handle, int count);

    /**
     * The monitors the desktop is made of, flattened four ints each. Null for a
     * single-monitor session and for a layout the server did not grant.
     */
    static native int[] nativeMonitors(long handle);

    /** A {@code null} password cancels, which ends the session. */
    static native void nativeAnswerCredentials(long handle, String userName, String domain,
                                               String password);

    /** Whether the certificate {@link Callbacks#onTrustNeeded} named is trusted. */
    static native void nativeAnswerTrust(long handle, boolean accept);

    static native void nativeDisconnect(long handle);

    /** Frees the handle and joins the protocol thread. Nothing may use it after. */
    static native void nativeDestroy(long handle);

    static native void nativePointer(long handle, int x, int y, int buttonMask);

    static native void nativeKeyDown(long handle, int keysym, long keyId);

    static native void nativeKeyUp(long handle, long keyId);

    /** Let go of every key still held — a screen going away mid-chord. */
    static native void nativeReleaseAllKeys(long handle);

    /**
     * Whether the session is on screen. A Suppress Output PDU: an RDP server
     * sends what it likes until it is told not to.
     */
    static native void nativeFocus(long handle, boolean focused);

    static native void nativeViewOnly(long handle, boolean viewOnly);

    /**
     * What this phone has copied, offered to the remote machine. RDP's clipboard
     * is a delayed rendering: this announces that there is text, and the bytes
     * go only if something over there pastes.
     */
    static native void nativeClipboard(long handle, String text);

    /** Protocol, connection, security, encoding, line speed, server and viewer pixels. */
    static native String[] nativeInfo(long handle);

    /** Received and sent since the connection was made, or null if there is no session. */
    static native long[] nativeTraffic(long handle);

    /**
     * Copy the desktop rectangle into {@code dst} at {@code (dstX, dstY)}, 1:1,
     * leaving the rest of {@code dst} alone.
     */
    static native boolean nativeReadRegion(long handle, int x, int y, int width, int height,
                                           Bitmap dst, int dstX, int dstY);

    /** The whole desktop at {@code 1/step}, nearest neighbour. */
    static native boolean nativeReadThumbnail(long handle, int step, Bitmap dst);
}
