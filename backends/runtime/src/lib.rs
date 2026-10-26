// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The neutral layer: what a protocol client is written against, and what a
//! platform is written to, with neither knowing the other.
//!
//! A protocol crate implements [`Client`] and calls a [`Handler`]. A platform
//! implements [`Callbacks`] (the outward, non-blocking half of the same
//! conversation) and gets a [`Runner`] that runs the client on its own thread,
//! with a [`Bridge`] in between that turns the two questions a protocol can
//! ask (credentials, trust) into an event and a wait on a [`Slot`]. The shape
//! of `Callbacks` is the shape of the C API's `vento_callbacks` and of the
//! Java `NativeBackend`'s `on…` methods: it is the one interface every
//! platform has to fit, so it is the one place this crate prescribes a shape.
//!
//! A protocol that uses TLS uses it through [`tls`]: the certificate pinned
//! rather than verified, and one connection read and written by two threads.
//!
//! The picture and the sound a session carries are [`media`]'s: the video
//! decoders and sound outputs a platform supplies by implementing its two
//! traits ([`media::video::Decoders`], [`media::audio::Outputs`]), and what is
//! done with their planes and samples, the same for every protocol and both
//! platforms. It and the conversation above are independent of each other.
//!
//! What is not here: anything with `jni` in it, which is
//! `remotedesktop-platform-android`.

pub mod address;
pub mod callbacks;
pub mod client;
pub mod cursor;
pub mod handler;
pub mod hangup;
pub mod media;
pub mod rate;
pub mod runner;
pub mod slot;
pub mod status;
pub mod text;
pub mod tls;

pub use callbacks::Callbacks;
pub use client::Client;
pub use handler::{Asked, Credentials, Handler, Withdraw};
pub use hangup::Hangup;
pub use runner::{Bridge, Runner};
pub use slot::Slot;

/// The most pixels a server may say its desktop has, for a backend whose
/// framebuffer is its own to allocate (RFB's and SPICE's). Not a limit on what
/// a client can draw (the Linux client tiles a desktop past its GPU's texture
/// limit, and this is three times the largest desktop tested, with an 8K
/// screen below it) but on what a pair of fields off the wire may cost before
/// the claim is treated as a protocol error: a framebuffer is four bytes a
/// pixel, 128 MiB at this bound, and is allocated as soon as the size arrives.
pub const MAX_DESKTOP_PIXELS: usize = 32 << 20;

/// The widest and tallest cursor a backend takes from the far end, which is
/// libvncclient's bound (TigerVNC's viewer uses 256). A cursor's size is two
/// fields from the server with no desktop to check them against, and its
/// pixels are allocated from them, so past this the shape is refused before
/// anything is allocated: at the bound it is 4 MiB, where two 16-bit fields
/// would allow 17 GB. SPICE limits its own to 256, the largest a guest draws.
pub const MAX_CURSOR_SIDE: usize = 1024;

/// The most of the far end's clipboard a backend takes, in bytes of text as
/// it arrives (after any compression the protocol puts over it, and as the
/// UTF-8 it becomes where the protocol's text is UTF-16, as RDP's is). A
/// larger clipboard is skipped rather than truncated, since part of a text
/// pasted here would look like the whole of it, and rather than treated as an
/// error, since a large copy at the far end is ordinary and no reason to end
/// the session. The two RDP backends apply it to what they offer the far end
/// too, and FreeRDP's C has it as `VENTO_MAX_CLIPBOARD` (`vento/text.h`),
/// which `api` asserts is equal to this.
pub const MAX_CLIPBOARD: usize = 1 << 20;
