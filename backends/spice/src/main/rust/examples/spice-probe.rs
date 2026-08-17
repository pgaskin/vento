// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

/*
 * This client, driven off a phone: the same `Client` the JNI half creates,
 * against a real server, with the events printed and the picture written out.
 *
 * The 46a probe (`scripts/measure/spice-probe`) asked whether the crates work;
 * this one asks whether *this* session layer does, which is a different
 * question and the one that has to be re-asked after every change here.
 *
 *   cargo run -p remotedesktop-spice --example spice-probe -- 10.33.0.200:5930 spicepass
 *
 * Environment:
 *   SPICE_SECONDS  how long to stay connected (default 5)
 *   SPICE_TLS      dial the TLS port, pinning whatever certificate arrives
 *   SPICE_OUT      where to write the picture (default /tmp/spice-backend.ppm)
 *   SPICE_KEYS     type this string at the guest, one key at a time
 *   SPICE_MOVE     `dx,dy`, sent ten times, which is how the pointer is checked
 *                  against a guest that draws its own cursor
 *   SPICE_COMPRESS what to ask the server to compress images with, which is
 *                  the one live option this backend has
 *   SPICE_CHORD    keysyms pressed together and released in reverse, as
 *                  `0xffe3,0xffe1` — the way a modifier is checked against a
 *                  guest that shows what it did with it
 *   SPICE_PAUSE    measure what a session costs on screen, off it and back on,
 *                  which is the whole of what this protocol's ack window can
 *                  be made to do instead of a pause message
 *   SPICE_COPY     offer this text to the guest's clipboard, which needs an
 *                  agent in there and does nothing without one
 *   SPICE_RESIZE   `1280x800`, asked of the same agent
 *   SPICE_HEADS    a layout rather than a size, `1024x768+0+0,800x600+1024+0`,
 *                  which is how a second monitor is made to exist
 */
use remotedesktop_spice::{Client, Config, Handler};
use std::sync::Arc;
use std::time::Duration;

/// The client logs where a phone would have logcat, and a picture with a hole
/// in it is a warning rather than a failure — so the probe needs somewhere for
/// those to go, and it is four lines rather than a dependency.
struct Stderr;

impl log::Log for Stderr {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        eprintln!("[{}] {}", record.level(), record.args());
    }

    fn flush(&self) {}
}

struct Printer {
    damage: u32,
    frames: u32,
    password: Option<String>,
}

impl Handler for Printer {
    fn connected(&mut self, width: usize, height: usize) {
        println!("connected: {width}x{height}");
    }

    fn desktop_size(&mut self, width: usize, height: usize) {
        println!("desktop: {width}x{height}");
    }

    fn damaged(&mut self, _x: usize, _y: usize, _width: usize, _height: usize) {
        self.damage += 1;
    }

    fn frame_end(&mut self) {
        self.frames += 1;
    }

    fn cursor(&mut self, pixels: &[u32], width: i32, height: i32, hot_x: i32, hot_y: i32) {
        println!("cursor: {width}x{height} at ({hot_x},{hot_y}), {} pixels", pixels.len());
    }

    fn pointer_mode(&mut self, relative: bool) {
        println!("pointer: {}", if relative { "the guest's" } else { "ours" });
    }

    fn password(&mut self) -> Option<String> {
        println!("the ticket was refused");
        self.password.take()
    }

    fn trust(&mut self, fingerprint: &str) -> bool {
        println!("certificate {fingerprint}");
        true
    }

    fn clipboard(&mut self, text: &str) {
        println!("clipboard from the guest: {} bytes, {text:?}", text.len());
    }
}

