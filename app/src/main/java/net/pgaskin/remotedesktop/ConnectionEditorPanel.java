// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.app.Activity;
import android.content.SharedPreferences;
import android.os.Bundle;
import android.view.LayoutInflater;
import android.view.View;
import android.view.ViewGroup;
import android.view.WindowInsets;
import android.view.WindowInsetsController;
import android.view.WindowManager;
import android.widget.TextView;

import androidx.activity.ComponentDialog;
import androidx.activity.OnBackPressedCallback;
import androidx.appcompat.app.AlertDialog;

import com.google.android.material.dialog.MaterialAlertDialogBuilder;
import androidx.core.graphics.Insets;
import androidx.core.view.ViewCompat;
import androidx.core.view.WindowInsetsCompat;

import com.google.android.material.button.MaterialButton;
import com.google.android.material.textfield.TextInputLayout;

import net.pgaskin.remotedesktop.backend.BackendOption;
import net.pgaskin.remotedesktop.backend.Backends;

import org.json.JSONObject;

import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.UUID;

/**
 * The connection editor: one sheet over the home screen, with a Save.
 *
 * <p>A preference row writes as it is touched, which is why this is not a
 * {@code PreferenceFragment} and why it can have a Save at all: every row here
 * writes into a map, the maps reach {@link Connections} only from {@link #save},
 * and leaving with changes asks first. Three things follow from being a sheet
 * rather than a screen:
 *
 * <ol>
 *   <li><b>Nothing opens a second window</b> — the same rule
 *       {@link ConnectionPanel} follows. A choice drops a menu under its own
 *       row and text is typed in the field itself, where the preference screen
 *       opened a dialog over the very form the value belongs to. The protocol
 *       is the exception, and it is one because its values need a line of prose
 *       each: two clients of the same protocol differ in what they are for, and
 *       a menu has room for a label and nothing else.
 *   <li><b>The record's own fields are described the way a backend describes
 *       its options</b>, so the whole form is one list and {@link PanelOptions}
 *       never learns which half it is drawing. They stay in
 *       {@link #record separate maps} all the same, because a backend is free to
 *       have a parameter called {@code name} and the two must not share a slot.
 *   <li><b>It survives a rotation by hand.</b> A dialog does not, so the
 *       activity keeps {@link #saveState} in its own instance state and shows
 *       the sheet again.
 * </ol>
 */
final class ConnectionEditorPanel {

    /** The record's own fields, as keys in {@link #record}. */
    private static final String NAME = "name";
    private static final String ADDRESS = "address";
    private static final String USER = "user";
    private static final String PASSWORD = "password";
    private static final String BACKEND = "backend";

    /** A sheet on a phone; a centred card once there is room for one. */

    private final Activity activity;
    private final Runnable onChanged;
    private final ComponentDialog dialog;

    private final ViewGroup fields;
    private final ViewGroup protocol;
    private final ViewGroup options;
    private final View optionsTitle;
    private final TextView title;
    private final MaterialButton delete;
    private final MaterialButton credentials;
    private final TextView credentialsSummary;
    private final MaterialButton pin;
    /** Whether this connection is pinned, which the toggle beside Save both
     * shows and sets; see the listener for why it does not wait for Save. */
    private boolean pinned;

    private String id;
    private boolean existing;

    /** The connection's own fields, and the backend's options, being edited. */
    private final Map<String, String> record = new LinkedHashMap<>();
    private final Map<String, String> chosen = new LinkedHashMap<>();

    /** What both looked like when it was opened, so "changed" is a comparison. */
    private final Map<String, String> openedRecord = new LinkedHashMap<>();
    private final Map<String, String> openedChosen = new LinkedHashMap<>();

    private PanelOptions fieldRows;
    private PanelOptions optionRows;

    /**
     * @param id        an existing connection, or null for a new one
     * @param onChanged run after anything is written — the list behind the sheet
     *                  is showing what was just edited
     */
    static ConnectionEditorPanel show(Activity activity, String id, Runnable onChanged) {
        final ConnectionEditorPanel panel = new ConnectionEditorPanel(activity, id, onChanged);
        panel.dialog.show();
        return panel;
    }

