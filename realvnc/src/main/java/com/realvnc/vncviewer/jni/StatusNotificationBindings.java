// SPDX-License-Identifier: CC0-1.0

package com.realvnc.vncviewer.jni;

/**
 * The session's status line — "Connecting…", "Authenticating…", and the rest of
 * the core's own account of what it is doing. Localised by the core.
 *
 * <p>Required by
 * {@code Java_com_realvnc_vncviewer_jni_StatusNotificationBindings_setStatusNotifier}
 * and by {@code StatusNotificationBindings$StatusNotifier}, which the core
 * resolves by name.
 */
public final class StatusNotificationBindings {

    public interface StatusNotifier {
        void update(SessionBindings.Session session, String text);
    }

    private StatusNotificationBindings() {
    }

    public static native void setStatusNotifier(StatusNotifier notifier);
}
