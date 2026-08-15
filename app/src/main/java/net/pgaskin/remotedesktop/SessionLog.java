// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Context;
import android.os.Handler;
import android.os.Looper;
import android.util.Log;
import android.graphics.Typeface;
import android.util.TypedValue;
import android.widget.HorizontalScrollView;
import android.widget.ScrollView;
import android.widget.TextView;
import android.widget.Toast;

import com.google.android.material.dialog.MaterialAlertDialogBuilder;

import java.io.BufferedReader;
import java.io.InputStreamReader;
import java.util.ArrayDeque;
import java.util.Deque;

/**
 * This process's own log, on screen.
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

    static void show(Context ctx) {
        final TextView view = new TextView(ctx);
        view.setText(R.string.log_reading);
        view.setTextIsSelectable(true);
        view.setTypeface(Typeface.MONOSPACE);
        view.setTextSize(TypedValue.COMPLEX_UNIT_SP, 10);
        final int pad = Math.round(TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_DIP, 16,
                ctx.getResources().getDisplayMetrics()));
        view.setPadding(pad, pad / 2, pad, 0);
        // Horizontally too: a log line is as long as it is, and wrapping one at
        // a phone's width is what makes a log unreadable.
        final HorizontalScrollView wide = new HorizontalScrollView(ctx);
        wide.addView(view);
        final ScrollView tall = new ScrollView(ctx);
        tall.addView(wide);

        final String[] text = {""};
        new MaterialAlertDialogBuilder(ctx, R.style.ThemeOverlay_RemoteDesktop_Dialog)
                .setTitle(R.string.log_title)
                .setView(tall)
                .setPositiveButton(android.R.string.ok, null)
                .setNeutralButton(R.string.log_copy, (d, w) -> copy(ctx, text[0]))
                .show();

        // Forking logcat and draining a whole log buffer is tens to hundreds of
        // milliseconds, and both places this is reachable from are moments the
        // app is already in trouble. The dialog goes up empty and fills in.
        final Handler main = new Handler(Looper.getMainLooper());
        new Thread(() -> {
            final String read = read(ctx);
            main.post(() -> {
                text[0] = read;
                view.setText(read);
                // Showing the top of a log is showing the oldest thing in it;
                // what somebody opening this wants is what just happened.
                tall.post(() -> tall.fullScroll(ScrollView.FOCUS_DOWN));
            });
        }, "session-log").start();
    }

    private static void copy(Context ctx, String text) {
        final ClipboardManager cm = ctx.getSystemService(ClipboardManager.class);
        if (cm == null) {
            return;
        }
        cm.setPrimaryClip(ClipData.newPlainText("log", text));
        Toast.makeText(ctx, R.string.log_copied, Toast.LENGTH_SHORT).show();
    }

    /** The last {@link #LINES} lines this process logged, oldest first. */
    private static String read(Context ctx) {
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