    /** Re-open one across a rotation, from {@link #saveState}. */
    static ConnectionEditorPanel restore(Activity activity, Bundle state, Runnable onChanged) {
        final ConnectionEditorPanel panel = new ConnectionEditorPanel(activity, null, onChanged);
        panel.id = state.getString("id", panel.id);
        panel.existing = state.getBoolean("existing", false);
        replace(panel.record, fromJson(state.getString("record", "{}")));
        panel.record.put(PASSWORD, Secrets.reveal(panel.record.get(PASSWORD)));
        replace(panel.chosen, fromJson(state.getString("chosen", "{}")));
        replace(panel.openedRecord, fromJson(state.getString("openedRecord", "{}")));
        replace(panel.openedChosen, fromJson(state.getString("openedChosen", "{}")));
        // After the fields, because both of these are answers to "is this a
        // connection that already exists" and the constructor was told no.
        panel.applyIdentity();
        panel.applyCredentials();
        panel.rebuild();
        panel.dialog.show();
        return panel;
    }

    Bundle saveState() {
        final Bundle b = new Bundle();
        b.putString("id", id);
        b.putBoolean("existing", existing);
        // Sealed on the way out and opened on the way back in: a saved instance
        // state crosses into the system process and stays there for the life of
        // the task, and a rotation is not one of the two moments this app has
        // decided a password may be in the clear.
        final Map<String, String> saved = new LinkedHashMap<>(record);
        final String sealed = Secrets.protect(saved.get(PASSWORD));
        saved.put(PASSWORD, sealed == null ? "" : sealed);
        b.putString("record", new JSONObject(saved).toString());
        b.putString("chosen", new JSONObject(chosen).toString());
        b.putString("openedRecord", new JSONObject(openedRecord).toString());
        b.putString("openedChosen", new JSONObject(openedChosen).toString());
        return b;
    }

    boolean isShowing() {
        return dialog.isShowing();
    }

    void dismiss() {
        // The questions this sheet asks are dialogs of their own, and one still
        // showing when the activity goes is a leaked window.
        for (AlertDialog d : new ArrayList<>(asked)) {
            d.dismiss();
        }
        asked.clear();
        dismissSheet();
    }

    /**
     * Go away, having first put the keyboard down.
     *
     * <p>The keyboard is not this window's decoration, it is a <em>request</em>
     * the window is holding, and a window dismissed while still holding one
     * hands it on: the insets controls change owner, whoever receives them
     * re-applies the request that arrived with them, and the keyboard appears
     * for a frame over the screen behind before something else takes it away
     * again. So every way out of this sheet goes through here, and nothing calls
     * {@code dialog.dismiss()} directly.
     *
     * <p>Focus first, then the keyboard: a field that still has focus is a
     * window that still wants one.
     */
    private void dismissSheet() {
        final View focused = dialog.getCurrentFocus();
        if (focused != null) {
            focused.clearFocus();
        }
        final android.view.Window w = dialog.getWindow();
        final WindowInsetsController insets = w == null ? null : w.getInsetsController();
        if (insets != null) {
            insets.hide(WindowInsets.Type.ime());
        }
        dialog.dismiss();
    }

    private final List<AlertDialog> asked = new ArrayList<>();

    private AlertDialog track(AlertDialog d) {
        asked.add(d);
        d.setOnDismissListener(x -> asked.remove(d));
        return d;
    }

