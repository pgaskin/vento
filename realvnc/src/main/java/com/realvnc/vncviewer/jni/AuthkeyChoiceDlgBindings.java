// SPDX-License-Identifier: CC0-1.0

package com.realvnc.vncviewer.jni;

/**
 * "Which of your identities should authenticate this connection?" — asked when a
 * server authenticates viewers by client certificate and more than one is
 * available. The list comes from {@link AuthkeyStoreAndroid}.
 *
 * <p>The prompt protocol is {@link PasswdDlgBindings}'s.
 *
 * <p>Required by
 * {@code Java_com_realvnc_vncviewer_jni_AuthkeyChoiceDlgBindings_setAuthkeyChoiceDlgFactory}
 * and {@code …_identityChosen}, and by {@code $AuthkeyChoiceDlg} and
 * {@code $AuthkeyChoiceDlgFactory}, which the core resolves by name.
 */
public final class AuthkeyChoiceDlgBindings {

    public interface AuthkeyChoiceDlg {

        void close();

        void show(String[] identities);
    }

    public interface AuthkeyChoiceDlgFactory {
        AuthkeyChoiceDlg createAuthkeyChoiceDlg(SessionBindings.Session session, long cookie);
    }

    private AuthkeyChoiceDlgBindings() {
    }

    public static native void identityChosen(long cookie, boolean ok, String identity);

    public static native void setAuthkeyChoiceDlgFactory(AuthkeyChoiceDlgFactory factory);
}
