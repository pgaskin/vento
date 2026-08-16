// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.app.Activity;
import android.content.ClipData;
import android.content.Context;
import android.content.Intent;
import android.graphics.Bitmap;
import android.net.Uri;
import android.os.Handler;
import android.os.Looper;
import android.util.Log;
import android.widget.Toast;

import androidx.core.content.FileProvider;

import net.pgaskin.remotedesktop.backend.Backend;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.time.LocalDateTime;
import java.time.format.DateTimeFormatter;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

/**
 * The desktop, photographed and handed to the share sheet.
 *
 * <p>What is captured is the <em>desktop</em>, whole and at the size the far
 * end has it — not what is on this phone, which is a viewport onto part of it
 * at some zoom with a cursor drawn on top by this app. A screenshot of a remote
 * machine that came back cropped to a phone's window, with a pointer in it that
 * does not exist over there, would be a picture of the client rather than of the
 * machine.
 *
 * <p>Nothing is needed below the seam for that. {@link Backend#thumbnail} is
 * already "the whole framebuffer, no bigger than", so asking it for the
 * desktop's own size asks for a scale of one — the same call the home card
 * takes its preview through, with the only bound it has set to the size that
 * does not bind.
 *
 * <p>Where it goes is the other half. The file is written to this app's cache
 * and handed over as a read grant on one URI, so a picture of somebody's
 * desktop reaches the app that was chosen and nothing else: not the gallery,
 * not the media scanner, and not another app that went looking. The grant dies
 * with the receiving task, and the file is deleted the next time this runs.
 */
final class Screenshot {

    private static final String TAG = "Screenshot";

    /** Under the cache, and the only thing {@code xml/shared_files} exposes. */
    private static final String DIR = "screenshots";

    /** Sortable, and the same in every locale — this ends up in a file name. */
    private static final DateTimeFormatter STAMP =
            DateTimeFormatter.ofPattern("yyyyMMdd-HHmmss");

    /**
     * One thread, off the main one. A 4K desktop is 33 MB of bitmap and PNG is
     * not a fast encoder; on the main thread that is the whole UI stopped for
     * as long as it takes, on the one screen where a stall reads as the
     * connection having died.
     */
    private static final ExecutorService writer = Executors.newSingleThreadExecutor(r -> {
        final Thread t = new Thread(r, "screenshot-writer");
        t.setDaemon(true);
        return t;
    });

    private static final Handler main = new Handler(Looper.getMainLooper());

    private Screenshot() {
    }

    /**
     * Capture what is connected and open the share sheet on it. Returns at
     * once; the encoding happens elsewhere and the sheet opens when it is done.
     *
     * <p>The capture itself is here rather than on the writer thread on
     * purpose: the seam allows a backend to expect its own thread, and this is
     * the one the rest of the panel already asks on.
     */
    static void share(Activity activity, Session session) {
        final Bitmap shot = capture(session);
        if (shot == null) {
            failed(activity);
            return;
        }
        final String name = fileName(session);
        // The application context on the way to the worker, so an encode that
        // outlives the screen is not also holding it.
        final Context app = activity.getApplicationContext();
        writer.execute(() -> {
            final Uri uri = write(app, shot, name);
            shot.recycle();
            main.post(() -> {
                // The panel is over a session, and a session can be left while
                // its desktop is still being encoded.
                if (activity.isFinishing() || activity.isDestroyed()) {
                    return;
                }
                if (uri == null) {
                    failed(activity);
                } else {
                    send(activity, uri, name);
                }
            });
        });
    }

