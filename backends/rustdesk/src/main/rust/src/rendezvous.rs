// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! Reaching a peer by id: the introduction, the punch, and the relay when the
//! punch cannot happen.
//!
//! A rendezvous server holds one thing per peer — where it is and what key it
//! has — and gives both out to whoever asks for an id. It is not an account and
//! not an address book: nothing about this phone's connections is kept there,
//! and what it does is introduce two ends and get out of the way.
//!
//! Three things in here have been somebody's bug, and each is a comment where
//! it happens:
//!
//! - **The relay is not an error path.** Two peers behind symmetric NAT never
//!   punch, so a client that treats a relay as a failure gets it wrong for most
//!   of the internet.
//! - **The punch needs the port the server saw.** A peer is dialled from the
//!   local port that talked to the rendezvous server, because that is the hole
//!   the peer was told to expect a packet on; a fresh port works on a LAN and
//!   nowhere else.
//! - **The id is an address and the key is the identity.** Nine digits are
//!   re-issued when a machine's configuration is wiped, so what a session is
//!   verified against is the key the server vouched for and the id is only what
//!   was asked for.
//!
//! None of the bytes in here are counted for the panel's Transferred row: this
//! is a few hundred of them on sockets that are not the session's, and the row
//! says what the connection has moved.

use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::error::{Error, Result};
use crate::protos::rendezvous::{
    PunchHoleRequest, RendezvousMessage, RequestRelay, punch_hole_response::Failure,
    rendezvous_message,
};
use crate::wire::{self, Traffic};

/// Their default ports (`libs/hbb_common/src/config.rs`): the rendezvous
/// server, the relay beside it, and direct access two above.
pub const RENDEZVOUS_PORT: u16 = 21116;
pub const RELAY_PORT: u16 = 21117;

/// The public network's identity, compiled into every build of theirs. A peer
/// reached through it is verified against this and nothing anybody typed, which
/// makes it a pin somebody else chose rather than no pin at all.
pub const PUBLIC_SERVER: &str = "rs-ny.rustdesk.com";
pub const PUBLIC_KEY: &str = "OeVuKk5nlHiXp+APNn0Y3pC1Iwpwn44JGqrQCsWqmBw=";

/// How long to hang back before meeting a relay, and why is at the one place
/// it is used.
const RELAY_PAIR_DELAY: Duration = Duration::from_millis(250);

/// How a peer was reached, for the panel's Connection row.
pub enum How {
    /// A socket straight to the peer, at the address the server gave.
    Punched(SocketAddr),
    /// Through a relay, which carries the session's bytes and can read none of
    /// them.
    Relayed(String),
}

