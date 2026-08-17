// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend.rustdesk;

import android.content.Context;

import net.pgaskin.remotedesktop.backend.Backend;
import net.pgaskin.remotedesktop.backend.BackendOption;
import net.pgaskin.remotedesktop.backend.BackendOption.Choice;
import net.pgaskin.remotedesktop.backend.BackendOption.Scope;
import net.pgaskin.remotedesktop.backend.BackendProvider;

import java.util.List;
import java.util.Map;

/**
 * This module's entry in a build's list of backends, and what the RustDesk
 * backend can be told.
 *
 * <p>A short list, and the reason is the protocol rather than restraint: no
 * security to choose, since each way to a machine has exactly one answer to
 * that, and no colour depth, since the picture is a video stream. The codec is
 * the nearest thing to an encoding here and behaves nothing like one — it is a
 * preference between whole video codecs, answered by whichever of them that
 * machine has an encoder for.
 *
 * <p>The first three rows are how a machine is reached, and they are here
 * rather than beside the address because only one protocol here has more than
 * one answer to it. The server and its key are {@code LAYERED}: somebody
 * running their own rendezvous server has every machine on it, so the answer
 * belongs to the backend with a connection able to disagree.
 *
 * <p>What is <b>not</b> here and could have been: the peer's other displays.
 * This protocol captures one at a time and switches with a message, so a
 * chooser is a row whose contents depend on the connection that is running —
 * a live fact rather than a static description, and it is in the connection
 * panel with the desktop size, which is there for the same reason.
 */
public final class RustDeskProvider implements BackendProvider {

    public static final String CONNECT_BY = "ConnectBy";
    public static final String RENDEZVOUS = "RendezvousServer";
    public static final String RENDEZVOUS_KEY = "RendezvousKey";
    public static final String QUALITY = "Quality";
    public static final String FPS = "FrameRate";
    public static final String CODEC = "Codec";
    public static final String LOCK_AFTER = "LockAfterSession";
    public static final String VIEW_ONLY = "ViewOnly";

    /** {@link #CONNECT_BY}'s two values, which decide what the address means. */
    public static final String BY_ID = "id";
    public static final String BY_ADDRESS = "address";

    /**
     * The public rendezvous network, which is what an empty
     * {@link #RENDEZVOUS} means. Named here so that a pin keyed on it is keyed
     * on something rather than on the absence of an answer.
     */
    public static final String PUBLIC_NETWORK = "rs-ny.rustdesk.com";

    static final List<BackendOption> OPTIONS = List.of(
            // The first row of the connection, because it says what the box
            // above it holds and because the two are not one transport with a
            // switch on it: one reaches a machine somebody can already address
            // and encrypts nothing, and the other reaches a machine nothing on
            // this network can address and is encrypted because of how it got
            // there.
            BackendOption.choice(CONNECT_BY, "Connect by",
                    null, BY_ID, Scope.CONNECTION, false,
                    new Choice(BY_ID, "ID",
                            "Ask a rendezvous server for the machine with that ID. Encrypted.",
                            null),
                    new Choice(BY_ADDRESS, "Address",
                            "Dial the machine directly, which it has to have turned on. Not encrypted.",
                            null)),
            BackendOption.text(RENDEZVOUS, "Rendezvous server",
                    "Where to ask for the machine. Leave empty for the public network.",
                    "", Scope.LAYERED, false).when(CONNECT_BY, BY_ID),
            BackendOption.text(RENDEZVOUS_KEY, "Rendezvous server key",
                    "The server's public key, which is what the machine's own key is checked against.",
                    "", Scope.LAYERED, false).when(CONNECT_BY, BY_ID),
            // Both of these are live in the way RFB's encoding is: their option
            // message is read on the next frame the peer encodes.
            BackendOption.choice(QUALITY, "Quality",
                    "Image quality, which the remote end turns into a bitrate.",
                    "balanced", Scope.LAYERED, true,
                    new Choice("low", "Low"),
                    new Choice("balanced", "Balanced"),
                    new Choice("best", "Best")),
            BackendOption.choice(FPS, "Frame rate",
                    "Frames per second, up to what the remote end can capture.",
                    "auto", Scope.LAYERED, true,
                    new Choice("auto", "Automatic"),
                    new Choice("10", "10"),
                    new Choice("20", "20"),
                    new Choice("30", "30"),
                    new Choice("60", "60")),
            // Live like the two above it, and the one that is not free: the far
            // end answers a codec it is not already using by building an
            // encoder and starting again with a key frame.
            BackendOption.choice(CODEC, "Codec",
                    "Which codec to ask the remote end for. It sends what it can encode.",
                    "auto", Scope.LAYERED, true,
                    new Choice("auto", "Automatic"),
                    new Choice("vp9", "VP9"),
                    new Choice("vp8", "VP8"),
                    new Choice("av1", "AV1"),
                    new Choice("h264", "H.264"),
                    new Choice("h265", "H.265")),
            BackendOption.bool(LOCK_AFTER, "Lock when the session ends",
                    "Lock the remote screen on disconnecting.",
                    false, Scope.LAYERED, false),
            BackendOption.bool(VIEW_ONLY, "View only",
                    "Do not send input or clipboard events.",
                    false, Scope.CONNECTION, true));

    @Override
    public String id() {
        return "rustdesk";
    }

    @Override
    public String name() {
        return "RustDesk";
    }

    @Override
    public String shortName() {
        return "RustDesk";
    }

    @Override
    public String description() {
        return "RustDesk implementation, reaching a machine by ID through a rendezvous server, or by address where the remote end has direct access turned on. The picture is video rather than screen updates.";
    }

    /**
     * What the address field holds by id: nine digits somebody reads off the
     * other machine. Dialled by address it is an address, which is what the app
     * calls it anyway.
     */
    @Override
    public String addressLabel(Map<String, String> options) {
        return BY_ID.equals(options.getOrDefault(CONNECT_BY, BY_ID)) ? "ID" : null;
    }

    /** Last: a third protocol, and the only one that arrives in an add-on. */
    @Override
    public int order() {
        return 40;
    }

    @Override
    public List<BackendOption> options() {
        return OPTIONS;
    }

    @Override
    public Backend create(Context context, String address, String userName, String password,
                          Map<String, String> options) {
        return new RustDeskBackend(context, address, userName, password, options);
    }

    /** The frame rate option as the native side wants it: 0 for their choice. */
    static int fps(String value) {
        try {
            return Integer.parseInt(value);
        } catch (NumberFormatException e) {
            return 0;
        }
    }
}
