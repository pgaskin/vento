// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! TigerVNC's `vento_tigervnc` as a [`Backend`](api::backend::Backend): its
//! config from the options, its live options and its rows. The Linux client
//! uses it directly; on Android the JNI layer wraps it, linked into the
//! library Meson builds from the C++. The client decides on its certificates:
//! the session passes them to `trust_needed`, so they are checked against the
//! known hosts as every other backend's are, and TigerVNC's own known-hosts
//! file is never read or written, which is why it is given no state
//! directory. Its H.264 is decoded by the platform's decoders, passed to the
//! C as `vento/video.h`'s table (`api::c::video`): FFmpeg on the Linux client,
//! with the user's hardware choice, and MediaCodec on Android. Its sound
//! (QEMU's audio extension) is played on the platform's output the same way,
//! passed to the C as `vento/audio.h`'s table (`api::c::audio`): a PipeWire
//! stream (or ALSA's default device) on the Linux client, AAudio on Android.

use api::c::{Behaviour, CSession, start};
use api::backend::{Backend, ConnectionFact, Field, Traffic};
use api::kind::{CONNECT_TIMEOUT_MS, Connect, Kind};
use api::options::Options;
use std::ffi::CString;

/// `vento/tigervnc.h`, as `build.rs` generated it, over the shared declarations
/// it is written against.
mod ffi {
    #![allow(non_camel_case_types, dead_code)]
    pub use api::c::ffi::*;
    include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
}

pub static KIND: Kind = Kind {
    about: &api::catalog::TIGERVNC,
    version: || LIBRARY.version(|c| unsafe { api::c::string(api::call!(c, ffi::vento_tigervnc_version())) }),
    create: api::create_if_loaded!(LIBRARY, "TigerVNC", create),
    forget: None,
};

// The Android library: this backend, registered on the module's generated
// natives class. Android only; on the host the C++ is a library of its own.
// MediaCodec decodes its H.264 there, through the same table as the Linux
// client's FFmpeg (see `create`), and AAudio plays its sound, through the
// same table as the Linux client's PipeWire or ALSA.
#[cfg(target_os = "android")]
platform_android::natives!(
    class = c"net/pgaskin/remotedesktop/backend/tigervnc/TigerVncNatives",
    kind = KIND,
    decoders = || Some(std::sync::Arc::new(platform_android::mediacodec::MediaCodec)),
    audio = || Some(std::sync::Arc::new(platform_android::aaudio::AAudio::new())),
);

static BEHAVIOUR: Behaviour = Behaviour {
    info,
    sent_counted: true,
    set_option,
    rdp_domain: false,
};

/// Automatic, for a quality or a colour depth: the client's own rule over
/// its bandwidth estimate.
const LEVEL_AUTO: i32 = -2;

const ENCODING_RAW: i32 = 0;
const ENCODING_RRE: i32 = 2;
const ENCODING_HEXTILE: i32 = 5;
const ENCODING_TIGHT: i32 = 7;
const ENCODING_ZRLE: i32 = 16;
const ENCODING_H264: i32 = 50;

fn encoding(value: &str) -> i32 {
    match value {
        "h264" => ENCODING_H264,
        "zrle" => ENCODING_ZRLE,
        "hextile" => ENCODING_HEXTILE,
        "rre" => ENCODING_RRE,
        "raw" => ENCODING_RAW,
        _ => ENCODING_TIGHT,
    }
}

fn level(value: &str) -> i32 {
    value.parse().unwrap_or(-1)
}

/// The quality option, which has one more answer than compression: "auto" is
/// the client's own rule, and "lossless" falls through to the absence of a
/// level.
fn quality(value: &str) -> i32 {
    if value == "auto" { LEVEL_AUTO } else { level(value) }
}

fn colour(value: &str) -> i32 {
    match value {
        "auto" => LEVEL_AUTO,
        "rgb222" => 1,
        "rgb111" => 2,
        _ => 0,
    }
}

