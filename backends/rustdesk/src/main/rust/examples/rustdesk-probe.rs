// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! A RustDesk client small enough to read: connect, log in, and write out what
//! the far end sends. It answered whether a backend could be written at all —
//! the framing, the password, the login exchange and the codec negotiation —
//! before there was a module, a JNI half or a phone in the way, and it is kept
//! because it is the only thing here that speaks **both** of this protocol's
//! transports.
//!
//! ```sh
//! scripts/testrustdesk/run.sh
//! cargo run --example rustdesk-probe -- direct 10.33.0.208 --password PASSword1 --out /tmp/rig.ivf
//! cargo run --example rustdesk-probe -- id 3936346 --password PASSword1 --seconds 30
//! ffprobe /tmp/rig.ivf                       # that it is VP9 of the right size
//! ffmpeg -i /tmp/rig.ivf -frames:v 1 f.png   # that it is the far end's screen
//! ```
//!
//! `direct` is the plaintext mode, on port 21118, and says so. `id` goes
//! through a rendezvous server — the public one and its compiled-in key unless
//! `--server` and `--key` say otherwise — verifies the two signatures and
//! encrypts the session. Both are the crate's own code: what this file is, is
//! the parts a backend has no use for — a session on one thread, every message
//! counted by arm, and the frames written out as a file to feed a decoder.
//!
//! `--after-login` selects which of the three messages a client sends once
//! `PeerInfo` arrives: `sid` answers a Windows peer's session choice, `capture`
//! names the display to capture, `refresh` asks for a key frame. All three by
//! default; the flag exists because which of them the picture depends on is a
//! measurement rather than a reading.

use remotedesktop_rustdesk::{crypto, protos, rendezvous, wire};

use std::io::Write;
use std::sync::Arc;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use protobuf::MessageField;
use sha2::{Digest, Sha256};

use protos::message::{
    message, video_frame, EncodedVideoFrame, LoginRequest, Message, OptionMessage,
    SupportedDecoding,
};


/// Their direct-access port, which is `RENDEZVOUS_PORT + 2`.
const DIRECT_PORT: u16 = 21118;

/// What we claim to be. Their server compares this against its own for feature
/// gating, so an honest one avoids a compatibility path nothing here implements.
const VERSION: &str = "1.4.9";

fn main() {
    // The crate says what it is doing through `log`, and on a phone that goes
    // to logcat. Here there is nothing to install one, and the rendezvous path
    // is exactly the part whose running commentary is the point of a probe.
    log::set_logger(&Stderr).ok();
    log::set_max_level(log::LevelFilter::Info);
    if let Err(e) = run() {
        eprintln!("probe: {e}");
        std::process::exit(1);
    }
}

struct Stderr;

impl log::Log for Stderr {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        eprintln!("   {}", record.args());
    }

    fn flush(&self) {}
}

struct Args {
    peer: String,
    password: String,
    server: String,
    key: String,
    frames: usize,
    seconds: u64,
    out: Option<String>,
    after_login: String,
}

fn run() -> Result<(), String> {
    let mut a = std::env::args().skip(1);
    let mode = a.next().unwrap_or_default();
    let peer = a.next().unwrap_or_default();
    if peer.is_empty() || !matches!(mode.as_str(), "direct" | "id") {
        return Err(format!(
            "usage:\n  rustdesk-probe direct <host[:port]> [options]\n  \
             rustdesk-probe id <peer-id> [options]\n\
             options: --password P --server HOST --key BASE64 --frames N --seconds N --out FILE\n\
             --after-login sid,capture,refresh (what the picture needs)"
        ));
    }
    let mut args = Args {
        peer,
        password: String::new(),
        server: rendezvous::PUBLIC_SERVER.into(),
        key: rendezvous::PUBLIC_KEY.into(),
        frames: 30,
        seconds: 20,
        out: None,
        after_login: "sid,capture,refresh".into(),
    };
    while let Some(flag) = a.next() {
        let v = a.next().unwrap_or_default();
        match flag.as_str() {
            "--password" => args.password = v,
            "--server" => args.server = v,
            "--key" => args.key = v,
            "--frames" => args.frames = v.parse().map_err(|_| "--frames wants a number")?,
            "--seconds" => args.seconds = v.parse().map_err(|_| "--seconds wants a number")?,
            "--out" => args.out = Some(v),
            "--after-login" => args.after_login = v,
            _ => return Err(format!("unknown option {flag}")),
        }
    }

    let mut stream = match mode.as_str() {
        "direct" => connect_direct(&args)?,
        _ => connect_by_id(&args)?,
    };
    println!(
        "== transport: {}",
        if stream.is_secure() {
            "encrypted (xsalsa20-poly1305)"
        } else {
            "PLAINTEXT — readable by anyone on the path"
        }
    );
    session(&mut stream, &args)
}


