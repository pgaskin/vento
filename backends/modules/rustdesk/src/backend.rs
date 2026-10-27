// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The RustDesk client as a [`Backend`](api::backend::Backend), with this
//! backend's option handling and info rows. Used directly by the Linux client,
//! and by the phone through `platform_android`'s JNI layer. Video is decoded
//! by the decoders the caller passes in [`Connect::decoders`]: libavcodec on
//! Linux, MediaCodec on the phone.

use crate::{Client, Config, Platform, PreferredCodec, Quality, Reach};
use runtime::{Bridge, Runner};
use api::backend::{
    Backend, ConnectionFact, Credentials, Facts, Field, Monitor, Picture, Traffic,
};
use api::events::{Feed, Mailbox};
use api::kind::{self, CONNECT_TIMEOUT_MS, Connect, Kind};
use api::options::Options;
use runtime::media::video::GpuPicture;
use std::sync::{Arc, Mutex, RwLockReadGuard};
use std::time::Duration;

use crate::framebuffer::Framebuffer;

pub static KIND: Kind = Kind {
    about: &api::catalog::RUSTDESK,
    version: || crate::VERSION.to_string(),
    create,
    forget: None,
};

/// The `Codec` option's value, as the catalog defines it.
fn codec_of(name: &str) -> PreferredCodec {
    match name {
        "vp9" => PreferredCodec::Vp9,
        "vp8" => PreferredCodec::Vp8,
        "av1" => PreferredCodec::Av1,
        "h264" => PreferredCodec::H264,
        "h265" => PreferredCodec::H265,
        _ => PreferredCodec::Auto,
    }
}

/// The `Quality` option's value, as the catalog defines it.
fn quality_of(name: &str) -> Quality {
    match name {
        "low" => Quality::Low,
        "best" => Quality::Best,
        _ => Quality::Balanced,
    }
}

/// The frame rate option as the client takes it: 0 lets the peer choose.
fn fps(options: &Options) -> i32 {
    options.int("FrameRate", 0).clamp(0, 120) as i32
}

struct RustDesk {
    session: Runner<Client>,
    mailbox: Arc<Mailbox>,
    options: Mutex<Options>,
}

fn create(c: &Connect) -> Result<Arc<dyn Backend>, String> {
    let o = c.options;
    let by_id = o.get("ConnectBy") == "id";
    let config = Config {
        address: c.address.to_string(),
        reach: if by_id {
            Reach::Id {
                server: o.get("RendezvousServer").to_string(),
                key: o.get("RendezvousKey").to_string(),
            }
        } else {
            Reach::Direct
        },
        password: c.password.map(String::from),
        connect_timeout: Duration::from_millis(CONNECT_TIMEOUT_MS as u64),
        // The name the far end shows and logs for this session.
        my_name: c.client_name.to_string(),
        // What RustDesk's own client reports on the same platform.
        platform: match c.platform {
            kind::Platform::Android => Platform::Android,
            kind::Platform::Linux => Platform::Linux,
        },
        quality: quality_of(o.get("Quality")),
        fps: fps(o),
        codec: codec_of(o.get("Codec")),
        lock_after: o.bool("LockAfterSession"),
        audio: c.audio.clone(),
        sound: o.bool("Audio"),
        network: Arc::clone(&c.network),
    };
    log::info!(
        "connecting to {} {} ({:?}, {} fps, sound {})",
        config.address,
        if by_id { "by id" } else { "directly" },
        config.quality,
        config.fps,
        match (&config.audio, config.sound) {
            (None, _) => "nowhere to play",
            (Some(_), true) => "on",
            (Some(_), false) => "off, until switched on",
        }
    );
    let decoders = c.decoders.clone().ok_or("no video decoder to hand the picture to")?;
    let client = Client::new(decoders);
    let session = Runner::start(
        Arc::new(client),
        config,
        Bridge::new(Feed(Arc::clone(&c.mailbox))),
    );
    session.client.set_view_only(o.bool("ViewOnly"));
    Ok(Arc::new(RustDesk {
        session,
        mailbox: Arc::clone(&c.mailbox),
        options: Mutex::new(o.clone()),
    }))
}

impl RustDesk {
    /// The framebuffer to read, first updated with any picture still on the
    /// GPU (see `Framebuffer::catch_up`). A picture that arrives between the
    /// two locks is picked up by the next read.
    fn framebuffer(&self) -> RwLockReadGuard<'_, Framebuffer> {
        let framebuffer = self.session.client.framebuffer();
        if framebuffer.read().unwrap().behind() {
            framebuffer.write().unwrap().catch_up();
        }
        framebuffer.read().unwrap()
    }
}

impl Backend for RustDesk {
    /// The client works out which changes are a move, a press, a release or
    /// a wheel notch.
    fn pointer(&self, x: i32, y: i32, button_mask: i32) {
        self.session.client.pointer(x, y, button_mask & 0x1ff);
    }

    fn key_down(&self, keysym: u32, key_id: i64) {
        self.session.client.key_down(keysym, key_id);
    }

    fn key_up(&self, key_id: i64) {
        self.session.client.key_up(key_id);
    }

