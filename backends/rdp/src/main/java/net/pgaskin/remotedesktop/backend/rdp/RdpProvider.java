// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend.rdp;

import android.content.Context;
import android.graphics.Insets;
import android.graphics.Rect;
import android.view.WindowInsets;
import android.view.WindowManager;
import android.view.WindowMetrics;

import net.pgaskin.remotedesktop.backend.Backend;
import net.pgaskin.remotedesktop.backend.BackendOption;
import net.pgaskin.remotedesktop.backend.BackendOption.Choice;
import net.pgaskin.remotedesktop.backend.BackendProvider;
import net.pgaskin.remotedesktop.backend.BackendOption.Scope;

import java.util.List;
import java.util.Map;

/**
 * This module's entry in a build's list of backends, and what the RDP backend
 * can be told.
 *
 * <p>Shorter than {@code RfbProvider}, and shorter for a different reason. RFB's
 * list is short because the protocol has little to say; this one is short
 * because almost everything RDP negotiates is negotiated <em>well</em> — the
 * codec, the compression, the colour depth — and an option that re-decides what
 * the two ends already agreed on is a way to make a session worse.
 *
 * <p>Five things are deliberately <b>not</b> here:
 *
 * <ul>
 *   <li><b>The keyboard layout.</b> RDP sends key <em>positions</em>, and
 *       {@code rdp::keymap} maps keysyms onto a US layout, so the layout named
 *       at connect time has to be the one that table was written for. Offering a
 *       choice would let someone pick a layout that turns every letter into a
 *       different letter.
 *   <li><b>The clipboard.</b> The channel is open in both directions, but
 *       whether to share this phone's clipboard is a question about the phone
 *       and is answered once for every protocol, in the app's own settings.
 *   <li><b>The domain</b>, RDP's third credential, which is not a row because
 *       it is already a field: {@code DOMAIN&#92;user} and {@code user@domain}
 *       are split out of the user name, which is what every RDP client takes
 *       and what anyone who has one will type.
 *   <li><b>The colour depth</b>, which the three VNC backends do offer. There
 *       it is a wire format the server encodes into; here the picture arrives
 *       as RemoteFX or as bitmaps, and the codec has already decided what a
 *       pixel costs — 16-bit would apply to one of those two paths, save
 *       nothing where the codec is on, and put a conversion in front of a
 *       framebuffer that is 32 bits either way.
 *   <li><b>The graphics pipeline (MS-RDPEGFX)</b>, which is how a machine since
 *       Windows 8 would rather send a desktop and the only path that carries
 *       H.264. It was built, driven against both servers and taken out again:
 *       IronRDP's implementation is incomplete and, where it is complete, wrong.
 *       Nothing in the library sets the capability flag a server needs before it
 *       will open the channel; the client receives RFX Progressive and discards
 *       it, though a finished decoder for it sits unused one crate away; and the
 *       ClearCodec that Windows sends for most of an interface is rejected
 *       outright — fifteen ClearCodec and twelve progressive failures in one
 *       short session, which is a desktop with solid rectangles of the wrong
 *       colour through it. Patched, all of that gets as far as working against
 *       xrdp and no further, so what would ship is a choice that is right on one
 *       server and corrupt on the one the protocol belongs to.
 * </ul>
 */
public final class RdpProvider implements BackendProvider {

