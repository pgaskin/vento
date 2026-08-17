// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend.rfb;

import android.content.Context;

import net.pgaskin.remotedesktop.backend.Backend;
import net.pgaskin.remotedesktop.backend.BackendOption;
import net.pgaskin.remotedesktop.backend.BackendOption.Choice;
import net.pgaskin.remotedesktop.backend.BackendProvider;
import net.pgaskin.remotedesktop.backend.BackendOption.Scope;

import java.util.List;
import java.util.Map;

/**
 * This module's entry in a build's list of backends, and what the RFB backend
 * can be told.
 *
 * <p>Deliberately shorter than {@code RealVncProvider}. That list is long because
 * it is a selection from somebody else's parameter registry, most of which
 * exists for their own transports and identity system; this one is the whole of
 * what the protocol has to offer, and an option nobody can describe is worse
 * than no option (the same rule, from the other end).
 *
 * <p>Two of these are <b>live in a way RealVNC's are not</b>, and that is the
 * protocol rather than a nicety: {@code SetEncodings} may be sent at any point
 * in a session, so changing the encoding or the compression acts on the picture
 * in front of you. RealVNC's equivalent needs a <em>changed</em> quality and
 * carries the group with it.
 */
public final class RfbProvider implements BackendProvider {

    public static final String SECURITY = "Security";
    public static final String ENCODING = "Encoding";
    public static final String COMPRESSION = "Compression";
    public static final String VIEW_ONLY = "ViewOnly";
    public static final String SHARED = "Shared";
    public static final String BELL = "AcceptBell";

    static final List<BackendOption> OPTIONS = List.of(
            // ---- per connection ---------------------------------------------
            BackendOption.choice(SECURITY, "Encryption",
                    "TLS (VeNCrypt) extension support.",
                    "prefer", Scope.CONNECTION, false,
                    new Choice("prefer", "Where the server offers it"),
                    new Choice("require", "Required"),
                    new Choice("plain", "Never")),
            // The two below are the connection's with the backend's as their
            // default, since which encoding and how hard to compress is mostly a
            // fact about the link this phone is on. There is no colour row: this
            // client works in one pixel format on purpose, which is what makes a
            // region read a row copy, and a reduced one would put a conversion
            // back in the path for every decoder it has.
            BackendOption.choice(ENCODING, "Encoding",
                    "Preferred image encoding, overriding the server's selection if not Automatic.",
                    "auto", Scope.LAYERED, true,
                    new Choice("auto", "Automatic"),
                    new Choice("zrle", "ZRLE"),
                    new Choice("hextile", "Hextile"),
                    new Choice("rre", "RRE"),
                    new Choice("raw", "Raw (uncompressed)")),
            BackendOption.choice(COMPRESSION, "Compression",
                    "Image compression level, where supported by the encoding.",
                    "auto", Scope.LAYERED, true,
                    new Choice("auto", "Automatic"),
                    new Choice("1", "1 (fastest)"),
                    new Choice("3", "3"),
                    new Choice("6", "6 (balanced)"),
                    new Choice("9", "9 (smallest)")),
            BackendOption.bool(VIEW_ONLY, "View only",
                    "Do not send input or clipboard events.",
                    false, Scope.CONNECTION, true),
            BackendOption.bool(SHARED, "Shared",
                    "Do not ask the server to disconnect other clients.",
                    true, Scope.CONNECTION, false),

            // ---- per backend -------------------------------------------------
            BackendOption.bool(BELL, "Bell",
                    "Vibrate when the remote servers rings the bell.",
                    true, Scope.GLOBAL, false));

    @Override
    public String id() {
        return "rfb";
    }

    @Override
    public String name() {
        // Named for what it is rather than for being the default one:
        // there is more than one VNC client here, and which implementation a
        // connection is on is the only thing these names have to tell apart.
        return "VNC (Rust)";
    }

    @Override
    public String shortName() {
        return "Rust";
    }

    @Override
    public String description() {
        return "Rust RFB implementation. Does not support Tight encoding, and typically uses more bandwith and performs worse on slow networks. Requires a certificate to use encryption. Allows remote pointer position updates, where supported by the server.";
    }

    /** First in a picker: it is ours, and it is the one that will still be here. */
    @Override
    public int order() {
        return 10;
    }

    @Override
    public List<BackendOption> options() {
        return OPTIONS;
    }

    @Override
    public Backend create(Context context, String address, String userName, String password,
                          Map<String, String> options) {
        return new RfbBackend(context, address, userName, password, options);
    }

    /** The compression option as the native side wants it: 0–9, or −1 for none. */
    static int compressLevel(String value) {
        try {
            return Integer.parseInt(value);
        } catch (NumberFormatException e) {
            return -1;
        }
    }
}
