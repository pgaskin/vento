// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.InputStream;

/** The InputStream reads Android only has from API 33. */
final class Streams {
    private Streams() {}

    /** {@link InputStream#readAllBytes}. */
    static byte[] readAll(InputStream in) throws IOException {
        return readN(in, Integer.MAX_VALUE);
    }

    /** {@link InputStream#readNBytes(int)}. */
    static byte[] readN(InputStream in, int max) throws IOException {
        final ByteArrayOutputStream out = new ByteArrayOutputStream();
        final byte[] buf = new byte[8192];
        int n;
        while (out.size() < max
                && (n = in.read(buf, 0, Math.min(buf.length, max - out.size()))) != -1) {
            out.write(buf, 0, n);
        }
        return out.toByteArray();
    }
}