// ---- the socket, as a probe wants one -------------------------------------

/// Both directions of one socket on one thread, which is what a probe wants and
/// the backend does not: there, the two halves are on two threads so that a
/// stalled write cannot hold up a read.
struct Stream {
    reader: wire::Reader,
    writer: wire::Writer,
    traffic: Arc<wire::Traffic>,
    secure: bool,
}

impl Stream {
    fn new(sock: TcpStream) -> Result<Stream, String> {
        let traffic = Arc::new(wire::Traffic::default());
        let (reader, writer, _) =
            wire::split(sock, Arc::clone(&traffic)).map_err(|e| e.to_string())?;
        Ok(Stream {
            reader,
            writer,
            traffic,
            secure: false,
        })
    }

    fn send(&mut self, msg: &impl protobuf::Message) -> std::io::Result<()> {
        self.writer.send(msg)
    }

    fn next_message(&mut self) -> std::io::Result<Message> {
        self.reader.message::<Message>()
    }

    /// Everything after the frame carrying it is sealed, in both directions,
    /// and the two directions count their frames separately.
    fn set_key(&mut self, key: [u8; 32]) {
        self.reader.set_cipher(Box::new(crypto::Secretbox::new(key)));
        self.writer.set_cipher(Box::new(crypto::Secretbox::new(key)));
        self.secure = true;
    }

    fn is_secure(&self) -> bool {
        self.secure
    }

