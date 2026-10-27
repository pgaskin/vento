// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The session: one connection, a reader thread and a writer thread.

use crate::clipboard::{self, Message};
use crate::decode;
use crate::error::{Error, Result};
use crate::framebuffer::Framebuffer;
use crate::pixel::PixelFormat;
use crate::proto::{Counter, MAX_CUT_TEXT, Reader, Writer, latin1, to_latin1};
use crate::security::{self, Security};
use crate::transport::{Wire, Writes};
use crate::zlib::ZlibStream;
use runtime::rate::Rate;
use runtime::net::{Connection, Liveness, Network, Socket, Timeout};
use runtime::{Credentials, Handler, Hangup};

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

/// A connection's settings, given before it starts.
pub struct Config {
    /// `host`, `host:port`, `host:display` or `host::port`, as every VNC
    /// client has accepted since the beginning.
    pub address: String,
    /// `None` means "ask", through [`Handler::credentials`].
    pub password: Option<String>,
    /// For schemes that use one; of those this client supports, only
    /// VeNCrypt's `Plain`.
    pub user_name: Option<String>,
    /// Whether to require TLS, prefer it, or refuse it.
    pub security: Security,
    /// Leave other viewers connected.
    pub shared: bool,
    /// Preferred encodings, best first; the server picks the first it knows.
    pub encodings: Vec<i32>,
    /// zlib effort, 0–9, sent as the `compressLevel` pseudo-encodings.
    pub compress_level: Option<u8>,
    pub connect_timeout: Duration,
    /// What the connection is made through.
    pub network: Arc<dyn Network>,
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
            // A mistyped address must not leave the connection spinning
            // forever.
            connect_timeout: Duration::from_secs(20),
            network: Arc::new(runtime::net::Direct),
        }
    }
}

/// The connection panel's values, as strings, since that is how it shows
/// them.
#[derive(Clone, Default)]
pub struct Info {
    pub desktop_name: String,
    pub protocol: String,
    /// The transport: `TLSv1_3 · TLS13_AES_128_GCM_SHA256`, or a plain socket.
    /// Worth its own row only because there are two transports: with one, it
    /// would add nothing to the address.
    pub connection: String,
    pub security: String,
    pub encoding: String,
    pub line_speed: String,
    pub server_pixels: String,
    pub viewer_pixels: String,
}

/// One screen of a desktop's layout. The rectangle is the screen's position in
/// the framebuffer; the id and the flags are the server's and are sent back
/// unchanged in a `SetDesktopSize`, since a server matches the screens it
/// receives against its own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Screen {
    pub id: u32,
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
    pub flags: u32,
}

/// One outbound message, already encoded. Encoding on the caller's thread and
/// writing on a thread of our own keeps a stalled socket from blocking the
/// main thread, since the app calls [`Client::pointer`] from its input
/// handlers.
enum Out {
    Bytes(Vec<u8>),
    Stop,
}

struct Shared {
    framebuffer: RwLock<Framebuffer>,
    out: Sender<Out>,
    /// The socket, from when dialling starts; `close` breaks it.
    hangup: Hangup,
    info: Mutex<Info>,
    /// Whether a framebuffer update has been requested and not yet received.
    /// A `Mutex` rather than an atomic because "request unless one is
    /// outstanding" is a test-and-set, done from two threads.
    requested: Mutex<bool>,
    running: AtomicBool,
    focused: AtomicBool,
    view_only: AtomicBool,
    /// The server acknowledged `ExtendedMouseButtons`, so the button mask may
    /// use its marker bit and its second byte.
    extended_buttons: AtomicBool,
    /// The server put the pointer in relative mode, so a `PointerEvent`'s
    /// coordinates are deltas biased by `0x7FFF` and this client no longer
    /// knows the cursor's position.
    relative_pointer: AtomicBool,
    /// The server has sent an `ExtendedDesktopSize` rectangle, the only
    /// indication that it accepts a `SetDesktopSize`.
    can_resize: AtomicBool,
    /// The layout the server last sent, in its own order. Empty until an
    /// `ExtendedDesktopSize` rectangle arrives, which for most servers is never.
    screens: Mutex<Vec<Screen>>,
    /// Bytes transferred in each direction since the socket was opened.
    /// Protocol bytes: inside TLS, outside TCP.
    received: Counter,
    sent: Counter,
    /// keyId → keysym, so a release names the same key as its press. The
    /// protocol has no key ids (`KeyEvent` carries a keysym), so the client
    /// tracks which one went down, here rather than in Java so the protocol's
    /// tests cover it.
    held: Mutex<HashMap<i64, u32>>,
    /// The extended clipboard's state (see `clipboard.rs`), used by the reader
    /// when answering the server and by the thread offering a copy.
    clip: Mutex<Clip>,
}

