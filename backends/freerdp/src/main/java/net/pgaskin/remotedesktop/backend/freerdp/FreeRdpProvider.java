// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend.freerdp;

import android.content.Context;
import android.graphics.Insets;
import android.graphics.Rect;
import android.view.WindowInsets;
import android.view.WindowManager;
import android.view.WindowMetrics;

import net.pgaskin.remotedesktop.backend.Backend;
import net.pgaskin.remotedesktop.backend.BackendOption;
import net.pgaskin.remotedesktop.backend.BackendOption.Choice;
import net.pgaskin.remotedesktop.backend.BackendOption.Scope;
import net.pgaskin.remotedesktop.backend.BackendProvider;

import java.util.List;
import java.util.Map;

/**
 * This module's entry in a build's list of backends, and what the FreeRDP
 * backend can be told.
 *
 * <p>It began as the other RDP backend's rows copied so the two would be
 * comparable, and three of these are now things that client cannot be asked at
 * all: <b>EGFX</b>, the graphics pipeline, which is how a machine since Windows
 * 8 would rather send a desktop and the only path that carries H.264; the
 * <b>sound</b>, which is the one whole subsystem no backend behind this seam has
 * ever had; and the <b>interface size</b>, which is the only answer this app has
 * to a desktop whose text is sized for a monitor and is being read on a phone.
 *
 * <p>What is deliberately <b>not</b> here:
 *
 * <ul>
 *   <li><b>The keyboard layout.</b> Every key goes as a position against a US
 *       table, and the phone's own layout has already turned a press into a
 *       character before the backend sees it — so the layout named at connect
 *       time is what turns those positions back into the characters that were
 *       typed, and naming another one makes every letter a different letter.
 *   <li><b>The codecs one at a time.</b> Progressive and planar cost nothing and
 *       are decoded in software either way; the two H.264 switches are one
 *       switch, since the pair also selects every capability version above 8.1.
 *       What is left is video or no video, which is a value on the picture row.
 *   <li><b>Small cache and thin client.</b> Both are a phone telling a server to
 *       expect less of it. The cache they shrink is the server's, the memory it
 *       costs is the server's, and the one part that reaches the picture asks
 *       for a cheaper H.264 profile on a phone that decodes in hardware.
 *   <li><b>Drives, printers and smartcards.</b> A printer needs CUPS and a
 *       smartcard needs PC/SC, neither of which is in this build; a drive needs
 *       a POSIX path, and what a person can grant on Android is a document tree
 *       with no path at all.
 *   <li><b>The clipboard</b> (a question about this phone, answered once for
 *       every protocol in the app's own settings), <b>the domain</b> (already a
 *       field, split out of the user name) and <b>the colour depth</b> (the
 *       codec has decided what a pixel costs long before it reaches a
 *       framebuffer that is 32 bits either way).
 * </ul>
 */
public final class FreeRdpProvider implements BackendProvider {

    public static final String NLA = "Nla";
    public static final String DESKTOP_SIZE = "DesktopSize";
    public static final String SCALE = "Scale";
    public static final String MONITORS = "Monitors";
    public static final String GRAPHICS = "Graphics";
    public static final String EXPERIENCE = "Experience";
    public static final String SOUND = "Sound";
    public static final String COMPRESSION = "Compression";
    public static final String VIEW_ONLY = "ViewOnly";

    /** The two answers to "how big" that are not a number; see {@link #size}. */
    static final String REMOTE = "remote";
    static final String DEVICE = "device";

    /**
     * What to ask for when nothing better is known: the first connection to a
     * machine under {@link #REMOTE}, and anything unparseable.
     */
    private static final int[] FALLBACK = {1920, 1200};

