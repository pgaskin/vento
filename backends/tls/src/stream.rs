// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! One TLS session, read and written by two threads at once.
//!
//! A plain socket splits with `try_clone` and the reader and writer threads
//! never meet. A TLS session cannot: it is one state machine, records are keyed
//! in sequence, and both directions run through it. So the `rustls` connection
//! sits behind a mutex taken **only for the crypto**, and the blocking socket
//! read happens with no lock held at all — otherwise the reader, parked on a
//! socket that says nothing for minutes at a time, would be holding the thing
//! the writer needs for every keystroke.
//!
//! Both halves lock `conn` before `socket_write`, and never the other way
//! round, which is the whole of the deadlock argument.
//!
//! What each protocol keeps for itself is the handshake: VeNCrypt has a plain
//! byte in the middle of its own, and RDP's yields a public key that CredSSP
//! binds to. What arrives here is a connection that is already up.

use rustls::ClientConnection;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};

struct Stream {
    conn: Mutex<ClientConnection>,
    socket_read: Mutex<TcpStream>,
    socket_write: Mutex<TcpStream>,
}

/// One thread's view of the session: `Read` and `Write`, so it goes wherever an
/// ordinary stream does.
///
/// It keeps ciphertext of its own because rustls will not accept more while its
/// decrypted-plaintext buffer is full — 16 KB of it, against a screenful of
/// pixels. So the loop is: hand back plaintext, and only when there is none
/// left take more of the stream, feeding it a record at a time and stopping the
/// moment something is decrypted. What the socket gave us and rustls has not
/// taken waits here rather than being dropped.
pub struct Handle {
    stream: Arc<Stream>,
    ciphertext: Vec<u8>,
    filled: usize,
    at: usize,
}

impl Handle {
    /// Take over a socket whose handshake is already complete. `conn` must be
    /// the connection that did it.
    pub fn new(conn: ClientConnection, socket: &TcpStream) -> io::Result<Handle> {
        Ok(Handle {
            stream: Arc::new(Stream {
                conn: Mutex::new(conn),
                socket_read: Mutex::new(socket.try_clone()?),
                socket_write: Mutex::new(socket.try_clone()?),
            }),
            ciphertext: vec![0u8; 16 * 1024],
            filled: 0,
            at: 0,
        })
    }

    /// A second view of the same session, for the other thread.
    pub fn clone_handle(&self) -> Handle {
        Handle {
            stream: Arc::clone(&self.stream),
            ciphertext: vec![0u8; 16 * 1024],
            filled: 0,
            at: 0,
        }
    }
}

impl Read for Handle {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            {
                let mut conn = self.stream.conn.lock().unwrap();
                match conn.reader().read(buf) {
                    // Nothing decrypted yet is not an error here: it is the
                    // signal to go and get more of the stream.
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                    other => return other,
                }
            }
            if self.at >= self.filled {
                // No lock held: this is the read that waits, and an idle
                // desktop waits in it for as long as nobody touches the far
                // end. Holding the connection here would stop every keystroke.
                let n = self
                    .stream
                    .socket_read
                    .lock()
                    .unwrap()
                    .read(&mut self.ciphertext)?;
                if n == 0 {
                    return Ok(0);
                }
                self.filled = n;
                self.at = 0;
            }
            let mut conn = self.stream.conn.lock().unwrap();
            while self.at < self.filled {
                let taken = conn.read_tls(&mut &self.ciphertext[self.at..self.filled])?;
                if taken == 0 {
                    break;
                }
                self.at += taken;
                let state = conn
                    .process_new_packets()
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                if state.plaintext_bytes_to_read() > 0 {
                    break;
                }
            }
            // A key update or an alert is a record the *reader* has to send.
            if conn.wants_write() {
                let mut socket = self.stream.socket_write.lock().unwrap();
                while conn.wants_write() {
                    conn.write_tls(&mut *socket)?;
                }
                socket.flush()?;
            }
        }
    }
}

impl Write for Handle {
    /// The whole buffer under one lock, and that is the point rather than an
    /// optimisation: rustls accepts at most its 64 KiB send buffer per call, so
    /// anything larger is a short write, and a caller's `write_all` would loop
    /// with this lock released. Where two threads write application data — a
    /// protocol thread's frames and a writer thread's input — the other one's
    /// message would land inside this one.
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut conn = self.stream.conn.lock().unwrap();
        let mut at = 0;
        while at < buf.len() {
            at += conn.writer().write(&buf[at..])?;
            // Draining is also what makes room for the rest: the buffer that
            // capped the write is emptied by the socket taking it.
            let mut socket = self.stream.socket_write.lock().unwrap();
            while conn.wants_write() {
                conn.write_tls(&mut *socket)?;
            }
            socket.flush()?;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut conn = self.stream.conn.lock().unwrap();
        conn.writer().flush()?;
        let mut socket = self.stream.socket_write.lock().unwrap();
        while conn.wants_write() {
            conn.write_tls(&mut *socket)?;
        }
        socket.flush()
    }
}
