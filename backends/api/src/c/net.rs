// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The session's network as the table in `vento/net.h`, so that the C
//! backends' connections are made by the same [`Network`] as the Rust
//! backends': the libvncclient and TigerVNC shims call it in place of their
//! own dial, and FreeRDP's shim from its `TCPConnect` override, for every
//! connection the library makes (the security-protocol fallbacks and a
//! redirection included).
//!
//! Each table has a [`Hangup`] of its own, the session's: `cancel` hangs it
//! up, which ends a dial or a lookup (`connect`'s or `resolve`'s) in
//! progress and refuses any later one, as a disconnect does for a Rust
//! backend. What `connect` hands over is the
//! socket's fd, after which the table keeps only the connection's
//! [`EndReason`], under the number `vento_net_peer.connection` gives it, for
//! `ended`: a number rather than the fd, since the library may have closed
//! the fd (or another connection may have its number) by the time the
//! backend asks.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::io;
use std::net::SocketAddr;
use std::os::fd::{IntoRawFd, OwnedFd};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use runtime::net::{End, EndReason, Hangup, Keepalive, Liveness, Network, Peer, Route, Timeout};

use super::ffi;
use crate::kind::CONNECT_TIMEOUT_MS;

/// What a table's `user` points at.
struct Table {
    network: Arc<dyn Network>,
    hangup: Hangup,
    /// Every connection's end, connection `n` at `n - 1`. A session makes a
    /// handful of connections, so they are kept until the table is released.
    ends: Mutex<Vec<EndReason>>,
}

/// `network` as the table a backend's config carries. The table's `user` is
/// a box holding its own reference to `network` and the session's hang-up,
/// owned by the backend from the moment the table is handed over and
/// released through `release`.
pub fn table(network: Arc<dyn Network>) -> ffi::vento_net {
    let direct = network.direct();
    ffi::vento_net {
        user: Box::into_raw(Box::new(Table {
            network,
            hangup: Hangup::new(),
            ends: Mutex::new(Vec::new()),
        })) as *mut c_void,
        direct: direct as c_int,
        connect: Some(connect),
        connect_datagram: Some(connect_datagram),
        resolve: Some(resolve),
        ended: Some(ended),
        cancel: Some(cancel),
        release: Some(release),
    }
}

/// The table behind a `user`, borrowed.
///
/// # Safety
///
/// `user` is from a table made by [`table`], not yet released.
unsafe fn borrow<'a>(user: *mut c_void) -> &'a Table {
    unsafe { &*(user as *const Table) }
}

/// The header's code for an error from [`Network::connect`].
pub fn code(e: &io::Error) -> c_int {
    use io::ErrorKind::*;
    if runtime::net::is_lookup_failure(e) {
        return ffi::VENTO_NET_ERESOLVE;
    }
    match e.kind() {
        Interrupted => ffi::VENTO_NET_ECANCELLED,
        TimedOut => ffi::VENTO_NET_ETIMEDOUT,
        ConnectionRefused => ffi::VENTO_NET_EREFUSED,
        HostUnreachable | NetworkUnreachable => ffi::VENTO_NET_EUNREACHABLE,
        Unsupported => ffi::VENTO_NET_ENOTSUP,
        _ => ffi::VENTO_NET_EFAILED,
    }
}

/// The reason for `why`, in the words the C libraries used for it when they
/// dialled themselves: an OS error as `strerror` says it, without std's
/// "(os error N)", and a lookup failure as the resolver says it, without
/// std's "failed to lookup address information".
pub fn why(e: &io::Error) -> String {
    let text = e.to_string();
    if let Some(n) = e.raw_os_error()
        && let Some(plain) = text.strip_suffix(&format!(" (os error {n})"))
    {
        return plain.to_string();
    }
    if runtime::net::is_lookup_failure(e)
        && let Some(plain) = text.strip_prefix("failed to lookup address information: ")
    {
        return plain.to_string();
    }
    if e.get_ref().is_none() && e.kind() == io::ErrorKind::TimedOut {
        return "Connection timed out".to_string();
    }
    text
}

/// Writes `addr`'s address and port into a C buffer and an int; "" and 0 for
/// none.
fn put_address(addr: Option<SocketAddr>, out: &mut [c_char], port: &mut c_int) {
    let text = addr.map(|a| a.ip().to_string()).unwrap_or_default();
    // An IP address in text is at most 45 bytes, which the header's 64 hold.
    unsafe { super::write_string(&text, out.as_mut_ptr(), out.len()) };
    if text.is_empty() {
        out[0] = 0;
    }
    *port = addr.map_or(0, |a| a.port() as c_int);
}

