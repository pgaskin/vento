//! One SPICE session: the client, the thread it runs on, and the state Java
//! holds a handle to.

use crate::client::{Client, Config, Handler};
use crate::error::Error;
use common::android::callbacks::Callbacks;
use common::android::slot::Slot;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

pub struct Session {
    pub client: Arc<Client>,
    password: Arc<Slot<String>>,
    trust: Arc<Slot<bool>>,
    thread: Mutex<Option<JoinHandle<()>>>,
    closed: AtomicBool,
}

/// What the session thread calls; everything is forwarded to Java as is.
struct Bridge {
    callbacks: Arc<Callbacks>,
    password: Arc<Slot<String>>,
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

    fn cursor(&mut self, pixels: &[u32], width: i32, height: i32, hot_x: i32, hot_y: i32) {
        self.callbacks.cursor(pixels, width, height, hot_x, hot_y);
    }

    fn pointer_mode(&mut self, relative: bool) {
        self.callbacks.pointer_mode(relative);
    }

    /// There is no user name in this protocol: a ticket is the whole of what a
    /// SPICE server asks for.
    fn password(&mut self) -> Option<String> {
        self.callbacks.credentials_needed(false);
        self.password.wait()
    }

    fn trust(&mut self, fingerprint: &str) -> bool {
        self.callbacks.trust_needed(fingerprint);
        self.trust.wait().unwrap_or(false)
    }

    fn clipboard(&mut self, text: &str) {
        self.callbacks.clipboard(text);
    }
}

impl Session {
    /// Creates the session and starts connecting. Returns immediately; the
    /// protocol runs on a thread of its own, which is where the runtime lives
    /// and where every callback comes from.
    pub fn start(callbacks: Callbacks, config: Config) -> Session {
        let callbacks = Arc::new(callbacks);
        let client = Arc::new(Client::new());
        let password = Arc::new(Slot::new());
        let trust = Arc::new(Slot::new());

        let mut bridge = Bridge {
            callbacks: Arc::clone(&callbacks),
            password: Arc::clone(&password),
            trust: Arc::clone(&trust),
        };

        let thread = {
            let client = Arc::clone(&client);
            std::thread::Builder::new()
                .name("spice-session".into())
                .spawn(move || {
                    let detail = match client.run(&config, &mut bridge) {
                        // Asked for, which the screen already knows about: what
                        // it wants said is nothing.
                        Ok(()) | Err(Error::Closed) => String::new(),
                        Err(e) => e.to_string(),
                    };
                    callbacks.closed(&detail);
                })
                .expect("spawning the SPICE session thread")
        };

        Session {
            client,
            password,
            trust,
            thread: Mutex::new(Some(thread)),
            closed: AtomicBool::new(false),
        }
    }

    pub fn answer_password(&self, password: Option<String>) {
        self.password.answer(password);
    }

    pub fn answer_trust(&self, accept: bool) {
        self.trust.answer(Some(accept));
    }

    pub fn disconnect(&self) {
        if !self.closed.swap(true, Ordering::AcqRel) {
            // A session sitting behind an unanswered prompt is blocked in one of
            // the slots rather than in a socket read — and on a current-thread
            // runtime that means the whole session is — so closing has to wake
            // those too or the thread never ends.
            self.password.answer(None);
            self.trust.answer(Some(false));
            self.client.close();
        }
    }

    /// Ends the session and waits for its thread, so that when this returns the
    /// callbacks are provably finished and the `Global` reference behind them
    /// can be dropped.
    pub fn destroy(&self) {
        self.disconnect();
        let thread = self.thread.lock().unwrap().take();
        if let Some(thread) = thread {
            let _ = thread.join();
        }
    }
}
