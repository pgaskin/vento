// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The version and security handshakes, including VeNCrypt's.

use crate::error::{Error, Result};
use crate::proto::{Reader, Writer, latin1, to_latin1};
use crate::transport::Wire;
use runtime::Credentials;
use cipher::{BlockCipherEncrypt, KeyInit};
use std::io::{Read, Write};

pub const SEC_INVALID: u8 = 0;
pub const SEC_NONE: u8 = 1;
pub const SEC_VNC_AUTH: u8 = 2;
pub const SEC_VENCRYPT: u8 = 19;

/// VeNCrypt's sub-types, of which this client supports the X.509 ones.
///
/// The others (257 `TLSNone`, 258 `TLSVnc`, 259 `TLSPlain`) are **anonymous**
/// TLS: a Diffie-Hellman exchange with no certificate at all. The libvncclient
/// and TigerVNC backends offer them and ask the user about them; this one
/// can't, because of its TLS library rather than by choice.
///
/// A gnutls server offering an anonymous sub-type answers a ClientHello with
/// `TLS_ECDH_anon_WITH_AES_256_CBC_SHA` and nothing else (measured against
/// TigerVNC 1.15, whose `+ANON-ECDH:+ANON-DH` leaves the GCM anon suites
/// unreachable), and drops the connection outright if the offer contains a
/// certificate suite it has no credentials for. `rustls` has no anonymous key
/// exchange (`KeyExchangeAlgorithm` is ECDHE or DHE), no CBC MAC-then-encrypt
/// suite by policy, and a TLS 1.2 client that reaches `ExpectCertificate`
/// whatever was agreed. Supporting them would take a second TLS library or a
/// hand-written TLS 1.2, for a case the libvncclient and TigerVNC backends
/// already cover.
///
/// And there would still be nothing to pin. Anonymous TLS encrypts the
/// connection but proves nothing about the far end, so a man in the middle is
/// undetectable by design.
pub const VENCRYPT_X509_NONE: u32 = 260;
pub const VENCRYPT_X509_VNC: u32 = 261;
pub const VENCRYPT_X509_PLAIN: u32 = 262;

/// What a connection accepts.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Security {
    /// TLS where the server offers it, and a plain connection where it does not.
    #[default]
    Prefer,
    /// TLS or nothing.
    Require,
    /// Never TLS: for a link that is already private, or a server whose
    /// certificate is more trouble than the LAN it is on.
    Plain,
}

/// `RFB 003.00x` as a pair, because the three versions behave differently.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Version(pub u8, pub u8);

impl Version {
    pub const V3_3: Version = Version(3, 3);
    pub const V3_7: Version = Version(3, 7);
    pub const V3_8: Version = Version(3, 8);

    pub fn text(&self) -> String {
        format!("RFB {:03}.{:03}\n", self.0, self.1)
    }
}

/// Agrees a version: the highest both ends know, at least 3.3.
pub fn handshake_version(r: &mut Reader<impl Read>, w: &mut Writer<impl Write>) -> Result<Version> {
    let mut buf = [0u8; 12];
    r.read_exact(&mut buf)?;
    let text = latin1(&buf);
    let parse = || -> Option<Version> {
        let rest = text.strip_prefix("RFB ")?;
        let (major, rest) = rest.split_at_checked(3)?;
        let minor = rest.strip_prefix('.')?.get(..3)?;
        Some(Version(major.parse().ok()?, minor.parse().ok()?))
    };
    let server = parse().ok_or_else(|| Error::Protocol(format!("not an RFB server: {text:?}")))?;
    let agreed = if server >= Version::V3_8 {
        Version::V3_8
    } else if server >= Version::V3_7 {
        Version::V3_7
    } else {
        Version::V3_3
    };
    w.write_all(agreed.text().as_bytes())?;
    w.flush()?;
    Ok(agreed)
}

/// The security type negotiated with the server.
pub struct Chosen {
    /// For the connection panel: "None", "VNC password (VeNCrypt)".
    pub description: String,
}

/// The two questions a handshake may need to ask the user.
///
/// Both are asked lazily, by design: either answer may take as long as the
/// user takes to read a dialog. The protocol thread waits here, which is also
/// why nothing else may be waiting on this thread.
///
/// One trait rather than two closures because both end up at the same
/// `Handler`, and Rust doesn't allow two closures over one `&mut`.
pub trait Ask {
    /// `needs_user` asks for a user name as well. VncAuth has no user name, so
    /// a dialog showing the field would ask for something that can't be used.
    fn credentials(&mut self, needs_user: bool) -> Option<Credentials>;

