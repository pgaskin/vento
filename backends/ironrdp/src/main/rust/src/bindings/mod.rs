// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The RDP backend's JNI surface: the entry points `IronRdpNative` declares, and
//! the session behind them.
//!
//! `libremotedesktop_ironrdp.so` is this crate, and this module is everything in
//! it that is not the protocol — it is compiled for Android and nowhere else.
//! What it needs of Android rather than of RDP — a locked bitmap, the callbacks
//! into Java, a place to wait for a person — is `common::android`, and the
//! protocol is the rest of this crate, which knows about neither.

mod session;

use crate::{Config, Credentials, Nla};
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

/// What the client speaks — the one call that proves the whole path.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_ironrdp_IronRdpNative_nativeVersion<'local>(
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
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_ironrdp_IronRdpNative_nativeCreate(
    mut env: EnvUnowned,
    _class: JClass,
    listener: JObject,
    address: JString,
    user_name: JString,
    domain: JString,
    password: JString,
    nla: JString,
    compression: JString,
    remote_fx: jboolean,
    experience: JString,
    width: jint,
    height: jint,
    monitors: jint,
    keyboard_layout: jint,
    client_name: JString,
    connect_timeout_ms: jint,
) -> jlong {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("Rdp"),
    );

    env.with_env(|env| -> Result<jlong, jni::errors::Error> {
        let callbacks = Callbacks::new(
            env,
            &listener,
            jni::jni_str!("net/pgaskin/remotedesktop/backend/ironrdp/IronRdpNative$Callbacks"),
        )?;
        let config = Config {
            address: string(env, &address)?.unwrap_or_default(),
            user_name: string(env, &user_name)?,
            domain: string(env, &domain)?,
            password: string(env, &password)?,
            desktop_size: (
                width.clamp(640, 8192) as u16,
                height.clamp(480, 8192) as u16,
            ),
            nla: match string(env, &nla)?.as_deref() {
                Some("require") => Nla::Require,
                Some("off") => Nla::Off,
                _ => Nla::Prefer,
            },
            compression: crate::compression(string(env, &compression)?.as_deref()),
            remote_fx,
            experience: crate::experience(string(env, &experience)?.as_deref()),
            monitors: monitors.clamp(1, 16) as u8,
            client_name: string(env, &client_name)?.unwrap_or_else(|| "remotedesktop".into()),
            keyboard_layout: keyboard_layout as u32,
            connect_timeout: Duration::from_millis(connect_timeout_ms.max(1000) as u64),
        };
        log::info!(
            "connecting to {} as {:?} ({}x{} × {}, nla {:?}, compression {:?}, remotefx {}, experience {:?})",
            config.address,
            config.user_name,
            config.desktop_size.0,
            config.desktop_size.1,
            config.monitors,
            config.nla,
            config.compression,
            config.remote_fx,
            config.experience
        );
        Ok(Box::into_raw(Box::new(Session::start(callbacks, config))) as jlong)
    })
    .resolve::<LogErrorAndDefault>()
}

/// A `null` password cancels, which ends the session.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_ironrdp_IronRdpNative_nativeAnswerCredentials(
    mut env: EnvUnowned,
    _class: JClass,
    handle: jlong,
    user_name: JString,
    domain: JString,
    password: JString,
) {
    let Some(session) = (unsafe { session(handle) }) else {
        return;
    };
    env.with_env(|env| -> Result<(), jni::errors::Error> {
        let password = string(env, &password)?;
        let user = string(env, &user_name)?.unwrap_or_default();
        let domain = string(env, &domain)?.unwrap_or_default();
        session.answer_credentials(password.map(|password| Credentials {
            user,
            domain,
            password,
        }));
        Ok(())
    })
    .resolve::<LogErrorAndDefault>();
}

/// Whether the certificate the session asked about is the right one. The pin
/// store is Java's, so this is the answer rather than the question.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_ironrdp_IronRdpNative_nativeAnswerTrust(
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
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_ironrdp_IronRdpNative_nativeDisconnect(
    _env: EnvUnowned,
    _class: JClass,
    handle: jlong,
) {
    if let Some(session) = unsafe { session(handle) } {
        session.disconnect();
    }
}

