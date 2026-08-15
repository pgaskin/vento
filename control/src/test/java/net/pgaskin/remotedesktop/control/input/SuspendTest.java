// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.input;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

import net.pgaskin.remotedesktop.control.harness.Harness;

import org.junit.Test;

import java.util.List;
import java.util.Map;

/**
 * The session leaving the screen, from the input stack's side.
 *
 * <p>One question, asked of every way this stack can be holding something down:
 * after {@code Harness.suspend()} — the three calls a host makes when its
 * session leaves the screen — is the remote holding anything? It must not be. A button or a
 * modifier held here is held <em>there</em>, and there is nobody left to lift
 * it: the finger that would have is gone with the screen.
 *
 * <p>The other half is that suspending is not a teardown. The stack has to work
 * afterwards, because the same view comes back when the session does.
 */
public class SuspendTest {

    private static final float[] OVERLAY_LEFT = {400, 1000};
    /** The extension keyboard's Ctrl, at the harness's geometry. */
    private static final float KBD_CTRL_X = 150, KBD_ROW_Y = 1000;

    // ---- what a suspend has to let go of ----------------------------------

    /**
     * The 250 ms click window is the case worth having: the gesture is over,
     * every finger is up, and a button is deliberately still down — which is
     * exactly the state a normal cancel is defined to leave alone.
     */
    @Test
    public void aButtonHeldByTheClickWindowIsReleased() {
        final Harness h = Harness.improved();
        h.down(0, 500, 500).up(0);
        assertEquals(List.of("down LEFT"), h.buttonEvents());

        h.suspend();
        assertEquals(List.of("down LEFT", "up LEFT"), h.buttonEvents());
        assertEquals(0, h.cursor.buttons());

        // ... and the timer that would have released it does not fire a second
        // release into whatever the next session is doing.
        h.advance(h.cfg.clickHoldMs * 2);
        assertEquals(List.of("down LEFT", "up LEFT"), h.buttonEvents());
    }

    /** Mid-drag: a finger down, a button down, and no up coming. */
    @Test
    public void aDragInProgressIsEnded() {
        final Harness h = Harness.improved();
        h.down(0, 500, 500).up(0).down(0, 500, 500)   // double-tap-drag
                .move(0, 600, 500).move(0, 700, 500);
        assertEquals(1, h.cursor.buttons());

        h.suspend();
        assertEquals(0, h.cursor.buttons());
        assertEquals(0, h.gestures.downCount());
    }

    /** The overlay's hold is the other producer of a button mask. */
    @Test
    public void theOverlaysHeldButtonIsReleased() {
        final Harness h = Harness.improved().withOverlay().reset();
        h.down(0, OVERLAY_LEFT[0], OVERLAY_LEFT[1]);
        assertEquals(1, h.cursor.buttons());

        h.suspend();
        assertEquals(List.of("ovl down LEFT", "ovl up LEFT"), h.overlayEvents());
        assertEquals(0, h.cursor.buttons());
    }

    /**
     * A locked modifier is the longest-lived hold in the stack — it survives
     * every gesture by design, which is what "locked" means — so it is the one
     * most likely to be left behind at the far end.
     */
    @Test
    public void aLockedModifierIsReleased() {
        final Harness h = Harness.improved().withKeyboard().reset();
        h.down(0, KBD_CTRL_X, KBD_ROW_Y).up(0)
                .down(0, KBD_CTRL_X, KBD_ROW_Y).up(0);   // two taps: locked
        assertEquals(List.of("key down Ctrl"), h.keys);
        assertEquals(1, h.held.size());

        h.suspend();
        assertEquals(List.of("key down Ctrl", "key up Ctrl"), h.keys);
        assertEquals("the far end is holding nothing", Map.of(), h.held);
    }

    /** A glide would go on moving somebody's cursor around an unwatched desktop. */
    @Test
    public void aGlideIsStopped() {
        final Harness h = Harness.improved();
        h.step = 10;
        h.down(0, 500, 500);
        for (int i = 1; i <= 6; i++) {
            h.move(0, 500 + i * 120f, 500);
        }
        h.up(0).advance(h.cfg.inertiaStartDelayMs + h.cfg.inertiaTickMs * 3);
        assertTrue("the flick should be gliding", h.gestures.glideSpeed() > 0);

        h.suspend().reset();
        h.advance(1000);
        assertEquals("no motion after the session left", List.of(), h.mouse);
    }

    // ---- and what it must not break ---------------------------------------

    @Test
    public void theStackStillWorksAfterwards() {
        final Harness h = Harness.improved().withOverlay();
        h.down(0, 500, 500).move(0, 600, 500);
        h.suspend().reset();

        h.tap(700, 500);
        assertEquals(List.of("down LEFT", "up LEFT"), h.buttonEvents());

        h.reset();
        h.down(0, OVERLAY_LEFT[0], OVERLAY_LEFT[1]).up(0);
        assertEquals(List.of("ovl down LEFT", "ovl up LEFT"), h.overlayEvents());
    }

    /** The keyboard is the other half of that, on its own view: the two
     *  widgets share the bottom of the screen and never share a session. */
    @Test
    public void theKeyboardStillWorksAfterwards() {
        final Harness h = Harness.improved().withKeyboard().reset();
        h.down(0, KBD_CTRL_X, KBD_ROW_Y).up(0);
        h.suspend().reset();

        h.down(0, KBD_CTRL_X, KBD_ROW_Y).up(0);
        assertEquals(List.of("key down Ctrl"), h.keys);
    }

    /**
     * Suspending twice, or suspending with nothing down at all, says nothing to
     * the remote. Both happen: a session can be stopped without ever having been
     * touched.
     */
    @Test
    public void anIdleSuspendIsSilent() {
        final Harness h = Harness.improved().withOverlay().withKeyboard().reset();
        h.suspend().suspend();
        assertEquals(List.of(), h.mouse);
        assertEquals(List.of(), h.keys);
        assertEquals(List.of(), h.pointer);
    }
}
