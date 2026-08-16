// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.plugin;

import android.app.Activity;
import android.content.Context;
import android.content.Intent;
import android.net.ConnectivityManager;
import android.net.NetworkInfo;
import android.net.Uri;
import android.os.Bundle;
import android.provider.Settings;
import android.os.Handler;
import android.os.Looper;
import android.util.Log;
import android.graphics.Insets;
import android.view.View;
import android.view.WindowInsets;
import android.widget.Button;
import android.widget.ProgressBar;
import android.widget.TextView;

import java.io.File;
import java.io.IOException;
import java.io.InputStream;
import java.io.FileOutputStream;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

/**
 * The screen an add-on acquires what it holds on.
 *
 * <p>It runs in the add-on's own process, which is the whole reason it exists:
 * the packages it may look at and the network it may use are the add-on's, so
 * the app's manifest asks for neither. Two routes, both ending in the same
 * check — an archive somebody picks, and one fetched from a mirror.
 *
 * <p>A subclass says what it is for and what it wants; everything about how a
 * library is verified and stored is here.
 */
public abstract class SetupActivity extends Activity {

    private static final String TAG = "PluginSetup";

    /**
     * One acquisition at a time, for the life of the process rather than of a
     * screen.
     *
     * <p>It was one per activity, and a rotation part-way through an
     * acquisition was then two of them: {@code shutdownNow} interrupts a thread
     * blocked in a file copy without stopping it, and the recreated screen
     * offered the same buttons — so two threads could be writing one
     * temporary file. Nothing unverified could come of that, since the hash
     * would not match, but "it says no twice" is not an answer anybody can act
     * on. What survives a rotation is here; what the screen shows is worked out
     * from it in {@link #onResume}.
     */
    private static final ExecutorService WORKER = Executors.newSingleThreadExecutor(r -> {
        final Thread t = new Thread(r, "plugin-setup");
        t.setDaemon(true);
        return t;
    });
    private static final Handler MAIN = new Handler(Looper.getMainLooper());

    /**
     * The application's context, for looking a string up off the worker: a route
     * outlives a rotation, and a string looked up on a dead screen's context is
     * one of the ways that shows up much later.
     */
    private static Context strings;

    /**
     * What the process is in the middle of, all main-thread only: whether a
     * route is running, what it is doing, how far through it is, and what the
     * last one that finished said. A screen is drawn from these rather than
     * holding any of it, so a rotation shows what a rotation should.
     */
    private static boolean busy;
    private static String doing;
    private static long done;
    private static long total;
    private static String failure;
    /** Whether that failure was this app being refused the network. */
    private static boolean denied;

    private TextView status;
    private TextView pins;
    private ProgressBar progress;
    private View buttons;
    private View fix;

    /** What this add-on is for, and where its library comes from. */
    protected abstract CharSequence explanation();

    /** Where a copy can be fetched, tried in order. */
    protected abstract List<Download.Source> downloads();

    /**
     * Name to SHA-256, for this device. Empty where there is no build of them
     * for it, which is a screen with nothing to offer rather than an error.
     */
    protected abstract Map<String, String> wanted();

    /** The ABI the libraries are wanted for, null where there is none. */
    protected abstract String abi();

    private final ActivityLauncher picker = new ActivityLauncher();

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        setContentView(R.layout.plugin_setup);
        strings = getApplicationContext();

        // The window is edge to edge and this screen has no bar of its own to
        // hang a title on, so both are done here: the label from the manifest at
        // the top, and the system's own bars kept out of the content.
        final View content = findViewById(R.id.plugin_content);
        content.setOnApplyWindowInsetsListener((v, insets) -> {
            final Insets bars = insets.getInsets(
                    WindowInsets.Type.systemBars() | WindowInsets.Type.displayCutout());
            v.setPadding(bars.left, bars.top, bars.right, bars.bottom);
            return insets;
        });
        ((TextView) findViewById(R.id.plugin_title)).setText(getTitle());
        ((TextView) findViewById(R.id.plugin_explanation)).setText(explanation());
        status = findViewById(R.id.plugin_status);
        pins = findViewById(R.id.plugin_pins);
        progress = findViewById(R.id.plugin_progress);
        buttons = findViewById(R.id.plugin_buttons);
        fix = findViewById(R.id.plugin_fix_network);
        fix.setOnClickListener(v -> offerNetwork());
        readColors();

