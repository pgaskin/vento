// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The session: one connection, a reader thread and a writer thread.

use crate::decode;
use crate::error::{Error, Result};
use crate::framebuffer::Framebuffer;
use crate::pixel::PixelFormat;
use crate::proto::{Counter, Reader, Writer, to_latin1};
use crate::security::{self, Credentials, Security};
use crate::transport::{Wire, Writes};
use crate::zlib::ZlibStream;

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

/// What a connection is told before it starts.
pub struct Config {
    /// `host`, `host:port`, `host:display` or `host::port`, the way every VNC
    /// client has taken one since the beginning.
    pub address: String,
    /// `None` means "ask", through [`Handler::credentials`].
    pub password: Option<String>,
    /// For the schemes that have one, which is VeNCrypt's `Plain` and nothing
    /// else this client speaks.
    pub user_name: Option<String>,
    /// Whether to insist on TLS, prefer it, or refuse it.
    pub security: Security,
    /// Leave other viewers connected.
    pub shared: bool,
    /// Preferred encoding, best first; the server picks the first it knows.
    pub encodings: Vec<i32>,
    /// zlib effort, 0–9, as the `compressLevel` pseudo-encodings.
    pub compress_level: Option<u8>,
    pub connect_timeout: Duration,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            address: String::new(),
            password: None,
            user_name: None,
            security: Security::Prefer,
            shared: true,
            encodings: vec![decode::ENC_ZRLE, decode::ENC_HEXTILE, decode::ENC_RRE],
            compress_level: None,
            // A mistyped address must not be a spinner that never stops.
            connect_timeout: Duration::from_secs(20),
        }
    }
}

/// Everything the session tells the app. Every method arrives on the protocol
/// thread and must be cheap; [`Handler::credentials`] and [`Handler::trust`] are
/// the exceptions and are allowed to take as long as a person does.
pub trait Handler {
    fn connected(&mut self, width: usize, height: usize);
    fn desktop_size(&mut self, width: usize, height: usize);
    fn damaged(&mut self, x: usize, y: usize, width: usize, height: usize);
    fn frame_end(&mut self);
    /// `pixels` is `width × height` in [`PixelFormat::NATIVE`], alpha from the
    /// cursor mask. A zero size means the server has hidden the pointer.
    fn cursor(&mut self, pixels: &[u32], width: usize, height: usize, hot_x: i32, hot_y: i32);
    /// The server has changed what a `PointerEvent`'s coordinates mean. In
    /// relative mode nobody here knows where the cursor is, so a client that
    /// draws one and follows it with a viewport has to stop doing both.
    fn pointer_mode(&mut self, relative: bool);
    fn bell(&mut self);
    fn clipboard(&mut self, text: &str);
    /// The server wants credentials and the config had none to give. `None`
    /// gives up on the connection.
    ///
    /// `needs_user` is false for VncAuth, which has no notion of a user name —
    /// a dialog that shows the field would be asking for something nobody can
    /// answer.
    fn credentials(&mut self, needs_user: bool) -> Option<Credentials>;
    /// The server's certificate, by fingerprint, before anything secret is
    /// sent. `false` ends the connection.
    fn trust(&mut self, fingerprint: &str) -> bool;
}

/// What the connection panel can be told, as strings, because that is what it
/// puts on screen.
#[derive(Clone, Default)]
pub struct Info {
    pub desktop_name: String,
    pub protocol: String,
    /// The transport: `TLSv1_3 · TLS13_AES_128_GCM_SHA256`, or the plain socket
    /// said plainly. Worth a row of its own only because there are two of
    /// them: with one transport it says nothing the address does not.
    pub connection: String,
    pub security: String,
    pub encoding: String,
    pub line_speed: String,
    pub server_pixels: String,
    pub viewer_pixels: String,
}

/// One screen of a desktop's layout. The rectangle is where the head is inside
/// the framebuffer; the id and the flags are the server's own and go back
/// unchanged in a `SetDesktopSize`, since a server matches the screens it is
/// sent against the ones it has.
#[derive(Clone, Copy, Default)]
pub struct Screen {
    pub id: u32,
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
    pub flags: u32,
}

/// One outbound message, already encoded. Encoding on the caller's thread and
/// writing on one thread of our own is what keeps a stalled socket from
/// reaching the main thread — the app calls [`Client::pointer`] from wherever
/// a finger moved.
enum Out {
    Bytes(Vec<u8>),
    Stop,
}

struct Shared {
    framebuffer: RwLock<Framebuffer>,
    out: Sender<Out>,
    socket: Mutex<Option<TcpStream>>,
    info: Mutex<Info>,
    /// Whether a framebuffer update has been asked for and not yet arrived.
    /// A `Mutex` rather than an atomic because "ask unless one is outstanding"
    /// is a test and a set, and two threads do it.
    requested: Mutex<bool>,
    running: AtomicBool,
    closing: AtomicBool,
    focused: AtomicBool,
    view_only: AtomicBool,
    /// The server acknowledged `ExtendedMouseButtons`, so the button mask may
    /// use its marker bit and its second byte.
    extended_buttons: AtomicBool,
    /// The server put the pointer in relative mode, so a `PointerEvent`'s
    /// coordinates are deltas biased by `0x7FFF` and this client no longer
    /// knows where the cursor is.
    relative_pointer: AtomicBool,
    /// The server has sent an `ExtendedDesktopSize` rectangle, which is the
    /// only announcement there is that it will take a `SetDesktopSize`.
    can_resize: AtomicBool,
    /// The layout the server last sent, in its own order. Empty until an
    /// `ExtendedDesktopSize` rectangle arrives, which for most servers is never.
    screens: Mutex<Vec<Screen>>,
    /// What the session has moved, both directions, since the socket was
    /// opened. Protocol bytes: inside TLS, outside TCP.
    received: Counter,
    sent: Counter,
    /// keyId → keysym, so a release names the key its press named. The
    /// protocol has no notion of a key id at all: `KeyEvent` carries a keysym,
    /// so remembering which one went down is the client's job, and doing it
    /// here rather than in Java keeps it under the protocol's own tests.
    held: Mutex<HashMap<i64, u32>>,
}

