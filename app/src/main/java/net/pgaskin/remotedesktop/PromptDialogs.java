// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.app.Activity;
import android.content.DialogInterface;
import android.text.InputType;
import android.view.ContextThemeWrapper;
import android.view.View;
import android.view.ViewGroup;
import android.widget.EditText;
import android.widget.LinearLayout;
import android.widget.TextView;

import androidx.appcompat.app.AlertDialog;

import com.google.android.material.checkbox.MaterialCheckBox;
import com.google.android.material.textfield.TextInputEditText;
import com.google.android.material.textfield.TextInputLayout;

import net.pgaskin.remotedesktop.backend.Prompt;

/**
 * The three kinds of question a connection can stop to ask, as dialogs over
 * the waiting session.
 *
 * <p>Dialogs rather than separate screens: the connection is stalled until each
 * one is answered either way, and a navigation step would hide that.
 *
 * <p>Every prompt must be answered, since the connection does not progress
 * until it is, so a dismissal counts as a no, and {@link Session} queues
 * prompts that arrive while no screen is showing.
 */
final class PromptDialogs implements Prompt.Handler {

    interface Host {

        /**
         * Credentials typed with "remember" ticked. Held rather than saved:
         * they are only known to be right once the far end accepts them, which
         * this class cannot see.
         */
        void rememberCredentials(String userName, String password);

        /**
         * The user declined a prompt. Distinguished from a connection that
         * failed by itself, which shows its message on screen: after a decline
         * the user has already chosen to leave, and showing "Disconnected" over
         * a black screen would make them press back to leave again.
         */
        void promptDeclined();
    }

    private final Activity activity;
    private final Dialogs.Tracker dialogs;
    private final Host host;
    /** Whether there is a saved connection to store a password on. */
    private final boolean mayRemember;
    /**
     * The connecting screen's status line, which a credentials dialog covers
     * and so shows as its message, and the open dialog's view for it.
     */
    private String status;
    private TextView statusView;

    PromptDialogs(Activity activity, Dialogs.Tracker dialogs, Host host, boolean mayRemember) {
        this.activity = activity;
        this.dialogs = dialogs;
        this.host = host;
        this.mayRemember = mayRemember;
    }

    /**
     * What the connection is waiting for, or null: shown in an open
     * credentials dialog at once, and in the next one.
     */
    void status(String text) {
        status = text;
        if (statusView != null) {
            showStatus(statusView);
        }
    }

    private void showStatus(TextView view) {
        view.setText(status);
        view.setVisibility(status != null ? View.VISIBLE : View.GONE);
    }

    @Override
    public void credentials(Prompt.Credentials prompt) {
        final LinearLayout form = new LinearLayout(activity);
        form.setOrientation(LinearLayout.VERTICAL);
        final int pad = Dialogs.padding(activity);
        form.setPadding(pad, Dialogs.dp(activity, 8), pad, 0);

        // The status line, as the dialog's message: built in the message's
        // style rather than set as one, since a dialog created without a
        // message has no view to show one that arrives later.
        final TextView line = new TextView(
                new ContextThemeWrapper(activity, R.style.ThemeOverlay_RemoteDesktop_Dialog), null, 0,
                com.google.android.material.R.style.MaterialAlertDialog_Material3_Body_Text);
        final LinearLayout.LayoutParams lineAt = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        lineAt.bottomMargin = Dialogs.dp(activity, 16);
        form.addView(line, lineAt);
        showStatus(line);

        if (prompt.instructions != null && !prompt.instructions.isEmpty()) {
            final TextView note = new TextView(activity);
            note.setText(prompt.instructions);
            form.addView(note);
        }
        // Only the fields the scheme has: VncAuth has no user name, so it gets
        // no box for one.
        final EditText user = field(form, R.string.prompt_username,
                InputType.TYPE_CLASS_TEXT, prompt.needsUserName);
        user.setText(prompt.userName);
        final EditText pass = field(form, R.string.prompt_password,
                InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_PASSWORD,
                prompt.needsPassword);
        if (prompt.catchphrase != null && !prompt.catchphrase.isEmpty()) {
            final TextView id = new TextView(activity);
            id.setText(activity.getString(R.string.prompt_catchphrase, prompt.catchphrase));
            form.addView(id);
        }
        // Only for a saved connection, the only place to store it. Ours rather
        // than the RealVNC core's credential store, which offers to save after
        // a *failed* attempt as well as a successful one.
        final MaterialCheckBox remember = Dialogs.checkBox(activity,
                R.string.prompt_remember_password);
        if (mayRemember && prompt.needsPassword) {
            form.addView(remember);
        }

        statusView = line;
        show(Dialogs.builder(activity)
                .setTitle(prompt.address != null && !prompt.address.isEmpty()
                        ? prompt.address : activity.getString(R.string.prompt_credentials_title))
                .setView(form)
                .setPositiveButton(R.string.prompt_connect, (d, w) -> {
                    final String u = prompt.needsUserName ? user.getText().toString() : null;
                    final String p = prompt.needsPassword ? pass.getText().toString() : null;
                    if (remember.isChecked()) {
                        host.rememberCredentials(u, p);
                    }
                    prompt.answer(u, p);
                })
                .setNegativeButton(android.R.string.cancel, (d, w) -> decline(prompt)), prompt);
    }

