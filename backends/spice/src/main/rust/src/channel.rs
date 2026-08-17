//! One channel of a session: its own TCP connection, its own link handshake,
//! its own ticket and its own ack accounting.
//!
//! A SPICE session is four of these — main, display, inputs, cursor — and what
//! ties them together is the session id the main channel's `INIT` hands out.
//! Everything below the session layer is the same for all four, which is why
//! it is here rather than four times over.
//!
//! **Reading is a task of its own and writing is not.** Every channel's reader
//! is spawned onto the session's runtime and pushes whole messages into one
//! queue, so the session `select!`s over a queue rather than over four
//! `read_exact`s — which matters because `read_exact` is not cancel-safe, and a
//! `select!` that drops one halfway has eaten bytes out of the middle of
//! somebody's message. The write half stays behind a mutex the session takes
//! per message.

use crate::error::{Error, Result};
use crate::tls;
use shakenfist_spice_protocol::constants::{ChannelType, SpiceError};
use shakenfist_spice_protocol::link::{SpiceLinkMess, SpiceLinkReply, SpiceStream, perform_auth};
use shakenfist_spice_protocol::messages::{MessageHeader, make_message};
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{Mutex, mpsc};

/// Which channel a message came off, for a session that reads one queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Main,
    Display,
    Inputs,
    Cursor,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Main => "main",
            Kind::Display => "display",
            Kind::Inputs => "inputs",
            Kind::Cursor => "cursor",
        }
    }

    fn channel_type(self) -> ChannelType {
        match self {
            Kind::Main => ChannelType::Main,
            Kind::Display => ChannelType::Display,
            Kind::Inputs => ChannelType::Inputs,
            Kind::Cursor => ChannelType::Cursor,
        }
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
    /// The server stops sending after this many unacknowledged messages, so a
    /// client that never acks gets one screenful and then silence that looks
    /// exactly like a server with nothing to say.
    pub ack_window: u32,
    since_ack: u32,
    /// Held acks: the ack window is this protocol's own flow control, so a
    /// session that has left the screen stops answering and the server stops
    /// sending. See `Client::set_focused`.
    paused: bool,
    /// The session's own counter, shared by all four channels — the received
    /// half of the pair belongs to the reader task and is only ever added to
    /// there.
    sent: Arc<AtomicU64>,
}

