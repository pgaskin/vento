// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! Where a protocol thread waits for the user's answer.

use crate::handler::Asked;
use std::sync::{Condvar, Mutex};

/// What a slot holds.
enum State<T> {
    /// Nothing yet.
    Empty,
    /// An answer, or a cancel (`None`).
    Answered(Option<T>),
    /// The question was taken back by the protocol (see
    /// [`Withdraw`](crate::handler::Withdraw)).
    Withdrawn,
}

/// There are two of these in a [`Bridge`](crate::Bridge) (the credentials
/// and the trust decision), and both work the same way: the handler fires an
/// event, blocks here, and [`Slot::answer`] wakes it from whatever thread the
/// answer arrives on.
///
/// An answer is kept until it is taken, which tells "cancelled" (a `None`
/// answer) apart from "not yet", so a cancel does not look like a spurious
/// wake-up, and a cancel that arrives before the question (a session closed
/// as it asks) still ends the wait. Taking the answer empties the slot again,
/// so a server that asks twice is asked twice.
pub struct Slot<T> {
    state: Mutex<(State<T>, bool)>,
    ready: Condvar,
}

impl<T> Slot<T> {
    pub fn new() -> Slot<T> {
        Slot {
            state: Mutex::new((State::Empty, false)),
            ready: Condvar::new(),
        }
    }

    /// A question is about to be asked: from here until it is answered, it
    /// can be [withdrawn](Self::withdraw). An answer left from an earlier
    /// question (one given as that question was withdrawn) is dropped, since
    /// it does not answer this one; a cancel is kept, since it ends the
    /// session whatever is asked.
    pub fn begin(&self) {
        let mut state = self.state.lock().unwrap();
        if matches!(state.0, State::Answered(Some(_)) | State::Withdrawn) {
            state.0 = State::Empty;
        }
        state.1 = true;
    }

    pub fn wait(&self) -> Option<T> {
        match self.wait_asked() {
            Asked::Answered(value) => Some(value),
            Asked::Cancelled | Asked::Withdrawn => None,
        }
    }

    /// The answer, or that the question was withdrawn.
    pub fn wait_asked(&self) -> Asked<T> {
        let mut state = self.state.lock().unwrap();
        while matches!(state.0, State::Empty) {
            state = self.ready.wait(state).unwrap();
        }
        state.1 = false;
        match std::mem::replace(&mut state.0, State::Empty) {
            State::Answered(Some(value)) => Asked::Answered(value),
            State::Answered(None) => Asked::Cancelled,
            State::Withdrawn | State::Empty => Asked::Withdrawn,
        }
    }

    pub fn answer(&self, value: Option<T>) {
        let mut state = self.state.lock().unwrap();
        state.0 = State::Answered(value);
        self.ready.notify_all();
    }

    /// Takes back the question [begun](Self::begin) and not yet answered.
    /// Nothing happens otherwise: an answer already given stands, and a
    /// question that has been answered is no longer outstanding.
    pub fn withdraw(&self) {
        let mut state = self.state.lock().unwrap();
        if state.1 && matches!(state.0, State::Empty) {
            state.0 = State::Withdrawn;
            self.ready.notify_all();
        }
    }
}

impl<T> Default for Slot<T> {
    fn default() -> Slot<T> {
        Slot::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn an_answer_wakes_a_waiter_and_is_taken_once() {
        let slot = Arc::new(Slot::new());
        let waiter = {
            let slot = Arc::clone(&slot);
            std::thread::spawn(move || slot.wait())
        };
        slot.answer(Some(7));
        assert_eq!(waiter.join().unwrap(), Some(7));

        // Taken: the next wait is a fresh question, not a replay.
        slot.answer(None);
        assert_eq!(slot.wait(), None);
    }

    #[test]
    fn a_withdrawal_wakes_a_waiter_and_only_for_an_open_question() {
        let slot = Arc::new(Slot::<i32>::new());
        // Nothing asked: nothing to withdraw.
        slot.withdraw();
        slot.answer(Some(1));
        assert!(matches!(slot.wait_asked(), Asked::Answered(1)));

        slot.begin();
        let waiter = {
            let slot = Arc::clone(&slot);
            std::thread::spawn(move || slot.wait_asked())
        };
        slot.withdraw();
        assert!(matches!(waiter.join().unwrap(), Asked::Withdrawn));
        // Late, for a question already withdrawn: kept until the next
        // question begins, which drops it.
        slot.answer(Some(2));
        slot.begin();
        slot.answer(Some(3));
        assert!(matches!(slot.wait_asked(), Asked::Answered(3)));

        // An answer given before the withdrawal stands.
        slot.begin();
        slot.answer(Some(4));
        slot.withdraw();
        assert!(matches!(slot.wait_asked(), Asked::Answered(4)));
        // And a withdrawal after the answer was taken does nothing.
        slot.withdraw();
        slot.answer(None);
        assert!(matches!(slot.wait_asked(), Asked::Cancelled));
    }

    /// A session closed as it asks: the cancel comes first, and the question
    /// begun after it still ends.
    #[test]
    fn a_cancel_before_the_question_is_kept() {
        let slot = Slot::<i32>::new();
        slot.answer(None);
        slot.begin();
        assert!(matches!(slot.wait_asked(), Asked::Cancelled));
    }
}
