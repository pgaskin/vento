// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! `vento`: one process that holds a session on any of the eight backends.
//!
//! [`main`] parses the command line ([`cli`]) into a [`Mode`] and
//! dispatches. The window is the default: nothing on the command line opens
//! the connection dialog empty, a backend and an address open it filled in
//! (connecting to another machine takes a click, so a typo does not start a
//! connection), and `--connect` opens straight into the session, which quits
//! the window when it is disconnected. `--help` prints the options and the
//! backends with theirs, `--list` each backend's library, and `--forget`
//! forgets a server (`known_hosts::forget`) without connecting anywhere.
//! `--probe` selects the headless mode instead, modelled on
//! `backends/platform/linux/probe/probe.c`: it connects, prints every event,
//! answers the prompts from the flags, runs the steps of its spec
//! ([`cli::Step`]: waits, input, requests, the framebuffer and its thumbnail
//! written), prints the facts, and exits with a status that is zero only if
//! it connected, stayed connected through the steps, wrote what was asked
//! for and closed when told to. Input is sent and not confirmed, since no
//! protocol confirms it; the failure that can be detected (a clipboard the
//! backend says has nowhere to go) fails it.

mod cli;

fn main() {
    std::process::exit(host::run(std::env::args().skip(1).collect()));
}

mod host {
    use super::cli::{self, Arg, Step};
    use runtime::media::audio::Outputs as _;
    use std::cell::Cell;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use vento::backend::{Backend, Credentials, Monitor};
    use vento::backends::{self, Connect, Dirs, Kind, Platform};
    use vento::events::{Event, Mailbox, State};
    use vento::gui;
    use api::trust::{self, Decision};
    use vento::known_hosts::{self, KnownHosts};
    use vento::options::{self, Options, Type};
    use vento::ppm;

    /// What the command line resolved to.
    enum Mode {
        /// The options, and the backends with theirs.
        Help,
        /// `vento` and [`cli::version`].
        Version,
        /// Each backend's library.
        List,
        /// A headless session, as the probe.
        Probe(Box<Probe>),
        /// The window: a session, or the dialog to make one.
        Gui(Box<gui::Launch>),
        /// A server forgotten: `known_hosts::forget`.
        Forget { address: String, dirs: Dirs },
    }

    /// A probe's command line, parsed and checked.
    struct Probe {
        kind: &'static Kind,
        address: String,
        user: String,
        domain: String,
        password: Option<String>,
        options: Options,
        dirs: Dirs,
        /// `--probe`'s spec, run in order once connected.
        steps: Vec<Step>,
        /// The spec's `wait-connect=`, or the default.
        connect_timeout: Duration,
        trust: bool,
        /// `sound-cycle=`: how often the far end's sound is turned off and
        /// on again, where the session's switch can ask it to.
        sound_cycle: Cell<Option<Duration>>,
        /// The session's sound controls, for `sound-cycle=`, and when it
        /// next turns the switch.
        sound_controls: std::cell::RefCell<Option<Arc<runtime::media::audio::Controls>>>,
        sound_cycle_at: Cell<Option<Instant>>,
        /// The session's sound as last printed, so that each change is
        /// printed on its own line.
        sound: Cell<vento::events::Sound>,
        /// `--no-recv-clipboard`'s opposite: whether what the far end copies
        /// is shown, which is the probe's way of taking it.
        clipboard_in: bool,
        /// `--video` and `--hw-decode`: how video is decoded.
        hw_decode: backends::Hardware,
        /// `--audio`: where the far end's sound plays.
        audio_output: backends::AudioOutput,
        /// Whether `--password` has been given to a credentials prompt.
        /// It is given once: a second prompt is the far end saying it was
        /// wrong, and answering that with the same password again is a loop
        /// (SPICE dials afresh for every refused ticket, nearly two hundred
        /// times in five seconds) that ends as "no connection in time".
        answered: Cell<bool>,
        /// Whether a second prompt came, so the password was refused.
        refused: Cell<bool>,
    }

    pub fn run(args: Vec<String>) -> i32 {
        let args = match cli::split(args, cli::FLAGS) {
            Ok(args) => args,
            Err(message) => return usage_error(&message),
        };
        vento::log::init(args.contains(&Arg::Flag("verbose", None)));
        match parse(args) {
            Ok(Mode::Help) => {
                print!("{}\n{}", cli::usage(&defaults()), cli::backends(&help_backends()));
                0
            }
            Ok(Mode::Version) => {
                println!("vento {}", cli::version());
                0
            }
            Ok(Mode::List) => {
                for kind in backends::all() {
                    println!("{}", cli::list_line(kind.id, &backends::library_path(kind)));
                }
                0
            }
            Ok(Mode::Probe(probe)) => probe.run(),
            Ok(Mode::Gui(launch)) => gui::run(*launch),
            Ok(Mode::Forget { address, dirs }) => forget(&address, &dirs),
            Err(message) => usage_error(&message),
        }
    }

    fn usage_error(message: &str) -> i32 {
        eprintln!("vento: {message}\nsee vento --help");
        2
    }

    /// The directories' defaults, as the help shows them.
    fn defaults() -> cli::Defaults {
        cli::Defaults {
            extlib_dir: backends::default_extlib_dir().display().to_string(),
            state_dir: backends::default_state_dir().display().to_string(),
        }
    }

