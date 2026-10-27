// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The TLS the RDP connection sequence upgrades into, and the two threads that
//! share it.
//!
//! RDP has no usable mode without TLS (the legacy "standard RDP security" is
//! RC4 with a key exchanged by the protocol itself), so unlike VeNCrypt this
//! is not an option but the transport. Everything around it matches the RFB
//! client: the certificate is pinned rather than verified
//! (`runtime::tls::pinning`), the user is asked on the protocol thread after
//! the handshake and before anything secret is sent, and one TLS session has
//! to be shared by a reader and a writer.
//!
//! Sharing one session between a reader and a writer is handled by
//! `runtime::tls::stream`, which documents the locking. This module does only
//! the handshake and returns its results.

use crate::error::{Error, Result};
use runtime::net::Socket;
use rustls::ClientConnection;
use rustls::pki_types::CertificateDer;
use std::sync::Arc;

/// The far end's identity, from the completed handshake.
pub struct Peer {
    /// SHA-256 of the leaf certificate, in `openssl x509 -fingerprint`'s form.
    pub fingerprint: String,
    /// For the connection panel: `TLSv1_3 · TLS13_AES_256_GCM_SHA384`.
    pub description: String,
    /// The leaf certificate's SubjectPublicKeyInfo bit string, which CredSSP
    /// binds its authentication to. Empty when it could not be parsed, which
    /// only matters if the server asks for CredSSP, and is better reported as
    /// a failure there than here.
    pub public_key: Vec<u8>,
}

/// One thread's handle on the TLS session, from `runtime::tls::stream`: the
/// same in both protocols, and documented there.
pub use runtime::tls::stream::Handle;

/// Do the handshake on `socket`, which must have nothing outstanding on it.
///
/// `server_name` is only required by rustls: nothing checks it, because the
/// pin is the identity (`runtime::tls::pinning`).
pub fn connect(socket: &Socket, server_name: &str) -> Result<(Handle, Peer)> {
    let (config, verifier) = runtime::tls::pinning::client_config().map_err(tls_error)?;
    let name = runtime::tls::pinning::server_name(server_name);
    let mut conn = ClientConnection::new(Arc::new(config), name).map_err(tls_error)?;

    // The handshake runs on the raw socket, which still has the connect
    // deadline: a server that accepts a TCP connection and then sends nothing
    // must not leave the session hanging forever.
    let mut raw = socket.try_clone()?;
    conn.complete_io(&mut raw)?;

    let certificate = verifier
        .certificate()
        .ok_or_else(|| Error::Protocol("the server sent no certificate".into()))?;
    let peer = Peer {
        fingerprint: runtime::tls::pinning::fingerprint(&certificate),
        description: runtime::tls::pinning::describe(conn.protocol_version(), conn.negotiated_cipher_suite()),
        public_key: public_key(&certificate),
    };

    Ok((Handle::new(conn, socket)?, peer))
}

/// The SubjectPublicKeyInfo's bit string, which CredSSP's `pubKeyAuth` is
/// computed over: the binding that stops a proxy relaying an authentication
/// it cannot decrypt to a server it does not own.
fn public_key(cert: &CertificateDer<'_>) -> Vec<u8> {
    use x509_cert::der::Decode as _;
    match x509_cert::Certificate::from_der(cert.as_ref()) {
        Ok(cert) => cert
            .tbs_certificate
            .subject_public_key_info
            .subject_public_key
            .as_bytes()
            .unwrap_or_default()
            .to_vec(),
        Err(e) => {
            log::warn!("the server's certificate did not parse ({e}); CredSSP will fail");
            Vec::new()
        }
    }
}

fn tls_error(e: rustls::Error) -> Error {
    Error::Protocol(format!("TLS: {e}"))
}
