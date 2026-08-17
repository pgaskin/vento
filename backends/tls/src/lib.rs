// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! TLS as this app means it: **pinned rather than verified**, and shared
//! between the thread that reads and the thread that writes.
//!
//! Both halves arrived the same way — the second protocol wanted the first's
//! answer unchanged — and they are one crate because they are always built
//! together and never separately: a client that pins a certificate is a client
//! that has a connection to pin it on.
//!
//! [`pinning`] is what "trusted" means when nothing vouches for a remote
//! desktop's certificate; [`stream`] is the locking that lets one connection
//! carry two threads.

pub mod pinning;
pub mod stream;
