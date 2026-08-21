// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.session;

import android.graphics.Canvas;
import android.os.SystemClock;

import net.pgaskin.remotedesktop.backend.Backend;
import net.pgaskin.remotedesktop.control.CursorController;
import net.pgaskin.remotedesktop.control.Viewport;
import net.pgaskin.remotedesktop.control.input.Button;
import net.pgaskin.remotedesktop.control.input.Config;
import net.pgaskin.remotedesktop.control.input.ExtensionKeyboard;
import net.pgaskin.remotedesktop.control.input.GestureRecognizer;
import net.pgaskin.remotedesktop.control.input.MouseOverlay;
import net.pgaskin.remotedesktop.control.input.PhysicalKeyboard;
import net.pgaskin.remotedesktop.control.input.PhysicalMouse;
import net.pgaskin.remotedesktop.control.ui.Hud;

import java.util.Locale;

/**
 * The playground's readout, on a real connection: the same lines in the same
 * order so the two screens can be read against each other, plus one for the
 * pixel path — which is the one the playground cannot have, and the only place
 * "is the mirror keeping up" is answered.
 *
 * <p>{@link Hud} draws the box and says why the box is shared; the lines are
 * this screen's, and this is where they are built. Separate from the view
 * because it is the one thing here that touches every collaborator at once,
 * and none of them for any reason the session has.
 */
final class SessionHud {

    private final Hud hud;
    private final Config cfg;
    private final Backend backend;
    private final SessionClipboard clipboard;
    private final CursorController cursor;
    private final Viewport viewport;
    private final GestureRecognizer gestures;
    private final MouseOverlay overlay;
    private final ExtensionKeyboard keyboard;
    private final PhysicalMouse mouse;
    private final PhysicalKeyboard keys;

    private final Hud.Rate frameRate = new Hud.Rate();
    private final Hud.Rate eventRate = new Hud.Rate();
    private final Hud.Rate damageRate = new Hud.Rate();
    private long frames;

    SessionHud(Config cfg, Backend backend, SessionClipboard clipboard, CursorController cursor,
               Viewport viewport, GestureRecognizer gestures, MouseOverlay overlay,
               ExtensionKeyboard keyboard, PhysicalMouse mouse, PhysicalKeyboard keys) {
        this.hud = new Hud(cfg);
        this.cfg = cfg;
        this.backend = backend;
        this.clipboard = clipboard;
        this.cursor = cursor;
        this.viewport = viewport;
        this.gestures = gestures;
        this.overlay = overlay;
        this.keyboard = keyboard;
        this.mouse = mouse;
        this.keys = keys;
    }

    /**
     * @param mirror    the pixel path's, or null before a desktop size arrived
     * @param imeHeight the system IME's, which is neither the overlay's nor the
     *                  row's and can outlive both
     */
    void draw(Canvas c, int viewW, int viewH, Mirror mirror, Backend.State state,
              String lastRegion, int imeHeight, boolean pointerCaptured) {
        final long now = System.nanoTime();
        final long rects = mirror == null ? 0 : mirror.damageRects();
        frames++;
        final String[] lines = {
                "state " + state
                        + "  desktop " + backend.desktopWidth() + "x" + backend.desktopHeight()
                        + "  fps " + frameRate.sample(frames, now)
                        + "  dmg " + rects + " (" + damageRate.sample(rects, now) + "/s)"
                        // The pixel path, in the order it happens.
                        + "  tile " + (mirror == null ? "-"
                        : mirror.tileWidth() + "x" + mirror.tileHeight()
                        + " " + mirror.visibleTiles() + "/" + mirror.allocatedTiles()
                        + "/" + mirror.tileCount()
                        + " rd " + mirror.lastRead() + " dty " + mirror.dirtyCount())
                        + "  clip " + clipboard.summary(),
                "cursor " + (cursor.isRelative() ? "theirs"
                        : (int) cursor.x() + "," + (int) cursor.y())
                        + "  btn " + cursor.buttonsName()
                        + "  scale " + String.format(Locale.ROOT, "%.3f", viewport.getScale())
                        + " [" + (viewport.zoomIndex() + 1) + "/" + viewport.zoomLadder().length + "]"
                        + "  origin " + (int) viewport.originX() + "," + (int) viewport.originY()
                        + "  content " + viewport.contentWidth() + "x" + viewport.contentHeight(),
                "down " + gestures.downCount() + "  max " + gestures.maxDownCount()
                        + "  mode " + gestures.mode()
                        + "  moving " + (gestures.moving() ? "Y" : "N")
                        + "  held " + (gestures.heldButton() == null ? "-" : gestures.heldButton()),
                "accel x" + String.format(Locale.ROOT, "%.2f", gestures.accelFactor())
                        // dp/ms, so it can be read against the Config thresholds
                        + "  spd " + String.format(Locale.ROOT, "%.2f", gestures.accelSpeed() / cfg.density)
                        + "  lock " + gestures.axisLock()
                        + "  hover x" + String.format(Locale.ROOT, "%.2f", gestures.hoverGain())
                        + " lag " + String.format(Locale.ROOT, "%.0f", gestures.hoverLagMs())
                        + (gestures.hoverLockedOut(SystemClock.uptimeMillis()) ? " LOCKED" : "")
                        + "  events " + cursor.eventCount()
                        + " (" + eventRate.sample(cursor.eventCount(), now) + "/s)",
                "ovl " + (overlay.visible()
                        ? Button.maskName(overlay.heldMask())
                        + " rate " + String.format(Locale.ROOT, "%.1f", overlay.scrollRate())
                        : "off")
                        + "   kbd " + (keyboard.visible()
                        ? "on ime " + imeHeight + " mod " + keyboard.heldModifierCount()
                        : "off")
                        + "   region " + lastRegion
                        + (backend.viewOnly() ? "   VIEW ONLY" : ""),
                // The physical pair. "cap" is worth a column because an
                // uncaptured mouse looks identical until it reaches the edge of
                // the screen and stops.
                "mouse " + (pointerCaptured ? "captured"
                        : mouse.seen() ? "hover" : "-")
                        + " btn " + Button.maskName(mouse.heldMask())
                        + "   keys " + keys.heldCount() + " held",
        };
        // heightPx(), not insetBottomPx(): the desktop is meant to run under the
        // info bar and the HUD is not, so the two want different answers.
        hud.draw(c, lines, viewW, viewH,
                Math.max(Math.max(overlay.insetBottomPx(), keyboard.heightPx()), imeHeight));
    }
}
