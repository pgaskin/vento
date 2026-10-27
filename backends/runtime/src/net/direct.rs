// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! This end's own TCP connection to the server: the network a session has
//! unless something else is given, and the one every other route dials its
//! first hop through.
//!
//! Its socket is the backend's, so everything about the connection is the
//! socket's own: it reports its end itself (no [`EndReason`] is recorded),
//! and the [`Liveness`] asked for is set on it.

use super::hangup::closed;
use super::{
    Connection, DatagramConnection, EndReason, Hangup, Liveness, Network, Peer, Route, Socket, Timeout, lookup_failed,
};
use socket2::{Domain, Protocol, SockAddr, Type};
use std::io;
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::os::fd::AsRawFd;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// A TCP connection from this end, resolved by the C library's resolver.
#[derive(Clone, Copy, Debug, Default)]
pub struct Direct;

impl Network for Direct {
    /// Every address `host` resolves to in turn, each given `timeout`
    /// ([`Timeout::PerAddress`]) or a share of what is left of it
    /// ([`Timeout::Whole`], [`share`]).
    fn connect(
        &self,
        host: &str,
        port: u16,
        timeout: Timeout,
        liveness: &Liveness,
        hangup: &Hangup,
    ) -> io::Result<Connection> {
        if host.is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "no address"));
        }
        let started = Instant::now();
        let addrs = resolve(host, port, timeout.lookup(), hangup)?;
        let (stream, addr) = dial_each(hangup, host, addrs, timeout, started)?;
        liveness.apply(&stream);
        Ok(connection(host, port, stream, addr))
    }

    /// The C library's lookup, without a connection.
    fn resolve(&self, host: &str, timeout: Duration, hangup: &Hangup) -> io::Result<()> {
        if host.is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "no address"));
        }
        resolve(host, 0, timeout, hangup).map(drop)
    }

    fn connect_from(
        &self,
        socket: socket2::Socket,
        addr: SocketAddr,
        timeout: Duration,
        hangup: &Hangup,
    ) -> io::Result<Connection> {
        let stream = dial_from(hangup, socket, addr, timeout)?;
        Ok(connection(&addr.ip().to_string(), addr.port(), stream, addr))
    }

    /// A UDP socket connected to the first address `host` resolves to: a
    /// datagram's connect sends nothing, so there is no answer to try the
    /// next one on.
    fn connect_datagram(
        &self,
        host: &str,
        port: u16,
        timeout: Duration,
        hangup: &Hangup,
    ) -> io::Result<DatagramConnection> {
        if host.is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "no address"));
        }
        let addr = resolve(host, port, timeout, hangup)?
            .into_iter()
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("no address for {host}")))?;
        let any: SocketAddr = match addr {
            SocketAddr::V4(_) => (std::net::Ipv4Addr::UNSPECIFIED, 0).into(),
            SocketAddr::V6(_) => (std::net::Ipv6Addr::UNSPECIFIED, 0).into(),
        };
        let socket = UdpSocket::bind(any)?;
        socket.connect(addr)?;
        let peer = Peer {
            host: host.to_string(),
            port,
            address: Some(addr),
            local: socket.local_addr().ok(),
            route: Route::Direct,
        };
        Ok(DatagramConnection {
            socket: socket.into(),
            peer,
            end: EndReason::none(),
        })
    }

    fn direct(&self) -> bool {
        true
    }
}

fn connection(host: &str, port: u16, stream: TcpStream, addr: SocketAddr) -> Connection {
    let peer = Peer {
        host: host.to_string(),
        port,
        address: Some(stream.peer_addr().unwrap_or(addr)),
        local: stream.local_addr().ok(),
        route: Route::Direct,
    };
    Connection {
        socket: Socket::Tcp(stream),
        peer,
        end: EndReason::none(),
    }
}

/// The least an address gets of what is left before `deadline`, where that
/// much is left: a share smaller than this would fail a reachable server
/// behind a slow link, so a name with many addresses runs past its deadline
/// before it starves any of them.
pub const LEAST_SHARE: Duration = Duration::from_secs(2);

/// How long the next of `remaining` addresses may take of the `left` before
/// the deadline: an even share, but at least [`LEAST_SHARE`] unless less is
/// left. A name whose addresses all drop what is sent to them fails about
/// when the deadline passes; one whose first address does and whose second
/// answers still connects, in the second's share.
pub fn share(left: Duration, remaining: usize) -> Duration {
    let even = left / u32::try_from(remaining.max(1)).unwrap_or(u32::MAX);
    even.max(LEAST_SHARE).min(left)
}

