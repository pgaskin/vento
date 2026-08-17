// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.app.Activity;
import androidx.appcompat.app.AlertDialog;

import com.google.android.material.button.MaterialButton;
import com.google.android.material.checkbox.MaterialCheckBox;
import com.google.android.material.color.MaterialColors;
import com.google.android.material.dialog.MaterialAlertDialogBuilder;
import com.google.android.material.loadingindicator.LoadingIndicator;
import com.google.android.material.textfield.TextInputEditText;
import com.google.android.material.textfield.TextInputLayout;
import android.content.Context;
import android.content.DialogInterface;
import android.content.res.TypedArray;
import android.content.Intent;
import android.net.Uri;
import android.os.Bundle;
import android.text.InputType;
import android.util.TypedValue;
import android.view.ContextThemeWrapper;
import android.view.WindowInsets;
import android.view.WindowInsetsController;
import android.view.WindowManager;
import android.window.OnBackInvokedDispatcher;
import android.view.ViewGroup;
import android.widget.EditText;
import android.widget.FrameLayout;
import android.widget.LinearLayout;
import android.view.View;
import android.widget.TextView;
import android.widget.Toast;

import net.pgaskin.remotedesktop.backend.Backend;
import net.pgaskin.remotedesktop.backend.Backends;
import net.pgaskin.remotedesktop.backend.Prompt;
import net.pgaskin.remotedesktop.control.input.TapRegions;

import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * One connection, on screen.
 *
 * <p>Normally that is a saved one, opened from the home screen by id. It can
 * also be an address straight from the command line, which is what
 * {@code scripts/session.sh} uses:
 *
 * <pre>
 *   adb shell am start -n net.pgaskin.remotedesktop/.SessionActivity \
 *       --es address 10.0.0.1:5901 [--es user NAME] [--es password PASS]
 * </pre>
 *
 * <p>Prompts are dialogs over the session rather than screens of their own:
 * the connection is stalled behind each one, so putting it behind a navigation
 * step would be pretending otherwise.
 */
