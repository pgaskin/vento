// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.annotation.SuppressLint;
import android.app.Activity;
import android.os.Bundle;
import android.view.LayoutInflater;
import android.view.View;
import android.view.ViewGroup;
import android.view.WindowInsets;
import android.view.WindowInsetsController;
import android.view.WindowManager;
import android.widget.TextView;
import android.widget.Toast;

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
import java.util.Objects;
import java.util.UUID;
import java.util.stream.Collectors;

/**
 * The connection editor: a full-screen dialog over the home screen, with a
 * Save button.
 *
 * <p>A preference row writes as it is touched, which is why this is not a
 * {@code PreferenceFragment} and why it can have a Save button: every row here
 * writes into a map, the maps reach {@link Connections} only from {@link #save},
 * and leaving with changes asks first.
 *
 * <ol>
 *   <li><b>Values are edited in place</b>, as in {@link ConnectionPanel}. A
 *       choice opens a menu under its own row and text is typed in the field
 *       itself, rather than in a dialog over the form as on a preference
 *       screen. The exceptions are choices that need a description each, such
 *       as the backend: two clients of the same protocol differ in what they
 *       are for, and a menu only has room for a label.
 *   <li><b>The record's own fields are described the way a backend describes
 *       its options</b>, so the whole form is one list and {@link PanelOptions}
 *       does not distinguish the two. They are still kept in
 *       {@link #record separate maps}, because a backend may have a parameter
 *       called {@code name} and the two must not collide.
 *   <li><b>Rotation is handled by hand.</b> A dialog does not survive it, so the
 *       activity keeps {@link #saveState} in its own instance state and shows
 *       the editor again.
 * </ol>
 */
final class ConnectionEditorPanel {

    /** The record's own fields, as keys in {@link #record}. */
    private static final String NAME = "name";
    private static final String ADDRESS = "address";
    private static final String USER = "user";
    private static final String PASSWORD = "password";
    private static final String BACKEND = "backend";

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
    private final MaterialButton homeScreen;
    /** Whether the launcher accepts pinned shortcuts, checked once: it cannot
     * change while the editor is open, and checking is a binder call. */
    private final boolean canPin;
    /** Whether this connection is pinned, which the toggle beside Save shows
     * and sets; see its listener for why it does not wait for Save. */
    private boolean pinned;

    private String id;
    private boolean existing;

    /** The connection's own fields, and the backend's options, being edited. */
    private final Map<String, String> record = new LinkedHashMap<>();
    private final Map<String, String> chosen = new LinkedHashMap<>();

    /** Both maps as they were when opened, to detect changes by comparison. */
    private final Map<String, String> openedRecord = new LinkedHashMap<>();
    private final Map<String, String> openedChosen = new LinkedHashMap<>();

    private PanelOptions fieldRows;
    /** The address field's current label, so a change to it can rebuild. */
    private String addressLabel;
    private PanelOptions optionRows;

