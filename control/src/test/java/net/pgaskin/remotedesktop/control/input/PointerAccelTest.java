// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.input;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

import net.pgaskin.remotedesktop.control.harness.Harness;

import org.junit.Test;

import java.util.ArrayList;
import java.util.List;

/** The jerk-based acceleration curve and its every-3rd-event sawtooth. */
public class PointerAccelTest {

    private static final class Sink implements MouseSink {
        final List<float[]> moves = new ArrayList<>();

        @Override public void mouseMove(float dx, float dy) { moves.add(new float[]{dx, dy}); }
        @Override public void mouseDown(int mask) { }
        @Override public void mouseUp(int mask) { }
    }

    /**
     * The ported curve on its own. {@code improved} ships the adaptive gate and
     * the axis lock on; the tests for those turn
     * them back on one at a time, so that what each one adds stays visible.
     */
    private static Config cfg() {
        final Config c = Config.improved(Harness.DENSITY);
        c.accelAdaptive = false;
        c.axisLockEnabled = false;
        return c;
    }

    @Test
    public void constantVelocityGivesTheSawtooth() {
        final Config c = cfg();
        final Sink out = new Sink();
        final PointerAccel a = new PointerAccel(c, out);

        final float[] factors = new float[6];
        for (int i = 0; i < 6; i++) {
            a.move(10, 0, i * 8L);
            factors[i] = a.lastFactor();
        }

        // Only every third event has a full history to work with; the other two
        // get the 1.1 floor. Zero jerk still gives sqrt(1.1)^2.6 = 1.132.
        assertEquals(1.1f, factors[0], 1e-4);
        assertEquals(1.1f, factors[1], 1e-4);
        assertEquals(1.1319f, factors[2], 1e-3);
        assertEquals(1.1f, factors[3], 1e-4);
        assertEquals(1.1f, factors[4], 1e-4);
        assertEquals(1.1319f, factors[5], 1e-3);

        assertEquals(11.0f, out.moves.get(0)[0], 1e-3);
        assertEquals(10 * factors[2], out.moves.get(2)[0], 1e-3);
    }

    @Test
    public void slidingWindowRemovesTheSawtooth() {
        final Config c = cfg();
        c.accelDrainHistory = false;
        final PointerAccel a = new PointerAccel(c, new Sink());

        final float[] factors = new float[6];
        for (int i = 0; i < 6; i++) {
            a.move(10, 0, i * 8L);
            factors[i] = a.lastFactor();
        }
        for (int i = 2; i < 6; i++) {
            assertEquals("every event past the first two scales",
                    1.1319f, factors[i], 1e-3);
        }
    }

    @Test
    public void aBigJerkIsClampedToTheMaximum() {
        final Config c = cfg();
        final PointerAccel a = new PointerAccel(c, new Sink());
        a.move(0, 0, 0);
        a.move(200, 0, 8);
        a.move(200, 0, 16);
        assertEquals(c.accelMax, a.lastFactor(), 1e-4);
    }

    @Test
    public void accelerationCanBeTurnedOff() {
        final Config c = cfg();
        c.accelEnabled = false;
        final Sink out = new Sink();
        final PointerAccel a = new PointerAccel(c, out);
        for (int i = 0; i < 4; i++) {
            a.move(10, 5, i * 8L);
        }
        for (float[] m : out.moves) {
            assertEquals(10.0f, m[0], 1e-4);
            assertEquals(5.0f, m[1], 1e-4);
        }
    }

    /** Fast movement is amplified more than slow movement of the same shape. */
    @Test
    public void fasterMovementAcceleratesMore() {
        final Config c = cfg();
        final PointerAccel slow = new PointerAccel(c, new Sink());
        slow.move(2, 0, 0);
        slow.move(4, 0, 16);
        slow.move(6, 0, 32);

        final PointerAccel fast = new PointerAccel(c, new Sink());
        fast.move(20, 0, 0);
        fast.move(40, 0, 16);
        fast.move(60, 0, 32);

        assertTrue(fast.lastFactor() > slow.lastFactor());
    }

    // ---- adaptive gate (ours) --------------------------------------------

    /** Deltas chosen so the jerk curve fires: {@code dx[1] - dx[0]} is non-zero. */
    private static float[] run(Config c, float[][] deltas, long stepMs) {
        final PointerAccel a = new PointerAccel(c, new Sink());
        final float[] factors = new float[deltas.length];
        for (int i = 0; i < deltas.length; i++) {
            a.move(deltas[i][0], deltas[i][1], i * stepMs);
            factors[i] = a.lastFactor();
        }
        return factors;
    }

    private static final float[][] SLOW = {{1, 0}, {4, 0}, {4, 0}};
    private static final float[][] FAST = {{20, 0}, {80, 0}, {80, 0}};

    @Test
    public void adaptivePinsSlowMovementToTheFloor() {
        final Config c = cfg();
        assertTrue("the curve fires without the gate", run(c, SLOW, 16)[2] > c.accelMin);

        c.accelAdaptive = true;
        assertEquals("jerk at a precision speed is not amplification",
                c.accelMin, run(c, SLOW, 16)[2], 1e-4);
    }

    /** The whole point of the gate is that a flick is untouched by it. */
    @Test
    public void adaptiveLeavesFastMovementExactlyAlone() {
        final Config c = cfg();
        final float plain = run(c, FAST, 16)[2];
        c.accelAdaptive = true;
        assertEquals(plain, run(c, FAST, 16)[2], 1e-4);
        assertTrue("and that factor is a real amplification", plain > c.accelMin);
    }

    // ---- axis locking (ours) ---------------------------------------------

