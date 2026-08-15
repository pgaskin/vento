// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend;

import java.util.concurrent.atomic.AtomicBoolean;

/**
 * A question the connection has to have answered before it can continue.
 *
 * <p>These are not optional politeness: with nothing answering them a session
 * against a server that wants a password simply never progresses. The answer
 * may be deferred
 * arbitrarily — that is the point, since answering means showing a dialog and
 * waiting for a person — and may come from any thread.
 *
 * <p>Sealed, so that a handler which compiles has considered every kind. What
 * the RealVNC core can ask is wider than this (a dynamic form for interactive
 * authentication, a choice of client identity); those are declined by the
 * backend rather than surfaced, and become new subclasses when something needs
 * them.
 */
public abstract sealed class Prompt {

    /**
     * Called on the main thread. Each method receives a prompt that must
     * eventually be answered or {@link Prompt#cancel cancelled} — exactly once.
     */
    public interface Handler {

        void credentials(Credentials prompt);

        void trust(Trust prompt);

        void message(Message prompt);
    }

    // Answering twice is a protocol error at the far end.
    private final AtomicBoolean answered = new AtomicBoolean();

    public final String address; // being connected to, for a dialog to name

    Prompt(String address) {
        this.address = address;
    }

    /** True the first time, false ever after. */
    protected final boolean claim() {
        return answered.compareAndSet(false, true);
    }

    /** Give up on the connection. Safe to call twice; the second does nothing. */
    public abstract void cancel();

    /**
     * "Who are you?" — a user name, a password, or both, depending on what the
     * server's authentication scheme wants. Which of the two it wants is not a
     * detail: VncAuth has no user name at all, so a dialog that shows one is
     * asking for something nobody can answer.
     */
    public abstract static non-sealed class Credentials extends Prompt {

        public final String instructions;    // the scheme's own words, or empty
        public final boolean needsUserName;  // false when the scheme has no notion of one
        public final String userName;        // pre-filled, from the record or a previous attempt
        public final boolean needsPassword;
        // The server's identity, so it is checkable before a secret is typed.
        public final String catchphrase;
        public final String signature;

        protected Credentials(String address, String instructions, boolean needsUserName,
                              String userName, boolean needsPassword, String catchphrase,
                              String signature) {
            super(address);
            this.instructions = instructions;
            this.needsUserName = needsUserName;
            this.userName = userName;
            this.needsPassword = needsPassword;
            this.catchphrase = catchphrase;
            this.signature = signature;
        }

        public final void answer(String userName, String password) {
            if (claim()) {
                deliver(true, userName, password);
            }
        }

        @Override
        public final void cancel() {
            if (claim()) {
                deliver(false, null, null);
            }
        }

        protected abstract void deliver(boolean ok, String userName, String password);
    }

    /**
     * "Is this the machine you meant, and is anyone listening?" One prompt for
     * both questions because the core asks them together — and because the
     * answer to "the identity changed" depends on whether the link is encrypted.
     */
    public abstract static non-sealed class Trust extends Prompt {

        public enum Encryption {
            ENCRYPTED,
            UNENCRYPTED_WARN,  // plain text, and the user should be told
            UNENCRYPTED_QUIET  // plain text, deliberately: a local or already-tunnelled link
        }

        public enum Identity {
            OK,               // known, and matches
            NEW,              // never seen before
            MATCHES_ANOTHER,  // seen, but under a different address
            CHANGED,          // seen under this address, and different now: the bad one
            MISSING,          // the server offered none
            PRESHARED,        // configured out of band
            ARD               // Apple Remote Desktop, which has no identity of this kind
        }

        public final Encryption encryption;
        public final Identity identity;
        public final String signature;        // the identity itself
        public final String catchphrase;      // and its human-readable form
        public final String matchingAddress;  // for MATCHES_ANOTHER: the address it belongs to

        protected Trust(String address, Encryption encryption, Identity identity,
                        String signature, String catchphrase, String matchingAddress) {
            super(address);
            this.encryption = encryption;
            this.identity = identity;
            this.signature = signature;
            this.catchphrase = catchphrase;
            this.matchingAddress = matchingAddress;
        }

        /** @param remember accept this identity for next time, without asking */
        public final void answer(boolean accept, boolean remember) {
            if (claim()) {
                deliver(accept, remember);
            }
        }

        @Override
        public final void cancel() {
            answer(false, false);
        }

        protected abstract void deliver(boolean accept, boolean remember);
    }

    /**
     * Something went wrong, or something needs confirming. {@link #question} is
     * the difference between a dialog with one button and one with two.
     */
    public abstract static non-sealed class Message extends Prompt {

        public enum Severity {
            INFORMATION,
            QUESTION,
            WARNING,
            ERROR
        }

        public final String text;
        public final Severity severity;
        public final boolean question;    // yes or no, rather than OK
        public final String confirmLabel; // the affirmative button's own text, if there is one

        protected Message(String address, String text, Severity severity,
                          boolean question, String confirmLabel) {
            super(address);
            this.text = text;
            this.severity = severity;
            this.question = question;
            this.confirmLabel = confirmLabel;
        }

        public final void answer(boolean ok) {
            if (claim()) {
                deliver(ok);
            }
        }

        @Override
        public final void cancel() {
            answer(false);
        }

        protected abstract void deliver(boolean ok);
    }
}