    // SOFT_INPUT_ADJUST_RESIZE is deprecated in favour of a window that does not
    // fit system windows and applies the IME inset itself. That is what this
    // sheet must not do: it would apply the keyboard twice — see the inset
    // listener below.
    @SuppressWarnings("deprecation")
    private ConnectionEditorPanel(Activity activity, String id, Runnable onChanged) {
        this.activity = activity;
        this.onChanged = onChanged;

        final Connection conn = id == null ? null : Connections.byId(activity, id);
        this.existing = conn != null;
        this.id = conn != null ? conn.id() : UUID.randomUUID().toString();
        this.pinned = conn != null && conn.pinned();
        if (conn != null) {
            record.put(NAME, conn.name());
            record.put(ADDRESS, conn.address());
            record.put(USER, conn.userName());
            record.put(PASSWORD, conn.password());
            record.put(BACKEND, conn.backendId());
            chosen.putAll(conn.options());
        } else {
            record.put(BACKEND, Backends.ids().isEmpty() ? "" : Backends.ids().get(0));
        }
        openedRecord.putAll(record);
        openedChosen.putAll(chosen);

        dialog = new ComponentDialog(activity, R.style.Theme_RemoteDesktop_EditorDialog);
        final View content = LayoutInflater.from(dialog.getContext())
                .inflate(R.layout.sheet_editor, null);
        dialog.setContentView(content);
        // A form is not left by a stray tap outside it: that is how typing gets
        // lost. What leaves is the ✕, Save, and back — and back asks.
        // Cancellable it stays, because that is what routes back through the
        // dispatcher below rather than straight to Dialog.cancel.
        dialog.setCanceledOnTouchOutside(false);
        if (dialog.getWindow() != null) {
            dialog.getWindow().setLayout(WindowManager.LayoutParams.MATCH_PARENT,
                    WindowManager.LayoutParams.MATCH_PARENT);
            // UNCHANGED rather than the UNSPECIFIED that setting ADJUST_RESIZE
            // alone leaves behind: unspecified is an invitation for the system
            // to decide, and the only thing that should raise this form's
            // keyboard is a finger on one of its fields.
            dialog.getWindow().setSoftInputMode(
                    WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE
                            | WindowManager.LayoutParams.SOFT_INPUT_STATE_UNCHANGED);
        }
        dialog.getOnBackPressedDispatcher().addCallback(new OnBackPressedCallback(true) {
            @Override
            public void handleOnBackPressed() {
                close();
            }
        });
        // Its own window, so it gets no system bars for free — and now it fills
        // the screen, so the status bar is in the sum as well as the navigation
        // one. The keyboard is *not*: the window is already resized to sit above
        // it (SOFT_INPUT_ADJUST_RESIZE above), so adding the IME inset as
        // padding would take a second keyboard's height out of the form.
        ViewCompat.setOnApplyWindowInsetsListener(content, (v, insets) -> {
            final Insets bars = insets.getInsets(WindowInsetsCompat.Type.systemBars());
            v.setPadding(v.getPaddingLeft(), bars.top, v.getPaddingRight(), bars.bottom);
            return insets;
        });

        title = content.findViewById(R.id.title);
        delete = content.findViewById(R.id.delete);
        delete.setOnClickListener(v -> confirmDelete());
        pin = content.findViewById(R.id.pin);
        pin.setOnClickListener(v -> {
            // Written straight through rather than held until Save: pinning is
            // about where a card sits in the list and not about what the
            // connection *is*, so it has no business being lost by a cancel or
            // being one of the things an unsaved-changes question is asking
            // about.
            pinned = !pinned;
            Connections.setPinned(activity, java.util.Set.of(id), pinned);
            applyIdentity();
            if (onChanged != null) {
                onChanged.run();
            }
        });
        applyIdentity();
        content.findViewById(R.id.close).setOnClickListener(v -> close());
        content.findViewById(R.id.save).setOnClickListener(v -> {
            if (save()) {
                dismissSheet();
            }
        });

        credentials = content.findViewById(R.id.credentials);
        credentialsSummary = content.findViewById(R.id.credentials_summary);
        credentials.setOnClickListener(v -> confirmClearCredentials());
        applyCredentials();

        fields = content.findViewById(R.id.fields);
        protocol = content.findViewById(R.id.protocol);
        options = content.findViewById(R.id.options);
        optionsTitle = content.findViewById(R.id.options_title);
        rebuild();
    }

