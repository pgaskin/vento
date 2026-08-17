//! Their session encryption: two signatures, one sealed key, and a stream
//! cipher counting its own frames.
//!
//! The design is worth reading before the code, because it is easy to read as
//! one signature and it is two:
//!
//! 1. the rendezvous server's `PunchHoleResponse` carries an `IdPk{id, pk}`
//!    signed with **its** secret key, where `pk` is the peer's **long-term**
//!    sign key. That is an introduction: this id belongs to this key.
//! 2. the peer then sends `SignedId`, another `IdPk`, signed with **that** key,
//!    whose `pk` is a **per-connection** curve25519 box key.
//! 3. a fresh secretbox key is sealed to that box key and sent, and every frame
//!    after the one carrying it is encrypted under it.
//!
//! So the server never sees the key a session is sealed to and cannot
//! substitute one. A server that lies can introduce the wrong machine, and only
//! for an id it has not already introduced honestly — which is what the pin on
//! the long-term key catches.

use crate::error::{Error, Result};
use crate::protos::message::IdPk;
use crate::wire::Cipher;

use dryoc::classic::crypto_box::{crypto_box_easy, crypto_box_keypair};
use dryoc::classic::crypto_secretbox::{crypto_secretbox_easy, crypto_secretbox_open_easy};
use dryoc::classic::crypto_sign::crypto_sign_open;
use protobuf::Message as _;
use sha2::{Digest, Sha256};

/// An ed25519 signature is attached to the message it signs rather than beside
/// it, so an opened message is the frame minus this much.
const SIGNATURE: usize = 64;

/// Their session cipher: xsalsa20-poly1305 under a nonce that is the frame's
/// sequence number, little-endian and zero-padded. Theirs increments before
/// use, so a frame numbered zero never goes on the wire, and each direction
/// counts on its own — which is why this is one per half rather than shared.
pub struct Secretbox {
    key: [u8; 32],
    counter: u64,
}

impl Secretbox {
    pub fn new(key: [u8; 32]) -> Secretbox {
        Secretbox { key, counter: 0 }
    }

    fn nonce(n: u64) -> [u8; 24] {
        let mut nonce = [0u8; 24];
        nonce[..8].copy_from_slice(&n.to_le_bytes());
        nonce
    }
}

impl Cipher for Secretbox {
    fn seal(&mut self, payload: Vec<u8>) -> std::io::Result<Vec<u8>> {
        self.counter += 1;
        let mut sealed = vec![0u8; payload.len() + 16];
        crypto_secretbox_easy(
            &mut sealed,
            &payload,
            &Secretbox::nonce(self.counter),
            &self.key,
        )
        .map_err(std::io::Error::other)?;
        Ok(sealed)
    }

    fn open(&mut self, payload: Vec<u8>) -> std::io::Result<Vec<u8>> {
        self.counter += 1;
        if payload.len() < 16 {
            return Err(std::io::Error::other("a sealed frame with no tag in it"));
        }
        let mut opened = vec![0u8; payload.len() - 16];
        crypto_secretbox_open_easy(
            &mut opened,
            &payload,
            &Secretbox::nonce(self.counter),
            &self.key,
        )
        .map_err(std::io::Error::other)?;
        Ok(opened)
    }
}

/// One link of the chain: open an attached signature, parse the `IdPk` inside,
/// insist the id is the one that was dialled, and hand back its key.
///
/// The id check is the whole point of the message. Without it a server could
/// introduce any machine it liked for any id, since the signature would still
/// be its own.
pub fn open_id_pk(signed: &[u8], with: &[u8; 32], peer_id: &str) -> Result<[u8; 32]> {
    if signed.len() < SIGNATURE {
        return Err(Error::Protocol("a signature shorter than a signature".into()));
    }
    let mut opened = vec![0u8; signed.len() - SIGNATURE];
    crypto_sign_open(&mut opened, signed, with)
        .map_err(|_| Error::Protocol("the signature does not check out".into()))?;
    let id_pk = IdPk::parse_from_bytes(&opened)
        .map_err(|e| Error::Protocol(format!("what was signed is not an IdPk: {e}")))?;
    if id_pk.id != peer_id {
        return Err(Error::Protocol(format!(
            "signed for {} rather than for {peer_id}",
            id_pk.id
        )));
    }
    id_pk
        .pk
        .to_vec()
        .try_into()
        .map_err(|_| Error::Protocol("the key inside is not 32 bytes".into()))
}

/// A fresh session key sealed to the peer's per-connection box key: what to
/// send, and the key itself.
///
/// The nonce is zero, which is safe exactly once for a key pair and is used
/// once — the box key is new for this connection and ours is new for this call.
pub fn seal_session_key(their_box_pk: &[u8; 32]) -> Result<(Vec<u8>, Vec<u8>, [u8; 32])> {
    let (our_pk, our_sk) = crypto_box_keypair();
    let mut key = [0u8; 32];
    dryoc::rng::copy_randombytes(&mut key);
    let mut sealed = vec![0u8; key.len() + 16];
    crypto_box_easy(&mut sealed, &key, &[0u8; 24], their_box_pk, &our_sk)
        .map_err(|e| Error::Protocol(format!("sealing the session key: {e}")))?;
    Ok((our_pk.to_vec(), sealed, key))
}