    /// `--forget`: what was removed, what was kept for the same host, and
    /// what could not be changed, which is the only failure.
    fn forget(address: &str, dirs: &Dirs) -> i32 {
        let done = known_hosts::forget(dirs, &backends::all(), address);
        if done.removed.is_empty() {
            println!("no known_hosts entries for {address}");
        } else {
            println!("removed {address} from known_hosts");
            for line in &done.removed {
                println!("  {line}");
            }
        }
        if !done.kept.is_empty() {
            println!("kept other hosts with the same identity:");
            for line in &done.kept {
                println!("  {line}");
            }
        }
        for line in &done.failed {
            eprintln!("failed to remove known_hosts entry: {line}");
        }
        i32::from(!done.failed.is_empty())
    }

    /// The backends and their options as the help lists them, each library
    /// opened for its version.
    fn help_backends() -> Vec<cli::Backend> {
        backends::all()
            .into_iter()
            .map(|kind| cli::Backend {
                id: kind.id.to_string(),
                name: kind.name.to_string(),
                version: (kind.version)(),
                description: kind.description.to_string(),
                options: kind
                    .options
                    .iter()
                    .map(|spec| {
                        let values = match spec.ty {
                            Type::Bool => vec!["true".to_string(), "false".to_string()],
                            Type::Choice => spec
                                .choices
                                .iter()
                                .map(|c| if c.value.is_empty() { "\"\"".to_string() } else { c.value.to_string() })
                                .collect(),
                            Type::Int | Type::Text => Vec::new(),
                        };
                        let gate = spec
                            .gate
                            .map(|g| format!(" (only when {}={})", g.key, g.values.join("/")))
                            .unwrap_or_default();
                        cli::BackendOption {
                            key: spec.key.to_string(),
                            default: spec.default.to_string(),
                            values,
                            description: format!("{}{gate}", spec.summary_on(Platform::Linux).unwrap_or(spec.label)),
                        }
                    })
                    .collect(),
            })
            .collect()
    }

    fn parse(args: Vec<Arg>) -> Result<Mode, String> {
        let mut help = false;
        let mut version = false;
        let mut list = false;
        let mut positional = Vec::new();
        let mut user = String::new();
        let mut domain = String::new();
        let mut password = None;
        let mut assignments: Vec<String> = Vec::new();
        let mut steps = None;
        let mut state_dir = None;
        let mut extlib_dir = None;
        let mut settings = gui::Settings::default();
        let mut connect = false;
        // The window's display server, if named; the probe has no window and
        // takes any.
        let mut display_server = None;
        // The window's own decorations, unless `--no-csd`.
        let mut csd = true;
        let mut forget = None;

        for arg in args {
            let (name, value) = match arg {
                Arg::Positional(arg) => {
                    positional.push(arg);
                    continue;
                }
                Arg::Flag(name, value) => (name, value.unwrap_or_default()),
            };
            match name {
                "help" => help = true,
                "version" => version = true,
                "list" => list = true,
                "forget" => forget = Some(value),
                "connect" => connect = true,
                "user" => user = value,
                "domain" => domain = value,
                "password" => password = Some(value),
                "option" => assignments.push(value),
                "trust" => settings.trust = true,
                "platform" => {
                    display_server = match value.as_str() {
                        "auto" => None,
                        "x11" => Some(gui::DisplayServer::X11),
                        "wayland" => Some(gui::DisplayServer::Wayland),
                        _ => return Err(format!("--platform: {value:?} is not one of auto, x11, or wayland")),
                    }
                }
                "video" => {
                    settings.video = backends::hardware_named(&value)
                        .ok_or_else(|| format!("--video: {value:?} is not one of auto, vaapi, or off"))?
                }
                "audio" => {
                    settings.audio_output = backends::AudioOutput::from_name(&value)
                        .ok_or_else(|| format!("--audio: {value:?} is not one of auto, pipewire, alsa, or off"))?
                }
                "no-csd" => csd = false,
                "cursor" => {
                    settings.software_cursor = match value.as_str() {
                        "native" => false,
                        "software" => true,
                        _ => return Err(format!("--cursor: {value:?} is not one of native or software")),
                    }
                }
                "extlib-dir" => extlib_dir = Some(PathBuf::from(value)),
                "state-dir" => state_dir = Some(PathBuf::from(value)),
                "verbose" => {}
                "scale" => (settings.scale, settings.fit_window) = scale(&value)?,
                "view-only" => settings.view_only = true,
                "hw-decode" => settings.hw_decode = true,
                "no-send-clipboard" => settings.clipboard_out = false,
                "no-recv-clipboard" => settings.clipboard_in = false,
                "no-grab-keyboard" => settings.grab_keyboard = false,
                "probe" => steps = Some(cli::steps(&value).map_err(|e| format!("--probe: {e}"))?),
                other => unreachable!("--{other} is in cli::FLAGS but not parsed"),
            }
        }

        if help {
            return Ok(Mode::Help);
        }
        if version {
            return Ok(Mode::Version);
        }
        if list {
            return Ok(Mode::List);
        }
        let headless = steps.is_some();
        let dirs = backends::dirs(
            &state_dir.unwrap_or_else(backends::default_state_dir),
            &extlib_dir.unwrap_or_else(backends::default_extlib_dir),
        );
        if let Some(address) = forget {
            if !positional.is_empty() || headless || connect || !assignments.is_empty() {
                return Err("--forget only takes an address, and not other options".into());
            }
            if trust::key(&address).is_empty() {
                return Err("--forget needs an address".into());
            }
            return Ok(Mode::Forget { address, dirs });
        }
        if positional.is_empty() && !headless && !connect && assignments.is_empty() {
            return Ok(Mode::Gui(Box::new(gui::Launch {
                request: None,
                connect: false,
                dirs,
                settings,
                display_server,
                csd,
            })));
        }
        // The dialog can be filled in from a backend alone; a session needs
        // somewhere to go.
        let whole = headless || connect;
        if positional.is_empty() || positional.len() > 2 || (whole && positional.len() != 2) {
            return Err("give a backend and an address".into());
        }
        let kind = backends::find(&positional[0])
            .ok_or_else(|| format!("no backend {:?}, see --list", positional[0]))?;

        let mut options = Options::defaults(kind.options);
        for assignment in &assignments {
            let (k, v) = options::parse_assignment(assignment)?;
            options::find(kind.options, k)?.check(v)?;
            options.set(k, v);
        }
        if settings.view_only && options::find(kind.options, "ViewOnly").is_ok() {
            options.set("ViewOnly", "true");
        }
        options.check_requirements(kind.options)?;

        let Some(mut steps) = steps else {
            return Ok(Mode::Gui(Box::new(gui::Launch {
                request: Some(gui::Request {
                    kind,
                    address: positional.get(1).cloned().unwrap_or_default(),
                    user,
                    domain,
                    password: password.filter(|p| !p.is_empty()),
                    options,
                }),
                connect,
                dirs,
                settings,
                display_server,
                csd,
            })));
        };

        // `cli::steps` takes `wait-connect=` only as the first step.
        let connect_timeout = match steps.first() {
            Some(&Step::WaitConnect(time)) => {
                steps.remove(0);
                time
            }
            _ => Duration::from_secs(cli::CONNECT_TIMEOUT),
        };

        Ok(Mode::Probe(Box::new(Probe {
            kind,
            address: positional.remove(1),
            user,
            domain,
            password,
            options,
            dirs,
            steps,
            connect_timeout,
            trust: settings.trust,
            sound_cycle: Cell::new(None),
            sound_controls: std::cell::RefCell::new(None),
            sound_cycle_at: Cell::new(None),
            sound: Cell::new(Default::default()),
            clipboard_in: settings.clipboard_in,
            hw_decode: settings.hardware(),
            audio_output: settings.audio_output,
            answered: Cell::new(false),
            refused: Cell::new(false),
        })))
    }