/// Ends the session, waits for its thread, and frees the handle.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_ironrdp_IronRdpNative_nativeDestroy(
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
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_ironrdp_IronRdpNative_nativePointer(
    _env: EnvUnowned,
    _class: JClass,
    handle: jlong,
    x: jint,
    y: jint,
    button_mask: jint,
) {
    if let Some(session) = unsafe { session(handle) } {
        session.client.pointer(
            x.clamp(0, 65535) as u16,
            y.clamp(0, 65535) as u16,
            // Nine bits: back and forward are buttons 8 and 9.
            (button_mask & 0x1ff) as u16,
        );
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_ironrdp_IronRdpNative_nativeKeyDown(
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
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_ironrdp_IronRdpNative_nativeKeyUp(
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
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_ironrdp_IronRdpNative_nativeReleaseAllKeys(
    _env: EnvUnowned,
    _class: JClass,
    handle: jlong,
) {
    if let Some(session) = unsafe { session(handle) } {
        session.client.release_all_keys();
    }
}

/// Whether the session is on screen. Unlike RFB's "stop asking", this is a
/// Suppress Output PDU — RDP's server sends what it likes until told not to.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_ironrdp_IronRdpNative_nativeFocus(
    _env: EnvUnowned,
    _class: JClass,
    handle: jlong,
    focused: jboolean,
) {
    if let Some(session) = unsafe { session(handle) } {
        session.client.set_focused(focused);
    }
}

/// What this phone has copied. Offered to the remote, which asks for it only if
/// somebody pastes there — see `clipboard`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_ironrdp_IronRdpNative_nativeClipboard(
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

/// Whether the far end will reshape the desktop. RDP settles the size in the
/// connection sequence, so this is the display control channel having opened
/// and said what it can do — which happens after the session starts, or never.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_ironrdp_IronRdpNative_nativeCanResize(
    _env: EnvUnowned,
    _class: JClass,
    handle: jlong,
) -> jboolean {
    match unsafe { session(handle) } {
        Some(session) => session.client.can_resize() as jboolean,
        None => false as jboolean,
    }
}

/// Ask for a desktop of this size per monitor. The answer is a reactivation
/// carrying whatever the server made of it, or nothing at all.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_ironrdp_IronRdpNative_nativeRequestDesktopSize(
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

/// How many monitors the next layout asks for.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_ironrdp_IronRdpNative_nativeSetMonitorCount(
    _env: EnvUnowned,
    _class: JClass,
    handle: jlong,
    count: jint,
) {
    if let Some(session) = unsafe { session(handle) } {
        session.client.set_monitor_count(count.clamp(1, 16) as u8);
    }
}

/// The monitors the desktop is made of, four ints each. Empty for a
/// single-monitor session and for one whose layout the server did not grant.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_ironrdp_IronRdpNative_nativeMonitors<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass,
    handle: jlong,
) -> jintArray {
    let flat = match unsafe { session(handle) } {
        Some(session) => session.client.monitors(),
        None => Vec::new(),
    };
    env.with_env(|env| -> Result<JIntArray<'local>, jni::errors::Error> {
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
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_ironrdp_IronRdpNative_nativeTraffic<'local>(
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

#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_ironrdp_IronRdpNative_nativeViewOnly(
    _env: EnvUnowned,
    _class: JClass,
    handle: jlong,
    view_only: jboolean,
) {
    if let Some(session) = unsafe { session(handle) } {
        session.client.set_view_only(view_only);
    }
}

/// Copy a desktop rectangle into `dst` at `(dst_x, dst_y)`, 1:1, leaving the
/// rest of `dst` alone. The offset is a slice offset and nothing else — see
/// `rfb`'s copy of this function for what it buys the caller.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_ironrdp_IronRdpNative_nativeReadRegion(
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
    framebuffer.read_rgba(
        x as usize,
        y as usize,
        width as usize,
        height as usize,
        &mut locked.bytes()[start..],
        stride,
    )
}

/// The whole desktop at `1/step`, into a bitmap Java has already sized.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_ironrdp_IronRdpNative_nativeReadThumbnail(
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

/// Everything the connection panel can be told, in the order `IronRdpBackend` reads
/// it back out. Shorter than the RFB one by the two rows RDP has no concept of.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_ironrdp_IronRdpNative_nativeInfo<'local>(
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
            info.protocol.as_str(),
            info.connection.as_str(),
            info.security.as_str(),
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
