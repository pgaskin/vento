// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.plugin;

import java.io.File;
import java.io.FileInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.security.DigestOutputStream;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;

/**
 * SHA-256, which is the whole of what makes loading somebody else's binary
 * decidable: a copy either is the build the declarations were read from or it is
 * not, and there is no third answer to act on.
 */
public final class Hashes {

    private Hashes() {
    }

    private static MessageDigest sha256() {
        try {
            return MessageDigest.getInstance("SHA-256");
        } catch (NoSuchAlgorithmException e) {
            throw new AssertionError(e); // required of every implementation
        }
    }

    public static String of(File file) throws IOException {
        try (InputStream in = new FileInputStream(file)) {
            return of(in);
        }
    }

    public static String of(InputStream in) throws IOException {
        final MessageDigest digest = sha256();
        final byte[] buffer = new byte[64 * 1024];
        for (int n; (n = in.read(buffer)) > 0; ) {
            digest.update(buffer, 0, n);
        }
        return hex(digest.digest());
    }

    /** Copies, and returns what went past. */
    public static String copy(InputStream in, OutputStream out) throws IOException {
        return copy(in, out, Long.MAX_VALUE);
    }

    /**
     * Copies at most {@code limit} bytes, and refuses rather than truncating:
     * everything on the other end of one of these streams is somebody else's
     * archive or somebody else's server, and a compressed entry declares one
     * size and delivers another.
     */
    public static String copy(InputStream in, OutputStream out, long limit) throws IOException {
        final MessageDigest digest = sha256();
        long total = 0;
        try (DigestOutputStream sink = new DigestOutputStream(out, digest)) {
            final byte[] buffer = new byte[64 * 1024];
            for (int n; (n = in.read(buffer)) > 0; ) {
                total += n;
                if (total > limit) {
                    throw new IOException("That is larger than this can accept.");
                }
                sink.write(buffer, 0, n);
            }
            sink.flush();
        }
        return hex(digest.digest());
    }

    private static String hex(byte[] bytes) {
        final StringBuilder sb = new StringBuilder(bytes.length * 2);
        for (byte b : bytes) {
            sb.append(Character.forDigit((b >> 4) & 0xf, 16));
            sb.append(Character.forDigit(b & 0xf, 16));
        }
        return sb.toString();
    }
}