fn put_peer(peer: &Peer, out: &mut ffi::vento_net_peer) {
    put_address(peer.address, &mut out.address, &mut out.port);
    put_address(peer.local, &mut out.local_address, &mut out.local_port);
    out.route = match peer.route {
        Route::Direct => ffi::VENTO_NET_DIRECT,
        Route::Tunnel => ffi::VENTO_NET_TUNNEL,
    } as c_int;
}

/// `vento_net_liveness` as [`Liveness`]; none for NULL.
fn liveness(from: Option<&ffi::vento_net_liveness>) -> Liveness {
    let Some(from) = from else {
        return Liveness::default();
    };
    let secs = |s: c_int| Duration::from_secs(s.max(0) as u64);
    Liveness {
        keepalive: (from.keepalive != 0).then(|| Keepalive {
            idle: secs(from.keepalive_idle_s),
            interval: secs(from.keepalive_interval_s),
            count: from.keepalive_count.max(0) as u32,
        }),
        user_timeout: (from.user_timeout_ms > 0).then(|| Duration::from_millis(from.user_timeout_ms as u64)),
    }
}

unsafe extern "C" fn connect(
    user: *mut c_void,
    host: *const c_char,
    port: c_int,
    timeout_ms: c_int,
    asked: *const ffi::vento_net_liveness,
    peer: *mut ffi::vento_net_peer,
    why_out: *mut c_char,
    why_len: usize,
) -> c_int {
    let table = unsafe { borrow(user) };
    let fail = |e: io::Error| {
        unsafe { super::write_string(&why(&e), why_out, why_len) };
        code(&e)
    };
    let host = if host.is_null() {
        ""
    } else {
        match unsafe { CStr::from_ptr(host) }.to_str() {
            Ok(host) => host,
            Err(_) => {
                return fail(io::Error::new(io::ErrorKind::InvalidInput, "the name is not UTF-8"));
            }
        }
    };
    let Ok(port) = u16::try_from(port) else {
        return fail(io::Error::new(io::ErrorKind::InvalidInput, format!("{port} is not a port")));
    };
    let timeout_ms = if timeout_ms > 0 { timeout_ms } else { CONNECT_TIMEOUT_MS };
    let timeout = Duration::from_millis(timeout_ms as u64);
    let liveness = liveness(unsafe { asked.as_ref() });
    match table.network.connect(host, port, Timeout::PerAddress(timeout), &liveness, &table.hangup) {
        Ok(connection) => {
            let number = {
                let mut ends = table.ends.lock().unwrap();
                ends.push(connection.end);
                ends.len() as c_int
            };
            if !peer.is_null() {
                let peer = unsafe { &mut *peer };
                put_peer(&connection.peer, peer);
                peer.connection = number;
            }
            OwnedFd::from(connection.socket).into_raw_fd()
        }
        Err(e) => fail(e),
    }
}

/// `host` and `port` from the header's arguments, or the error to fail with.
fn host_and_port<'a>(host: *const c_char, port: c_int) -> io::Result<(&'a str, u16)> {
    let host = if host.is_null() {
        ""
    } else {
        unsafe { CStr::from_ptr(host) }
            .to_str()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "the name is not UTF-8"))?
    };
    let port = u16::try_from(port)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, format!("{port} is not a port")))?;
    Ok((host, port))
}

unsafe extern "C" fn connect_datagram(
    user: *mut c_void,
    host: *const c_char,
    port: c_int,
    timeout_ms: c_int,
    peer: *mut ffi::vento_net_peer,
    why_out: *mut c_char,
    why_len: usize,
) -> c_int {
    let table = unsafe { borrow(user) };
    let fail = |e: io::Error| {
        unsafe { super::write_string(&why(&e), why_out, why_len) };
        code(&e)
    };
    let (host, port) = match host_and_port(host, port) {
        Ok(asked) => asked,
        Err(e) => return fail(e),
    };
    let timeout_ms = if timeout_ms > 0 { timeout_ms } else { CONNECT_TIMEOUT_MS };
    match table.network.connect_datagram(host, port, Duration::from_millis(timeout_ms as u64), &table.hangup) {
        Ok(datagrams) => {
            let number = {
                let mut ends = table.ends.lock().unwrap();
                ends.push(datagrams.end);
                ends.len() as c_int
            };
            if !peer.is_null() {
                let peer = unsafe { &mut *peer };
                put_peer(&datagrams.peer, peer);
                peer.connection = number;
            }
            datagrams.socket.into_raw_fd()
        }
        Err(e) => fail(e),
    }
}