    /// The server's certificate fingerprint. `false` abandons the connection.
    fn trust(&mut self, fingerprint: &str) -> bool;

    /// The handler's [`status_setter`](runtime::Handler::status_setter).
    fn status_setter(&mut self) -> Option<runtime::status::StatusSetter> {
        None
    }
}

/// The status while the server has not answered the authentication: a
/// server set to ask its user (TigerVNC's `QueryConnect`, RealVNC's and
/// x11vnc's equivalents) sends its result only once they answer.
const WAITING_FOR_APPROVAL: &str = "Waiting for the server to accept the connection…";

/// Negotiates a security type, runs it, and reads the result.
pub fn handshake_security(
    wire: &mut Wire,
    version: Version,
    policy: Security,
    host: &str,
    ask: &mut dyn Ask,
) -> Result<Chosen> {
    let kind = if version >= Version::V3_7 {
        let count = wire.r.u8()? as usize;
        if count == 0 {
            // A zero-length list is a refusal, and the reason is worth having:
            // it is where "too many authentication failures" comes from.
            return Err(Error::Refused(wire.r.string()?));
        }
        // Named explicitly: `Reader::bytes` and `Read::bytes` are different
        // methods with the same name, and on a `Reader` held by value the
        // trait's wins.
        let offered = Reader::bytes(&mut wire.r, count)?;
        let chosen = choose(&offered, policy)?;
        wire.w.u8(chosen)?;
        wire.w.flush()?;
        chosen
    } else {
        // 3.3 lets the server dictate, and has no VeNCrypt to dictate.
        if policy == Security::Require {
            return Err(Error::Unsupported(
                "this connection requires TLS, and RFB 3.3 has no security type that offers it"
                    .into(),
            ));
        }
        let kind = wire.r.u32()?;
        if kind == SEC_INVALID as u32 {
            return Err(Error::Refused(wire.r.string()?));
        }
        kind as u8
    };

    let description = match kind {
        SEC_NONE => "None".to_string(),
        SEC_VNC_AUTH => {
            vnc_auth_exchange(wire, ask)?;
            "VNC password".to_string()
        }
        SEC_VENCRYPT => vencrypt(wire, host, ask)?,
        other => {
            return Err(Error::Unsupported(format!("security type {other}")));
        }
    };

    // 3.3 and 3.7 send no result for None; 3.8 always does, and is the only
    // one that gives a reason. VeNCrypt never counts as None for this rule: the
    // result comes through the tunnel even when the sub-type inside it
    // authenticated nothing.
    let expect_result = version >= Version::V3_8 || kind != SEC_NONE;
    if expect_result {
        let result = {
            let _waiting = runtime::status::Delayed::new(ask.status_setter(), WAITING_FOR_APPROVAL);
            wire.r.u32()?
        };
        if result != 0 {
            let reason = if version >= Version::V3_8 {
                wire.r.string().unwrap_or_default()
            } else {
                String::new()
            };
            return Err(Error::AuthFailed(if reason.is_empty() {
                match kind {
                    SEC_NONE => "The server refused the connection.".to_string(),
                    _ => "The password was not accepted.".to_string(),
                }
            } else {
                reason
            }));
        }
    }

    Ok(Chosen { description })
}

/// Which of the offered types to use.
///
/// VeNCrypt first wherever allowed, because the alternative to an encrypted
/// session is one in which the desktop and every keystroke cross the network
/// unencrypted. `Security::Plain` is how to accept that.
fn choose(offered: &[u8], policy: Security) -> Result<u8> {
    let has = |t: u8| offered.contains(&t);
    if policy != Security::Plain && has(SEC_VENCRYPT) {
        return Ok(SEC_VENCRYPT);
    }
    if policy == Security::Require {
        return Err(Error::Unsupported(format!(
            "this connection requires TLS and the server does not offer VeNCrypt \
             (it offers security types {offered:?})"
        )));
    }
    if has(SEC_VNC_AUTH) {
        return Ok(SEC_VNC_AUTH);
    }
    if has(SEC_NONE) {
        return Ok(SEC_NONE);
    }
    Err(Error::Unsupported(format!(
        "the server offers only security types {offered:?}, and this client speaks \
         None, VncAuth and VeNCrypt"
    )))
}

