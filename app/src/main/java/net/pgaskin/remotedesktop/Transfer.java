// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.content.Context;
import android.content.SharedPreferences;
import android.util.Log;

import net.pgaskin.remotedesktop.backend.BackendOption;
import net.pgaskin.remotedesktop.backend.Backends;
import net.pgaskin.remotedesktop.control.input.Config;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.util.ArrayList;
import java.util.Iterator;
import java.util.LinkedHashMap;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.UUID;

/**
 * The two documents this app writes and reads back: the connections, and the
 * settings.
 *
 * <p>Two rather than one, because a list of machines and a set of preferences
 * are different things to want — moving to a new phone wants both, handing a
 * lab's machines to a colleague wants only the first. Each carries its own kind,
 * and an import applies a document of one kind or refuses it, never half of it.
 *
 * <h2>A file that arrives is not one this app wrote</h2>
 * Everything on the way in is checked against the descriptions the editor and
 * the settings screens build their own rows from: a backend id against what this
 * build has, an option key against that backend's schema, and its value against
 * the option's type and choices. So there is one answer to "is this a legal
 * value" rather than two, and it is the one a person would have been offered.
 *
 * <p>What does not check out is dropped and counted rather than fatal: a record
 * naming a backend this build lacks is the ordinary case rather than the
 * exceptional one — the two flavours differ by a backend — and "imported 11 of
 * 13" is a useful sentence where "the file is invalid" is not. Each drop is a
 * line in the log, which is the only place the reason exists.
 *
 * <h2>The passwords go out in the clear</h2>
 * The stored form is sealed under a key that belongs to this installation
 * ({@link Secrets}), so it is worthless anywhere else and an export carrying it
 * would be an export of nothing. What protects them instead is outside this
 * class: the lock screen in front of the export, and a sentence saying what the
 * file contains. That is a fair trade for a file somebody chooses a destination
 * for, and would not be one for anything automatic.
 *
 * <p>Three things are deliberately in neither document. The identity pins, since
 * importing somebody's {@code KnownHosts} is importing their decision that a key
 * was the right one, and a new phone asking again on first connection is the
 * system working. The desktop previews, which are photographs of whatever was on
 * a machine's screen and are sealed on this device on purpose. And anything a
 * backend remembers for itself — a desktop size, a library's state directory —
 * which is derived from a session and comes back with the next one.
 */
final class Transfer {

    private static final String TAG = "Transfer";

    /** In every document, so a file picked by mistake can say what it is. */
    private static final String FORMAT = "net.pgaskin.remotedesktop";
    private static final int VERSION = 1;

    static final String KIND_CONNECTIONS = "connections";
    static final String KIND_SETTINGS = "settings";

    /**
     * The bounds are about what a mistake costs rather than about what this app
     * writes: the file is one somebody picked, and the largest thing here is a
     * few dozen connections.
     */
    static final int MAX_BYTES = 1 << 20;

    private static final int MAX_RECORDS = 1000;
    private static final int MAX_FIELD = 256;    // a name, an address, a user name
    private static final int MAX_SECRET = 1024;
    private static final int MAX_VALUE = 1024;   // one option's value

    private Transfer() {
    }

    /**
     * A document, either just built or just opened and found to be one of ours.
     *
     * @param count how much is in it — connections, or settings — which is what
     *              the import asks about before anything is applied
     */
    record Document(String kind, JSONObject root, int count) {
        String text() throws JSONException {
            return root.toString(2);
        }
    }

    /** How many were written or applied, out of how many the document held. */
    record Result(int applied, int total) {
    }

    // ---- out ----------------------------------------------------------------

    /**
     * @param passwords whether the connections carry theirs. Off is what makes
     *                  this file shareable — the machines without the keys to
     *                  them — and it is also the only version of it that does
     *                  not have to be asked for behind the lock screen. Nothing
     *                  for the settings document, which has no secrets in it.
     */
    static Document write(Context ctx, String kind, boolean passwords) throws JSONException {
        final JSONObject root = new JSONObject();
        root.put("format", FORMAT);
        root.put("kind", kind);
        root.put("version", VERSION);
        final int count = KIND_CONNECTIONS.equals(kind)
                ? writeConnections(ctx, root, passwords)
                : writeSettings(ctx, root);
        return new Document(kind, root, count);
    }

