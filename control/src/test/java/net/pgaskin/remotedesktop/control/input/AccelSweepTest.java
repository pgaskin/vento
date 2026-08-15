// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.input;

import net.pgaskin.remotedesktop.control.harness.Harness;

import org.junit.Assume;
import org.junit.Test;

import java.io.File;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.Locale;
import java.util.function.Consumer;

/**
 * Not a test: the analysis harness the acceleration variants were chosen with,
 * kept checked in so the numbers can be reproduced. It replays every recording
 * through the stack once per variant and prints what each does to the travel,
 * to the ripple, and to how often the axis lock engages.
 *
 * <pre>
 *   ./gradlew :control:testDebugUnitTest -Dremotedesktop.accelSweep=true \
 *       --tests '*AccelSweepTest'
 * </pre>
 *
 * <p>{@code -Dremotedesktop.recDir=DIR} adds a directory of recordings outside
 * the fixture set, for a question the fixtures were not recorded to answer.
 *
 * <p>Skipped otherwise, since its output is a table to read rather than an
 * assertion — the thresholds it exists to choose are a feel question, and the
 * regression net for the code is {@link PointerAccelTest} plus the goldens.
 *
 * <p>One pitfall, which cost the first run of this analysis: the whole frame
 * list goes through {@code play} in one call, because the clock is rebased on
 * the first frame. {@link #replay} does its own loop for the per-frame
 * sampling, and rebases exactly once.
 */
public class AccelSweepTest {

    /** Time to let the click window and any glide finish, ms. */
    private static final long SETTLE_MS = 2000;

    private interface Variant {
        void apply(Config c);
    }

    /**
     * The baseline is the original's curve with nothing of ours on it, so every
     * row reads as a difference from the port — which is not what
     * {@link Config#improved} is any more (it ships ADAPT+LOCK).
     */
    private static final String[] NAMES = {
            "SAWTOOTH", "SMOOTH", "ADAPTIVE", "AXISLOCK", "ADAPT+LOCK",
    };

    private static final Variant[] VARIANTS = {
            c -> { },
            c -> c.accelDrainHistory = false,
            c -> c.accelAdaptive = true,
            c -> c.axisLockEnabled = true,
            c -> { c.accelAdaptive = true; c.axisLockEnabled = true; },
    };

    @Test
    public void sweep() throws IOException {
        Assume.assumeTrue("set -Dremotedesktop.accelSweep=true to run the sweep",
                Boolean.getBoolean("remotedesktop.accelSweep"));

        final List<File> files = new ArrayList<>();
        collect(files, dir("src/test/fixtures"));
        collect(files, dir(System.getProperty("remotedesktop.recDir", "")));

        System.out.println();
        System.out.printf(Locale.ROOT, "%-26s %-10s %7s %7s %7s %6s %6s %6s %6s %6s%n",
                "recording", "variant", "travel", "net", "glide", "vs saw",
                "ripple", "fmax", "f>=2%", "lock%");
        for (File f : files) {
            final Result[] rs = new Result[VARIANTS.length];
            for (int i = 0; i < VARIANTS.length; i++) {
                rs[i] = replay(f, VARIANTS[i]);
            }
            if (rs[0].moves == 0) {
                continue; // multi-finger only: nothing goes through the accel
            }
            System.out.println();
            System.out.printf(Locale.ROOT,
                    "%-26s %d moves, %.0f finger px, density %.3f, speed dp/ms p50 %.2f p90 %.2f max %.2f%n",
                    f.getName().replaceAll("\\.touch$", ""), rs[0].moves, rs[0].fingerPx,
                    rs[0].density, rs[0].speedP50, rs[0].speedP90, rs[0].speedMax);
            for (int i = 0; i < VARIANTS.length; i++) {
                System.out.printf(Locale.ROOT,
                        "%-26s %-10s %7.0f %7.0f %7.0f %6.2f %6.2f %6.2f %6.0f %6.0f%n",
                        "", NAMES[i], rs[i].travel, rs[i].net, rs[i].glide,
                        rs[0].travel > 0 ? rs[i].travel / rs[0].travel : 0,
                        rs[i].ripple, rs[i].factorMax, rs[i].spikePct, rs[i].lockedPct);
            }
        }
        System.out.println();
    }

    // ---- one replay -------------------------------------------------------

    private static final class Result {
        int moves;
        float density;
        double fingerPx, travel, net, glide, ripple, lockedPct;
        double speedP50, speedP90, speedMax;
        double factorMax, spikePct;
    }

