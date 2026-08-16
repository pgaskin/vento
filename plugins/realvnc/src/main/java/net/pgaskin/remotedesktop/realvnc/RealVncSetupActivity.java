// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.realvnc;

import com.realvnc.vncviewer.jni.Library;

import net.pgaskin.remotedesktop.plugin.Download;
import net.pgaskin.remotedesktop.plugin.SetupActivity;

import java.util.List;
import java.util.Map;

/**
 * The one screen this add-on has: where RealVNC's library comes from, and the
 * three ways to get it.
 */
public final class RealVncSetupActivity extends SetupActivity {

    @Override
    protected CharSequence explanation() {
        return getString(R.string.setup_explanation, Library.VERSION);
    }

    @Override
    protected List<Download.Source> downloads() {
        return Viewer.sources();
    }

    @Override
    protected Map<String, String> wanted() {
        return Library.wanted();
    }

    @Override
    protected String abi() {
        return Library.abi();
    }
}
