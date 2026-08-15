// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend.tigervnc;

import android.content.Context;

import net.pgaskin.remotedesktop.backend.Backend;
import net.pgaskin.remotedesktop.backend.BackendOption;
import net.pgaskin.remotedesktop.backend.BackendOption.Choice;
import net.pgaskin.remotedesktop.backend.BackendOption.Scope;
import net.pgaskin.remotedesktop.backend.BackendProvider;

import java.util.List;
import java.util.Map;

/**
 * This module's entry in a build's list of backends, and what the TigerVNC
 * backend can be told.
 *
 * <p>The option list is the libvncclient one with a different meaning under the
 * encoding row: TigerVNC's client always offers every encoding it can decode
 * and names one of them preferred, so this is a hint rather than a restriction
 * and there is no "automatic" to choose — the server chooses either way.
 *
 * <p>Where there <em>is</em> an automatic to offer is the picture quality and
 * the colour depth, and only here: this is the one client in the repository
 * that measures its own throughput, so it is the one that can move a setting on
 * somebody's behalf. Both rules are the viewer's own, thresholds included.
 */
public final class TigerVncProvider implements BackendProvider {

    public static final String ENCODING = "Encoding";
    public static final String COMPRESSION = "Compression";
    public static final String QUALITY = "Quality";
    /** The same key, values and words the RealVNC backend's colour row has. */
    public static final String COLOUR = "ColorLevel";
    public static final String VIEW_ONLY = "ViewOnly";
    public static final String SHARED = "Shared";
    public static final String BELL = "AcceptBell";
    public static final String H264 = "H264";

    /** RFB encoding numbers, which is what this client's preference is. */
    private static final int ENCODING_RAW = 0;
    private static final int ENCODING_RRE = 2;
    private static final int ENCODING_HEXTILE = 5;
    private static final int ENCODING_TIGHT = 7;
    private static final int ENCODING_ZRLE = 16;
    private static final int ENCODING_H264 = 50;

    /**
     * Automatic, for a quality or a colour depth. Both are the client's own
     * rules over its own bandwidth estimate, which is the row the panel already
     * shows as the line speed — this is the one client here that measures
     * anything, so it is the only one with an automatic to offer.
     */
    static final int LEVEL_AUTO = -2;