/// A connection. Created first, connected second: the framebuffer exists from
/// the start so the drawing thread has something to lock, and input sent before
/// the session is up is dropped rather than queued.
pub struct Client {
    shared: Arc<Shared>,
    rx: Mutex<Option<Receiver<Out>>>,
}

impl Client {
    pub fn new() -> Client {
        let (tx, rx) = channel();
        Client {
            shared: Arc::new(Shared {
                framebuffer: RwLock::new(Framebuffer::new()),
                out: tx,
                socket: Mutex::new(None),
                info: Mutex::new(Info::default()),
                requested: Mutex::new(false),
                running: AtomicBool::new(false),
                closing: AtomicBool::new(false),
                focused: AtomicBool::new(true),
                view_only: AtomicBool::new(false),
                extended_buttons: AtomicBool::new(false),
                relative_pointer: AtomicBool::new(false),
                can_resize: AtomicBool::new(false),
                screens: Mutex::new(Vec::new()),
                received: Counter::default(),
                sent: Counter::default(),
                held: Mutex::new(HashMap::new()),
            }),
            rx: Mutex::new(Some(rx)),
        }
    }

    /// The desktop, for the drawing thread. Held under a read lock for the
    /// length of a copy and nothing longer.
    pub fn framebuffer(&self) -> &RwLock<Framebuffer> {
        &self.shared.framebuffer
    }

    pub fn info(&self) -> Info {
        self.shared.info.lock().unwrap().clone()
    }

    /// Received and sent, in bytes, since the socket was opened.
    pub fn traffic(&self) -> (u64, u64) {
        (
            self.shared.received.load(Ordering::Relaxed),
            self.shared.sent.load(Ordering::Relaxed),
        )
    }

    pub fn is_running(&self) -> bool {
        self.shared.running.load(Ordering::Acquire)
    }

    // ---- input, callable from any thread ----------------------------------

    /// The pointer, where the server thinks in pixels.
    pub fn pointer(&self, x: u16, y: u16, button_mask: u16) {
        if self.shared.view_only.load(Ordering::Relaxed) {
            return;
        }
        self.send(pointer_event(
            x,
            y,
            button_mask,
            self.shared.extended_buttons.load(Ordering::Relaxed),
        ));
    }

    /// The pointer, where the server thinks in deltas — the same message with
    /// the coordinates biased, which is the whole of QEMU's relative mode.
    ///
    /// A delta outside the biased range is clamped rather than wrapped: a
    /// pointer that jumps to the far side of the desktop is wrong, but a
    /// pointer that jumps to the *opposite* side of it is wrong in a way that
    /// looks like a different bug.
    pub fn pointer_relative(&self, dx: i32, dy: i32, button_mask: u16) {
        if self.shared.view_only.load(Ordering::Relaxed) {
            return;
        }
        self.send(pointer_event(
            bias(dx),
            bias(dy),
            button_mask,
            self.shared.extended_buttons.load(Ordering::Relaxed),
        ));
    }

    /// Whether the server has put the pointer in relative mode. Asked as well
    /// as announced, because a screen that attaches to a session already
    /// running has missed the announcement.
    pub fn pointer_is_relative(&self) -> bool {
        self.shared.relative_pointer.load(Ordering::Relaxed)
    }

    /// Whether the ninth button has anywhere to go.
    pub fn has_extended_buttons(&self) -> bool {
        self.shared.extended_buttons.load(Ordering::Relaxed)
    }

    /// Whether this server will take a new desktop size.
    ///
    /// View-only says no, which is the same rule TigerVNC's viewer has: a
    /// session that promises to change nothing at the far end does not get to
    /// change the shape of it either.
    ///
    /// So does a desktop with more than one screen in it, and that one is this
    /// client's own limitation rather than the protocol's: what is sent is a
    /// layout of one screen covering the desktop, so granting a resize would
    /// merge somebody's monitors into one. Refusing is the smaller wrong
    /// answer, since nothing on a phone should quietly rearrange a workstation.
    pub fn can_resize(&self) -> bool {
        self.shared.can_resize.load(Ordering::Relaxed)
            && !self.shared.view_only.load(Ordering::Relaxed)
            && self.shared.screens.lock().unwrap().len() <= 1
    }

    /// How the far end's desktop is divided, in its own order. Empty until an
    /// `ExtendedDesktopSize` rectangle has arrived.
    pub fn screens(&self) -> Vec<Screen> {
        self.shared.screens.lock().unwrap().clone()
    }

    /// Ask the server for a desktop this size, as one screen covering it.
    ///
    /// The answer is another `ExtendedDesktopSize` rectangle, which is where
    /// the size that actually happened comes from — this sends and reports
    /// nothing.
    pub fn request_desktop_size(&self, width: u16, height: u16) {
        if !self.can_resize() || width == 0 || height == 0 {
            return;
        }
        let screen = self
            .shared
            .screens
            .lock()
            .unwrap()
            .first()
            .copied()
            .unwrap_or_default();
        self.send(set_desktop_size(width, height, screen));
    }

