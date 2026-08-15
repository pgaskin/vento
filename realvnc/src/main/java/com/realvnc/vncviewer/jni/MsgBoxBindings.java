// SPDX-License-Identifier: CC0-1.0

package com.realvnc.vncviewer.jni;

/**
 * A message with Win32-style flags, which is how the core reports most of what
 * goes wrong short of losing the session.
 *
 * <p>The prompt protocol is {@link PasswdDlgBindings}'s. This one differs in
 * that the question arrives at the factory rather than at the object: there is
 * no second call, so a message box has only {@code close()}.
 *
 * <p>Required by
 * {@code Java_com_realvnc_vncviewer_jni_MsgBoxBindings_setMsgBoxFactory} and
 * {@code …_msgBoxResult}, and by {@code MsgBoxBindings$MsgBox} and
 * {@code $MsgBoxFactory}, which the core resolves by name.
 */
public final class MsgBoxBindings {

    /** Buttons. {@code MB_CUSTOM} means the button text came with the message. */
    public static final int MB_OK = 0;
    public static final int MB_OKCANCEL = 1;
    public static final int MB_YESNO = 4;
    public static final int MB_CUSTOM = 8;

    /**
     * The icon, which is a small enumeration in bits 4–6 rather than a set of
     * independent flags: {@code MB_ICONWARNING} is {@code MB_ICONERROR |
     * MB_ICONQUESTION}, so it has to be read with {@link #MB_ICON_MASK} and
     * compared, not tested a bit at a time.
     */
    public static final int MB_ICONERROR = 16;
    public static final int MB_ICONQUESTION = 32;
    public static final int MB_ICONWARNING = 48;
    public static final int MB_ICONINFORMATION = 64;
    public static final int MB_ICON_MASK = 0x70;

    /** Which button is focused. Not a distinction a phone makes. */
    public static final int MB_DEFBUTTON1 = 0;
    public static final int MB_DEFBUTTON2 = 256;

    public interface MsgBox {
        void close();
    }

    public interface MsgBoxFactory {
        MsgBox createMsgBox(SessionBindings.Session session, long cookie, String message,
                            int flags, String customButton);
    }

    private MsgBoxBindings() {
    }

    public static native void msgBoxResult(long cookie, boolean ok);

    public static native void setMsgBoxFactory(MsgBoxFactory factory);
}
