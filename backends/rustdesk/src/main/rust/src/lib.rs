// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! A RustDesk client, written rather than bound to.
//!
//! There was nothing to bind to: their client is `src/client.rs` at 4443 lines
//! plus an io loop of 2521, wired into their config store, their UI interface
//! and their rendezvous mediator. What can be taken is `hbb_common` — the
//! message definitions and the framing — and the client written above it, which
//! is what this is.
//!
//! It is the first protocol here that is somebody's product rather than a
//! published specification: one implementation, no second opinion, and a wire
//! that changes when they change it. Everything in here was read against
//! **rustdesk 1.4.9** and driven against a peer of that version.
//!
//! What it speaks:
//!
//! | | |
//! |---|---|
//! | Transport | a peer by id, punched or relayed and encrypted; or direct IP access, which is **plaintext in both directions** |
//! | Login | their permanent password, `sha256(sha256(password ++ salt) ++ challenge)` |
//! | Picture | VP9, whole frames, decoded by the phone |
//! | Cursor | shape and hotspot, by id |
//! | Input | absolute pointer, five buttons and a wheel; keys as their named set or as characters |
//! | Clipboard | both ways, text |
//! | Live | image quality, frame rate, and the desktop's size out of the list the peer offers |
//!
//! What it does not: audio, file transfer, the terminal, tunnelling, 2FA,
//! elevation, privacy mode and virtual displays, which are the product rather
//! than the desktop. Nor KCP, their newer UDP transport, which nothing has to
//! offer for a session to be reached.

/// The JNI half: the entry points the Java in this module declares, and the
/// session behind them. Android only — everything above is the protocol, and it
/// builds and tests on the host with none of this compiled.
#[cfg(target_os = "android")]
mod bindings;

mod client;
pub mod crypto;
mod error;
mod framebuffer;
mod keymap;
pub mod protos;
pub mod rendezvous;
mod video;
pub mod wire;

pub use client::{Client, Config, Handler, Info, Quality, Reach, VERSION};
pub use error::{Error, Result};
pub use framebuffer::Framebuffer;
pub use video::Codec;