    private static int writeConnections(Context ctx, JSONObject root, boolean passwords)
            throws JSONException {
        final JSONArray out = new JSONArray();
        for (Connection c : Connections.all(ctx)) {
            final JSONObject o = new JSONObject();
            o.put("id", c.id());
            o.put("name", c.name());
            o.put("backend", c.backendId());
            o.put("address", c.address());
            o.put("user", c.userName());
            // Absent rather than empty when they are left out, so importing the
            // file cannot be read as "this connection has no password" — the
            // record it lands on is a new one either way, but the file says what
            // it knows rather than what it was told not to say.
            if (passwords) {
                o.put("password", c.password());
            }
            o.put("options", new JSONObject(c.options()));
            o.put("pinned", c.pinned());
            out.put(o);
        }
        root.put("connections", out);
        return out.length();
    }

    /**
     * <b>Every</b> setting, including the ones nobody has ever touched: what
     * goes in the file is the answer this phone gives, not the subset it has
     * been told. A snapshot is what makes the file worth having — reset the
     * settings and import it and the phone is the one the file came off,
     * whatever either build's defaults happen to be — and it is what the reset
     * on the way in is for ({@link #applySettings}).
     *
     * <p>What that costs is the other reading: a default that moves in a later
     * version no longer reaches a phone this file lands on, because the file now
     * has an answer of its own for every key.
     */
    private static int writeSettings(Context ctx, JSONObject root) throws JSONException {
        final JSONObject app = new JSONObject();
        for (String key : AppSettings.KEYS) {
            // Whatever type the table says this key is: a switch goes out as
            // true or false and the timeout as the text it is stored as.
            app.put(key, AppSettings.value(ctx, key));
        }
        root.put("app", app);

        final JSONObject input = new JSONObject();
        final SharedPreferences ip = InputSettings.prefs(ctx);
        input.put(InputSettings.KEY_PRESET,
                ip.getString(InputSettings.KEY_PRESET, InputSettings.PRESET_IMPROVED));
        // The preset with this phone's overrides on top, which is what a tunable
        // nobody has touched answers. A Config cannot be built without a
        // density and no tunable is derived from one, so nothing written here is
        // a fact about this screen rather than a choice about the feel.
        final Config config =
                InputSettings.config(ctx, ctx.getResources().getDisplayMetrics().density);
        for (InputSettings.Tunable t : InputSettings.tunables()) {
            input.put(t.key(), t.read().apply(config));
        }
        root.put("input", input);

        final JSONObject backends = new JSONObject();
        for (String id : Backends.ids()) {
            final SharedPreferences bp = Connections.backendPrefs(ctx, id);
            final JSONObject one = new JSONObject();
            for (BackendOption o : Backends.options(id)) {
                if (o.scope() == BackendOption.Scope.CONNECTION) {
                    continue;
                }
                final String v = bp.getString(o.key(), null);
                one.put(o.key(), v == null || v.isEmpty() ? o.defaultValue() : v);
            }
            if (one.length() > 0) {
                backends.put(id, one);
            }
        }
        root.put("backends", backends);

        return app.length() + input.length() + count(backends);
    }

    // ---- in -----------------------------------------------------------------

    /**
     * What a picked file turns out to be, before anything is applied.
     *
     * @throws JSONException if it is not a document of ours, which includes one
     *                       from a version that knows things this build does not
     */
    static Document open(String text) throws JSONException {
        final JSONObject root = new JSONObject(text);
        if (!FORMAT.equals(root.optString("format"))) {
            throw new JSONException("not a file this app wrote");
        }
        final int version = root.optInt("version");
        if (version <= 0 || version > VERSION) {
            throw new JSONException("version " + version + " is not one this build reads");
        }
        final String kind = root.optString("kind");
        return switch (kind) {
            case KIND_CONNECTIONS -> {
                final JSONArray a = root.optJSONArray("connections");
                yield new Document(kind, root, a == null ? 0 : a.length());
            }
            case KIND_SETTINGS -> new Document(kind, root,
                    count(root.optJSONObject("app")) + count(root.optJSONObject("input"))
                            + count(nested(root.optJSONObject("backends"))));
            default -> throw new JSONException("unknown kind: " + kind);
        };
    }