unsafe extern "C" fn resolve(
    user: *mut c_void,
    host: *const c_char,
    timeout_ms: c_int,
    why_out: *mut c_char,
    why_len: usize,
) -> c_int {
    let table = unsafe { borrow(user) };
    let fail = |e: io::Error| {
        unsafe { super::write_string(&why(&e), why_out, why_len) };
        code(&e)
    };
    let (host, _) = match host_and_port(host, 0) {
        Ok(asked) => asked,
        Err(e) => return fail(e),
    };
    let timeout_ms = if timeout_ms > 0 { timeout_ms } else { CONNECT_TIMEOUT_MS };
    match table.network.resolve(host, Duration::from_millis(timeout_ms as u64), &table.hangup) {
        Ok(()) => 0,
        Err(e) => fail(e),
    }
}

/// The header's code for `end`, the errno a direct socket's read would have
/// failed with, and its reason in words.
pub fn end_code(end: Option<&End>) -> (c_int, c_int, String) {
    let code = match end {
        None => ffi::VENTO_NET_END_UNKNOWN,
        Some(End::Closed) => ffi::VENTO_NET_END_CLOSED,
        Some(End::Local) => ffi::VENTO_NET_END_LOCAL,
        Some(End::Reset) => ffi::VENTO_NET_END_RESET,
        Some(End::TimedOut) => ffi::VENTO_NET_END_TIMEDOUT,
        Some(End::Os(_)) => ffi::VENTO_NET_END_ERROR,
        Some(End::Tunnel(_)) => ffi::VENTO_NET_END_TUNNEL,
    };
    let text = match end {
        Some(End::Tunnel(text)) => text.clone(),
        Some(end) => end.error().map(|e| why(&e)).unwrap_or_default(),
        None => String::new(),
    };
    (code as c_int, end.map_or(0, End::errno), text)
}

unsafe extern "C" fn ended(
    user: *mut c_void,
    connection: c_int,
    error: *mut c_int,
    why_out: *mut c_char,
    why_len: usize,
) -> c_int {
    let table = unsafe { borrow(user) };
    let end = usize::try_from(connection)
        .ok()
        .and_then(|n| n.checked_sub(1))
        .and_then(|i| table.ends.lock().unwrap().get(i).and_then(EndReason::get));
    let (code, errno, text) = end_code(end.as_ref());
    if !error.is_null() {
        unsafe { *error = errno };
    }
    unsafe { super::write_string(&text, why_out, why_len) };
    code
}

unsafe extern "C" fn cancel(user: *mut c_void) {
    unsafe { borrow(user) }.hangup.hang_up();
}

