// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.playground;

import android.content.Context;
import android.util.Log;
import android.view.InputDevice;
import android.view.KeyEvent;
import android.view.KeyCharacterMap;

import net.pgaskin.remotedesktop.control.input.KeySink;
import net.pgaskin.remotedesktop.control.input.Keysym;
import net.pgaskin.remotedesktop.control.input.PhysicalKeyboard;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.OutputStreamWriter;
import java.io.Writer;
import java.nio.charset.StandardCharsets;
import java.util.HashMap;
import java.util.HashSet;
import java.util.Locale;
import java.util.Map;
import java.util.Set;
import java.util.function.BooleanSupplier;

/**
 * Every key event and what became of it, one line each, for walking a real
 * keyboard key by key.
 *
 * <p>The HUD can say how many keys are held; it cannot answer "what did Android
 * deliver for this key, and what keysym did we make of it", which is a table and
 * needs a file. So this sits in two places at once: around the dispatch
 * ({@link #record}), where the event is, and <em>as</em> the keyboard's
 * {@link KeySink}, where the decision is. Recomputing the keysym here instead
 * would make the trace a second opinion about the rule rather than a record of
 * it, which is the one thing it must not be.
 *
 * <p>Armed from the {@code KEYTRACE} toggle square, or by the host before the
 * view is shown, since a keyboard walk is driven from a script.
 *
 * <pre>
 *   adb pull /sdcard/Android/data/net.pgaskin.remotedesktop/files/keytrace
 * </pre>
 */
public final class KeyTrace implements KeySink {

    private static final String TAG = "KeyTrace";

    private static final String HEADER = String.format("%-4s %-22s %4s %4s %8s %-8s %5s %3s %-7s %-7s %8s %-12s %s",
            "ev", "key", "code", "scan", "meta", "mods", "flags", "rpt",
            "uni", "chr", "keysym", "name", "outcome");

    private final Context ctx;
    private final KeySink next;

    // What the sink saw during the event being recorded, and the keysym each
    // held key went down with, so a release can name one (KeySink §"the key id").
    private final Map<Integer, Integer> held = new HashMap<>();
    private final Set<Integer> describedDevices = new HashSet<>();
    private int edgeKeysym;
    private boolean sawEdge;
    private boolean inEvent;

    private boolean armed;
    private Writer out;
    private int lines;
    private String lastPath = "";

    public KeyTrace(Context ctx, KeySink next) {
        this.ctx = ctx;
        this.next = next;
    }

    public boolean armed() {
        return armed;
    }

    public String label() {
        return armed ? "REC " + lines : (lines > 0 ? "OFF " + lines : "OFF");
    }

    public String lastPath() {
        return lastPath;
    }

    public void toggle() {
        setArmed(!armed);
    }

    public void setArmed(boolean on) {
        if (on == armed) {
            return;
        }
        armed = on;
        if (armed) {
            open();
        } else {
            close();
        }
    }

    /**
     * Record what one dispatched event became. The delivery is run inside so the
     * sink calls it makes land between this line's two halves; a trace that is
     * not armed is the delivery and nothing else.
     */
    public boolean record(KeyEvent ev, BooleanSupplier deliver) {
        if (!armed) {
            return deliver.getAsBoolean();
        }
        describe(ev.getDeviceId());
        edgeKeysym = 0;
        sawEdge = false;
        inEvent = true;
        final boolean consumed = deliver.getAsBoolean();
        inEvent = false;
        write(line(ev, consumed));
        return consumed;
    }

    @Override
    public void keyDown(int keysym, int keyId) {
        edgeKeysym = keysym;
        sawEdge = true;
        held.put(keyId, keysym);
        unprompted("down", keysym, keyId);
        next.keyDown(keysym, keyId);
    }

    @Override
    public void keyUp(int keyId) {
        final Integer keysym = held.remove(keyId);
        edgeKeysym = keysym == null ? 0 : keysym;
        sawEdge = true;
        unprompted("up", edgeKeysym, keyId);
        next.keyUp(keyId);
    }

    /**
     * An edge with no key event behind it, which is what letting go of
     * everything on the way out of a session looks like. Worth a line of its
     * own: the far end was told something nobody pressed.
     */
    private void unprompted(String edge, int keysym, int keyId) {
        if (!inEvent) {
            write("# " + edge + " " + Keysym.name(keysym) + " (id 0x"
                    + Integer.toHexString(keyId) + ") with no key event");
        }
    }

    private String line(KeyEvent ev, boolean consumed) {
        final int keyCode = ev.getKeyCode();
        final int meta = ev.getMetaState();
        final boolean down = ev.getAction() == KeyEvent.ACTION_DOWN;
        return String.format(Locale.ROOT, "%-4s %-22s %4d %4d %08x %-8s %5s %3d %-7s %-7s %8s %-12s %s",
                down ? "down" : "up",
                keyName(keyCode),
                keyCode,
                ev.getScanCode(),
                meta,
                mods(meta),
                // Chiefly FLAG_CANCELED (0x20), which is how a chord the window
                // manager took is taken back off the app that saw its first half.
                "0x" + Integer.toHexString(ev.getFlags()),
                ev.getRepeatCount(),
                ch(ev.getUnicodeChar(meta)),
                ch(ev.getUnicodeChar(PhysicalKeyboard.charMeta(meta))),
                sawEdge ? Integer.toHexString(edgeKeysym) : "-",
                sawEdge ? Keysym.name(edgeKeysym) : "-",
                outcome(ev, consumed));
    }