    /// A screen as the probe prints it: `id=N WxH+X+Y`, as heads.sh does.
    fn screen(m: &Monitor) -> String {
        format!("id={} {}x{}+{}+{}", m.id, m.width, m.height, m.x, m.y)
    }

    /// `--scale`'s modes: the window's own, and `none-resize`, which is
    /// `none` (one to one) with the window made the desktop's size once.
    fn scale(value: &str) -> Result<(gui::ScaleMode, bool), String> {
        use gui::ScaleMode;
        Ok(match value {
            "auto" => (ScaleMode::Auto, false),
            "fit" => (ScaleMode::Fit, false),
            "fill" => (ScaleMode::Fill, false),
            "fit-width" => (ScaleMode::FitWidth, false),
            "fit-height" => (ScaleMode::FitHeight, false),
            "fill-width" => (ScaleMode::FillWidth, false),
            "fill-height" => (ScaleMode::FillHeight, false),
            "none" => (ScaleMode::Original, false),
            "none-resize" => (ScaleMode::Original, true),
            _ => {
                return Err(format!(
                    "--scale: {value:?} is not one of auto, fit, fill, fit-height, fit-width, fill-width, \
                     fill-height, none, or none-resize"
                ));
            }
        })
    }

    /// At most `bytes` of `text`, cut where a character starts: the far end's
    /// clipboard can hold any text, and a cut through the middle of a
    /// character is a panic rather than a shorter line.
    fn head(text: &str, bytes: usize) -> &str {
        &text[..text.floor_char_boundary(bytes)]
    }

    impl Probe {
        fn run(self: Box<Self>) -> i32 {
            let start = Instant::now();
            let say = |event: &str| println!("[{:8.3}] {event}", start.elapsed().as_secs_f64());

            println!("{}", (self.kind.version)());
            let mailbox = Mailbox::new(None);
            let backend = {
                let client_name = backends::client_name();
                let identity = known_hosts::saved_identity(&self.dirs.config, self.kind, &self.address);
                let connect = Connect {
                    address: &self.address,
                    user: &self.user,
                    domain: backends::domain(self.kind, &self.domain),
                    password: self.password.as_deref(),
                    options: &self.options,
                    mailbox: Arc::clone(&mailbox),
                    dirs: &self.dirs,
                    screen: None,
                    // The desktop keeps no note of a desktop's last size, so
                    // `DesktopSize=remote` is the fallback here.
                    last_size: None,
                    identity: identity.as_deref(),
                    client_name: &client_name,
                    platform: Platform::Linux,
                    decoders: Some(backends::decoders(self.hw_decode)),
                    // `--audio off`: no output, so the far end's sound is
                    // dropped.
                    audio: {
                        let audio = backends::audio(&self.address, self.audio_output);
                        *self.sound_controls.borrow_mut() = Some(Arc::clone(audio.controls()));
                        audio.plays().then_some(audio as _)
                    },
                };
                say("create");
                match self.kind.start(&connect) {
                    Ok(backend) => backend,
                    Err(why) => {
                        say(&format!("create failed: {why}"));
                        return 1;
                    }
                }
            };

            let mut status = 0;
            // Drain and print until connected, or the connect deadline.
            let connected = self.pump(&mailbox, &backend, &say, self.connect_timeout, |m| {
                m.state() != State::Connecting
            });
            if mailbox.state() != State::Connected {
                say(if self.refused.get() {
                    "authentication failed: the password was refused"
                } else if connected {
                    "ended before connecting"
                } else {
                    "no connection in time"
                });
                status = 1;
            } else {
                for step in &self.steps {
                    if mailbox.state() != State::Connected {
                        break;
                    }
                    if !self.step(step, &mailbox, &backend, &say) {
                        status = 1;
                    }
                }
                if mailbox.state() != State::Connected {
                    say(if self.refused.get() {
                        "ended by server: the password was refused"
                    } else {
                        "ended by server"
                    });
                    status = 1;
                } else {
                    self.facts(&backend, &say);
                    // Asked at the end rather than when it was sent, since an
                    // agent that comes up late still gets the offer.
                    let sent_clipboard = self.steps.iter().any(|s| matches!(s, Step::Clipboard(_)));
                    if sent_clipboard && backend.carries_clipboard() == Some(false) {
                        say("clipboard not carried: the far end has nowhere to put it");
                        status = 1;
                    }
                }
            }

            say("disconnect");
            backend.disconnect();
            if !self.pump(&mailbox, &backend, &say, Duration::from_secs(10), |m| m.state() == State::Closed) {
                say("no close within 10 s of disconnect");
                status = 1;
            }
            say("destroy");
            backend.destroy();
            say("done");
            status
        }

