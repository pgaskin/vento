//! Why a session ended, in the words the app puts on the screen.

use std::fmt;

#[derive(Debug)]
pub enum Error {
    /// The socket, at any point, on any of the four connections.
    Io(std::io::Error),
    /// The ticket was refused. SPICE's own auth has one answer and no
    /// message, so what this carries is the link error's name.
    Refused(String),
    /// Nobody answered the password prompt, or the certificate was not
    /// accepted.
    Cancelled,
    /// The bytes did not mean what they have to mean.
    Protocol(String),
    /// The server said so: `MAIN_DISCONNECTING`, or a `NOTIFY` at error
    /// severity, in its own words.
    Server(String),
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
            Error::Server(why) if why.is_empty() => write!(f, "The server ended the session."),
            Error::Server(why) => write!(f, "{why}"),
            Error::Closed => write!(f, "Disconnected"),
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Error {
        Error::Io(e)
    }
}
