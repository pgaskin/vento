// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.input;

import android.view.MotionEvent;

/**
 * A real mouse: relative motion, a button mask, and a wheel — none of it run
 * through gesture recognition.
 *
 * <p>That last part is the whole design. The original feeds a mouse through the
 * <em>touch</em> path, so its buttons go through tap/drag disambiguation, which
 * can only add latency and failure modes to something already unambiguous. Here
 * a mouse is its own producer feeding the same sinks the gesture layer does, so
 * the centre-follow desktop, the edge clamp and the zoom come free and none of
 * the gesture machinery is in the path. Acceleration and momentum go with it:
 * the jerk curve is tuned for a finger that lifts and re-lands, and what somebody
 * expects from their mouse is their own OS pointer profile, which was applied
 * before the event ever reached us. {@link Config#mouseSpeed} is the one dial.
 *
 * <p>{@code View.requestPointerCapture()} delivers <em>relative</em> deltas and
 * stops the local pointer leaving the window, which is what a remote desktop
 * wants and what the original never asks for: without it the remote pointer can
 * only go where the local one can and movement stops dead at the screen edge —
 * which is what the original's 32 ms edge auto-scroll exists to paper over. The
 * uncaptured path is kept anyway, since capture only holds while the window has
 * focus and a mouse should keep working while a sheet is open; it derives deltas
 * from successive positions, so it drives the same model rather than a second
 * one, minus the thing capture exists for.
 */
public final class PhysicalMouse {

    /** Told when the state a HUD would draw has changed. */
    public interface Listener {
        void mouseActivity();
    }

    // MotionEvent button bits, as literals so this class stays testable.
    private static final int BUTTON_PRIMARY = 1;
    private static final int BUTTON_SECONDARY = 2;
    private static final int BUTTON_TERTIARY = 4;
    private static final int BUTTON_BACK = 8;
    private static final int BUTTON_FORWARD = 16;

    private final Config cfg;
    private final MouseSink sink;
    private Listener listener;

    private int held;                    // its own button source, so nothing else's is lost
    private final WheelSteps wheel;  // the notches not yet worth a click live in here
    private float lastX, lastY;          // the last uncaptured position, to derive deltas from
    private boolean havePosition;
    private boolean seen;                // so the HUD can say "none"

    public PhysicalMouse(Config cfg, MouseSink sink) {
        this.cfg = cfg;
        this.sink = sink;
        this.wheel = new WheelSteps(sink);
    }

    public void setListener(Listener l) {
        this.listener = l;
    }

    public int heldMask() {
        return held;
    }

    public boolean seen() {
        return seen;
    }

    // ---- the model ---------------------------------------------------------

    /** Relative motion in screen pixels, exactly as the device reported it. */
    public void motion(float dx, float dy) {
        if (dx == 0 && dy == 0) {
            return;
        }
        seen = true;
        sink.mouseMove(dx * cfg.mouseSpeed, dy * cfg.mouseSpeed);
    }

    /**
     * The whole button state at once, which is how a mouse reports it — a
     * chorded press arrives as one event, which is why {@link MouseSink} takes a
     * mask rather than a button.
     */
    public void buttonState(int androidButtons) {
        final int now = toRfbMask(androidButtons);
        if (now == held) {
            return;
        }
        seen = true;
        final int pressed = now & ~held;
        final int released = held & ~now;
        held = now;
        // Released first: a chord that swaps one button for another should not
        // be seen by the far end as both held at once.
        if (released != 0) {
            sink.mouseUp(released);
        }
        if (pressed != 0) {
            sink.mouseDown(pressed);
        }
        changed();
    }

    /**
     * Wheel motion in notches: 1.0 is one detent, and a high-resolution wheel or
     * a laptop touchpad sends fractions of one. Accumulated rather than rounded,
     * so a precision wheel produces the same number of clicks per turn as a
     * detented one instead of a click per event or none at all.
     */
    public void scroll(float hscroll, float vscroll) {
        if (hscroll == 0 && vscroll == 0) {
            return;
        }
        seen = true;
        final float sign = cfg.naturalScrolling ? -1.0f : 1.0f;
        // Android's vertical axis is positive *up*; the wheel buttons count
        // down, as the gesture layer's do.
        wheel.add(hscroll * sign, -vscroll * sign, cfg.mouseWheelStep);
    }

    /**
     * The mouse is gone — the window lost focus, the session left the screen.
     * Everything it holds is held <em>there</em>, so it is let go of, on the
     * same argument as {@link TouchRouter#cancel}.
     */
    public void cancel() {
        havePosition = false;
        wheel.reset();
        if (held != 0) {
            sink.mouseUp(held);
            held = 0;
            changed();
        }
    }

