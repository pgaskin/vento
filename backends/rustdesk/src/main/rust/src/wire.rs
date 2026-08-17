// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The whole of their transport: one length-prefixed frame format, and a hook
//! for the cipher a session may agree on.
//!
//! Split in two because the two directions run on different threads — the
//! protocol thread reads and a writer thread of its own writes, so a stalled
//! socket cannot reach whichever thread pressed a key. A cipher counts its
//! frames, and each direction counts separately, which is why [`Cipher`] is
//! per-half rather than shared.
//!
//! A direct-access session agrees no cipher at all: their `direct_server` hands
//! its socket to `create_tcp_connection` with `secure = false`, so nothing is
//! offered and nothing is asked for, and every frame there is plaintext
//! protobuf. A session reached by id agrees one before its first message, which
//! is `crypto::Secretbox`.

use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// What a session's key does to a frame's payload. It is a trait rather than the
/// cipher itself because this file is the framing and knows nothing about keys:
/// `crypto::Secretbox` is the one implementation, and a direct-access session
/// installs none.
pub trait Cipher: Send {
    fn seal(&mut self, payload: Vec<u8>) -> std::io::Result<Vec<u8>>;

    fn open(&mut self, payload: Vec<u8>) -> std::io::Result<Vec<u8>>;
}

/// The largest payload this will allocate for. Their header can express a
/// gigabyte; a key frame of a 4K desktop is a few megabytes, and the far end is
/// not trusted to be well behaved with the difference.
const MAX_FRAME: usize = 64 << 20;

/// Bytes moved since the socket was opened — protocol bytes, inside whatever is
/// encrypting them and outside what the link adds, which is what the panel's
/// row says it means. Shared because the two halves are on two threads and the
/// row is read from a third.
#[derive(Default)]
pub struct Traffic {
    pub received: AtomicU64,
    pub sent: AtomicU64,
}

pub struct Reader {
    sock: TcpStream,
    cipher: Option<Box<dyn Cipher>>,
    traffic: Arc<Traffic>,
}

pub struct Writer {
    sock: TcpStream,
    cipher: Option<Box<dyn Cipher>>,
    traffic: Arc<Traffic>,
}

/// Both halves of one socket, and the handle that closes it.
///
/// The close handle is what ends a session from another thread: a read blocked
/// on a socket wakes when the socket is shut down, and there is nothing else to
/// interrupt it with.
pub fn split(sock: TcpStream, traffic: Arc<Traffic>) -> std::io::Result<(Reader, Writer, TcpStream)> {
    let write_half = sock.try_clone()?;
    let closer = sock.try_clone()?;
    Ok((
        Reader {
            sock,
            cipher: None,
            traffic: Arc::clone(&traffic),
        },
        Writer {
            sock: write_half,
            cipher: None,
            traffic,
        },
        closer,
    ))
}

impl Reader {
    pub fn set_cipher(&mut self, cipher: Box<dyn Cipher>) {
        self.cipher = Some(cipher);
    }

    /// One frame's payload, decrypted where a cipher has been agreed.
    ///
    /// A payload of one byte or less is passed through as it is, which is their
    /// rule and is what makes the empty `Message` a client sends when it has no
    /// key legible either way.
    pub fn frame(&mut self) -> std::io::Result<Vec<u8>> {
        let mut first = [0u8; 1];
        self.sock.read_exact(&mut first)?;
        let head_len = (first[0] & 0x3) as usize + 1;
        let mut head = [0u8; 4];
        head[0] = first[0];
        if head_len > 1 {
            self.sock.read_exact(&mut head[1..head_len])?;
        }
        // The low two bits are the header's own length minus one; the length is
        // everything above them.
        let n = (u32::from_le_bytes(head) >> 2) as usize;
        if n > MAX_FRAME {
            return Err(std::io::Error::other(format!("a frame of {n} bytes")));
        }
        let mut payload = vec![0u8; n];
        self.sock.read_exact(&mut payload)?;
        self.traffic
            .received
            .fetch_add((head_len + n) as u64, Ordering::Relaxed);
        if payload.len() > 1 {
            if let Some(cipher) = self.cipher.as_mut() {
                payload = cipher.open(payload)?;
            }
        }
        Ok(payload)
    }

    /// One of their messages, parsed. `Message` on a session and
    /// `RendezvousMessage` on the way to one: the framing is the same and only
    /// the top-level type differs.
    pub fn message<M: protobuf::Message>(&mut self) -> std::io::Result<M> {
        let bytes = self.frame()?;
        M::parse_from_bytes(&bytes).map_err(|e| std::io::Error::other(format!("not a message: {e}")))
    }

    pub fn set_timeout(&self, timeout: Option<std::time::Duration>) -> std::io::Result<()> {
        self.sock.set_read_timeout(timeout)
    }
}

impl Writer {
    pub fn set_cipher(&mut self, cipher: Box<dyn Cipher>) {
        self.cipher = Some(cipher);
    }

    pub fn send(&mut self, msg: &impl protobuf::Message) -> std::io::Result<()> {
        self.send_bytes(msg.write_to_bytes()?)
    }

    /// A message somebody else has already serialized — which is how input
    /// leaves the thread that produced it without waiting for the socket.
    pub fn send_bytes(&mut self, payload: Vec<u8>) -> std::io::Result<()> {
        let payload = match self.cipher.as_mut() {
            Some(cipher) => cipher.seal(payload)?,
            None => payload,
        };
        let n = payload.len();
        let mut frame = Vec::with_capacity(n + 4);
        match n {
            0..=0x3F => frame.push((n << 2) as u8),
            0x40..=0x3FFF => frame.extend_from_slice(&(((n << 2) as u16) | 0x1).to_le_bytes()),
            0x4000..=0x3F_FFFF => {
                let h = ((n << 2) as u32) | 0x2;
                frame.extend_from_slice(&(h as u16).to_le_bytes());
                frame.push((h >> 16) as u8);
            }
            _ => frame.extend_from_slice(&(((n << 2) as u32) | 0x3).to_le_bytes()),
        }
        frame.extend_from_slice(&payload);
        self.traffic
            .sent
            .fetch_add(frame.len() as u64, Ordering::Relaxed);
        self.sock.write_all(&frame)
    }

    pub fn close(&self) {
        let _ = self.sock.shutdown(Shutdown::Both);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every header length the format has, round-tripped through a socket pair:
    /// the boundaries are where a shift and a mask are easy to write the wrong
    /// way round, and each is one byte either side of a header growing.
    #[test]
    fn header_lengths() {
        for n in [0usize, 1, 0x3F, 0x40, 0x3FFF, 0x4000, 0x3F_FFFF, 0x40_0000] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let payload = vec![0xa5u8; n];
            let expected = payload.clone();
            let sender = std::thread::spawn(move || {
                let sock = TcpStream::connect(addr).unwrap();
                let (_, mut writer, _) = split(sock, Arc::new(Traffic::default())).unwrap();
                writer.send_bytes(payload).unwrap();
            });
            let (sock, _) = listener.accept().unwrap();
            let (mut reader, _, _) = split(sock, Arc::new(Traffic::default())).unwrap();
            assert_eq!(reader.frame().unwrap(), expected, "{n} bytes");
            sender.join().unwrap();
        }
    }
}
