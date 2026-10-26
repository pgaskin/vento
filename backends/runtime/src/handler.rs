// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! What a protocol thread tells the app, and the two things it asks.

use std::sync::{Arc, Mutex};

/// The answer to "who are you", for the schemes that ask.
///
/// One shape for every protocol, which is the superset: RDP carries a domain,
/// and where a protocol has no notion of one it arrives empty. RFB's VncAuth
/// and SPICE's ticket and RustDesk's password have no user either, and ask
/// with `needs_user` false so that a dialog does not show a field that does
/// not apply.
#[derive(Clone, Default)]
pub struct Credentials {
    pub user: String,
    pub domain: String,
    pub password: String,
}

/// What a question that can be withdrawn came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Asked<T> {
    Answered(T),
    /// The user gave up on the connection.
    Cancelled,
    /// The protocol took the question back through its [`Withdraw`].
    Withdrawn,
}

/// The waiting handler's side of a [`Withdraw`].
type Wake = Box<dyn Fn() + Send>;

/// A protocol's way to take back a question it is blocked on, from another
/// of its threads: the far end can make a password prompt pointless while it
/// is open (a RustDesk peer whose user accepted the session). Fired once, it
/// stays fired: a question asked with it afterwards is withdrawn at once.
/// Cheap to clone; every clone is the same handle.
#[derive(Clone, Default)]
pub struct Withdraw(Arc<Mutex<(bool, Option<Wake>)>>);

impl Withdraw {
    pub fn new() -> Withdraw {
        Withdraw::default()
    }

    /// Takes the question back. Safe from any thread, and more than once.
    pub fn fire(&self) {
        let mut state = self.0.lock().unwrap();
        state.0 = true;
        if let Some(wake) = &state.1 {
            wake();
        }
    }

    pub fn fired(&self) -> bool {
        self.0.lock().unwrap().0
    }

    /// For a handler that is about to wait: `wake` is called when the handle
    /// fires, at once if it already has, until [`disarm`](Self::disarm).
    /// It runs under the handle's lock and must not block.
    pub fn arm(&self, wake: impl Fn() + Send + 'static) {
        let mut state = self.0.lock().unwrap();
        if state.0 {
            wake();
        }
        state.1 = Some(Box::new(wake));
    }

    pub fn disarm(&self) {
        self.0.lock().unwrap().1 = None;
    }
}

/// Everything a session tells the app, called from the protocol thread.
///
/// Every method arrives on that thread and must be cheap, because the
/// connection is not being read while it runs; [`credentials`], [`trust`] and
/// [`unverified`] are the exceptions and may block for as long as the user
/// takes, since the session has nothing else to do while it waits for one.
///
/// This is the superset of what four protocols need, and the methods only
/// some of them call have bodies that do nothing: a handler for a client that
/// never changes pointer mode or rings a bell does not have to implement them.
///
/// [`credentials`]: Handler::credentials
/// [`trust`]: Handler::trust
/// [`unverified`]: Handler::unverified
pub trait Handler {
    /// The connection is up and the desktop is this big. Once per session,
    /// before anything is drawn.
    fn connected(&mut self, width: usize, height: usize);

    /// The desktop changed size mid-session: RFB's `DesktopSize`, RDP's
    /// reactivation, SPICE's surface reset, RustDesk's switch of display.
    fn desktop_size(&mut self, width: usize, height: usize);

    /// This rectangle of the framebuffer has new pixels. Hundreds of times a
    /// frame on a busy screen, so it should mark the area, not draw.
    fn damaged(&mut self, x: usize, y: usize, width: usize, height: usize);

    /// Everything that belongs to one update has arrived; a good moment to
    /// draw what [`damaged`](Handler::damaged) marked.
    fn frame_end(&mut self);

    /// The pointer's shape. `pixels` is `width × height` in the framebuffer's
    /// own word order (`R, G, B, A` in memory), alpha from the cursor's mask
    /// where the protocol has one. A zero size means the server has hidden
    /// the pointer.
    fn cursor(&mut self, pixels: &[u32], width: usize, height: usize, hot_x: i32, hot_y: i32);

    /// The server has changed what a pointer event's coordinates mean. In
    /// relative mode the client does not know where the cursor is, so a
    /// client that draws one and follows it with a viewport has to stop doing
    /// both. RFB says this with `PointerTypeChange`, SPICE in the main
    /// channel's `INIT` and whenever it changes after; the other protocols are
    /// absolute for the length of a session and never call it.
    fn pointer_mode(&mut self, relative: bool) {
        let _ = relative;
    }

    /// RFB's `Bell`; no other protocol sends one.
    fn bell(&mut self) {}

    /// What the connection is waiting for while it connects, as a short
    /// sentence for the connecting screen, or `None` to clear it: a far end
    /// whose user has to accept the session, a server asking its user, a
    /// retry. Only for a wait long enough to read. The latest one stands
    /// until replaced, and the screen drops it once the session connects or
    /// ends, so a backend clears it only when a wait ends without either.
    fn status(&mut self, text: Option<&str>) {
        let _ = text;
    }

    /// [`status`](Handler::status) as a handle another thread can call, for
    /// a status that is due only once a wait has lasted long enough to read
    /// ([`status::Delayed`](crate::status::Delayed)). `None` for a handler
    /// with no screen to show one on.
    fn status_setter(&mut self) -> Option<crate::status::StatusSetter> {
        None
    }

    /// What the far end's clipboard now holds, as text.
    fn clipboard(&mut self, text: &str);

    /// The connection needs credentials and the config had none to give, or
    /// gave ones that were refused. `None` gives up on the connection.
    ///
    /// `needs_user` is false for a scheme that has no notion of a user name
    /// (VncAuth, a SPICE ticket, a RustDesk password), so that a dialog does
    /// not show a field that does not apply. When it is asked differs by
    /// protocol: RFB and SPICE and RustDesk ask when the server requests them;
    /// RDP carries them in the connection sequence and asks **before**
    /// anything is connected.
    fn credentials(&mut self, needs_user: bool) -> Option<Credentials>;

    /// [`credentials`](Handler::credentials), for a question the protocol
    /// may take back while it is open: when `withdraw` fires, the prompt is
    /// closed without an answer and this returns [`Asked::Withdrawn`]. A
    /// handler with no prompt to close (one that answers at once) can leave
    /// the default, which asks as `credentials` does.
    fn credentials_or_withdrawn(&mut self, needs_user: bool, withdraw: &Withdraw) -> Asked<Credentials> {
        if withdraw.fired() {
            return Asked::Withdrawn;
        }
        match self.credentials(needs_user) {
            Some(credentials) => Asked::Answered(credentials),
            None => Asked::Cancelled,
        }
    }

    /// The far end's identity, by fingerprint, before anything secret is sent:
    /// a TLS certificate for RFB, RDP and SPICE, the peer's long-term key for
    /// RustDesk. `false` ends the connection. The pin store is the app's, so
    /// this is the question rather than the decision.
    fn trust(&mut self, fingerprint: &str) -> bool;

    /// The far end could not be verified at all, with the reason as text. This
    /// is a different question from whether an identity is the right one:
    /// there is nothing to pin and nothing to compare, so the app can only
    /// show the reason and a way out. `true` goes on with the session in
    /// the clear; `false` ends it. Only RustDesk asks, and the default is the
    /// safe answer.
    fn unverified(&mut self, why: &str) -> bool {
        let _ = why;
        false
    }
}
