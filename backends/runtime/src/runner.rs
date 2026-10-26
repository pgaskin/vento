// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! What runs one session: a client, the thread it runs on, and the state its
//! creator holds a handle to.
//!
//! Two threads reach a session: the protocol thread, which owns the connection
//! and delivers every callback, and the holder of the handle. On Android that
//! is Java, calling in from the UI thread and from **whichever thread is
//! drawing**, which reads regions directly so that pixel fetches never queue
//! behind connection work. The client is behind an `Arc` for that reason, and
//! the protocol crates keep their own state behind locks fine-grained enough
//! for the drawing thread to take the read side.
//!
//! There was one of these per protocol, and the four were copies of each
//! other but for the thread's name and the shape of the credentials; what
//! differed was the protocol's and is now behind [`Client`].

use crate::callbacks::Callbacks;
use crate::client::Client;
use crate::handler::{Credentials, Handler};
use crate::slot::Slot;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// A [`Handler`] over a [`Callbacks`]: everything is forwarded as is, and the
/// questions become an event and a wait.
///
/// Cheap to clone (three `Arc`s), because the runner keeps one end to answer
/// through and the protocol thread takes the other.
pub struct Bridge {
    events: Arc<dyn Callbacks>,
    credentials: Arc<Slot<Credentials>>,
    trust: Arc<Slot<bool>>,
}

impl Clone for Bridge {
    fn clone(&self) -> Bridge {
        Bridge {
            events: Arc::clone(&self.events),
            credentials: Arc::clone(&self.credentials),
            trust: Arc::clone(&self.trust),
        }
    }
}

impl Bridge {
    pub fn new(events: impl Callbacks + 'static) -> Bridge {
        Bridge {
            events: Arc::new(events),
            credentials: Arc::new(Slot::new()),
            trust: Arc::new(Slot::new()),
        }
    }

    /// The answer to [`Callbacks::credentials_needed`]. `None` cancels, which
    /// ends the session.
    pub fn answer_credentials(&self, credentials: Option<Credentials>) {
        self.credentials.answer(credentials);
    }

    /// The answer to [`Callbacks::trust_needed`] and to [`Callbacks::unverified`],
    /// which share a slot because they are one question at the protocol
    /// thread's end (go on or not), and only one of them can be outstanding.
    pub fn answer_trust(&self, accept: bool) {
        self.trust.answer(Some(accept));
    }

    /// Answer whatever is pending with a refusal, for a session that is being
    /// closed from under a prompt.
    fn cancel(&self) {
        self.credentials.answer(None);
        self.trust.answer(Some(false));
    }
}

impl Handler for Bridge {
    fn connected(&mut self, width: usize, height: usize) {
        self.events.connected(width, height);
    }

    fn desktop_size(&mut self, width: usize, height: usize) {
        self.events.desktop_size(width, height);
    }

    fn damaged(&mut self, x: usize, y: usize, width: usize, height: usize) {
        self.events.damaged(x, y, width, height);
    }

    fn frame_end(&mut self) {
        self.events.frame_end();
    }

    fn cursor(&mut self, pixels: &[u32], width: usize, height: usize, hot_x: i32, hot_y: i32) {
        self.events.cursor(pixels, width, height, hot_x, hot_y);
    }

    fn pointer_mode(&mut self, relative: bool) {
        self.events.pointer_mode(relative);
    }

    fn bell(&mut self) {
        self.events.bell();
    }

    fn status(&mut self, text: Option<&str>) {
        self.events.status(text);
    }

    fn clipboard(&mut self, text: &str) {
        self.events.clipboard(text);
    }

    fn credentials(&mut self, needs_user: bool) -> Option<Credentials> {
        self.events.credentials_needed(needs_user);
        self.credentials.wait()
    }

    fn trust(&mut self, fingerprint: &str) -> bool {
        self.events.trust_needed(fingerprint);
        // A cancelled prompt is a refusal, the safe default for the one
        // question where a wrong answer connects to an impersonated host.
        self.trust.wait().unwrap_or(false)
    }

