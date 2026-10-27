// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! One SPICE session: its channels, the state above them, and the handle other
//! threads hold.
//!
//! **The session is one task on one current-thread runtime**, the only async
//! executor in this repository. A session is four sockets (five with audio)
//! that have to be read at once, where a thread per socket is awkward. It costs
//! no extra threads: each channel's reader is a task beside this one, on the
//! same thread.
//!
//! Two other threads reach a session, and neither runs the session task: the
//! drawing thread, which takes the framebuffer's read lock directly, and the
//! input thread, which leaves a command on a queue and returns. Neither ever
//! waits for a socket.

use crate::agent::{Agent, Event};
use crate::channel::{Channel, Ended, Kind, Message, Server};
use crate::cursor::Cursors;
use crate::display::Display;
use crate::error::{Error, Result};
use crate::framebuffer::Framebuffer;
use crate::keymap;
use crate::playback::Playback;
use crate::tls;
use runtime::net::Network;
use runtime::{Handler, Hangup};
use runtime::media::audio::Outputs;
use runtime::address::Ports;
use shakenfist_spice_protocol::constants::{
    MOUSE_MODE_CLIENT, MOUSE_MODE_SERVER, cursor_client, display_client, image_compression,
    inputs_client, main_client, main_server,
};
use shakenfist_spice_protocol::messages::{
    ChannelsList, DisplayInit, KeyEvent, MainInit, MouseMotion, MousePosition, Notify,
    Ping, SetAck, WireType,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use tokio::sync::mpsc;

/// The protocol version this client implements, as shown in the connection
/// panel.
pub const VERSION: &str = "SPICE 2.2";

/// The port used when the address has none. QEMU has no default SPICE port, so
/// this is what `spice://host` means in other clients.
pub const DEFAULT_PORT: u16 = 5900;

/// How a session reaches a server, and what it sends.
pub struct Config {
    /// `host:port`. A plain SPICE port and a TLS SPICE port are different
    /// ports rather than a negotiation, so `tls` is a setting rather than
    /// something negotiated.
    pub address: String,
    pub tls: bool,
    pub password: Option<String>,
    pub connect_timeout: Duration,
    /// What to ask the server to compress images with, or `None` for the
    /// server's default (`auto_glz` for QEMU).
    pub compression: Option<u8>,
    pub view_only: bool,
    /// Where the guest's sound goes, or `None` for a session that never
    /// opens the playback channel, so the server does not encode sound that
    /// is never played.
    pub audio: Option<Arc<dyn Outputs>>,
    /// Whether the playback channel is opened at the start, if the server has
    /// one. This is the initial state of the session's Enable switch, which
    /// opens and closes the channel from then on.
    pub sound: bool,
    /// What every channel's connection is made through.
    pub network: Arc<dyn Network>,
}

/// What the connection panel shows about a live session.
#[derive(Clone, Default)]
pub struct Info {
    pub desktop_name: String,
    pub protocol: String,
    pub connection: String,
    pub security: String,
    pub encoding: String,
    /// Which channels linked. A SPICE session is several connections, and
    /// what it can do depends on which of them came up.
    pub channels: String,
    /// Whether the guest is running `spice-vdagent`, which decides whether
    /// there is a clipboard, a resize or a client-mode pointer at all.
    pub agent: String,
    pub display: String,
}

/// Requests from other threads to the session task.
enum Command {
    Pointer { x: i32, y: i32, buttons: i32 },
    PointerRelative { dx: i32, dy: i32, buttons: i32 },
    KeyDown { keysym: u32, id: i64 },
    KeyUp { id: i64 },
    ReleaseAllKeys,
    Focus(bool),
    Compression(u8),
    Clipboard(String),
    /// The layout to ask the guest for: `x, y, width, height` per head, in
    /// the order of the heads' ids; a zero-size entry turns that head off.
    Monitors(Vec<(i32, i32, u32, u32)>),
    /// The session's sound switch: open or close the playback channel.
    Sound(bool),
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
    /// The guest's heads, five ints each: id, x, y, width, height.
    monitors: Arc<Mutex<Vec<i32>>>,
    /// How many heads the device can have, as its last `MONITORS_CONFIG` said;
    /// zero before it has.
    max_heads: Arc<AtomicU32>,
    /// Whether the guest agent is running and accepts a size. This depends on
    /// the session rather than the protocol, so it is polled, and it can turn
    /// true seconds after the picture arrives.
    resizable: Arc<AtomicBool>,
    /// Whether the guest agent is running and accepts a clipboard. Without it
    /// an offered clipboard is dropped, and only this client can report that.
    clipboard: Arc<AtomicBool>,
    /// Signalled by `close`. `Command::Close` does the same, but only the
    /// running session reads the queue: before that, connecting (four dials,
    /// four links, a TLS handshake each) only has per-step deadlines, so a
    /// close sent only through the queue would wait for all of them.
    closing: tokio::sync::Notify,
    /// Ends a channel's dial, which the network does on a blocking thread
    /// that the dropped session future does not stop.
    hangup: Arc<Hangup>,
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
            max_heads: Arc::new(AtomicU32::new(0)),
            resizable: Arc::new(AtomicBool::new(false)),
            clipboard: Arc::new(AtomicBool::new(false)),
            closing: tokio::sync::Notify::new(),
            hangup: Arc::new(Hangup::new()),
        }
    }

    pub fn framebuffer(&self) -> &Arc<RwLock<Framebuffer>> {
        &self.framebuffer
    }

    pub fn info(&self) -> Info {
        self.info.lock().unwrap().clone()
    }

    /// Received and sent, in protocol bytes over all channels.
    pub fn traffic(&self) -> (u64, u64) {
        (
            self.received.load(Ordering::Relaxed),
            self.sent.load(Ordering::Relaxed),
        )
    }

    /// The guest's monitors as `x, y, width, height` each, or empty if it has
    /// not reported a layout.
    pub fn monitors(&self) -> Vec<i32> {
        self.monitors.lock().unwrap().chunks_exact(5).flat_map(|m| [m[1], m[2], m[3], m[4]]).collect()
    }

    /// The same with each head's id in front: five ints each.
    pub fn screens(&self) -> Vec<i32> {
        self.monitors.lock().unwrap().clone()
    }

    pub fn pointer_is_relative(&self) -> bool {
        self.relative.load(Ordering::Relaxed)
    }

    /// Whether the guest accepts a desktop size now, which requires its agent
    /// to be running.
    pub fn can_resize(&self) -> bool {
        self.resizable.load(Ordering::Relaxed)
    }

    /// Whether a clipboard offered now reaches the guest, which requires its
    /// agent to have reported clipboard support.
    pub fn has_clipboard(&self) -> bool {
        self.clipboard.load(Ordering::Relaxed)
    }

    /// Asks the guest for a desktop this size. The guest may answer with this
    /// size, a different one, or nothing.
    pub fn request_desktop_size(&self, width: u32, height: u32) {
        self.request_monitors(&[(0, 0, width, height)]);
    }

    /// The same request with several heads, each at the list position its id
    /// names, which is the message's actual form. This is the only way to add
    /// a second monitor, since a QXL device reports one connected head until a
    /// client asks for more.
    pub fn request_monitors(&self, monitors: &[(i32, i32, u32, u32)]) {
        let _ = self.commands.send(Command::Monitors(monitors.to_vec()));
    }

    /// Requests a layout of `screens` (`id, x, y, width, height` each),
    /// matched to the guest's heads by id (see `heads_for`): existing heads
    /// are moved or resized, and new ones are numbered here.
    pub fn request_layout(&self, screens: &[(u32, i32, i32, u32, u32)]) {
        let have: Vec<u32> = self.monitors.lock().unwrap().chunks_exact(5).map(|m| m[0] as u32).collect();
        let heads = heads_for(screens, &have, self.max_heads.load(Ordering::Relaxed) as usize);
        if !heads.is_empty() {
            self.request_monitors(&heads);
        }
    }

    /// Offers the local clipboard text to the guest.
    pub fn set_clipboard(&self, text: String) {
        let _ = self.commands.send(Command::Clipboard(text));
    }

    pub fn set_view_only(&self, view_only: bool) {
        self.view_only.store(view_only, Ordering::Relaxed);
        if view_only {
            // Keys held on the guest were pressed by this client, and turning
            // on view only must not leave one down.
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

    /// Sets the image compression the server is asked for; the server uses it
    /// from the next image it encodes.
    pub fn set_compression(&self, compression: u8) {
        let _ = self.commands.send(Command::Compression(compression));
    }

    pub fn close(&self) {
        // `notify_one` rather than `notify_waiters`: it leaves a permit when
        // nothing is waiting yet, so a close that arrives before the runtime
        // has started is still the first thing the session sees.
        self.closing.notify_one();
        self.hangup.hang_up();
        let _ = self.commands.send(Command::Close);
    }

    /// Connects, then runs the session until it ends. Blocks the calling
    /// thread, which becomes the session's thread.
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
        // The whole session is raced against the close, rather than the
        // session listening for it: dropping the session future ends a dial,
        // handshake or read wherever it is, and dropping the runtime ends the
        // channels' reader tasks.
        //
        // `biased`, with the close first: `close` hangs up a dial in progress,
        // which ends the session with the hang-up's `Interrupted`, so both
        // branches can be ready at once. `close` notifies before it hangs up,
        // so the close is always ready by then, and checked first it wins.
        let ended = runtime.block_on(async {
            tokio::select! {
                biased;
                () = self.closing.notified() => Err(Error::Closed),
                ended = self.session(config, handler, inbox) => ended,
            }
        });
        // A dial still in progress on the blocking pool (a channel opened as
        // the session ended) is ended here, since dropping the runtime waits
        // for it.
        self.hangup.hang_up();
        // The sound switch goes away with the session.
        if let Some(audio) = &config.audio {
            audio.controls().withdraw();
        }
        ended
    }

    /// Offers the session's sound switch as an Enable switch, initially `on`
    /// or off. Turning it asks the session loop to open or close the playback
    /// channel.
    fn offer_sound(&self, audio: &Arc<dyn Outputs>, on: bool) {
        let commands = self.commands.clone();
        audio.controls().offer_enable(
            on,
            Box::new(move |on| {
                let _ = commands.send(Command::Sound(on));
            }),
        );
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
        let (host, port) = runtime::address::split(&config.address, DEFAULT_PORT, Ports::Plain)
            .map_err(Error::Address)?;
        let connector = match config.tls {
            true => Some(tls::Connector::new(&host)?),
            false => None,
        };

        // 64 messages of buffer between the readers and this task. The ack
        // window (20) is the real flow control, so this queue never stalls a
        // server.
        let (queue, mut messages) = mpsc::channel::<Message>(64);
        let (ends, mut ended) = mpsc::channel::<Ended>(4);

        let mut session = Session {
            client: self,
            server: Server {
                host,
                port,
                network: Arc::clone(&config.network),
                hangup: Arc::clone(&self.hangup),
            },
            channels: HashMap::new(),
            display: Display::new(Arc::clone(&self.framebuffer), IMAGE_CACHE),
            cursors: Cursors::new(),
            agent: Agent::new(),
            keyboard: keymap::Keyboard::default(),
            buttons: 0,
            mouse_modes: 0,
            focused: true,
            connected: false,
            described: (0, 0, 0),
            ticket: config.password.clone(),
            playback: None,
        };

        // The main channel first, then the others with the session id from
        // its INIT, which is the only thing tying the sockets into one
        // session.
        let main = session
            .connect(Kind::Main, 0, config, connector.as_ref(), 0, handler, &queue, &ends)
            .await?;
        session.channels.insert(Kind::Main, main);

        let (session_id, sound) = tokio::time::timeout(
            config.connect_timeout,
            session.handshake(&mut messages, handler),
        )
        .await
        .map_err(|_| Error::Protocol("the server never sent a session id".into()))??;

        for kind in [Kind::Inputs, Kind::Display, Kind::Cursor] {
            let channel = session
                .connect(kind, 0, config, connector.as_ref(), session_id, handler, &queue, &ends)
                .await?;
            session.channels.insert(kind, channel);
        }
        // The guest's sound, if the server has a playback channel and the
        // session has an audio output. The session's switch is an Enable
        // switch from here on, since the channel can be closed and reopened at
        // any time; the option sets whether it opens now. A playback channel
        // that fails to open, or ends later, loses the sound but not the
        // session.
        let audio = config.audio.clone().filter(|_| sound);
        if let Some(audio) = &audio {
            let on = config.sound
                && session
                    .open_playback(audio, config, connector.as_ref(), session_id, handler, &queue, &ends)
                    .await;
            self.offer_sound(audio, on);
        }

        // Sent before the display channel sends anything: how much of the
        // server's image cache this client keeps, and the size of its GLZ
        // window.
        let mut payload = Vec::new();
        DisplayInit {
            cache_id: 1,
            cache_size: IMAGE_CACHE as i64,
            glz_dict_id: 1,
            glz_dict_window: GLZ_WINDOW as i32,
        }
        .write(&mut payload);
        session.send(Kind::Display, display_client::INIT, &payload).await?;
        if let Some(compression) = config.compression {
            session.compression(compression).await?;
        }

        session.describe(config, connector.as_ref());
        session.describe_channels();
        loop {
            // `tick` closes a playback stream that has gone quiet. The
            // half-second wake below runs it when nothing else is arriving.
            if let Some(playback) = &mut session.playback {
                playback.tick();
            }
            let sounding = session.playback.as_ref().is_some_and(Playback::is_open);
            tokio::select! {
                () = tokio::time::sleep(Duration::from_millis(500)), if sounding => {}
                message = messages.recv() => {
                    let Some(message) = message else { return Ok(()) };
                    session.message(message, handler).await?;
                    // Drain whatever else has arrived before ending the frame:
                    // a screenful is a burst of draws, and it should cause one
                    // repaint rather than one per draw.
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
                        // The switch needs what the channels were opened
                        // with, so it is handled here rather than in
                        // `command`.
                        Some(Command::Sound(on)) => {
                            let Some(audio) = &audio else { continue };
                            if on && session.playback.is_none() {
                                log::info!("playback: opening the channel");
                                if !session
                                    .open_playback(audio, config, connector.as_ref(), session_id, handler, &queue, &ends)
                                    .await
                                {
                                    self.offer_sound(audio, false);
                                }
                                session.describe_channels();
                            } else if !on && session.playback.is_some() {
                                // Closing the channel is how the server is
                                // asked to stop: it sends nothing to a closed
                                // channel, and the stream is dropped with
                                // whatever it has queued.
                                log::info!("playback: closing the channel");
                                session.channels.remove(&Kind::Playback);
                                session.playback = None;
                                session.describe_channels();
                            }
                        }
                        Some(command) => session.command(command, handler).await?,
                    }
                }
                end = ended.recv() => {
                    if let Some(end) = end {
                        // The playback channel ending does not end the
                        // session. The switch turns off with it, so turning
                        // it on again retries the channel.
                        if end.kind == Kind::Playback {
                            log::warn!("playback ended ({:?}); the guest's sound is not played", end.error);
                            session.channels.remove(&Kind::Playback);
                            session.playback = None;
                            session.describe_channels();
                            if let Some(audio) = &audio {
                                self.offer_sound(audio, false);
                            }
                            continue;
                        }
                        // Any other channel ending ends the session: besides
                        // main, they carry the picture, the cursor and the
                        // input, and the session cannot work without any of
                        // them.
                        log::info!("{} ended", end.kind.name());
                        return match end.error {
                            Some(e) => Err(e),
                            None => Err(Error::Server),
                        };
                    }
                }
            }
        }
    }
}

