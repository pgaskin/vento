//! Why a session ended, in the words the app puts on the screen.

use std::fmt;

#[derive(Debug)]
pub enum Error {
    /// The socket, at any point.
    Io(std::io::Error),
    /// The server said no before authentication — "too many attempts", usually.
    Refused(String),
    /// The credentials were wrong.
    AuthFailed(String),
    /// Nobody answered the password prompt.
    Cancelled,
    /// The server's certificate was not the one this address is pinned to, or
    /// nobody vouched for a new one.
    Untrusted,
    /// Something in this client is not implemented, and the server needs it.
    Unsupported(String),
    /// The bytes did not mean what they have to mean.
    Protocol(String),
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
                std::io::ErrorKind::UnexpectedEof => write!(f, "The server closed the connection."),
                _ => write!(f, "{e}"),
            },
            Error::Refused(why) if why.is_empty() => write!(f, "The server refused the connection."),
            Error::Refused(why) => write!(f, "{why}"),
            Error::AuthFailed(why) => write!(f, "{why}"),
            Error::Cancelled => write!(f, "Cancelled."),
            Error::Untrusted => write!(f, "The server's certificate was not accepted."),
            Error::Unsupported(what) => write!(f, "Unsupported: {what}"),
            Error::Protocol(what) => write!(f, "The server is not making sense: {what}"),
            Error::Closed => write!(f, "Disconnected"),
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Error {
        Error::Io(e)
    }
}
