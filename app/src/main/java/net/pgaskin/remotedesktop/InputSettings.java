// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.content.Context;
import android.content.SharedPreferences;

import net.pgaskin.remotedesktop.control.input.Config;

import java.util.List;
import java.util.function.BiConsumer;
import java.util.function.Function;

/**
 * The input stack's tunables, as preferences.
 *
 * <p>{@link Config} has some sixty fields and nearly all of them are
 * reverse-engineered constants that exist to be compared against the original,
 * not settings anybody wants. What is offered here is the part a person can
 * have an opinion about — plus the four wheel numbers, which can only be
 * judged against a document that really scrolls.
 *
 * <p>The table is read twice: once to build the screen and once to build a
 * {@link Config}, which is what keeps a preference that does nothing from
 * existing. The same idea as {@link net.pgaskin.remotedesktop.backend.BackendOption},
 * one layer up — except that here the app owns both ends, so a tunable is a
 * pair of lambdas onto a field rather than a string key into a map.
 *
 * <h2>Presets and defaults</h2>
 * A preference that has never been touched is <em>absent</em>, and the preset
 * answers for it. So switching {@code preset} from {@code improved} to
 * {@code faithful} moves everything that has not been explicitly overridden,
 * which is what makes the A/B square the playground has meaningful as a
 * setting. Changing the preset clears the overrides, because a half-faithful
 * stack is not a comparison of anything.
 */
public final class InputSettings {

    public static final String FILE = "settings_input";
    public static final String KEY_PRESET = "preset";
    public static final String PRESET_IMPROVED = "improved";
    public static final String PRESET_FAITHFUL = "faithful";

    /**
     * One field of {@link Config}, in both directions.
     *
     * @param read  its value in a given config, as a preference string — used
     *              for the default shown when nothing is stored
     * @param write parse a stored string back into the config
     */
    public record Tunable(String key, String label, String summary, Kind kind,
                          Function<Config, String> read, BiConsumer<Config, String> write) {
    }

    public enum Kind {
        BOOL,
        /** A number, in whatever unit the label says. Stored as text. */
        NUMBER
    }

    private static Tunable bool(String key, String label, String summary,
                                Function<Config, Boolean> get, BiConsumer<Config, Boolean> set) {
        return new Tunable(key, label, summary, Kind.BOOL,
                c -> Boolean.toString(get.apply(c)),
                (c, v) -> set.accept(c, Boolean.parseBoolean(v)));
    }

    private static Tunable number(String key, String label, String summary,
                                  Function<Config, Float> get, BiConsumer<Config, Float> set) {
        return new Tunable(key, label, summary, Kind.NUMBER,
                c -> trim(get.apply(c)),
                (c, v) -> {
                    try {
                        set.accept(c, Float.parseFloat(v.trim()));
                    } catch (NumberFormatException ignored) {
                        // A field left empty or full of nonsense keeps the preset's
                        // value; refusing to start over a typo would be worse.
                    }
                });
    }

    /** "3" rather than "3.0", since these end up in a text box someone edits. */
    private static String trim(float v) {
        return v == Math.rint(v) ? Integer.toString((int) v) : Float.toString(v);
    }

