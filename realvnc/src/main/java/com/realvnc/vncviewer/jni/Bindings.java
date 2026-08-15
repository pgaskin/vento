// SPDX-License-Identifier: CC0-1.0

package com.realvnc.vncviewer.jni;

import android.content.Context;

import java.util.List;
import java.util.Map;

/**
 * Process-wide bootstrap: bring the core up, then bring the viewer layer up.
 * Both are once-only, both must have returned before any session exists.
 *
 * <p>Required by {@code Java_com_realvnc_vncviewer_jni_Bindings_initApp} and
 * {@code …_initVwr}.
 */
public final class Bindings {

    /** Where the core writes its log. The values travel to the native side as an ordinal. */
    public enum LogMode {
        LOG_FILE,
        LOG_DEBUG,
        LOG_DEBUG_APPONLY
    }

    private Bindings() {
    }

    /**
     * Guarded by a once-flag natively, so a second call is a no-op.
     *
     * @param appName    parameter namespace, and the name of the data directory
     *                   created under {@code dataRoot}
     * @param version    version string
     * @param dataRoot   {@code dataRoot + "/" + appName} becomes the config
     *                   layer's data directory
     * @param logDir     directory for the log file
     * @param logName    log file base name; {@code logName + ".log"}
     * @param context    retained natively for platform services
     * @param parameters global parameters, applied at init
     */
    public static native void initApp(String appName, String version, String dataRoot,
                                      String logDir, String logName, Context context,
                                      Map<String, String> parameters, boolean fileLogging,
                                      LogMode logMode);

    /**
     * The second half of the bootstrap. It takes four callbacks, all four of
     * them about RealVNC accounts — signing in, syncing connection records,
     * browsing a team directory, naming team labels — and this client supports
     * none of that.
     *
     * <p>So the four objects are made here and never leave: {@link NoCloud}
     * answers every one of them with nothing, and a caller sees a bootstrap
     * call with no arguments instead of a cloud API it would have to implement
     * in order to ignore. The interfaces exist because the core resolves them
     * by name, and are package-private for the same reason.
     */
    public static void initViewer() {
        final NoCloud stub = new NoCloud();
        initVwr(stub, stub, stub, stub);
    }

    private static native void initVwr(SignInMgrBindings.SignInUi signInUi,
                                       SyncMgrBindings.Callback syncCallback,
                                       DirectoryBrowserBindings.Callback dirCallback,
                                       LabelBindings.Callback labelCallback);

    /**
     * One object for all four, since it says the same thing to each.
     *
     * <p>Every method is declared on the class rather than left to a default on
     * the interface: the core reaches these through {@code GetMethodID} on this
     * object's own class, and inherited defaults are not worth betting a native
     * lookup on.
     */
    private static final class NoCloud implements SignInMgrBindings.SignInUi,
            SyncMgrBindings.Callback, DirectoryBrowserBindings.Callback, LabelBindings.Callback {

        @Override
        public void openSsoUrl(String url) {
        }

        @Override
        public void signInChanged(int state) {
        }

        @Override
        public void signInFailed(String message, boolean canRetry, boolean fatal) {
        }

        @Override
        public void signInNetworkStatusChanged() {
        }

        @Override
        public void signInSecondFactorsRequired(SignInMgrBindings.SecondFactorRequest request) {
        }

        @Override
        public void signInSignedIn() {
        }

        @Override
        public void signInSignedOut(boolean forced) {
        }

        @Override
        public boolean signInSsoRefreshSignedOut() {
            return false;
        }

        @Override
        public void signInStarted() {
        }

        @Override
        public void signInTermsChanged() {
        }

        @Override
        public void signInTimedOut() {
        }

        @Override
        public void serverEntriesChanged() {
        }

        @Override
        public void directoryChanged(List<?> entries, String[] labels) {
        }

        @Override
        public void directoryEntryGone(String id) {
        }

        @Override
        public void directorySearchCoveredChanged(boolean covered) {
        }

        @Override
        public void labelsChanged() {
        }
    }
}
