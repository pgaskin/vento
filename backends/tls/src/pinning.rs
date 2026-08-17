// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! Trust on first sight: the certificate check both protocol clients share.
//!
//! A VNC server presents a certificate it generated for itself on first start,
//! and so does an xrdp; a Windows RDP host presents one signed by nothing a
//! phone has ever heard of. Verifying any of them against a certificate
//! authority would reject every server anybody runs, and a client that then
//! offered "continue anyway" would have taught its user to click through the
//! only check it has.
//!
//! So the identity is the **key**, as it is over SSH: the leaf certificate is
//! fingerprinted, the app pins it against the address on first sight, a match
//! is silent, and anything else is a question. This module is the half of that
//! which is the same whatever the protocol underneath — and it is shared rather
//! than sitting in whichever client needed it first because "what does trusted
//! mean here" should have exactly one answer in this app, not one per
//! protocol.
//!
//! What is **not** deferred is the signature over the handshake:
//! [`Pinning::verify_tls12_signature`] and its 1.3 counterpart are the
//! provider's own. Without them a fingerprint proves only that somebody can
//! copy a public certificate; with them it proves the far end holds the key.
//!
//! The decision itself is taken above this, on the protocol thread, after the
//! handshake and before anything secret goes out — which is both the last safe
//! moment and the first one on a thread that is allowed to wait for a person.
//! A rustls verifier callback is neither.

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};

/// A verifier that accepts every certificate and remembers what it accepted.
///
/// The certificate is not the decision, it is the *evidence*.
#[derive(Debug)]
pub struct Pinning {
    provider: Arc<CryptoProvider>,
    seen: Mutex<Option<CertificateDer<'static>>>,
}

impl Pinning {
    pub fn new(provider: Arc<CryptoProvider>) -> Pinning {
        Pinning {
            provider,
            seen: Mutex::new(None),
        }
    }

    /// The leaf certificate the handshake presented, once there has been one.
    pub fn certificate(&self) -> Option<CertificateDer<'static>> {
        self.seen.lock().unwrap().clone()
    }

    /// Its SHA-256, in the form [`fingerprint`] describes.
    pub fn fingerprint(&self) -> Option<String> {
        self.certificate().as_ref().map(fingerprint)
    }
}

impl ServerCertVerifier for Pinning {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        *self.seen.lock().unwrap() = Some(end_entity.clone().into_owned());
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// A rustls client configuration that pins rather than verifies, and the
/// verifier to ask afterwards what it saw.
///
/// `ring` is the provider because aws-lc-rs wants cmake and bindgen to build
/// for Android; it is the only dependency in this workspace with any C in it.
pub fn client_config() -> Result<(ClientConfig, Arc<Pinning>), rustls::Error> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = Arc::new(Pinning::new(Arc::clone(&provider)));
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(Arc::clone(&verifier) as Arc<dyn ServerCertVerifier>)
        .with_no_client_auth();
    Ok((config, verifier))
}

/// What rustls insists on having, out of an address somebody typed.
///
/// Nothing here checks the name — the name in a certificate a server signed for
/// itself is whatever it felt like writing, and the pin is the identity — so a
/// host that is not a valid DNS name or address becomes a placeholder rather
/// than an error.
pub fn server_name(host: &str) -> ServerName<'static> {
    ServerName::try_from(host.to_string())
        .unwrap_or_else(|_| ServerName::try_from("server.invalid").expect("a literal name"))
}

/// `openssl x509 -fingerprint -sha256`'s output, so that a fingerprint on the
/// phone can be checked against one printed by something that is not us.
pub fn fingerprint(cert: &CertificateDer<'_>) -> String {
    let digest = Sha256::digest(cert.as_ref());
    digest
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// For the connection panel's Connection row: `TLSv1_3 · TLS13_AES_256_GCM_SHA384`.
pub fn describe(
    version: Option<rustls::ProtocolVersion>,
    suite: Option<rustls::SupportedCipherSuite>,
) -> String {
    format!(
        "{} · {}",
        version.map_or("TLS".to_string(), |v| format!("{v:?}")),
        suite.map_or("?".to_string(), |s| format!("{:?}", s.suite()))
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one thing in here with a known answer: `sha256` of the empty
    /// certificate, formatted the way openssl formats it.
    ///
    /// ```text
    /// $ printf '' | openssl dgst -sha256 -c -upper
    /// SHA2-256(stdin)= E3:B0:C4:42:98:FC:1C:14:9A:FB:F4:C8:99:6F:B9:24:…
    /// ```
    #[test]
    fn fingerprints_look_like_openssls() {
        let empty = CertificateDer::from(vec![]);
        assert!(fingerprint(&empty).starts_with("E3:B0:C4:42:98:FC:1C:14:9A:FB:F4:C8:99:6F:B9:24"));
    }
}
