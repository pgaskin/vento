// SPDX-License-Identifier: CC0-1.0

package com.realvnc.vncviewer.jni;

import java.io.Closeable;
import java.util.List;
import java.util.Map;

/**
 * A small dynamic-form protocol, used for multi-factor and other interactive
 * authentication: the core hands over a list of elements to put on screen, and
 * takes back a map of answers.
 *
 * <p>The prompt protocol is {@link PasswdDlgBindings}'s.
 *
 * <p>Required by
 * {@code Java_com_realvnc_vncviewer_jni_InteractiveTextDlgBindings_setInteractiveTextDlgFactory},
 * {@code …_promptResult} and the two {@code …_00024UiResponses_native*} symbols;
 * and by {@code $InteractiveTextDlg}, {@code $InteractiveTextDlgFactory},
 * {@code $UiResponses} and the five {@code $Ui…} element classes, which the core
 * resolves by name and constructs.
 */
public final class InteractiveTextDlgBindings {

    /**
     * One field of the form. The core constructs the members of this set
     * directly, so each record's name and canonical constructor is the ABI; the
     * interface over them is not — it is here so that a caller can switch over
     * the set and have the compiler notice when it misses one.
     */
    public sealed interface UiElement {
        String id();
    }

    /** A text field. {@code echo} false means a password. */
    public record UiEdit(String id, String label, boolean echo, String placeholder,
                         String initialText) implements UiElement {
    }

    /** A drop-down. {@code editable} allows a value that is not in the list. */
    public record UiChoice(String id, String label, List<String> choices, boolean editable,
                           int initialChoice) implements UiElement {
    }

    /** A list to pick one row from. */
    public record UiList(String id, String header, List<String> choices,
                         int initialChoice) implements UiElement {
    }

    /** A check box. */
    public record UiCheck(String id, String label, boolean initiallyChecked)
            implements UiElement {
    }

    /** Text with nothing to fill in. */
    public record UiMessage(String id, String message) implements UiElement {
    }

    public interface InteractiveTextDlg {

        void close();

        /**
         * @param elements      the form, in order; each is one of the
         *                      {@link UiElement} records
         * @param userNameFixed the user name is already decided, so a field for
         *                      it should be shown but not edited
         */
        void promptInteractive(List<UiElement> elements, boolean userNameFixed);
    }

    public interface InteractiveTextDlgFactory {
        InteractiveTextDlg createInteractiveTextDlg(SessionBindings.Session session, long cookie);
    }

    /**
     * The answers, as a native-side copy of {@code id → answer} that outlives
     * the map it was built from. {@link #close} frees it; it is a
     * {@link Closeable} so that a caller can hand it to the result native inside
     * a try-with-resources and not think about it again.
     */
    public static final class UiResponses implements Closeable {

        private long token;

        public UiResponses(Map<String, String> responses) {
            this.token = nativeInit(responses);
        }

        private native long nativeInit(Map<String, String> responses);

        private native void nativeClose(long token);

        @Override
        public void close() {
            final long t = token;
            token = 0;
            if (t != 0) {
                nativeClose(t);
            }
        }
    }

    private InteractiveTextDlgBindings() {
    }

    /** {@code responses} may be null, and must be when {@code ok} is false. */
    public static native void promptResult(long cookie, boolean ok, UiResponses responses);

    public static native void setInteractiveTextDlgFactory(InteractiveTextDlgFactory factory);
}
