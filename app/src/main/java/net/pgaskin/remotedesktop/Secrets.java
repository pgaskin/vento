// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.security.keystore.KeyGenParameterSpec;
import android.security.keystore.KeyProperties;
import android.util.Base64;
import android.util.Log;

import java.nio.charset.StandardCharsets;
import java.security.KeyStore;
import java.security.SecureRandom;

import javax.crypto.Cipher;
import javax.crypto.KeyGenerator;
import javax.crypto.SecretKey;
import javax.crypto.spec.GCMParameterSpec;

/**
 * What this app keeps under a key: a saved connection's password, and the
 * desktop preview on its home card.
 *
 * <p>The preview is the more revealing of the two: a password is a string
 * somebody chose, and a preview is a photograph of whatever was on that
 * machine's screen when the session ended, written to disk without anyone
 * asking for it. It gets the same treatment under a key of its own.
 *
 * <p>AES-256-GCM under a key generated inside the Android keystore, which means
 * the key material never exists in this process. Whether it is also beyond the
 * reach of anything but the processor is the platform's guarantee rather than
 * one this code checks: nothing here asks for StrongBox or consults
 * {@code KeyInfo.isInsideSecureHardware}, because a phone that cannot offer
 * that should still be able to save a password. Everything else about a connection — its name, address and user name
 * — stays in plain text, because it is not a secret and hiding it would only
 * make the file harder to read while protecting nothing.
 *
 * <p>Android's auto-backup takes the preference file, and the ciphertext in it
 * is useless anywhere else — on another device, whose keystore has no such key,
 * and on the same one, because a keystore entry belongs to an app installation
 * and clearing the app's data takes the key with it. That is intended, and all
 * it requires is that failing to decrypt be <em>ordinary</em>: it means "there
 * is no saved password here", the connection still opens, the server still
 * asks, and the prompt still offers to remember.
 *
 * <p>Failing closed: if the keystore will not produce a key at all,
 * {@link #protect} returns null and nothing is saved. A password that cannot be
 * encrypted is not written in plain text as a consolation.
 */
final class Secrets {

    private static final String TAG = "Secrets";
    private static final String KEYSTORE = "AndroidKeyStore";
    private static final String ALIAS = "connection-password";
    /** A separate key, because a preview is not a password. */
    private static final String ALIAS_PREVIEW = "connection-preview";
    private static final String TRANSFORM = "AES/GCM/NoPadding";
    /** Marks a value as ciphertext, and leaves room to change the scheme. */
    private static final String PREFIX = "v1:";
    private static final int IV_BYTES = 12;
    private static final int TAG_BITS = 128;

    private Secrets() {
    }

    /**
     * @return the stored form of {@code plaintext}, or null if it cannot be
     * encrypted — in which case the caller stores nothing
     */
    static String protect(String plaintext) {
        if (plaintext == null || plaintext.isEmpty()) {
            return "";
        }
        try {
            return PREFIX + Base64.encodeToString(
                    seal(plaintext.getBytes(StandardCharsets.UTF_8), ALIAS), Base64.NO_WRAP);
        } catch (Exception e) {
            Log.w(TAG, "cannot encrypt; the password will not be saved", e);
            return null;
        }
    }

    /**
     * The other direction. Returns "" for anything this device cannot open,
     * which is the same answer as "nothing was saved" — see the class comment.
     */
    static String reveal(String stored) {
        if (stored == null || stored.isEmpty()) {
            return "";
        }
        if (!stored.startsWith(PREFIX)) {
            // Written before the passwords were encrypted. Taken as it is, and
            // re-written encrypted by the next save of that connection.
            return stored;
        }
        try {
            final byte[] blob = Base64.decode(stored.substring(PREFIX.length()), Base64.NO_WRAP);
            if (blob.length <= IV_BYTES) {
                return "";
            }
            return new String(open(blob, ALIAS), StandardCharsets.UTF_8);
        } catch (Exception e) {
            // A restored backup, a cleared app, a re-installed app: the key is
            // gone and this is not openable by anything, ever. Say "no password"
            // and let the server ask.
            Log.i(TAG, "no usable key for the saved password (" + e.getClass().getSimpleName()
                    + "); it will be asked for");
            return "";
        }
    }

    /** A twelve-byte IV and then the ciphertext, under {@code alias}'s key. */
    private static byte[] seal(byte[] plaintext, String alias) throws Exception {
        final Cipher cipher = Cipher.getInstance(TRANSFORM);
        cipher.init(Cipher.ENCRYPT_MODE, key(alias));
        final byte[] iv = cipher.getIV();
        final byte[] ct = cipher.doFinal(plaintext);
        final byte[] out = new byte[iv.length + ct.length];
        System.arraycopy(iv, 0, out, 0, iv.length);
        System.arraycopy(ct, 0, out, iv.length, ct.length);
        return out;
    }

    /** The other direction. The caller has checked that there is an IV to read. */
    private static byte[] open(byte[] sealed, String alias) throws Exception {
        final Cipher cipher = Cipher.getInstance(TRANSFORM);
        cipher.init(Cipher.DECRYPT_MODE, key(alias),
                new GCMParameterSpec(TAG_BITS, sealed, 0, IV_BYTES));
        return cipher.doFinal(sealed, IV_BYTES, sealed.length - IV_BYTES);
    }

    /**
     * The device's key for this app, made once and kept in the keystore.
     *
     * <p>{@code setUnlockedDeviceRequired} because a password is only ever
     * needed while someone is looking at the screen, and {@code randomizedEncryptionRequired}
     * (the default, stated for the record) because GCM with a repeated IV is
     * not encryption.
     */
    private static SecretKey key(String alias) throws Exception {
        final KeyStore ks = KeyStore.getInstance(KEYSTORE);
        ks.load(null);
        final KeyStore.Entry entry = ks.getEntry(alias, null);
        if (entry instanceof KeyStore.SecretKeyEntry e) {
            return e.getSecretKey();
        }
        final KeyGenerator gen = KeyGenerator.getInstance(
                KeyProperties.KEY_ALGORITHM_AES, KEYSTORE);
        gen.init(new KeyGenParameterSpec.Builder(alias,
                KeyProperties.PURPOSE_ENCRYPT | KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .setRandomizedEncryptionRequired(true)
                .setUnlockedDeviceRequired(true)
                .build(), new SecureRandom());
        return gen.generateKey();
    }

    // ---- bytes, for the desktop preview ------------------------------------

    /**
     * A preview, sealed as it goes to disk — no Base64 and no prefix, because
     * this goes into a file of its own rather than into a preference string.
     *
     * @return null if it cannot be encrypted, in which case nothing is written
     * — a preview is worth less than the guarantee that what is on disk is
     * unreadable
     */
    static byte[] sealBytes(byte[] plaintext) {
        if (plaintext == null || plaintext.length == 0) {
            return null;
        }
        try {
            return seal(plaintext, ALIAS_PREVIEW);
        } catch (Exception e) {
            Log.w(TAG, "cannot encrypt; nothing will be written", e);
            return null;
        }
    }

    /** Null for anything this device cannot open, which is not an error. */
    static byte[] openBytes(byte[] sealed) {
        if (sealed == null || sealed.length <= IV_BYTES) {
            return null;
        }
        try {
            return open(sealed, ALIAS_PREVIEW);
        } catch (Exception e) {
            Log.i(TAG, "no usable key for a stored preview ("
                    + e.getClass().getSimpleName() + "); it will be drawn again");
            return null;
        }
    }
}
