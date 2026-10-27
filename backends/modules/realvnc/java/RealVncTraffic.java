// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend.realvnc;

import android.util.Log;

import com.realvnc.vncviewer.jni.ConnectionInfoBindings;
import com.realvnc.vncviewer.jni.SessionBindings;

/**
 * How much this session has received, accumulated from a counter the viewer
 * core keeps but does not publish. The pointer walk is in
 * {@code cpp/realvnc_traffic.h}, called through {@code cpp/realvnc_traffic.c};
 * this class makes every decision the walk must not make itself.
 *
 * <p><b>The number is checked against a published one on every sample.</b> The
 * same six words produce the line speed and the round-trip time, and
 * {@code getLineSpeed} formats both through a supported call, so computing
 * them from the walk and matching them against that string checks, once a
 * second, that the memory being read is still the estimator. Nothing is
 * accumulated from a sample that disagrees, and three in a row retire the
 * counter for the rest of the session: if an offset has moved, the row must
 * disappear rather than show a plausible number.
 *
 * <p>The mark is 32 bits and the total is 64, so the total is accumulated from
 * deltas. A sample is skipped whenever the walk fails (the connection object is
 * null until there is a connection), and skipping loses nothing, because the
 * mark is a stream position rather than an increment.
 *
 * <p>Session thread only, necessarily: the estimator is written by the thread
 * that handles framebuffer updates, so a sample taken there cannot see it
 * half-written, and the string it is checked against is read one call later
 * with nothing in between.
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
    private long mark = -1; // no sample yet, as distinct from a mark of zero
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

        // The core's arithmetic, both parts: a kilobit total over an elapsed
        // time in 100 ns units, and a division that truncates towards zero. The
        // divisor is zero until the first window closes, where the core's
        // aarch64 sdiv returns zero and Java would throw.
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
        // The mark wraps at 32 bits, so the difference is taken modulo 2^32;
        // a mark that has gone back instead (a reconnected session's new
        // stream, counted from zero) is what the new stream has received,
        // not a difference, which would wrap to about 4 GiB.
        final long step = (position - mark) & 0xFFFFFFFFL;
        total = mark < 0 ? position : total + (step > 0x80000000L ? position : step);
        mark = position;

        if (!logged) {
            logged = true;
            Log.i(TAG, "the bandwidth estimator agrees with \"" + published + "\": "
                    + total + " bytes received so far");
        }
    }

    /**
     * Whether a string formatted by the core states these two numbers. The
     * core's template is {@code "%1$D kbit/s (RTT ~%2$Dms)"} in one locale and
     * reworded in others, so the comparison is of the two integers in it rather
     * than its shape, skipping digit group separators, since {@code %D} is the
     * core's own conversion and inserts them.
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
