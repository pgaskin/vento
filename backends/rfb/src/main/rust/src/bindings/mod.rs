// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The RFB backend's JNI surface: the entry points `RfbNative` declares, and
//! the session behind them.
//!
//! `libremotedesktop_rfb.so` is this crate, and this module is everything in
//! it that is not the protocol — it is compiled for Android and nowhere else.
//! What it needs of Android rather than of RFB — a locked bitmap, the callbacks
//! into Java, a place to wait for a person — is `common::android`, and the
//! protocol is the rest of this crate, which knows about neither.

mod session;

use crate::{Config, Credentials, Security};
use common::android::bitmap;
use common::android::callbacks::Callbacks;
use common::android::string;
use jni::EnvUnowned;
use jni::errors::LogErrorAndDefault;
use jni::objects::{JClass, JIntArray, JLongArray, JObject, JObjectArray, JString};
use jni::sys::{jboolean, jint, jintArray, jlong, jlongArray, jobjectArray, jstring};
use session::Session;
use std::time::Duration;

/// SAFETY: `handle` must be one `nativeCreate` returned and `nativeDestroy` has
/// not been called for. A zero handle is the ordinary "already retired" case.
unsafe fn session<'a>(handle: jlong) -> Option<&'a Session> {
    if handle == 0 {
        None
    } else {
        Some(unsafe { &*(handle as *const Session) })
    }
}

/// The RFB version the client speaks.
///
/// The one call that proves the whole path — cargo-ndk built it, Gradle
/// packaged it, `System.loadLibrary` found it, and a string crosses the
/// boundary.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_rfb_RfbNative_nativeVersion<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass,
) -> jstring {
    env.with_env(|env| -> Result<JString<'local>, jni::errors::Error> {
        env.new_string(crate::PROTOCOL_VERSION)
    })
    .resolve::<LogErrorAndDefault>()
    .into_raw()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_rfb_RfbNative_nativeCreate(
    mut env: EnvUnowned,
    _class: JClass,
    listener: JObject,
    address: JString,
    user_name: JString,
    password: JString,
    security: JString,
    shared: jboolean,
    encoding: JString,
    compress_level: jint,
    connect_timeout_ms: jint,
) -> jlong {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("Rfb"),
    );

    env.with_env(|env| -> Result<jlong, jni::errors::Error> {
        let callbacks = Callbacks::new(
            env,
            &listener,
            jni::jni_str!("net/pgaskin/remotedesktop/backend/rfb/RfbNative$Callbacks"),
        )?;
        let config = Config {
            address: string(env, &address)?.unwrap_or_default(),
            user_name: string(env, &user_name)?,
            password: string(env, &password)?,
            security: match string(env, &security)?.as_deref() {
                Some("require") => Security::Require,
                Some("plain") => Security::Plain,
                _ => Security::Prefer,
            },
            shared,
            encodings: encodings(string(env, &encoding)?.as_deref()),
            compress_level: if (0..=9).contains(&compress_level) {
                Some(compress_level as u8)
            } else {
                None
            },
            connect_timeout: Duration::from_millis(connect_timeout_ms.max(1000) as u64),
        };
        log::info!(
            "connecting to {} ({} encodings, compression {:?}, security {:?})",
            config.address,
            config.encodings.len(),
            config.compress_level,
            config.security
        );
        Ok(Box::into_raw(Box::new(Session::start(callbacks, config))) as jlong)
    })
    .resolve::<LogErrorAndDefault>()
}

/// A `null` password cancels, which ends the session — the only answer a
/// `Prompt.Credentials` has besides a secret.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_rfb_RfbNative_nativeAnswerCredentials(
    mut env: EnvUnowned,
    _class: JClass,
    handle: jlong,
    user_name: JString,
    password: JString,
) {
    let Some(session) = (unsafe { session(handle) }) else {
        return;
    };
    env.with_env(|env| -> Result<(), jni::errors::Error> {
        let password = string(env, &password)?;
        let user = string(env, &user_name)?.unwrap_or_default();
        session.answer_credentials(password.map(|password| Credentials { user, password }));
        Ok(())
    })
    .resolve::<LogErrorAndDefault>();
}

