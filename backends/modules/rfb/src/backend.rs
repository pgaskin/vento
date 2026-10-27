// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The RFB client as a [`Backend`](api::backend::Backend), with this
//! backend's option handling and info rows. The Linux client uses it directly;
//! Android reaches it through `platform_android`'s JNI layer.

use crate::client::Screen;
use crate::{Client, Config, Security};
use runtime::{Bridge, Runner};
use api::backend::{Backend, ConnectionFact, Credentials, Facts, Field, Monitor, Picture, Traffic};
use api::credentials::fill_user;
use api::events::{Feed, Mailbox};
use api::kind::{CONNECT_TIMEOUT_MS, Connect, Kind};
use api::options::Options;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub static KIND: Kind = Kind {
    about: &api::catalog::RFB,
    version: || crate::PROTOCOL_VERSION.to_string(),
    create,
    forget: None,
};

struct Rfb {
    session: Runner<Client>,
    mailbox: Arc<Mailbox>,
    options: Mutex<Options>,
    user: String,
}

/// The `Encoding` option's value: one encoding, or for "auto" and anything
/// else all of them, best first, for the server to pick from.
fn encodings(name: &str) -> Vec<i32> {
    match name {
        "raw" => vec![crate::ENC_RAW],
        "rre" => vec![crate::ENC_RRE],
        "hextile" => vec![crate::ENC_HEXTILE],
        "zrle" => vec![crate::ENC_ZRLE],
        _ => vec![
            crate::ENC_ZRLE,
            crate::ENC_HEXTILE,
            crate::ENC_RRE,
        ],
    }
}

/// The compression option as the client takes it: 0–9, or none.
fn compress_level(value: &str) -> Option<u8> {
    value.parse::<i32>().ok().filter(|n| (0..=9).contains(n)).map(|n| n as u8)
}

fn create(c: &Connect) -> Result<Arc<dyn Backend>, String> {
    let o = c.options;
    let config = Config {
        address: c.address.to_string(),
        user_name: Some(c.user.to_string()),
        password: c.password.map(String::from),
        security: match o.get("Security") {
            "require" => Security::Require,
            "plain" => Security::Plain,
            _ => Security::Prefer,
        },
        shared: o.bool("Shared"),
        encodings: encodings(o.get("Encoding")),
        compress_level: compress_level(o.get("Compression")),
        connect_timeout: Duration::from_millis(CONNECT_TIMEOUT_MS as u64),
        network: Arc::clone(&c.network),
    };
    log::info!(
        "connecting to {} ({} encodings, compression {:?}, security {:?})",
        config.address,
        config.encodings.len(),
        config.compress_level,
        config.security
    );
    let session = Runner::start(
        Arc::new(Client::new()),
        config,
        Bridge::new(Feed(Arc::clone(&c.mailbox))),
    );
    session.client.set_view_only(o.bool("ViewOnly"));
    Ok(Arc::new(Rfb {
        session,
        mailbox: Arc::clone(&c.mailbox),
        options: Mutex::new(o.clone()),
        user: c.user.to_string(),
    }))
}

impl Backend for Rfb {
    fn pointer(&self, x: i32, y: i32, button_mask: i32) {
        // All nine buttons are passed on; the client drops the ninth on a
        // server that never agreed to it.
        self.session.client.pointer(
            x.clamp(0, 65535) as u16,
            y.clamp(0, 65535) as u16,
            (button_mask & 0x1ff) as u16,
        );
    }

    fn pointer_relative(&self, dx: i32, dy: i32, button_mask: i32) {
        self.session
            .client
            .pointer_relative(dx, dy, (button_mask & 0x1ff) as u16);
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
            .request_desktop_size(width.min(65535) as u16, height.min(65535) as u16);
    }

    /// A screen outside RFB's 16-bit range can't be expressed, so a layout
    /// containing one is not sent rather than sent wrapped.
    fn request_layout(&self, width: u32, height: u32, screens: &[Monitor]) {
        let u16 = |v: i64| u16::try_from(v).ok();
        let screens: Option<Vec<Screen>> = screens
            .iter()
            .map(|m| {
                Some(Screen {
                    id: m.id,
                    x: u16(m.x as i64)?,
                    y: u16(m.y as i64)?,
                    width: u16(m.width as i64)?,
                    height: u16(m.height as i64)?,
                    flags: 0,
                })
            })
            .collect();
        let size = (u16(width as i64), u16(height as i64));
        if let (Some(screens), (Some(width), Some(height))) = (screens, size) {
            self.session.client.request_layout(width, height, &screens);
        }
    }

    /// The picture-quality options apply live: RFB allows `SetEncodings` at
    /// any time, so this changes the current session. Shared is decided at
    /// ClientInit and the bell option is read when a bell arrives; neither
    /// needs a call.
    fn set_option(&self, key: &str, value: &str) {
        let mut options = self.options.lock().unwrap();
        options.set(key, value);
        match key {
            "ViewOnly" => self.session.client.set_view_only(options.bool(key)),
            "Encoding" | "Compression" => self.session.client.set_encodings(
                &encodings(options.get("Encoding")),
                compress_level(options.get("Compression")),
            ),
            _ => {}
        }
    }

    fn facts(&self) -> Facts {
        let (width, height) = self.mailbox.size();
        let connected = self.mailbox.connected();
        let client = &self.session.client;
        // Read whether or not the state is connected yet: a rectangle can
        // arrive before the state changes. Includes the server's ids, which a
        // layout request sends back.
        let flat: Vec<i32> = client
            .screens()
            .iter()
            .flat_map(|s| [s.id as i32, s.x as i32, s.y as i32, s.width as i32, s.height as i32])
            .collect();
        Facts {
            width,
            height,
            monitors: Monitor::from_flat5(&flat),
            displays: Vec::new(),
            display: -1,
            can_resize: connected && client.can_resize(),
            can_layout: connected && client.can_layout(),
            view_only: self.options.lock().unwrap().bool("ViewOnly"),
            pointer_relative: client.pointer_is_relative(),
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
            ConnectionFact::of(Field::Security, "Security", &i.security),
            ConnectionFact::of(Field::Encoding, "Encoding", &i.encoding),
            ConnectionFact::of(Field::LineSpeed, "Line speed", &i.line_speed),
        ];
        if let Some(traffic) = self.traffic() {
            facts.push(ConnectionFact::data(traffic));
        }
        facts.push(ConnectionFact::diagnostic(
            Field::ServerPixels,
            "Server pixels",
            &i.server_pixels,
        ));
        facts.push(ConnectionFact::diagnostic(
            Field::ViewerPixels,
            "Viewer pixels",
            &i.viewer_pixels,
        ));
        facts
    }

    fn traffic(&self) -> Option<Traffic> {
        let (received, sent) = self.session.client.traffic();
        Some(Traffic {
            received,
            sent: Some(sent),
        })
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
        let framebuffer = self.session.client.framebuffer().read().unwrap();
        framebuffer.read_rgba(x, y, width, height, &mut dst[start..], dst_stride)
    }

    fn thumbnail(&self, step: usize) -> Option<Picture> {
        let framebuffer = self.session.client.framebuffer().read().unwrap();
        let (w, h, words) = framebuffer.thumbnail(step.max(1))?;
        Some(Picture::from_words(w, h, &words))
    }

    fn answer_credentials(&self, credentials: Option<Credentials>) {
        self.session
            .answer_credentials(credentials.map(|c| fill_user(c, &self.user)));
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