    /**
     * @param replaceExisting whether what is here goes first — the connections
     *                        deleted, or the settings put back to their
     *                        defaults. Without it either kind is a merge.
     */
    static Result apply(Context ctx, Document doc, boolean replaceExisting) {
        return KIND_CONNECTIONS.equals(doc.kind())
                ? applyConnections(ctx, doc.root(), replaceExisting)
                : applySettings(ctx, doc.root(), replaceExisting);
    }

    /**
     * Every settings store this app has, emptied, so each answers with its own
     * defaults again: the app's, the input stack's, and one per backend.
     *
     * <p>Here rather than beside the button that offers it, because it is the
     * same enumeration {@link #writeSettings} walks — a settings file added
     * later and reset in only one of the two places is what this prevents.
     * Connections are not settings and are not touched.
     */
    static void resetSettings(Context ctx) {
        AppSettings.prefs(ctx).edit().clear().apply();
        InputSettings.prefs(ctx).edit().clear().apply();
        for (String id : Backends.ids()) {
            Connections.backendPrefs(ctx, id).edit().clear().apply();
        }
    }

    private static Result applyConnections(Context ctx, JSONObject root, boolean deleteExisting) {
        final JSONArray a = root.optJSONArray("connections");
        final int total = a == null ? 0 : Math.min(a.length(), MAX_RECORDS);
        // What is here before anything is deleted, by id. A record that carries
        // no password of its own and lands on an id this phone already knew
        // keeps the password already stored under it — which is the difference
        // between restoring a file exported without passwords and losing every
        // password on the phone to it. The sealed form is copied rather than
        // opened: it is already this installation's, and nothing here has to see
        // the plain text to move it.
        final Map<String, String> known = new LinkedHashMap<>();
        for (Connection c : Connections.all(ctx)) {
            known.put(c.id(), c.sealedPassword());
        }
        if (deleteExisting) {
            // Which is what "restore this backup" means, and takes the previews
            // with it — they belong to the connections being replaced.
            Connections.delete(ctx, Connections.all(ctx).stream().map(Connection::id).toList());
        }
        final Set<String> taken = new LinkedHashSet<>();
        for (Connection c : Connections.all(ctx)) {
            taken.add(c.id());
        }
        final List<Connection> added = new ArrayList<>();
        for (int i = 0; i < total; i++) {
            final JSONObject o = a.optJSONObject(i);
            final Connection c = o == null ? null : connection(ctx, o, taken, known);
            if (c != null) {
                taken.add(c.id());
                added.add(c);
            }
        }
        Connections.addAll(ctx, added);
        return new Result(added.size(), total);
    }

    /**
     * @param taken ids already spoken for, so no import overwrites a record it
     *              was not meant to
     * @param known what was here before this import began, with each sealed
     *              password — see {@link #applyConnections}
     * @return the record, or null if this is not one this build can use
     */
    private static Connection connection(Context ctx, JSONObject o, Set<String> taken,
                                         Map<String, String> known) {
        final String backendId = o.optString("backend");
        if (!Backends.ids().contains(backendId)) {
            Log.w(TAG, "skipping a connection: no backend " + backendId + " in this build");
            return null;
        }
        final String name = o.optString("name", "");
        final String address = o.optString("address", "");
        final String user = o.optString("user", "");
        final String password = o.optString("password", "");
        // One rule for all four: a field longer than a person types, or with a
        // control character in it, is not one this app wrote, and there is
        // nothing sensible to do with half of it.
        if (!plain(name, MAX_FIELD) || !plain(address, MAX_FIELD) || !plain(user, MAX_FIELD)
                || !plain(password, MAX_SECRET)) {
            Log.w(TAG, "skipping a connection: a field is not one this app would have written");
            return null;
        }
        if (address.isEmpty()) {
            Log.w(TAG, "skipping a connection with no address");
            return null;
        }
        // An id says which record this is, and two records with one id are two
        // records that overwrite each other. A collision with what is already
        // here — the same file imported twice — makes a second connection
        // rather than replacing the first, which is the safer of the two
        // readings and the one "delete the existing connections first" exists
        // for the other of.
        String id = o.optString("id", "");
        if (id.isEmpty() || id.length() > MAX_FIELD || taken.contains(id)) {
            id = UUID.randomUUID().toString();
        }
        final Connection record = new Connection(id, name, backendId, address, user,
                known.getOrDefault(id, ""),
                options(ctx, o.optJSONObject("options"), backendId), o.optBoolean("pinned", false));
        // A file exported without them says nothing about passwords rather than
        // saying there are none, so what a record starts with is whatever was
        // stored under its id — nothing at all, unless this is that connection
        // coming back.
        return o.has("password") ? record.withPassword(password) : record;
    }