/// For `runtime::Runner`, which drives any protocol's client: delegates to
/// the inherent `run` and `close`.
impl runtime::Client for Client {
    type Config = Config;
    type Error = Error;
    const NAME: &'static str = "spice";

    fn run(&self, config: &Config, handler: &mut dyn Handler) -> Result<()> {
        self.run(config, handler)
    }

    fn close(&self) {
        self.close();
    }

    fn closed_on_request(error: &Error) -> bool {
        matches!(error, Error::Closed)
    }
}

impl Default for Client {
    fn default() -> Client {
        Client::new()
    }
}

/// The image cache size `DISPLAY_INIT` promises the server, the same as
/// spice-gtk's. The server counts against it, so a client that keeps less than
/// it promised gets cache misses.
const IMAGE_CACHE: usize = 20 * 1024 * 1024;
/// How far back a GLZ image may refer.
const GLZ_WINDOW: usize = 3 * 1024 * 1024;

/// The session task's state, private to this file.
struct Session<'a> {
    client: &'a Client,
    /// The host and port every channel of this session dials, parsed from the
    /// address once, and the network it dials through.
    server: Server,
    channels: HashMap<Kind, Channel>,
    display: Display,
    cursors: Cursors,
    /// The guest agent (`spice-vdagent`), which handles the clipboard and
    /// resizing. Its messages travel inside the main channel's.
    agent: Agent,
    /// Keys held on the guest, and the Shift added for shifted characters.
    keyboard: keymap::Keyboard,
    buttons: u16,
    mouse_modes: u32,
    focused: bool,
    connected: bool,
    /// What the panel was last told about the picture (the encodings seen, the
    /// live streams and the draws dropped), so that the rows are rebuilt when
    /// one of them changes rather than on every frame.
    described: (usize, usize, u64),
    /// The ticket every channel is opened with: the configured one until the
    /// user answers a refusal, and then the answer. A session is several
    /// connections, and each presents the ticket, so a password typed for the
    /// main channel has to be presented by the others too, or each of them is
    /// refused and prompts again.
    ticket: Option<String>,
    /// The guest's sound, while the session plays it: `None` where the
    /// server has none, there is nowhere to play it, the switch is off, or
    /// its channel has ended.
    playback: Option<Playback>,
}

