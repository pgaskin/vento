// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend.spice;

import android.content.Context;

import net.pgaskin.remotedesktop.backend.Backend;
import net.pgaskin.remotedesktop.backend.BackendOption;
import net.pgaskin.remotedesktop.backend.BackendOption.Choice;
import net.pgaskin.remotedesktop.backend.BackendOption.Scope;
import net.pgaskin.remotedesktop.backend.BackendProvider;

import java.util.List;
import java.util.Map;

/**
 * This module's entry in a build's list of backends, and what the SPICE backend
 * can be told.
 *
 * <p>A short list, and the reason is the protocol rather than restraint. There
 * is no security to negotiate — a plain port and a TLS port are different ports,
 * which is why the first row is a fact about the address rather than a
 * preference. There is no colour depth, since a surface is 32-bit and the
 * server compresses it. And there is no resize and no clipboard switch, because
 * both of those live in the guest's own agent rather than in the protocol.
 *
 * <p>What is <b>not</b> here and could have been: which end owns the pointer.
 * SPICE has two mouse modes and lets a client ask for one, but which are on
 * offer is the far end's answer — a guest with a tablet attached offers both
 * and one without offers neither — so it is a fact about a live session and is
 * announced through the seam rather than chosen in a form.
 */
public final class SpiceProvider implements BackendProvider {

    public static final String SECURITY = "Security";
    public static final String COMPRESSION = "Compression";
    public static final String VIEW_ONLY = "ViewOnly";

    /** {@link #SECURITY}'s two values, which are two different ports. */
    public static final String PLAIN = "plain";
    public static final String TLS = "tls";

    static final List<BackendOption> OPTIONS = List.of(
            BackendOption.choice(SECURITY, "Encryption",
                    null, PLAIN, Scope.CONNECTION, false,
                    new Choice(PLAIN, "None",
                            "Plaintext.",
                            null),
                    new Choice(TLS, "TLS",
                            "Encrypted.",
                            null)),
            // Live in the way RFB's encoding is: the message is read on the next
            // image the server encodes. Automatic is what a hypervisor does
            // without being asked, which for QEMU is GLZ where it helps.
            BackendOption.choice(COMPRESSION, "Image compression",
                    "Preferred image compression method.",
                    "", Scope.LAYERED, true,
                    new Choice("", "Automatic"),
                    new Choice("auto-glz", "GLZ, if better"),
                    new Choice("auto-lz", "LZ, if better"),
                    new Choice("quic", "QUIC"),
                    new Choice("glz", "GLZ"),
                    new Choice("lz", "LZ"),
                    new Choice("lz4", "LZ4"),
                    new Choice("off", "None")),
            BackendOption.bool(VIEW_ONLY, "View only",
                    "Do not send input events.",
                    false, Scope.CONNECTION, true));

    @Override
    public String id() {
        return "spice";
    }

    @Override
    public String name() {
        return "SPICE";
    }

    @Override
    public String shortName() {
        return "SPICE";
    }

    /**
     * The sentence that writes itself: the others show you a desktop, and this
     * one shows you a machine.
     */
    @Override
    public String description() {
        return "Usually used for QEMU/libvirt/Proxmox virtual machines. Supports clipboard and screen resizing if the guest agent is running.";
    }

    /** After the RDP clients and before the one that arrives in an add-on. */
    @Override
    public int order() {
        return 35;
    }

    @Override
    public List<BackendOption> options() {
        return OPTIONS;
    }

    @Override
    public Backend create(Context context, String address, String userName, String password,
                          Map<String, String> options) {
        return new SpiceBackend(context, address, userName, password, options);
    }
}
