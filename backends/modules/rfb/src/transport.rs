// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The socket, and the TLS VeNCrypt may wrap around it.
//!
//! Everything above this reads and writes through [`Wire`], a pair of boxed
//! halves rather than a socket: the same code drives an encrypted session
//! and a plain one, and the swap happens in the middle of the security
//! handshake, where VeNCrypt puts it.
//!
//! ## Two threads, one TLS session
//!
//! That is `runtime::tls::stream`, which documents the locking: RDP had the
//! same problem and needed the same solution. The upgrade itself stays here,
//! because VeNCrypt's is unlike any other protocol's.
//!
//! ## What is trusted
//!
//! Nothing, by a certificate authority: that reasoning and the verifier that
//! implements it are in `runtime::tls::pinning`, because RDP has the same
//! problem. What is left here is *where* the check happens: after the
//! handshake and before any byte of the sub-authentication, which is where the
//! password would go. A completed TLS handshake carries nothing secret, which
//! lets the decision happen on a thread that can wait for the user rather than
//! inside a verifier callback that can't.

use crate::error::{Error, Result};
use crate::proto::{Counter, Reader, Writer};
use runtime::net::Socket;
use rustls::ClientConnection;
use std::io::{self, Read, Write};
use std::sync::Arc;

pub type Reads = Box<dyn Read + Send>;
pub type Writes = Box<dyn Write + Send>;

/// The far end's identity, once TLS is up.
pub struct Peer {
    /// The leaf certificate's SHA-256, in openssl's `AB:CD:…` form so it can
    /// be compared against other tools' output.
    pub fingerprint: String,
    /// For the connection panel: `TLS 1.3 · TLS13_AES_128_GCM_SHA256`.
    pub description: String,
}

/// The connection, at its current layer.
pub struct Wire {
    pub r: Reader<Reads>,
    pub w: Writer<Writes>,
    /// The socket underneath, kept for the upgrade and for shutdown.
    socket: Socket,
    peer: Option<Peer>,
}

impl Wire {
    pub fn plain(socket: Socket, received: Counter, sent: Counter) -> io::Result<Wire> {
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

    /// Wraps the connection in TLS, in place.
    ///
    /// `server_name` is required by rustls; nothing here checks it, because the
    /// name in a self-signed VNC certificate is arbitrary and the pin is the
    /// identity. A name that is not a valid DNS name or address (an address
    /// book entry, say) becomes a placeholder rather than an error.
    ///
    /// `after_client_hello` is read from the socket **unbuffered**, between our
    /// ClientHello and the rest of the handshake, for one reason: VeNCrypt has
    /// a plain byte there that a real server doesn't send until it has received
    /// our ClientHello (see `security::vencrypt`). It must read exactly that
    /// much, because everything after it is a TLS record.
    pub fn start_tls(
        &mut self,
        server_name: &str,
        after_client_hello: &mut dyn FnMut(&mut dyn Read) -> Result<()>,
    ) -> Result<()> {
        // Anything the server sent before the upgrade would be encrypted bytes
        // read as plain text, and the far end can't have sent any: it is
        // waiting for our ClientHello. The check is one line and turns a
        // desynchronised stream into a clear error.
        if self.r.buffered() != 0 {
            return Err(Error::Protocol(format!(
                "{} bytes arrived before the TLS handshake",
                self.r.buffered()
            )));
        }

        let (config, verifier) = runtime::tls::pinning::client_config().map_err(tls_error)?;
        let name = runtime::tls::pinning::server_name(server_name);
        let mut conn = ClientConnection::new(Arc::new(config), name).map_err(tls_error)?;

        // The handshake runs on the raw socket, which still has the connect
        // deadline: a server that opens a TCP connection and then sends
        // nothing must not hang the session forever.
        //
        // Write first, then read. `complete_io` would do both, but can't be
        // used until any remaining plain-text bytes have been read from the
        // stream, which for VeNCrypt is a byte the server withholds until it
        // has seen the ClientHello.
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
        let description = runtime::tls::pinning::describe(conn.protocol_version(), conn.negotiated_cipher_suite());

        // The counters move with the streams they were counting: this is the
        // same session, and the handshake's own records are the only bytes of
        // it not counted here.
        let handle = runtime::tls::stream::Handle::new(conn, &self.socket)?;
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