    /**
     * Android's button bits as RFB's. Back and forward are buttons 8 and 9,
     * which is what X11 calls them and what both desktops mean by them; the
     * original has no concept of either.
     */
    static int toRfbMask(int androidButtons) {
        int m = 0;
        if ((androidButtons & BUTTON_PRIMARY) != 0) m |= Button.LEFT.mask();
        if ((androidButtons & BUTTON_SECONDARY) != 0) m |= Button.RIGHT.mask();
        if ((androidButtons & BUTTON_TERTIARY) != 0) m |= Button.MIDDLE.mask();
        if ((androidButtons & BUTTON_BACK) != 0) m |= Button.BACK.mask();
        if ((androidButtons & BUTTON_FORWARD) != 0) m |= Button.FORWARD.mask();
        return m;
    }

    private void changed() {
        if (listener != null) {
            listener.mouseActivity();
        }
    }

    // ---- the Android adapters ----------------------------------------------

    /**
     * A captured pointer: {@code getX()}/{@code getY()} are already deltas, and
     * every button and wheel event arrives here rather than through the other two.
     */
    public boolean onCapturedPointerEvent(MotionEvent ev) {
        switch (ev.getActionMasked()) {
            case MotionEvent.ACTION_MOVE, MotionEvent.ACTION_HOVER_MOVE -> {
                // Batched samples first: a 1000 Hz mouse delivers several per
                // frame and dropping them would quietly halve its resolution.
                for (int h = 0; h < ev.getHistorySize(); h++) {
                    motion(ev.getHistoricalX(h), ev.getHistoricalY(h));
                }
                motion(ev.getX(), ev.getY());
            }
            case MotionEvent.ACTION_BUTTON_PRESS, MotionEvent.ACTION_BUTTON_RELEASE,
                 MotionEvent.ACTION_DOWN, MotionEvent.ACTION_UP ->
                    buttonState(ev.getButtonState());
            case MotionEvent.ACTION_SCROLL -> scroll(
                    ev.getAxisValue(MotionEvent.AXIS_HSCROLL),
                    ev.getAxisValue(MotionEvent.AXIS_VSCROLL));
            default -> {
                return false;
            }
        }
        return true;
    }

    /** An uncaptured mouse hovering over the view, plus its wheel. */
    public boolean onGenericMotionEvent(MotionEvent ev) {
        if (!isMouse(ev)) {
            return false;
        }
        switch (ev.getActionMasked()) {
            case MotionEvent.ACTION_HOVER_MOVE -> {
                for (int h = 0; h < ev.getHistorySize(); h++) {
                    moveTo(ev.getHistoricalX(h), ev.getHistoricalY(h));
                }
                moveTo(ev.getX(), ev.getY());
                buttonState(ev.getButtonState());
            }
            case MotionEvent.ACTION_HOVER_ENTER -> {
                lastX = ev.getX();
                lastY = ev.getY();
                havePosition = true;
                seen = true;
            }
            case MotionEvent.ACTION_HOVER_EXIT -> havePosition = false;
            case MotionEvent.ACTION_SCROLL -> scroll(
                    ev.getAxisValue(MotionEvent.AXIS_HSCROLL),
                    ev.getAxisValue(MotionEvent.AXIS_VSCROLL));
            default -> {
                return false;
            }
        }
        return true;
    }

    /**
     * An uncaptured mouse with a button down, which Android delivers as
     * <em>touch</em>. Taking it here is what keeps a mouse out of the gesture
     * layer.
     */
    public boolean onTouchEvent(MotionEvent ev) {
        if (!isMouse(ev)) {
            return false;
        }
        switch (ev.getActionMasked()) {
            case MotionEvent.ACTION_DOWN -> {
                lastX = ev.getX();
                lastY = ev.getY();
                havePosition = true;
                buttonState(ev.getButtonState());
            }
            case MotionEvent.ACTION_MOVE -> {
                for (int h = 0; h < ev.getHistorySize(); h++) {
                    moveTo(ev.getHistoricalX(h), ev.getHistoricalY(h));
                }
                moveTo(ev.getX(), ev.getY());
                buttonState(ev.getButtonState());
            }
            case MotionEvent.ACTION_UP -> {
                moveTo(ev.getX(), ev.getY());
                // A mouse's own up reports no buttons, which is the release.
                buttonState(ev.getButtonState());
                havePosition = false;
            }
            case MotionEvent.ACTION_CANCEL -> cancel();
            default -> {
                return false;
            }
        }
        return true;
    }

    /** True for a mouse or a trackball-like device, false for a finger or a stylus. */
    public static boolean isMouse(MotionEvent ev) {
        // SOURCE_MOUSE, and SOURCE_MOUSE_RELATIVE for a captured or trackpad
        // device that reports relative motion of its own accord.
        final int source = ev.getSource();
        if ((source & 0x2002) == 0x2002 || (source & 0x8002) == 0x8002) {
            // A stylus reports SOURCE_MOUSE on some devices; it is a third input
            // model (absolute, and with a hover of its own) and not this one.
            return ev.getPointerCount() == 0
                    || ev.getToolType(0) != MotionEvent.TOOL_TYPE_STYLUS;
        }
        return false;
    }

    private void moveTo(float x, float y) {
        if (havePosition) {
            motion(x - lastX, y - lastY);
        }
        lastX = x;
        lastY = y;
        havePosition = true;
    }
}