        /// Prints events until `done` is true, the session closes, or `time`
        /// passes; whether `done` was reached. Prompts are answered from the
        /// flags as they arrive.
        fn pump(
            &self,
            mailbox: &Mailbox,
            backend: &Arc<dyn Backend>,
            say: &dyn Fn(&str),
            time: Duration,
            done: impl Fn(&Mailbox) -> bool,
        ) -> bool {
            let deadline = Instant::now() + time;
            loop {
                while let Some(event) = mailbox.take() {
                    self.print(&event, say);
                    self.answer(&event, backend, say);
                    if let Event::Closed { .. } = event {
                        return done(mailbox);
                    }
                }
                if done(mailbox) {
                    return true;
                }
                let now = Instant::now();
                if now >= deadline {
                    return done(mailbox);
                }
                self.cycle_sound(now, say);
                mailbox.wait((deadline - now).min(Duration::from_millis(200)));
            }
        }

        /// `sound-cycle=`: turns the switch once it is due, where the far
        /// end can be asked; the first turn is a period after the step, or
        /// after the switch is offered if that is later.
        fn cycle_sound(&self, now: Instant, say: &dyn Fn(&str)) {
            let (Some(every), Some(controls)) = (self.sound_cycle.get(), self.sound_controls.borrow().clone()) else {
                return;
            };
            if controls.sound().switch != Some(runtime::media::audio::Switch::Enable) {
                self.sound_cycle_at.set(None);
                return;
            }
            match self.sound_cycle_at.get() {
                None => self.sound_cycle_at.set(Some(now + every)),
                Some(at) if now >= at => {
                    let on = !controls.enabled();
                    say(if on { "turning the server's sound on" } else { "turning the server's sound off" });
                    controls.set_enabled(on);
                    self.sound_cycle_at.set(Some(now + every));
                }
                Some(_) => {}
            }
        }

        fn print(&self, event: &Event, say: &dyn Fn(&str)) {
            match event {
                Event::Connected { width, height } => say(&format!("connected {width}x{height}")),
                Event::DesktopSize { width, height } => say(&format!("desktop_size {width}x{height}")),
                Event::Frame { rects, .. } => say(&format!("frame_end ({} rectangles)", rects.len())),
                Event::Cursor(None) => say("cursor default"),
                Event::Cursor(Some(c)) => {
                    let opaque = c.rgba.chunks_exact(4).filter(|p| p[3] != 0).count();
                    say(&format!(
                        "cursor {}x{} hot {},{}, {opaque} of {} pixels visible",
                        c.width,
                        c.height,
                        c.hot_x,
                        c.hot_y,
                        c.width * c.height
                    ));
                }
                Event::PointerMode { relative } => {
                    say(&format!("pointer_mode {}", if *relative { "relative" } else { "absolute" }))
                }
                Event::Bell => say("bell"),
                Event::Status(Some(text)) => say(&format!("status: {text}")),
                Event::Status(None) => say("status cleared"),
                Event::Sound(sound) => {
                    let before = self.sound.replace(*sound);
                    if (sound.switch, sound.enabled, sound.muted) != (before.switch, before.enabled, before.muted) {
                        say(&match sound.switch {
                            None => "sound switch: none".to_string(),
                            Some(switch) => format!(
                                "sound switch: {} {}",
                                switch.label(),
                                if sound.on() { "on" } else { "off" }
                            ),
                        });
                    }
                    if sound.playing != before.playing {
                        say(if sound.playing { "audio playing" } else { "audio stopped" });
                    }
                }
                Event::Clipboard(text) if !self.clipboard_in => {
                    say(&format!("clipboard {} bytes, not taken (--no-recv-clipboard)", text.len()))
                }
                Event::Clipboard(text) => say(&format!(
                    "clipboard {} bytes: {:?}{}",
                    text.len(),
                    head(text, 60),
                    if text.len() > 60 { "..." } else { "" }
                )),
                Event::CredentialsNeeded { needs_user } => {
                    say(&format!("credentials_needed (user {})", if *needs_user { "wanted" } else { "not wanted" }))
                }
                Event::PromptWithdrawn => say("prompt withdrawn"),
                Event::TrustNeeded { fingerprint } => say(&format!("trust_needed {fingerprint}")),
                Event::Unverified { why } => say(&format!("unverified: {why}")),
                Event::Question { title, text } => say(&format!("question {title:?}: {text}")),
                Event::SecurityNeeded {
                    identity,
                    encryption,
                    signature,
                    catchphrase,
                    matching_address,
                } => say(&format!(
                    "security_needed {identity:?}, {encryption:?}, {signature:?} ({catchphrase}){}",
                    if matching_address.is_empty() {
                        String::new()
                    } else {
                        format!(", known as {matching_address}")
                    }
                )),
                Event::Message {
                    text,
                    severity,
                    question,
                    confirm_label,
                } => say(&format!(
                    "message ({severity:?}) {text:?}{} {}",
                    if confirm_label.is_empty() {
                        String::new()
                    } else {
                        format!(" [{confirm_label}]")
                    },
                    if *question { "(question)" } else { "(ok)" }
                )),
                Event::Closed { detail } => {
                    say(&format!("closed{}{}", if detail.is_empty() { "" } else { ": " }, detail))
                }
            }
        }

