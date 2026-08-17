// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend;

import android.app.Activity;
import android.content.Context;

import java.util.List;
import java.util.Map;

/**
 * A protocol module's statement that it is in this build.
 *
 * <p>One per protocol, declared as a {@code java.util.ServiceLoader} service in
 * the module that implements it, so what a build can connect with is exactly
 * what is on its classpath: adding a protocol is a dependency line, and
 * dropping one is deleting that line. Nothing above this interface names a
 * protocol.
 *
 * <p>It carries the description as well as the constructor because everything
 * here is a fact about the protocol rather than about the app — what the thing
 * is called, what it can be told — and the module that speaks it is the one
 * that knows.
 */
public interface BackendProvider {

    /** Stored in a connection record, so it must not change once shipped. */
    String id();

    /** For a picker and a title. */
    String name();

    /**
     * A word or two, for a badge with no room for more. Two backends may speak
     * the same protocol, so this sometimes has to name the implementation
     * instead.
     */
    String shortName();

    /**
     * Why somebody would choose this one, in a sentence or two, under
     * {@link #name} in a picker.
     *
     * <p>A choice rather than a description: what it does that the others in the
     * build do not, and what that costs. It is here rather than in the app
     * because a build's list of backends is whatever is on its classpath, so a
     * list of sentences about protocols the app is not allowed to name could
     * only ever be a stale copy of it.
     */
    String description();

    /** Where in a picker, lowest first; a build must not reshuffle its own list. */
    int order();

    /** What this backend can be told, for the editor and its settings screen. */
    List<BackendOption> options();

    /**
     * A session for one connection, not yet started.
     *
     * <p>The address is what somebody typed, and every backend reads it the same
     * way: {@code host}, {@code host:port}, {@code [literal]} or
     * {@code [literal]:port}, where a host is a name or an IPv4 literal and an
     * <b>IPv6 literal is always bracketed</b>. The brackets are not a courtesy
     * to the parser. The VNC backends read a port under 100 as a display number,
     * which is what {@code :1} has meant since the 1990s, so {@code ::1:1} would
     * have to be an address and a display at once; a bare literal is an error
     * with a sentence in it rather than a guess, because the guesses on offer
     * land on a different machine or on the wildcard address. What a port means
     * when there is one is the protocol's own business, and only the VNC ones
     * have the display rule.
     *
     * <p>A backend whose {@link #addressLabel} says this field is not a host —
     * an id a rendezvous server resolves — is outside all of that, and says so
     * where it parses.
     */
    Backend create(Context context, String address, String userName, String password,
                   Map<String, String> options);

    /**
     * What the editor's address field is called for this backend, or null for
     * the app's own wording.
     *
     * <p>It takes the connection's options because for one protocol here the
     * answer moves with them: a peer reached through a rendezvous server is
     * named by digits rather than by a host and a port, and a field labelled
     * for the other mode would be asking for the wrong thing. The app rebuilds
     * the form when the answer changes.
     */
    default String addressLabel(Map<String, String> options) {
        return null;
    }

    /**
     * Whether this backend can be connected with yet.
     *
     * <p>False only for one that needs something the build cannot contain — a
     * library it has to be given on the device. It appears everywhere it would
     * otherwise appear while this is false, because being chosen is one of the
     * things that leads to {@link #setup}.
     */
    default boolean isSetup(Context context) {
        return true;
    }

    /**
     * Ask for whatever {@link #isSetup} is waiting for, and return.
     *
     * <p>There is no callback: what this starts may be a screen in another
     * process which can be killed behind a dialog, so the only answer that
     * survives is the caller asking {@link #isSetup} again when it resumes.
     */
    default void setup(Activity host) {
    }

    /**
     * Forget every server identity this backend has accepted.
     *
     * <p>Empty for all but the two that need it. {@link KnownHosts} is the
     * store, and {@link Backends#forgetHosts} clears it for everybody; this is
     * for a backend whose library pins the far end <em>itself</em> — a
     * known-hosts file of its own, or a blob only it can compare — which nothing
     * above this interface can find, since it is not a fact about the app.
     */
    default void forgetHosts(Context context) {
    }
}
