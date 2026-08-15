// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.ui;

import android.view.KeyEvent;
import android.view.View;
import android.view.inputmethod.BaseInputConnection;

import net.pgaskin.remotedesktop.control.input.KeySink;
import net.pgaskin.remotedesktop.control.input.Keysym;
import net.pgaskin.remotedesktop.control.input.PhysicalKeyboard;
import net.pgaskin.remotedesktop.control.input.TextDelta;

import java.util.Set;

/**
 * The system soft keyboard, turned into keysyms.
 *
 * <p>The extension keyboard is a <em>row of the keys a soft keyboard lacks</em>,
 * which only means anything if the soft keyboard is there supplying the rest —
 * so a screen that hosts the row opens a real IME against this connection and
 * forwards whatever it produces to the same {@link KeySink} the extension keys
 * use.
 *
 * <p>It is a <em>dummy</em> {@code BaseInputConnection} over an editor declaring
 * {@code TYPE_NULL}, the way terminal emulators do it: with no text field to
 * edit, an IME falls back to sending key events, which is exactly what a remote
 * machine wants. Committed text is handled as well, because prediction and
 * emoji arrive that way regardless.
 *
 * <p><b>Every call in and every keysym out can be traced</b>, because what an
 * IME does with an editor it cannot read is not documented anywhere and differs
 * between keyboards — the only way to know is to watch:
 *
 * <pre>adb shell setprop log.tag.TextInput VERBOSE</pre>
 *
 * The original does the same thing permanently, under this very tag.
 */
public final class TextInput extends BaseInputConnection {

    private static final String TAG = "TextInput";

    private static void trace(String what) {
        if (android.util.Log.isLoggable(TAG, android.util.Log.VERBOSE)) {
            android.util.Log.v(TAG, what);
        }
    }

    /** The other keyboard: what it is holding, and what it has to be told. */
    public interface Watcher {
        /** Notified after each key, so it can consume its one-shot modifiers. */
        void sent(int keysym);

        /**
         * The modifier keysyms it is holding down at the far end, which is what
         * {@link Keysym#forCharacter(int, boolean, boolean)}'s case rule needs.
         */
        Set<Integer> heldModifiers();

        /**
         * The IME asked for the clipboard to be pasted, and there is no document
         * here to paste into — so it is typed out, as the row's Paste key does.
         */
        void pasteRequested();
    }

    private final KeySink sink;
    private final Watcher watcher;
    private final TextDelta delta = new TextDelta();

    private final TextDelta.Out out = new TextDelta.Out() {
        @Override
        public void backspace() {
            send(Keysym.BACKSPACE);
        }

        @Override
        public void character(int codePoint) {
            send(Keysym.forCharacter(codePoint, shiftHeld(), otherModifierHeld()));
        }
    };

    public TextInput(View target, KeySink sink, Watcher watcher) {
        super(target, false);
        this.sink = sink;
        this.watcher = watcher;
    }

    @Override
    public boolean commitText(CharSequence text, int newCursorPosition) {
        trace("commitText(\"" + text + "\", " + newCursorPosition + ")");
        delta.commit(text, out);
        return true;
    }

    /**
     * A completion picked from the IME's own list. Rare with {@code TYPE_NULL},
     * but it is an insertion like any other and nothing else would deliver it.
     */
    @Override
    public boolean commitCompletion(android.view.inputmethod.CompletionInfo text) {
        trace("commitCompletion(" + text + ")");
        delta.commit(text == null ? null : text.getText(), out);
        return true;
    }

    /**
     * <b>The one an editor gets for free and a remote does not.</b> An IME with
     * a clipboard key — AOSP's own, on the comma key's long press
     * ({@code LatinIME}, {@code CODE_CLIPBOARD_PASTE}) — does not commit text
     * for it: it asks the <em>editor</em> to paste, through the same context
     * menu action a text field's own Paste item uses. {@code BaseInputConnection}
     * answers that with a no-op, so before this the key looked like it worked
     * and typed nothing at all.
     *
     * <p>Cut, copy and select-all are refused rather than faked: they are about
     * a selection in a document, and the document is on the other machine.
     */
    @Override
    public boolean performContextMenuAction(int id) {
        trace("performContextMenuAction(" + id + ")");
        if (id == android.R.id.paste || id == android.R.id.pasteAsPlainText) {
            watcher.pasteRequested();
            return true;
        }
        return false;
    }

    /**
     * The IME's action key. With {@code TYPE_NULL} and {@code IME_ACTION_NONE}
     * most keyboards send Enter as a key event, but the ones that take the
     * action route would otherwise be silent.
     */
    @Override
    public boolean performEditorAction(int editorAction) {
        trace("performEditorAction(" + editorAction + ")");
        send(Keysym.RETURN);
        return true;
    }

    /**
     * Composing text is sent as it is typed, as a <em>difference</em> against
     * what was sent before it ({@link TextDelta}): holding it back until the IME
     * decides would make typing feel broken, and a revision of the region —
     * which is most of what an IME does — has to correct the far end rather than
     * append to it.
     */
    @Override
    public boolean setComposingText(CharSequence text, int newCursorPosition) {
        trace("setComposingText(\"" + text + "\", " + newCursorPosition + ")");
        delta.compose(text, out);
        return true;
    }

