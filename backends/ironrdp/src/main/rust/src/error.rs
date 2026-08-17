// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! What can go wrong, and what the person holding the phone is told about it.

use std::fmt;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    /// The far end did something the protocol does not allow, or IronRDP
    /// refused to go on. The string is what reaches the screen.
    Protocol(String),
    /// Refused on purpose: no TLS on offer, or a certificate nobody accepted.
    Refused(String),
    /// [`crate::Client::close`] was called. Not a failure, and the app already
    /// knows.
    Closed,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // Word for word `rfb::Error`'s, because what a person reads when a
            // server goes away should not depend on which protocol reached it.
            // The eof case is the one that matters: a server that vanishes
            // leaves the frame reader mid-PDU, and its own words for that are
            // "not enough bytes".
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

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Error {
        Error::Io(e)
    }
}

/// IronRDP's errors nest a source chain that says the useful part — "invalid
/// credentials", "the server closed the connection" — one or two levels down,
/// so the whole chain goes on screen rather than the outermost frame's "active
/// stage".
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