        /// Answers a prompt from the flags, as probe.c does: the password
        /// once, then a cancel, and `--trust` to everything else.
        fn answer(&self, event: &Event, backend: &Arc<dyn Backend>, say: &dyn Fn(&str)) {
            match event {
                Event::CredentialsNeeded { .. } => match &self.password {
                    Some(_) if self.answered.get() => {
                        say("  -> asked again, so the password was refused; cancelling");
                        self.refused.set(true);
                        backend.answer_credentials(None);
                    }
                    Some(password) => {
                        self.answered.set(true);
                        say("  -> answering with the password");
                        backend.answer_credentials(Some(Credentials {
                            user: self.user.clone(),
                            domain: self.domain.clone(),
                            password: password.clone(),
                        }));
                    }
                    None => {
                        say("  -> no --password, cancelling");
                        backend.answer_credentials(None);
                    }
                },
                Event::TrustNeeded { fingerprint } => {
                    // The known hosts first, as the window has them: read,
                    // and never written, since `--trust` applies to this run
                    // alone.
                    let pinned = backends::pinned_as(self.kind, &self.address, &self.options);
                    let hosts = KnownHosts::in_dir(&self.dirs.config);
                    let (yes, why) = match trust::decide(&trust::ReadOnly(&hosts), &pinned, fingerprint) {
                        Decision::Matches => (true, format!("matches the pin for {}", pinned.key)),
                        Decision::FirstSight => {
                            (true, format!("first sight of {}, taken without asking (not pinned)", pinned.key))
                        }
                        Decision::Ask {
                            identity,
                            matching_address,
                        } => (
                            self.trust,
                            format!(
                                "{identity:?}{}, {}",
                                if matching_address.is_empty() {
                                    String::new()
                                } else {
                                    format!(" (pinned for {matching_address})")
                                },
                                if self.trust { "yes (--trust, not pinned)" } else { "no" }
                            ),
                        ),
                    };
                    say(&format!("  -> {why}"));
                    backend.answer_trust(yes);
                }
                Event::Unverified { .. } | Event::Question { .. } => {
                    say(&format!("  -> {}", if self.trust { "yes (--trust)" } else { "no" }));
                    backend.answer_trust(self.trust);
                }
                Event::SecurityNeeded { .. } => {
                    say(&format!("  -> {}", if self.trust { "yes (--trust)" } else { "no" }));
                    backend.answer_security(self.trust, false);
                }
                Event::Message { question, .. } => {
                    // A box with one button has one answer; a question gets
                    // --trust's.
                    let yes = !question || self.trust;
                    say(&format!("  -> {}", if yes { "ok/yes" } else { "no" }));
                    backend.answer_trust(yes);
                }
                _ => {}
            }
        }

        /// Runs one of `--probe`'s steps; whether it did what it says.
        fn step(&self, step: &Step, mailbox: &Mailbox, backend: &Arc<dyn Backend>, say: &dyn Fn(&str)) -> bool {
            match step {
                Step::WaitConnect(_) => unreachable!("wait-connect= is taken off the spec as the connect timeout"),
                Step::Wait(time) => {
                    self.pump(mailbox, backend, say, *time, |_| false);
                }
                &Step::Click(x, y) => {
                    say(&format!("pointer -> click at {x},{y}"));
                    backend.pointer(x, y, 0);
                    backend.pointer(x, y, 1);
                    backend.pointer(x, y, 0);
                }
                Step::Clipboard(text) => {
                    say(&format!("clipboard -> {text:?}"));
                    backend.clipboard(text);
                }
                &Step::Key(keysym) => {
                    say(&format!("key_down/key_up -> keysym {keysym:#x}"));
                    backend.key_down(keysym, 1);
                    backend.key_up(1);
                }
                &Step::Resize(w, h) => {
                    say(&format!("request_desktop_size -> {w}x{h}"));
                    backend.request_desktop_size(w, h);
                }
                Step::Layout(sizes) => self.layout(sizes, backend, say),
                Step::SoundCycle(every) => {
                    self.sound_cycle.set(Some(*every).filter(|e| !e.is_zero()));
                    self.sound_cycle_at.set(None);
                }
                Step::Out(path) => return self.out(path, backend, say),
                Step::Thumb(path, scale) => return self.thumb(path, *scale, backend, say),
            }
            true
        }

