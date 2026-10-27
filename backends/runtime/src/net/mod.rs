// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The connections a session makes: a trait a platform implements, as it
//! implements `media::video::Decoders` and `media::audio::Outputs`, so that
//! where a backend's bytes go (this end's own TCP connection, or a proxy or a
//! tunnel) is decided outside the backend.
//!
//! **What a backend gets is a connected fd** ([`Socket`]), never a stream
//! object: a TCP socket where this end's own connection reaches the server,
//! and one end of a socketpair where vento carries the bytes itself. Every
//! library here reads, writes, polls and runs its TLS on an fd, so either
//! kind goes through it unchanged, and a hang-up is still a `shutdown` on it
//! ([`Hangup`]). What differs is what the socket says about itself, so the
//! rest travels beside it in the [`Connection`].
//!
//! [`Direct`] is the network a session has unless it is given another, and
//! what the others dial their first hop through. [`Socketpair`] relays every
//! connection through a socketpair, as a tunnel would, to test what the
//! backends need of a socket that is not TCP. The C backends get the same
//! network as the table in `vento/net.h` (`api::c::net`), with the same rules.
//!
//! # What a backend must do
//!
//! - **Every connection through the session's network.** A backend never
//!   opens a socket or looks a name up itself. Where a library would (a
//!   second connection, a listening socket, UDP, a lookup of its own), the
//!   backend's glue routes it through the network or, where it cannot, refuses
//!   it with an error the user sees when [`Network::direct`] is false, rather
//!   than let it leave this machine around the route the user chose. A
//!   library that looks a name up before it connects (RealVNC's core) asks
//!   [`Network::resolve`], which opens nothing; the connect resolves the
//!   name again, so a name that goes away between the two fails there.
//! - **The socket is TCP or `AF_UNIX`.** Both are `SOCK_STREAM`. TCP options
//!   (`TCP_NODELAY`, keep-alive timing, `TCP_USER_TIMEOUT`) fail with
//!   `EOPNOTSUPP` on the second kind and are hints: [`Socket::set_nodelay`]
//!   ignores them there, a C library's failure to set one is to be ignored.
//!   `SO_RCVTIMEO`/`SO_SNDTIMEO`, `O_NONBLOCK`, `MSG_PEEK`, `SO_ERROR`,
//!   `shutdown` and `dup` work on both.
//! - **Addresses come from the [`Peer`]**, not from `getpeername` or
//!   `getsockname`, which on a socketpair name no IP address. Either address
//!   may be `None` (a tunnel that resolves at its far end has neither), and
//!   the backend must still work: IronRDP and FreeRDP send `0.0.0.0` as the
//!   client address, RustDesk logs in with the name as typed.
//! - **Liveness is asked for at the connect** ([`Liveness`]), not only set on
//!   the socket: on a relayed route the socket the backend holds is not the
//!   one that reaches the server.
//! - **Why the connection ended is asked of the route** ([`Connection::end`],
//!   [`EndReason::explain`]) when the backend's stream ends or fails, since a
//!   socketpair can only say "end of stream". The backend words its message
//!   from the answer, so that a reset or a tunnel's failure reads the same
//!   whatever carried the connection.
//! - **Datagrams through [`Network::connect_datagram`]**, which a route may
//!   not carry (`Unsupported`): a backend that can fall back to a stream (as
//!   RealVNC's UDP transport falls back to TCP) does so, as when a server
//!   does not answer over UDP. The socket is a connected UDP socket or an
//!   `AF_UNIX` `SOCK_SEQPACKET` socketpair's end: each `send` is one datagram
//!   and each `recv` one, boundaries kept, but it names no IP address, takes
//!   no IP-level options and carries no ancillary data (`IP_PKTINFO`), and an
//!   error from the server's side (an ICMP port unreachable, `ECONNREFUSED`
//!   on a direct socket) reads as the end of the socketpair: the backend asks
//!   [`DatagramConnection::end`] for it, as for a stream's end.
//!
//! # What a network must do
//!
//! - [`Network::connect`]: take the name as typed and resolve it where the
//!   route resolves; bound the lookup and the dials as the [`Timeout`] says;
//!   end a lookup or a dial at once on a hang-up with
//!   `Interrupted`; fail with the kinds the kernel would (`ConnectionRefused`,
//!   `TimedOut`, `HostUnreachable`), and mark a failed lookup with
//!   [`lookup_failed`] so that it can be told from a failed connect.
//! - [`Network::resolve`], where the route can resolve a name without
//!   connecting: the same lookup, marked the same way, ended by a hang-up
//!   the same way, and nothing opened. Where it cannot, `Unsupported`.
//! - Hand over a blocking, close-on-exec `SOCK_STREAM` fd: a TCP socket, or a
//!   socketpair's end (not a pipe: the libraries `send` and `recv`).
//! - Fill in the [`Peer`] with what the route knows of both ends, and leave
//!   out only what it does not.
//! - Carry bytes both ways in order, with backpressure, and pass on a
//!   half-close either way; when the backend hangs up or closes its end, end
//!   the connection to the server promptly (after passing on what the backend
//!   had sent). `MSG_NOSIGNAL` on every send: on Android nothing ignores
//!   `SIGPIPE` for the process.
//! - Record why the connection ended in its [`EndReason`] **before** the
//!   backend's end can see the end of its stream (the first cause stands),
//!   and put the route's own failures in its own words ([`End::Tunnel`], or
//!   an error carrying a [`TunnelFailed`]): nothing else is shown to the
//!   user as the tunnel's failure.
//! - Apply the [`Liveness`] asked for to what reaches the server (its own TCP
//!   socket's options, or its transport's own keep-alive), and turn Nagle off
//!   on a TCP socket of its own.
//! - Say whether its fds are this end's own TCP sockets ([`Network::direct`];
//!   only then [`Network::connect_from`]), and, where they are not, what
//!   carries them ([`Network::tunnel`]), which the session's facts show.
//! - [`Network::connect_datagram`], where the route carries datagrams:
//!   the same name rules, a blocking, close-on-exec fd connected to one
//!   address of the server's, datagram boundaries kept both ways, a datagram
//!   that cannot be passed on at once dropped rather than held, an error
//!   from the server's side recorded in the connection's [`EndReason`]
//!   before the backend's end is closed, and the backend's close of its end
//!   ending whatever carries them.
//! - Any thread, several connections at once (SPICE dials four or five).

