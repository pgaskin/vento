// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The sockets a session is blocked on, for ending it from another thread.
//!
//! A protocol thread spends most of its time in a socket call (a read, or,
//! before there is anything to read, the dial), and only the socket itself can
//! interrupt one: shut it down and the call returns. Every client here did
//! that for the connected socket and none of them for the dial, because
//! `TcpStream::connect_timeout` creates its socket internally and gives no
//! handle to it. A disconnect while a black-holed address was being dialled
//! therefore waited out the whole connect timeout, and a `destroy` after it
//! blocked the window's thread, joined on the session's, for twenty seconds.
//!
//! So the dial ([`Direct`](super::Direct)) is done on a socket that is held
//! here from the moment its connect has begun, and [`Hangup::hang_up`] shuts
//! down whatever is held at the time. Shutting down a socket whose connect is
//! still in flight aborts the connect (Linux's `inet_shutdown` disconnects a
//! `SYN_SENT` socket and wakes the threads waiting on it), so one mechanism
//! covers both the dial and the connected session. A socketpair's end shut
//! down wakes its reader the same way.
//!
//! A wait that is not on a socket (name resolution, which the C library does
//! on a thread of its own) is held as a function to call instead
//! ([`Hangup::on_hang_up`]), which ends the wait though not the work.

use super::Socket;
use std::io;
use std::net::Shutdown;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// A flag and the sockets it breaks. One per session.
#[derive(Default)]
pub struct Hangup {
    hung_up: AtomicBool,
    held: Mutex<Held>,
}

#[derive(Default)]
struct Held {
    next: u64,
    entries: Vec<(u64, Entry)>,
}

enum Entry {
    Socket(Socket),
    Wake(Box<dyn FnOnce() + Send>),
}

/// A socket or a wake [`Hangup`] is holding, until this is dropped. A socket
/// is held as a second descriptor for it, so letting go of it closes nothing
/// the caller still has.
#[must_use = "it is held only while this is"]
pub struct Holding<'a> {
    hangup: &'a Hangup,
    id: u64,
}

impl Drop for Holding<'_> {
    fn drop(&mut self) {
        let mut held = self.hangup.held.lock().unwrap();
        held.entries.retain(|(id, _)| *id != self.id);
    }
}

impl Hangup {
    pub fn new() -> Hangup {
        Hangup::default()
    }

    /// Ends whatever is held, and refuses anything held from now on. Safe to
    /// call from any thread, any number of times.
    pub fn hang_up(&self) {
        // The flag before the lock, and `hold` tests it under the lock: a
        // socket is either held in time to be shut down here or refused there,
        // and never slips between the two.
        self.hung_up.store(true, Ordering::Release);
        let wakes: Vec<Box<dyn FnOnce() + Send>> = {
            let mut held = self.held.lock().unwrap();
            let mut wakes = Vec::new();
            let mut kept = Vec::new();
            for (id, entry) in held.entries.drain(..) {
                match entry {
                    Entry::Socket(socket) => {
                        let _ = socket.shutdown(Shutdown::Both);
                        kept.push((id, Entry::Socket(socket)));
                    }
                    Entry::Wake(wake) => wakes.push(wake),
                }
            }
            held.entries = kept;
            wakes
        };
        // Outside the lock, so that a wake may touch this hang-up.
        for wake in wakes {
            wake();
        }
    }

    pub fn is_hung_up(&self) -> bool {
        self.hung_up.load(Ordering::Acquire)
    }

