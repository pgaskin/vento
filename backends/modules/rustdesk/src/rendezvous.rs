// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! Reaching a peer by id: the introduction, the punch, and the relay when the
//! punch fails.
//!
//! A rendezvous server holds two facts per peer (its address and its key) and
//! gives both to any client that asks for an id. It is not an account or an
//! address book: nothing about this device's connections is stored there, and
//! it only introduces the two ends.
//!
//! Three points here are easy to get wrong, and each has a comment where it
//! applies:
//!
//! - **The relay is not an error path.** Two peers behind symmetric NAT never
//!   punch through, so a client that treats a relay as a failure is wrong for
//!   most of the internet.
//! - **The punch needs the port the server saw.** A peer is dialled from the
//!   local port used for the rendezvous server, because that is the hole the
//!   peer was told to expect a packet on; a fresh port works only on a LAN.
//! - **The id is an address and the key is the identity.** Ids are re-issued
//!   when a machine's configuration is wiped, so a session is verified against
//!   the key the server vouched for, and the id only says what was requested.
//!
//! None of the bytes here are counted in the panel's Transferred row: they are
//! a few hundred bytes on sockets other than the session's, and the row covers
//! the connection's traffic.

use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::error::{Error, Result};
use crate::protos::rendezvous::{
    PunchHoleRequest, RendezvousMessage, RequestRelay, punch_hole_response::Failure,
    rendezvous_message,
};
use crate::wire::{self, Traffic};
use runtime::Hangup;

/// RustDesk's default ports (`libs/hbb_common/src/config.rs`): the rendezvous
/// server and the relay next to it. Direct access is two above the rendezvous
/// port (`client::DIRECT_PORT`).
pub const RENDEZVOUS_PORT: u16 = 21116;
pub const RELAY_PORT: u16 = 21117;

/// The public network's identity, built into every RustDesk build. A peer
/// reached through it is verified against this key rather than one the user
/// entered, which makes it a pin chosen by RustDesk rather than no pin at all.
pub const PUBLIC_SERVER: &str = "rs-ny.rustdesk.com";
pub const PUBLIC_KEY: &str = "OeVuKk5nlHiXp+APNn0Y3pC1Iwpwn44JGqrQCsWqmBw=";

/// How long to wait before meeting a relay; see `meet_relay` for why.
const RELAY_PAIR_DELAY: Duration = Duration::from_millis(250);

/// How a peer was reached, for the panel's Connection row.
pub enum How {
    /// A socket straight to the peer, at the address the server gave.
    Punched(SocketAddr),
    /// Through a relay, which carries the session's bytes but cannot read them.
    Relayed(String),
}

/// A socket to the peer, and what the rendezvous server said about it.
pub struct Reached {
    pub sock: TcpStream,
    /// The peer's long-term sign key, signed by the rendezvous server. Empty if
    /// the server answered without one, in which case the session cannot be
    /// verified.
    pub signed_id_pk: Vec<u8>,
    pub how: How,
}

impl How {
    pub fn describe(&self) -> String {
        match self {
            How::Punched(addr) => format!("Punched through to {addr}"),
            How::Relayed(server) => format!("Relayed by {server}"),
        }
    }
}