    pub fn key_down(&self, keysym: u32, key_id: i64) {
        if self.shared.view_only.load(Ordering::Relaxed) {
            return;
        }
        self.shared.held.lock().unwrap().insert(key_id, keysym);
        self.send(key_event(keysym, true));
    }

    pub fn key_up(&self, key_id: i64) {
        if self.shared.view_only.load(Ordering::Relaxed) {
            return;
        }
        // Unknown means it was never pressed, or was released twice: sending
        // nothing is right either way.
        if let Some(keysym) = self.shared.held.lock().unwrap().remove(&key_id) {
            self.send(key_event(keysym, false));
        }
    }

    /// Let go of everything, for a screen going away with keys still held:
    /// "held" means held at the far end, and it stays that way until told.
    pub fn release_all_keys(&self) {
        let held: Vec<u32> = self.shared.held.lock().unwrap().drain().map(|(_, k)| k).collect();
        for keysym in held {
            self.send(key_event(keysym, false));
        }
    }

    /// Not sent while view-only: the far end's clipboard is the far end's, and
    /// replacing it is driving rather than watching. Both borrowed clients
    /// already refuse it there.
    pub fn clipboard(&self, text: &str) {
        if self.shared.view_only.load(Ordering::Relaxed) {
            return;
        }
        let bytes = to_latin1(text);
        let mut msg = Vec::with_capacity(8 + bytes.len());
        msg.push(6u8);
        msg.extend_from_slice(&[0, 0, 0]);
        msg.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        msg.extend_from_slice(&bytes);
        self.send(msg);
    }

    pub fn set_view_only(&self, view_only: bool) {
        self.shared.view_only.store(view_only, Ordering::Relaxed);
    }

    /// Whether to keep asking for updates.
    ///
    /// The pause is entirely this: an RFB server sends nothing that was not
    /// asked for, so not asking is the whole mechanism — which is what the
    /// RealVNC core's `focusEvent` turned out to be doing inside itself.
    pub fn set_focused(&self, focused: bool) {
        self.shared.focused.store(focused, Ordering::Release);
        if focused {
            self.shared.request_update();
        }
    }

    /// End the session. The reader is blocked on a socket read, so it is woken
    /// by shutting the socket down rather than by a flag it cannot see.
    pub fn close(&self) {
        self.shared.closing.store(true, Ordering::Release);
        let _ = self.shared.out.send(Out::Stop);
        if let Some(socket) = self.shared.socket.lock().unwrap().as_ref() {
            let _ = socket.shutdown(std::net::Shutdown::Both);
        }
    }

    fn send(&self, bytes: Vec<u8>) {
        if self.shared.running.load(Ordering::Acquire) {
            let _ = self.shared.out.send(Out::Bytes(bytes));
        }
    }

    // ---- the session -------------------------------------------------------

    /// Connect and run until the session ends. Blocks; every [`Handler`] call
    /// happens on this thread.
    pub fn run(&self, config: &Config, handler: &mut dyn Handler) -> Result<()> {
        let result = self.session(config, handler);
        self.shared.running.store(false, Ordering::Release);
        let _ = self.shared.out.send(Out::Stop);
        if self.shared.closing.load(Ordering::Acquire) {
            return Err(Error::Closed);
        }
        result
    }

    fn session(&self, config: &Config, handler: &mut dyn Handler) -> Result<()> {
        let (host, port) = resolve(&config.address)?;
        let socket = connect(&host, port, config.connect_timeout)?;
        socket.set_nodelay(true).ok();
        socket.set_read_timeout(Some(config.connect_timeout)).ok();
        *self.shared.socket.lock().unwrap() = Some(socket.try_clone()?);

        let mut wire = Wire::plain(
            socket.try_clone()?,
            Arc::clone(&self.shared.received),
            Arc::clone(&self.shared.sent),
        )?;

        let version = security::handshake_version(&mut wire.r, &mut wire.w)?;
        let chosen = {
            // The stored password answers the first ask and no more: a wrong
            // one must not lock the session out of ever prompting.
            let mut ask = FromConfigThenHandler {
                handler: &mut *handler,
                stored: config.password.clone().filter(|p| !p.is_empty()).map(|password| {
                    Credentials {
                        user: config.user_name.clone().unwrap_or_default(),
                        password,
                    }
                }),
            };
            security::handshake_security(&mut wire, version, config.security, &host, &mut ask)?
        };

        // ClientInit, then ServerInit.
        wire.w.u8(config.shared as u8)?;
        wire.w.flush()?;

        let width = wire.r.u16()? as usize;
        let height = wire.r.u16()? as usize;
        check_desktop(width, height)?;
        let server_format = PixelFormat::read(&mut wire.r)?;
        let name = wire.r.string()?;

        {
            let mut info = self.shared.info.lock().unwrap();
            info.desktop_name = name;
            info.protocol = format!("RFB {}.{}", version.0, version.1);
            info.connection = match wire.peer() {
                Some(peer) => peer.description.clone(),
                // Said rather than left blank: "this desktop and every
                // keystroke are on the wire in the clear" is the fact a panel
                // most needs to be able to state.
                None => "Not encrypted".to_string(),
            };
            info.security = chosen.description;
            info.server_pixels = server_format.describe();
            info.viewer_pixels = PixelFormat::NATIVE.describe();
            info.line_speed = String::new();
            info.encoding = String::new();
        }

        // Everything downstream assumes NATIVE, so this is not a preference.
        wire.w.u8(0)?;
        wire.w.pad(3)?;
        wire.w.pixel_format(&PixelFormat::NATIVE)?;
        wire.w
            .write_all(&set_encodings(&config.encodings, config.compress_level))?;
        wire.w.flush()?;

        // The handshake is the only part with a deadline. Past it, an idle
        // desktop is silent for as long as it likes — and the deadline must not
        // cover the password prompt either, which is a person rather than a
        // server: a timeout has to know what it is waiting for, and one that
        // covers a prompt kills the connection under whoever is typing. Both
        // fall out of putting it on the socket and taking
        // it off here: a `Condvar` wait is not a read.
        socket.set_read_timeout(None).ok();

        self.shared.framebuffer.write().unwrap().resize(width, height);
        self.shared.running.store(true, Ordering::Release);
        handler.connected(width, height);

        // The writer thread takes the write half itself rather than a second
        // clone of the socket, because with TLS there is no second clone to
        // take: one session encrypts both directions (see `transport`).
        let mut r = wire.r;
        let writer = self.spawn_writer(wire.w);
        self.shared.request_update_full();

        let result = self.read_loop(&mut r, handler);
        let _ = self.shared.out.send(Out::Stop);
        let _ = socket.shutdown(std::net::Shutdown::Both);
        let _ = writer.join();
        result
    }

