// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! Connect to a server, print what it says, and write the first frame out.
//!
//! The protocol crate's own test rig: it runs on the host against
//! `scripts/testvnc`, so "does ZRLE decode?" is a question with a picture for
//! an answer instead of a phone.
//!
//! ```sh
//! scripts/testvnc/run.sh
//! cargo run --example rfb-probe -- 127.0.0.1:5901 vncpass /tmp/frame.ppm
//! cargo run --example rfb-probe -- 127.0.0.1:5901 vncpass /tmp/frame.ppm hextile
//! ```
//!
//! The name carries the protocol because the workspace has one of these per
//! protocol and an example's output file is named after the example alone.
//!
//! The last argument names the encoding to ask for, which is how the ones the
//! server would never pick on its own — Raw, RRE, Hextile — get exercised
//! against a real server rather than only against the unit tests' hand-written
//! bytes.

use remotedesktop_rfb::{Client, Config, Credentials, Handler, Security};
use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

struct Probe {
    user: String,
    password: Option<String>,
    /// What the certificate has to hash to, if anything is being insisted on.
    pin: Option<String>,
    frames: usize,
    damage: usize,
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
        println!("cursor: {width}x{height} hotspot {hot_x},{hot_y}");
    }

    fn bell(&mut self) {
        println!("bell");
    }

    fn clipboard(&mut self, text: &str) {
        println!("clipboard: {} chars", text.chars().count());
    }

    fn pointer_mode(&mut self, relative: bool) {
        println!(
            "pointer mode: {}",
            if relative { "relative" } else { "absolute" }
        );
    }

    fn credentials(&mut self, needs_user: bool) -> Option<Credentials> {
        println!("credentials asked for (user name: {needs_user})");
        Some(Credentials {
            user: self.user.clone(),
            password: self.password.clone()?,
        })
    }

    fn trust(&mut self, fingerprint: &str) -> bool {
        println!("certificate: {fingerprint}");
        match &self.pin {
            // The rig's stand-in for the app's pin store, so that "a changed
            // certificate is refused" is testable from a shell.
            Some(pin) => {
                let ok = pin.eq_ignore_ascii_case(fingerprint);
                println!("pinned:      {pin} ({})", if ok { "matches" } else { "DOES NOT MATCH" });
                ok
            }
            None => true,
        }
    }
}

