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
 * App-level preferences: those about this app rather than about a backend or
 * the input stack's tuning, which have their own files. Separate files so each
 * can be exported or reset on its own.
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
    public static final String KEY_CONTROLS = "controls";          // which controls the session has
    public static final String KEY_TOOLBAR_POS = "toolbarPos";     // ... and where that one sits, %
    public static final String KEY_CLIPBOARD_OUT = "clipboardOut";
    public static final String KEY_CLIPBOARD_IN = "clipboardIn";
    public static final String KEY_REGION_HINTS = "regionHints";  // where the controls are
    public static final String KEY_RELEASE_KEYS = "releaseKeys";
    public static final String KEY_SESSION_TIMEOUT = "sessionTimeout";  // minutes off screen
    public static final String KEY_DEVELOPER_MODE = "developerMode";    // the hidden rows
    public static final String KEY_EXTERNAL_LAUNCH = "externalLaunch";  // a hidden row
    public static final String KEY_FORCE_SOCKETPAIR = "forceSocketpair"; // a hidden row

    /**
     * Every key in this file with its default value. The getters below read it,
     * an export walks it and an import checks a file's keys against it: one
     * table, so the defaults cannot diverge. A setting added above but not here
     * is not exported.
     *
     * <p>A value's <em>class</em> says how the key is stored, and is the only
     * type information here: an import checks a file's values against it rather
     * than against a second table of types.
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
        m.put(KEY_EXTERNAL_LAUNCH, false);
        m.put(KEY_FORCE_SOCKETPAIR, false);
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
     * The value of {@code key}, stored or default, as the class the table gives
     * it. An export writes this, so the exported value and the value read by the
     * app cannot differ.
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
     * On by default: text typed into a remote machine is as likely to be a
     * secret as text typed into a field here, and there is no way to mark only
     * part of a desktop, so the whole desktop is treated as a password field.
     */
    public static boolean privateIme(Context ctx) {
        return get(ctx, KEY_PRIVATE_IME);
    }

    /** Whether a session saves a preview of its last frame for the home screen. */
    public static boolean previews(Context ctx) {
        return get(ctx, KEY_PREVIEWS);
    }

    /**
     * Whether the home screen draws its connections as rows rather than cards.
     *
     * <p>Switched from the home screen's bar, where the list is visible; it is
     * only stored here, and the settings tree deliberately does not offer it.
     *
     * <p>On by default: rows still work with twenty connections, so they are
     * the safer default. Cards are a tap away, and the choice persists.
     */
    public static boolean listView(Context ctx) {
        return get(ctx, KEY_LIST_VIEW);
    }

    public static void setListView(Context ctx, boolean rows) {
        prefs(ctx).edit().putBoolean(KEY_LIST_VIEW, rows).apply();
    }

    /**
     * Whether pressing a modifier on the extension keyboard switches the soft
     * keyboard back to its letters, as RealVNC's viewer does: a chord is a
     * modifier on the row above and a letter on the keyboard below, and pressing
     * Ctrl while the IME shows symbols otherwise means switching back by hand.
     */
    public static boolean modifierResetsIme(Context ctx) {
        return get(ctx, KEY_MODIFIER_RESETS_IME);
    }

    /**
     * Whether the extension keyboard is drawn as two lines of keys grouped as on
     * a physical keyboard, rather than as one scrolling line.
     *
     * <p>Off by default: the second line covers 40 dp of the desktop, and the
     * benefit (the F-keys without scrolling) only matters to users of those
     * keys.
     */
    public static boolean twoLineKeys(Context ctx) {
        return get(ctx, KEY_TWO_LINE_KEYS);
    }

    /**
     * Whether the extension keyboard keeps the two modifiers that are there for
     * a Mac at the far end: Option and Command.
     *
     * <p>Removing them has different effects. Command is {@code XK_Super_L}, as is
     * Windows, so removing it removes a label rather than a capability. Option is
     * {@code XK_ISO_Level3_Shift}, which is AltGr and only a Mac key in RealVNC
     * Viewer's labelling. It is the only key here that is actually lost: on a
     * German or French desktop there is no other way to send it, since neither
     * the soft keyboard nor a physical keyboard on the phone produces that
     * keysym. This is why the row names the two keys instead of saying "Mac".
     *
     * <p>On by default, so existing installs keep their behaviour.
     */
    public static boolean macKeys(Context ctx) {
        return get(ctx, KEY_MAC_KEYS);
    }

    /**
     * Whether a session shares this phone's clipboard, in each direction.
     *
     * <p>Here rather than with a backend's own settings, though most protocols
     * have their own clipboard messages: the setting is about this phone's
     * clipboard, one value covers every connection, and a switch per backend
     * would repeat the same decision. Neither affects the Paste key, which types
     * the clipboard out as keystrokes on an explicit request.
     */
    public static boolean clipboardOut(Context ctx) {
        return get(ctx, KEY_CLIPBOARD_OUT);
    }

    public static boolean clipboardIn(Context ctx) {
        return get(ctx, KEY_CLIPBOARD_IN);
    }

    /**
     * Whether a session explains its controls on the first frame and asks which
     * of them ({@link #KEY_CONTROLS}) it should have.
     *
     * <p>Turned off by the dialog's own "do not ask again", and back on in the
     * settings tree, so dismissing it by accident is not permanent. The controls
     * choice in the dialog is saved when made, whether or not that box is
     * ticked.
     */
    public static boolean regionHints(Context ctx) {
        return get(ctx, KEY_REGION_HINTS);
    }

    /**
     * Whether leaving a session lets go of every key still held at the far end.
     *
     * <p>On by default. A key stays held at the far end until it is released, so
     * a session left mid-chord (the home gesture with Ctrl down, an incoming
     * call) leaves a modifier stuck on the remote machine, turning every later
     * keystroke into a shortcut. The session screen already releases each key it
     * knows about; this releases the rest, and it is here rather than in each
     * backend because it is about this phone.
     *
     * <p>Off is for a chord meant to survive the app going away, as when
     * controlling a remote machine through a second window.
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
     * Which controls the session screen offers: the toolbar, the tap regions,
     * or both.
     *
     * <p>The regions are the default, since existing installs already have them;
     * the toolbar is the visible option and the one the first frame's dialog
     * offers, and an install that has not been asked keeps its behaviour.
     *
     * <p>One key with three values rather than two switches, because two
     * switches have a fourth state (neither) in which the session screen has no
     * controls beyond the notification and the system back gesture.
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
     * <p>Per phone rather than per connection: where a thumb reaches depends on
     * the hand holding the phone. Stored as a percentage in a string, and parsed
     * leniently as the session timeout is, since an imported file can contain
     * anything; an unparseable value falls back to the default of 35%.
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
     * How long a session may stay connected while not on screen, in minutes, or
     * 0 for never. Never is the default, since a connection ending by itself is
     * unexpected unless the user asked for it.
     *
     * <p>Global rather than per connection: whether to drop a session that is
     * not being watched depends on how this phone is used, not on the remote
     * machine. Stored as text because that is what a list row persists, and
     * parsed leniently because an imported file can contain anything; 0 is the
     * safe fallback since it changes nothing.
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
     * Whether the developer rows are shown: the playground's two recorders,
     * the row that deletes their recordings, {@link #externalLaunch} and
     * {@link #forceSocketpair}.
     *
     * <p>Unlocked like Android's own developer options, with ten taps on the
     * version row, because the recorders are of no use to other users: they
     * write raw touch and key streams to a folder, to be pulled off the phone
     * and committed as test fixtures. Hidden rather than left out of the build,
     * since a separate build would mean testing something other than what ships.
     *
     * <p>Reversible: "Restore defaults" clears this file, including this key. It
     * is exported like everything else in the table, since it only unlocks a
     * test surface.
     */
    public static boolean developerMode(Context ctx) {
        return get(ctx, KEY_DEVELOPER_MODE);
    }

    public static void setDeveloperMode(Context ctx, boolean on) {
        prefs(ctx).edit().putBoolean(KEY_DEVELOPER_MODE, on).apply();
    }

    /**
     * Whether {@link SessionActivity} may start a connection from the address,
     * credentials and options in its launch intent ({@code scripts/session.sh}
     * from adb), rather than only from a saved connection's id.
     *
     * <p>A developer row, and only in effect in developer mode: turning that off
     * (with "Restore defaults", or by importing a file) hides the row, and a
     * stored value left from before must not keep working unseen.
     */
    public static boolean externalLaunch(Context ctx) {
        return developerMode(ctx) && get(ctx, KEY_EXTERNAL_LAUNCH);
    }

    /**
     * Whether each connection a session dials through vento is relayed through
     * a socketpair ({@code Backends.forceSocketpair()}), to test the backends
     * on a socket that is not TCP.
     *
     * <p>A developer row, in effect only in developer mode, as
     * {@link #externalLaunch} is.
     */
    public static boolean forceSocketpair(Context ctx) {
        return developerMode(ctx) && get(ctx, KEY_FORCE_SOCKETPAIR);
    }

    /** The row's choices, in minutes, in the order shown. */
    public static final List<Integer> SESSION_TIMEOUTS =
            List.of(0, 1, 5, 15, 30, 45, 60, 120, 240, 480, 720, 1440);

    /** The label for one of those, on the row and on a session that timed out. */
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
