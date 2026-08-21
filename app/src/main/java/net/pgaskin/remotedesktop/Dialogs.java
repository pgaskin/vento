// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.content.Context;
import android.content.DialogInterface;
import android.content.res.TypedArray;
import android.util.TypedValue;
import android.view.ContextThemeWrapper;

import androidx.appcompat.app.AlertDialog;

import com.google.android.material.checkbox.MaterialCheckBox;
import com.google.android.material.dialog.MaterialAlertDialogBuilder;

import java.util.ArrayList;
import java.util.List;

/**
 * What every dialog in this app needs and none of it is about: the theme they
 * are all built against, the two things that go inside one and have to match
 * its own metrics, and the list that takes them down again.
 */
final class Dialogs {

    private Dialogs() {
    }

    /**
     * A builder against this app's dialog theme, which is not decoration:
     * {@code ThemeOverlay.RemoteDesktop.Dialog} is what stops an alert arriving
     * in two tones with a hard edge across it. Named once here because a style
     * reference repeated at every call site is the kind of thing one of them
     * silently stops doing.
     */
    static MaterialAlertDialogBuilder builder(Context ctx) {
        return new MaterialAlertDialogBuilder(ctx, R.style.ThemeOverlay_RemoteDesktop_Dialog);
    }

    /**
     * A tick box for a dialog, built against the dialog's own theme.
     *
     * <p>Both of those matter and neither is decoration. A plain
     * {@code android.widget.CheckBox} built from an activity is the framework's,
     * not Material's: different metrics, and a tint from the activity rather
     * than from the surface it ends up on. And the box is drawn at the view's
     * left edge with the label after it, so the padding that lines it up with
     * the dialog's own text belongs on whatever contains it — put here, it
     * moves the box and leaves the label where it was.
     */
    static MaterialCheckBox checkBox(Context ctx, int text) {
        final MaterialCheckBox box = new MaterialCheckBox(
                new ContextThemeWrapper(ctx, R.style.ThemeOverlay_RemoteDesktop_Dialog));
        box.setText(text);
        box.setPadding(0, box.getPaddingTop(), box.getPaddingRight(), box.getPaddingBottom());
        return box;
    }

    /** What a dialog indents its title and message by, so a view can match. */
    static int padding(Context ctx) {
        final TypedArray a = new ContextThemeWrapper(ctx, R.style.ThemeOverlay_RemoteDesktop_Dialog)
                .obtainStyledAttributes(new int[]{androidx.appcompat.R.attr.dialogPreferredPadding});
        final int pad = a.getDimensionPixelSize(0, dp(ctx, 24));
        a.recycle();
        return pad;
    }

    static int dp(Context ctx, float v) {
        return Math.round(TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_DIP, v,
                ctx.getResources().getDisplayMetrics()));
    }

    /**
     * Every dialog a screen has put up, so that its {@code onDestroy} can take
     * them down: one still showing when its activity goes is a leaked window.
     */
    static final class Tracker {

        private final List<AlertDialog> open = new ArrayList<>();

        AlertDialog track(AlertDialog d) {
            return track(d, null);
        }

        /**
         * A dialog has exactly <em>one</em> dismiss listener, and this method
         * owns it — so a caller that needs to know when its dialog goes hands
         * the work in here rather than setting one of its own, which would
         * silently replace this one or be replaced by it depending on the order.
         */
        AlertDialog track(AlertDialog d, DialogInterface.OnDismissListener also) {
            open.add(d);
            d.setOnDismissListener(x -> {
                open.remove(d);
                if (also != null) {
                    also.onDismiss(x);
                }
            });
            return d;
        }

        void dismissAll() {
            for (AlertDialog d : new ArrayList<>(open)) {
                d.dismiss();
            }
            open.clear();
        }
    }
}
