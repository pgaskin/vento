// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.app.Activity;
import android.os.Handler;
import android.os.Looper;
import android.view.LayoutInflater;
import android.view.View;
import android.view.ViewGroup;
import android.widget.TextView;

import androidx.core.graphics.Insets;
import androidx.core.view.ViewCompat;
import androidx.core.view.WindowInsetsCompat;

import com.google.android.material.bottomsheet.BottomSheetBehavior;
import com.google.android.material.bottomsheet.BottomSheetDialog;
import com.google.android.material.button.MaterialButton;

import net.pgaskin.remotedesktop.backend.Backend;
import net.pgaskin.remotedesktop.backend.BackendOption;
import net.pgaskin.remotedesktop.backend.Backends;
import net.pgaskin.remotedesktop.backend.ConnectionFact;
import net.pgaskin.remotedesktop.backend.Monitor;

import java.util.ArrayList;
import java.util.List;
import java.util.Map;

/**
 * What the {@code information} tap region opens: one sheet holding the session's
 * facts <em>and</em> the options it can still be told, over the live desktop.
 *
 * <p>Four decisions in it:
 *
 * <ol>
 *   <li><b>Controls above facts.</b> The region's real job is "change the
 *       quality", and the facts are what you read <em>after</em> deciding
 *       something is wrong.
 *   <li><b>Nothing opens a second window, and nothing leaves.</b> A choice drops
 *       a menu under its own row; a switch resolves where it is. The footer
 *       appears only once there is no session to leave, because this panel is
 *       about the connection in front of you rather than the record it was
 *       opened from.
 *   <li><b>The facts refresh while it is open</b>, once a second. Two of them
 *       move on their own — the encoding follows the quality group and the line
 *       speed is a running measurement — so this is where a quality change
 *       becomes visible as something other than a feeling.
 *   <li><b>Both lists are generated</b>, so this class knows no parameter names
 *       and no protocol vocabulary.
 *   <li><b>The desktop size and the far end's display are here rather than in
 *       the editor</b>, and are the two controls that are not options: whether
 *       the far end will take a size at all, and whether it has another screen
 *       to send, are facts about the connection in front of you, which is
 *       exactly what this panel is for.
 * </ol>
 *
 * <p>A sheet over the desktop rather than a screen replacing it, which the
 * original cannot have: the pause hangs off visibility rather than window
 * focus, so the desktop behind this stays live and a quality change can be
 * watched landing. Nothing here touches the viewport insets — the desktop must
 * not move to make room for a panel that is about to close again.
 */
final class ConnectionPanel {

    /** What the panel can ask of the screen that owns the session. */
    interface Host {
        /** Start again, for a session that has already ended. */
        void panelReconnect();

        /**
         * End this one. The screen's own disconnect, so the panel's button asks
         * the same question the tap region and the back gesture ask, and leaves
         * the same way — a preview taken before the framebuffer goes, and one
         * confirmation however it was reached.
         */
        void panelDisconnect();

        /**
         * A live option was changed from here. Most of them are the backend's
         * business alone; view-only is not, because the screen offers a keyboard
         * and a mouse overlay that it would make meaningless.
         */
        void panelOptionChanged();
    }

    private static final long REFRESH_MS = 1000; // fast enough that a change reads as a result

    private static final int MAX_WIDTH_DP = 560; // past which it is a centred card

    private final Activity activity;
    private final Session session;
    private final Host host;
    private final BottomSheetDialog dialog;
    private final Handler handler = new Handler(Looper.getMainLooper());

    private final TextView title;
    private final TextView subtitle;
    private final ViewGroup controls;
    private final ViewGroup displayRow;
    private final ViewGroup resize;
    private final ViewGroup facts;
    private final ViewGroup diagnostics;
    private final MaterialButton action;
    private final MaterialButton screenshot;

    private final String backendName;       // which client this session is on
    private final List<BackendOption> live; // changeable on a running session

    private final PanelOptions optionRows;  // so a refresh writes values instead of views

    /**
     * What this session connected with — the saved connection's options over the
     * backend's globals over the schema's defaults. The bottom two layers of
     * what a control shows; {@link Session#liveOption} is the top one.
     */
    private final Map<String, String> connectedWith;