/// VeNCrypt 0.2: agree a version, pick a sub-type, start TLS, and run the
/// authentication the sub-type names inside the tunnel.
fn vencrypt(wire: &mut Wire, host: &str, ask: &mut dyn Ask) -> Result<String> {
    let major = wire.r.u8()?;
    let minor = wire.r.u8()?;
    if (major, minor) < (0, 2) {
        return Err(Error::Unsupported(format!(
            "VeNCrypt {major}.{minor}; this client speaks 0.2"
        )));
    }
    wire.w.u8(0)?;
    wire.w.u8(2)?;
    wire.w.flush()?;
    if wire.r.u8()? != 0 {
        return Err(Error::Refused("the server refused VeNCrypt 0.2".into()));
    }

    let count = wire.r.u8()? as usize;
    if count == 0 {
        return Err(Error::Refused(
            "the server offers no VeNCrypt sub-types".into(),
        ));
    }
    let mut offered = Vec::with_capacity(count);
    for _ in 0..count {
        offered.push(wire.r.u32()?);
    }
    // X509Vnc before X509Plain before X509None: prefer the sub-type that
    // authenticates *this client* to the server, since a tunnel proves the
    // server's identity and says nothing about the client's.
    let chosen = *[VENCRYPT_X509_VNC, VENCRYPT_X509_PLAIN, VENCRYPT_X509_NONE]
        .iter()
        .find(|t| offered.contains(t))
        .ok_or_else(|| {
            Error::Unsupported(format!(
                "the server offers VeNCrypt sub-types {offered:?}, and this client speaks \
                 the X.509 ones (260, 261, 262). 257–259 are anonymous TLS, which this \
                 client's TLS stack has no key exchange for"
            ))
        })?;
    wire.w.write_all(&chosen.to_be_bytes())?;
    wire.w.flush()?;

    // One plain byte says the sub-type was accepted, and it arrives *after*
    // our ClientHello, which is the most surprising thing in this protocol.
    // TigerVNC writes it into a buffered stream and goes straight into the TLS
    // handshake, so nothing flushes it until the server has something to send;
    // a client that waits for it before starting TLS waits forever, and one
    // that never reads it feeds `01` to its TLS parser and gets a
    // corrupt-record error. Measured against the server rather than taken
    // from a specification: a refused sub-type is not a zero here, but the
    // connection closing.
    wire.start_tls(host, &mut |socket| {
        let mut ack = [0u8; 1];
        socket.read_exact(&mut ack)?;
        if ack[0] == 0 {
            return Err(Error::Refused(format!(
                "the server refused VeNCrypt sub-type {chosen}"
            )));
        }
        Ok(())
    })?;
    let fingerprint = wire
        .peer()
        .map(|p| p.fingerprint.clone())
        .ok_or_else(|| Error::Protocol("TLS came up without a certificate".into()))?;
    // Before the sub-type runs, which is where a password would go.
    if !ask.trust(&fingerprint) {
        return Err(Error::Untrusted);
    }

    Ok(match chosen {
        VENCRYPT_X509_NONE => "None (VeNCrypt)".to_string(),
        VENCRYPT_X509_VNC => {
            vnc_auth_exchange(wire, ask)?;
            "VNC password (VeNCrypt)".to_string()
        }
        _ => {
            plain_exchange(wire, ask)?;
            "User name and password (VeNCrypt)".to_string()
        }
    })
}

/// The 16-byte challenge, and DES over it.
fn vnc_auth_exchange(wire: &mut Wire, ask: &mut dyn Ask) -> Result<()> {
    let mut challenge = [0u8; 16];
    wire.r.read_exact(&mut challenge)?;
    let credentials = ask.credentials(false).ok_or(Error::Cancelled)?;
    let response = vnc_auth(&credentials.password, &challenge);
    wire.w.write_all(&response)?;
    wire.w.flush()?;
    Ok(())
}

/// VeNCrypt's `Plain`: a user name and a password in the clear, which is why
/// this client only runs it inside a tunnel it has already pinned.
fn plain_exchange(wire: &mut Wire, ask: &mut dyn Ask) -> Result<()> {
    let credentials = ask.credentials(true).ok_or(Error::Cancelled)?;
    let user = to_latin1(&credentials.user);
    let password = to_latin1(&credentials.password);
    wire.w.write_all(&(user.len() as u32).to_be_bytes())?;
    wire.w.write_all(&(password.len() as u32).to_be_bytes())?;
    wire.w.write_all(&user)?;
    wire.w.write_all(&password)?;
    wire.w.flush()?;
    Ok(())
}