        /// `layout=`: the screens asked for side by side.
        fn layout(&self, sizes: &[(u32, u32)], backend: &Arc<dyn Backend>, say: &dyn Fn(&str)) {
            // The far end's screens keep their ids by position, as a
            // window placing its monitors left to right would hand them
            // back; one past them has an id the far end cannot have yet
            // (none has all 2^32), and the backend numbers it.
            let have = backend.facts().monitors;
            let fresh = (0..=u32::MAX).rev().find(|id| have.iter().all(|m| m.id != *id));
            let mut x = 0i32;
            let screens: Vec<Monitor> = sizes
                .iter()
                .enumerate()
                .map(|(i, &(w, h))| {
                    let id = have.get(i).map(|m| m.id).or(fresh).unwrap_or(0);
                    let m = Monitor {
                        id,
                        x,
                        y: 0,
                        width: w as i32,
                        height: h as i32,
                    };
                    x += w as i32;
                    m
                })
                .collect();
            let width = x.max(0) as u32;
            let height = sizes.iter().map(|s| s.1).max().unwrap_or(0);
            say(&format!(
                "request_layout -> {width}x{height}: {}",
                screens.iter().map(screen).collect::<Vec<_>>().join(", ")
            ));
            backend.request_layout(width, height, &screens);
        }

        fn facts(&self, backend: &Arc<dyn Backend>, say: &dyn Fn(&str)) {
            let facts = backend.facts();
            say(&format!(
                "facts {}x{}, {} monitors, {} displays (on {}){}{}",
                facts.width,
                facts.height,
                facts.monitors.len(),
                facts.displays.len(),
                facts.display,
                if facts.can_resize { ", resizable" } else { "" },
                if facts.pointer_relative { ", relative pointer" } else { "" }
            ));
            if facts.can_layout {
                say("facts: takes a layout");
            }
            for m in &facts.monitors {
                say(&format!("monitor {}", screen(m)));
            }
            for (i, fact) in backend.info().iter().enumerate() {
                say(&format!(
                    "info[{i}] {}{}: {}",
                    fact.label,
                    if fact.diagnostic { " (diag)" } else { "" },
                    fact.value
                ));
            }
            // The overlay's line, which says why a picture asked for in
            // hardware is in software.
            if let Some(status) = backend.decoder_status() {
                say(&format!("decoder {status}"));
            }
            match backend.traffic() {
                Some(t) => say(&format!(
                    "traffic {} in{}",
                    t.received,
                    t.sent.map(|s| format!(", {s} out")).unwrap_or_default()
                )),
                None => say("traffic: none yet"),
            }
        }

        /// `out=`: whether the framebuffer was written.
        fn out(&self, path: &std::path::Path, backend: &Arc<dyn Backend>, say: &dyn Fn(&str)) -> bool {
            let facts = backend.facts();
            let (width, height) = (facts.width, facts.height);
            if width == 0 || height == 0 {
                say("no framebuffer size to read");
                return false;
            }
            let mut rgba = vec![0u8; width * height * 4];
            if !backend.read_region(0, 0, width, height, &mut rgba, width * 4, 0, 0) {
                say(&format!("read_region {width}x{height} refused"));
                return false;
            }
            let picture = vento::backend::Picture { width, height, rgba };
            write_picture(path, &picture, &format!("read_region {width}x{height}"), say)
        }

        /// `thumb=`: whether the thumbnail was written.
        fn thumb(&self, path: &std::path::Path, step: usize, backend: &Arc<dyn Backend>, say: &dyn Fn(&str)) -> bool {
            let facts = backend.facts();
            if facts.width == 0 || facts.height == 0 {
                say("no framebuffer size to read");
                return false;
            }
            match backend.thumbnail(step) {
                Some(picture) => {
                    let what = format!("read_thumbnail 1/{step} {}x{}", picture.width, picture.height);
                    write_picture(path, &picture, &what, say)
                }
                None => {
                    say(&format!("read_thumbnail 1/{step} refused"));
                    false
                }
            }
        }
    }