    /**
     * The desktop-size row, and the choices it was built with. Rebuilt rather
     * than refreshed when those change, which is what a granted resize does to
     * them: the size that has just arrived stops being one of the offers.
     */
    private PanelOptions resizeRow;
    private List<BackendOption.Choice> resizeChoices = List.of();

    /** The same again for the far end's displays, where it sends one at a time. */
    private PanelOptions displayOptions;
    private List<BackendOption.Choice> displayChoices = List.of();

    /** The fact rows on screen: what they are, and where their values go. */
    private List<String> factShape = List.of();
    private final List<TextView> factValues = new ArrayList<>();

    private String desktopName;             // once the protocol has said it

    private boolean polling;

    static ConnectionPanel show(Activity activity, Session session,
                                String backendId, Host host) {
        final ConnectionPanel panel = new ConnectionPanel(activity, session, backendId, host);
        panel.open();
        return panel;
    }

    private ConnectionPanel(Activity activity, Session session, String backendId, Host host) {
        this.activity = activity;
        this.session = session;
        this.host = host;

        dialog = new BottomSheetDialog(activity);
        final View content = LayoutInflater.from(dialog.getContext())
                .inflate(R.layout.sheet_connection, null);
        dialog.setContentView(content);
        dialog.getBehavior().setMaxWidth(dp(MAX_WIDTH_DP));
        // Open at full height, and have a drag downwards dismiss it rather than
        // park it half way: a collapsed state is for a sheet whose content
        // continues below the fold, and this one is a page. In landscape the
        // default showed the title and nothing else.
        dialog.getBehavior().setSkipCollapsed(true);
        dialog.getBehavior().setState(BottomSheetBehavior.STATE_EXPANDED);
        // The session screen is drawn edge to edge and may be hiding the system
        // bars entirely; the sheet is a different window and gets neither for
        // free. Its own bottom padding is the whole of what it needs.
        ViewCompat.setOnApplyWindowInsetsListener(content, (v, insets) -> {
            final Insets bars = insets.getInsets(WindowInsetsCompat.Type.systemBars());
            v.setPadding(v.getPaddingLeft(), v.getPaddingTop(), v.getPaddingRight(), bars.bottom);
            return insets;
        });

        title = content.findViewById(R.id.title);
        subtitle = content.findViewById(R.id.subtitle);
        controls = content.findViewById(R.id.controls);
        displayRow = content.findViewById(R.id.display);
        resize = content.findViewById(R.id.resize);
        facts = content.findViewById(R.id.facts);
        diagnostics = content.findViewById(R.id.diagnostics);
        action = content.findViewById(R.id.action);
        screenshot = content.findViewById(R.id.screenshot);

        content.findViewById(R.id.close).setOnClickListener(v -> dialog.dismiss());
        content.findViewById(R.id.log).setOnClickListener(v -> SessionLog.show(activity));
        // The panel stays open behind the share sheet, and the desktop behind
        // it stays live: this takes a copy of the connection rather than doing
        // anything to it, so there is nothing here to come back to.
        screenshot.setTooltipText(activity.getString(R.string.panel_screenshot));
        screenshot.setOnClickListener(v -> Screenshot.share(activity, session));
        action.setOnClickListener(v -> {
            // Which of the two it is now, not which it was when the panel
            // opened: a session can end while this is on screen.
            final boolean ended = ended();
            dialog.dismiss();
            if (ended) {
                host.panelReconnect();
            } else {
                host.panelDisconnect();
            }
        });

        backendName = Backends.name(backendId);
        live = Backends.options(backendId).stream().filter(BackendOption::live).toList();
        // What the session is on, rather than what the record says: the two
        // differ once either has been edited since it opened, and an
        // address with no record behind it has only this.
        connectedWith = session.openedWith();
        optionRows = new PanelOptions(controls, live, new PanelOptions.Values() {
            @Override
            public String get(BackendOption o) {
                return value(o);
            }

            @Override
            public void set(BackendOption o, String value) {
                session.setLiveOption(o.key(), value);
                host.panelOptionChanged();
            }

            @Override
            public String get(String key) {
                final String changed = session.liveOption(key);
                return changed != null ? changed : connectedWith.get(key);
            }
        });
        controls.setVisibility(optionRows.isEmpty() ? View.GONE : View.VISIBLE);
        refreshAction();    // before it is shown, so the button is never briefly the other one
        refreshDisplays();
        screenshot.setEnabled(connected());  // ... and the same for this one
        refreshResize();
        setFacts(List.of());

        dialog.setOnDismissListener(d -> pause());
    }

