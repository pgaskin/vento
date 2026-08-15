// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.input;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public class TouchLogTest {

    @Test
    public void parsesAFixture() {
        final TouchLog.Log log = TouchLog.parse("""
                # format remotedesktop-touch-1
                # label two-finger-tap
                # density 2.625

                0 D 0 | 0 540.0 300.0
                16 M | 0 541.5 300.5
                20 D 1 | 0 541.5 300.5 | 1 700.0 320.0
                150 U 1 | 0 541.5 300.5 | 1 700.0 320.0
                160 U 0 | 0 541.5 300.5
                170 C
                """);

        assertEquals(TouchLog.FORMAT, log.meta().get("format"));
        assertEquals("two-finger-tap", log.meta().get("label"));
        assertEquals(2.625f, log.metaFloat("density", 0), 1e-6);
        assertEquals(6, log.frames().size());

        final TouchFrame down2 = log.frames().get(2);
        assertEquals(TouchFrame.Action.DOWN, down2.action);
        assertEquals(1, down2.index);
        assertEquals(20, down2.time);
        assertEquals(2, down2.count);
        assertEquals(1, down2.id[1]);
        assertEquals(700.0f, down2.x[1], 1e-6);

        final TouchFrame move = log.frames().get(1);
        assertEquals(TouchFrame.Action.MOVE, move.action);
        assertEquals(1, move.count);

        assertEquals(TouchFrame.Action.CANCEL, log.frames().get(5).action);
        assertEquals(0, log.frames().get(5).count);
    }

    @Test
    public void writesWhatItCanReadBack() {
        final TouchLog.Writer w = new TouchLog.Writer().meta("label", "tap");
        w.add(new TouchFrame().set(TouchFrame.Action.DOWN, 0, 1000).add(0, 10, 20));
        w.add(new TouchFrame().set(TouchFrame.Action.MOVE, 0, 1016).add(0, 12, 22));
        w.add(new TouchFrame().set(TouchFrame.Action.UP, 0, 1032).add(0, 12, 22));

        final TouchLog.Log log = TouchLog.parse(w.text());
        assertEquals("tap", log.meta().get("label"));
        assertEquals(3, log.frames().size());
        assertEquals("times are rebased onto the first frame", 0, log.frames().get(0).time);
        assertEquals(16, log.frames().get(1).time);
        assertEquals(32, log.frames().get(2).time);
        assertEquals(12.0f, log.frames().get(2).x[0], 1e-6);
        assertTrue(w.text().startsWith("# format " + TouchLog.FORMAT));
    }
}