    /**
     * @param id        an existing connection, or null for a new one
     * @param onChanged run after anything is written, so the list behind the
     *                  editor can show the change
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
        // After the fields, because both depend on whether the connection
        // already exists, and the constructor was given no id.
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
        // Encrypted when saved and decrypted when restored: a saved instance
        // state is passed to the system process and kept there for the life of
        // the task, and a rotation is no reason to keep a password there in the
        // clear.
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
        // The editor's confirmations are separate dialogs, and one still
        // showing when the activity is destroyed is a leaked window.
        for (AlertDialog d : new ArrayList<>(asked)) {
            d.dismiss();
        }
        asked.clear();
        dismissSheet();
    }

    /**
     * Dismisses the editor after hiding the keyboard.
     *
     * <p>The keyboard is a <em>request</em> the window holds, and a window
     * dismissed while still holding one passes it on: the insets controls
     * change owner, the new owner re-applies the request, and the keyboard
     * appears for a frame over the screen behind before it is hidden again. So
     * every way out of the editor goes through here, and nothing calls
     * {@code dialog.dismiss()} directly.
     *
     * <p>Focus first, then the keyboard: a focused field keeps requesting it.
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
    // fit system windows and applies the IME inset itself. This dialog must not
    // do that, since it would apply the keyboard twice; see the inset listener
    // below.
    //
    // The form's root has no parent to be measured against either: the dialog's
    // window supplies the layout parameters and the ones in the file are not read.
    @SuppressWarnings("deprecation")
    @SuppressLint("InflateParams")
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
        // A stray tap outside must not close the form and lose what was typed.
        // It closes with the close button, Save, or back, and back asks first.
        // It stays cancellable, because that routes back through the dispatcher
        // below rather than straight to Dialog.cancel.
        dialog.setCanceledOnTouchOutside(false);
        if (dialog.getWindow() != null) {
            dialog.getWindow().setLayout(WindowManager.LayoutParams.MATCH_PARENT,
                    WindowManager.LayoutParams.MATCH_PARENT);
            // UNCHANGED rather than the UNSPECIFIED that setting ADJUST_RESIZE
            // alone leaves: with unspecified the system decides, and this form's
            // keyboard should only open when one of its fields is tapped.
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
        // Its own window, so it inherits no system bar insets, and it fills the
        // screen, so both the status bar and the navigation bar are padded. The
        // keyboard is *not*: the window is already resized to sit above it
        // (SOFT_INPUT_ADJUST_RESIZE above), so adding the IME inset as padding
        // would take a second keyboard's height out of the form.
        ViewCompat.setOnApplyWindowInsetsListener(content, (v, insets) -> {
            final Insets bars = insets.getInsets(WindowInsetsCompat.Type.systemBars());
            v.setPadding(v.getPaddingLeft(), bars.top, v.getPaddingRight(), bars.bottom);
            return insets;
        });

        title = content.findViewById(R.id.title);
        delete = content.findViewById(R.id.delete);
        delete.setOnClickListener(v -> confirmDelete());
        homeScreen = content.findViewById(R.id.home_screen);
        canPin = Shortcuts.canPin(activity);
        homeScreen.setTooltipText(activity.getString(R.string.editor_home_screen));
        homeScreen.setOnClickListener(v -> addToHomeScreen());
        pin = content.findViewById(R.id.pin);
        pin.setOnClickListener(v -> {
            // Written immediately rather than held until Save: pinning is about
            // where a card sits in the list, not about the connection itself,
            // so it should not be lost by a cancel or count as an unsaved
            // change.
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
     * Everything that depends on whether this connection exists yet: the title,
     * and the delete, pin and home screen buttons. These change when a new
     * connection is saved, and have to be re-applied to an editor rebuilt from
     * a bundle by {@link #restore}.
     */
    private void applyIdentity() {
        title.setText(existing ? R.string.editor_edit : R.string.editor_new);
        delete.setVisibility(existing ? View.VISIBLE : View.GONE);
        // Nothing to pin until the record exists, as with delete.
        pin.setVisibility(existing ? View.VISIBLE : View.GONE);
        // The checked state rather than a second icon: the button shows
        // "pinned", and the toggle state shows whether it is.
        pin.setChecked(pinned);
        pin.setContentDescription(activity.getString(
                pinned ? R.string.home_unpin : R.string.home_pin));
        pin.setTooltipText(pin.getContentDescription());
        // Needs a record like the other two, and a launcher that accepts
        // pinned shortcuts, since otherwise the button can only fail.
        homeScreen.setVisibility(existing && canPin ? View.VISIBLE : View.GONE);
    }

    /**
     * Offers this connection to the launcher as a home screen icon.
     *
     * <p>Uses the stored record rather than the form, as the credentials button
     * does: the launcher gets a name and an address, and half-typed ones are not
     * ready. Nothing is lost: {@link Shortcuts} rewrites every shortcut whenever
     * the list is written, so a later Save updates the icon.
     *
     * <p>No message on success: the system shows its own dialog asking whether
     * to add it.
     */
    private void addToHomeScreen() {
        final Connection stored = Connections.byId(activity, id);
        if (stored == null) {
            return;
        }
        if (!Shortcuts.requestPin(activity, stored)) {
            Toast.makeText(activity, R.string.editor_home_screen_failed, Toast.LENGTH_SHORT)
                    .show();
        }
    }

    // ---- the credentials ----------------------------------------------------

