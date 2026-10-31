// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.os.Bundle;
import android.util.Log;
import android.webkit.WebView;

import java.io.IOException;
import java.io.InputStream;
import java.nio.charset.StandardCharsets;

/**
 * The licence page, which the build generates into the assets: this app's own
 * modules, the Rust crates the backends in <em>this APK</em> are built on, and
 * what that means for redistributing the build.
 *
 * <p>An add-on is a separate build under its own terms, so it generates its own
 * page into its own APK, and this screen shows that page when given
 * {@link #PACKAGE}. The user only browses the app, not the add-on, so the
 * add-on's page is shown here. The only contract is the asset's path, so an
 * add-on made only of this project's code ships no page and gets no row.
 *
 * <p>A {@link WebView} because the page is generated HTML and licence texts are
 * long: it provides scrolling, selection and the system's dark mode. Nothing in
 * it is loaded from the network and no JavaScript runs.
 */
public final class LicensesActivity extends AppCompatToolbarActivity {

    private static final String TAG = "Licenses";

    /** Which package's page to show; this app's own when it is absent. */
    static final String PACKAGE = "package";

    static final String ASSET = "licenses/index.html";
    private static final String PAGE = "file:///android_asset/" + ASSET;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        setTitle(R.string.settings_licenses);
        final WebView web = new WebView(this);
        final String plugin = getIntent().getStringExtra(PACKAGE);
        final String page = plugin == null ? null : read(this, plugin);
        if (page == null) {
            web.loadUrl(PAGE);
        } else {
            // Another package's assets cannot be named in a URL, so the page is
            // read and passed as text. No base URL: it is one self-contained
            // document and nothing in it may resolve anywhere.
            web.loadDataWithBaseURL(null, page, "text/html", "UTF-8", null);
        }
        setContent(web);
    }

    /**
     * An add-on's own page, or null if it has none (the usual case).
     */
    static String read(Context context, String packageName) {
        try (InputStream in = open(context, packageName)) {
            return in == null ? null : new String(Streams.readAll(in), StandardCharsets.UTF_8);
        } catch (IOException e) {
            Log.i(TAG, "reading the licence page in " + packageName, e);
            return null;
        }
    }

    /**
     * Whether that add-on has a page, for the row that would open it. Called
     * while a screen is being built, so it only opens the asset rather than
     * reading a megabyte of licence text.
     */
    static boolean has(Context context, String packageName) {
        try (InputStream in = open(context, packageName)) {
            return in != null;
        } catch (IOException e) {
            return false;
        }
    }

    private static InputStream open(Context context, String packageName) {
        try {
            return context.getPackageManager()
                    .getResourcesForApplication(packageName).getAssets().open(ASSET);
        } catch (IOException | PackageManager.NameNotFoundException e) {
            // Not an error: most add-ons contain no third-party code.
            return null;
        }
    }

    static Intent of(Context context, String packageName) {
        return new Intent(context, LicensesActivity.class).putExtra(PACKAGE, packageName);
    }
}
