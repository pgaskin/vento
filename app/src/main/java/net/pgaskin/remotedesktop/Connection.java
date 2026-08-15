// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import org.json.JSONException;
import org.json.JSONObject;

import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.Iterator;
import java.util.Map;
import java.util.UUID;

/**
 * One saved connection: what to connect to, with what, and how.
 *
 * <p>Deliberately not RealVNC's {@code ServerRec} — that drags in native lists,
 * a finalizer idiom and an account sync we do not use, and the one thing it
 * was needed for is a thumbnail. This is a record and a JSON object.
 *
 * <p>{@link #options} holds only the {@link net.pgaskin.remotedesktop.backend.BackendOption.Scope#CONNECTION}
 * settings, and only the ones that have been changed from the backend's
 * default — an absent key means "whatever the backend says", so a default that
 * changes in a later version reaches connections saved before it.
 *
 * @param pinned   kept at the top of the home screen. A fact about the list
 *                 rather than about connecting, and it lives here for the same
 *                 reason the name does: it is what somebody decided about this
 *                 connection.
 * @param sealedPassword the password as it is stored: encrypted under a
 *                 keystore key, or empty for none — see {@link Secrets}, which
 *                 is also where the answer to "what happens to a restored
 *                 backup" is written down. Kept sealed rather than revealed
 *                 because this record is read whenever the list is, and only
 *                 two callers want the plain text; a record that carried it
 *                 would decrypt every password to draw the home screen, and
 *                 re-encrypt every password to save any one of them — which on
 *                 a phone whose keystore is momentarily unavailable is not a
 *                 failure to read, it is the whole file overwritten with
 *                 nothing. The other fields are not secrets and are stored as
 *                 they are.
 */
public record Connection(String id, String name, String backendId, String address,
                         String userName, String sealedPassword, Map<String, String> options,
                         boolean pinned) {

    public Connection {
        options = Collections.unmodifiableMap(new LinkedHashMap<>(options));
        sealedPassword = sealedPassword == null ? "" : sealedPassword;
    }

    public static Connection blank(String backendId) {
        return new Connection(UUID.randomUUID().toString(), "", backendId, "", "", "",
                Map.of(), false);
    }

    /**
     * The password in the clear, decrypted here and now. Empty for "none
     * stored", which is also what an unopenable one means — see {@link Secrets}.
     */
    public String password() {
        return Secrets.reveal(sealedPassword);
    }

    /**
     * The same connection with a new password, sealed on the way in. A password
     * that cannot be encrypted is not stored at all rather than stored in the
     * clear, which is why this can return a record with none.
     */
    public Connection withPassword(String plaintext) {
        final String sealed = Secrets.protect(plaintext);
        return new Connection(id, name, backendId, address, userName,
                sealed == null ? "" : sealed, options, pinned);
    }

    /** What to put on the card: the name if there is one, else the address. */
    public String title() {
        return name == null || name.isEmpty() ? address : name;
    }

    /** ... and under it, the address, unless that is already the title. */
    public String subtitle() {
        return name == null || name.isEmpty() ? "" : address;
    }

    public Connection withOptions(Map<String, String> newOptions) {
        return new Connection(id, name, backendId, address, userName, sealedPassword,
                newOptions, pinned);
    }

    public Connection withPinned(boolean nowPinned) {
        return new Connection(id, name, backendId, address, userName, sealedPassword, options,
                nowPinned);
    }

    JSONObject toJson() throws JSONException {
        final JSONObject o = new JSONObject();
        o.put("id", id);
        o.put("name", name);
        o.put("backend", backendId);
        o.put("address", address);
        o.put("user", userName);
        // Already sealed, and written as it is: encrypting here would mean
        // every save re-encrypting every record, and a keystore that refuses
        // once would take the whole file's passwords with it.
        o.put("password", sealedPassword);
        final JSONObject opts = new JSONObject();
        for (Map.Entry<String, String> e : options.entrySet()) {
            opts.put(e.getKey(), e.getValue());
        }
        o.put("options", opts);
        if (pinned) {
            o.put("pinned", true);
        }
        return o;
    }

    static Connection fromJson(JSONObject o) {
        final Map<String, String> opts = new LinkedHashMap<>();
        final JSONObject j = o.optJSONObject("options");
        if (j != null) {
            for (Iterator<String> it = j.keys(); it.hasNext(); ) {
                final String k = it.next();
                opts.put(k, j.optString(k));
            }
        }
        return new Connection(
                o.optString("id", UUID.randomUUID().toString()),
                o.optString("name", ""),
                o.optString("backend", ""),
                o.optString("address", ""),
                o.optString("user", ""),
                o.optString("password", ""),
                opts,
                o.optBoolean("pinned", false));
    }
}
