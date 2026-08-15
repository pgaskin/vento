// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.content.Context;
import android.text.Editable;
import android.text.InputType;
import android.text.TextWatcher;
import android.view.LayoutInflater;
import android.view.Menu;
import android.view.View;
import android.view.ViewGroup;
import android.widget.CompoundButton;
import android.widget.TextView;

import androidx.appcompat.app.AlertDialog;
import androidx.appcompat.widget.PopupMenu;

import com.google.android.material.button.MaterialButton;
import com.google.android.material.dialog.MaterialAlertDialogBuilder;
import com.google.android.material.materialswitch.MaterialSwitch;
import com.google.android.material.textfield.TextInputEditText;
import com.google.android.material.textfield.TextInputLayout;

import net.pgaskin.remotedesktop.backend.BackendOption;

import java.util.ArrayList;
import java.util.List;

/**
 * {@link BackendOption}s as rows in a sheet — the one renderer both panels use.
 *
 * <p>There were two: this one, inline in {@link ConnectionPanel}, and the
 * androidx {@code Preference} one in {@link OptionScreen}. The editor was the
 * second renderer's only reason to exist for a <em>connection</em>, and when
 * the editor became a sheet it came here: one description, one thing that
 * draws it.
 * {@link OptionScreen} still exists because the settings tree is a real
 * preference screen — a backend's global options and the input stack's tunables
 * — and that is a different screen rather than a second reading of this one.
 *
 * <p>The switch over {@link BackendOption.Type} is an expression with no
 * {@code default}, here and in {@link OptionScreen}, so a type added later is a
 * compile error in both places rather than a row that silently does not appear.
 *
 * <p>Every value is a string, in and out, for the reason
 * {@link BackendOption} gives: that is what the backends' own configuration
 * layers take.
 */
final class PanelOptions {

    /** Where a row's value comes from and where a change goes. */
    interface Values {
        String get(BackendOption option);

        void set(BackendOption option, String value);

        /**
         * The effective value of another option, for a choice that depends on
         * one. Answering null hides every dependent choice, which is the right
         * answer for a caller that cannot see the option being asked about.
         */
        default String get(String key) {
            return null;
        }
    }

    private final List<Row> rows = new ArrayList<>();
    private final Values values;

    /** Build a row per option into {@code into}, in the order given. */
    PanelOptions(ViewGroup into, List<BackendOption> options, Values values) {
        this.values = values;
        final LayoutInflater inf = LayoutInflater.from(into.getContext());
        for (BackendOption o : options) {
            final Row row = switch (o.type()) {
                case BOOL -> {
                    final View v = inf.inflate(R.layout.item_option_switch, into, false);
                    ((TextView) v.findViewById(R.id.label)).setText(o.label());
                    final MaterialSwitch s = v.findViewById(R.id.toggle);
                    v.setOnClickListener(x -> set(o, Boolean.toString(!s.isChecked())));
                    yield new Row(o, v) {
                        @Override
                        void refresh() {
                            s.setChecked(Boolean.parseBoolean(value(o)));
                        }
                    };
                }
                case CHOICE -> {
                    final View v = inf.inflate(R.layout.item_option_choice, into, false);
                    ((TextView) v.findViewById(R.id.label)).setText(o.label());
                    final MaterialButton b = v.findViewById(R.id.value);
                    b.setOnClickListener(this::chooser);
                    b.setTag(o);
                    yield new Row(o, v) {
                        @Override
                        void refresh() {
                            b.setText(o.labelFor(value(o)));
                        }

                        @Override
                        void enable(boolean enabled) {
                            super.enable(enabled);
                            b.setEnabled(enabled);
                        }
                    };
                }
                case INT, TEXT -> {
                    final TextInputLayout field = (TextInputLayout)
                            inf.inflate(R.layout.item_option_text, into, false);
                    field.setHint(o.label());
                    field.setHelperText(o.summary());
                    final TextInputEditText box = field.findViewById(R.id.value);
                    if (o.type() == BackendOption.Type.INT) {
                        box.setInputType(InputType.TYPE_CLASS_NUMBER);
                    }
                    // `writing` is what keeps a refresh from looking like typing.
                    // Without it the first refresh — the one in this
                    // constructor, filling a saved connection in — reports every
                    // field as a change back to the caller, before the caller
                    // has finished building the thing it would report them to.
                    final boolean[] writing = {false};
                    box.addTextChangedListener(new Watcher(() -> {
                        if (!writing[0]) {
                            values.set(o, box.getText() == null ? "" : box.getText().toString());
                        }
                    }));
                    yield new Row(o, field) {
                        @Override
                        void refresh() {
                            // A field somebody is typing in is not refreshed
                            // under them, and one whose text already matches is
                            // left alone — setText moves the caret to the end.
                            final String want = value(o);
                            final String has = box.getText() == null ? "" : box.getText().toString();
                            if (!box.hasFocus() && !want.equals(has)) {
                                writing[0] = true;
                                box.setText(want);
                                writing[0] = false;
                            }
                        }

                        @Override
                        void enable(boolean enabled) {
                            super.enable(enabled);
                            field.setEnabled(enabled);
                        }
                    };
                }
            };
            into.addView(row.view);
            rows.add(row);
        }
        // The container is the group, so what a row looks like depends on the
        // ones either side of it and cannot be decided while it is being built.
        RowGroups.apply(into);
        refresh();
    }

