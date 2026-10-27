// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! One remote desktop session, whatever the protocol: the interface the
//! clients are written against.
//!
//! It is modelled on `Backend.java` without the Android-specific parts, since
//! that interface already fits all eight backends: the `Bitmap` becomes a
//! slice of `R, G, B, A` bytes, the main-thread listener becomes the
//! [`Mailbox`](crate::events::Mailbox) a session is created with, and the
//! asynchronous `connectionInfo` becomes a plain call. Every backend answers
//! it from state kept alongside the connection, including RealVNC, whose
//! session file asks the core on its own thread and waits.
//!
//! Threads: every method may be called from any thread, and [`read_region`]
//! and [`thumbnail`] are meant to be called from the drawing thread,
//! concurrently with everything else. They take the read side of a lock the
//! protocol thread holds for one blit at a time, so a pixel fetch never
//! queues behind a socket read. Events arrive through the mailbox, nearly
//! always from the protocol thread (a call into the session can queue one
//! too), and can be read from any thread.
//!
//! [`read_region`]: Backend::read_region
//! [`thumbnail`]: Backend::thumbnail

pub use runtime::Credentials;
use runtime::media::audio::Controls;

/// One of the far end's screens, as a rectangle in its coordinates.
///
/// Used for two different things: see [`Facts::monitors`] and
/// [`Facts::displays`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Monitor {
    /// The far end's opaque id for this screen, only for passing back in
    /// [`Backend::request_layout`]: RFB's screen id, which a server uses to
    /// match a requested layout against its screens. When the protocol has
    /// no ids, it is the screen's index in the list.
    pub id: u32,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Monitor {
    /// The format every backend passes its layout in: four ints per screen,
    /// x, y, width, height. One reader for all of them, so that every library
    /// gives the same result for an empty layout; a screen with no area is
    /// dropped. Each id is the screen's index in `flat`. The phone's JNI
    /// layer flattens a layout back into this format, the only one Java
    /// reads.
    pub fn from_flat(flat: &[i32]) -> Vec<Monitor> {
        flat.chunks_exact(4)
            .enumerate()
            .filter(|(_, m)| m[2] > 0 && m[3] > 0)
            .map(|(i, m)| Monitor {
                id: i as u32,
                x: m[0],
                y: m[1],
                width: m[2],
                height: m[3],
            })
            .collect()
    }

    /// The same with the far end's own ids: five ints per screen, id, x, y,
    /// width, height, with the id's bits unchanged. Used by a backend whose
    /// protocol numbers its screens, and for layout requests across the C
    /// interface.
    pub fn from_flat5(flat: &[i32]) -> Vec<Monitor> {
        flat.chunks_exact(5)
            .filter(|m| m[3] > 0 && m[4] > 0)
            .map(|m| Monitor {
                id: m[0] as u32,
                x: m[1],
                y: m[2],
                width: m[3],
                height: m[4],
            })
            .collect()
    }

    /// `screens` in the format [`from_flat5`](Monitor::from_flat5) reads.
    pub fn flat5(screens: &[Monitor]) -> Vec<i32> {
        screens
            .iter()
            .flat_map(|m| [m.id as i32, m.x, m.y, m.width, m.height])
            .collect()
    }
}