    /// Holds `socket` until the returned guard is dropped, so that a hang-up
    /// shuts it down. Refused, with the socket shut down, once the session has
    /// been hung up.
    pub fn hold(&self, socket: &Socket) -> io::Result<Holding<'_>> {
        let mut held = self.held.lock().unwrap();
        if self.is_hung_up() {
            let _ = socket.shutdown(Shutdown::Both);
            return Err(closed());
        }
        let id = held.next;
        held.next += 1;
        held.entries.push((id, Entry::Socket(socket.try_clone()?)));
        Ok(Holding { hangup: self, id })
    }

    /// Calls `wake` on a hang-up while the returned guard is held, for a wait
    /// on something other than a socket. Refused, without calling it, once
    /// the session has been hung up.
    pub fn on_hang_up(&self, wake: Box<dyn FnOnce() + Send>) -> io::Result<Holding<'_>> {
        let mut held = self.held.lock().unwrap();
        if self.is_hung_up() {
            return Err(closed());
        }
        let id = held.next;
        held.next += 1;
        held.entries.push((id, Entry::Wake(wake)));
        Ok(Holding { hangup: self, id })
    }
}

/// What a dial or a hold says once the session has been hung up.
pub(super) fn closed() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "the session was closed")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{TcpListener, TcpStream};
    use std::os::unix::net::UnixStream;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    #[test]
    fn a_hang_up_breaks_a_read() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let hangup = Hangup::new();
        let stream = Socket::Tcp(TcpStream::connect(listener.local_addr().unwrap()).unwrap());
        let _accepted = listener.accept().unwrap();
        let holding = hangup.hold(&stream).unwrap();
        std::thread::scope(|scope| {
            let reading = scope.spawn(|| {
                let mut byte = [0u8];
                std::io::Read::read(&mut &stream, &mut byte)
            });
            std::thread::sleep(Duration::from_millis(100));
            hangup.hang_up();
            // A shut-down socket reads as the end of the stream.
            assert_eq!(reading.join().unwrap().unwrap(), 0);
        });
        drop(holding);
    }

    /// The same on a socketpair's end, which is what a relayed session reads.
    #[test]
    fn a_hang_up_breaks_a_read_on_a_socketpair() {
        let (near, _far) = UnixStream::pair().unwrap();
        let near = Socket::Unix(near);
        let hangup = Hangup::new();
        let holding = hangup.hold(&near).unwrap();
        std::thread::scope(|scope| {
            let reading = scope.spawn(|| {
                let mut byte = [0u8];
                std::io::Read::read(&mut &near, &mut byte)
            });
            std::thread::sleep(Duration::from_millis(100));
            hangup.hang_up();
            assert_eq!(reading.join().unwrap().unwrap(), 0);
        });
        drop(holding);
    }

    #[test]
    fn a_dropped_holding_lets_go() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let hangup = Hangup::new();
        let stream = Socket::Tcp(TcpStream::connect(listener.local_addr().unwrap()).unwrap());
        drop(hangup.hold(&stream).unwrap());
        drop(hangup.on_hang_up(Box::new(|| {})).unwrap());
        assert!(hangup.held.lock().unwrap().entries.is_empty());
    }

    #[test]
    fn nothing_is_held_after_a_hang_up() {
        let (near, _far) = UnixStream::pair().unwrap();
        let hangup = Hangup::new();
        hangup.hang_up();
        let near = Socket::Unix(near);
        assert_eq!(hangup.hold(&near).err().unwrap().kind(), io::ErrorKind::Interrupted);
        assert_eq!(
            hangup.on_hang_up(Box::new(|| panic!("called after the hang-up"))).err().unwrap().kind(),
            io::ErrorKind::Interrupted
        );
    }

    /// A wake is called once, by the first hang-up, and not after its guard
    /// has gone.
    #[test]
    fn a_wake_is_called_once() {
        let hangup = Hangup::new();
        let called = Arc::new(AtomicUsize::new(0));
        let count = |called: &Arc<AtomicUsize>| {
            let called = Arc::clone(called);
            Box::new(move || {
                called.fetch_add(1, Ordering::SeqCst);
            })
        };
        drop(hangup.on_hang_up(count(&called)).unwrap());
        let _held = hangup.on_hang_up(count(&called)).unwrap();
        hangup.hang_up();
        hangup.hang_up();
        assert_eq!(called.load(Ordering::SeqCst), 1);
    }
}