/// Asks a rendezvous server for a peer, and returns a socket to it.
///
/// Both outcomes are handled here rather than in the caller, because the
/// network decides which happens, not a setting: either the server tells both
/// ends to dial each other and this end dials, or the peer asks for a relay
/// and this end meets it there.
///
/// Every socket it opens is dialled through `hangup` and held there while it
/// is being waited on, so that closing the session ends this at any step.
pub fn reach(hangup: &Hangup, id: &str, server: &str, key: &str, timeout: Duration) -> Result<Reached> {
    let server_addr = resolve(server, RENDEZVOUS_PORT)?;
    log::info!("rendezvous: asking {server_addr} for {id}");
    let sock = connect_reusable(hangup, server_addr, None, timeout)?;
    let _holding = hangup.hold(&sock)?;
    // The peer is told to expect a packet on the hole this socket opened, so
    // the peer is dialled from this socket's local address.
    let local = sock.local_addr()?;
    let (mut reader, mut writer, _) = wire::split(sock, Arc::new(Traffic::default()))?;

    let mut request = PunchHoleRequest::new();
    request.id = id.into();
    request.version = crate::VERSION.into();
    // A self-hosted server refuses a client that does not send its key, with
    // `LICENSE_MISMATCH`: despite the name, the key is a shared secret, not a
    // licence. The public network expects it empty.
    request.licence_key = key.into();
    let mut out = RendezvousMessage::new();
    out.set_punch_hole_request(request);

    // Three attempts with a growing timeout, as RustDesk's client makes: the
    // server answers a repeated request, and the peer may be slow to answer
    // its own rendezvous connection.
    let started = Instant::now();
    for attempt in 1..=3u32 {
        writer.send(&out)?;
        reader.set_timeout(Some(Duration::from_secs(3 * attempt as u64)))?;
        let message = match next_message(&mut reader) {
            Ok(m) => m,
            Err(e) if would_block(&e) => continue,
            Err(e) => return Err(Error::Io(e)),
        };
        match message.union {
            Some(rendezvous_message::Union::PunchHoleResponse(response)) => {
                if response.socket_addr.is_empty() {
                    return Err(Error::Refused(why(
                        &response.failure.enum_value_or_default(),
                        &response.other_failure,
                    )));
                }
                let peer = unmangle(&response.socket_addr)?;
                log::info!(
                    "rendezvous: {id} is at {peer}, {} ms{}",
                    started.elapsed().as_millis(),
                    if response.is_local() {
                        ", on this network"
                    } else {
                        ""
                    }
                );
                let signed_id_pk = response.pk.to_vec();
                // How long to wait for a punch that may never succeed.
                // RustDesk's client computes this from information this end
                // does not have; this uses its fallback, six times the round
                // trip the introduction took. That is a second or two rather
                // than the twenty of a connection timeout, and the difference
                // is how long the user waits before the relay is tried. With no
                // relay to fall back to, the whole timeout is used instead.
                let punch = if response.relay_server.is_empty() {
                    timeout
                } else {
                    (started.elapsed() * 6).clamp(Duration::from_secs(1), timeout)
                };
                return match connect_reusable(hangup, peer, Some(local), punch) {
                    Ok(sock) => Ok(Reached {
                        sock,
                        signed_id_pk,
                        how: How::Punched(peer),
                    }),
                    // A failed punch is normal rather than a fault, and this
                    // end then asks for a relay: the peer is still connected
                    // to the server, waiting to be told where to meet.
                    Err(e) if !response.relay_server.is_empty() && !hangup.is_hung_up() => {
                        log::info!("rendezvous: no way through to {peer} ({e}), asking for a relay");
                        let sock = ask_for_relay(
                            hangup,
                            id,
                            server_addr,
                            &response.relay_server,
                            key,
                            timeout,
                            !signed_id_pk.is_empty(),
                        )?;
                        Ok(Reached {
                            sock,
                            signed_id_pk,
                            how: How::Relayed(response.relay_server),
                        })
                    }
                    Err(e) => Err(Error::Io(e)),
                };
            }
            // The peer asked for a relay itself, as a peer behind a NAT it
            // knows it cannot punch through does. It is already waiting there,
            // so this end meets it rather than dialling the peer.
            Some(rendezvous_message::Union::RelayResponse(response)) => {
                let relay = if response.relay_server.is_empty() {
                    server.to_string()
                } else {
                    response.relay_server.clone()
                };
                log::info!("rendezvous: {id} asked for a relay at {relay}");
                let sock = meet_relay(hangup, id, &response.uuid, &relay, key, timeout)?;
                return Ok(Reached {
                    sock,
                    signed_id_pk: response.pk().to_vec(),
                    how: How::Relayed(relay),
                });
            }
            Some(_) | None => {}
        }
    }
    Err(Error::Refused(
        "The rendezvous server did not answer.".into(),
    ))
}

/// A connection whose local port can be reused, which hole punching needs: the
/// peer is dialled from the port the rendezvous server saw, since that is the
/// hole the peer was told to expect a packet on.
///
/// **The socket to the rendezvous server has to be opened this way too.**
/// `SO_REUSEPORT` must be set on every socket sharing the address, so binding a
/// port held by an ordinary connection fails with "address in use" however the
/// second socket is set up. That happened against a peer on the same machine
/// before both sockets were opened this way.
fn connect_reusable(
    hangup: &Hangup,
    peer: SocketAddr,
    local: Option<SocketAddr>,
    timeout: Duration,
) -> std::io::Result<TcpStream> {
    use socket2::{Domain, Protocol, SockAddr, Socket, Type};

    let domain = if peer.is_ipv4() {
        Domain::IPV4
    } else {
        Domain::IPV6
    };
    let sock = Socket::new(domain, Type::STREAM, Some(Protocol::TCP))?;
    // Both, as RustDesk's client sets both: one is not enough to bind a port a
    // live connection is using, and the other is not enough once it has
    // closed.
    let _ = sock.set_reuse_port(true);
    let _ = sock.set_reuse_address(true);
    let local = local.unwrap_or(if peer.is_ipv4() {
        SocketAddr::from(([0, 0, 0, 0], 0))
    } else {
        SocketAddr::from(([0u16; 8], 0))
    });
    sock.bind(&SockAddr::from(local))?;
    // Returned in blocking mode, as the callers expect: they read with their
    // own timeouts.
    let stream = hangup.dial_from(sock, peer, timeout)?;
    stream.set_nodelay(true)?;
    Ok(stream)
}