mod direct;
mod end;
mod hangup;
mod socket;
mod socketpair;

pub use direct::{Direct, LEAST_SHARE, resolve, share};
pub use end::{End, EndReason, TunnelFailed};
pub use hangup::{Hangup, Holding};
pub use socket::Socket;
pub use socketpair::Socketpair;

use std::io;
use std::net::SocketAddr;
use std::os::fd::OwnedFd;
use std::time::Duration;

/// Where a session's connections go. One per session, shared by every
/// connection it makes, and called on whichever thread dials. The module's
/// documentation says what an implementation must do.
pub trait Network: Send + Sync {
    /// A connected stream to `host`:`port`, or why there is none.
    ///
    /// `host` is the name as the user gave it (without an IPv6 literal's
    /// brackets), since resolving it is the route's business: a tunnel
    /// resolves at its far end. `timeout` bounds the lookup and then each
    /// address's dial ([`Timeout::PerAddress`]), or the lookup and every
    /// address together ([`Timeout::Whole`]); a hang-up of `hangup` ends
    /// the lookup or the dial at once, with [`io::ErrorKind::Interrupted`].
    /// `liveness` is what the backend would have set on its own TCP socket
    /// to notice a server that has gone away.
    ///
    /// The socket comes back blocking, with no timeouts set.
    fn connect(
        &self,
        host: &str,
        port: u16,
        timeout: Timeout,
        liveness: &Liveness,
        hangup: &Hangup,
    ) -> io::Result<Connection>;

