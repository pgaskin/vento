// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.session;

import net.pgaskin.remotedesktop.control.input.ExtensionKeyboard;
import net.pgaskin.remotedesktop.control.input.KeySink;
import net.pgaskin.remotedesktop.control.input.Keysym;
import net.pgaskin.remotedesktop.control.input.Scheduler;

/**
 * Pasting is <b>typing</b>: the clipboard's characters go to the remote one at
 * a time as key presses, rather than as a Ctrl+V that assumes the far end can
 * reach a clipboard of its own and that its desktop pastes with that shortcut.
 *
 * <p>Two things can make that a bad idea, and both are worth a question first
 * because none of it can be undone from here: a lot of text, and a modifier
 * locked on the extension row. Asking is somebody else's — a canvas has no
 * dialogs — so both are handed out as callbacks and this only decides when
 * there is a question.
 */
final class PasteTyper {

    interface Confirm {

        /**
         * Something worth asking about before any of it is typed. Run
         * {@code proceed} if the answer is yes.
         *
         * @param heldModifiers the locked ones, "Ctrl + Shift", or empty
         */
        void ask(int characters, String heldModifiers, Runnable proceed);
    }

    private static final int CONFIRM_CHARS = 250; // longer is worth confirming first

    private static final long CHAR_MS = 8;        // so a long paste is not a burst

    private final KeySink keys;
    private final Scheduler scheduler;
    private final Confirm confirm;
    private final Runnable nothingToPaste;

    private String pasting;
    private int at;

    PasteTyper(KeySink keys, Scheduler scheduler, Confirm confirm, Runnable nothingToPaste) {
        this.keys = keys;
        this.scheduler = scheduler;
        this.confirm = confirm;
        this.nothingToPaste = nothingToPaste;
    }

    /**
     * Type {@code text} out, asking first if there is a reason to.
     *
     * @param text            the clipboard's, or null or empty for none
     * @param lockedModifiers what {@link #lockedModifiers} says about the row
     *                        these keys will arrive alongside
     */
    void paste(String text, String lockedModifiers) {
        if (text == null || text.isEmpty()) {
            nothingToPaste.run();
            return;
        }
        // A locked modifier is held at the far end for as long as it is locked,
        // so every character of the paste would arrive as a shortcut — worth a
        // question at any length. One-shot modifiers are not: the row consumes
        // those the moment the paste key fires.
        final int chars = text.codePointCount(0, text.length());
        if (chars > CONFIRM_CHARS || !lockedModifiers.isEmpty()) {
            confirm.ask(chars, lockedModifiers, () -> typeOut(text));
        } else {
            typeOut(text);
        }
    }

    /** The locked modifiers, in row order, as "Ctrl + Shift"; empty if none. */
    static String lockedModifiers(ExtensionKeyboard keyboard) {
        final StringBuilder sb = new StringBuilder();
        for (ExtensionKeyboard.Key m : keyboard.modifiers()) {
            if (keyboard.sticky(m) == ExtensionKeyboard.Sticky.LOCKED) {
                if (sb.length() > 0) {
                    sb.append(" + ");
                }
                sb.append(m.label());
            }
        }
        return sb.toString();
    }

    /**
     * A character per tick. On a clock rather than in a loop for two reasons: a
     * thousand key events posted in one go is a burst the far end has no reason
     * to survive in order, and a paste in progress has to be abandonable, which
     * {@link #cancel()} does.
     */
    private void typeOut(String text) {
        pasting = text;
        at = 0;
        scheduler.removeCallbacks(tick);
        scheduler.postDelayed(tick, 0);
    }

    private final Runnable tick = new Runnable() {
        @Override
        public void run() {
            if (pasting == null) {
                return;
            }
            if (at >= pasting.length()) {
                pasting = null;
                return;
            }
            final int cp = pasting.codePointAt(at);
            at += Character.charCount(cp);
            // A newline is Return, a tab is Tab: the characters a text field
            // holds but a keyboard does not have as characters.
            final int keysym = Keysym.forCharacter(cp);
            if (keysym != 0) {
                keys.keyDown(keysym, KeySink.ID_TEXT);
                keys.keyUp(KeySink.ID_TEXT);
            }
            scheduler.postDelayed(this, CHAR_MS);
        }
    };

    /** Give up on the rest of it; what has already been typed is typed. */
    void cancel() {
        pasting = null;
        scheduler.removeCallbacks(tick);
    }
}
