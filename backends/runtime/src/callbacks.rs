// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The outward half of a session: what a platform is told, and nothing it is
//! asked.

/// What a [`Runner`](crate::Runner) tells the platform, fire-and-forget.
///
/// This is [`Handler`](crate::Handler) with the blocking taken out: every
/// question becomes an event that says a question is pending, and the answer
/// arrives later through the runner, which is how a dialog on another thread
/// has to work, and how a C caller would want it too. The C API's
/// `vento_callbacks` is the same list, method for method, with three more; on
/// Android the calls reach the Java `NativeBackend` as its `on…` methods, by
/// way of the `api` crate's mailbox rather than one for one.
///
/// Every method is called from the protocol thread, through a shared
/// reference, and must not block on anything the protocol thread is waiting
/// for; or, behind the C API, from another thread the C library runs
/// (FreeRDP's graphics pipeline paints on its dynamic channels' thread:
/// `vento/backend.h`). `Send + Sync` because the session's thread calls these
/// and the session's creator holds the other end.
pub trait Callbacks: Send + Sync {
    fn connected(&self, width: usize, height: usize);
    fn desktop_size(&self, width: usize, height: usize);
    fn damaged(&self, x: usize, y: usize, width: usize, height: usize);
    fn frame_end(&self);
    /// `pixels` is the framebuffer's own word order (`R, G, B, A` in memory);
    /// whatever the platform wants instead, it converts on its side of this
    /// call, where it already has to copy. See [`crate::cursor`] for the two
    /// things every platform so far has wanted.
    fn cursor(&self, pixels: &[u32], width: usize, height: usize, hot_x: i32, hot_y: i32);
    fn pointer_mode(&self, relative: bool);
    fn bell(&self);
    /// What the connection is waiting for, or `None` to clear it; see
    /// [`Handler::status`](crate::Handler::status).
    fn status(&self, text: Option<&str>);
    fn clipboard(&self, text: &str);
    /// Credentials are wanted; answered through
    /// [`Runner::answer_credentials`](crate::Runner::answer_credentials).
    fn credentials_needed(&self, needs_user: bool);
    /// A trust decision is wanted; answered through
    /// [`Runner::answer_trust`](crate::Runner::answer_trust).
    fn trust_needed(&self, fingerprint: &str);
    /// The far end could not be verified and the session wants to know
    /// whether to go on. Answered through the same
    /// [`Runner::answer_trust`](crate::Runner::answer_trust), since a yes
    /// and a no mean the same things to the protocol thread.
    fn unverified(&self, why: &str);
    /// The session is over and its thread is about to end. `detail` is empty
    /// for an ending that needs no message, and otherwise is the error's
    /// description of why.
    fn closed(&self, detail: &str);
}
