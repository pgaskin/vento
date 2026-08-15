// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.input;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

/**
 * The composing region as a difference. The cases are the ones a real IME
 * produces — a word grown a letter at a time, a syllable rewritten in place, a
 * prediction accepted, an emoji — because the failure this class exists to stop
 * is silent: every character is sent, just several times over.
 */
public class TextDeltaTest {

    /** The stream of key presses a difference turns into, as a readable string. */
    private static final class Log implements TextDelta.Out {
        final StringBuilder keys = new StringBuilder();

        @Override
        public void backspace() {
            keys.append('\b');
        }

        @Override
        public void character(int codePoint) {
            keys.appendCodePoint(codePoint);
        }

        /** What the far end holds, if it started empty and obeys a backspace. */
        String farEnd() {
            final StringBuilder s = new StringBuilder();
            for (int i = 0; i < keys.length(); ) {
                final int cp = keys.codePointAt(i);
                i += Character.charCount(cp);
                if (cp == '\b') {
                    if (s.length() > 0) {
                        s.deleteCharAt(s.length() - 1);
                        if (s.length() > 0 && Character.isHighSurrogate(s.charAt(s.length() - 1))) {
                            s.deleteCharAt(s.length() - 1);
                        }
                    }
                } else {
                    s.appendCodePoint(cp);
                }
            }
            return s.toString();
        }
    }

    /** A Latin IME growing a word: each revision appends, nothing is re-sent. */
    @Test
    public void aGrowingWordSendsOnlyWhatIsNew() {
        final TextDelta d = new TextDelta();
        final Log log = new Log();
        d.compose("h", log);
        d.compose("he", log);
        d.compose("hel", log);
        d.compose("hell", log);
        d.compose("hello", log);
        assertEquals("hello", log.keys.toString());
        assertEquals("hello", log.farEnd());
    }

    /**
     * The case the whole class is for: a syllabic IME rewriting the
     * <em>same</em> character position. Naively typed, three characters arrive
     * where one was meant.
     */
    @Test
    public void aRewrittenCharacterIsCorrectedRatherThanAppended() {
        final TextDelta d = new TextDelta();
        final Log log = new Log();
        d.compose("ㄱ", log);
        d.compose("가", log);
        d.compose("각", log);
        assertEquals("ㄱ\b가\b각", log.keys.toString());
        assertEquals("각", log.farEnd());
    }

    /** An autocorrection: the common prefix stays, the rest is rewritten. */
    @Test
    public void acorrectionBacksOverOnlyTheDifference() {
        final TextDelta d = new TextDelta();
        final Log log = new Log();
        d.compose("teh", log);
        log.keys.setLength(0);
        d.commit("the", log);
        // "t" is common; "eh" goes back and "he" replaces it.
        assertEquals("\b\bhe", log.keys.toString());
    }

    /** A committed region is finished: the next composition starts from nothing. */
    @Test
    public void commitClosesTheRegion() {
        final TextDelta d = new TextDelta();
        final Log log = new Log();
        d.compose("hi", log);
        d.commit("hi ", log);
        assertFalse(d.composing());
        d.compose("t", log);
        assertEquals("hi t", log.keys.toString());
        assertEquals("hi t", log.farEnd());
    }

    /**
     * {@code finishComposingText}: the characters stand and nothing goes out,
     * but the region is no longer this end's to revise.
     */
    @Test
    public void finishSendsNothingAndForgets() {
        final TextDelta d = new TextDelta();
        final Log log = new Log();
        d.compose("word", log);
        assertTrue(d.composing());
        d.finish();
        assertFalse(d.composing());
        d.compose("x", log);
        assertEquals("wordx", log.keys.toString());
    }

    /** A region cleared by the IME backs out everything it sent. */
    @Test
    public void anAbandonedRegionIsTakenBack() {
        final TextDelta d = new TextDelta();
        final Log log = new Log();
        d.compose("abc", log);
        d.compose("", log);
        assertEquals("abc\b\b\b", log.keys.toString());
        assertEquals("", log.farEnd());
        assertFalse(d.composing());
    }

    /**
     * One backspace per <em>code point</em>. An emoji is two chars and one key
     * press, and a prefix may never end between the halves of its pair.
     */
    @Test
    public void anAstralCharacterIsOneKeyPress() {
        final TextDelta d = new TextDelta();
        final Log log = new Log();
        d.compose("a😀", log);
        d.compose("a😃", log);       // the same high surrogate, a different pair
        assertEquals("a😀\b😃", log.keys.toString());
        assertEquals("a😃", log.farEnd());
    }

    /** Nulls are the same as an empty region, not a crash and not a character. */
    @Test
    public void nullIsAnEmptyRegion() {
        final TextDelta d = new TextDelta();
        final Log log = new Log();
        d.compose("ab", log);
        d.compose(null, log);
        assertEquals("ab\b\b", log.keys.toString());
        assertFalse(d.composing());
    }

    /** A revision that changes nothing sends nothing. */
    @Test
    public void anIdenticalRevisionIsSilent() {
        final TextDelta d = new TextDelta();
        final Log log = new Log();
        d.compose("same", log);
        log.keys.setLength(0);
        d.compose("same", log);
        assertEquals("", log.keys.toString());
    }
}