/// The state of a live session that is polled rather than pushed as events,
/// in one answer: `Backend.Facts` in Java, plus `can_layout`.
///
/// A caller must expect every one of these to change while a session runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Facts {
    /// Zero until the connection is up.
    pub width: usize,
    pub height: usize,
    /// How the far end's *one* desktop is divided, or empty when it has not
    /// sent a layout (which differs from one monitor, and is always the case
    /// for a library that never exposes the layout).
    pub monitors: Vec<Monitor>,
    /// The far end's displays, of which it sends only one at a time and
    /// sends another on request; empty for every protocol but RustDesk. A
    /// menu rather than a map: only [`display`](Facts::display) is the
    /// picture.
    pub displays: Vec<Monitor>,
    /// Which of `displays` is on screen, or -1 when there is no choice.
    pub display: i32,
    /// Whether the far end accepts a new single-screen desktop size right
    /// now: false for a multi-screen desktop, which a resize would merge.
    pub can_resize: bool,
    /// Whether the far end accepts a layout of any number of screens right
    /// now, through [`Backend::request_layout`]. Only true for a protocol
    /// that can place each screen.
    pub can_layout: bool,
    pub view_only: bool,
    /// Whether the far end owns the cursor. Also pushed as
    /// [`Event::PointerMode`](crate::events::Event::PointerMode); this is for
    /// a screen that attaches to a session that is already running.
    pub pointer_relative: bool,
}

impl Facts {
    /// Nothing connected: the answer for a session that is over.
    pub const NONE: Facts = Facts {
        width: 0,
        height: 0,
        monitors: Vec::new(),
        displays: Vec::new(),
        display: -1,
        can_resize: false,
        can_layout: false,
        view_only: false,
        pointer_relative: false,
    };
}

impl Default for Facts {
    fn default() -> Facts {
        Facts::NONE
    }
}

/// The meaning of a connection panel row, independent of the backend's
/// label for it: `ConnectionFact.Field` in Java.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    /// The desktop's name. The panel's heading.
    DesktopName,
    /// The transport: direct TCP, relayed, UDP.
    Connection,
    /// The encryption, if any.
    Security,
    /// The protocol and its version.
    Protocol,
    /// The encoding in use for pixels.
    Encoding,
    /// The measured throughput, however the backend measures it.
    LineSpeed,
    /// The data transferred both ways since the session opened.
    Data,
    /// The pixel format the server is sending.
    ServerPixels,
    /// The pixel format the client asked for.
    ViewerPixels,
    /// Anything protocol-specific not covered above.
    Other,
    /// What carries the connection where it is not this end's own TCP
    /// connection ([`runtime::net::Network::tunnel`]): its own group, last in
    /// the panel. Added by the client ([`with_network`]), not the backend.
    Tunnel,
}

/// One row of information a backend reports about its connection.
///
/// `field` is the shared meaning and `label` is the backend's own word for
/// the row, which is what goes on screen. A backend omits a field it has no
/// concept of, and includes one with an empty value when it has the concept
/// but no value yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectionFact {
    pub field: Field,
    pub label: &'static str,
    pub value: String,
    /// Only useful when debugging (the pixel formats, for example).
    pub diagnostic: bool,
}

impl ConnectionFact {
    pub fn of(field: Field, label: &'static str, value: &str) -> ConnectionFact {
        ConnectionFact {
            field,
            label,
            value: value.trim().to_string(),
            diagnostic: false,
        }
    }

    pub fn diagnostic(field: Field, label: &'static str, value: &str) -> ConnectionFact {
        ConnectionFact {
            diagnostic: true,
            ..ConnectionFact::of(field, label, value)
        }
    }

    /// [`Field::Data`], from the raw counts. The only row worded here rather
    /// than by the backend: the backends count in different places, and the
    /// row is only comparable if every backend words and rounds it the same
    /// way, so they pass bytes and this formats them. A direction the
    /// protocol has no count for is left out rather than shown as zero.
    pub fn data(traffic: Traffic) -> ConnectionFact {
        let value = match traffic.sent {
            Some(sent) => format!("{} in · {} out", size(traffic.received), size(sent)),
            None => format!("{} in", size(traffic.received)),
        };
        ConnectionFact::of(Field::Data, "Transferred", &value)
    }

