// SPDX-License-Identifier: CC0-1.0

package com.realvnc.vncviewer.jni;

import java.util.List;

/**
 * Browsing a team's directory of machines, which is a cloud account feature and
 * is not supported here.
 *
 * <p>Present because {@link Bindings#initViewer} passes a {@link Callback} and
 * the core resolves
 * {@code com/realvnc/vncviewer/jni/DirectoryBrowserBindings$Callback} by name.
 * {@code directoryChanged} is {@code (Ljava/util/List;[Ljava/lang/String;)V};
 * what is in the list is a connection record, which this module does not bind,
 * hence the wildcard.
 */
final class DirectoryBrowserBindings {

    interface Callback {

        void directoryChanged(List<?> entries, String[] labels);

        void directoryEntryGone(String id);

        void directorySearchCoveredChanged(boolean covered);
    }

    private DirectoryBrowserBindings() {
    }
}
