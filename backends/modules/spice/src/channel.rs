// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! One channel of a session: its own connection, its own link handshake, its
//! own ticket and its own ack accounting.
//!
//! A SPICE session is four of these (main, display, inputs, cursor), plus
//! playback where the server has sound and the session plays it. The session
//! id from the main channel's `INIT` ties them together. Everything below the
//! session layer is the same for all of them, so it is here once.
//!
//! **Reading is a separate task; writing is not.** Each channel's reader is
//! spawned onto the session's runtime and pushes whole messages into one
//! queue, so the session `select!`s over a queue rather than over several
//! `read_exact`s. This matters because `read_exact` is not cancel-safe: a
//! `select!` that drops one halfway loses bytes from the middle of a message.
//! The write half stays behind a mutex the session takes per message.

use crate::error::{Error, Result};
use crate::tls;
use shakenfist_spice_protocol::constants::{ChannelType, SpiceError};
use runtime::Hangup;
use runtime::net::{EndReason, Liveness, Network, Socket, Timeout};
use shakenfist_spice_protocol::link::{SpiceLinkMess, SpiceLinkReply, perform_auth};
use shakenfist_spice_protocol::messages::{MessageHeader, make_message};
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{Mutex, mpsc};

/// Which channel a message came off, for a session that reads one queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Main,
    Display,
    Inputs,
    Cursor,
    Playback,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Main => "main",
            Kind::Display => "display",
            Kind::Inputs => "inputs",
            Kind::Cursor => "cursor",
            Kind::Playback => "playback",
        }
    }

    pub fn channel_type(self) -> ChannelType {
        match self {
            Kind::Main => ChannelType::Main,
            Kind::Display => ChannelType::Display,
            Kind::Inputs => ChannelType::Inputs,
            Kind::Cursor => ChannelType::Cursor,
            Kind::Playback => ChannelType::Playback,
        }
    }
}

/// The playback channel's capabilities (`SPICE_PLAYBACK_CAP_*` in
/// spice-protocol's `enums.h`), which ryll's protocol crate does not define.
/// CELT (bit 0) is obsolete and not offered.
pub mod playback_caps {
    /// The guest's volume and mute, as messages rather than in the samples.
    pub const VOLUME: u32 = 1 << 1;
    /// Opus rather than PCM: a tenth of the bytes, and what a server with
    /// libopus picks for a client that offers it.
    pub const OPUS: u32 = 1 << 3;
}

/// A connection's bytes, plain or inside TLS, whichever kind of socket is
/// under them.
pub trait Duplex: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T: AsyncRead + AsyncWrite + Unpin + Send> Duplex for T {}

/// Where every channel of a session connects, and through what.
pub struct Server {
    pub host: String,
    pub port: u16,
    pub network: Arc<dyn Network>,
    /// Ends a dial in progress, which runs on the runtime's blocking pool
    /// and so outlives a dropped session future otherwise.
    pub hangup: Arc<Hangup>,
}

impl Server {
    /// A connection to the server, as the session's async I/O takes it, and
    /// where its route records why it ended.
    async fn dial(&self, deadline: Duration) -> Result<(Box<dyn Duplex>, EndReason)> {
        let network = Arc::clone(&self.network);
        let hangup = Arc::clone(&self.hangup);
        let (host, port) = (self.host.clone(), self.port);
        let connection = tokio::task::spawn_blocking(move || {
            network.connect(&host, port, Timeout::Whole(deadline), &Liveness::default(), &hangup)
        })
        .await
        .map_err(|e| Error::Io(std::io::Error::other(e)))??;
        let socket = connection.socket;
        // Input is small messages that must not wait for a full segment.
        socket.set_nodelay(true).ok();
        socket.set_nonblocking(true)?;
        let stream: Box<dyn Duplex> = match socket {
            Socket::Tcp(s) => Box::new(tokio::net::TcpStream::from_std(s)?),
            Socket::Unix(s) => Box::new(tokio::net::UnixStream::from_std(s)?),
        };
        Ok((stream, connection.end))
    }
}