/// What a person is shown of a peer's long-term key, and what the pin store
/// holds: its SHA-256, spelled the way `openssl dgst -sha256 -c` spells one.
///
/// A digest rather than the base64 their own client shows, because the store
/// compares hex digits and drops everything else — it is keyed on machines
/// rather than on clients, so what it holds has to be in the one form every
/// backend here already writes.
pub fn fingerprint(key: &[u8; 32]) -> String {
    Sha256::digest(key)
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// A rendezvous server's public key as it is written down: 32 bytes of base64,
/// which is the form their config file, their web UI and their `id_ed25519.pub`
/// all use.
pub fn server_key(text: &str) -> Result<[u8; 32]> {
    base64(text.trim())?
        .try_into()
        .map_err(|_| Error::Protocol("a rendezvous key that is not 32 bytes".into()))
}

/// Standard base64, decoded. Padding and whitespace are ignored rather than
/// insisted on: this decodes something somebody pasted.
fn base64(text: &str) -> Result<Vec<u8>> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in text
        .bytes()
        .filter(|c| *c != b'=' && !c.is_ascii_whitespace())
    {
        let value = ALPHABET
            .iter()
            .position(|a| *a == c)
            .ok_or_else(|| Error::Protocol("a rendezvous key that is not base64".into()))?
            as u32;
        acc = (acc << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The public network's own key, which is a constant in every build of
    /// theirs and the one piece of this with a known answer.
    #[test]
    fn the_public_network_key_is_32_bytes() {
        let key = server_key(crate::rendezvous::PUBLIC_KEY).unwrap();
        assert_eq!(key.len(), 32);
        assert_eq!(key[0], 0x39);
    }

    /// Both directions of a session, one after another, which is the property
    /// the counter has to have: the nonce is the frame's number and the two
    /// ends agree on it only because each direction counts alone.
    #[test]
    fn frames_open_in_the_order_they_were_sealed() {
        let key = [7u8; 32];
        let mut sender = Secretbox::new(key);
        let mut receiver = Secretbox::new(key);
        for n in 0..4usize {
            let payload = vec![n as u8; n * 100];
            let sealed = sender.seal(payload.clone()).unwrap();
            assert_eq!(receiver.open(sealed).unwrap(), payload);
        }
    }

    /// A frame opened out of order fails rather than producing something: the
    /// nonce is wrong, so the tag does not check.
    #[test]
    fn a_frame_out_of_order_does_not_open() {
        let key = [9u8; 32];
        let mut sender = Secretbox::new(key);
        let mut receiver = Secretbox::new(key);
        let first = sender.seal(b"one".to_vec()).unwrap();
        let second = sender.seal(b"two".to_vec()).unwrap();
        assert!(receiver.open(second).is_err());
        assert!(receiver.open(first).is_err());
    }

    /// Their whole chain, built here and taken apart the way a session takes it
    /// apart: a server vouching for a peer's long-term key, the peer signing a
    /// box key with it, and a session key sealed to that.
    #[test]
    fn the_two_signatures_and_the_sealed_key() {
        use dryoc::classic::crypto_box::crypto_box_open_easy;
        use dryoc::classic::crypto_sign::{crypto_sign, crypto_sign_keypair};

        let (server_pk, server_sk) = crypto_sign_keypair();
        let (peer_pk, peer_sk) = crypto_sign_keypair();
        let (box_pk, box_sk) = crypto_box_keypair();

        let sign = |pk: &[u8; 32], id: &str, sk: &_| {
            let mut id_pk = IdPk::new();
            id_pk.id = id.into();
            id_pk.pk = pk.to_vec().into();
            let message = id_pk.write_to_bytes().unwrap();
            let mut signed = vec![0u8; message.len() + SIGNATURE];
            crypto_sign(&mut signed, &message, sk).unwrap();
            signed
        };

        let introduction = sign(&peer_pk, "123456789", &server_sk);
        assert_eq!(
            open_id_pk(&introduction, &server_pk, "123456789").unwrap(),
            peer_pk
        );
        // The id in the signature is checked against the id that was dialled,
        // so a valid signature for another machine is refused.
        assert!(open_id_pk(&introduction, &server_pk, "987654321").is_err());

        let signed_id = sign(&box_pk, "123456789", &peer_sk);
        let their_box_pk = open_id_pk(&signed_id, &peer_pk, "123456789").unwrap();
        let (our_box_pk, sealed, key) = seal_session_key(&their_box_pk).unwrap();
        let mut opened = vec![0u8; sealed.len() - 16];
        crypto_box_open_easy(
            &mut opened,
            &sealed,
            &[0u8; 24],
            &our_box_pk.try_into().unwrap(),
            &box_sk,
        )
        .unwrap();
        assert_eq!(opened, key);
    }
}