    boolean isEmpty() {
        return rows.isEmpty();
    }

    /**
     * Write the current values into the rows, and take out the rows that the
     * current values leave nothing to say.
     *
     * <p>The group is re-formed afterwards because its shape is its members:
     * hiding the row before last makes the one above it the last.
     */
    void refresh() {
        boolean moved = false;
        for (Row r : rows) {
            final BackendOption.Gate gate = r.option.gate();
            final int want = gate == null || gate.open(values.get(gate.key()))
                    ? View.VISIBLE : View.GONE;
            if (r.view.getVisibility() != want) {
                r.view.setVisibility(want);
                moved = true;
            }
            if (want == View.VISIBLE) {
                r.refresh();
            }
        }
        if (moved && !rows.isEmpty() && rows.get(0).view.getParent() instanceof ViewGroup g) {
            RowGroups.apply(g);
        }
    }

    /**
     * Whether the rows mean anything at all. The connection panel greys them
     * against a session that is connecting or gone: the backend would take the
     * change and hold it for a connection that may never happen, and a switch
     * that moves and does nothing is a lie.
     */
    void setEnabled(boolean enabled) {
        for (Row r : rows) {
            r.enable(enabled);
        }
    }

    /** The text field a row owns, for a caller that wants to mark it wrong. */
    TextInputLayout fieldFor(String key) {
        for (Row r : rows) {
            if (r.option.key().equals(key) && r.view instanceof TextInputLayout f) {
                return f;
            }
        }
        return null;
    }

    /** The choices actually on offer: one may depend on a switch elsewhere. */
    private List<BackendOption.Choice> choices(BackendOption o) {
        if (o.choices().stream().noneMatch(c -> c.requires() != null)) {
            return o.choices();
        }
        return o.choices().stream()
                .filter(c -> c.requires() == null
                        || Boolean.parseBoolean(values.get(c.requires())))
                .toList();
    }

    private String value(BackendOption o) {
        final String v = values.get(o);
        return v == null ? "" : v;
    }

    private void set(BackendOption o, String value) {
        values.set(o, value);
        refresh();
    }

