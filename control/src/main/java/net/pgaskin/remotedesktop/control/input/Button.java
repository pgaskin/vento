// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.input;

/**
 * The RFB pointer buttons, as used by RealVNC Viewer's TouchManager.
 *
 * <p>An RFB {@code PointerEvent} carries a <em>mask</em> of these, so the two
 * representations both exist and mean different things: a {@code Button} is one
 * button, which is what the gesture layer presses and releases, and an
 * {@code int} is the set currently held, which is what goes on the wire. The
 * mask never leaves {@link #mask()} / {@link #in(int)} / {@link #maskName(int)}.
 */
public enum Button {
    LEFT(1),
    MIDDLE(2),
    RIGHT(4),
    WHEEL_UP(8),
    WHEEL_DOWN(16),
    WHEEL_LEFT(32),
    WHEEL_RIGHT(64),
    /**
     * Buttons 8 and 9 — the side buttons of a real mouse, which no touch gesture
     * can produce and the original has no concept of. X11 numbers them this way
     * and both desktops agree.
     *
     * <p>{@link #FORWARD} is the one place this vocabulary is wider than a
     * protocol: an RFB {@code PointerEvent}'s mask is a single <em>byte</em>, so
     * button 9 needs the {@code ExtendedMouseButtons} pseudo-encoding, which our
     * client does not speak. The RFB backend drops it and says so; RDP sends
     * both as X1/X2.
     */
    BACK(128),
    FORWARD(256);

    /**
     * Buttons that can hold a drag — the three under the hand, not the wheel
     * pseudo-buttons and not {@link #BACK} / {@link #FORWARD}, which are
     * shortcuts rather than something anybody drags with.
     */
    public static final int DRAG_MASK = LEFT.mask | MIDDLE.mask | RIGHT.mask;

    private static final Button[] ALL = values();

    private static final int KNOWN_MASK = maskOfAll();

    private final int mask;

    Button(int mask) {
        this.mask = mask;
    }

    public int mask() {
        return mask;
    }

    /** Is this button in {@code mask}? */
    public boolean in(int mask) {
        return (mask & this.mask) != 0;
    }

    /** The lowest-valued button in {@code mask}, or {@code null} if it is empty. */
    public static Button lowest(int mask) {
        for (Button b : ALL) {
            if (b.in(mask)) {
                return b;
            }
        }
        return null;
    }

    /** A whole mask, as {@code "-"} / {@code "LEFT"} / {@code "LEFT+RIGHT"}. */
    public static String maskName(int mask) {
        if (mask == 0) {
            return "-";
        }
        final StringBuilder sb = new StringBuilder();
        for (Button b : ALL) {
            if (b.in(mask)) {
                if (sb.length() > 0) {
                    sb.append('+');
                }
                sb.append(b);
            }
        }
        // RFB allows buttons 8 and up; nothing here emits them, but a backend
        // reading a mask off the wire could hand one back.
        final int unknown = mask & ~KNOWN_MASK;
        if (unknown != 0) {
            if (sb.length() > 0) {
                sb.append('+');
            }
            sb.append("0x").append(Integer.toHexString(unknown));
        }
        return sb.toString();
    }

    private static int maskOfAll() {
        int m = 0;
        for (Button b : ALL) {
            m |= b.mask;
        }
        return m;
    }
}
