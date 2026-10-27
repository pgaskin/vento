// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.annotation.SuppressLint;
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
import net.pgaskin.remotedesktop.session.WindowFollower;

import java.util.ArrayList;
import java.util.List;
import java.util.Map;

/**
 * What the {@code information} tap region opens: one sheet holding the session's
 * facts <em>and</em> its live options, over the live desktop.
 *
 * <ol>
 *   <li><b>Controls above facts.</b> The panel is mostly opened to change the
 *       quality, and the facts are read <em>after</em> deciding something is
 *       wrong.
 *   <li><b>Changes happen in place.</b> A choice opens a menu under its own row
 *       (a dialog only when its choices need descriptions); a switch takes
 *       effect where it is. The panel is about the live connection, not the
 *       saved record, so it has no link to the editor.
 *   <li><b>The facts refresh while it is open</b>, once a second. Some change
 *       by themselves (the encoding follows the quality setting and the line
 *       speed is a running measurement), so a quality change is visible here.
 *   <li><b>Both lists are generated</b>, so this class knows no parameter names
 *       and no protocol vocabulary.
 *   <li><b>The desktop size, the far end's display and the sound switch are here
 *       rather than in the editor</b>, and are not backend options: whether
 *       the far end accepts a size, has another screen to send, or can send
 *       sound are facts about the live connection.
 * </ol>
 *
 * <p>A sheet over the desktop rather than a screen replacing it, which RealVNC
 * Viewer does not do: the session pauses on visibility rather than window
 * focus, so the desktop behind this stays live and a quality change can be
 * seen taking effect. Nothing here changes the viewport insets, so the desktop
 * does not move to make room for a panel that will close again.
 */
final class ConnectionPanel {

    /** What the panel can ask of the screen that owns the session. */
    interface Host {
        /** Start again, for a session that has already ended. */
        void panelReconnect();

        /**
         * End this session. The screen's own disconnect, so the panel's button
         * asks the same confirmation as the tap region and the back gesture,
         * and ends the same way, with a preview taken before the framebuffer is
         * gone.
         */
        void panelDisconnect();

        /**
         * A live option was changed from here. Most only concern the backend;
         * view-only also affects the screen, which offers a keyboard and a mouse
         * overlay that view-only makes useless.
         */
        void panelOptionChanged();
    }

    private static final long REFRESH_MS = 1000; // fast enough to show the effect of a change

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
    private final ViewGroup soundRow;
    private final ViewGroup facts;
    private final ViewGroup diagnostics;
    /** The Tunnel group, last, shown only where the connection has one. */
    private final ViewGroup tunnel;
    private final MaterialButton action;
    private final MaterialButton screenshot;

    private final String backendName;       // which client this session is on
    private final List<BackendOption> live; // changeable on a running session

    private final PanelOptions optionRows;  // so a refresh writes values instead of views

    /**
     * What this session connected with: the saved connection's options over the
     * backend's globals. With the schema's defaults these are the lower layers
     * of what a control shows; {@link Session#liveOption} is the top one.
     */
    private final Map<String, String> connectedWith;

    /**
     * The desktop-size row, and the choices it was built with. Rebuilt rather
     * than refreshed when the choices change, as they do after a granted
     * resize, since the current size is one of them.
     */
    private PanelOptions resizeRow;
    private List<BackendOption.Choice> resizeChoices = List.of();

    /** The same for the far end's displays, where it sends one at a time. */
    private PanelOptions displayOptions;
    private List<BackendOption.Choice> displayChoices = List.of();

    /** The sound switch's row, and which switch it was built for. */
    private PanelOptions soundOptions;
    private int soundKind = Backend.SOUND_SWITCH_NONE;

    /** The fact rows on screen: their identity, and their value views. */
    private List<String> factShape = List.of();
    private final List<TextView> factValues = new ArrayList<>();

    private String desktopName;             // once the protocol has sent it

    private boolean polling;