    /// A connection from a TCP socket the caller has set up (bound to a
    /// local port, for RustDesk's hole punch), to an address the caller has
    /// resolved. Only where this end's own TCP socket reaches the server
    /// ([`direct`](Network::direct)); elsewhere `Unsupported`.
    fn connect_from(
        &self,
        socket: socket2::Socket,
        addr: SocketAddr,
        timeout: Duration,
        hangup: &Hangup,
    ) -> io::Result<Connection> {
        let _ = (socket, addr, timeout, hangup);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "this network does not connect from a socket of the caller's",
        ))
    }

    /// Whether `host` has an address where the route resolves names, without
    /// connecting to it: `Ok(())` where it has; a failure marked with
    /// [`lookup_failed`] where it has none or the lookup took longer than
    /// `timeout`; `Interrupted` on a hang-up of `hangup`; `Unsupported`
    /// where the route resolves names only as it connects (the default: a
    /// tunnel whose far end takes a name and answers with a connection or a
    /// refusal), and the caller goes on as if the name resolved and learns
    /// at the connect.
    ///
    /// The addresses are not given back: the caller never dials them itself
    /// ([`connect`](Network::connect) resolves the name again, and may find
    /// something else by then), and a route that resolves at its far end
    /// has none this end could use. A literal address resolves to itself,
    /// without a lookup, on every route.
    fn resolve(&self, host: &str, timeout: Duration, hangup: &Hangup) -> io::Result<()> {
        let _ = (host, timeout, hangup);
        Err(io::Error::new(io::ErrorKind::Unsupported, "this tunnel resolves names only as it connects"))
    }

    /// A connected datagram socket to `host`:`port`, or why there is none:
    /// `Unsupported` where the route carries streams only (the default). The
    /// name rules are [`connect`](Network::connect)'s, `timeout` bounds the
    /// lookup, and nothing is sent: a datagram route has no handshake. The
    /// fd is blocking and close-on-exec.
    fn connect_datagram(
        &self,
        host: &str,
        port: u16,
        timeout: Duration,
        hangup: &Hangup,
    ) -> io::Result<DatagramConnection> {
        let _ = (host, port, timeout, hangup);
        Err(io::Error::new(io::ErrorKind::Unsupported, "this tunnel carries streams only"))
    }

    /// Whether a connection is this end's own TCP socket to the server, so
    /// that what is bound locally (a port the server saw) means something
    /// there, and [`connect_from`](Network::connect_from) works. Where this
    /// is false, nothing of the session's may reach the server except through
    /// [`connect`](Network::connect).
    fn direct(&self) -> bool {
        false
    }

    /// What carries the connections, where this end's own TCP socket does
    /// not: the kind of route and what it was set up with, in one line, as
    /// the session's "Tunnel" fact shows it ("dev-socketpair"; a WireGuard
    /// peer would say "wireguard peer=5.6.7.8 local=1.2.3.4"). `None` for a
    /// direct network.
    fn tunnel(&self) -> Option<String> {
        None
    }
}

/// How long a connect may take, where a name has several addresses and the
/// route tries them in turn.
///
/// [`PerAddress`](Timeout::PerAddress) is what the libraries did with their
/// own sockets (each address its own full timeout, so a name with N
/// addresses that drop what is sent to them takes N times as long to fail),
/// and what every backend asks for but SPICE, which bounded a channel's
/// whole dial before it went through the network and asks for
/// [`Whole`](Timeout::Whole).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Timeout {
    /// The lookup, and then each address's dial, may each take this long.
    PerAddress(Duration),
    /// The lookup and every address tried, together, take at most this
    /// long: a name whose addresses all drop what is sent to them fails
    /// about when it passes. A route that tries the addresses in turn gives
    /// each a share of what is left, as [`Direct`] does with [`share`].
    Whole(Duration),
}

impl Timeout {
    /// What the lookup may take: all of it, either way.
    pub fn lookup(self) -> Duration {
        match self {
            Timeout::PerAddress(t) | Timeout::Whole(t) => t,
        }
    }
}

