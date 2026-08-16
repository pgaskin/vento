// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend.realvnc;

import android.app.Activity;
import android.content.Context;
import android.content.Intent;

import net.pgaskin.remotedesktop.backend.Backend;
import net.pgaskin.remotedesktop.backend.BackendOption;
import net.pgaskin.remotedesktop.backend.BackendOption.Choice;
import net.pgaskin.remotedesktop.backend.BackendProvider;
import net.pgaskin.remotedesktop.backend.BackendOption.Scope;
import net.pgaskin.remotedesktop.plugin.Plugin;

import java.util.List;
import java.util.Map;

/**
 * This module's entry in a build's list of backends, and which of RealVNC's
 * parameters this app offers.
 *
 * <p>Every name here is one of two things: a parameter recovered from the
 * binary's own registry, or one the original itself sets on
 * {@code createSession}. The registry holds a great many more — the whole
 * underscore-prefixed {@code _Hosted*} / {@code _Udp*} / {@code _Opus*}
 * transport surface — and none is offered until there is a reason to, because
 * an option nobody can describe is worse than no option.
 *
 * <p>{@link Scope#CONNECTION} versus {@link Scope#GLOBAL} is a question about
 * the setting, not about the protocol: does one server want a different answer
 * from another? Quality does, encryption policy does not.
 */
public final class RealVncProvider implements BackendProvider {

    /**
     * The colour depth, offered where the core reads it and nowhere else.
     *
     * <p><b>Low and Custom are the qualities that consult it.</b> The picture
     * policy resolves a quality to one of four classes and picks the pixel
     * format from that: High and Medium ask for full colour whatever this says,
     * Low and Custom use it, and Automatic is whichever class the measured line
     * speed puts it in. Their own published reference says the parameter is
     * ignored unless the quality is Custom, and the library disagrees with the
     * documentation — the Low case is real, and was read out of the picture
     * policy and then driven: at High a connection with {@code rgb111} asks for
     * 24-bit colour, at Low and at Custom it asks for 3-bit.
     *
     * <p>The default is full colour where the original forces {@code rgb222}
     * at every quality but Custom. That difference is smaller than it looked
     * when it was made: at High and Medium the core asks for full colour
     * whatever either of us says, so what it actually buys is a Low quality
     * that keeps its colour, and an Automatic one that keeps it on a slow line.
     */
    private static final BackendOption COLOR_LEVEL = BackendOption.choice(
            "ColorLevel", "Colour depth",
            "Pixel color depth. Only used when quality is set to Low or Custom.",
            "full", Scope.LAYERED, true,
            new Choice("full", "Full colour (24-bit)"),
            new Choice("rgb222", "64 colours (rgb222)"),
            new Choice("rgb111", "8 colours (rgb111)")).when("Quality", "Low", "Custom");

    /**
     * What Low means here, when nobody has said otherwise: the one quality this
     * app gives a colour depth of its own.
     *
     * <p>Only Low, because only Low and Custom reach the parameter at all — a
     * Medium that asked for {@code rgb222} was asking for something the core
     * throws away — and not Custom, because Custom is the value that hands the
     * choice back to whoever set it. A connection that has chosen a depth
     * explicitly keeps it (see {@code RealVncBackend}).
     */
    static String colorLevelFor(String quality) {
        return "Low".equals(quality) ? "rgb111" : "full";
    }

    /**
     * The one row every other RFB backend has and this one did not. All eight
     * values of the enum the core registers, in the order it registers them,
     * rather than a guess or a subset — including the two that are lossy, since
     * the quality dial that would otherwise be how lossiness is asked for is by
     * definition out of the way here.
     *
     * <p><b>Only a Custom quality reaches it</b>, which is why the row is not
     * offered at any other. The picture policy builds its own list of encodings
     * for each of the three fixed classes and consults this parameter only for
     * the fourth, and that is the library rather than an interface decision: it
     * was read out of the class switch and driven both ways — Automatic, High
     * and Low all sent ZRLE with Hextile asked for, and Custom sent Hextile.
     */
    private static final BackendOption ENCODING = BackendOption.choice(
            "PreferredEncoding", "Encoding",
            "Preferred image encoding. The server may still choose to use a different one. Only used when quality is set to Custom",
            "ZRLE2", Scope.LAYERED, true,
            new Choice("JPEG", "JPEG (lossy)"),
            new Choice("Zlib", "Zlib"),
            new Choice("ZRLE", "ZRLE"),
            new Choice("ZRLE2", "ZRLE2"),
            new Choice("TRLE", "TRLE"),
            new Choice("Hextile", "Hextile"),
            new Choice("Raw", "Raw (uncompressed)"),
            new Choice("JRLE", "JRLE (lossy)")).when("Quality", "Custom");

