// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! Every connection handed over as one end of a socketpair, with vento
//! carrying the bytes between the other end and the real connection: what a
//! tunnel will do, without a tunnel. A test mode (the Linux client's
//! `--dev-force-socketpair`), for finding out what each backend needs of an
//! fd that is not a TCP socket, before any route depends on the answer.
//!
//! The relay is one thread per connection, polling both sockets, with a
//! buffer of [`BUFFER`] each way. It reads a side only once what it last read
//! from it has been passed on, so a side that stops reading stops the other
//! side's writer, as on a single TCP connection. What it passes on:
//!
//! - **Bytes**, both ways, in order.
//! - **A half-close.** The end of one side's stream (its `shutdown(SHUT_WR)`,
//!   or the server's FIN) becomes a `shutdown(SHUT_WR)` on the other side once
//!   everything read before it has been written there.
//! - **A hang-up.** The backend's end shut down both ways or closed (which
//!   [`Hangup::hang_up`](super::Hangup::hang_up) does) ends the relay: what it
//!   had sent is still passed on, for up to [`FLUSH`], and then the TCP socket
//!   is closed. What the server sends after that is dropped, as the backend's
//!   own socket would drop it.
//! - **An error or a reset from the server** closes the backend's end, which
//!   the backend reads as the end of the stream (or, when it had written
//!   something the relay had not yet taken, as `ECONNRESET`, which is what a
//!   socketpair says when its other end is closed with data unread). A
//!   socketpair cannot carry the error itself, so it is recorded in the
//!   connection's [`EndReason`] first, where the backend asks for it.
//!
//! What ended the connection is recorded before the backend's end can see
//! it: the server's end of stream as [`End::Closed`], its reset or any other
//! error as [`End::from_error`] maps it (keep-alive or `TCP_USER_TIMEOUT`
//! running out is `ETIMEDOUT`, [`End::TimedOut`]), and the backend's own
//! hang-up as [`End::Local`]. Where the inner network carries the bytes
//! itself (a relay over a tunnel, or over another relay), its connection's
//! end of stream says nothing of why, so what the inner network recorded is
//! what is recorded here.
//!
//! The relay's TCP socket is the inner network's, so it gets the
//! [`Liveness`] the backend asked for, and Nagle is off on it.
//!
//! **Datagrams** ([`Network::connect_datagram`]) go through a `SOCK_SEQPACKET`
//! socketpair, which keeps their boundaries, to the inner network's datagram
//! socket, on a relay thread of their own ([`start_datagrams`]). A datagram
//! that cannot be passed on at once (a full queue either way) is dropped, as
//! UDP drops it. An error from the server's socket (an ICMP port unreachable
//! is `ECONNREFUSED` there) ends the relay: it is recorded in the
//! connection's [`EndReason`] first, and the backend's end is closed, which
//! it reads as the end of file (a `recv` of 0 with `POLLHUP`, a `send` failing
//! `EPIPE`) and asks the reason for. The relay ends when the backend closes
//! its end or shuts it down both ways. A `shutdown(SHUT_WR)` alone ends only
//! what the backend sends, as on a UDP socket: the server's datagrams still
//! reach it until it closes its end. On a `SOCK_SEQPACKET` socket an empty
//! datagram and the end of file both read as 0, so the end is told from an
//! empty datagram by `POLLHUP` or `POLLRDHUP` on the relay's end, and an
//! empty datagram sent just before a shutdown is lost.

use super::{Connection, DatagramConnection, End, EndReason, Hangup, Liveness, Network, Peer, Route, Socket, Timeout};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::time::Duration;

/// How much the relay holds in each direction between a read and its write.
/// The socketpair's own buffers (the kernel's default for `AF_UNIX`, about
/// 200 KiB) come on top of it.
pub const BUFFER: usize = 64 * 1024;

/// How long the relay goes on passing a backend's last bytes to a server that
/// is not taking them, once the backend has closed its end.
pub const FLUSH: Duration = Duration::from_secs(10);

/// [`Network`] that dials through `inner` and hands over a socketpair's end in
/// place of the connection, relaying between the two.
///
/// The [`Peer`] it returns has the inner connection's addresses (the
/// server's, and the relay's own as the server sees it), as a tunnel that
/// knows them would report them; [`hiding_addresses`](Socketpair::hiding_addresses)
/// leaves them out, as a tunnel that resolves at its far end would.
pub struct Socketpair {
    inner: Arc<dyn Network>,
    hide_addresses: bool,
}

impl Socketpair {
    pub fn new(inner: Arc<dyn Network>) -> Socketpair {
        Socketpair {
            inner,
            hide_addresses: false,
        }
    }

    /// The same relay, reporting neither address in its [`Peer`]s: for
    /// testing a backend's handling of a route that knows neither (a tunnel
    /// that resolves the name at its far end).
    pub fn hiding_addresses(self) -> Socketpair {
        Socketpair {
            hide_addresses: true,
            ..self
        }
    }
}

/// What [`Socketpair::tunnel`](Network::tunnel) says.
pub const TUNNEL: &str = "dev-socketpair";

impl Network for Socketpair {
    fn connect(
        &self,
        host: &str,
        port: u16,
        timeout: Timeout,
        liveness: &Liveness,
        hangup: &Hangup,
    ) -> io::Result<Connection> {
        let Connection {
            socket: server,
            peer,
            end: inner,
        } = self.inner.connect(host, port, timeout, liveness, hangup)?;
        let (near, far) = UnixStream::pair()?;
        let asked = Peer { address: None, ..peer.clone() }.describe();
        let label = match peer.address {
            Some(address) if address.to_string() != asked => format!("{asked} ({address})"),
            _ => asked,
        };
        let end = EndReason::new();
        start(far, server, label, end.clone(), inner)?;
        let (address, local) = if self.hide_addresses {
            (None, None)
        } else {
            (peer.address, peer.local)
        };
        Ok(Connection {
            socket: Socket::Unix(near),
            peer: Peer {
                host: peer.host,
                port: peer.port,
                address,
                local,
                route: Route::Tunnel,
            },
            end,
        })
    }