    /**
     * The framebuffer at 1:1, or null when there is nothing to photograph —
     * a session that is connecting, or one that has gone since the panel
     * opened.
     *
     * <p>Every backend's {@code thumbnail} takes its step from the bound it is
     * given, so the desktop's own size is a step of one in all of them; none of
     * this depends on which protocol is underneath.
     */
    private static Bitmap capture(Session session) {
        if (session.isClosed()) {
            return null;
        }
        final Backend backend = session.backend();
        final int w = backend.desktopWidth();
        final int h = backend.desktopHeight();
        if (w <= 0 || h <= 0) {
            return null;
        }
        try {
            return backend.thumbnail(w, h);
        } catch (OutOfMemoryError e) {
            // A desktop at 1:1 is four bytes a pixel and a phone's heap is not
            // large: 3840x2400 is 35 MB in one allocation, and the alternative
            // to catching it is the session dying to photograph itself. Caught
            // rather than pre-empted by a size limit because what a given phone
            // will give out is not a number this can compute.
            Log.w(TAG, "not enough memory for a " + w + "x" + h + " screenshot", e);
            return null;
        }
    }

    /**
     * Write it, and take the last one with it: a share is a copy handed to
     * somebody else, so what is left here afterwards is a picture of a desktop
     * nobody asked to keep. Clearing on the way in rather than on the way out
     * because the app it went to may not have read it yet.
     */
    private static Uri write(Context ctx, Bitmap shot, String name) {
        final File dir = new File(ctx.getCacheDir(), DIR);
        clear(dir);
        if (!dir.isDirectory() && !dir.mkdirs()) {
            Log.w(TAG, "could not create " + dir);
            return null;
        }
        final File file = new File(dir, name);
        try (FileOutputStream out = new FileOutputStream(file)) {
            // PNG rather than JPEG, and not for the usual reason: a desktop is
            // mostly text, which is exactly what chroma subsampling ruins, and
            // a screenshot somebody is going to read is worth the bytes.
            if (!shot.compress(Bitmap.CompressFormat.PNG, 100, out)) {
                Log.w(TAG, "could not encode the screenshot");
                file.delete();
                return null;
            }
        } catch (IOException e) {
            Log.w(TAG, "writing the screenshot", e);
            file.delete();
            return null;
        }
        try {
            return FileProvider.getUriForFile(ctx, ctx.getPackageName() + ".files", file);
        } catch (IllegalArgumentException e) {
            // The provider and xml/shared_files disagreeing about this
            // directory, which is a build-time mistake rather than a state a
            // phone can get into.
            Log.w(TAG, "the screenshot is not under a shared path", e);
            file.delete();
            return null;
        }
    }

    private static void clear(File dir) {
        final File[] old = dir.listFiles();
        if (old != null) {
            for (File f : old) {
                if (!f.delete()) {
                    Log.w(TAG, "could not delete " + f);
                }
            }
        }
    }

    /**
     * The share sheet, with no title of its own: the system has not shown one
     * since Android 13, and what it does show is the picture — which is why the
     * URI is in the clip data as well as the extra rather than being left to
     * the migration that would otherwise put it there.
     */
    private static void send(Activity activity, Uri uri, String name) {
        final Intent send = new Intent(Intent.ACTION_SEND);
        send.setType("image/png");
        send.putExtra(Intent.EXTRA_STREAM, uri);
        send.setClipData(ClipData.newUri(activity.getContentResolver(), name, uri));
        send.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);
        activity.startActivity(Intent.createChooser(send, null));
    }

    /**
     * What it is called wherever it lands: the machine, and when. The desktop's
     * own name is not used even though the panel has one — it arrives from the
     * far end, and a file name is not the place to find out what a server can
     * put in a string.
     */
    private static String fileName(Session session) {
        final StringBuilder b = new StringBuilder();
        final String title = session.title();
        for (int i = 0; title != null && i < title.length() && b.length() < 40; i++) {
            final char c = title.charAt(i);
            if (c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z' || c >= '0' && c <= '9') {
                b.append(c);
            } else if (b.length() > 0 && b.charAt(b.length() - 1) != '-') {
                b.append('-');
            }
        }
        while (b.length() > 0 && b.charAt(b.length() - 1) == '-') {
            b.setLength(b.length() - 1);
        }
        if (b.length() == 0) {
            b.append("desktop");
        }
        return b + "-" + STAMP.format(LocalDateTime.now()) + ".png";
    }

    private static void failed(Activity activity) {
        Toast.makeText(activity, R.string.screenshot_failed, Toast.LENGTH_SHORT).show();
    }
}