/// VncAuth: DES-ECB over the 16-byte challenge, keyed with the first eight
/// bytes of the password, with every byte's *bits reversed*.
///
/// The reversal comes from the original AT&T implementation, which fed a DES
/// routine that took the key least significant bit first, and every server
/// now matches it.
fn vnc_auth(password: &str, challenge: &[u8; 16]) -> [u8; 16] {
    let mut key = [0u8; 8];
    for (slot, byte) in key.iter_mut().zip(crate::proto::to_latin1(password)) {
        *slot = byte.reverse_bits();
    }
    let des = des::Des::new_from_slice(&key).expect("DES takes exactly eight bytes");
    let mut out = *challenge;
    for block in out.chunks_exact_mut(8) {
        let block: &mut [u8; 8] = block.try_into().expect("chunks_exact_mut(8)");
        des.encrypt_block(block.into());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    /// Answers nothing, and records the statuses it is given.
    struct Statuses(Arc<Mutex<Vec<String>>>);

    impl Ask for Statuses {
        fn credentials(&mut self, _needs_user: bool) -> Option<Credentials> {
            None
        }
        fn trust(&mut self, _fingerprint: &str) -> bool {
            false
        }
        fn status_setter(&mut self) -> Option<runtime::status::StatusSetter> {
            let said = Arc::clone(&self.0);
            Some(Arc::new(move |text: Option<&str>| {
                said.lock().unwrap().push(text.unwrap_or("-").to_string())
            }))
        }
    }

    /// RFB 3.8 with security type None, the result sent after `delay`: a
    /// server asking its user is named once the wait has lasted a second,
    /// and a prompt one is not named at all.
    fn result_after(delay: Duration) -> Vec<String> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let near = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut far, _) = listener.accept().unwrap();
        let server = std::thread::spawn(move || {
            use std::io::{Read, Write};
            far.write_all(&[1, SEC_NONE]).unwrap();
            let mut chosen = [0u8; 1];
            far.read_exact(&mut chosen).unwrap();
            std::thread::sleep(delay);
            far.write_all(&0u32.to_be_bytes()).unwrap();
        });
        let mut wire = Wire::plain(near, Arc::default(), Arc::default()).unwrap();
        let said = Arc::new(Mutex::new(Vec::new()));
        let mut ask = Statuses(Arc::clone(&said));
        let chosen = handshake_security(&mut wire, Version::V3_8, Security::Plain, "far", &mut ask);
        server.join().unwrap();
        assert_eq!(chosen.unwrap().description, "None");
        let said = said.lock().unwrap().clone();
        said
    }

    #[test]
    fn a_server_slow_to_accept_is_named() {
        assert!(result_after(Duration::ZERO).is_empty());
        let slow = runtime::status::READABLE + Duration::from_millis(300);
        assert_eq!(result_after(slow), [WAITING_FOR_APPROVAL]);
    }

    /// Password "test", all-zero challenge, checked against another
    /// implementation:
    ///
    /// ```text
    /// $ printf '\0\0\0\0\0\0\0\0' | openssl enc -des-ecb -K 2ea6ce2e00000000 \
    ///       -nopad -provider legacy -provider default | xxd -p
    /// 77dfa81c9fd7b407
    /// ```
    ///
    /// `2ea6ce2e` is `test` with each byte's bits reversed, the only part of
    /// this that is VNC's rather than DES's, so the test really checks that
    /// [`u8::reverse_bits`] is applied, and openssl supplies the rest.
    #[test]
    fn vnc_auth_known_answer() {
        let out = vnc_auth("test", &[0u8; 16]);
        let half = [0x77, 0xdf, 0xa8, 0x1c, 0x9f, 0xd7, 0xb4, 0x07];
        assert_eq!(&out[0..8], &half);
        assert_eq!(&out[8..16], &half, "ECB: both blocks are the same");
    }

    /// Only the first eight bytes are the key, and a shorter one is zero-padded.
    #[test]
    fn vnc_auth_truncates() {
        assert_eq!(
            vnc_auth("12345678", &[1u8; 16]),
            vnc_auth("123456789abc", &[1u8; 16])
        );
    }
}