/// One message off the wire, and which channel it came from.
pub struct Message {
    pub kind: Kind,
    pub message_type: u16,
    pub body: Vec<u8>,
}

/// What the reader task hands back when its channel ends.
pub struct Ended {
    pub kind: Kind,
    pub error: Option<Error>,
}

/// The write half of a channel, plus what a session has to remember about it.
pub struct Channel {
    writer: Arc<Mutex<Box<dyn AsyncWrite + Unpin + Send>>>,
    /// The task reading the other half, stopped when the channel is dropped:
    /// with both halves gone the socket closes and the server sees the channel
    /// end (which is how the sound is turned off), and no `Ended` is sent.
    reader: tokio::task::JoinHandle<()>,
    /// The server stops sending after this many unacknowledged messages, so a
    /// client that never acks gets one screenful and then silence,
    /// indistinguishable from an idle server.
    pub ack_window: u32,
    since_ack: u32,
    /// Held acks: the ack window is this protocol's own flow control, so a
    /// session that has left the screen stops acking and the server stops
    /// sending. See `Session::focus` in `client.rs`.
    paused: bool,
    /// The session's sent-bytes counter, shared by all its channels. The
    /// received counter belongs to the reader task and is only added to there.
    sent: Arc<AtomicU64>,
}

impl Channel {
    /// Dial, link and authenticate one channel, then split it: the reader goes
    /// onto `queue` as a task of its own and the write half comes back.
    ///
    /// `session_id` is 0 for the main channel and the id its `INIT` gave for
    /// every other one. `trust` is asked once, on the first channel of a TLS
    /// session, at the only point where the answer matters: after the
    /// handshake, when there is a certificate to check, and before the link,
    /// when the ticket is sent. Every other channel must present the
    /// certificate accepted then (`tls::Connector::admit`).
    ///
    /// **The deadline applies to each step and never to the question.**
    /// Reading a fingerprint takes longer than a server takes to answer, and a
    /// timeout covering both would end the session while the dialog is still
    /// on screen.
    ///
    /// `caps` are channel capabilities beyond the ones this client always
    /// offers on a channel of this kind: what the playback channel offers
    /// depends on what the platform decodes.
    #[allow(clippy::too_many_arguments)]
    pub async fn open(
        kind: Kind,
        caps: u32,
        server: &Server,
        session_id: u32,
        password: Option<&str>,
        tls: Option<&tls::Connector>,
        trust: Option<&mut dyn FnMut(&str) -> bool>,
        queue: mpsc::Sender<Message>,
        ended: mpsc::Sender<Ended>,
        traffic: (Arc<AtomicU64>, Arc<AtomicU64>),
        deadline: Duration,
    ) -> Result<Channel> {
        // The network's own deadline, over the lookup and every address:
        // one per channel, as before the channels went through the network.
        // Each channel is its own connection, so each has its own end: a
        // relayed channel's end of stream is explained by what its route says
        // ended it, so that a reset reads as one either way.
        let (plain, end) = server.dial(deadline).await?;

        let mut stream: Box<dyn Duplex> = match tls {
            Some(connector) => {
                let (stream, fingerprint) = within(deadline, connector.connect(plain)).await??;
                match connector.admit(fingerprint.as_deref(), trust) {
                    Err(Error::Cancelled) => return Err(Error::Cancelled),
                    Err(e) => {
                        log::warn!("{}: {e}", kind.name());
                        return Err(e);
                    }
                    Ok(()) => {}
                }
                Box::new(stream)
            }
            None => plain,
        };

        let reply = within(deadline, link(&mut stream, session_id, kind, caps))
            .await?
            .map_err(|e| e.explained(&end))?;
        // The link reply's public key is not an identity: QEMU generates an
        // RSA-1024 key per *connection* only to encrypt this ticket, so there
        // is nothing to pin, and on a plain port the password is protected
        // against a passive observer only.
        within(deadline, perform_auth(&mut stream, &reply.pub_key, password))
            .await?
            .map_err(|e| refused(&e.to_string()))?;

        let (read, write) = tokio::io::split(stream);
        // One pair of counters for the whole session rather than per channel,
        // since the panel has one traffic row.
        let (received, sent) = traffic;
        let reader = tokio::spawn(async move {
            let error = read_messages(kind, read, &queue, &received)
                .await
                .err()
                .map(|e| e.explained(&end));
            let _ = ended.send(Ended { kind, error }).await;
        });

        Ok(Channel {
            writer: Arc::new(Mutex::new(Box::new(write))),
            reader,
            ack_window: 0,
            since_ack: 0,
            paused: false,
            sent,
        })
    }