    @Test
    public void axisLockZeroesTheMinorAxisOfSlowNearAxialMovement() {
        final Config c = cfg();
        c.axisLockEnabled = true;
        final Sink out = new Sink();
        final PointerAccel a = new PointerAccel(c, out);
        a.move(2, 0, 0);
        a.move(2, 0, 16);
        a.move(2, 1, 32);   // one pixel of cross-axis wander
        assertEquals(PointerAccel.Axis.X, a.axisLock());
        assertEquals("the wander is dropped", 0.0f, out.moves.get(2)[1], 1e-6);
        assertTrue("the movement itself is not", out.moves.get(2)[0] > 0);
    }

    @Test
    public void axisLockDoesNotEngageAtSpeed() {
        final Config c = cfg();
        c.axisLockEnabled = true;
        final Sink out = new Sink();
        final PointerAccel a = new PointerAccel(c, out);
        a.move(40, 0, 0);
        a.move(40, 0, 16);
        a.move(40, 20, 32);
        assertEquals(PointerAccel.Axis.NONE, a.axisLock());
        assertTrue(out.moves.get(2)[1] > 0);
    }

    /**
     * A circle is near-axial four times per revolution, so the ratio test on its
     * own flattens four arcs of it. What tells a circle apart from a drag along
     * an axis is that its direction keeps turning while the ratio test is happy.
     */
    @Test
    public void aSlowCircleIsNotLocked() {
        for (float radius : new float[]{80, 150, 300}) {
            assertTrue("r=" + radius + " locked " + lockedPct(circle(radius, 3f)) + "%",
                    lockedPct(circle(radius, 3f)) < 10);
        }
    }

    /**
     * ... but a corner is not a curve. Selecting text down and then across has to
     * lock on both legs, so a sharp turn drops the direction history rather than
     * ageing it out over a whole span.
     */
    @Test
    public void bothLegsOfACornerAreLocked() {
        final List<float[]> l = new ArrayList<>();
        l.addAll(straight(150, 3f, true));
        l.addAll(straight(150, 3f, false));
        final int half = l.size() / 2;
        assertTrue("first leg " + lockedPct(l.subList(0, half)) + "%",
                lockedPct(l.subList(0, half)) > 80);
        assertTrue("second leg " + lockedPct(l) + "% overall",
                lockedPct(l) > 80);

        final Config c = cfg();
        c.axisLockEnabled = true;
        final PointerAccel a = new PointerAccel(c, new Sink());
        for (int i = 0; i < l.size(); i++) {
            a.move(l.get(i)[0], l.get(i)[1], i * 16L);
            if (i == half - 1) {
                assertEquals("down the page", PointerAccel.Axis.Y, a.axisLock());
            }
        }
        assertEquals("and then across it", PointerAccel.Axis.X, a.axisLock());
    }

    /** Share of events the lock was engaged for, ignoring the first few. */
    private static float lockedPct(List<float[]> deltas) {
        final Config c = cfg();
        c.axisLockEnabled = true;
        final PointerAccel a = new PointerAccel(c, new Sink());
        int locked = 0, n = 0;
        for (int i = 0; i < deltas.size(); i++) {
            a.move(deltas.get(i)[0], deltas.get(i)[1], i * 16L);
            if (i >= 10) {
                n++;
                if (a.axisLock() != PointerAccel.Axis.NONE) {
                    locked++;
                }
            }
        }
        return n == 0 ? 0 : 100f * locked / n;
    }

    /** Per-event deltas around a circle, at {@code step} px per 16 ms event. */
    private static List<float[]> circle(float r, float step) {
        final List<float[]> out = new ArrayList<>();
        final int n = (int) (2 * Math.PI * r / step);
        for (int i = 0; i < n; i++) {
            final double a = 2 * Math.PI * i / n, b = 2 * Math.PI * (i + 1) / n;
            out.add(new float[]{(float) (r * (Math.cos(b) - Math.cos(a))),
                    (float) (r * (Math.sin(b) - Math.sin(a)))});
        }
        return out;
    }

    /** A straight run with a hand's worth of cross-axis wobble on it. */
    private static List<float[]> straight(float len, float step, boolean vertical) {
        final List<float[]> out = new ArrayList<>();
        long seed = 12345;
        for (int i = 0; i < len / step; i++) {
            seed = seed * 6364136223846793005L + 1442695040888963407L;
            final float w = 0.6f * (((seed >>> 40) % 2000) / 1000.0f - 1.0f);
            out.add(vertical ? new float[]{w, step} : new float[]{step, w});
        }
        return out;
    }

    @Test
    public void axisLockReleasesWhenTheMovementTurns() {
        final Config c = cfg();
        c.axisLockEnabled = true;
        final PointerAccel a = new PointerAccel(c, new Sink());
        a.move(2, 0, 0);
        a.move(2, 0, 16);
        assertEquals(PointerAccel.Axis.X, a.axisLock());
        for (int i = 0; i < 4; i++) {
            a.move(2, 2, 32 + i * 16L); // a diagonal, well past the exit ratio
        }
        assertEquals(PointerAccel.Axis.NONE, a.axisLock());
    }

    /**
     * The history is not cleared between gestures in the original, so the first
     * event of a new drag is scaled by the jerk between two unrelated gestures.
     */
    @Test
    public void resetOnDownIsWhatSeparatesGestures() {
        final Config c = cfg();
        final PointerAccel a = new PointerAccel(c, new Sink());
        a.move(1, 0, 0);
        a.move(1, 0, 8);
        a.reset();
        a.move(50, 0, 1000);
        a.move(50, 0, 1008);
        assertEquals("history dropped, so still on the floor", 1.1f, a.lastFactor(), 1e-4);
    }
}
