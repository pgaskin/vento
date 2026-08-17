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
    public static final String KEY_TWO_LINE_KEYS = "twoLineKeys";  // the extension row's shape
    public static final String KEY_MAC_KEYS = "macKeys";           // Option and CMD on it
    public static final String KEY_CONTROLS = "controls";          // which affordance the session has
    public static final String KEY_TOOLBAR_POS = "toolbarPos";     // ... and where that one sits, %
    public static final String KEY_CLIPBOARD_OUT = "clipboardOut";
    public static final String KEY_CLIPBOARD_IN = "clipboardIn";
    public static final String KEY_REGION_HINTS = "regionHints";  // where the controls are
    public static final String KEY_RELEASE_KEYS = "releaseKeys";
    public static final String KEY_SESSION_TIMEOUT = "sessionTimeout";  // minutes off screen
    public static final String KEY_DEVELOPER_MODE = "developerMode";    // the hidden rows

    /**
     * Every key in this file with the answer it gives when nothing is stored.
     * The getters below read it, an export walks it and an import checks a
     * file's keys against it — one table, because a default written in two
     * places is one that will differ in two places. A setting added above and
     * not added here is one that does not travel.
     *
     * <p>A value's <em>class</em> is what says how the key is stored, which is
     * the whole of the type information here: everything was a switch until the
     * timeout, and an import checks what a file offers against it rather than
     * against a second table of types.
     */
    static final Map<String, Object> DEFAULTS = defaults();

    static final List<String> KEYS = List.copyOf(DEFAULTS.keySet());

    private static Map<String, Object> defaults() {
        final Map<String, Object> m = new LinkedHashMap<>();
        m.put(KEY_HUD, false);
        m.put(KEY_KEEP_AWAKE, true);
        m.put(KEY_IMMERSIVE, true);
        m.put(KEY_PRIVATE_IME, true);
        m.put(KEY_PREVIEWS, true);
        m.put(KEY_LIST_VIEW, true);
        m.put(KEY_MODIFIER_RESETS_IME, true);
        m.put(KEY_TWO_LINE_KEYS, false);
        m.put(KEY_MAC_KEYS, true);
        m.put(KEY_CONTROLS, CONTROLS_REGIONS);
        m.put(KEY_TOOLBAR_POS, "35");
        m.put(KEY_CLIPBOARD_OUT, true);
        m.put(KEY_CLIPBOARD_IN, true);
        m.put(KEY_REGION_HINTS, true);
        m.put(KEY_RELEASE_KEYS, true);
        m.put(KEY_SESSION_TIMEOUT, "0");
        m.put(KEY_DEVELOPER_MODE, false);
        return Collections.unmodifiableMap(m);
    }

    private AppSettings() {
    }

    public static SharedPreferences prefs(Context ctx) {
        return ctx.getApplicationContext().getSharedPreferences(FILE, Context.MODE_PRIVATE);
    }

    private static boolean get(Context ctx, String key) {
        return prefs(ctx).getBoolean(key, (Boolean) DEFAULTS.get(key));
    }

    /**
     * What this phone answers for {@code key}, stored or not, as the class the
     * table says it is. What an export writes, so that the reading of the file
     * and the reading of the preference cannot drift apart.
     */
    static Object value(Context ctx, String key) {
        final Object def = DEFAULTS.get(key);
        return def instanceof Boolean b
                ? prefs(ctx).getBoolean(key, b)
                : prefs(ctx).getString(key, (String) def);
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
     * Whether the extension keyboard is drawn as two lines of keys grouped the
     * way a keyboard groups them, rather than as the one scrolling line this app
     * has always had.
     *
     * <p>Off by default: the second line costs 40 dp of somebody else's desktop,
     * and what it buys — the F-keys without scrolling for them — is worth that to
     * the people who use them and to nobody else.
     */
    public static boolean twoLineKeys(Context ctx) {
        return get(ctx, KEY_TWO_LINE_KEYS);
    }

    /**
     * Whether the extension keyboard keeps the two modifiers that are there for
     * a Mac at the far end: Option and Command.
     *
     * <p>They are not the same loss. Command is {@code XK_Super_L} and so is
     * Windows, so taking it away takes a label rather than a capability. Option
     * is {@code XK_ISO_Level3_Shift}, which is AltGr and not a Mac key at all
     * outside the original's labelling — it is the one key here that genuinely
     * goes, and somebody driving a German or French desktop has no other route
     * to it, since neither the soft keyboard nor a phone's physical one will
     * produce that keysym. Which is why the row names the two keys instead of
     * saying "Mac".
     *
     * <p>On by default, so no phone that has the app today changes behaviour
     * without being asked.
     */
    public static boolean macKeys(Context ctx) {
        return get(ctx, KEY_MAC_KEYS);
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
     * Whether a session still explains its controls on the first frame — and,
     * since {@link #KEY_CONTROLS} exists, asks which of them it should have.
     *
     * <p>Turned off by the dialog's own "do not ask again", and turned back on
     * in the settings tree — dismissing something by accident should not be the
     * last time it can be read. The choice it offers is saved as it is made,
     * whether or not that box is ticked: they are different questions.
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

    /** What {@link #KEY_CONTROLS} may be, in the order the rows offer them. */
    public static final String CONTROLS_TOOLBAR = "toolbar";
    public static final String CONTROLS_REGIONS = "regions";
    public static final String CONTROLS_BOTH = "both";

    /**
     * Which affordance the session screen offers: the toolbar, the tap regions,
     * or both.
     *
     * <p>The regions are the default, which is what every phone that has this
     * app already has: the toolbar is the visible one and the one the first
     * frame's dialog offers, and a phone that has not been asked keeps what it
     * was doing.
     *
     * <p>One key with three values rather than two switches, because two
     * switches have a fourth state — neither — in which the session screen has
     * no affordance at all beyond the notification and the system's own back
     * gesture, and a preference that can be set to "no way out" is one that will
     * be.
     */
    public static String controls(Context ctx) {
        final String v = (String) value(ctx, KEY_CONTROLS);
        return CONTROLS_TOOLBAR.equals(v) || CONTROLS_BOTH.equals(v) ? v : CONTROLS_REGIONS;
    }

    public static void setControls(Context ctx, String value) {
        prefs(ctx).edit().putString(KEY_CONTROLS, value).apply();
    }

    public static boolean toolbarShown(Context ctx) {
        return !CONTROLS_REGIONS.equals(controls(ctx));
    }

    public static boolean regionsShown(Context ctx) {
        return !CONTROLS_TOOLBAR.equals(controls(ctx));
    }

    /**
     * Where the toolbar sits, as a fraction of the band it may be dragged in.
     *
     * <p>Per phone rather than per connection: where a thumb reaches is about
     * the hand holding it. Stored as a percentage in a string, and parsed
     * leniently for the reason the session timeout is — a file from another
     * phone can say anything, and the middle of the band is a safe answer.
     */
    public static float toolbarPosition(Context ctx) {
        try {
            final int pc = Integer.parseInt((String) value(ctx, KEY_TOOLBAR_POS));
            return Math.min(100, Math.max(0, pc)) / 100f;
        } catch (NumberFormatException e) {
            return 0.35f;
        }
    }

    public static void setToolbarPosition(Context ctx, float fraction) {
        prefs(ctx).edit().putString(KEY_TOOLBAR_POS,
                String.valueOf(Math.round(Math.min(1, Math.max(0, fraction)) * 100))).apply();
    }

    /**
     * How long a session may stay connected with nothing looking at it, in
     * minutes, or 0 for never — which is where a phone that has not been asked
     * stays, since a connection ending by itself is a surprise until somebody
     * has asked for it.
     *
     * <p>Global rather than per connection: whether to drop a session nobody is
     * watching is about how this phone is used, not about the machine at the
     * other end. Stored as text because that is what a list row persists, and
     * parsed leniently because a file from another phone can say anything —
     * where 0 is the safe answer, being the one that changes nothing.
     */
    public static int sessionTimeout(Context ctx) {
        final String v = (String) value(ctx, KEY_SESSION_TIMEOUT);
        try {
            return Math.max(0, Integer.parseInt(v));
        } catch (NumberFormatException e) {
            return 0;
        }
    }

    /**
     * Whether this phone has asked to be shown the rows that are not for
     * everybody: the playground's two recorders and the row that deletes what
     * they wrote.
     *
     * <p>Unlocked the way Android's own developer options are — ten taps on the
     * version row — because what is behind it is worth nothing to somebody who
     * was not looking for it: a fixture recorder writes raw touch and key
     * streams to a folder, which is a thing to pull off the phone and commit,
     * not a thing to find. Hidden rather than absent, since the alternative is a
     * separate build, and then what is tested is not what ships.
     *
     * <p>A switch and not a one-way door: "Restore defaults" clears this file,
     * and this key with it, which is the way back for a phone that got here by
     * curiosity. It travels in an export like everything else in the table —
     * what it unlocks is a test surface, so a second phone of the same person's
     * arriving already unlocked is the answer they gave the first one.
     */
    public static boolean developerMode(Context ctx) {
        return get(ctx, KEY_DEVELOPER_MODE);
    }

    public static void setDeveloperMode(Context ctx, boolean on) {
        prefs(ctx).edit().putBoolean(KEY_DEVELOPER_MODE, on).apply();
    }

    /** What the row offers, in minutes, in the order it offers them. */
    public static final List<Integer> SESSION_TIMEOUTS =
            List.of(0, 1, 5, 15, 30, 45, 60, 120, 240, 480, 720, 1440);

    /** What one of those is called, on the row and on a session that hit one. */
    public static String timeoutLabel(Context ctx, int minutes) {
        if (minutes <= 0) {
            return ctx.getString(R.string.settings_session_timeout_never);
        }
        return minutes % 60 == 0
                ? ctx.getResources().getQuantityString(
                        R.plurals.duration_hours, minutes / 60, minutes / 60)
                : ctx.getResources().getQuantityString(
                        R.plurals.duration_minutes, minutes, minutes);
    }
}