    /// The session's sound, "AAC 44.1 kHz stereo", with the output where
    /// the platform has more than one ("(PipeWire)", as a video row says
    /// "(hardware)") and ", muted" while muted. The client produces this row
    /// rather than the backend, since the client holds the sound's controls
    /// (the backend has only the streams it writes), and it is worded here
    /// so that both clients word it the same. `None` while nothing plays;
    /// the session's sound switch, shown from when the far end may send
    /// sound, covers the rest.
    pub fn sound(controls: &Controls) -> Option<ConnectionFact> {
        let playback = controls.playback()?;
        let output = playback.output.map(|o| format!(" ({o})")).unwrap_or_default();
        let muted = if controls.muted() { ", muted" } else { "" };
        Some(ConnectionFact::of(
            Field::Other,
            "Sound",
            &format!("{}{output}{muted}", playback.describe()),
        ))
    }
}

impl ConnectionFact {
    /// The [`Field::Tunnel`] row for a session on `network`: the route's own
    /// description ("dev-socketpair"), or `None` where the connections are
    /// this end's own.
    pub fn tunnel(network: &dyn runtime::net::Network) -> Option<ConnectionFact> {
        let text = network.tunnel()?;
        Some(ConnectionFact::of(Field::Tunnel, "Tunnel", &text))
    }
}

/// Adds the session's [tunnel row](ConnectionFact::tunnel), if any, to the
/// end of `info`, after the diagnostics: the panels show it as a group of its
/// own, last.
pub fn with_network(info: &mut Vec<ConnectionFact>, network: &dyn runtime::net::Network) {
    if let Some(row) = ConnectionFact::tunnel(network) {
        info.push(row);
    }
}

/// Adds the session's [sound row](ConnectionFact::sound), if any, to a
/// backend's rows in `info`: after the backend's own rows and before its
/// diagnostics, the order the panels show them in.
pub fn with_sound(info: &mut Vec<ConnectionFact>, controls: &Controls) {
    if let Some(row) = ConnectionFact::sound(controls) {
        let at = info.iter().position(|f| f.diagnostic).unwrap_or(info.len());
        info.insert(at, row);
    }
}

/// Bytes in human-readable form: powers of a thousand, with decimals only
/// where the mantissa is small enough for them to matter. SI rather than
/// binary, because network quantities are conventionally quoted in
/// thousands.
fn size(bytes: u64) -> String {
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    const UNITS: [&str; 4] = ["kB", "MB", "GB", "TB"];
    let mut value = bytes as f64 / 1000.0;
    let mut unit = 0;
    while value >= 999.5 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if value < 9.995 {
        format!("{value:.2} {}", UNITS[unit])
    } else if value < 99.95 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.0} {}", UNITS[unit])
    }
}

/// The data the session has transferred, in bytes, counted between the
/// protocol and its transport. `sent` is `None` for a backend with no count
/// for that direction (RealVNC's core counts only one way), rather than a
/// misleading zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Traffic {
    pub received: u64,
    pub sent: Option<u64>,
}

/// A picture, `width × height` pixels of `R, G, B, A` bytes, tightly packed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picture {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

impl Picture {
    /// From the framebuffer's own words, which are `R, G, B, A` in memory
    /// (the little-endian `0xAABBGGRR` every client here uses).
    pub fn from_words(width: usize, height: usize, words: &[u32]) -> Picture {
        Picture {
            width,
            height,
            rgba: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
        }
    }
}

/// The step for a thumbnail no bigger than `max_width × max_height`. Always
/// an integer, since every backend here takes an integer step.
pub fn thumbnail_step(width: usize, height: usize, max_width: usize, max_height: usize) -> usize {
    let max_width = max_width.max(1);
    let max_height = max_height.max(1);
    width
        .div_ceil(max_width)
        .max(height.div_ceil(max_height))
        .max(1)
}

/// One remote desktop connection, whatever protocol is underneath.
///
/// Starts connecting when created (there is no `connect()`, since nothing
/// here needs a backend to exist before it has an address) and is over when
/// the mailbox reports `Closed`. `destroy` ends it and waits for its thread;
/// it takes `&self` so that a shared handle can be destroyed from anywhere,
/// and is idempotent so that it can be called twice.
pub trait Backend: Send + Sync {
    /// Absolute desktop coordinates and an RFB button mask.
    fn pointer(&self, x: i32, y: i32, button_mask: i32);

