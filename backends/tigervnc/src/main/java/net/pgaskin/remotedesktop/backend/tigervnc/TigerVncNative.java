// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend.tigervnc;

import android.graphics.Bitmap;

/**
 * The JNI surface of {@code libremotedesktop_tigervnc.so} — TigerVNC's own
 * client code and the shim in {@code src/main/cpp} that adapts it.
 *
 * <p>Deliberately the same surface {@code RfbNative} and {@code LibVncNative}
 * describe, so that everything above it is the same code against any of the
 * three and a comparison is between the clients. Where it differs from
 * libvncclient's:
 *
 * <ul>
 *   <li><b>The encoding is a number, not a string.</b> TigerVNC's client asks
 *       for every encoding it can decode and names one of them <em>preferred</em>
 *       — the server still chooses. So there is no list to pass, and no way to
 *       forbid an encoding either.
 *   <li><b>No {@code nativePointerRelative} and no pointer mode</b>, as with
 *       libvncclient: QEMU's {@code PointerTypeChange} is not among the
 *       encodings this client asks for, so this backend is always absolute.
 *   <li><b>{@code nativeAnswerQuestion} rather than {@code nativeAnswerTrust}.</b>
 *       This client checks the certificate itself and remembers what was
 *       accepted, so what crosses the JNI surface is the question it decided to
 *       ask and the yes or no it gets back.
 * </ul>
 *
 * <h2>Threads</h2>
 * <ul>
 *   <li>Every {@link Callbacks} method arrives on the protocol thread.
 *   <li>{@link #nativeReadRegion} and {@link #nativeReadThumbnail} are called
 *       from whichever thread is drawing. They take a reader lock over a copy
 *       of the framebuffer the shim keeps, because the decoders write into
 *       TigerVNC's own with no lock at all.
 *   <li>Everything else may be called from anywhere and is queued: one thread
 *       owns the connection and drains the queue between messages.
 * </ul>
 */
final class TigerVncNative {

    private TigerVncNative() {
    }

    static {
        System.loadLibrary("remotedesktop_tigervnc");
    }

    /** Everything the session says. Protocol thread, always. */
    interface Callbacks {

        void onConnected(int width, int height);

        void onDesktopSize(int width, int height);

        void onDamage(int x, int y, int width, int height);

        void onFrameEnd();

        /**
         * @param argb {@code width * height} pixels, {@code Color}-packed
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
         * @param needsUserName false for VncAuth, which has no notion of one
         */
        void onCredentialsNeeded(boolean needsUserName);

        /**
         * The library wants a yes or a no before the handshake goes on — which
         * for this client is always about the far end's identity, since that is
         * the one thing it cannot check on its own. The session is stopped
         * until {@link #nativeAnswerQuestion}, and a no ends it.
         */
        void onQuestion(String title, String text);

        /** Terminal. Empty {@code detail} for an ordinary disconnect. */
        void onClosed(String detail);
    }

    /** The library's version — the one call that proves the whole path. */
    static native String nativeVersion();

    /**
     * Where the client may keep the certificates a person has accepted. It has
     * a known-hosts file of its own and finds it through the environment, which
     * on Android points nowhere writable — so this is said once, before any
     * session, and names the directory the library's own goes under.
     */
    static native void nativeSetStateDir(String path);

    /**
     * Start connecting. Returns a handle immediately; everything after this
     * arrives through {@code listener}.
     *
     * @param encoding      an RFB encoding number, the one to ask for first
     * @param compressLevel 0–9, or −1 for the server's own default
     * @param qualityLevel  0–9 for Tight's JPEG, −1 for lossless — which is the
     *                      absence of the quality pseudo-encoding rather than a
     *                      value of it — or −2 to follow the measured line speed
     * @param colorLevel    0 for full colour, 1 for 64 colours, 2 for 8, or −2 to
     *                      follow the measured line speed. Only the wire format
     *                      changes: the decoders convert into a framebuffer that
     *                      is always 32-bit
     * @param h264          whether to offer H.264 at all. Every other encoding
     *                      this client can decode is offered unconditionally;
     *                      this one is not, because the server picks from the
     *                      list and this one is lossy and costs a whole frame
     *                      however little changed
     */
    static native long nativeCreate(Callbacks listener, String address, String userName,
                                    String password, boolean shared, int encoding,
                                    int compressLevel, int qualityLevel, int colorLevel,
                                    boolean h264, int connectTimeoutMs);

    /** A {@code null} password cancels, which ends the session. */
    static native void nativeAnswerCredentials(long handle, String userName, String password);

    /** A no ends the session, since the question is whether to trust the far end. */
    static native void nativeAnswerQuestion(long handle, boolean yes);

    static native void nativeDisconnect(long handle);

    /** Frees the handle and joins the protocol thread. Nothing may use it after. */
    static native void nativeDestroy(long handle);

    static native void nativePointer(long handle, int x, int y, int buttonMask);

    static native void nativeKeyDown(long handle, int keysym, int keyId);

    static native void nativeKeyUp(long handle, int keyId);

    /** Let go of every key still held — a screen going away mid-chord. */
    static native void nativeReleaseAllKeys(long handle);

    /**
     * Whether to keep reading. The client asks for the next update at the end
     * of the one it has just handled, so leaving the last one unread is the
     * whole of the pause: one update sits in the socket and the server waits.
     */
    static native void nativeFocus(long handle, boolean focused);

    static native void nativeViewOnly(long handle, boolean viewOnly);

    /**
     * Whether the server has said it takes a desktop size from the client,
     * which it does by sending an {@code ExtendedDesktopSize} rectangle at all.
     */
    static native boolean nativeCanResize(long handle);

    /**
     * The screen layout the same rectangle carries, flattened four ints per
     * screen — x, y, width, height. Empty when none has arrived.
     */
    static native int[] nativeMonitors(long handle);

    /**
     * Ask for a desktop this size, as one screen covering it. Granted, it
     * arrives back as {@link Callbacks#onDesktopSize}; refused, the library
     * logs the result code and nothing changes.
     */
    static native void nativeRequestDesktopSize(long handle, int width, int height);

    /**
     * Announce this text as ours. The extended clipboard is a conversation:
     * the server asks for the text if it wants it, and the shim answers with
     * whatever was announced last.
     */
    static native void nativeClipboard(long handle, String text);

    /**
     * Live, all of it: {@code SetEncodings} may be sent at any point in a
     * session, and the pixel format is this client's to change safely — it holds
     * the new one until nothing is in flight, which is what libvncclient has
     * nowhere to do.
     */
    static native void nativeSetPicture(long handle, int encoding, int compressLevel,
                                        int qualityLevel, int colorLevel, boolean h264);

    /**
     * Desktop name, protocol, connection, security, the encoding the server
     * last used, the line speed, and the two pixel formats.
     */
    static native String[] nativeInfo(long handle);

    /**
     * Received and sent since the connection was made, or -1 for either before
     * the protocol thread has published one. Null if the session has gone.
     */
    static native long[] nativeTraffic(long handle);

    /**
     * Copy the desktop rectangle into {@code dst} at {@code (dstX, dstY)}, 1:1,
     * out of the shim's copy of the framebuffer and under its reader lock.
     */
    static native boolean nativeReadRegion(long handle, int x, int y, int width, int height,
                                           Bitmap dst, int dstX, int dstY);

    /**
     * The whole desktop at {@code 1/step}, nearest neighbour, into a bitmap the
     * caller has already sized to {@code ceil(w/step) × ceil(h/step)}.
     */
    static native boolean nativeReadThumbnail(long handle, int step, Bitmap dst);
}