fn create(c: &Connect) -> Result<std::sync::Arc<dyn Backend>, String> {
    let o = c.options;
    let address = CString::new(c.address).map_err(|_| "address has a NUL")?;
    let user = (!c.user.is_empty()).then(|| CString::new(c.user).unwrap_or_default());
    let password = c.password.and_then(|p| CString::new(p).ok());
    let mut config = ffi::vento_tigervnc_config {
        address: address.as_ptr(),
        user_name: user.as_ref().map_or(std::ptr::null(), |u| u.as_ptr()),
        password: password.as_ref().map_or(std::ptr::null(), |p| p.as_ptr()),
        shared: o.bool("Shared"),
        anonymous_tls: o.bool("AnonymousTLS"),
        encoding: encoding(o.get("Encoding")),
        compress_level: level(o.get("Compression")),
        quality_level: quality(o.get("Quality")),
        color_level: colour(o.get("ColorLevel")),
        h264: o.bool("H264"),
        video: std::ptr::null(),
        audio: o.bool("Audio"),
        audio_output: std::ptr::null(),
        connect_timeout_ms: CONNECT_TIMEOUT_MS,
        net: std::ptr::null(),
    };
    log::info!("connecting to {}", c.address);
    let backend = start(
        c,
        &BEHAVIOUR,
        |callbacks| {
            // H.264 is decoded by the platform's decoders, which are passed
            // to the session here. It owns them from this call and releases
            // them when it ends (or at once, if it does not start). Made here
            // rather than above so that they are passed on every path that
            // makes them. Without decoders the session never offers H.264.
            let video = c.decoders.clone().map(api::c::video::table);
            if let Some(video) = &video {
                config.video = video;
            }
            // The sound output, the same way: owned by the session from here,
            // and without one the server is never asked for its sound.
            let audio = c.audio.clone().map(api::c::audio::table);
            if let Some(audio) = &audio {
                config.audio_output = audio;
            }
            // And the session's network, which makes the connection.
            let net = api::c::net::table(c.network.clone());
            config.net = &net;
            unsafe { api::call!(LIBRARY.loaded(), ffi::vento_tigervnc_create(&config, callbacks)) }
        },
        None,
    )?;
    backend.set_view_only(o.bool("ViewOnly"));
    Ok(backend)
}

/// All of the picture options are live: `SetEncodings` can be sent at any
/// point, and the client can change the pixel format safely. Shared is decided
/// at ClientInit, audio at the first SetEncodings, and the bell when a bell
/// arrives.
fn set_option(session: &CSession, o: &Options, key: &str) {
    if matches!(key, "Encoding" | "Compression" | "Quality" | "ColorLevel" | "H264") {
        unsafe {
            api::call!(LIBRARY.loaded(), ffi::vento_tigervnc_set_picture(
                session.session,
                encoding(o.get("Encoding")),
                level(o.get("Compression")),
                quality(o.get("Quality")),
                colour(o.get("ColorLevel")),
                o.bool("H264"),
            ))
        }
    }
}

fn info(s: &[String], traffic: Option<Traffic>) -> Vec<ConnectionFact> {
    if s.len() < 8 {
        return Vec::new();
    }
    let mut facts = vec![
        ConnectionFact::of(Field::DesktopName, "Desktop", &s[0]),
        ConnectionFact::of(Field::Protocol, "Protocol", &s[1]),
        ConnectionFact::of(Field::Connection, "Connection", &s[2]),
        ConnectionFact::of(Field::Security, "Security", &s[3]),
        ConnectionFact::of(Field::Encoding, "Encoding", &s[4]),
        ConnectionFact::of(Field::LineSpeed, "Line speed", &s[5]),
    ];
    if let Some(traffic) = traffic {
        facts.push(ConnectionFact::data(traffic));
    }
    facts.push(ConnectionFact::diagnostic(Field::ServerPixels, "Server pixels", &s[6]));
    facts.push(ConnectionFact::diagnostic(Field::ViewerPixels, "Viewer pixels", &s[7]));
    facts
}

/// The library the C++ is in: on the Linux client `libvento_tigervnc.so`,
/// opened the first time it is needed, so that one that cannot be loaded makes
/// this backend unavailable rather than stopping the client from starting; on
/// Android, linked into the same library as this crate.
#[cfg(target_os = "linux")]
static LIBRARY: api::c::library::Library<ffi::Library> = api::c::library::Library::new(
    "libvento_tigervnc.so",
    "vento/tigervnc.h",
    api::c::library::Scope::Local,
    ffi::Library::from_library,
);
#[cfg(not(target_os = "linux"))]
static LIBRARY: api::c::library::Linked = api::c::library::Linked;

/// The file `libvento_tigervnc.so` was opened from, opening it first, or its name
/// and why it cannot be opened: for the client's list of its backends.
#[cfg(target_os = "linux")]
pub fn library_path() -> Result<std::path::PathBuf, (&'static str, &'static str)> {
    LIBRARY.path()
}
