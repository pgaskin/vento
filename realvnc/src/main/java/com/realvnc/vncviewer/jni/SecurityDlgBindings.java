// SPDX-License-Identifier: CC0-1.0

package com.realvnc.vncviewer.jni;

/**
 * "Is this server who it says it is, and is the connection encrypted?" The other
 * mandatory prompt: it fires on every unencrypted connection and on every server
 * identity that is not already known and matching.
 *
 * <p>The prompt protocol is {@link PasswdDlgBindings}'s.
 *
 * <p>Required by
 * {@code Java_com_realvnc_vncviewer_jni_SecurityDlgBindings_setSecurityDlgFactory}
 * and {@code …_securityResult}, and by {@code SecurityDlgBindings$SecurityDlg}
 * and {@code $SecurityDlgFactory}, which the core resolves by name.
 */
public final class SecurityDlgBindings {

    /** What is known about the server's identity. The first argument of the prompt. */
    public static final int ID_OK = 0;
    public static final int ID_NEW = 1;
    public static final int ID_MATCHES_ANOTHER_SERVER = 2;
    public static final int ID_CHANGED = 3;
    public static final int ID_MISSING = 4;
    public static final int ID_PRESHARED = 5;
    public static final int ID_ARD = 6;

    /** What is known about the line. The second argument. */
    public static final int ENC_ENCRYPTED = 0;
    public static final int ENC_UNENCRYPTED_WARN = 1;
    public static final int ENC_UNENCRYPTED_NO_WARN = 2;

    public interface SecurityDlg {

        void close();

        /**
         * The identity comes <b>first</b> and the encryption second, which is
         * the opposite of what a decompiled signature suggests. The viewer's own
         * wrapper is the proof rather than the names: it stores the first
         * argument and tests it against 5, which is {@link #ID_PRESHARED}, while
         * it derives its "warn about encryption" flag from the second being 1,
         * which is {@link #ENC_UNENCRYPTED_WARN}.
         *
         * <p>Reading them the other way round tells someone that an encrypted
         * connection is in plain text and that a server they have never seen is
         * already known.
         *
         * <p>The strings are ordered the same way — by which field of the
         * viewer's dialog each one lands in, not by the order their invented
         * names suggest.
         *
         * @param identityState   one of the {@code ID_*} constants
         * @param encryptionState one of the {@code ENC_*} constants
         * @param matchingName    the server this identity is already known as,
         *                        for {@link #ID_MATCHES_ANOTHER_SERVER}
         */
        void promptSecurity(int identityState, int encryptionState, String signature,
                            String catchphrase, String name, String matchingName,
                            String hint);
    }

    public interface SecurityDlgFactory {
        SecurityDlg createSecurityDlg(SessionBindings.Session session, long cookie);
    }

    private SecurityDlgBindings() {
    }

    /**
     * @param accept         proceed with the connection
     * @param saveIdentity   remember this server's identity, which is what turns
     *                       a later {@link #ID_NEW} into {@link #ID_OK}
     * @param saveEncryption remember the encryption decision
     */
    public static native void securityResult(long cookie, boolean accept, boolean saveIdentity,
                                             boolean saveEncryption);

    public static native void setSecurityDlgFactory(SecurityDlgFactory factory);
}
