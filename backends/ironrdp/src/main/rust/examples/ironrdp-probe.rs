// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! Connect to a server, print what it says, and write the first frame out.
//!
//! The protocol crate's own test rig, and `rfb`'s probe line for line so that
//! the two can be compared: it runs on the host against `scripts/testrdp`, so
//! "does the desktop decode?" is a question with a picture for an answer
//! instead of a phone.
//!
//! ```sh
//! scripts/testrdp/run.sh
//! cargo run --example rdp-probe -- 127.0.0.1:3389 proto protopass /tmp/rdp.ppm
//! RDP_PIN=AB:CD:… cargo run --example rdp-probe -- …    # refuse anything else
//! RDP_NLA=off RDP_SIZE=1280x800 cargo run --example rdp-probe -- …
//! RDP_CLIP='hello' cargo run --example rdp-probe -- …    # offer it, print theirs
//! RDP_RFX=off RDP_COMPRESSION=rdp61 cargo run --example rdp-probe -- …
//! ```

use remotedesktop_ironrdp::{Client, Config, Credentials, Handler, Nla};
use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

struct Probe {
    user: String,
    password: String,
    domain: String,
    /// What the certificate has to hash to, if anything is being insisted on.
    pin: Option<String>,
    frames: usize,
    damage: usize,
    cursors: usize,
}

impl Handler for Probe {
    fn connected(&mut self, width: usize, height: usize) {
        println!("connected: {width}x{height}");
    }

    fn desktop_size(&mut self, width: usize, height: usize) {
        println!("desktop size: {width}x{height}");
    }

    fn damaged(&mut self, _x: usize, _y: usize, _w: usize, _h: usize) {
        self.damage += 1;
    }

    fn frame_end(&mut self) {
        self.frames += 1;
    }

    fn cursor(&mut self, _pixels: &[u32], width: usize, height: usize, hot_x: i32, hot_y: i32) {
        self.cursors += 1;
        if self.cursors <= 3 {
            println!("cursor: {width}x{height} hotspot {hot_x},{hot_y}");
        }
    }

    fn bell(&mut self) {
        println!("bell");
    }

    fn clipboard(&mut self, text: &str) {
        println!("clipboard: {} chars: {text:?}", text.chars().count());
    }

    fn credentials(&mut self, needs_user: bool) -> Option<Credentials> {
        println!("credentials asked for (user name: {needs_user})");
        Some(Credentials {
            user: self.user.clone(),
            domain: self.domain.clone(),
            password: self.password.clone(),
        })
    }

    fn trust(&mut self, fingerprint: &str) -> bool {
        println!("certificate: {fingerprint}");
        match &self.pin {
            // The rig's stand-in for the app's pin store, so that "a changed
            // certificate is refused" is testable from a shell.
            Some(pin) => {
                let ok = pin.eq_ignore_ascii_case(fingerprint);
                println!(
                    "pinned:      {pin} ({})",
                    if ok { "matches" } else { "DOES NOT MATCH" }
                );
                ok
            }
            None => true,
        }
    }
}

