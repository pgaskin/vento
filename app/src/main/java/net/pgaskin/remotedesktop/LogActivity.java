// SPDX-FileCopyrightText: 2026 Nick Haghiri
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.content.Context;
import android.content.Intent;
import android.graphics.Typeface;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.util.TypedValue;
import android.view.ViewGroup;
import android.widget.HorizontalScrollView;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.TextView;

import com.google.android.material.button.MaterialButton;

/**
 * The session log, on a screen of its own.
 *
 * <p>A dialog was the first try, and its Copy button never reliably put anything
 * on the clipboard: the dialog takes the focus the clipboard needs, and gives it
 * back a moment late. This screen is the window in front, so Copy and Share here
 * are plain buttons on the page.
 */
public final class LogActivity extends AppCompatToolbarActivity {

    /** The log as it has been read, empty until the reading is done. */
    private String text = "";

    static void open(Context ctx) {
        ctx.startActivity(new Intent(ctx, LogActivity.class));
    }

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        setTitle(getString(R.string.log_title));

        final TextView view = new TextView(this);
        view.setText(R.string.log_reading);
        view.setTextIsSelectable(true);
        view.setTypeface(Typeface.MONOSPACE);
        view.setTextSize(TypedValue.COMPLEX_UNIT_SP, 10);
        final int pad = dp(16);
        view.setPadding(pad, pad / 2, pad, pad);
        // Horizontally too: a log line is as long as it is, and wrapping one at
        // a phone's width is what makes a log unreadable.
        final HorizontalScrollView wide = new HorizontalScrollView(this);
        wide.addView(view);
        final ScrollView tall = new ScrollView(this);
        tall.addView(wide);

        final LinearLayout actions = new LinearLayout(this);
        actions.setOrientation(LinearLayout.HORIZONTAL);
        actions.setPadding(pad, pad / 2, pad, pad / 2);
        final MaterialButton copy = new MaterialButton(this);
        copy.setText(R.string.log_copy);
        copy.setOnClickListener(v -> SessionLog.copy(this, text));
        final MaterialButton share = new MaterialButton(this);
        share.setText(R.string.log_share);
        share.setOnClickListener(v -> share());
        // Two equal halves across the row, with a gap between them.
        final int gap = dp(8);
        final LinearLayout.LayoutParams copyParams = new LinearLayout.LayoutParams(0,
                ViewGroup.LayoutParams.WRAP_CONTENT, 1f);
        copyParams.setMarginEnd(gap / 2);
        final LinearLayout.LayoutParams shareParams = new LinearLayout.LayoutParams(0,
                ViewGroup.LayoutParams.WRAP_CONTENT, 1f);
        shareParams.setMarginStart(gap / 2);
        actions.addView(copy, copyParams);
        actions.addView(share, shareParams);

        final LinearLayout page = new LinearLayout(this);
        page.setOrientation(LinearLayout.VERTICAL);
        page.addView(actions, new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT));
        page.addView(tall, new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, 0, 1f));
        setContent(page);

        // Forking logcat and draining a whole log buffer takes tens to hundreds of
        // milliseconds, so the page goes up empty and fills in.
        final Handler main = new Handler(Looper.getMainLooper());
        new Thread(() -> {
            final String read = SessionLog.read(this);
            main.post(() -> {
                text = read;
                view.setText(read);
                // What somebody opening this wants is the newest end of it.
                tall.post(() -> tall.fullScroll(ScrollView.FOCUS_DOWN));
            });
        }, "session-log").start();
    }

    /** The system's own sheet, for sending the log somewhere other than the clipboard. */
    private void share() {
        if (text.isEmpty()) {
            return;
        }
        final Intent send = new Intent(Intent.ACTION_SEND)
                .setType("text/plain")
                .putExtra(Intent.EXTRA_TEXT, text);
        startActivity(Intent.createChooser(send, getString(R.string.log_title)));
    }

    private int dp(int value) {
        return Math.round(TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_DIP, value,
                getResources().getDisplayMetrics()));
    }
}