public final class SessionActivity extends Activity
        implements SessionView.Host, ConnectionPanel.Host, Prompt.Handler {

    public static final String EXTRA_CONNECTION = "connection"; // else a bare address

    /**
     * The way in to one saved connection, wherever it is asked for: the home
     * screen, a launcher shortcut, an icon on a home screen.
     *
     * <p>One factory rather than one per caller, because this intent is an
     * identity as well as a request. A session's window is a document, and what
     * decides whether a launch lands in the one that is already open is
     * {@link Intent#filterEquals} — the component, the action <em>and</em> the
     * data together. Two callers that agreed about the URI and differed about
     * the action would open the same machine in two windows, each with a live
     * connection to it.
     *
     * <p>So: the data URI is what names the document, and the action is here
     * because a shortcut's intent must have one and the home screen must then
     * have the same. {@code NEW_DOCUMENT} is what asks for a window per session
     * — {@code intoExisting} in the manifest is the other half of that rule.
     * The extra is what the screen actually reads; the URI is for the system
     * and for {@link SessionService}, which reads it back off a task that has
     * been swiped away.
     */
    public static Intent intentFor(Context ctx, String connectionId) {
        return new Intent(Intent.ACTION_VIEW,
                Uri.fromParts("connection", connectionId, null), ctx, SessionActivity.class)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_DOCUMENT)
                .putExtra(EXTRA_CONNECTION, connectionId);
    }

    /**
     * The live connection, which this screen <em>attaches</em> to rather than
     * owns: opening the activity again for the same thing picks up the session
     * that is already running.
     */
    private Session session;
    private SessionView view;
    private Connection connection; // non-null from the store, so it can leave a preview

    /**
     * What was stored for the input stack when the view's config was built from
     * it, so that coming back from the settings screen can tell a file somebody
     * edited from one they only looked at. Every trip through another activity
     * ends here, and re-applying settings that have not moved is not free: it
     * re-clamps the viewport, which is a thing the eye can catch.
     */
    private Map<String, ?> inputPrefs;

    // setDecorFitsSystemWindows is deprecated because a build targeting API 35
    // or above is edge to edge whether it asks or not. This one still runs on
    // 34, where it has to ask.
    @SuppressWarnings("deprecation")
    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        getWindow().setDecorFitsSystemWindows(false);

        connection = Connections.byId(this, getIntent().getStringExtra(EXTRA_CONNECTION));
        final String address = connection != null ? connection.address()
                : getIntent().getStringExtra("address");
        // A connection saved with a backend this build does not have is the
        // ordinary consequence of the flavour split, so it is a sentence rather
        // than the exception Backends.create throws.
        final String backendId = backendId();
        final boolean haveBackend = Backends.ids().contains(backendId);
        if (address == null || address.isEmpty() || !haveBackend) {
            final TextView tv = new TextView(this);
            if (Backends.ids().isEmpty()) {
                tv.setText(R.string.session_no_backend);
            } else if (!haveBackend) {
                tv.setText(getString(R.string.session_no_such_backend,
                        Backends.name(backendId)));
            } else {
                tv.setText(R.string.session_no_address);
            }
            tv.setPadding(48, 96, 48, 48);
            setContentView(tv);
            return;
        }

        // A backend waiting for something it has to be given offers that
        // instead of connecting, which is where its own code would otherwise
        // throw on a screen that is already up. Only where there is nothing
        // running: a session that came up before a plugin lost its copy is
        // still a session, and this screen attaches to it.
        final Session running = Sessions.byKey(sessionKey(address));
        if (running != null && !running.isClosed()) {
            start(address, backendId);
            return;
        }
        final Intent asked = getIntent();
        Backends.isSetup(this, backendId, ready -> {
            if (getIntent() != asked) {
                recreate(); // the notification landed here for another connection
            } else if (ready) {
                start(address, backendId);
            } else {
                setUpFirst(backendId);
            }
        });
    }

    /**
     * The window this screen is when there is a session to have: everything
     * from creating the backend to attaching the view.
     *
     * <p>Its own method because it can happen after {@code onCreate} has
     * returned — the question of whether the backend is ready is not one for
     * the main thread — and so the two lines {@link #onStart} would have run
     * are here as well.
     */
    private void start(String address, String backendId) {
        try {
            session = openOrAttach(address);
        } catch (Throwable t) {
            // Creating a backend is running somebody else's constructor, and a
            // window that dies here takes the app with it. The screen says what
            // happened; the plugin it came from grows a card.
            Plugins.failed(this, backendId, t);
            final TextView tv = new TextView(this);
            tv.setText(t.getMessage() == null ? String.valueOf(t) : t.getMessage());
            tv.setPadding(48, 96, 48, 48);
            setContentView(tv);
            return;
        }
        // Which machine this phone is actually used to connect to, which is
        // what the launcher ranks its short menu by. Here rather than at each
        // way in, so that every one of them counts and none counts twice, and
        // only once there is a session: a window that came up to say there is
        // no backend for this connection is not somebody using it.
        if (connection != null) {
            Shortcuts.used(this, connection.id());
        }
        // No TaskDescription naming the machine: a window per session makes that
        // the obvious thing to want, and the launcher does not offer it — the
        // recents card is headed with the app's name whatever a label says, and
        // what tells two sessions apart there is the picture on the card.
        inputPrefs = InputSettings.prefs(this).getAll();
        view = new SessionView(this, session.backend(), this,
                InputSettings.config(this, getResources().getDisplayMetrics().density));
        // The connection's own map rather than what the backend was created
        // with: an app option is not part of a backend's schema and never
        // reaches one, so `effectiveOptions` has already dropped it. A session
        // off the command line reads it out of the same --es options it reads
        // every other setting from.
        view.setFollowWindow(AppOptions.followWindow(
                connection != null ? connection.options() : optionsFromIntent()));
        // The switch is in the settings tree now; --ez hud true still forces it
        // on for a session started from the command line.
        view.setHudVisible(hudWanted());
        // --ei tile 512, for the sweep the tile size was chosen from. No
        // setting: the default is the answer, and a session started any other
        // way gets it.
        view.setTileSize(getIntent().getIntExtra("tile", 0));
        // The desktop, and one panel over it. Nothing in the panel is clickable
        // except its buttons, and a view that does not take a touch does not
        // stop the one under it getting it — so the touch surface is still the
        // whole screen.
        // An empty window during a connection is a screen of this app, not the
        // edges of a picture, so it is the app's own surface rather than the
        // black that letterboxes a desktop once one has arrived.
        view.setEmptyColor(MaterialColors.getColor(view,
                com.google.android.material.R.attr.colorSurface, 0xff000000));
        final FrameLayout root = new FrameLayout(this);
        root.addView(view, new FrameLayout.LayoutParams(
                FrameLayout.LayoutParams.MATCH_PARENT, FrameLayout.LayoutParams.MATCH_PARENT));
        root.addView(buildScrim());
        root.addView(buildStatusPanel());
        setContentView(root);
        session.attach(view, this);
        // This screen is the session, so leaving it ends the connection and back
        // asks what the disconnect region asks. Registered only once there is a
        // session to lose — the no-address screen keeps a plain back.
        getOnBackInvokedDispatcher().registerOnBackInvokedCallback(
                OnBackInvokedDispatcher.PRIORITY_DEFAULT, this::disconnectRequested);
        askAboutNotifications();
        if (started) {
            session.backend().focus(true);
            session.onScreen(true);
        }
    }

    /** The backend this window is waiting to have set up, while it is waiting. */
    private String awaitingSetup;

    /**
     * The whole of the window for a connection that cannot be made yet: what it
     * is waiting for, and the button that asks the plugin for it.
     *
     * <p>Nothing is waited on. The setup screen is another process's and can be
     * killed behind a dialog, so what reports success is this window resuming
     * and asking again.
     */
    private void setUpFirst(String backendId) {
        awaitingSetup = backendId;
        final LinearLayout box = new LinearLayout(this);
        box.setOrientation(LinearLayout.VERTICAL);
        box.setPadding(48, 96, 48, 48);
        final TextView tv = new TextView(this);
        tv.setText(getString(R.string.session_backend_setup, Backends.name(backendId)));
        box.addView(tv, new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT));
        box.addView(quietButton(R.string.plugin_set_up, () -> Plugins.setup(this, backendId)),
                new LinearLayout.LayoutParams(
                        LinearLayout.LayoutParams.WRAP_CONTENT,
                        LinearLayout.LayoutParams.WRAP_CONTENT));
        setContentView(box);
    }

    @Override
    protected void onResume() {
        super.onResume();
        refreshSettings();
        if (awaitingSetup != null) {
            // Everything this window is was decided in onCreate, against an
            // answer that may have changed while the plugin's own screen was up.
            Backends.isSetup(this, awaitingSetup, ready -> {
                if (ready) {
                    recreate();
                }
            });
        }
    }

    /**
     * Settings are changed on screens of their own, so a session that outlives
     * a trip to one has to be told on the way back rather than at the next
     * connection — this is the whole of "somewhere else" for a screen that is
     * only ever left and returned to.
     *
     * <p>The readout is set every time because it is a field and a redraw. The
     * input stack is set only when the file behind it has actually moved, since
     * applying it re-clamps the viewport, and a session panned into a corner
     * being nudged out of it every time the app is switched away from and back
     * is a worse thing than a stale setting.
     *
     * <p>What still waits for the next session is the private-IME flag, which
     * an editor declares when a keyboard connects to it: an IME already up when
     * it changed keeps the old one until it starts again.
     */
    private void refreshSettings() {
        if (view == null) {
            return;
        }
        view.setHudVisible(hudWanted());
        // App preferences rather than the input stack's, so above the comparison
        // that follows: that one is about one file, and this is in another.
        view.applyKeyList();
        final Map<String, ?> stored = InputSettings.prefs(this).getAll();
        if (stored.equals(inputPrefs)) {
            return;
        }
        inputPrefs = stored;
        view.applySettings(InputSettings.config(this,
                getResources().getDisplayMetrics().density));
    }

    /** The switch in the settings tree, or {@code --ez hud true} for one session. */
    private boolean hudWanted() {
        return AppSettings.hud(this) || getIntent().getBooleanExtra("hud", false);
    }

    // ---- what the session has to say for itself ----------------------------

    private LinearLayout statusPanel;
    private TextView statusText;
    private View statusActions;
    private View scrim;
    private LoadingIndicator loading;

    /**
     * The desktop, held back while there is nothing to be doing with it.
     *
     * <p>Surface rather than the black a dialog dims with, because this is not a
     * modal over the session — it *is* the session, in a state where touching it
     * does nothing. Heavier than that scrim for the same reason: a bottom sheet
     * covers the desktop for a moment and this covers it until the connection
     * comes back, so the words over it have to win rather than compete. Not
     * clickable, so it takes no touch the desktop under it would have had.
     */
    private View buildScrim() {
        scrim = new View(this);
        scrim.setBackgroundColor(MaterialColors.compositeARGBWithAlpha(
                MaterialColors.getColor(this, com.google.android.material.R.attr.colorSurface,
                        0xff000000),
                (int) (0.72f * 255)));
        scrim.setVisibility(View.GONE);
        scrim.setLayoutParams(new FrameLayout.LayoutParams(
                FrameLayout.LayoutParams.MATCH_PARENT, FrameLayout.LayoutParams.MATCH_PARENT));
        return scrim;
    }

    /**
     * The message a session ends with, and two things to do about it.
     *
     * <p>It used to be ink on the desktop, which was enough while it only ever
     * said "Connecting…" or "Disconnected". A connection that fails says a
     * sentence instead, and the two useful answers to a sentence — try again,
     * and read the log the sentence is the last line of — were reachable only
     * through a tap region that nothing pointed at. They are buttons now,
     * quiet ones, under the words they are about.
     */
    private View buildStatusPanel() {
        statusPanel = new LinearLayout(this);
        statusPanel.setOrientation(LinearLayout.VERTICAL);
        statusPanel.setGravity(android.view.Gravity.CENTER_HORIZONTAL);
        statusPanel.setVisibility(View.GONE);
        final android.graphics.drawable.GradientDrawable bg =
                new android.graphics.drawable.GradientDrawable();
        bg.setColor(0xcc101418);
        bg.setCornerRadius(dp(6));
        statusPanel.setBackground(bg);
        statusPanel.setPadding(dp(20), dp(14), dp(20), dp(8));

        // Above the words rather than beside them, because the panel is centred
        // on a screen with nothing else on it and a sentence that grows to two
        // lines would drag a leading spinner off centre with it.
        loading = new LoadingIndicator(new ContextThemeWrapper(this,
                com.google.android.material.R.style.Widget_Material3_LoadingIndicator));
        final LinearLayout.LayoutParams llp = new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.WRAP_CONTENT,
                LinearLayout.LayoutParams.WRAP_CONTENT);
        llp.gravity = android.view.Gravity.CENTER_HORIZONTAL;
        llp.bottomMargin = dp(10);
        statusPanel.addView(loading, llp);

        statusText = new TextView(this);
        statusText.setTextColor(0xffe8f0ff);
        statusText.setTextSize(TypedValue.COMPLEX_UNIT_DIP, 14);
        statusText.setGravity(android.view.Gravity.CENTER_HORIZONTAL);
        statusText.setMaxWidth(
                getResources().getDisplayMetrics().widthPixels - dp(24 + 20) * 2);
        // Both of these on purpose. A vertical LinearLayout gives a child it is
        // not given parameters for MATCH_PARENT width, and a MATCH_PARENT child
        // of a WRAP_CONTENT parent is measured at the space available and then
        // laid out at whatever the other children made the parent — so the
        // sentence wrapped into more lines than it had been measured for and
        // lost its last one off the bottom.
        statusPanel.addView(statusText, new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.WRAP_CONTENT,
                LinearLayout.LayoutParams.WRAP_CONTENT));

        final LinearLayout actions = new LinearLayout(this);
        actions.setOrientation(LinearLayout.HORIZONTAL);
        actions.setGravity(android.view.Gravity.CENTER_HORIZONTAL);
        actions.addView(quietButton(R.string.panel_reconnect, this::panelReconnect));
        actions.addView(quietButton(R.string.panel_log, () -> SessionLog.show(this)));
        final LinearLayout.LayoutParams alp = new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.WRAP_CONTENT,
                LinearLayout.LayoutParams.WRAP_CONTENT);
        // The panel is as wide as the sentence, which is wider than the two
        // buttons; without this they sit under its first word.
        alp.gravity = android.view.Gravity.CENTER_HORIZONTAL;
        statusPanel.addView(actions, alp);
        statusActions = actions;

        final FrameLayout.LayoutParams lp = new FrameLayout.LayoutParams(
                FrameLayout.LayoutParams.WRAP_CONTENT, FrameLayout.LayoutParams.WRAP_CONTENT,
                android.view.Gravity.CENTER);
        lp.leftMargin = dp(24);
        lp.rightMargin = dp(24);
        statusPanel.setLayoutParams(lp);
        return statusPanel;
    }

    /** No outline: it is a thing to tap, not a second panel inside the first. */
    private MaterialButton quietButton(int text, Runnable action) {
        final MaterialButton b = new MaterialButton(this, null,
                androidx.appcompat.R.attr.borderlessButtonStyle);
        b.setText(text);
        b.setOnClickListener(v -> action.run());
        return b;
    }

    @Override
    public void status(String text, boolean ended) {
        if (statusPanel == null) {
            return;
        }
        statusText.setText(text);
        statusPanel.setVisibility(text.isEmpty() ? View.GONE : View.VISIBLE);
        // Only where there is something to act on. A session still connecting
        // has nothing to reconnect and nothing yet to explain.
        statusActions.setVisibility(ended ? View.VISIBLE : View.GONE);
        // The three states this screen has are exactly what these two lines
        // read off one string and one flag: connected says nothing and shows
        // the desktop, working says something and turns, ended says something
        // and stops.
        loading.setVisibility(!ended && !text.isEmpty() ? View.VISIBLE : View.GONE);
        scrim.setVisibility(text.isEmpty() ? View.GONE : View.VISIBLE);
    }

    /** What a session is "of", so a second screen for it re-attaches. */
    private String sessionKey(String address) {
        return connection != null ? Sessions.keyFor(connection.id())
                : Sessions.keyForAddress(address);
    }

    private Session openOrAttach(String address) {
        final Session live = Sessions.byKey(sessionKey(address));
        if (live != null && !live.isClosed()) {
            return live;
        }
        // A saved connection carries its own options, layered over its backend's;
        // an address from the command line gets the defaults. The application
        // context, because the backend outlives this activity by design.
        final Map<String, String> options = connection != null
                ? Connections.effectiveOptions(this, connection) : optionsFromIntent();
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
     * Tapping the notification lands here with the activity already up. The
     * only case that needs anything is a different connection, which means the
     * screen is about to be one for a session it is not attached to.
     */
    @Override
    protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        setIntent(intent);
        final Connection c = Connections.byId(this, intent.getStringExtra(EXTRA_CONNECTION));
        final String address = c != null ? c.address() : intent.getStringExtra("address");
        if (session != null && address != null && !session.matches(
                c != null ? Sessions.keyFor(c.id()) : Sessions.keyForAddress(address))) {
            recreate();
        }
    }

    /**
     * The notification is not decoration — it is what keeps the connection
     * alive, so it is worth asking for once. Denied, the service still runs and
     * the only loss is being able to see or stop it from the shade.
     */
    private void askAboutNotifications() {
        if (checkSelfPermission(android.Manifest.permission.POST_NOTIFICATIONS)
                != android.content.pm.PackageManager.PERMISSION_GRANTED) {
            requestPermissions(new String[]{android.Manifest.permission.POST_NOTIFICATIONS}, 1);
        }
    }

    /**
     * Focus is told to the backend on the way <em>up</em> only, and from here
     * rather than from {@link #onStart}, because of the clipboard: the core
     * answers a focus gain by asking for ours, the answer comes out of a cache
     * only the main thread fills ({@link SessionClipboard}), and Android lets
     * only a focused window read the clipboard at all — so the refresh has to
     * happen here and before the backend hears about the focus. Saying it again
     * on a re-gain is the point: it is what picks up a copy made in another app.
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
                // Sticky immersive: a swipe brings the bars back for a moment and
                // they go again on their own, so a gesture aimed at the edge of the
                // desktop does not leave them up.
                bars.setSystemBarsBehavior(
                        WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE);
                bars.hide(WindowInsets.Type.systemBars());
            } else {
                // And the way back, for the setting having been turned off over
                // a running session: this window is the only thing that hid
                // them, so it is the only thing that can put them back.
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

    /**
     * Pause when the session is not on screen. The pause itself is the
     * backend's — the RealVNC core stops asking for framebuffer updates, an RFB
     * server sends nothing that was not asked for, RDP is told to suppress
     * output — so there is nothing left here to throttle.
     *
     * <p>Visibility, not window focus: a dialog over the session takes the
     * focus without hiding anything, and freezing the desktop behind an open
     * "connection information" box would be a bug rather than a saving.
     */
    /** Whether this window is on screen, for a session that arrives after it is. */
    private boolean started;

    @Override
    protected void onStart() {
        super.onStart();
        started = true;
        // Here rather than in onCreate, so that turning the row off reaches the
        // session it was turned off during — which is what somebody switching
        // it means, and what the immersive row below already does.
        if (AppSettings.keepAwake(this)) {
            getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        } else {
            getWindow().clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        }
        // The resume half is here as well as in onWindowFocusChanged, for the one
        // case where the two differ: a split screen, where this window is visible
        // and interactive with the focus in the other app. Saying it twice costs
        // nothing — the core compares the flag.
        if (session != null) {
            session.backend().focus(true);
            // The same signal at a longer scale: what pauses the far end is
            // what decides a session has nobody looking at it.
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
            // After suspendInput, which releases each key it is holding by id.
            // This is the sweep for anything the far end still thinks is down
            // that nothing here knows about any more.
            if (AppSettings.releaseKeys(this)) {
                session.backend().releaseAllKeys();
            }
            session.backend().focus(false);
            session.onScreen(false);
        }
        // The panel polls the connection once a second while it is open, and
        // a screen that is not on screen has no business asking.
        if (panel != null) {
            panel.pause();
        }
    }

    /**
     * The card on the home screen: the desktop as it last looked, captured on
     * the way out rather than on a timer, because a preview is a reminder of
     * what is on that machine and not a live view. The backend does the
     * downscale, in whole integer steps, because that is all RealVNC's scaler
     * can do.
     */
    @Override
    protected void onPause() {
        super.onPause();
        savePreview();
    }

    /**
     * Capture the desktop for the home card, if there is one to capture. Called
     * from two places for one reason: {@link #onPause} covers the session that
     * is merely being left on screen, and the disconnect covers the one that is
     * being ended — where the state is CLOSED by the time onPause runs and
     * there is no framebuffer left to photograph.
     */
    private void savePreview() {
        if (connection != null && session != null && !session.isClosed()
                && session.backend().state() == Backend.State.CONNECTED) {
            Connections.saveThumbnail(this, connection.id(),
                    session.backend().thumbnail(THUMBNAIL_MAX, THUMBNAIL_MAX));
        }
    }

    private static final int THUMBNAIL_MAX = 640; // a card on a tablet, decodable on a scroll

    /**
     * The screen goes; the session does not — a connection is not a property of
     * the window looking at it. What is released here is only what this activity
     * made: the mirror bitmap, and the session's pointer back to a view that is
     * about to stop existing.
     */
    @Override
    protected void onDestroy() {
        super.onDestroy();
        // A dialog still up when its activity goes is a leaked window.
        if (panel != null) {
            panel.dismiss();
            panel = null;
        }
        for (AlertDialog d : new ArrayList<>(dialogs)) {
            d.dismiss();
        }
        dialogs.clear();
        if (session != null && view != null) {
            session.detach(view);
        }
        if (view != null) {
            view.release();
        }
        // A session that ended by itself — a refused password, an address that is
        // not there — stays registered on purpose, so the screen can keep
        // showing the reason it stopped. Once that screen is going for good
        // nobody is left to read it, and the next screen for the same machine
        // would re-attach and show "Disconnected" instead of connecting.
        // Rotation is why this is not a rule in Session.matches: a recreated
        // activity does want the dead session, so that a failure survives being
        // turned sideways rather than silently redialling.
        if (isFinishing() && session != null && session.state() == Backend.State.CLOSED) {
            session.close();
            session = null;
        }
    }


    // ---- SessionView.Host ---------------------------------------------------

    /**
     * Both ways out of a session, the tap region and the back gesture, and one
     * dialog between them however many times either is used: back is easy to
     * repeat, and two stacked copies of the same question would need answering
     * twice.
     */
    @Override
    public void disconnectRequested() {
        if (leaving != null && leaving.isShowing()) {
            return;
        }
        // Nothing to disconnect: the connection is already over, or never
        // started. Asking would be a question with one answer.
        if (session == null || session.isClosed()
                || session.backend().state() == Backend.State.CLOSED) {
            leave();
            return;
        }
        leaving = track(new MaterialAlertDialogBuilder(this, R.style.ThemeOverlay_RemoteDesktop_Dialog)
                .setMessage(getString(R.string.session_disconnect_confirm))
                .setNegativeButton(android.R.string.cancel, null)
                // The third answer to "are you leaving": neither, another one as
                // well. This window and its session are untouched, and the list
                // opens beside them — where a card whose session is up returns
                // to its own window, so it is a way to a second machine rather
                // than a way out of this one.
                .setNeutralButton(R.string.session_new_window, (d, w) ->
                        startActivity(new Intent(this, HomeActivity.class)
                                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK
                                        | Intent.FLAG_ACTIVITY_CLEAR_TOP
                                        | Intent.FLAG_ACTIVITY_SINGLE_TOP)))
                .setPositiveButton(R.string.session_disconnect, (d, w) -> {
                    // Before the disconnect, not after: onPause is where a
                    // preview is normally taken, and by the time it runs this
                    // session is CLOSED and its framebuffer gone.
                    savePreview();
                    session.disconnect();
                    leave();
                })
                .show());
    }

    /**
     * Close this window and put the list back.
     *
     * <p>A session is its own document task ({@code documentLaunchMode}), so
     * finishing it lands wherever that task was opened from — the launcher when
     * the session was reached from a notification, or when the home screen's own
     * task has since gone. Leaving a desktop should end up somewhere in this
     * app, and the list is the only place there is: it is also where the other
     * sessions are, since a card whose session is up returns to its window.
     */
    private void leave() {
        // CLEAR_TOP with SINGLE_TOP rather than a plain start: the home screen
        // is usually already the root of its own task, and this brings that task
        // forward instead of building a second copy of it.
        startActivity(new Intent(this, HomeActivity.class)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK
                        | Intent.FLAG_ACTIVITY_CLEAR_TOP
                        | Intent.FLAG_ACTIVITY_SINGLE_TOP));
        finish();
    }

    private AlertDialog leaving;

    /**
     * Every dialog this screen puts up, so that {@link #onDestroy} can take
     * them down: one still showing when its activity goes is a leaked window.
     */
    private final List<AlertDialog> dialogs = new ArrayList<>();

    private AlertDialog track(AlertDialog d) {
        return track(d, null);
    }

    /**
     * A dialog has exactly <em>one</em> dismiss listener, and this method owns
     * it — so a caller that needs to know when its dialog goes hands the work in
     * here rather than setting one of its own, which would silently replace this
     * one or be replaced by it depending on the order.
     */
    private AlertDialog track(AlertDialog d, DialogInterface.OnDismissListener also) {
        dialogs.add(d);
        d.setOnDismissListener(x -> {
            dialogs.remove(d);
            if (also != null) {
                also.onDismiss(x);
            }
        });
        return d;
    }

    /**
     * The connection panel: the session's facts and the options it can still be
     * told, in one sheet over the live desktop ({@link ConnectionPanel}).
     *
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
     * Which backend this session is on: the connection's, the one named on the
     * command line, or the first this build has. The extra is what makes "the
     * same server through two different backends" one command apart.
     */
    private String backendId() {
        if (connection != null && !connection.backendId().isEmpty()) {
            return connection.backendId();
        }
        final String named = getIntent().getStringExtra("backend");
        if (named != null && Backends.ids().contains(named)) {
            return named;
        }
        // A build with no backends at all is possible, and this is called
        // before that case has been reported.
        return Backends.ids().isEmpty() ? "" : Backends.ids().get(0);
    }

    /**
     * Backend options named on the command line, {@code Key=value,Key=value},
     * for the same reason as the backend extra: sweeping a client over a list of
     * encodings through the editor is a person tapping. A saved connection is
     * unaffected — its own options answer for it.
     *
     * <p>Over the backend's own settings rather than instead of them, so this
     * path answers every option the way a saved connection would and the command
     * line is the connection's half of the layering.
     */
    private Map<String, String> optionsFromIntent() {
        final Map<String, String> out = Connections.backendOptions(this, backendId());
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

    /**
     * Start again. The session is over and its backend freed, so there is
     * nothing to re-attach to — the screen is rebuilt from the same intent, and
     * {@link #openOrAttach} finds no live session and makes a new one. Closing
     * the old one first is what makes that true: a session that ended by itself
     * is still registered until somebody lets go of it.
     */
    /** A live option changed on the panel; the screen shows one of them. */
    @Override
    public void panelOptionChanged() {
        if (view != null) {
            view.optionsChanged();
        }
    }

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

    /** The panel's button, on the screen's own path out. */
    @Override
    public void panelDisconnect() {
        disconnectRequested();
    }

    // ---- Prompt.Handler -----------------------------------------------------

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
    private MaterialCheckBox checkBox(int text) {
        final MaterialCheckBox box = new MaterialCheckBox(
                new ContextThemeWrapper(this, R.style.ThemeOverlay_RemoteDesktop_Dialog));
        box.setText(text);
        box.setPadding(0, box.getPaddingTop(), box.getPaddingRight(), box.getPaddingBottom());
        return box;
    }

    /** What the dialog indents its title and message by, so a view can match. */
    private int dialogPadding() {
        final TypedArray a = new ContextThemeWrapper(this, R.style.ThemeOverlay_RemoteDesktop_Dialog)
                .obtainStyledAttributes(new int[]{androidx.appcompat.R.attr.dialogPreferredPadding});
        final int pad = a.getDimensionPixelSize(0, dp(24));
        a.recycle();
        return pad;
    }

    /**
     * A box to type in, in the shape every other form in this app uses — the
     * option row's own layout, so a password is asked for the way an address
     * is. Built whether or not it is wanted, since the caller reads it either
     * way and an unasked field answers with the empty string.
     */
    private EditText field(LinearLayout form, int hint, int inputType, boolean wanted) {
        final TextInputLayout box = (TextInputLayout) getLayoutInflater()
                .inflate(R.layout.item_option_text, form, false);
        box.setHint(hint);
        final TextInputEditText text = box.findViewById(R.id.value);
        text.setInputType(inputType);
        if (wanted) {
            ((ViewGroup.MarginLayoutParams) box.getLayoutParams()).bottomMargin = dp(8);
            form.addView(box);
        }
        return text;
    }

    @Override
    public void credentials(Prompt.Credentials prompt) {
        final LinearLayout form = new LinearLayout(this);
        form.setOrientation(LinearLayout.VERTICAL);
        final int pad = dialogPadding();
        form.setPadding(pad, dp(8), pad, 0);

        if (prompt.instructions != null && !prompt.instructions.isEmpty()) {
            final TextView note = new TextView(this);
            note.setText(prompt.instructions);
            form.addView(note);
        }
        // Only the fields the scheme actually has. VncAuth has no user name,
        // and a box for one is a question nobody can answer.
        final EditText user = field(form, R.string.prompt_username,
                InputType.TYPE_CLASS_TEXT, prompt.needsUserName);
        user.setText(prompt.userName);
        final EditText pass = field(form, R.string.prompt_password,
                InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_PASSWORD,
                prompt.needsPassword);
        if (prompt.catchphrase != null && !prompt.catchphrase.isEmpty()) {
            final TextView id = new TextView(this);
            id.setText(getString(R.string.prompt_catchphrase, prompt.catchphrase));
            form.addView(id);
        }
        // Only for a saved connection: there is nowhere else to put the answer.
        // Ours rather than the core's own credential store, which offers to save
        // after a *failed* attempt as well as a successful one.
        final MaterialCheckBox remember = checkBox(R.string.prompt_remember_password);
        if (connection != null && prompt.needsPassword) {
            form.addView(remember);
        }

        show(new MaterialAlertDialogBuilder(this, R.style.ThemeOverlay_RemoteDesktop_Dialog)
                .setTitle(prompt.address != null && !prompt.address.isEmpty()
                        ? prompt.address : getString(R.string.prompt_credentials_title))
                .setView(form)
                .setPositiveButton(R.string.prompt_connect, (d, w) -> {
                    final String u = prompt.needsUserName ? user.getText().toString() : null;
                    final String p = prompt.needsPassword ? pass.getText().toString() : null;
                    if (remember.isChecked()) {
                        // Held, not saved: what was typed is only known to be
                        // right once the connection comes up.
                        pendingUser = u;
                        pendingPassword = p;
                    }
                    prompt.answer(u, p);
                })
                .setNegativeButton(android.R.string.cancel, (d, w) -> decline(prompt)), prompt);
    }

    private String pendingUser;
    private String pendingPassword;

    /**
     * Whether the session ended because a prompt was declined. A connection that
     * fails on its own deserves its message on screen; a cancelled prompt is an
     * answer already given, and leaving "Disconnected" over a black screen for
     * it means pressing back to leave a room you just left.
     */
    private boolean declined;

    /**
     * Credentials typed into a prompt with "remember" ticked, written to the
     * connection once — and only once — the far end has accepted them.
     */
    @Override
    public void disconnected() {
        // Declined at a prompt, or disconnected on purpose — from the region, the
        // back gesture, or the notification's button.
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
        track(new MaterialAlertDialogBuilder(this, R.style.ThemeOverlay_RemoteDesktop_Dialog)
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

    // ---- where the controls are --------------------------------------------

    /**
     * The four tap regions, said out loud once.
     *
     * <p>They have been the way to reach everything on this screen since there
     * was a screen, and nothing announces them: there is no toolbar to see and
     * no button to find, which is what makes the desktop the whole surface and
     * also what makes it unguessable.
     *
     * <p>The dialog dims nothing behind it on purpose. What it is about is the
     * four highlighted bands underneath, and a scrim would hide the
     * demonstration to make room for the explanation.
     */
    @Override
    public void firstFrame() {
        if (view == null || !AppSettings.regionHints(this)) {
            return;
        }
        view.setRegionHints(Map.of(
                TapRegions.DISCONNECT, getString(R.string.hints_region_disconnect),
                TapRegions.INFORMATION, getString(R.string.hints_region_information),
                TapRegions.KEYBOARD, getString(R.string.hints_region_keyboard),
                TapRegions.MOUSE, getString(R.string.hints_region_mouse)));

        final MaterialCheckBox never = checkBox(R.string.hints_dismiss);
        final LinearLayout box = new LinearLayout(this);
        box.setPadding(dialogPadding(), dp(8), dialogPadding(), 0);
        box.addView(never);

        final AlertDialog dialog = new MaterialAlertDialogBuilder(this,
                R.style.ThemeOverlay_RemoteDesktop_Dialog)
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
        // dialog's dismiss listener, and a dialog has only one — so a listener
        // installed here would be thrown away, which is what left the four bands
        // lit for the rest of the session and "Do not show this again" doing
        // nothing at all.
        track(dialog, d -> {
            if (never.isChecked()) {
                AppSettings.setRegionHints(this, false);
            }
            if (view != null) {
                view.setRegionHints(null);
            }
        });
        dialog.show();
    }

    @Override
    public void connected() {
        if (connection == null || pendingPassword == null) {
            return;
        }
        connection = new Connection(connection.id(), connection.name(), connection.backendId(),
                connection.address(),
                pendingUser != null && !pendingUser.isEmpty() ? pendingUser
                        : connection.userName(),
                connection.sealedPassword(), connection.options(), connection.pinned())
                .withPassword(pendingPassword);
        Connections.save(this, connection);
        pendingUser = null;
        pendingPassword = null;
    }

    @Override
    public void trust(Prompt.Trust prompt) {
        final StringBuilder sb = new StringBuilder();
        // OK, PRESHARED and ARD say nothing: this prompt is up because
        // something else about the connection is worth asking about.
        final String why = switch (prompt.identity) {
            case CHANGED -> getString(R.string.prompt_trust_changed);
            case MATCHES_ANOTHER -> getString(R.string.prompt_trust_another,
                    prompt.matchingAddress != null && !prompt.matchingAddress.isEmpty()
                            ? prompt.matchingAddress
                            : getString(R.string.prompt_trust_another_address));
            case NEW -> getString(R.string.prompt_trust_new);
            case MISSING -> getString(R.string.prompt_trust_missing);
            case OK, PRESHARED, ARD -> "";
        };
        if (!why.isEmpty()) {
            sb.append(why).append("\n\n");
        }
        if (prompt.encryption == Prompt.Trust.Encryption.UNENCRYPTED_WARN) {
            sb.append(getString(R.string.prompt_unencrypted)).append("\n\n");
        }
        if (prompt.catchphrase != null && !prompt.catchphrase.isEmpty()) {
            sb.append(getString(R.string.prompt_catchphrase, prompt.catchphrase)).append('\n');
        }
        if (prompt.signature != null && !prompt.signature.isEmpty()) {
            sb.append(getString(R.string.prompt_signature, prompt.signature));
        }

        final MaterialCheckBox remember = checkBox(R.string.prompt_remember_server);
        remember.setChecked(prompt.identity != Prompt.Trust.Identity.CHANGED);
        final LinearLayout box = new LinearLayout(this);
        box.setPadding(dialogPadding(), dp(8), dialogPadding(), 0);
        box.addView(remember);

        show(new MaterialAlertDialogBuilder(this, R.style.ThemeOverlay_RemoteDesktop_Dialog)
                .setTitle(prompt.address != null ? prompt.address
                        : getString(R.string.prompt_trust_title))
                .setMessage(sb.toString())
                .setView(box)
                .setPositiveButton(R.string.prompt_continue,
                        (d, w) -> prompt.answer(true, remember.isChecked()))
                .setNegativeButton(android.R.string.cancel, (d, w) -> decline(prompt)), prompt);
    }

    @Override
    public void message(Prompt.Message prompt) {
        final MaterialAlertDialogBuilder b = new MaterialAlertDialogBuilder(this, R.style.ThemeOverlay_RemoteDesktop_Dialog)
                .setMessage(prompt.text)
                .setPositiveButton(prompt.confirmLabel != null ? prompt.confirmLabel
                                : getString(android.R.string.ok),
                        (d, w) -> prompt.answer(true));
        if (prompt.question) {
            b.setNegativeButton(android.R.string.cancel, (d, w) -> prompt.answer(false));
        }
        show(b, prompt);
    }

    /**
     * A dismissed dialog still owes the core an answer — the session is
     * blocked until it gets one — so cancelling counts as declining.
     */
    private void show(AlertDialog.Builder builder, Prompt prompt) {
        final AlertDialog dialog = builder.create();
        dialog.setOnCancelListener((DialogInterface d) -> decline(prompt));
        track(dialog);
        dialog.show();
    }

    /** Say no, and remember that it was us who did. */
    private void decline(Prompt prompt) {
        declined = true;
        prompt.cancel();
    }

    private int dp(float v) {
        return Math.round(TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_DIP, v,
                getResources().getDisplayMetrics()));
    }
}