    fn sent_bytes(&self) -> u64 {
        self.traffic.sent.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn received_bytes(&self) -> u64 {
        self.traffic
            .received
            .load(std::sync::atomic::Ordering::Relaxed)
    }
}

// ---- getting a socket ------------------------------------------------------

fn resolve(host: &str, default_port: u16) -> Result<SocketAddr, String> {
    let with_port = if host.contains(':') {
        host.to_string()
    } else {
        format!("{host}:{default_port}")
    };
    with_port
        .to_socket_addrs()
        .map_err(|e| format!("resolving {with_port}: {e}"))?
        .next()
        .ok_or_else(|| format!("{with_port} resolves to nothing"))
}

fn tcp(addr: SocketAddr, seconds: u64) -> Result<TcpStream, String> {
    let sock = TcpStream::connect_timeout(&addr, Duration::from_secs(10))
        .map_err(|e| format!("connecting to {addr}: {e}"))?;
    sock.set_nodelay(true).ok();
    sock.set_read_timeout(Some(Duration::from_secs(seconds)))
        .map_err(|e| e.to_string())?;
    Ok(sock)
}

/// The plaintext mode. Their `direct_server` hands the socket to
/// `create_tcp_connection` with `secure = false`, so no key is offered and none
/// is asked for; the client sends an empty `Message` because it has nothing to
/// verify with, which is the whole of the handshake this mode has.
fn connect_direct(args: &Args) -> Result<Stream, String> {
    let addr = resolve(&args.peer, DIRECT_PORT)?;
    println!("== direct to {addr}");
    let mut s = Stream::new(tcp(addr, args.seconds)?)?;
    s.send(&Message::new()).map_err(|e| e.to_string())?;
    Ok(s)
}

/// A peer reached by id, through the crate's own rendezvous path: the server
/// introduces the two ends and hands over the peer's long-term key signed with
/// its own, and the punch, the relay and the two signatures are all in there.
///
/// What the probe adds is that it says each step out loud, which is why it is
/// still the thing to reach for when a far end will not answer.
fn connect_by_id(args: &Args) -> Result<Stream, String> {
    let reached = rendezvous::reach(&args.peer, &args.server, key_of(args), Duration::from_secs(10))
        .map_err(|e| e.to_string())?;
    println!(
        "== {} ({} bytes of peer key)",
        reached.how.describe(),
        reached.signed_id_pk.len()
    );
    reached
        .sock
        .set_read_timeout(Some(Duration::from_secs(args.seconds)))
        .map_err(|e| e.to_string())?;
    let mut s = Stream::new(reached.sock)?;
    secure(&mut s, &args.peer, &reached.signed_id_pk, &args.key)?;
    Ok(s)
}

/// What a self-hosted server wants named in the request and the public network
/// wants left out: the same key the peer's own signature is checked against,
/// which their server calls a licence.
fn key_of(args: &Args) -> &str {
    if args.key == rendezvous::PUBLIC_KEY {
        ""
    } else {
        &args.key
    }
}

/// The handshake the direct mode has none of, which is **two signatures rather
/// than one**: the server vouching for the peer's long-term key, and the peer
/// signing a per-connection box key with it. The crate does each step; this
/// prints between them, since a probe exists to say which half failed.
fn secure(s: &mut Stream, peer_id: &str, signed_id_pk: &[u8], key_b64: &str) -> Result<(), String> {
    use protos::message::PublicKey;

    if signed_id_pk.is_empty() {
        return Err("the rendezvous server sent no peer key".into());
    }
    let server_pk = crypto::server_key(key_b64).map_err(|e| e.to_string())?;
    let their_sign_pk = crypto::open_id_pk(signed_id_pk, &server_pk, peer_id)
        .map_err(|e| format!("the server's introduction: {e}"))?;
    println!(
        "== the rendezvous server vouches for {peer_id}: {}",
        crypto::fingerprint(&their_sign_pk)
    );

    let msg = s.next_message().map_err(|e| e.to_string())?;
    let Some(message::Union::SignedId(si)) = msg.union else {
        return Err("the peer sent no SignedId, so this socket is not a secure one".into());
    };
    let their_pk = crypto::open_id_pk(&si.id, &their_sign_pk, peer_id)
        .map_err(|e| format!("the peer's own signature: {e}"))?;
    println!("== the peer signed its session key with the key it was vouched for");

    let (our_pk, sealed, key) = crypto::seal_session_key(&their_pk).map_err(|e| e.to_string())?;
    let mut pk = PublicKey::new();
    pk.asymmetric_value = our_pk.into();
    pk.symmetric_value = sealed.into();
    let mut out = Message::new();
    out.set_public_key(pk);
    s.send(&out).map_err(|e| e.to_string())?;
    s.set_key(key);
    Ok(())
}

// ---- the session ------------------------------------------------------------

fn session(s: &mut Stream, args: &Args) -> Result<(), String> {
    let started = Instant::now();
    let deadline = Duration::from_secs(args.seconds);

    // The server opens with Hash before anything is asked of it.
    let hash = loop {
        let msg = s.next_message().map_err(|e| format!("waiting for Hash: {e}"))?;
        match msg.union {
            Some(message::Union::Hash(h)) => break h,
            Some(other) => println!("   (before Hash: {})", arm(&other)),
            None => {}
        }
    };
    println!(
        "== Hash: salt {} chars, challenge {} chars",
        hash.salt.len(),
        hash.challenge.len()
    );

    // sha256(sha256(password ++ salt) ++ challenge). The inner hash is what a
    // client stores, so a saved password is already salted per peer.
    let mut password = Vec::new();
    if !args.password.is_empty() {
        let stored = Sha256::new()
            .chain_update(args.password.as_bytes())
            .chain_update(hash.salt.as_bytes())
            .finalize();
        password = Sha256::new()
            .chain_update(stored)
            .chain_update(hash.challenge.as_bytes())
            .finalize()
            .to_vec();
    }

    let mut lr = LoginRequest::new();
    lr.username = args.peer.clone();
    lr.password = password.into();
    lr.my_id = "probe".into();
    lr.my_name = "probe".into();
    lr.my_platform = "Linux".into();
    lr.version = VERSION.into();
    lr.session_id = 1;
    // The one thing the client must ask for: a peer told VP9 is preferred sends
    // VP9 unconditionally, and VP9 is compiled into every build of theirs.
    let mut decoding = SupportedDecoding::new();
    decoding.ability_vp9 = 1;
    decoding.prefer = protos::message::supported_decoding::PreferCodec::VP9.into();
    let mut option = OptionMessage::new();
    option.supported_decoding = MessageField::some(decoding);
    lr.option = MessageField::some(option);
    let mut out = Message::new();
    out.set_login_request(lr);
    s.send(&out).map_err(|e| e.to_string())?;

    let mut video = Video::default();
    let mut counts: Vec<(&'static str, usize)> = Vec::new();
    let mut logged_in = false;

    while started.elapsed() < deadline && video.frames.len() < args.frames {
        let msg = match s.next_message() {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(e) => return Err(format!("reading: {e}")),
        };
        let Some(u) = msg.union else { continue };
        let name = arm(&u);
        match counts.iter_mut().find(|(n, _)| *n == name) {
            Some((_, c)) => *c += 1,
            None => counts.push((name, 1)),
        }
        match u {
            message::Union::LoginResponse(r) => {
                match r.union {
                    Some(protos::message::login_response::Union::Error(e)) => {
                        return Err(format!("login refused: {e}"));
                    }
                    Some(protos::message::login_response::Union::PeerInfo(p)) => {
                        logged_in = true;
                        println!(
                            "== PeerInfo: {} on {} running {}, {} display(s), current {}",
                            p.hostname,
                            p.platform,
                            p.version,
                            p.displays.len(),
                            p.current_display
                        );
                        for (i, d) in p.displays.iter().enumerate() {
                            println!(
                                "   display {i}: {}x{} at {},{} \"{}\"{}",
                                d.width,
                                d.height,
                                d.x,
                                d.y,
                                d.name,
                                if d.cursor_embedded { " cursor-embedded" } else { "" }
                            );
                        }
                        if let Some(d) = p.displays.get(p.current_display as usize) {
                            video.width = d.width as u16;
                            video.height = d.height as u16;
                        }
                        // A Windows peer with RDP sharing on and more than one
                        // session logged in **stops here**: it subscribes no
                        // video service until the client names a session id.
                        // Nothing announces this as a wait — login succeeds, the
                        // desktop simply never arrives.
                        if let Some(w) = p.windows_sessions.as_ref() {
                            println!(
                                "   windows sessions: {} offered, current {}",
                                w.sessions.len(),
                                w.current_sid
                            );
                            for sess in &w.sessions {
                                println!("      sid {} \"{}\"", sess.sid, sess.name);
                            }
                            if args.after_login.contains("sid") {
                                let mut misc = protos::message::Misc::new();
                                misc.set_selected_sid(w.current_sid);
                                let mut out = Message::new();
                                out.set_misc(misc);
                                s.send(&out).map_err(|e| e.to_string())?;
                            }
                        }
                        // What a client of this version is expected to send at
                        // once: which displays it wants captured, and a refresh
                        // so the first frame is a key frame rather than a wait
                        // for whatever the encoder does next. Separable so the
                        // stage can say which of the two the picture depends on.
                        if args.after_login.contains("capture") {
                            let mut caps = protos::message::CaptureDisplays::new();
                            caps.set = vec![p.current_display];
                            let mut misc = protos::message::Misc::new();
                            misc.set_capture_displays(caps);
                            let mut out = Message::new();
                            out.set_misc(misc);
                            s.send(&out).map_err(|e| e.to_string())?;
                        }
                        if args.after_login.contains("refresh") {
                            let mut misc = protos::message::Misc::new();
                            misc.set_refresh_video(true);
                            let mut out = Message::new();
                            out.set_misc(misc);
                            s.send(&out).map_err(|e| e.to_string())?;
                        }

                        let e = &p.encoding;
                        println!(
                            "   server encodings: h264 {} h265 {} vp8 {} av1 {}",
                            e.h264, e.h265, e.vp8, e.av1
                        );
                        println!("   resolutions offered: {}", p.resolutions.resolutions.len());
                    }
                    // Their oneofs are non-exhaustive to anything outside the
                    // crate that generated them, so an arm they add is one this
                    // ignores rather than one this fails to compile against.
                    _ => {}
                }
            }
            message::Union::VideoFrame(f) => {
                let display = f.display;
                if let Some(u) = f.union {
                    let (codec, frames) = match u {
                        video_frame::Union::Vp9s(v) => ("VP9", v.frames),
                        video_frame::Union::Vp8s(v) => ("VP8", v.frames),
                        video_frame::Union::H264s(v) => ("H264", v.frames),
                        video_frame::Union::H265s(v) => ("H265", v.frames),
                        video_frame::Union::Av1s(v) => ("AV1", v.frames),
                        video_frame::Union::Rgb(_) => ("RGB", Vec::new()),
                        video_frame::Union::Yuv(_) => ("YUV", Vec::new()),
                        _ => ("something new", Vec::new()),
                    };
                    if video.codec.is_empty() {
                        println!("== video: {codec} on display {display}");
                        video.codec = codec.into();
                    }
                    for fr in frames {
                        video.push(fr);
                    }
                }
            }
            message::Union::TestDelay(t) => {
                // Echoed back **unchanged**, which is the client's half of the
                // only timing this protocol has: the flag says who started it,
                // and a client that sets it has the far end echo the echo. Their
                // server does that unconditionally, so one flipped bit is a
                // busy loop between the two — measured at 9701 messages in
                // fifteen seconds before this was read the right way round.
                if !t.from_client {
                    let mut out = Message::new();
                    out.set_test_delay(t);
                    s.send(&out).map_err(|e| e.to_string())?;
                }
            }
            message::Union::MessageBox(b) => {
                println!("== message box: [{}] {} {}", b.msgtype, b.title, b.text);
                if b.msgtype.starts_with("re-input-password") || b.msgtype == "wrong-password" {
                    return Err("wrong password".into());
                }
            }
            message::Union::Misc(m) => match m.union {
                Some(protos::message::misc::Union::CloseReason(r)) => {
                    return Err(format!("closed by peer: {r}"));
                }
                Some(u) => println!("   Misc: {u:?}"),
                None => {}
            },
            _ => {}
        }
    }

    if !logged_in {
        return Err("no LoginResponse".into());
    }
    println!("== after {:.1}s:", started.elapsed().as_secs_f64());
    counts.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
    for (name, c) in &counts {
        println!("   {c:>5} {name}");
    }
    println!(
        "   {} bytes sent, {} received",
        s.sent_bytes(),
        s.received_bytes()
    );
    println!(
        "   {} video frames, {} of them key, {} bytes",
        video.frames.len(),
        video.keys,
        video.bytes
    );
    if let Some(path) = &args.out {
        video.write_ivf(path)?;
        println!("== wrote {path}");
    }
    Ok(())
}

// ---- what came back ---------------------------------------------------------

#[derive(Default)]
struct Video {
    codec: String,
    width: u16,
    height: u16,
    keys: usize,
    bytes: usize,
    frames: Vec<(Vec<u8>, i64)>,
}

impl Video {
    fn push(&mut self, f: EncodedVideoFrame) {
        if f.key {
            self.keys += 1;
        }
        self.bytes += f.data.len();
        self.frames.push((f.data.to_vec(), f.pts));
    }

    /// IVF, because the point of the file is to be fed to the phone's own
    /// decoder: it is the one container a MediaCodec harness can walk without a
    /// demuxer, and it keeps each frame's boundary, which is what the decoder
    /// wants a buffer of.
    fn write_ivf(&self, path: &str) -> Result<(), String> {
        let fourcc: &[u8; 4] = match self.codec.as_str() {
            "VP9" => b"VP90",
            "VP8" => b"VP80",
            "AV1" => b"AV01",
            other => return Err(format!("{other} is not an IVF codec; use --out with VP8/VP9/AV1")),
        };
        let mut f = std::fs::File::create(path).map_err(|e| e.to_string())?;
        let mut h = Vec::with_capacity(32);
        h.extend_from_slice(b"DKIF");
        h.extend_from_slice(&0u16.to_le_bytes());
        h.extend_from_slice(&32u16.to_le_bytes());
        h.extend_from_slice(fourcc);
        h.extend_from_slice(&self.width.to_le_bytes());
        h.extend_from_slice(&self.height.to_le_bytes());
        h.extend_from_slice(&1000u32.to_le_bytes()); // timebase numerator: ms
        h.extend_from_slice(&1u32.to_le_bytes());
        h.extend_from_slice(&(self.frames.len() as u32).to_le_bytes());
        h.extend_from_slice(&0u32.to_le_bytes());
        f.write_all(&h).map_err(|e| e.to_string())?;
        for (i, (data, pts)) in self.frames.iter().enumerate() {
            f.write_all(&(data.len() as u32).to_le_bytes())
                .map_err(|e| e.to_string())?;
            // Their pts is milliseconds since the session started, and the first
            // frame's is not zero; a decoder does not care and a player does.
            let pts = if *pts > 0 { *pts } else { i as i64 };
            f.write_all(&(pts as u64).to_le_bytes())
                .map_err(|e| e.to_string())?;
            f.write_all(data).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

fn arm(u: &message::Union) -> &'static str {
    match u {
        message::Union::SignedId(_) => "SignedId",
        message::Union::PublicKey(_) => "PublicKey",
        message::Union::TestDelay(_) => "TestDelay",
        message::Union::VideoFrame(_) => "VideoFrame",
        message::Union::LoginRequest(_) => "LoginRequest",
        message::Union::LoginResponse(_) => "LoginResponse",
        message::Union::Hash(_) => "Hash",
        message::Union::MouseEvent(_) => "MouseEvent",
        message::Union::AudioFrame(_) => "AudioFrame",
        message::Union::CursorData(_) => "CursorData",
        message::Union::CursorPosition(_) => "CursorPosition",
        message::Union::CursorId(_) => "CursorId",
        message::Union::KeyEvent(_) => "KeyEvent",
        message::Union::Clipboard(_) => "Clipboard",
        message::Union::FileAction(_) => "FileAction",
        message::Union::FileResponse(_) => "FileResponse",
        message::Union::Misc(_) => "Misc",
        message::Union::Cliprdr(_) => "Cliprdr",
        message::Union::MessageBox(_) => "MessageBox",
        message::Union::SwitchSidesResponse(_) => "SwitchSidesResponse",
        message::Union::VoiceCallRequest(_) => "VoiceCallRequest",
        message::Union::VoiceCallResponse(_) => "VoiceCallResponse",
        message::Union::PeerInfo(_) => "PeerInfo",
        message::Union::PointerDeviceEvent(_) => "PointerDeviceEvent",
        message::Union::Auth2fa(_) => "Auth2FA",
        message::Union::MultiClipboards(_) => "MultiClipboards",
        message::Union::ScreenshotRequest(_) => "ScreenshotRequest",
        message::Union::ScreenshotResponse(_) => "ScreenshotResponse",
        message::Union::TerminalAction(_) => "TerminalAction",
        message::Union::TerminalResponse(_) => "TerminalResponse",
        _ => "something new",
    }
}
