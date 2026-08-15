// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.input;

/**
 * The IME's composing region, kept as <em>what has already been sent</em>.
 *
 * <p>An input method does not hand a client finished words. It hands it the same
 * word over and over as it is revised — {@code setComposingText("h")}, then
 * {@code ("he")}, then {@code ("hel")} for a Latin prediction, and for a Korean
 * or Japanese IME the <em>same</em> character position rewritten
 * ({@code "ㄱ"} → {@code "가"} → {@code "각"}). A client with a text field of its
 * own never notices: the field replaces the region and the field is the truth.
 * There is no field here — the document is on the other machine and the only
 * thing that reaches it is a key press — so a client that types each revision as
 * it arrives sends {@code h}, {@code he}, {@code hel} and the far end reads
 * {@code hhehel}.
 *
 * <p>So the region has to be remembered and each revision sent as a
 * <em>difference</em>: keep the common prefix, backspace the rest of what was
 * sent, type the rest of what is meant. The original arrives at the same place
 * by a different route, diffing a real hidden {@code EditText} in its
 * {@code TextWatcher} — the whole of a text field's behaviour for the price of
 * a text field's problems.
 *
 * <p>One backspace per <em>code point</em>, because a code point is what one
 * press of a Backspace key removes at the far end. What an editor there does
 * with a combining mark or a joined emoji is its business.
 */
public final class TextDelta {

    /** Where a difference goes. One call is one key press and release. */
    public interface Out {
        void backspace();

        void character(int codePoint);
    }

    /** What the far end has been told the composing region is. */
    private final StringBuilder sent = new StringBuilder();

    /** True while a composing region is open. */
    public boolean composing() {
        return sent.length() > 0;
    }

    /** The region was revised — {@code InputConnection.setComposingText}. */
    public void compose(CharSequence text, Out out) {
        replace(text, out);
    }

    /**
     * The region was accepted — {@code commitText}, or a completion picked from
     * the IME's list. The difference goes out the same way; what changes is that
     * the text is now the far end's and is never revised again.
     */
    public void commit(CharSequence text, Out out) {
        replace(text, out);
        sent.setLength(0);
    }

    /**
     * The region ended without changing — {@code finishComposingText}, or
     * anything that means the client can no longer say what the far end's
     * composing region contains: a key event, a deletion, a screen going away.
     * The characters stay where they were sent; only the memory of them goes.
     */
    public void finish() {
        sent.setLength(0);
    }

    private void replace(CharSequence text, Out out) {
        final CharSequence next = text == null ? "" : text;
        int common = 0;
        while (common < sent.length() && common < next.length()
                && sent.charAt(common) == next.charAt(common)) {
            common++;
        }
        // A common prefix may not end between the halves of a surrogate pair: if
        // it did, the pair's low half differs and the character it belongs to is
        // being replaced, so the whole character has to go.
        if (common > 0 && Character.isHighSurrogate(sent.charAt(common - 1))) {
            common--;
        }
        for (int i = sent.length(); i > common; ) {
            i -= Character.charCount(sent.codePointBefore(i));
            out.backspace();
        }
        for (int i = common; i < next.length(); ) {
            final int cp = Character.codePointAt(next, i);
            i += Character.charCount(cp);
            out.character(cp);
        }
        sent.setLength(0);
        sent.append(next);
    }
}
