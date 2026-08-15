// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend;

import java.util.List;

/**
 * One setting a backend understands, described well enough for the app to
 * <em>offer</em> it without knowing what it means.
 *
 * <p>A backend is built with an opaque {@code Map<String, String>}, which is
 * enough to pass options and useless for building a screen out of them. The
 * alternative to this is a hard-coded list of RealVNC's parameters in the
 * settings tree, which is exactly the coupling the flavour split exists to
 * prevent.
 *
 * <p>The values are strings, because that is what the backends' own
 * configuration layers take — RealVNC's parameter registry parses
 * {@code "true"}/{@code "30"}/{@code "ZRLE2"} out of strings itself.
 * {@link Type} says how to ask for one, not how to store it.
 *
 * @param key          the backend's own name for it; the map key
 * @param label        one line, for a preference title
 * @param summary      the longer explanation, or null
 * @param type         which control to offer
 * @param choices      for {@link Type#CHOICE}, in display order; empty otherwise
 * @param defaultValue what the backend does when nothing is set
 * @param scope        whose setting it is: the connection's or the backend's
 * @param live         whether {@link Backend#setOption} does anything mid-session
 * @param gate         what another option must say for this one to be offered at
 *                     all, or null
 */
public record BackendOption(String key, String label, String summary, Type type,
                            List<Choice> choices, String defaultValue, Scope scope,
                            boolean live, Gate gate) {

    public enum Type {
        /** {@code "true"} / {@code "false"}. */
        BOOL,
        /** One of {@link #choices}. */
        CHOICE,
        /** A decimal integer. */
        INT,
        /** Free text. */
        TEXT
    }

    public enum Scope {
        /**
         * Belongs to one connection: offered in the editor, stored on the
         * connection record. View-only and the desktop size are the shape of
         * this — they are about one machine, and two servers on the same phone
         * want different answers.
         */
        CONNECTION,
        /**
         * Belongs to the backend: offered in its settings screen, stored in its
         * own preference file, and applied to every connection that uses it. Its
         * own preference file, so a backend's settings live and die with the
         * backend.
         */
        GLOBAL,
        /**
         * The connection's, with the backend's answer as its default: offered in
         * both screens, and read from the connection where it has one and from
         * the backend where it has not. The encoding and how much of the picture
         * may be thrown away are the shape of this — mostly a fact about the
         * link this phone is on rather than about one machine, but with one
         * machine always able to disagree.
         *
         * <p>An editor stores an override only where the answer differs from
         * what the connection would do anyway, so moving the backend's answer
         * moves every connection that agreed with it and leaves the rest.
         */
        LAYERED
    }

    /**
     * A value and what to call it on screen.
     *
     * @param summary  why you would pick this one over the others, or null. A
     *                 value whose label is the whole of what there is to say has
     *                 none, which is nearly all of them; where one is given, the
     *                 choice is offered as a list of described items rather than
     *                 as a menu of labels, since a menu has no room for a line
     *                 of prose. Only the connection editor draws these.
     * @param requires the key of a {@code BOOL} option this choice depends on,
     *                 or null. A choice whose dependency is off is not offered
     *                 at all, which is for the case where a backend can do
     *                 something it should not do unasked — an encoding that is
     *                 lossy, say — and offering it as a preference while it is
     *                 switched off would be offering something that cannot
     *                 happen.
     */
    public record Choice(String value, String label, String summary, String requires) {
        public Choice(String value, String label) {
            this(value, label, null, null);
        }
    }

    public static BackendOption bool(String key, String label, String summary,
                                     boolean defaultValue, Scope scope, boolean live) {
        return new BackendOption(key, label, summary, Type.BOOL, List.of(),
                Boolean.toString(defaultValue), scope, live, null);
    }

    public static BackendOption integer(String key, String label, String summary,
                                        int defaultValue, Scope scope, boolean live) {
        return new BackendOption(key, label, summary, Type.INT, List.of(),
                Integer.toString(defaultValue), scope, live, null);
    }

    public static BackendOption text(String key, String label, String summary,
                                     String defaultValue, Scope scope, boolean live) {
        return new BackendOption(key, label, summary, Type.TEXT, List.of(),
                defaultValue, scope, live, null);
    }

    public static BackendOption choice(String key, String label, String summary,
                                       String defaultValue, Scope scope, boolean live,
                                       Choice... choices) {
        return new BackendOption(key, label, summary, Type.CHOICE, List.of(choices),
                defaultValue, scope, live, null);
    }

    /**
     * The same option, offered only where another one says one of
     * {@code values}.
     *
     * <p>For a setting a backend <em>has</em> and does not always consult: not a
     * capability, which is a fact about a live session and belongs on
     * {@link Backend}, and not a choice that depends on a switch, which is
     * {@link Choice#requires}. The case it is for is a mode dial that owns
     * several settings until it is set to the value that hands them back — where
     * leaving the row on screen means offering a control that provably does
     * nothing, and greying it out means saying "not now" where the honest answer
     * is "not here".
     *
     * <p>The gate is read from the same values the rows are drawn from, so a
     * screen where the controlling option is missing offers nothing that depends
     * on it.
     */
    public BackendOption when(String key, String... values) {
        return new BackendOption(this.key, label, summary, type, choices, defaultValue,
                scope, live, new Gate(key, List.of(values)));
    }

    /**
     * @param key    another option's key
     * @param values the values of it that make this option worth offering
     */
    public record Gate(String key, List<String> values) {
        public boolean open(String value) {
            return value != null && values.contains(value);
        }
    }

    /** For a {@link Type#CHOICE}: what to show for {@code value}, or the value itself. */
    public String labelFor(String value) {
        for (Choice c : choices) {
            if (c.value().equals(value)) {
                return c.label();
            }
        }
        return value;
    }
}