    /// Change the encodings on a running session.
    ///
    /// Live, and not by our contrivance: `SetEncodings` may be sent whenever a
    /// client likes and the server uses the new list from the next rectangle
    /// on. So the picture-quality control on this backend really does act on
    /// the session in front of you — which the RealVNC one cannot do, since
    /// its equivalent needs a *changed* quality and carries the group with it.
    ///
    /// A full update follows, because otherwise nothing repaints until
    /// something on the desktop moves and the change is invisible.
    pub fn set_encodings(&self, encodings: &[i32], compress_level: Option<u8>) {
        if !self.shared.running.load(Ordering::Acquire) {
            return;
        }
        let _ = self
            .shared
            .out
            .send(Out::Bytes(set_encodings(encodings, compress_level)));
        self.shared.request_update_full();
    }

    fn spawn_writer(&self, mut w: Writer<Writes>) -> std::thread::JoinHandle<()> {
        let rx = self.rx.lock().unwrap().take();
        std::thread::Builder::new()
            .name("rfb-write".into())
            .spawn(move || {
                let Some(rx) = rx else { return };
                while let Ok(msg) = rx.recv() {
                    match msg {
                        Out::Bytes(bytes) => {
                            if w.write_all(&bytes).is_err() || w.flush().is_err() {
                                break;
                            }
                        }
                        Out::Stop => break,
                    }
                }
            })
            .expect("spawning the RFB writer thread")
    }

    fn read_loop(&self, r: &mut Reader<impl Read>, handler: &mut dyn Handler) -> Result<()> {
        let mut zlib = ZlibStream::new();
        let mut scratch: Vec<u32> = Vec::new();
        let mut rate = Rate::new();

        loop {
            match r.u8()? {
                0 => {
                    let before = r.byte_count();
                    r.skip(1)?;
                    let count = r.u16()?;
                    self.framebuffer_update(r, handler, count, &mut zlib, &mut scratch)?;
                    if let Some(speed) = rate.sample(r.byte_count() - before) {
                        self.shared.info.lock().unwrap().line_speed = speed;
                    }
                    *self.shared.requested.lock().unwrap() = false;
                    if self.shared.focused.load(Ordering::Acquire) {
                        self.shared.request_update();
                    }
                    handler.frame_end();
                }
                1 => {
                    // SetColourMapEntries: only meaningful without true colour,
                    // and we asked for true colour.
                    r.skip(1)?;
                    let _first = r.u16()?;
                    let count = r.u16()? as usize;
                    r.skip(count * 6)?;
                }
                2 => handler.bell(),
                3 => {
                    r.skip(3)?;
                    let length = r.i32()?;
                    // `unsigned_abs`, because negating i32::MIN is the one
                    // value that overflows — in a release build silently, into
                    // a skip of two gigabytes that reads until the far end
                    // gives up.
                    let n = length.unsigned_abs() as usize;
                    if n > crate::proto::MAX_STRING {
                        return Err(Error::Protocol(format!("cut text of {n} bytes")));
                    }
                    if length < 0 {
                        // The extended clipboard's negative length is a
                        // capability exchange we did not ask for; skipping the
                        // body is how a client that does not speak it stays in
                        // sync with the stream.
                        r.skip(n)?;
                    } else {
                        let text = crate::proto::latin1(&r.bytes(n)?);
                        handler.clipboard(&text);
                    }
                }
                other => {
                    return Err(Error::Protocol(format!("server message type {other}")));
                }
            }
        }
    }

