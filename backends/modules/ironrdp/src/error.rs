// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The client's errors, and the messages shown for them.

use std::fmt;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    /// The far end violated the protocol, or IronRDP refused to continue.
    /// The string is shown on screen.
    Protocol(String),
    /// Refused deliberately: no TLS offered, a certificate that was not
    /// accepted, or credentials the server refused.
    Refused(String),
    /// [`crate::Client::close`] was called. Not a failure, and the app
    /// initiated it.
    Closed,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // The same text as `rfb::Error`'s, so that the message for a lost
            // server does not depend on the protocol. The EOF case matters
            // most: a server that vanishes leaves the frame reader mid-PDU,
            // and IronRDP's own message for that is "not enough bytes".
            Error::Io(e) => match e.kind() {
                std::io::ErrorKind::ConnectionRefused => {
                    write!(f, "Nothing is listening at that address.")
                }
                std::io::ErrorKind::TimedOut => write!(f, "The connection timed out."),
                std::io::ErrorKind::UnexpectedEof
                | std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::ConnectionAborted
                | std::io::ErrorKind::BrokenPipe => {
                    write!(f, "The server closed the connection.")
                }
                _ => write!(f, "{e}"),
            },
            Error::Protocol(m) => write!(f, "{m}"),
            Error::Refused(m) => write!(f, "{m}"),
            Error::Closed => write!(f, "disconnected"),
        }
    }
}

impl std::error::Error for Error {}

impl Error {
    /// This error, with a socket error that came from the stream ending
    /// replaced by what the connection's route says ended it
    /// ([`runtime::net::EndReason::explain`]).
    pub fn explained(self, end: &runtime::net::EndReason) -> Error {
        match self {
            Error::Io(e) => Error::Io(end.explain(e)),
            other => other,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Error {
        Error::Io(e)
    }
}

/// IronRDP's errors carry the useful part ("invalid credentials", "the server
/// closed the connection") one or two levels down their source chain, so the
/// whole chain is shown rather than only the outermost "active stage".
pub fn describe(e: &dyn std::error::Error) -> String {
    let mut out = e.to_string();
    let mut source = e.source();
    while let Some(inner) = source {
        let text = inner.to_string();
        if !out.contains(&text) {
            out.push_str(": ");
            out.push_str(&text);
        }
        source = inner.source();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::net::{End, EndReason};

    /// A relayed connection's end of stream reads as the route recorded
    /// it: IronRDP words a reset as a close, either way.
    #[test]
    fn a_relayed_end_is_worded_as_a_direct_one() {
        let eof = || Error::Io(std::io::ErrorKind::UnexpectedEof.into());
        let reset = EndReason::new();
        reset.record(End::Reset);
        assert_eq!(eof().explained(&reset).to_string(), "The server closed the connection.");
        let closed = EndReason::new();
        closed.record(End::Closed);
        assert_eq!(eof().explained(&closed).to_string(), "The server closed the connection.");
        assert_eq!(eof().explained(&EndReason::none()).to_string(), "The server closed the connection.");
    }
}
