// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.os.Bundle;
import android.webkit.WebView;

/**
 * The licence page, which the build generates into the assets: this app's own
 * modules, the Rust crates the open backends are built on, and — in a build that
 * contains RealVNC's library — what that means for handing the build on.
 *
 * <p>A {@link WebView} because the page is generated HTML and because licence
 * texts are long: it brings scrolling, selection and the system's own dark mode
 * with it. Nothing in it is loaded from the network and no JavaScript runs.
 */
public final class LicensesActivity extends AppCompatToolbarActivity {

    private static final String PAGE = "file:///android_asset/licenses/index.html";

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        setTitle(R.string.settings_licenses);
        final WebView web = new WebView(this);
        web.loadUrl(PAGE);
        setContent(web);
    }
}
