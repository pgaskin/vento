// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.os.Bundle;
import android.view.View;
import android.view.ViewGroup;
import android.widget.TextView;

import androidx.appcompat.app.AppCompatActivity;
import androidx.core.graphics.Insets;
import androidx.core.view.ViewCompat;
import androidx.core.view.WindowCompat;
import androidx.core.view.WindowInsetsCompat;
import androidx.fragment.app.Fragment;

/**
 * A header over a fragment, with the system bars kept out of both.
 *
 * <p>The settings tree's frame. It was the connection editor's too, until the
 * editor became a sheet with a Save on it — which is the difference between the
 * two: a settings screen has no Save
 * because every row on it takes effect as it is touched, and a form does.
 */
public abstract class AppCompatToolbarActivity extends AppCompatActivity {

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        WindowCompat.setDecorFitsSystemWindows(getWindow(), false);
        setContentView(R.layout.activity_preferences);
        findViewById(R.id.back).setOnClickListener(
                v -> getOnBackPressedDispatcher().onBackPressed());
        setTitle(getTitle());
        ViewCompat.setOnApplyWindowInsetsListener(findViewById(R.id.root), (v, insets) -> {
            final Insets bars = insets.getInsets(WindowInsetsCompat.Type.systemBars());
            v.setPadding(bars.left, bars.top, bars.right, bars.bottom);
            return insets;
        });
    }

    /**
     * The header's own title, which is where a screen's title goes now that
     * the header is not a toolbar. Everything else about it is unchanged: the
     * window still has one, and it is still what the recents list shows.
     */
    @Override
    public void setTitle(CharSequence title) {
        super.setTitle(title);
        final TextView view = findViewById(R.id.title);
        if (view != null) {
            view.setText(title);
        }
    }

    /** Put a view in the frame under the header, in place of whatever is there. */
    protected void setContent(View view) {
        final ViewGroup frame = findViewById(R.id.content);
        frame.removeAllViews();
        frame.addView(view, new ViewGroup.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT));
    }

    /** Put a fragment in the frame under the header. */
    protected void show(Fragment fragment) {
        getSupportFragmentManager().beginTransaction()
                .replace(R.id.content, fragment)
                .commit();
    }
}
