// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! Wire primitives. RFB is big-endian everywhere except inside a pixel.

use std::io::{self, BufReader, BufWriter, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// The longest run of bytes a length field may claim before it is treated as a
/// protocol error rather than as a string. One number for both the strings
/// [`Reader::string`] reads and the clipboard's own length field, since what it
/// bounds is the same thing: how much a server can make this end allocate by
/// saying so.
pub const MAX_STRING: usize = 1 << 20;

/// How much has gone past, kept where more than one thread can read it: the
/// reader ends up on the protocol thread and the writer on its own, and what
/// asks for the total is neither of them. Shared rather than per-stream so that
/// a TLS upgrade — which replaces both — carries its counts over instead of
/// starting the session's total again at the handshake.
pub type Counter = Arc<AtomicU64>;

/// A buffered reader with the fixed-width reads the protocol is made of.
pub struct Reader<R: Read> {
    inner: BufReader<R>,
    /// Everything that has come off the socket, for the line-speed estimate
    /// and for what the panel says the session has moved.
    count: Counter,
}

impl<R: Read> Read for Reader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.count.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

impl<R: Read> Reader<R> {
    /// A reader that counts for nobody but itself. Tests only: a session's two
    /// counters are the connection's, not a stream's.
    #[cfg(test)]
    pub fn new(inner: R) -> Reader<R> {
        Reader::counting(inner, Counter::default())
    }

    pub fn counting(inner: R, count: Counter) -> Reader<R> {
        Reader {
            inner: BufReader::with_capacity(64 * 1024, inner),
            count,
        }
    }

    pub fn counter(&self) -> Counter {
        Arc::clone(&self.count)
    }

    pub fn byte_count(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }

    /// What has been read off the socket and not yet consumed. Zero everywhere
    /// the protocol is in step; the one caller that asks is the TLS upgrade,
    /// where anything held here would be a byte of the encrypted stream about
    /// to be read as clear text (see `transport::Wire::start_tls`).
    pub fn buffered(&self) -> usize {
        self.inner.buffer().len()
    }

    pub fn u8(&mut self) -> io::Result<u8> {
        let mut b = [0u8; 1];
        self.read_exact(&mut b)?;
        Ok(b[0])
    }

    pub fn u16(&mut self) -> io::Result<u16> {
        let mut b = [0u8; 2];
        self.read_exact(&mut b)?;
        Ok(u16::from_be_bytes(b))
    }

    pub fn u32(&mut self) -> io::Result<u32> {
        let mut b = [0u8; 4];
        self.read_exact(&mut b)?;
        Ok(u32::from_be_bytes(b))
    }

    pub fn i32(&mut self) -> io::Result<i32> {
        Ok(self.u32()? as i32)
    }

    pub fn skip(&mut self, n: usize) -> io::Result<()> {
        // Through `self`, not `self.inner`: skipped bytes came off the socket
        // too, and the line-speed estimate is a measurement of the socket.
        let skipped = io::copy(&mut Read::by_ref(self).take(n as u64), &mut io::sink())?;
        if skipped != n as u64 {
            return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
        }
        Ok(())
    }

    pub fn bytes(&mut self, n: usize) -> io::Result<Vec<u8>> {
        let mut v = vec![0u8; n];
        self.read_exact(&mut v)?;
        Ok(v)
    }

    /// A `u32` length and that many bytes: how RFB carries every string.
    pub fn string(&mut self) -> io::Result<String> {
        let n = self.u32()? as usize;
        // A hostile length would otherwise be a multi-gigabyte allocation
        // before the read fails.
        if n > MAX_STRING {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("string of {n} bytes"),
            ));
        }
        Ok(latin1(&self.bytes(n)?))
    }
}

/// A buffered writer. Every message is written whole and then flushed, because
/// a half-written message on a socket nobody flushes is a hang.
pub struct Writer<W: Write> {
    inner: BufWriter<W>,
    /// Everything handed to the socket, counted here rather than at the far
    /// end of the buffer: every message is flushed as it is written, so the
    /// two differ by at most the message in hand.
    count: Counter,
}

impl<W: Write> Write for Writer<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.count.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl<W: Write> Writer<W> {
    pub fn counting(inner: W, count: Counter) -> Writer<W> {
        Writer {
            inner: BufWriter::with_capacity(8 * 1024, inner),
            count,
        }
    }

    pub fn counter(&self) -> Counter {
        Arc::clone(&self.count)
    }

    pub fn u8(&mut self, v: u8) -> io::Result<()> {
        self.write_all(&[v])
    }

    pub fn pad(&mut self, n: usize) -> io::Result<()> {
        self.write_all(&vec![0u8; n])
    }
}

/// RFB's strings are bytes with no declared encoding; the specification calls
/// them ISO 8859-1 and that is what `ClientCutText` is defined as.
pub fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

/// The other direction. Anything outside Latin-1 has no representation on the
/// wire at all, so it becomes `?` rather than silently truncating the message.
pub fn to_latin1(s: &str) -> Vec<u8> {
    s.chars()
        .map(|c| if (c as u32) < 256 { c as u8 } else { b'?' })
        .collect()
}
