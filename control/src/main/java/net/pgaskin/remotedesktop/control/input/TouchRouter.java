// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.input;

import android.view.MotionEvent;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;

/**
 * Turns a touch event stream into per-pointer callbacks, remembering each
 * pointer's previous position.
 *
 * <p>Equivalent to the useful half of RealVNC's {@code InterceptingRelativeLayout},
 * minus the toolbar gate, the hover path and the mouse-as-touch path, and
 * without its two mistakes: coordinates stay {@code float} (the original
 * truncates to {@code int} at the source, which costs real accuracy for slow
 * movement on a high-DPI screen), and callbacks carry the pointer id so the
 * consumer never has to match slots by position equality.
 *
 * <p>{@link #handle(TouchFrame)} is the whole implementation and touches no
 * Android APIs, so the stack below it runs in a plain JVM test;
 * {@link #onTouchEvent(MotionEvent)} is only an adapter, and
 * {@link #optionalRecorder} taps the same seam to record fixtures.
 */
public final class TouchRouter {

    public interface Listener {
        void touchBegan(int id, float x, float y, long t);

        void touchMoved(int id, float prevX, float prevY, float x, float y, long t);

        void touchEnded(int id, float prevX, float prevY, float x, float y, long t);

        void touchCancelled(long t);
    }

    /** Sees every frame before it is dispatched. Used by the fixture recorder. */
    public interface Tap {
        void onFrame(TouchFrame f);
    }

    /**
     * First refusal on every new pointer, for a widget sharing the touch
     * surface with the touchpad — the mouse overlay ({@link MouseOverlay}) and
     * the extension keyboard ({@link ExtensionKeyboard}).
     *
     * <p>A claimed pointer belongs to the claimant for the rest of its life and
     * is never shown to the {@link Listener}, so the gesture layer's finger
     * count stays right and a second finger on the desktop drives the cursor
     * normally while the first holds a button down. That combination is the
     * whole point of the overlay, and it is why the split happens here rather
     * than by putting a clickable view on top: the two have to work at once. The
     * original arrives at the same place from the other direction, with real
     * {@code View}s that swallow their own touches and a special case in
     * {@code InterceptingRelativeLayout} to undo it for the second finger.
     */
    public interface Claim {
        /** Take this pointer? Asked once, when it goes down. */
        boolean claimTouch(int id, float x, float y, long t);

        void claimMoved(int id, float x, float y, long t);

        void claimEnded(int id, float x, float y, long t);

        /** Every claimed pointer is gone. */
        void claimCancelled(long t);
    }

    private final Listener listener;
    private final TouchFrame scratch = new TouchFrame();

    // Previous position per pointer id; linear scan, there are never many.
    private final int[] lastId = new int[TouchFrame.MAX_POINTERS];
    private final float[] lastX = new float[TouchFrame.MAX_POINTERS];
    private final float[] lastY = new float[TouchFrame.MAX_POINTERS];
    private int lastCount;

    private Tap optionalRecorder;
    // Offered each new pointer in order; the first to take it owns it.
    private final List<Claim> claims = new ArrayList<>(2);
    private final Claim[] owner = new Claim[64]; // by pointer id, which is small and dense

    public TouchRouter(Listener listener) {
        this.listener = listener;
    }

    public void setTap(Tap tap) {
        this.optionalRecorder = tap;
    }

    /** Give {@code c} first refusal on new pointers, after any already added. */
    public void addClaim(Claim c) {
        claims.add(c);
    }

    private static boolean trackable(int id) {
        return id >= 0 && id < 64;
    }

