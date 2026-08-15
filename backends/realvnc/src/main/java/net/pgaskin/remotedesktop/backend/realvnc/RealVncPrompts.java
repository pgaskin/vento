// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend.realvnc;

import android.util.Log;

import com.realvnc.vncviewer.jni.AuthkeyChoiceDlgBindings;
import com.realvnc.vncviewer.jni.InteractiveTextDlgBindings;
import com.realvnc.vncviewer.jni.MsgBoxBindings;
import com.realvnc.vncviewer.jni.PasswdDlgBindings;
import com.realvnc.vncviewer.jni.ReconnectorBindings;
import com.realvnc.vncviewer.jni.SaveCredentialsBindings;
import com.realvnc.vncviewer.jni.SaveIdentityBindings;
import com.realvnc.vncviewer.jni.SecurityDlgBindings;
import com.realvnc.vncviewer.jni.SecurityNotificationBindings;
import com.realvnc.vncviewer.jni.SessionBindings;
import com.realvnc.vncviewer.jni.StatusNotificationBindings;

import net.pgaskin.remotedesktop.backend.Prompt;

import java.util.IdentityHashMap;
import java.util.Map;

/**
 * The six prompt families, and the three "please persist this" notifications,
 * bridged onto {@link Prompt}.
 *
 * <p>Factories are process-wide, so this is where the {@code Session} a prompt
 * carries is turned back into the backend that owns it. There is one wrinkle:
 * a prompt can arrive <em>during</em> {@code createSession}, before the handle
 * it is keyed on exists. Since every {@code createSession} happens on the one
 * session thread, at most one can be in flight, so a single {@link #pending}
 * slot covers that window exactly.
 *
 * <p>Answers go back on the session thread, as the original does — the result
 * natives are static and take only the cookie, so the only ordering that
 * matters is that they are called at all.
 */
final class RealVncPrompts {

    private static final Map<SessionBindings.Session, RealVncBackend> owners =
            new IdentityHashMap<>();

    /** The backend whose {@code createSession} has not returned yet. */
    private static volatile RealVncBackend pending;

    private RealVncPrompts() {
    }

    static synchronized void attach(SessionBindings.Session session, RealVncBackend backend) {
        owners.put(session, backend);
    }

    static synchronized void detach(SessionBindings.Session session) {
        owners.remove(session);
    }

    static void connecting(RealVncBackend backend) {
        pending = backend;
    }

    static void connected(RealVncBackend backend) {
        if (pending == backend) {
            pending = null;
        }
    }

    /**
     * Every answer takes the same two steps: tell the backend a dialog is gone,
     * so its connect timeout can start running again, and post the result to the
     * session thread, which is the only one allowed to call the natives.
     */
    private static void answered(RealVncBackend backend, Runnable result) {
        backend.promptAnswered();
        RealVncCore.post(result);
    }

    private static synchronized RealVncBackend owner(SessionBindings.Session session) {
        final RealVncBackend b = owners.get(session);
        return b != null ? b : pending;
    }

