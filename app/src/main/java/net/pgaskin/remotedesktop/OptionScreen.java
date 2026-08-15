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

import net.pgaskin.remotedesktop.backend.BackendOption;
import net.pgaskin.remotedesktop.control.input.Config;

import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * Turning a description of a setting into a row on a screen.
 *
 * <p>Two descriptions, one builder: {@link BackendOption}, which a backend
 * supplies about itself, and {@link InputSettings.Tunable}, which the app keeps
 * about the input stack. Neither knows anything about androidx, which is the
 * point — {@code control} may not depend on it at all, and a backend describing
 * its options in terms of {@code SwitchPreferenceCompat} would be a backend
 * that can only be used from an Android app with androidx in it.
 *
 * <h2>Everything is a string</h2>
 * Both descriptions carry string values, so the preference screens store
 * strings — including the switches, whose {@code true}/{@code false} would
 * otherwise land in the preference file as booleans and come back out through a
 * different door than they went in. The {@link PreferenceDataStore} below is
 * what makes that uniform.
 */
final class OptionScreen {

    private OptionScreen() {
    }

    /** A store over one preference file, where every value is a string. */
    static Store store(SharedPreferences prefs) {
        return new Store(prefs, true);
    }

    /**
     * The same, over a file whose values are booleans — the app's own switches,
     * which were written that way before there was a store here and are read
     * that way by {@link AppSettings}.
     */
    static Store switches(SharedPreferences prefs) {
        return new Store(prefs, false);
    }

    /**
     * Where a screen's values are kept, deaf until {@link #built()} says the
     * screen is up.
     *
     * <p>The deafness is the point. A preference persists its default the moment
     * it is bound — androidx has no "show this and store nothing" — so a screen
     * that had merely been <em>looked at</em> wrote an answer to every row on
     * it, and everything here rests on an unanswered setting being absent: a
     * backend cannot tell "this asked for full colour" from "nobody has said",
     * a connection that agrees with a backend's answer stores nothing so that
     * moving the answer moves it too, and a default improved in a later version
     * reaches nobody whose file already claims to have chosen the old one. A
     * write while the rows are going in is the framework filling them; a write
     * after that is a person.
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
    }

    /**
     * The rows that are only offered for some answers to another row, hidden
     * now and kept in step with it after.
     *
     * <p>One listener per controlling row rather than one per dependent, since
     * a preference has room for exactly one and a mode dial may own several
     * rows. The listener runs <em>before</em> the value is stored, so what it
     * decides on is what it was handed.
     *
     * <p>A gate whose controlling row is not on this screen closes: it is a
     * screen that cannot answer the question the option depends on, so it has
     * no business offering the option.
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
     * The input stack's tunables, each showing what {@code defaults} says until
     * it is overridden — the stack's own answer rather than a copy of it kept on
     * this screen.
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
     * The summary of a row whose value is already its summary: the value, then
     * the explanation under it. A choice with no explanation is just its value.
     */
    private static CharSequence withNote(CharSequence value, String note) {
        final CharSequence v = value == null ? "" : value;
        return note == null || note.isEmpty() ? v : v + "\n" + note;
    }
}
