// SPDX-License-Identifier: CC0-1.0

package com.realvnc.vncviewer.jni;

import android.annotation.SuppressLint;
import android.content.Context;
import android.security.KeyChain;

import java.security.Signature;
import java.util.LinkedHashSet;
import java.util.Set;

/**
 * The one class here the core calls <em>into</em> without being asked first:
 * signing a challenge with a client certificate from the Android key chain, for
 * servers that authenticate viewers by identity rather than by password.
 *
 * <p>Required by three names the core resolves on this class — {@code getAliases}
 * {@code ()[Ljava/lang/Object;}, {@code getPubKey} {@code (Ljava/lang/String;)[B}
 * and {@code signDataWithIdentity}
 * {@code (Ljava/lang/String;Ljava/lang/String;[B)[B}. They are not public: they
 * are the core's to call, not a caller's.
 *
 * <p>Everything around them is this module's design rather than the ABI, and it
 * is deliberately thin. The viewer reaches a global {@code Application}
 * singleton for its {@code Context} and keeps its aliases in a preferences file
 * of its own; ours is told both, once, by whoever is embedding it — so this
 * package needs no application classes, no storage, and no opinion about where a
 * list of enrolled identities comes from.
 *
 * <p>With no aliases configured — which is the default, and is the case for a
 * client that does not offer certificate enrolment — {@link #getAliases} answers
 * empty and the core falls back to another authentication method. The other two
 * are then unreachable.
 */
public final class AuthkeyStoreAndroid {

    // An application context, which configure() takes care to be given: the core
    // is a process-wide singleton and this is what it asks the key chain through.
    @SuppressLint("StaticFieldLeak")
    private static Context context;
    private static Set<String> aliases = Set.of();

    private AuthkeyStoreAndroid() {
    }

    /**
     * Called once, before any session, from whatever brings the core up.
     *
     * @param context the context {@link KeyChain} is asked through; its
     *                application context is retained
     * @param aliases key chain aliases this client is willing to authenticate
     *                with, in the order they should be offered
     */
    public static synchronized void configure(Context context, Set<String> aliases) {
        AuthkeyStoreAndroid.context = context == null ? null : context.getApplicationContext();
        AuthkeyStoreAndroid.aliases = aliases == null ? Set.of() : new LinkedHashSet<>(aliases);
    }

    static synchronized Object[] getAliases() {
        return aliases.toArray();
    }

    /**
     * The leaf certificate of {@code alias}, DER-encoded.
     *
     * <p>Exceptions are left to propagate, as they do in the viewer this
     * interface was read from: the core is the only caller, and it is the only
     * side that knows what a failure to produce a key should do to the
     * connection in progress.
     */
    static byte[] getPubKey(String alias) throws Exception {
        return KeyChain.getCertificateChain(context, alias)[0].getEncoded();
    }

    static byte[] signDataWithIdentity(String algorithm, String alias, byte[] data)
            throws Exception {
        final Signature signature = Signature.getInstance(algorithm);
        signature.initSign(KeyChain.getPrivateKey(context, alias));
        signature.update(data);
        return signature.sign();
    }
}
