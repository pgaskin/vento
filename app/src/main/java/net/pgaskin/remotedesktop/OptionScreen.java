// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.content.Context;
import android.content.SharedPreferences;
import android.text.InputType;

import androidx.preference.EditTextPreference;
import androidx.preference.ListPreference;
import androidx.preference.Preference;
import androidx.preference.PreferenceDataStore;
import androidx.preference.PreferenceGroup;
import androidx.preference.SwitchPreferenceCompat;
import androidx.preference.TwoStatePreference;

import net.pgaskin.remotedesktop.backend.BackendOption;
import net.pgaskin.remotedesktop.control.input.Config;

import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.stream.Collectors;

/**
 * Builds preference rows from setting descriptions.
 *
 * <p>Two kinds of description, one builder: {@link BackendOption}, which a
 * backend supplies about itself, and {@link InputSettings.Tunable}, which the
 * app keeps about the input stack. Neither depends on androidx, deliberately:
 * {@code control} may not depend on it at all, and a backend describing its
 * options in terms of {@code SwitchPreferenceCompat} could only be used from
 * an Android app that includes androidx.
 *
 * <h2>Everything is a string</h2>
 * Both descriptions carry string values, so the preference screens store
 * strings, including for switches, whose {@code true}/{@code false} would
 * otherwise be stored as booleans and read back differently from how other
 * values are. The {@link PreferenceDataStore} below makes that uniform.
 */
final class OptionScreen {

    private OptionScreen() {
    }

    /** A store over one preference file, where every value is a string. */
    static Store store(SharedPreferences prefs) {
        return new Store(prefs, true);
    }

    /**
     * The same, over a file whose switch values are booleans: the app's own
     * switches, which {@link AppSettings} reads as booleans.
     */
    static Store switches(SharedPreferences prefs) {
        return new Store(prefs, false);
    }

    /**
     * Where a screen's values are stored, ignoring writes until {@link #built()}
     * says the screen is complete.
     *
     * <p>Ignoring them is deliberate. A preference persists its default as soon
     * as it is bound (androidx has no "show this and store nothing"), so merely
     * opening a screen would store a value for every row on it, and everything
     * here relies on an unset setting being absent: otherwise a backend cannot
     * tell "this asked for full colour" from "never set", a connection that
     * matches its backend's value would no longer follow changes to it, and an
     * improved default in a later version would not reach an install whose file
     * already stores the old one. A write while the rows are being added is the
     * framework filling them in; a write after that is the user's.
     */
    static final class Store extends PreferenceDataStore {

        private final SharedPreferences prefs;
        /** Booleans as {@code "true"}/{@code "false"}, so one file holds one type. */
        private final boolean asStrings;
        private boolean building = true;

        private Store(SharedPreferences prefs, boolean asStrings) {
            this.prefs = prefs;
            this.asStrings = asStrings;
        }

        /** Called once, after the last row has been added. */
        void built() {
            building = false;
        }

        @Override
        public void putString(String key, String value) {
            if (!building) {
                prefs.edit().putString(key, value).apply();
            }
        }

        @Override
        public String getString(String key, String defValue) {
            return prefs.getString(key, defValue);
        }

        /**
         * Removes the value even while building: for a value the screen itself
         * withdraws ({@link #requirements}), which the user did not set.
         */
        void remove(String key) {
            prefs.edit().remove(key).apply();
        }

        @Override
        public void putBoolean(String key, boolean value) {
            if (asStrings) {
                putString(key, Boolean.toString(value));
            } else if (!building) {
                prefs.edit().putBoolean(key, value).apply();
            }
        }

        @Override
        public boolean getBoolean(String key, boolean defValue) {
            return asStrings
                    ? Boolean.parseBoolean(getString(key, Boolean.toString(defValue)))
                    : prefs.getBoolean(key, defValue);
        }
    }

