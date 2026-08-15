// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.app.Activity;
import android.os.Bundle;
import android.view.WindowInsets;
import android.view.WindowInsetsController;
import android.view.WindowManager;

import net.pgaskin.remotedesktop.control.playground.PlaygroundView;

/**
 * The fake desktop, as a screen inside the app rather than as the app.
 *
 * <p>It was the whole app before there was a protocol to speak, and is now
 * reachable from the settings tree: a test surface with known geometry for
 * exercising the input options, and where the fixture recorder lives.
 */
public final class PlaygroundActivity extends Activity {

    // setDecorFitsSystemWindows is deprecated because a build targeting API 35
    // or above is edge to edge whether it asks or not. This one still runs on
    // 34, where it has to ask.
    @SuppressWarnings("deprecation")
    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        getWindow().setDecorFitsSystemWindows(false);
        final PlaygroundView view = new PlaygroundView(this,
                InputSettings.config(this, getResources().getDisplayMetrics().density));
        // adb shell am start -n net.pgaskin.remotedesktop/.PlaygroundActivity --ez record true
        view.setRecording(getIntent().getBooleanExtra("record", false));
        // ... and --ez keys true for the key trace, which is what a keyboard walk reads.
        view.setKeyTrace(getIntent().getBooleanExtra("keys", false));
        // ... and --ez relative true for a far end that owns the cursor.
        view.setRelativePointer(getIntent().getBooleanExtra("relative", false));
        setContentView(view);
    }

    @Override
    public void onWindowFocusChanged(boolean hasFocus) {
        super.onWindowFocusChanged(hasFocus);
        if (hasFocus) {
            final WindowInsetsController bars = getWindow().getInsetsController();
            if (bars != null) {
                // Sticky immersive: a swipe brings the bars back for a moment
                // and they go again on their own, so a gesture aimed at the
                // desktop edge does not leave them up.
                bars.setSystemBarsBehavior(
                        WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE);
                bars.hide(WindowInsets.Type.systemBars());
            }
        }
    }
}