    fn framebuffer_update(
        &self,
        r: &mut Reader<impl Read>,
        handler: &mut dyn Handler,
        count: u16,
        zlib: &mut ZlibStream,
        scratch: &mut Vec<u32>,
    ) -> Result<()> {
        for _ in 0..count {
            let x = r.u16()? as usize;
            let y = r.u16()? as usize;
            let w = r.u16()? as usize;
            let h = r.u16()? as usize;
            let encoding = r.i32()?;

            // Every decoder below sizes its scratch from w and h, so this is
            // what keeps a rectangle's cost bounded by the desktop rather than
            // by two 16-bit fields a server chose: 65535×65535 raw is a 17 GB
            // allocation, and an allocation that fails aborts the process
            // instead of returning. A conforming server never sends one — it
            // clips to what was asked for — so the check is free.
            if !decode::is_pseudo(encoding) {
                let fb = self.shared.framebuffer.read().unwrap();
                if x + w > fb.width() || y + h > fb.height() {
                    return Err(Error::Protocol(format!(
                        "rectangle {w}x{h}+{x}+{y} outside the {}x{} desktop",
                        fb.width(),
                        fb.height()
                    )));
                }
            }

            match encoding {
                decode::ENC_LAST_RECT => return Ok(()),
                decode::ENC_DESKTOP_SIZE => {
                    check_desktop(w, h)?;
                    self.shared.framebuffer.write().unwrap().resize(w, h);
                    handler.desktop_size(w, h);
                    // Everything that was on it is gone, so ask again rather
                    // than leaving a screenful of the previous size's pixels.
                    self.shared.request_update_full();
                    continue;
                }
                decode::ENC_CURSOR => {
                    if decode::empty_cursor(w, h) {
                        handler.cursor(&[], 0, 0, 0, 0);
                    } else {
                        let pixels = decode::cursor(r, w, h)?;
                        // x and y are the hotspot, positively. The negated
                        // one RealVNC's API hands out is their own rewriting of
                        // the rectangle, not the protocol's.
                        handler.cursor(&pixels, w, h, x as i32, y as i32);
                    }
                    continue;
                }
                decode::ENC_EXTENDED_MOUSE_BUTTONS => {
                    // An empty rectangle whose whole content is that it arrived.
                    self.shared.extended_buttons.store(true, Ordering::Relaxed);
                    continue;
                }
                decode::ENC_POINTER_TYPE_CHANGE => {
                    // The flag is the rectangle's x; the width and height are
                    // the framebuffer's and mean nothing. Sent again whenever
                    // the far end's mouse changes shape underneath — a tablet
                    // plugged into the virtual machine — so this is a mode
                    // being set, not a capability being learnt.
                    let relative = x == 0;
                    if self.shared.relative_pointer.swap(relative, Ordering::Relaxed) != relative {
                        handler.pointer_mode(relative);
                    }
                    continue;
                }
                decode::ENC_EXTENDED_DESKTOP_SIZE => {
                    *self.shared.screens.lock().unwrap() = screen_layout(r)?;
                    // The rectangle arriving at all is the announcement, and it
                    // is the only one there is: it says the server speaks
                    // SetDesktopSize, not that it will grant one. A server
                    // configured to refuse sends exactly this and then says no.
                    self.shared.can_resize.store(true, Ordering::Relaxed);
                    if x == decode::RESIZE_REASON_CLIENT && y != decode::RESIZE_RESULT_SUCCESS {
                        // Nothing changed, the pixels included, so this must not
                        // ask for them again. The size in a refusal is the one
                        // the desktop still has rather than the one asked for;
                        // 1 is prohibited, 2 out of resources, 3 a bad layout.
                        log::warn!("the server refused a resize and stayed at {w}x{h} (result {y})");
                        continue;
                    }
                    let changed = {
                        let fb = self.shared.framebuffer.read().unwrap();
                        fb.width() != w || fb.height() != h
                    };
                    if changed {
                        check_desktop(w, h)?;
                        self.shared.framebuffer.write().unwrap().resize(w, h);
                        handler.desktop_size(w, h);
                        self.shared.request_update_full();
                    }
                    continue;
                }
                decode::ENC_COPY_RECT => {
                    let src_x = r.u16()? as usize;
                    let src_y = r.u16()? as usize;
                    self.shared
                        .framebuffer
                        .write()
                        .unwrap()
                        .copy(src_x, src_y, x, y, w, h);
                    self.shared.info.lock().unwrap().encoding =
                        decode::encoding_name(encoding).into();
                    handler.damaged(x, y, w, h);
                    continue;
                }
                decode::ENC_RAW => decode::raw(r, w, h, scratch)?,
                decode::ENC_RRE => decode::rre(r, w, h, scratch)?,
                decode::ENC_HEXTILE => decode::hextile(r, w, h, scratch)?,
                decode::ENC_ZRLE => decode::zrle(r, zlib, w, h, scratch)?,
                other => {
                    // Nothing can be skipped safely: an unknown encoding has an
                    // unknown length, so the stream is lost from here.
                    return Err(Error::Unsupported(format!("encoding {other}")));
                }
            }
            self.shared
                .framebuffer
                .write()
                .unwrap()
                .blit(x, y, w, h, scratch);
            self.shared.info.lock().unwrap().encoding = decode::encoding_name(encoding).into();
            handler.damaged(x, y, w, h);
        }
        Ok(())
    }
}

impl Shared {
    /// Ask for whatever has changed, unless a request is already outstanding.
    fn request_update(&self) {
        let mut requested = self.requested.lock().unwrap();
        if *requested || !self.running.load(Ordering::Acquire) {
            return;
        }
        *requested = true;
        let (w, h) = {
            let fb = self.framebuffer.read().unwrap();
            (fb.width(), fb.height())
        };
        let _ = self.out.send(Out::Bytes(update_request(true, w, h)));
    }

    /// Ask for all of it, outstanding request or not.
    fn request_update_full(&self) {
        let mut requested = self.requested.lock().unwrap();
        *requested = true;
        let (w, h) = {
            let fb = self.framebuffer.read().unwrap();
            (fb.width(), fb.height())
        };
        let _ = self.out.send(Out::Bytes(update_request(false, w, h)));
    }
}

/// The most pixels a server may say its desktop has. Not a judgement about what
/// a phone can draw — it is three times the largest desktop this client has been
/// measured against — but about what a pair of 16-bit fields may cost before the
/// claim is treated as a protocol error: the framebuffer is four bytes a pixel
/// and is allocated the moment the size arrives.
const MAX_PIXELS: usize = 32 << 20;