    public static final String NLA = "Nla";
    public static final String DESKTOP_SIZE = "DesktopSize";
    public static final String MONITORS = "Monitors";
    public static final String GRAPHICS = "Graphics";
    public static final String EXPERIENCE = "Experience";
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
            // ---- per connection ---------------------------------------------
            BackendOption.choice(NLA, "Authentication",
                    "Network Level Authentication checks your password over "
                            + "CredSSP before the session exists. A server without "
                            + "it falls back to its own login screen.",
                    "prefer", Scope.CONNECTION, false,
                    new Choice("prefer", "Where the server offers it"),
                    new Choice("require", "Required"),
                    new Choice("off", "Never")),
            BackendOption.choice(DESKTOP_SIZE, "Desktop size",
                    "The size the remote machine is asked to make the desktop, in "
                            + "pixels. RDP creates the session at this size rather "
                            + "than showing one that already exists.",
                    "1920x1200", Scope.CONNECTION, false,
                    new Choice(REMOTE, "The size it was last time"),
                    new Choice(DEVICE, "This phone's screen"),
                    new Choice("1920x1200", "1920 × 1200"),
                    new Choice("1920x1080", "1920 × 1080"),
                    new Choice("1600x1000", "1600 × 1000"),
                    new Choice("1366x768", "1366 × 768"),
                    new Choice("1280x800", "1280 × 800"),
                    new Choice("2560x1440", "2560 × 1440")),
            BackendOption.choice(MONITORS, "Monitors",
                    "How many screens the remote machine is asked to make, each "
                            + "the size above, side by side. More than one is a "
                            + "wider desktop to move the viewport over, and the "
                            + "remote machine treats them as separate screens — a "
                            + "window maximises onto one of them. Default: one.",
                    "1", Scope.CONNECTION, false,
                    new Choice("1", "One"),
                    new Choice("2", "Two"),
                    new Choice("3", "Three")),
            BackendOption.choice(GRAPHICS, "Graphics",
                    "RemoteFX is a video codec: much less data over a picture "
                            + "with photographs in it, and a little of the detail "
                            + "lost. Bitmaps are exact and cost what they cost.",
                    "rfx", Scope.LAYERED, false,
                    new Choice("rfx", "RemoteFX"),
                    new Choice("bitmap", "Bitmaps")),
            BackendOption.choice(EXPERIENCE, "Visual effects",
                    "What the remote machine may spend the link on drawing for "
                            + "you. Fewer effects is a plainer desktop and less to "
                            + "send, and the machine decides some of the rest for "
                            + "itself from what this says about the connection.",
                    "balanced", Scope.LAYERED, false,
                    new Choice("full", "Everything"),
                    new Choice("balanced", "No animation"),
                    new Choice("plain", "As little as possible")),
            BackendOption.bool(COMPRESSION, "Compression",
                    "The remote machine compresses what it sends, on top of "
                            + "whatever the picture is already encoded as. It "
                            + "costs it some work and changes nothing about what "
                            + "is drawn. Default: on.",
                    true, Scope.LAYERED, false),
            BackendOption.bool(VIEW_ONLY, "View only",
                    "The desktop is shown and no key, pointer or clipboard event "
                            + "is sent.",
                    false, Scope.CONNECTION, true));

    @Override
    public String id() {
        return "rdp";
    }

    @Override
    public String name() {
        return "RDP (IronRDP)";
    }

    @Override
    public String shortName() {
        return "IronRDP";
    }

    @Override
    public String description() {
        return "The lighter of the two RDP clients, at about half the decoding of the other for "
                + "more dropped frames. It has no EGFX and no sound, so no video either, and a "
                + "recent Windows machine can be very slow over it.";
    }

    @Override
    public int order() {
        return 25;
    }

    @Override
    public List<BackendOption> options() {
        return OPTIONS;
    }

    @Override
    public Backend create(Context context, String address, String userName, String password,
                          Map<String, String> options) {
        return new RdpBackend(context, address, userName, password, options);
    }

    /**
     * The desktop size to ask this connection's server for, as
     * {@code {width, height}}.
     *
     * <p>Whatever comes back is the server's to decide and is reported in the
     * connection result — an odd width comes back one pixel narrower, which is
     * why nothing is rounded here.
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
     * the window, so it is that rectangle, and not the panel, that makes a scale
     * of 1 look like one pixel each. There is no activity here to ask — the
     * session outlives every screen — so this is the display's own window, which
     * differs from the session's only in a split screen.
     */
    private static int[] deviceSize(Context ctx) {
        final WindowManager wm = ctx.getSystemService(WindowManager.class);
        if (wm == null) {
            return FALLBACK;
        }
        final WindowMetrics metrics = wm.getCurrentWindowMetrics();
        // Ignoring visibility: a bar hidden at this instant is one the session
        // will show again, and a desktop that changed size for it would be
        // worse than one that is a few pixels short.
        final Insets bars = metrics.getWindowInsets().getInsetsIgnoringVisibility(
                WindowInsets.Type.systemBars() | WindowInsets.Type.displayCutout());
        final Rect bounds = metrics.getBounds();
        final int w = bounds.width() - bars.left - bars.right;
        final int h = bounds.height() - bars.top - bars.bottom;
        // Both ends of the protocol's range: the field is 16 bits, and a server
        // that is handed something absurd refuses the connection rather than
        // arguing about it.
        if (w < 200 || h < 200 || w > 4096 || h > 4096) {
            return FALLBACK;
        }
        return new int[]{w, h};
    }
}
