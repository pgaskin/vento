// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The TLS port, pinned rather than verified.
//!
//! `runtime::tls::pinning`'s verifier, the same one the RFB and RDP clients
//! use, and the whole of what "trusted" means in this app: no CA vouches for a
//! hypervisor's certificate, so the identity is the key, and the app remembers
//! it per address. The only difference here is that the handshake is async,
//! so it uses `tokio-rustls` rather than the blocking connection
//! `runtime::tls::stream` wraps.
//!
//! **A plain SPICE port and a TLS SPICE port are different ports**, not a
//! negotiation, so whether a session is encrypted depends on the connection's
//! settings rather than on what the server offered. It matters more here than
//! elsewhere: on the plain port the ticket is encrypted to a key the server
//! generates per connection and sends in the clear immediately before, so
//! anyone able to replace that key on the path reads the password.

use crate::error::{Error, Result};
use std::sync::{Arc, Mutex};
use runtime::tls::pinning;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

/// A configured connector, and the certificate the user accepted for the
/// session.
pub struct Connector {
    connector: TlsConnector,
    host: String,
    /// The fingerprint accepted on the session's first channel, which every
    /// channel after it has to present again (see [`Connector::admit`]).
    accepted: Mutex<Option<String>>,
    /// What the handshake settled on, for the panel's Connection row. Taken
    /// off the connection while it is still in hand, because it goes into the
    /// channel's stream immediately afterwards.
    negotiated: Mutex<Option<String>>,
}

impl Connector {
    pub fn new(host: &str) -> Result<Connector> {
        // The protocol crate takes its signature algorithms from the
        // process-wide provider rather than naming one (the point of its
        // patch), so this install makes both halves of the session use `ring`.
        let _ = rustls::crypto::ring::default_provider().install_default();
        // The verifier is not asked what it saw: it remembers only the last
        // of the four or five handshakes this connector makes, whereas each
        // connection's own peer certificate belongs to that connection.
        let (config, _) = pinning::client_config().map_err(|e| Error::Protocol(e.to_string()))?;
        Ok(Connector {
            connector: TlsConnector::from(Arc::new(config)),
            host: host.to_string(),
            accepted: Mutex::new(None),
            negotiated: Mutex::new(None),
        })
    }

    /// The handshake, and the SHA-256 of the leaf certificate it presented:
    /// what the app pins, and what the user is asked about when it changes.
    pub async fn connect<S>(&self, io: S) -> Result<(TlsStream<S>, Option<String>)>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let name = pinning::server_name(&self.host);
        let stream = self.connector.connect(name, io).await?;
        let (_, conn) = stream.get_ref();
        *self.negotiated.lock().unwrap() = Some(pinning::describe(
            conn.protocol_version(),
            conn.negotiated_cipher_suite(),
        ));
        let fingerprint = conn.peer_certificates().and_then(|certs| certs.first()).map(pinning::fingerprint);
        Ok((stream, fingerprint))
    }

    /// Whether a connection that presented `fingerprint` may carry the session
    /// on, before its ticket goes out.
    ///
    /// The session's first channel asks `trust`, and the fingerprint it
    /// accepts is remembered. Every later channel (the other three, the sound
    /// channel opened later, and the main channel dialled again after a
    /// refused ticket) reaches the same server moments later without asking
    /// again, so it must present the same certificate. A different one is
    /// refused without asking, since the user approved one server, and a
    /// channel reaching another would send its ticket there.
    pub fn admit(&self, fingerprint: Option<&str>, trust: Option<&mut dyn FnMut(&str) -> bool>) -> Result<()> {
        // rustls completes no handshake without a server certificate, so
        // this does not come from rustls.
        let Some(fingerprint) = fingerprint else {
            return Err(Error::Protocol("the TLS handshake did not present a certificate".into()));
        };
        let mut accepted = self.accepted.lock().unwrap();
        match (trust, accepted.as_deref()) {
            (Some(ask), _) => {
                if !ask(fingerprint) {
                    return Err(Error::Cancelled);
                }
                *accepted = Some(fingerprint.to_string());
                Ok(())
            }
            (None, Some(first)) if first == fingerprint => Ok(()),
            (None, _) => Err(Error::Protocol(format!(
                "a channel presented a certificate ({fingerprint}) that is not the one accepted for this session"
            ))),
        }
    }

    /// `TLSv1_3 · TLS13_AES_256_GCM_SHA384`, for the panel.
    pub fn negotiated(&self) -> Option<String> {
        self.negotiated.lock().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first channel's answer is the session's: a later channel with the
    /// same certificate goes on unasked, one with another is refused unasked,
    /// and a refused first one is Cancelled and accepts nothing.
    #[test]
    fn later_channels_present_the_accepted_certificate() {
        let connector = Connector::new("127.0.0.1").unwrap();
        let mut asked = Vec::new();
        let mut yes = |fp: &str| {
            asked.push(fp.to_string());
            true
        };
        assert!(connector.admit(Some("AA"), Some(&mut yes)).is_ok());
        assert!(connector.admit(Some("AA"), None).is_ok());
        assert!(matches!(connector.admit(Some("BB"), None), Err(Error::Protocol(_))));
        assert!(matches!(connector.admit(None, None), Err(Error::Protocol(_))));
        assert_eq!(asked, ["AA"]);

        let refused = Connector::new("127.0.0.1").unwrap();
        let mut no = |_: &str| false;
        assert!(matches!(refused.admit(Some("AA"), Some(&mut no)), Err(Error::Cancelled)));
        assert!(matches!(refused.admit(Some("AA"), None), Err(Error::Protocol(_))));
    }
}