    private static final List<Tunable> TUNABLES = List.of(
            bool("accelEnabled", "Pointer acceleration",
                    "Scale cursor movement by the speed of the gesture.",
                    c -> c.accelEnabled, (c, v) -> c.accelEnabled = v),
            bool("accelAdaptive", "Adaptive acceleration",
                    "Reduce acceleration at low speeds for better precision.",
                    c -> c.accelAdaptive, (c, v) -> c.accelAdaptive = v),
            bool("axisLockEnabled", "Axis locking",
                    "Snap slow straight movements to the horizontal or vertical axis.",
                    c -> c.axisLockEnabled, (c, v) -> c.axisLockEnabled = v),
            bool("inertiaEnabled", "Momentum",
                    "Keep moving the cursor after a flick.",
                    c -> c.inertiaEnabled, (c, v) -> c.inertiaEnabled = v),
            bool("rawMotionWhenRelative", "Raw motion for remote cursors",
                    "Skip acceleration, axis locking, and momentum when the remote cursor is relative.",
                    c -> c.rawMotionWhenRelative, (c, v) -> c.rawMotionWhenRelative = v),
            bool("bumpScrollEnabled", "Edge scrolling",
                    "Pan the desktop while dragging against the edge of the screen.",
                    c -> c.bumpScrollEnabled, (c, v) -> c.bumpScrollEnabled = v),
            bool("naturalScrolling", "Natural scrolling",
                    "Invert the scroll direction of gestures and the mouse wheel.",
                    c -> c.naturalScrolling, (c, v) -> c.naturalScrolling = v),
            bool("recentreCursorOnZoom", "Recenter cursor when zooming",
                    "Move the cursor to the center of the view when the zoom changes.",
                    c -> c.recentreCursorOnZoom, (c, v) -> c.recentreCursorOnZoom = v),
            bool("keyboardInfoSolid", "Solid extension keyboard bar",
                    "Inset the desktop above the extension keyboard bar instead of fading it over the desktop.",
                    c -> c.keyboardInfoSolid, (c, v) -> c.keyboardInfoSolid = v),
            bool("keyboardHaptics", "Extension keyboard haptics",
                    "Vibrate when pressing extension keyboard keys.",
                    c -> c.keyboardHaptics, (c, v) -> c.keyboardHaptics = v),
            bool("mouseCapture", "Capture physical mouse",
                    "Capture the pointer while connected to send relative motion instead of using the local cursor.",
                    c -> c.mouseCapture, (c, v) -> c.mouseCapture = v),
            number("mouseSpeed", "Physical mouse speed",
                    "Multiplier for captured mouse motion.",
                    c -> c.mouseSpeed, (c, v) -> c.mouseSpeed = v),
            number("mouseWheelStep", "Physical mouse wheel step",
                    "Wheel detents per scroll event sent. Increase for high-resolution wheels.",
                    c -> c.mouseWheelStep, (c, v) -> c.mouseWheelStep = v),
            number("clickHoldMs", "Click hold (ms)",
                    "Button hold time for taps, and the window for double-taps and tap-then-drag.",
                    c -> (float) c.clickHoldMs, (c, v) -> c.clickHoldMs = (long) (float) v),
            number("wheelStepPx", "Two-finger scroll step (px)",
                    "Finger travel per scroll event sent.",
                    c -> c.wheelStepPx, (c, v) -> c.wheelStepPx = v),
            number("overlayWheelTicksPerClick", "Scroll bar gearing (ticks per click)",
                    "Ticks between scroll events at rate 1, scaling down towards the ends of the bar.",
                    c -> c.overlayWheelTicksPerClick, (c, v) -> c.overlayWheelTicksPerClick = v),
            number("overlayWheelMaxRate", "Scroll bar maximum rate (clicks per tick)",
                    "Scroll rate at the ends of the bar, scaling to zero at the middle.",
                    c -> c.overlayWheelMaxRate, (c, v) -> c.overlayWheelMaxRate = v),
            number("overlayWheelTickMs", "Scroll bar tick (ms)",
                    "Interval the two settings above are counted in.",
                    c -> (float) c.overlayWheelTickMs,
                    (c, v) -> c.overlayWheelTickMs = (long) (float) v),
            number("overlayWheelStartDelayTicks", "Scroll bar repeat delay (ticks)",
                    "Ticks before the first scroll event repeats, so a tap is a single scroll.",
                    c -> (float) c.overlayWheelStartDelayTicks,
                    (c, v) -> c.overlayWheelStartDelayTicks = (int) (float) v));

    private InputSettings() {
    }

    public static List<Tunable> tunables() {
        return TUNABLES;
    }

    public static SharedPreferences prefs(Context ctx) {
        return ctx.getApplicationContext().getSharedPreferences(FILE, Context.MODE_PRIVATE);
    }

    /** The preset, before any override. */
    public static Config preset(Context ctx, float density) {
        return PRESET_FAITHFUL.equals(prefs(ctx).getString(KEY_PRESET, PRESET_IMPROVED))
                ? Config.faithful(density)
                : Config.improved(density);
    }

    /** The preset with the stored overrides applied — what a session runs on. */
    public static Config config(Context ctx, float density) {
        final Config c = preset(ctx, density);
        final SharedPreferences p = prefs(ctx);
        for (Tunable t : TUNABLES) {
            final String v = p.getString(t.key(), null);
            if (v != null && !v.isEmpty()) {
                t.write().accept(c, v);
            }
        }
        return c;
    }

    /** Forget every override, so the preset answers for all of them again. */
    public static void clearOverrides(Context ctx) {
        final SharedPreferences.Editor e = prefs(ctx).edit();
        for (Tunable t : TUNABLES) {
            e.remove(t.key());
        }
        e.apply();
    }
}