    fn release_all_keys(&self) {
        self.session.client.release_all_keys();
    }

    /// There is no pause message in this protocol, so this asks the far end
    /// for one frame a second instead.
    fn focus(&self, focused: bool) {
        self.session.client.set_focused(focused);
    }

    fn set_view_only(&self, view_only: bool) {
        self.set_option("ViewOnly", if view_only { "true" } else { "false" });
    }

    fn clipboard(&self, text: &str) {
        self.session.client.clipboard(text);
    }

    fn request_desktop_size(&self, width: u32, height: u32) {
        self.session
            .client
            .request_desktop_size(width.min(i32::MAX as u32) as i32, height.min(i32::MAX as u32) as i32);
    }

    /// The peer answers with a switch message, which may name a different
    /// display than the one requested.
    fn request_display(&self, index: i32) {
        self.session.client.request_display(index);
    }

    /// The picture-quality controls are live: one `Misc::option` message,
    /// which the peer applies from the next frame.
    fn set_option(&self, key: &str, value: &str) {
        let mut options = self.options.lock().unwrap();
        options.set(key, value);
        match key {
            "ViewOnly" => self.session.client.set_view_only(options.bool(key)),
            "Quality" | "FrameRate" | "Codec" => self.session.client.set_options(
                quality_of(options.get("Quality")),
                fps(&options),
                codec_of(options.get("Codec")),
            ),
            _ => {}
        }
    }

    /// The displays and the current index come from one call, so they stay
    /// consistent if a display arrives in between.
    fn facts(&self) -> Facts {
        let (width, height) = self.mailbox.size();
        let connected = self.mailbox.connected();
        let client = &self.session.client;
        let (flat, current) = if connected {
            client.displays()
        } else {
            (Vec::new(), -1)
        };
        Facts {
            width,
            height,
            monitors: Vec::new(),
            display: if flat.is_empty() { -1 } else { current },
            displays: Monitor::from_flat(&flat),
            can_resize: connected && client.can_resize(),
            can_layout: false,
            view_only: self.options.lock().unwrap().bool("ViewOnly"),
            pointer_relative: false,
        }
    }

    fn info(&self) -> Vec<ConnectionFact> {
        if !self.mailbox.connected() {
            return Vec::new();
        }
        let i = self.session.client.info();
        let mut facts = vec![
            ConnectionFact::of(Field::DesktopName, "Desktop", &i.desktop_name),
            ConnectionFact::of(Field::Protocol, "Protocol", &i.protocol),
            ConnectionFact::of(Field::Connection, "Connection", &i.connection),
            // Matters most for this backend: there is no encryption in direct
            // mode and no option to enable it.
            ConnectionFact::of(Field::Security, "Security", &i.security),
            ConnectionFact::of(Field::Encoding, "Encoding", &i.encoding),
            // The peer's timing message measures a round trip rather than a
            // rate, so it is not the line speed row.
            ConnectionFact::of(Field::Other, "Round trip", &i.round_trip),
        ];
        if let Some(traffic) = self.traffic() {
            facts.push(ConnectionFact::data(traffic));
        }
        facts.push(ConnectionFact::diagnostic(Field::Other, "Peer", &i.platform));
        if !i.display.is_empty() {
            facts.push(ConnectionFact::diagnostic(Field::Other, "Display", &i.display));
        }
        facts
    }

    fn traffic(&self) -> Option<Traffic> {
        let (received, sent) = self.session.client.traffic();
        Some(Traffic {
            received,
            sent: Some(sent),
        })
    }

    fn decoder_status(&self) -> Option<String> {
        if !self.mailbox.connected() {
            return None;
        }
        self.session.client.decoder_status()
    }

    fn gpu_picture(&self) -> Option<Arc<dyn GpuPicture>> {
        self.session.client.gpu_picture()
    }

    fn read_region(
        &self,
        x: usize,
        y: usize,
        width: usize,
        height: usize,
        dst: &mut [u8],
        dst_stride: usize,
        dst_x: usize,
        dst_y: usize,
    ) -> bool {
        let start = dst_y * dst_stride + dst_x * 4;
        if start > dst.len() {
            return false;
        }
        let framebuffer = self.framebuffer();
        framebuffer.read_rgba(x, y, width, height, &mut dst[start..], dst_stride)
    }

    fn thumbnail(&self, step: usize) -> Option<Picture> {
        let framebuffer = self.framebuffer();
        let (w, h, words) = framebuffer.thumbnail(step.max(1))?;
        Some(Picture::from_words(w, h, &words))
    }

    /// There is no user name in this protocol: a peer asks only for a
    /// password, and the rest of the answer is left blank.
    fn answer_credentials(&self, credentials: Option<Credentials>) {
        self.session.answer_credentials(credentials.map(|c| Credentials {
            password: c.password,
            ..Credentials::default()
        }));
    }

    fn answer_trust(&self, accept: bool) {
        self.session.answer_trust(accept);
    }

    fn disconnect(&self) {
        self.session.disconnect();
    }

    fn destroy(&self) {
        self.session.destroy();
    }
}
