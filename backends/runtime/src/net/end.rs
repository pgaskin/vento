// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! Why a connection ended, as the route saw it.
//!
//! On a direct route the socket itself says: a reset is `ECONNRESET` on the
//! next read, a dead server `ETIMEDOUT`. On a relayed route (a tunnel, or
//! [`Socketpair`](super::Socketpair)) the backend's socket is a socketpair's
//! end, which can only end its stream: whatever happened to the connection
//! behind it (a reset, a timeout, the tunnel's own failure) reads as the end
//! of the stream. So the route records it here, before it ends the
//! backend's stream, and a backend that sees its stream end or fail asks
//! ([`EndReason::explain`]) and reports what it is told, so that a reset
//! through a tunnel reads as a reset does on a direct connection, and a
//! tunnel's own error is shown in its own words.

use std::fmt;
use std::io;
use std::sync::{Arc, OnceLock};

/// How a connection ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum End {
    /// The server closed its side: the end of its stream (a FIN).
    Closed,
    /// The server reset the connection.
    Reset,
    /// The connection to the server stopped answering: retransmissions,
    /// keep-alive probes or `TCP_USER_TIMEOUT` ran out.
    TimedOut,
    /// Another error on the connection to the server, as the OS's error
    /// number (`EHOSTUNREACH` from an ICMP error, for example).
    Os(i32),
    /// The route itself failed (a tunnel's transport or its far end), in the
    /// route's own words, shown to the user as they are. Only a route says
    /// this, about itself: it records it, or fails with a [`TunnelFailed`]
    /// error, which [`End::from_error`] maps here. No other error does.
    Tunnel(String),
    /// This end ended it: the backend closed its socket or hung up.
    Local,
}

impl End {
    /// What the server's side of a connection ending with `e` was. An error
    /// with no OS error number is the route's own failure only where it is
    /// a [`TunnelFailed`]; anything else of that kind is an `EIO`, so that a
    /// relay's own fallback text never reaches the user as a tunnel's words.
    pub fn from_error(e: &io::Error) -> End {
        if let Some(n) = e.raw_os_error() {
            return match n {
                libc::ECONNRESET => End::Reset,
                libc::ETIMEDOUT => End::TimedOut,
                n => End::Os(n),
            };
        }
        if let Some(TunnelFailed(text)) = e.get_ref().and_then(|inner| inner.downcast_ref::<TunnelFailed>()) {
            return End::Tunnel(text.clone());
        }
        match e.kind() {
            io::ErrorKind::ConnectionReset => End::Reset,
            io::ErrorKind::TimedOut => End::TimedOut,
            _ => End::Os(libc::EIO),
        }
    }

    /// The error a read on a direct TCP socket would have given for this
    /// ending, or `None` where it would have been the end of the stream
    /// ([`Closed`](End::Closed)) or where this end ended it.
    pub fn error(&self) -> Option<io::Error> {
        match self {
            End::Closed | End::Local => None,
            End::Reset => Some(io::Error::from_raw_os_error(libc::ECONNRESET)),
            End::TimedOut => Some(io::Error::from_raw_os_error(libc::ETIMEDOUT)),
            End::Os(n) => Some(io::Error::from_raw_os_error(*n)),
            End::Tunnel(text) => Some(io::Error::other(TunnelFailed(text.clone()))),
        }
    }
}

impl End {
    /// The errno a read on a direct TCP socket would have failed with:
    /// `ECONNRESET`, `ETIMEDOUT`, the error itself, `ECONNABORTED` for the
    /// route's own failure, and 0 where it would have read the end of the
    /// stream.
    pub fn errno(&self) -> i32 {
        match self {
            End::Closed | End::Local => 0,
            End::Reset => libc::ECONNRESET,
            End::TimedOut => libc::ETIMEDOUT,
            End::Os(n) => *n,
            End::Tunnel(_) => libc::ECONNABORTED,
        }
    }
}

/// A route's own failure as an error: its text, as it is.
#[derive(Debug)]
pub struct TunnelFailed(pub String);

impl fmt::Display for TunnelFailed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TunnelFailed {}

/// Where a connection's [`End`] is recorded: shared by the route, which
/// records it, and the connection's holder, which asks. Cheap to clone.
///
/// A route that hands over its own TCP socket records nothing
/// ([`EndReason::none`]): the socket says itself. A route that carries the
/// bytes records the first thing that ends the connection, and does so
/// before the backend's end of it can see that it has ended, so that an
/// answer is there as soon as the backend reads the end of its stream.
#[derive(Clone, Debug, Default)]
pub struct EndReason(Option<Arc<OnceLock<End>>>);

