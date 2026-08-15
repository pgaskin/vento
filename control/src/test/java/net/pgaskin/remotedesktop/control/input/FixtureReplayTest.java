// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.input;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;
import static org.junit.Assert.fail;

import net.pgaskin.remotedesktop.control.harness.Harness;

import org.junit.Test;

import java.io.File;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;

/**
 * Replays touch streams recorded on a real device through the whole input
 * stack and compares the emitted events against a checked-in golden file.
 *
 * <p>Fixtures live in {@code src/test/fixtures}: {@code NAME.touch} is the
 * recording ({@link TouchLog}), {@code NAME.expected} is what the stack does
 * with it. The script beside them records a new one — it arms the playground's
 * {@code RECORD} toggle on a device, prompts for each gesture, and pulls what
 * came back:
 *
 * <pre>
 *   ./src/test/fixtures/record-fixtures.sh
 * </pre>
 *
 * <p>A missing golden is created on the first run and the test fails, so that
 * new expectations always get read by a human before being committed. To
 * rewrite them all after a deliberate behaviour change:
 *
 * <pre>
 *   ./gradlew :control:test -Dremotedesktop.updateGolden=true
 * </pre>
 *
 * This is the only regression net for the multi-finger gestures, since
 * {@code adb shell input} cannot inject more than one pointer.
 */
public class FixtureReplayTest {

    /** Time to let the click-release window and any glide finish, ms. */
    private static final long SETTLE_MS = 1200;

    @Test
    public void fixturesStillProduceTheSameEvents() throws IOException {
        final File dir = fixtureDir();
        final File[] files = dir.listFiles((d, n) -> n.endsWith(".touch"));
        if (files == null || files.length == 0) {
            fail("no fixtures in " + dir.getAbsolutePath()
                    + " — record some with the RECORD toggle");
            return;
        }
        Arrays.sort(files);

        final List<String> failures = new ArrayList<>();
        for (File f : files) {
            final String name = f.getName().replaceAll("\\.touch$", "");
            final File golden = new File(dir, name + ".expected");
            final String actual = String.join("\n", replay(f)) + "\n";

            if (!golden.isFile() || Boolean.getBoolean("remotedesktop.updateGolden")) {
                Files.write(golden.toPath(), actual.getBytes(StandardCharsets.UTF_8));
                failures.add("wrote " + golden.getName() + " — review and re-run");
                continue;
            }
            final String expected = new String(Files.readAllBytes(golden.toPath()),
                    StandardCharsets.UTF_8);
            if (!expected.equals(actual)) {
                failures.add(name + ":\n--- expected ---\n" + expected
                        + "--- actual ---\n" + actual);
            }
        }
        assertEquals(String.join("\n", failures), 0, failures.size());
    }

    /**
     * Every recorded tap has to survive as a click under both presets.
     *
     * <p>Under {@code faithful} that is the vncpatch#1 margin: the worst of
     * these 20 hand taps emits 9 {@code ACTION_MOVE}s against the original's
     * "more than 10 ⇒ drag" test. One event of headroom, on this device, at
     * this touch report rate — which is exactly why the bug is device
     * dependent, and why the test is here.
     */
    @Test
    public void everyRecordedTapClicksUnderBothPresets() throws IOException {
        final File[] taps = fixtureDir().listFiles((d, n) -> n.matches("tap-\\d+\\.touch"));
        if (taps == null || taps.length == 0) {
            return;
        }
        Arrays.sort(taps);
        final List<String> failures = new ArrayList<>();
        for (File f : taps) {
            for (boolean faithful : new boolean[]{false, true}) {
                final List<String> buttons = run(f, faithful).buttonEvents();
                if (!buttons.equals(Arrays.asList("down LEFT", "up LEFT"))) {
                    failures.add(f.getName() + (faithful ? " [faithful] " : " [improved] ") + buttons);
                }
            }
        }
        assertEquals(String.join("\n", failures), 0, failures.size());
    }

    /**
     * The one recorded gesture that pinches and then travels, replayed twice:
     * with the cursor ours it is the pinch it has always been, and with the far
     * end owning the cursor the same fingers also move the picture.
     *
     * <p>The recording is from before the mode existed, which is the point of
     * replaying it in both — the touches are identical, so the whole of the
     * difference is what the stack does with them.
     */
    @Test
    public void aRecordedPinchThatTravelsPansOnlyWhenTheFarEndOwnsTheCursor() throws IOException {
        final File f = new File(fixtureDir(), "two-finger-spread-and-translate.touch");
        final Harness ours = run(f, null, false);
        final Harness theirs = run(f, null, true);

        assertEquals("the same pinch either way",
                ours.viewport.getScale(), theirs.viewport.getScale(), 1e-4);
        assertEquals("and the same pans asked for",
                Harness.count(ours.zoom, "zoomPanned"),
                Harness.count(theirs.zoom, "zoomPanned"));
        assertEquals("none of which moved the cursor's own picture", 0.0, ours.pannedPx, 0.0);
        assertTrue("the fingers travelled hundreds of pixels, and so did the picture",
                theirs.pannedPx > 300);
    }

    private static List<String> replay(File file) throws IOException {
        return run(file, null).all;
    }

    private static Harness run(File file, Boolean faithfulOverride) throws IOException {
        return run(file, faithfulOverride, null);
    }

    private static Harness run(File file, Boolean faithfulOverride, Boolean relativeOverride)
            throws IOException {
        final TouchLog.Log log;
        try (java.io.Reader r = Files.newBufferedReader(file.toPath(), StandardCharsets.UTF_8)) {
            log = TouchLog.parse(r);
        }

        final float density = log.metaFloat("density", Harness.DENSITY);
        final boolean faithful = faithfulOverride != null ? faithfulOverride
                : "faithful".equals(log.meta("preset", "improved"));
        final Config cfg = faithful ? Config.faithful(density) : Config.improved(density);

        final String[] view = log.meta("view", Harness.VIEW_W + " " + Harness.VIEW_H)
                .trim().split("\\s+");
        final Harness h = new Harness(cfg,
                Integer.parseInt(view[0]), Integer.parseInt(view[1]));
        // Absent from every fixture recorded before the mode existed, and the
        // default is the mode they were all recorded in.
        final boolean relative = relativeOverride != null ? relativeOverride
                : "relative".equals(log.meta("pointer", "absolute"));
        if (relative) {
            h.withRelativePointer();
        }

        h.play(log.frames());
        h.advance(SETTLE_MS);
        return h;
    }

    private static File fixtureDir() {
        return new File("src/test/fixtures");
    }
}