/// What a backend would have set on its own TCP socket to notice a server
/// that has gone away without a word (a cable pulled, a machine powered off).
/// Given with each connect, so that a route that carries the bytes itself
/// can set it on its own connection to the server, where it means something:
/// on the socketpair's end the backend holds, these options do not exist.
///
/// [`Direct`] sets it on the socket it hands over, so a backend can leave it
/// to the connect on any route. FreeRDP is the only backend here that asks
/// for any (`TcpKeepAlive*`, `TcpAckTimeout`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Liveness {
    /// `SO_KEEPALIVE` with this timing.
    pub keepalive: Option<Keepalive>,
    /// `TCP_USER_TIMEOUT`: how long sent data may go unacknowledged before
    /// the connection is given up.
    pub user_timeout: Option<Duration>,
}

/// Keep-alive probes: the first after `idle` without traffic, then every
/// `interval`, giving up after `count` unanswered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Keepalive {
    pub idle: Duration,
    pub interval: Duration,
    pub count: u32,
}

impl Liveness {
    /// Sets these options on `socket`, a TCP socket. A failure is logged and
    /// otherwise ignored: the options are a hint, as they are to the
    /// libraries that set them.
    pub fn apply(&self, socket: &impl std::os::fd::AsRawFd) {
        let fd = socket.as_raw_fd();
        let set = |level: libc::c_int, name: libc::c_int, what: &str, value: u64| {
            let value = value.min(libc::c_int::MAX as u64) as libc::c_int;
            // SAFETY: an int and its size, living across the call.
            let set = unsafe {
                libc::setsockopt(
                    fd,
                    level,
                    name,
                    (&raw const value).cast(),
                    std::mem::size_of::<libc::c_int>() as libc::socklen_t,
                )
            };
            if set != 0 {
                log::warn!("could not set {what}: {}", io::Error::last_os_error());
            }
        };
        if let Some(k) = self.keepalive {
            set(libc::SOL_SOCKET, libc::SO_KEEPALIVE, "SO_KEEPALIVE", 1);
            set(libc::IPPROTO_TCP, libc::TCP_KEEPIDLE, "TCP_KEEPIDLE", k.idle.as_secs().max(1));
            set(libc::IPPROTO_TCP, libc::TCP_KEEPINTVL, "TCP_KEEPINTVL", k.interval.as_secs().max(1));
            set(libc::IPPROTO_TCP, libc::TCP_KEEPCNT, "TCP_KEEPCNT", u64::from(k.count.max(1)));
        }
        if let Some(timeout) = self.user_timeout {
            set(
                libc::IPPROTO_TCP,
                libc::TCP_USER_TIMEOUT,
                "TCP_USER_TIMEOUT",
                timeout.as_millis() as u64,
            );
        }
    }
}

/// A connected stream, and where it went.
#[derive(Debug)]
pub struct Connection {
    pub socket: Socket,
    pub peer: Peer,
    /// Why the connection ended, where the route knows better than the
    /// socket: ask it ([`EndReason::explain`]) when the stream ends or fails.
    pub end: EndReason,
}

/// A connected datagram socket, and where it goes.
#[derive(Debug)]
pub struct DatagramConnection {
    /// A UDP socket connected to the server on a direct route; one end of an
    /// `AF_UNIX` `SOCK_SEQPACKET` socketpair where vento carries the
    /// datagrams. One datagram per `send` and per `recv` either way.
    pub socket: OwnedFd,
    pub peer: Peer,
    /// Why the route stopped carrying the datagrams, where it knows better
    /// than the socket (a socketpair end reads only its end): ask it when a
    /// relayed socket ends. Nothing for a direct socket, which reports its
    /// own errors.
    pub end: EndReason,
}

/// What is known of a connection's two ends. Asked here rather than of the
/// socket, which on a relayed route is a socketpair and has no IP addresses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Peer {
    /// The name or address the connection was asked for, as given.
    pub host: String,
    pub port: u16,
    /// The server's address as the connection reached it, where the route
    /// knows it; `None` where it does not (a tunnel that resolves the name at
    /// its far end).
    pub address: Option<SocketAddr>,
    /// The address the server sees this connection come from, where the
    /// route knows it; `None` where it does not. On a relayed route this is
    /// the relay's address, not the socketpair's.
    pub local: Option<SocketAddr>,
    pub route: Route,
}

