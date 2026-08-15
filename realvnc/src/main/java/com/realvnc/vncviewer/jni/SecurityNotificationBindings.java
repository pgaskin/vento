// SPDX-License-Identifier: CC0-1.0

package com.realvnc.vncviewer.jni;

/**
 * The encrypted / unencrypted banner: a line of text and the name of a
 * background to draw it on.
 *
 * <p>The background name is the only thing in the callback that distinguishes
 * the two states — the text is already localised prose — so the two values it
 * takes are worth having as constants even though they are the viewer's own
 * drawable keys and mean nothing outside it.
 *
 * <p>Required by
 * {@code Java_com_realvnc_vncviewer_jni_SecurityNotificationBindings_setSecurityNotifier}
 * and by {@code SecurityNotificationBindings$SecurityNotifier}, which the core
 * resolves by name.
 */
public final class SecurityNotificationBindings {

    /** The line is encrypted. */
    public static final String BACKGROUND_SECURE = "StripesTileGreen";

    /** It is not. */
    public static final String BACKGROUND_INSECURE = "StripesTileRed";

    public interface SecurityNotifier {
        void show(SessionBindings.Session session, String text, String background, int flags);
    }

    private SecurityNotificationBindings() {
    }

    public static native void setSecurityNotifier(SecurityNotifier notifier);
}
