//! What more than one protocol needs and none of them owns.
//!
//! Both halves arrived the same way — the second protocol wanted the first's
//! answer unchanged — and they have nothing else in common, which is why they
//! are modules of one small crate rather than crates of their own: nothing here
//! is published, and a crate boundary between two things that are always built
//! together buys a manifest and no checking.

pub mod pinning;
pub mod tls;

#[cfg(target_os = "android")]
pub mod android;
