// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.app.Activity;
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
        final var dialog = new MaterialAlertDialogBuilder(ctx, R.style.ThemeOverlay_RemoteDesktop_Dialog)
                .setTitle(R.string.log_title)
                .setView(tall)
                .setPositiveButton(android.R.string.ok, null)
                .setNeutralButton(R.string.log_copy, null)
                .show();
        // The button is handled here rather than by the builder: copy the text as
        // it is now, then close. An empty log is refused in copy() itself.
        final android.widget.Button copyButton =
                dialog.getButton(android.content.DialogInterface.BUTTON_NEUTRAL);
        copyButton.setOnClickListener(v -> {
            Log.i(TAG, "copy tapped, " + text[0].length() + " chars");
            copyWhenFocused(ctx, text[0], 0);
            dialog.dismiss();
        });

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

    /** Waits, a few times over, for the activity to have focus again before copying. */
    private static void copyWhenFocused(Context ctx, String text, int attempt) {
        if (!(ctx instanceof Activity activity) || activity.hasWindowFocus() || attempt >= 20) {
            copy(ctx, text);
            return;
        }
        new Handler(Looper.getMainLooper()).postDelayed(
                () -> copyWhenFocused(ctx, text, attempt + 1), 50);
    }

    private static void copy(Context ctx, String text) {
        final boolean focused = !(ctx instanceof Activity a) || a.hasWindowFocus();
        Log.i(TAG, "copy: " + (text == null ? -1 : text.length()) + " chars, focused " + focused);
        final ClipboardManager cm = ctx.getSystemService(ClipboardManager.class);
        if (cm == null || text == null || text.isEmpty()) {
            Log.w(TAG, "copy: nothing to copy (clipboard " + (cm != null) + ")");
            return;
        }
        try {
            cm.setPrimaryClip(ClipData.newPlainText("log", text));
            Log.i(TAG, "copy: clipboard now has " + cm.hasPrimaryClip());
        } catch (RuntimeException e) {
            Log.w(TAG, "copy: the clipboard refused", e);
            return;
        }
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
