// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! One RFB session: the client, the thread it runs on, and the state Java holds
//! a handle to.

use common::android::callbacks::Callbacks;
use common::android::slot::Slot;
use crate::{Client, Config, Credentials, Handler};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

pub struct Session {
    pub client: Arc<Client>,
    credentials: Arc<Slot<Credentials>>,
    trust: Arc<Slot<bool>>,
    thread: Mutex<Option<JoinHandle<()>>>,
    closed: AtomicBool,
}

/// What the protocol thread calls; everything is forwarded to Java as is.
struct Bridge {
    callbacks: Arc<Callbacks>,
    credentials: Arc<Slot<Credentials>>,
    trust: Arc<Slot<bool>>,
}

impl Handler for Bridge {
    fn connected(&mut self, width: usize, height: usize) {
        self.callbacks.connected(width as i32, height as i32);
    }

    fn desktop_size(&mut self, width: usize, height: usize) {
        self.callbacks.desktop_size(width as i32, height as i32);
    }

    fn damaged(&mut self, x: usize, y: usize, width: usize, height: usize) {
        self.callbacks
            .damaged(x as i32, y as i32, width as i32, height as i32);
    }

    fn frame_end(&mut self) {
        self.callbacks.frame_end();
    }

    fn cursor(&mut self, pixels: &[u32], width: usize, height: usize, hot_x: i32, hot_y: i32) {
        self.callbacks
            .cursor(pixels, width as i32, height as i32, hot_x, hot_y);
    }

    fn pointer_mode(&mut self, relative: bool) {
        self.callbacks.pointer_mode(relative);
    }

    fn bell(&mut self) {
        self.callbacks.bell();
    }

    fn clipboard(&mut self, text: &str) {
        self.callbacks.clipboard(text);
    }

    fn credentials(&mut self, needs_user: bool) -> Option<Credentials> {
        self.callbacks.credentials_needed(needs_user);
        self.credentials.wait()
    }

    fn trust(&mut self, fingerprint: &str) -> bool {
        self.callbacks.trust_needed(fingerprint);
        // A cancelled prompt is a refusal, which is the safe direction for the
        // one question whose wrong answer is a stranger's desktop.
        self.trust.wait().unwrap_or(false)
    }
}

impl Session {
    /// Creates the session and starts connecting. Returns immediately; the
    /// protocol runs on a thread of its own, which is where every callback
    /// comes from.
    pub fn start(callbacks: Callbacks, config: Config) -> Session {
        let callbacks = Arc::new(callbacks);
        let client = Arc::new(Client::new());
        let credentials = Arc::new(Slot::new());
        let trust = Arc::new(Slot::new());

        let mut bridge = Bridge {
            callbacks: Arc::clone(&callbacks),
            credentials: Arc::clone(&credentials),
            trust: Arc::clone(&trust),
        };

        let thread = {
            let client = Arc::clone(&client);
            std::thread::Builder::new()
                .name("rfb-session".into())
                .spawn(move || {
                    let detail = match client.run(&config, &mut bridge) {
                        Ok(()) => String::new(),
                        Err(e) => e.to_string(),
                    };
                    callbacks.closed(&detail);
                })
                .expect("spawning the RFB session thread")
        };

        Session {
            client,
            credentials,
            trust,
            thread: Mutex::new(Some(thread)),
            closed: AtomicBool::new(false),
        }
    }

    pub fn answer_credentials(&self, credentials: Option<Credentials>) {
        self.credentials.answer(credentials);
    }

    pub fn answer_trust(&self, accept: bool) {
        self.trust.answer(Some(accept));
    }

    pub fn disconnect(&self) {
        if !self.closed.swap(true, Ordering::AcqRel) {
            // A session sitting behind an unanswered prompt is blocked in one
            // of the slots, not in a socket read, so closing has to wake those
            // too or the thread never ends.
            self.credentials.answer(None);
            self.trust.answer(Some(false));
            self.client.close();
        }
    }

    /// Ends the session and waits for its thread, so that when this returns the
    /// callbacks are provably finished and the `Global` reference behind them
    /// can be dropped.
    ///
    /// The RealVNC backend had to solve the same problem the other way round —
    /// retire the handle, then post the free — because their core keeps
    /// calling after `sessionClosed`. Here both ends of the boundary are ours,
    /// so a join is enough and there is no corpse to leave behind.
    pub fn destroy(&self) {
        self.disconnect();
        let thread = self.thread.lock().unwrap().take();
        if let Some(thread) = thread {
            let _ = thread.join();
        }
    }
}