impl Session<'_> {
    #[allow(clippy::too_many_arguments)]
    async fn connect(
        &mut self,
        kind: Kind,
        caps: u32,
        config: &Config,
        connector: Option<&tls::Connector>,
        session_id: u32,
        handler: &mut dyn Handler,
        queue: &mpsc::Sender<Message>,
        ends: &mpsc::Sender<Ended>,
    ) -> Result<Channel> {
        let mut password = self.ticket.clone();
        // The user is asked about the certificate once, on the main channel:
        // the others reach the same server moments later and must present the
        // same certificate (`tls::Connector::admit`), without asking again.
        let mut asked = false;
        loop {
            // The borrow of `handler` for the trust question ends with this
            // block, so that a refused ticket can ask the same handler for a
            // password on the next iteration.
            let opened = {
                let mut ask = |fingerprint: &str| handler.trust(fingerprint);
                let trust: Option<&mut dyn FnMut(&str) -> bool> = if session_id == 0 && !asked {
                    asked = true;
                    Some(&mut ask)
                } else {
                    None
                };
                // No deadline around this call: `Channel::open` applies one to
                // each step of the connection, but not to the trust question it
                // may ask.
                Channel::open(
                    kind,
                    caps,
                    &self.server,
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
                // A refused ticket is the one error the user can fix. The key
                // it was encrypted to is generated per connection, so retrying
                // means dialling again; this socket cannot be reused.
                Err(Error::Refused(_)) => {
                    log::info!("{}: the ticket was refused", kind.name());
                    // SPICE has only the ticket, no user name.
                    match handler.credentials(false).map(|c| c.password) {
                        Some(answer) => password = Some(answer),
                        None => return Err(Error::Cancelled),
                    }
                }
                Ok(channel) => {
                    self.ticket = password;
                    return Ok(channel);
                }
                other => return other,
            }
        }
    }

    /// Updates the panel's channels row, which changes with the sound switch.
    fn describe_channels(&self) {
        let mut kinds: Vec<Kind> = self.channels.keys().copied().collect();
        kinds.sort_by_key(|kind| kind.channel_type() as u8);
        self.client.info.lock().unwrap().channels =
            kinds.iter().map(|kind| kind.name()).collect::<Vec<_>>().join(", ");
    }

    /// Opens the playback channel, playing on `audio`; whether it opened.
    #[allow(clippy::too_many_arguments)]
    async fn open_playback(
        &mut self,
        audio: &Arc<dyn Outputs>,
        config: &Config,
        connector: Option<&tls::Connector>,
        session_id: u32,
        handler: &mut dyn Handler,
        queue: &mpsc::Sender<Message>,
        ends: &mpsc::Sender<Ended>,
    ) -> bool {
        let playback = Playback::new(Arc::clone(audio));
        match self
            .connect(Kind::Playback, playback.caps(), config, connector, session_id, handler, queue, ends)
            .await
        {
            Ok(channel) => {
                self.channels.insert(Kind::Playback, channel);
                self.playback = Some(playback);
                true
            }
            Err(e) => {
                log::warn!("playback: the channel did not open ({e}); the guest's sound is not played");
                false
            }
        }
    }

    /// Runs the main channel until the rest of the session can be opened: its
    /// `INIT` carries the session id, and the channel list says which channels
    /// the server has. Returns the session id and whether the server has a
    /// playback channel.
    async fn handshake(
        &mut self,
        messages: &mut mpsc::Receiver<Message>,
        handler: &mut dyn Handler,
    ) -> Result<(u32, bool)> {
        let mut session_id = 0;
        while let Some(message) = messages.recv().await {
            match (message.kind, message.message_type) {
                (Kind::Main, main_server::INIT) => {
                    self.acknowledge(&message).await?;
                    let init = MainInit::decode(&message.body)?;
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
                    // The agent was already running when this session started
                    // (the usual case): the token count is in `INIT` and no
                    // `AGENT_CONNECTED` follows.
                    if init.agent_connected != 0 {
                        self.agent_up(init.agent_tokens).await?;
                    } else {
                        self.describe_agent();
                    }
                }
                (Kind::Main, main_server::CHANNELS_LIST) => {
                    self.acknowledge(&message).await?;
                    let list = ChannelsList::decode(&message.body)?;
                    log::info!("{} channels offered", list.channels.len());
                    let playback = Kind::Playback.channel_type() as u8;
                    let sound = list.channels.iter().any(|c| c.channel_type == playback);
                    return Ok((session_id, sound));
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
                let set_ack = SetAck::decode(&message.body)?;
                if let Some(channel) = self.channels.get_mut(&message.kind) {
                    channel.ack_window = set_ack.window;
                }
                let mut payload = Vec::new();
                set_ack.ack_sync().write(&mut payload);
                self.send(message.kind, ack_sync_of(message.kind), &payload).await?;
                return Ok(());
            }
            main_server::PING => {
                // The server's ping carries padding for measuring the link,
                // which is most of the traffic on an idle session.
                let ping = Ping::decode(&message.body)?;
                let mut payload = Vec::new();
                ping.pong().write(&mut payload);
                self.send(message.kind, pong_of(message.kind), &payload).await?;
                return Ok(());
            }
            main_server::NOTIFY => {
                let notify = Notify::decode(&message.body)?;
                log::info!("{}: {}", message.kind.name(), notify.message_text());
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
                    .flat_map(|&(id, x, y, w, h)| [id as i32, x, y, w, h])
                    .collect();
                self.client
                    .max_heads
                    .store(u32::from(self.display.max_heads()), Ordering::Relaxed);
            }
            Kind::Cursor => self.cursors.message(message.message_type, &message.body, handler)?,
            Kind::Playback => {
                if let Some(playback) = &mut self.playback {
                    playback.message(message.message_type, &message.body);
                }
            }
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
                // The only message with the machine's name, and it is
                // optional: QEMU without `-name` never sends it.
                let name = String::from_utf8_lossy(
                    message.body.get(4..).unwrap_or_default().split(|b| *b == 0).next().unwrap_or_default(),
                )
                .to_string();
                if !name.is_empty() {
                    self.client.info.lock().unwrap().desktop_name = name;
                }
            }
            main_server::AGENT_CONNECTED | main_server::AGENT_CONNECTED_TOKENS => {
                // Only `AGENT_CONNECTED_TOKENS` carries tokens, for a client
                // that asked for them; without them this client has none and
                // waits for an `AGENT_TOKEN`.
                let tokens = match message.body.len() >= 4 {
                    true => u32::from_le_bytes(message.body[0..4].try_into().unwrap()),
                    false => 0,
                };
                self.agent_up(tokens).await?;
            }
            main_server::AGENT_DISCONNECTED => {
                // The guest logged out or the agent was killed. Its clipboard
                // and resize support go with it, so the panel, `can_resize` and
                // `has_clipboard` change.
                self.agent.disconnected();
                self.client.resizable.store(false, Ordering::Relaxed);
                self.client.clipboard.store(false, Ordering::Relaxed);
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
            main_server::DISCONNECTING => return Err(Error::Server),
            _ => {}
        }
        Ok(())
    }

    /// The agent came up, with the session or later when a user logged into
    /// the guest.
    ///
    /// `AGENT_START` opens the channel to it, and its one field is how many
    /// chunks the *guest* may send this client before waiting. It is
    /// unlimited, as in spice-gtk: metering a clipboard would cost a token
    /// message per chunk and gain nothing on a link already flow-controlled by
    /// the ack window.
    async fn agent_up(&mut self, tokens: u32) -> Result<()> {
        log::info!("agent: connected, with {tokens} token(s) to send on");
        self.agent.connected(tokens);
        self.describe_agent();
        self.send(Kind::Main, main_client::AGENT_START, &u32::MAX.to_le_bytes())
            .await?;
        self.flush_agent().await
    }

    /// Sends the agent's queued messages as far as the tokens allow. The rest
    /// stays queued until the server grants more.
    async fn flush_agent(&mut self) -> Result<()> {
        while let Some(piece) = self.agent.next_piece() {
            self.send(Kind::Main, main_client::AGENT_DATA, &piece).await?;
        }
        Ok(())
    }

    /// Updates `has_clipboard` and the panel's guest agent row, which shows
    /// whether the session has a clipboard (and so why Paste may do nothing).
    fn describe_agent(&self) {
        self.client
            .clipboard
            .store(self.agent.has_clipboard(), Ordering::Relaxed);
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

    /// Tracks which end owns the pointer, and asks for client mode where the
    /// server supports it.
    ///
    /// Client mode is the absolute pointer every other backend here uses. In
    /// server mode the guest owns the cursor, and a guest with no tablet
    /// attached offers only that, so the handler is told and the app switches
    /// to its relative pointer.
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
                    buttons_state: buttons_of(buttons),
                    display_id: 0,
                }
                .write(&mut payload);
                self.send(Kind::Inputs, inputs_client::MOUSE_POSITION, &payload).await?;
                self.mouse_buttons(buttons).await?;
            }
            Command::PointerRelative { dx, dy, buttons } if !view_only => {
                let mut payload = Vec::new();
                MouseMotion {
                    dx,
                    dy,
                    buttons_state: buttons_of(buttons),
                }
                .write(&mut payload);
                self.send(Kind::Inputs, inputs_client::MOUSE_MOTION, &payload).await?;
                self.mouse_buttons(buttons).await?;
            }
            Command::KeyDown { keysym, id } if !view_only => self.key_down(keysym, id).await?,
            Command::KeyUp { id } if !view_only => self.key_up(id).await?,
            Command::ReleaseAllKeys => self.release_all_keys().await?,
            Command::Focus(focused) => self.focus(focused).await?,
            Command::Compression(compression) => self.compression(compression).await?,
            // Replacing the guest's clipboard is input, so view only blocks
            // it, as in every other backend here.
            Command::Clipboard(text) if !view_only => {
                self.agent.offer_clipboard(text);
                self.flush_agent().await?;
            }
            // View only does not block a resize: it changes the size of the
            // picture rather than acting inside the guest, and no other backend
            // here blocks it either.
            Command::Monitors(monitors) => {
                log::info!("agent: asking the guest for {monitors:?}");
                self.agent.request_monitors(&monitors);
                self.flush_agent().await?;
            }
            Command::Close => return Err(Error::Closed),
            // View only. The app already stops sending input, so what arrives
            // here was in flight when it was turned on.
            _ => {
                let _ = handler;
            }
        }
        Ok(())
    }

    /// Sends a press or release for each button that changed in the mask,
    /// since SPICE has a message per transition where the backend API passes
    /// the button state.
    async fn mouse_buttons(&mut self, mask: i32) -> Result<()> {
        let wanted = buttons_of(mask);
        for bit in 0..BUTTONS {
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
            let payload = press_of(bit, self.buttons);
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
            // with no place on a US keyboard is dropped rather than sent
            // wrongly. Logged at trace only, since the keysym reveals what was
            // typed.
            log::trace!("no scancode for keysym {keysym:#x}");
            return Ok(());
        };
        let strokes = self.keyboard.down(id, key);
        self.strokes(strokes).await
    }

    async fn key_up(&mut self, id: i64) -> Result<()> {
        let strokes = self.keyboard.up(id);
        self.strokes(strokes).await
    }

    /// Releases everything this session still holds down on the guest.
    async fn release_all_keys(&mut self) -> Result<()> {
        let strokes = self.keyboard.release_all();
        self.strokes(strokes).await
    }

    async fn strokes(&mut self, strokes: Vec<keymap::Stroke>) -> Result<()> {
        for stroke in strokes {
            let (message, code) = match stroke {
                keymap::Stroke::Down(code) => (inputs_client::KEY_DOWN, code),
                keymap::Stroke::Up(code) => (inputs_client::KEY_UP, code),
            };
            // Trace only: with the keysym it is what was typed.
            log::trace!("scancode {code:#x} {}", if message == inputs_client::KEY_DOWN { "down" } else { "up" });
            let mut payload = Vec::new();
            KeyEvent { scancode: code }.write(&mut payload);
            self.send(Kind::Inputs, message, &payload).await?;
        }
        Ok(())
    }

    /// The session left the screen, or came back.
    ///
    /// There is no pause message in SPICE, so the protocol's flow control
    /// stands in for one: the server stops after a window of unacknowledged
    /// messages, so an unfocused session stops acking and the picture stops.
    /// What is already in flight still arrives, up to one window.
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

    /// The connection panel's fixed rows, set once at connect time; the rows
    /// that change later are filled in as they change.
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
            // On a plain port the ticket is encrypted to a key the server made
            // for this connection and sent in the clear a moment earlier. That
            // protects it from a passive observer, but not from anyone able to
            // replace the key.
            "None · ticket, in the clear".into()
        };
        if info.desktop_name.is_empty() {
            info.desktop_name = config.address.clone();
        }
    }
}