    fn connect_datagram(
        &self,
        host: &str,
        port: u16,
        timeout: Duration,
        hangup: &Hangup,
    ) -> io::Result<DatagramConnection> {
        let DatagramConnection {
            socket: server,
            peer,
            end: inner,
        } = self.inner.connect_datagram(host, port, timeout, hangup)?;
        let (near, far) = seqpacket_pair()?;
        let asked = Peer { address: None, ..peer.clone() }.describe();
        let label = match peer.address {
            Some(address) if address.to_string() != asked => format!("{asked} ({address})"),
            _ => asked,
        };
        let end = EndReason::new();
        start_datagrams(far, server, label, end.clone(), inner)?;
        let (address, local) = if self.hide_addresses {
            (None, None)
        } else {
            (peer.address, peer.local)
        };
        Ok(DatagramConnection {
            socket: near,
            peer: Peer {
                host: peer.host,
                port: peer.port,
                address,
                local,
                route: Route::Tunnel,
            },
            end,
        })
    }

    /// The inner network's lookup: the relay resolves nothing itself.
    fn resolve(&self, host: &str, timeout: Duration, hangup: &Hangup) -> io::Result<()> {
        self.inner.resolve(host, timeout, hangup)
    }

    fn tunnel(&self) -> Option<String> {
        Some(TUNNEL.to_string())
    }
}

/// A connected `AF_UNIX` `SOCK_SEQPACKET` pair, close-on-exec.
fn seqpacket_pair() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0 as libc::c_int; 2];
    // SAFETY: two ints for the two fds, living across the call.
    if unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC, 0, fds.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: both fds are new and owned here alone.
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

/// Starts relaying datagrams between `far` (a `SOCK_SEQPACKET` end) and
/// `server` (the inner network's datagram socket) on a thread of its own,
/// which ends when the backend's end is closed or shut down, or when the
/// server's socket reports an error or ends (recorded in `end` first, from
/// `inner` where the inner network recorded why), closing both.
fn start_datagrams(far: OwnedFd, server: OwnedFd, label: String, end: EndReason, inner: EndReason) -> io::Result<()> {
    log::info!("relaying datagrams for {label} through a socketpair");
    std::thread::Builder::new()
        .name("vento-relay".into())
        .spawn(move || {
            let mut count = Count::default();
            let how = pump_datagrams(far.as_raw_fd(), server.as_raw_fd(), &mut count, &end, &inner);
            end.record(End::Local);
            log::info!(
                "datagram relay for {label} ended: {how} ({} datagrams sent, {} received, {} dropped)",
                count.sent,
                count.received,
                count.dropped
            );
            drop(far);
            drop(server);
        })?;
    Ok(())
}

#[derive(Default)]
struct Count {
    sent: u64,
    received: u64,
    dropped: u64,
}

/// The datagram relay's loop: returns how it ended, having recorded an
/// error from the server's socket, or the end of it, in `end`.
fn pump_datagrams(far: RawFd, server: RawFd, count: &mut Count, end: &EndReason, inner: &EndReason) -> String {
    let mut buf = vec![0u8; BUFFER];
    // The backend has shut its end for writing (`shutdown(SHUT_WR)`): nothing
    // more comes from it, and its end is no longer polled for reading, where
    // it would read as ready for good. The server's datagrams still go to it
    // until it closes its end.
    let mut far_done = false;
    loop {
        let mut fds = [
            libc::pollfd {
                fd: far,
                events: if far_done { 0 } else { libc::POLLIN | libc::POLLRDHUP },
                revents: 0,
            },
            libc::pollfd {
                fd: server,
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        // SAFETY: two pollfds, which live across the call.
        if unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) } < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return e.to_string();
        }
        let (f, s) = (fds[0].revents, fds[1].revents);
        if f & libc::POLLNVAL != 0 || s & libc::POLLNVAL != 0 {
            return io::Error::from_raw_os_error(libc::EBADF).to_string();
        }
        // The backend's end closed, or shut down for writing: what it sent
        // before is read first, and then its end of file.
        let far_gone = f & (libc::POLLHUP | libc::POLLERR) != 0;
        if far_gone && far_done {
            return "the backend closed its end".into();
        }
        let far_shut = far_gone || f & libc::POLLRDHUP != 0;
        if !far_done && (f & libc::POLLIN != 0 || far_shut) {
            match datagram(far, server, &mut buf, far_shut) {
                Moved::Yes => count.sent += 1,
                Moved::Dropped => count.dropped += 1,
                Moved::Nothing => {}
                Moved::End if far_gone => return "the backend closed its end".into(),
                Moved::End => far_done = true,
                // On sending to the server: what it said to an earlier one.
                Moved::Failed { to: true, e } => {
                    end.record(inner_end(inner).unwrap_or_else(|| End::from_error(&e)));
                    return e.to_string();
                }
                Moved::Failed { to: false, e } => return format!("the backend's end failed: {e}"),
            }
        }
        // The server's side ends only where it is itself a socketpair's end
        // (a relay over another relay): a UDP socket does not hang up.
        let server_gone = s & libc::POLLHUP != 0;
        if s & (libc::POLLIN | libc::POLLERR) != 0 || server_gone {
            match datagram(server, far, &mut buf, server_gone) {
                Moved::Yes => count.received += 1,
                Moved::Dropped => count.dropped += 1,
                Moved::Nothing => {}
                Moved::End => {
                    end.record(inner_end(inner).unwrap_or(End::Closed));
                    return "the server's side ended".into();
                }
                Moved::Failed { to: false, e } => {
                    end.record(inner_end(inner).unwrap_or_else(|| End::from_error(&e)));
                    return e.to_string();
                }
                // The backend's end is gone; its hang-up ends the loop.
                Moved::Failed { to: true, .. } => count.dropped += 1,
            }
        }
    }
}

enum Moved {
    Yes,
    Dropped,
    Nothing,
    /// `from` has ended: nothing was read, and it has hung up.
    End,
    /// Reading `from` (`to` false) or sending to `to` failed with `e`, an
    /// error rather than a full queue.
    Failed { to: bool, e: io::Error },
}

