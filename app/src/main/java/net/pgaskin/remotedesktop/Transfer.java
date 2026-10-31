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
import java.util.stream.Collectors;

/**
 * The two documents this app exports and imports: the connections, and the
 * settings.
 *
 * <p>Two rather than one, because they serve different needs: moving to a new
 * phone needs both, while sharing a lab's machines with a colleague needs only
 * the connections. Each document has its own kind, and an import applies a
 * document of one kind or refuses it, never part of it.
 *
 * <h2>Imported files are untrusted</h2>
 * Everything imported is checked against the same descriptions the editor and
 * the settings screens build their rows from: a backend id against the backends
 * this build has, an option key against that backend's schema, and its value
 * against the option's type and choices. So there is one definition of a valid
 * value, the same one the UI offers.
 *
 * <p>Invalid entries are dropped and counted rather than failing the import: a
 * record for a backend this build lacks (such as one from an add-on that is not
 * installed) is a normal case, and "imported 11 of 13" is more useful than "the
 * file is invalid". Each drop is logged with the reason.
 *
 * <h2>Passwords are exported in plain text</h2>
 * The stored form is encrypted under a key that belongs to this installation
 * ({@link Secrets}), so it cannot be decrypted anywhere else and exporting it
 * would be useless. They are protected outside this class instead: the export
 * asks for the screen lock first, and the screen says what the file contains.
 * That is acceptable for a file the user chooses a destination for, and would
 * not be for anything automatic.
 *
 * <p>Three things are deliberately in neither document. The identity pins,
 * since importing another device's {@code KnownHosts} would import its decision
 * that a key is trusted, and a new phone asking again on first connection is
 * intended. The desktop previews, which are images of the remote screen and are
 * encrypted on this device on purpose. And anything a backend stores for itself
 * (a desktop size, a library's state directory), which is derived from a
 * session and recreated by the next one.
 */
final class Transfer {

    private static final String TAG = "Transfer";

    /** In every document, so a file of another format is recognised as such. */
    private static final String FORMAT = "net.pgaskin.remotedesktop";
    private static final int VERSION = 1;

    static final String KIND_CONNECTIONS = "connections";
    static final String KIND_SETTINGS = "settings";

    /**
     * The limits guard against a wrong or malicious file rather than reflecting
     * what this app writes, which is at most a few dozen connections.
     */
    static final int MAX_BYTES = 1 << 20;

    private static final int MAX_RECORDS = 1000;
    private static final int MAX_FIELD = 256;    // a name, an address, a user name
    private static final int MAX_SECRET = 1024;
    private static final int MAX_VALUE = 1024;   // one option's value

    private Transfer() {
    }

    /**
     * A document, either just built or just opened and validated as this app's
     * format.
     *
     * @param count the number of connections or settings in it, which the import
     *              confirmation shows before anything is applied
     */
    record Document(String kind, JSONObject root, int count) {
        String text() throws JSONException {
            return root.toString(2);
        }
    }

    /** How many entries were applied, out of how many the document held. */
    record Result(int applied, int total) {
    }

    // ---- export -------------------------------------------------------------