    private void open() {
        dialog.show();
        resume();
    }

    boolean isShowing() {
        return dialog.isShowing();
    }

    void dismiss() {
        dialog.dismiss();
    }

    // ---- the poll -----------------------------------------------------------

    /**
     * Stop asking. Called on dismissal, and by the screen when it stops — a
     * panel left open behind a backgrounded app would be putting a question to
     * the protocol every second, which is precisely what the pause exists to
     * prevent.
     */
    void pause() {
        polling = false;
        handler.removeCallbacks(tick);
    }

    void resume() {
        if (polling || !dialog.isShowing()) {
            return;
        }
        polling = true;
        tick.run();
    }

    private final Runnable tick = new Runnable() {
        @Override
        public void run() {
            if (!polling) {
                return;
            }
            refreshHeader();
            refreshControls();
            refreshAction();
            // Asynchronous, and may never answer: the backend's thread is
            // allowed to be inside a connect or behind an unanswered prompt.
            // Nothing above depends on it arriving.
            session.backend().connectionInfo(f -> {
                if (polling) {
                    setFacts(f);
                }
            });
            handler.postDelayed(this, REFRESH_MS);
        }
    };

    // ---- the header and the footer -----------------------------------------

    private void refreshHeader() {
        title.setText(desktopName != null && !desktopName.isEmpty()
                ? desktopName : session.title());
        subtitle.setText(stateLine());
    }

    /**
     * What the connection is doing, in one line: the state, which client it is
     * on, and the desktop. The client is here because it is the one thing about
     * a live session that no other surface repeats — a machine can be reached
     * by four of them and they do not behave alike, so "which one is this" is
     * asked of the panel the way the picture's quality is.
     *
     * <p>The size comes from the backend rather than from the facts: it is
     * known locally the moment the first framebuffer arrives, and this line has
     * to say something before any answer has come back over the wire.
     */
    private String stateLine() {
        if (!connected()) {
            return session.status();
        }
        final int w = session.backend().desktopWidth();
        final int h = session.backend().desktopHeight();
        if (w <= 0 || h <= 0) {
            return activity.getString(R.string.panel_connected, backendName);
        }
        // How the desktop is divided is only worth a word when it is divided:
        // one monitor is what a desktop is, and saying so on every session
        // would be a fact that is never news.
        final int monitors = session.backend().monitors().size();
        return monitors > 1
                ? activity.getString(R.string.panel_connected_monitors, backendName, w, h, monitors)
                : activity.getString(R.string.panel_connected_size, backendName, w, h);
    }

    /** A session that is over: the bottom action becomes a way back in. */
    private boolean ended() {
        return session.isClosed() || session.state() == Backend.State.CLOSED;
    }

    /**
     * A session with a desktop behind it, which is what everything on this
     * panel that acts on the connection needs and none of it can assume: one
     * that is still connecting has no picture to photograph and no option that
     * would take effect.
     *
     * <p>Not {@code !ended()} — the two differ for a session that has not
     * connected yet, which is neither.
     */
    private boolean connected() {
        return session.state() == Backend.State.CONNECTED && !session.isClosed();
    }

    /**
     * The footer. One button at the end of the row, and which one it is follows
     * the session: Disconnect while there is something to disconnect, Reconnect
     * once there is not — two states of a slot rather than two buttons, since
     * no session is ever in both. The log is beside it always, and is the only
     * place the reason a connection failed is written down in full.
     */
    private void refreshAction() {
        action.setText(ended() ? R.string.panel_reconnect : R.string.session_disconnect);
    }

    // ---- the controls -------------------------------------------------------

    /**
     * Values, and whether the controls mean anything at all. None of them is
     * live against a session that is connecting or gone — the backend would
     * take the change and hold it for a connection that may never happen, and a
     * switch that moves and does nothing is a lie.
     */
    private void refreshControls() {
        final boolean enabled = connected();
        optionRows.setEnabled(enabled);
        optionRows.refresh();
        refreshDisplays();
        // On the same poll rather than on the click, so that a session which
        // has gone while the panel was open says so by going grey instead of
        // by a toast after the fact.
        screenshot.setEnabled(enabled);
        refreshResize();
    }