/// A socket to the peer, and what the rendezvous server said about it.
pub struct Reached {
    pub sock: TcpStream,
    /// The peer's long-term sign key, signed by the rendezvous server. Empty
    /// where a server answered without one, which is a server offering a
    /// session nothing can be verified against.
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

/// Ask a rendezvous server for a peer, and come back with a socket to it.
///
/// Both ways that can end are here rather than in the caller, because which one
/// happens is the network's choice rather than a setting: either the server
/// tells both ends to dial each other and this end dials, or the peer asks for
/// a relay and this end meets it there.
pub fn reach(id: &str, server: &str, key: &str, timeout: Duration) -> Result<Reached> {
    let server_addr = resolve(server, RENDEZVOUS_PORT)?;
    log::info!("rendezvous: asking {server_addr} for {id}");
    let sock = connect_reusable(server_addr, None, timeout)?;
    // The hole the peer is told to expect a packet on is the one this socket
    // opened, so the peer is dialled from this socket's own address.
    let local = sock.local_addr()?;
    let (mut reader, mut writer, _) = wire::split(sock, Arc::new(Traffic::default()))?;

    let mut request = PunchHoleRequest::new();
    request.id = id.into();
    request.version = crate::VERSION.into();
    // A self-hosted server refuses a client that does not name its key —
    // `LICENSE_MISMATCH`, which reads like a licensing feature and is a shared
    // secret standing in for one. The public network wants it empty.
    request.licence_key = key.into();
    let mut out = RendezvousMessage::new();
    out.set_punch_hole_request(request);

    // Three attempts with a window that grows, as theirs makes: their server
    // answers a request it has already seen, and the peer being prodded may be
    // a machine that is slow to answer its own rendezvous connection.
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
                // How long to wait for a punch that may never land. Theirs
                // works this out from several things it knows and this end does
                // not; what is left of that is their last resort, six times the
                // round trip the introduction itself took — which is a second
                // or two rather than the twenty a connection timeout is, and
                // the difference is how long somebody waits before the relay is
                // even tried. Where there is no relay to fall back to, waiting
                // the whole timeout is the right thing instead.
                let punch = if response.relay_server.is_empty() {
                    timeout
                } else {
                    (started.elapsed() * 6).clamp(Duration::from_secs(1), timeout)
                };
                return match connect_reusable(peer, Some(local), punch) {
                    Ok(sock) => Ok(Reached {
                        sock,
                        signed_id_pk,
                        how: How::Punched(peer),
                    }),
                    // A punch that did not land is the ordinary case rather
                    // than a fault, and asking for a relay is what this end
                    // does about it — the peer is at the server waiting to be
                    // told where to meet.
                    Err(e) if !response.relay_server.is_empty() => {
                        log::info!("rendezvous: no way through to {peer} ({e}), asking for a relay");
                        let sock =
                            ask_for_relay(id, server_addr, &response.relay_server, key, timeout)?;
                        Ok(Reached {
                            sock,
                            signed_id_pk,
                            how: How::Relayed(response.relay_server),
                        })
                    }
                    Err(e) => Err(Error::Io(e)),
                };
            }
            // The peer asked for a relay itself, which is what a peer behind a
            // NAT it knows it cannot punch through does. It is already waiting
            // there, so this end meets it rather than dialling anything.
            Some(rendezvous_message::Union::RelayResponse(response)) => {
                let relay = if response.relay_server.is_empty() {
                    server.to_string()
                } else {
                    response.relay_server.clone()
                };
                log::info!("rendezvous: {id} asked for a relay at {relay}");
                let sock = meet_relay(id, &response.uuid, &relay, key, timeout)?;
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

/// A connection whose local port can be used again, which is what a punch is
/// made of: the peer is dialled from the port the rendezvous server saw, since
/// that is the hole the peer was told to expect a packet on.
///
/// **The socket that talks to the rendezvous server has to be one of these
/// too.** `SO_REUSEPORT` is a property of every socket sharing the address, so
/// binding a port that an ordinary connection is holding fails with the address
/// in use however the second socket is set up — which is what it did, against a
/// peer on the same machine, before this was the way both are opened.
fn connect_reusable(
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
    // Both, as theirs sets both: one is not enough to bind a port a live
    // connection is using, and the other is not enough once one has closed.
    let _ = sock.set_reuse_port(true);
    let _ = sock.set_reuse_address(true);
    let local = local.unwrap_or(if peer.is_ipv4() {
        SocketAddr::from(([0, 0, 0, 0], 0))
    } else {
        SocketAddr::from(([0u16; 8], 0))
    });
    sock.bind(&SockAddr::from(local))?;
    sock.connect_timeout(&SockAddr::from(peer), timeout)?;
    let stream: TcpStream = sock.into();
    stream.set_nodelay(true)?;
    // `connect_timeout` leaves the socket non-blocking; everything above reads
    // with timeouts of its own and expects a blocking one.
    stream.set_nonblocking(false)?;
    Ok(stream)
}

/// This end asking for a relay: a second connection to the rendezvous server
/// carrying a `RequestRelay` it passes on to the peer, and then the relay
/// itself.
///
/// A fresh socket rather than the one that punched, which is their server's
/// requirement rather than a choice — it tells attempts apart by the address
/// they came from.
fn ask_for_relay(
    id: &str,
    server: SocketAddr,
    relay: &str,
    key: &str,
    timeout: Duration,
) -> Result<TcpStream> {
    let uuid = uuid();
    let sock = TcpStream::connect_timeout(&server, timeout)?;
    sock.set_nodelay(true)?;
    let (mut reader, mut writer, _) = wire::split(sock, Arc::new(Traffic::default()))?;
    let mut request = RequestRelay::new();
    request.id = id.into();
    request.uuid = uuid.clone();
    request.relay_server = relay.into();
    request.licence_key = key.into();
    // Whether what the relay carries will be encrypted. Theirs answers this
    // with whether the rendezvous server vouched for a key, and on this path it
    // always has: there is no way to reach a peer by id and then not verify it.
    request.secure = true;
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
            meet_relay(id, &uuid, &relay, key, timeout)
        }
        // Worth naming rather than reporting silence: a rendezvous server that
        // answers this with anything else is one that has decided not to relay,
        // and which message it sent instead is the whole of what it said about
        // why.
        other => {
            log::info!("rendezvous: no relay, {}", named(&other));
            Err(Error::Refused("The relay was not offered.".into()))
        }
    }
}