    static final List<BackendOption> OPTIONS = List.of(
            BackendOption.choice(NLA, "NLA",
                    "Network Level Authentication authenticates using CredSSP before the session starts rather than using the server-side login screen.",
                    "prefer", Scope.CONNECTION, false,
                    new Choice("prefer", "If supported by the server"),
                    new Choice("require", "Required"),
                    new Choice("off", "Never")),
            BackendOption.choice(DESKTOP_SIZE, "Desktop size",
                    "Preferred screen resolution.",
                    "1920x1200", Scope.CONNECTION, false,
                    new Choice(REMOTE, "Do not change"),
                    new Choice(DEVICE, "Match local device"),
                    new Choice("1920x1200", "1920 × 1200"),
                    new Choice("1920x1080", "1920 × 1080"),
                    new Choice("1600x1000", "1600 × 1000"),
                    new Choice("1366x768", "1366 × 768"),
                    new Choice("1280x800", "1280 × 800"),
                    new Choice("2560x1440", "2560 × 1440")),
            BackendOption.choice(SCALE, "Display scale",
                    "Supported on Windows 8.1 and later.",
                    "100", Scope.CONNECTION, false,
                    new Choice("100", "100%"),
                    new Choice("140", "140%"),
                    new Choice("180", "180%")),
            BackendOption.choice(MONITORS, "Monitors",
                    "Simulate multiple monitors.",
                    "1", Scope.CONNECTION, false,
                    new Choice("1", "One"),
                    new Choice("2", "Two"),
                    new Choice("3", "Three")),
            BackendOption.choice(GRAPHICS, "Graphics",
                    "EGFX is the newest and most advanced codec, RemoteFX is older, and Bitmap sends raw images.",
                    "gfx", Scope.LAYERED, false,
                    new Choice("gfx", "EGFX"),
                    new Choice("gfx-novideo", "EGFX, no video"),
                    new Choice("rfx", "RemoteFX"),
                    new Choice("bitmap", "Bitmap")),
            BackendOption.choice(EXPERIENCE, "Visual effects",
                    "Controls the amount of animation and visual effects, if supported by the server.",
                    "balanced", Scope.LAYERED, false,
                    new Choice("full", "Full"),
                    new Choice("balanced", "Balanced"),
                    new Choice("plain", "Limited")),
            BackendOption.choice(SOUND, "Sound",
                    "Controls where sound is played, if supported by the server.",
                    "off", Scope.LAYERED, false,
                    new Choice("off", "Off"),
                    new Choice("local", "This phone"),
                    new Choice("remote", "The remote machine")),
            BackendOption.bool(COMPRESSION, "Compression",
                    "Enable connection-level compression. Uses less bandwidth, but more CPU.",
                    true, Scope.LAYERED, false),
            BackendOption.bool(VIEW_ONLY, "View only",
                    "Do not send input or clipboard events.",
                    false, Scope.CONNECTION, true));

    @Override
    public String id() {
        return "freerdp";
    }

    @Override
    public String name() {
        return "RDP (FreeRDP)";
    }

    @Override
    public String shortName() {
        return "FreeRDP";
    }

    @Override
    public String description() {
        return "The most robust RDP implementation. Support EGFX with hardware-accelerated H.264 video, and supports audio. This is the best one to use with a Windows RDP server.";
    }

    @Override
    public int order() {
        // Ahead of the other RDP client: it is the one that reaches a Windows
        // desktop, and the one whose picture can be a video.
        return 20;
    }

    @Override
    public List<BackendOption> options() {
        return OPTIONS;
    }

    @Override
    public Backend create(Context context, String address, String userName, String password,
                          Map<String, String> options) {
        return new FreeRdpBackend(context, address, userName, password, options);
    }

    /** The library keeps a certificate store of its own; see {@link FreeRdpBackend}. */
    @Override
    public void forgetHosts(Context context) {
        FreeRdpBackend.forgetHosts(context);
    }

    /**
     * The desktop size to ask this connection's server for, as
     * {@code {width, height}}.
     *
     * <p>Whatever comes back is the server's to decide and is reported in the
     * connection result, which is why nothing is rounded here.
     */
    static int[] size(Context ctx, String address, String value) {
        if (REMOTE.equals(value)) {
            final int[] last = DesktopSizes.remembered(ctx, address);
            return last != null ? last : FALLBACK;
        }
        if (DEVICE.equals(value)) {
            return deviceSize(ctx);
        }
        final int[] parsed = parse(value);
        return parsed != null ? parsed : FALLBACK;
    }

    /** {@code 1920x1200} as its two numbers, or null. */
    static int[] parse(String value) {
        if (value == null) {
            return null;
        }
        final int at = value.indexOf('x');
        if (at <= 0) {
            return null;
        }
        try {
            final int w = Integer.parseInt(value.substring(0, at));
            final int h = Integer.parseInt(value.substring(at + 1));
            return w > 0 && h > 0 ? new int[]{w, h} : null;
        } catch (NumberFormatException e) {
            return null;
        }
    }

    /**
     * The window a session gets on this phone, in pixels — the screen less the
     * status and navigation bars, in whichever orientation it is held now.
     *
     * <p>The bars are subtracted because the desktop is drawn in what is left of
     * the window. There is no activity here to ask — the session outlives every
     * screen — so this is the display's own window, which differs from the
     * session's only in a split screen.
     */
    private static int[] deviceSize(Context ctx) {
        final WindowManager wm = ctx.getSystemService(WindowManager.class);
        if (wm == null) {
            return FALLBACK;
        }
        final WindowMetrics metrics = wm.getCurrentWindowMetrics();
        final Insets bars = metrics.getWindowInsets().getInsetsIgnoringVisibility(
                WindowInsets.Type.systemBars() | WindowInsets.Type.displayCutout());
        final Rect bounds = metrics.getBounds();
        final int w = bounds.width() - bars.left - bars.right;
        final int h = bounds.height() - bars.top - bars.bottom;
        // Both ends of the protocol's range: the field is 16 bits, and a server
        // handed something absurd refuses the connection rather than arguing.
        if (w < 200 || h < 200 || w > 4096 || h > 4096) {
            return FALLBACK;
        }
        return new int[]{w, h};
    }
}