fn check_desktop(width: usize, height: usize) -> Result<()> {
    if width * height > MAX_PIXELS {
        return Err(Error::Protocol(format!("desktop of {width}x{height}")));
    }
    Ok(())
}

/// A `SetEncodings` message: the caller's preferences, then the floor, then the
/// pseudo-encodings.
fn set_encodings(preferred: &[i32], compress_level: Option<u8>) -> Vec<u8> {
    let mut list: Vec<i32> = Vec::new();
    for &e in preferred {
        if !list.contains(&e) && e != decode::ENC_RAW && e != decode::ENC_COPY_RECT {
            list.push(e);
        }
    }
    // CopyRect first and Raw last, always: Raw is the one encoding a server may
    // not refuse, so it is the floor rather than a preference, and CopyRect
    // costs four bytes for a window drag whatever else is in use.
    list.insert(0, decode::ENC_COPY_RECT);
    list.push(decode::ENC_RAW);
    // The pseudo-encodings are requests for behaviour, not for pixels. Cursor
    // is the load-bearing one: it is what lets the pointer be drawn here
    // instead of a round trip away, which is the assumption the whole control
    // stack rests on.
    list.push(decode::ENC_CURSOR);
    list.push(decode::ENC_DESKTOP_SIZE);
    // Both halves of "the desktop may change size". The plain one is a server
    // announcing that it has; the extended one is a conversation, and asking
    // for it is what makes this end able to ask for a size at all.
    list.push(decode::ENC_EXTENDED_DESKTOP_SIZE);
    list.push(decode::ENC_LAST_RECT);
    // Both of these are asked for unconditionally, and neither is a preference
    // about how this client would like to work: they are questions about the
    // server, whose answers arrive as rectangles. Asking about the pointer mode
    // is what makes a QEMU virtual machine with an ordinary PS/2 mouse usable
    // at all — a server in relative mode with no client that says −257 drops
    // every pointer event on the floor.
    list.push(decode::ENC_EXTENDED_MOUSE_BUTTONS);
    list.push(decode::ENC_POINTER_TYPE_CHANGE);
    if let Some(level) = compress_level {
        list.push(-256 + level.min(9) as i32);
    }

    let mut msg = Vec::with_capacity(4 + list.len() * 4);
    msg.push(2u8);
    msg.push(0);
    msg.extend_from_slice(&(list.len() as u16).to_be_bytes());
    for e in list {
        msg.extend_from_slice(&e.to_be_bytes());
    }
    msg
}

/// A `PointerEvent`, in one of its two shapes.
///
/// Without `ExtendedMouseButtons` the mask is one byte, buttons 1–8, and the
/// ninth is dropped where it can be seen rather than by an `as u8` somewhere.
/// With it, bit 7 becomes a *marker* saying a
/// second byte follows, so button 8 moves out of the first byte and the two are
/// reassembled at the far end as `(high << 7) | (low & 0x7f)`.
fn pointer_event(x: u16, y: u16, buttons: u16, extended: bool) -> Vec<u8> {
    let mut msg = Vec::with_capacity(7);
    msg.push(5u8);
    msg.push(if extended {
        (buttons & 0x7f) as u8 | 0x80
    } else {
        (buttons & 0xff) as u8
    });
    msg.extend_from_slice(&x.to_be_bytes());
    msg.extend_from_slice(&y.to_be_bytes());
    if extended {
        msg.push(((buttons >> 7) & 0xff) as u8);
    }
    msg
}

/// A `SetDesktopSize`: the size wanted, and a layout of one screen covering it.
///
/// The screen keeps the id and flags the server gave it, which is what makes
/// this a change to the desktop that is there rather than a proposal of a new
/// one — a server matches what it is sent against the screens it has.
fn set_desktop_size(width: u16, height: u16, screen: Screen) -> Vec<u8> {
    let mut msg = Vec::with_capacity(24);
    msg.push(251u8);
    msg.push(0);
    msg.extend_from_slice(&width.to_be_bytes());
    msg.extend_from_slice(&height.to_be_bytes());
    msg.push(1); // one screen
    msg.push(0);
    msg.extend_from_slice(&screen.id.to_be_bytes());
    msg.extend_from_slice(&0u16.to_be_bytes());
    msg.extend_from_slice(&0u16.to_be_bytes());
    msg.extend_from_slice(&width.to_be_bytes());
    msg.extend_from_slice(&height.to_be_bytes());
    msg.extend_from_slice(&screen.flags.to_be_bytes());
    msg
}

/// The screen list inside an `ExtendedDesktopSize` rectangle.
///
/// Every screen is read whatever the rectangle turns out to say — a refusal
/// carries a layout too — because the list is part of the rectangle and
/// stopping early would leave the next rectangle's header being read out of the
/// middle of this one.
fn screen_layout<R: Read>(r: &mut Reader<R>) -> Result<Vec<Screen>> {
    let count = r.u8()? as usize;
    r.skip(3)?;
    let mut screens = Vec::with_capacity(count);
    for _ in 0..count {
        screens.push(Screen {
            id: r.u32()?,
            x: r.u16()?,
            y: r.u16()?,
            width: r.u16()?,
            height: r.u16()?,
            flags: r.u32()?,
        });
    }
    Ok(screens)
}

/// A delta as the relative mode carries it: `0x7FFF` is no movement.
fn bias(delta: i32) -> u16 {
    (0x7fff + delta).clamp(0, 0xffff) as u16
}