/// `addrs` in turn until one connects, each given `timeout`'s per-address
/// time or its [`share`] of what is left of the whole since `started`; the
/// last failure where none does, and `TimedOut` where a whole timeout passes
/// first.
fn dial_each(
    hangup: &Hangup,
    host: &str,
    addrs: Vec<SocketAddr>,
    timeout: Timeout,
    started: Instant,
) -> io::Result<(TcpStream, SocketAddr)> {
    let mut last = None;
    let count = addrs.len();
    for (i, addr) in addrs.into_iter().enumerate() {
        let each = match timeout {
            Timeout::PerAddress(each) => each,
            Timeout::Whole(whole) => {
                let left = (started + whole).saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Err(last.unwrap_or_else(|| io::ErrorKind::TimedOut.into()));
                }
                share(left, count - i)
            }
        };
        match dial(hangup, addr, each) {
            Ok(stream) => return Ok((stream, addr)),
            Err(_) if hangup.is_hung_up() => return Err(closed()),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("no address for {host}"))))
}

/// The addresses `host` has, as the C library's `getaddrinfo` gives them,
/// waiting at most `timeout` and ending the wait on a hang-up. A literal
/// address is itself, at once.
///
/// The lookup runs on a thread of its own, because `getaddrinfo` cannot be
/// interrupted: a hang-up or the timeout ends the wait and leaves the thread
/// to finish by itself, as the C library's own retries allow.
pub fn resolve(host: &str, port: u16, timeout: Duration, hangup: &Hangup) -> io::Result<Vec<SocketAddr>> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(vec![SocketAddr::new(ip, port)]);
    }
    let (tx, rx) = mpsc::channel::<Option<io::Result<Vec<SocketAddr>>>>();
    let wake = tx.clone();
    let _waiting = hangup.on_hang_up(Box::new(move || {
        let _ = wake.send(None);
    }))?;
    let name = host.to_string();
    std::thread::Builder::new()
        .name("vento-resolve".into())
        .spawn(move || {
            let found = (name.as_str(), port).to_socket_addrs().map(Iterator::collect).map_err(lookup_failed);
            let _ = tx.send(Some(found));
        })?;
    match rx.recv_timeout(timeout) {
        Ok(Some(found)) => found,
        Ok(None) => Err(closed()),
        Err(mpsc::RecvTimeoutError::Timeout) => Err(lookup_failed(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("{host} did not resolve within {} seconds", timeout.as_secs()),
        ))),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(io::Error::other(format!("{host} could not be looked up"))),
    }
}

/// `TcpStream::connect_timeout`, but a hang-up ends it. The socket comes back
/// blocking, as std's does.
fn dial(hangup: &Hangup, addr: SocketAddr, timeout: Duration) -> io::Result<TcpStream> {
    let socket = socket2::Socket::new(Domain::for_address(addr), Type::STREAM, Some(Protocol::TCP))?;
    dial_from(hangup, socket, addr, timeout)
}

/// [`dial`] on a socket the caller has already set up, such as one bound to a
/// particular port for a hole punch.
fn dial_from(hangup: &Hangup, socket: socket2::Socket, addr: SocketAddr, timeout: Duration) -> io::Result<TcpStream> {
    socket.set_nonblocking(true)?;
    match socket.connect(&SockAddr::from(addr)) {
        Ok(()) => {}
        Err(e) if e.raw_os_error() == Some(libc::EINPROGRESS) || e.kind() == io::ErrorKind::WouldBlock => {}
        Err(e) => return Err(e),
    }
    let stream = Socket::Tcp(TcpStream::from(socket));
    // Held only now that the connect has begun: shutting down a socket
    // before `connect` is called does not stop `connect` from using it.
    let holding = hangup.hold(&stream)?;
    let waited = wait_connected(&stream, timeout);
    drop(holding);
    let Socket::Tcp(stream) = stream else { unreachable!() };
    match waited {
        Err(_) if hangup.is_hung_up() => Err(closed()),
        Err(e) => Err(e),
        Ok(()) => {
            stream.set_nonblocking(false)?;
            Ok(stream)
        }
    }
}

