// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.input;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

import net.pgaskin.remotedesktop.control.harness.Harness;

import org.junit.Test;

import java.util.ArrayList;
import java.util.List;

/**
 * The detent the far end's cursor opens: what it costs, and the five tests that
 * decide whether it opens at all.
 */
public class HoverAssistTest {

    private static final class Sink implements MouseSink {
        final List<float[]> moves = new ArrayList<>();

        @Override public void mouseMove(float dx, float dy) { moves.add(new float[]{dx, dy}); }
        @Override public void mouseDown(int mask) { }
        @Override public void mouseUp(int mask) { }

        float travelled() {
            float d = 0;
            for (float[] m : moves) {
                d += (float) Math.sqrt(m[0] * m[0] + m[1] * m[1]);
            }
            return d;
        }
    }

    /**
     * The assist on its own: no acceleration and no lock, so what comes out is
     * the finger's own distance times the detent and the arithmetic is the
     * mechanism's rather than three mechanisms'.
     */
    private static Config cfg() {
        final Config c = Config.improved(Harness.DENSITY);
        c.accelEnabled = false;
        c.axisLockEnabled = false;
        c.hoverAssistEnabled = true;
        return c;
    }

    /** 0.25 px/ms, well inside every arming band. */
    private static final float SLOW_PX = 2.0f;
    private static final long GAP_MS = 8;

    /**
     * A straight drag to the right, with the far end changing its cursor after
     * {@code changeAt} events and again on every {@code every} after that.
     * {@code changeAt < 0} is the run with no news in it at all.
     */
    private static Sink drag(Config c, int events, float perEvent, int changeAt, int every) {
        final Sink out = new Sink();
        final PointerAccel a = new PointerAccel(c, out);
        for (int i = 0; i < events; i++) {
            final long t = i * GAP_MS;
            a.move(perEvent, 0, t);
            if (changeAt >= 0 && i >= changeAt && (every <= 0 ? i == changeAt : (i - changeAt) % every == 0)) {
                a.remoteCursorChanged(t, false);
            }
        }
        return out;
    }

    @Test
    public void aChangeCostsTheDetentAndNothingElse() {
        final Config c = cfg();
        final Sink plain = drag(c, 60, SLOW_PX, -1, 0);
        final Sink news = drag(c, 60, SLOW_PX, 12, 0);

        // Identical up to the change: the detent is applied ahead of the news,
        // never behind it.
        for (int i = 0; i <= 12; i++) {
            assertEquals(plain.moves.get(i)[0], news.moves.get(i)[0], 1e-4);
        }

        // The whole cost is span * (1 - gain) / 2 of finger travel, which is
        // the area above a smoothstep from gain back to 1.
        final float withheld = c.hoverAssistSpanPx * (1 - c.hoverAssistGain) / 2;
        assertEquals(withheld, plain.travelled() - news.travelled(), withheld * 0.1f);

        // ... and once the budget is spent, the two runs move together again.
        final int spent = 12 + (int) Math.ceil(c.hoverAssistSpanPx / SLOW_PX) + 1;
        for (int i = spent; i < 60; i++) {
            assertEquals(plain.moves.get(i)[0], news.moves.get(i)[0], 1e-4);
        }
    }

    @Test
    public void nothingIsBanked() {
        final Config c = cfg();
        final Sink plain = drag(c, 60, SLOW_PX, -1, 0);
        final Sink news = drag(c, 60, SLOW_PX, 12, 0);

        // The withheld distance is discarded, so the assisted run is short from
        // the change onwards and never catches up.
        float shortfall = 0;
        for (int i = 0; i < 60; i++) {
            final float d = plain.moves.get(i)[0] - news.moves.get(i)[0];
            assertTrue("event " + i + " gave back " + -d, d >= -1e-4);
            shortfall += d;
        }
        assertTrue(shortfall > 0);
    }

    @Test
    public void aPurposefulDragDoesNotArm() {
        final Config c = cfg();
        // 1.25 px/ms, well above the 0.35 dp/ms band.
        final Sink plain = drag(c, 40, 10.0f, -1, 0);
        final Sink news = drag(c, 40, 10.0f, 12, 0);
        assertEquals(plain.travelled(), news.travelled(), 1e-3);
    }

    @Test
    public void aChangeWithNothingMovingDoesNotArm() {
        final Config c = cfg();
        final Sink out = new Sink();
        final PointerAccel a = new PointerAccel(c, out);
        for (int i = 0; i < 20; i++) {
            a.move(SLOW_PX, 0, i * GAP_MS);
        }
        // Two idle windows later, which is a desktop doing something of its own.
        a.remoteCursorChanged(20 * GAP_MS + c.hoverAssistIdleMs * 2, false);
        final float before = out.travelled();
        for (int i = 20; i < 30; i++) {
            a.move(SLOW_PX, 0, i * GAP_MS + c.hoverAssistIdleMs * 2 + 400);
        }
        assertEquals(10 * SLOW_PX, out.travelled() - before, 1e-3);
    }

    @Test
    public void twoChangesInTheSamePlaceArmOnce() {
        final Config c = cfg();
        final Sink out = new Sink();
        final PointerAccel a = new PointerAccel(c, out);
        int catches = 0;
        for (int i = 0; i < 40; i++) {
            final long t = i * GAP_MS;
            a.move(SLOW_PX, 0, t);
            if (i == 12 || i == 13) { // 2 px apart, inside the 6 dp travel test
                a.remoteCursorChanged(t, false);
            }
            if (a.hoverGain() == c.hoverAssistGain) {
                catches++;
            }
        }
        assertEquals(1, catches);
    }