fn update_request(incremental: bool, w: usize, h: usize) -> Vec<u8> {
    let mut msg = Vec::with_capacity(10);
    msg.push(3u8);
    msg.push(incremental as u8);
    msg.extend_from_slice(&0u16.to_be_bytes());
    msg.extend_from_slice(&0u16.to_be_bytes());
    msg.extend_from_slice(&(w as u16).to_be_bytes());
    msg.extend_from_slice(&(h as u16).to_be_bytes());
    msg
}

fn key_event(keysym: u32, down: bool) -> Vec<u8> {
    let mut msg = Vec::with_capacity(8);
    msg.push(4u8);
    msg.push(down as u8);
    msg.extend_from_slice(&[0, 0]);
    msg.extend_from_slice(&keysym.to_be_bytes());
    msg
}

/// The stored password first, then the person.
///
/// It goes through the prompt path rather than through the handshake's own
/// config so that "wrong stored password" and "no stored password" reach the
/// same place, and `take` is what makes it once: a stored password that the
/// server refuses must not lock the session out of ever asking for the real one.
struct FromConfigThenHandler<'a> {
    handler: &'a mut dyn Handler,
    stored: Option<Credentials>,
}

impl security::Ask for FromConfigThenHandler<'_> {
    fn credentials(&mut self, needs_user: bool) -> Option<Credentials> {
        // A stored password with no user name is no answer to a scheme that
        // wants one, so that case goes to the prompt with the name filled in
        // by the app rather than being sent half-blank.
        if let Some(stored) = self.stored.take()
            && (!needs_user || !stored.user.is_empty())
        {
            return Some(stored);
        }
        self.handler.credentials(needs_user)
    }

    fn trust(&mut self, fingerprint: &str) -> bool {
        self.handler.trust(fingerprint)
    }
}

/// A throughput estimate for the connection panel, smoothed so it reads as a
/// speed rather than as whatever the last rectangle happened to cost.
struct Rate {
    last: Instant,
    bytes_per_second: f64,
}

impl Rate {
    fn new() -> Rate {
        Rate {
            last: Instant::now(),
            bytes_per_second: 0.0,
        }
    }

    fn sample(&mut self, bytes: u64) -> Option<String> {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last).as_secs_f64();
        self.last = now;
        // An idle desktop produces one update every few seconds, and dividing
        // by that gap would report a trickle as the line's speed. Only samples
        // taken while something is actually arriving say anything about the
        // line.
        if elapsed <= 0.0 || elapsed > 1.0 || bytes < 1024 {
            return None;
        }
        let instant = bytes as f64 / elapsed;
        self.bytes_per_second = if self.bytes_per_second == 0.0 {
            instant
        } else {
            self.bytes_per_second * 0.7 + instant * 0.3
        };
        let kbit = self.bytes_per_second * 8.0 / 1000.0;
        Some(if kbit >= 1000.0 {
            format!("{:.1} Mbit/s", kbit / 1000.0)
        } else {
            format!("{kbit:.0} kbit/s")
        })
    }
}

/// `host`, `host:port`, `host:display` or `host::port`, plus the bracketed IPv6
/// form. The display convention is older than the port one and every VNC client
/// still takes it, so `10.0.0.5:1` has to mean 5901 rather than port 1.
fn resolve(address: &str) -> Result<(String, u16)> {
    common::address::split(address, 5900, common::address::Ports::Display).map_err(Error::Address)
}

fn connect(host: &str, port: u16, timeout: Duration) -> Result<TcpStream> {
    if host.is_empty() {
        return Err(Error::Protocol("no address".into()));
    }
    let mut last = None;
    for addr in (host, port).to_socket_addrs()? {
        match TcpStream::connect_timeout(&addr, timeout) {
            Ok(socket) => return Ok(socket),
            Err(e) => last = Some(e),
        }
    }
    Err(Error::Io(last.unwrap_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, format!("no address for {host}"))
    })))
}

