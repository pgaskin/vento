// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.session;

import android.app.Activity;
import android.view.WindowInsets;
import android.view.WindowMetrics;

import net.pgaskin.remotedesktop.backend.Backend;

/**
 * Asks the far end for a desktop the shape of this phone's window.
 *
 * <p>Off unless somebody has said so per connection: resizing a desktop is a
 * change to somebody else's machine, and one that happens because a phone was
 * turned over is not one they asked for.
 *
 * <p>A debounce and a negotiation rather than anything to do with drawing. It
 * is asked on a tick rather than told, because whether the far end will take a
 * size becomes true some time after the connection and a one-shot that fired
 * too early would never fire again.
 */
public final class WindowFollower {

    /**
     * How soon after the window changes shape the question is asked. A rotation
     * is one event and a split screen dragged about is dozens; a resize is a
     * round trip and, on RDP, a whole reactivation.
     */
    static final long DEBOUNCE_MS = 600;

    private final Backend backend;

    private boolean enabled;
    /** The size last asked for, so an unchanged window asks for nothing. */
    private int followedW, followedH;
    /** The window at the previous tick: two the same means it has settled. */
    private int settledW, settledH;

    WindowFollower(Backend backend) {
        this.backend = backend;
    }

    boolean enabled() {
        return enabled;
    }

    /**
     * Set from the connection's own answer. Asked again at the next tick rather
     * than at the next rotation, so turning it on takes effect where it was
     * turned on.
     */
    void setEnabled(boolean follow) {
        enabled = follow;
    }

    /**
     * Ask, if this connection has asked for that and the window has stopped
     * moving.
     *
     * <p>"Stopped moving" is two ticks the same, which is the debounce: a split
     * screen being dragged is a new size every frame and a resize is a round
     * trip. Nothing is asked twice — the same window is the same question, and
     * a far end that refused it will refuse it again — so a refusal costs one
     * request rather than one a second.
     */
    void tick(Activity activity) {
        if (!enabled) {
            return;
        }
        final int[] size = deviceSize(activity);
        if (size == null) {
            return;
        }
        final boolean settled = size[0] == settledW && size[1] == settledH;
        settledW = size[0];
        settledH = size[1];
        if (!settled || (size[0] == followedW && size[1] == followedH)
                || !backend.canResize()) {
            return;
        }
        followedW = size[0];
        followedH = size[1];
        backend.requestDesktopSize(size[0], size[1]);
    }

    /**
     * The window this session gets on this phone, less the status and
     * navigation bars — the rectangle the desktop is actually drawn in, so a
     * desktop of this size is one pixel to a pixel at scale 1. Bars hidden at
     * this instant are still subtracted: they come back, and a desktop that is
     * a few pixels short is better than one that changed size for a gesture.
     *
     * <p>Deliberately not the content rect the viewport works in, which shrinks
     * for the extension keyboard and the IME: those come and go several times a
     * minute, and a desktop that resized for each of them would be unusable.
     *
     * <p>Public because the connection panel offers the same size as a choice,
     * and the two must not disagree about what this window is worth.
     */
    public static int[] deviceSize(Activity activity) {
        if (activity == null) {
            return null;
        }
        final WindowMetrics metrics = activity.getWindowManager().getCurrentWindowMetrics();
        final android.graphics.Insets bars = metrics.getWindowInsets().getInsetsIgnoringVisibility(
                WindowInsets.Type.systemBars() | WindowInsets.Type.displayCutout());
        final int w = metrics.getBounds().width() - bars.left - bars.right;
        final int h = metrics.getBounds().height() - bars.top - bars.bottom;
        // A sanity check on the rectangle rather than the protocol's limit:
        // anything outside this is not a window somebody is looking at a
        // desktop in, and offering it as a size would be offering nonsense.
        return w < 200 || h < 200 || w > 16384 || h > 16384 ? null : new int[]{w, h};
    }
}