unsafe extern "C" fn release(user: *mut c_void) {
    if !user.is_null() {
        drop(unsafe { Box::from_raw(user as *mut Table) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::net::{Connection, Direct, Socket, Socketpair};
    use std::ffi::CString;
    use std::net::TcpListener;
    use std::os::fd::{AsRawFd, FromRawFd, RawFd};
    use std::time::Instant;

    fn call(t: &ffi::vento_net, host: &str, port: c_int, timeout_ms: c_int) -> (c_int, ffi::vento_net_peer, String) {
        let host = CString::new(host).unwrap();
        let mut peer = ffi::vento_net_peer::default();
        let mut why = [0 as c_char; 128];
        let fd = unsafe {
            t.connect.unwrap()(
                t.user,
                host.as_ptr(),
                port,
                timeout_ms,
                std::ptr::null(),
                &mut peer,
                why.as_mut_ptr(),
                why.len(),
            )
        };
        let why = unsafe { CStr::from_ptr(why.as_ptr()) }.to_string_lossy().into_owned();
        (fd, peer, why)
    }

    fn text(buf: &[c_char]) -> String {
        unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned()
    }

    fn owned(fd: RawFd) -> OwnedFd {
        assert!(fd >= 0, "{fd}");
        unsafe { OwnedFd::from_raw_fd(fd) }
    }

    fn call_datagram(t: &ffi::vento_net, host: &str, port: c_int) -> (c_int, ffi::vento_net_peer, String) {
        let host = CString::new(host).unwrap();
        let mut peer = ffi::vento_net_peer::default();
        let mut why = [0 as c_char; 128];
        let fd = unsafe {
            t.connect_datagram.unwrap()(t.user, host.as_ptr(), port, 5000, &mut peer, why.as_mut_ptr(), why.len())
        };
        let why = unsafe { CStr::from_ptr(why.as_ptr()) }.to_string_lossy().into_owned();
        (fd, peer, why)
    }

    fn sock_type(fd: &OwnedFd) -> c_int {
        let mut kind: c_int = 0;
        let mut len = std::mem::size_of::<c_int>() as libc::socklen_t;
        let got = unsafe {
            libc::getsockopt(fd.as_raw_fd(), libc::SOL_SOCKET, libc::SO_TYPE, (&raw mut kind).cast(), &mut len)
        };
        assert_eq!(got, 0);
        kind
    }

    #[test]
    fn datagrams_are_a_udp_socket_direct_and_a_seqpacket_end_relayed() {
        let server = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        server.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let port = server.local_addr().unwrap().port() as c_int;
        for (network, kind, route) in [
            (Arc::new(Direct) as Arc<dyn Network>, libc::SOCK_DGRAM, ffi::VENTO_NET_DIRECT),
            (Arc::new(Socketpair::new(Arc::new(Direct))), libc::SOCK_SEQPACKET, ffi::VENTO_NET_TUNNEL),
        ] {
            let t = table(network);
            let (fd, peer, why) = call_datagram(&t, "127.0.0.1", port);
            let fd = owned(fd);
            assert_eq!(sock_type(&fd), kind, "{why}");
            assert_eq!(peer.route, route as c_int);
            assert!(peer.connection > 0);
            assert_eq!(text(&peer.address), "127.0.0.1");
            assert_eq!(peer.port, port);
            let fdfl = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFD) };
            assert_ne!(fdfl & libc::FD_CLOEXEC, 0);
            let sent = unsafe { libc::send(fd.as_raw_fd(), b"hi".as_ptr().cast(), 2, 0) };
            assert_eq!(sent, 2);
            let mut buf = [0u8; 8];
            assert_eq!(server.recv(&mut buf).unwrap(), 2);
            unsafe { release(t.user) };
        }
    }

    #[test]
    fn a_route_without_datagrams_answers_enotsup() {
        struct Streams;
        impl Network for Streams {
            fn connect(&self, _: &str, _: u16, _: Timeout, _: &Liveness, _: &Hangup) -> io::Result<Connection> {
                Err(io::ErrorKind::ConnectionRefused.into())
            }
        }
        let t = table(Arc::new(Streams));
        let (fd, _, why) = call_datagram(&t, "127.0.0.1", 9);
        assert_eq!(fd, ffi::VENTO_NET_ENOTSUP, "{why}");
        assert_eq!(why, "this tunnel carries streams only");
        unsafe { release(t.user) };
    }

    #[test]
    fn a_direct_connection_is_a_tcp_fd_with_both_addresses() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let t = table(Arc::new(Direct));
        let (fd, peer, _) = call(&t, "127.0.0.1", port as c_int, 5000);
        let fd = owned(fd);
        let (_accepted, from) = listener.accept().unwrap();
        assert_eq!(text(&peer.address), "127.0.0.1");
        assert_eq!(peer.port, port as c_int);
        assert_eq!(text(&peer.local_address), "127.0.0.1");
        assert_eq!(peer.local_port, from.port() as c_int);
        assert_eq!(peer.route, ffi::VENTO_NET_DIRECT as c_int);
        // Blocking, and close-on-exec.
        let fl = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) };
        assert_eq!(fl & libc::O_NONBLOCK, 0);
        let fdfl = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFD) };
        assert_ne!(fdfl & libc::FD_CLOEXEC, 0);
        unsafe { t.release.unwrap()(t.user) };
    }

    #[test]
    fn a_relayed_connection_has_the_relays_addresses() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let t = table(Arc::new(Socketpair::new(Arc::new(Direct))));
        assert_eq!(t.direct, 0);
        let (fd, peer, _) = call(&t, "127.0.0.1", port as c_int, 5000);
        let _fd = owned(fd);
        let (_accepted, from) = listener.accept().unwrap();
        assert_eq!(text(&peer.address), "127.0.0.1");
        assert_eq!(peer.port, port as c_int);
        assert_eq!(text(&peer.local_address), "127.0.0.1");
        assert_eq!(peer.local_port, from.port() as c_int);
        assert_eq!(peer.route, ffi::VENTO_NET_TUNNEL as c_int);
        assert_eq!(peer.connection, 1);
        unsafe { t.release.unwrap()(t.user) };
    }

    #[test]
    fn a_tunnel_without_addresses_has_none() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let t = table(Arc::new(Socketpair::new(Arc::new(Direct)).hiding_addresses()));
        let (fd, peer, _) = call(&t, "127.0.0.1", port as c_int, 5000);
        let fd = owned(fd);
        let (mut accepted, _) = listener.accept().unwrap();
        assert_eq!(text(&peer.address), "");
        assert_eq!(peer.port, 0);
        assert_eq!(text(&peer.local_address), "");
        assert_eq!(peer.local_port, 0);
        assert_eq!(peer.route, ffi::VENTO_NET_TUNNEL as c_int);
        // The bytes go through.
        let socket = Socket::Unix(std::os::unix::net::UnixStream::from(fd));
        io::Write::write_all(&mut &socket, b"hi").unwrap();
        let mut got = [0u8; 2];
        io::Read::read_exact(&mut accepted, &mut got).unwrap();
        assert_eq!(&got, b"hi");
        unsafe { t.release.unwrap()(t.user) };
    }

    #[test]
    fn a_refused_connection_says_so() {
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let t = table(Arc::new(Direct));
        let (fd, _, why) = call(&t, "127.0.0.1", port as c_int, 5000);
        assert_eq!(fd, ffi::VENTO_NET_EREFUSED);
        assert_eq!(why, "Connection refused");
        unsafe { t.release.unwrap()(t.user) };
    }

    #[test]
    fn a_name_without_an_address_says_so() {
        let t = table(Arc::new(Direct));
        let (fd, _, why) = call(&t, "vento-test.invalid", 1, 20_000);
        assert_eq!(fd, ffi::VENTO_NET_ERESOLVE, "{why}");
        assert!(!why.is_empty() && !why.starts_with("failed to lookup"), "{why}");
        unsafe { t.release.unwrap()(t.user) };
    }

    /// `resolve` for `host`: the code and the reason.
    fn call_resolve(t: &ffi::vento_net, host: &str, timeout_ms: c_int) -> (c_int, String) {
        let host = CString::new(host).unwrap();
        let mut why = [0 as c_char; 128];
        let rc = unsafe { t.resolve.unwrap()(t.user, host.as_ptr(), timeout_ms, why.as_mut_ptr(), why.len()) };
        (rc, text(&why))
    }

    /// A name is resolved without a connection, through the same route as
    /// a connect, failing the same way and in the same words.
    #[test]
    fn a_name_is_resolved_without_a_connection() {
        for network in [Arc::new(Direct) as Arc<dyn Network>, Arc::new(Socketpair::new(Arc::new(Direct)))] {
            let t = table(network);
            assert_eq!(call_resolve(&t, "localhost", 0), (0, String::new()));
            assert_eq!(call_resolve(&t, "127.0.0.1", 0), (0, String::new()));
            let (rc, why) = call_resolve(&t, "vento-test.invalid", 20_000);
            assert_eq!(rc, ffi::VENTO_NET_ERESOLVE, "{why}");
            assert!(!why.is_empty() && !why.starts_with("failed to lookup"), "{why}");
            assert_eq!(call_resolve(&t, "", 0).0, ffi::VENTO_NET_EFAILED);
            // Nothing written where there is no room.
            let host = CString::new("vento-test.invalid").unwrap();
            let rc = unsafe { t.resolve.unwrap()(t.user, host.as_ptr(), 0, std::ptr::null_mut(), 0) };
            assert_eq!(rc, ffi::VENTO_NET_ERESOLVE);
            unsafe { t.release.unwrap()(t.user) };
        }
        // A route that resolves only as it connects.
        struct Connecting;
        impl Network for Connecting {
            fn connect(&self, _: &str, _: u16, _: Timeout, _: &Liveness, _: &Hangup) -> io::Result<Connection> {
                Err(io::ErrorKind::ConnectionRefused.into())
            }
        }
        let t = table(Arc::new(Connecting));
        let (rc, why) = call_resolve(&t, "localhost", 0);
        assert_eq!(rc, ffi::VENTO_NET_ENOTSUP);
        assert_eq!(why, "this tunnel resolves names only as it connects");
        unsafe { t.release.unwrap()(t.user) };
    }

    /// `cancel` ends a lookup in progress, and every later one.
    #[test]
    fn cancel_ends_a_resolve_in_flight() {
        struct SlowLookup;
        impl Network for SlowLookup {
            fn connect(&self, _: &str, _: u16, _: Timeout, _: &Liveness, _: &Hangup) -> io::Result<Connection> {
                Err(io::ErrorKind::ConnectionRefused.into())
            }
            fn resolve(&self, _: &str, timeout: Duration, hangup: &Hangup) -> io::Result<()> {
                let (woken, wait) = std::sync::mpsc::channel::<()>();
                let _held = hangup.on_hang_up(Box::new(move || {
                    let _ = woken.send(());
                }))?;
                match wait.recv_timeout(timeout) {
                    Ok(()) => Err(io::Error::new(io::ErrorKind::Interrupted, "the session was closed")),
                    Err(_) => Err(runtime::net::lookup_failed(io::ErrorKind::TimedOut.into())),
                }
            }
        }
        let t = table(Arc::new(SlowLookup));
        let user = t.user as usize;
        let resolve = t.resolve.unwrap();
        let started = Instant::now();
        let looking = std::thread::spawn(move || {
            let host = CString::new("slow.invalid").unwrap();
            unsafe { resolve(user as *mut c_void, host.as_ptr(), 20_000, std::ptr::null_mut(), 0) }
        });
        std::thread::sleep(Duration::from_millis(200));
        unsafe { t.cancel.unwrap()(t.user) };
        assert_eq!(looking.join().unwrap(), ffi::VENTO_NET_ECANCELLED);
        assert!(started.elapsed() < Duration::from_secs(3));
        assert_eq!(call_resolve(&t, "slow.invalid", 20_000).0, ffi::VENTO_NET_ECANCELLED);
        unsafe { t.release.unwrap()(t.user) };
    }

    #[test]
    fn a_bad_port_or_name_fails() {
        let t = table(Arc::new(Direct));
        assert_eq!(call(&t, "127.0.0.1", 70_000, 0).0, ffi::VENTO_NET_EFAILED);
        assert_eq!(call(&t, "", 1, 0).0, ffi::VENTO_NET_EFAILED);
        // No room for a reason: nothing written, and no harm done.
        let host = CString::new("").unwrap();
        let fd = unsafe {
            t.connect.unwrap()(
                t.user,
                host.as_ptr(),
                1,
                0,
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                0,
            )
        };
        assert_eq!(fd, ffi::VENTO_NET_EFAILED);
        unsafe { t.release.unwrap()(t.user) };
    }

    /// The point of `cancel`: a dial that would sit out its timeout ends
    /// at once, and nothing is dialled afterwards.
    #[test]
    fn cancel_ends_a_dial_and_every_later_one() {
        let t = table(Arc::new(Direct));
        let user = t.user as usize;
        let connect = t.connect.unwrap();
        let started = Instant::now();
        let dialling = std::thread::spawn(move || {
            let host = CString::new("10.255.255.1").unwrap();
            let mut why = [0 as c_char; 64];
            let fd = unsafe {
                connect(
                    user as *mut c_void,
                    host.as_ptr(),
                    9,
                    20_000,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    why.as_mut_ptr(),
                    why.len(),
                )
            };
            (fd, text(&why))
        });
        std::thread::sleep(Duration::from_millis(200));
        unsafe { t.cancel.unwrap()(t.user) };
        let (fd, why) = dialling.join().unwrap();
        let took = started.elapsed();
        assert!(took < Duration::from_secs(3), "took {took:?}");
        // Where the network says at once that the address is unreachable,
        // the dial ended before the cancel.
        if took >= Duration::from_millis(200) {
            assert_eq!(fd, ffi::VENTO_NET_ECANCELLED, "{why}");
        }

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (fd, _, _) = call(&t, "127.0.0.1", port as c_int, 5000);
        assert_eq!(fd, ffi::VENTO_NET_ECANCELLED);
        unsafe {
            t.cancel.unwrap()(t.user);
            t.release.unwrap()(t.user);
        }
    }

    /// One `cancel` ends every dial in progress on the table.
    #[test]
    fn cancel_ends_several_dials_at_once() {
        let t = table(Arc::new(Direct));
        let user = t.user as usize;
        let connect = t.connect.unwrap();
        let started = Instant::now();
        let dials: Vec<_> = (0..3)
            .map(|_| {
                std::thread::spawn(move || {
                    let host = CString::new("10.255.255.1").unwrap();
                    unsafe {
                        connect(
                            user as *mut c_void,
                            host.as_ptr(),
                            9,
                            20_000,
                            std::ptr::null(),
                            std::ptr::null_mut(),
                            std::ptr::null_mut(),
                            0,
                        )
                    }
                })
            })
            .collect();
        std::thread::sleep(Duration::from_millis(200));
        unsafe { t.cancel.unwrap()(t.user) };
        for dial in dials {
            let fd = dial.join().unwrap();
            assert!(fd < 0, "{fd}");
        }
        let took = started.elapsed();
        assert!(took < Duration::from_secs(3), "took {took:?}");
        unsafe { t.release.unwrap()(t.user) };
    }

    /// `cancel` ends a datagram dial waiting on its lookup, which a route
    /// ends at the session's hang-up, as a stream's.
    #[test]
    fn cancel_ends_a_datagram_dial_in_its_lookup() {
        struct SlowLookup;
        impl Network for SlowLookup {
            fn connect(&self, _: &str, _: u16, _: Timeout, _: &Liveness, _: &Hangup) -> io::Result<Connection> {
                Err(io::ErrorKind::ConnectionRefused.into())
            }
            fn connect_datagram(
                &self,
                _: &str,
                _: u16,
                timeout: Duration,
                hangup: &Hangup,
            ) -> io::Result<runtime::net::DatagramConnection> {
                let (woken, wait) = std::sync::mpsc::channel::<()>();
                let _held = hangup.on_hang_up(Box::new(move || {
                    let _ = woken.send(());
                }))?;
                match wait.recv_timeout(timeout) {
                    Ok(()) => Err(io::Error::new(io::ErrorKind::Interrupted, "the session was closed")),
                    Err(_) => Err(runtime::net::lookup_failed(io::ErrorKind::TimedOut.into())),
                }
            }
        }
        let t = table(Arc::new(SlowLookup));
        let user = t.user as usize;
        let connect_datagram = t.connect_datagram.unwrap();
        let started = Instant::now();
        let dial = std::thread::spawn(move || {
            let host = CString::new("slow.invalid").unwrap();
            unsafe {
                connect_datagram(
                    user as *mut c_void,
                    host.as_ptr(),
                    9,
                    20_000,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    0,
                )
            }
        });
        std::thread::sleep(Duration::from_millis(200));
        unsafe { t.cancel.unwrap()(t.user) };
        assert_eq!(dial.join().unwrap(), ffi::VENTO_NET_ECANCELLED);
        assert!(started.elapsed() < Duration::from_secs(3));
        // And every later one.
        assert_eq!(call_datagram(&t, "slow.invalid", 9).0, ffi::VENTO_NET_ECANCELLED);
        unsafe { t.release.unwrap()(t.user) };
    }

    /// A relayed datagram connection ended by the server's port unreachable
    /// says so, as `ended` says it for a stream.
    #[test]
    fn a_relayed_datagram_refusal_is_ended_with_econnrefused() {
        let port = {
            let unused = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
            unused.local_addr().unwrap().port() as c_int
        };
        let t = table(Arc::new(Socketpair::new(Arc::new(Direct))));
        let (fd, peer, why) = call_datagram(&t, "127.0.0.1", port);
        let fd = owned(fd);
        assert!(peer.connection > 0, "{why}");
        let sent = unsafe { libc::send(fd.as_raw_fd(), b"hi".as_ptr().cast(), 2, libc::MSG_NOSIGNAL) };
        assert_eq!(sent, 2);
        let mut pfd = libc::pollfd {
            fd: fd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        assert_eq!(unsafe { libc::poll(&mut pfd, 1, 5000) }, 1);
        assert_ne!(pfd.revents & libc::POLLHUP, 0);
        assert_eq!(
            ask(&t, peer.connection),
            (ffi::VENTO_NET_END_ERROR, libc::ECONNREFUSED, "Connection refused".into())
        );
        unsafe { t.release.unwrap()(t.user) };
    }

    /// The table holds a reference to the network until it is released.
    #[test]
    fn release_drops_the_network() {
        struct Counted {
            _count: Arc<()>,
        }
        impl Network for Counted {
            fn connect(&self, _: &str, _: u16, _: Timeout, _: &Liveness, _: &Hangup) -> io::Result<Connection> {
                Err(io::ErrorKind::HostUnreachable.into())
            }
        }
        let count = Arc::new(());
        let t = table(Arc::new(Counted { _count: Arc::clone(&count) }));
        assert_eq!(call(&t, "x", 1, 0).0, ffi::VENTO_NET_EUNREACHABLE);
        assert_eq!(Arc::strong_count(&count), 2);
        unsafe { t.release.unwrap()(t.user) };
        assert_eq!(Arc::strong_count(&count), 1);
    }

    /// `ended` for connection `n`: the code, the errno and the reason.
    fn ask(t: &ffi::vento_net, n: c_int) -> (u32, c_int, String) {
        let mut error = -1;
        let mut why = [0 as c_char; 128];
        let code = unsafe { t.ended.unwrap()(t.user, n, &mut error, why.as_mut_ptr(), why.len()) };
        (code as u32, error, text(&why))
    }

    /// Waits until the relay has recorded connection `n`'s end.
    fn ask_ended(t: &ffi::vento_net, n: c_int) -> (u32, c_int, String) {
        let started = Instant::now();
        loop {
            let answer = ask(t, n);
            if answer.0 != ffi::VENTO_NET_END_UNKNOWN || started.elapsed() > Duration::from_secs(5) {
                return answer;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_relayed_reset_and_close_are_told_apart() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let t = table(Arc::new(Socketpair::new(Arc::new(Direct))));
        let (reset_fd, reset, _) = call(&t, "127.0.0.1", port as c_int, 5000);
        let (reset_server, _) = listener.accept().unwrap();
        let (closed_fd, closed, _) = call(&t, "127.0.0.1", port as c_int, 5000);
        let (closed_server, _) = listener.accept().unwrap();
        let (reset_fd, closed_fd) = (owned(reset_fd), owned(closed_fd));
        assert_eq!((reset.connection, closed.connection), (1, 2));
        assert_eq!(ask(&t, 1).0, ffi::VENTO_NET_END_UNKNOWN);

        socket2::SockRef::from(&reset_server).set_linger(Some(Duration::ZERO)).unwrap();
        drop(reset_server);
        drop(closed_server);
        assert_eq!(
            ask_ended(&t, 1),
            (ffi::VENTO_NET_END_RESET, libc::ECONNRESET, "Connection reset by peer".into())
        );
        assert_eq!(ask_ended(&t, 2), (ffi::VENTO_NET_END_CLOSED, 0, String::new()));
        // Still there once the fds are closed.
        drop((reset_fd, closed_fd));
        assert_eq!(ask(&t, 1).0, ffi::VENTO_NET_END_RESET);
        // Numbers that are not connections.
        for n in [0, -1, 3, c_int::MAX] {
            assert_eq!(ask(&t, n), (ffi::VENTO_NET_END_UNKNOWN, 0, String::new()));
        }
        // Nothing asked of a NULL error or reason.
        let code = unsafe { t.ended.unwrap()(t.user, 1, std::ptr::null_mut(), std::ptr::null_mut(), 0) };
        assert_eq!(code as u32, ffi::VENTO_NET_END_RESET);
        unsafe { t.release.unwrap()(t.user) };
    }

    #[test]
    fn a_direct_connection_reports_its_own_end() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let t = table(Arc::new(Direct));
        assert_ne!(t.direct, 0);
        let (fd, peer, _) = call(&t, "127.0.0.1", port as c_int, 5000);
        let _fd = owned(fd);
        drop(listener.accept().unwrap());
        assert_eq!(ask(&t, peer.connection), (ffi::VENTO_NET_END_UNKNOWN, 0, String::new()));
        unsafe { t.release.unwrap()(t.user) };
    }

    /// A route's own failure comes back in its own words.
    #[test]
    fn a_tunnels_failure_keeps_its_words() {
        struct Failing;
        impl Network for Failing {
            fn connect(
                &self,
                host: &str,
                port: u16,
                timeout: Timeout,
                liveness: &Liveness,
                hangup: &Hangup,
            ) -> io::Result<Connection> {
                let connection = Socketpair::new(Arc::new(Direct)).connect(host, port, timeout, liveness, hangup)?;
                connection.end.record(End::Tunnel("the tunnel's server went away".into()));
                Ok(connection)
            }
        }
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let t = table(Arc::new(Failing));
        let (fd, peer, _) = call(&t, "127.0.0.1", port as c_int, 5000);
        let _fd = owned(fd);
        assert_eq!(
            ask(&t, peer.connection),
            (
                ffi::VENTO_NET_END_TUNNEL,
                libc::ECONNABORTED,
                "the tunnel's server went away".into()
            )
        );
        unsafe { t.release.unwrap()(t.user) };
    }

    #[test]
    fn liveness_is_read_from_the_header() {
        assert_eq!(liveness(None), Liveness::default());
        let asked = ffi::vento_net_liveness {
            keepalive: 1,
            keepalive_idle_s: 5,
            keepalive_interval_s: 2,
            keepalive_count: 3,
            user_timeout_ms: 9000,
        };
        assert_eq!(
            liveness(Some(&asked)),
            Liveness {
                keepalive: Some(Keepalive {
                    idle: Duration::from_secs(5),
                    interval: Duration::from_secs(2),
                    count: 3
                }),
                user_timeout: Some(Duration::from_millis(9000)),
            }
        );
        let none = ffi::vento_net_liveness { keepalive: 0, user_timeout_ms: 0, ..asked };
        assert_eq!(liveness(Some(&none)), Liveness::default());
    }

    #[test]
    fn errors_map_to_codes_and_plain_reasons() {
        let refused = io::Error::from_raw_os_error(111);
        assert_eq!(code(&refused), ffi::VENTO_NET_EREFUSED);
        assert_eq!(why(&refused), "Connection refused");
        let timed_out: io::Error = io::ErrorKind::TimedOut.into();
        assert_eq!(code(&timed_out), ffi::VENTO_NET_ETIMEDOUT);
        assert_eq!(why(&timed_out), "Connection timed out");
        let slow =
            runtime::net::lookup_failed(io::Error::new(io::ErrorKind::TimedOut, "x did not resolve within 20 seconds"));
        assert_eq!(code(&slow), ffi::VENTO_NET_ERESOLVE);
        assert_eq!(why(&slow), "x did not resolve within 20 seconds");
        let closed = io::Error::new(io::ErrorKind::Interrupted, "the session was closed");
        assert_eq!(code(&closed), ffi::VENTO_NET_ECANCELLED);
        assert_eq!(code(&io::Error::from_raw_os_error(113)), ffi::VENTO_NET_EUNREACHABLE);
        assert_eq!(code(&io::Error::from_raw_os_error(101)), ffi::VENTO_NET_EUNREACHABLE);
        assert_eq!(code(&io::ErrorKind::NotFound.into()), ffi::VENTO_NET_ERESOLVE);
        assert_eq!(code(&io::ErrorKind::PermissionDenied.into()), ffi::VENTO_NET_EFAILED);
    }
}