    fn unverified(&mut self, why: &str) -> bool {
        self.events.unverified(why);
        self.trust.wait().unwrap_or(false)
    }
}

/// One session of a [`Client`], running: the thread it is on, and the handle
/// its creator holds.
pub struct Runner<C: Client> {
    /// The protocol's own interface (pointer, keys, region reads), used
    /// directly, because what it offers differs per protocol and a runner is
    /// only the thread and the questions.
    pub client: Arc<C>,
    bridge: Bridge,
    thread: Mutex<Option<JoinHandle<()>>>,
    closed: AtomicBool,
}

impl<C: Client> Runner<C> {
    /// Creates the session and starts connecting. Returns immediately; the
    /// protocol runs on its own thread, which is where every event comes from,
    /// including [`Callbacks::closed`], the last thing the thread does.
    pub fn start(client: Arc<C>, config: C::Config, bridge: Bridge) -> Runner<C> {
        let thread = {
            let client = Arc::clone(&client);
            let mut handler = bridge.clone();
            std::thread::Builder::new()
                .name(format!("{}-session", C::NAME))
                .spawn(move || {
                    let detail = match client.run(&config, &mut handler) {
                        Ok(()) => String::new(),
                        // Requested, so the screen already knows and shows no
                        // message.
                        Err(e) if C::closed_on_request(&e) => String::new(),
                        Err(e) => e.to_string(),
                    };
                    handler.events.closed(&detail);
                })
                .unwrap_or_else(|e| panic!("spawning the {} session thread: {e}", C::NAME))
        };

        Runner {
            client,
            bridge,
            thread: Mutex::new(Some(thread)),
            closed: AtomicBool::new(false),
        }
    }

    pub fn answer_credentials(&self, credentials: Option<Credentials>) {
        self.bridge.answer_credentials(credentials);
    }

    pub fn answer_trust(&self, accept: bool) {
        self.bridge.answer_trust(accept);
    }

    /// Ends the session without waiting for it. Idempotent.
    pub fn disconnect(&self) {
        if !self.closed.swap(true, Ordering::AcqRel) {
            // A session waiting on an unanswered prompt is blocked in one of
            // the slots, not in a socket read (and on a current-thread runtime
            // that blocks the whole session), so closing has to wake those too
            // or the thread never ends. The client is closed first: the woken
            // thread sees a refused prompt, and only a protocol that already
            // knows it was closed reports that as a requested close
            // (`closed_on_request`) rather than as a refusal. Woken in the
            // other order, it can read its flag before `close` has set it.
            self.client.close();
            self.bridge.cancel();
        }
    }

    /// Ends the session and waits for its thread, so that when this returns
    /// the events are finished and their receiver (on Android, a `Global`
    /// reference into Java) can be dropped.
    ///
    /// The RealVNC backend solves the same problem the other way round (retire
    /// the handle, then post the free) because RealVNC's core keeps calling
    /// after `sessionClosed`. Here both sides of the boundary are ours, so a
    /// join is enough and nothing has to outlive the session.
    pub fn destroy(&self) {
        self.disconnect();
        let thread = self.thread.lock().unwrap().take();
        if let Some(thread) = thread {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc::{Sender, channel};
    use std::time::Duration;

    /// Records what it is told, on a channel the test reads.
    struct Recorder(Mutex<Sender<String>>);

    impl Recorder {
        fn say(&self, what: String) {
            let _ = self.0.lock().unwrap().send(what);
        }
    }

    impl Callbacks for Recorder {
        fn connected(&self, width: usize, height: usize) {
            self.say(format!("connected {width}x{height}"));
        }
        fn desktop_size(&self, _width: usize, _height: usize) {}
        fn damaged(&self, _x: usize, _y: usize, _width: usize, _height: usize) {}
        fn frame_end(&self) {}
        fn cursor(&self, _pixels: &[u32], _width: usize, _height: usize, _hot_x: i32, _hot_y: i32) {
        }
        fn pointer_mode(&self, _relative: bool) {}
        fn bell(&self) {}
        fn status(&self, _text: Option<&str>) {}
        fn clipboard(&self, _text: &str) {}
        fn credentials_needed(&self, needs_user: bool) {
            self.say(format!("credentials? {needs_user}"));
        }
        fn trust_needed(&self, fingerprint: &str) {
            self.say(format!("trust? {fingerprint}"));
        }
        fn unverified(&self, why: &str) {
            self.say(format!("unverified? {why}"));
        }
        fn closed(&self, detail: &str) {
            self.say(format!("closed {detail:?}"));
        }
    }

    /// A protocol that asks both questions and then reports what it was told.
    struct Asker {
        closing: AtomicBool,
    }

    #[derive(Debug)]
    enum Ending {
        Closed,
        Refused(String),
    }

    impl std::fmt::Display for Ending {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Ending::Closed => write!(f, "closed"),
                Ending::Refused(why) => write!(f, "refused: {why}"),
            }
        }
    }

