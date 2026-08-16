// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.plugin;

import android.content.ContentProvider;
import android.content.ContentValues;
import android.content.pm.PackageManager;
import android.database.Cursor;
import android.net.Uri;
import android.os.Binder;
import android.os.Bundle;
import android.os.ParcelFileDescriptor;
import android.os.Process;

import java.io.File;
import java.io.FileNotFoundException;
import java.util.List;

/**
 * The one way anything of the add-on's leaves its uid: a read-only descriptor on
 * a file it has already verified.
 *
 * <p>An add-on subclasses this and declares it with the authority
 * {@code ${applicationId}.libraries}. It is exported, because the app is another
 * uid and there is no other way to reach it, and it answers nobody who is not
 * signed with our key — a permission would have been an ordering problem, since
 * whichever of the two is installed first would be declaring it.
 */
public abstract class LibraryProvider extends ContentProvider {

    @Override
    public boolean onCreate() {
        return true;
    }

    private void requireOurCaller() {
        final int caller = Binder.getCallingUid();
        if (caller != Process.myUid()
                && getContext().getPackageManager().checkSignatures(caller, Process.myUid())
                != PackageManager.SIGNATURE_MATCH) {
            throw new SecurityException("not signed with the same key");
        }
    }

    /** {@code content://<add-on>.libraries/<name>}, read-only, nothing else. */
    @Override
    public ParcelFileDescriptor openFile(Uri uri, String mode) throws FileNotFoundException {
        requireOurCaller();
        if (!"r".equals(mode)) {
            throw new SecurityException("read-only");
        }
        final List<String> path = uri.getPathSegments();
        if (path.size() != 1) {
            throw new FileNotFoundException(uri.toString());
        }
        final String name = path.get(0);
        // A name is a file in one directory and never a path into another.
        if (name.isEmpty() || name.indexOf('/') >= 0 || name.contains("..")) {
            throw new FileNotFoundException(uri.toString());
        }
        final File file = LibraryStore.file(getContext(), name);
        if (!file.isFile()) {
            throw new FileNotFoundException(name);
        }
        return ParcelFileDescriptor.open(file, ParcelFileDescriptor.MODE_READ_ONLY);
    }

    /**
     * Whether this add-on has everything it needs, which is what the app's
     * "needs setting up" comes down to. The caller names what it wants and the
     * hashes it wants them at, so the answer is about the same pin both sides
     * were built with.
     */
    @Override
    public Bundle call(String method, String arg, Bundle extras) {
        requireOurCaller();
        if (!Plugin.CALL_READY.equals(method) || extras == null) {
            return null;
        }
        boolean ready = true;
        for (String name : extras.keySet()) {
            if (!LibraryStore.has(getContext(), name, extras.getString(name))) {
                ready = false;
                break;
            }
        }
        final Bundle answer = new Bundle();
        answer.putBoolean(Plugin.EXTRA_READY, ready);
        return answer;
    }

    // Not a database. Everything below is the part of the interface that has no
    // meaning here, and says so rather than pretending.

    @Override
    public Cursor query(Uri uri, String[] projection, String selection, String[] args,
                        String sortOrder) {
        return null;
    }

    @Override
    public String getType(Uri uri) {
        return "application/octet-stream";
    }

    @Override
    public Uri insert(Uri uri, ContentValues values) {
        throw new UnsupportedOperationException();
    }

    @Override
    public int delete(Uri uri, String selection, String[] args) {
        throw new UnsupportedOperationException();
    }

    @Override
    public int update(Uri uri, ContentValues values, String selection, String[] args) {
        throw new UnsupportedOperationException();
    }
}
