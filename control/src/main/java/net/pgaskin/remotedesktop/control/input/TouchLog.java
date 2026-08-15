// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.input;

import java.io.BufferedReader;
import java.io.IOException;
import java.io.Reader;
import java.io.StringReader;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * A recorded stream of {@link TouchFrame}s, as text.
 *
 * <p>Recorded on a device (the {@code RECORD} toggle writes one file per
 * gesture), replayed into the input stack in a JVM unit test. That is the only
 * way to regression-test the multi-finger gestures: {@code adb shell input}
 * cannot inject more than one pointer, and the tap/drag thresholds are exactly
 * the kind of thing that is easy to break silently.
 *
 * <pre>
 *   # format remotedesktop-touch-1
 *   # label two-finger-tap
 *   # density 2.625
 *   0 D 0 | 0 540.0 300.0
 *   16 M | 0 541.0 300.5
 *   20 D 1 | 0 541.0 300.5 | 1 700.0 320.0
 *   150 U 1 | 0 541.0 300.5 | 1 700.0 320.0
 *   160 U 0 | 0 541.0 300.5
 * </pre>
 *
 * Every {@code # key value} line is metadata, {@code format} included; the rest
 * are frames. Times are ms relative to the first frame. A {@code D}/{@code U}
 * line carries the index (into that line's pointer list) of the pointer that
 * went down or up; {@code M} and {@code C} do not.
 */
public final class TouchLog {

    /** Value of the {@code format} header. Bump it if the grammar changes. */
    public static final String FORMAT = "remotedesktop-touch-1";

    private TouchLog() {
    }

    /** A parsed log: its headers, and its frames in order. */
    public record Log(Map<String, String> meta, List<TouchFrame> frames) {

        Log() {
            this(new LinkedHashMap<>(), new ArrayList<>());
        }

        public String meta(String key, String def) {
            final String v = meta.get(key);
            return v != null ? v : def;
        }

        public float metaFloat(String key, float def) {
            final String v = meta.get(key);
            try {
                return v != null ? Float.parseFloat(v) : def;
            } catch (NumberFormatException e) {
                return def;
            }
        }
    }

    /** Builds the text form; {@link #add} rebases times onto the first frame. */
    public static final class Writer {
        private final StringBuilder head = new StringBuilder();
        private final StringBuilder body = new StringBuilder();
        private long t0 = Long.MIN_VALUE;
        private int frames;

        public Writer() {
            meta("format", FORMAT);
        }

        public Writer meta(String key, String value) {
            head.append("# ").append(key).append(' ').append(value).append('\n');
            return this;
        }

        public Writer add(TouchFrame f) {
            if (t0 == Long.MIN_VALUE) {
                t0 = f.time;
            }
            body.append(f.time - t0).append(' ').append(f.action.code());
            if (f.action == TouchFrame.Action.DOWN || f.action == TouchFrame.Action.UP) {
                body.append(' ').append(f.index);
            }
            for (int i = 0; i < f.count; i++) {
                body.append(" | ").append(f.id[i])
                        .append(' ').append(f.x[i])
                        .append(' ').append(f.y[i]);
            }
            body.append('\n');
            frames++;
            return this;
        }

        public int frameCount() {
            return frames;
        }

        public String text() {
            return head.toString() + body;
        }
    }

    public static Log parse(String text) {
        try {
            return parse(new StringReader(text));
        } catch (IOException e) {
            throw new IllegalStateException(e); // StringReader does not throw
        }
    }

    public static Log parse(Reader in) throws IOException {
        final Log log = new Log();
        final BufferedReader r = in instanceof BufferedReader
                ? (BufferedReader) in : new BufferedReader(in);
        String line;
        int lineNo = 0;
        while ((line = r.readLine()) != null) {
            lineNo++;
            line = line.trim();
            if (line.isEmpty()) {
                continue;
            }
            if (line.charAt(0) == '#') {
                final String[] kv = line.substring(1).trim().split("\\s+", 2);
                if (kv.length == 2) {
                    log.meta().put(kv[0], kv[1]);
                }
                continue;
            }
            try {
                log.frames().add(parseFrame(line));
            } catch (RuntimeException e) {
                throw new IllegalArgumentException("line " + lineNo + ": " + line, e);
            }
        }
        return log;
    }

    private static TouchFrame parseFrame(String line) {
        final String[] parts = line.split("\\|");
        final String[] head = parts[0].trim().split("\\s+");
        final long time = Long.parseLong(head[0]);
        final TouchFrame.Action action = TouchFrame.Action.fromCode(head[1]);
        final int index = head.length > 2 ? Integer.parseInt(head[2]) : 0;

        final TouchFrame f = new TouchFrame().set(action, index, time);
        for (int i = 1; i < parts.length; i++) {
            final String[] p = parts[i].trim().split("\\s+");
            f.add(Integer.parseInt(p[0]), Float.parseFloat(p[1]), Float.parseFloat(p[2]));
        }
        return f;
    }
}
