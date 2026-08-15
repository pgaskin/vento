// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.input;

/**
 * One touch event, with no Android types in it: the action, the pointer it
 * applies to, and the full set of pointers currently on the screen.
 *
 * <p>This is the seam that makes the input stack testable — {@link TouchRouter}
 * works on these, and converting a {@code MotionEvent} into one is a dozen
 * lines. It is also the on-disk fixture format (see {@link TouchLog}).
 *
 * <p>Semantics follow {@code MotionEvent}: {@link Action#DOWN} covers both
 * {@code ACTION_DOWN} and {@code ACTION_POINTER_DOWN} and the new pointer is
 * already in the list; {@link Action#UP} covers both up actions and the lifting
 * pointer is still in the list.
 */
public final class TouchFrame {

    public static final int MAX_POINTERS = 10;

    /** What happened, and the letter that stands for it in a {@link TouchLog}. */
    public enum Action {
        DOWN("D"), MOVE("M"), UP("U"), CANCEL("C");

        private final String code;

        Action(String code) {
            this.code = code;
        }

        public String code() {
            return code;
        }

        public static Action fromCode(String code) {
            return switch (code) {
                case "D" -> DOWN;
                case "M" -> MOVE;
                case "U" -> UP;
                case "C" -> CANCEL;
                default -> throw new IllegalArgumentException("bad action " + code);
            };
        }
    }

    public Action action;
    /** Index into the pointer arrays the action applies to (DOWN/UP only). */
    public int index;
    /** Event time, ms, on an arbitrary monotonic clock. */
    public long time;

    public int count;
    public final int[] id = new int[MAX_POINTERS];
    public final float[] x = new float[MAX_POINTERS];
    public final float[] y = new float[MAX_POINTERS];

    public TouchFrame set(Action action, int index, long time) {
        this.action = action;
        this.index = index;
        this.time = time;
        this.count = 0;
        return this;
    }

    public TouchFrame add(int id, float x, float y) {
        if (count < MAX_POINTERS) {
            this.id[count] = id;
            this.x[count] = x;
            this.y[count] = y;
            count++;
        }
        return this;
    }

    public TouchFrame copy() {
        final TouchFrame f = new TouchFrame().set(action, index, time);
        for (int i = 0; i < count; i++) {
            f.add(id[i], x[i], y[i]);
        }
        return f;
    }

    @Override
    public String toString() {
        final StringBuilder sb = new StringBuilder();
        sb.append(time).append(' ').append(action.code());
        if (action == Action.DOWN || action == Action.UP) {
            sb.append(' ').append(index);
        }
        for (int i = 0; i < count; i++) {
            sb.append(" | ").append(id[i]).append(' ').append(x[i]).append(' ').append(y[i]);
        }
        return sb.toString();
    }
}
