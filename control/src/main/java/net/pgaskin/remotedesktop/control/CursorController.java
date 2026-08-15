// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control;

import net.pgaskin.remotedesktop.control.input.Button;
import net.pgaskin.remotedesktop.control.input.Config;
import net.pgaskin.remotedesktop.control.input.MouseSink;
import net.pgaskin.remotedesktop.control.input.Scheduler;

import java.util.ArrayList;
import java.util.List;

/**
 * Turns the gesture layer's screen-space deltas into absolute remote-desktop
 * pointer events. Ported from RealVNC Viewer's cursor driver.
 *
 * <p>The whole centre-follow behaviour is the four lines in {@link #mouseMove}:
 * move the cursor in <em>desktop</em> space, clamp it to the framebuffer, then
 * ask the viewport to re-centre on it. See {@link Viewport#centreOn}.
 *
 * <p>Unlike the original, motion is coalesced to one pointer event per frame
 * (button changes always flush immediately, so a wheel click's down/up pair is
 * never collapsed).
 */
public final class CursorController implements MouseSink {

    public interface PointerSink {
        /** Absolute position in desktop coords, plus the current button mask. */
        void pointerEvent(float x, float y, int buttons);

        /**
         * How far the pointer moved, in whole desktop pixels, for a far end that
         * owns the cursor — see {@link CursorController#setRelative}. Defaulted
         * because a sink that never asks for relative mode never sees one.
         */
        default void pointerEventRelative(int dx, int dy, int buttons) {
        }
    }

    public interface Listener {
        void onCursorChanged();

        /**
         * Every button somebody clicks with has just been let go, whichever
         * source was holding them — so a click, or a drag, has ended. The
         * wheel's pseudo-buttons are not among them: a scroll is not a click.
         *
         * <p>Separate from {@link #onCursorChanged} because that one fires for
         * every movement as well, and because this is an <em>edge</em>: the host
         * acts on the moment, not on the state. Defaulted, since a host with
         * nothing to consume never asks.
         */
        default void onButtonsReleased() {
        }
    }

    private final Config cfg;
    private final Viewport viewport;
    private final PointerSink out;
    private Listener listener;

    private float x, y; // desktop coords

    private int buttons; // the union of every source's mask: what the remote is told
    private int clicking; // the same, wheel excluded, for the edge in clickEdge
    private final List<int[]> sources = new ArrayList<>();
    private final int[] own = newSourceMask();

    private boolean relative; // the far end owns the cursor, so x and y mean nothing

    // Motion owed to the far end, in desktop pixels, not yet whole. The wire
    // carries integers, and a slow drag is a stream of deltas each smaller than
    // one, so rounding every event on its own would send nothing at all.
    private float owedX, owedY;

    private final Scheduler scheduler;
    private boolean framePending;
    private final Runnable frameCallback = () -> {
        framePending = false;
        emit();
    };

    private int eventCount;
    private int suppressedCount;

    // The last triple actually handed to out, for the dedupe.
    private float sentX, sentY;
    private int sentButtons;
    private boolean everSent;

    public CursorController(Config cfg, Viewport viewport, PointerSink out, Scheduler scheduler) {
        this.cfg = cfg;
        this.viewport = viewport;
        this.out = out;
        this.scheduler = scheduler;
    }

    public void setListener(Listener l) {
        this.listener = l;
    }

    /**
     * Which end owns the cursor.
     *
     * <p>Relative means the far end does, and the centre-follow behaviour in
     * {@link #mouseMove} stops running: the deltas go out as deltas, {@link #x}
     * and {@link #y} stand still, and the viewport has nothing to follow. What
     * survives is everything that was about the finger rather than the cursor —
     * the gesture recognition, the button masks, the wheel, and bump scroll,
     * which measures the finger against the edge of the <em>screen</em>.
     * {@code ARCHITECTURE.md} §3.16 has the rest.
     */
    public void setRelative(boolean relative) {
        if (this.relative == relative) {
            return;
        }
        this.relative = relative;
        owedX = 0;
        owedY = 0;
        // Whatever was last sent describes the other kind of session, and in
        // relative mode a "same as last time" test would swallow a real repeat
        // of the same delta.
        everSent = false;
        changed();
    }