    // ---- the far end's displays ----------------------------------------------

    /**
     * Which screen the far end is sending, for the one protocol that sends one
     * of them at a time. A live fact for {@link #refreshResize}'s reason, and
     * gone entirely where the far end has a single screen or reports none.
     *
     * <p>Its value is the display the session <em>has</em>, asked for again
     * every poll: a switch is a request, and a peer that answers with a
     * different screen than the one tapped is a tick that moves somewhere else.
     */
    private void refreshDisplays() {
        final Backend backend = session.backend();
        final List<Monitor> displays = session.isClosed() ? List.of() : backend.displays();
        displayRow.setVisibility(displays.size() > 1 ? View.VISIBLE : View.GONE);
        if (displays.size() < 2) {
            displayOptions = null;
            displayChoices = List.of();
            displayRow.removeAllViews();
            return;
        }
        final List<BackendOption.Choice> choices = new ArrayList<>();
        for (int i = 0; i < displays.size(); i++) {
            final Monitor m = displays.get(i);
            choices.add(new BackendOption.Choice(String.valueOf(i),
                    activity.getString(R.string.panel_display_value, i + 1, m.width(), m.height())));
        }
        if (displayOptions != null && choices.equals(displayChoices)) {
            displayOptions.refresh();
            return;
        }
        displayChoices = choices;
        displayRow.removeAllViews();
        final BackendOption option = BackendOption.choice("", // no backend owns this one
                activity.getString(R.string.panel_display), null,
                "", BackendOption.Scope.CONNECTION, true,
                choices.toArray(new BackendOption.Choice[0]));
        displayOptions = new PanelOptions(displayRow, List.of(option), new PanelOptions.Values() {
            @Override
            public String get(BackendOption o) {
                return String.valueOf(session.backend().display());
            }

            @Override
            public void set(BackendOption o, String value) {
                session.backend().requestDisplay(Integer.parseInt(value));
            }
        });
    }

    // ---- the desktop size ----------------------------------------------------

    /**
     * The one control here that is not a {@link BackendOption}, because whether
     * it can be offered at all is a fact about the live connection rather than
     * a description of the backend: a VNC server announces that it takes a size
     * by sending a rectangle, which may never arrive. So this asks every poll
     * and the row appears when the answer does.
     *
     * <p>Its value is the size the desktop <em>has</em>, read from the backend
     * rather than remembered from what was asked for. A server may refuse or
     * grant something else, and there is nothing here that could tell the
     * difference — so the control reports what arrived, and a refusal is a tick
     * that stays where it was.
     */
    private void refreshResize() {
        final Backend backend = session.backend();
        final boolean can = !session.isClosed() && backend.canResize();
        resize.setVisibility(can ? View.VISIBLE : View.GONE);
        if (!can) {
            resizeRow = null;
            resizeChoices = List.of();
            resize.removeAllViews();
            return;
        }
        final List<BackendOption.Choice> choices = sizeChoices();
        if (resizeRow != null && choices.equals(resizeChoices)) {
            resizeRow.refresh();
            return;
        }
        resizeChoices = choices;
        resize.removeAllViews();
        final BackendOption option = BackendOption.choice("", // no backend owns this one
                activity.getString(R.string.panel_desktop_size), null,
                "", BackendOption.Scope.CONNECTION, true,
                choices.toArray(new BackendOption.Choice[0]));
        // Through the session each time rather than through the backend caught
        // above: a reconnection from the footer builds a new one, and this row
        // outlives that.
        resizeRow = new PanelOptions(resize, List.of(option), new PanelOptions.Values() {
            @Override
            public String get(BackendOption o) {
                return size(session.backend().desktopWidth(), session.backend().desktopHeight());
            }

            @Override
            public void set(BackendOption o, String value) {
                final int at = value.indexOf('x');
                if (at > 0) {
                    session.backend().requestDesktopSize(
                            Integer.parseInt(value.substring(0, at)),
                            Integer.parseInt(value.substring(at + 1)));
                }
            }
        });
    }

