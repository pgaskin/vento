// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.plugin;

import android.util.Log;

import org.json.JSONObject;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.net.HttpURLConnection;
import java.net.URI;
import java.net.URL;

/**
 * Fetching an archive from somebody else's server, which is the one route an
 * add-on has that the app could not have offered.
 *
 * <p>Nothing here decides whether what arrives is any good: that is the pin's
 * job, once the file is on disk. A mirror repacks, so an archive's own hash
 * names a copy rather than a build.
 */
public final class Download {

    private static final String TAG = "PluginDownload";

    /** Some hosts refuse anything that is not a browser. */
    private static final String AGENT = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 "
            + "(KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

    /** How a source's URL turns into the URL of the file itself. */
    public enum Kind {
        /** The URL is the file. */
        DIRECT,
        /**
         * An API that answers with the path to one build. Aptoide's, asked for
         * an exact version code, which is the one mirror here with something to
         * ask rather than a page to scrape.
         */
        APTOIDE,
    }

    public record Source(Kind kind, String url) {
    }

    private Download() {
    }

    /** How far through, for a screen to draw. A negative total is not yet known. */
    public interface Progress {
        void at(long done, long total);
    }

    /**
     * The first source that answers wins.
     *
     * <p>Every failure is reported, not the last: two mirrors fail for two
     * different reasons, and being told about the second when the first is the
     * one that usually works is being told the wrong thing. Each is logged with
     * the host it came from as well, since a message on a screen cannot carry a
     * URL.
     */
    public static void toFile(Iterable<Source> sources, File out, Progress progress)
            throws IOException {
        final StringBuilder failures = new StringBuilder();
        for (Source source : sources) {
            try {
                fetch(resolve(source, progress), out, progress);
                return;
            } catch (IOException e) {
                Log.w(TAG, "fetching from " + source.url(), e);
                final String said = e.getMessage();
                if (failures.length() > 0) {
                    failures.append('\n');
                }
                failures.append(said == null || said.isEmpty() ? String.valueOf(e) : said);
            }
        }
        throw new IOException(failures.length() == 0
                ? "There is nowhere to fetch it from." : failures.toString());
    }

    private static String resolve(Source source, Progress progress) throws IOException {
        if (source.kind() == Kind.DIRECT) {
            return source.url();
        }
        final File meta = File.createTempFile("meta", ".json");
        try {
            fetch(source.url(), meta, progress);
            final String path = new JSONObject(new String(
                    java.nio.file.Files.readAllBytes(meta.toPath()), "UTF-8"))
                    .getJSONObject("data").getJSONObject("file").getString("path");
            if (path.isEmpty()) {
                throw new IOException("The download service did not say where the file is.");
            }
            return path;
        } catch (org.json.JSONException e) {
            throw new IOException("The download service did not answer the way it is documented to.", e);
        } finally {
            //noinspection ResultOfMethodCallIgnored
            meta.delete();
        }
    }

    /**
     * What an archive may be. The viewer's own APK is 11.7 MB, so this is a
     * ceiling nothing real approaches; what it is for is a server that answers
     * a request for one with an endless stream.
     */
    private static final long MAX_BYTES = 256L * 1024 * 1024;

    /**
     * Redirects by hand: HttpURLConnection will not follow one across schemes.
     * Which is also why each hop's scheme is checked — following one by hand is
     * agreeing to whatever it names, and a mirror that redirects to http is a
     * mirror sending somebody's ten megabytes in the clear.
     */
    private static void fetch(String url, File out, Progress progress) throws IOException {
        String location = url;
        for (int hop = 0; hop < 5; hop++) {
            if (!location.regionMatches(true, 0, "https://", 0, 8)) {
                throw new IOException("The download was redirected to an insecure address.");
            }
            final HttpURLConnection conn =
                    (HttpURLConnection) new URL(location).openConnection();
            conn.setRequestProperty("User-Agent", AGENT);
            conn.setInstanceFollowRedirects(false);
            conn.setConnectTimeout(30_000);
            conn.setReadTimeout(120_000);
            try {
                final int code = conn.getResponseCode();
                if (code == 301 || code == 302 || code == 303 || code == 307 || code == 308) {
                    final String next = conn.getHeaderField("Location");
                    if (next == null) {
                        throw new IOException("The download was redirected to nowhere.");
                    }
                    location = URI.create(location).resolve(next).toString();
                    continue;
                }
                if (code != 200) {
                    throw new IOException("The download failed with HTTP " + code + ".");
                }
                // What the server says it is sending, which is a hint for a bar
                // and nothing else: the ceiling below counts what arrives.
                final long expected = conn.getContentLengthLong();
                try (InputStream in = conn.getInputStream();
                     FileOutputStream sink = new FileOutputStream(out)) {
                    final byte[] buffer = new byte[64 * 1024];
                    long got = 0;
                    long said = 0;
                    progress.at(0, expected);
                    for (int n; (n = in.read(buffer)) > 0; ) {
                        got += n;
                        if (got > MAX_BYTES) {
                            throw new IOException("The download is larger than this can accept.");
                        }
                        sink.write(buffer, 0, n);
                        // Every quarter megabyte rather than every buffer: this
                        // hops to the main thread, and a bar cannot show more.
                        if (got - said >= 256 * 1024) {
                            said = got;
                            progress.at(got, expected);
                        }
                    }
                    progress.at(got, expected);
                }
                return;
            } finally {
                conn.disconnect();
            }
        }
        throw new IOException("The download was redirected too many times.");
    }
}
