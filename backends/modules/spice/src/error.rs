// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! Why a session ended, in the words the app puts on the screen.

use std::fmt;

#[derive(Debug)]
pub enum Error {
    /// The socket, at any point, on any of the session's connections.
    Io(std::io::Error),
    /// The ticket was refused. SPICE's own auth has one answer and no
    /// message, so what this carries is the link error's name.
    Refused(String),
    /// The password prompt was dismissed, or the certificate was not
    /// accepted.
    Cancelled,
    /// The server sent malformed or unexpected data.
    Protocol(String),
    /// The configured address is not valid; the text says why.
    Address(String),
    /// The server ended the session: `MAIN_DISCONNECTING`, or a required
    /// channel closed without an error.
    Server,
    /// `Client::close` was called; the only ordinary ending.
    Closed,
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => match e.kind() {
                std::io::ErrorKind::ConnectionRefused => {
                    write!(f, "Nothing is listening at that address.")
                }
                std::io::ErrorKind::TimedOut => write!(f, "The connection timed out."),
                std::io::ErrorKind::UnexpectedEof => write!(f, "The server closed the connection."),
                _ => write!(f, "{e}"),
            },
            Error::Refused(_) => write!(f, "The password was refused."),
            Error::Cancelled => write!(f, "Cancelled."),
            Error::Protocol(what) => write!(f, "The server is not making sense: {what}"),
            Error::Address(why) => write!(f, "{why}"),
            Error::Server => write!(f, "The server ended the session."),
            Error::Closed => write!(f, "Disconnected"),
        }
    }
}

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

impl From<shakenfist_spice_protocol::reader::LinkError> for Error {
    fn from(e: shakenfist_spice_protocol::reader::LinkError) -> Error {
        Error::Protocol(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::net::{End, EndReason};

    /// A relayed connection's end of stream reads as the route recorded
    /// it: a reset as a direct socket's reset, a close as a close.
    #[test]
    fn a_relayed_end_is_worded_as_a_direct_one() {
        let eof = || Error::Io(std::io::ErrorKind::UnexpectedEof.into());
        let reset = EndReason::new();
        reset.record(End::Reset);
        assert_eq!(eof().explained(&reset).to_string(), "Connection reset by peer (os error 104)");
        let closed = EndReason::new();
        closed.record(End::Closed);
        assert_eq!(eof().explained(&closed).to_string(), "The server closed the connection.");
        assert_eq!(eof().explained(&EndReason::none()).to_string(), "The server closed the connection.");
    }
}