        // A phone none of the pinned builds is for. Nothing below can end in
        // anything but a refusal, so none of it is offered: the explanation
        // stays, since it is what this add-on is for either way.
        nothingForThisDevice = wanted().isEmpty();
        if (nothingForThisDevice) {
            buttons.setVisibility(View.GONE);
            show(getString(R.string.plugin_wrong_device), false);
            return;
        }

        pins.setText(pins());
        pins.setVisibility(View.VISIBLE);

        findViewById(R.id.plugin_from_file).setOnClickListener(v -> picker.launch(
                new Intent(Intent.ACTION_OPEN_DOCUMENT)
                        .addCategory(Intent.CATEGORY_OPENABLE)
                        .setType("*/*")));

        findViewById(R.id.plugin_from_download).setOnClickListener(v -> run(() -> {
            final File apk = File.createTempFile("download", ".apk", getCacheDir());
            try {
                requireNetwork();
                // Named as it goes rather than up front, since which mirror is
                // answering is the part of this that is not decided in advance.
                Download.toFile(downloads(), apk, SetupActivity::progress, new Download.Where() {
                    @Override
                    public void asking(String host) {
                        say(R.string.plugin_asking, host);
                    }

                    @Override
                    public void fetching(String host) {
                        say(R.string.plugin_downloading, host);
                    }
                });
                return take(apk);
            } finally {
                //noinspection ResultOfMethodCallIgnored
                apk.delete();
            }
        }));