    /**
     * The two things that depend on whether this connection exists yet: what the
     * sheet is called, and whether there is anything to delete. Both move when a
     * new connection is saved, and both have to be re-applied to a sheet rebuilt
     * from a bundle by {@link #restore}.
     */
    private void applyIdentity() {
        title.setText(existing ? R.string.editor_edit : R.string.editor_new);
        delete.setVisibility(existing ? View.VISIBLE : View.GONE);
        // Nothing to pin until there is a record to pin, which is the same
        // condition the delete has and for the same reason.
        pin.setVisibility(existing ? View.VISIBLE : View.GONE);
        // Checked rather than a second icon: the button is "pinned", and what a
        // press does to it is the toggle's business rather than the icon's.
        pin.setChecked(pinned);
        pin.setContentDescription(activity.getString(
                pinned ? R.string.home_unpin : R.string.home_pin));
    }

    // ---- the credentials ----------------------------------------------------

    /** Whether there is anything to clear, which is what the button reads. */
    private boolean hasCredentials() {
        return !value(USER).isEmpty() || !value(PASSWORD).isEmpty();
    }

    private void applyCredentials() {
        final boolean any = hasCredentials();
        credentials.setEnabled(any);
        credentialsSummary.setText(any
                ? R.string.editor_credentials_saved : R.string.editor_credentials_none);
    }

    /**
     * Forget the user name and the password, on the record as it is stored.
     *
     * <p>Written straight through rather than held until Save, and asked about
     * first, which are two halves of one decision: this is the only control here
     * that destroys something, so it must not be reachable by accident and must
     * not be undone by a Discard that the person leaving meant to apply to the
     * fields above it. The record it writes is {@link Connections}' own with two
     * fields emptied — not one built from this form — so an address being edited
     * at the time is neither saved nor lost.
     */
    private void confirmClearCredentials() {
        track(new MaterialAlertDialogBuilder(activity, R.style.ThemeOverlay_RemoteDesktop_Dialog)
                .setTitle(R.string.editor_credentials_title)
                .setMessage(R.string.editor_credentials_message)
                .setNegativeButton(android.R.string.cancel, null)
                .setPositiveButton(R.string.editor_credentials_confirm, (d, w) -> {
                    final Connection stored = Connections.byId(activity, id);
                    if (stored != null) {
                        Connections.save(activity, new Connection(stored.id(), stored.name(),
                                stored.backendId(), stored.address(), "", "",
                                stored.options(), stored.pinned()));
                    }
                    // Both maps, so that a form which has changed nothing else
                    // still leaves without a question.
                    record.put(USER, "");
                    record.put(PASSWORD, "");
                    openedRecord.put(USER, "");
                    openedRecord.put(PASSWORD, "");
                    applyCredentials();
                })
                .show());
    }

    // ---- the rows -----------------------------------------------------------

