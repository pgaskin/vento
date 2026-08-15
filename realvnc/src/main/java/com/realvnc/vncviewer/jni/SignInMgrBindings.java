// SPDX-License-Identifier: CC0-1.0

package com.realvnc.vncviewer.jni;

import java.util.List;

/**
 * RealVNC account sign-in, which this client does not support.
 *
 * <p>It is here at all because {@link Bindings#initViewer} has to pass a
 * {@link SignInUi}: the core resolves
 * {@code com/realvnc/vncviewer/jni/SignInMgrBindings$SignInUi} by name and
 * caches a method id for each of the eleven methods below. Nothing here is
 * public — the stub that answers them lives in {@link Bindings} — and none of
 * the class's twenty natives is declared.
 *
 * <p>{@link Factor} and {@link SecondFactorRequest} are constructed <em>by</em>
 * the core, before it calls {@link SignInUi#signInSecondFactorsRequired}, so
 * their names and their constructor signatures are as fixed as the interface is:
 * {@code (JLjava/lang/String;Ljava/lang/String;Ljava/lang/String;JJ)V} and
 * {@code (Ljava/lang/String;JLjava/util/List;)V}. Records, because that is all
 * they are, and because the canonical constructor is exactly the one the core
 * calls.
 */
final class SignInMgrBindings {

    /** A second authentication factor offered to the person signing in. */
    record Factor(long type, String id, String totpName, String emailAddress,
                  long backupGeneratedAt, long backupDigits) {
    }

    /** The set of them, and the token that a chosen one is answered with. */
    record SecondFactorRequest(String preAuthenticatedToken, long expiry, List<Factor> factors) {
    }

    interface SignInUi {

        void openSsoUrl(String url);

        void signInChanged(int state);

        void signInFailed(String message, boolean canRetry, boolean fatal);

        void signInNetworkStatusChanged();

        void signInSecondFactorsRequired(SecondFactorRequest request);

        void signInSignedIn();

        void signInSignedOut(boolean forced);

        boolean signInSsoRefreshSignedOut();

        void signInStarted();

        void signInTermsChanged();

        void signInTimedOut();
    }

    private SignInMgrBindings() {
    }
}
