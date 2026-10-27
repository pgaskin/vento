// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The connected stream a [`Network`](super::Network) hands a session.
//!
//! An fd either way, so the C libraries' own reads, writes and polls work on
//! it unchanged: a TCP socket where this end's own connection reaches the
//! server, and one end of an `AF_UNIX` socketpair where vento carries the
//! bytes itself ([`Socketpair`](super::Socketpair), and later a tunnel). The
//! difference shows only in what a socket says about itself, which is why the
//! connection's addresses travel beside it in [`Peer`](super::Peer) rather
//! than being asked of the socket.

use std::fmt;
use std::io::{self, IoSlice, IoSliceMut, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::time::Duration;

/// A connected stream socket, owned. Read and written as `TcpStream` is, by
/// value or through a shared reference, so a reader and a writer thread can
/// each hold a [`try_clone`](Socket::try_clone) of it.
pub enum Socket {
    Tcp(TcpStream),
    /// One end of a socketpair, the other end pumped to the server by vento.
    Unix(UnixStream),
}

impl Socket {
    /// A second descriptor for the same socket.
    pub fn try_clone(&self) -> io::Result<Socket> {
        Ok(match self {
            Socket::Tcp(s) => Socket::Tcp(s.try_clone()?),
            Socket::Unix(s) => Socket::Unix(s.try_clone()?),
        })
    }

    /// Ends one or both directions. `Both` wakes a thread blocked reading
    /// it, on either kind, which is how a session is hung up.
    pub fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        match self {
            Socket::Tcp(s) => s.shutdown(how),
            Socket::Unix(s) => s.shutdown(how),
        }
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        match self {
            Socket::Tcp(s) => s.set_read_timeout(timeout),
            Socket::Unix(s) => s.set_read_timeout(timeout),
        }
    }

    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        match self {
            Socket::Tcp(s) => s.set_write_timeout(timeout),
            Socket::Unix(s) => s.set_write_timeout(timeout),
        }
    }

    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        match self {
            Socket::Tcp(s) => s.set_nonblocking(nonblocking),
            Socket::Unix(s) => s.set_nonblocking(nonblocking),
        }
    }

    /// `TCP_NODELAY` on a TCP socket. Nothing on a socketpair, which has no
    /// Nagle delay to turn off and refuses the option (`EOPNOTSUPP`); the
    /// relay behind it sets the option on its own TCP socket.
    pub fn set_nodelay(&self, nodelay: bool) -> io::Result<()> {
        match self {
            Socket::Tcp(s) => s.set_nodelay(nodelay),
            Socket::Unix(_) => Ok(()),
        }
    }

    /// What the next read would return, left to be read: `TcpStream::peek`,
    /// for both kinds (std has it only for TCP). Waits as a read does, up
    /// to the read timeout.
    pub fn peek(&self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            // SAFETY: the buffer outlives the call, and recv writes at most
            // its length.
            let n = unsafe { libc::recv(self.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len(), libc::MSG_PEEK) };
            if n >= 0 {
                return Ok(n as usize);
            }
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::Interrupted {
                return Err(e);
            }
        }
    }

    /// The socket's pending error (`SO_ERROR`), which both kinds keep.
    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        match self {
            Socket::Tcp(s) => s.take_error(),
            Socket::Unix(s) => s.take_error(),
        }
    }

    /// The TCP socket, where this is one.
    pub fn as_tcp(&self) -> Option<&TcpStream> {
        match self {
            Socket::Tcp(s) => Some(s),
            Socket::Unix(_) => None,
        }
    }
}

impl From<TcpStream> for Socket {
    fn from(s: TcpStream) -> Socket {
        Socket::Tcp(s)
    }
}

impl From<UnixStream> for Socket {
    fn from(s: UnixStream) -> Socket {
        Socket::Unix(s)
    }
}

impl From<Socket> for OwnedFd {
    fn from(s: Socket) -> OwnedFd {
        match s {
            Socket::Tcp(s) => s.into(),
            Socket::Unix(s) => s.into(),
        }
    }
}

impl AsFd for Socket {
    fn as_fd(&self) -> BorrowedFd<'_> {
        match self {
            Socket::Tcp(s) => s.as_fd(),
            Socket::Unix(s) => s.as_fd(),
        }
    }
}

impl AsRawFd for Socket {
    fn as_raw_fd(&self) -> RawFd {
        self.as_fd().as_raw_fd()
    }
}

impl fmt::Debug for Socket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Socket::Tcp(s) => s.fmt(f),
            Socket::Unix(s) => s.fmt(f),
        }
    }
}

impl Read for &Socket {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Socket::Tcp(s) => (&*s).read(buf),
            Socket::Unix(s) => (&*s).read(buf),
        }
    }

    fn read_vectored(&mut self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        match self {
            Socket::Tcp(s) => (&*s).read_vectored(bufs),
            Socket::Unix(s) => (&*s).read_vectored(bufs),
        }
    }
}

impl Write for &Socket {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Socket::Tcp(s) => (&*s).write(buf),
            Socket::Unix(s) => (&*s).write(buf),
        }
    }

    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        match self {
            Socket::Tcp(s) => (&*s).write_vectored(bufs),
            Socket::Unix(s) => (&*s).write_vectored(bufs),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Read for Socket {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        (&*self).read(buf)
    }

    fn read_vectored(&mut self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        (&*self).read_vectored(bufs)
    }
}

impl Write for Socket {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        (&*self).write(buf)
    }

    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        (&*self).write_vectored(bufs)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
