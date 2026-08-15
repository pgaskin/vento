// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.content.Context;
import android.util.AttributeSet;

import androidx.appcompat.widget.AppCompatImageView;

/**
 * The desktop preview on a home card: as wide as the column, and a fixed
 * fraction of that tall.
 *
 * <p>The cards are a grid now, so the picture cannot have a height in dp — the
 * column width depends on how many columns fit, which depends on the screen.
 * What it can have is a ratio, chosen so that the picture plus the two lines of
 * text under it comes out roughly square whatever the column is.
 *
 * <p>Not the desktop's own aspect ratio, deliberately: a 16:10 card beside a 4:3
 * one reads as two sizes of card rather than as two desktops.
 */
public final class PreviewImageView extends AppCompatImageView {

    /** Height as a fraction of width. Leaves the card a little taller than wide. */
    private static final float RATIO = 0.72f;

    public PreviewImageView(Context context) {
        super(context);
    }

    public PreviewImageView(Context context, AttributeSet attrs) {
        super(context, attrs);
    }

    public PreviewImageView(Context context, AttributeSet attrs, int defStyleAttr) {
        super(context, attrs, defStyleAttr);
    }

    @Override
    protected void onMeasure(int widthMeasureSpec, int heightMeasureSpec) {
        super.onMeasure(widthMeasureSpec, heightMeasureSpec);
        setMeasuredDimension(getMeasuredWidth(), Math.round(getMeasuredWidth() * RATIO));
    }
}
