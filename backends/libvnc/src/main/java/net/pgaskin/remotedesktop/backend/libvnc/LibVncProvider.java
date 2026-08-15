// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend.libvnc;

import android.content.Context;

import net.pgaskin.remotedesktop.backend.Backend;
import net.pgaskin.remotedesktop.backend.BackendOption;
import net.pgaskin.remotedesktop.backend.BackendOption.Choice;
import net.pgaskin.remotedesktop.backend.BackendOption.Scope;
import net.pgaskin.remotedesktop.backend.BackendProvider;

import java.util.List;
import java.util.Map;

/**
 * This module's entry in a build's list of backends, and what the libvncclient
 * backend can be told.
 *
 * <p>The option list is the same shape as the RFB one and differs where the two
 * clients differ, which is the point of having both: <b>Tight is here</b>, with
 * a JPEG quality beside it, and <b>the encryption choice is not</b> — this
 * client does VeNCrypt, but it takes whatever the server offers and there is no
 * call to prefer, require or refuse it.
 *
 * <p>The colour row is the same question the RealVNC and TigerVNC backends ask,
 * asked in the same words, and it is the only one of the three that cannot
 * change mid-session: libvncclient sends a pixel format and reallocates around
 * it with no point at which nothing is in flight, so a change here would be
 * decoding whatever is already on its way at the wrong width.
 */
public final class LibVncProvider implements BackendProvider {

    public static final String ENCODING = "Encoding";
    public static final String COMPRESSION = "Compression";
    public static final String QUALITY = "Quality";
    /** The same key, values and words the RealVNC backend's colour row has. */
    public static final String COLOUR = "ColorLevel";
    public static final String VIEW_ONLY = "ViewOnly";
    public static final String SHARED = "Shared";
    public static final String BELL = "AcceptBell";

    /**
     * The encodings string libvncclient parses, best first, and "Automatic" is
     * its own default list unchanged — including the two nothing here decodes
     * any better than the server's alternative, since what the list decides is
     * what the server picks.
     */
    static final String AUTO_ENCODINGS = "tight zrle ultra copyrect hextile zlib corre rre raw";

    static final List<BackendOption> OPTIONS = List.of(
            // ---- the picture: a connection's, defaulting to the backend's ----
            BackendOption.choice(ENCODING, "Encoding",
                    "How the server compresses the picture. Automatic offers "
                            + "every encoding this client knows, best first, and "
                            + "lets the server choose. Tight sends photographs and "
                            + "gradients as JPEG.",
                    "auto", Scope.LAYERED, true,
                    new Choice("auto", "Automatic"),
                    new Choice("tight", "Tight"),
                    new Choice("zrle", "ZRLE"),
                    new Choice("trle", "TRLE"),
                    new Choice("ultra", "Ultra"),
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
                            + "what Tight is faster for. Only Tight uses this.",
                    "lossless", Scope.LAYERED, true,
                    new Choice("lossless", "Lossless"),
                    new Choice("9", "9 (best)"),
                    new Choice("7", "7"),
                    new Choice("5", "5 (balanced)"),
                    new Choice("3", "3"),
                    new Choice("0", "0 (smallest)")),
            BackendOption.choice(COLOUR, "Colour depth",
                    "The colour depth of the pixel format asked for. Less colour "
                            + "is fewer bytes over the link whatever the encoding, "
                            + "and visible banding on a photograph. Takes effect on "
                            + "the next connection.",
                    "full", Scope.LAYERED, false,
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
            BackendOption.bool(BELL, "Bell",
                    "The phone buzzes when the remote machine rings its bell.",
                    true, Scope.GLOBAL, false));

    @Override
    public String id() {
        return "libvnc";
    }

    @Override
    public String name() {
        // The project is LibVNCServer and the library is libvncclient; on a
        // phone what matters is that this row and the two beside it are the
        // same protocol with a different implementation behind it.
        return "VNC (LibVNC)";
    }

    @Override
    public String shortName() {
        return "LibVNC";
    }

    @Override
    public String description() {
        return "Tight, which sends photographs as JPEG for fewer bytes over the link, and the "
                + "one that asks least of the phone on a desktop that is only partly moving. "
                + "It takes encryption where a server offers it and cannot be asked to require "
                + "it.";
    }

    /** After our own client, which is the one that will still be here. */
    @Override
    public int order() {
        return 15;
    }

    @Override
    public List<BackendOption> options() {
        return OPTIONS;
    }

    @Override
    public Backend create(Context context, String address, String userName, String password,
                          Map<String, String> options) {
        return new LibVncBackend(context, address, userName, password, options);
    }

    /** A 0–9 option as the native side wants it, or −1 for "do not send it". */
    static int level(String value) {
        try {
            return Integer.parseInt(value);
        } catch (NumberFormatException e) {
            return -1;
        }
    }

    /** The colour option as the shim's level: 0 full, 1 rgb222, 2 rgb111. */
    static int colorLevel(String value) {
        return switch (value == null ? "" : value) {
            case "rgb222" -> 1;
            case "rgb111" -> 2;
            default -> 0;
        };
    }

    /** The encoding option as libvncclient's encodings string. */
    static String encodings(String value) {
        return value == null || value.equals("auto") ? AUTO_ENCODINGS : value + " copyrect";
    }
}
