// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend.realvnc;

/**
 * A session's connections, routed under the viewer core, which dials the
 * server itself: on a network that is not direct (the developer setting
 * that relays connections through a socketpair), the core's socket calls are
 * hooked in memory and dial through the session's network, as on Linux.
 * {@code cpp/realvnc_got.c} and {@code cpp/realvnc_net.h} say how.
 *
 * <p>Nothing the core calls says which session it is for, and it keys what it
 * remembers of a server on the address as given, so sessions are told apart
 * by their server alone: a session to a server another session is open to on
 * another route is refused ({@link #attach} throws).
 *
 * <p>{@link #attach} on the session thread, before {@code createSession};
 * {@link #closed} once the session has closed; {@link #cancel} from any
 * thread, first thing on a disconnect, so that a dial through the network in
 * progress ends at once; {@link #detach} on the session thread once the
 * session is closed.
 */
final class RealVncNet {

    static {
        System.loadLibrary("remotedesktop_realvnc");
    }

    /** The application context, for {@link #notice}; set by the backend. */
    private static volatile android.content.Context appContext;

    private RealVncNet() {
    }

    static void context(android.content.Context context) {
        appContext = context.getApplicationContext();
    }

    /**
     * Called from the native side, on one of the core's threads, when the
     * hooks refuse a lookup they cannot assign to a session (a direct and a
     * tunnelled session open at once): shows {@code text} as a toast, from
     * the main thread.
     */
    static void notice(String text) {
        final android.content.Context ctx = appContext;
        if (ctx == null) {
            return;
        }
        new android.os.Handler(android.os.Looper.getMainLooper()).post(() ->
                android.widget.Toast.makeText(ctx, text, android.widget.Toast.LENGTH_LONG).show());
    }

    /**
     * Routes the session to {@code address}. Positive where its connections
     * go through the session's network (its UDP transport too, where the
     * network carries datagrams); negative where the core dials the server
     * itself, as it always has. To be detached.
     *
     * @throws java.io.IOException where the session must not start, direct
     *     or not, with the reason: the network is not direct and the core's
     *     calls cannot be put through it (the core would dial directly), a
     *     session to the same server is open on another route (its
     *     connections and this one's could not be told apart), or there are
     *     too many sessions.
     */
    static int attach(String address, int timeoutMs, boolean forceSocketpair)
            throws java.io.IOException {
        return nativeAttach(address, timeoutMs, forceSocketpair);
    }

    /**
     * The session has closed: nothing the core dials from here on is for it,
     * and another session to its server may attach on another route.
     */
    static void closed(int route) {
        if (route != 0) {
            nativeClosed(route);
        }
    }

    /**
     * What carries the connections of a session attached with
     * {@code forceSocketpair} (the session's network says, as
     * "dev-socketpair"), for the panel's Tunnel row; {@code null} where the
     * core's connections are its own.
     */
    static String tunnel(boolean forceSocketpair) {
        return nativeTunnel(forceSocketpair);
    }

    static void cancel(int route) {
        if (route != 0) {
            nativeCancel(route);
        }
    }

    static void detach(int route) {
        if (route != 0) {
            nativeDetach(route);
        }
    }

    private static native int nativeAttach(String address, int timeoutMs,
            boolean forceSocketpair) throws java.io.IOException;

    private static native void nativeClosed(int route);

    private static native String nativeTunnel(boolean forceSocketpair);

    private static native void nativeCancel(int route);

    private static native void nativeDetach(int route);
}
