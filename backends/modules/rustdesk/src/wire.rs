// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! RustDesk's entire transport: one length-prefixed frame format, and a hook
//! for the cipher a session may agree on.
//!
//! Split in two because the two directions run on different threads: the
//! protocol thread reads and a dedicated writer thread writes, so a stalled
//! socket cannot block the thread that pressed a key. A cipher counts its
//! frames, and each direction counts separately, so [`Cipher`] is per
//! direction rather than shared.
//!
//! A direct-access session agrees no cipher at all: RustDesk's `direct_server`
//! passes its socket to `create_tcp_connection` with `secure = false`, so
//! nothing is offered or requested, and every frame is plaintext protobuf. A
//! session reached by id agrees one (`crypto::Secretbox`) before its first
//! message.

use std::io::{Read, Write};
use runtime::net::Socket;
use std::net::Shutdown;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Encryption of a frame's payload with the session key. A trait rather than
/// the cipher itself because this file only does framing and knows nothing
/// about keys: `crypto::Secretbox` is the only implementation, and a
/// direct-access session installs none.
pub trait Cipher: Send {
    fn seal(&mut self, payload: Vec<u8>) -> std::io::Result<Vec<u8>>;

    fn open(&mut self, payload: Vec<u8>) -> std::io::Result<Vec<u8>>;
}

/// The largest payload this will allocate. The header can express a gigabyte;
/// a key frame of a 4K desktop is a few megabytes, and the far end is not
/// trusted to stay within that.
const MAX_FRAME: usize = 64 << 20;

/// Bytes received and sent since the socket was opened: the frames as read
/// from and written to the socket, headers and encryption included, without
/// what TCP and IP add. Shared because the two directions are on two threads
/// and the panel's row is read from a third.
#[derive(Default)]
pub struct Traffic {
    pub received: AtomicU64,
    pub sent: AtomicU64,
}

pub struct Reader {
    sock: Socket,
    cipher: Option<Box<dyn Cipher>>,
    traffic: Arc<Traffic>,
}

pub struct Writer {
    sock: Socket,
    cipher: Option<Box<dyn Cipher>>,
    traffic: Arc<Traffic>,
}

/// Both directions of one socket, and a handle for closing it.
///
/// The close handle ends a session from another thread: shutting the socket
/// down is the only way to wake a read blocked on it.
pub fn split(sock: Socket, traffic: Arc<Traffic>) -> std::io::Result<(Reader, Writer, Socket)> {
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

    /// One frame's payload, decrypted if a cipher has been agreed.
    ///
    /// A payload of one byte or less is passed through unchanged, as RustDesk
    /// does, so the empty `Message` a client without a key sends is readable
    /// either way.
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

    /// One message, parsed: `Message` in a session and `RendezvousMessage`
    /// while setting one up. The framing is the same and only the top-level
    /// type differs.
    pub fn message<M: protobuf::Message>(&mut self) -> std::io::Result<M> {
        let bytes = self.frame()?;
        M::parse_from_bytes(&bytes).map_err(|e| std::io::Error::other(format!("not a message: {e}")))
    }

    pub fn set_timeout(&self, timeout: Option<std::time::Duration>) -> std::io::Result<()> {
        self.sock.set_read_timeout(timeout)
    }

    /// Whether a frame has begun to arrive within `timeout`, reading none of
    /// it: a reader that has to stop now and then can wait with this rather
    /// than with a timed-out read, which would lose part of a frame. True at
    /// the end of the stream too, for the read to report. Leaves `timeout`
    /// as the socket's read timeout.
    pub fn arriving(&self, timeout: std::time::Duration) -> std::io::Result<bool> {
        self.sock.set_read_timeout(Some(timeout))?;
        match self.sock.peek(&mut [0u8; 1]) {
            Ok(_) => Ok(true),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => Ok(false),
            Err(e) => Err(e),
        }
    }
}

impl Writer {
    pub fn set_cipher(&mut self, cipher: Box<dyn Cipher>) {
        self.cipher = Some(cipher);
    }

    pub fn send(&mut self, msg: &impl protobuf::Message) -> std::io::Result<()> {
        self.send_bytes(msg.write_to_bytes()?)
    }

    /// A message already serialized by the caller, which lets input leave the
    /// thread that produced it without waiting for the socket.
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
    /// shifts and masks are easy to get wrong at the boundaries, and each test
    /// size is one byte either side of the header growing.
    #[test]
    fn header_lengths() {
        for n in [0usize, 1, 0x3F, 0x40, 0x3FFF, 0x4000, 0x3F_FFFF, 0x40_0000] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let payload = vec![0xa5u8; n];
            let expected = payload.clone();
            let sender = std::thread::spawn(move || {
                let sock = std::net::TcpStream::connect(addr).unwrap();
                let (_, mut writer, _) = split(sock.into(), Arc::new(Traffic::default())).unwrap();
                writer.send_bytes(payload).unwrap();
            });
            let (sock, _) = listener.accept().unwrap();
            let (mut reader, _, _) = split(sock.into(), Arc::new(Traffic::default())).unwrap();
            assert_eq!(reader.frame().unwrap(), expected, "{n} bytes");
            sender.join().unwrap();
        }
    }
}
