// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! Why a session ended, in the words the app puts on the screen.

use std::fmt;

#[derive(Debug)]
pub enum Error {
    /// The socket, at any point.
    Io(std::io::Error),
    /// The peer refused the login, in its own words.
    Refused(String),
    /// Nobody answered the password prompt.
    Cancelled,
    /// The bytes did not mean what they have to mean.
    Protocol(String),
    /// What somebody typed is not an address, said in the words they need.
    Address(String),
    /// The peer stopped saying anything at all. Their server sends a
    /// `TestDelay` about once a second whatever else is happening, so silence
    /// for as long as this takes is a connection that has gone rather than a
    /// desktop nobody is touching.
    Silent,
    /// `Client::close` was called; the only ordinary ending.
    Closed,
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // A dropped connection is the common case and the message is what
            // somebody reads on a screen, so it says what happened rather than
            // naming an errno.
            Error::Io(e) => match e.kind() {
                std::io::ErrorKind::ConnectionRefused => {
                    write!(f, "Nothing is listening at that address.")
                }
                std::io::ErrorKind::TimedOut => write!(f, "The connection timed out."),
                std::io::ErrorKind::UnexpectedEof => write!(f, "The peer closed the connection."),
                _ => write!(f, "{e}"),
            },
            Error::Refused(why) if why.is_empty() => write!(f, "The peer refused the connection."),
            Error::Refused(why) => write!(f, "{why}"),
            Error::Cancelled => write!(f, "Cancelled."),
            Error::Protocol(what) => write!(f, "The peer is not making sense: {what}"),
            Error::Address(why) => write!(f, "{why}"),
            Error::Silent => write!(f, "The connection stopped responding."),
            Error::Closed => write!(f, "Disconnected"),
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Error {
        Error::Io(e)
    }
}