    /// The same event where the far end owns the cursor: the movement
    /// rather than the position. Only meaningful while
    /// [`Facts::pointer_relative`]; other backends ignore it.
    fn pointer_relative(&self, dx: i32, dy: i32, button_mask: i32) {
        let _ = (dx, dy, button_mask);
    }

    /// `keysym` is an X11 keysym; `key_id` is a stable identity for the
    /// physical key, which pairs a key up with its key down.
    fn key_down(&self, keysym: u32, key_id: i64);

    fn key_up(&self, key_id: i64);

    /// Release every key this session still holds down at the far end.
    fn release_all_keys(&self);

    /// Whether the session is visible: false while its window is minimised
    /// or hidden (wholly covered on X11, suspended by the compositor on
    /// Wayland), true again when it is shown. Backends may pause updates
    /// while it is not. Not the keyboard focus: a visible window without
    /// focus stays live. (The name comes from the Android app, where the
    /// session's screen going to the background is the equivalent.)
    fn focus(&self, focused: bool);

    fn set_view_only(&self, view_only: bool);

    /// Offers this end's copied text to the far end.
    fn clipboard(&self, text: &str);

    /// Whether a clipboard offered now would reach anything at the far end,
    /// if the session can know: `Some(false)` means it would go nowhere, as
    /// with SPICE when no guest agent accepts one. `None`, the default, is
    /// for a protocol that sends the clipboard as a message with no
    /// feedback. Not in `Backend.java`, since the app does not use it; the
    /// probe's status does.
    fn carries_clipboard(&self) -> Option<bool> {
        None
    }

    /// Ask the far end to make the desktop this size. The result arrives as
    /// a desktop size event, or not at all.
    fn request_desktop_size(&self, width: u32, height: u32) {
        let _ = (width, height);
    }

    /// Ask the far end to make the desktop `width × height`, divided into
    /// `screens`. Like [`request_desktop_size`], the result arrives as a
    /// desktop size and a new [`Facts::monitors`], or not at all.
    ///
    /// A screen's id is one from [`Facts::monitors`], passed back so that
    /// the far end changes that screen rather than replacing it; an unknown
    /// id, or one already used earlier in the list, is a new screen, and the
    /// backend assigns its id. The default, for a protocol that cannot send
    /// a layout ([`Facts::can_layout`] false), requests the size when there
    /// is one screen and does nothing otherwise.
    ///
    /// [`request_desktop_size`]: Backend::request_desktop_size
    fn request_layout(&self, width: u32, height: u32, screens: &[Monitor]) {
        if screens.len() == 1 {
            self.request_desktop_size(width, height);
        }
    }

    /// Ask the far end to send another of its displays.
    fn request_display(&self, index: i32) {
        let _ = index;
    }

    /// Change one of this backend's options on the running session. Only
    /// meaningful for a live option; anything else is at best remembered for
    /// the next connection.
    fn set_option(&self, key: &str, value: &str);

    fn facts(&self) -> Facts;

    /// The backend's information rows about the live connection, in display
    /// order, or empty when nothing is connected.
    fn info(&self) -> Vec<ConnectionFact>;

    /// `None` while there is nothing to count.
    fn traffic(&self) -> Option<Traffic>;

    /// How the picture is being decoded, for a debug overlay, when the
    /// session decodes video through the platform's
    /// [`Decoders`](runtime::media::video::Decoders): the decoder's own
    /// [`status`](runtime::media::video::Decoder::status) line, as of roughly
    /// the last picture. `None` for a protocol whose picture is not video,
    /// and before the first frame.
    fn decoder_status(&self) -> Option<String> {
        None
    }