    /** Add every option of one {@link BackendOption.Scope} to a group. */
    static void addOptions(PreferenceGroup group, List<BackendOption> options,
                           BackendOption.Scope scope) {
        final Context ctx = group.getContext();
        for (BackendOption o : options) {
            if (o.scope() != scope) {
                continue;
            }
            final String def = o.defaultValue();
            final Preference p = switch (o.type()) {
                case BOOL -> {
                    final SwitchPreferenceCompat s = new SwitchPreferenceCompat(ctx);
                    s.setDefaultValue(Boolean.parseBoolean(def));
                    yield s;
                }
                case CHOICE -> {
                    final ListPreference l = new ListPreference(ctx);
                    l.setEntries(o.choices().stream()
                            .map(BackendOption.Choice::label).toArray(CharSequence[]::new));
                    l.setEntryValues(o.choices().stream()
                            .map(BackendOption.Choice::value).toArray(CharSequence[]::new));
                    l.setDefaultValue(def);
                    l.setSummaryProvider(pref -> withNote(((ListPreference) pref).getEntry(),
                            o.summary()));
                    l.setDialogTitle(o.label());
                    yield l;
                }
                case INT, TEXT -> {
                    final EditTextPreference e = new EditTextPreference(ctx);
                    e.setDefaultValue(def);
                    e.setDialogTitle(o.label());
                    if (o.type() == BackendOption.Type.INT) {
                        e.setOnBindEditTextListener(t ->
                                t.setInputType(InputType.TYPE_CLASS_NUMBER));
                    }
                    e.setSummaryProvider(pref -> withNote(
                            ((EditTextPreference) pref).getText(), o.summary()));
                    yield e;
                }
            };
            p.setKey(o.key());
            p.setTitle(o.label());
            p.setIconSpaceReserved(false);
            p.setSingleLineTitle(false);
            if (o.type() == BackendOption.Type.BOOL && o.summary() != null) {
                p.setSummary(o.summary());
            }
            group.addPreference(p);
        }
        gates(group, options);
        requirements(group, options);
    }

    /**
     * Choices that require a switch are offered only while it is on, and one
     * already chosen is withdrawn when it goes off ({@link
     * BackendOption#settle}, the same rule the connection panel and the Linux
     * client follow). Withdrawn to unset rather than to a stored fallback: the
     * fallback is not the user's choice, and an unset row follows later
     * default changes.
     *
     * <p>The switch may be in another group of the same screen (a backend's own
     * switches are above its connection defaults), so it is searched for from
     * the root. One not on the screen at all counts as on, as in the panel:
     * changing a choice for a reason not visible on the screen is worse than
     * offering one that the switch elsewhere may allow.
     */
    private static void requirements(PreferenceGroup group, List<BackendOption> options) {
        PreferenceGroup root = group;
        while (root.getParent() != null) {
            root = root.getParent();
        }
        final PreferenceGroup screen = root;
        final Map<String, List<BackendOption>> bySwitch = new LinkedHashMap<>();
        for (BackendOption o : options) {
            if (!(group.findPreference(o.key()) instanceof ListPreference)) {
                continue;
            }
            for (BackendOption.Choice c : o.choices()) {
                if (c.requires() != null) {
                    final List<BackendOption> dependents =
                            bySwitch.computeIfAbsent(c.requires(), k -> new ArrayList<>());
                    if (!dependents.contains(o)) {
                        dependents.add(o);
                    }
                }
            }
        }
        for (Map.Entry<String, List<BackendOption>> e : bySwitch.entrySet()) {
            final List<BackendOption> dependents = e.getValue();
            offer(group, dependents, key -> on(screen, key));
            final Preference control = screen.findPreference(e.getKey());
            if (control == null) {
                continue;
            }
            // Chained after any existing listener. The value passed in is the
            // one about to be stored, which the choices have to be offered for.
            final Preference.OnPreferenceChangeListener before =
                    control.getOnPreferenceChangeListener();
            control.setOnPreferenceChangeListener((pref, value) -> {
                if (before != null && !before.onPreferenceChange(pref, value)) {
                    return false;
                }
                offer(group, dependents, key -> key.equals(e.getKey())
                        ? Boolean.parseBoolean(String.valueOf(value)) : on(screen, key));
                return true;
            });
        }
    }

    private static boolean on(PreferenceGroup screen, String key) {
        return !(screen.findPreference(key) instanceof TwoStatePreference t) || t.isChecked();
    }

