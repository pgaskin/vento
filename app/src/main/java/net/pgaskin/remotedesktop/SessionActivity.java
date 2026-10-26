// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.app.Activity;
import androidx.appcompat.app.AlertDialog;

import com.google.android.material.button.MaterialButton;
import com.google.android.material.checkbox.MaterialCheckBox;
import com.google.android.material.color.MaterialColors;
import com.google.android.material.radiobutton.MaterialRadioButton;
import android.content.Context;
import android.content.Intent;
import android.net.Uri;
import android.os.Bundle;
import android.util.Log;
import android.view.ContextThemeWrapper;
import android.view.WindowInsets;
import android.view.WindowInsetsController;
import android.view.WindowManager;
import android.window.OnBackInvokedDispatcher;
import android.widget.FrameLayout;
import android.widget.LinearLayout;
import android.widget.RadioGroup;
import android.view.View;
import android.widget.TextView;
import android.widget.Toast;

import net.pgaskin.remotedesktop.backend.Backend;
import net.pgaskin.remotedesktop.backend.Backends;
import net.pgaskin.remotedesktop.control.input.TapRegions;
import net.pgaskin.remotedesktop.session.SessionView;

import java.util.Map;

/**
 * The screen for one connection.
 *
 * <p>Usually a saved connection, opened from the home screen by id. It can
 * also be an address from the command line, as {@code scripts/session.sh}
 * does, if the developer setting {@link AppSettings#externalLaunch} allows it:
 *
 * <pre>
 *   adb shell am start -n net.pgaskin.remotedesktop/.SessionActivity \
 *       --es address 10.0.0.1:5901 [--es user NAME] [--es password PASS]
 * </pre>
 *
 * <p>This screen owns the window: the desktop, the session status over it, and
 * the way out. Connection prompts are handled by {@link PromptDialogs}, and the
 * live connection belongs to {@link Session}, which this attaches to rather
 * than owns.
 */
