// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend.rustdesk;

import android.graphics.Bitmap;

/**
 * The JNI surface of {@code libremotedesktop_rustdesk.so} — the RustDesk client
 * in {@code src/main/rust}, bound in its {@code src/bindings} and compiled by
 * this module's {@code cargoBuild*Native}.
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
 *       They take the framebuffer's read lock, which is only ever held against
 *       them for a buffer swap.
 *   <li>Everything else may be called from anywhere; input is encoded on the
 *       calling thread and written on a thread of the binding's own, so a
 *       stalled socket cannot reach the main thread.
 * </ul>
 *
 * <p>{@code nativeDestroy} frees the session and joins its thread, so nothing
 * may hold a handle across it: {@link RustDeskBackend} retires the handle under
 * its own lock first, and the join is what makes the callbacks provably
 * finished.
 */
final class RustDeskNative {

    private RustDeskNative() {
    }

    static {
        System.loadLibrary("remotedesktop_rustdesk");
    }

    /**
     * Everything the session says. Protocol thread, always.
     *
     * <p>Three of these are never called here and are declared because the
     * interface is the same one every backend answers: this protocol has no
     * relative pointer, no bell, and — in the mode this backend has — no
     * certificate to trust. The Rust side resolves every method at create time,
     * so a shorter interface would be a different one.
     */
    interface Callbacks {

        void onConnected(int width, int height);

        void onDesktopSize(int width, int height);

        /**
         * What moved, which this protocol never says: a frame is a whole
         * picture rather than a change to one, so the rectangle is one the
         * client works out by comparing the frame against the last.
         */
        void onDamage(int x, int y, int width, int height);

        void onFrameEnd();

        /**
         * @param argb {@code width * height} pixels, {@code Color}-packed
         * @param hash of the pixels, which is the identity {@code CursorCache}
         *             keeps the built bitmap under
         */
        void onCursor(int[] argb, int width, int height, int hotX, int hotY, long hash);

        /** Never called: the cursor is this end's for the life of a session. */
        void onPointerMode(boolean relative);

        /** Never called: there is no bell in this protocol. */
        void onBell();

        void onClipboard(String text);

        /**
         * The peer refused the password, or has one and was given none. The
         * session is stopped in the login exchange until
         * {@link #nativeAnswerPassword} is called — the peer is waiting for
         * another login request on the same connection, which is what their own
         * client sends.
         *
         * @param needsUserName always false: this protocol has no user name
         */
        void onCredentialsNeeded(boolean needsUserName);

        /**
         * The peer's long-term key, by fingerprint, before this end has told it
         * anything. The session waits for {@link #nativeAnswerTrust}.
         *
         * <p>Only the id path asks: direct IP access asserts no identity at all,
         * so there is nothing to put the question about.
         */
        void onTrustNeeded(String fingerprint);

        /**
         * The machine could not be verified at all, in the words of why, and
         * the session is stopped until {@link #nativeAnswerTrust}. Saying yes
         * goes on in the clear, which is what their own client does without
         * asking; saying no ends the connection.
         */
        void onUnverified(String why);

        /** Terminal. Empty {@code detail} for an ordinary disconnect. */
        void onClosed(String detail);
    }

    /** The version of theirs this client speaks, and the proof of the path. */
    static native String nativeVersion();

    /**
     * Start connecting. Returns a handle immediately; everything after this
     * arrives through {@code listener}.
     *
     * @param address   {@code host} or {@code host:port} for a direct session,
     *                  the peer's own digits for one by id
     * @param byId      whether to ask a rendezvous server for the peer rather
     *                  than dialling the address
     * @param server    the rendezvous server, or empty for the public network
     * @param serverKey its public key in base64, or empty for the public
     *                  network's, which is a constant in every build of theirs
     * @param myName    what the far end calls this phone, on its own screen
     * @param quality   {@code low}, {@code balanced} or {@code best}
     * @param fps       frames a second, or 0 for the far end's own choice
     * @param codec     which codec to ask the far end for, or {@code auto}
     * @param lockAfter whether the far end locks its screen when this ends
     */
    static native long nativeCreate(Callbacks listener, String address, boolean byId,
                                    String server, String serverKey, String password,
                                    String myName, String quality, int fps, String codec,
                                    boolean lockAfter, int connectTimeoutMs);

    /** A {@code null} password cancels, which ends the session. */
    static native void nativeAnswerPassword(long handle, String password);

    /** Whether the key {@link Callbacks#onTrustNeeded} named may be gone on with. */
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
     * Whether to keep asking for a full frame rate. There is no pause message
     * in this protocol, so an unfocused session asks for one frame a second and
     * for a fresh picture on the way back.
     */
    static native void nativeFocus(long handle, boolean focused);

    static native void nativeViewOnly(long handle, boolean viewOnly);

    static native void nativeClipboard(long handle, String text);

    /**
     * Live: their option message acts on the picture in front of you.
     *
     * <p>The codec is in it too, and is the one that is not free: the far end
     * answers a codec it is not already using by building an encoder and
     * starting again with a key frame.
     */
    static native void nativeSetOptions(long handle, String quality, int fps, String codec);

    /**
     * Whether the peer published a list of sizes it will take. Unlike RFB it is
     * a list rather than a free choice, so a request becomes the nearest of
     * them.
     */
    static native boolean nativeCanResize(long handle);

    static native void nativeRequestDesktopSize(long handle, int width, int height);

    /**
     * The far end's displays as {@code x, y, width, height} each, with the index
     * of the one being captured on the end — or null where it has one, since a
     * choice of one is not a control.
     */
    static native int[] nativeDisplays(long handle);

    /**
     * Ask for one of the peer's other displays. A request: what comes back is a
     * switch message, which may name a different display than the one asked for.
     */
    static native void nativeRequestDisplay(long handle, int index);

    /**
     * Desktop name, protocol, connection, security, encoding, round trip, the
     * peer's platform, and which display of how many.
     */
    static native String[] nativeInfo(long handle);

    /** Received and sent since the socket was opened, or null if it has gone. */
    static native long[] nativeTraffic(long handle);

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