    private static final List<BackendOption> OPTIONS = List.of(
            // ---- per connection ---------------------------------------------
            // Auto, not the core's own default of High: Auto follows the
            // measured line speed, and pinning High on every connection is
            // deciding for a network nobody has seen yet.
            BackendOption.choice("Quality", "Picture quality",
                    "Encoding and color depth preset.",
                    "Auto", Scope.LAYERED, true,
                    new Choice("Auto", "Automatic"),
                    new Choice("High", "High"),
                    new Choice("Medium", "Medium"),
                    new Choice("Low", "Low"),
                    new Choice("Custom", "Custom")),
            ENCODING,
            COLOR_LEVEL,
            BackendOption.bool("ViewOnly", "View only",
                    "Do not send input or clipboard events.",
                    false, Scope.CONNECTION, true),
            BackendOption.bool("Shared", "Shared",
                    "Do not ask the server to disconnect other clients.",
                    true, Scope.CONNECTION, false),
            BackendOption.bool("AutoReconnect", "Reconnect automatically",
                    "Do not ask before re-establishing dropped connections.",
                    true, Scope.CONNECTION, true),
            BackendOption.bool("EnableUdpRfb", "Enable UDP",
                    "Enable RealVNC's proprietary SCTP/RTP-based UDP protocol, falling back to TCP if the server doesn't respond within 500ms.",
                    true, Scope.CONNECTION, false),

            // ---- per backend -------------------------------------------------
            BackendOption.choice("Encryption", "Encryption",
                    "Whether to encrypt connections.",
                    "PreferOn", Scope.GLOBAL, false,
                    new Choice("AlwaysOn", "Required"),
                    new Choice("PreferOn", "If supported by the server"),
                    new Choice("PreferOff", "Only if required by the server"),
                    new Choice("Server", "Let the server decide")),
            BackendOption.bool("WarnUnencrypted", "Warn when unencrypted",
                    "Show a warning when connecting to a server which doesn't support encryption.",
                    true, Scope.GLOBAL, false),
            BackendOption.bool("DotWhenNoCursor", "Dot for an invisible cursor",
                    "Show a dot when the server hides the cursor.",
                    true, Scope.GLOBAL, false),
            BackendOption.bool("AcceptBell", "Bell",
                    "Vibrate when the remote servers rings the bell.",
                    true, Scope.GLOBAL, false),
            // The three clipboard parameters this core has are deliberately not
            // rows. Whether a phone's clipboard goes to a remote machine and
            // comes back is a question about the phone, asked once in the app's
            // own settings and enforced there — the core's copies would be a
            // second answer to it, in a second screen, one layer down. Left at
            // their defaults, which is on, so the app's answer is the only one.
            BackendOption.integer("KeepAliveInterval", "Keep-alive interval",
                    "Seconds between keep-alives on an idle connection.",
                    30, Scope.GLOBAL, false),
            BackendOption.integer("KeepAliveResponseTimeout", "Keep-alive timeout",
                    "Seconds to wait for an answer before giving up.",
                    30, Scope.GLOBAL, false));

    /**
     * The quality group, and the only thing known to need {@code applyOptions}
     * after {@code setOption}. The original sets {@code ViewOnly} and
     * {@code AutoReconnect} alone and calls {@code applyOptions} only after
     * quality and colour depth.
     */
    static boolean needsApply(String key) {
        return "Quality".equals(key) || "ColorLevel".equals(key)
                || "PreferredEncoding".equals(key);
    }

    @Override
    public String id() {
        return "realvnc";
    }

    @Override
    public String name() {
        return "VNC (RealVNC)";
    }

    /**
     * Two of the three backends speak RFB, so a badge has to name the
     * implementation as well as the protocol.
     */
    @Override
    public String shortName() {
        return "RealVNC";
    }

    @Override
    public String description() {
        return "RealVNC's proprietary client implementation. This is the only one which supports UDP, ZRLE2, and RealVNC's proprietary authentication/encryption. It performs significantly better than the others when connecting to a RealVNC server, especially for typical desktop usage.";
    }

    /**
     * Last: still the more capable client, and the one with a ceiling on its
     * life — 4.9.3's {@code libvncviewer.so} is 16 KB aligned, but it is
     * somebody else's binary and cannot be rebuilt when the next page size
     * arrives.
     */
    @Override
    public int order() {
        return 30;
    }

    @Override
    public List<BackendOption> options() {
        return OPTIONS;
    }

    @Override
    public Backend create(Context context, String address, String userName, String password,
                          Map<String, String> options) {
        return new RealVncBackend(context, address, userName, password, options);
    }

    /**
     * The one backend that is not ready the moment it is installed: its library
     * is RealVNC's, is in no build, and arrives on the device.
     */
    @Override
    public boolean isSetup(Context context) {
        return RealVncLibrary.isSetup(context);
    }

    @Override
    public void setup(Activity host) {
        host.startActivity(new Intent(Plugin.ACTION_SETUP)
                .setPackage(RealVncLibrary.pluginPackage()));
    }

    /** Every option at its default, as the map {@code createSession} takes. */
    static Map<String, String> defaults() {
        final java.util.Map<String, String> m = new java.util.LinkedHashMap<>();
        for (BackendOption o : OPTIONS) {
            m.put(o.key(), o.defaultValue());
        }
        return m;
    }
}