    static void registerFactories() {
        PasswdDlgBindings.setPasswdDlgFactory(RealVncPrompts::createPasswd);
        SecurityDlgBindings.setSecurityDlgFactory(RealVncPrompts::createSecurity);
        MsgBoxBindings.setMsgBoxFactory(RealVncPrompts::createMsgBox);

        // The three below are answered without asking anyone. Interactive
        // authentication needs a form UI, and choosing a client identity needs
        // identities we never enrol; both decline rather than hang, which is
        // the difference between "this server is not supported yet" and "the
        // app froze". Both become real prompts when something needs them.
        InteractiveTextDlgBindings.setInteractiveTextDlgFactory((session, cookie) ->
                new InteractiveTextDlgBindings.InteractiveTextDlg() {
                    @Override
                    public void promptInteractive(
                            java.util.List<InteractiveTextDlgBindings.UiElement> elements,
                            boolean userNameFixed) {
                        Log.w(RealVncCore.TAG, "declining interactive prompt with "
                                + (elements == null ? 0 : elements.size()) + " elements");
                        RealVncCore.post(() ->
                                InteractiveTextDlgBindings.promptResult(cookie, false, null));
                    }

                    @Override
                    public void close() {
                    }
                });

        AuthkeyChoiceDlgBindings.setAuthkeyChoiceDlgFactory((session, cookie) ->
                new AuthkeyChoiceDlgBindings.AuthkeyChoiceDlg() {
                    @Override
                    public void show(String[] identities) {
                        Log.w(RealVncCore.TAG, "declining identity choice");
                        RealVncCore.post(() ->
                                AuthkeyChoiceDlgBindings.identityChosen(cookie, false, null));
                    }

                    @Override
                    public void close() {
                    }
                });

        ReconnectorBindings.setDlgFactory(session -> new ReconnectorBindings.ReconnectDlg() {
            @Override
            public void show(String message, String detail) {
                final RealVncBackend b = owner(session);
                if (b != null) {
                    b.reconnecting(message, detail);
                }
            }

            @Override
            public void hide() {
                final RealVncBackend b = owner(session);
                if (b != null) {
                    b.reconnected();
                }
            }

            @Override
            public void close() {
            }
        });

        // Notifications, not questions: the core has already acted.
        SaveCredentialsBindings.setCredentialsStore((session, userName, password) -> {
            final RealVncBackend b = owner(session);
            if (b != null) {
                b.saveCredentials(userName, password);
            }
        });
        SaveIdentityBindings.setIdentityStore((session, identity) -> {
            final RealVncBackend b = owner(session);
            if (b != null) {
                b.saveIdentity(identity);
            }
        });
        StatusNotificationBindings.setStatusNotifier((session, text) -> {
            final RealVncBackend b = owner(session);
            if (b != null) {
                b.status(text);
            }
        });
        SecurityNotificationBindings.setSecurityNotifier((session, text, background, flags) ->
                Log.i(RealVncCore.TAG, "security banner: " + text + " (" + background + ")"));
    }

    // ---- the three we actually ask -----------------------------------------

    /**
     * The seven arguments are not what their names in the decompile suggest.
     * Read off which field of the original's own password dialog each one is
     * put in, they are: an unused first string, the server
     * name, the user name — <em>null when the scheme has none</em>, which is
     * what hides the field — a "wants a password" flag, the catchphrase, the
     * signature, and the instructions.
     */
    private static PasswdDlgBindings.PasswdDlg createPasswd(
            SessionBindings.Session session, long cookie, String unused) {
        final RealVncBackend backend = owner(session);
        return new PasswdDlgBindings.PasswdDlg() {
            @Override
            public void getUserPasswd(String ignored, String name, String userName,
                                      boolean needsPassword, String catchphrase,
                                      String signature, String instructions) {
                if (backend == null) {
                    RealVncCore.post(() ->
                            PasswdDlgBindings.passwdResult(cookie, false, null, null));
                    return;
                }
                backend.ask(new Prompt.Credentials(name, instructions, userName != null,
                        userName == null ? "" : userName, needsPassword, catchphrase, signature) {
                    @Override
                    protected void deliver(boolean ok, String user, String pass) {
                        answered(backend, () ->
                                PasswdDlgBindings.passwdResult(cookie, ok, user, pass));
                    }
                });
            }

            @Override
            public void close() {
            }
        };
    }