    /**
     * What to offer: this phone's window first, since fitting it is the whole
     * point; then the size the desktop already has, so the current state has a
     * row of its own; then the ordinary ones, which are the RDP backend's list
     * for the same reason it has one.
     */
    private List<BackendOption.Choice> sizeChoices() {
        final List<BackendOption.Choice> choices = new ArrayList<>();
        final int[] device = SessionView.deviceSize(activity);
        if (device != null) {
            choices.add(new BackendOption.Choice(size(device[0], device[1]),
                    activity.getString(R.string.panel_desktop_size_device,
                            device[0], device[1])));
        }
        final int[][] sizes = {
                {session.backend().desktopWidth(), session.backend().desktopHeight()},
                {1920, 1200}, {1920, 1080}, {1600, 1000},
                {1366, 768}, {1280, 800}, {2560, 1440},
        };
        for (int[] s : sizes) {
            if (s[0] <= 0 || s[1] <= 0) {
                continue;
            }
            final String value = size(s[0], s[1]);
            if (choices.stream().noneMatch(c -> c.value().equals(value))) {
                choices.add(new BackendOption.Choice(value,
                        activity.getString(R.string.panel_desktop_size_value, s[0], s[1])));
            }
        }
        return choices;
    }

    /** A size as the choice's value, which is what {@code set} parses back. */
    private static String size(int width, int height) {
        return width + "x" + height;
    }

    /**
     * What an option is set to right now: what this session was told, else what
     * it connected with, else what the backend does when nothing is set.
     */
    private String value(BackendOption o) {
        final String changed = session.liveOption(o.key());
        if (changed != null) {
            return changed;
        }
        final String connected = connectedWith.get(o.key());
        return connected != null ? connected : o.defaultValue();
    }

    // ---- the facts ----------------------------------------------------------

    /**
     * What the backend has to say, rebuilt only when its <em>shape</em> changes.
     * A poll that re-inflated eight rows a second would throw away the fold
     * state and flicker; what actually moves is two values.
     */
    private void setFacts(List<ConnectionFact> all) {
        final List<ConnectionFact> rows = new ArrayList<>();
        desktopName = null;
        for (ConnectionFact f : all) {
            if (f.field() == ConnectionFact.Field.DESKTOP_NAME) {
                // The heading of the panel, not a row in it.
                desktopName = f.value();
            } else {
                rows.add(f);
            }
        }
        refreshHeader();

        final List<String> shape = new ArrayList<>();
        for (ConnectionFact f : rows) {
            shape.add(f.field() + "/" + f.label() + "/" + f.diagnostic());
        }
        if (!shape.equals(factShape)) {
            rebuildFacts(rows);
            factShape = shape;
        } else {
            for (int i = 0; i < rows.size(); i++) {
                factValues.get(i).setText(display(rows.get(i)));
            }
        }
    }

    private void rebuildFacts(List<ConnectionFact> rows) {
        facts.removeAllViews();
        diagnostics.removeAllViews();
        factValues.clear();
        final LayoutInflater inf = LayoutInflater.from(dialog.getContext());
        int plain = 0;
        int diags = 0;
        for (ConnectionFact f : rows) {
            final ViewGroup into = f.diagnostic() ? diagnostics : facts;
            final View row = inf.inflate(R.layout.item_fact, into, false);
            ((TextView) row.findViewById(R.id.label)).setText(f.label());
            final TextView value = row.findViewById(R.id.value);
            value.setText(display(f));
            into.addView(row);
            factValues.add(value);
            if (f.diagnostic()) {
                diags++;
            } else {
                plain++;
            }
        }
        RowGroups.apply(facts);
        RowGroups.apply(diagnostics);
        facts.setVisibility(plain == 0 ? View.GONE : View.VISIBLE);
        diagnostics.setVisibility(diags == 0 ? View.GONE : View.VISIBLE);
    }

    /**
     * A field the backend has but no answer for yet says so, rather than
     * vanishing: the shape of the panel depends on the protocol, not on how much
     * this server has got round to saying.
     */
    private String display(ConnectionFact f) {
        return f.value().isEmpty() ? activity.getString(R.string.panel_not_set) : f.value();
    }

    private int dp(float v) {
        return Math.round(android.util.TypedValue.applyDimension(
                android.util.TypedValue.COMPLEX_UNIT_DIP, v,
                activity.getResources().getDisplayMetrics()));
    }
}
