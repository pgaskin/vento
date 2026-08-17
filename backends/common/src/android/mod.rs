// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The parts of a backend's JNI half that are about Android rather than about a
//! protocol: [`bitmap`], which writes pixels straight into a Java `Bitmap`,
//! [`callbacks`], which is a session's Java side resolved once, and [`slot`],
//! where a protocol thread waits for a person.
//!
//! It was one crate with the RFB bindings in it, then one crate with both, and
//! the splits changed only where the seams are. Everything a second protocol
//! wanted unchanged is here, and what is
//! left in a protocol's own `bindings` module is that protocol's entry points
//! and its session.
//!
//! Two threads reach a session, as they do in the RealVNC backend and for the
//! same reasons: the protocol thread, which owns the connection and delivers
//! every callback, and **whichever thread is drawing**, which calls
//! `nativeReadRegion` straight through so that pixel fetches never queue behind
//! connection work.
//!
//! The handle is a `Box::into_raw`'d session. Java retires it under its own lock
//! before calling `nativeDestroy`, which is the same discipline the RealVNC
//! backend follows — the difference being that here the join in `destroy` can
//! prove the callbacks are finished, because both ends of the boundary are ours.

pub mod bitmap;
pub mod callbacks;
pub mod slot;

use jni::objects::JString;

/// `null` and the empty string both mean "nothing", because a Java field left
/// blank arrives as one or the other depending on who filled it in.
pub fn string(env: &mut jni::Env, s: &JString) -> Result<Option<String>, jni::errors::Error> {
    if s.is_null() {
        return Ok(None);
    }
    let text = s.try_to_string(env)?;
    Ok(if text.is_empty() { None } else { Some(text) })
}