impl Channel {
    /// Dial, link and authenticate one channel, then split it: the reader goes
    /// onto `queue` as a task of its own and the write half comes back.
    ///
    /// `session_id` is 0 for the main channel and the id its `INIT` gave for
    /// every other one. `trust` is asked once, on the first channel of a TLS
    /// session, in the one moment where the answer is worth anything: after the
    /// handshake, which is when there is a certificate to judge, and before the
    /// link, which is when the ticket goes out.
    ///
    /// **The deadline is around each step and never around the question.** A
    /// person reading a fingerprint takes longer than a server does to answer,
    /// and a timeout that covers both kills the session while the dialog is
    /// still on screen — which is what it did here, at twenty seconds, until
    /// this was three deadlines rather than one.
    #[allow(clippy::too_many_arguments)]
    pub async fn open(
        kind: Kind,
        address: (&str, u16),
        session_id: u32,
        password: Option<&str>,
        tls: Option<&tls::Connector>,
        trust: Option<&mut dyn FnMut(&str) -> bool>,
        queue: mpsc::Sender<Message>,
        ended: mpsc::Sender<Ended>,
        traffic: (Arc<AtomicU64>, Arc<AtomicU64>),
        deadline: Duration,
    ) -> Result<Channel> {
        let tcp = within(deadline, TcpStream::connect(address)).await??;
        // Input is small messages that must not wait for a full segment.
        tcp.set_nodelay(true)?;

        let mut stream = match tls {
            Some(connector) => {
                let stream = within(deadline, connector.connect(tcp)).await??;
                if let (Some(ask), Some(fingerprint)) = (trust, connector.fingerprint())
                    && !ask(&fingerprint)
                {
                    return Err(Error::Cancelled);
                }
                SpiceStream::Tls(stream)
            }
            None => SpiceStream::Plain(tcp),
        };

        let reply = within(deadline, link(&mut stream, session_id, kind)).await??;
        // The link reply carries a public key, and 46a asked whether it is an
        // identity: it is not. QEMU generates an RSA-1024 key per *connection*
        // for the sole purpose of encrypting this ticket, so there is nothing
        // to pin and the password crosses a plain port protected against a
        // passive observer alone.
        within(deadline, perform_auth(&mut stream, &reply.pub_key, password))
            .await?
            .map_err(|e| refused(&e.to_string()))?;

        let (read, write) = tokio::io::split(stream);
        // One pair of counters for the whole session rather than one per
        // channel: the panel has one row, and four numbers that have to be
        // added up before they mean anything are four chances to add them up
        // differently.
        let (received, sent) = traffic;
        {
            tokio::spawn(async move {
                let error = read_messages(kind, read, &queue, &received).await.err();
                let _ = ended.send(Ended { kind, error }).await;
            });
        }

        Ok(Channel {
            writer: Arc::new(Mutex::new(Box::new(write))),
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
    /// Every message counts, not only the ones a client finds interesting,
    /// because that is what the server counts too.
    pub async fn acknowledge(&mut self, ack: u16) -> Result<()> {
        self.since_ack += 1;
        if self.ack_window > 0 && !self.paused && self.since_ack >= self.ack_window {
            self.since_ack = 0;
            self.send(ack, &[]).await?;
        }
        Ok(())
    }

    /// Stop or resume answering the window, which is what an unfocused session
    /// does instead of the pause message this protocol does not have.
    pub async fn set_paused(&mut self, paused: bool, ack: u16) -> Result<()> {
        let was = self.paused;
        self.paused = paused;
        if was && !paused && self.since_ack > 0 {
            // Whatever built up while nobody was looking is acknowledged in one
            // go, so the picture starts again on the way back rather than after
            // another window's worth of messages.
            self.since_ack = 0;
            self.send(ack, &[]).await?;
        }
        Ok(())
    }

}

/// The link handshake, with capabilities of this client's own.
///
/// Theirs is `perform_link` and this is the same handshake, because the
/// capability list is not: their display default advertises **H.264 video
/// streams**, and this build has that decoder patched out, so a server that
/// took the offer would send a codec this end cannot decode and the region
/// under the stream would freeze. It advertises the stream *report* too, which
/// is a message this client never sends back.
///
/// The rest is what makes the picture work and is theirs unchanged — chiefly
/// `COMPOSITE`, without which the guest's QXL driver falls back to a software
/// path and sends far fewer updates.
async fn link<S>(stream: &mut S, session_id: u32, kind: Kind) -> Result<SpiceLinkReply>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    use shakenfist_spice_protocol::constants::capabilities as cap;
    let channel_caps = match kind {
        // The agent's token count, which a server sends only to a client that
        // asked for it: without this bit an agent connecting *after* the
        // session says so with no number in the message, and a client that
        // sends what it has not been granted is disconnected.
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
        _ => cap::DEFAULT_MAIN,
    };
    let message = SpiceLinkMess::new(
        session_id,
        kind.channel_type(),
        0,
        cap::DEFAULT_COMMON,
        channel_caps,
    );
    stream.write_all(&message.serialize()).await?;
    stream.flush().await?;

    // The reply's own header says how much follows it, and the parser wants
    // both halves in one buffer.
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

/// Their own cap on a link message, applied to the reply as well: real ones are
/// a couple of hundred bytes.
const MAX_LINK_REPLY: usize = 4096;

/// One step of a connection, with a deadline on it. A server that stops
/// answering halfway through a handshake is a session that never opens and
/// never says why, which is what a mistyped port looks like.
async fn within<T>(deadline: Duration, step: impl Future<Output = T>) -> Result<T> {
    tokio::time::timeout(deadline, step)
        .await
        .map_err(|_| Error::Io(std::io::ErrorKind::TimedOut.into()))
}

/// A refused ticket is the one error worth telling apart, because it is the one
/// with a person on the other end of it — and it arrives as text, since what
/// their auth hands back is an `anyhow` with the link error's name in it.
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

/// The cap on one message, which is a bound on what a hostile server can make
/// this end allocate. A whole 4K desktop as one uncompressed image is 33 MB and
/// is the largest thing a server has any reason to send — and it does not, since
/// QEMU compresses anything worth compressing.
const MAX_MESSAGE: usize = 64 * 1024 * 1024;