    private static void offer(PreferenceGroup group, List<BackendOption> dependents,
                              java.util.function.Predicate<String> on) {
        for (BackendOption o : dependents) {
            final ListPreference l = group.findPreference(o.key());
            final List<BackendOption.Choice> offered = o.choices().stream()
                    .filter(c -> c.requires() == null || on.test(c.requires()))
                    .collect(Collectors.toList());
            l.setEntries(offered.stream()
                    .map(BackendOption.Choice::label).toArray(CharSequence[]::new));
            l.setEntryValues(offered.stream()
                    .map(BackendOption.Choice::value).toArray(CharSequence[]::new));
            BackendOption.settle(List.of(o), x -> l.getValue(), on, (x, fallback) -> {
                if (l.getPreferenceDataStore() instanceof Store store) {
                    store.remove(o.key());
                }
                // Shown, not stored: the value was just removed from the store,
                // and the row shows the default that unset value stands for.
                l.setPersistent(false);
                l.setValue(fallback);
                l.setPersistent(true);
            });
        }
    }

    /**
     * Rows only offered for some values of another row: hidden now if needed,
     * and kept in step with that row afterwards.
     *
     * <p>One listener per controlling row rather than one per dependent, since
     * a preference has exactly one listener and one controlling row may gate
     * several rows. The listener runs <em>before</em> the value is stored, so
     * it decides on the value passed in.
     *
     * <p>A gate whose controlling row is not on this screen is closed: the
     * screen cannot set the value the option depends on, so it does not offer
     * the option.
     */
    private static void gates(PreferenceGroup group, List<BackendOption> options) {
        final Map<String, List<BackendOption>> byControl = new LinkedHashMap<>();
        for (BackendOption o : options) {
            if (o.gate() != null && group.findPreference(o.key()) != null) {
                byControl.computeIfAbsent(o.gate().key(), k -> new ArrayList<>()).add(o);
            }
        }
        for (Map.Entry<String, List<BackendOption>> e : byControl.entrySet()) {
            final Preference control = group.findPreference(e.getKey());
            final List<BackendOption> gated = e.getValue();
            show(group, gated, control instanceof ListPreference l ? l.getValue() : null);
            if (control != null) {
                control.setOnPreferenceChangeListener((pref, value) -> {
                    show(group, gated, String.valueOf(value));
                    return true;
                });
            }
        }
    }

    private static void show(PreferenceGroup group, List<BackendOption> gated, String value) {
        for (BackendOption o : gated) {
            group.findPreference(o.key()).setVisible(o.gate().open(value));
        }
    }

    /**
     * The input stack's tunables, each showing its value from {@code defaults}
     * until overridden, so the defaults are the stack's own rather than a copy
     * kept on this screen.
     */
    static void addTunables(PreferenceGroup group, List<InputSettings.Tunable> tunables,
                            Config defaults) {
        final Context ctx = group.getContext();
        for (InputSettings.Tunable t : tunables) {
            final String def = t.read().apply(defaults);
            final Preference p;
            if (t.kind() == InputSettings.Kind.BOOL) {
                final SwitchPreferenceCompat s = new SwitchPreferenceCompat(ctx);
                s.setDefaultValue(Boolean.parseBoolean(def));
                s.setSummary(t.summary());
                p = s;
            } else {
                final EditTextPreference e = new EditTextPreference(ctx);
                e.setDefaultValue(def);
                e.setDialogTitle(t.label());
                e.setDialogMessage(t.summary());
                e.setOnBindEditTextListener(box ->
                        box.setInputType(InputType.TYPE_CLASS_NUMBER
                                | InputType.TYPE_NUMBER_FLAG_DECIMAL));
                e.setSummaryProvider(pref -> withNote(
                        ((EditTextPreference) pref).getText(), t.summary()));
                p = e;
            }
            p.setKey(t.key());
            p.setTitle(t.label());
            p.setIconSpaceReserved(false);
            p.setSingleLineTitle(false);
            group.addPreference(p);
        }
    }

    /**
     * The summary of a row that shows its value as the summary: the value, then
     * the explanation under it, if any.
     */
    private static CharSequence withNote(CharSequence value, String note) {
        final CharSequence v = value == null ? "" : value;
        return note == null || note.isEmpty() ? v : v + "\n" + note;
    }
}
