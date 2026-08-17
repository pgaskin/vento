//! One SPICE session: four channels, the state above them, and the handle the
//! other threads hold.
//!
//! **The session is one task on one current-thread runtime**, which is note 83
//! §5's decision and the only one in this repository that has an executor in it
//! at all. A session is four sockets that have to be read at once, which is the
//! shape where a thread each is the awkward answer; what it costs is no threads
//! — the reader of every channel is a task beside this one, and `/proc/self/task`
//! has the same entry in it either way.
//!
//! Two other threads reach a session and neither of them is in here: whichever
//! thread is drawing, which takes the framebuffer's read lock straight through
//! the JNI, and whichever thread has input, which leaves a command on a queue
//! and returns. Nothing above this line ever waits for a socket.

use crate::agent::{Agent, Event};
use crate::channel::{Channel, Ended, Kind, Message};
use crate::cursor::Cursors;
use crate::display::Display;
use crate::error::{Error, Result};
use crate::framebuffer::Framebuffer;
use crate::keymap::{self, Key};
use crate::tls;
use common::address::Ports;
use shakenfist_spice_protocol::constants::{
    MOUSE_MODE_CLIENT, MOUSE_MODE_SERVER, cursor_client, display_client, image_compression,
    inputs_client, main_client, main_server,
};
use shakenfist_spice_protocol::messages::{
    ChannelsList, DisplayInit, KeyEvent, MainInit, MouseButton, MouseMotion, MousePosition, Notify,
    Ping, SetAck,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use tokio::sync::mpsc;

/// What this client says it is on the wire, and the version of the protocol
/// those numbers name.
pub const VERSION: &str = "SPICE 2.2";

/// Where a server is when nobody said. QEMU makes somebody choose, so this is
/// what `spice://host` means elsewhere rather than a default anything here has.
pub const DEFAULT_PORT: u16 = 5900;

/// How a session was told to reach a server, and what it may say about it.
pub struct Config {
    /// `host:port`. A plain SPICE port and a TLS SPICE port are different
    /// ports rather than a negotiation, which is why the next field is a
    /// setting rather than something learnt.
    pub address: String,
    pub tls: bool,
    pub password: Option<String>,
    pub connect_timeout: Duration,
    /// What to ask the server to compress images with, or `None` for whatever
    /// it does by itself — which for QEMU is `auto_glz`.
    pub compression: Option<u8>,
    pub view_only: bool,
}

/// What the session tells the app. Every one of these arrives on the session's
/// own thread and none of them may block for long.
pub trait Handler {
    fn connected(&mut self, width: usize, height: usize);
    fn desktop_size(&mut self, width: usize, height: usize);
    fn damaged(&mut self, x: usize, y: usize, width: usize, height: usize);
    fn frame_end(&mut self);
    /// The cursor's pixels in the framebuffer's own word order, or empty for a
    /// cursor the guest has hidden.
    fn cursor(&mut self, pixels: &[u32], width: i32, height: i32, hot_x: i32, hot_y: i32);
    /// Which end owns the pointer, which SPICE says in the main channel's
    /// `INIT` and can change mid-session.
    fn pointer_mode(&mut self, relative: bool);
    /// The ticket was refused, or none was given and one is wanted. Blocks the
    /// session until somebody answers, which on this runtime blocks everything
    /// — and is what waiting for a person means: there is nothing else this
    /// session could usefully be doing.
    fn password(&mut self) -> Option<String>;
    /// The TLS certificate, by fingerprint, before the ticket goes out.
    fn trust(&mut self, fingerprint: &str) -> bool;
    /// What the guest's clipboard now holds, which arrives only where the guest
    /// is running the agent.
    fn clipboard(&mut self, text: &str);
}

/// Everything the connection panel can be told about a live session.
#[derive(Clone, Default)]
pub struct Info {
    pub desktop_name: String,
    pub protocol: String,
    pub connection: String,
    pub security: String,
    pub encoding: String,
    /// Which channels linked, which is a row nothing else here has: a SPICE
    /// session is several connections and what it can do depends on which of
    /// them came up.
    pub channels: String,
    /// Whether the guest is running `spice-vdagent`, which decides whether
    /// there is a clipboard, a resize or a client-mode pointer at all.
    pub agent: String,
    pub display: String,
}

/// What the input threads leave for the session.
enum Command {
    Pointer { x: i32, y: i32, buttons: i32 },
    PointerRelative { dx: i32, dy: i32, buttons: i32 },
    KeyDown { keysym: u32, id: i64 },
    KeyUp { id: i64 },
    ReleaseAllKeys,
    Focus(bool),
    Compression(u8),
    Clipboard(String),
    /// The layout this end is asking the guest for: `x, y, width, height` per
    /// head, which for the app is always one.
    Monitors(Vec<(i32, i32, u32, u32)>),
    Close,
}

pub struct Client {
    framebuffer: Arc<RwLock<Framebuffer>>,
    commands: mpsc::UnboundedSender<Command>,
    inbox: Mutex<Option<mpsc::UnboundedReceiver<Command>>>,
    info: Arc<Mutex<Info>>,
    received: Arc<std::sync::atomic::AtomicU64>,
    sent: Arc<std::sync::atomic::AtomicU64>,
    relative: Arc<AtomicBool>,
    view_only: Arc<AtomicBool>,
    monitors: Arc<Mutex<Vec<i32>>>,
    /// Whether the guest's agent is there and will take a size, which is a fact
    /// about this session rather than about the protocol — so the seam polls it
    /// and it can turn true seconds after the picture arrives.
    resizable: Arc<AtomicBool>,
    closed: AtomicBool,
}

impl Client {
    pub fn new() -> Client {
        let (commands, inbox) = mpsc::unbounded_channel();
        Client {
            framebuffer: Arc::new(RwLock::new(Framebuffer::new())),
            commands,
            inbox: Mutex::new(Some(inbox)),
            info: Arc::new(Mutex::new(Info::default())),
            received: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            sent: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            relative: Arc::new(AtomicBool::new(false)),
            view_only: Arc::new(AtomicBool::new(false)),
            monitors: Arc::new(Mutex::new(Vec::new())),
            resizable: Arc::new(AtomicBool::new(false)),
            closed: AtomicBool::new(false),
        }
    }

    pub fn framebuffer(&self) -> &Arc<RwLock<Framebuffer>> {
        &self.framebuffer
    }

    pub fn info(&self) -> Info {
        self.info.lock().unwrap().clone()
    }

    /// Received and sent, in protocol bytes over all four channels.
    pub fn traffic(&self) -> (u64, u64) {
        (
            self.received.load(Ordering::Relaxed),
            self.sent.load(Ordering::Relaxed),
        )
    }

    /// The far end's monitors as `x, y, width, height` each, or empty where it
    /// has not published a layout.
    pub fn monitors(&self) -> Vec<i32> {
        self.monitors.lock().unwrap().clone()
    }

    pub fn pointer_is_relative(&self) -> bool {
        self.relative.load(Ordering::Relaxed)
    }

    /// Whether the far end will take a desktop size right now, which for this
    /// protocol means a guest with the agent running in it.
    pub fn can_resize(&self) -> bool {
        self.resizable.load(Ordering::Relaxed)
    }

    /// Ask the guest for a desktop this size. A request: what answers it is a
    /// new size, a different one, or nothing.
    pub fn request_desktop_size(&self, width: u32, height: u32) {
        self.request_monitors(&[(0, 0, width, height)]);
    }

    /// The same request with more than one head in it, which is the shape the
    /// message actually has. Nothing in the app asks for this — it shows one
    /// picture — but it is how a second monitor is made to exist at all: a QXL
    /// device reports one connected head until a client says otherwise.
    pub fn request_monitors(&self, monitors: &[(i32, i32, u32, u32)]) {
        let _ = self.commands.send(Command::Monitors(monitors.to_vec()));
    }

    /// What this phone has on its clipboard, offered to the guest.
    pub fn set_clipboard(&self, text: String) {
        let _ = self.commands.send(Command::Clipboard(text));
    }

    pub fn set_view_only(&self, view_only: bool) {
        self.view_only.store(view_only, Ordering::Relaxed);
        if view_only {
            // Whatever is held at the far end is this end's doing, and the
            // switch that stops sending input must not leave a key down there.
            let _ = self.commands.send(Command::ReleaseAllKeys);
        }
    }

    pub fn pointer(&self, x: i32, y: i32, buttons: i32) {
        let _ = self.commands.send(Command::Pointer { x, y, buttons });
    }

    pub fn pointer_relative(&self, dx: i32, dy: i32, buttons: i32) {
        let _ = self
            .commands
            .send(Command::PointerRelative { dx, dy, buttons });
    }

    pub fn key_down(&self, keysym: u32, id: i64) {
        let _ = self.commands.send(Command::KeyDown { keysym, id });
    }

    pub fn key_up(&self, id: i64) {
        let _ = self.commands.send(Command::KeyUp { id });
    }

    pub fn release_all_keys(&self) {
        let _ = self.commands.send(Command::ReleaseAllKeys);
    }

    pub fn set_focused(&self, focused: bool) {
        let _ = self.commands.send(Command::Focus(focused));
    }

    /// The image compression the server is asked for, live: one message, and
    /// the server encodes the next image with it.
    pub fn set_compression(&self, compression: u8) {
        let _ = self.commands.send(Command::Compression(compression));
    }

    pub fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
        let _ = self.commands.send(Command::Close);
    }

    /// Connect, and then run the session until it ends. Blocks the calling
    /// thread, which is the session's own.
    pub fn run(&self, config: &Config, handler: &mut dyn Handler) -> Result<()> {
        self.view_only.store(config.view_only, Ordering::Relaxed);
        let inbox = self
            .inbox
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| Error::Protocol("this session has already run".into()))?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        runtime.block_on(self.session(config, handler, inbox))
    }

    async fn session(
        &self,
        config: &Config,
        handler: &mut dyn Handler,
        mut inbox: mpsc::UnboundedReceiver<Command>,
    ) -> Result<()> {
        // Every channel of a session dials the same place, and the host is also
        // the name the certificate is checked against, so this is read once
        // rather than per channel.
        let (host, port) = common::address::split(&config.address, DEFAULT_PORT, Ports::Plain)
            .map_err(Error::Protocol)?;
        let connector = match config.tls {
            true => Some(tls::Connector::new(&host)?),
            false => None,
        };

        // 64 messages of slack between the readers and this task. The ack
        // window is the real flow control and it is 20, so this is deep enough
        // that a queue is never the thing that stops a server.
        let (queue, mut messages) = mpsc::channel::<Message>(64);
        let (ends, mut ended) = mpsc::channel::<Ended>(4);

        let mut session = Session {
            client: self,
            server: (host, port),
            channels: HashMap::new(),
            display: Display::new(Arc::clone(&self.framebuffer), IMAGE_CACHE),
            cursors: Cursors::new(),
            agent: Agent::new(),
            keys: HashMap::new(),
            shift_held: false,
            buttons: 0,
            mouse_modes: 0,
            focused: true,
            connected: false,
            described: (0, 0, 0),
        };

        // The main channel first, and everything else with the id its INIT
        // hands out: that id is the whole of what ties four sockets into one
        // session.
        let main = session
            .connect(Kind::Main, config, connector.as_ref(), 0, handler, &queue, &ends)
            .await?;
        session.channels.insert(Kind::Main, main);

        let session_id = tokio::time::timeout(
            config.connect_timeout,
            session.handshake(&mut messages, handler),
        )
        .await
        .map_err(|_| Error::Protocol("the server never sent a session id".into()))??;

        for kind in [Kind::Inputs, Kind::Display, Kind::Cursor] {
            let channel = session
                .connect(kind, config, connector.as_ref(), session_id, handler, &queue, &ends)
                .await?;
            session.channels.insert(kind, channel);
        }

        // What the display channel is told before it says anything: how much of
        // the server's image cache this end promises to keep, and how big a GLZ
        // window it can look back through.
        let mut payload = Vec::new();
        DisplayInit {
            cache_id: 1,
            cache_size: IMAGE_CACHE as u64,
            glz_dict_id: 1,
            glz_dict_window: GLZ_WINDOW as u32,
        }
        .write(&mut payload)?;
        session.send(Kind::Display, display_client::INIT, &payload).await?;
        if let Some(compression) = config.compression {
            session.compression(compression).await?;
        }

        session.describe(config, connector.as_ref());
        {
            let mut info = self.info.lock().unwrap();
            info.channels = session
                .channels
                .keys()
                .map(|kind| kind.name())
                .collect::<Vec<_>>()
                .join(", ");
        }
        loop {
            tokio::select! {
                message = messages.recv() => {
                    let Some(message) = message else { return Ok(()) };
                    session.message(message, handler).await?;
                    // Drain whatever else has arrived before ending the frame:
                    // a screenful is a burst of draws and the drawing thread
                    // wants one repaint out of it rather than one each.
                    while let Ok(message) = messages.try_recv() {
                        session.message(message, handler).await?;
                    }
                    if session.display.take_dirty() {
                        session.describe_picture();
                        handler.frame_end();
                    }
                }
                command = inbox.recv() => {
                    match command {
                        None | Some(Command::Close) => return Err(Error::Closed),
                        Some(command) => session.command(command, handler).await?,
                    }
                }
                end = ended.recv() => {
                    if let Some(end) = end {
                        // Any channel ending ends the session: three of the
                        // four are the picture, the pointer and the keyboard,
                        // and a session missing one of those is not one.
                        log::info!("{} ended", end.kind.name());
                        return match end.error {
                            Some(e) => Err(e),
                            None => Err(Error::Server(String::new())),
                        };
                    }
                }
            }
        }
    }
}

