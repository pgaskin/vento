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
                    "Cursor travel is scaled by a jerk-based factor, clamped to "
                            + "1.1–5.0. Off moves the cursor exactly as far as "
                            + "the finger.",
                    c -> c.accelEnabled, (c, v) -> c.accelEnabled = v),
            bool("accelAdaptive", "Fade acceleration when slow",
                    "Below 0.15 dp/ms the factor is held at its 1.1 floor, and "
                            + "the full curve returns above 0.6 dp/ms. Off "
                            + "applies the curve at every speed.",
                    c -> c.accelAdaptive, (c, v) -> c.accelAdaptive = v),
            bool("axisLockEnabled", "Axis locking",
                    "A straight drag slower than 0.25 dp/ms has its minor axis "
                            + "zeroed, which holds a text selection on one line. "
                            + "The lock releases while the direction turns, so a "
                            + "circle stays a circle.",
                    c -> c.axisLockEnabled, (c, v) -> c.axisLockEnabled = v),
            bool("inertiaEnabled", "Momentum",
                    "The cursor glides on after a flick, starting 50 ms after "
                            + "the finger lifts and losing 15% of its speed every "
                            + "10 ms. Off stops the cursor with the finger.",
                    c -> c.inertiaEnabled, (c, v) -> c.inertiaEnabled = v),
            bool("rawMotionWhenRelative", "Send raw motion when the remote owns the cursor",
                    "Some machines keep their own cursor and are told how far "
                            + "the finger moved rather than where it is, and "
                            + "they accelerate that motion themselves. "
                            + "Acceleration, axis locking and momentum are all "
                            + "skipped in those sessions. Off applies them here "
                            + "as well, on top of whatever the machine does.",
                    c -> c.rawMotionWhenRelative, (c, v) -> c.rawMotionWhenRelative = v),
            bool("bumpScrollEnabled", "Bump scroll",
                    "A held drag pushed into the outer 24 dp of the screen keeps "
                            + "scrolling the desktop, 12 dp every 100 ms.",
                    c -> c.bumpScrollEnabled, (c, v) -> c.bumpScrollEnabled = v),
            bool("naturalScrolling", "Natural scrolling",
                    "Two-finger scrolling and a physical wheel are inverted, so "
                            + "the content follows the fingers.",
                    c -> c.naturalScrolling, (c, v) -> c.naturalScrolling = v),
            bool("recentreCursorOnZoom", "Re-centre the cursor when zooming",
                    "The cursor moves to the middle of the view when the zoom "
                            + "changes. Off leaves it where it was on the desktop "
                            + "while the view scales around it.",
                    c -> c.recentreCursorOnZoom, (c, v) -> c.recentreCursorOnZoom = v),
            bool("keyboardInfoSolid", "Solid extension keyboard readout",
                    "The strip above the extension keys is part of the keyboard: "
                            + "the desktop stops above it. Off floats it over the "
                            + "last 30 dp of desktop and fades it as the cursor "
                            + "comes near, which is what the original does.",
                    c -> c.keyboardInfoSolid, (c, v) -> c.keyboardInfoSolid = v),
            bool("keyboardHaptics", "Extension keyboard haptics",
                    "The extension row buzzes on each key, and again when a "
                            + "modifier locks. The system keyboard's own setting "
                            + "is separate.",
                    c -> c.keyboardHaptics, (c, v) -> c.keyboardHaptics = v),
            bool("mouseCapture", "Capture a physical mouse",
                    "A connected mouse is captured while the session is open, so "
                            + "it reports relative motion and cannot run out of "
                            + "screen. Off leaves the phone's own cursor visible "
                            + "and stops it at the screen edge.",
                    c -> c.mouseCapture, (c, v) -> c.mouseCapture = v),
            number("mouseSpeed", "Physical mouse speed",
                    "A multiplier on a captured mouse's motion. Default 1, which "
                            + "is the pointer speed the phone applies everywhere "
                            + "else.",
                    c -> c.mouseSpeed, (c, v) -> c.mouseSpeed = v),
            number("mouseWheelStep", "Physical wheel notches per click",
                    "Wheel notches per click sent, where one notch is a detent "
                            + "on an ordinary mouse. Default 1; raise it for a "
                            + "high-resolution wheel that scrolls too far.",
                    c -> c.mouseWheelStep, (c, v) -> c.mouseWheelStep = v),
            number("clickHoldMs", "Click hold (ms)",
                    "How long a tap holds the button down, which is also the "
                            + "double-tap window and the tap-then-drag window. "
                            + "Default 250 ms.",
                    c -> (float) c.clickHoldMs, (c, v) -> c.clickHoldMs = (long) (float) v),
            number("wheelStepPx", "Two-finger scroll step (px)",
                    "Finger travel per wheel click. Default 8 px.",
                    c -> c.wheelStepPx, (c, v) -> c.wheelStepPx = v),
            number("overlayWheelTicksPerClick", "Wheel strip gearing (ticks per click)",
                    "Ticks between clicks at rate 1 on the mouse overlay's scroll "
                            + "strip, and proportionally fewer further from its "
                            + "middle. Default 3.",
                    c -> c.overlayWheelTicksPerClick, (c, v) -> c.overlayWheelTicksPerClick = v),
            number("overlayWheelMaxRate", "Wheel strip top rate (clicks per tick)",
                    "The rate at each end of the strip, with the middle at 0 and "
                            + "position scaling between them. Default 4.",
                    c -> c.overlayWheelMaxRate, (c, v) -> c.overlayWheelMaxRate = v),
            number("overlayWheelTickMs", "Wheel strip tick (ms)",
                    "The strip's clock, which the two settings above are counted "
                            + "in. Default 40 ms.",
                    c -> (float) c.overlayWheelTickMs,
                    (c, v) -> c.overlayWheelTickMs = (long) (float) v),
            number("overlayWheelStartDelayTicks", "Wheel strip repeat delay (ticks)",
                    "Ticks between the first click and the repeat, so a tap on "
                            + "the strip is one click. Default 8, or 320 ms.",
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
