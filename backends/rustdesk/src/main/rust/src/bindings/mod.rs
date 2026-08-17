//! The RustDesk backend's JNI surface: the entry points `RustDeskNative`
//! declares, and the session behind them.
//!
//! `libremotedesktop_rustdesk.so` is this crate, and this module is everything
//! in it that is not the protocol — it is compiled for Android and nowhere
//! else. What it needs of Android rather than of RustDesk — a locked bitmap,
//! the callbacks into Java, a place to wait for a person — is `common::android`.

mod session;

use crate::client::{Config, PreferredCodec, Quality, Reach};
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

/// The version of theirs this client speaks, which is what it claims to be on
/// the wire — and the one call that proves the whole path.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_rustdesk_RustDeskNative_nativeVersion<
    'local,
>(
    mut env: EnvUnowned<'local>,
    _class: JClass,
) -> jstring {
    env.with_env(|env| -> Result<JString<'local>, jni::errors::Error> {
        env.new_string(crate::VERSION)
    })
    .resolve::<LogErrorAndDefault>()
    .into_raw()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_rustdesk_RustDeskNative_nativeCreate(
    mut env: EnvUnowned,
    _class: JClass,
    listener: JObject,
    address: JString,
    by_id: jboolean,
    server: JString,
    server_key: JString,
    password: JString,
    my_name: JString,
    quality: JString,
    fps: jint,
    codec: JString,
    lock_after: jboolean,
    connect_timeout_ms: jint,
) -> jlong {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("RustDesk"),
    );

    env.with_env(|env| -> Result<jlong, jni::errors::Error> {
        let callbacks = Callbacks::new(
            env,
            &listener,
            jni::jni_str!("net/pgaskin/remotedesktop/backend/rustdesk/RustDeskNative$Callbacks"),
        )?;
        let config = Config {
            address: string(env, &address)?.unwrap_or_default(),
            reach: if by_id {
                Reach::Id {
                    server: string(env, &server)?.unwrap_or_default(),
                    key: string(env, &server_key)?.unwrap_or_default(),
                }
            } else {
                Reach::Direct
            },
            password: string(env, &password)?,
            connect_timeout: Duration::from_millis(connect_timeout_ms.max(1000) as u64),
            my_name: string(env, &my_name)?.unwrap_or_else(|| "Android".into()),
            quality: quality_of(string(env, &quality)?.as_deref()),
            fps: fps.clamp(0, 120),
            codec: codec_of(string(env, &codec)?.as_deref()),
            lock_after,
        };
        log::info!(
            "connecting to {} {} ({:?}, {} fps)",
            config.address,
            if by_id { "by id" } else { "directly" },
            config.quality,
            config.fps
        );
        Ok(Box::into_raw(Box::new(Session::start(callbacks, config))) as jlong)
    })
    .resolve::<LogErrorAndDefault>()
}

/// A `null` password cancels, which ends the session — the only answer a
/// `Prompt.Credentials` has besides a secret.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_rustdesk_RustDeskNative_nativeAnswerPassword(
    mut env: EnvUnowned,
    _class: JClass,
    handle: jlong,
    password: JString,
) {
    let Some(session) = (unsafe { session(handle) }) else {
        return;
    };
    env.with_env(|env| -> Result<(), jni::errors::Error> {
        session.answer_password(string(env, &password)?);
        Ok(())
    })
    .resolve::<LogErrorAndDefault>();
}