/// Requests a relay: a second connection to the rendezvous server carrying a
/// `RequestRelay`, which it forwards to the peer, and then the relay itself.
///
/// A fresh socket rather than the one used for the punch, as RustDesk's server
/// requires: it tells attempts apart by their source address.
///
/// `vouched` is whether the server introduced the peer with a signed key.
#[allow(clippy::too_many_arguments)]
fn ask_for_relay(
    hangup: &Hangup,
    id: &str,
    server: SocketAddr,
    relay: &str,
    key: &str,
    timeout: Duration,
    vouched: bool,
) -> Result<TcpStream> {
    let uuid = uuid();
    let sock = hangup.dial(server, timeout)?;
    let _holding = hangup.hold(&sock)?;
    sock.set_nodelay(true)?;
    let (mut reader, mut writer, _) = wire::split(sock, Arc::new(Traffic::default()))?;
    let mut request = RequestRelay::new();
    request.id = id.into();
    request.uuid = uuid.clone();
    request.relay_server = relay.into();
    request.licence_key = key.into();
    // Whether the peer should open with its signed key. As in RustDesk's
    // client, only when the server vouched for one: without that there is
    // nothing to check the key against, and `Client::secure` goes in the
    // clear either way, so asking would only have the peer send a `SignedId`
    // that is skipped.
    request.secure = vouched;
    let mut out = RendezvousMessage::new();
    out.set_request_relay(request);
    writer.send(&out)?;
    reader.set_timeout(Some(timeout))?;
    match next_message(&mut reader)?.union {
        Some(rendezvous_message::Union::RelayResponse(response)) => {
            if !response.refuse_reason.is_empty() {
                return Err(Error::Refused(response.refuse_reason));
            }
            let relay = if response.relay_server.is_empty() {
                relay.to_string()
            } else {
                response.relay_server
            };
            meet_relay(hangup, id, &uuid, &relay, key, timeout)
        }
        // Logged by name: a rendezvous server that answers with anything else
        // has decided not to relay, and the message it sent instead is the
        // only indication of why.
        other => {
            log::info!("rendezvous: no relay, {}", named(&other));
            Err(Error::Refused("The relay was not offered.".into()))
        }
    }
}

/// The next relevant message, which is not always the next one on the socket:
/// the public network starts by offering a key exchange, which protects a
/// token this client does not use. RustDesk's client skips such a message, and
/// so does this.
fn next_message(reader: &mut wire::Reader) -> std::io::Result<RendezvousMessage> {
    for _ in 0..2 {
        let message = reader.message::<RendezvousMessage>()?;
        if !matches!(
            message.union,
            Some(rendezvous_message::Union::KeyExchange(_))
        ) {
            return Ok(message);
        }
    }
    reader.message::<RendezvousMessage>()
}

/// Which arm arrived, for a log line about one that was not expected.
fn named(union: &Option<rendezvous_message::Union>) -> &'static str {
    match union {
        Some(rendezvous_message::Union::PunchHoleResponse(_)) => "a punch hole response",
        Some(rendezvous_message::Union::RelayResponse(_)) => "a relay response",
        Some(rendezvous_message::Union::ConfigureUpdate(_)) => "a configuration update",
        Some(rendezvous_message::Union::RegisterPeerResponse(_)) => "a peer registration",
        Some(rendezvous_message::Union::PunchHole(_)) => "a punch hole",
        Some(rendezvous_message::Union::RequestRelay(_)) => "a relay request",
        Some(rendezvous_message::Union::FetchLocalAddr(_)) => "a local address request",
        Some(rendezvous_message::Union::KeyExchange(_)) => "a key exchange",
        Some(_) => "something else",
        None => "an empty message",
    }
}

