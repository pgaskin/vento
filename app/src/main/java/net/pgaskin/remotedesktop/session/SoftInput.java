// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.session;

import android.view.View;
import android.view.WindowInsets;
import android.view.inputmethod.InputMethodManager;

import net.pgaskin.remotedesktop.control.input.ExtensionKeyboard;
import net.pgaskin.remotedesktop.control.input.MouseOverlay;

/**
 * What is showing along the bottom of the desktop, and keeping the system's own
 * keyboard in step with it.
 *
 * <p>There are three of them and only room for so much: the extension row, the
 * mouse overlay, and the system IME under whichever of those asked for it. The
 * rules between them are what this is for — showing one overlay hides the
 * other and a displaced one comes back; the row and the IME are one keyboard as
 * far as anybody using them is concerned, so dismissing either takes both.
 *
 * <p>The awkward part is that the IME is a window rather than a widget, so
 * "showing" is a request and "shown" is an inset that arrives later. Both are
 * tracked, because the gap between them is where the mistakes are: an inset
 * saying the IME is down, read while a request for it is outstanding, is this
 * window's own stale state rather than news.
 */
final class SoftInput {

    interface Listener {

        /**
         * One of the two came or went. What follows is the host's: the
         * toolbar's stateful buttons, and the insets the viewport clamps in.
         */
        void chromeChanged();
    }

    private final View view;
    private final ExtensionKeyboard keyboard;
    private final MouseOverlay overlay;
    private final Listener listener;

    private boolean overlayHiddenByKeyboard;  // put it back when the keyboard goes
    private boolean overlayShown;
    private boolean keyboardShown;
    private int imeHeight;                    // the system IME's, from the window insets
    private boolean imeUp;                    // ... which is not the same as it being up
    private boolean imeRequested;             // asked for and not arrived yet

    SoftInput(View view, ExtensionKeyboard keyboard, MouseOverlay overlay, Listener listener) {
        this.view = view;
        this.keyboard = keyboard;
        this.overlay = overlay;
        this.listener = listener;
    }

    /**
     * How much of the bottom of this window the system IME covers. Wanted on
     * its own, and not only as part of what the row insets by, because it can
     * outlive the row that asked for it.
     */
    int imeHeight() {
        return imeHeight;
    }

    /** Showing one overlay hides the other, and a displaced one comes back. */
    void setKeyboardVisible(boolean show) {
        if (show == keyboard.visible()) {
            return;
        }
        if (show) {
            overlayHiddenByKeyboard = overlay.visible();
            overlay.setVisible(false);
        }
        // Everything else follows from the model changing, in keyboardChanged()
        // — because the model can also hide itself, from its own ✕.
        keyboard.setVisible(show);
    }

    void toggleKeyboard() {
        setKeyboardVisible(!keyboard.visible());
    }

    void toggleOverlay() {
        if (overlay.visible()) {
            overlay.setVisible(false);
        } else {
            setKeyboardVisible(false);
            overlay.setVisible(true);
        }
    }

    /**
     * Both away, and neither owed a return. For a session that has become
     * view-only, where they are not inactive but absent: a row of keys that
     * types nothing is indistinguishable from a session that has stopped
     * answering.
     */
    void hideAll() {
        setKeyboardVisible(false);
        overlay.setVisible(false);
        overlayHiddenByKeyboard = false;
    }

    /** The extension row's model changed — from here, or from its own ✕. */
    void keyboardChanged() {
        if (keyboardShown != keyboard.visible()) {
            keyboardShown = keyboard.visible();
            sync();
            listener.chromeChanged();
        }
    }

    /** The overlay's did. It has no IME to keep in step, so only the chrome. */
    void overlayChanged() {
        if (overlayShown != overlay.visible()) {
            overlayShown = overlay.visible();
            listener.chromeChanged();
        }
    }

    /**
     * Put the system keyboard where the extension row now says it should be.
     *
     * <p>Called on every focus gain as well as on every change, in both
     * directions and unconditionally. Shown: the IME went with an app switch
     * and does not come back on its own, so the row would have nothing under
     * it. Hidden: something hid the row while another window held the focus —
     * turning on view-only from the connection panel — and an IMM call made
     * then is dropped, leaving a soft keyboard up under a row that is gone.
     */
    void sync() {
        final InputMethodManager imm = view.getContext().getSystemService(InputMethodManager.class);
        if (keyboard.visible()) {
            view.requestFocus();
            // Asked for and not yet seen: until the insets say it is up, an
            // inset saying it is down is this window's own stale state.
            imeRequested = true;
            imm.showSoftInput(view, 0);
        } else {
            imeRequested = false;
            imm.hideSoftInputFromWindow(view.getWindowToken(), 0);
            if (overlayHiddenByKeyboard) {
                overlayHiddenByKeyboard = false;
                overlay.setVisible(true);
            }
        }
    }

    /**
     * The IME's height, so the extension keyboard sits on top of it rather than
     * behind it and the desktop insets by the pair. A soft keyboard dismissed
     * with the back gesture takes the extension row with it.
     *
     * @return whether any of that moved, so the caller can re-inset once for
     * this and its own reasons together
     */
    boolean imeInsets(WindowInsets insets) {
        final int ime = insets.getInsets(WindowInsets.Type.ime()).bottom;
        // Whether it is up, asked separately from how much of this window it
        // covers. In multi-window the two part company: the window is resized
        // around the IME rather than overlapped by it, so the inset is 0 the
        // whole time one is showing, and a height that never rises is a height
        // that never falls. Reading the dismissal off the height alone left the
        // extension row up after a back gesture had taken the system keyboard.
        final boolean up = insets.isVisible(WindowInsets.Type.ime());
        if (ime == imeHeight && up == imeUp) {
            return false;
        }
        // Only while this window has focus, and only while we are not asking
        // for the IME back. An IME also closes when the app is switched away
        // from, and a sheet over this screen takes the focus and the IME with
        // it; either read as a dismissal hides the extension row, and the
        // second one is a race between two windows — so closing the
        // connection panel would put the system keyboard back with no row.
        // The back gesture this rule is for happens with the session in
        // front and no request outstanding.
        final boolean closed = imeUp && !up && view.hasWindowFocus() && !imeRequested;
        if (up) {
            imeRequested = false;
        }
        imeHeight = ime;
        imeUp = up;
        keyboard.setBottomOffset(ime);
        if (closed && keyboard.visible()) {
            setKeyboardVisible(false);
        }
        return true;
    }
}