    static ConnectionPanel show(Activity activity, Session session,
                                String backendId, Host host) {
        final ConnectionPanel panel = new ConnectionPanel(activity, session, backendId, host);
        panel.open();
        return panel;
    }

    // A sheet's root has no parent to be measured against: the dialog's window
    // supplies the layout parameters and the ones in the file are not read.
    @SuppressLint("InflateParams")
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
        // collapse it half way: a collapsed state suits a sheet whose content
        // continues below the fold, and this one is a page. In landscape the
        // default showed only the title.
        dialog.getBehavior().setSkipCollapsed(true);
        dialog.getBehavior().setState(BottomSheetBehavior.STATE_EXPANDED);
        // The session screen is drawn edge to edge and may hide the system bars;
        // the sheet is a separate window and inherits neither. Bottom padding
        // is all it needs.
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
        soundRow = content.findViewById(R.id.sound);
        facts = content.findViewById(R.id.facts);
        diagnostics = content.findViewById(R.id.diagnostics);
        tunnel = content.findViewById(R.id.tunnel);
        action = content.findViewById(R.id.action);
        screenshot = content.findViewById(R.id.screenshot);

        content.findViewById(R.id.close).setOnClickListener(v -> dialog.dismiss());
        content.findViewById(R.id.log).setOnClickListener(v -> SessionLog.show(activity));
        // The panel stays open behind the share sheet, and the desktop behind
        // it stays live: a screenshot does not change the session.
        screenshot.setTooltipText(activity.getString(R.string.panel_screenshot));
        screenshot.setOnClickListener(v -> Screenshot.share(activity, session));
        action.setOnClickListener(v -> {
            // Checked now, not when the panel opened: a session can end while
            // this is on screen.
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
        // What the session was opened with, rather than what the record says:
        // the two differ once the record has been edited since, and a session
        // opened by address has no record.
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
        refreshAction();    // before it is shown, so the button never briefly shows the other label
        refreshDisplays();
        screenshot.setEnabled(connected());  // likewise
        refreshResize();
        refreshSound();
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
     * Stops polling. Called on dismissal, and by the screen when it stops: a
     * panel left open behind a backgrounded app would query the backend every
     * second, which the session pause exists to prevent.
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

    /**
     * What the backend returned at the last tick, which every row on this panel
     * is drawn from.
     *
     * <p>One call into the backend per poll rather than one per row, and, more
     * importantly, one consistent snapshot: as three separate queries (the
     * display list, the current display, and whether a size can be requested),
     * a screen plugged in at the far end between two of them could make the
     * panel draw a row from one answer and a value from another.
     */
    private Backend.Facts polled = Backend.Facts.NONE;

    private final Runnable tick = new Runnable() {
        @Override
        public void run() {
            if (!polling) {
                return;
            }
            polled = session.isClosed() ? Backend.Facts.NONE : session.backend().facts();
            refreshHeader();
            refreshControls();
            refreshAction();
            // Asynchronous, and may never return: the backend's thread may be
            // inside a connect or waiting on an unanswered prompt. Nothing above
            // depends on it.
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
     * The connection's state in one line: the state, the backend, and the
     * desktop size. The backend is here because no other part of the session
     * screen shows it, and a machine may be reachable through several backends
     * that behave differently.
     *
     * <p>The size comes from the backend rather than from the facts: it is
     * known locally as soon as the first framebuffer arrives, and this line has
     * to show something before any facts have arrived.
     */
    private String stateLine() {
        if (!connected()) {
            return session.status().text();
        }
        final int w = session.backend().desktopWidth();
        final int h = session.backend().desktopHeight();
        if (w <= 0 || h <= 0) {
            return activity.getString(R.string.panel_connected, backendName);
        }
        // The monitor count is only shown when there is more than one.
        final int monitors = polled.monitors().size();
        return monitors > 1
                ? activity.getString(R.string.panel_connected_monitors, backendName, w, h, monitors)
                : activity.getString(R.string.panel_connected_size, backendName, w, h);
    }

    /** A session that is over, for which the footer action becomes Reconnect. */
    private boolean ended() {
        return session.isClosed() || session.state() == Backend.State.CLOSED;
    }

    /**
     * A session with a desktop, which every action on this panel needs: one
     * that is still connecting has no picture to capture and no option that
     * would take effect.
     *
     * <p>Not {@code !ended()}: a session that has not connected yet is neither.
     */
    private boolean connected() {
        return session.state() == Backend.State.CONNECTED && !session.isClosed();
    }

    /**
     * The footer's action button follows the session: Disconnect while it is
     * live, Reconnect once it has ended. One button with two states rather than
     * two buttons, since a session is never in both. The log button is always
     * beside it, and the log is the only place the full reason a connection
     * failed is shown.
     */
    private void refreshAction() {
        action.setText(ended() ? R.string.panel_reconnect : R.string.session_disconnect);
    }

    // ---- the controls -------------------------------------------------------

    /**
     * Values, and whether the controls are enabled. They are disabled while the
     * session is connecting or gone: the backend would accept the change and
     * hold it for a connection that may never happen, and a switch that moves
     * but has no effect is misleading.
     */
    private void refreshControls() {
        final boolean enabled = connected();
        optionRows.setEnabled(enabled);
        optionRows.refresh();
        refreshDisplays();
        // On each poll rather than on the click, so a session that ends while
        // the panel is open disables the button instead of showing a toast
        // after the tap.
        screenshot.setEnabled(enabled);
        refreshResize();
        refreshSound();
    }

    // ---- the far end's sound -------------------------------------------------

    /**
     * The session's sound switch, shown from when the far end may send sound
     * rather than from when it does, so sound can be refused before it starts.
     * The backend decides which switch: "Enable audio" where the far end can be
     * asked to stop and resume sending sound, "Mute audio" where it cannot
     * (RDP) and the sound is silenced here instead. Here and not on the
     * toolbar, whose buttons match the tap regions exactly, since a screen may
     * have either one alone. Live session state like {@link #refreshResize},
     * and not an option: it applies immediately and is not saved (see
     * {@link Backend#setSoundEnabled} and {@link Backend#setMuted}).
     *
     * <p>Called by the poll and, through the screen, whenever the session says
     * the switch changed.
     */
    void refreshSound() {
        final int kind = session.isClosed()
                ? Backend.SOUND_SWITCH_NONE : session.backend().soundSwitch();
        soundRow.setVisibility(kind == Backend.SOUND_SWITCH_NONE ? View.GONE : View.VISIBLE);
        if (kind != soundKind) {
            // Rebuilt rather than relabelled: a session that withdraws an
            // Enable switch may offer a Mute one later, and the label states
            // what the switch does.
            soundKind = kind;
            soundOptions = null;
            soundRow.removeAllViews();
            if (kind != Backend.SOUND_SWITCH_NONE) {
                soundOptions = soundSwitch(kind == Backend.SOUND_SWITCH_ENABLE);
            }
        } else if (soundOptions != null) {
            soundOptions.refresh();
        }
        if (soundOptions != null) {
            soundOptions.setEnabled(connected());
        }
    }

    private PanelOptions soundSwitch(boolean enable) {
        final BackendOption option = BackendOption.bool("", // no backend owns this one
                activity.getString(enable ? R.string.panel_sound_enable : R.string.panel_sound_mute),
                null, false, BackendOption.Scope.CONNECTION, true);
        return new PanelOptions(soundRow, List.of(option), new PanelOptions.Values() {
            @Override
            public String get(BackendOption o) {
                final Backend b = session.backend();
                return Boolean.toString(enable ? b.soundEnabled() : b.muted());
            }

            @Override
            public void set(BackendOption o, String value) {
                final Backend b = session.backend();
                if (enable) {
                    b.setSoundEnabled(Boolean.parseBoolean(value));
                } else {
                    b.setMuted(Boolean.parseBoolean(value));
                }
            }
        });
    }

    // ---- the far end's displays ----------------------------------------------

    /**
     * Which screen the far end is sending, for the protocol that sends one at a
     * time (RustDesk). Live session state like {@link #refreshResize}, and
     * hidden where the far end has a single screen or reports none.
     *
     * <p>Its value is the display the session <em>has</em>, polled each time:
     * a switch is only a request, and if the peer sends a different screen
     * than the one tapped, the selection moves to that one.
     */
    private void refreshDisplays() {
        final Backend backend = session.backend();
        final List<Monitor> displays = polled.displays();
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
                return String.valueOf(polled.display());
            }

            @Override
            public void set(BackendOption o, String value) {
                session.backend().requestDisplay(Integer.parseInt(value));
            }
        });
    }

    // ---- the desktop size ----------------------------------------------------

    /**
     * Not one of the backend's {@link BackendOption}s, because whether it can be
     * offered at all is a fact about the live connection rather than a
     * description of the backend: a VNC server announces that it accepts a size
     * by sending a rectangle, which may never arrive. So this checks every poll
     * and the row appears once the answer is yes.
     *
     * <p>Its value is the size the desktop <em>has</em>, read from the backend
     * rather than remembered from the request. A server may refuse or grant a
     * different size, which this cannot distinguish, so the control shows the
     * resulting size, and after a refusal the selection stays where it was.
     */
    private void refreshResize() {
        final boolean can = polled.canResize();
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
        // Through the session each time rather than through a captured
        // backend: a reconnection from the footer creates a new one, and this
        // row outlives it.
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
     * The choices: this phone's window first, since fitting it is the main
     * use; then the desktop's current size, so the current state has its own
     * entry; then common sizes, matching the RDP backends' list.
     */
    private List<BackendOption.Choice> sizeChoices() {
        final List<BackendOption.Choice> choices = new ArrayList<>();
        final int[] device = WindowFollower.deviceSize(activity);
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
     * An option's current value, including the session layer.
     *
     * @see Options#live
     */
    private String value(BackendOption o) {
        return Options.live(session.liveOption(o.key()), connectedWith, o);
    }

    // ---- the facts ----------------------------------------------------------

    /**
     * The backend's facts, rebuilt only when their <em>shape</em> changes. A
     * poll that re-inflated every row each second would flicker, and usually
     * only a few values change.
     */
    private void setFacts(List<ConnectionFact> all) {
        final List<ConnectionFact> rows = new ArrayList<>();
        desktopName = null;
        for (ConnectionFact f : all) {
            if (f.field() == ConnectionFact.Field.DESKTOP_NAME) {
                // Shown as the panel's heading, not as a row.
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
        tunnel.removeAllViews();
        factValues.clear();
        final LayoutInflater inf = LayoutInflater.from(dialog.getContext());
        int plain = 0;
        int diags = 0;
        int tunnels = 0;
        for (ConnectionFact f : rows) {
            final ViewGroup into = f.field() == ConnectionFact.Field.TUNNEL ? tunnel
                    : f.diagnostic() ? diagnostics : facts;
            final View row = inf.inflate(R.layout.item_fact, into, false);
            ((TextView) row.findViewById(R.id.label)).setText(f.label());
            final TextView value = row.findViewById(R.id.value);
            value.setText(display(f));
            into.addView(row);
            factValues.add(value);
            if (into == tunnel) {
                tunnels++;
            } else if (f.diagnostic()) {
                diags++;
            } else {
                plain++;
            }
        }
        RowGroups.apply(facts);
        RowGroups.apply(diagnostics);
        RowGroups.apply(tunnel);
        facts.setVisibility(plain == 0 ? View.GONE : View.VISIBLE);
        diagnostics.setVisibility(diags == 0 ? View.GONE : View.VISIBLE);
        tunnel.setVisibility(tunnels == 0 ? View.GONE : View.VISIBLE);
    }

    /**
     * A field the backend has but no value for yet is shown as not set rather
     * than hidden: the panel's layout depends on the protocol, not on how much
     * the server has sent so far.
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