    /// The picture in GPU memory, when the backend's last picture was left
    /// there for the renderer on that GPU: for a session whose decoders were
    /// told the window accepts such pictures
    /// ([`Decoders::gpu_pictures`](runtime::media::video::Decoders::gpu_pictures)),
    /// and only while the newest picture is one. The frame events report
    /// changes as for any other picture. `read_region` and `thumbnail` still
    /// work by reading the picture back first, which is the cost this
    /// avoids. `None` for every other backend and picture.
    fn gpu_picture(&self) -> Option<std::sync::Arc<dyn runtime::media::video::GpuPicture>> {
        None
    }

    /// Copy the desktop rectangle `(x, y, width, height)` into `dst` at
    /// `(dst_x, dst_y)`, 1:1, where `dst` is rows of `R, G, B, A` bytes
    /// `dst_stride` bytes apart. The rest of `dst` is left unchanged, so a
    /// caller can hold a picture bigger than any one read and refresh only
    /// the changed parts. False if there is nothing to read yet, or if the
    /// rectangle does not fit either buffer, in which case `dst` is
    /// untouched.
    #[allow(clippy::too_many_arguments)]
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
    ) -> bool;

    /// The whole desktop at `1/step`, nearest neighbour, or `None` if there
    /// is nothing to show yet. [`thumbnail_step`] turns a bound into a step.
    fn thumbnail(&self, step: usize) -> Option<Picture>;

    /// The answer to a credentials event. `None` cancels, which ends the
    /// session. An empty user name means the one the session was created
    /// with.
    fn answer_credentials(&self, credentials: Option<Credentials>);

    /// The answer to a trust, unverified, question, security or message
    /// event. A no ends the session, except for a message that was not a
    /// question, where it only dismisses the message.
    fn answer_trust(&self, accept: bool);

    /// [`answer_trust`](Backend::answer_trust) for a security event, plus
    /// whether to remember the identity. Only RealVNC supports remembering;
    /// the other backends ignore `remember`.
    fn answer_security(&self, accept: bool, remember: bool) {
        let _ = remember;
        self.answer_trust(accept);
    }

    /// An identity the library asked to have remembered, for the caller to
    /// store and pass to the next session: RealVNC's opaque blob, which its
    /// core sends after a security prompt answered with *remember*. The
    /// caller decides whether to store it: the window does, the probe never
    /// asks.
    fn saved_identity(&self) -> Option<String> {
        None
    }

    /// Ask for a graceful disconnect. `Closed` follows.
    fn disconnect(&self);

    /// Ends the session, waits for its thread, and releases whatever the
    /// native side is holding. Nothing but `destroy` may be called afterwards.
    fn destroy(&self);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_the_way_the_java_row_reads() {
        assert_eq!(size(0), "0 B");
        assert_eq!(size(999), "999 B");
        assert_eq!(size(1000), "1.00 kB");
        assert_eq!(size(12_345), "12.3 kB");
        assert_eq!(size(123_456), "123 kB");
        assert_eq!(size(999_500), "1.00 MB");
        assert_eq!(size(2_500_000_000), "2.50 GB");
    }

    #[test]
    fn the_data_row_leaves_out_a_direction_that_was_not_counted() {
        let both = ConnectionFact::data(Traffic {
            received: 1500,
            sent: Some(20),
        });
        assert_eq!(both.value, "1.50 kB in · 20 B out");
        assert_eq!(both.label, "Transferred");
        let one = ConnectionFact::data(Traffic {
            received: 1500,
            sent: None,
        });
        assert_eq!(one.value, "1.50 kB in");
    }

    #[test]
    fn the_sound_row_goes_before_the_diagnostics_while_sound_plays() {
        use runtime::media::audio::{Device, Format, NoOutputs, Outputs, Queue};
        use std::sync::Arc;

        struct Quiet;
        impl Device for Quiet {}
        /// The session's controls, and the output name its devices report.
        struct Out(Arc<Controls>, Option<&'static str>);
        impl Outputs for Out {
            fn controls(&self) -> &Arc<Controls> {
                &self.0
            }
            fn device(&self, queue: Arc<Queue>) -> Result<Box<dyn Device>, String> {
                if let Some(output) = self.1 {
                    queue.set_output(output);
                }
                Ok(Box::new(Quiet))
            }
        }

        let rows = || {
            vec![
                ConnectionFact::of(Field::Protocol, "Protocol", "RDP"),
                ConnectionFact::diagnostic(Field::Other, "Pixels", "32 bpp"),
            ]
        };
        let out = Out(Arc::new(Controls::new()), None);
        let mut info = rows();
        with_sound(&mut info, out.controls());
        assert_eq!(info, rows(), "nothing playing, no row");
        assert!(ConnectionFact::sound(NoOutputs::new().controls()).is_none());

        let mut stream = out.open(Format::new(48_000, 2).unwrap()).unwrap();
        stream.write(&[0, 0]);
        with_sound(&mut info, out.controls());
        assert_eq!(info[1].label, "Sound");
        assert_eq!(info[1].value, "PCM 48 kHz stereo");
        assert!(!info[1].diagnostic);
        assert_eq!(info[2].label, "Pixels");
        out.controls().set_muted(true);
        assert_eq!(ConnectionFact::sound(out.controls()).unwrap().value, "PCM 48 kHz stereo, muted");
        // A platform that reports which output plays it (the desktop's
        // PipeWire or ALSA) has it shown, like a decoder's "(hardware)".
        let piped = Out(Arc::clone(&out.0), Some("PipeWire"));
        let mut later = piped.open(Format::new(44_100, 2).unwrap()).unwrap();
        later.write(&[0, 0]);
        assert_eq!(
            ConnectionFact::sound(out.controls()).unwrap().value,
            "PCM 44.1 kHz stereo (PipeWire), muted"
        );
    }

    #[test]
    fn monitors_without_area_are_not_monitors() {
        let flat = [0, 0, 1920, 1200, 1920, 0, 0, 1200, 1920, 0, 1280, 800, 7];
        let monitors = Monitor::from_flat(&flat);
        assert_eq!(monitors.len(), 2);
        assert_eq!(monitors[1].x, 1920);
        assert_eq!(monitors[1].width, 1280);
        // An id is the index in the library's list, including gaps.
        assert_eq!(monitors[0].id, 0);
        assert_eq!(monitors[1].id, 2);
    }

    #[test]
    fn five_ints_a_screen_carry_the_far_ends_own_ids() {
        let screens = [
            Monitor {
                id: 0x8000_0001,
                x: 0,
                y: 0,
                width: 1280,
                height: 800,
            },
            Monitor {
                id: 7,
                x: 1280,
                y: 0,
                width: 1024,
                height: 768,
            },
        ];
        let flat = Monitor::flat5(&screens);
        assert_eq!(flat.len(), 10);
        assert_eq!(Monitor::from_flat5(&flat), screens);
        // And a screen with no area is dropped, as in four ints.
        assert_eq!(Monitor::from_flat5(&[3, 0, 0, 0, 800, 9]), []);
    }

    #[test]
    fn a_thumbnail_step_fits_both_bounds() {
        assert_eq!(thumbnail_step(1920, 1200, 240, 240), 8);
        assert_eq!(thumbnail_step(1920, 1200, 1000, 100), 12);
        assert_eq!(thumbnail_step(100, 100, 1000, 1000), 1);
        assert_eq!(thumbnail_step(0, 0, 0, 0), 1);
    }

    /// `0xAABBGGRR` as a little-endian word is `R, G, B, A` in memory.
    #[test]
    fn words_come_out_in_their_memory_order() {
        let p = Picture::from_words(2, 1, &[0x4433_2211, 0]);
        assert_eq!(p.rgba, [0x11, 0x22, 0x33, 0x44, 0, 0, 0, 0]);
    }
}