    @Test
    public void anAnimatedCursorLocksItselfOut() {
        final Config c = cfg();
        final Sink out = new Sink();
        final PointerAccel a = new PointerAccel(c, out);
        int catches = 0;
        // A change every 96 ms for four seconds, with the finger crawling: the
        // one case the other four tests all pass.
        for (int i = 0; i < 500; i++) {
            final long t = i * GAP_MS;
            a.move(SLOW_PX, 0, t);
            if (i % 12 == 0) {
                a.remoteCursorChanged(t, false);
            }
            if (a.hoverGain() == c.hoverAssistGain) {
                catches++;
            }
        }
        // Three per burst window, then two seconds of nothing, over four
        // seconds of it.
        assertTrue("armed " + catches + " times", catches <= 2 * c.hoverAssistBurstCount);
    }

    @Test
    public void crossingManyThingsAtSpeedDoesNotLockOut() {
        final Config c = cfg();
        final Sink out = new Sink();
        final PointerAccel a = new PointerAccel(c, out);

        // A hand crossing a desktop full of small objects: a change every other
        // event, far too fast to arm anything. None of them may count towards a
        // burst, or the aim that follows would be locked out.
        long t = 0;
        for (int i = 0; i < 20; i++, t += GAP_MS) {
            a.move(10.0f, 0, t);
            if (i % 2 == 0) {
                a.remoteCursorChanged(t, false);
            }
        }
        assertTrue("not locked out", !a.hoverLockedOut(t));

        // ... and the slow approach right after it still gets its detent.
        final Sink plain = drag(c, 40, SLOW_PX, -1, 0);
        float assisted = 0;
        for (int i = 0; i < 40; i++, t += GAP_MS) {
            a.move(SLOW_PX, 0, t);
            if (i == 12) {
                a.remoteCursorChanged(t, false);
            }
        }
        for (int i = out.moves.size() - 40; i < out.moves.size(); i++) {
            assisted += out.moves.get(i)[0];
        }
        assertTrue("the detent still opens", plain.travelled() - assisted > 1);
    }

    @Test
    public void aGlideIsNotAFinger() {
        final Config c = cfg();
        final Sink out = new Sink();
        final PointerAccel a = new PointerAccel(c, out);
        for (int i = 0; i < 20; i++) {
            a.move(SLOW_PX, 0, i * GAP_MS);
        }
        a.gestureEnded();
        // Momentum is still carrying the pointer across things half a second
        // later. Nothing it runs into says anything about how late this far end
        // is, because no finger caused it.
        for (int i = 0; i < 6; i++) {
            a.remoteCursorChanged(20 * GAP_MS + 200L + i * 60, true);
        }
        assertEquals(0.0f, a.hoverLagMs(), 1e-4);
    }

    @Test
    public void aReversalDropsTheRestOfTheDetent() {
        final Config c = cfg();
        final Sink out = new Sink();
        final PointerAccel a = new PointerAccel(c, out);
        for (int i = 0; i < 14; i++) {
            final long t = i * GAP_MS;
            a.move(SLOW_PX, 0, t);
            if (i == 12) {
                a.remoteCursorChanged(t, false);
            }
        }
        assertTrue("in a detent", a.hoverGain() < 1);
        a.move(-SLOW_PX, 0, 14 * GAP_MS);
        assertEquals("full gain from the turn on", 1.0f, a.hoverGain(), 1e-4);
        a.move(-SLOW_PX, 0, 15 * GAP_MS);
        assertEquals(-SLOW_PX, out.moves.get(out.moves.size() - 1)[0], 1e-4);
    }

    @Test
    public void aFarEndMeasuredLateDoesNotArm() {
        final Config c = cfg();
        final Sink out = new Sink();
        final PointerAccel a = new PointerAccel(c, out);

        // Four gestures whose news arrives long after the finger has stopped,
        // which is what a server that polls its own screen looks like.
        long t = 0;
        for (int g = 0; g < 4; g++) {
            for (int i = 0; i < 20; i++, t += GAP_MS) {
                a.move(SLOW_PX, 0, t);
            }
            a.gestureEnded();
            t += c.hoverAssistMaxLagMs * 3;
            a.remoteCursorChanged(t, false);
            t += 500;
        }
        assertTrue("measured " + a.hoverLagMs() + " ms", a.hoverLagMs() > c.hoverAssistMaxLagMs);

        final float before = out.travelled();
        for (int i = 0; i < 20; i++, t += GAP_MS) {
            a.move(SLOW_PX, 0, t);
            if (i == 12) {
                a.remoteCursorChanged(t, false);
            }
        }
        assertEquals(20 * SLOW_PX, out.travelled() - before, 1e-3);
    }

    @Test
    public void offIsOff() {
        final Config c = cfg();
        c.hoverAssistEnabled = false;
        assertEquals(drag(c, 60, SLOW_PX, -1, 0).travelled(),
                drag(c, 60, SLOW_PX, 12, 0).travelled(), 1e-3);

        final Config f = Config.faithful(Harness.DENSITY);
        assertEquals(drag(f, 60, SLOW_PX, -1, 0).travelled(),
                drag(f, 60, SLOW_PX, 12, 0).travelled(), 1e-3);
    }
}