    public boolean isRelative() {
        return relative;
    }

    public float x() {
        return x;
    }

    public float y() {
        return y;
    }

    public int buttons() {
        return buttons;
    }

    public float screenX() {
        return viewport.toScreenX(x);
    }

    public float screenY() {
        return viewport.toScreenY(y);
    }

    /** Number of pointer events actually emitted so far (for the HUD). */
    public int eventCount() {
        return eventCount;
    }

    /** Events the dedupe swallowed because nothing had changed (for the HUD). */
    public int suppressedCount() {
        return suppressedCount;
    }

    public void setPosition(float dx, float dy) {
        x = Viewport.clamp(dx, 0, viewport.desktopWidth() - 1);
        y = Viewport.clamp(dy, 0, viewport.desktopHeight() - 1);
        viewport.centreOn(x, y);
        changed();
    }

    /**
     * The window the desktop gets has changed shape — an overlay came up, the
     * IME appeared, a system bar went away — and the picture must not jump.
     *
     * <p>It would otherwise, and by half the inset. The viewport's origin is not
     * independent state: after every move it is
     * {@code clamp(contentCentre - cursor × scale)}, so once the content centre
     * moves, either the cursor moves or the desktop does. Re-centring on the
     * cursor picks the desktop, and the whole picture slides every time the
     * overlay is shown or hidden; moving the cursor instead leaves the desktop
     * where it was.
     *
     * <p>Per axis, because the choice only exists where the invariant holds. Up
     * against a desktop edge the origin is pinned by the clamp and the cursor is
     * <em>not</em> at the centre; there the desktop has to move, and dragging the
     * cursor to the middle would throw the pointer off what it was aimed at.
     */
    public void setInsets(int left, int top, int right, int bottom) {
        final boolean centredX = Math.abs(screenX() - viewport.centreScreenX()) <= 0.5f;
        final boolean centredY = Math.abs(screenY() - viewport.centreScreenY()) <= 0.5f;
        viewport.setInsets(left, top, right, bottom);
        // A window that grew can leave the picture smaller than the space for
        // it; nothing else here changes the scale.
        if (viewport.getScale() < viewport.minScale()) {
            viewport.centreOn(x, y, viewport.minScale());
        }
        final float nx = centredX ? viewport.toDesktopX(viewport.centreScreenX()) : x;
        final float ny = centredY ? viewport.toDesktopY(viewport.centreScreenY()) : y;
        if (nx != x || ny != y) {
            x = Viewport.clamp(nx, 0, viewport.desktopWidth() - 1);
            y = Viewport.clamp(ny, 0, viewport.desktopHeight() - 1);
            // The far end is told, because its idea of where the pointer is
            // decides what is highlighted under it. Not in relative mode: this is
            // our cursor moving to where the window now is, not the pointer
            // moving on the desktop.
            if (!relative) {
                post();
            }
        }
        viewport.centreOn(x, y);
        changed();
    }

    /** CursorDriver.b — put the cursor back at the middle of the viewport. */
    public void centreCursor(boolean send) {
        x = Viewport.clamp(viewport.toDesktopX(viewport.centreScreenX()),
                0, viewport.desktopWidth() - 1);
        y = Viewport.clamp(viewport.toDesktopY(viewport.centreScreenY()),
                0, viewport.desktopHeight() - 1);
        if (send && !relative) {
            post();
        }
        viewport.centreOn(x, y);
        changed();
    }

    // ---- MouseSink --------------------------------------------------------

    @Override
    public void mouseMove(float dx, float dy) {
        final float s = viewport.getScale();
        if (relative) {
            // Still divided by the scale: a finger crossing a zoomed-in screen
            // covers fewer desktop pixels than the same finger at 1×, and that
            // is as true of a delta as of a position.
            owedX += dx / s;
            owedY += dy / s;
            post();
            changed();
            return;
        }
        x = Viewport.clamp(x + dx / s, 0, viewport.desktopWidth() - 1);
        y = Viewport.clamp(y + dy / s, 0, viewport.desktopHeight() - 1);
        post();
        viewport.centreOn(x, y);
        changed();
    }

