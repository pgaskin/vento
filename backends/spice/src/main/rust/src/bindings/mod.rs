//! The SPICE backend's JNI surface: the entry points `SpiceNative` declares,
//! and the session behind them.
//!
//! `libremotedesktop_spice.so` is this crate, and this module is everything in
//! it that is not the protocol — it is compiled for Android and nowhere else.
//! What it needs of Android rather than of SPICE — a locked bitmap, the
//! callbacks into Java, a place to wait for a person — is `common::android`.

mod session;

use crate::client::{Config, compression_of};
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

/// The protocol version this client speaks, and the one call that proves the
/// whole path.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_spice_SpiceNative_nativeVersion<
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
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_spice_SpiceNative_nativeCreate(
    mut env: EnvUnowned,
    _class: JClass,
    listener: JObject,
    address: JString,
    tls: jboolean,
    password: JString,
    compression: JString,
    view_only: jboolean,
    connect_timeout_ms: jint,
) -> jlong {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("Spice"),
    );

    env.with_env(|env| -> Result<jlong, jni::errors::Error> {
        let callbacks = Callbacks::new(
            env,
            &listener,
            jni::jni_str!("net/pgaskin/remotedesktop/backend/spice/SpiceNative$Callbacks"),
        )?;
        let config = Config {
            address: string(env, &address)?.unwrap_or_default(),
            tls,
            password: string(env, &password)?,
            connect_timeout: Duration::from_millis(connect_timeout_ms.max(1000) as u64),
            compression: string(env, &compression)?
                .as_deref()
                .and_then(compression_of),
            view_only,
        };
        log::info!(
            "connecting to {} ({})",
            config.address,
            if tls { "TLS" } else { "plain" }
        );
        Ok(Box::into_raw(Box::new(Session::start(callbacks, config))) as jlong)
    })
    .resolve::<LogErrorAndDefault>()
}

/// A `null` password cancels, which ends the session — the only answer a
/// `Prompt.Credentials` has besides a secret.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_spice_SpiceNative_nativeAnswerPassword(
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

/// Whether the certificate the TLS handshake presented is one this connection
/// may go on with. Only a TLS session ever waits for it.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_spice_SpiceNative_nativeAnswerTrust(
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
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_spice_SpiceNative_nativeDisconnect(
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
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_spice_SpiceNative_nativeDestroy(
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

/// Absolute desktop coordinates and an RFB button mask, for a session where
/// this end owns the cursor.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_spice_SpiceNative_nativePointer(
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

/// How far the pointer moved rather than where it is, for a session where the
/// guest owns the cursor — which is what a QEMU with no tablet attached offers.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_spice_SpiceNative_nativePointerRelative(
    _env: EnvUnowned,
    _class: JClass,
    handle: jlong,
    dx: jint,
    dy: jint,
    button_mask: jint,
) {
    if let Some(session) = unsafe { session(handle) } {
        session.client.pointer_relative(dx, dy, button_mask & 0x1ff);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_spice_SpiceNative_nativePointerIsRelative(
    _env: EnvUnowned,
    _class: JClass,
    handle: jlong,
) -> jboolean {
    match unsafe { session(handle) } {
        Some(session) => session.client.pointer_is_relative(),
        None => false,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_spice_SpiceNative_nativeKeyDown(
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
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_spice_SpiceNative_nativeKeyUp(
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
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_spice_SpiceNative_nativeReleaseAllKeys(
    _env: EnvUnowned,
    _class: JClass,
    handle: jlong,
) {
    if let Some(session) = unsafe { session(handle) } {
        session.client.release_all_keys();
    }
}

/// Whether the session is on screen. There is no pause message in this
/// protocol, so what this does is stop answering the ack window — which is the
/// server's own reason to stop sending.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_spice_SpiceNative_nativeFocus(
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
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_spice_SpiceNative_nativeViewOnly(
    _env: EnvUnowned,
    _class: JClass,
    handle: jlong,
    view_only: jboolean,
) {
    if let Some(session) = unsafe { session(handle) } {
        session.client.set_view_only(view_only);
    }
}

/// The image compression the server is asked for, live: one message, and the
/// next image it encodes uses it.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_spice_SpiceNative_nativeSetCompression(
    mut env: EnvUnowned,
    _class: JClass,
    handle: jlong,
    compression: JString,
) {
    let Some(session) = (unsafe { session(handle) }) else {
        return;
    };
    env.with_env(|env| -> Result<(), jni::errors::Error> {
        if let Some(compression) = string(env, &compression)?.as_deref().and_then(compression_of) {
            session.client.set_compression(compression);
        }
        Ok(())
    })
    .resolve::<LogErrorAndDefault>();
}

/// What this phone has on its clipboard, offered to the guest's agent. The text
/// itself crosses when the guest asks for it, which is the agent's own shape
/// and not this seam's.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_spice_SpiceNative_nativeClipboard(
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
            session.client.set_clipboard(text);
        }
        Ok(())
    })
    .resolve::<LogErrorAndDefault>();
}

/// Whether the guest will take a desktop size right now, which for this
/// protocol is whether it is running the agent and said so.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_spice_SpiceNative_nativeCanResize(
    _env: EnvUnowned,
    _class: JClass,
    handle: jlong,
) -> jboolean {
    unsafe { session(handle) }.is_some_and(|session| session.client.can_resize())
}

/// Ask the guest for a desktop this size. What actually happened arrives as a
/// desktop size or as nothing at all.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_spice_SpiceNative_nativeRequestDesktopSize(
    _env: EnvUnowned,
    _class: JClass,
    handle: jlong,
    width: jint,
    height: jint,
) {
    if let Some(session) = unsafe { session(handle) }
        && width > 0
        && height > 0
    {
        session
            .client
            .request_desktop_size(width as u32, height as u32);
    }
}

/// The far end's monitors, four numbers each — x, y, width and height in its
/// own desktop — or null where it has not published a layout.
///
/// Announced rather than inferred, which is what makes SPICE different from
/// every other protocol here: `MONITORS_CONFIG` says which part of the surface
/// is which screen instead of leaving a client to guess at a boundary.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_spice_SpiceNative_nativeMonitors<
    'local,
>(
    mut env: EnvUnowned<'local>,
    _class: JClass,
    handle: jlong,
) -> jintArray {
    let Some(session) = (unsafe { session(handle) }) else {
        return std::ptr::null_mut();
    };
    let flat = session.client.monitors();
    if flat.is_empty() {
        return std::ptr::null_mut();
    }
    env.with_env(|env| -> Result<JIntArray<'local>, jni::errors::Error> {
        let array = env.new_int_array(flat.len())?;
        array.set_region(env, 0, &flat)?;
        Ok(array)
    })
    .resolve::<LogErrorAndDefault>()
    .into_raw()
}

/// Received and sent, in bytes, over all four of a session's connections —
/// protocol bytes, inside whatever is encrypting them and outside what the link
/// adds.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_spice_SpiceNative_nativeTraffic<
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

/// Everything the connection panel can be told, in the order `SpiceBackend`
/// reads it back out.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_spice_SpiceNative_nativeInfo<
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
            info.channels.as_str(),
            info.agent.as_str(),
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
/// side. The write side is held for one blit at a time.
#[unsafe(no_mangle)]
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_spice_SpiceNative_nativeReadRegion(
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
pub extern "system" fn Java_net_pgaskin_remotedesktop_backend_spice_SpiceNative_nativeReadThumbnail(
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