/// Whether the certificate the session asked about is the right one. The pin
/// store is Java's, so this is the answer rather than the question.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_rfb_RfbNative_nativeAnswerTrust(
    _env: EnvUnowned,
    _class: JClass,
    handle: jlong,
    accept: jboolean,
) {
    if let Some(session) = unsafe { session(handle) } {
        session.answer_trust(accept);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_rfb_RfbNative_nativeDisconnect(
    _env: EnvUnowned,
    _class: JClass,
    handle: jlong,
) {
    if let Some(session) = unsafe { session(handle) } {
        session.disconnect();
    }
}

/// Ends the session, waits for its thread, and frees the handle. Nothing may
/// use it afterwards, which Java guarantees by retiring it first.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_rfb_RfbNative_nativeDestroy(
    _env: EnvUnowned,
    _class: JClass,
    handle: jlong,
) {
    if handle == 0 {
        return;
    }
    let session = unsafe { Box::from_raw(handle as *mut Session) };
    session.destroy();
    drop(session);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_rfb_RfbNative_nativePointer(
    _env: EnvUnowned,
    _class: JClass,
    handle: jlong,
    x: jint,
    y: jint,
    button_mask: jint,
) {
    if let Some(session) = unsafe { session(handle) } {
        // Nine buttons go across, and how many arrive is the client's business:
        // it drops the ninth on a server that never agreed to carry one.
        session.client.pointer(
            x.clamp(0, 65535) as u16,
            y.clamp(0, 65535) as u16,
            (button_mask & 0x1ff) as u16,
        );
    }
}

/// The same event where the far end owns the cursor: a delta rather than a
/// place. Only ever called when the server has said it wants one.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_rfb_RfbNative_nativePointerRelative(
    _env: EnvUnowned,
    _class: JClass,
    handle: jlong,
    dx: jint,
    dy: jint,
    button_mask: jint,
) {
    if let Some(session) = unsafe { session(handle) } {
        session
            .client
            .pointer_relative(dx, dy, (button_mask & 0x1ff) as u16);
    }
}

/// Whether this server has offered `ExtendedDesktopSize`. Polled rather than
/// announced: it becomes true when the rectangle arrives, which is after the
/// connection and may be never.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_rfb_RfbNative_nativeCanResize(
    _env: EnvUnowned,
    _class: JClass,
    handle: jlong,
) -> jboolean {
    match unsafe { session(handle) } {
        Some(session) => session.client.can_resize() as jboolean,
        None => false as jboolean,
    }
}

/// The far end's screen layout, four ints per screen — x, y, width, height —
/// or an empty array when no `ExtendedDesktopSize` rectangle has arrived.
///
/// Flattened rather than an array of objects because the alternative is
/// constructing a Java record per screen from here, and there are at most a
/// handful of screens read at most once a second.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_rfb_RfbNative_nativeMonitors<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass,
    handle: jlong,
) -> jintArray {
    let screens = match unsafe { session(handle) } {
        Some(session) => session.client.screens(),
        None => Vec::new(),
    };
    env.with_env(|env| -> Result<JIntArray<'local>, jni::errors::Error> {
        let flat: Vec<jint> = screens
            .iter()
            .flat_map(|s| {
                [
                    jint::from(s.x),
                    jint::from(s.y),
                    jint::from(s.width),
                    jint::from(s.height),
                ]
            })
            .collect();
        let array = env.new_int_array(flat.len())?;
        array.set_region(env, 0, &flat)?;
        Ok(array)
    })
    .resolve::<LogErrorAndDefault>()
    .into_raw()
}

/// Received and sent, in bytes, since the socket was opened — protocol bytes,
/// counted inside TLS and outside TCP, which is what the row this feeds says it
/// means.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_rfb_RfbNative_nativeTraffic<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass,
    handle: jlong,
) -> jlongArray {
    let (received, sent) = match unsafe { session(handle) } {
        Some(session) => session.client.traffic(),
        None => return std::ptr::null_mut(),
    };
    env.with_env(|env| -> Result<JLongArray<'local>, jni::errors::Error> {
        let both = [received as jlong, sent as jlong];
        let array = env.new_long_array(both.len())?;
        array.set_region(env, 0, &both)?;
        Ok(array)
    })
    .resolve::<LogErrorAndDefault>()
    .into_raw()
}