    /** Whether there is anything to clear, which enables the button. */
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
     * Clears the user name and the password on the stored record.
     *
     * <p>Written immediately rather than held until Save, and confirmed first:
     * this is the only control here that deletes something, so it must not
     * happen by accident, and must not be undone by a Discard meant for the
     * fields above it. It writes the stored record with two fields emptied,
     * not one built from this form, so an address being edited at the time is
     * neither saved nor lost.
     */
    private void confirmClearCredentials() {
        track(new MaterialAlertDialogBuilder(activity, R.style.ThemeOverlay_RemoteDesktop_Dialog)
                .setTitle(R.string.editor_credentials_title)
                .setMessage(R.string.editor_credentials_message)
                .setNegativeButton(android.R.string.cancel, null)
                .setPositiveButton(R.string.editor_credentials_confirm, (d, w) -> {
                    final Connection stored = Connections.byId(activity, id);
                    if (stored != null) {
                        Connections.save(activity, stored.withCredentials("", ""));
                    }
                    // Both maps, so a form with no other changes still closes
                    // without asking.
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
     * Both halves of the form. Rebuilt whole when the backend changes, since
     * the options below belong to the previously chosen backend.
     */
    private void rebuild() {
        fields.removeAllViews();
        protocol.removeAllViews();
        options.removeAllViews();

        // One reader for two lists, because they are one list with the
        // credentials between them: the connection's identity, its stored
        // credentials, and its backend.
        final PanelOptions.Values recordValues = recordValues();
        fieldRows = new PanelOptions(fields, recordFields(), recordValues);
        final List<BackendOption> protocolField = protocolField();
        new PanelOptions(protocol, protocolField, recordValues);
        protocol.setVisibility(protocolField.isEmpty() ? View.GONE : View.VISIBLE);

        final String backendId = value(BACKEND);
        // What an untouched option would actually be: the backend's own global
        // setting, or the schema's default where nothing has been set. Not the
        // schema default alone, since the editor has to show the effective
        // value.
        final Map<String, String> defaults = Options.inherited(activity, backendId);
        final List<BackendOption> connectionOptions = new ArrayList<>(
                Backends.options(backendId).stream()
                        .filter(o -> o.scope() != BackendOption.Scope.GLOBAL)
                        .map(o -> o.scope() == BackendOption.Scope.LAYERED
                                ? withAppDefault(o, defaults.get(o.key())) : o)
                        .collect(Collectors.toList()));
        // Last, after the backend's own: the app's per-connection options are
        // fewer and less central than the protocol's.
        connectionOptions.addAll(AppOptions.options(activity));
        optionRows = new PanelOptions(options, connectionOptions, optionValues(defaults));
        final int vis = optionRows.isEmpty() ? View.GONE : View.VISIBLE;
        options.setVisibility(vis);
        optionsTitle.setVisibility(vis);
    }

    /**
     * The upper half's reader: the form's own fields, whose values are the
     * record being edited and whose unset state is the schema's default.
     *
     * <p>Two of the keys have side effects as well as being stored, which is
     * why this is not a plain map view.
     */
    private PanelOptions.Values recordValues() {
        return new PanelOptions.Values() {
            @Override
            public String get(BackendOption o) {
                final String v = record.get(o.key());
                return v != null ? v : o.defaultValue();
            }

            @Override
            public void set(BackendOption o, String value) {
                // A row writes back its displayed value when bound, so an
                // untouched form would otherwise fill this map with values the
                // user never set and count as edited when closed. A write that
                // does not change the value is not recorded.
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
                    // Choosing a backend that needs setup starts its setup. The
                    // choice is kept either way: a connection can be saved now
                    // and set up later, and the setup screen returns no result.
                    Backends.isSetup(activity, value, ready -> {
                        if (!ready) {
                            Plugins.setup(activity, value);
                        }
                    });
                }
            }
        };
    }

    /**
     * The lower half's reader: this connection's own values over the inherited
     * ones.
     *
     * @param defaults what {@link Options#inherited} returned for this backend
     *                 at the rebuild that made these rows; the rows are rebuilt
     *                 whenever it could have changed
     */
    private PanelOptions.Values optionValues(Map<String, String> defaults) {
        return new PanelOptions.Values() {
            @Override
            public String get(BackendOption o) {
                final String own = chosen.get(o.key());
                if (own != null) {
                    return own;
                }
                // An unset layered option shows the empty "default" choice
                // rather than the value it would inherit: that is the row's
                // first choice, and its label names the inherited value.
                // getOrDefault, for an app option: no backend provides a
                // default for one, so the schema's default applies.
                return o.scope() == BackendOption.Scope.LAYERED
                        ? "" : defaults.getOrDefault(o.key(), o.defaultValue());
            }

            @Override
            public void set(BackendOption o, String value) {
                // The same rule as the fields above, against this row's own
                // unset value, which for a layered option is the empty string
                // rather than the schema's default.
                if (chosen.containsKey(o.key()) || !value.equals(get(o))) {
                    chosen.put(o.key(), value);
                }
                // An option can change what the address field asks for, and a
                // row's label is fixed when the row is built, so the form is
                // rebuilt when the label changes.
                if (!Objects.equals(addressLabel, Backends.addressLabel(value(BACKEND), chosen))) {
                    rebuild();
                }
            }

            @Override
            public String get(String key) {
                final String own = chosen.get(key);
                return own != null && !own.isEmpty() ? own : defaults.get(key);
            }
        };
    }

    /**
     * A layered option as the editor offers it: its own values after an
     * explicit "default" entry, whose label names the inherited value.
     *
     * <p>Without that entry, a value would be stored only where it differs from
     * the backend's, so choosing the same value as the backend would silently
     * follow later changes to it. The empty value is the default entry, and a
     * switch becomes a menu so it can have three states.
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
            // An empty field is already the unset case, and its default is in
            // the helper text under it.
            case INT, TEXT -> {
                return o;
            }
        }
        return new BackendOption(o.key(), o.label(), o.summary(), BackendOption.Type.CHOICE,
                List.copyOf(choices), "", o.scope(), o.live(), o.gate());
    }

    /**
     * The record's own fields, described the way a backend describes an option,
     * so the form is one list drawn by one renderer rather than a hand-built
     * form above a generated one.
     */
    private List<BackendOption> recordFields() {
        final List<BackendOption> out = new ArrayList<>();
        // No summaries: the labels say what goes in each field.
        out.add(BackendOption.text(NAME, activity.getString(R.string.editor_name),
                null, "", BackendOption.Scope.CONNECTION, false));
        // The backend's label where it has one: one protocol here can reach a
        // machine by an ID rather than a host and port, depending on an option
        // further down.
        addressLabel = Backends.addressLabel(value(BACKEND), chosen);
        out.add(BackendOption.text(ADDRESS,
                addressLabel != null ? addressLabel : activity.getString(R.string.editor_address),
                null, "", BackendOption.Scope.CONNECTION, false));
        // No user name and no password fields: both are kept in `record` and
        // written back unchanged by save(). The credentials button is the only
        // control for them.
        return out;
    }

    /**
     * The backend picker, as a list so that it can be empty: with one backend
     * there is nothing to choose.
     *
     * <p>A separate list rather than the last of {@link #recordFields}, because
     * the credentials are rendered between the two.
     *
     * <p>Every backend has a description, so this row opens a dialog: two
     * clients of the same protocol differ in what each is for, which a menu of
     * names alone does not show.
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

    /** True if nothing would be lost by closing. */
    private boolean unchanged() {
        return record.equals(openedRecord) && chosen.equals(openedChosen);
    }

    /**
     * Closing without Save. An untouched form just closes; an edited one asks,
     * because the user may not have meant either to save or to discard.
     */
    private void close() {
        if (unchanged()) {
            dismissSheet();
            return;
        }
        if (value(ADDRESS).isEmpty()) {
            // Without an address it cannot be saved, so the only question is
            // whether to discard.
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

    /** @return whether it was written; false leaves the editor open on the problem */
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
        // Only values the user set, and only this backend's options: the map
        // may still hold values chosen before the backend was changed. For a
        // layered option, `chosen` holds exactly what the row shows: empty means
        // the default and stores nothing, and any other value is stored even if
        // it matches the default, so it stays chosen when the default changes.
        // For the rest, an untouched row still equals the default it was drawn
        // with.
        final Map<String, String> defaults = Options.inherited(activity, backendId);
        final Map<String, String> options = new LinkedHashMap<>();
        final List<BackendOption> saveable = new ArrayList<>(Backends.options(backendId));
        // The app's own per-connection options are stored in the same map, so
        // the same loop writes them; their default is the schema's, since no
        // backend provides one.
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
        // The form does not hold the pin, so it is read from the stored record:
        // a record written from these fields alone would unpin the connection.
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