/// Passes one datagram from `from` to `to`, if one is waiting: dropped where
/// `to` cannot take it now. An error reading `from` or sending to `to` (the
/// server's socket reports an ICMP error either way) is returned. `hung_up`:
/// `from` has hung up or shut down its sending side, so that an empty read
/// is its end, not an empty datagram (both read as 0 on a `SOCK_SEQPACKET`
/// socket, and nothing tells them apart: an empty datagram sent just before
/// the end is taken as the end).
fn datagram(from: RawFd, to: RawFd, buf: &mut [u8], hung_up: bool) -> Moved {
    // SAFETY: the buffer outlives the call, and recv writes at most its
    // length. MSG_TRUNC: a datagram longer than the buffer is not passed on
    // cut short.
    let n = unsafe { libc::recv(from, buf.as_mut_ptr().cast(), buf.len(), libc::MSG_DONTWAIT | libc::MSG_TRUNC) };
    if n < 0 {
        let e = io::Error::last_os_error();
        return match e.kind() {
            io::ErrorKind::WouldBlock if hung_up => Moved::End,
            io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted => Moved::Nothing,
            _ => Moved::Failed { to: false, e },
        };
    }
    if n == 0 && hung_up {
        return Moved::End;
    }
    if n as usize > buf.len() {
        return Moved::Dropped;
    }
    // SAFETY: as above; MSG_NOSIGNAL as in `Pipe::drain`.
    let sent = unsafe { libc::send(to, buf.as_ptr().cast(), n as usize, libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL) };
    if sent >= 0 {
        return Moved::Yes;
    }
    let e = io::Error::last_os_error();
    match e.raw_os_error() {
        // A full queue, or a datagram too large for the path: lost, as
        // UDP loses it.
        Some(libc::EAGAIN | libc::ENOBUFS | libc::EINTR | libc::EMSGSIZE) => Moved::Dropped,
        _ => Moved::Failed { to: true, e },
    }
}

/// How the inner network says its connection ended, where it recorded
/// anything but its own end's close (which is this relay's doing): what a
/// relay over another relay (or over a tunnel) passes on, since the end of
/// the stream it reads from the inner one carries no reason.
fn inner_end(inner: &EndReason) -> Option<End> {
    inner.get().filter(|end| *end != End::Local)
}

/// Starts relaying between `far` and `server` on a thread of its own, which
/// ends by itself (see the module's documentation), closing both, after
/// recording in `end` how the connection ended, from `inner` (the inner
/// network's record) where it says.
fn start(far: UnixStream, server: Socket, label: String, end: EndReason, inner: EndReason) -> io::Result<()> {
    // The backend's own TCP_NODELAY lands on its socketpair end, which has
    // no such option; every backend here asks for it, so the relay's socket
    // has it.
    let _ = server.set_nodelay(true);
    log::info!("relaying {label} through a socketpair");
    let far = OwnedFd::from(far);
    let server = OwnedFd::from(server);
    std::thread::Builder::new()
        .name("vento-relay".into())
        .spawn(move || {
            let mut up = Pipe::new();
            let mut down = Pipe::new();
            let ended = pump(far.as_raw_fd(), server.as_raw_fd(), &mut up, &mut down, &end, &inner, FLUSH);
            let (sent, received) = (up.moved, down.moved);
            match ended {
                Ok(how) => log::info!("relay for {label} ended: {how} ({sent} bytes sent, {received} received)"),
                Err(e) => {
                    log::info!("relay for {label} ended: {e} ({sent} bytes sent, {received} received)");
                    end.record(End::from_error(&e));
                }
            }
            // Whatever ended it, it is over for the backend from here on.
            end.record(End::Local);
            // Both closed here, the backend's end first, so that it learns
            // of the end before the server does.
            drop(far);
            drop(server);
        })?;
    Ok(())
}

/// One direction: what was read and not yet written, and whether its source
/// has ended.
struct Pipe {
    buf: Box<[u8]>,
    at: usize,
    len: usize,
    /// The source has said end of stream.
    eof: bool,
    /// ...and that has been passed on, with everything before it.
    done: bool,
    moved: u64,
}

impl Pipe {
    fn new() -> Pipe {
        Pipe {
            buf: vec![0u8; BUFFER].into_boxed_slice(),
            at: 0,
            len: 0,
            eof: false,
            done: false,
            moved: 0,
        }
    }

    fn pending(&self) -> bool {
        self.at < self.len
    }

    fn wants_read(&self) -> bool {
        !self.eof && !self.pending()
    }

    /// Reads once from `fd`, if anything is there.
    fn fill(&mut self, fd: RawFd) -> io::Result<()> {
        // SAFETY: the buffer outlives the call, and recv writes at most its
        // length.
        let n = unsafe { libc::recv(fd, self.buf.as_mut_ptr().cast(), self.buf.len(), libc::MSG_DONTWAIT) };
        match n {
            0 => self.eof = true,
            n if n > 0 => {
                self.at = 0;
                self.len = n as usize;
            }
            _ => {
                let e = io::Error::last_os_error();
                if !matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) {
                    return Err(e);
                }
            }
        }
        Ok(())
    }

    /// Writes what it can of what is pending to `fd`.
    fn drain(&mut self, fd: RawFd) -> io::Result<()> {
        let pending = &self.buf[self.at..self.len];
        // SAFETY: as in `fill`. MSG_NOSIGNAL: a closed reader is an EPIPE
        // here, not a SIGPIPE for the whole process (which on Android has no
        // handler that ignores it).
        let n = unsafe {
            libc::send(
                fd,
                pending.as_ptr().cast(),
                pending.len(),
                libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL,
            )
        };
        if n < 0 {
            let e = io::Error::last_os_error();
            if !matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) {
                return Err(e);
            }
            return Ok(());
        }
        self.at += n as usize;
        self.moved += n as u64;
        Ok(())
    }
}