impl Default for Client {
    fn default() -> Client {
        Client::new()
    }
}

/// What `DISPLAY_INIT` promises the server this end will keep, which is what
/// spice-gtk promises: the server counts against it and a client that keeps
/// less than it said is a client whose cache misses.
const IMAGE_CACHE: usize = 20 * 1024 * 1024;
/// And how far back a GLZ image may refer.
const GLZ_WINDOW: usize = 3 * 1024 * 1024;

/// The session's own state, which nothing outside this file touches.
struct Session<'a> {
    client: &'a Client,
    /// Where every channel of this session dials, read out of the address once.
    server: (String, u16),
    channels: HashMap<Kind, Channel>,
    display: Display,
    cursors: Cursors,
    /// The guest's own program, which is where the clipboard and the resize
    /// live. Its messages travel inside the main channel's.
    agent: Agent,
    /// What is held at the far end, by the seam's key id, so that a release
    /// names the key rather than the keysym — which is what makes auto-repeat
    /// and a chord work.
    keys: HashMap<i64, Key>,
    /// Whether this end has put Shift down to make a character, which must not
    /// be taken away from a Shift the person is holding.
    shift_held: bool,
    buttons: u16,
    mouse_modes: u32,
    focused: bool,
    connected: bool,
    /// What the panel was last told about the picture — the encodings seen, the
    /// live streams and the draws dropped — so that the rows are rebuilt when
    /// one of them changes rather than on every frame.
    described: (usize, usize, u64),
}