/// The other half of a non-blocking connect: writable means done, and the
/// socket's pending error says how.
fn wait_connected(stream: &Socket, timeout: Duration) -> io::Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        let mut fd = libc::pollfd {
            fd: stream.as_raw_fd(),
            events: libc::POLLOUT,
            revents: 0,
        };
        let ms = left.as_millis().clamp(1, libc::c_int::MAX as u128) as libc::c_int;
        // SAFETY: one pollfd, which lives across the call.
        match unsafe { libc::poll(&mut fd, 1, ms) } {
            0 => return Err(io::ErrorKind::TimedOut.into()),
            n if n < 0 => {
                let e = io::Error::last_os_error();
                if e.kind() != io::ErrorKind::Interrupted {
                    return Err(e);
                }
            }
            _ => {
                return match stream.take_error()? {
                    Some(e) => Err(e),
                    // POLLHUP with no error is a socket shut down before it
                    // ever connected.
                    None if fd.revents & libc::POLLHUP != 0 => Err(io::ErrorKind::NotConnected.into()),
                    None => Ok(()),
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::Arc;

    fn dial_listener(hangup: &Hangup, listener: &TcpListener) -> io::Result<Connection> {
        let addr = listener.local_addr().unwrap();
        Direct.connect(
            &addr.ip().to_string(),
            addr.port(),
            Timeout::PerAddress(Duration::from_secs(5)),
            &Liveness::default(),
            hangup,
        )
    }

    #[test]
    fn a_datagram_socket_is_connected_to_the_server() {
        let server = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        server.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let addr = server.local_addr().unwrap();
        let d = Direct.connect_datagram("127.0.0.1", addr.port(), Duration::from_secs(5), &Hangup::new()).unwrap();
        assert_eq!((d.peer.address, d.peer.route), (Some(addr), Route::Direct));
        let socket = UdpSocket::from(d.socket);
        socket.send(b"ping").unwrap();
        let mut buf = [0u8; 16];
        let (n, from) = server.recv_from(&mut buf).unwrap();
        assert_eq!((&buf[..n], Some(from)), (&b"ping"[..], d.peer.local));
        let e = Direct
            .connect_datagram("vento-test.invalid", 9, Duration::from_secs(5), &Hangup::new())
            .unwrap_err();
        assert!(crate::net::is_lookup_failure(&e), "{e}");
    }

    #[test]
    fn a_dial_to_a_listener_connects() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let hangup = Hangup::new();
        let connection = dial_listener(&hangup, &listener).unwrap();
        let (accepted, from) = listener.accept().unwrap();
        assert_eq!(connection.peer.address, Some(listener.local_addr().unwrap()));
        assert_eq!(connection.peer.local, Some(from));
        assert_eq!(connection.peer.route, Route::Direct);
        assert_eq!(connection.peer.host, "127.0.0.1");
        assert_eq!(connection.peer.port, listener.local_addr().unwrap().port());
        // Blocking again, as std's connect leaves it.
        let stream = connection.socket;
        stream.set_read_timeout(Some(Duration::from_millis(10))).unwrap();
        let mut byte = [0u8];
        let e = std::io::Read::read(&mut &stream, &mut byte).unwrap_err();
        assert!(matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut));
        drop(accepted);
    }

    #[test]
    fn a_name_resolves_and_connects() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let connection = Direct.connect(
            "localhost",
            port,
            Timeout::PerAddress(Duration::from_secs(5)),
            &Liveness::default(),
            &Hangup::new(),
        );
        // `localhost` may come first as ::1, where nothing listens; the
        // next address is tried.
        let connection = connection.unwrap();
        assert_eq!(connection.peer.host, "localhost");
        assert_eq!(connection.peer.address, Some(listener.local_addr().unwrap()));
    }

    #[test]
    fn a_refused_dial_says_so() {
        let addr = {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap()
        };
        let e = Direct
            .connect(
                "127.0.0.1",
                addr.port(),
                Timeout::PerAddress(Duration::from_secs(5)),
                &Liveness::default(),
                &Hangup::new(),
            )
            .err()
            .unwrap();
        assert_eq!(e.kind(), io::ErrorKind::ConnectionRefused);
    }

    /// The reason this exists: a dial that would sit out its timeout ends when
    /// the session is hung up. The address is one that drops what is sent to
    /// it; where the network says at once that it is unreachable instead, the
    /// dial ends at once too, and the bound below still holds.
    #[test]
    fn a_hang_up_ends_a_dial_in_flight() {
        let hangup = Arc::new(Hangup::new());
        let started = Instant::now();
        let dialling = {
            let hangup = Arc::clone(&hangup);
            std::thread::spawn(move || {
                Direct.connect(
                    "10.255.255.1",
                    9,
                    Timeout::PerAddress(Duration::from_secs(20)),
                    &Liveness::default(),
                    &hangup,
                )
            })
        };
        std::thread::sleep(Duration::from_millis(200));
        hangup.hang_up();
        let e = dialling.join().unwrap().err().unwrap();
        let took = started.elapsed();
        assert!(took < Duration::from_secs(3), "took {took:?}");
        // Still dialling when it was hung up, so it was the hang-up that ended it.
        if took >= Duration::from_millis(200) {
            assert_eq!(e.kind(), io::ErrorKind::Interrupted, "{e}");
        }
    }

    #[test]
    fn nothing_is_dialled_after_a_hang_up() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let hangup = Hangup::new();
        hangup.hang_up();
        let e = dial_listener(&hangup, &listener).err().unwrap();
        assert_eq!(e.kind(), io::ErrorKind::Interrupted);
    }

    /// A lookup the resolver is slow to answer is a wait a hang-up ends. No
    /// resolver is slow on demand, so the wait is modelled by the wake alone:
    /// `resolve` returns as soon as it is woken, whatever the lookup thread
    /// is doing.
    #[test]
    fn a_hang_up_ends_a_lookup() {
        let hangup = Arc::new(Hangup::new());
        let started = Instant::now();
        let looking = {
            let hangup = Arc::clone(&hangup);
            // Reserved for documentation (RFC 6761): never resolves, and a
            // resolver may take its time saying so.
            std::thread::spawn(move || resolve("vento-test.invalid", 1, Duration::from_secs(20), &hangup))
        };
        hangup.hang_up();
        let looked = looking.join().unwrap();
        assert!(started.elapsed() < Duration::from_secs(3));
        // Either the hang-up came first, or the resolver said at once that
        // the name does not exist.
        if let Err(e) = looked {
            assert!(e.kind() == io::ErrorKind::Interrupted || e.raw_os_error().is_none(), "{e}");
        }
    }

    /// A name with no address says so as a lookup failure, which a refused
    /// or timed-out connection is not, in the resolver's own words.
    #[test]
    fn a_failed_lookup_is_marked() {
        use crate::net::is_lookup_failure;
        let e = resolve("vento-test.invalid", 1, Duration::from_secs(20), &Hangup::new()).unwrap_err();
        assert!(is_lookup_failure(&e), "{e:?}");
        assert!(e.to_string().starts_with("failed to lookup address information"), "{e}");
        assert!(!is_lookup_failure(&io::ErrorKind::ConnectionRefused.into()));
        assert!(!is_lookup_failure(&io::ErrorKind::TimedOut.into()));
    }

    #[test]
    fn a_share_is_even_but_at_least_two_seconds_of_what_is_left() {
        assert_eq!(share(Duration::from_secs(20), 2), Duration::from_secs(10));
        assert_eq!(share(Duration::from_secs(20), 1), Duration::from_secs(20));
        assert_eq!(share(Duration::from_secs(20), 40), LEAST_SHARE);
        assert_eq!(share(Duration::from_millis(900), 3), Duration::from_millis(900));
        assert_eq!(share(Duration::ZERO, 3), Duration::ZERO);
        assert_eq!(share(Duration::from_secs(9), 0), Duration::from_secs(9));
    }

    /// Addresses that drop what is sent to them (where the network does not
    /// say at once that they are unreachable): a whole timeout ends the dial
    /// about when it passes, however many there are.
    #[test]
    fn a_whole_timeout_ends_the_dial_whatever_the_number_of_addresses() {
        let addrs: Vec<SocketAddr> = (1..=4).map(|n| SocketAddr::from(([10, 255, 255, n], 9))).collect();
        let started = Instant::now();
        let whole = Timeout::Whole(Duration::from_millis(900));
        let e = dial_each(&Hangup::new(), "black.hole", addrs, whole, started).unwrap_err();
        let took = started.elapsed();
        assert!(took < Duration::from_millis(1500), "took {took:?}");
        if took >= Duration::from_millis(800) {
            assert_eq!(e.kind(), io::ErrorKind::TimedOut, "{e}");
        }
        // A whole timeout already spent (by the lookup) dials nothing.
        let started = Instant::now();
        let spent = started - Duration::from_secs(1);
        let e = dial_each(&Hangup::new(), "x", vec![([10, 255, 255, 1], 9).into()], whole, spent).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_millis(100));
    }

    /// A per-address timeout is each address's own: the same addresses take
    /// about the timeout each, and a lookup's time is not counted.
    #[test]
    fn a_per_address_timeout_is_given_to_each_address() {
        let addrs: Vec<SocketAddr> = (1..=2).map(|n| SocketAddr::from(([10, 255, 255, n], 9))).collect();
        let started = Instant::now();
        let each = Timeout::PerAddress(Duration::from_millis(500));
        let spent = started - Duration::from_secs(1);
        let e = dial_each(&Hangup::new(), "black.hole", addrs, each, spent).unwrap_err();
        let took = started.elapsed();
        assert!(took < Duration::from_millis(1600), "took {took:?}");
        if took >= Duration::from_millis(450) {
            assert!(took >= Duration::from_millis(900), "took {took:?}: not both addresses");
            assert_eq!(e.kind(), io::ErrorKind::TimedOut, "{e}");
        }
    }

    /// A name whose first address drops what is sent to it and whose second
    /// answers connects, in the second address's share of the time left.
    #[test]
    fn a_later_address_gets_its_share() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let live = listener.local_addr().unwrap();
        let started = Instant::now();
        let (stream, addr) = dial_each(
            &Hangup::new(),
            "half.dead",
            vec![([10, 255, 255, 1], 9).into(), live],
            Timeout::Whole(2 * LEAST_SHARE),
            started,
        )
        .unwrap();
        let took = started.elapsed();
        assert_eq!((addr, stream.peer_addr().unwrap()), (live, live));
        assert!(took < 2 * LEAST_SHARE, "took {took:?}");
        drop(listener.accept().unwrap());
    }

    /// The route's lookup alone: no connection, the same marking of a
    /// failure, and a hang-up ends it.
    #[test]
    fn a_name_is_resolved_without_a_connection() {
        use crate::net::is_lookup_failure;
        let hangup = Hangup::new();
        Direct.resolve("localhost", Duration::from_secs(20), &hangup).unwrap();
        Direct.resolve("::1", Duration::from_secs(20), &hangup).unwrap();
        let e = Direct.resolve("vento-test.invalid", Duration::from_secs(20), &hangup).unwrap_err();
        assert!(is_lookup_failure(&e), "{e:?}");
        assert_eq!(
            Direct.resolve("", Duration::from_secs(20), &hangup).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        hangup.hang_up();
        let e = Direct.resolve("localhost", Duration::from_secs(20), &hangup).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::Interrupted);
        // Still a literal's own answer, which needs no lookup.
        Direct.resolve("10.0.0.5", Duration::from_secs(20), &hangup).unwrap();
    }

    /// A slow lookup ends at the hang-up, as a connect's does.
    #[test]
    fn a_hang_up_ends_a_resolve_in_flight() {
        let hangup = Arc::new(Hangup::new());
        let started = Instant::now();
        let looking = {
            let hangup = Arc::clone(&hangup);
            std::thread::spawn(move || Direct.resolve("vento-test.invalid", Duration::from_secs(20), &hangup))
        };
        hangup.hang_up();
        let looked = looking.join().unwrap();
        assert!(started.elapsed() < Duration::from_secs(3));
        if let Err(e) = looked {
            assert!(e.kind() == io::ErrorKind::Interrupted || e.raw_os_error().is_none(), "{e}");
        }
    }

    #[test]
    fn a_literal_resolves_to_itself() {
        let hangup = Hangup::new();
        assert_eq!(
            resolve("::1", 5900, Duration::from_secs(1), &hangup).unwrap(),
            vec!["[::1]:5900".parse().unwrap()]
        );
        // Without a lookup, so even after a hang-up.
        hangup.hang_up();
        assert_eq!(
            resolve("10.0.0.5", 1, Duration::from_secs(1), &hangup).unwrap(),
            vec!["10.0.0.5:1".parse().unwrap()]
        );
    }
}
