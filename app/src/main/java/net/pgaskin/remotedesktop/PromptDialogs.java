// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.app.Activity;
import android.content.DialogInterface;
import android.text.InputType;
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
 * The three questions a connection can stop and ask, as dialogs over the
 * session that has stopped for them.
 *
 * <p>Dialogs rather than screens of their own: the connection is stalled behind
 * each one either way, so putting it behind a navigation step would be
 * pretending otherwise.
 *
 * <p>Every one of them owes an answer — the core blocks its session thread
 * until it gets one — which is why a dismissal counts as a no rather than as
 * nothing having happened, and why {@link Session} queues what arrives while
 * there is nobody to ask.
 */
final class PromptDialogs implements Prompt.Handler {

    interface Host {

        /**
         * Credentials typed with "remember" ticked. Held rather than saved:
         * what was typed is only known to be right once the far end has taken
         * it, which is a moment this class cannot see.
         */
        void rememberCredentials(String userName, String password);

        /**
         * A question was answered with no, here. Worth telling from a
         * connection that failed by itself: that one deserves its message on
         * screen, and a declined prompt is an answer already given — leaving
         * "Disconnected" over a black screen for it means pressing back to
         * leave a room you just left.
         */
        void promptDeclined();
    }

    private final Activity activity;
    private final Dialogs.Tracker dialogs;
    private final Host host;
    /** Whether there is anywhere to put a password worth keeping. */
    private final boolean mayRemember;

    PromptDialogs(Activity activity, Dialogs.Tracker dialogs, Host host, boolean mayRemember) {
        this.activity = activity;
        this.dialogs = dialogs;
        this.host = host;
        this.mayRemember = mayRemember;
    }

    @Override
    public void credentials(Prompt.Credentials prompt) {
        final LinearLayout form = new LinearLayout(activity);
        form.setOrientation(LinearLayout.VERTICAL);
        final int pad = Dialogs.padding(activity);
        form.setPadding(pad, Dialogs.dp(activity, 8), pad, 0);

        if (prompt.instructions != null && !prompt.instructions.isEmpty()) {
            final TextView note = new TextView(activity);
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
            final TextView id = new TextView(activity);
            id.setText(activity.getString(R.string.prompt_catchphrase, prompt.catchphrase));
            form.addView(id);
        }
        // Only for a saved connection: there is nowhere else to put the answer.
        // Ours rather than the core's own credential store, which offers to save
        // after a *failed* attempt as well as a successful one.
        final MaterialCheckBox remember = Dialogs.checkBox(activity,
                R.string.prompt_remember_password);
        if (mayRemember && prompt.needsPassword) {
            form.addView(remember);
        }

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
        // OK, PRESHARED and ARD say nothing: this prompt is up because
        // something else about the connection is worth asking about.
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
     * A box to type in, in the shape every other form in this app uses — the
     * option row's own layout, so a password is asked for the way an address
     * is. Built whether or not it is wanted, since the caller reads it either
     * way and an unasked field answers with the empty string.
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
     * A dismissed dialog still owes the core an answer — the session is
     * blocked until it gets one — so cancelling counts as declining.
     */
    private void show(AlertDialog.Builder builder, Prompt prompt) {
        final AlertDialog dialog = builder.create();
        dialog.setOnCancelListener((DialogInterface d) -> decline(prompt));
        dialogs.track(dialog);
        dialog.show();
    }

    /** Say no, and tell the screen it was us who did. */
    private void decline(Prompt prompt) {
        host.promptDeclined();
        prompt.cancel();
    }
}