    @Override
    public void trust(Prompt.Trust prompt) {
        final StringBuilder sb = new StringBuilder();
        // OK, PRESHARED and ARD add no text: the prompt is shown for some other
        // reason, such as an unencrypted connection.
        final String why = switch (prompt.identity) {
            case CHANGED -> activity.getString(R.string.prompt_trust_changed);
            case MATCHES_ANOTHER -> activity.getString(R.string.prompt_trust_another,
                    prompt.matchingAddress != null && !prompt.matchingAddress.isEmpty()
                            ? prompt.matchingAddress
                            : activity.getString(R.string.prompt_trust_another_address));
            case NEW -> activity.getString(R.string.prompt_trust_new);
            case MISSING -> activity.getString(R.string.prompt_trust_missing);
            case OK, PRESHARED, ARD -> "";
        };
        if (!why.isEmpty()) {
            sb.append(why).append("\n\n");
        }
        if (prompt.encryption == Prompt.Trust.Encryption.UNENCRYPTED_WARN) {
            sb.append(activity.getString(R.string.prompt_unencrypted)).append("\n\n");
        }
        if (prompt.catchphrase != null && !prompt.catchphrase.isEmpty()) {
            sb.append(activity.getString(R.string.prompt_catchphrase, prompt.catchphrase))
                    .append('\n');
        }
        if (prompt.signature != null && !prompt.signature.isEmpty()) {
            sb.append(activity.getString(R.string.prompt_signature, prompt.signature));
        }

        final MaterialCheckBox remember = Dialogs.checkBox(activity,
                R.string.prompt_remember_server);
        remember.setChecked(prompt.identity != Prompt.Trust.Identity.CHANGED);
        final LinearLayout box = new LinearLayout(activity);
        box.setPadding(Dialogs.padding(activity), Dialogs.dp(activity, 8),
                Dialogs.padding(activity), 0);
        box.addView(remember);

        show(Dialogs.builder(activity)
                .setTitle(prompt.address != null ? prompt.address
                        : activity.getString(R.string.prompt_trust_title))
                .setMessage(sb.toString())
                .setView(box)
                .setPositiveButton(R.string.prompt_continue,
                        (d, w) -> prompt.answer(true, remember.isChecked()))
                .setNegativeButton(android.R.string.cancel, (d, w) -> decline(prompt)), prompt);
    }

    @Override
    public void message(Prompt.Message prompt) {
        final AlertDialog.Builder b = Dialogs.builder(activity)
                .setMessage(prompt.text)
                .setPositiveButton(prompt.confirmLabel != null ? prompt.confirmLabel
                                : activity.getString(android.R.string.ok),
                        (d, w) -> prompt.answer(true));
        if (prompt.question) {
            b.setNegativeButton(android.R.string.cancel, (d, w) -> prompt.answer(false));
        }
        show(b, prompt);
    }

    /**
     * A text field, using the option row's layout like every other form in this
     * app, so a password is entered the same way as an address. Built whether
     * or not it is wanted, so the caller always has a field to reference; an
     * unwanted one is not added to the form.
     */
    private EditText field(LinearLayout form, int hint, int inputType, boolean wanted) {
        final TextInputLayout box = (TextInputLayout) activity.getLayoutInflater()
                .inflate(R.layout.item_option_text, form, false);
        box.setHint(hint);
        final TextInputEditText text = box.findViewById(R.id.value);
        text.setInputType(inputType);
        if (wanted) {
            ((ViewGroup.MarginLayoutParams) box.getLayoutParams()).bottomMargin =
                    Dialogs.dp(activity, 8);
            form.addView(box);
        }
        return text;
    }

    /**
     * A dismissed dialog still has to answer the prompt, since the session
     * waits for it, so cancelling counts as declining.
     */
    private void show(AlertDialog.Builder builder, Prompt prompt) {
        final AlertDialog dialog = builder.create();
        dialog.setOnCancelListener((DialogInterface d) -> decline(prompt));
        final TextView line = prompt instanceof Prompt.Credentials ? statusView : null;
        dialogs.track(dialog, d -> {
            if (statusView == line) {
                statusView = null;
            }
        });
        dialog.show();
    }

    /** Declines the prompt, and tells the host the user declined it. */
    private void decline(Prompt prompt) {
        host.promptDeclined();
        prompt.cancel();
    }
}