impl EndReason {
    /// For a connection whose socket reports its own end.
    pub fn none() -> EndReason {
        EndReason(None)
    }

    /// For a connection whose route will [`record`](EndReason::record) its
    /// end.
    pub fn new() -> EndReason {
        EndReason(Some(Arc::new(OnceLock::new())))
    }

    /// Records how the connection ended. The first record stands: what ended
    /// it first is what the backend saw.
    pub fn record(&self, end: End) {
        if let Some(slot) = &self.0 {
            let _ = slot.set(end);
        }
    }

    /// How the connection ended, once the route knows.
    pub fn get(&self) -> Option<End> {
        self.0.as_ref().and_then(|slot| slot.get().cloned())
    }

    /// `seen`, an error a backend's read or write gave when its stream ended
    /// (`UnexpectedEof` for the end of the stream where the protocol expected
    /// more; a reset, an aborted connection or a broken pipe), replaced by
    /// the error a direct connection would have given, where the route
    /// recorded one. Any other error is the backend's own and comes back as
    /// it was.
    pub fn explain(&self, seen: io::Error) -> io::Error {
        use io::ErrorKind::*;
        if !matches!(
            seen.kind(),
            UnexpectedEof | ConnectionReset | ConnectionAborted | BrokenPipe | NotConnected
        ) {
            return seen;
        }
        self.get().and_then(|end| end.error()).unwrap_or(seen)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_end_recorded_stands() {
        let end = EndReason::new();
        assert_eq!(end.get(), None);
        end.record(End::Closed);
        end.record(End::Reset);
        assert_eq!(end.get(), Some(End::Closed));
        let none = EndReason::none();
        none.record(End::Reset);
        assert_eq!(none.get(), None);
    }

    #[test]
    fn an_end_explains_what_the_backend_saw_as_a_direct_socket_would() {
        let eof = || io::Error::from(io::ErrorKind::UnexpectedEof);
        let end = EndReason::new();
        end.record(End::Reset);
        let e = end.explain(eof());
        assert_eq!(e.raw_os_error(), Some(libc::ECONNRESET));
        assert_eq!(e.kind(), io::ErrorKind::ConnectionReset);
        // Errors that have nothing to do with the stream ending are the
        // backend's.
        let e = end.explain(io::Error::new(io::ErrorKind::InvalidData, "bad"));
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
        // The end of the stream stays one where the server closed it, and
        // where nothing is known.
        let closed = EndReason::new();
        closed.record(End::Closed);
        assert_eq!(closed.explain(eof()).kind(), io::ErrorKind::UnexpectedEof);
        assert_eq!(EndReason::new().explain(eof()).kind(), io::ErrorKind::UnexpectedEof);
        assert_eq!(EndReason::none().explain(eof()).kind(), io::ErrorKind::UnexpectedEof);
        let timed_out = EndReason::new();
        timed_out.record(End::TimedOut);
        assert_eq!(timed_out.explain(eof()).kind(), io::ErrorKind::TimedOut);
    }

    #[test]
    fn a_tunnels_error_keeps_its_words() {
        let end = EndReason::new();
        end.record(End::Tunnel("the SSH server closed the channel".into()));
        let e = end.explain(io::Error::from(io::ErrorKind::BrokenPipe));
        assert_eq!(e.to_string(), "the SSH server closed the channel");
        assert!(e.get_ref().is_some_and(|inner| inner.is::<TunnelFailed>()));
    }

    #[test]
    fn errors_map_to_ends() {
        assert_eq!(End::from_error(&io::Error::from_raw_os_error(libc::ECONNRESET)), End::Reset);
        assert_eq!(End::from_error(&io::Error::from_raw_os_error(libc::ETIMEDOUT)), End::TimedOut);
        assert_eq!(
            End::from_error(&io::Error::from_raw_os_error(libc::EHOSTUNREACH)),
            End::Os(libc::EHOSTUNREACH)
        );
        // Only a route's own failure is one.
        assert_eq!(End::from_error(&io::Error::other("lost")), End::Os(libc::EIO));
        assert_eq!(
            End::from_error(&io::Error::other(TunnelFailed("the channel closed".into()))),
            End::Tunnel("the channel closed".into())
        );
        assert_eq!(End::from_error(&io::ErrorKind::ConnectionReset.into()), End::Reset);
        assert_eq!(End::from_error(&io::ErrorKind::TimedOut.into()), End::TimedOut);
    }
}