    pub async fn send(&self, message_type: u16, payload: &[u8]) -> Result<()> {
        let message = make_message(message_type, payload);
        let mut writer = self.writer.lock().await;
        writer.write_all(&message).await?;
        writer.flush().await?;
        self.sent.fetch_add(message.len() as u64, Ordering::Relaxed);
        Ok(())
    }

    /// Count a message towards the ack window and answer when it is full.
    ///
    /// Every message counts, including ones the client ignores, because the
    /// server counts them all.
    pub async fn acknowledge(&mut self, ack: u16) -> Result<()> {
        self.since_ack += 1;
        if self.ack_window > 0 && !self.paused && self.since_ack >= self.ack_window {
            self.since_ack = 0;
            self.send(ack, &[]).await?;
        }
        Ok(())
    }

    /// Stop or resume acking, which an unfocused session does instead of
    /// sending the pause message this protocol lacks.
    pub async fn set_paused(&mut self, paused: bool, ack: u16) -> Result<()> {
        let was = self.paused;
        self.paused = paused;
        if was && !paused && self.ack_window > 0 {
            // Messages received while paused are acknowledged on resume, so
            // the picture restarts at once rather than after another window of
            // messages, but one window per ack and only whole windows. The
            // server subtracts a whole window from its unsigned count per ack,
            // so an ack for part of a window wraps that count around, and the
            // channel then waits for an ack that never comes: nothing more
            // arrives on it, including the new surface after a resize. A
            // partial window needs no ack, since the server has not stopped
            // for it.
            let windows = self.since_ack / self.ack_window;
            self.since_ack %= self.ack_window;
            for _ in 0..windows {
                self.send(ack, &[]).await?;
            }
        }
        Ok(())
    }

}

impl Drop for Channel {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

/// The link handshake, with capabilities of this client's own.
///
/// The same handshake as ryll's `perform_link`, with a different capability
/// list: ryll's display default advertises **H.264 video streams**, and this
/// build has that decoder patched out, so a server that took the offer would
/// send a codec this client cannot decode and the region under the stream
/// would freeze. It also advertises the stream *report*, a message this client
/// never sends.
///
/// The rest of ryll's list is unchanged, chiefly `COMPOSITE`, without which the
/// guest's QXL driver falls back to a software path and sends far fewer
/// updates.
async fn link<S>(stream: &mut S, session_id: u32, kind: Kind, caps: u32) -> Result<SpiceLinkReply>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    use shakenfist_spice_protocol::constants::capabilities as cap;
    let channel_caps = match kind {
        // The agent's token count, which a server sends only to a client that
        // asks for it: without this bit, an agent connecting *after* the
        // session is announced with no token count, and a client that sends
        // more than it has been granted is disconnected.
        Kind::Main => cap::DEFAULT_MAIN | cap::MAIN_AGENT_CONNECTED_TOKENS,
        Kind::Display => {
            cap::DISPLAY_SIZED_STREAM
                | cap::DISPLAY_MONITORS_CONFIG
                | cap::DISPLAY_COMPOSITE
                | cap::DISPLAY_A8_SURFACE
                | cap::DISPLAY_LZ4_COMPRESSION
                | cap::DISPLAY_PREF_COMPRESSION
                | cap::DISPLAY_MULTI_CODEC
                | cap::DISPLAY_CODEC_MJPEG
        }
        // Only what the caller adds: the volume, and Opus where the
        // platform decodes it. Without the Opus bit a server sends PCM.
        Kind::Playback => 0,
        // Neither has a capability this client uses. The inputs channel's only
        // one is the scancode message, which this client does not send; the
        // cursor channel has none.
        Kind::Inputs | Kind::Cursor => 0,
    };
    let message = SpiceLinkMess::new(
        session_id,
        kind.channel_type(),
        0,
        cap::DEFAULT_COMMON,
        channel_caps | caps,
    );
    stream.write_all(&message.serialize()).await?;
    stream.flush().await?;