    /**
     * @param passwords whether to include the connections' passwords. Without
     *                  them the file can be shared, and the export does not
     *                  ask for the screen lock. Ignored for the settings
     *                  document, which has no secrets.
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
            // Absent rather than empty when left out, so an import does not
            // read it as "this connection has no password" and keeps any
            // password already stored under the same id (see connection()).
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
     * <b>Every</b> setting, including unchanged ones: the file holds the
     * effective values, not only the overrides. A full snapshot means that
     * importing it with the reset option ({@link #applySettings}) reproduces the
     * source phone's settings, whatever either build's defaults are.
     *
     * <p>The cost is that a default changed in a later version does not apply on
     * a phone that imported this file, because the file has a value for every
     * key.
     */
    private static int writeSettings(Context ctx, JSONObject root) throws JSONException {
        final JSONObject app = new JSONObject();
        for (String key : AppSettings.KEYS) {
            // In the type the table gives for the key: a switch as true or
            // false, and the timeout as the string it is stored as.
            app.put(key, AppSettings.value(ctx, key));
        }
        root.put("app", app);

        final JSONObject input = new JSONObject();
        // The defaults with this phone's overrides on top. A Config needs a
        // density, but no tunable is derived from it, so nothing written here
        // depends on this screen.
        final Config config =
                InputSettings.config(ctx, ctx.getResources().getDisplayMetrics().density);
        for (InputSettings.Tunable t : InputSettings.tunables()) {
            input.put(t.key(), t.read().apply(config));
        }
        root.put("input", input);

        final JSONObject backends = new JSONObject();
        for (String id : Backends.ids()) {
            final SharedPreferences bp = Options.backendPrefs(ctx, id);
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

        return app.length() + input.length() + count(nested(backends));
    }

    // ---- import -------------------------------------------------------------

    /**
     * Parse and identify a picked file, before anything is applied.
     *
     * @throws JSONException if it is not a document in this app's format,
     *                       including one from a newer format version
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
     * @param replaceExisting whether to clear the existing data first (delete
     *                        the connections, or reset the settings to their
     *                        defaults). Otherwise either kind is merged.
     */
    static Result apply(Context ctx, Document doc, boolean replaceExisting) {
        return KIND_CONNECTIONS.equals(doc.kind())
                ? applyConnections(ctx, doc.root(), replaceExisting)
                : applySettings(ctx, doc.root(), replaceExisting);
    }

    /**
     * Clear every settings store so each returns its defaults: the app's, the
     * input stack's, and one per backend.
     *
     * <p>Here rather than beside the button that offers it, because it covers
     * the same stores {@link #writeSettings} exports, and keeping both in one
     * place keeps a settings file added later from being handled in only one
     * of them.
     * Connections are not settings and are not touched.
     */
    static void resetSettings(Context ctx) {
        AppSettings.prefs(ctx).edit().clear().apply();
        InputSettings.prefs(ctx).edit().clear().apply();
        for (String id : Backends.ids()) {
            Options.backendPrefs(ctx, id).edit().clear().apply();
        }
    }

    private static Result applyConnections(Context ctx, JSONObject root, boolean deleteExisting) {
        final JSONArray a = root.optJSONArray("connections");
        final int total = a == null ? 0 : Math.min(a.length(), MAX_RECORDS);
        // The existing connections by id, before anything is deleted. An
        // imported record without a password whose id already existed keeps the
        // stored password, so restoring a file exported without passwords does
        // not lose every password on the phone. The encrypted form is copied
        // without decrypting it, since it already belongs to this installation.
        final Map<String, String> known = new LinkedHashMap<>();
        for (Connection c : Connections.all(ctx)) {
            known.put(c.id(), c.sealedPassword());
        }
        if (deleteExisting) {
            // A restore. This also deletes the previews, which belong to the
            // connections being replaced.
            Connections.delete(ctx, Connections.all(ctx).stream().map(Connection::id).collect(Collectors.toList()));
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
     * @param taken ids already in use, so an import never overwrites an
     *              existing record
     * @param known the connections before this import began, with each
     *              encrypted password (see {@link #applyConnections})
     * @return the record, or null if this build cannot use it
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
        // One rule for all four: a field that is too long or contains a control
        // character was not written by this app, and truncating it would not
        // help.
        if (!plain(name, MAX_FIELD) || !plain(address, MAX_FIELD) || !plain(user, MAX_FIELD)
                || !plain(password, MAX_SECRET)) {
            Log.w(TAG, "skipping a connection: a field is not one this app would have written");
            return null;
        }
        if (address.isEmpty()) {
            Log.w(TAG, "skipping a connection with no address");
            return null;
        }
        // Two records with one id overwrite each other. A collision with an
        // existing record (the same file imported twice) creates a second
        // connection rather than replacing the first, which is safer; deleting
        // the existing connections first is the option for replacing them.
        String id = o.optString("id", "");
        if (id.isEmpty() || id.length() > MAX_FIELD || taken.contains(id)) {
            id = UUID.randomUUID().toString();
        }
        final Connection record = new Connection(id, name, backendId, address, user,
                known.getOrDefault(id, ""),
                options(ctx, o.optJSONObject("options"), backendId), o.optBoolean("pinned", false));
        // A file exported without passwords says nothing about them, so the
        // record keeps whatever was stored under its id, which is nothing
        // unless the connection already existed.
        return o.has("password") ? record.withPassword(password) : record;
    }

    /** The connection's own options, limited to the ones this backend defines. */
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
        // The app's own per-connection options are stored in the same map and
        // checked the same way.
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
     * A merge unless {@code reset} is set: a key the file does not mention keeps
     * its current value, as an import from a file written by an older build
     * requires.
     *
     * <p>{@code reset} is the settings counterpart of deleting the existing
     * connections first: every setting returns to its default before the file is
     * applied, so the result is only the file's settings. Since
     * {@link #writeSettings} writes every setting, the two together are a
     * restore.
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
                // The defaults table defines each key's type, so a value of
                // another class is invalid. A key this build does not know has
                // no default, so it is dropped the same way.
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
                final SharedPreferences.Editor e = Options.backendPrefs(ctx, id).edit();
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

    // ---- validation ---------------------------------------------------------

    /** Whether a stored string is one the settings row for this option could produce. */
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
        for (InputSettings.Tunable t : InputSettings.tunables()) {
            if (!t.key().equals(key)) {
                continue;
            }
            if (t.kind() == InputSettings.Kind.BOOL) {
                return "true".equals(v) || "false".equals(v);
            }
            try {
                // Rejected here rather than when used: the input stack ignores
                // an override that does not parse, so the import would appear
                // to succeed while changing nothing. Finite because "NaN"
                // parses, and a NaN threshold compares false against
                // everything.
                return Float.isFinite(Float.parseFloat(v.trim()));
            } catch (NumberFormatException e) {
                return false;
            }
        }
        return false;
    }

    /** Short enough, and no control characters. */
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

    private static int count(JSONObject o) {
        return o == null ? 0 : o.length();
    }

    /** Flatten every backend's settings into one object, for counting the values. */
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