/// Ask the far end for a desktop this size. What came of it arrives as
/// `onDesktopSize`, or as nothing at all.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_rfb_RfbNative_nativeRequestDesktopSize(
    _env: EnvUnowned,
    _class: JClass,
    handle: jlong,
    width: jint,
    height: jint,
) {
    if let Some(session) = unsafe { session(handle) } {
        session
            .client
            .request_desktop_size(width.clamp(0, 65535) as u16, height.clamp(0, 65535) as u16);
    }
}

/// Whether the far end owns the cursor, for a screen attaching to a session
/// that has already been told.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_rfb_RfbNative_nativePointerIsRelative(
    _env: EnvUnowned,
    _class: JClass,
    handle: jlong,
) -> jboolean {
    match unsafe { session(handle) } {
        Some(session) => session.client.pointer_is_relative() as jboolean,
        None => false as jboolean,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_rfb_RfbNative_nativeKeyDown(
    _env: EnvUnowned,
    _class: JClass,
    handle: jlong,
    keysym: jint,
    key_id: jlong,
) {
    if let Some(session) = unsafe { session(handle) } {
        session.client.key_down(keysym as u32, key_id);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_rfb_RfbNative_nativeKeyUp(
    _env: EnvUnowned,
    _class: JClass,
    handle: jlong,
    key_id: jlong,
) {
    if let Some(session) = unsafe { session(handle) } {
        session.client.key_up(key_id);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_rfb_RfbNative_nativeReleaseAllKeys(
    _env: EnvUnowned,
    _class: JClass,
    handle: jlong,
) {
    if let Some(session) = unsafe { session(handle) } {
        session.client.release_all_keys();
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_rfb_RfbNative_nativeFocus(
    _env: EnvUnowned,
    _class: JClass,
    handle: jlong,
    focused: jboolean,
) {
    if let Some(session) = unsafe { session(handle) } {
        session.client.set_focused(focused);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_rfb_RfbNative_nativeViewOnly(
    _env: EnvUnowned,
    _class: JClass,
    handle: jlong,
    view_only: jboolean,
) {
    if let Some(session) = unsafe { session(handle) } {
        session.client.set_view_only(view_only);
    }
}

/// The picture-quality control, live. RFB lets `SetEncodings` be sent at any
/// time, so unlike the RealVNC backend's quality group this really does act on
/// the session in front of you.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_rfb_RfbNative_nativeSetEncodings(
    mut env: EnvUnowned,
    _class: JClass,
    handle: jlong,
    encoding: JString,
    compress_level: jint,
) {
    let Some(session) = (unsafe { session(handle) }) else {
        return;
    };
    env.with_env(|env| -> Result<(), jni::errors::Error> {
        let list = encodings(string(env, &encoding)?.as_deref());
        session.client.set_encodings(
            &list,
            if (0..=9).contains(&compress_level) {
                Some(compress_level as u8)
            } else {
                None
            },
        );
        Ok(())
    })
    .resolve::<LogErrorAndDefault>();
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_rfb_RfbNative_nativeClipboard(
    mut env: EnvUnowned,
    _class: JClass,
    handle: jlong,
    text: JString,
) {
    let Some(session) = (unsafe { session(handle) }) else {
        return;
    };
    env.with_env(|env| -> Result<(), jni::errors::Error> {
        if let Some(text) = string(env, &text)? {
            session.client.clipboard(&text);
        }
        Ok(())
    })
    .resolve::<LogErrorAndDefault>();
}

/// Copy a desktop rectangle into `dst` at `(dst_x, dst_y)`, 1:1, leaving the
/// rest of `dst` alone.
///
/// The offset is what makes this the whole of the pixel path: the caller holds
/// its picture as tiles and refreshes part of one without a scratch bitmap
/// anywhere, because "into the destination's own rows" is a slice offset here
/// and a copy there.
///
/// Called from the drawing thread, concurrently with everything else — which is
/// safe because the framebuffer is behind an `RwLock` and this takes the read
/// side. The write side is only ever held for a blit (see `rfb::Framebuffer`).
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_rfb_RfbNative_nativeReadRegion(
    env: EnvUnowned,
    _class: JClass,
    handle: jlong,
    x: jint,
    y: jint,
    width: jint,
    height: jint,
    dst: JObject,
    dst_x: jint,
    dst_y: jint,
) -> jboolean {
    let Some(session) = (unsafe { session(handle) }) else {
        return false;
    };
    if x < 0 || y < 0 || width <= 0 || height <= 0 || dst_x < 0 || dst_y < 0 {
        return false;
    }
    let raw_env = env.as_raw();
    let raw_dst = dst.as_raw();
    let Some(mut locked) = (unsafe { bitmap::Locked::new(raw_env, raw_dst) }) else {
        return false;
    };
    if locked.width < dst_x as usize + width as usize
        || locked.height < dst_y as usize + height as usize
    {
        return false;
    }
    let stride = locked.stride;
    let start = dst_y as usize * stride + dst_x as usize * 4;
    let framebuffer = session.client.framebuffer().read().unwrap();
    let ok = framebuffer.read_rgba(
        x as usize,
        y as usize,
        width as usize,
        height as usize,
        &mut locked.bytes()[start..],
        stride,
    );
    ok
}

/// The whole desktop at `1/step`, into a bitmap Java has already sized.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_rfb_RfbNative_nativeReadThumbnail(
    env: EnvUnowned,
    _class: JClass,
    handle: jlong,
    step: jint,
    dst: JObject,
) -> jboolean {
    let Some(session) = (unsafe { session(handle) }) else {
        return false;
    };
    if step <= 0 {
        return false;
    }
    let scaled = {
        let framebuffer = session.client.framebuffer().read().unwrap();
        framebuffer.thumbnail(step as usize)
    };
    let Some((w, h, pixels)) = scaled else {
        return false;
    };
    let raw_env = env.as_raw();
    let raw_dst = dst.as_raw();
    let Some(mut locked) = (unsafe { bitmap::Locked::new(raw_env, raw_dst) }) else {
        return false;
    };
    if locked.width < w || locked.height < h {
        return false;
    }
    let stride = locked.stride;
    let bytes = locked.bytes();
    for row in 0..h {
        let (_, src, _) = unsafe { pixels[row * w..row * w + w].align_to::<u8>() };
        bytes[row * stride..row * stride + w * 4].copy_from_slice(src);
    }
    true
}

/// Everything the connection panel can be told, in the order `RfbBackend` reads
/// it back out. A fixed-length array rather than a map, for the reason
/// `ConnectionFact` replaced one: the order and the identity of each row are
/// decisions, and they belong on one side.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_rfb_RfbNative_nativeInfo<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass,
    handle: jlong,
) -> jobjectArray {
    let Some(session) = (unsafe { session(handle) }) else {
        return std::ptr::null_mut();
    };
    let info = session.client.info();
    env.with_env(|env| -> Result<JObjectArray<'local>, jni::errors::Error> {
        let fields = [
            info.desktop_name.as_str(),
            info.protocol.as_str(),
            info.connection.as_str(),
            info.security.as_str(),
            info.encoding.as_str(),
            info.line_speed.as_str(),
            info.server_pixels.as_str(),
            info.viewer_pixels.as_str(),
        ];
        let class = env.find_class(jni::jni_str!("java/lang/String"))?;
        let array = env.new_object_array(fields.len() as i32, &class, JObject::null())?;
        for (i, value) in fields.iter().enumerate() {
            let s = env.new_string(*value)?;
            array.set_element(env, i, &s)?;
        }
        Ok(array)
    })
    .resolve::<LogErrorAndDefault>()
    .into_raw()
}


/// The `Encoding` option's value, as `RfbProvider` writes it.
fn encodings(name: Option<&str>) -> Vec<i32> {
    match name {
        Some("raw") => vec![crate::ENC_RAW],
        Some("rre") => vec![crate::ENC_RRE],
        Some("hextile") => vec![crate::ENC_HEXTILE],
        Some("zrle") => vec![crate::ENC_ZRLE],
        // "auto", anything unknown, and nothing at all: offer them all, best
        // first, and let the server pick — which is what RFB's SetEncodings is
        // for.
        _ => vec![crate::ENC_ZRLE, crate::ENC_HEXTILE, crate::ENC_RRE],
    }
}
