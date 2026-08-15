// SPDX-License-Identifier: CC0-1.0

package com.realvnc.vncviewer.jni;

/**
 * The credentials prompt, and the pattern every other prompt in this package
 * follows.
 *
 * <h2>The prompt protocol</h2>
 *
 * A factory is registered once, process-wide. When the core needs an answer it
 * asks the factory for an object, calls the question method on that object, and
 * <em>waits</em> — arbitrarily long, on the session thread — until a static
 * result native is called with the same {@code cookie}. Then it calls
 * {@code close()} on the object and drops it.
 *
 * <p>Two consequences worth stating once: the cookie is the whole identity of a
 * question, so an answer may be given from any thread as long as it carries the
 * cookie back; and a prompt can arrive <em>during</em>
 * {@link SessionBindings#createSession}, before that call has returned a
 * {@code Session} to key anything on.
 *
 * <p>This one is mandatory: with no factory registered, a session against a
 * server that wants a password never progresses.
 *
 * <p>Required by
 * {@code Java_com_realvnc_vncviewer_jni_PasswdDlgBindings_setPasswdDlgFactory}
 * and {@code …_passwdResult}, and by {@code PasswdDlgBindings$PasswdDlg} and
 * {@code $PasswdDlgFactory}, which the core resolves by name.
 */
public final class PasswdDlgBindings {

    public interface PasswdDlg {

        void close();

        /**
         * The seven arguments are not what their names in a decompile suggest.
         * Read off which field of the viewer's own dialog each one lands in,
         * they are: an unused first string, the server name, the user name, a
         * "wants a password" flag, the catchphrase, the signature, and the
         * instructions.
         *
         * <p>The user name is <b>null when the scheme has none</b> — VncAuth
         * takes a password only — which is how a dialog knows not to offer the
         * field at all.
         */
        void getUserPasswd(String unused, String serverName, String userName,
                           boolean wantsPassword, String catchphrase, String signature,
                           String instructions);
    }

    public interface PasswdDlgFactory {
        PasswdDlg createPasswdDlg(SessionBindings.Session session, long cookie, String address);
    }

    private PasswdDlgBindings() {
    }

    public static native void passwdResult(long cookie, boolean ok, String userName, String password);

    public static native void setPasswdDlgFactory(PasswdDlgFactory factory);
}