impl Peer {
    /// `host:port`, with an IPv6 literal bracketed: the address as resolved
    /// where there is one, and the name otherwise.
    pub fn describe(&self) -> String {
        match self.address {
            Some(address) => address.to_string(),
            None if self.host.contains(':') => format!("[{}]:{}", self.host, self.port),
            None => format!("{}:{}", self.host, self.port),
        }
    }
}

/// Whether `e` is a failure to find the name's addresses, rather than to
/// reach one of them. A route that resolves names marks its lookup's errors
/// with [`lookup_failed`].
pub fn is_lookup_failure(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::NotFound || e.get_ref().is_some_and(|inner| inner.is::<LookupFailed>())
}

/// `e`, an error from a name lookup, marked for [`is_lookup_failure`]: the
/// same kind and text, since the C library's lookup errors have no kind of
/// their own to tell them apart.
pub fn lookup_failed(e: io::Error) -> io::Error {
    io::Error::new(e.kind(), LookupFailed(e))
}

#[derive(Debug)]
struct LookupFailed(io::Error);

impl std::fmt::Display for LookupFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for LookupFailed {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

/// How a connection reaches its server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    /// This end's own TCP connection.
    Direct,
    /// Bytes carried by vento between a socketpair and the server: the
    /// backend's socket is `AF_UNIX`.
    Tunnel,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsRawFd;

    fn int_option(fd: &impl AsRawFd, level: libc::c_int, name: libc::c_int) -> libc::c_int {
        let mut value: libc::c_int = 0;
        let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
        // SAFETY: an int and its length, living across the call.
        let got = unsafe { libc::getsockopt(fd.as_raw_fd(), level, name, (&raw mut value).cast(), &mut len) };
        assert_eq!(got, 0, "{}", io::Error::last_os_error());
        value
    }

    /// Timings of zero are raised to the kernel's least, one.
    #[test]
    fn liveness_of_zero_is_clamped_to_one() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let stream = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        Liveness {
            keepalive: Some(Keepalive {
                idle: Duration::ZERO,
                interval: Duration::ZERO,
                count: 0,
            }),
            user_timeout: None,
        }
        .apply(&stream);
        assert_eq!(int_option(&stream, libc::SOL_SOCKET, libc::SO_KEEPALIVE), 1);
        assert_eq!(int_option(&stream, libc::IPPROTO_TCP, libc::TCP_KEEPIDLE), 1);
        assert_eq!(int_option(&stream, libc::IPPROTO_TCP, libc::TCP_KEEPINTVL), 1);
        assert_eq!(int_option(&stream, libc::IPPROTO_TCP, libc::TCP_KEEPCNT), 1);
    }

    /// On a socket that is not TCP, what does not apply is logged and
    /// skipped, and the rest is still set.
    #[test]
    fn liveness_on_a_socketpair_sets_what_it_can() {
        let (near, _far) = std::os::unix::net::UnixStream::pair().unwrap();
        Liveness {
            keepalive: Some(Keepalive {
                idle: Duration::from_secs(5),
                interval: Duration::from_secs(2),
                count: 3,
            }),
            user_timeout: Some(Duration::from_secs(9)),
        }
        .apply(&near);
        assert_eq!(int_option(&near, libc::SOL_SOCKET, libc::SO_KEEPALIVE), 1);
    }

    #[test]
    fn a_peer_describes_what_it_knows() {
        let peer = |host: &str, address: Option<&str>| Peer {
            host: host.into(),
            port: 5900,
            address: address.map(|a| a.parse().unwrap()),
            local: None,
            route: Route::Tunnel,
        };
        assert_eq!(peer("vnc.example", Some("10.0.0.5:5900")).describe(), "10.0.0.5:5900");
        assert_eq!(peer("vnc.example", None).describe(), "vnc.example:5900");
        assert_eq!(peer("10.0.0.5", None).describe(), "10.0.0.5:5900");
        assert_eq!(peer("fe80::1", None).describe(), "[fe80::1]:5900");
    }
}
