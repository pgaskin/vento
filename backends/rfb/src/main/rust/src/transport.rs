//! The socket, and the TLS that VeNCrypt may end up wrapping around it.
//!
//! Everything above this reads and writes through [`Wire`], which is a pair of
//! boxed halves rather than a `TcpStream`: the same code drives an encrypted
//! session and a plain one, and the swap happens in the middle of the security
//! handshake, which is where VeNCrypt puts it.
//!
//! ## Two threads, one TLS session
//!
//! Which is `common::tls`, where the locking argument is written down: RDP
//! reached the same problem and wanted the same answer. What stays here is the
//! upgrade itself, because VeNCrypt's is unlike anybody else's.
//!
//! ## What is trusted
//!
//! Nothing, by a certificate authority — that argument and the verifier that
//! implements it are `common::pinning`, because RDP reaches the same conclusion
//! about the same problem. What is left here is *where* the question is asked:
//! after the handshake and before a byte of the sub-authentication, which is
//! where the password would go. A completed TLS handshake carries nothing
//! secret, which is what keeps the decision on a thread that can wait for a
//! person rather than inside a verifier callback that cannot.

use crate::error::{Error, Result};
use crate::proto::{Counter, Reader, Writer};
use rustls::ClientConnection;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::sync::Arc;

pub type Reads = Box<dyn Read + Send>;
pub type Writes = Box<dyn Write + Send>;

/// What the far end turned out to be, once TLS is up.
pub struct Peer {
    /// The leaf certificate's SHA-256, in openssl's own `AB:CD:…` form so that
    /// it can be compared against something that is not us.
    pub fingerprint: String,
    /// For the connection panel: `TLS 1.3 · TLS13_AES_128_GCM_SHA256`.
    pub description: String,
}

/// The connection, at whatever layer it currently has.
pub struct Wire {
    pub r: Reader<Reads>,
    pub w: Writer<Writes>,
    /// The socket underneath, kept for the upgrade and for shutdown.
    socket: TcpStream,
    peer: Option<Peer>,
}

impl Wire {
    pub fn plain(socket: TcpStream, received: Counter, sent: Counter) -> io::Result<Wire> {
        Ok(Wire {
            r: Reader::counting(Box::new(socket.try_clone()?) as Reads, received),
            w: Writer::counting(Box::new(socket.try_clone()?) as Writes, sent),
            socket,
            peer: None,
        })
    }

    pub fn peer(&self) -> Option<&Peer> {
        self.peer.as_ref()
    }

    /// Wrap the connection in TLS, in place.
    ///
    /// `server_name` is what rustls insists on having; nothing here checks it,
    /// because the name in a self-signed VNC certificate is whatever the server
    /// felt like writing and the pin is the identity. A name that is not a valid
    /// DNS name or address — an address book entry, say — becomes a placeholder
    /// rather than an error.
    ///
    /// `after_client_hello` is read from the socket **unbuffered**, between our
    /// ClientHello and the rest of the handshake, and exists for one reason:
    /// VeNCrypt has a plain byte there that a real server does not send until
    /// it has our ClientHello in hand (see `security::vencrypt`). Whatever it
    /// reads it must read exactly, because everything after it is a TLS record.
    pub fn start_tls(
        &mut self,
        server_name: &str,
        after_client_hello: &mut dyn FnMut(&mut dyn Read) -> Result<()>,
    ) -> Result<()> {
        // Anything the server sent before the upgrade would be encrypted-stream
        // bytes read as clear text, and the far end cannot have sent any: it is
        // waiting for our ClientHello. Checking is one line and turns a
        // desynchronised stream into a sentence.
        if self.r.buffered() != 0 {
            return Err(Error::Protocol(format!(
                "{} bytes arrived before the TLS handshake",
                self.r.buffered()
            )));
        }

        let (config, verifier) = common::pinning::client_config().map_err(tls_error)?;
        let name = common::pinning::server_name(server_name);
        let mut conn = ClientConnection::new(Arc::new(config), name).map_err(tls_error)?;

        // The handshake runs on the raw socket, which still carries the
        // connect deadline: a server that opens a TCP connection and then says
        // nothing must not be a session that hangs for ever.
        //
        // Write first, then read. `complete_io` would do both, and cannot be
        // used until whatever is still owed in clear text has been taken off
        // the stream — which for VeNCrypt is a byte the server will not part
        // with until it has seen the ClientHello.
        let mut socket = self.socket.try_clone()?;
        while conn.wants_write() {
            conn.write_tls(&mut socket)?;
        }
        socket.flush()?;
        after_client_hello(&mut socket)?;
        conn.complete_io(&mut socket)?;

        let fingerprint = verifier
            .fingerprint()
            .ok_or_else(|| Error::Protocol("the server sent no certificate".into()))?;
        let description = common::pinning::describe(conn.protocol_version(), conn.negotiated_cipher_suite());

        // The counters come across with the streams they were counting: this
        // is the same session, and the handshake's own records are the only
        // bytes of it nothing here sees.
        let handle = common::tls::Handle::new(conn, &self.socket)?;
        self.r = Reader::counting(Box::new(handle.clone_handle()) as Reads, self.r.counter());
        self.w = Writer::counting(Box::new(handle) as Writes, self.w.counter());
        self.peer = Some(Peer {
            fingerprint,
            description,
        });
        Ok(())
    }
}

fn tls_error(e: rustls::Error) -> Error {
    Error::Protocol(format!("TLS: {e}"))
}
