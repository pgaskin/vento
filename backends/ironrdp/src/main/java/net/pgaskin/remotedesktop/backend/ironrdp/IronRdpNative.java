// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend.ironrdp;

import android.graphics.Bitmap;

/**
 * The JNI surface of {@code libremotedesktop_ironrdp.so} — IronRDP wrapped in
 * {@code src/main/rust} and bound in its {@code src/bindings}.
 *
 * <p>Deliberately {@code RfbNative}'s twin: same handle, same listener, same two
 * pixel calls from the drawing thread. Where they differ, the protocol is why —
 * a domain beside the user name, a desktop size asked for rather than
 * discovered, and a graphics choice made once at connect time because that is
 * when RDP makes it.
 *
 * <h2>Threads</h2>
 * <ul>
 *   <li>Every {@link Callbacks} method arrives on the protocol thread.
 *   <li>{@link #nativeReadRegion} and {@link #nativeReadThumbnail} are called
 *       from whichever thread is drawing, concurrently with everything else.
 *   <li>Everything else may be called from anywhere; input is encoded on the
 *       calling thread and written on a thread of the binding's own, so a
 *       stalled socket cannot reach the main thread.
 * </ul>
 *
 * <h2>The handle</h2>
 * {@code nativeDestroy} frees the session and joins its thread, so nothing may
 * hold a handle across it. {@link IronRdpBackend} retires the handle under its own
 * lock first.
 */
final class IronRdpNative {

    private IronRdpNative() {
    }

    static {
        System.loadLibrary("remotedesktop_ironrdp");
    }

    /** Everything the session says. Protocol thread, always. */
    interface Callbacks {

        void onConnected(int width, int height);

        void onDesktopSize(int width, int height);

        void onDamage(int x, int y, int width, int height);

        void onFrameEnd();

        /**
         * @param argb {@code width * height} pixels, {@code Color}-packed;
         *             empty when the server has hidden the pointer
         * @param hotX the hotspot, positively
         * @param hash of the pixels, computed where they were filled in: the
         *             identity {@code CursorCache} keeps the built bitmap
         *             under, so an unchanged shape costs no bitmap at all
         */
        void onCursor(int[] argb, int width, int height, int hotX, int hotY, long hash);

        /**
         * Never called: RDP has no relative pointer and this session is
         * absolute for the whole of its life. It is declared because one Rust
         * {@code Callbacks} resolves both backends' interfaces by name, and
         * the two being the same shape is the property that keeps it one.
         */
        void onPointerMode(boolean relative);

        /**
         * Never called: nothing in the RDP client rings one. Declared for the
         * same reason {@link #onPointerMode} is — one Rust {@code Callbacks}
         * resolves both backends' interfaces by name.
         */
        void onBell();

        void onClipboard(String text);

        /**
         * The connection needs credentials and none were stored.
         *
         * <p>Asked <b>before anything is connected</b>, unlike the RFB
         * backend's: RDP carries the user name and password in the connection
         * sequence rather than being asked for them by the server, so there is
         * no point at which a server asks.
         *
         * @param needsUserName always true here; RDP has no scheme without one
         */
        void onCredentialsNeeded(boolean needsUserName);

        /**
         * The server's certificate, after the TLS handshake and before anything
         * secret is sent. Nothing vouches for it — an xrdp signs its own and a
         * Windows host's is signed by nothing a phone has heard of — so the
         * fingerprint is the identity, and the answer comes from the pin store
         * and, where that has nothing to say, from a person.
         *
         * @param fingerprint SHA-256, as {@code openssl x509 -fingerprint} prints it
         */
        void onTrustNeeded(String fingerprint);

        /** Terminal. Empty {@code detail} for an ordinary disconnect. */
        void onClosed(String detail);
    }

    /** What the client speaks — the one call that proves the whole path. */
    static native String nativeVersion();

    /**
     * Start connecting. Returns a handle immediately; everything after this
     * arrives through {@code listener}.
     *
     * @param nla         {@code prefer}, {@code require} or {@code off}
     * @param compression {@code rdp61}, {@code rdp60}, {@code 64k}, {@code 8k},
     *                    or null for none
     * @param experience  {@code full}, {@code balanced} or {@code plain}: how
     *                    much of its own decoration the remote machine may
     *                    spend the link on
     * @param monitors    how many screens of {@code width} by {@code height} to
     *                    ask for, side by side; one is a plain session
     */
    static native long nativeCreate(Callbacks listener, String address, String userName,
                                    String domain, String password, String nla,
                                    String compression, boolean remoteFx, String experience,
                                    int width, int height, int monitors, int keyboardLayout,
                                    String clientName, int connectTimeoutMs);

    /**
     * Whether the display control channel is open and has said what it can do,
     * which is the only way an RDP desktop is reshaped after connect.
     */
    static native boolean nativeCanResize(long handle);

    /** Ask for a desktop of this size per monitor. */
    static native void nativeRequestDesktopSize(long handle, int width, int height);

    /** How many monitors the next layout asks for. */
    static native void nativeSetMonitorCount(long handle, int count);

    /**
     * The monitors the desktop is made of, flattened four ints each. Empty for
     * a single-monitor session and for a layout the server did not grant.
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
     * Whether the session is on screen. The reverse of RFB's pause: an RDP
     * server sends what it likes, so this is a Suppress Output PDU telling it
     * not to, and a redraw request on the way back.
     */
    static native void nativeFocus(long handle, boolean focused);

    static native void nativeViewOnly(long handle, boolean viewOnly);

    /**
     * What this phone has copied, offered to the remote machine. RDP's clipboard
     * is a delayed rendering: this announces that there is text, and the bytes
     * go only if something over there pastes.
     */
    static native void nativeClipboard(long handle, String text);

    /** Protocol, connection, security, line speed, server and viewer pixels. */
    static native String[] nativeInfo(long handle);

    /** Received and sent since the socket was opened, or null if the session has gone. */
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