/// The backend API's button mask is RFB's, and SPICE's first five bits are the
/// same five buttons in the same order: left, middle, right, wheel up, wheel
/// down. RFB's buttons 8 and 9, a mouse's side buttons (back and forward), are
/// SPICE's next two, side and extra. RFB's horizontal wheel (bits 5 and 6) has
/// no equivalent in this protocol and is dropped.
fn buttons_of(mask: i32) -> u16 {
    let mut buttons = (mask & 0x1f) as u16;
    if mask & 0x80 != 0 {
        buttons |= 1 << 5;
    }
    if mask & 0x100 != 0 {
        buttons |= 1 << 6;
    }
    buttons
}

/// How many buttons SPICE's mask has: [`buttons_of`]'s seven bits.
const BUTTONS: u16 = 7;

/// A press or release of the button at `bit` of SPICE's mask: its
/// `SpiceMouseButton` (the bit's place counted from one: left, middle,
/// right, up, down, side, extra) and the mask after it. Written here rather
/// than by ryll's `MouseButton::id_for_mask`, which maps only left, middle,
/// right and the wheel and makes side and extra the invalid button 0.
fn press_of(bit: u16, buttons: u16) -> Vec<u8> {
    let mut payload = vec![bit as u8 + 1];
    payload.extend_from_slice(&buttons.to_le_bytes());
    payload
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

/// The agent's request for a layout of `screens` (`id, x, y, width, height`
/// each). The agent's message has no head ids: a head is its position in the
/// list, so each screen goes at the position its id names, and an unused
/// position is a zero-size head, which the guest turns off. An id the guest
/// has (`have`) is that head; an unknown or repeated id is a new head at the
/// lowest position neither list uses, as RFB does for a new screen. Past
/// `max` heads, if the device has reported a maximum, a screen is left out.
fn heads_for(screens: &[(u32, i32, i32, u32, u32)], have: &[u32], max: usize) -> Vec<(i32, i32, u32, u32)> {
    let mut placed: Vec<Option<(i32, i32, u32, u32)>> = Vec::new();
    let mut taken: Vec<u32> = Vec::new();
    let known: Vec<u32> = screens.iter().map(|s| s.0).filter(|id| have.contains(id)).collect();
    for &(id, x, y, w, h) in screens {
        let id = if have.contains(&id) && !taken.contains(&id) {
            id
        } else {
            match (0..).find(|n| !have.contains(n) && !known.contains(n) && !taken.contains(n)) {
                Some(n) => n,
                None => continue,
            }
        };
        if max > 0 && id as usize >= max {
            log::warn!("display: head {id} is past the {max} the device has");
            continue;
        }
        taken.push(id);
        let at = id as usize;
        if placed.len() <= at {
            placed.resize(at + 1, None);
        }
        placed[at] = Some((x, y, w, h));
    }
    placed.into_iter().map(|p| p.unwrap_or((0, 0, 0, 0))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_layout_puts_each_screen_at_its_head() {
        // The guest's two heads, resized: each at its own place.
        assert_eq!(
            heads_for(&[(0, 0, 0, 1280, 800), (1, 1280, 0, 1024, 768)], &[0, 1], 4),
            vec![(0, 0, 1280, 800), (1280, 0, 1024, 768)]
        );
        // Listed the other way round: still by id, not by place in the list.
        assert_eq!(
            heads_for(&[(1, 0, 0, 1024, 768), (0, 1024, 0, 1280, 800)], &[0, 1], 4),
            vec![(1024, 0, 1280, 800), (0, 0, 1024, 768)]
        );
        // A new screen is the lowest unused head; one of the guest's heads
        // left out is a zero-size head, which turns it off.
        assert_eq!(
            heads_for(&[(1, 0, 0, 1280, 800), (u32::MAX, 1280, 0, 800, 600)], &[0, 1], 4),
            vec![(0, 0, 0, 0), (0, 0, 1280, 800), (1280, 0, 800, 600)]
        );
        // A repeated id is a new screen too.
        assert_eq!(
            heads_for(&[(0, 0, 0, 800, 600), (0, 800, 0, 800, 600)], &[0], 0),
            vec![(0, 0, 800, 600), (800, 0, 800, 600)]
        );
        // Past what the device has: nowhere to go.
        let three = [(0, 0, 0, 800, 600), (1, 800, 0, 800, 600), (7, 1600, 0, 800, 600)];
        assert_eq!(heads_for(&three, &[0, 1], 2), vec![(0, 0, 800, 600), (800, 0, 800, 600)]);
    }

    #[test]
    fn the_button_mask_is_the_same_five_bits_and_the_side_buttons() {
        assert_eq!(buttons_of(0x01), 0x01, "left");
        assert_eq!(buttons_of(0x04), 0x04, "right");
        assert_eq!(buttons_of(0x08), 0x08, "wheel up");
        assert_eq!(buttons_of(0x80), 0x20, "back is side");
        assert_eq!(buttons_of(0x100), 0x40, "forward is extra");
        assert_eq!(buttons_of(0x20 | 0x40), 0, "the horizontal wheel is not sent");
        // spice-protocol's SpiceMouseButton: the id is the bit's place
        // from one, and the mask follows little-endian.
        assert_eq!(press_of(0, 0x01), [1, 0x01, 0], "left");
        assert_eq!(press_of(2, 0x04), [3, 0x04, 0], "right");
        assert_eq!(press_of(4, 0x10), [5, 0x10, 0], "wheel down");
        assert_eq!(press_of(5, 0x21), [6, 0x21, 0], "side, left still held");
        assert_eq!(press_of(6, 0), [7, 0, 0], "extra, let go");
    }

    #[test]
    fn every_compression_the_row_offers_is_one_the_wire_has() {
        for name in ["off", "auto-glz", "auto-lz", "quic", "glz", "lz", "lz4"] {
            assert!(compression_of(name).is_some(), "{name}");
        }
        assert_eq!(compression_of("automatic"), None);
    }
}
