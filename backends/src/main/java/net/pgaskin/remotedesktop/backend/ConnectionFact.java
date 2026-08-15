// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend;

/**
 * One line of what a backend can say about the connection it is running.
 *
 * <p>This replaces the {@code Map<String, String>} {@link Backend} used to
 * return, which carried display order and nothing else — enough for a dialog
 * that joined it with newlines, and not enough for a screen. A panel has to know which row is the
 * desktop's own name, because that is the heading rather than a row, and which
 * rows are diagnostics, because those are folded away.
 *
 * <p>{@link Field} is the identity and is a fixed set: the panel matches on it,
 * so a backend that calls line speed something else still lands in the right
 * place. {@link #label} is the backend's own wording for the same thing, which
 * is what actually goes on screen — the two are separate because the identity
 * is ours and the wording is theirs.
 *
 * <p>A backend <b>omits</b> a field it has no concept of, and <b>includes one
 * with an empty value</b> when it has the concept but no answer yet. That
 * distinction is the original's — "Not set" rather than a vanishing row: the
 * shape of the panel should depend on what the protocol is, not on how much
 * this particular server has got round to saying.
 *
 * <p>There is no field for the desktop <em>size</em>. It is known locally from
 * {@link Backend#desktopWidth()} the moment the first framebuffer arrives, so
 * asking the protocol for it would be a round trip for something already on the
 * screen.
 *
 * @param diagnostic true for something only worth reading when hunting a
 *                   problem — the pixel formats are the case this exists for
 */
public record ConnectionFact(Field field, String label, String value, boolean diagnostic) {

    /** What a row <em>is</em>, independently of what a backend calls it. */
    public enum Field {
        /** The desktop's own name for itself. The panel's heading. */
        DESKTOP_NAME,
        /** How the bytes get there: direct TCP, relayed, UDP. */
        CONNECTION,
        /** What is protecting them, if anything. */
        SECURITY,
        /** Which version of which protocol is being spoken. */
        PROTOCOL,
        /** The encoding in use for pixels — moves when quality moves. */
        ENCODING,
        /** The measured throughput, however the backend measures it. */
        LINE_SPEED,
        /** What the session has moved, both ways, since it was opened. */
        DATA,
        /** What the server is sending. */
        SERVER_PIXELS,
        /** What we asked it for. */
        VIEWER_PIXELS,
        /** Anything a particular protocol has that this list does not. */
        OTHER
    }

    public ConnectionFact {
        value = value == null ? "" : value.trim();
    }

    public static ConnectionFact of(Field field, String label, String value) {
        return new ConnectionFact(field, label, value, false);
    }

    public static ConnectionFact diagnostic(Field field, String label, String value) {
        return new ConnectionFact(field, label, value, true);
    }

    /**
     * {@link Field#DATA}, from two raw counts. A direction the protocol has no
     * count for is <b>negative</b>, and is left out of the row rather than
     * shown as a zero.
     *
     * <p>The one row whose <em>wording</em> is ours rather than the backend's,
     * which is the rule everywhere else. The reason is the number: six clients
     * count in six places, and a row that means "since this session opened, at
     * the protocol, inside whatever is encrypting it" is only comparable if
     * every backend says it the same way and rounds it the same way. So they
     * hand over bytes and this decides the rest.
     *
     * <p>Not called by anything that has only an estimate. A backend that
     * cannot count accurately omits the field, as it would any other.
     */
    public static ConnectionFact data(long received, long sent) {
        final String value = sent < 0
                ? size(received) + " in"
                : size(received) + " in · " + size(sent) + " out";
        return of(Field.DATA, "Transferred", value);
    }

    /**
     * Bytes as a person reads them: powers of a thousand, and a decimal only
     * where the mantissa is small enough for one to mean anything. SI rather
     * than binary because this is a quantity that crossed a network, which is
     * measured in thousands everywhere else it is quoted.
     */
    private static String size(long bytes) {
        if (bytes < 1000) {
            return bytes + " B";
        }
        final String[] units = {"kB", "MB", "GB", "TB"};
        double value = bytes / 1000.0;
        int unit = 0;
        while (value >= 999.5 && unit < units.length - 1) {
            value /= 1000.0;
            unit++;
        }
        // The phone's own separators, since this is a number somebody reads.
        return String.format(java.util.Locale.getDefault(),
                value < 9.995 ? "%.2f %s" : value < 99.95 ? "%.1f %s" : "%.0f %s",
                value, units[unit]);
    }
}