    /** The connection's own options: what this backend has, and nothing else. */
    private static Map<String, String> options(Context ctx, JSONObject j, String backendId) {
        final Map<String, String> out = new LinkedHashMap<>();
        if (j == null) {
            return out;
        }
        final Map<String, BackendOption> schema = new LinkedHashMap<>();
        for (BackendOption o : Backends.options(backendId)) {
            if (o.scope() != BackendOption.Scope.GLOBAL) {
                schema.put(o.key(), o);
            }
        }
        // The app's own per-connection options ride in the same map and are
        // checked from the same table, which is how they came to be there.
        for (BackendOption o : AppOptions.options(ctx)) {
            schema.put(o.key(), o);
        }
        for (Iterator<String> it = j.keys(); it.hasNext(); ) {
            final String key = it.next();
            final BackendOption o = schema.get(key);
            final String v = j.optString(key, "");
            if (o == null) {
                Log.w(TAG, "dropping option " + key + ": not one " + backendId + " has");
            } else if (!valid(o, v)) {
                Log.w(TAG, "dropping option " + key + ": " + v + " is not a value it takes");
            } else {
                out.put(key, v);
            }
        }
        return out;
    }

    /**
     * A merge unless it is asked not to be: a key the file does not mention
     * keeps whatever this phone answers for it, which is what an import from a
     * file written by an older build has to mean.
     *
     * <p>{@code reset} is the other reading, and the settings' half of "delete
     * the connections already here first": every setting back to its default
     * before anything is read, so what the phone ends up with is the file and
     * nothing that happened to be here before it. Since {@link #writeSettings}
     * writes every setting, that pair is a restore.
     */
    private static Result applySettings(Context ctx, JSONObject root, boolean reset) {
        if (reset) {
            resetSettings(ctx);
        }
        int applied = 0;
        int total = 0;

        final JSONObject app = root.optJSONObject("app");
        if (app != null) {
            final SharedPreferences.Editor e = AppSettings.prefs(ctx).edit();
            for (Iterator<String> it = app.keys(); it.hasNext(); ) {
                final String key = it.next();
                final Object def = AppSettings.DEFAULTS.get(key);
                final Object v = app.opt(key);
                total++;
                // The table is the only thing that says what a key's answer
                // looks like, so a value of another class is not an answer to
                // this setting — which is also how a key this build does not
                // know is dropped rather than stored.
                if (def == null || v == null || v.getClass() != def.getClass()) {
                    Log.w(TAG, "dropping app setting " + key);
                    continue;
                }
                if (v instanceof Boolean b) {
                    e.putBoolean(key, b);
                } else {
                    e.putString(key, (String) v);
                }
                applied++;
            }
            e.apply();
        }

        final JSONObject input = root.optJSONObject("input");
        if (input != null) {
            final SharedPreferences.Editor e = InputSettings.prefs(ctx).edit();
            for (Iterator<String> it = input.keys(); it.hasNext(); ) {
                final String key = it.next();
                final String v = input.optString(key, "");
                total++;
                if (!validInput(key, v)) {
                    Log.w(TAG, "dropping input setting " + key + " = " + v);
                    continue;
                }
                e.putString(key, v);
                applied++;
            }
            e.apply();
        }

        final JSONObject backends = root.optJSONObject("backends");
        if (backends != null) {
            for (Iterator<String> bt = backends.keys(); bt.hasNext(); ) {
                final String id = bt.next();
                final JSONObject one = backends.optJSONObject(id);
                if (one == null) {
                    continue;
                }
                if (!Backends.ids().contains(id)) {
                    total += one.length();
                    Log.w(TAG, "dropping the settings of " + id + ": not in this build");
                    continue;
                }
                final Map<String, BackendOption> schema = new LinkedHashMap<>();
                for (BackendOption o : Backends.options(id)) {
                    if (o.scope() != BackendOption.Scope.CONNECTION) {
                        schema.put(o.key(), o);
                    }
                }
                final SharedPreferences.Editor e = Connections.backendPrefs(ctx, id).edit();
                for (Iterator<String> it = one.keys(); it.hasNext(); ) {
                    final String key = it.next();
                    final String v = one.optString(key, "");
                    final BackendOption o = schema.get(key);
                    total++;
                    if (o == null || !valid(o, v)) {
                        Log.w(TAG, "dropping " + id + " setting " + key + " = " + v);
                        continue;
                    }
                    e.putString(key, v);
                    applied++;
                }
                e.apply();
            }
        }

        return new Result(applied, total);
    }

