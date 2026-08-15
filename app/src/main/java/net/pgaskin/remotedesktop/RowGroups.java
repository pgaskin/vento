// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.content.res.Resources;
import android.view.View;
import android.view.ViewGroup;

import java.util.ArrayList;
import java.util.List;

import com.google.android.material.textfield.TextInputLayout;

/**
 * Rows grouped onto a container with large corners, which is the one shape this
 * app's settings, panel and editor are all made of.
 *
 * <p>Where a row sits in its group is what decides its background — first,
 * middle, last, or a group of one — so this is the only place that knows the
 * four drawables, and both renderers come through it: the settings tree's
 * adapter binds a row at a time, and the hand-built sheets hand over a whole
 * container.
 *
 * <p>A row that is already a container of its own is left out of the run: an
 * outlined text field has a box and corners of its own, and a card behind it is
 * a second one. It keeps the group's inset, takes a wider gap, and ends the run
 * either side of itself.
 */
final class RowGroups {

    private RowGroups() {
    }

    /** The background for a row, given the ends of the group it is in. */
    static int background(boolean first, boolean last) {
        if (first) {
            return last ? R.drawable.bg_group_single : R.drawable.bg_group_top;
        }
        return last ? R.drawable.bg_group_bottom : R.drawable.bg_group_middle;
    }

    /** The gap between two rows of one group. */
    static int gap(View v) {
        return v.getResources().getDimensionPixelSize(R.dimen.group_gap);
    }

    /** The gap between one group and the next, where no heading separates them. */
    static int spacing(View v) {
        return v.getResources().getDimensionPixelSize(R.dimen.group_spacing);
    }

    /**
     * One row: its background, the inset either side, and the gap under it.
     * Every call sets all three, since a recycled row arrives wearing the last
     * position's answers.
     */
    static void row(View view, boolean first, boolean last, int gapBelow) {
        view.setBackgroundResource(background(first, last));
        margins(view, gapBelow);
    }

    /** A row with no group: no background, no inset, and no gap of its own. */
    static void plain(View view) {
        view.setBackground(null);
        final ViewGroup.MarginLayoutParams lp = params(view);
        if (lp != null) {
            lp.setMarginStart(0);
            lp.setMarginEnd(0);
            lp.bottomMargin = 0;
            view.setLayoutParams(lp);
        }
    }

    /**
     * Every child of a container as one group, in the order they were added.
     * Called after the rows are built, and again whenever they are rebuilt.
     *
     * <p>A hidden row is not in the group: an option that is only offered under
     * some other option's answer leaves a run of rows behind it, and a gap where
     * the corners do not meet is exactly what the run is drawn to avoid.
     */
    static void apply(ViewGroup container) {
        final Resources res = container.getResources();
        final int gap = res.getDimensionPixelSize(R.dimen.group_gap);
        final int fieldGap = res.getDimensionPixelSize(R.dimen.group_gap_field);
        final List<View> shown = new ArrayList<>();
        for (int i = 0; i < container.getChildCount(); i++) {
            if (container.getChildAt(i).getVisibility() != View.GONE) {
                shown.add(container.getChildAt(i));
            }
        }
        final int n = shown.size();
        for (int i = 0; i < n; i++) {
            final View view = shown.get(i);
            final boolean own = ownContainer(view);
            final boolean first = i == 0 || ownContainer(shown.get(i - 1));
            final boolean last = i == n - 1 || ownContainer(shown.get(i + 1));
            if (own) {
                view.setBackground(null);
            } else {
                view.setBackgroundResource(background(first, last));
            }
            // The last row's gap is the container's business: what follows it is
            // another section, or the end of the sheet.
            margins(view, i == n - 1 ? 0 : (own || ownContainer(shown.get(i + 1))
                    ? fieldGap : gap));
        }
    }

    private static boolean ownContainer(View view) {
        return view instanceof TextInputLayout;
    }

    private static void margins(View view, int gapBelow) {
        final ViewGroup.MarginLayoutParams lp = params(view);
        if (lp == null) {
            return;
        }
        final int inset = view.getResources().getDimensionPixelSize(R.dimen.group_inset);
        lp.setMarginStart(inset);
        lp.setMarginEnd(inset);
        lp.bottomMargin = gapBelow;
        view.setLayoutParams(lp);
    }

    private static ViewGroup.MarginLayoutParams params(View view) {
        return view.getLayoutParams() instanceof ViewGroup.MarginLayoutParams lp ? lp : null;
    }
}
