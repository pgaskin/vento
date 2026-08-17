//! The TLS the RDP connection sequence upgrades into, and the two threads that
//! share it.
//!
//! RDP has no unencrypted mode worth having — the legacy "standard RDP
//! security" is RC4 with a key the protocol hands over — so unlike VeNCrypt
//! this is not an option, it is the transport. What is the same as the RFB
//! client is everything around it: the certificate is pinned rather than
//! verified (`tls::pinning`), the question is asked on the protocol thread after the
//! handshake and before anything secret goes out, and one TLS session has to be
//! reachable from a reader and a writer at once.
//!
//! That last part is not this protocol's problem to solve: sharing one session
//! between a reader and a writer is `tls::stream`, which is where the locking
//! argument is written down. What is left here is the handshake and what it
//! yields.

use crate::error::{Error, Result};
use rustls::ClientConnection;
use rustls::pki_types::CertificateDer;
use std::net::TcpStream;
use std::sync::Arc;

/// What the far end turned out to be, once the handshake is done.
pub struct Peer {
    /// SHA-256 of the leaf certificate, in `openssl x509 -fingerprint`'s form.
    pub fingerprint: String,
    /// For the connection panel: `TLSv1_3 · TLS13_AES_256_GCM_SHA384`.
    pub description: String,
    /// The leaf certificate's SubjectPublicKeyInfo bit string, which is what
    /// CredSSP binds its authentication to. Empty when it could not be parsed —
    /// which only matters if the server asks for CredSSP, and is a better
    /// failure there than here.
    pub public_key: Vec<u8>,
}

/// One thread's view of the TLS session, which is `tls::stream`' — the shape is
/// the same in both protocols and the argument for it is written down there.
pub use tls::stream::Handle;

/// Do the handshake on `socket`, which must have nothing outstanding on it.
///
/// `server_name` is only what rustls insists on having: nothing checks it,
/// because the pin is the identity (`tls::pinning`).
pub fn connect(socket: &TcpStream, server_name: &str) -> Result<(Handle, Peer)> {
    let (config, verifier) = tls::pinning::client_config().map_err(tls_error)?;
    let name = tls::pinning::server_name(server_name);
    let mut conn = ClientConnection::new(Arc::new(config), name).map_err(tls_error)?;

    // The handshake runs on the raw socket, which still carries the connect
    // deadline: a server that opens a TCP connection and then says nothing must
    // not be a session that hangs for ever.
    let mut raw = socket.try_clone()?;
    conn.complete_io(&mut raw)?;

    let certificate = verifier
        .certificate()
        .ok_or_else(|| Error::Protocol("the server sent no certificate".into()))?;
    let peer = Peer {
        fingerprint: tls::pinning::fingerprint(&certificate),
        description: tls::pinning::describe(conn.protocol_version(), conn.negotiated_cipher_suite()),
        public_key: public_key(&certificate),
    };

    Ok((Handle::new(conn, socket)?, peer))
}

/// The SubjectPublicKeyInfo's bit string, which is what CredSSP's `pubKeyAuth`
/// is computed over — the binding that stops a proxy relaying an
/// authentication it cannot decrypt to a server it does not own.
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
