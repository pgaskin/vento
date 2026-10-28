// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The Linux client's library half: the eight backends as this client lists
//! them, where it keeps its pins, and the window. The interface they all
//! implement ([`Backend`](backend::Backend), the [`Event`](events::Event)
//! channel, the option registry) is `remotedesktop-api`'s, shared with the
//! phone, and re-exported here as `backend`, `events` and `options`.

pub use api::{backend, events, options};

pub mod backends;
pub mod gui;
pub mod known_hosts;
pub mod ppm;
pub mod realvnc;

pub mod log;