/// Connects to the relay with the uuid both ends were given, which is all the
/// relay needs to pair them. Everything after that frame is the session, which
/// the relay forwards without being able to read it: the key is agreed with
/// the peer and never passes through the relay.
fn meet_relay(
    hangup: &Hangup,
    id: &str,
    uuid: &str,
    relay: &str,
    key: &str,
    timeout: Duration,
) -> Result<TcpStream> {
    let addr = resolve(relay, RELAY_PORT)?;
    log::info!("relay: meeting {id} at {addr}");
    // RustDesk's relay pairs two sockets by looking up the uuid and, if
    // nothing is there, registering itself, taking the lock separately for
    // each step. Two ends that arrive at the same moment therefore both look,
    // both find nothing, and both wait for the other; the connection then dies
    // thirty seconds later having relayed nothing. This is an upstream bug
    // that does happen: with the peer on the same machine as this end, every
    // attempt landed inside that window and none paired. Arriving deliberately
    // late costs a quarter of a second once and orders the two.
    std::thread::sleep(RELAY_PAIR_DELAY);
    let sock = hangup.dial(addr, timeout)?;
    sock.set_nodelay(true)?;
    let session = sock.try_clone()?;
    let (_, mut writer, _) = wire::split(sock, Arc::new(Traffic::default()))?;
    let mut request = RequestRelay::new();
    request.id = id.into();
    request.uuid = uuid.into();
    request.licence_key = key.into();
    let mut out = RendezvousMessage::new();
    out.set_request_relay(request);
    writer.send(&out)?;
    Ok(session)
}

/// A token for pairing two sockets at a relay, which is all RustDesk's uuid is
/// used for: it is opaque at both ends and lives for the few seconds between
/// requesting a relay and arriving at it. The clock is enough, and a random
/// number generator is not worth a dependency for it.
fn uuid() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("{now:032x}")
}

/// RustDesk's address encoding: an IPv4 address and port folded into a u128
/// with a microsecond timestamp, trailing zero bytes trimmed; IPv6 is the plain
/// sixteen bytes and a port. Not encryption: it only keeps an address from
/// being obvious in a packet dump.
fn unmangle(bytes: &[u8]) -> Result<SocketAddr> {
    if bytes.len() == 18 {
        let ip: [u8; 16] = bytes[..16].try_into().unwrap();
        let port = u16::from_le_bytes(bytes[16..].try_into().unwrap());
        return Ok(SocketAddr::new(std::net::Ipv6Addr::from(ip).into(), port));
    }
    if bytes.len() > 16 {
        return Err(Error::Protocol(format!(
            "{} bytes is not one of their addresses",
            bytes.len()
        )));
    }
    let mut padded = [0u8; 16];
    padded[..bytes.len()].copy_from_slice(bytes);
    let n = u128::from_le_bytes(padded);
    let tm = (n >> 17) & (u32::MAX as u128);
    // An encoded address always holds at least the timestamp it was offset
    // by, so a subtraction that would go below zero, or an address of more
    // than 32 bits once undone, is not a valid encoding; RustDesk's decoder
    // wraps (or panics in a debug build) instead.
    let not_theirs = || Error::Protocol("the server sent an address that does not decode".into());
    let ip = (n >> 49).checked_sub(tm).and_then(|ip| u32::try_from(ip).ok()).ok_or_else(not_theirs)?;
    let ip = ip.to_le_bytes();
    // The mask includes the timestamp's low bits above the port's seventeen;
    // the cast drops them, as RustDesk's decoder does.
    let port = (n & 0xFF_FFFF).checked_sub(tm & 0xFFFF).ok_or_else(not_theirs)? as u16;
    Ok(SocketAddr::new(
        std::net::Ipv4Addr::new(ip[0], ip[1], ip[2], ip[3]).into(),
        port,
    ))
}

/// RustDesk's four refusals, as messages the user can act on. An unknown id
/// and an offline peer are the common ones, and they need different
/// responses: one is a typo and the other is a machine that is switched off.
fn why(failure: &Failure, other: &str) -> String {
    if !other.is_empty() {
        return other.into();
    }
    match failure {
        Failure::ID_NOT_EXIST => "No machine with that ID is registered.".into(),
        Failure::OFFLINE => "That machine is not connected.".into(),
        Failure::LICENSE_MISMATCH => "The rendezvous server refused the key.".into(),
        Failure::LICENSE_OVERUSE => "The rendezvous server is at its limit.".into(),
    }
}

/// A rendezvous or relay server: `host`, `host:port`, `[literal]` or
/// `[literal]:port`, as in the address field, and also an IPv6 literal without
/// brackets, which means that address on the default port. RustDesk reads
/// these servers that way (hbb_common's `check_port`), so a relay named by
/// its server or a server copied from RustDesk's settings can be one.
fn resolve(host: &str, default_port: u16) -> Result<SocketAddr> {
    let host = host.trim();
    if let Ok(ip) = host.parse::<std::net::Ipv6Addr>() {
        return Ok(SocketAddr::new(ip.into(), default_port));
    }
    let (name, port) = runtime::address::split(host, default_port, runtime::address::Ports::Plain)
        .map_err(Error::Address)?;
    (name.as_str(), port)
        .to_socket_addrs()
        .map_err(|e| Error::Protocol(format!("{host} does not resolve: {e}")))?
        .next()
        .ok_or_else(|| Error::Protocol(format!("{host} resolves to nothing")))
}

