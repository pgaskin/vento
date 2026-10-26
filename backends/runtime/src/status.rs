// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! A connecting screen's status for a wait that is usually too short to
//! read.
//!
//! Most of a connection's steps are over in milliseconds, and a status for
//! each would flash past unread. Some of them sometimes last (a server asking
//! its user to accept the connection, a slow authentication), and those are
//! worth naming. [`Delayed`] names a wait only once it has lasted
//! [`READABLE`], from a thread of its own, since the protocol thread is
//! blocked in the wait it describes.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{RecvTimeoutError, Sender, channel};
use std::thread::JoinHandle;
use std::time::Duration;

/// [`Handler::status`](crate::Handler::status) as a handle another thread
/// can hold: see [`Handler::status_setter`](crate::Handler::status_setter).
pub type StatusSetter = Arc<dyn Fn(Option<&str>) + Send + Sync>;

/// How long a wait lasts before its status is shown.
pub const READABLE: Duration = Duration::from_secs(1);

/// A status shown once the wait it names has lasted [`READABLE`], and never
/// if it ends sooner, which is when this is dropped. What was shown stays
/// until the next status replaces it, or the screen drops it as the session
/// connects or ends.
pub struct Delayed {
    stop: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
    shown: Arc<AtomicBool>,
}

impl Delayed {
    /// `setter` is the handler's; with none, this does nothing.
    pub fn new(setter: Option<StatusSetter>, text: &str) -> Delayed {
        Delayed::after(setter, text, READABLE)
    }

    fn after(setter: Option<StatusSetter>, text: &str, delay: Duration) -> Delayed {
        let shown = Arc::new(AtomicBool::new(false));
        let Some(setter) = setter else {
            return Delayed {
                stop: None,
                thread: None,
                shown,
            };
        };
        let text = text.to_string();
        let (stop, stopped) = channel::<()>();
        let showing = Arc::clone(&shown);
        let thread = std::thread::Builder::new()
            .name("status".into())
            .spawn(move || {
                if let Err(RecvTimeoutError::Timeout) = stopped.recv_timeout(delay) {
                    showing.store(true, Ordering::Release);
                    setter(Some(&text));
                }
            })
            .ok();
        Delayed {
            stop: Some(stop),
            thread,
            shown,
        }
    }

    /// Ends the wait, and says whether its status was shown, for a caller
    /// that has to clear it when the next step names nothing.
    pub fn end(mut self) -> bool {
        self.stop_thread();
        self.shown.load(Ordering::Acquire)
    }

    fn stop_thread(&mut self) {
        drop(self.stop.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Delayed {
    fn drop(&mut self) {
        self.stop_thread();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn recorder() -> (StatusSetter, Arc<Mutex<Vec<String>>>) {
        let said = Arc::new(Mutex::new(Vec::new()));
        let into = Arc::clone(&said);
        let setter: StatusSetter = Arc::new(move |text: Option<&str>| {
            into.lock().unwrap().push(text.unwrap_or("-").to_string());
        });
        (setter, said)
    }

    #[test]
    fn a_wait_shorter_than_the_delay_says_nothing() {
        let (setter, said) = recorder();
        drop(Delayed::after(Some(setter), "waiting", Duration::from_secs(5)));
        assert!(said.lock().unwrap().is_empty());
    }

    #[test]
    fn a_wait_that_lasts_is_named_once() {
        let (setter, said) = recorder();
        let waiting = Delayed::after(Some(setter.clone()), "waiting", Duration::from_millis(10));
        std::thread::sleep(Duration::from_millis(200));
        assert!(waiting.end());
        assert_eq!(*said.lock().unwrap(), ["waiting"]);
        assert!(!Delayed::after(Some(setter), "brief", Duration::from_secs(5)).end());
    }
}