/// Whether the peer's long-term key is one this connection may go on with.
/// Only the id path ever waits for it.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_rustdesk_RustDeskNative_nativeAnswerTrust(
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
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_rustdesk_RustDeskNative_nativeDisconnect(
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
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_rustdesk_RustDeskNative_nativeDestroy(
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

/// Absolute desktop coordinates and an RFB button mask. Which of those become a
/// move, a press, a release or a notch of the wheel is the client's business.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_rustdesk_RustDeskNative_nativePointer(
    _env: EnvUnowned,
    _class: JClass,
    handle: jlong,
    x: jint,
    y: jint,
    button_mask: jint,
) {
    if let Some(session) = unsafe { session(handle) } {
        session.client.pointer(x, y, button_mask & 0x1ff);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_rustdesk_RustDeskNative_nativeKeyDown(
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
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_rustdesk_RustDeskNative_nativeKeyUp(
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
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_rustdesk_RustDeskNative_nativeReleaseAllKeys(
    _env: EnvUnowned,
    _class: JClass,
    handle: jlong,
) {
    if let Some(session) = unsafe { session(handle) } {
        session.client.release_all_keys();
    }
}

/// Whether the session is on screen. There is no pause message in this
/// protocol, so what this does is ask the far end for one frame a second.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_rustdesk_RustDeskNative_nativeFocus(
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
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_rustdesk_RustDeskNative_nativeViewOnly(
    _env: EnvUnowned,
    _class: JClass,
    handle: jlong,
    view_only: jboolean,
) {
    if let Some(session) = unsafe { session(handle) } {
        session.client.set_view_only(view_only);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_rustdesk_RustDeskNative_nativeClipboard(
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

/// The picture-quality controls, live: one `Misc::option` message, which their
/// server acts on for the next frame.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_rustdesk_RustDeskNative_nativeSetOptions(
    mut env: EnvUnowned,
    _class: JClass,
    handle: jlong,
    quality: JString,
    fps: jint,
    codec: JString,
) {
    let Some(session) = (unsafe { session(handle) }) else {
        return;
    };
    env.with_env(|env| -> Result<(), jni::errors::Error> {
        let quality = quality_of(string(env, &quality)?.as_deref());
        let codec = codec_of(string(env, &codec)?.as_deref());
        session.client.set_options(quality, fps.clamp(0, 120), codec);
        Ok(())
    })
    .resolve::<LogErrorAndDefault>();
}

/// Whether the far end published a list of sizes it will take. Polled rather
/// than announced: the list arrives with the login response and a peer that
/// cannot resize sends an empty one.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_rustdesk_RustDeskNative_nativeCanResize(
    _env: EnvUnowned,
    _class: JClass,
    handle: jlong,
) -> jboolean {
    match unsafe { session(handle) } {
        Some(session) => session.client.can_resize(),
        None => false,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_rustdesk_RustDeskNative_nativeRequestDesktopSize(
    _env: EnvUnowned,
    _class: JClass,
    handle: jlong,
    width: jint,
    height: jint,
) {
    if let Some(session) = unsafe { session(handle) } {
        session.client.request_desktop_size(width, height);
    }
}

/// The far end's displays, four numbers each — x, y, width and height in its
/// own desktop — with the index of the one being captured on the end.
///
/// Empty where the peer has one display, since a choice of one is not a
/// control. Polled rather than announced, like the size list beside it: a
/// display appearing over there is a message this end may or may not get.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_rustdesk_RustDeskNative_nativeDisplays<
    'local,
>(
    mut env: EnvUnowned<'local>,
    _class: JClass,
    handle: jlong,
) -> jintArray {
    let Some(session) = (unsafe { session(handle) }) else {
        return std::ptr::null_mut();
    };
    let (mut flat, current) = session.client.displays();
    if flat.is_empty() {
        return std::ptr::null_mut();
    }
    flat.push(current);
    env.with_env(|env| -> Result<JIntArray<'local>, jni::errors::Error> {
        let array = env.new_int_array(flat.len())?;
        array.set_region(env, 0, &flat)?;
        Ok(array)
    })
    .resolve::<LogErrorAndDefault>()
    .into_raw()
}

/// Ask the far end to capture one of its other displays. What comes back is a
/// switch message, which may name a different display than the one asked for.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_rustdesk_RustDeskNative_nativeRequestDisplay(
    _env: EnvUnowned,
    _class: JClass,
    handle: jlong,
    index: jint,
) {
    if let Some(session) = unsafe { session(handle) } {
        session.client.request_display(index);
    }
}

/// Received and sent, in bytes, since the socket was opened — protocol bytes,
/// inside whatever is encrypting them and outside what the link adds.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_rustdesk_RustDeskNative_nativeTraffic<
    'local,
>(
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

/// Everything the connection panel can be told, in the order `RustDeskBackend`
/// reads it back out.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_rustdesk_RustDeskNative_nativeInfo<
    'local,
>(
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
            info.round_trip.as_str(),
            info.platform.as_str(),
            info.display.as_str(),
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

/// Copy a desktop rectangle into `dst` at `(dst_x, dst_y)`, 1:1, leaving the
/// rest of `dst` alone.
///
/// Called from the drawing thread, concurrently with everything else, which is
/// safe because the framebuffer is behind an `RwLock` and this takes the read
/// side. The write side is only ever held for a buffer swap.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_rustdesk_RustDeskNative_nativeReadRegion(
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
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_rustdesk_RustDeskNative_nativeReadThumbnail(
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

/// The `Codec` option's value, as `RustDeskProvider` writes it.
fn codec_of(name: Option<&str>) -> PreferredCodec {
    match name {
        Some("vp9") => PreferredCodec::Vp9,
        Some("vp8") => PreferredCodec::Vp8,
        Some("av1") => PreferredCodec::Av1,
        Some("h264") => PreferredCodec::H264,
        Some("h265") => PreferredCodec::H265,
        _ => PreferredCodec::Auto,
    }
}

/// The `Quality` option's value, as `RustDeskProvider` writes it.
fn quality_of(name: Option<&str>) -> Quality {
    match name {
        Some("low") => Quality::Low,
        Some("best") => Quality::Best,
        _ => Quality::Balanced,
    }
}
