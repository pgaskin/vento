// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! Where a protocol thread waits for a person.

use std::sync::{Condvar, Mutex};

/// There are three of these across the two backends — a password, a
/// certificate, and RDP's credentials — and all of them work the same way: the
/// handler asks Java, blocks here, and [`Slot::answer`] wakes it.
///
/// `answered` is what tells "cancelled" (a `None` answer) apart from "not yet",
/// so a cancel does not look like a spurious wake-up; taking the answer clears
/// it again, so a server that asks twice is asked twice.
pub struct Slot<T> {
    state: Mutex<(bool, Option<T>)>,
    ready: Condvar,
}

impl<T> Slot<T> {
    pub fn new() -> Slot<T> {
        Slot {
            state: Mutex::new((false, None)),
            ready: Condvar::new(),
        }
    }

    pub fn wait(&self) -> Option<T> {
        let mut state = self.state.lock().unwrap();
        while !state.0 {
            state = self.ready.wait(state).unwrap();
        }
        state.0 = false;
        state.1.take()
    }

    pub fn answer(&self, value: Option<T>) {
        let mut state = self.state.lock().unwrap();
        *state = (true, value);
        self.ready.notify_all();
    }
}

impl<T> Default for Slot<T> {
    fn default() -> Slot<T> {
        Slot::new()
    }
}