    /// Writes `picture` to `path` as a PPM, saying so; whether it was written.
    fn write_picture(
        path: &std::path::Path,
        picture: &vento::backend::Picture,
        what: &str,
        say: &dyn Fn(&str),
    ) -> bool {
        match ppm::write(path, picture) {
            Ok(()) => {
                say(&format!("{what} -> {}", path.display()));
                true
            }
            Err(e) => {
                say(&format!("writing {}: {e}", path.display()));
                false
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn parsed(args: &[&str]) -> Result<Mode, String> {
            parse(cli::split(args.iter().map(|a| a.to_string()).collect(), cli::FLAGS)?)
        }

        fn launch(args: &[&str]) -> Box<gui::Launch> {
            match parsed(args) {
                Ok(Mode::Gui(launch)) => launch,
                Ok(_) => panic!("{args:?} is not the window"),
                Err(e) => panic!("{args:?}: {e}"),
            }
        }

        fn probe(args: &[&str]) -> Box<Probe> {
            match parsed(args) {
                Ok(Mode::Probe(probe)) => probe,
                Ok(_) => panic!("{args:?} is not the probe"),
                Err(e) => panic!("{args:?}: {e}"),
            }
        }

        #[test]
        fn hardware_decoding_is_off_unless_asked_and_decodes_as_video_says() {
            use backends::Hardware;
            let hardware = |args: &[&str]| launch(args).settings.hardware();
            assert_eq!(hardware(&[]), Hardware::Off);
            assert_eq!(hardware(&["--hw-decode"]), Hardware::Auto);
            assert_eq!(hardware(&["--hw-decode", "--video", "vaapi"]), Hardware::On);
            assert_eq!(hardware(&["--video=vaapi"]), Hardware::Off);
            assert_eq!(hardware(&["--video=off", "--hw-decode"]), Hardware::Off);
            assert_eq!(hardware(&["--video=vaapi", "--video=auto", "--hw-decode"]), Hardware::Auto);
            assert_eq!(launch(&["--video=vaapi"]).settings.video, Hardware::On);
            assert_eq!(
                probe(&["rustdesk", "127.0.0.1", "--probe", "wait=1", "--hw-decode", "--video=vaapi"]).hw_decode,
                Hardware::On
            );
            for refused in [&["--video="][..], &["--video", "VAAPI"], &["--video=nvdec"], &["--video"]] {
                assert!(parsed(refused).is_err(), "{refused:?}");
            }
            assert_eq!(
                parsed(&["--video=nvdec"]).err().unwrap(),
                "--video: \"nvdec\" is not one of auto, vaapi, or off"
            );
        }

        #[test]
        fn the_sound_plays_where_it_is_told_and_auto_otherwise() {
            use backends::AudioOutput;
            let output = |args: &[&str]| launch(args).settings.audio_output;
            assert_eq!(output(&[]), AudioOutput::Auto);
            assert_eq!(output(&["--audio", "alsa"]), AudioOutput::Alsa);
            assert_eq!(output(&["--audio=pipewire"]), AudioOutput::PipeWire);
            assert_eq!(output(&["--audio=off"]), AudioOutput::Off);
            assert_eq!(
                probe(&["tigervnc", "127.0.0.1", "--probe", "wait=1", "--audio=alsa", "--audio=auto"]).audio_output,
                AudioOutput::Auto
            );
            for refused in [&["--audio="][..], &["--audio=pulse"], &["--audio=PipeWire"], &["--audio"]] {
                assert!(parsed(refused).is_err(), "{refused:?}");
            }
            assert_eq!(
                parsed(&["--audio=pulse"]).err().unwrap(),
                "--audio: \"pulse\" is not one of auto, pipewire, alsa, or off"
            );
        }

        #[test]
        fn the_window_s_platform_cursor_and_scale_are_by_name() {
            use gui::{DisplayServer, ScaleMode};
            assert_eq!(launch(&[]).display_server, None);
            assert_eq!(launch(&["--platform", "x11"]).display_server, Some(DisplayServer::X11));
            assert_eq!(launch(&["--platform=wayland"]).display_server, Some(DisplayServer::Wayland));
            assert_eq!(launch(&["--platform=x11", "--platform=auto"]).display_server, None);
            assert!(parsed(&["--platform=X11"]).is_err());
            assert!(!launch(&[]).settings.software_cursor);
            assert!(launch(&["--cursor=software"]).settings.software_cursor);
            assert!(!launch(&["--cursor=software", "--cursor", "native"]).settings.software_cursor);
            assert!(parsed(&["--cursor=hardware"]).is_err());
            let scale = |args: &[&str]| {
                let settings = launch(args).settings;
                (settings.scale, settings.fit_window)
            };
            assert_eq!(scale(&[]), (ScaleMode::Auto, false));
            assert_eq!(scale(&["--scale", cli::SCALE]), scale(&[]));
            assert_eq!(scale(&["--scale=fit"]), (ScaleMode::Fit, false));
            assert_eq!(scale(&["--scale", "fill"]), (ScaleMode::Fill, false));
            assert_eq!(scale(&["--scale=fill", "--scale=auto"]), (ScaleMode::Auto, false));
            assert_eq!(scale(&["--scale=fit-width"]), (ScaleMode::FitWidth, false));
            assert_eq!(scale(&["--scale=fit-height"]), (ScaleMode::FitHeight, false));
            assert_eq!(scale(&["--scale=fill-width"]), (ScaleMode::FillWidth, false));
            assert_eq!(scale(&["--scale=fill-height"]), (ScaleMode::FillHeight, false));
            assert_eq!(scale(&["--scale=none"]), (ScaleMode::Original, false));
            assert_eq!(scale(&["--scale=none-resize"]), (ScaleMode::Original, true));
            assert_eq!(scale(&["--scale=none-resize", "--scale=none"]), (ScaleMode::Original, false));
            assert!(parsed(&["--scale=1:1"]).is_err());
        }

        #[test]
        fn the_session_options_are_the_settings_and_view_only_is_forced() {
            let settings = launch(&["--no-send-clipboard", "--no-recv-clipboard", "--no-grab-keyboard"]).settings;
            assert!(!settings.clipboard_out && !settings.clipboard_in && !settings.grab_keyboard);
            let settings = launch(&[]).settings;
            assert!(settings.clipboard_out && settings.clipboard_in && settings.grab_keyboard);
            assert!(!settings.view_only && !settings.trust);

            let forced = launch(&["--view-only", "rfb", "127.0.0.1:1", "-o", "ViewOnly=false"]);
            assert!(forced.settings.view_only);
            assert_eq!(forced.request.unwrap().options.get("ViewOnly"), "true");
            let probe = probe(&["rfb", "127.0.0.1:1", "-Pwait=1", "--view-only"]);
            assert_eq!(probe.options.get("ViewOnly"), "true");
        }

        #[test]
        fn trust_is_for_the_window_and_the_probe_and_the_steps_only_for_the_probe() {
            assert!(launch(&["--trust"]).settings.trust);
            assert!(launch(&["--trust", "-c", "rfb", "127.0.0.1:1"]).settings.trust);
            assert!(probe(&["rfb", "127.0.0.1:1", "--trust", "--probe=wait=1"]).trust);
            assert!(!probe(&["rfb", "127.0.0.1:1", "--probe=wait=1"]).trust);
            // The former probe flags are gone; their steps are in the spec.
            for flag in [
                "--out",
                "--out-thumb",
                "--thumb-step",
                "--click",
                "--key",
                "--clipboard",
                "--resize",
                "--connect-timeout",
            ] {
                assert_eq!(
                    parsed(&["rfb", "127.0.0.1:1", "-P", "wait=1", flag, "x"]).err(),
                    Some(format!("unknown option {flag}"))
                );
            }
            let p = probe(&["rfb", "127.0.0.1:1", "-P", "wait=3,out=a.ppm,thumb=b.ppm:4,clipboard='a, b'"]);
            assert_eq!(
                p.steps,
                [
                    Step::Wait(Duration::from_secs(3)),
                    Step::Out(PathBuf::from("a.ppm")),
                    Step::Thumb(PathBuf::from("b.ppm"), 4),
                    Step::Clipboard("a, b".into()),
                ]
            );
            assert_eq!(p.connect_timeout, Duration::from_secs(cli::CONNECT_TIMEOUT));
            // wait-connect= is the connect timeout, and not a step of its own.
            let p = probe(&["rfb", "127.0.0.1:1", "-P", "wait-connect=5"]);
            assert_eq!((p.connect_timeout, p.steps.len()), (Duration::from_secs(5), 0));
            let p = probe(&["rfb", "127.0.0.1:1", "-P", "wait-connect=0.5,wait=1"]);
            assert_eq!(p.connect_timeout, Duration::from_millis(500));
            assert_eq!(p.steps, [Step::Wait(Duration::from_secs(1))]);
            assert_eq!(
                parsed(&["rfb", "127.0.0.1:1", "-P", "wait=1,wait-connect=5"]).err().unwrap(),
                "--probe: wait-connect must be the first step"
            );
            assert_eq!(
                parsed(&["rfb", "127.0.0.1:1", "--probe", "3"]).err().unwrap(),
                "--probe: \"3\" is not a step (wait=3 waits 3 seconds)"
            );
            assert_eq!(
                parsed(&["rfb", "127.0.0.1:1", "--probe=wait=1,bogus=2"]).err().unwrap(),
                "--probe: unknown step \"bogus\""
            );
            assert_eq!(parsed(&["rfb", "127.0.0.1:1", "--probe"]).err().unwrap(), "--probe needs a value");
        }

        #[test]
        fn known_hosts_and_state_are_in_the_state_dir_and_realvnc_in_the_extlib_dir() {
            let dirs = launch(&["--state-dir", "/s", "--extlib-dir=/e"]).dirs;
            assert_eq!(dirs.config, PathBuf::from("/s"));
            assert_eq!(dirs.state, PathBuf::from("/s"));
            assert_eq!(dirs.realvnc_libs, PathBuf::from("/e/realvnc"));
            match parsed(&["-F", "host:1", "--state-dir=/s"]) {
                Ok(Mode::Forget { address, dirs }) => {
                    assert_eq!(address, "host:1");
                    assert_eq!(dirs.config, PathBuf::from("/s"));
                }
                _ => panic!("not --forget"),
            }
        }

        #[test]
        fn each_form_of_the_command_line_is_its_mode() {
            assert!(matches!(parsed(&["-h"]), Ok(Mode::Help)));
            assert!(parsed(&["rfb", "--help", "--x"]).is_err());
            assert!(matches!(parsed(&["rfb", "127.0.0.1:1", "-c", "--help"]), Ok(Mode::Help)));
            assert!(matches!(parsed(&["--version"]), Ok(Mode::Version)));
            assert!(matches!(parsed(&["rfb", "-cV"]), Ok(Mode::Version)));
            assert!(matches!(parsed(&["-V", "--help"]), Ok(Mode::Help)));
            assert!(matches!(parsed(&["-L", "-V"]), Ok(Mode::Version)));
            assert!(parsed(&["--version=1"]).is_err());
            assert!(matches!(parsed(&["--list"]), Ok(Mode::List)));
            assert!(matches!(parsed(&["-L", "rfb"]), Ok(Mode::List)));
            assert!(matches!(parsed(&["--forget=host:1"]), Ok(Mode::Forget { .. })));
            assert_eq!(
                parsed(&["-F", "host:1", "-c"]).err().unwrap(),
                "--forget only takes an address, and not other options"
            );
            assert!(launch(&[]).request.is_none());
            let dialog = launch(&["rfb"]);
            assert!(!dialog.connect && dialog.request.as_ref().unwrap().address.is_empty());
            let session = launch(&["-cume", "-pPW", "--domain", "D", "rfb", "127.0.0.1:1", "-oShared=false"]);
            assert!(session.connect);
            let request = session.request.unwrap();
            assert_eq!((request.user.as_str(), request.domain.as_str()), ("me", "D"));
            assert_eq!(request.password.as_deref(), Some("PW"));
            assert_eq!(request.options.get("Shared"), "false");
            assert_eq!(parsed(&["-c"]).err().unwrap(), "give a backend and an address");
            assert_eq!(parsed(&["-c", "rfb"]).err().unwrap(), "give a backend and an address");
            assert_eq!(parsed(&["-P", "wait=1", "rfb"]).err().unwrap(), "give a backend and an address");
            assert_eq!(parsed(&["nope", "h"]).err().unwrap(), "no backend \"nope\", see --list");
            assert_eq!(probe(&["-Pwait=5", "rfb", "127.0.0.1:1"]).address, "127.0.0.1:1");
            assert_eq!(parsed(&["--x11"]).err().unwrap(), "unknown option --x11");
        }

        #[test]
        fn a_clipboard_line_is_cut_between_characters() {
            let text = format!("{}é tail", "0".repeat(59));
            assert_eq!(head(&text, 60), "0".repeat(59));
            assert_eq!(head(&text, 61), format!("{}é", "0".repeat(59)));
            assert_eq!(head("short", 60), "short");
            assert_eq!(head("", 60), "");
        }
    }
}
