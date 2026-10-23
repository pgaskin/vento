// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Context;
import android.util.Log;
import android.widget.Toast;

import java.io.BufferedReader;
import java.io.InputStreamReader;
import java.util.ArrayDeque;
import java.util.Deque;

/**
 * This process's own log: read it, and put it on the clipboard. The screen that
 * shows it is {@link LogActivity}.
 *
 * <p>A failed connection puts one sentence on the session screen, and the
 * sentence is whatever the protocol thought was worth saying. Everything that
 * led to it — which security types a server offered, which encoding was
 * negotiated, what the library said on its way out — is in the log, and until
 * now the only way to read that was a cable and {@code adb}.
 *
 * <p>An app may read its own log entries without any permission: the log daemon
 * filters by uid, so {@code --pid} is a second filter rather than the only one.
 * Nothing else on the device is readable from here.
 */
final class SessionLog {

    private static final String TAG = "SessionLog";

    /** Enough to hold a connection attempt and what went before it. */
    private static final int LINES = 400;

    private SessionLog() {
    }

    /** Called from the log screen while it is in front, which is what lets the clipboard take it. */
    static void copy(Context ctx, String text) {
        final ClipboardManager cm = ctx.getSystemService(ClipboardManager.class);
        if (cm == null || text == null || text.isEmpty()) {
            return;
        }
        try {
            cm.setPrimaryClip(ClipData.newPlainText("log", text));
        } catch (RuntimeException e) {
            Log.w(TAG, "the clipboard refused the log", e);
            return;
        }
        Toast.makeText(ctx, R.string.log_copied, Toast.LENGTH_SHORT).show();
    }

    /** The last {@link #LINES} lines this process logged, oldest first. */
    static String read(Context ctx) {
        final Deque<String> tail = new ArrayDeque<>();
        try {
            final Process p = new ProcessBuilder(
                    "logcat", "-d", "-v", "time", "--pid=" + android.os.Process.myPid())
                    .redirectErrorStream(true)
                    .start();
            try (BufferedReader in = new BufferedReader(
                    new InputStreamReader(p.getInputStream()))) {
                String line;
                while ((line = in.readLine()) != null) {
                    if (tail.size() == LINES) {
                        tail.removeFirst();
                    }
                    tail.addLast(line);
                }
            }
            p.waitFor();
        } catch (Exception e) {
            Log.w(TAG, "reading the log", e);
            return ctx.getString(R.string.log_unreadable);
        }
        return tail.isEmpty() ? ctx.getString(R.string.log_empty) : String.join("\n", tail);
    }
}