/// Which characters a US keyboard puts behind Shift. The rig types shell
/// commands, and a redirection nobody can type is a rig that cannot ask the
/// guest anything.
fn needs_shift(c: char) -> bool {
    c.is_ascii_uppercase() || "~!@#$%^&*()_+{}|:\"<>?".contains(c)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let address = args.next().unwrap_or_else(|| "127.0.0.1:5901".into());
    let password = args.next();
    let out = args.next().unwrap_or_else(|| "/tmp/rfb-frame.ppm".into());
    let encodings = match args.next().as_deref() {
        Some("raw") => vec![remotedesktop_rfb::ENC_RAW],
        Some("rre") => vec![remotedesktop_rfb::ENC_RRE],
        Some("hextile") => vec![remotedesktop_rfb::ENC_HEXTILE],
        Some("zrle") => vec![remotedesktop_rfb::ENC_ZRLE],
        Some(other) => panic!("unknown encoding {other}"),
        None => Config::default().encodings,
    };

    // The environment rather than more positional arguments: these are the
    // knobs a TLS session needs and none of them is wanted twice in a row.
    let security = match std::env::var("RFB_SECURITY").as_deref() {
        Ok("require") => Security::Require,
        Ok("plain") => Security::Plain,
        _ => Security::Prefer,
    };
    let user = std::env::var("RFB_USER").unwrap_or_default();
    let pin = std::env::var("RFB_PIN").ok().filter(|p| !p.is_empty());

    let client = Arc::new(Client::new());
    let config = Config {
        address,
        password,
        user_name: Some(user.clone()),
        security,
        encodings,
        ..Config::default()
    };

    // `RFB_MOVE=dx,dy,count` walks the pointer before the picture is taken, in
    // whichever coordinates the server asked for. The point is that the
    // *command* is the same for both: the same walk against `testvnc` (which is
    // absolute) and `testqemu` (which is relative) should move the pointer the
    // same distance, and the screenshot is what says whether it did.
    let walk = std::env::var("RFB_MOVE").ok().map(|s| {
        let mut it = s.split(',').map(|n| n.trim().parse::<i32>().expect("RFB_MOVE"));
        (
            it.next().unwrap_or(0),
            it.next().unwrap_or(0),
            it.next().unwrap_or(1),
        )
    });

    // `RFB_TYPE=...` types a line into whatever has the far end's focus, `\n`
    // included. A shell on the other side of it makes the whole guest
    // scriptable through the client being tested, which is what `testqemu`
    // needs: there is no way in to a virtual machine except its screen.
    let typing = std::env::var("RFB_TYPE").ok();

    // Two seconds of whatever the desktop is doing, then look at it.
    let watcher = {
        let client = Arc::clone(&client);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(2));
            if let Some(text) = typing {
                println!("typing {} characters", text.chars().count());
                for (i, ch) in text.replace("\\n", "\n").chars().enumerate() {
                    // A keysym for Latin-1 is the code point itself, and the
                    // one control character worth typing has a name.
                    let keysym = match ch {
                        '\n' => 0xff0d,
                        c => c as u32,
                    };
                    let id = i as i64;
                    // Shift has to be sent, not implied: QEMU's server turns a
                    // keysym into a *scancode*, so `*` with no Shift held
                    // arrives as `8`. The same thing the RDP client has to do
                    // when a layout needs Shift for a character.
                    let shifted = needs_shift(ch);
                    if shifted {
                        client.key_down(0xffe1, -1);
                    }
                    client.key_down(keysym, id);
                    std::thread::sleep(Duration::from_millis(12));
                    client.key_up(id);
                    if shifted {
                        client.key_up(-1);
                    }
                    std::thread::sleep(Duration::from_millis(12));
                }
                // Long enough for a command to have run and drawn something.
                std::thread::sleep(Duration::from_secs(3));
            }
            if let Some((dx, dy, count)) = walk {
                let relative = client.pointer_is_relative();
                println!(
                    "walking {count} × ({dx},{dy}), {}",
                    if relative { "relative" } else { "absolute" }
                );
                // Somewhere with room to move in either direction, for the
                // absolute case; the relative one starts wherever the far end's
                // pointer already is — and must not be sent a position at all,
                // since to a server in relative mode `(600, 400)` is a delta of
                // (−32167, −32367) and the pointer ends up in the corner.
                let (mut x, mut y) = (600i32, 400i32);
                if !relative {
                    client.pointer(x as u16, y as u16, 0);
                }
                for _ in 0..count {
                    if relative {
                        client.pointer_relative(dx, dy, 0);
                    } else {
                        x += dx;
                        y += dy;
                        client.pointer(x.max(0) as u16, y.max(0) as u16, 0);
                    }
                    // Roughly a frame apart, as a finger's moves arrive.
                    std::thread::sleep(Duration::from_millis(16));
                }
                std::thread::sleep(Duration::from_millis(500));
            }
            // `RFB_CLICK=256` presses and releases a button mask where it is.
            // 128 is button 8 and 256 is button 9 — the one that needs
            // `ExtendedMouseButtons` to have anywhere to go.
            if let Ok(spec) = std::env::var("RFB_CLICK") {
                // `mask` or `mask@x,y` — the position matters because what an
                // X server does with a button depends on the window under it.
                let (mask, at) = match spec.split_once('@') {
                    Some((m, pos)) => {
                        let mut it = pos.split(',').map(|n| n.trim().parse::<u16>().unwrap());
                        (m, Some((it.next().unwrap(), it.next().unwrap())))
                    }
                    None => (spec.as_str(), None),
                };
                let mask: u16 = mask.trim().parse().expect("RFB_CLICK");
                println!("clicking mask {mask} at {at:?}");
                if client.pointer_is_relative() {
                    client.pointer_relative(0, 0, mask);
                    std::thread::sleep(Duration::from_millis(100));
                    client.pointer_relative(0, 0, 0);
                } else {
                    let (x, y) = at.unwrap_or((600, 400));
                    client.pointer(x, y, 0);
                    std::thread::sleep(Duration::from_millis(100));
                    client.pointer(x, y, mask);
                    std::thread::sleep(Duration::from_millis(100));
                    client.pointer(x, y, 0);
                }
                std::thread::sleep(Duration::from_millis(500));
            }
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
            println!("desktop name: {}", info.desktop_name);
            println!("protocol:     {}", info.protocol);
            println!("connection:   {}", info.connection);
            println!("security:     {}", info.security);
            println!("encoding:     {}", info.encoding);
            println!("line speed:   {}", info.line_speed);
            println!("server px:    {}", info.server_pixels);
            println!("viewer px:    {}", info.viewer_pixels);
            println!(
                "pointer:      {}",
                if client.pointer_is_relative() {
                    "relative"
                } else {
                    "absolute"
                }
            );
            println!(
                "buttons:      {}",
                if client.has_extended_buttons() {
                    "9 (ExtendedMouseButtons)"
                } else {
                    "8"
                }
            );
            client.close();
        })
    };

    let mut probe = Probe {
        user,
        password: config.password.clone(),
        pin,
        frames: 0,
        damage: 0,
    };
    match client.run(&config, &mut probe) {
        Ok(()) => println!("ended"),
        Err(e) => println!("ended: {e}"),
    }
    println!("{} frames, {} rectangles", probe.frames, probe.damage);
    watcher.join().ok();
}
