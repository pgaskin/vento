//! The TLS port, pinned rather than verified.
//!
//! `remotedesktop-tls`'s verifier, which is the same one the RFB and RDP
//! clients use and the whole of what "trusted" means in this app: nothing
//! vouches for a hypervisor's certificate, so the identity is the key and the
//! app remembers it per address. What is different here is only that the
//! handshake is async, so it is `tokio-rustls` rather than the blocking
//! connection that crate's `stream` module wraps.
//!
//! **A plain SPICE port and a TLS SPICE port are different ports**, not a
//! negotiation, so whether a session is encrypted is a fact about what somebody
//! typed rather than about what the server offered. It matters more here than
//! elsewhere: on the plain port the ticket is encrypted to a key the server
//! generates per connection and hands over in the clear immediately before, so
//! anyone on the path reads the password.

use crate::error::{Error, Result};
use std::sync::{Arc, Mutex};
use tls::pinning::{self, Pinning};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

/// A configured connector, and the verifier to ask afterwards what it saw.
pub struct Connector {
    connector: TlsConnector,
    host: String,
    verifier: Arc<Pinning>,
    /// What the handshake settled on, for the panel's Connection row. Taken
    /// off the connection while it is still in hand, because it goes into the
    /// channel's stream immediately afterwards.
    negotiated: Mutex<Option<String>>,
}

impl Connector {
    pub fn new(host: &str) -> Result<Connector> {
        // The protocol crate takes its signature algorithms from the
        // process-wide provider rather than naming one, which is what its patch
        // is about — so this install is what decides that both halves of the
        // session use `ring`.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let (config, verifier) =
            pinning::client_config().map_err(|e| Error::Protocol(e.to_string()))?;
        Ok(Connector {
            connector: TlsConnector::from(Arc::new(config)),
            host: host.to_string(),
            verifier,
            negotiated: Mutex::new(None),
        })
    }

    pub async fn connect(&self, tcp: TcpStream) -> Result<TlsStream<TcpStream>> {
        let name = pinning::server_name(&self.host);
        let stream = self.connector.connect(name, tcp).await?;
        let (_, conn) = stream.get_ref();
        *self.negotiated.lock().unwrap() = Some(pinning::describe(
            conn.protocol_version(),
            conn.negotiated_cipher_suite(),
        ));
        Ok(stream)
    }

    /// The leaf certificate's SHA-256, once a handshake has presented one. What
    /// the app pins, and what a person is asked about when it changes.
    pub fn fingerprint(&self) -> Option<String> {
        self.verifier.fingerprint()
    }

    /// `TLSv1_3 · TLS13_AES_256_GCM_SHA384`, for the panel.
    pub fn negotiated(&self) -> Option<String> {
        self.negotiated.lock().unwrap().clone()
    }
}