impl Session<'_> {
    #[allow(clippy::too_many_arguments)]
    async fn connect(
        &mut self,
        kind: Kind,
        config: &Config,
        connector: Option<&tls::Connector>,
        session_id: u32,
        handler: &mut dyn Handler,
        queue: &mpsc::Sender<Message>,
        ends: &mpsc::Sender<Ended>,
    ) -> Result<Channel> {
        let mut password = config.password.clone();
        // The certificate is asked about once, on the first channel: the other
        // three reach the same server moments later and would present it again.
        let mut asked = false;
        loop {
            // The borrow of `handler` the trust question needs ends with this
            // block, so that a refused ticket can ask the same handler for a
            // password on the next turn round the loop.
            let opened = {
                let mut ask = |fingerprint: &str| handler.trust(fingerprint);
                let trust: Option<&mut dyn FnMut(&str) -> bool> = if session_id == 0 && !asked {
                    asked = true;
                    Some(&mut ask)
                } else {
                    None
                };
                // No deadline around this call: the one inside it is around
                // each step of the connection and not around the question it
                // may stop to ask.
                Channel::open(
                    kind,
                    (&self.server.0, self.server.1),
                    session_id,
                    password.as_deref(),
                    connector,
                    trust,
                    queue.clone(),
                    ends.clone(),
                    (
                        Arc::clone(&self.client.received),
                        Arc::clone(&self.client.sent),
                    ),
                    config.connect_timeout,
                )
                .await
            };

            match opened {
                // A refused ticket is the one error with a person on the other
                // end of it. The key it was encrypted to is generated per
                // connection, so asking again means dialling again — there is
                // nothing on this socket left to retry with.
                Err(Error::Refused(_)) => {
                    log::info!("{}: the ticket was refused", kind.name());
                    match handler.password() {
                        Some(answer) => password = Some(answer),
                        None => return Err(Error::Cancelled),
                    }
                }
                other => return other,
            }
        }
    }

    /// The main channel up to the point where the rest of the session can be
    /// opened: its `INIT` carries the session id, and the channel list says
    /// what the server has.
    async fn handshake(
        &mut self,
        messages: &mut mpsc::Receiver<Message>,
        handler: &mut dyn Handler,
    ) -> Result<u32> {
        let mut session_id = 0;
        while let Some(message) = messages.recv().await {
            match (message.kind, message.message_type) {
                (Kind::Main, main_server::INIT) => {
                    self.acknowledge(&message).await?;
                    let init = MainInit::read(&message.body)?;
                    session_id = init.session_id;
                    self.mouse_modes = init.supported_mouse_modes;
                    log::info!(
                        "session {}: agent {}, mouse mode {} of {:#x}",
                        init.session_id,
                        if init.agent_connected != 0 { "connected" } else { "absent" },
                        init.current_mouse_mode,
                        init.supported_mouse_modes
                    );
                    self.mouse_mode(init.current_mouse_mode, handler).await?;
                    self.send(Kind::Main, main_client::ATTACH_CHANNELS, &[]).await?;
                    // A guest that was already running the agent when this
                    // session started, which is the ordinary case: the token
                    // count is in `INIT` and there is no `AGENT_CONNECTED` to
                    // wait for.
                    if init.agent_connected != 0 {
                        self.agent_up(init.agent_tokens).await?;
                    } else {
                        self.describe_agent();
                    }
                }
                (Kind::Main, main_server::CHANNELS_LIST) => {
                    self.acknowledge(&message).await?;
                    let list = ChannelsList::read(&message.body)?;
                    log::info!("{} channels offered", list.channels.len());
                    return Ok(session_id);
                }
                _ => self.message(message, handler).await?,
            }
        }
        Err(Error::Protocol("the main channel ended in the handshake".into()))
    }

    async fn message(&mut self, message: Message, handler: &mut dyn Handler) -> Result<()> {
        self.acknowledge(&message).await?;
        match message.message_type {
            main_server::SET_ACK => {
                let set_ack = SetAck::read(&message.body)?;
                if let Some(channel) = self.channels.get_mut(&message.kind) {
                    channel.ack_window = set_ack.window;
                }
                let mut payload = Vec::new();
                SetAck::write_ack_sync(set_ack.generation, &mut payload)?;
                self.send(message.kind, ack_sync_of(message.kind), &payload).await?;
                return Ok(());
            }
            main_server::PING => {
                // Their ping carries filler so that a client can measure a link
                // with it, which on an idle session is most of the traffic.
                let ping = Ping::read(&message.body)?;
                let mut payload = Vec::new();
                ping.write_pong(&mut payload)?;
                self.send(message.kind, pong_of(message.kind), &payload).await?;
                return Ok(());
            }
            main_server::NOTIFY => {
                let notify = Notify::read(&message.body)?;
                log::info!("{}: {}", message.kind.name(), notify.message);
                return Ok(());
            }
            _ => {}
        }

        match message.kind {
            Kind::Main => self.main(message, handler).await?,
            Kind::Display => {
                self.display
                    .message(message.message_type, &message.body, handler)
                    .await?;
                if self.display.ready() && !self.connected {
                    self.connected = true;
                    let framebuffer = self.client.framebuffer.read().unwrap();
                    let (width, height) = (framebuffer.width(), framebuffer.height());
                    drop(framebuffer);
                    handler.connected(width, height);
                }
                *self.client.monitors.lock().unwrap() = self
                    .display
                    .monitors()
                    .iter()
                    .flat_map(|(x, y, w, h)| [*x, *y, *w, *h])
                    .collect();
            }
            Kind::Cursor => self.cursors.message(message.message_type, &message.body, handler)?,
            Kind::Inputs => {}
        }
        Ok(())
    }

    async fn main(&mut self, message: Message, handler: &mut dyn Handler) -> Result<()> {
        match message.message_type {
            main_server::MOUSE_MODE => {
                // Two `u32`s: what the server supports and what it is doing.
                if message.body.len() >= 8 {
                    self.mouse_modes = u32::from_le_bytes(message.body[0..4].try_into().unwrap());
                    let current = u32::from_le_bytes(message.body[4..8].try_into().unwrap());
                    self.mouse_mode(current, handler).await?;
                }
            }
            main_server::NAME => {
                // The one place a SPICE server says what the machine is called,
                // and it is optional: a QEMU without `-name` never sends it.
                let name = String::from_utf8_lossy(
                    message.body.get(4..).unwrap_or_default().split(|b| *b == 0).next().unwrap_or_default(),
                )
                .to_string();
                if !name.is_empty() {
                    self.client.info.lock().unwrap().desktop_name = name;
                }
            }
            main_server::AGENT_CONNECTED | main_server::AGENT_CONNECTED_TOKENS => {
                // The tokens are in the message only for a client that asked to
                // be told; without them this end has none and waits to be
                // granted some, which the server does as soon as it has any.
                let tokens = match message.body.len() >= 4 {
                    true => u32::from_le_bytes(message.body[0..4].try_into().unwrap()),
                    false => 0,
                };
                self.agent_up(tokens).await?;
            }
            main_server::AGENT_DISCONNECTED => {
                // A guest logging out or the program being killed. What it can
                // do goes with it, so the panel and `canResize` both change.
                self.agent.disconnected();
                self.client.resizable.store(false, Ordering::Relaxed);
                self.client.info.lock().unwrap().agent = "Not running".into();
            }
            main_server::AGENT_TOKEN => {
                if message.body.len() >= 4 {
                    self.agent
                        .add_tokens(u32::from_le_bytes(message.body[0..4].try_into().unwrap()));
                }
                self.flush_agent().await?;
            }
            main_server::AGENT_DATA => {
                let events = self.agent.receive(&message.body);
                for event in events {
                    match event {
                        Event::Clipboard(text) => handler.clipboard(&text),
                        Event::Capabilities => {
                            self.client
                                .resizable
                                .store(self.agent.can_resize(), Ordering::Relaxed);
                            self.describe_agent();
                        }
                    }
                }
                self.flush_agent().await?;
            }
            main_server::DISCONNECTING => return Err(Error::Server(String::new())),
            _ => {}
        }
        Ok(())
    }

    /// The agent came up — with the session, or minutes later when somebody
    /// logged into the guest.
    ///
    /// `AGENT_START` is what opens the channel to it, and its one field is how
    /// many chunks the *guest* may send this end before waiting. It is
    /// everything, which is what spice-gtk does: metering a clipboard would
    /// cost a token message per chunk of it and buy nothing on a link that is
    /// already flow-controlled by the ack window.
    async fn agent_up(&mut self, tokens: u32) -> Result<()> {
        log::info!("agent: connected, with {tokens} token(s) to send on");
        self.agent.connected(tokens);
        self.describe_agent();
        self.send(Kind::Main, main_client::AGENT_START, &u32::MAX.to_le_bytes())
            .await?;
        self.flush_agent().await
    }

    /// Whatever the agent has to say, as far as the tokens go. What is left
    /// stays queued until the server grants more.
    async fn flush_agent(&mut self) -> Result<()> {
        while let Some(piece) = self.agent.next_piece() {
            self.send(Kind::Main, main_client::AGENT_DATA, &piece).await?;
        }
        Ok(())
    }

    /// The panel's row for it, which is the difference between a session with a
    /// clipboard and one without — and the place somebody finds out why Paste
    /// does nothing.
    fn describe_agent(&self) {
        let mut what = Vec::new();
        if self.agent.has_clipboard() {
            what.push("clipboard");
        }
        if self.agent.can_resize() {
            what.push("resize");
        }
        let mut info = self.client.info.lock().unwrap();
        info.agent = match (self.agent.is_connected(), what.is_empty()) {
            (false, _) => "Not running".into(),
            (true, true) => "Connected".into(),
            (true, false) => format!("Connected · {}", what.join(", ")),
        };
    }

    /// Which end owns the pointer, and the request that decides it where the
    /// server will take one.
    ///
    /// Client mode is the absolute pointer every other backend here has, and it
    /// is what this asks for. Server mode is the far end owning the cursor, and
    /// a guest with no tablet attached offers nothing else — so the seam is
    /// told, and the app switches to the relative pointer it has had since the
    /// stage that built it for QEMU's *other* protocol.
    async fn mouse_mode(&mut self, current: u32, handler: &mut dyn Handler) -> Result<()> {
        if current == MOUSE_MODE_SERVER && self.mouse_modes & MOUSE_MODE_CLIENT != 0 {
            self.send(
                Kind::Main,
                main_client::MOUSE_MODE_REQUEST,
                &MOUSE_MODE_CLIENT.to_le_bytes(),
            )
            .await?;
            // The server does not announce the change back to the client that
            // asked for it, so what this end believes is what it asked for.
            self.client.relative.store(false, Ordering::Relaxed);
            handler.pointer_mode(false);
            return Ok(());
        }
        let relative = current != MOUSE_MODE_CLIENT;
        if relative != self.client.relative.swap(relative, Ordering::Relaxed) {
            log::info!("the pointer is {}", if relative { "the guest's" } else { "ours" });
            handler.pointer_mode(relative);
        }
        Ok(())
    }

    async fn command(&mut self, command: Command, handler: &mut dyn Handler) -> Result<()> {
        let view_only = self.client.view_only.load(Ordering::Relaxed);
        match command {
            Command::Pointer { x, y, buttons } if !view_only => {
                let mut payload = Vec::new();
                MousePosition {
                    x: x.max(0) as u32,
                    y: y.max(0) as u32,
                    buttons: buttons_of(buttons),
                    display_id: 0,
                }
                .write(&mut payload)?;
                self.send(Kind::Inputs, inputs_client::MOUSE_POSITION, &payload).await?;
                self.mouse_buttons(buttons).await?;
            }
            Command::PointerRelative { dx, dy, buttons } if !view_only => {
                let mut payload = Vec::new();
                MouseMotion {
                    dx,
                    dy,
                    buttons: buttons_of(buttons),
                }
                .write(&mut payload)?;
                self.send(Kind::Inputs, inputs_client::MOUSE_MOTION, &payload).await?;
                self.mouse_buttons(buttons).await?;
            }
            Command::KeyDown { keysym, id } if !view_only => self.key_down(keysym, id).await?,
            Command::KeyUp { id } if !view_only => self.key_up(id).await?,
            Command::ReleaseAllKeys => self.release_all_keys().await?,
            Command::Focus(focused) => self.focus(focused).await?,
            Command::Compression(compression) => self.compression(compression).await?,
            // The far end's clipboard is the far end's, and replacing it is
            // driving rather than watching — which is the answer every client
            // here gives.
            Command::Clipboard(text) if !view_only => {
                self.agent.offer_clipboard(text);
                self.flush_agent().await?;
            }
            // A resize is not gated the same way: it is the size of the picture
            // being looked at rather than something done inside the guest, and
            // no other backend here gates it either.
            Command::Monitors(monitors) => {
                log::info!("agent: asking the guest for {monitors:?}");
                self.agent.request_monitors(&monitors);
                self.flush_agent().await?;
            }
            Command::Close => return Err(Error::Closed),
            // View only, and the seam's own switch has already stopped most of
            // this: what gets here is a pointer that was in flight.
            _ => {
                let _ = handler;
            }
        }
        Ok(())
    }

    /// Presses and releases for whatever changed in the button mask, since
    /// SPICE has a message per transition where the seam has a state.
    async fn mouse_buttons(&mut self, mask: i32) -> Result<()> {
        let wanted = buttons_of(mask);
        for bit in 0..5u16 {
            let button = 1u16 << bit;
            let was = self.buttons & button != 0;
            let is = wanted & button != 0;
            if was == is {
                continue;
            }
            self.buttons = if is {
                self.buttons | button
            } else {
                self.buttons & !button
            };
            let mut payload = Vec::new();
            MouseButton {
                button: button as u8,
                buttons_state: self.buttons,
            }
            .write(&mut payload)?;
            let message = if is {
                inputs_client::MOUSE_PRESS
            } else {
                inputs_client::MOUSE_RELEASE
            };
            self.send(Kind::Inputs, message, &payload).await?;
        }
        Ok(())
    }

    async fn key_down(&mut self, keysym: u32, id: i64) -> Result<()> {
        let Some(key) = keymap::scancode(keysym) else {
            // There is no Unicode key event in this protocol, so a character
            // with no place on a US keyboard is not sent rather than sent
            // wrongly.
            log::debug!("no scancode for keysym {keysym:#x}");
            return Ok(());
        };
        // Auto-repeat is a second press of a key already down, which is what
        // the far end wants and what must not put a second Shift under it.
        let repeat = self.keys.insert(id, key).is_some();
        if let Key::Scancode { shift: true, .. } = key
            && !self.shift_held
            && !repeat
        {
            self.shift_held = true;
            self.scancode(SHIFT.press()).await?;
        }
        self.scancode(key.press()).await
    }

    async fn key_up(&mut self, id: i64) -> Result<()> {
        let Some(key) = self.keys.remove(&id) else {
            return Ok(());
        };
        if let Some(code) = key.release() {
            self.scancode_up(code).await?;
        }
        if let Key::Scancode { shift: true, .. } = key
            && self.shift_held
            && !self.keys.values().any(|k| matches!(k, Key::Scancode { shift: true, .. }))
        {
            self.shift_held = false;
            if let Some(code) = SHIFT.release() {
                self.scancode_up(code).await?;
            }
        }
        Ok(())
    }

    /// Let go of everything this session still has down at the far end: a
    /// screen taken away mid-chord, an event stream that stopped between a
    /// press and its release.
    async fn release_all_keys(&mut self) -> Result<()> {
        let held: Vec<Key> = self.keys.drain().map(|(_, key)| key).collect();
        for key in held {
            if let Some(code) = key.release() {
                self.scancode_up(code).await?;
            }
        }
        if self.shift_held {
            self.shift_held = false;
            if let Some(code) = SHIFT.release() {
                self.scancode_up(code).await?;
            }
        }
        Ok(())
    }

    async fn scancode(&mut self, code: u32) -> Result<()> {
        let mut payload = Vec::new();
        KeyEvent { scancode: code }.write(&mut payload)?;
        self.send(Kind::Inputs, inputs_client::KEY_DOWN, &payload).await
    }

    async fn scancode_up(&mut self, code: u32) -> Result<()> {
        let mut payload = Vec::new();
        KeyEvent { scancode: code }.write(&mut payload)?;
        self.send(Kind::Inputs, inputs_client::KEY_UP, &payload).await
    }

    /// The session left the screen, or came back.
    ///
    /// There is no pause message in SPICE, so what stands in for one is the
    /// protocol's own flow control: the server stops after a window of
    /// unacknowledged messages, so a session nobody is looking at stops
    /// acknowledging and the pixels stop. What is already in flight still
    /// arrives, which is the window and no more.
    async fn focus(&mut self, focused: bool) -> Result<()> {
        if self.focused == focused {
            return Ok(());
        }
        self.focused = focused;
        for kind in [Kind::Display, Kind::Cursor] {
            let ack = ack_of(kind);
            if let Some(channel) = self.channels.get_mut(&kind) {
                channel.set_paused(!focused, ack).await?;
            }
        }
        Ok(())
    }

    async fn compression(&mut self, compression: u8) -> Result<()> {
        self.send(
            Kind::Display,
            display_client::PREFERRED_COMPRESSION,
            &[compression],
        )
        .await
    }

    async fn acknowledge(&mut self, message: &Message) -> Result<()> {
        let ack = ack_of(message.kind);
        let Some(channel) = self.channels.get_mut(&message.kind) else {
            return Ok(());
        };
        // A borrow of the channel cannot be held across the send, since the
        // send is a method on it: the count is taken here and the message goes
        // out under the channel's own lock.
        channel.acknowledge(ack).await
    }

    async fn send(&self, kind: Kind, message_type: u16, payload: &[u8]) -> Result<()> {
        match self.channels.get(&kind) {
            Some(channel) => channel.send(message_type, payload).await,
            None => Ok(()),
        }
    }

    /// The rows that follow the picture: what the server is compressing with,
    /// how many regions it is sending as video, and whether anything was drawn
    /// for a surface this client does not keep.
    fn describe_picture(&mut self) {
        let now = (
            self.display.encodings().len(),
            self.display.streams(),
            self.display.off_surface(),
        );
        if now == self.described {
            return;
        }
        self.described = now;
        let mut info = self.client.info.lock().unwrap();
        info.encoding = self.display.encodings().join(", ");
        info.display = match (now.1, now.2) {
            (0, 0) => String::new(),
            (streams, 0) => format!("{streams} video stream(s)"),
            (0, dropped) => format!("{dropped} draw(s) for another surface"),
            (streams, dropped) => {
                format!("{streams} video stream(s), {dropped} draw(s) for another surface")
            }
        };
    }

    /// The connection panel's fixed rows, taken once at connect time — the ones
    /// that change afterwards are filled in as they do.
    fn describe(&self, config: &Config, connector: Option<&tls::Connector>) {
        let mut info = self.client.info.lock().unwrap();
        info.protocol = VERSION.into();
        info.connection = match connector.and_then(|c| c.negotiated()) {
            Some(negotiated) => format!("{} · {negotiated}", config.address),
            None => config.address.clone(),
        };
        info.security = if config.tls {
            "TLS, pinned · ticket".into()
        } else {
            // The row this backend has to be honest in. On a plain port the
            // ticket is encrypted to a key the server made for this connection
            // and handed over in the clear a moment earlier, so it is proof
            // against nothing that was listening.
            "None · ticket, in the clear".into()
        };
        if info.desktop_name.is_empty() {
            info.desktop_name = config.address.clone();
        }
    }
}