    impl Client for Asker {
        type Config = &'static str;
        type Error = Ending;
        const NAME: &'static str = "asker";

        fn run(&self, config: &&'static str, handler: &mut dyn Handler) -> Result<(), Ending> {
            if !handler.trust(config) {
                return Err(Ending::Refused("trust".into()));
            }
            let Some(credentials) = handler.credentials(true) else {
                return Err(if self.closing.load(Ordering::Acquire) {
                    Ending::Closed
                } else {
                    Ending::Refused("credentials".into())
                });
            };
            handler.connected(credentials.user.len(), credentials.password.len());
            Ok(())
        }

        fn close(&self) {
            self.closing.store(true, Ordering::Release);
        }

        fn closed_on_request(error: &Ending) -> bool {
            matches!(error, Ending::Closed)
        }
    }

    fn start() -> (Runner<Asker>, std::sync::mpsc::Receiver<String>) {
        let (tx, rx) = channel();
        let client = Arc::new(Asker {
            closing: AtomicBool::new(false),
        });
        let bridge = Bridge::new(Recorder(Mutex::new(tx)));
        (Runner::start(client, "ab:cd", bridge), rx)
    }

    fn next(rx: &std::sync::mpsc::Receiver<String>) -> String {
        rx.recv_timeout(Duration::from_secs(5)).expect("an event")
    }

    #[test]
    fn answers_reach_the_protocol_thread_in_order() {
        let (session, rx) = start();
        assert_eq!(next(&rx), "trust? ab:cd");
        session.answer_trust(true);
        assert_eq!(next(&rx), "credentials? true");
        session.answer_credentials(Some(Credentials {
            user: "me".into(),
            domain: String::new(),
            password: "secret".into(),
        }));
        assert_eq!(next(&rx), "connected 2x6");
        assert_eq!(next(&rx), "closed \"\"");
        session.destroy();
    }

    #[test]
    fn a_cancelled_trust_prompt_is_a_refusal() {
        let (session, rx) = start();
        assert_eq!(next(&rx), "trust? ab:cd");
        session.answer_trust(false);
        assert_eq!(next(&rx), "closed \"refused: trust\"");
        session.destroy();
    }

    /// The reason `disconnect` wakes the slots: a thread parked in one would
    /// otherwise never see `close`, and `destroy`'s join would never return.
    #[test]
    fn destroy_wakes_a_session_parked_behind_a_prompt() {
        let (session, rx) = start();
        assert_eq!(next(&rx), "trust? ab:cd");
        session.answer_trust(true);
        assert_eq!(next(&rx), "credentials? true");
        session.destroy();
        // A requested close says nothing, however the protocol reported it.
        assert_eq!(next(&rx), "closed \"\"");
    }
}