/// The relay's loop: `up` is the backend's bytes on their way to the server,
/// `down` the server's on their way back. Returns how it ended, or the error
/// that ended it. What ended the server's side is recorded in `end` as soon
/// as it is known, before it is passed on: what `inner` recorded where it
/// did, else what the server's socket said. `flush` is [`FLUSH`].
fn pump(
    far: RawFd,
    server: RawFd,
    up: &mut Pipe,
    down: &mut Pipe,
    end: &EndReason,
    inner: &EndReason,
    flush: Duration,
) -> io::Result<&'static str> {
    // An error on the server's side, recorded before it ends the loop (and
    // with it the backend's end).
    let failed = |e: io::Error| {
        end.record(inner_end(inner).unwrap_or_else(|| End::from_error(&e)));
        e
    };
    // The backend's end reads nothing more (it has hung up, closed, or shut
    // down its reading side), so anything still to come down is dropped. It
    // may still be sending, where only its reading side is shut.
    let mut far_gone = false;
    // ...before the server's stream had ended.
    let mut cut = false;
    loop {
        if up.done && down.done {
            return Ok(if cut { "the backend closed its end" } else { "both sides ended" });
        }
        let events = |read: bool, write: bool| {
            (if read { libc::POLLIN } else { 0 }) | (if write { libc::POLLOUT } else { 0 })
        };
        let far_events = events(up.wants_read(), down.pending());
        let server_events = events(down.wants_read(), up.pending());
        // A side with nothing to wait for is left out, rather than polled for
        // a hang-up that is already known or does not matter yet, which would
        // wake the loop at once on every turn. At least one side always has
        // something: a direction is done, reading, or holding bytes to write.
        // The backend's end is the exception until it is known to be gone:
        // polled for nothing, it still reports its hang-up, which starts the
        // flush time while the relay holds bytes for a server that is not
        // taking them.
        let mut fds = [
            libc::pollfd {
                fd: if far_events == 0 && far_gone { -1 } else { far },
                events: far_events,
                revents: 0,
            },
            libc::pollfd {
                fd: if server_events == 0 { -1 } else { server },
                events: server_events,
                revents: 0,
            },
        ];
        // Only while the backend's last bytes wait for a server that is not
        // taking them: a backend that has only shut its reading side may
        // pause between writes for as long as it likes.
        let timeout = if far_gone && up.pending() {
            flush.as_millis() as libc::c_int
        } else {
            -1
        };
        // SAFETY: two pollfds, which live across the call.
        let n = unsafe { libc::poll(fds.as_mut_ptr(), 2, timeout) };
        if n < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        if n == 0 {
            return Ok("the server did not take the backend's last bytes in time");
        }
        let (f, s) = (fds[0].revents, fds[1].revents);
        if s & libc::POLLERR != 0 {
            return Err(failed(pending_error(server)));
        }
        if f & libc::POLLERR != 0 {
            end.record(End::Local);
            return Err(pending_error(far));
        }
        if f & libc::POLLNVAL != 0 || s & libc::POLLNVAL != 0 {
            return Err(io::Error::from_raw_os_error(libc::EBADF));
        }
        let readable = libc::POLLIN | libc::POLLHUP;
        let writable = libc::POLLOUT | libc::POLLHUP;
        if up.wants_read() && f & readable != 0 {
            up.fill(far).inspect_err(|_| end.record(End::Local))?;
        }
        if down.wants_read() && s & readable != 0 {
            down.fill(server).map_err(failed)?;
            if down.eof {
                end.record(inner_end(inner).unwrap_or(End::Closed));
            }
        }
        if down.pending() && f & writable != 0 && down.drain(far).is_err() {
            // The backend's end is closed or shut for reading.
            far_gone = true;
        }
        if up.pending() && s & writable != 0 {
            up.drain(server).map_err(failed)?;
        }
        if f & libc::POLLHUP != 0 {
            far_gone = true;
        }
        if far_gone {
            end.record(End::Local);
        }
        if far_gone && !down.done {
            cut = true;
            down.at = down.len;
            down.eof = true;
            down.done = true;
        }
        if up.eof && !up.pending() && !up.done {
            // SAFETY: shutdown on an fd this relay owns.
            unsafe { libc::shutdown(server, libc::SHUT_WR) };
            up.done = true;
        }
        if down.eof && !down.pending() && !down.done {
            // SAFETY: as above.
            unsafe { libc::shutdown(far, libc::SHUT_WR) };
            down.done = true;
        }
    }
}

/// The socket's pending error, which a POLLERR says it has: `EIO` where
/// `SO_ERROR` has none to give (it was read already, or the error is on the
/// socket's error queue).
fn pending_error(fd: RawFd) -> io::Error {
    let mut error: libc::c_int = 0;
    let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
    // SAFETY: an int and its length, both living across the call.
    let got = unsafe { libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_ERROR, (&raw mut error).cast(), &mut len) };
    io::Error::from_raw_os_error(if got == 0 && error != 0 { error } else { libc::EIO })
}

#[cfg(test)]
mod tests {
    use super::super::Direct;
    use super::*;
    use std::io::{Read, Write};
    use std::net::{Shutdown, TcpListener, TcpStream};
    use std::time::Instant;

    /// A connection through the relay to a listener here, and the listener's
    /// side of it.
    fn relayed() -> (Connection, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let network = Socketpair::new(Arc::new(Direct));
        let connection = network
            .connect(
                "127.0.0.1",
                port,
                Timeout::PerAddress(Duration::from_secs(5)),
                &Liveness::default(),
                &Hangup::new(),
            )
            .unwrap();
        let (server, _) = listener.accept().unwrap();
        server.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        connection.socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        (connection, server)
    }

    fn send(fd: &OwnedFd, data: &[u8]) {
        // SAFETY: the slice outlives the call.
        let n = unsafe { libc::send(fd.as_raw_fd(), data.as_ptr().cast(), data.len(), libc::MSG_NOSIGNAL) };
        assert_eq!(n, data.len() as isize, "{}", io::Error::last_os_error());
    }