fn main() {
    log::set_logger(&Stderr).expect("the logger");
    log::set_max_level(log::LevelFilter::Debug);

    let args: Vec<String> = std::env::args().collect();
    let address = args.get(1).cloned().unwrap_or_else(|| "127.0.0.1:5930".into());
    let password = args.get(2).cloned().filter(|p| !p.is_empty());
    let seconds: u64 = std::env::var("SPICE_SECONDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);
    let out = std::env::var("SPICE_OUT").unwrap_or_else(|_| "/tmp/spice-backend.ppm".into());

    let client = Arc::new(Client::new());
    let config = Config {
        address: address.clone(),
        tls: std::env::var("SPICE_TLS").is_ok(),
        password: password.clone(),
        connect_timeout: Duration::from_secs(10),
        compression: std::env::var("SPICE_COMPRESS")
            .ok()
            .and_then(|name| remotedesktop_spice::compression_of(&name)),
        view_only: false,
    };

    let session = {
        let client = Arc::clone(&client);
        std::thread::spawn(move || {
            let mut printer = Printer {
                damage: 0,
                frames: 0,
                password,
            };
            let result = client.run(&config, &mut printer);
            println!(
                "session ended: {}",
                result.err().map_or("disconnected".into(), |e| e.to_string())
            );
            (printer.damage, printer.frames)
        })
    };

    // A pointer move that nets to nothing, which is also what wakes a guest
    // whose screen saver has started.
    std::thread::sleep(Duration::from_millis(1500));
    client.pointer(100, 100, 0);
    client.pointer_relative(1, 0, 0);
    client.pointer_relative(-1, 0, 0);
    if let Ok(move_by) = std::env::var("SPICE_MOVE") {
        let (dx, dy) = move_by.split_once(',').unwrap_or(("0", "0"));
        let (dx, dy): (i32, i32) = (dx.parse().unwrap_or(0), dy.parse().unwrap_or(0));
        for _ in 0..10 {
            client.pointer(200 + dx, 200 + dy, 0);
            client.pointer_relative(dx, dy, 0);
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    if let Ok(chord) = std::env::var("SPICE_CHORD") {
        let keysyms: Vec<u32> = chord
            .split(',')
            .filter_map(|k| u32::from_str_radix(k.trim().trim_start_matches("0x"), 16).ok())
            .collect();
        for (i, keysym) in keysyms.iter().enumerate() {
            client.key_down(*keysym, 100 + i as i64);
            std::thread::sleep(Duration::from_millis(60));
        }
        for i in (0..keysyms.len()).rev() {
            client.key_up(100 + i as i64);
            std::thread::sleep(Duration::from_millis(60));
        }
    }
    if let Ok(keys) = std::env::var("SPICE_KEYS") {
        for (i, ch) in keys.chars().enumerate() {
            let keysym = ch as u32;
            client.key_down(keysym, i as i64);
            std::thread::sleep(Duration::from_millis(40));
            client.key_up(i as i64);
        }
    }

    // The two the guest's agent carries, and nothing else here has a far end
    // that can be asked for either.
    if let Ok(text) = std::env::var("SPICE_COPY") {
        client.set_clipboard(text);
    }
    // More than one head, which nothing above the crate asks for and which is
    // the only way to make a second one exist: a QXL device reports one until a
    // client says otherwise.
    if let Ok(heads) = std::env::var("SPICE_HEADS") {
        let monitors: Vec<(i32, i32, u32, u32)> = heads
            .split(',')
            .filter_map(|head| {
                let (size, at) = head.split_once('+')?;
                let (width, height) = size.split_once('x')?;
                let (x, y) = at.split_once('+')?;
                Some((
                    x.parse().ok()?,
                    y.parse().ok()?,
                    width.parse().ok()?,
                    height.parse().ok()?,
                ))
            })
            .collect();
        println!("heads: asking for {monitors:?}");
        client.request_monitors(&monitors);
    }
    if let Ok(size) = std::env::var("SPICE_RESIZE") {
        let (width, height) = size.split_once('x').unwrap_or(("0", "0"));
        let (width, height) = (width.parse().unwrap_or(0), height.parse().unwrap_or(0));
        println!("resize: the guest {} take one", if client.can_resize() { "will" } else { "will not" });
        client.request_desktop_size(width, height);
    }

    if std::env::var("SPICE_PAUSE").is_ok() {
        let window = Duration::from_secs(seconds.max(10));
        let over = |client: &Client, was: u64| client.traffic().0 - was;
        let at = client.traffic().0;
        std::thread::sleep(window);
        println!("on screen:    {} bytes in {seconds}s", over(&client, at));
        client.set_focused(false);
        let at = client.traffic().0;
        std::thread::sleep(window);
        println!("off screen:   {} bytes in {seconds}s", over(&client, at));
        client.set_focused(true);
        let at = client.traffic().0;
        std::thread::sleep(window);
        println!("back:         {} bytes in {seconds}s", over(&client, at));
    }

    std::thread::sleep(Duration::from_secs(seconds));
    let info = client.info();
    println!("desktop name: {}", info.desktop_name);
    println!("protocol:     {}", info.protocol);
    println!("connection:   {}", info.connection);
    println!("security:     {}", info.security);
    println!("encoding:     {}", info.encoding);
    println!("channels:     {}", info.channels);
    println!("agent:        {}", info.agent);
    println!("display:      {}", info.display);
    let (received, sent) = client.traffic();
    println!("traffic:      {received} in, {sent} out");
    println!("monitors:     {:?}", client.monitors());
    println!("pointer:      {}", if client.pointer_is_relative() { "guest" } else { "ours" });

    write_ppm(&client, &out);
    client.close();
    let (damage, frames) = session.join().expect("the session thread");
    println!("{damage} damaged rectangles in {frames} frames");
}

fn write_ppm(client: &Client, path: &str) {
    let framebuffer = client.framebuffer().read().unwrap();
    let (width, height) = (framebuffer.width(), framebuffer.height());
    if width == 0 {
        println!("nothing was drawn");
        return;
    }
    let mut rgba = vec![0u8; width * height * 4];
    assert!(framebuffer.read_rgba(0, 0, width, height, &mut rgba, width * 4));
    let mut out = format!("P6\n{width} {height}\n255\n").into_bytes();
    for px in rgba.chunks_exact(4) {
        out.extend_from_slice(&px[..3]);
    }
    std::fs::write(path, out).expect("writing the picture");
    println!("{path}: {width}x{height}");
}
