// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.input;

/**
 * Where the keyboard's output goes: a keysym, an edge, and the identity of the
 * key the edge belongs to — which is what every remote-desktop protocol wants
 * at bottom even when it spells it differently.
 *
 * <p>Deliberately <em>not</em> a "type this character" call. A modifier held by
 * the extension keyboard is genuinely held down at the far end for as long as
 * the light is on — that is how Ctrl+C works when the C came from the system
 * IME and the Ctrl came from a key on our row — so presses and releases have to
 * be separable.
 *
 * <p>Keysyms are X11 keysyms ({@link Keysym}). A backend that speaks something
 * else translates; a backend that has no key for a given keysym drops it, which
 * is why {@link ExtensionKeyboard} takes its key list from the caller.
 *
 * <h2>The key id</h2>
 *
 * <p>{@link #keyUp} names a <em>key</em>, not a keysym, because that is what
 * the far end is keeping: RFB's own client-side keyboard state is a
 * {@code map<keycode, keysym>} of what is held, and a release looks the keycode
 * up and lets go of whatever keysym was recorded when it went down — the keycode
 * is the identity, the keysym only the payload. RealVNC's viewer passes the
 * keysym as the keycode too, which is safe only because a soft keyboard never
 * holds one key while another with the same keysym is pressed.
 *
 * <p>The id is opaque — any int, as long as the same key always uses the same
 * one — but every producer feeding one sink shares the space, so they have to
 * agree. The three in this project each take a base below and add their own
 * per-key number to it; a fourth picks another base.
 */
public interface KeySink {

    /** {@link ExtensionKeyboard}, plus the key's index in the row. */
    int ID_EXTENSION_KEYBOARD = 0x0001_0000;

    /** The system IME's key events, plus the Android key code. */
    int ID_SYSTEM_KEYBOARD = 0x0002_0000;

    /**
     * Text committed by the system IME. One id for all of it: a committed
     * character's press and release are adjacent with nothing in between, so
     * there is never more than one of them held.
     */
    int ID_TEXT = 0x0003_0000;

    void keyDown(int keysym, int keyId);

    void keyUp(int keyId);
}