    // ---- what a value may be ------------------------------------------------

    /** Whether a stored string is one the row for this option could produce. */
    private static boolean valid(BackendOption o, String v) {
        if (!plain(v, MAX_VALUE)) {
            return false;
        }
        return switch (o.type()) {
            case BOOL -> "true".equals(v) || "false".equals(v);
            case CHOICE -> o.choices().stream().anyMatch(c -> c.value().equals(v));
            case INT -> {
                try {
                    Integer.parseInt(v.trim());
                    yield true;
                } catch (NumberFormatException e) {
                    yield false;
                }
            }
            case TEXT -> true;
        };
    }

    private static boolean validInput(String key, String v) {
        if (!plain(v, MAX_VALUE)) {
            return false;
        }
        if (InputSettings.KEY_PRESET.equals(key)) {
            return InputSettings.PRESET_IMPROVED.equals(v)
                    || InputSettings.PRESET_FAITHFUL.equals(v);
        }
        for (InputSettings.Tunable t : InputSettings.tunables()) {
            if (!t.key().equals(key)) {
                continue;
            }
            if (t.kind() == InputSettings.Kind.BOOL) {
                return "true".equals(v) || "false".equals(v);
            }
            try {
                // Dropped here rather than where it lands: an override that will
                // not parse is silently ignored by the stack, so a screen full
                // of preset values would be showing an import that did nothing.
                // Finite because "NaN" parses, and a NaN threshold compares
                // false against everything it is asked about.
                return Float.isFinite(Float.parseFloat(v.trim()));
            } catch (NumberFormatException e) {
                return false;
            }
        }
        return false;
    }

    /** Short enough, and no control characters: a line a person could have typed. */
    private static boolean plain(String v, int max) {
        if (v == null || v.length() > max) {
            return false;
        }
        for (int i = 0; i < v.length(); i++) {
            if (v.charAt(i) < 0x20 || v.charAt(i) == 0x7f) {
                return false;
            }
        }
        return true;
    }

    // ---- plumbing -----------------------------------------------------------

    private static List<String> inputKeys() {
        final List<String> keys = new ArrayList<>();
        keys.add(InputSettings.KEY_PRESET);
        for (InputSettings.Tunable t : InputSettings.tunables()) {
            keys.add(t.key());
        }
        return keys;
    }

    private static int count(JSONObject o) {
        return o == null ? 0 : o.length();
    }

    /** One object holding every backend's, so the count is of the values in it. */
    private static JSONObject nested(JSONObject backends) throws JSONException {
        if (backends == null) {
            return null;
        }
        final JSONObject flat = new JSONObject();
        for (Iterator<String> it = backends.keys(); it.hasNext(); ) {
            final String id = it.next();
            final JSONObject one = backends.optJSONObject(id);
            if (one == null) {
                continue;
            }
            for (Iterator<String> kt = one.keys(); kt.hasNext(); ) {
                final String key = kt.next();
                flat.put(id + "." + key, one.opt(key));
            }
        }
        return flat;
    }
}