    /**
     * Why nothing was sent, in the vocabulary of the decisions that could have
     * refused it: this client keeping the key, no keysym for it at all, or an up
     * for a key whose down was already refused.
     */
    private String outcome(KeyEvent ev, boolean consumed) {
        if (sawEdge) {
            return consumed ? "sent" : "sent-not-consumed";
        }
        if (PhysicalKeyboard.reserved(ev.getKeyCode())) {
            return "reserved";
        }
        return ev.getAction() == KeyEvent.ACTION_DOWN ? "no-keysym" : "not-held";
    }

    /** {@code KEYCODE_} is what every line would start with. */
    private static String keyName(int keyCode) {
        final String s = KeyEvent.keyCodeToString(keyCode);
        return s.startsWith("KEYCODE_") ? s.substring(8) : s;
    }

    private static String mods(int meta) {
        final StringBuilder sb = new StringBuilder();
        if ((meta & KeyEvent.META_SHIFT_ON) != 0) sb.append('S');
        if ((meta & KeyEvent.META_CTRL_ON) != 0) sb.append('C');
        if ((meta & KeyEvent.META_ALT_LEFT_ON) != 0) sb.append('A');
        if ((meta & KeyEvent.META_ALT_RIGHT_ON) != 0) sb.append('G');
        if ((meta & KeyEvent.META_META_ON) != 0) sb.append('M');
        if ((meta & KeyEvent.META_CAPS_LOCK_ON) != 0) sb.append('L');
        if ((meta & KeyEvent.META_NUM_LOCK_ON) != 0) sb.append('N');
        if ((meta & KeyEvent.META_SCROLL_LOCK_ON) != 0) sb.append('K');
        return sb.length() == 0 ? "-" : sb.toString();
    }

    /**
     * A character as something readable. A dead key is its own answer rather
     * than a code point: {@code getUnicodeChar} returns the accent with
     * {@link KeyCharacterMap#COMBINING_ACCENT} set, and what is done with that
     * is a decision rather than an encoding.
     */
    private static String ch(int cp) {
        if (cp == 0) {
            return "-";
        }
        if ((cp & KeyCharacterMap.COMBINING_ACCENT) != 0) {
            return "dead" + Integer.toHexString(cp & KeyCharacterMap.COMBINING_ACCENT_MASK);
        }
        if (cp >= 0x21 && cp <= 0x7e) {
            return "'" + (char) cp + "'";
        }
        return "u+" + Integer.toHexString(cp);
    }

    /**
     * The keyboard itself, once per device id it produces an event from. Which
     * device an event came from is half of what the trace is for: the system
     * IME's events and a real keyboard's arrive at the same place, and only this
     * says which was which.
     */
    private void describe(int deviceId) {
        if (!describedDevices.add(deviceId)) {
            return;
        }
        final InputDevice dev = InputDevice.getDevice(deviceId);
        if (dev == null) {
            write("# device " + deviceId + " (gone or virtual)");
            return;
        }
        write(String.format(Locale.ROOT, "# device %d \"%s\" vid=%04x pid=%04x keyboardType=%d sources=%08x",
                deviceId, dev.getName(), dev.getVendorId(), dev.getProductId(),
                dev.getKeyboardType(), dev.getSources()));
    }

    private void open() {
        final File dir = Recordings.dir(ctx, Recordings.KEYS);
        if (dir == null || (!dir.isDirectory() && !dir.mkdirs())) {
            Log.e(TAG, "cannot create " + dir);
            return;
        }
        File file;
        int n = 0;
        do {
            file = new File(dir, String.format(Locale.ROOT, "%03d.keys", ++n));
        } while (file.exists());
        try {
            out = new OutputStreamWriter(new FileOutputStream(file), StandardCharsets.UTF_8);
        } catch (IOException e) {
            Log.e(TAG, "open " + file, e);
            return;
        }
        lines = 0;
        describedDevices.clear();
        lastPath = file.getName();
        Log.i(TAG, "tracing to " + file);
        write("# key trace: every key event, and what the physical keyboard made of it");
        write(HEADER);
    }

    private void close() {
        if (out == null) {
            return;
        }
        try {
            out.close();
        } catch (IOException e) {
            Log.e(TAG, "close", e);
        }
        out = null;
        Log.i(TAG, "traced " + lines + " events to " + lastPath);
    }

    /**
     * Flushed per line rather than buffered: a walk ends by killing the app more
     * often than by disarming, and a trace that only exists once it is closed
     * would be lost every time.
     */
    private void write(String s) {
        if (out == null) {
            return;
        }
        try {
            out.write(s);
            out.write('\n');
            out.flush();
        } catch (IOException e) {
            Log.e(TAG, "write", e);
        }
        if (!s.startsWith("#") && !s.equals(HEADER)) {
            lines++;
        }
    }
}
