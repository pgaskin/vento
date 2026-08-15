// SPDX-License-Identifier: CC0-1.0

package com.realvnc.vncviewer.jni;

/**
 * Sync of connection records to a RealVNC account, which this client does not
 * support: it keeps its own connections and never had theirs.
 *
 * <p>Only the callback is here, and only because {@link Bindings#initViewer}
 * passes one — the core resolves
 * {@code com/realvnc/vncviewer/jni/SyncMgrBindings$Callback} by name. Every
 * native on this class is about the connection store, so none is declared.
 */
final class SyncMgrBindings {

    interface Callback {
        void serverEntriesChanged();
    }

    private SyncMgrBindings() {
    }
}