/// Left Shift, which is what a layout's shifted characters are made with.
const SHIFT: Key = Key::Scancode {
    code: 0x2a,
    shift: false,
};

/// The seam's button mask is RFB's, and SPICE's first five bits are the same
/// five buttons in the same order — left, middle, right, wheel up, wheel down.
/// The two beyond them, which RFB carries and this protocol's message cannot,
/// are dropped rather than sent as something else.
fn buttons_of(mask: i32) -> u16 {
    (mask & 0x1f) as u16
}

fn ack_of(kind: Kind) -> u16 {
    match kind {
        Kind::Cursor => cursor_client::ACK,
        Kind::Display => display_client::ACK,
        _ => main_client::ACK,
    }
}

fn ack_sync_of(kind: Kind) -> u16 {
    match kind {
        Kind::Cursor => cursor_client::ACK_SYNC,
        Kind::Display => display_client::ACK_SYNC,
        _ => main_client::ACK_SYNC,
    }
}

fn pong_of(kind: Kind) -> u16 {
    match kind {
        Kind::Cursor => cursor_client::PONG,
        Kind::Display => display_client::PONG,
        _ => main_client::PONG,
    }
}

/// What the app's compression option means on the wire.
pub fn compression_of(name: &str) -> Option<u8> {
    Some(match name {
        "off" => image_compression::OFF,
        "auto-glz" => image_compression::AUTO_GLZ,
        "auto-lz" => image_compression::AUTO_LZ,
        "quic" => image_compression::QUIC,
        "glz" => image_compression::GLZ,
        "lz" => image_compression::LZ,
        "lz4" => image_compression::LZ4,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_button_mask_is_the_same_five_bits() {
        assert_eq!(buttons_of(0x01), 0x01, "left");
        assert_eq!(buttons_of(0x04), 0x04, "right");
        assert_eq!(buttons_of(0x08), 0x08, "wheel up");
        assert_eq!(buttons_of(0x100), 0, "and the ninth button is not sent");
    }

    #[test]
    fn every_compression_the_row_offers_is_one_the_wire_has() {
        for name in ["off", "auto-glz", "auto-lz", "quic", "glz", "lz", "lz4"] {
            assert!(compression_of(name).is_some(), "{name}");
        }
        assert_eq!(compression_of("automatic"), None);
    }
}
