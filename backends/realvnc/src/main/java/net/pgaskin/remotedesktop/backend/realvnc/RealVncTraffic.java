// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend.realvnc;

import android.util.Log;

import com.realvnc.vncviewer.jni.ConnectionInfoBindings;
import com.realvnc.vncviewer.jni.SessionBindings;

/**
 * How much this session has received, accumulated from a counter the viewer
 * core keeps and does not publish. The walk itself is
 * {@code src/main/cpp/realvnc_traffic.c}; what is here is everything the walk
 * must not decide for itself.
 *
 * <p><b>The number is checked against a published one on every sample.</b> The
 * same six words produce the line speed and the round-trip time, and
 * {@code getLineSpeed} formats both through a supported call — so computing
 * them from the walk and matching them against that string is the running app
 * asking, once a second, whether the memory it is reading is still the
 * estimator. Nothing is accumulated from a sample that disagrees, and three in
 * a row retire the counter for the rest of the session: an offset that has
 * moved must lose the row, not fill it with a plausible number.
 *
 * <p>The mark is 32 bits and the total is 64, so what is accumulated is deltas.
 * A sample is skipped whenever the walk fails — the connection object is null
 * until there is a connection — and skipping costs nothing, because the mark is
 * a stream position rather than an increment.
 *
 * <p>Session thread only, and it has to be: the estimator is written by the
 * same thread that handles framebuffer updates, so a sample taken there cannot
 * see it half-written, and the string it is checked against is read one call
 * later with nothing in between.
 */
final class RealVncTraffic {

    private static final String TAG = RealVncCore.TAG;

    static {
        System.loadLibrary("remotedesktop_realvnc");
    }

    /** {@code {elapsed, kilobits, rtt, mark}}, all four raw; false if any hop is unreadable. */
    private static native boolean nativeEstimator(long token, long[] out);

    private static final int GIVE_UP_AFTER = 3;

    private final long[] fields = new long[4];

    private long total;
    private long mark = -1; // no sample yet, which is not a mark of zero
    private int mismatches;
    private boolean agreed;
    private boolean gaveUp;
    private boolean logged;

    /** Bytes received since the connection opened, or -1 if there is no trustworthy number. */
    long received() {
        return agreed && !gaveUp ? total : -1;
    }

    void sample(SessionBindings.Session session) {
        if (gaveUp || session == null) {
            return;
        }
        if (!nativeEstimator(session.getToken(), fields)) {
            return;
        }
        final long elapsed = fields[0], kilobits = fields[1], rtt = fields[2], position = fields[3];

        // Their arithmetic, both of it: a kilobit total over an elapsed time in
        // 100 ns units, and a division that truncates towards zero. The divisor
        // is zero until the first window closes, where their aarch64 sdiv
        // answers zero and Java would throw.
        final int speed = elapsed == 0 ? 0 : (int) (kilobits * 10_000_000L / elapsed);
        final int rttMs = (int) (rtt / 10_000);
        final String published = ConnectionInfoBindings.getLineSpeed(session);
        if (!states(published, speed, rttMs)) {
            if (++mismatches >= GIVE_UP_AFTER) {
                gaveUp = true;
                Log.w(TAG, "the line speed estimator is not where it was: \"" + published
                        + "\" against " + speed + " kbit/s, " + rttMs + " ms — no byte count");
            }
            return;
        }
        mismatches = 0;
        agreed = true;

        // The first mark is the total rather than a baseline: the position is
        // the stream's, so it already counts everything before this sample.
        total = mark < 0 ? position : total + ((position - mark) & 0xFFFFFFFFL);
        mark = position;

        if (!logged) {
            logged = true;
            Log.i(TAG, "the bandwidth estimator agrees with \"" + published + "\": "
                    + total + " bytes received so far");
        }
    }

    /**
     * Does a string the core formatted say these two numbers? Their template is
     * {@code "%1$D kbit/s (RTT ~%2$Dms)"} in one locale and reworded in others,
     * so what is compared is the two integers in it rather than its shape —
     * with whatever groups the digits skipped, since {@code %D} is their own
     * conversion and puts a separator in.
     */
    private static boolean states(String published, int speed, int rttMs) {
        if (published == null || published.isEmpty()) {
            return false;
        }
        long first = -1, second = -1, value = 0;
        boolean inNumber = false;
        for (int i = 0; i <= published.length(); i++) {
            final char c = i < published.length() ? published.charAt(i) : ' ';
            final boolean grouped = inNumber && isGroupSeparator(c)
                    && i + 1 < published.length() && isDigit(published.charAt(i + 1));
            if (isDigit(c)) {
                value = value * 10 + (c - '0');
                inNumber = true;
            } else if (!grouped && inNumber) {
                if (first < 0) {
                    first = value;
                } else if (second < 0) {
                    second = value;
                }
                value = 0;
                inNumber = false;
            }
        }
        return first == speed && second == rttMs;
    }

    private static boolean isDigit(char c) {
        return c >= '0' && c <= '9';
    }

    /** Comma, full stop and the three spaces a number can be grouped with. */
    private static boolean isGroupSeparator(char c) {
        return c == ',' || c == '.' || c == ' ' || c == '\u00a0' || c == '\u202f';
    }
}