    private static Result replay(File file, Variant variant) throws IOException {
        final TouchLog.Log log;
        try (java.io.Reader r = Files.newBufferedReader(file.toPath(), StandardCharsets.UTF_8)) {
            log = TouchLog.parse(r);
        }
        final float density = log.metaFloat("density", Harness.DENSITY);
        final Config cfg = "faithful".equals(log.meta("preset", "improved"))
                ? Config.faithful(density) : Config.improved(density);
        // Strip ours back off, whatever the preset ships, so the variant column
        // is the only thing that differs between rows.
        cfg.accelAdaptive = false;
        cfg.axisLockEnabled = false;
        variant.apply(cfg);

        final String[] view = log.meta("view", Harness.VIEW_W + " " + Harness.VIEW_H)
                .trim().split("\\s+");
        final Harness h = new Harness(cfg,
                Integer.parseInt(view[0]), Integer.parseInt(view[1]));

        final Result out = new Result();
        out.density = density;
        final List<Double> speeds = new ArrayList<>();
        final int[] locked = {0, 0, 0};   // locked, sampled, factor >= 2
        final double[] factorMax = {0};

        play(h, log.frames(), unused -> {
            if (h.gestures.downCount() == 1 && h.gestures.moving()) {
                speeds.add((double) h.gestures.accelSpeed() / density);
                locked[1]++;
                if (h.gestures.axisLock() != PointerAccel.Axis.NONE) {
                    locked[0]++;
                }
                final float f = h.gestures.accelFactor();
                factorMax[0] = Math.max(factorMax[0], f);
                if (f >= 2.0f) {
                    locked[2]++;
                }
            }
        });

        out.moves = speeds.size();
        out.fingerPx = fingerPath(log.frames());
        final List<double[]> driven = moves(h.mouse);
        out.travel = sum(driven);
        out.net = net(driven);
        out.ripple = ripple(driven);
        h.advance(SETTLE_MS);
        out.glide = sum(moves(h.mouse)) - out.travel;
        out.lockedPct = locked[1] == 0 ? 0 : 100.0 * locked[0] / locked[1];
        out.spikePct = locked[1] == 0 ? 0 : 100.0 * locked[2] / locked[1];
        out.factorMax = factorMax[0];

        final double[] s = speeds.stream().mapToDouble(Double::doubleValue).sorted().toArray();
        out.speedP50 = percentile(s, 0.50);
        out.speedP90 = percentile(s, 0.90);
        out.speedMax = s.length == 0 ? 0 : s[s.length - 1];
        return out;
    }

    /** {@link Harness#play}, with a hook after each frame. */
    private static void play(Harness h, List<TouchFrame> frames, Consumer<Harness> after) {
        if (frames.isEmpty()) {
            return;
        }
        final long base = frames.get(0).time;
        for (TouchFrame f : frames) {
            h.clock.advanceTo(f.time - base);
            final TouchFrame g = f.copy();
            g.time = h.clock.now();
            h.router.handle(g);
            after.accept(h);
        }
    }

    // ---- metrics ----------------------------------------------------------

    private static List<double[]> moves(List<String> mouse) {
        final List<double[]> out = new ArrayList<>();
        for (String l : mouse) {
            if (l.startsWith("move ")) {
                final String[] p = l.substring(5).split(",");
                out.add(new double[]{Double.parseDouble(p[0]), Double.parseDouble(p[1])});
            }
        }
        return out;
    }

    private static double sum(List<double[]> ms) {
        double t = 0;
        for (double[] m : ms) {
            t += Math.hypot(m[0], m[1]);
        }
        return t;
    }

    /**
     * Net displacement — where the cursor ended up, as opposed to how far it
     * travelled getting there. The two columns say different things: a variant
     * can shorten the path (cancelled jitter) without moving the destination,
     * and the axis lock does exactly that.
     */
    private static double net(List<double[]> ms) {
        double x = 0, y = 0;
        for (double[] m : ms) {
            x += m[0];
            y += m[1];
        }
        return Math.hypot(x, y);
    }

    /**
     * How uneven consecutive emitted displacements are: mean absolute step
     * change over mean magnitude. The {@code 1.1, 1.1, 5.0} sawtooth is exactly
     * what this is meant to see. Kept after output spreading was deleted,
     * because it is what measured the ripple that turned out to be
     * imperceptible, and any future attempt on it starts here.
     */
    private static double ripple(List<double[]> ms) {
        if (ms.size() < 2) {
            return 0;
        }
        double mean = 0, step = 0;
        double prev = Math.hypot(ms.get(0)[0], ms.get(0)[1]);
        mean += prev;
        for (int i = 1; i < ms.size(); i++) {
            final double m = Math.hypot(ms.get(i)[0], ms.get(i)[1]);
            step += Math.abs(m - prev);
            mean += m;
            prev = m;
        }
        mean /= ms.size();
        step /= ms.size() - 1;
        return mean == 0 ? 0 : step / mean;
    }

    /** Path length of the raw touch positions, single-finger frames only. */
    private static double fingerPath(List<TouchFrame> frames) {
        double total = 0;
        float px = 0, py = 0;
        boolean have = false;
        for (TouchFrame f : frames) {
            if (f.count != 1) {
                have = false;
                continue;
            }
            final float x = f.x[0], y = f.y[0];
            if (have && f.action == TouchFrame.Action.MOVE) {
                total += Math.hypot(x - px, y - py);
            }
            px = x;
            py = y;
            have = true;
        }
        return total;
    }

    private static double percentile(double[] sorted, double p) {
        if (sorted.length == 0) {
            return 0;
        }
        return sorted[Math.min(sorted.length - 1, (int) (p * sorted.length))];
    }

    // ---- files ------------------------------------------------------------

    private static void collect(List<File> into, File dir) {
        if (dir == null) {
            return;
        }
        final File[] fs = dir.listFiles((d, n) -> n.endsWith(".touch"));
        if (fs != null) {
            Arrays.sort(fs);
            into.addAll(Arrays.asList(fs));
        }
    }

    private static File dir(String path) {
        if (path.isEmpty()) {
            return null;
        }
        final File f = new File(path);
        return f.isDirectory() ? f : null;
    }
}