    @Override
    public void mouseDown(int mask) {
        press(own, mask);
    }

    @Override
    public void mouseUp(int mask) {
        unpress(own, mask);
    }


    // ---- button sources ---------------------------------------------------

    /**
     * A second, independent producer of button presses — the mouse overlay.
     *
     * <p>One owner of the mask cannot mean "last writer wins": the overlay holds
     * {@code LEFT} for a long drag, and a tap on the touchpad during it makes
     * the gesture layer press {@code LEFT} and release it 250 ms later, dropping
     * whatever was being dragged. So each producer keeps its own mask and the
     * remote is told the union — a button stays down until everyone holding it
     * lets go.
     */
    public MouseSink newButtonSource() {
        final int[] mask = newSourceMask();
        return new MouseSink() {
            @Override
            public void mouseMove(float dx, float dy) {
                CursorController.this.mouseMove(dx, dy);
            }

            @Override
            public void mouseDown(int m) {
                press(mask, m);
            }

            @Override
            public void mouseUp(int m) {
                unpress(mask, m);
            }

        };
    }

    private int[] newSourceMask() {
        final int[] m = new int[1];
        sources.add(m);
        return m;
    }

    // A button is part of what the cursor is, so a listener hears about one the
    // same way it hears about a move — after the event has gone, since the sink
    // may act on it. A host that draws the pointer's buttons, or one whose
    // "remote" is the same process and so sends no frame back, has nothing else
    // to repaint on: a press with no movement is invisible without this.
    private void press(int[] source, int mask) {
        source[0] |= mask;
        union();
        flush();
        clickEdge();
        changed();
    }

    private void unpress(int[] source, int mask) {
        source[0] &= ~mask;
        union();
        flush();
        clickEdge();
        changed();
    }

    // After the flush, so that whatever the host does about the click — letting
    // go of a modifier it was holding for it — reaches the far end after the
    // click itself rather than in the middle of it.
    private void clickEdge() {
        final int now = buttons & Button.CLICK_MASK;
        final int was = clicking;
        clicking = now;
        if (was != 0 && now == 0 && listener != null) {
            listener.onButtonsReleased();
        }
    }

    private void union() {
        int m = 0;
        for (int[] s : sources) {
            m |= s[0];
        }
        buttons = m;
    }

    // ---- event plumbing ---------------------------------------------------

    private void post() {
        if (!cfg.coalescePointerEvents) {
            emit();
            return;
        }
        if (!framePending) {
            framePending = true;
            scheduler.postFrame(frameCallback);
        }
    }

    private void flush() {
        if (framePending) {
            framePending = false;
            scheduler.removeFrame(frameCallback);
        }
        emit();
    }

    /**
     * Hand the current state to the remote, unless it is byte-for-byte what the
     * remote was last told. Only the wire message is skipped — the local cursor
     * and viewport have already been updated by the caller.
     */
    private void emit() {
        if (relative) {
            emitRelative();
            return;
        }
        if (cfg.dedupePointerEvents && everSent
                && x == sentX && y == sentY && buttons == sentButtons) {
            suppressedCount++;
            return;
        }
        everSent = true;
        sentX = x;
        sentY = y;
        sentButtons = buttons;
        eventCount++;
        out.pointerEvent(x, y, buttons);
    }

    /**
     * The relative half of {@link #emit}: send what is owed, keep the fraction.
     *
     * <p>The dedupe is the other one's mirror image: nothing has changed if
     * there is no movement owed and the mask has not moved. A second delta
     * identical to the first is a second real movement, and dropping it would
     * make a steady drag stop after one frame.
     */
    private void emitRelative() {
        final int dx = (int) owedX;
        final int dy = (int) owedY;
        if (dx == 0 && dy == 0 && everSent && buttons == sentButtons) {
            suppressedCount++;
            return;
        }
        owedX -= dx;
        owedY -= dy;
        everSent = true;
        sentButtons = buttons;
        eventCount++;
        out.pointerEventRelative(dx, dy, buttons);
    }

    private void changed() {
        if (listener != null) {
            listener.onCursorChanged();
        }
    }

    public String buttonsName() {
        return Button.maskName(buttons);
    }
}