fn main() {
    env_logger_lite();

    let mut args = std::env::args().skip(1);
    let address = args.next().unwrap_or_else(|| "127.0.0.1:3389".into());
    let user = args.next().unwrap_or_else(|| "proto".into());
    let password = args.next().unwrap_or_else(|| "protopass".into());
    let out = args.next().unwrap_or_else(|| "/tmp/rdp-frame.ppm".into());

    let pin = std::env::var("RDP_PIN").ok().filter(|p| !p.is_empty());
    let domain = std::env::var("RDP_DOMAIN").unwrap_or_default();
    let nla = match std::env::var("RDP_NLA").as_deref() {
        Ok("off") => Nla::Off,
        Ok("require") => Nla::Require,
        _ => Nla::Prefer,
    };
    let desktop_size = std::env::var("RDP_SIZE")
        .ok()
        .and_then(|s| {
            let (w, h) = s.split_once('x')?;
            Some((w.parse().ok()?, h.parse().ok()?))
        })
        .unwrap_or((1920, 1200));
    let seconds: u64 = std::env::var("RDP_WAIT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);

    let client = Arc::new(Client::new());
    let config = Config {
        address,
        user_name: Some(user.clone()),
        domain: Some(domain.clone()),
        password: Some(password.clone()),
        desktop_size,
        nla,
        compression: remotedesktop_ironrdp::compression(
            std::env::var("RDP_COMPRESSION").ok().as_deref(),
        ),
        remote_fx: std::env::var("RDP_RFX").as_deref() != Ok("off"),
        ..Config::default()
    };

    let clip = std::env::var("RDP_CLIP").ok().filter(|c| !c.is_empty());

    // A few seconds of whatever the desktop is doing, then look at it. Longer
    // than the RFB probe's two, because an xrdp session has a window manager
    // and four X clients to start before there is anything to see.
    let watcher = {
        let client = Arc::clone(&client);
        std::thread::spawn(move || {
            if let Some(text) = clip {
                // A second in, so the channel's opening exchange is over and
                // this is a copy rather than part of the handshake.
                std::thread::sleep(Duration::from_secs(1));
                println!("offering {} chars to the remote", text.chars().count());
                client.clipboard(&text);
            }
            std::thread::sleep(Duration::from_secs(seconds));
            let fb = client.framebuffer().read().unwrap();
            let (w, h) = (fb.width(), fb.height());
            if w == 0 {
                println!("no framebuffer");
                return;
            }
            let mut rgba = vec![0u8; w * h * 4];
            assert!(fb.read_rgba(0, 0, w, h, &mut rgba, w * 4), "read_rgba");
            drop(fb);

            let mut file = std::fs::File::create(&out).expect("creating the output");
            write!(file, "P6\n{w} {h}\n255\n").unwrap();
            let rgb: Vec<u8> = rgba
                .chunks_exact(4)
                .flat_map(|p| [p[0], p[1], p[2]])
                .collect();
            file.write_all(&rgb).unwrap();
            println!("wrote {out} ({w}x{h})");

            let info = client.info();
            println!("protocol:     {}", info.protocol);
            println!("connection:   {}", info.connection);
            println!("security:     {}", info.security);
            println!("line speed:   {}", info.line_speed);
            println!("server px:    {}", info.server_pixels);
            println!("viewer px:    {}", info.viewer_pixels);
            client.close();
        })
    };

    let mut probe = Probe {
        user,
        password,
        domain,
        pin,
        frames: 0,
        damage: 0,
        cursors: 0,
    };
    match client.run(&config, &mut probe) {
        Ok(()) => println!("ended"),
        Err(e) => println!("ended: {e}"),
    }
    println!(
        "{} frames, {} rectangles, {} cursors",
        probe.frames, probe.damage, probe.cursors
    );
    watcher.join().ok();
}

/// IronRDP logs through `tracing`, which forwards to `log`; on the phone that
/// is `android_logger` and here it is this. Fifteen lines rather than a
/// dependency, since the probe is a rig and not a program.
fn env_logger_lite() {
    struct Stderr;
    impl log::Log for Stderr {
        fn enabled(&self, metadata: &log::Metadata) -> bool {
            metadata.level() <= log::max_level()
        }
        fn log(&self, record: &log::Record) {
            if self.enabled(record.metadata()) {
                eprintln!("[{}] {}: {}", record.level(), record.target(), record.args());
            }
        }
        fn flush(&self) {}
    }
    let level = match std::env::var("RDP_LOG").as_deref() {
        Ok("trace") => log::LevelFilter::Trace,
        Ok("debug") => log::LevelFilter::Debug,
        Ok("off") => log::LevelFilter::Off,
        _ => log::LevelFilter::Info,
    };
    static LOGGER: Stderr = Stderr;
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(level);
}