    /**
     * A menu under the row's own button, or a dialog where the values have more
     * to say than a label.
     *
     * <p>Which one is decided by the choices rather than by the key, so the row
     * that opens a window over the sheet is the row whose values could not be
     * told apart in a menu. Only the protocol has any today.
     */
    private void chooser(View anchor) {
        final BackendOption o = (BackendOption) anchor.getTag();
        final List<BackendOption.Choice> choices = choices(o);
        if (choices.stream().anyMatch(c -> c.summary() != null)) {
            described(anchor, o, choices);
            return;
        }
        final PopupMenu menu = new PopupMenu(anchor.getContext(), anchor);
        final String current = value(o);
        for (int i = 0; i < choices.size(); i++) {
            menu.getMenu().add(Menu.NONE, i, i, choices.get(i).label());
        }
        // Checkable before checked: the flag is what makes the tick visible, and
        // setting it afterwards on an item that was never checkable does nothing.
        menu.getMenu().setGroupCheckable(Menu.NONE, true, true);
        for (int i = 0; i < choices.size(); i++) {
            if (choices.get(i).value().equals(current)) {
                menu.getMenu().findItem(i).setChecked(true);
            }
        }
        menu.setOnMenuItemClickListener(item -> {
            set(o, choices.get(item.getItemId()).value());
            return true;
        });
        menu.show();
    }

    /**
     * The same choice as a list of items, each with its line under it.
     *
     * <p>A tap on a row is the answer and closes the dialog, so there is no
     * button to confirm one: the radios say which is chosen now, and picking a
     * value is the whole of what this asks.
     *
     * <p>It goes when its own row does. This is a second window over a sheet
     * whose owner does not hold a reference to these rows, and a rotation takes
     * the sheet with the activity — so the row being detached is what closes
     * this, which covers both that and the rebuild a changed protocol causes.
     */
    private void described(View anchor, BackendOption o, List<BackendOption.Choice> choices) {
        final Context ctx = anchor.getContext();
        final LayoutInflater inf = LayoutInflater.from(ctx);
        final View body = inf.inflate(R.layout.dialog_choices, null);
        final ViewGroup items = body.findViewById(R.id.items);
        final String current = value(o);
        final AlertDialog dialog = new MaterialAlertDialogBuilder(
                ctx, R.style.ThemeOverlay_RemoteDesktop_Dialog)
                .setTitle(o.label())
                .setView(body)
                .setNegativeButton(android.R.string.cancel, null)
                .create();
        for (BackendOption.Choice c : choices) {
            final View row = inf.inflate(R.layout.item_choice_described, items, false);
            ((TextView) row.findViewById(R.id.label)).setText(c.label());
            final TextView summary = row.findViewById(R.id.summary);
            summary.setText(c.summary());
            summary.setVisibility(c.summary() == null || c.summary().isEmpty()
                    ? View.GONE : View.VISIBLE);
            ((CompoundButton) row.findViewById(R.id.tick)).setChecked(c.value().equals(current));
            row.setOnClickListener(v -> {
                set(o, c.value());
                dialog.dismiss();
            });
            items.addView(row);
        }
        final View.OnAttachStateChangeListener gone = new View.OnAttachStateChangeListener() {
            @Override
            public void onViewAttachedToWindow(View v) {
            }

            @Override
            public void onViewDetachedFromWindow(View v) {
                dialog.dismiss();
            }
        };
        anchor.addOnAttachStateChangeListener(gone);
        dialog.setOnDismissListener(d -> anchor.removeOnAttachStateChangeListener(gone));
        dialog.show();
    }

    private abstract static class Row {
        final BackendOption option;
        final View view;

        Row(BackendOption option, View view) {
            this.option = option;
            this.view = view;
        }

        abstract void refresh();

        void enable(boolean enabled) {
            view.setEnabled(enabled);
            view.setAlpha(enabled ? 1f : 0.38f);
        }
    }

    /** {@link TextWatcher} is three methods and every caller wants one. */
    static final class Watcher implements TextWatcher {
        private final Runnable changed;

        Watcher(Runnable changed) {
            this.changed = changed;
        }

        @Override
        public void beforeTextChanged(CharSequence s, int start, int count, int after) {
        }

        @Override
        public void onTextChanged(CharSequence s, int start, int before, int count) {
        }

        @Override
        public void afterTextChanged(Editable s) {
            changed.run();
        }
    }
}