        picker.onResult(uri -> run(() -> {
            final File apk = File.createTempFile("picked", ".apk", getCacheDir());
            try {
                say(R.string.plugin_reading);
                try (InputStream in = getContentResolver().openInputStream(uri);
                     FileOutputStream out = new FileOutputStream(apk)) {
                    if (in == null) {
                        throw new IOException(getString(R.string.plugin_unreadable));
                    }
                    Hashes.copy(in, out);
                }
                return take(apk);
            } finally {
                //noinspection ResultOfMethodCallIgnored
                apk.delete();
            }
        }));
    }

    /**
     * A refusal a download would otherwise report as a DNS failure.
     *
     * <p>An app whose network access has been switched off resolves no host at
     * all — every lookup fails in a few milliseconds with "unable to resolve
     * host", which is a sentence about DNS for something that is nothing to do
     * with it. The state is exactly reported, in the one place that still
     * reports it: {@code getActiveNetwork} is null either way, and only the
     * deprecated {@code NetworkInfo} distinguishes being refused from there
     * being nothing to be refused.
     *
     * <p>It cannot be asked for. The policy behind it is settable only with a
     * signature permission, so there is no request to make and no result to
     * wait for — what there is is the screen it is switched on, which
     * {@link #offerNetwork} opens.
     */
    @SuppressWarnings("deprecation")
    private void requireNetwork() throws IOException {
        final NetworkInfo info =
                getSystemService(ConnectivityManager.class).getActiveNetworkInfo();
        if (info != null && info.getDetailedState() == NetworkInfo.DetailedState.BLOCKED) {
            denied = true;
            throw new IOException(getString(R.string.plugin_no_network));
        }
    }

    /** Settings' own page for it, on the switch itself. */
    private void offerNetwork() {
        startActivity(new Intent(
                Settings.ACTION_IGNORE_BACKGROUND_DATA_RESTRICTIONS_SETTINGS,
                Uri.fromParts("package", getPackageName(), null)));
    }

    /** The half both routes share: what is in the archive, checked. */
    private Map<String, String> take(File apk) throws IOException {
        say(R.string.plugin_opening);
        progress(0, -1);
        // Each one named as it comes out: a library is hashed while it is being
        // written, so this line is also which of them is being verified.
        return Apks.take(this, List.of(apk), abi(), wanted(),
                name -> say(R.string.plugin_extracting, name));
    }

    /**
     * The small print under the line: where in the archive each library is
     * looked for, and what it has to hash to.
     *
     * <p>This screen's whole claim is that it keeps a copy only if it is the
     * build these declarations were read from, and a claim nobody can check is
     * worth about as much as no claim. So the pins are on the screen making it,
     * where somebody who has unzipped their own copy of the APK and hashed what
     * was in it has something to compare that against.
     */
    private CharSequence pins() {
        final StringBuilder sb = new StringBuilder();
        for (Map.Entry<String, String> e : wanted().entrySet()) {
            if (sb.length() > 0) {
                sb.append('\n');
            }
            sb.append("lib/").append(abi()).append('/').append(e.getKey());
            // A SHA-256 is 64 characters and a phone is not that wide: broken
            // in half here rather than left to wrap wherever the font runs out.
            final String sha256 = e.getValue();
            for (int at = 0; at < sha256.length(); at += 32) {
                sb.append("\n  ").append(sha256, at, Math.min(at + 32, sha256.length()));
            }
        }
        return sb;
    }

    /** Nothing here has a build for this phone: see {@link #onCreate}. */
    private boolean nothingForThisDevice;

    /** The screen a finished route reports to, which is whichever is up. */
    private static SetupActivity onScreen;

    @Override
    protected void onResume() {
        super.onResume();
        if (nothingForThisDevice) {
            return;
        }
        if (LibraryStore.has(this, wanted())) {
            // Whatever brought us here has been done, possibly by an earlier
            // visit or by a route that finished while this screen was being
            // recreated: there is nothing left to ask for.
            setResult(RESULT_OK);
            finish();
            return;
        }
        onScreen = this;
        // A rotation does not stop what is running, so this screen is whatever
        // the process is in the middle of rather than a fresh one: the same
        // three things, drawn from the same three fields.
        working(busy);
        show(busy ? doing : failure, !busy);
        fix.setVisibility(!busy && denied ? View.VISIBLE : View.GONE);
        if (busy) {
            progress.setIndeterminate(total <= 0);
            if (total > 0) {
                progress.setMax(1000);
                progress.setProgress((int) (done * 1000 / total));
            }
        }
    }

    @Override
    protected void onPause() {
        super.onPause();
        if (onScreen == this) {
            onScreen = null;
        }
    }

    /** What an acquisition route does, off the main thread. */
    private interface Route {
        /** What is still missing afterwards; empty means done. */
        Map<String, String> run() throws Exception;
    }

    private void run(Route route) {
        busy = true;
        failure = null;
        denied = false;
        doing = null;
        done = 0;
        total = -1;
        working(true);
        WORKER.execute(() -> {
            String said = null;
            try {
                final Map<String, String> missing = route.run();
                if (!missing.isEmpty()) {
                    said = strings.getString(R.string.plugin_not_in_it);
                }
            } catch (Throwable t) {
                Log.w(TAG, "acquiring: " + t, t);
                // A message where there is one: this is where "not the build
                // this was written for" reaches somebody. Where there is not —
                // a socket giving up says nothing worth reading — the generic
                // one, since a class name on a screen helps nobody.
                final String message = t.getMessage();
                said = message == null || message.isEmpty()
                        ? strings.getString(R.string.plugin_failed) : message;
            }
            final String result = said;
            MAIN.post(() -> {
                busy = false;
                failure = result;
                doing = null;
                // Whichever screen is up, which after a rotation is not the one
                // that started this and after a kill is none.
                final SetupActivity host = onScreen;
                if (host == null) {
                    return;
                }
                host.working(false);
                if (result == null) {
                    host.setResult(RESULT_OK);
                    host.finish();
                } else {
                    host.show(result, false);
                    host.fix.setVisibility(denied ? View.VISIBLE : View.GONE);
                }
            });
        });
    }

    /**
     * How long a step is held either side of being said.
     *
     * <p>Most of these steps are quick — an archive already in the cache is
     * opened and both libraries are out of it in well under a second — and a
     * line nobody can read is not a line that says anything. Half a second
     * before and half a second after means each one is on the screen for a
     * second at least, whatever it went on to do.
     */
    private static final long LINGER = 500;

    /**
     * What the route is doing now, for the line above the bar. Called from the
     * worker, so it goes through the same slot a rotation redraws from — and it
     * blocks the worker for {@link #LINGER} either side, which is the point of
     * it and the reason nothing calls it from the main thread.
     */
    private static void say(int what, Object... args) {
        final String message = strings.getString(what, args);
        linger();
        MAIN.post(() -> {
            doing = message;
            final SetupActivity host = onScreen;
            if (host != null && busy) {
                host.show(message, true);
            }
        });
        linger();
    }

    private static void linger() {
        try {
            Thread.sleep(LINGER);
        } catch (InterruptedException e) {
            // Nothing interrupts this worker, and a route that is somehow being
            // stopped should stop rather than sleep through it again.
            Thread.currentThread().interrupt();
        }
    }

    /**
     * How far through, where the other end said how far there is to go. A
     * negative total is a server that did not say, and the bar keeps sweeping.
     */
    private static void progress(long at, long of) {
        MAIN.post(() -> {
            done = at;
            total = of;
            final SetupActivity host = onScreen;
            if (host == null || !busy) {
                return;
            }
            host.progress.setIndeterminate(of <= 0);
            if (of > 0) {
                host.progress.setMax(1000);
                host.progress.setProgress((int) (at * 1000 / of));
            }
        });
    }

    /**
     * The one line, in the one place, in whichever of its two colours: what is
     * happening reads as ordinary text and what went wrong reads as an error,
     * and there is only ever one of them to say.
     */
    private void show(String message, boolean working) {
        if (message == null || message.isEmpty()) {
            status.setVisibility(View.GONE);
            return;
        }
        status.setTextColor(working ? ordinaryColor : errorColor);
        status.setText(message);
        status.setVisibility(View.VISIBLE);
    }

    /** Both off the theme: the layout supplies the error one. */
    private int errorColor;
    private int ordinaryColor;

    private void readColors() {
        errorColor = status.getCurrentTextColor();
        final android.util.TypedValue v = new android.util.TypedValue();
        getTheme().resolveAttribute(android.R.attr.textColorSecondary, v, true);
        ordinaryColor = v.resourceId != 0 ? getColor(v.resourceId) : v.data;
    }

    private void working(boolean busy) {
        progress.setVisibility(busy ? View.VISIBLE : View.GONE);
        buttons.setVisibility(busy ? View.GONE : View.VISIBLE);
        if (busy) {
            progress.setIndeterminate(true);
        }
    }

    /**
     * The document picker, without androidx: one request code, one callback,
     * and a result that survives the activity being recreated behind it because
     * {@link #onResume} re-asks the store rather than trusting a field.
     */
    private final class ActivityLauncher {

        private static final int REQUEST = 1;

        private java.util.function.Consumer<Uri> onResult;

        void launch(Intent intent) {
            startActivityForResult(intent, REQUEST);
        }

        void onResult(java.util.function.Consumer<Uri> handler) {
            onResult = handler;
        }

        void deliver(int request, int result, Intent data) {
            if (request == REQUEST && result == RESULT_OK && data != null
                    && data.getData() != null && onResult != null) {
                onResult.accept(data.getData());
            }
        }
    }

    @Override
    protected void onActivityResult(int request, int result, Intent data) {
        super.onActivityResult(request, result, data);
        picker.deliver(request, result, data);
    }
}
