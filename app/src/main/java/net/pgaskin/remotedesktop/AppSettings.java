// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.content.Context;
import android.content.SharedPreferences;

import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * App-level preferences: the ones that are about this program rather than about
 * a backend or about the feel of the input stack, which have their own files.
 * Separate files so each can be exported or reset on its own.
 */
public final class AppSettings {

    public static final String FILE = "settings_app";

    public static final String KEY_HUD = "hud";                 // the diagnostic readout
    public static final String KEY_KEEP_AWAKE = "keepAwake";
    public static final String KEY_IMMERSIVE = "immersive";     // hide the system bars
    public static final String KEY_PRIVATE_IME = "privateIme";  // the desktop as a password field
    public static final String KEY_PREVIEWS = "previews";       // a picture on the home card
    public static final String KEY_LIST_VIEW = "listView";      // the home screen as rows
    public static final String KEY_MODIFIER_RESETS_IME = "modifierResetsIme";
    public static final String KEY_CLIPBOARD_OUT = "clipboardOut";
    public static final String KEY_CLIPBOARD_IN = "clipboardIn";
    public static final String KEY_REGION_HINTS = "regionHints";  // where the controls are
    public static final String KEY_RELEASE_KEYS = "releaseKeys";

    /**
     * Every key in this file with the answer it gives when nothing is stored,
     * and every one of them a switch. The getters below read it, an export walks
     * it and an import checks a file's keys against it — one table, because a
     * default written in two places is one that will differ in two places. A
     * setting added above and not added here is one that does not travel.
     */
    static final Map<String, Boolean> DEFAULTS = defaults();

    static final List<String> KEYS = List.copyOf(DEFAULTS.keySet());

    private static Map<String, Boolean> defaults() {
        final Map<String, Boolean> m = new LinkedHashMap<>();
        m.put(KEY_HUD, false);
        m.put(KEY_KEEP_AWAKE, true);
        m.put(KEY_IMMERSIVE, true);
        m.put(KEY_PRIVATE_IME, true);
        m.put(KEY_PREVIEWS, true);
        m.put(KEY_LIST_VIEW, true);
        m.put(KEY_MODIFIER_RESETS_IME, true);
        m.put(KEY_CLIPBOARD_OUT, true);
        m.put(KEY_CLIPBOARD_IN, true);
        m.put(KEY_REGION_HINTS, true);
        m.put(KEY_RELEASE_KEYS, true);
        return Collections.unmodifiableMap(m);
    }

    private AppSettings() {
    }

    public static SharedPreferences prefs(Context ctx) {
        return ctx.getApplicationContext().getSharedPreferences(FILE, Context.MODE_PRIVATE);
    }

    private static boolean get(Context ctx, String key) {
        return prefs(ctx).getBoolean(key, DEFAULTS.get(key));
    }

    public static boolean hud(Context ctx) {
        return get(ctx, KEY_HUD);
    }

    public static boolean keepAwake(Context ctx) {
        return get(ctx, KEY_KEEP_AWAKE);
    }

    public static boolean immersive(Context ctx) {
        return get(ctx, KEY_IMMERSIVE);
    }

    /**
     * On by default: what is typed at a remote machine is as likely to be a
     * secret as what is typed into a text field here, and there is no way to
     * mark only part of a desktop, so the whole of it is treated as one.
     */
    public static boolean privateIme(Context ctx) {
        return get(ctx, KEY_PRIVATE_IME);
    }

    /** Whether a session leaves a picture of its last screenful behind. */
    public static boolean previews(Context ctx) {
        return get(ctx, KEY_PREVIEWS);
    }

    /**
     * Whether the home screen draws its connections as rows rather than cards.
     *
     * <p>Answered on the home bar rather than in here, because it is asked
     * while looking at the list — this file is where it is remembered, and the
     * settings tree deliberately does not offer it a second time.
     *
     * <p>On by default: rows are the shape that still works at twenty
     * connections, and the shape somebody has not chosen yet should be the one
     * that does not stop working. Cards remain a tap away, and a phone that has
     * been switched to them stays switched.
     */
    public static boolean listView(Context ctx) {
        return get(ctx, KEY_LIST_VIEW);
    }

    public static void setListView(Context ctx, boolean rows) {
        prefs(ctx).edit().putBoolean(KEY_LIST_VIEW, rows).apply();
    }

    /**
     * Whether pressing a modifier on the extension keyboard sends the soft
     * keyboard back to its letters, as RealVNC's viewer does: a chord is a
     * modifier on the row above and a letter on the row below, and reaching for
     * Ctrl while the IME shows symbols otherwise means going back by hand.
     */
    public static boolean modifierResetsIme(Context ctx) {
        return get(ctx, KEY_MODIFIER_RESETS_IME);
    }

    /**
     * Whether a session shares this phone's clipboard, in each direction.
     *
     * <p>Here rather than with a backend's own settings, though two of the three
     * protocols have a message for it: the question is about this phone's
     * clipboard, one answer covers every connection, and a switch per protocol
     * would be the same decision made three times. Neither touches the Paste
     * key, which types the clipboard out as keystrokes — that is somebody
     * asking, once, for the text they are looking at.
     */
    public static boolean clipboardOut(Context ctx) {
        return get(ctx, KEY_CLIPBOARD_OUT);
    }

    public static boolean clipboardIn(Context ctx) {
        return get(ctx, KEY_CLIPBOARD_IN);
    }

    /**
     * Whether a session still explains its tap regions on the first frame.
     *
     * <p>Turned off by the explanation's own "do not show again", and turned
     * back on in the settings tree — dismissing something by accident should
     * not be the last time it can be read.
     */
    public static boolean regionHints(Context ctx) {
        return get(ctx, KEY_REGION_HINTS);
    }

    /**
     * Whether leaving a session lets go of every key still held at the far end.
     *
     * <p>On by default. "Held" means held over there and stays that way until
     * something says otherwise, so a session left mid-chord — the home gesture
     * with Ctrl down, a call arriving — leaves a modifier stuck on the remote
     * machine for the rest of the session, where every keystroke afterwards is
     * a shortcut. The session screen already releases each key it knows about;
     * this is the sweep for the ones it does not, and it is here rather than in
     * five backends because the question is about this phone.
     *
     * <p>Off is for the one case it costs something: a chord meant to survive
     * the app going away, which is what somebody driving a remote machine
     * through a second window would want.
     */
    public static boolean releaseKeys(Context ctx) {
        return get(ctx, KEY_RELEASE_KEYS);
    }

    public static void setRegionHints(Context ctx, boolean show) {
        prefs(ctx).edit().putBoolean(KEY_REGION_HINTS, show).apply();
    }
}