    /// One datagram from `fd`, waiting up to five seconds.
    fn recv(fd: &OwnedFd) -> Vec<u8> {
        let mut pfd = libc::pollfd {
            fd: fd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one pollfd, living across the call.
        assert_eq!(unsafe { libc::poll(&mut pfd, 1, 5000) }, 1, "no datagram within 5 s");
        let mut buf = vec![0u8; 4096];
        // SAFETY: as in `send`.
        let n = unsafe { libc::recv(fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len(), 0) };
        assert!(n >= 0, "{}", io::Error::last_os_error());
        buf.truncate(n as usize);
        buf
    }

    #[test]
    fn datagrams_go_both_ways_whole() {
        let server = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        server.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let port = server.local_addr().unwrap().port();
        let network = Socketpair::new(Arc::new(Direct));
        let d = network.connect_datagram("127.0.0.1", port, Duration::from_secs(5), &Hangup::new()).unwrap();
        assert_eq!(d.peer.route, Route::Tunnel);
        assert_eq!(d.peer.address, Some(server.local_addr().unwrap()));
        send(&d.socket, b"one");
        send(&d.socket, b"two, longer");
        send(&d.socket, &[9u8; 1400]);
        let mut buf = [0u8; 2048];
        let (n, client) = server.recv_from(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"one");
        assert_eq!(server.recv_from(&mut buf).unwrap().0, 11);
        assert_eq!(server.recv_from(&mut buf).unwrap().0, 1400);
        assert_eq!(d.peer.local.map(|a| a.port()), Some(client.port()));
        server.send_to(b"back", client).unwrap();
        server.send_to(&[7u8; 1300], client).unwrap();
        assert_eq!(recv(&d.socket), b"back");
        assert_eq!(recv(&d.socket), vec![7u8; 1300]);
    }

    #[test]
    fn closing_the_backends_datagram_end_ends_the_relay() {
        let server = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        server.set_read_timeout(Some(Duration::from_millis(200))).unwrap();
        let port = server.local_addr().unwrap().port();
        let d = Socketpair::new(Arc::new(Direct))
            .connect_datagram("127.0.0.1", port, Duration::from_secs(5), &Hangup::new())
            .unwrap();
        send(&d.socket, b"hello");
        let mut buf = [0u8; 64];
        let (_, client) = server.recv_from(&mut buf).unwrap();
        server.connect(client).unwrap();
        drop(d);
        // Once the relay has closed its socket, the kernel answers what the
        // server sends there with a port unreachable.
        let started = Instant::now();
        loop {
            assert!(started.elapsed() < Duration::from_secs(5), "the relay's socket is still open");
            let _ = server.send(b"anyone?");
            match server.recv(&mut buf) {
                Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => break,
                _ => {}
            }
        }
    }

    #[test]
    fn a_servers_refusal_is_recorded_and_ends_the_backends_end() {
        // A port nothing listens on: the kernel answers with a port
        // unreachable, which the relay's socket reports as ECONNREFUSED.
        let port = {
            let unused = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
            unused.local_addr().unwrap().port()
        };
        let d = Socketpair::new(Arc::new(Direct))
            .connect_datagram("127.0.0.1", port, Duration::from_secs(5), &Hangup::new())
            .unwrap();
        send(&d.socket, b"anyone?");
        let mut pfd = libc::pollfd {
            fd: d.socket.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one pollfd, living across the call.
        assert_eq!(unsafe { libc::poll(&mut pfd, 1, 5000) }, 1);
        assert_ne!(pfd.revents & libc::POLLHUP, 0);
        assert_eq!(d.end.get(), Some(End::Os(libc::ECONNREFUSED)));
    }

    /// A UDP socket here, and a relayed datagram connection to it.
    fn relayed_datagrams() -> (DatagramConnection, std::net::UdpSocket) {
        let server = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        server.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let port = server.local_addr().unwrap().port();
        let d = Socketpair::new(Arc::new(Direct))
            .connect_datagram("127.0.0.1", port, Duration::from_secs(5), &Hangup::new())
            .unwrap();
        (d, server)
    }

    /// The backend's `shutdown(SHUT_WR)` ends what it sends, and nothing
    /// else: no empty datagrams reach the server, and the server's still
    /// reach the backend.
    #[test]
    fn a_backends_shutdown_for_writing_ends_only_what_it_sends() {
        let (d, server) = relayed_datagrams();
        send(&d.socket, b"last");
        let mut buf = [0u8; 64];
        let (n, client) = server.recv_from(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"last");
        // SAFETY: shutdown on an fd the test owns.
        assert_eq!(unsafe { libc::shutdown(d.socket.as_raw_fd(), libc::SHUT_WR) }, 0);
        server.set_read_timeout(Some(Duration::from_millis(300))).unwrap();
        match server.recv_from(&mut buf) {
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {}
            other => panic!("the server got {other:?} after the backend's shutdown"),
        }
        server.send_to(b"still", client).unwrap();
        assert_eq!(recv(&d.socket), b"still");
        assert_eq!(d.end.get(), None);
    }

    /// A datagram longer than the relay's buffer is dropped, not cut short,
    /// and the next one goes through.
    #[test]
    fn a_datagram_too_long_for_the_relay_is_dropped() {
        let (d, server) = relayed_datagrams();
        let long = vec![1u8; BUFFER + 4464];
        // SAFETY: the vector outlives the call.
        let n = unsafe { libc::send(d.socket.as_raw_fd(), long.as_ptr().cast(), long.len(), libc::MSG_NOSIGNAL) };
        assert_eq!(n, long.len() as isize, "{}", io::Error::last_os_error());
        send(&d.socket, b"after");
        let mut buf = vec![0u8; 2 * BUFFER];
        let (n, _) = server.recv_from(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"after");
    }

    /// A backend that does not read loses the server's datagrams once its
    /// queue is full, and the relay goes on passing on what it sends.
    #[test]
    fn a_full_backend_queue_drops_rather_than_blocks() {
        let (d, server) = relayed_datagrams();
        send(&d.socket, b"hello");
        let mut buf = [0u8; 64];
        let (_, client) = server.recv_from(&mut buf).unwrap();
        let chunk = [3u8; 1024];
        for _ in 0..4096 {
            server.send_to(&chunk, client).unwrap();
        }
        send(&d.socket, b"still here");
        let (n, _) = server.recv_from(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"still here");
        // What the backend's queue took arrives whole.
        assert_eq!(recv(&d.socket), chunk.to_vec());
    }

    /// The server's port unreachable arriving after the backend has closed
    /// its end: the relay ends either way.
    #[test]
    fn a_refusal_after_the_backend_closed_ends_the_relay() {
        let port = {
            let unused = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
            unused.local_addr().unwrap().port()
        };
        let d = Socketpair::new(Arc::new(Direct))
            .connect_datagram("127.0.0.1", port, Duration::from_secs(5), &Hangup::new())
            .unwrap();
        let end = d.end.clone();
        send(&d.socket, b"anyone?");
        drop(d.socket);
        let started = Instant::now();
        while end.get().is_none() {
            assert!(started.elapsed() < Duration::from_secs(5), "the relay is still running");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(matches!(end.get(), Some(End::Local | End::Os(libc::ECONNREFUSED))), "{:?}", end.get());
    }

    #[test]
    fn a_route_without_datagrams_says_so() {
        struct Streams;
        impl Network for Streams {
            fn connect(&self, _: &str, _: u16, _: Timeout, _: &Liveness, _: &Hangup) -> io::Result<Connection> {
                Err(io::ErrorKind::ConnectionRefused.into())
            }
        }
        let e = Socketpair::new(Arc::new(Streams))
            .connect_datagram("127.0.0.1", 9, Duration::from_secs(1), &Hangup::new())
            .unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::Unsupported);
    }

    #[test]
    fn a_relayed_connection_has_the_relays_addresses() {
        let (connection, server) = relayed();
        assert!(matches!(connection.socket, Socket::Unix(_)));
        assert_eq!(connection.peer.host, "127.0.0.1");
        assert_eq!(connection.peer.route, Route::Tunnel);
        assert_eq!(connection.peer.address, Some(server.local_addr().unwrap()));
        assert_eq!(connection.peer.local, Some(server.peer_addr().unwrap()));
        // Asked of every socket by the backends, and harmless here.
        connection.socket.set_nodelay(true).unwrap();
        assert_eq!(Socketpair::new(Arc::new(Direct)).tunnel().as_deref(), Some(TUNNEL));
    }

    #[test]
    fn a_tunnel_hiding_addresses_says_only_where_it_was_asked_to_go() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let network = Socketpair::new(Arc::new(Direct)).hiding_addresses();
        let connection = network
            .connect(
                "127.0.0.1",
                port,
                Timeout::PerAddress(Duration::from_secs(5)),
                &Liveness::default(),
                &Hangup::new(),
            )
            .unwrap();
        assert_eq!((connection.peer.address, connection.peer.local), (None, None));
        assert_eq!(connection.peer.describe(), format!("127.0.0.1:{port}"));
    }

    /// Waits for the relay to record how the connection ended.
    fn ended(connection: &Connection) -> End {
        let started = Instant::now();
        loop {
            if let Some(end) = connection.end.get() {
                return end;
            }
            assert!(started.elapsed() < Duration::from_secs(5), "nothing recorded");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// The server's end of stream is recorded as closed, by the time the
    /// backend reads it.
    #[test]
    fn the_servers_close_is_recorded() {
        let (connection, server) = relayed();
        drop(server);
        let mut near = &connection.socket;
        let mut byte = [0u8];
        assert_eq!(near.read(&mut byte).unwrap(), 0);
        assert_eq!(connection.end.get(), Some(End::Closed));
        let eof = io::Error::from(io::ErrorKind::UnexpectedEof);
        assert_eq!(connection.end.explain(eof).kind(), io::ErrorKind::UnexpectedEof);
    }

    /// The server's reset is recorded by the time the backend reads the end
    /// of its stream, and explains it as the reset a direct socket reports.
    #[test]
    fn the_servers_reset_is_recorded() {
        let (connection, server) = relayed();
        socket2::SockRef::from(&server).set_linger(Some(Duration::ZERO)).unwrap();
        drop(server);
        let mut near = &connection.socket;
        let mut byte = [0u8];
        let seen = match near.read(&mut byte) {
            Ok(0) => io::Error::from(io::ErrorKind::UnexpectedEof),
            Err(e) if e.kind() == io::ErrorKind::ConnectionReset => e,
            other => panic!("{other:?}"),
        };
        assert_eq!(connection.end.get(), Some(End::Reset));
        let explained = connection.end.explain(seen);
        assert_eq!(explained.raw_os_error(), Some(libc::ECONNRESET));
    }

    /// A relay over another relay passes on what the inner one recorded:
    /// the server's reset, which reads as the end of the stream between the
    /// two relays.
    #[test]
    fn a_relay_over_another_keeps_the_inner_ones_end() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let network = Socketpair::new(Arc::new(Socketpair::new(Arc::new(Direct))));
        let connection = network
            .connect(
                "127.0.0.1",
                port,
                Timeout::PerAddress(Duration::from_secs(5)),
                &Liveness::default(),
                &Hangup::new(),
            )
            .unwrap();
        let (server, _) = listener.accept().unwrap();
        connection.socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        socket2::SockRef::from(&server).set_linger(Some(Duration::ZERO)).unwrap();
        drop(server);
        let mut byte = [0u8];
        match (&connection.socket).read(&mut byte) {
            Ok(0) => {}
            Err(e) if e.kind() == io::ErrorKind::ConnectionReset => {}
            other => panic!("{other:?}"),
        }
        assert_eq!(connection.end.get(), Some(End::Reset));
    }

    /// The same for datagrams: the inner relay's refusal, not the end of the
    /// socketpair between the two.
    #[test]
    fn a_datagram_relay_over_another_keeps_the_inner_ones_end() {
        let port = {
            let unused = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
            unused.local_addr().unwrap().port()
        };
        let d = Socketpair::new(Arc::new(Socketpair::new(Arc::new(Direct))))
            .connect_datagram("127.0.0.1", port, Duration::from_secs(5), &Hangup::new())
            .unwrap();
        send(&d.socket, b"anyone?");
        let mut pfd = libc::pollfd {
            fd: d.socket.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one pollfd, living across the call.
        assert_eq!(unsafe { libc::poll(&mut pfd, 1, 5000) }, 1);
        assert_ne!(pfd.revents & libc::POLLHUP, 0);
        assert_eq!(d.end.get(), Some(End::Os(libc::ECONNREFUSED)));
    }

    /// The backend's own hang-up is recorded as this end's.
    #[test]
    fn the_backends_hang_up_is_recorded() {
        let (connection, mut server) = relayed();
        connection.socket.shutdown(Shutdown::Both).unwrap();
        let mut got = Vec::new();
        server.read_to_end(&mut got).unwrap();
        assert_eq!(ended(&connection), End::Local);
    }

    /// The relay's TCP socket is the inner network's, asked for the liveness
    /// the backend asked for, which `Direct` sets on it.
    #[test]
    fn liveness_reaches_the_relays_socket() {
        struct Keeping(std::sync::Mutex<Vec<(Liveness, Socket)>>);
        impl Network for Keeping {
            fn connect(
                &self,
                host: &str,
                port: u16,
                timeout: Timeout,
                liveness: &Liveness,
                hangup: &Hangup,
            ) -> io::Result<Connection> {
                let connection = Direct.connect(host, port, timeout, liveness, hangup)?;
                let kept = connection.socket.try_clone()?;
                self.0.lock().unwrap().push((*liveness, kept));
                Ok(connection)
            }
        }
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let inner = Arc::new(Keeping(Default::default()));
        let liveness = Liveness {
            keepalive: Some(super::super::Keepalive {
                idle: Duration::from_secs(5),
                interval: Duration::from_secs(2),
                count: 3,
            }),
            user_timeout: Some(Duration::from_millis(9000)),
        };
        let network = Socketpair::new(Arc::clone(&inner) as Arc<dyn Network>);
        let _connection = network
            .connect("127.0.0.1", port, Timeout::PerAddress(Duration::from_secs(5)), &liveness, &Hangup::new())
            .unwrap();
        let kept = inner.0.lock().unwrap();
        let (asked, socket) = &kept[0];
        assert_eq!(*asked, liveness);
        let get = |level, name| {
            let mut value: libc::c_int = 0;
            let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
            let got =
                unsafe { libc::getsockopt(socket.as_raw_fd(), level, name, (&raw mut value).cast(), &mut len) };
            assert_eq!(got, 0);
            value
        };
        assert_eq!(get(libc::SOL_SOCKET, libc::SO_KEEPALIVE), 1);
        assert_eq!(get(libc::IPPROTO_TCP, libc::TCP_KEEPIDLE), 5);
        assert_eq!(get(libc::IPPROTO_TCP, libc::TCP_KEEPINTVL), 2);
        assert_eq!(get(libc::IPPROTO_TCP, libc::TCP_KEEPCNT), 3);
        assert_eq!(get(libc::IPPROTO_TCP, libc::TCP_USER_TIMEOUT), 9000);
        assert_eq!(get(libc::IPPROTO_TCP, libc::TCP_NODELAY), 1);
    }

    #[test]
    fn bytes_go_both_ways() {
        let (connection, mut server) = relayed();
        let mut near = &connection.socket;
        near.write_all(b"hello").unwrap();
        let mut got = [0u8; 5];
        server.read_exact(&mut got).unwrap();
        assert_eq!(&got, b"hello");
        server.write_all(b"world").unwrap();
        near.read_exact(&mut got).unwrap();
        assert_eq!(&got, b"world");
    }

    /// More than every buffer on the way holds, both ways at once, read
    /// slowly at one end: the bytes arrive whole and in order, and the
    /// writer is held back rather than anything dropped.
    #[test]
    fn a_large_transfer_arrives_whole() {
        const SIZE: usize = 8 << 20;
        let (connection, server) = relayed();
        let pattern = |i: usize| (i * 7 + i / 251) as u8;
        let data: Vec<u8> = (0..SIZE).map(pattern).collect();
        std::thread::scope(|scope| {
            let near = &connection.socket;
            let server = &server;
            scope.spawn(|| (&*near).write_all(&data).unwrap());
            scope.spawn(|| (&*server).write_all(&data).unwrap());
            let at_server = scope.spawn(|| {
                let mut got = vec![0u8; SIZE];
                // Slowly at first, so that the relay has to hold back.
                std::thread::sleep(Duration::from_millis(300));
                (&*server).read_exact(&mut got).unwrap();
                got
            });
            let mut got = vec![0u8; SIZE];
            (&*near).read_exact(&mut got).unwrap();
            assert!(got == data, "the server's bytes changed on the way");
            assert!(at_server.join().unwrap() == data, "the backend's bytes changed on the way");
        });
    }

    /// The backend's half-close reaches the server as the end of the stream,
    /// and the server can still answer.
    #[test]
    fn the_backends_half_close_is_passed_on() {
        let (connection, mut server) = relayed();
        let mut near = &connection.socket;
        near.write_all(b"last").unwrap();
        near.shutdown(Shutdown::Write).unwrap();
        let mut got = Vec::new();
        server.read_to_end(&mut got).unwrap();
        assert_eq!(got, b"last");
        server.write_all(b"answer").unwrap();
        server.shutdown(Shutdown::Write).unwrap();
        let mut got = Vec::new();
        near.read_to_end(&mut got).unwrap();
        assert_eq!(got, b"answer");
    }

    /// The server's half-close reaches the backend as the end of the stream,
    /// and the backend can still send.
    #[test]
    fn the_servers_half_close_is_passed_on() {
        let (connection, mut server) = relayed();
        let mut near = &connection.socket;
        server.write_all(b"bye").unwrap();
        server.shutdown(Shutdown::Write).unwrap();
        let mut got = Vec::new();
        near.read_to_end(&mut got).unwrap();
        assert_eq!(got, b"bye");
        near.write_all(b"still here").unwrap();
        let mut got = [0u8; 10];
        server.read_exact(&mut got).unwrap();
        assert_eq!(&got, b"still here");
    }

    /// A reset from the server ends the backend's stream, and its writes
    /// fail soon after: a socketpair has no reset to pass on.
    #[test]
    fn a_reset_ends_the_backends_stream() {
        let (connection, server) = relayed();
        socket2::SockRef::from(&server).set_linger(Some(Duration::ZERO)).unwrap();
        drop(server);
        let mut near = &connection.socket;
        let mut byte = [0u8];
        match near.read(&mut byte) {
            Ok(0) => {}
            Err(e) if e.kind() == io::ErrorKind::ConnectionReset => {}
            other => panic!("{other:?}"),
        }
        let started = Instant::now();
        let failed = loop {
            if let Err(e) = near.write_all(b"x") {
                break e;
            }
            assert!(started.elapsed() < Duration::from_secs(5), "writes still accepted");
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(
            matches!(failed.kind(), io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset),
            "{failed}"
        );
    }

    /// A hang-up in the middle of a transfer wakes the backend's read at
    /// once, and the server sees its connection end.
    #[test]
    fn a_hang_up_mid_transfer_ends_both_sides() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let hangup = Hangup::new();
        let network = Socketpair::new(Arc::new(Direct));
        let connection = network
            .connect("127.0.0.1", port, Timeout::PerAddress(Duration::from_secs(5)), &Liveness::default(), &hangup)
            .unwrap();
        let (server, _) = listener.accept().unwrap();
        let near = &connection.socket;
        let holding = hangup.hold(near).unwrap();
        std::thread::scope(|scope| {
            // The server sends without end until its connection is gone.
            let sending = scope.spawn(|| {
                let chunk = [0x55u8; 16 * 1024];
                let started = Instant::now();
                loop {
                    if let Err(e) = (&server).write_all(&chunk) {
                        return (e, started);
                    }
                }
            });
            let reading = scope.spawn(|| {
                let mut buf = [0u8; 4096];
                let mut total = 0usize;
                loop {
                    match (&*near).read(&mut buf) {
                        Ok(0) => return Ok(total),
                        Ok(n) => total += n,
                        Err(e) => return Err(e),
                    }
                }
            });
            std::thread::sleep(Duration::from_millis(200));
            let hung_up = Instant::now();
            hangup.hang_up();
            let read = reading.join().unwrap().unwrap();
            assert!(read > 0);
            assert!(hung_up.elapsed() < Duration::from_secs(1));
            let (e, _) = sending.join().unwrap();
            assert!(hung_up.elapsed() < Duration::from_secs(3), "{:?}", hung_up.elapsed());
            assert!(
                matches!(e.kind(), io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset),
                "{e}"
            );
        });
        drop(holding);
    }

    /// The backend closing its end is the end of the connection for the
    /// server.
    #[test]
    fn closing_the_backends_end_closes_the_connection() {
        let (connection, mut server) = relayed();
        let mut near = &connection.socket;
        near.write_all(b"done").unwrap();
        drop(connection);
        let mut got = Vec::new();
        server.read_to_end(&mut got).unwrap();
        assert_eq!(got, b"done");
    }

    /// A relay's two sides run by `pump` on a thread here, with a `flush` of
    /// its own: the backend's end, and the server's side of a TCP
    /// connection whose other side is returned (`accepted`).
    fn pumping(flush: Duration) -> (UnixStream, TcpStream, std::thread::JoinHandle<(io::Result<&'static str>, EndReason)>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (accepted, _) = listener.accept().unwrap();
        let (near, far) = UnixStream::pair().unwrap();
        let pumping = std::thread::spawn(move || {
            let (mut up, mut down) = (Pipe::new(), Pipe::new());
            let end = EndReason::new();
            let how = pump(
                far.as_raw_fd(),
                client.as_raw_fd(),
                &mut up,
                &mut down,
                &end,
                &EndReason::none(),
                flush,
            );
            (how, end)
        });
        (near, accepted, pumping)
    }

    /// The backend's last bytes, for a server that does not read them, are
    /// given up after the flush time once the backend has closed its end.
    #[test]
    fn a_server_not_taking_the_last_bytes_is_given_up_on() {
        let (near, _accepted, pumping) = pumping(Duration::from_millis(300));
        near.set_nonblocking(true).unwrap();
        // Until every buffer on the way is full: the backend's end stays
        // unwritable for a while.
        let chunk = [0u8; 64 * 1024];
        let mut last = Instant::now();
        while last.elapsed() < Duration::from_millis(300) {
            match (&near).write(&chunk) {
                Ok(_) => last = Instant::now(),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(10)),
                Err(e) => panic!("{e}"),
            }
        }
        let closed = Instant::now();
        drop(near);
        let (how, end) = pumping.join().unwrap();
        assert_eq!(how.unwrap(), "the server did not take the backend's last bytes in time");
        assert!(closed.elapsed() < Duration::from_secs(3), "{:?}", closed.elapsed());
        assert_eq!(end.get(), Some(End::Local));
    }

    /// A backend that has shut only its reading side may pause between its
    /// writes for longer than the flush time.
    #[test]
    fn a_backend_shut_for_reading_still_sends_after_a_pause() {
        let (near, mut accepted, pumping) = pumping(Duration::from_millis(200));
        near.shutdown(Shutdown::Read).unwrap();
        // What the server sends now cannot reach the backend.
        accepted.write_all(b"unread").unwrap();
        (&near).write_all(b"a").unwrap();
        std::thread::sleep(Duration::from_millis(600));
        (&near).write_all(b"b").unwrap();
        near.shutdown(Shutdown::Write).unwrap();
        let mut got = Vec::new();
        accepted.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        accepted.read_to_end(&mut got).unwrap();
        assert_eq!(got, b"ab");
        drop(near);
        assert_eq!(pumping.join().unwrap().0.unwrap(), "the backend closed its end");
    }

    /// The backend closing its end with the server's bytes unread there:
    /// the relay's end reports ECONNRESET (`POLLERR`), and the connection to
    /// the server ends, as this end's doing.
    #[test]
    fn closing_with_bytes_unread_ends_the_connection() {
        let (connection, mut server) = relayed();
        server.write_all(b"never read").unwrap();
        std::thread::sleep(Duration::from_millis(100));
        drop(connection.socket);
        let mut got = Vec::new();
        let started = Instant::now();
        let _ = server.read_to_end(&mut got);
        assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
        assert_eq!(ended(&Connection { socket: Socket::Unix(UnixStream::pair().unwrap().0), ..connection }), End::Local);
    }

    /// A hang-up between the dial's return and the backend's `hold`: the
    /// hold shuts the backend's end down, which ends the relay.
    #[test]
    fn a_hang_up_before_the_hold_ends_the_relay() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let hangup = Hangup::new();
        let connection = Socketpair::new(Arc::new(Direct))
            .connect("127.0.0.1", port, Timeout::PerAddress(Duration::from_secs(5)), &Liveness::default(), &hangup)
            .unwrap();
        let (mut server, _) = listener.accept().unwrap();
        hangup.hang_up();
        assert_eq!(hangup.hold(&connection.socket).err().unwrap().kind(), io::ErrorKind::Interrupted);
        server.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut got = Vec::new();
        server.read_to_end(&mut got).unwrap();
        assert_eq!(ended(&connection), End::Local);
    }

    /// The relay's dial is the inner network's, and so is how it fails.
    #[test]
    fn a_refused_dial_through_the_relay_says_so() {
        let port = {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        let network = Socketpair::new(Arc::new(Direct));
        let e = network
            .connect(
                "127.0.0.1",
                port,
                Timeout::PerAddress(Duration::from_secs(5)),
                &Liveness::default(),
                &Hangup::new(),
            )
            .err()
            .unwrap();
        assert_eq!(e.kind(), io::ErrorKind::ConnectionRefused);
    }

    /// So is its lookup: the inner network's, with no relay started, and
    /// `Unsupported` where the inner network resolves only as it connects.
    #[test]
    fn a_lookup_through_the_relay_is_the_inner_networks() {
        let hangup = Hangup::new();
        let network = Socketpair::new(Arc::new(Direct));
        network.resolve("localhost", Duration::from_secs(20), &hangup).unwrap();
        let e = network.resolve("vento-test.invalid", Duration::from_secs(20), &hangup).unwrap_err();
        assert!(crate::net::is_lookup_failure(&e), "{e:?}");
        struct Connecting;
        impl Network for Connecting {
            fn connect(&self, _: &str, _: u16, _: Timeout, _: &Liveness, _: &Hangup) -> io::Result<Connection> {
                Err(io::ErrorKind::ConnectionRefused.into())
            }
        }
        let e = Socketpair::new(Arc::new(Connecting))
            .resolve("localhost", Duration::from_secs(20), &hangup)
            .unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::Unsupported);
    }

    #[test]
    fn a_hang_up_ends_a_dial_through_the_relay() {
        let hangup = Arc::new(Hangup::new());
        let started = Instant::now();
        let dialling = {
            let hangup = Arc::clone(&hangup);
            std::thread::spawn(move || {
                Socketpair::new(Arc::new(Direct)).connect(
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
        assert!(dialling.join().unwrap().is_err());
        assert!(started.elapsed() < Duration::from_secs(3));
    }
}
