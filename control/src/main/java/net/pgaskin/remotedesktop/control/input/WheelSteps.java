// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.input;

/**
 * Travel in, wheel clicks out, with the remainder carried.
 *
 * <p>Two things scroll — two fingers on the pad and a real wheel — and they
 * differ only in what a step is worth and in which way their axis points. What
 * they must not differ in is the arithmetic: accumulating rather than rounding
 * is what makes a high-resolution wheel produce the same number of clicks per
 * turn as a detented one, and a sign error here would be a sign error in one
 * place and not the other.
 */
final class WheelSteps {

    private final MouseSink sink;
    private float accX, accY;

    WheelSteps(MouseSink sink) {
        this.sink = sink;
    }

    /** Positive {@code dy} scrolls down, as the wheel buttons count. */
    void add(float dx, float dy, float stepPx) {
        accY += dy;
        accX += dx;
        final int vsteps = (int) (accY / stepPx);
        final int hsteps = (int) (accX / stepPx);
        for (int i = 0; i < vsteps; i++) {
            click(Button.WHEEL_DOWN);
        }
        for (int i = 0; i > vsteps; i--) {
            click(Button.WHEEL_UP);
        }
        for (int i = 0; i < hsteps; i++) {
            click(Button.WHEEL_RIGHT);
        }
        for (int i = 0; i > hsteps; i--) {
            click(Button.WHEEL_LEFT);
        }
        accY %= stepPx;
        accX %= stepPx;
    }

    /** A new gesture starts owing nothing. */
    void reset() {
        accX = 0;
        accY = 0;
    }

    private void click(Button button) {
        sink.mouseDown(button.mask());
        sink.mouseUp(button.mask());
    }
}