    public boolean onTouchEvent(MotionEvent ev) {
        final TouchFrame.Action action = switch (ev.getActionMasked()) {
            case MotionEvent.ACTION_DOWN, MotionEvent.ACTION_POINTER_DOWN -> TouchFrame.Action.DOWN;
            case MotionEvent.ACTION_MOVE -> TouchFrame.Action.MOVE;
            case MotionEvent.ACTION_UP, MotionEvent.ACTION_POINTER_UP -> TouchFrame.Action.UP;
            case MotionEvent.ACTION_CANCEL -> TouchFrame.Action.CANCEL;
            default -> null;
        };
        if (action == null) {
            return false;
        }
        scratch.set(action, ev.getActionIndex(), ev.getEventTime());
        for (int i = 0; i < ev.getPointerCount(); i++) {
            scratch.add(ev.getPointerId(i), ev.getX(i), ev.getY(i));
        }
        return handle(scratch);
    }

    public boolean handle(TouchFrame f) {
        if (optionalRecorder != null) {
            optionalRecorder.onFrame(f);
        }
        switch (f.action) {
            case DOWN -> {
                final int i = clampIndex(f);
                final int id = f.id[i];
                final float x = f.x[i], y = f.y[i];
                if (trackable(id)) {
                    for (Claim c : claims) {
                        if (c.claimTouch(id, x, y, f.time)) {
                            owner[id] = c;
                            return true;
                        }
                    }
                }
                remember(id, x, y);
                listener.touchBegan(id, x, y, f.time);
            }
            case MOVE -> {
                for (int i = 0; i < f.count; i++) {
                    final int id = f.id[i];
                    if (trackable(id) && owner[id] != null) {
                        owner[id].claimMoved(id, f.x[i], f.y[i], f.time);
                        continue;
                    }
                    final int k = indexOf(id);
                    if (k < 0) {
                        continue; // never saw it go down
                    }
                    final float x = f.x[i], y = f.y[i];
                    final float px = lastX[k], py = lastY[k];
                    lastX[k] = x;
                    lastY[k] = y;
                    listener.touchMoved(id, px, py, x, y, f.time);
                }
            }
            case UP -> {
                final int i = clampIndex(f);
                final int id = f.id[i];
                final float x = f.x[i], y = f.y[i];
                if (trackable(id) && owner[id] != null) {
                    final Claim c = owner[id];
                    owner[id] = null;
                    c.claimEnded(id, x, y, f.time);
                    return true;
                }
                final int k = indexOf(id);
                final float px = k >= 0 ? lastX[k] : x, py = k >= 0 ? lastY[k] : y;
                forget(id);
                listener.touchEnded(id, px, py, x, y, f.time);
            }
            case CANCEL -> cancel(f.time);
        }
        return true;
    }

    /**
     * Every pointer is gone, whatever the screen thinks. Android raises this as
     * {@code ACTION_CANCEL} when something upstream takes the gesture over; the
     * app also calls it directly when the session stops, since a finger down at
     * that moment never gets an up.
     */
    public void cancel(long time) {
        lastCount = 0;
        Arrays.fill(owner, null);
        // Every claimant, not only those currently holding a pointer: cancel
        // means the touch stream is gone, and a widget left holding a button
        // because it happened to own nothing would hold it forever.
        for (Claim c : claims) {
            c.claimCancelled(time);
        }
        listener.touchCancelled(time);
    }

    private static int clampIndex(TouchFrame f) {
        return (f.index >= 0 && f.index < f.count) ? f.index : 0;
    }

    private int indexOf(int id) {
        for (int i = 0; i < lastCount; i++) {
            if (lastId[i] == id) {
                return i;
            }
        }
        return -1;
    }

    private void remember(int id, float x, float y) {
        int k = indexOf(id);
        if (k < 0) {
            if (lastCount == TouchFrame.MAX_POINTERS) {
                return;
            }
            k = lastCount++;
            lastId[k] = id;
        }
        lastX[k] = x;
        lastY[k] = y;
    }

    private void forget(int id) {
        final int k = indexOf(id);
        if (k < 0) {
            return;
        }
        lastCount--;
        lastId[k] = lastId[lastCount];
        lastX[k] = lastX[lastCount];
        lastY[k] = lastY[lastCount];
    }
}
