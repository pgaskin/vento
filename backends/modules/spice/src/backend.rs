// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The SPICE client as a [`Backend`](api::backend::Backend), with its option
//! handling and connection panel rows. The Linux client uses it directly, and
//! Android reaches it through `platform_android`'s JNI layer.

use crate::{Client, Config, compression_of};
use runtime::{Bridge, Runner};
use api::backend::{
    Backend, ConnectionFact, Credentials, Facts, Field, Monitor, Picture, Traffic,
};
use api::events::{Feed, Mailbox};
use api::kind::{CONNECT_TIMEOUT_MS, Connect, Kind};
use api::options::Options;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub static KIND: Kind = Kind {
    about: &api::catalog::SPICE,
    version: || crate::VERSION.to_string(),
    create,
    forget: None,
};

struct Spice {
    session: Runner<Client>,
    mailbox: Arc<Mailbox>,
    options: Mutex<Options>,
}

fn create(c: &Connect) -> Result<Arc<dyn Backend>, String> {
    let o = c.options;
    let tls = o.get("Security") == "tls";
    let config = Config {
        address: c.address.to_string(),
        tls,
        password: c.password.map(String::from),
        connect_timeout: Duration::from_millis(CONNECT_TIMEOUT_MS as u64),
        compression: compression_of(o.get("Compression")),
        view_only: o.bool("ViewOnly"),
        // The guest's sound, if there is an audio output: the session's
        // switch opens and closes the playback channel, and the option sets
        // whether it starts open.
        audio: c.audio.clone(),
        sound: o.bool("Audio"),
        network: Arc::clone(&c.network),
    };
    log::info!(
        "connecting to {} ({}, sound {})",
        config.address,
        if tls { "TLS" } else { "plain" },
        match (&config.audio, config.sound) {
            (None, _) => "nowhere to play",
            (Some(_), true) => "on",
            (Some(_), false) => "off, until switched on",
        }
    );
    let session = Runner::start(
        Arc::new(Client::new()),
        config,
        Bridge::new(Feed(Arc::clone(&c.mailbox))),
    );
    Ok(Arc::new(Spice {
        session,
        mailbox: Arc::clone(&c.mailbox),
        options: Mutex::new(o.clone()),
    }))
}

impl Backend for Spice {
    fn pointer(&self, x: i32, y: i32, button_mask: i32) {
        self.session.client.pointer(x, y, button_mask & 0x1ff);
    }

    /// For a session where the guest owns the cursor, the only mode a QEMU
    /// with no tablet attached offers.
    fn pointer_relative(&self, dx: i32, dy: i32, button_mask: i32) {
        self.session
            .client
            .pointer_relative(dx, dy, button_mask & 0x1ff);
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

    /// There is no pause message in this protocol, so this stops acking
    /// messages, and the server stops sending once the ack window is full.
    fn focus(&self, focused: bool) {
        self.session.client.set_focused(focused);
    }

    fn set_view_only(&self, view_only: bool) {
        self.set_option("ViewOnly", if view_only { "true" } else { "false" });
    }

    /// Offered to the guest agent; the text itself is sent when the guest
    /// requests it, as the agent protocol works.
    fn clipboard(&self, text: &str) {
        self.session.client.set_clipboard(text.to_string());
    }

    /// Only through the guest agent, once it reports clipboard support; the
    /// panel's "Guest agent" row shows the same.
    fn carries_clipboard(&self) -> Option<bool> {
        Some(self.session.client.has_clipboard())
    }

    fn request_desktop_size(&self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            self.session.client.request_desktop_size(width, height);
        }
    }

    /// Screens are matched to the guest's heads by id; the agent message
    /// identifies a head by its position in the list (see
    /// `client::heads_for`). The size is unused: the guest sizes the desktop
    /// to the layout's bounding box.
    fn request_layout(&self, _width: u32, _height: u32, screens: &[Monitor]) {
        let screens: Vec<(u32, i32, i32, u32, u32)> = screens
            .iter()
            .filter(|m| m.width > 0 && m.height > 0)
            .map(|m| (m.id, m.x, m.y, m.width as u32, m.height as u32))
            .collect();
        self.session.client.request_layout(&screens);
    }

    /// The image compression is live: the server uses it from the next image
    /// it encodes.
    fn set_option(&self, key: &str, value: &str) {
        let mut options = self.options.lock().unwrap();
        options.set(key, value);
        match key {
            "ViewOnly" => self.session.client.set_view_only(options.bool(key)),
            "Compression" => {
                if let Some(compression) = compression_of(value) {
                    self.session.client.set_compression(compression);
                }
            }
            _ => {}
        }
    }

    fn facts(&self) -> Facts {
        let (width, height) = self.mailbox.size();
        let connected = self.mailbox.connected();
        let client = &self.session.client;
        let monitors = if connected {
            Monitor::from_flat5(&client.screens())
        } else {
            Vec::new()
        };
        // A single size is one head, which on a guest with several turns the
        // rest off; a layout sets each head.
        let takes = connected && client.can_resize();
        Facts {
            width,
            height,
            can_resize: takes && monitors.len() <= 1,
            can_layout: takes,
            monitors,
            displays: Vec::new(),
            display: -1,
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
            ConnectionFact::of(Field::DesktopName, "Machine", &i.desktop_name),
            ConnectionFact::of(Field::Protocol, "Protocol", &i.protocol),
            ConnectionFact::of(Field::Connection, "Connection", &i.connection),
            ConnectionFact::of(Field::Security, "Security", &i.security),
        ];
        if !i.encoding.is_empty() {
            facts.push(ConnectionFact::of(Field::Encoding, "Encoding", &i.encoding));
        }
        if let Some(traffic) = self.traffic() {
            facts.push(ConnectionFact::data(traffic));
        }
        facts.push(ConnectionFact::of(Field::Other, "Channels", &i.channels));
        facts.push(ConnectionFact::of(Field::Other, "Guest agent", &i.agent));
        if !i.display.is_empty() {
            facts.push(ConnectionFact::diagnostic(Field::Other, "Picture", &i.display));
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

    /// SPICE has no user name, only a ticket, so the other fields are
    /// cleared.
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