    // The reply's header gives the size of the rest, and the parser needs
    // both in one buffer.
    let mut reply = vec![0u8; 16];
    stream.read_exact(&mut reply).await?;
    let size = u32::from_le_bytes(reply[12..16].try_into().expect("four bytes")) as usize;
    if size > MAX_LINK_REPLY {
        return Err(Error::Protocol(format!("a {size} byte link reply")));
    }
    reply.resize(16 + size, 0);
    stream.read_exact(&mut reply[16..]).await?;
    SpiceLinkReply::parse(&reply).map_err(|e| Error::Protocol(e.to_string()))
}

/// ryll's cap on a link message (`MAX_LINK_MESSAGE_SIZE`), applied to the
/// reply as well: real ones are a couple of hundred bytes.
const MAX_LINK_REPLY: usize = 4096;

/// One step of a connection, with a deadline. Without it, a server that stops
/// answering halfway through a handshake (as with a mistyped port) leaves a
/// session that never opens and reports no error.
async fn within<T>(deadline: Duration, step: impl Future<Output = T>) -> Result<T> {
    tokio::time::timeout(deadline, step)
        .await
        .map_err(|_| Error::Io(std::io::ErrorKind::TimedOut.into()))
}

/// A refused ticket is the one error worth telling apart, because the user can
/// fix it. It arrives as text, since ryll's `perform_auth` returns an `anyhow`
/// error containing the link error's name.
fn refused(text: &str) -> Error {
    if text.contains(&format!("{:?}", SpiceError::PermissionDenied))
        || text.contains("Authentication failed")
    {
        Error::Refused(text.into())
    } else {
        Error::Protocol(text.into())
    }
}

/// The reader task: whole messages onto the queue until the socket ends.
async fn read_messages<R: AsyncRead + Unpin>(
    kind: Kind,
    mut read: R,
    queue: &mpsc::Sender<Message>,
    received: &AtomicU64,
) -> Result<()> {
    let mut header = [0u8; MessageHeader::SIZE];
    loop {
        read.read_exact(&mut header).await?;
        let header = MessageHeader::read(&header)?;
        // A message size is a `u32` off the wire and nothing is allocated
        // before it is checked.
        if header.message_size as usize > MAX_MESSAGE {
            return Err(Error::Protocol(format!(
                "{} sent a {} byte message",
                kind.name(),
                header.message_size
            )));
        }
        let mut body = vec![0u8; header.message_size as usize];
        read.read_exact(&mut body).await?;
        received.fetch_add((MessageHeader::SIZE + body.len()) as u64, Ordering::Relaxed);
        if queue
            .send(Message {
                kind,
                message_type: header.message_type,
                body,
            })
            .await
            .is_err()
        {
            return Ok(()); // the session has gone
        }
    }
}

/// The cap on one message, which bounds what a hostile server can make this
/// client allocate. A whole 4K desktop as one uncompressed image is 33 MB, the
/// largest thing a server has any reason to send, and QEMU compresses it
/// anyway.
const MAX_MESSAGE: usize = 64 * 1024 * 1024;
