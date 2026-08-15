// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.input;

import android.view.KeyEvent;

import java.util.Arrays;
import java.util.function.IntConsumer;

/**
 * A real keyboard — Bluetooth, USB, or the one built into a dock — turned into
 * keysyms for the remote.
 *
 * <p><b>The phone owns the layout; the remote is told characters.</b> A printing
 * key sends whatever character the phone's own keyboard layout produces for it —
 * Shift, AltGr and Caps Lock included — and a non-printing key sends the keysym
 * for its <em>position</em>. It is the same rule the system IME's path follows
 * ({@code control/ui/TextInput}), which is what lets the two coexist without
 * either knowing about the other. Three consequences, each of which could have
 * gone the other way:
 *
 * <ol>
 * <li><b>The lock keys stay here.</b> Caps Lock, Num Lock and Scroll Lock are
 *     applied to the character and <em>not</em> forwarded, or they would apply
 *     twice, and the two ends would keep lock states that drift apart the moment
 *     anything else touches either.</li>
 * <li><b>Ctrl and Alt are held, not folded in.</b> The character lookup is done
 *     with Ctrl and left Alt <em>masked out</em>, so Ctrl+C is {@code c} with
 *     {@code Control_L} held rather than whatever the layout thinks Ctrl+C is
 *     (usually nothing, which is how a naive mapping loses every shortcut).
 *     Right Alt is not masked out: on a PC layout it is AltGr and the character
 *     is the point of it, so {@code ISO_Level3_Shift} goes out with it and an X
 *     server looking the character up finds the modifier already there.</li>
 * <li><b>The keypad is its own set of keys.</b> {@link Keysym#keypadKeysym}, not
 *     the digit above it: with Num Lock off the keypad <em>is</em> the navigation
 *     cluster.</li>
 * </ol>
 *
 * <p>The character lookup is the caller's ({@link CharMap}) because it belongs
 * to the device's own {@code KeyCharacterMap}; a test supplies a layout of its
 * own.
 */
public final class PhysicalKeyboard {

    /**
     * A key's character under a given modifier state — {@code KeyEvent}'s
     * {@code getUnicodeChar(int)}. {@code 0} when the key has none.
     */
    public interface CharMap {
        int unicode(int keyCode, int metaState);
    }

    /** Told when something changed that is worth redrawing (the HUD's counts). */
    public interface Listener {
        void keyboardActivity();
    }

    // MetaState bits, as literals for the same reason Keysym's keycodes are.
    private static final int META_SHIFT = 0x1 | 0x40 | 0x80;      // ON, LEFT, RIGHT
    private static final int META_ALT = 0x02;
    private static final int META_ALT_RIGHT = 0x20;
    private static final int META_CAPS_LOCK = 0x100000;
    private static final int META_NUM_LOCK = 0x200000;

    // What the character lookup is allowed to see: the modifiers that choose a
    // character. The rest are the remote's to apply (see the class comment).
    private static final int CHAR_META = META_SHIFT | META_CAPS_LOCK;

    private final KeySink sink;
    private Listener listener;

    // The keysym each held key went down with, by Android keycode, or 0. A
    // release names the key rather than the keysym (KeySink §"the key id"), so
    // this only records what is held, for releaseAll and for telling a repeat
    // apart from a fresh press.
    //
    // Grown to fit rather than sized once: Android's keycodes are a list that
    // gets longer with each release — F13 is 326 — and a key past the end of a
    // fixed table can only be refused, since a press it cannot record is a key
    // held down at the far end for ever.
    private int[] held = new int[256];
    private int heldCount;

    public PhysicalKeyboard(KeySink sink) {
        this.sink = sink;
    }

    public void setListener(Listener l) {
        this.listener = l;
    }

    /** How many keys are held down at the far end because of this keyboard. */
    public int heldCount() {
        return heldCount;
    }

    /**
     * Keys this client keeps for itself, so there is always a way out of a
     * session and the phone stays a phone: Back, the volume and media keys, and
     * the ones Android never delivers anyway.
     */
    public static boolean reserved(int keyCode) {
        return switch (keyCode) {
            case 3, 4, 5, 6 -> true;         // HOME, BACK, CALL, ENDCALL
            case 24, 25, 164 -> true;        // VOLUME_UP, VOLUME_DOWN, VOLUME_MUTE
            case 26, 223, 224 -> true;       // POWER, SLEEP, WAKEUP
            case 27 -> true;                 // CAMERA
            case 79 -> true;                 // HEADSETHOOK
            case 187 -> true;                // APP_SWITCH
            case 220, 221 -> true;           // BRIGHTNESS_DOWN, BRIGHTNESS_UP
            // The lock keys: applied to the character here, never forwarded.
            case 115, 143, 116 -> true;      // CAPS_LOCK, NUM_LOCK, SCROLL_LOCK
            default -> (keyCode >= 85 && keyCode <= 91) || (keyCode >= 126 && keyCode <= 130);
        };
    }

    /**
     * A key went down. Returns whether it was sent — {@code false} means the key
     * is not ours and the caller should let Android have it.
     *
     * @param repeat the event's repeat count; anything above zero is Android's
     *               own auto-repeat, which is forwarded as a further press of
     *               the keysym the key <em>went down with</em>
     */
    public boolean keyDown(int keyCode, int metaState, int repeat, CharMap chars) {
        if (keyCode < 0) {
            return false;
        }
        if (keyCode >= held.length) {
            held = Arrays.copyOf(held, keyCode + 1);
        }
        // A repeat re-sends the keysym recorded at press time rather than looking
        // it up again: the far end keys its held-key map on the id, so a repeat
        // that arrived as a different keysym — hold "a", press Shift, Android
        // says "A" — would overwrite the entry, and the release would let go of
        // "A" while "a" stayed down at the far end for ever.
        if (repeat > 0 && held[keyCode] != 0) {
            sink.keyDown(held[keyCode], KeySink.ID_SYSTEM_KEYBOARD + keyCode);
            return true;
        }
        final int keysym = keysym(keyCode, metaState, chars);
        if (keysym == 0) {
            return false;
        }
        if (held[keyCode] == 0) {
            heldCount++;
        }
        held[keyCode] = keysym;
        sink.keyDown(keysym, KeySink.ID_SYSTEM_KEYBOARD + keyCode);
        changed();
        return true;
    }