    /**
     * Both halves of the form. Rebuilt whole when the protocol changes, since
     * the options below it belong to the backend that was chosen before.
     */
    private void rebuild() {
        fields.removeAllViews();
        protocol.removeAllViews();
        options.removeAllViews();

        // One reader for two lists, because they are one list with the
        // credentials between them: what this connection is, what is kept for
        // it, and what speaks to it.
        final PanelOptions.Values recordValues = new PanelOptions.Values() {
            @Override
            public String get(BackendOption o) {
                final String v = record.get(o.key());
                return v != null ? v : o.defaultValue();
            }

            @Override
            public void set(BackendOption o, String value) {
                // A row writes back what it was shown, merely by being bound —
                // so a form nobody has touched would otherwise fill this map
                // with answers nobody gave, and read as edited on the way out.
                // A write that does not change the answer is not recorded.
                if (record.containsKey(o.key()) || !value.equals(o.defaultValue())) {
                    record.put(o.key(), value);
                }
                if (o.key().equals(ADDRESS)) {
                    final TextInputLayout field = fieldRows.fieldFor(ADDRESS);
                    if (field != null) {
                        field.setError(null);
                    }
                }
                if (o.key().equals(BACKEND)) {
                    rebuild();
                }
            }
        };
        fieldRows = new PanelOptions(fields, recordFields(), recordValues);
        final List<BackendOption> protocolField = protocolField();
        new PanelOptions(protocol, protocolField, recordValues);
        protocol.setVisibility(protocolField.isEmpty() ? View.GONE : View.VISIBLE);

        final String backendId = value(BACKEND);
        // What an untouched option would actually do: the backend's own global
        // setting, or the schema's default where nothing has ever been set. Not
        // the schema default alone — the editor has to show what would happen,
        // rather than what the schema says.
        final Map<String, String> defaults = defaults(backendId);
        final List<BackendOption> connectionOptions = new ArrayList<>(
                Backends.options(backendId).stream()
                        .filter(o -> o.scope() != BackendOption.Scope.GLOBAL)
                        .map(o -> o.scope() == BackendOption.Scope.LAYERED
                                ? withAppDefault(o, defaults.get(o.key())) : o)
                        .toList());
        // Last, and after the backend's own: what the app wants to know per
        // machine is a smaller category than what a protocol does, and it reads
        // as an afterthought because it is one.
        connectionOptions.addAll(AppOptions.options(activity));
        optionRows = new PanelOptions(options, connectionOptions, new PanelOptions.Values() {
            @Override
            public String get(BackendOption o) {
                final String own = chosen.get(o.key());
                if (own != null) {
                    return own;
                }
                // A layered option that has not been answered shows the answer
                // it has not given, rather than the one it would inherit: the
                // row's first choice is that, and it says where it comes from.
                // getOrDefault, for an app option: there is no backend under
                // one to have a default of its own, so the schema's is it.
                return o.scope() == BackendOption.Scope.LAYERED
                        ? "" : defaults.getOrDefault(o.key(), o.defaultValue());
            }

            @Override
            public void set(BackendOption o, String value) {
                // The same rule as the fields above, against this row's own
                // idea of unanswered — which for a layered option is the empty
                // string it shows rather than the schema's default.
                if (chosen.containsKey(o.key()) || !value.equals(get(o))) {
                    chosen.put(o.key(), value);
                }
            }

            @Override
            public String get(String key) {
                final String own = chosen.get(key);
                return own != null && !own.isEmpty() ? own : defaults.get(key);
            }
        });
        final int vis = optionRows.isEmpty() ? View.GONE : View.VISIBLE;
        options.setVisibility(vis);
        optionsTitle.setVisibility(vis);
    }

    /**
     * A layered option as the editor offers it: its own answers under an
     * explicit unanswered one, which names what the backend would do.
     *
     * <p>The alternative is what this was first — no such entry, and an answer
     * stored only where it differs from the backend's — which reads the same
     * until the backend's answer moves: agreeing with it silently meant
     * following it. Empty is the answer that is not one, and a switch has to
     * become a menu to have three states.
     */
    private BackendOption withAppDefault(BackendOption o, String inherited) {
        final List<BackendOption.Choice> choices = new ArrayList<>();
        final String shown;
        switch (o.type()) {
            case BOOL -> {
                shown = activity.getString(Boolean.parseBoolean(inherited)
                        ? R.string.editor_option_on : R.string.editor_option_off);
                choices.add(new BackendOption.Choice("",
                        activity.getString(R.string.editor_option_default, shown)));
                choices.add(new BackendOption.Choice("true",
                        activity.getString(R.string.editor_option_on)));
                choices.add(new BackendOption.Choice("false",
                        activity.getString(R.string.editor_option_off)));
            }
            case CHOICE -> {
                choices.add(new BackendOption.Choice("",
                        activity.getString(R.string.editor_option_default,
                                o.labelFor(inherited))));
                choices.addAll(o.choices());
            }
            // A field left empty is already the unanswered case, and its default
            // is in the helper text under it rather than in a list of one.
            case INT, TEXT -> {
                return o;
            }
        }
        return new BackendOption(o.key(), o.label(), o.summary(), BackendOption.Type.CHOICE,
                List.copyOf(choices), "", o.scope(), o.live(), o.gate());
    }