    /**
     * Same lesson as {@link #createPasswd}: the string order here is
     * signature, catchphrase, server name, matching server name — read off the
     * fields the original's own security dialog puts each one in, not off the
     * parameter names, which a decompiler invented.
     */
    private static SecurityDlgBindings.SecurityDlg createSecurity(
            SessionBindings.Session session, long cookie) {
        final RealVncBackend backend = owner(session);
        return new SecurityDlgBindings.SecurityDlg() {
            @Override
            public void promptSecurity(int identityState, int encryptionState, String signature,
                                       String catchphrase, String name, String matchingName,
                                       String hint) {
                if (backend == null) {
                    RealVncCore.post(() ->
                            SecurityDlgBindings.securityResult(cookie, false, false, false));
                    return;
                }
                // The identity is the first argument and the encryption the
                // second, which is the opposite of what their names in a
                // decompiled signature suggest — see SecurityDlgBindings.
                Log.i(RealVncCore.TAG, "promptSecurity id=" + identityState
                        + " enc=" + encryptionState + " name=" + name);
                backend.ask(new Prompt.Trust(name, encryption(encryptionState),
                        identity(identityState), signature, catchphrase, matchingName) {
                    @Override
                    protected void deliver(boolean accept, boolean remember) {
                        // The two save flags are identity and encryption; the
                        // encryption decision is not one we want remembered
                        // silently, so only the identity is offered.
                        answered(backend, () ->
                                SecurityDlgBindings.securityResult(cookie, accept, remember, false));
                    }
                });
            }

            @Override
            public void close() {
            }
        };
    }

    private static MsgBoxBindings.MsgBox createMsgBox(SessionBindings.Session session, long cookie,
                                                      String message, int flags,
                                                      String customButton) {
        final RealVncBackend backend = owner(session);
        final boolean question = (flags & MsgBoxBindings.MB_OKCANCEL) != 0
                || (flags & MsgBoxBindings.MB_YESNO) != 0;
        final Prompt.Message.Severity severity = severity(flags);
        if (backend != null) {
            backend.ask(new Prompt.Message(backend.address(), message, severity, question,
                    (flags & MsgBoxBindings.MB_CUSTOM) != 0 ? customButton : null) {
                @Override
                protected void deliver(boolean ok) {
                    answered(backend, () -> MsgBoxBindings.msgBoxResult(cookie, ok));
                }
            });
        } else {
            RealVncCore.post(() -> MsgBoxBindings.msgBoxResult(cookie, false));
        }
        return () -> {
        };
    }

    private static Prompt.Trust.Encryption encryption(int state) {
        return switch (state) {
            case SecurityDlgBindings.ENC_ENCRYPTED -> Prompt.Trust.Encryption.ENCRYPTED;
            case SecurityDlgBindings.ENC_UNENCRYPTED_NO_WARN ->
                    Prompt.Trust.Encryption.UNENCRYPTED_QUIET;
            default -> Prompt.Trust.Encryption.UNENCRYPTED_WARN;
        };
    }

    private static Prompt.Trust.Identity identity(int state) {
        return switch (state) {
            case SecurityDlgBindings.ID_OK -> Prompt.Trust.Identity.OK;
            case SecurityDlgBindings.ID_NEW -> Prompt.Trust.Identity.NEW;
            case SecurityDlgBindings.ID_MATCHES_ANOTHER_SERVER ->
                    Prompt.Trust.Identity.MATCHES_ANOTHER;
            case SecurityDlgBindings.ID_CHANGED -> Prompt.Trust.Identity.CHANGED;
            case SecurityDlgBindings.ID_PRESHARED -> Prompt.Trust.Identity.PRESHARED;
            case SecurityDlgBindings.ID_ARD -> Prompt.Trust.Identity.ARD;
            default -> Prompt.Trust.Identity.MISSING;
        };
    }

    private static Prompt.Message.Severity severity(int flags) {
        // The icon bits are a small enum in the high nibble, not independent
        // flags: MB_ICONWARNING is 48, which is MB_ICONERROR | MB_ICONQUESTION.
        return switch (flags & MsgBoxBindings.MB_ICON_MASK) {
            case MsgBoxBindings.MB_ICONERROR -> Prompt.Message.Severity.ERROR;
            case MsgBoxBindings.MB_ICONQUESTION -> Prompt.Message.Severity.QUESTION;
            case MsgBoxBindings.MB_ICONWARNING -> Prompt.Message.Severity.WARNING;
            default -> Prompt.Message.Severity.INFORMATION;
        };
    }
}