/// The next message that is worth reading, which is not always the next one on
/// the socket: the public network opens by offering a key exchange, which is
/// for protecting a token nothing here has. Theirs skips one such message and
/// so does this — an answer that follows a greeting is not an answer to a
/// question that was not asked.
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

/// The relay itself: a socket to it carrying the uuid both ends were given,
/// which is the whole of the introduction it needs. Everything after that frame
/// is the session, and the relay copies it without being able to read a byte —
/// the key was agreed with the peer and never went near this machine.
fn meet_relay(
    id: &str,
    uuid: &str,
    relay: &str,
    key: &str,
    timeout: Duration,
) -> Result<TcpStream> {
    let addr = resolve(relay, RELAY_PORT)?;
    log::info!("relay: meeting {id} at {addr}");
    // Their relay pairs two sockets by looking the uuid up and, finding
    // nothing, putting itself there — and it takes the lock twice to do it. Two
    // ends that arrive in the same microsecond therefore both look, both find
    // nothing, and both wait for somebody who is already waiting; the
    // connection then dies thirty seconds later having relayed nothing. It is
    // theirs and it is not hypothetical: with the peer on the same machine as
    // this end, every attempt landed inside that window and none of them
    // paired. Arriving deliberately late costs a quarter of a second once and
    // makes the two orderable.
    std::thread::sleep(RELAY_PAIR_DELAY);
    let sock = TcpStream::connect_timeout(&addr, timeout)?;
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

/// Something to pair two sockets at a server with, which is the whole of what
/// their uuid is for: it is opaque at both ends and lives for the few seconds
/// between asking for a relay and arriving at one. The clock is enough, and a
/// random number generator is not worth a dependency for it.
fn uuid() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("{now:032x}")
}

/// Their address encoding: an IPv4 address and port folded into a u128 with a
/// microsecond timestamp, trailing zero bytes trimmed; IPv6 is the plain
/// sixteen bytes and a port. Not a cipher — it keeps an address from being
/// obvious in a packet dump and nothing more.
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
    let ip = (((n >> 49) - tm) as u32).to_le_bytes();
    let port = ((n & 0xFF_FFFF) - (tm & 0xFFFF)) as u16;
    Ok(SocketAddr::new(
        std::net::Ipv4Addr::new(ip[0], ip[1], ip[2], ip[3]).into(),
        port,
    ))
}

/// Their four refusals, in words somebody reading a screen can act on. The id
/// not existing and the peer being offline are the two that happen to people,
/// and they are different things: one is a typo and one is a machine that is
/// not switched on.
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

fn resolve(host: &str, default_port: u16) -> Result<SocketAddr> {
    let with_port = if host.contains(':') {
        host.to_string()
    } else {
        format!("{host}:{default_port}")
    };
    with_port
        .to_socket_addrs()
        .map_err(|e| Error::Protocol(format!("{with_port} does not resolve: {e}")))?
        .next()
        .ok_or_else(|| Error::Protocol(format!("{with_port} resolves to nothing")))
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

    /// Their `AddrMangle::encode`, transcribed, so that what is tested below is
    /// this file's decode rather than a copy of itself.
    fn mangle(addr: std::net::SocketAddrV4, micros: u32) -> Vec<u8> {
        let tm = micros as u128;
        let ip = u32::from_le_bytes(addr.ip().octets()) as u128;
        let port = addr.port() as u128;
        let v = ((ip + tm) << 49) | (tm << 17) | (port + (tm & 0xFFFF));
        let bytes = v.to_le_bytes();
        let keep = bytes.iter().rposition(|b| *b != 0).map_or(0, |i| i + 1);
        bytes[..keep].to_vec()
    }

    /// The rig's own peer, at the port a punched session lands on. The
    /// timestamp is what makes two encodings of one address differ, so several
    /// of them decode to the same place or the timestamp is not being undone.
    #[test]
    fn an_address_comes_back_out() {
        let addr = "10.33.0.208:21118".parse().unwrap();
        for micros in [0, 1, 999_999, u32::MAX] {
            let bytes = mangle(addr, micros);
            assert_eq!(unmangle(&bytes).unwrap(), SocketAddr::V4(addr), "{micros}");
        }
    }

    /// An IPv6 address is the plain sixteen bytes and a port, which is the one
    /// length that is not a folded number.
    #[test]
    fn an_ipv6_address_is_not_folded() {
        let mut bytes = [0u8; 18];
        bytes[15] = 1;
        bytes[16..].copy_from_slice(&21118u16.to_le_bytes());
        assert_eq!(unmangle(&bytes).unwrap().to_string(), "[::1]:21118");
    }
}