fn would_block(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RustDesk's `AddrMangle::encode`, transcribed, so that the tests below
    /// check this file's decoder against upstream's encoder rather than
    /// against itself.
    fn mangle(addr: std::net::SocketAddrV4, micros: u32) -> Vec<u8> {
        let tm = micros as u128;
        let ip = u32::from_le_bytes(addr.ip().octets()) as u128;
        let port = addr.port() as u128;
        let v = ((ip + tm) << 49) | (tm << 17) | (port + (tm & 0xFFFF));
        let bytes = v.to_le_bytes();
        let keep = bytes.iter().rposition(|b| *b != 0).map_or(0, |i| i + 1);
        bytes[..keep].to_vec()
    }

    /// The rig's peer, at the port a punched session arrives on. The timestamp
    /// makes encodings of one address differ, so several of them must decode
    /// to the same address, or the timestamp is not being removed.
    #[test]
    fn an_address_comes_back_out() {
        let addr = "10.33.0.208:21118".parse().unwrap();
        for micros in [0, 1, 999_999, u32::MAX] {
            let bytes = mangle(addr, micros);
            assert_eq!(unmangle(&bytes).unwrap(), SocketAddr::V4(addr), "{micros}");
        }
    }

    /// A folded number whose address or port is smaller than the timestamp it
    /// claims to be offset by is not a valid encoding, and is refused rather
    /// than wrapped (or, in a debug build, panicked on).
    #[test]
    fn a_number_below_its_timestamp_is_refused() {
        let trimmed = |v: u128| {
            let bytes = v.to_le_bytes();
            let keep = bytes.iter().rposition(|b| *b != 0).map_or(0, |i| i + 1);
            bytes[..keep].to_vec()
        };
        // The address part (zero) below the timestamp (one).
        assert!(matches!(unmangle(&trimmed(1 << 17)), Err(Error::Protocol(_))));
        // The address part fine, the port part (zero) below the timestamp.
        let tm = 0x80u128;
        assert!(matches!(unmangle(&trimmed((tm << 49) | (tm << 17))), Err(Error::Protocol(_))));
        // An address part more than 32 bits once the timestamp is taken off.
        assert!(matches!(unmangle(&trimmed(1 << 100)), Err(Error::Protocol(_))));
        assert!(unmangle(&[]).is_ok());
    }

    /// Every form a server may be given in. An IPv6 literal has colons
    /// whether or not a port follows, so the presence of a colon does not
    /// say whether the port was given.
    #[test]
    fn a_server_resolves_in_every_form() {
        let at = |host: &str| resolve(host, RENDEZVOUS_PORT).unwrap().to_string();
        assert_eq!(at("10.33.0.208"), "10.33.0.208:21116");
        assert_eq!(at("10.33.0.208:21200"), "10.33.0.208:21200");
        assert_eq!(at("::1"), "[::1]:21116");
        assert_eq!(at("fd00:33::208"), "[fd00:33::208]:21116");
        assert_eq!(at("[::1]"), "[::1]:21116");
        assert_eq!(at("[::1]:21200"), "[::1]:21200");
        assert_eq!(at(" [fd00:33::208]:21117 "), "[fd00:33::208]:21117");
        assert_eq!(resolve("localhost", RELAY_PORT).unwrap().port(), RELAY_PORT);
        assert_eq!(resolve("localhost:21200", RELAY_PORT).unwrap().port(), 21200);
        assert!(matches!(resolve("[::1", RELAY_PORT), Err(Error::Address(_))));
        assert!(matches!(resolve("::1:x", RELAY_PORT), Err(Error::Address(_))));
        assert!(matches!(resolve("10.33.0.208:0", RELAY_PORT), Err(Error::Address(_))));
    }

    /// An IPv6 address is the plain sixteen bytes and a port, the only length
    /// that is not a folded number.
    #[test]
    fn an_ipv6_address_is_not_folded() {
        let mut bytes = [0u8; 18];
        bytes[15] = 1;
        bytes[16..].copy_from_slice(&21118u16.to_le_bytes());
        assert_eq!(unmangle(&bytes).unwrap().to_string(), "[::1]:21118");
    }
}