    static final List<BackendOption> OPTIONS = List.of(
            // ---- the picture: a connection's, defaulting to the backend's ----
            BackendOption.choice(ENCODING, "Preferred encoding",
                    "Which encoding to ask the server for first. This client "
                            + "offers every encoding it can decode whatever is "
                            + "chosen here, and the server has the last word.",
                    "tight", Scope.LAYERED, true,
                    new Choice("tight", "Tight"),
                    new Choice("h264", "H.264", null, H264),
                    new Choice("zrle", "ZRLE"),
                    new Choice("hextile", "Hextile"),
                    new Choice("rre", "RRE"),
                    new Choice("raw", "Raw (uncompressed)")),
            BackendOption.choice(COMPRESSION, "Compression",
                    "The compressLevel pseudo-encoding sent with SetEncodings, 1 "
                            + "to 9. Higher costs the remote machine's processor "
                            + "and sends less over the link. Automatic sends none "
                            + "of it, which leaves the server its own.",
                    "auto", Scope.LAYERED, true,
                    new Choice("auto", "Automatic"),
                    new Choice("1", "1 (fastest)"),
                    new Choice("3", "3"),
                    new Choice("6", "6 (balanced)"),
                    new Choice("9", "9 (smallest)")),
            BackendOption.choice(QUALITY, "Picture quality",
                    "What Tight is allowed to throw away. Lossless sends every "
                            + "pixel exactly; anything else lets the server send "
                            + "photographs and gradients as JPEG, which is most of "
                            + "what Tight is faster for. Automatic follows the "
                            + "measured line speed. Only Tight uses this.",
                    "lossless", Scope.LAYERED, true,
                    new Choice("auto", "Automatic"),
                    new Choice("lossless", "Lossless"),
                    new Choice("9", "9 (best)"),
                    new Choice("7", "7"),
                    new Choice("5", "5 (balanced)"),
                    new Choice("3", "3"),
                    new Choice("0", "0 (smallest)")),
            BackendOption.choice(COLOUR, "Colour depth",
                    "The colour depth of the pixel format asked for. Less colour "
                            + "is fewer bytes over the link whatever the encoding, "
                            + "and visible banding on a photograph. Automatic "
                            + "follows the measured line speed. Applies at once.",
                    "full", Scope.LAYERED, true,
                    new Choice("auto", "Automatic"),
                    new Choice("full", "Full colour (24-bit)"),
                    new Choice("rgb222", "6-bit (rgb222)"),
                    new Choice("rgb111", "3-bit (rgb111)")),

            // ---- per connection ---------------------------------------------
            BackendOption.bool(VIEW_ONLY, "View only",
                    "The desktop is shown and no key, pointer or clipboard event "
                            + "is sent.",
                    false, Scope.CONNECTION, true),
            BackendOption.bool(SHARED, "Share the desktop",
                    "Other viewers stay connected. Off asks the server to "
                            + "disconnect them.",
                    true, Scope.CONNECTION, false),

            // ---- per backend -------------------------------------------------
            BackendOption.bool(H264, "Offer H.264 video",
                    "Lets a server send the desktop as video, decoded by the "
                            + "phone's video hardware. It sends about a quarter "
                            + "of the data and costs about twice the processor, "
                            + "since a frame costs the same however little "
                            + "changed, and the picture is never exact. Few "
                            + "servers can send it. Off unless you want it.",
                    false, Scope.GLOBAL, true),
            BackendOption.bool(BELL, "Bell",
                    "The phone buzzes when the remote machine rings its bell.",
                    true, Scope.GLOBAL, false));

    @Override
    public String id() {
        return "tigervnc";
    }

    @Override
    public String name() {
        return "VNC (TigerVNC)";
    }

    @Override
    public String shortName() {
        return "TigerVNC";
    }

    @Override
    public String description() {
        return "Tight, H.264 video on the phone's own decoder, and the only client here that "
                + "can talk to a RealVNC server or follow the line speed it measures. It "
                + "decodes on four threads, which drops the fewest frames and costs two to "
                + "five times the processor of the others.";
    }

    /** After libvncclient, which came first. */
    @Override
    public int order() {
        return 17;
    }

    @Override
    public List<BackendOption> options() {
        return OPTIONS;
    }

    @Override
    public Backend create(Context context, String address, String userName, String password,
                          Map<String, String> options) {
        return new TigerVncBackend(context, address, userName, password, options);
    }

    /** A 0–9 option as the native side wants it, or −1 for "do not send it". */
    static int level(String value) {
        try {
            return Integer.parseInt(value);
        } catch (NumberFormatException e) {
            return -1;
        }
    }

    /**
     * The quality option, which has one more answer than the compression one:
     * "auto" is the client's own rule over its estimate, and everything else is
     * a level, with "lossless" falling through to the absence of one.
     */
    static int quality(String value) {
        return "auto".equals(value) ? LEVEL_AUTO : level(value);
    }

    /** The colour option as the shim's level: 0 full, 1 rgb222, 2 rgb111. */
    static int colorLevel(String value) {
        return switch (value == null ? "" : value) {
            case "auto" -> LEVEL_AUTO;
            case "rgb222" -> 1;
            case "rgb111" -> 2;
            default -> 0;
        };
    }

    /** The encoding option as the RFB number this client prefers. */
    static int encoding(String value) {
        if (value == null) {
            return ENCODING_TIGHT;
        }
        return switch (value) {
            case "h264" -> ENCODING_H264;
            case "zrle" -> ENCODING_ZRLE;
            case "hextile" -> ENCODING_HEXTILE;
            case "rre" -> ENCODING_RRE;
            case "raw" -> ENCODING_RAW;
            default -> ENCODING_TIGHT;
        };
    }
}