    /**
     * The record's own fields, described the way a backend describes an option.
     *
     * <p>{@link OptionScreen} already turns two descriptions into one kind of
     * row; this is a third producer of the same description, and what it buys is
     * one list and one renderer rather than a hand-built form above a generated
     * one.
     */
    private List<BackendOption> recordFields() {
        final List<BackendOption> out = new ArrayList<>();
        // No summaries: two fields whose label already says the whole of what
        // goes in them, and the address carries its own format.
        out.add(BackendOption.text(NAME, activity.getString(R.string.editor_name),
                null, "", BackendOption.Scope.CONNECTION, false));
        out.add(BackendOption.text(ADDRESS, activity.getString(R.string.editor_address),
                null, "", BackendOption.Scope.CONNECTION, false));
        // No user name and no password: both are kept in {@link #record} and
        // written back by {@link #save} untouched, but neither is a field here.
        // See the credentials button, which is the whole of what this form does
        // about them.
        return out;
    }

    /**
     * The protocol picker, which is a list so that it can be an empty one: one
     * backend is not a choice, and a picker offering it would be a row that can
     * only say what it already says.
     *
     * <p>Its own list rather than the last of {@link #recordFields}, because it
     * is rendered under the credentials and they are between the two.
     *
     * <p>Every backend carries its own line, so this row is the one that opens a
     * dialog: two clients of the same protocol are told apart by what each is
     * for, and a menu of six names says that to somebody who already knows it
     * and to nobody else.
     */
    private List<BackendOption> protocolField() {
        if (Backends.ids().size() <= 1) {
            return List.of();
        }
        final BackendOption.Choice[] choices = Backends.ids().stream()
                .map(b -> new BackendOption.Choice(b, Backends.name(b),
                        Backends.description(b), null))
                .toArray(BackendOption.Choice[]::new);
        return List.of(BackendOption.choice(BACKEND, activity.getString(R.string.editor_backend),
                null, Backends.ids().get(0), BackendOption.Scope.CONNECTION, false, choices));
    }

    // ---- leaving ------------------------------------------------------------

    /** True once nothing here would be lost by closing. */
    private boolean unchanged() {
        return record.equals(openedRecord) && chosen.equals(openedChosen);
    }

    /**
     * The way out that is not Save. An untouched form just closes; an edited one
     * asks — because silently committing and silently discarding are both
     * answers the person leaving may not have meant to give.
     */
    private void close() {
        if (unchanged()) {
            dismissSheet();
            return;
        }
        if (value(ADDRESS).isEmpty()) {
            // There is nothing to offer to keep: an address is what makes this a
            // connection. So the question is the other one.
            track(new MaterialAlertDialogBuilder(activity, R.style.ThemeOverlay_RemoteDesktop_Dialog)
                    .setTitle(R.string.editor_discard_title)
                    .setMessage(R.string.editor_address_required)
                    .setNegativeButton(R.string.editor_keep_editing, null)
                    .setPositiveButton(R.string.editor_discard, (d, w) -> dismissSheet())
                    .show());
            return;
        }
        track(new MaterialAlertDialogBuilder(activity, R.style.ThemeOverlay_RemoteDesktop_Dialog)
                .setTitle(R.string.editor_keep_title)
                .setMessage(R.string.editor_keep_message)
                .setNegativeButton(R.string.editor_discard, (d, w) -> dismissSheet())
                .setPositiveButton(R.string.editor_save, (d, w) -> {
                    if (save()) {
                        dismissSheet();
                    }
                })
                .show());
    }

