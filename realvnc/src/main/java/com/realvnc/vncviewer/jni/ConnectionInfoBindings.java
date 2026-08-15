// SPDX-License-Identifier: CC0-1.0

package com.realvnc.vncviewer.jni;

/**
 * What a live session can be asked about itself. Every one of these is a display
 * string the core formats, not a value — the size comes back as
 * {@code "1920 x 1200"} and the speed as a rate with its unit on it.
 *
 * <p>They are safe to call at any point in a session's life and answer an empty
 * string when there is nothing to say, which happens more often than it sounds:
 * a server that goes away while auto-reconnect keeps the session nominally open
 * empties every one of them at once, the desktop name included.
 *
 * <p>Required by eight exported
 * {@code Java_com_realvnc_vncviewer_jni_ConnectionInfoBindings_*} symbols.
 */
public final class ConnectionInfoBindings {

    private ConnectionInfoBindings() {
    }

    public static native String getConnectionType(SessionBindings.Session session);

    public static native String getDesktopName(SessionBindings.Session session);

    public static native String getLastUsedEncoding(SessionBindings.Session session);

    public static native String getLineSpeed(SessionBindings.Session session);

    /** A protocol version below 4.0 means the server is not a RealVNC one. */
    public static native String getProtoVersion(SessionBindings.Session session);

    public static native String getSecurityDesc(SessionBindings.Session session);

    public static native String getServerPixelFormat(SessionBindings.Session session);

    public static native String getViewerPixelFormat(SessionBindings.Session session);
}
