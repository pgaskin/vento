// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.content.Context;
import android.content.SharedPreferences;

import net.pgaskin.remotedesktop.backend.BackendOption;
import net.pgaskin.remotedesktop.backend.Backends;

import java.util.LinkedHashMap;
import java.util.Map;

/**
 * Where an option's answer comes from.
 *
 * <p>Four layers, the most general first, each able to overrule the one above
 * it:
 *
 * <ol>
 *   <li><b>the schema's default</b> — what the backend does when nobody has
 *       said anything
 *   <li><b>the backend's own settings</b>, in a preference file of its own
 *   <li><b>this connection's</b>, on the record
 *   <li><b>this session's</b>, told to a backend that is already running
 * </ol>
 *
 * <p>Which of the middle two an option may be read from is its
 * {@link BackendOption.Scope}, and that rule is the reason this is one class
 * rather than a ladder written out wherever one is wanted. A
 * {@link BackendOption.Scope#CONNECTION} option is offered in the editor only,
 * so a value for it in a backend's preference file is not that option's answer
 * — it is something somebody set when the option had a different scope, or a
 * key another one used first, and reading it means a connection quietly
 * inheriting a setting no screen ever offered. The same in reverse for
 * {@link BackendOption.Scope#GLOBAL}.
 *
 * <p><b>Only what has been chosen.</b> {@link #forConnection} and
 * {@link #forBackend} leave out every option nobody has answered rather than
 * filling in its default, which looks harmless and is not: a backend then
 * cannot tell "this connection asked for full colour" from "nobody has ever
 * said", and one that wants to move a setting on another's behalf has no way
 * to know whether it may. That cost the RealVNC quality control, which
 * re-wrote ColorLevel at whatever it already was, so applyOptions re-applied
 * the same pixel format and the picture never changed. Backends fill their own
 * defaults in, which is where a default belongs.
 */
public final class Options {

    private Options() {
    }

    /** Where a backend's {@link BackendOption.Scope#GLOBAL} settings live. */
    public static SharedPreferences backendPrefs(Context ctx, String backendId) {
        return ctx.getApplicationContext()
                .getSharedPreferences("backend_" + backendId, Context.MODE_PRIVATE);
    }

    /**
     * One option's answer, from the most specific layer that has one.
     *
     * @param own the connection's own answers, or null where there is no
     *            connection — a session opened from the command line
     * @return what somebody chose, or null where nobody has
     */
    private static String chosen(BackendOption o, SharedPreferences global,
                                 Map<String, String> own) {
        if (own != null && o.scope() != BackendOption.Scope.GLOBAL) {
            final String v = own.get(o.key());
            if (v != null && !v.isEmpty()) {
                return v;
            }
        }
        if (o.scope() != BackendOption.Scope.CONNECTION) {
            final String v = global.getString(o.key(), null);
            if (v != null && !v.isEmpty()) {
                return v;
            }
        }
        return null;
    }

    /** What to hand {@link Backends#create} for a saved connection. */
    public static Map<String, String> forConnection(Context ctx, Connection conn) {
        return chosen(ctx, conn.backendId(), conn.options());
    }

    /**
     * ... and for a session with no record behind it: everything set in the
     * backend's own settings screen, and nothing else.
     */
    public static Map<String, String> forBackend(Context ctx, String backendId) {
        return chosen(ctx, backendId, null);
    }

    private static Map<String, String> chosen(Context ctx, String backendId,
                                              Map<String, String> own) {
        final Map<String, String> out = new LinkedHashMap<>();
        final SharedPreferences global = backendPrefs(ctx, backendId);
        for (BackendOption o : Backends.options(backendId)) {
            final String v = chosen(o, global, own);
            if (v != null) {
                out.put(o.key(), v);
            }
        }
        return out;
    }

    /**
     * What every one of a backend's options would be for a connection that
     * overrode none of them — the ladder one rung short, which is what lets the
     * editor show an inherited answer and mean it.
     *
     * <p>Every key, with the schema's default where no layer has anything:
     * unlike the two above, the caller is showing an answer rather than
     * handing one over, and "nothing" is not something a row can display.
     */
    public static Map<String, String> inherited(Context ctx, String backendId) {
        final Map<String, String> out = new LinkedHashMap<>();
        final SharedPreferences global = backendPrefs(ctx, backendId);
        for (BackendOption o : Backends.options(backendId)) {
            final String v = chosen(o, global, null);
            out.put(o.key(), v != null ? v : o.defaultValue());
        }
        return out;
    }

    /**
     * The fourth rung, for a session that is already running: what it has been
     * told since it came up, else what it was created with — which is
     * {@link #forConnection}'s answer, computed once when the backend was made
     * — else the schema's default.
     *
     * @param told what {@link Session#liveOption} says, or null for nothing
     */
    public static String live(String told, Map<String, String> connectedWith, BackendOption o) {
        if (told != null) {
            return told;
        }
        final String connected = connectedWith.get(o.key());
        return connected != null ? connected : o.defaultValue();
    }
}