    /** @return whether it was written; false leaves the sheet open on the problem */
    private boolean save() {
        if (value(ADDRESS).isEmpty()) {
            final TextInputLayout field = fieldRows.fieldFor(ADDRESS);
            if (field != null) {
                field.setError(activity.getString(R.string.editor_address_required));
                field.requestFocus();
            }
            return false;
        }
        final String backendId = value(BACKEND);
        // Only what somebody chose, and only what this backend understands — the
        // map may still hold whatever was chosen before the protocol was
        // changed. For a layered option, chosen is exactly what the row says it
        // is: an empty answer is the app default and stores nothing, and any
        // other answer is stored even where it agrees with the default, because
        // the point of picking it is that it stays picked when the default
        // moves. For the rest, an untouched row is one that still equals the
        // default it was drawn with.
        final Map<String, String> defaults = defaults(backendId);
        final Map<String, String> options = new LinkedHashMap<>();
        final List<BackendOption> saveable = new ArrayList<>(Backends.options(backendId));
        // The app's own per-connection options ride in the same map, so they are
        // written by the same loop; their default is the schema's, since there
        // is no backend underneath one to have an opinion.
        saveable.addAll(AppOptions.options(activity));
        for (BackendOption o : saveable) {
            if (o.scope() == BackendOption.Scope.GLOBAL) {
                continue;
            }
            final String v = chosen.get(o.key());
            if (v == null || v.isEmpty()) {
                continue;
            }
            final String def = defaults.getOrDefault(o.key(), o.defaultValue());
            if (o.scope() == BackendOption.Scope.LAYERED || !v.equals(def)) {
                options.put(o.key(), v);
            }
        }
        // The form does not carry the pin, so it has to be read rather than
        // rebuilt: a record written from these fields alone would quietly unpin
        // whatever was being edited.
        final Connection prior = Connections.byId(activity, id);
        Connections.save(activity, new Connection(id, value(NAME), backendId, value(ADDRESS),
                value(USER), "", options, prior != null && prior.pinned())
                .withPassword(value(PASSWORD)));
        existing = true;
        applyIdentity();
        replace(openedRecord, record);
        replace(openedChosen, chosen);
        onChanged.run();
        return true;
    }

    private void confirmDelete() {
        track(new MaterialAlertDialogBuilder(activity, R.style.ThemeOverlay_RemoteDesktop_Dialog)
                .setTitle(activity.getString(R.string.home_delete_title,
                        value(NAME).isEmpty() ? value(ADDRESS) : value(NAME)))
                .setMessage(R.string.home_delete_message)
                .setNegativeButton(android.R.string.cancel, null)
                .setPositiveButton(R.string.home_delete, (d, w) -> {
                    Connections.delete(activity, List.of(id));
                    onChanged.run();
                    dismissSheet();
                })
                .show());
    }

    // ---- plumbing -----------------------------------------------------------

    private String value(String key) {
        final String v = record.get(key);
        return v == null ? "" : v;
    }

    /**
     * What each of a backend's options would be for this connection without an
     * override: the same layering {@link Connections#effectiveOptions} does, one
     * layer short, which is why the editor can show it as the default and mean
     * it.
     */
    private Map<String, String> defaults(String backendId) {
        final SharedPreferences global = Connections.backendPrefs(activity, backendId);
        final Map<String, String> out = new LinkedHashMap<>();
        for (BackendOption o : Backends.options(backendId)) {
            out.put(o.key(), global.getString(o.key(), o.defaultValue()));
        }
        return out;
    }

    private static void replace(Map<String, String> map, Map<String, String> with) {
        map.clear();
        map.putAll(with);
    }

    private static Map<String, String> fromJson(String json) {
        final Map<String, String> out = new LinkedHashMap<>();
        try {
            final JSONObject o = new JSONObject(json);
            for (var it = o.keys(); it.hasNext(); ) {
                final String k = it.next();
                out.put(k, o.optString(k));
            }
        } catch (Exception ignored) {
            // A malformed bundle is not worth a crash; the editor opens empty.
        }
        return out;
    }
}
