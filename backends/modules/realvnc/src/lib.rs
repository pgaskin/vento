// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! RealVNC's viewer core: on Linux, the backend over the hosting layer
//! ([`linux`]); on the phone, where the backend is the module's Java over the
//! viewer's own JNI, only the session's network for the module's native
//! library to put under the core ([`android`]). Empty on any other target.

#[cfg(target_os = "android")]
pub mod android;
#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "linux")]
pub use linux::*;