/// The extended clipboard's state: the server's capabilities, and the text
/// this end has announced and not yet been asked for.
#[derive(Default)]
struct Clip {
    /// The server's caps, action bits included; 0 until it sends them, after
    /// which the plain messages are not used again.
    server: u32,
    /// The most text the server accepts unrequested; 0 means notify only.
    server_utf8_size: u32,
    /// The announced text, with LF line ends, until the server's clipboard
    /// changes. TigerVNC's `hasLocalClipboard`, plus the text.
    local: Option<String>,
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
                hangup: Hangup::new(),
                info: Mutex::new(Info::default()),
                requested: Mutex::new(false),
                running: AtomicBool::new(false),
                focused: AtomicBool::new(true),
                view_only: AtomicBool::new(false),
                extended_buttons: AtomicBool::new(false),
                relative_pointer: AtomicBool::new(false),
                can_resize: AtomicBool::new(false),
                screens: Mutex::new(Vec::new()),
                received: Counter::default(),
                sent: Counter::default(),
                held: Mutex::new(HashMap::new()),
                clip: Mutex::new(Clip::default()),
            }),
            rx: Mutex::new(Some(rx)),
        }
    }

    /// The desktop, for the drawing thread. Held under a read lock only for
    /// the duration of a copy.
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

    /// An absolute pointer event, in desktop pixels.
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

    /// A relative pointer event: the same message with the coordinates
    /// biased, which is all QEMU's relative mode is.
    ///
    /// A delta outside the biased range is clamped rather than wrapped: a
    /// pointer that jumps to the edge of the desktop is wrong, but one that
    /// jumps to the *opposite* edge is wrong in a way that looks like a
    /// different bug.
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

    /// Whether the server has put the pointer in relative mode. Queryable as
    /// well as announced, because a screen that attaches to a session already
    /// running has missed the announcement.
    pub fn pointer_is_relative(&self) -> bool {
        self.shared.relative_pointer.load(Ordering::Relaxed)
    }

    /// Whether the server accepts a ninth button.
    pub fn has_extended_buttons(&self) -> bool {
        self.shared.extended_buttons.load(Ordering::Relaxed)
    }

    /// Whether this server accepts a new desktop size.
    ///
    /// Not in view-only mode, the same rule as TigerVNC's viewer: a session
    /// that promises to change nothing at the far end must not change its
    /// shape either.
    ///
    /// Not for a desktop with more than one screen either, which is this
    /// client's limitation rather than the protocol's: the request is a layout
    /// of one screen covering the desktop, so granting a resize would merge
    /// the far end's monitors into one. Refusing is the lesser problem, since
    /// a phone should not silently rearrange a workstation.
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

    /// Asks the server for a desktop of this size, as one screen covering it.
    ///
    /// The server answers with another `ExtendedDesktopSize` rectangle, which
    /// reports the resulting size; this only sends.
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
        let screen = Screen {
            x: 0,
            y: 0,
            width,
            height,
            ..screen
        };
        self.send(set_desktop_size(width, height, &[screen]));
    }

    /// Whether the server accepts a layout of any number of screens now: the
    /// same rule as [`can_resize`](Client::can_resize) without the
    /// single-screen limit, since a layout places each screen.
    pub fn can_layout(&self) -> bool {
        self.shared.can_resize.load(Ordering::Relaxed) && !self.shared.view_only.load(Ordering::Relaxed)
    }

    /// Asks the server for a desktop of this size divided into `screens`, each
    /// moving or resizing the server's screen with that id, or a new one (see
    /// [`requested_layout`]); their flags are ignored and the server's kept.
    /// Nothing is sent for a layout that doesn't fit this desktop, or that
    /// matches the server's current one.
    pub fn request_layout(&self, width: u16, height: u16, screens: &[Screen]) {
        if !self.can_layout() || width == 0 || height == 0 {
            return;
        }
        let have = self.screens();
        let Some(layout) = requested_layout(&have, width, height, screens) else {
            let n = screens.len();
            log::warn!("not asking for {width}x{height} in {n} screens: that is not a valid layout");
            return;
        };
        let same_size = {
            let fb = self.shared.framebuffer.read().unwrap();
            fb.width() == width as usize && fb.height() == height as usize
        };
        if same_size && layout == have {
            return;
        }
        self.send(set_desktop_size(width, height, &layout));
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
        // An unknown id was never pressed or was already released: sending
        // nothing is right either way.
        if let Some(keysym) = self.shared.held.lock().unwrap().remove(&key_id) {
            self.send(key_event(keysym, false));
        }
    }

    /// Releases every held key, for a screen going away with keys still held:
    /// they stay held at the far end until released.
    pub fn release_all_keys(&self) {
        let held: Vec<u32> = self.shared.held.lock().unwrap().drain().map(|(_, k)| k).collect();
        for keysym in held {
            self.send(key_event(keysym, false));
        }
    }

    /// Not sent in view-only mode: replacing the far end's clipboard is
    /// control, not viewing. The libvncclient and TigerVNC backends refuse it
    /// there too.
    ///
    /// With the extended clipboard the text is announced and kept, and sent
    /// when the server requests it, or immediately if the server accepts that
    /// much unrequested, as TigerVNC's `announceClipboard` does. Without it,
    /// the text is sent as a plain `ClientCutText`: Latin-1, with LF line
    /// ends, as RFB specifies.
    pub fn clipboard(&self, text: &str) {
        if self.shared.view_only.load(Ordering::Relaxed) {
            return;
        }
        let text = clipboard::to_lf(text);
        let mut clip = self.shared.clip.lock().unwrap();
        let server = clip.server;
        if server & clipboard::CAPS == 0 {
            drop(clip);
            self.send(client_cut_text(&text));
            return;
        }
        let wire = clipboard::wire_text(&text);
        let msg = if server & clipboard::PROVIDE != 0 && wire.len() <= clip.server_utf8_size as usize {
            clipboard::provide(&wire)
        } else if server & clipboard::NOTIFY != 0 {
            clipboard::notify(true)
        } else {
            // A server that can't be notified of text can only be sent it,
            // which is what TigerVNC does then.
            local_text(server, &text)
        };
        clip.local = Some(text);
        drop(clip);
        self.send(msg);
    }

    pub fn set_view_only(&self, view_only: bool) {
        self.shared.view_only.store(view_only, Ordering::Relaxed);
    }

    /// Whether to keep requesting updates.
    ///
    /// This is the whole pause mechanism: an RFB server sends nothing
    /// unrequested, so not requesting pauses it, which is also what the
    /// RealVNC core's `focusEvent` does internally.
    pub fn set_focused(&self, focused: bool) {
        self.shared.focused.store(focused, Ordering::Release);
        if focused {
            self.shared.request_update();
        }
    }

    /// Ends the session. The reader is blocked on a socket read or still
    /// dialling, so it is woken by shutting the socket down rather than by a
    /// flag it can't see.
    pub fn close(&self) {
        self.shared.hangup.hang_up();
        let _ = self.shared.out.send(Out::Stop);
    }

    fn send(&self, bytes: Vec<u8>) {
        if self.shared.running.load(Ordering::Acquire) {
            let _ = self.shared.out.send(Out::Bytes(bytes));
        }
    }

    // ---- the session -------------------------------------------------------

    /// Connects and runs until the session ends. Blocks; every [`Handler`]
    /// call happens on this thread.
    pub fn run(&self, config: &Config, handler: &mut dyn Handler) -> Result<()> {
        let result = self.session(config, handler);
        self.shared.running.store(false, Ordering::Release);
        let _ = self.shared.out.send(Out::Stop);
        if self.shared.hangup.is_hung_up() {
            return Err(Error::Closed);
        }
        result
    }

    fn session(&self, config: &Config, handler: &mut dyn Handler) -> Result<()> {
        let (host, port) = resolve(&config.address)?;
        let Connection { socket, end, .. } =
            connect(&*config.network, &self.shared.hangup, &host, port, config.connect_timeout)?;
        // The end of the stream on a relayed connection is whatever ended
        // the connection behind it: the route says which, so that a reset
        // reads as one either way.
        self.connected(config, handler, &host, socket).map_err(|e| e.explained(&end))
    }

    fn connected(&self, config: &Config, handler: &mut dyn Handler, host: &str, socket: Socket) -> Result<()> {
        // Held for the rest of the session, including the handshake: if a
        // server accepts the connection and then sends nothing, only shutting
        // the socket down ends the read.
        let _holding = self.shared.hangup.hold(&socket)?;
        socket.set_nodelay(true).ok();
        socket.set_read_timeout(Some(config.connect_timeout)).ok();

        let mut wire = Wire::plain(
            socket.try_clone()?,
            Arc::clone(&self.shared.received),
            Arc::clone(&self.shared.sent),
        )?;

        let version = security::handshake_version(&mut wire.r, &mut wire.w)?;
        let chosen = {
            let mut ask = FromConfigThenHandler {
                handler: &mut *handler,
                stored: config
                    .password
                    .clone()
                    .filter(|p| !p.is_empty())
                    .map(|password| Credentials {
                        user: config.user_name.clone().unwrap_or_default(),
                        domain: String::new(),
                        password,
                    }),
            };
            security::handshake_security(&mut wire, version, config.security, host, &mut ask)?
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
                // Stated rather than left blank: that the desktop and every
                // keystroke cross the network unencrypted is the fact the
                // panel most needs to show.
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

        // The handshake is the only part with a deadline. After it, an idle
        // desktop may be silent indefinitely, and the deadline must not cover
        // the password prompt either, which waits for the user rather than the
        // server: a timeout covering a prompt drops the connection while the
        // user is typing. Setting the timeout on the socket and removing it
        // here handles both, since a `Condvar` wait is not a read.
        socket.set_read_timeout(None).ok();

        self.shared.framebuffer.write().unwrap().resize(width, height);
        self.shared.running.store(true, Ordering::Release);
        handler.connected(width, height);

        // The writer thread takes the write half itself rather than a second
        // clone of the socket, because with TLS there is none: one session
        // encrypts both directions (see `transport`).
        let mut r = wire.r;
        let writer = self.spawn_writer(wire.w);
        self.shared.request_update_full();

        let result = self.read_loop(&mut r, handler);
        let _ = self.shared.out.send(Out::Stop);
        let _ = socket.shutdown(std::net::Shutdown::Both);
        let _ = writer.join();
        result
    }

    /// Changes the encodings on a running session.
    ///
    /// `SetEncodings` may be sent at any time, and the server uses the new
    /// list from the next rectangle on, so this applies live.
    ///
    /// A full update follows, because otherwise nothing repaints until
    /// something on the desktop changes, and the change is invisible.
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
                    // and this client asks for true colour.
                    r.skip(1)?;
                    let _first = r.u16()?;
                    let count = r.u16()? as usize;
                    r.skip(count * 6)?;
                }
                2 => handler.bell(),
                3 => self.server_cut_text(r, handler)?,
                other => {
                    return Err(Error::Protocol(format!("server message type {other}")));
                }
            }
        }
    }

    /// `ServerCutText`, plain or extended.
    fn server_cut_text(&self, r: &mut Reader<impl Read>, handler: &mut dyn Handler) -> Result<()> {
        r.skip(3)?;
        let length = r.i32()?;
        // `unsigned_abs`, because negating i32::MIN overflows, silently in a
        // release build, into a two-gigabyte skip that reads until the far
        // end gives up.
        let n = length.unsigned_abs() as usize;
        if n > MAX_CUT_TEXT {
            // Skipped, not refused: ending the session over a large copy at
            // the far end lost the desktop for a clipboard. Nothing is
            // delivered, since a truncated text pasted here would look
            // complete.
            log::warn!("the far end's clipboard of {n} bytes is over the {MAX_CUT_TEXT} this client takes; skipped");
            r.skip(n)?;
            return Ok(());
        }
        let body = r.bytes(n)?;
        if length >= 0 {
            // The far end owns the clipboard now, so a request for ours would
            // be for text that is gone.
            self.shared.clip.lock().unwrap().local = None;
            handler.clipboard(&clipboard::to_lf(&latin1(&body)));
            return Ok(());
        }
        let message = clipboard::parse(&body, MAX_CUT_TEXT).map_err(Error::Protocol)?;
        let mut clip = self.shared.clip.lock().unwrap();
        match message {
            Message::Caps { flags, utf8_size } => {
                clip.server = flags;
                clip.server_utf8_size = utf8_size.unwrap_or(0);
                log::debug!(
                    "extended clipboard: the server takes {:#x}, text unasked up to {} bytes",
                    flags,
                    clip.server_utf8_size
                );
                let _ = self.shared.out.send(Out::Bytes(clipboard::caps()));
            }
            Message::Notify(formats) => {
                // The server's clipboard changed: request its text
                // immediately, as the TigerVNC backend does.
                if formats & clipboard::UTF8 != 0 {
                    clip.local = None;
                    if clip.server & clipboard::REQUEST != 0 {
                        let _ = self.shared.out.send(Out::Bytes(clipboard::request()));
                    }
                }
            }
            Message::Provide(text) => {
                drop(clip);
                if let Some(text) = text {
                    handler.clipboard(&text);
                }
            }
            Message::Request(formats) => {
                if formats & clipboard::UTF8 != 0
                    && let Some(local) = &clip.local
                    && !self.shared.view_only.load(Ordering::Relaxed)
                {
                    let _ = self.shared.out.send(Out::Bytes(local_text(clip.server, local)));
                }
            }
            Message::Peek => {
                if clip.server & clipboard::NOTIFY != 0 {
                    let _ = self.shared.out.send(Out::Bytes(clipboard::notify(clip.local.is_some())));
                }
            }
        }
        Ok(())
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

            // Every decoder below sizes its scratch from w and h, so this keeps
            // a rectangle's cost bounded by the desktop rather than by two
            // 16-bit fields from the server: 65535×65535 raw is a 17 GB
            // allocation, and a failed allocation aborts the process instead
            // of returning. A conforming server never sends one (it clips to
            // the requested area), so the check costs nothing. A cursor's
            // rectangle belongs to a pseudo-encoding and is not on the
            // desktop; `decode::cursor` limits it to the cursor bound instead.
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
                    // Everything on it is gone, so request it again rather
                    // than leaving the previous size's pixels on screen.
                    self.shared.request_update_full();
                    continue;
                }
                decode::ENC_CURSOR => {
                    if decode::empty_cursor(w, h) {
                        handler.cursor(&[], 0, 0, 0, 0);
                    } else {
                        let pixels = decode::cursor(r, w, h)?;
                        // x and y are the hotspot, positive. The negated one
                        // RealVNC's API returns is their own rewriting of the
                        // rectangle, not the protocol's.
                        handler.cursor(&pixels, w, h, x as i32, y as i32);
                    }
                    continue;
                }
                decode::ENC_EXTENDED_MOUSE_BUTTONS => {
                    // An empty rectangle; its arrival is the whole message.
                    self.shared.extended_buttons.store(true, Ordering::Relaxed);
                    continue;
                }
                decode::ENC_POINTER_TYPE_CHANGE => {
                    // The flag is the rectangle's x; the width and height are
                    // the framebuffer's and mean nothing. Sent again whenever
                    // the far end's pointer device changes (a tablet plugged
                    // into the virtual machine), so this sets a mode rather
                    // than reporting a capability.
                    let relative = x == 0;
                    if self.shared.relative_pointer.swap(relative, Ordering::Relaxed) != relative {
                        handler.pointer_mode(relative);
                    }
                    continue;
                }
                decode::ENC_EXTENDED_DESKTOP_SIZE => {
                    *self.shared.screens.lock().unwrap() = screen_layout(r)?;
                    // The rectangle's arrival is the only announcement there
                    // is: it says the server supports SetDesktopSize, not that
                    // it will grant one. A server configured to refuse sends
                    // exactly this and then refuses.
                    self.shared.can_resize.store(true, Ordering::Relaxed);
                    if x == decode::RESIZE_REASON_CLIENT && y != decode::RESIZE_RESULT_SUCCESS {
                        // Nothing changed, including the pixels, so this must
                        // not request them again. The size in a refusal is the
                        // desktop's current one rather than the requested one;
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
    /// Requests the changed areas, unless a request is already outstanding.
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

    /// Requests the whole desktop, whether or not a request is outstanding.
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

/// A desktop size within [`runtime::MAX_DESKTOP_PIXELS`]. Both are 16-bit
/// fields, so the product cannot overflow.
fn check_desktop(width: usize, height: usize) -> Result<()> {
    if width * height > runtime::MAX_DESKTOP_PIXELS {
        return Err(Error::Protocol(format!("desktop of {width}x{height}")));
    }
    Ok(())
}

/// A `SetEncodings` message: the caller's preferences, then the fallback, then
/// the pseudo-encodings.
fn set_encodings(preferred: &[i32], compress_level: Option<u8>) -> Vec<u8> {
    let mut list: Vec<i32> = Vec::new();
    for &e in preferred {
        if !list.contains(&e) && e != decode::ENC_RAW && e != decode::ENC_COPY_RECT {
            list.push(e);
        }
    }
    // CopyRect first and Raw last, always: Raw is the one encoding a server may
    // not refuse, so it is the fallback rather than a preference, and CopyRect
    // costs four bytes for a window drag whatever else is in use.
    list.insert(0, decode::ENC_COPY_RECT);
    list.push(decode::ENC_RAW);
    // The pseudo-encodings request behaviour, not pixels. Cursor matters most:
    // it lets the pointer be drawn locally instead of a round trip away, which
    // the whole control stack assumes.
    list.push(decode::ENC_CURSOR);
    list.push(decode::ENC_DESKTOP_SIZE);
    // Both halves of desktop resizing. With the plain one the server announces
    // a change; the extended one is a two-way exchange, and requesting it is
    // what lets this end request a size at all.
    list.push(decode::ENC_EXTENDED_DESKTOP_SIZE);
    list.push(decode::ENC_LAST_RECT);
    // Both of these are requested unconditionally, and neither is a preference
    // about how this client works: they ask about the server, whose answers
    // arrive as rectangles. Requesting the pointer mode is what makes a QEMU
    // virtual machine with an ordinary PS/2 mouse usable at all: a server in
    // relative mode drops every pointer event from a client that doesn't
    // request −257.
    list.push(decode::ENC_EXTENDED_MOUSE_BUTTONS);
    list.push(decode::ENC_POINTER_TYPE_CHANGE);
    // The clipboard in UTF-8 rather than Latin-1, if the server supports it;
    // the server answers with its caps, as a ServerCutText (see
    // `clipboard.rs`).
    list.push(clipboard::ENC_EXTENDED_CLIPBOARD);
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

/// A plain `ClientCutText`: Latin-1, `?` for the rest.
fn client_cut_text(text: &str) -> Vec<u8> {
    let bytes = to_latin1(text);
    let mut msg = Vec::with_capacity(8 + bytes.len());
    msg.push(6u8);
    msg.extend_from_slice(&[0, 0, 0]);
    msg.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    msg.extend_from_slice(&bytes);
    msg
}

/// Our text in the form a server with the extended clipboard accepts: a
/// provide if it supports that action, and otherwise the plain message, as
/// TigerVNC's `sendClipboardData` chooses.
fn local_text(server: u32, text: &str) -> Vec<u8> {
    if server & clipboard::PROVIDE != 0 {
        clipboard::provide(&clipboard::wire_text(text))
    } else {
        client_cut_text(text)
    }
}

/// A `PointerEvent`, in one of its two forms.
///
/// Without `ExtendedMouseButtons` the mask is one byte, buttons 1–8, and the
/// ninth is dropped explicitly here rather than by an `as u8` elsewhere. With
/// it, bit 7 becomes a *marker* saying a second byte follows, so button 8
/// moves out of the first byte and the far end reassembles the two as
/// `(high << 7) | (low & 0x7f)`.
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

/// A `SetDesktopSize`: the requested size and the screens it is divided into,
/// at most 255, as given.
///
/// Screens that keep the ids and flags the server gave them make this a change
/// to the existing desktop rather than a proposal of a new one, since a server
/// matches the screens it receives against its own.
fn set_desktop_size(width: u16, height: u16, screens: &[Screen]) -> Vec<u8> {
    let screens = &screens[..screens.len().min(255)];
    let mut msg = Vec::with_capacity(8 + 16 * screens.len());
    msg.push(251u8);
    msg.push(0);
    msg.extend_from_slice(&width.to_be_bytes());
    msg.extend_from_slice(&height.to_be_bytes());
    msg.push(screens.len() as u8);
    msg.push(0);
    for screen in screens {
        msg.extend_from_slice(&screen.id.to_be_bytes());
        msg.extend_from_slice(&screen.x.to_be_bytes());
        msg.extend_from_slice(&screen.y.to_be_bytes());
        msg.extend_from_slice(&screen.width.to_be_bytes());
        msg.extend_from_slice(&screen.height.to_be_bytes());
        msg.extend_from_slice(&screen.flags.to_be_bytes());
    }
    msg
}

/// A requested layout, as the screens to send given the server's own `have`:
/// a screen whose id the server has is that screen, moved or resized, and
/// keeps its flags; one with an id the server doesn't have, or an id already
/// used earlier in the list, is new and gets the lowest id neither list uses,
/// as TigerVNC's viewer gives its new screens a fresh one. `None` if the
/// result is not a valid layout of a `width × height` desktop, by the rules
/// TigerVNC's `ScreenSet::validate` checks before sending one: at least one
/// screen and at most 255, none empty or outside the desktop.
pub fn requested_layout(have: &[Screen], width: u16, height: u16, wanted: &[Screen]) -> Option<Vec<Screen>> {
    if wanted.is_empty() || wanted.len() > 255 {
        return None;
    }
    let mut kept: Vec<Option<&Screen>> = Vec::with_capacity(wanted.len());
    for (i, w) in wanted.iter().enumerate() {
        let repeat = wanted[..i].iter().zip(&kept).any(|(v, k)| k.is_some() && v.id == w.id);
        kept.push(if repeat { None } else { have.iter().find(|h| h.id == w.id) });
    }
    let mut next = 0u32;
    let mut layout = Vec::with_capacity(wanted.len());
    for (w, k) in wanted.iter().zip(&kept) {
        let fits = w.width > 0
            && w.height > 0
            && w.x as u32 + w.width as u32 <= width as u32
            && w.y as u32 + w.height as u32 <= height as u32;
        if !fits {
            return None;
        }
        let (id, flags) = match k {
            Some(h) => (h.id, h.flags),
            None => {
                let used = |id: u32| {
                    have.iter().any(|h| h.id == id)
                        || wanted.iter().zip(&kept).any(|(v, k)| k.is_some() && v.id == id)
                };
                while used(next) {
                    next += 1;
                }
                next += 1;
                (next - 1, 0)
            }
        };
        layout.push(Screen { id, flags, ..*w });
    }
    Some(layout)
}

/// The screen list inside an `ExtendedDesktopSize` rectangle.
///
/// Every screen is read regardless of what the rectangle reports (a refusal
/// carries a layout too), because the list is part of the rectangle, and
/// stopping early would read the next rectangle's header from the middle of
/// this one.
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

/// A delta as relative mode encodes it: `0x7FFF` is no movement.
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

/// The stored password first, then the user.
///
/// It goes through the prompt path rather than the handshake's config so a
/// wrong stored password and no stored password end up in the same place, and
/// `take` makes it used only once: a stored password the server refuses must
/// not stop the session from ever asking for the real one.
struct FromConfigThenHandler<'a> {
    handler: &'a mut dyn Handler,
    stored: Option<Credentials>,
}

impl security::Ask for FromConfigThenHandler<'_> {
    fn credentials(&mut self, needs_user: bool) -> Option<Credentials> {
        // A stored password without a user name doesn't answer a scheme that
        // needs one, so that case goes to the prompt with the name filled in by
        // the app rather than being sent half-blank.
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

    fn status_setter(&mut self) -> Option<runtime::status::StatusSetter> {
        self.handler.status_setter()
    }
}

/// `host`, `host:port`, `host:display` or `host::port`, plus the bracketed IPv6
/// form. The display convention is older than the port one and every VNC client
/// still accepts it, so `10.0.0.5:1` means 5901 rather than port 1.
fn resolve(address: &str) -> Result<(String, u16)> {
    runtime::address::split(address, 5900, runtime::address::Ports::Display).map_err(Error::Address)
}

/// Dials through `network`, which tries each address in turn, each with the
/// whole `timeout`, with `hangup` so a close also ends the dial.
fn connect(network: &dyn Network, hangup: &Hangup, host: &str, port: u16, timeout: Duration) -> Result<Connection> {
    if host.is_empty() {
        return Err(Error::Protocol("no address".into()));
    }
    match network.connect(host, port, Timeout::PerAddress(timeout), &Liveness::default(), hangup) {
        Ok(connection) => Ok(connection),
        Err(_) if hangup.is_hung_up() => Err(Error::Closed),
        Err(e) => Err(Error::Io(e)),
    }
}

/// The interface `runtime::Runner` runs: the inherent `run` and `close` are the
/// implementation, exposed here for a session that doesn't know which protocol
/// it has.
impl runtime::Client for Client {
    type Config = Config;
    type Error = Error;
    const NAME: &'static str = "rfb";

    fn run(&self, config: &Config, handler: &mut dyn Handler) -> Result<()> {
        self.run(config, handler)
    }

    fn close(&self) {
        self.close();
    }

    /// `run` reports a requested close as [`Error::Closed`], the one ending the
    /// screen requested, which needs no message.
    fn closed_on_request(error: &Error) -> bool {
        matches!(error, Error::Closed)
    }
}

impl Default for Client {
    fn default() -> Client {
        Client::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The forms are `runtime::address`'s; this client's own parts are the
    /// default port and the display rule being on.
    #[test]
    fn addresses() {
        assert_eq!(resolve("10.0.0.5").unwrap(), ("10.0.0.5".into(), 5900));
        assert_eq!(resolve("10.0.0.5:1").unwrap(), ("10.0.0.5".into(), 5901));
        assert_eq!(resolve("10.0.0.5::5901").unwrap(), ("10.0.0.5".into(), 5901));
        assert_eq!(resolve("[::1]").unwrap(), ("::1".into(), 5900));
        assert!(resolve("2001:db8::1").is_err());
    }

    /// A release names the same key as its press, and an unmatched one sends
    /// nothing, which prevents a stuck modifier at the far end.
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
    /// forms are checked byte for byte, because the difference is one bit that
    /// means "another byte follows" to one server and "button 8 is down" to
    /// another, and getting it wrong desynchronises the stream rather than
    /// misplacing the pointer.
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
        // The far end reassembles the original mask.
        let low = (extended[1] & 0x7f) as u16;
        let high = extended[6] as u16;
        assert_eq!((high << 7) | low, buttons);
    }

    /// The marker bit is set whenever the server agreed to it, even with no
    /// button down: a first byte of zero followed by a trailing byte would be
    /// read as a message without the extra byte, and the byte after it as the
    /// next message's type.
    #[test]
    fn the_marker_bit_does_not_depend_on_which_buttons_are_down() {
        assert_eq!(pointer_event(1, 2, 0, true), vec![5, 0x80, 0, 1, 0, 2, 0]);
        assert_eq!(pointer_event(1, 2, 0, false), vec![5, 0, 0, 1, 0, 2]);
    }

    /// Two screens side by side, and the reader left exactly at the end of the
    /// list: one screen too few or too many desynchronises every following
    /// rectangle, and the stream fails somewhere else.
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

    /// The whole `SetDesktopSize`, byte for byte: a server that reads a screen
    /// count where the padding should be closes the connection without an
    /// error message.
    #[test]
    fn set_desktop_size_is_one_screen_covering_the_desktop() {
        let screen = Screen {
            id: 0x0102_0304,
            flags: 0x0000_0001,
            width: 1280,
            height: 800,
            ..Screen::default()
        };
        assert_eq!(
            set_desktop_size(1280, 800, &[screen]),
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

    /// Nothing is sent until the server has announced support, because there
    /// is nothing to send it with: the screen layout comes from the server's
    /// own announcement.
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
        let whole = Screen {
            width: 1280,
            height: 800,
            ..Screen::default()
        };
        assert_eq!(sent, vec![set_desktop_size(1280, 800, &[whole])]);
    }

    /// Every screen of a layout, in the order given, after the one header.
    #[test]
    fn set_desktop_size_carries_every_screen() {
        let screens = [
            Screen {
                id: 1,
                x: 0,
                y: 0,
                width: 1024,
                height: 768,
                flags: 0,
            },
            Screen {
                id: 2,
                x: 1024,
                y: 0,
                width: 1280,
                height: 800,
                flags: 9,
            },
        ];
        let msg = set_desktop_size(2304, 800, &screens);
        assert_eq!(msg.len(), 8 + 2 * 16);
        assert_eq!(&msg[..8], &[251, 0, 9, 0, 3, 32, 2, 0]);
        assert_eq!(&msg[24..], &[0, 0, 0, 2, 4, 0, 0, 0, 5, 0, 3, 32, 0, 0, 0, 9]);
    }

    fn screen(id: u32, x: u16, width: u16, height: u16, flags: u32) -> Screen {
        Screen {
            id,
            x,
            y: 0,
            width,
            height,
            flags,
        }
    }

    /// The server's screens keep their ids and flags wherever they are put;
    /// anything else is new, with the lowest id free in both lists.
    #[test]
    fn a_layout_keeps_the_servers_screens_and_numbers_new_ones() {
        let have = [screen(1, 0, 1280, 800, 5), screen(2, 1280, 1280, 800, 0)];
        let wanted = [
            screen(2, 0, 1024, 768, 0),
            screen(1, 1024, 1280, 800, 0),
            screen(77, 2304, 800, 600, 3),
        ];
        let got = requested_layout(&have, 3104, 800, &wanted).unwrap();
        assert_eq!(
            got,
            [screen(2, 0, 1024, 768, 0), screen(1, 1024, 1280, 800, 5), screen(0, 2304, 800, 600, 0)]
        );
        // A repeat is a new screen too, and 0 is taken by then.
        let wanted = [screen(1, 0, 800, 600, 0), screen(1, 800, 800, 600, 0), screen(0, 1600, 800, 600, 0)];
        let got = requested_layout(&have, 2400, 600, &wanted).unwrap();
        assert_eq!(got.iter().map(|s| s.id).collect::<Vec<_>>(), [1, 0, 3]);
        assert_eq!(got[0].flags, 5);
    }

    #[test]
    fn a_layout_that_is_not_one_of_the_desktop_is_not_built() {
        let have = [screen(1, 0, 1280, 800, 0)];
        assert_eq!(requested_layout(&have, 1280, 800, &[]), None);
        // Past the right-hand edge, and empty.
        assert_eq!(requested_layout(&have, 2000, 800, &[screen(1, 1000, 1280, 800, 0)]), None);
        assert_eq!(requested_layout(&have, 1280, 800, &[screen(1, 0, 0, 800, 0)]), None);
        assert!(requested_layout(&have, 1280, 800, &[screen(1, 0, 1280, 800, 0)]).is_some());
    }

    /// Like a resize, a layout waits for the server's announcement and is not
    /// sent in view-only mode; when sent, it includes every screen.
    #[test]
    fn a_layout_is_asked_for_only_once_the_server_offers_one() {
        let client = Client::new();
        client.shared.running.store(true, Ordering::Release);
        let two = [screen(9, 0, 1280, 800, 0), screen(9, 1280, 1280, 800, 0)];
        client.request_layout(2560, 800, &two);
        client.shared.can_resize.store(true, Ordering::Relaxed);
        *client.shared.screens.lock().unwrap() =
            vec![screen(9, 0, 1920, 1200, 0), screen(4, 1920, 800, 600, 0)];
        // Two screens: not a resize, but a layout.
        assert!(!client.can_resize());
        assert!(client.can_layout());
        client.request_layout(2560, 800, &two);
        client.set_view_only(true);
        client.request_layout(1280, 800, &two[..1]);

        let rx = client.rx.lock().unwrap().take().unwrap();
        let mut sent = Vec::new();
        while let Ok(Out::Bytes(b)) = rx.try_recv() {
            sent.push(b);
        }
        let expected = [screen(9, 0, 1280, 800, 0), screen(0, 1280, 1280, 800, 0)];
        assert_eq!(sent, vec![set_desktop_size(2560, 800, &expected)]);
    }

    /// Records the handler calls, for the tests that run the read loop over
    /// bytes rather than a socket.
    #[derive(Default)]
    struct Recorder {
        clipboard: Vec<String>,
        bells: usize,
        cursors: Vec<(usize, usize)>,
    }

    impl Handler for Recorder {
        fn connected(&mut self, _: usize, _: usize) {}
        fn desktop_size(&mut self, _: usize, _: usize) {}
        fn damaged(&mut self, _: usize, _: usize, _: usize, _: usize) {}
        fn frame_end(&mut self) {}
        fn cursor(&mut self, pixels: &[u32], w: usize, h: usize, _: i32, _: i32) {
            assert_eq!(pixels.len(), w * h);
            self.cursors.push((w, h));
        }
        fn bell(&mut self) {
            self.bells += 1;
        }
        fn clipboard(&mut self, text: &str) {
            self.clipboard.push(text.to_string());
        }
        fn credentials(&mut self, _: bool) -> Option<Credentials> {
            None
        }
        fn trust(&mut self, _: &str) -> bool {
            false
        }
    }

    /// Runs the read loop over `bytes` until they run out, which is how
    /// every such test ends: the error is the end of the input.
    fn read_all(client: &Client, bytes: &[u8]) -> Recorder {
        let mut recorder = Recorder::default();
        let mut r = Reader::new(bytes);
        let end = client.read_loop(&mut r, &mut recorder);
        assert!(matches!(end, Err(Error::Io(_))), "the loop ran to the end of the input");
        recorder
    }

    /// A clipboard over the limit is skipped and the session continues: the
    /// bell after it still rings, and none of the text is delivered.
    #[test]
    fn an_oversized_cut_text_is_skipped_not_fatal() {
        let n = crate::proto::MAX_CUT_TEXT + 1;
        let mut bytes = vec![3, 0, 0, 0];
        bytes.extend_from_slice(&(n as u32).to_be_bytes());
        bytes.resize(bytes.len() + n, b'x');
        bytes.push(2);
        let seen = read_all(&Client::new(), &bytes);
        assert!(seen.clipboard.is_empty());
        assert_eq!(seen.bells, 1);
    }

    /// A `FramebufferUpdate` of one `Cursor` rectangle of `w` by `h`, with
    /// `body` after it where the pixels and the mask would be.
    fn cursor_update(w: u16, h: u16, body: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0, 0, 0, 1];
        bytes.extend_from_slice(&[0, 1, 0, 2]); // the hotspot
        bytes.extend_from_slice(&w.to_be_bytes());
        bytes.extend_from_slice(&h.to_be_bytes());
        bytes.extend_from_slice(&decode::ENC_CURSOR.to_be_bytes());
        bytes.extend_from_slice(body);
        bytes
    }

    /// A cursor up to the shared bound is a cursor, mask and all.
    #[test]
    fn a_cursor_as_big_as_any_is_drawn() {
        let side = runtime::MAX_CURSOR_SIDE;
        let body = vec![0xff; side * 4 + side.div_ceil(8)];
        let mut bytes = cursor_update(side as u16, 1, &body);
        bytes.push(2);
        let seen = read_all(&Client::new(), &bytes);
        assert_eq!(seen.cursors, vec![(side, 1)]);
        assert_eq!(seen.bells, 1, "and what follows it is read in step");
    }

    /// The `Cursor` pseudo-encoding has no desktop to be checked against, so
    /// its two 16-bit fields are limited to the shared cursor bound before
    /// anything is allocated from them: 65535x65535 requested 17 GB before a
    /// pixel had arrived. Past it the session ends, as TigerVNC's and
    /// libvncclient's do.
    #[test]
    fn a_cursor_bigger_than_any_cursor_ends_the_session() {
        for (w, h) in [(u16::MAX, u16::MAX), (runtime::MAX_CURSOR_SIDE as u16 + 1, 1)] {
            let bytes = cursor_update(w, h, &[0; 64]);
            let mut seen = Recorder::default();
            let end = Client::new().read_loop(&mut Reader::new(&bytes[..]), &mut seen);
            assert!(matches!(end, Err(Error::Protocol(_))), "{w}x{h}: {end:?}");
            assert!(seen.cursors.is_empty());
        }
    }

    /// What the client queued for the server, in order.
    fn sent(client: &Client) -> Vec<Vec<u8>> {
        let rx = client.rx.lock().unwrap();
        let mut sent = Vec::new();
        while let Ok(Out::Bytes(b)) = rx.as_ref().unwrap().try_recv() {
            sent.push(b);
        }
        sent
    }

    /// One of our extended messages as the server would send it: the same
    /// bytes under `ServerCutText`'s type.
    fn from_server(mut msg: Vec<u8>) -> Vec<u8> {
        msg[0] = 3;
        msg
    }

    /// An extended clipboard message from the server: flags and payload.
    fn extended(flags: u32, payload: &[u8]) -> Vec<u8> {
        let mut msg = vec![3, 0, 0, 0];
        msg.extend_from_slice(&(-((4 + payload.len()) as i32)).to_be_bytes());
        msg.extend_from_slice(&flags.to_be_bytes());
        msg.extend_from_slice(payload);
        msg
    }

    /// TigerVNC's sequence, against a server that accepts text by notify only
    /// (Xvnc's default): its caps are answered with ours, its notify with a
    /// request, and its provide delivered as UTF-8 with LF line ends. A copy
    /// here is announced, sent when requested, and reported on a peek; once
    /// the server's clipboard changes, a request for ours goes unanswered.
    #[test]
    fn the_extended_clipboard_takes_tigervncs_turns() {
        use clipboard::{CAPS, NOTIFY, PEEK, PROVIDE, REQUEST, UTF8};
        let client = Client::new();
        client.shared.running.store(true, Ordering::Release);
        let mut bytes = extended(UTF8 | CAPS | REQUEST | PEEK | NOTIFY | PROVIDE, &0u32.to_be_bytes());
        bytes.extend(extended(NOTIFY | UTF8, &[]));
        bytes.extend(from_server(clipboard::provide(&clipboard::wire_text("far caf\u{e9} \u{2014} x\r\nnext"))));
        let seen = read_all(&client, &bytes);
        assert_eq!(seen.clipboard, vec!["far caf\u{e9} \u{2014} x\nnext".to_string()]);
        assert_eq!(sent(&client), vec![clipboard::caps(), clipboard::request()]);

        let near = "near caf\u{e9} \u{2014} \u{1f600}";
        client.clipboard(near);
        assert_eq!(sent(&client), vec![clipboard::notify(true)]);
        let mut bytes = extended(REQUEST | UTF8, &[]);
        bytes.extend(extended(PEEK, &[]));
        read_all(&client, &bytes);
        assert_eq!(
            sent(&client),
            vec![clipboard::provide(&clipboard::wire_text(near)), clipboard::notify(true)]
        );

        let mut bytes = extended(NOTIFY | UTF8, &[]);
        bytes.extend(extended(REQUEST | UTF8, &[]));
        bytes.extend(extended(PEEK, &[]));
        read_all(&client, &bytes);
        assert_eq!(sent(&client), vec![clipboard::request(), clipboard::notify(false)]);
    }

    /// A server that accepts text unrequested is sent it immediately, up to
    /// the size it gave; past that it is only notified.
    #[test]
    fn text_goes_unasked_where_the_server_takes_it() {
        use clipboard::{CAPS, NOTIFY, PROVIDE, REQUEST, UTF8};
        let client = Client::new();
        client.shared.running.store(true, Ordering::Release);
        read_all(&client, &extended(UTF8 | CAPS | REQUEST | NOTIFY | PROVIDE, &16u32.to_be_bytes()));
        let _ = sent(&client);
        client.clipboard("short");
        client.clipboard("rather longer than sixteen");
        assert_eq!(
            sent(&client),
            vec![clipboard::provide(&clipboard::wire_text("short")), clipboard::notify(true)]
        );
    }

    /// No caps, no extension: the plain messages, Latin-1 both ways, `?`
    /// for what Latin-1 lacks, and LF line ends.
    #[test]
    fn without_caps_the_clipboard_is_latin_1() {
        let client = Client::new();
        client.shared.running.store(true, Ordering::Release);
        client.clipboard("caf\u{e9} \u{2014}\r\nx");
        assert_eq!(sent(&client), vec![vec![6, 0, 0, 0, 0, 0, 0, 8, b'c', b'a', b'f', 0xe9, b' ', b'?', b'\n', b'x']]);
        let seen = read_all(&client, &[3, 0, 0, 0, 0, 0, 0, 6, b'c', b'a', b'f', 0xe9, b'\r', b'\n']);
        assert_eq!(seen.clipboard, vec!["caf\u{e9}\n".to_string()]);
    }

    /// `0x7FFF` is no movement, and a delta too large for the field stops at
    /// its end rather than wrapping to the other side of the desktop.
    #[test]
    fn relative_deltas_are_biased_and_clamped() {
        assert_eq!(bias(0), 0x7fff);
        assert_eq!(bias(1), 0x8000);
        assert_eq!(bias(-1), 0x7ffe);
        assert_eq!(bias(-0x7fff), 0);
        assert_eq!(bias(-100_000), 0);
        assert_eq!(bias(100_000), 0xffff);
    }

    /// A relative pointer event is an ordinary one with biased coordinates:
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