impl Default for Client {
    fn default() -> Client {
        Client::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The forms are `common::address`'s; what is this client's own is the
    /// default port and the display rule being on.
    #[test]
    fn addresses() {
        assert_eq!(resolve("10.0.0.5").unwrap(), ("10.0.0.5".into(), 5900));
        assert_eq!(resolve("10.0.0.5:1").unwrap(), ("10.0.0.5".into(), 5901));
        assert_eq!(resolve("10.0.0.5::5901").unwrap(), ("10.0.0.5".into(), 5901));
        assert_eq!(resolve("[::1]").unwrap(), ("::1".into(), 5900));
        assert!(resolve("2001:db8::1").is_err());
    }

    /// A release names the key its press named, and an unmatched one sends
    /// nothing at all — which is what stops a stuck modifier at the far end.
    #[test]
    fn key_ids_map_to_the_keysym_that_went_down() {
        let client = Client::new();
        client.shared.running.store(true, Ordering::Release);
        client.key_down(0xffe1, 7);
        client.key_up(7);
        client.key_up(7);
        let mut sent = Vec::new();
        let rx = client.rx.lock().unwrap().take().unwrap();
        while let Ok(Out::Bytes(b)) = rx.try_recv() {
            sent.push(b);
        }
        assert_eq!(sent.len(), 2, "the second release matched nothing");
        assert_eq!(sent[0], key_event(0xffe1, true));
        assert_eq!(sent[1], key_event(0xffe1, false));
    }

    /// The ninth button, before and after the server agrees to carry it. Both
    /// shapes are checked byte for byte, because the difference between them is
    /// one bit that means "there is another byte" to one server and "button 8
    /// is down" to another — and getting it wrong desynchronises the stream
    /// rather than moving the pointer somewhere silly.
    #[test]
    fn extended_buttons_move_button_eight_into_the_second_byte() {
        // Buttons 1 (bit 0), 8 (bit 7) and 9 (bit 8).
        let buttons = 0b1_1000_0001u16;

        let plain = pointer_event(100, 200, buttons, false);
        assert_eq!(plain, vec![5, 0b1000_0001, 0, 100, 0, 200]);

        let extended = pointer_event(100, 200, buttons, true);
        assert_eq!(
            extended,
            vec![5, 0b1000_0001, 0, 100, 0, 200, 0b0000_0011],
            "the marker bit is set, button 8 is bit 0 of the extra byte and 9 is bit 1"
        );
        // What the far end reassembles is what we meant.
        let low = (extended[1] & 0x7f) as u16;
        let high = extended[6] as u16;
        assert_eq!((high << 7) | low, buttons);
    }

    /// The marker bit is set whenever the server agreed to it, even with no
    /// button down at all: a first byte of zero and a trailing byte would be
    /// read as a message with no extra byte, and the byte after it as the next
    /// message's type.
    #[test]
    fn the_marker_bit_does_not_depend_on_which_buttons_are_down() {
        assert_eq!(pointer_event(1, 2, 0, true), vec![5, 0x80, 0, 1, 0, 2, 0]);
        assert_eq!(pointer_event(1, 2, 0, false), vec![5, 0, 0, 1, 0, 2]);
    }

    /// Two heads side by side, and the reader left exactly at the end of the
    /// list: a screen short or a screen long desynchronises every rectangle
    /// after this one, which is a stream that dies somewhere else.
    #[test]
    fn a_screen_layout_is_read_whole() {
        let bytes: Vec<u8> = vec![
            2, 0, 0, 0, // two screens, then the padding
            0, 0, 0, 1, 0, 0, 0, 0, 7, 128, 4, 56, 0, 0, 0, 0, // 1920×1080 at the origin
            0, 0, 0, 2, 7, 128, 0, 0, 5, 0, 3, 32, 0, 0, 0, 0, // 1280×800 to its right
            0xaa, // the next rectangle's first byte, which must still be there
        ];
        let mut r = Reader::new(&bytes[..]);
        let screens = screen_layout(&mut r).unwrap();
        assert_eq!(screens.len(), 2);
        assert_eq!((screens[0].x, screens[0].y), (0, 0));
        assert_eq!((screens[0].width, screens[0].height), (1920, 1080));
        assert_eq!(screens[1].id, 2);
        assert_eq!((screens[1].x, screens[1].y), (1920, 0));
        assert_eq!((screens[1].width, screens[1].height), (1280, 800));
        assert_eq!(r.u8().unwrap(), 0xaa);
    }

    /// The whole of `SetDesktopSize`, byte for byte: it is written once and a
    /// server that reads a screen count where the padding should be closes the
    /// connection rather than complaining.
    #[test]
    fn set_desktop_size_is_one_screen_covering_the_desktop() {
        let screen = Screen {
            id: 0x0102_0304,
            flags: 0x0000_0001,
            ..Screen::default()
        };
        assert_eq!(
            set_desktop_size(1280, 800, screen),
            vec![
                251, 0, // SetDesktopSize
                5, 0, 3, 32, // 1280 × 800
                1, 0, // one screen
                1, 2, 3, 4, // the id the server gave it
                0, 0, 0, 0, // at the origin
                5, 0, 3, 32, // covering the whole desktop
                0, 0, 0, 1, // and the flags it gave it
            ]
        );
    }

    /// Nothing is sent until the server has said it takes one, because there is
    /// nothing to say it with: the screen layout comes from the server's own
    /// announcement.
    #[test]
    fn a_resize_is_not_asked_for_before_the_server_offers_it() {
        let client = Client::new();
        client.shared.running.store(true, Ordering::Release);
        client.request_desktop_size(1280, 800);
        client.shared.can_resize.store(true, Ordering::Relaxed);
        client.request_desktop_size(1280, 800);
        client.set_view_only(true);
        client.request_desktop_size(1024, 768);

        let rx = client.rx.lock().unwrap().take().unwrap();
        let mut sent = Vec::new();
        while let Ok(Out::Bytes(b)) = rx.try_recv() {
            sent.push(b);
        }
        assert_eq!(sent, vec![set_desktop_size(1280, 800, Screen::default())]);
    }

    /// `0x7FFF` is standing still, and a delta too big for the field stops at
    /// the end of it rather than wrapping to the other side of the desktop.
    #[test]
    fn relative_deltas_are_biased_and_clamped() {
        assert_eq!(bias(0), 0x7fff);
        assert_eq!(bias(1), 0x8000);
        assert_eq!(bias(-1), 0x7ffe);
        assert_eq!(bias(-0x7fff), 0);
        assert_eq!(bias(-100_000), 0);
        assert_eq!(bias(100_000), 0xffff);
    }

    /// A relative pointer event is an ordinary one with biased coordinates —
    /// same message type, same length, same button byte.
    #[test]
    fn relative_and_absolute_are_the_same_message() {
        let client = Client::new();
        client.shared.running.store(true, Ordering::Release);
        client.pointer_relative(-3, 4, 1);
        let rx = client.rx.lock().unwrap().take().unwrap();
        let Ok(Out::Bytes(sent)) = rx.try_recv() else {
            panic!("nothing was sent");
        };
        assert_eq!(sent, pointer_event(0x7ffc, 0x8003, 1, false));
        assert_eq!(sent.len(), 6);
    }
}
