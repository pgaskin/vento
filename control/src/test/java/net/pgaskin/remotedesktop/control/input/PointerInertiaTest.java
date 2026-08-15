// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.input;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

import net.pgaskin.remotedesktop.control.harness.FakeScheduler;
import net.pgaskin.remotedesktop.control.harness.Harness;

import org.junit.Test;

import java.util.ArrayList;
import java.util.List;

/** Flick momentum: the 50 ms arming delay, the decay, and the stop threshold. */
public class PointerInertiaTest {

    private static final class Sink implements MouseSink {
        final List<float[]> moves = new ArrayList<>();

        @Override public void mouseMove(float dx, float dy) { moves.add(new float[]{dx, dy}); }
        @Override public void mouseDown(int mask) { }
        @Override public void mouseUp(int mask) { }
    }

    private final Config cfg = Config.improved(Harness.DENSITY);
    private final FakeScheduler clock = new FakeScheduler();
    private final Sink out = new Sink();
    private final PointerInertia inertia = new PointerInertia(cfg, out, clock);

    /** Four evenly spaced 10 px samples, ending at t = 30. */
    private void flickRight() {
        for (int i = 0; i <= 3; i++) {
            clock.advanceTo(i * 10L);
            inertia.sample(10, 0, i * 10L);
        }
        inertia.flick();
    }

    @Test
    public void glideStartsAfterTheDelayAndDecays() {
        flickRight();

        clock.advance(49);
        assertEquals("nothing until the arming delay is up", 0, out.moves.size());

        clock.advance(1 + cfg.inertiaTickMs);
        assertEquals(1, out.moves.size());
        // speed = |dx[0] + dx[1]| * 25 / (t2 - t0) = 20 * 25 / 20
        assertEquals(25.0f, out.moves.get(0)[0], 1e-3);
        assertEquals(0.0f, out.moves.get(0)[1], 1e-3);

        clock.advance(cfg.inertiaTickMs);
        assertEquals(25.0 * cfg.inertiaDecay, out.moves.get(1)[0], 1e-3);
    }

    @Test
    public void glideStopsBelowTheStopSpeed() {
        flickRight();
        clock.advance(5000);

        assertTrue(out.moves.size() > 5);
        final float last = out.moves.get(out.moves.size() - 1)[0];
        assertTrue("the last step is the first at or below the stop speed",
                last <= cfg.inertiaStopSpeed / cfg.inertiaDecay);
        assertEquals("and then it is over", 0.0, inertia.speed(), 0.0);

        final int n = out.moves.size();
        clock.advance(1000);
        assertEquals(n, out.moves.size());
    }

    @Test
    public void aFurtherSampleCancelsThePendingGlide() {
        flickRight();
        clock.advance(20);
        inertia.sample(1, 0, clock.now()); // finger came back down / kept moving
        clock.advance(500);
        assertEquals(0, out.moves.size());
    }

    @Test
    public void cancelStopsAGlideInProgress() {
        flickRight();
        clock.advance(70);
        final int n = out.moves.size();
        assertTrue(n > 0);
        inertia.cancel();
        clock.advance(500);
        assertEquals(n, out.moves.size());
    }

    @Test
    public void speedIsClampedToTheMaximum() {
        for (int i = 0; i <= 3; i++) {
            clock.advanceTo(i * 2L);
            inertia.sample(500, 0, i * 2L);
        }
        inertia.flick();
        clock.advance(60);
        assertEquals(cfg.inertiaSpeedMax, out.moves.get(0)[0], 1e-3);
    }

    @Test
    public void directionIsTakenFromTheTwoOldestOfThreeSamples() {
        for (int i = 0; i <= 3; i++) {
            clock.advanceTo(i * 10L);
            inertia.sample(0, -10, i * 10L);
        }
        inertia.flick();
        clock.advance(60);
        assertEquals(0.0f, out.moves.get(0)[0], 1e-3);
        assertEquals(-25.0f, out.moves.get(0)[1], 1e-3);
    }

    /** A flick with fewer than three samples cannot start a glide. */
    @Test
    public void tooFewSamplesDoNotGlide() {
        clock.advanceTo(0);
        inertia.sample(10, 0, 0);
        inertia.flick();
        clock.advance(500);
        assertEquals(0, out.moves.size());
    }

    @Test
    public void momentumCanBeTurnedOff() {
        cfg.inertiaEnabled = false;
        flickRight();
        clock.advance(500);
        assertEquals(0, out.moves.size());
    }

    /**
     * Two move events do not glide. The original's sentinel makes the span it
     * divides by an absolute clock reading, so on a phone this has always been
     * true and under a test clock starting at zero it was not — which is the
     * one place a fixture could record something a device never does.
     */
    @Test
    public void twoSamplesDoNotGlide() {
        clock.advanceTo(0);
        inertia.sample(60, 0, 0);
        clock.advanceTo(16);
        inertia.sample(60, 0, 16);
        inertia.flick();
        clock.advanceTo(1016);
        assertTrue(!inertia.isGliding());
        assertEquals(0, out.moves.size());
    }
}
