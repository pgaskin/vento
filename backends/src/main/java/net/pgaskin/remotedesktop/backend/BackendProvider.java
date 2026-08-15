// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend;

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

    Backend create(Context context, String address, String userName, String password,
                   Map<String, String> options);
}