    /**
     * A key came up. Returns the keysym that was released, or {@code 0} if this
     * keyboard was not holding that key — which is also how a reserved key's up
     * is refused as its down was, and how a key held across a
     * {@link #releaseAll()} avoids releasing twice.
     *
     * <p>The keysym rather than a {@code boolean} because the caller has one
     * more thing to do with it: a key typed here consumes a one-shot modifier
     * armed on the extension row, and a <em>modifier</em> typed here must not
     * ({@link #isModifier}).
     */
    public int keyUp(int keyCode) {
        if (keyCode < 0 || keyCode >= held.length || held[keyCode] == 0) {
            return 0;
        }
        final int keysym = held[keyCode];
        held[keyCode] = 0;
        heldCount--;
        sink.keyUp(KeySink.ID_SYSTEM_KEYBOARD + keyCode);
        changed();
        return keysym;
    }

    /**
     * The keysym {@code keyCode} means right now, or {@code 0} for a key that
     * has none. The order is the rule: position first for the keys with no
     * character, then the keypad, which needs the lock state, then the layout.
     */
    private int keysym(int keyCode, int metaState, CharMap chars) {
        if (reserved(keyCode)) {
            return 0;
        }
        final int byPosition = Keysym.fromAndroidKeyCode(keyCode);
        if (byPosition != 0) {
            return byPosition;
        }
        final int keypad = Keysym.keypadKeysym(keyCode, (metaState & META_NUM_LOCK) != 0);
        if (keypad != 0) {
            return keypad;
        }
        int cp = chars.unicode(keyCode, charMeta(metaState));
        if (cp == 0 && (metaState & META_ALT_RIGHT) != 0) {
            // AltGr held over a layout with no third level, which is every US
            // one: the phone has no character to offer, so send the key's own
            // and let the far end apply the level it has already been told is
            // held. Without this the whole third level is unreachable from a
            // phone whose layout has none, and AltGr does nothing at all.
            cp = chars.unicode(keyCode, charMeta(metaState) & ~(META_ALT | META_ALT_RIGHT));
        }
        return cp == 0 ? 0 : Keysym.fromKeyChar(cp);
    }

    /**
     * {@link #CHAR_META}, plus AltGr when — and only when — it is right Alt.
     * Public because the system IME's key events go through the same rule:
     * which modifiers may choose a character is a property of this client, not
     * of where the event came from ({@code ui/TextInput}).
     */
    public static int charMeta(int metaState) {
        int m = metaState & CHAR_META;
        if ((metaState & META_ALT_RIGHT) != 0) {
            m |= META_ALT | META_ALT_RIGHT;
        }
        return m;
    }

    /**
     * Whether a keysym is one of the modifiers the extension keyboard also has,
     * which is the one thing the two keyboards have to agree about: a key typed
     * on this one consumes a one-shot modifier armed on that one, and a modifier
     * typed on this one must not.
     */
    static boolean isModifier(int keysym) {
        return keysym == Keysym.SHIFT_L || keysym == Keysym.SHIFT_R
                || keysym == Keysym.CONTROL_L || keysym == Keysym.CONTROL_R
                || keysym == Keysym.ALT_L || keysym == Keysym.SUPER_L
                || keysym == Keysym.SUPER_R || keysym == Keysym.ISO_LEVEL3_SHIFT;
    }

    /**
     * Let go of every key this keyboard is holding at the far end: a key held
     * when the host is switched away from never gets an up, and a stuck Ctrl on
     * somebody's desktop is indistinguishable from a broken keyboard.
     */
    public void releaseAll() {
        for (int keyCode = 0; keyCode < held.length; keyCode++) {
            if (held[keyCode] != 0) {
                held[keyCode] = 0;
                sink.keyUp(KeySink.ID_SYSTEM_KEYBOARD + keyCode);
            }
        }
        heldCount = 0;
        changed();
    }

    private void changed() {
        if (listener != null) {
            listener.keyboardActivity();
        }
    }

    // ---- the Android adapter ----------------------------------------------

    /**
     * A {@code KeyEvent} from a real keyboard. Returns whether it was consumed;
     * {@code typed} is handed the keysym of each non-modifier key as it is
     * released, which is what the extension row's one-shot modifiers and its
     * info bar are waiting for.
     *
     * <p>{@code ev::getUnicodeChar} rather than a cached {@code KeyCharacterMap}
     * because the map belongs to the device the event came from, and two
     * keyboards with two layouts can be connected at once.
     */
    public boolean onKeyEvent(KeyEvent ev, IntConsumer typed) {
        switch (ev.getAction()) {
            case KeyEvent.ACTION_DOWN -> {
                return keyDown(ev.getKeyCode(), ev.getMetaState(),
                        ev.getRepeatCount(), (keyCode, meta) -> ev.getUnicodeChar(meta));
            }
            case KeyEvent.ACTION_UP -> {
                final int keysym = keyUp(ev.getKeyCode());
                if (keysym != 0 && !isModifier(keysym)) {
                    typed.accept(keysym);
                }
                return keysym != 0;
            }
            default -> {
                return false;
            }
        }
    }
}
