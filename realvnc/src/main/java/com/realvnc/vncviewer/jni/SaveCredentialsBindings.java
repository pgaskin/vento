// SPDX-License-Identifier: CC0-1.0

package com.realvnc.vncviewer.jni;

/**
 * "Persist these credentials." A notification rather than a question — the core
 * has already authenticated with them — and the only way a client that keeps its
 * own connection records learns what was typed into a prompt the core owned.
 *
 * <p>Required by
 * {@code Java_com_realvnc_vncviewer_jni_SaveCredentialsBindings_setCredentialsStore}
 * and by {@code SaveCredentialsBindings$CredentialsStore}, which the core
 * resolves by name.
 */
public final class SaveCredentialsBindings {

    public interface CredentialsStore {
        void saveCredentials(SessionBindings.Session session, String userName, String password);
    }

    private SaveCredentialsBindings() {
    }

    public static native void setCredentialsStore(CredentialsStore store);
}