    /** The region ended and its text stands: every character in it is already typed. */
    @Override
    public boolean finishComposingText() {
        trace("finishComposingText()");
        delta.finish();
        return true;
    }

    /**
     * <b>Refused, and this is the honest answer.</b> The IME wants text it
     * believes is in the document back in a composing region, so that a later
     * {@code setComposingText} replaces it — but the document is the far end's
     * and cannot be read back, so the replacement would be typed without the
     * original being removed. Saying no leaves the IME to commit its revision as
     * an ordinary insertion.
     */
    @Override
    public boolean setComposingRegion(int start, int end) {
        trace("setComposingRegion(" + start + ", " + end + ") — refused");
        return false;
    }

    @Override
    public boolean deleteSurroundingText(int beforeLength, int afterLength) {
        trace("deleteSurroundingText(" + beforeLength + ", " + afterLength + ")");
        // A deletion is measured against a document this end does not have, so
        // where it lands relative to the composing region is unknowable — and
        // once one has gone out, what the far end holds is no longer what the
        // region says it holds.
        delta.finish();
        for (int i = 0; i < beforeLength; i++) {
            send(Keysym.BACKSPACE);
        }
        for (int i = 0; i < afterLength; i++) {
            send(Keysym.DELETE);
        }
        return true;
    }

    /**
     * The same request counted in code points. An IME deleting an emoji or an
     * accented character uses this one, and {@code BaseInputConnection}'s
     * default works on its own empty editable — so without it a backspace over
     * anything outside the BMP does nothing.
     *
     */
    @Override
    public boolean deleteSurroundingTextInCodePoints(int beforeLength, int afterLength) {
        return deleteSurroundingText(beforeLength, afterLength);
    }

    // ACTION_MULTIPLE and getCharacters are deprecated and are still what some
    // IMEs send; there is nothing to replace them with, only to ignore.
    @SuppressWarnings("deprecation")
    @Override
    public boolean sendKeyEvent(KeyEvent event) {
        final int action = event.getAction();
        trace("sendKeyEvent(" + event + ")");
        // The deprecated third action, which some IMEs still use to deliver a run
        // of characters in one event. It is text, and is typed out as text.
        if (action == KeyEvent.ACTION_MULTIPLE) {
            delta.finish();
            type(event.getCharacters());
            return true;
        }
        if (action != KeyEvent.ACTION_DOWN && action != KeyEvent.ACTION_UP) {
            return true;
        }
        // A key press is outside the composing region by definition, and the IME
        // is not obliged to say so first.
        delta.finish();
        int keysym = Keysym.fromAndroidKeyCode(event.getKeyCode());
        if (keysym == 0) {
            // Only the modifiers that choose the character, exactly as the
            // physical keyboard masks them: a layout asked about Ctrl+C answers
            // with nothing, and the shortcut would be lost rather than sent.
            final int cp = event.getUnicodeChar(PhysicalKeyboard.charMeta(event.getMetaState()));
            if (cp == 0) {
                return true;
            }
            keysym = Keysym.forCharacter(cp, shiftHeld(), otherModifierHeld());
        }
        // The Android key code is this key's identity for as long as it is held
        // (KeySink §"the key id"); the keysym is only what it currently means,
        // which a modifier the IME is holding can change between the two edges.
        final int id = KeySink.ID_SYSTEM_KEYBOARD + event.getKeyCode();
        trace("  → keysym 0x" + Integer.toHexString(keysym)
                + (action == KeyEvent.ACTION_DOWN ? " down" : " up"));
        if (action == KeyEvent.ACTION_DOWN) {
            sink.keyDown(keysym, id);
        } else {
            sink.keyUp(id);
            watcher.sent(keysym);
        }
        return true;
    }

    /**
     * {@link Keysym#forCharacter}, not {@code fromUnicode}: an IME can commit a
     * newline as <em>text</em> rather than send it as a key — LatinIME does,
     * depending on what it was in the middle of — and the X11 encoding of code
     * point 10 arrives as {@code NoSymbol}, which is what an Enter that only
     * sometimes works looks like.
     */
    private void type(CharSequence text) {
        if (text == null) {
            return;
        }
        for (int i = 0; i < text.length(); ) {
            final int cp = Character.codePointAt(text, i);
            i += Character.charCount(cp);
            send(Keysym.forCharacter(cp));
        }
    }

    private boolean shiftHeld() {
        return watcher.heldModifiers().contains(Keysym.SHIFT_L);
    }

    private boolean otherModifierHeld() {
        final Set<Integer> held = watcher.heldModifiers();
        return !held.isEmpty() && !held.contains(Keysym.SHIFT_L);
    }

    private void send(int keysym) {
        trace("  → keysym 0x" + Integer.toHexString(keysym));
        if (keysym == 0) {
            return;
        }
        sink.keyDown(keysym, KeySink.ID_TEXT);
        sink.keyUp(KeySink.ID_TEXT);
        watcher.sent(keysym);
    }
}