public final class SessionActivity extends Activity
        implements SessionView.Host, ConnectionPanel.Host, PromptDialogs.Host {

    public static final String EXTRA_CONNECTION = "connection"; // else a bare address

    private static final String TAG = "SessionActivity";

    /**
     * The intent that opens a saved connection, used by every caller: the home
     * screen, launcher shortcuts and pinned shortcuts.
     *
     * <p>One factory rather than one per caller, because this intent identifies
     * the window as well as requesting it. A session's window is a document, and
     * whether a launch reuses the open one is decided by
     * {@link Intent#filterEquals}: the component, the action <em>and</em> the
     * data together. Two callers with the same URI and different actions would
     * open the same machine in two windows, each with its own connection.
     *
     * <p>The data URI identifies the document, and the action is set because a
     * shortcut's intent must have one, so the home screen's must match.
     * {@code NEW_DOCUMENT} requests a window per session, together with
     * {@code intoExisting} in the manifest. The screen reads the extra; the URI
     * is for the system and for {@link SessionService}, which reads it from a
     * task that has been swiped away.
     */
    public static Intent intentFor(Context ctx, String connectionId) {
        return new Intent(Intent.ACTION_VIEW,
                Uri.fromParts("connection", connectionId, null), ctx, SessionActivity.class)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_DOCUMENT)
                .putExtra(EXTRA_CONNECTION, connectionId);
    }

    /**
     * The live connection, which this screen <em>attaches</em> to rather than
     * owns: opening the activity again for the same target picks up the running
     * session.
     */
    private Session session;
    private SessionView view;
    private Connection connection; // non-null for a saved connection, which gets a preview

    /** Every open dialog on this screen, and the connection's prompt dialogs. */
    private final Dialogs.Tracker dialogs = new Dialogs.Tracker();
    private PromptDialogs prompts;

    /**
     * The input preferences the view's config was built from, so that returning
     * from the settings screen can tell whether they changed. Every return from
     * another activity checks this, and re-applying unchanged settings is not
     * free: it re-clamps the viewport, which can be visible.
     */
    private Map<String, ?> inputPrefs;

    // setDecorFitsSystemWindows is deprecated because apps targeting API 35 or
    // above are always edge to edge. This app still runs on 34, where it has to
    // be requested.
    @SuppressWarnings("deprecation")
    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        getWindow().setDecorFitsSystemWindows(false);

        connection = Connections.byId(this, getIntent().getStringExtra(EXTRA_CONNECTION));
        if (connection == null && refused(getIntent())) {
            finish();
            return;
        }
        final String address = connection != null ? connection.address()
                : getIntent().getStringExtra("address");
        // A connection saved with a backend this build does not have (from an
        // add-on that has been uninstalled, for example) is an expected case, so
        // it gets a message rather than the exception Backends.create throws.
        final String backendId = backendId();
        final boolean haveBackend = Backends.ids().contains(backendId);
        if (address == null || address.isEmpty() || !haveBackend) {
            message(Backends.ids().isEmpty() ? getString(R.string.session_no_backend)
                    : !haveBackend ? getString(R.string.session_no_such_backend,
                            Backends.name(backendId))
                    : getString(R.string.session_no_address));
            return;
        }

        // A backend that is not set up offers its setup instead of connecting,
        // where its own code would otherwise throw. Only when no session is
        // running: a session that started before a plugin lost its library is
        // still usable, and this screen attaches to it.
        final Session running = Sessions.byKey(sessionKey(address));
        if (running != null && !running.isClosed()) {
            start(address, backendId);
            return;
        }
        final Intent asked = getIntent();
        Backends.isSetup(this, backendId, ready -> {
            if (getIntent() != asked) {
                recreate(); // a notification reopened this for another connection
            } else if (ready) {
                start(address, backendId);
            } else {
                setUpFirst(backendId);
            }
        });
    }

    /**
     * Set up the session window: everything from creating the backend to
     * attaching the view.
     *
     * <p>A separate method because it can run after {@code onCreate} has
     * returned (checking whether the backend is set up happens off the main
     * thread), so it also runs the two calls {@link #onStart} would have made.
     */
    private void start(String address, String backendId) {
        try {
            session = openOrAttach(address);
        } catch (Throwable t) {
            // Creating a backend runs plugin code, and a throw here would crash
            // the app. The screen shows the error, and the plugin gets a card on
            // the home screen.
            Plugins.failed(this, backendId, t);
            message(t.getMessage() == null ? String.valueOf(t) : t.getMessage());
            return;
        }
        // Report usage, which the launcher ranks its shortcut menu by. Here
        // rather than at each entry point, so every launch counts exactly once,
        // and only once there is a session: a window showing that the backend
        // is missing is not a use of the connection.
        if (connection != null) {
            Shortcuts.used(this, connection.id());
        }
        // No TaskDescription naming the machine: the launcher ignores it (the
        // recents card shows the app's name whatever the label), and the card's
        // screenshot is what tells two sessions apart.
        inputPrefs = InputSettings.prefs(this).getAll();
        view = new SessionView(this, session.backend(), this,
                InputSettings.config(this, getResources().getDisplayMetrics().density));
        // The connection's own options rather than the backend's: an app option
        // is not part of a backend's schema, so `Options.forConnection` drops
        // it. A session from the command line reads it from the same --es
        // options as every other setting.
        view.setFollowWindow(AppOptions.followWindow(
                connection != null ? connection.options() : optionsFromIntent()));
        // The setting, or --ez hud true for a session started from the command
        // line.
        view.setHudVisible(hudWanted());
        // --ei tile 512, for testing tile sizes from the command line. There is
        // no setting; other sessions use the default.
        view.setTileSize(AppSettings.externalLaunch(this)
                ? getIntent().getIntExtra("tile", 0) : 0);
        // Before the desktop arrives the window is an app screen, so it uses the
        // surface color rather than the black that letterboxes a desktop.
        view.setEmptyColor(MaterialColors.getColor(view,
                com.google.android.material.R.attr.colorSurface, 0xff000000));
        setContentView(R.layout.activity_session);
        // Below everything in the layout. Added in code because the view takes
        // its host and backend as constructor arguments.
        final FrameLayout root = findViewById(android.R.id.content);
        root.addView(view, 0, new FrameLayout.LayoutParams(
                FrameLayout.LayoutParams.MATCH_PARENT, FrameLayout.LayoutParams.MATCH_PARENT));
        bindStatusPanel();
        prompts = new PromptDialogs(this, dialogs, this, connection != null);
        session.attach(view, prompts);
        // Leaving this screen ends the connection, so back asks the same
        // question as the disconnect region. Registered only once there is a
        // session; the message screens keep the default back.
        getOnBackInvokedDispatcher().registerOnBackInvokedCallback(
                OnBackInvokedDispatcher.PRIORITY_DEFAULT, this::disconnectRequested);
        askAboutNotifications();
        if (started) {
            session.backend().focus(true);
            session.onScreen(true);
        }
    }

    /** The backend this window is waiting for setup of, or null. */
    private String awaitingSetup;

    /**
     * The window for a connection whose backend is not set up: a message, and a
     * button that starts the plugin's setup.
     *
     * <p>Nothing waits for the result. The setup screen is in another process,
     * and this one can be killed meanwhile, so success is detected by this
     * window checking again when it resumes.
     */
    private void setUpFirst(String backendId) {
        awaitingSetup = backendId;
        message(getString(R.string.session_backend_setup, Backends.name(backendId)),
                R.string.plugin_set_up, () -> Plugins.setup(this, backendId));
    }

    /** A window showing only a message. */
    private void message(CharSequence text) {
        message(text, 0, null);
    }

    /** A window showing a message and an optional action button. */
    private void message(CharSequence text, int action, Runnable onAction) {
        setContentView(R.layout.activity_session_message);
        ((TextView) findViewById(R.id.message)).setText(text);
        if (action != 0) {
            final MaterialButton button = findViewById(R.id.action);
            button.setText(action);
            button.setOnClickListener(v -> onAction.run());
            button.setVisibility(View.VISIBLE);
        }
    }

    @Override
    protected void onResume() {
        super.onResume();
        refreshSettings();
        if (awaitingSetup != null) {
            // The window was set up in onCreate, from an answer that may have
            // changed while the plugin's setup screen was open.
            Backends.isSetup(this, awaitingSetup, ready -> {
                if (ready) {
                    recreate();
                }
            });
        }
    }

    /**
     * Apply settings changed on another screen to the running session when
     * this screen returns, rather than at the next connection.
     *
     * <p>The HUD setting is applied every time, since it is a field and a
     * redraw. The input settings are applied only when they changed, since
     * applying them re-clamps the viewport, and moving a session panned into a
     * corner every time the user switches apps is worse than a stale setting.
     *
     * <p>The private-IME flag is the exception: an editor declares it when a
     * keyboard connects, so an IME already open keeps the old value until it
     * restarts.
     */
    private void refreshSettings() {
        if (view == null) {
            return;
        }
        view.setHudVisible(hudWanted());
        // An app preference, not an input one, so it is applied before the
        // input preference comparison below.
        view.applyKeyList();
        final Map<String, ?> stored = InputSettings.prefs(this).getAll();
        if (stored.equals(inputPrefs)) {
            return;
        }
        inputPrefs = stored;
        view.applySettings(InputSettings.config(this,
                getResources().getDisplayMetrics().density));
    }

    /** The HUD setting, or {@code --ez hud true} for one session. */
    private boolean hudWanted() {
        return AppSettings.hud(this) || (AppSettings.externalLaunch(this)
                && getIntent().getBooleanExtra("hud", false));
    }

    /**
     * Whether a launch with an address rather than a saved connection is
     * refused, because {@link AppSettings#externalLaunch} is off. Says so in
     * the log and in a toast, since the window closes without showing anything.
     *
     * <p>Not for a session from such a launch that is still running: the
     * notification reopens it with the same intent, and the setting is about
     * starting connections.
     */
    private boolean refused(Intent intent) {
        final String address = intent.getStringExtra("address");
        if (address == null || AppSettings.externalLaunch(this)) {
            return false;
        }
        final Session live = Sessions.byKey(Sessions.keyForAddress(address));
        if (live != null && !live.isClosed()) {
            return false;
        }
        Log.i(TAG, "not connecting to " + address + " from the launch intent:"
                + " external launches need developer mode and the setting allowing them");
        Toast.makeText(this, R.string.session_external_launch_off, Toast.LENGTH_SHORT).show();
        return true;
    }

    // ---- status panel -----------------------------------------------------

    private View statusPanel;
    private TextView statusText;
    private TextView statusLine;
    private View statusActions;
    /** The backend's status line, shown under the status while connecting. */
    private String statusLineText;
    private View scrim;
    private View loading;

    /** Find the panel's views and set up its two buttons. */
    private void bindStatusPanel() {
        scrim = findViewById(R.id.scrim);
        statusPanel = findViewById(R.id.status_panel);
        statusText = findViewById(R.id.status_text);
        statusLine = findViewById(R.id.status_line);
        statusActions = findViewById(R.id.status_actions);
        loading = findViewById(R.id.loading);
        findViewById(R.id.reconnect).setOnClickListener(v -> panelReconnect());
        findViewById(R.id.log).setOnClickListener(v -> SessionLog.show(this));
    }

    /**
     * Show the session status, computed by {@link Session.Status}: the panel
     * text, the actions and the spinner map to its three fields.
     */
    @Override
    public void status(Backend.State state, String detail) {
        if (statusPanel == null) {
            return;
        }
        final Session.Status s = Session.Status.of(this, state, detail);
        final int panel = s.text().isEmpty() ? View.GONE : View.VISIBLE;
        statusText.setText(s.text());
        // Only while connecting: an ended session's message is the reason it
        // ended, and the wait it was in no longer matters.
        if ((s.ended() || !s.working()) && statusLineText != null) {
            statusLineText = null;
            prompts.status(null);
        }
        showStatusLine();
        statusPanel.setVisibility(panel);
        scrim.setVisibility(panel);
        // Only once the session has ended. A session still connecting has
        // nothing to reconnect and no log to show yet.
        statusActions.setVisibility(s.ended() ? View.VISIBLE : View.GONE);
        loading.setVisibility(s.working() ? View.VISIBLE : View.GONE);
    }

    @Override
    public void statusLine(String text) {
        statusLineText = text == null || text.isEmpty() ? null : text;
        showStatusLine();
        // The prompt covers the panel, so it shows the line as well.
        if (prompts != null) {
            prompts.status(statusLineText);
        }
    }

    private void showStatusLine() {
        if (statusLine == null) {
            return;
        }
        statusLine.setText(statusLineText);
        statusLine.setVisibility(statusLineText != null ? View.VISIBLE : View.GONE);
    }

    /** The session key, so a second screen for the same target re-attaches. */
    private String sessionKey(String address) {
        return connection != null ? Sessions.keyFor(connection.id())
                : Sessions.keyForAddress(address);
    }

    private Session openOrAttach(String address) {
        final Session live = Sessions.byKey(sessionKey(address));
        if (live != null && !live.isClosed()) {
            return live;
        }
        // A saved connection's options are layered over its backend's; an
        // address from the command line gets the backend's settings plus the
        // options extra. The application context, because the backend outlives
        // this activity.
        final Map<String, String> options = connection != null
                ? Options.forConnection(this, connection) : optionsFromIntent();
        final Backend backend = Backends.create(getApplicationContext(), backendId(), address,
                connection != null ? connection.userName() : getIntent().getStringExtra("user"),
                connection != null ? connection.password()
                        : getIntent().getStringExtra("password"),
                options);
        return Session.open(this, sessionKey(address),
                connection != null && !connection.name().isEmpty() ? connection.name() : address,
                address, new Intent(getIntent()), backend, options);
    }

    /**
     * Tapping the notification arrives here when the activity is already open.
     * Only a different connection needs handling: the screen is recreated for
     * the session it is not attached to.
     */
    @Override
    protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        final Connection c = Connections.byId(this, intent.getStringExtra(EXTRA_CONNECTION));
        if (c == null && refused(intent)) {
            return; // this window keeps its own intent and session
        }
        setIntent(intent);
        final String address = c != null ? c.address() : intent.getStringExtra("address");
        if (session != null && address != null && !session.matches(
                c != null ? Sessions.keyFor(c.id()) : Sessions.keyForAddress(address))) {
            recreate();
        }
    }

    /**
     * The notification belongs to the service that keeps the connection alive,
     * so the permission is requested once. If denied, the service still runs;
     * only seeing or stopping it from the notification shade is lost.
     */
    private void askAboutNotifications() {
        if (checkSelfPermission(android.Manifest.permission.POST_NOTIFICATIONS)
                != android.content.pm.PackageManager.PERMISSION_GRANTED) {
            requestPermissions(new String[]{android.Manifest.permission.POST_NOTIFICATIONS}, 1);
        }
    }

    /**
     * Only focus gains are sent from here ({@link #onStop} sends the loss). They
     * are sent here as well as from {@link #onStart} because of the clipboard:
     * the core responds to a focus gain by requesting the local clipboard, the
     * answer comes from a cache only the main thread fills
     * ({@code SessionClipboard}), and Android lets only a focused window read
     * the clipboard. So the cache is refreshed here, before the backend gets
     * the focus. Repeating it on every focus gain is what picks up a copy made
     * in another app.
     */
    @Override
    public void onWindowFocusChanged(boolean hasFocus) {
        super.onWindowFocusChanged(hasFocus);
        if (!hasFocus) {
            return;
        }
        final WindowInsetsController bars = getWindow().getInsetsController();
        if (bars != null) {
            if (AppSettings.immersive(this)) {
                // Sticky immersive: a swipe shows the bars briefly and they hide
                // again on their own, so a gesture at the edge of the desktop
                // does not leave them shown.
                bars.setSystemBarsBehavior(
                        WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE);
                bars.hide(WindowInsets.Type.systemBars());
            } else {
                // For the setting being turned off during a session: this
                // window hid the bars, so it has to show them again.
                bars.show(WindowInsets.Type.systemBars());
            }
        }
        if (view != null) {
            view.refreshClipboard();
        }
        if (session != null) {
            session.backend().focus(true);
        }
    }

    /** Whether this window is visible, for a session that starts after it is. */
    private boolean started;

    @Override
    protected void onStart() {
        super.onStart();
        started = true;
        // Here rather than in onCreate, so that changing the setting applies to
        // the running session, as the immersive setting does in
        // onWindowFocusChanged.
        if (AppSettings.keepAwake(this)) {
            getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        } else {
            getWindow().clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        }
        // The session is paused while not visible: resumed here, paused in
        // onStop. The backend does the pausing: the RealVNC core stops
        // requesting framebuffer updates, an RFB server sends nothing
        // unrequested, and RDP is told to suppress output. Visibility rather
        // than window focus, because a dialog over the session takes the focus
        // without hiding it.
        // The focus gain is sent here as well as in onWindowFocusChanged for
        // split screen, where this window is visible and interactive while the
        // other app has the focus. Sending it twice costs nothing, since the
        // core compares the flag.
        if (session != null) {
            session.backend().focus(true);
            // The same signal drives the session timeout.
            session.onScreen(true);
        }
        if (panel != null && panel.isShowing()) {
            panel.resume();
        }
    }

    @Override
    protected void onStop() {
        super.onStop();
        started = false;
        if (view != null) {
            view.suspendInput();
        }
        if (session != null) {
            // After suspendInput, which releases each key it holds by id. This
            // releases any key the far end still has down that this side no
            // longer tracks.
            if (AppSettings.releaseKeys(this)) {
                session.backend().releaseAllKeys();
            }
            session.backend().focus(false);
            session.onScreen(false);
        }
        // The panel polls the connection once a second while it is open; stop
        // polling while the screen is not visible.
        if (panel != null) {
            panel.pause();
        }
    }

    /**
     * Save the home card preview: the desktop as it last looked, captured when
     * leaving rather than on a timer, since a preview is a reminder rather than
     * a live view. The backend downscales it, in whole integer steps for
     * RealVNC, whose scaler supports nothing else.
     */
    @Override
    protected void onPause() {
        super.onPause();
        savePreview();
    }

    /**
     * Capture the desktop for the home card, if there is one. Called from two
     * places: {@link #onPause} covers a session that is left running, and the
     * disconnect covers one being ended, which is CLOSED with no framebuffer
     * by the time onPause runs.
     */
    private void savePreview() {
        if (connection != null && session != null && !session.isClosed()
                && session.backend().state() == Backend.State.CONNECTED) {
            Connections.saveThumbnail(this, connection.id(),
                    session.backend().thumbnail(THUMBNAIL_MAX, THUMBNAIL_MAX));
        }
    }

    private static final int THUMBNAIL_MAX = 640; // a tablet card, quick to decode on scroll

    /**
     * The screen closes but the session stays. Only what this activity created
     * is released: the mirror bitmap, and the session's reference to the view.
     */
    @Override
    protected void onDestroy() {
        super.onDestroy();
        // A dialog still open when its activity is destroyed leaks its window.
        if (panel != null) {
            panel.dismiss();
            panel = null;
        }
        dialogs.dismissAll();
        if (session != null && view != null) {
            session.detach(view);
        }
        if (view != null) {
            view.release();
        }
        // A session that ended on its own (a refused password, an unreachable
        // address) stays registered so the screen can keep showing why it
        // stopped. Once the screen is finishing, the next screen for the same
        // target would re-attach and show "Disconnected" instead of connecting.
        // This is not a rule in Session.matches because of rotation: a
        // recreated activity needs the ended session, so a failure survives a
        // rotation rather than silently reconnecting.
        if (isFinishing() && session != null && session.state() == Backend.State.CLOSED) {
            session.close();
            session = null;
        }
    }


    // ---- SessionView.Host ---------------------------------------------------

    /**
     * Both ways out of a session, the tap region and the back gesture, share
     * one dialog however many times either is used: back is easy to repeat, and
     * stacked copies of the dialog would each need answering.
     */
    @Override
    public void disconnectRequested() {
        if (leaving != null && leaving.isShowing()) {
            return;
        }
        // Nothing to disconnect: the connection has ended or never started, so
        // there is nothing to confirm.
        if (session == null || session.isClosed()
                || session.backend().state() == Backend.State.CLOSED) {
            leave();
            return;
        }
        leaving = dialogs.track(Dialogs.builder(this)
                .setMessage(getString(R.string.session_disconnect_confirm))
                .setNegativeButton(android.R.string.cancel, null)
                // Opens the connection list in its own task, leaving this window
                // and its session running. A card whose session is running
                // returns to its window, so this opens a second connection
                // rather than leaving this one.
                .setNeutralButton(R.string.session_new_window, (d, w) ->
                        startActivity(new Intent(this, HomeActivity.class)
                                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK
                                        | Intent.FLAG_ACTIVITY_CLEAR_TOP
                                        | Intent.FLAG_ACTIVITY_SINGLE_TOP)))
                .setPositiveButton(R.string.session_disconnect, (d, w) -> {
                    // Before the disconnect: onPause normally takes the
                    // preview, but by the time it runs this session is CLOSED
                    // and its framebuffer is gone.
                    savePreview();
                    session.disconnect();
                    leave();
                })
                .show());
    }

    /**
     * Close this window and show the connection list.
     *
     * <p>A session is its own document task ({@code documentLaunchMode}), so
     * finishing it returns to wherever that task was opened from: the launcher,
     * if the session was opened from a notification or the home screen's task
     * has since closed. Leaving a desktop should stay in this app, and the list
     * also leads to the other sessions, since a card whose session is running
     * returns to its window.
     */
    private void leave() {
        // CLEAR_TOP with SINGLE_TOP rather than a plain start: the home screen
        // is usually already the root of its own task, and this brings that task
        // forward instead of creating a second copy.
        startActivity(new Intent(this, HomeActivity.class)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK
                        | Intent.FLAG_ACTIVITY_CLEAR_TOP
                        | Intent.FLAG_ACTIVITY_SINGLE_TOP));
        finish();
    }

    private AlertDialog leaving;

    /**
     * Show the connection panel: the session's information and the options
     * that can be changed while connected, in one sheet over the live desktop
     * ({@link ConnectionPanel}).
     */
    @Override
    public void informationRequested() {
        if (panel != null && panel.isShowing()) {
            return;
        }
        panel = ConnectionPanel.show(this, session, backendId(), this);
    }

    private ConnectionPanel panel;

    /**
     * The sound switch is on the panel, and the session reports when it is
     * offered, withdrawn or toggled (including by the far end, as when a
     * channel fails to open), so an open panel updates at once rather than at
     * its next poll a second later.
     */
    @Override
    public void soundChanged() {
        if (panel != null && panel.isShowing()) {
            panel.refreshSound();
        }
    }

    /**
     * The backend for this session: the connection's, the one named on the
     * command line, or the first this build has. The extra makes it easy to
     * test the same server with two backends from the command line.
     */
    private String backendId() {
        if (connection != null && !connection.backendId().isEmpty()) {
            return connection.backendId();
        }
        final String named = getIntent().getStringExtra("backend");
        if (named != null && Backends.ids().contains(named)) {
            return named;
        }
        // A build with no backends is possible, and this is called before that
        // case has been reported.
        return Backends.ids().isEmpty() ? "" : Backends.ids().get(0);
    }

    /**
     * Backend options from the command line, {@code Key=value,Key=value}, for
     * the same reason as the backend extra: testing a list of encodings through
     * the editor would mean tapping through each one. Saved connections use
     * their own options.
     *
     * <p>Layered over the backend's settings, so every option resolves the same
     * way as for a saved connection, with the command line taking the place of
     * the connection's own options.
     */
    private Map<String, String> optionsFromIntent() {
        final Map<String, String> out = Options.forBackend(this, backendId());
        final String spec = getIntent().getStringExtra("options");
        if (spec == null || spec.isEmpty()) {
            return out;
        }
        for (String pair : spec.split(",")) {
            final int eq = pair.indexOf('=');
            if (eq > 0) {
                out.put(pair.substring(0, eq).trim(), pair.substring(eq + 1).trim());
            }
        }
        return out;
    }

    // ---- ConnectionPanel.Host -----------------------------------------------

    /** A live option changed on the panel; the screen shows one of them. */
    @Override
    public void panelOptionChanged() {
        if (view != null) {
            view.optionsChanged();
        }
    }

    /**
     * Reconnect. The session has ended and its backend is freed, so the screen
     * is recreated from the same intent, and {@link #openOrAttach} finds no live
     * session and opens a new one. The old one is closed first because a
     * session that ended on its own stays registered until it is closed.
     */
    @Override
    public void panelReconnect() {
        if (session != null) {
            if (view != null) {
                session.detach(view);
            }
            session.close();
            session = null;
        }
        recreate();
    }

    /** The panel's disconnect button, which goes through the same dialog. */
    @Override
    public void panelDisconnect() {
        disconnectRequested();
    }

    // ---- PromptDialogs.Host -------------------------------------------------

    private String pendingUser;
    private String pendingPassword;

    @Override
    public void rememberCredentials(String userName, String password) {
        pendingUser = userName;
        pendingPassword = password;
    }

    /** @see PromptDialogs.Host#promptDeclined() */
    private boolean declined;

    @Override
    public void promptDeclined() {
        declined = true;
    }

    @Override
    public void disconnected() {
        // Declined at a prompt, or disconnected by the user from the region,
        // the back gesture, or the notification's button.
        if (declined || (session != null && session.leaving())) {
            leave();
        }
    }

    // ---- the paste key -----------------------------------------------------

    @Override
    public void confirmPaste(int characters, String heldModifiers, Runnable proceed) {
        final StringBuilder msg = new StringBuilder();
        if (!heldModifiers.isEmpty()) {
            msg.append(getString(R.string.paste_modifiers, heldModifiers)).append("\n\n");
        }
        msg.append(getResources().getQuantityString(R.plurals.paste_confirm,
                characters, characters));
        dialogs.track(Dialogs.builder(this)
                .setTitle(R.string.paste_title)
                .setMessage(msg.toString())
                .setNegativeButton(android.R.string.cancel, null)
                .setPositiveButton(R.string.paste_confirm_yes, (d, w) -> proceed.run())
                .show());
    }

    @Override
    public void nothingToPaste() {
        Toast.makeText(this, R.string.paste_empty, Toast.LENGTH_SHORT).show();
    }

    // ---- controls hint ----------------------------------------------------

    /**
     * Explain the controls once, on the first frame: the choice between the
     * toolbar and the four tap regions, which nothing on screen otherwise
     * reveals.
     *
     * <p>The dialog does not dim the screen behind it, because the toolbar and
     * the highlighted bands it explains are underneath.
     */
    @Override
    public void firstFrame() {
        if (view == null || !AppSettings.regionHints(this)) {
            return;
        }
        final MaterialCheckBox never = Dialogs.checkBox(this, R.string.hints_dismiss);
        final LinearLayout box = new LinearLayout(this);
        box.setOrientation(LinearLayout.VERTICAL);
        box.setPadding(Dialogs.padding(this), Dialogs.dp(this, 8), Dialogs.padding(this), 0);

        // The choice is previewed behind the dialog as it is made, which is
        // easier to judge than a description.
        final RadioGroup choices = new RadioGroup(this);
        final String[] values = {AppSettings.CONTROLS_TOOLBAR, AppSettings.CONTROLS_REGIONS,
                AppSettings.CONTROLS_BOTH};
        final int[] labels = {R.string.controls_toolbar, R.string.controls_regions,
                R.string.controls_both};
        final String chosen = AppSettings.controls(this);
        for (int i = 0; i < values.length; i++) {
            final MaterialRadioButton b = new MaterialRadioButton(
                    new ContextThemeWrapper(this, R.style.ThemeOverlay_RemoteDesktop_Dialog));
            b.setId(i + 1);
            b.setText(labels[i]);
            b.setChecked(values[i].equals(chosen));
            choices.addView(b);
        }
        choices.setOnCheckedChangeListener((g, id) -> {
            AppSettings.setControls(this, values[id - 1]);
            previewControls();
        });
        box.addView(choices);
        box.addView(never);
        previewControls();

        final AlertDialog dialog = Dialogs.builder(this)
                .setTitle(R.string.hints_title)
                .setMessage(R.string.hints_message)
                .setView(box)
                .setPositiveButton(android.R.string.ok, null)
                .create();
        final android.view.Window w = dialog.getWindow();
        if (w != null) {
            w.clearFlags(WindowManager.LayoutParams.FLAG_DIM_BEHIND);
        }
        // Through the tracker, not on the builder: the tracker sets the
        // dialog's dismiss listener, and a dialog has only one, so a listener
        // set on the builder was replaced (which left the bands lit for the rest
        // of the session and "Do not show this again" without effect).
        // The choice is saved as it is made, whether or not the box is ticked:
        // the checkbox only controls whether the dialog is shown again.
        dialogs.track(dialog, d -> {
            if (never.isChecked()) {
                AppSettings.setRegionHints(this, false);
            }
            if (view != null) {
                view.setRegionHints(null);
            }
        });
        dialog.show();
    }

    /**
     * Preview the current choice with the dialog still open: the toolbar appears
     * at its saved position, and the bands are highlighted or cleared.
     */
    private void previewControls() {
        if (view == null) {
            return;
        }
        view.applyControls();
        view.setRegionHints(AppSettings.regionsShown(this) ? Map.of(
                TapRegions.DISCONNECT, getString(R.string.hints_region_disconnect),
                TapRegions.INFORMATION, getString(R.string.hints_region_information),
                TapRegions.KEYBOARD, getString(R.string.hints_region_keyboard),
                TapRegions.MOUSE, getString(R.string.hints_region_mouse)) : null);
    }

    /**
     * Save credentials entered at a prompt with "remember" ticked, only once the
     * far end has accepted them.
     */
    @Override
    public void connected() {
        if (connection == null || pendingPassword == null) {
            return;
        }
        connection = connection.withCredentials(
                pendingUser != null && !pendingUser.isEmpty() ? pendingUser
                        : connection.userName(),
                pendingPassword);
        Connections.save(this, connection);
        pendingUser = null;
        pendingPassword = null;
    }
}
