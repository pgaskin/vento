// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! An RDP client, bound rather than written.
//!
//! The opposite choice from `rfb`: RFB is small enough that writing it was the
//! only way to be sure of the encodings, and RDP is not a protocol anybody
//! should implement twice. IronRDP is the connection sequence, the decoders and
//! the input encoding; what is here is the shape around them — threads, the
//! framebuffer, the trust decision, the keysym map — and it is deliberately the
//! same shape `rfb::Client` has, because `Backend` is what both have to fit.
//!
//! What it speaks:
//!
//! | | |
//! |---|---|
//! | Security | TLS, with or without CredSSP (NLA); never the legacy RC4 one |
//! | Graphics | fast-path and slow-path bitmaps, RemoteFX, NSCodec — whatever IronRDP decodes |
//! | Pointer | the server's own shape and hotspot, as a bitmap we draw |
//! | Input | fast-path, scancodes from a US layout with Unicode for the rest |
//! | Trust | the certificate pinned per address, as `rfb` does it (`pinning`) |
//! | Clipboard | text both ways over CLIPRDR (`clipboard`) |
//!
//! What it does not, and where each is written down: **audio**, **drive and
//! printer redirection**, **files on the clipboard**, and **resizing the
//! desktop from here**. The desktop size is asked for at connect time instead,
//! which is RDP's own model and the reverse of RFB's.

/// The protocol this client speaks, for the one JNI call that proves the whole
/// native path is wired up.
pub const PROTOCOL_VERSION: &str = "RDP 5.x (IronRDP)";

// The JNI half: the entry points the Java in this module declares, and the
// session behind them. Android only — everything above is the protocol, and it
// builds, tests and runs an example on the host with none of this compiled.
#[cfg(target_os = "android")]
mod bindings;

mod client;
mod clipboard;
mod error;
mod framebuffer;
mod keymap;
mod tls;

pub use client::{Client, Config, Credentials, Experience, Handler, Info, Nla, compression, experience};
pub use error::{Error, Result};
pub use framebuffer::Framebuffer;
pub use keymap::{Key, scancode};
