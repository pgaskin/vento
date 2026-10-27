// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! Any `api::Backend` as the natives `NativeBackend.java` calls: the one
//! JNI layer every backend but RealVNC's is reached through on the phone.
//!
//! A backend's library is its protocol, its `api::Backend` adapter (the
//! same one the Linux client holds) and one line naming the adapter to this
//! module, [`natives!`](crate::natives), which writes the `JNI_OnLoad` that
//! registers [`METHODS`] on the module's own natives class. Registered rather
//! than exported, because the natives are the same functions in every
//! library: a `Java_…` symbol per backend would need a copy of them per
//! backend, and one class for all of them would be one class every library
//! registers on, the last answering for all, which with the RustDesk backend
//! (a separate APK loaded into the app) would depend on load order. So each
//! module has a class of its own,
//! `vento-codegen natives` writes it from [`surface`](crate::surface), and
//! the library registers on that.
//!
//! For the four backends written in Rust the protocol is the crate and the
//! library is a `cdylib` of it. For the three in C the protocol is a C
//! library behind the vtable in `vento/backend.h`, the adapter is
//! `api::c` and a small crate beside the C, and that crate is a static
//! library Meson links into the C's own `.so`, whose one export is the
//! `JNI_OnLoad` written here. The questions only the C API asks (a question in
//! the library's words, a message, RealVNC's security prompt) come through
//! here like any other event.
//!
//! Decisions about a session, as opposed to what is shown, are made here, by
//! the rules the Linux client uses too: a far end's identity against the pins
//! (`api::trust`, over the phone's `known_hosts` preferences, which
//! `NativeBackend` reads and writes for it), and the name an RDP credentials
//! prompt starts with (`api::credentials`). What crosses to Java is the
//! question, when there is one.
//!
//! The handle Java holds is a `Box<Handle>`: the backend, and the mailbox its
//! events go through. Java retires the handle under its own lock before
//! `destroy` frees it, so the drawing thread is never inside a pixel read of
//! a freed session; `destroy` joins the session's thread, which proves no
//! event is still being delivered.
//!
//! Events cross as they are taken from the session's [`Mailbox`], on whichever
//! thread put them there (nearly always the protocol thread), and in the
//! order they were put. They get the mailbox's own coalescing, which is the
//! reason for using it here: damage is marked into a list and crosses once per
//! frame, as one `int[]`, rather than once per rectangle, and a cursor, a
//! pointer mode or a clipboard that was superseded before it was read never
//! crosses at all.

use crate::{bitmap, string};
pub use runtime::media::audio::Outputs;
use runtime::media::audio::Switch;
pub use runtime::media::video::Decoders;
use jni::errors::{Error, LogErrorAndDefault};
use jni::objects::{Global, JIntArray, JLongArray, JMethodID, JObject, JObjectArray, JString};
use jni::signature::{Primitive, ReturnType};
use jni::strings::JNIStr;
use jni::sys::{jboolean, jint, jlong, jvalue};
use jni::{Env, JavaVM, NativeMethod, jni_sig, jni_str, native_method};
use api::backend::{Backend, Credentials, Field};
use api::events::{Event, Identity, Mailbox};
use api::kind::{Connect, Dirs, Kind, Platform};
use api::options::Options;
use api::trust::{self, Decision, Pinned};
use std::cell::RefCell;
use std::ffi::CStr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

/// The backend a library holds, as [`natives!`](crate::natives) named it.
pub struct Module {
    pub kind: &'static Kind,
    /// What decodes the protocol's video, for the backends that have some:
    /// the phone's MediaCodec for RustDesk, and for TigerVNC's and FreeRDP's
    /// H.264; nothing for the others.
    pub decoders: fn() -> Option<Arc<dyn Decoders>>,
    /// Where the far end's sound goes, for the backends that have sound:
    /// AAudio for TigerVNC, FreeRDP, IronRDP, SPICE and RustDesk, whose
    /// compressed sound MediaCodec's audio decoders decode; nothing for the
    /// others. Called once per session, since an output's controls are its
    /// session's.
    ///
    /// What a library links follows from these only for the Rust ones: a
    /// cdylib that names neither (RFB's) has no `libmediandk.so` or
    /// `libaaudio.so` among its NEEDED, and SPICE's and IronRDP's have both
    /// for their sound alone. A C backend's library is linked by
    /// `vento_android_library`, which names both for every one (libvnc's
    /// included).
    pub audio: fn() -> Option<Arc<dyn Outputs>>,
}

/// One per library: each `.so` has its own copy of this crate, statics and
/// all, which is what lets the natives be the same functions everywhere and
/// still know which backend they are in front of.
static MODULE: OnceLock<Module> = OnceLock::new();

/// Writes the library's `JNI_OnLoad`: register [`METHODS`] on `class`, the
/// module's natives class, for `kind`.
///
/// ```ignore
/// platform_android::natives!(
///     class = c"net/pgaskin/remotedesktop/backend/rfb/RfbNatives",
///     kind = crate::backend::KIND,
/// );
/// ```
///
/// `decoders = <fn() -> Option<Arc<dyn Decoders>>>` hands the session a video
/// decoder, for a backend that needs one, and `audio = <fn() ->
/// Option<Arc<dyn Outputs>>>` a sound output likewise. `on_load = <unsafe
/// fn(*mut RawVm)>` is run first, with the runtime's `JavaVM`, for a library
/// linked into the same `.so` that wanted a `JNI_OnLoad` of its own: there can
/// be only one, and FreeRDP's WinPR takes the `JavaVM` it reads the time zone
/// through from its own.
#[macro_export]
macro_rules! natives {
    (class = $class:expr, kind = $kind:expr $(, decoders = $decoders:expr)? $(, audio = $audio:expr)? $(, on_load = $on_load:expr)? $(,)?) => {
        /// Registers this library's natives on its module's class. Called by
        /// the runtime on the thread that loaded the library, which is what
        /// lets `FindClass` see the module's class loader.
        #[unsafe(no_mangle)]
        // The runtime's entry point: what it is handed is the runtime's own.
        #[allow(clippy::not_unsafe_ptr_arg_deref)]
        pub extern "system" fn JNI_OnLoad(
            vm: *mut $crate::backend::RawVm,
            _reserved: *mut ::std::ffi::c_void,
        ) -> i32 {
            #[allow(unused_mut, unused_assignments)]
            let mut decoders: fn() -> Option<::std::sync::Arc<dyn $crate::backend::Decoders>> = || None;
            $(decoders = $decoders;)?
            #[allow(unused_mut, unused_assignments)]
            let mut audio: fn() -> Option<::std::sync::Arc<dyn $crate::backend::Outputs>> = || None;
            $(audio = $audio;)?
            // SAFETY: `vm` is the runtime's own, which is all either asks.
            $(unsafe { ($on_load)(vm) };)?
            unsafe { $crate::backend::on_load(vm, $class, $crate::backend::Module { kind: &$kind, decoders, audio }) }
        }
    };
}

#[doc(hidden)]
pub use jni::sys::JavaVM as RawVm;

/// What [`natives!`](crate::natives)' `JNI_OnLoad` does.
///
/// # Safety
///
/// `vm` must be the `JavaVM` the runtime handed `JNI_OnLoad`.
#[doc(hidden)]
pub unsafe fn on_load(vm: *mut jni::sys::JavaVM, class: &'static CStr, module: Module) -> i32 {
    let Some(name) = JNIStr::from_cstr(class) else {
        return jni::sys::JNI_ERR;
    };
    if MODULE.set(module).is_err() {
        // A library is loaded once per class loader; a second load of the
        // same one would be registering the same functions again.
        return jni::sys::JNI_VERSION_1_6;
    }
    let vm = unsafe { JavaVM::from_raw(vm) };
    let registered: jni::errors::Result<()> = vm.attach_current_thread(|env| {
        let class = env.find_class(name)?;
        unsafe { env.register_native_methods(&class, METHODS) }
    });
    match registered {
        Ok(()) => jni::sys::JNI_VERSION_1_6,
        Err(e) => {
            // Logged where it can be seen: a library that loads and registers
            // nothing is an UnsatisfiedLinkError on the first call, which says
            // less than this.
            crate::log::init("NativeBackend");
            log::error!("registering natives on {class:?}: {e}");
            jni::sys::JNI_ERR
        }
    }
}

fn module() -> &'static Module {
    MODULE.get().expect("JNI_OnLoad ran")
}

/// What a handle points at.
struct Handle {
    backend: Arc<dyn Backend>,
    /// The session's sound output, also the backend's: its controls are what
    /// `setMuted` mutes and the panel's sound row is read from. `None` for a
    /// library whose protocol has no sound.
    audio: Option<Arc<dyn Outputs>>,
    /// The session's network, also the backend's: the panel's Tunnel row says
    /// what carries its connections.
    network: Arc<dyn runtime::net::Network>,
    /// Kept so that a handle outlives every event it could still deliver; the
    /// outlet holds only a weak reference back to it.
    _mailbox: Arc<Mailbox>,
    /// Where an answer to a trust question is pinned through.
    outlet: Arc<Outlet>,
}

/// SAFETY: `handle` must be one `create` returned that `destroy` has not been
/// called for. Zero is the ordinary "already retired" case.
unsafe fn handle<'a>(handle: jlong) -> Option<&'a Handle> {
    if handle == 0 {
        None
    } else {
        Some(unsafe { &*(handle as *const Handle) })
    }
}

/// SAFETY: as [`handle`].
unsafe fn backend<'a>(h: jlong) -> Option<&'a dyn Backend> {
    unsafe { handle(h) }.map(|h| h.backend.as_ref())
}

/// The table [`on_load`] registers, one entry per [`surface::NATIVES`] entry.
/// Each entry is checked against its function when it compiles, and its name
/// and descriptor against the generated Java by `scripts/check-jni-symbols.sh`.
///
/// [`surface::NATIVES`]: crate::surface::NATIVES
pub const METHODS: &[NativeMethod] = &[
    native_method! { fn version() -> JString, error_policy = LogErrorAndDefault },
    native_method! {
        fn create(
            events: net.pgaskin.remotedesktop.backend.NativeBackend,
            tag: JString,
            address: JString,
            user_name: JString,
            password: JString,
            options: [JString],
            screen_width: jint,
            screen_height: jint,
            last_width: jint,
            last_height: jint,
            client_name: JString,
            files_dir: JString,
        ) -> jlong,
        error_policy = LogErrorAndDefault,
    },
    native_method! {
        fn answer_credentials(handle: jlong, user_name: JString, password: JString),
        error_policy = LogErrorAndDefault,
    },
    native_method! { fn answer_trust(handle: jlong, accept: jboolean), error_policy = LogErrorAndDefault },
    native_method! {
        fn answer_pin(handle: jlong, fingerprint: JString, accept: jboolean, remember: jboolean),
        error_policy = LogErrorAndDefault,
    },
    native_method! { fn disconnect(handle: jlong), error_policy = LogErrorAndDefault },
    native_method! { fn destroy(handle: jlong), error_policy = LogErrorAndDefault },
    native_method! {
        fn pointer(handle: jlong, x: jint, y: jint, button_mask: jint),
        error_policy = LogErrorAndDefault,
    },
    native_method! {
        fn pointer_relative(handle: jlong, dx: jint, dy: jint, button_mask: jint),
        error_policy = LogErrorAndDefault,
    },
    native_method! {
        fn key_down(handle: jlong, keysym: jint, key_id: jlong),
        error_policy = LogErrorAndDefault,
    },
    native_method! { fn key_up(handle: jlong, key_id: jlong), error_policy = LogErrorAndDefault },
    native_method! { fn release_all_keys(handle: jlong), error_policy = LogErrorAndDefault },
    native_method! { fn focus(handle: jlong, focused: jboolean), error_policy = LogErrorAndDefault },
    native_method! { fn clipboard(handle: jlong, text: JString), error_policy = LogErrorAndDefault },
    native_method! {
        fn set_option(handle: jlong, key: JString, value: JString),
        error_policy = LogErrorAndDefault,
    },
    native_method! {
        fn request_desktop_size(handle: jlong, width: jint, height: jint),
        error_policy = LogErrorAndDefault,
    },
    native_method! {
        fn request_display(handle: jlong, index: jint),
        error_policy = LogErrorAndDefault,
    },
    native_method! { fn set_muted(handle: jlong, muted: jboolean), error_policy = LogErrorAndDefault },
    native_method! {
        fn set_sound_enabled(handle: jlong, enabled: jboolean),
        error_policy = LogErrorAndDefault,
    },
    native_method! { fn facts(handle: jlong) -> [jint], error_policy = LogErrorAndDefault },
    native_method! { fn info(handle: jlong) -> [JString], error_policy = LogErrorAndDefault },
    native_method! { fn sound_line(handle: jlong) -> JString, error_policy = LogErrorAndDefault },
    native_method! { fn traffic(handle: jlong) -> [jlong], error_policy = LogErrorAndDefault },
    native_method! {
        raw fn read_region(
            handle: jlong,
            x: jint,
            y: jint,
            width: jint,
            height: jint,
            dst: android.graphics.Bitmap,
            dst_x: jint,
            dst_y: jint,
        ) -> jboolean,
    },
    native_method! {
        raw fn read_thumbnail(handle: jlong, step: jint, dst: android.graphics.Bitmap) -> jboolean,
    },
];

/// The library's version: the one call that proves the whole path works.
fn version<'local>(env: &mut Env<'local>, _this: JObject<'local>) -> Result<JString<'local>, Error> {
    env.new_string((module().kind.version)())
}

/// Start a session. Returns a handle at once, or zero where the backend
/// could not even start; everything after this arrives on `events`.
///
/// `options` is the connection's options as key, value, key, value, with
/// the catalog's defaults filled in. `lastWidth` × `lastHeight` is the size
/// the far end was left at last time where the app stores it, and zero where
/// it does not.
#[allow(clippy::too_many_arguments)]
fn create<'local>(
    env: &mut Env<'local>,
    _this: JObject<'local>,
    events: JObject<'local>,
    tag: JString<'local>,
    address: JString<'local>,
    user_name: JString<'local>,
    password: JString<'local>,
    options: JObjectArray<'local, JString<'local>>,
    screen_width: jint,
    screen_height: jint,
    last_width: jint,
    last_height: jint,
    client_name: JString<'local>,
    files_dir: JString<'local>,
) -> Result<jlong, Error> {
    crate::log::init(&string(env, &tag)?.unwrap_or_else(|| "NativeBackend".into()));
    let module = module();
    let kind = module.kind;

    let mut o = Options::defaults(kind.options);
    if !options.is_null() {
        let n = options.len(env)?;
        for i in (0..n.saturating_sub(1)).step_by(2) {
            let key = options.get_element(env, i)?;
            let value = options.get_element(env, i + 1)?;
            if let Some(key) = string(env, &key)? {
                o.set(&key, &string(env, &value)?.unwrap_or_default());
            }
        }
    }

    let address = string(env, &address)?.unwrap_or_default();
    let user = string(env, &user_name)?.unwrap_or_default();

    let asks = Asks {
        pinned: trust::pinned_as(kind, &address, &o),
        prompt_user: api::credentials::prompt_user(kind, &user),
    };
    let outlet = Arc::new(Outlet::new(env, &events, asks)?);
    let wake = Arc::clone(&outlet);
    let mailbox = Mailbox::new(Some(Box::new(move || wake.wake())));
    let _ = outlet.mailbox.set(Arc::downgrade(&mailbox));

    // Empty is not a password: something that looks like a stored secret
    // and is not one fails the connection instead of asking for the real one.
    let password = string(env, &password)?;
    let client_name = string(env, &client_name)?.unwrap_or_else(|| "Android".into());
    let files = PathBuf::from(string(env, &files_dir)?.unwrap_or_default());
    let dirs = Dirs {
        config: files.clone(),
        state: files,
        realvnc_libs: PathBuf::new(),
    };
    let pair = |w: jint, h: jint| (w > 0 && h > 0).then_some((w as u32, h as u32));
    let audio = (module.audio)();
    let network: Arc<dyn runtime::net::Network> = Arc::new(runtime::net::Direct);
    let connect = Connect {
        address: &address,
        user: &user,
        domain: "",
        password: password.as_deref(),
        options: &o,
        mailbox: Arc::clone(&mailbox),
        dirs: &dirs,
        screen: pair(screen_width, screen_height),
        last_size: pair(last_width, last_height),
        // RealVNC's identities are its Java backend's, the one backend on the
        // phone that is not behind this layer.
        identity: None,
        client_name: &client_name,
        platform: Platform::Android,
        decoders: (module.decoders)(),
        audio: audio.clone(),
        network: Arc::clone(&network),
    };
    match kind.start(&connect) {
        Ok(backend) => {
            outlet.started(&backend);
            Ok(Box::into_raw(Box::new(Handle {
                backend,
                audio,
                network,
                _mailbox: mailbox,
                outlet,
            })) as jlong)
        }
        Err(why) => {
            log::warn!("{}: {why}", kind.id);
            Ok(0)
        }
    }
}

/// Where a session's connections go: [`Direct`](runtime::net::Direct), as on
/// Linux, or with `force_socketpair` each connection relayed through a
/// socketpair.
///
/// Direct is this platform's network too. Its sockets are bound to no
/// network, so they go where the app's go: the default network, or the VPN
/// where one applies to the app. Its lookup is bionic's `getaddrinfo`, which
/// is the resolver's whole lookup (search domains, the hosts file, `.local`
/// names over mDNS, the order of the addresses). `android_res_nquery` could
/// be cancelled, but it sends one query for the name as given and leaves the
/// rest to the caller; Direct already stops waiting for a lookup at a
/// hang-up. Also the RealVNC module's, whose backend is not behind this
/// layer.
pub fn network(force_socketpair: bool) -> Arc<dyn runtime::net::Network> {
    let direct = Arc::new(runtime::net::Direct);
    if force_socketpair {
        log::info!("connections dialled through vento are relayed through a socketpair (developer setting)");
        Arc::new(runtime::net::Socketpair::new(direct))
    } else {
        direct
    }
}

/// A `null` or empty password cancels, which ends the session: the only
/// answer a prompt has besides a secret. The user name is what was typed,
/// empty for none; the adapter fills it from the session's and, for RDP,
/// splits a domain out of it.
fn answer_credentials<'local>(
    env: &mut Env<'local>,
    _this: JObject<'local>,
    handle: jlong,
    user_name: JString<'local>,
    password: JString<'local>,
) -> Result<(), Error> {
    let Some(backend) = (unsafe { backend(handle) }) else {
        return Ok(());
    };
    let user = string(env, &user_name)?.unwrap_or_default();
    // Empty is a cancel too, as it is for a stored password: nothing that
    // looks like a secret and is not one goes to the far end.
    let password = string(env, &password)?;
    backend.answer_credentials(password.map(|password| Credentials {
        user,
        domain: String::new(),
        password,
    }));
    Ok(())
}

fn answer_trust<'local>(_env: &mut Env<'local>, _this: JObject<'local>, handle: jlong, accept: jboolean) -> Result<(), Error> {
    if let Some(backend) = unsafe { backend(handle) } {
        backend.answer_trust(accept);
    }
    Ok(())
}

/// The user's answer to a trust question [`Outlet`] asked: the session is
/// answered, and the identity pinned where the user accepted it and chose to
/// remember it.
fn answer_pin<'local>(
    env: &mut Env<'local>,
    _this: JObject<'local>,
    handle: jlong,
    fingerprint: JString<'local>,
    accept: jboolean,
    remember: jboolean,
) -> Result<(), Error> {
    let Some(handle) = (unsafe { self::handle(handle) }) else {
        return Ok(());
    };
    let fingerprint = string(env, &fingerprint)?.unwrap_or_default();
    let outlet = &handle.outlet;
    let pins = Pins::new(outlet, env);
    let accept = trust::answered(&pins, &outlet.asks.pinned, &fingerprint, accept, remember);
    handle.backend.answer_trust(accept);
    Ok(())
}

fn disconnect<'local>(_env: &mut Env<'local>, _this: JObject<'local>, handle: jlong) -> Result<(), Error> {
    if let Some(backend) = unsafe { backend(handle) } {
        backend.disconnect();
    }
    Ok(())
}

/// Ends the session, waits for its thread, and frees the handle. Nothing may
/// use it afterwards, which Java guarantees by retiring it first.
fn destroy<'local>(_env: &mut Env<'local>, _this: JObject<'local>, handle: jlong) -> Result<(), Error> {
    if handle != 0 {
        let handle = unsafe { Box::from_raw(handle as *mut Handle) };
        handle.backend.destroy();
    }
    Ok(())
}

fn pointer<'local>(
    _env: &mut Env<'local>,
    _this: JObject<'local>,
    handle: jlong,
    x: jint,
    y: jint,
    button_mask: jint,
) -> Result<(), Error> {
    if let Some(backend) = unsafe { backend(handle) } {
        backend.pointer(x, y, button_mask);
    }
    Ok(())
}

fn pointer_relative<'local>(
    _env: &mut Env<'local>,
    _this: JObject<'local>,
    handle: jlong,
    dx: jint,
    dy: jint,
    button_mask: jint,
) -> Result<(), Error> {
    if let Some(backend) = unsafe { backend(handle) } {
        backend.pointer_relative(dx, dy, button_mask);
    }
    Ok(())
}

fn key_down<'local>(
    _env: &mut Env<'local>,
    _this: JObject<'local>,
    handle: jlong,
    keysym: jint,
    key_id: jlong,
) -> Result<(), Error> {
    if let Some(backend) = unsafe { backend(handle) } {
        backend.key_down(keysym as u32, key_id);
    }
    Ok(())
}

fn key_up<'local>(_env: &mut Env<'local>, _this: JObject<'local>, handle: jlong, key_id: jlong) -> Result<(), Error> {
    if let Some(backend) = unsafe { backend(handle) } {
        backend.key_up(key_id);
    }
    Ok(())
}

fn release_all_keys<'local>(_env: &mut Env<'local>, _this: JObject<'local>, handle: jlong) -> Result<(), Error> {
    if let Some(backend) = unsafe { backend(handle) } {
        backend.release_all_keys();
    }
    Ok(())
}

fn focus<'local>(_env: &mut Env<'local>, _this: JObject<'local>, handle: jlong, focused: jboolean) -> Result<(), Error> {
    if let Some(backend) = unsafe { backend(handle) } {
        backend.focus(focused);
    }
    Ok(())
}

fn clipboard<'local>(env: &mut Env<'local>, _this: JObject<'local>, handle: jlong, text: JString<'local>) -> Result<(), Error> {
    if let Some(backend) = unsafe { backend(handle) }
        && let Some(text) = string(env, &text)?
    {
        backend.clipboard(&text);
    }
    Ok(())
}

/// One option, changed on the running session. The adapter decides what is
/// live and what is remembered for next time.
fn set_option<'local>(
    env: &mut Env<'local>,
    _this: JObject<'local>,
    handle: jlong,
    key: JString<'local>,
    value: JString<'local>,
) -> Result<(), Error> {
    if let Some(backend) = unsafe { backend(handle) }
        && let Some(key) = string(env, &key)?
    {
        backend.set_option(&key, &string(env, &value)?.unwrap_or_default());
    }
    Ok(())
}

/// Mutes or unmutes the session's sound at once: the output's controls,
/// which every stream of the session plays under and the device applies as it
/// takes the samples. The session screen's sound button; logged, since a
/// muted stream is otherwise only silence.
fn set_muted<'local>(_env: &mut Env<'local>, _this: JObject<'local>, handle: jlong, muted: jboolean) -> Result<(), Error> {
    if let Some(audio) = unsafe { self::handle(handle) }.and_then(|h| h.audio.as_ref()) {
        audio.controls().set_muted(muted);
        log::info!("sound {}", if muted { "muted" } else { "unmuted" });
    }
    Ok(())
}

/// The session's Enable switch: asks the far end to stop sending its sound,
/// or to send it again, where the backend offered that
/// (`runtime::media::audio::Switch::Enable`, reported through `onSound`). A
/// switch that asks nothing (a mute, or none) is left alone, and that is
/// logged.
fn set_sound_enabled<'local>(
    _env: &mut Env<'local>,
    _this: JObject<'local>,
    handle: jlong,
    enabled: jboolean,
) -> Result<(), Error> {
    if let Some(audio) = unsafe { self::handle(handle) }.and_then(|h| h.audio.as_ref()) {
        if audio.controls().set_enabled(enabled) {
            log::info!("asking the far end {}", if enabled { "for its sound" } else { "to stop its sound" });
        } else {
            log::info!("sound switch: nothing to ask the far end");
        }
    }
    Ok(())
}

fn request_desktop_size<'local>(
    _env: &mut Env<'local>,
    _this: JObject<'local>,
    handle: jlong,
    width: jint,
    height: jint,
) -> Result<(), Error> {
    if let Some(backend) = unsafe { backend(handle) } {
        backend.request_desktop_size(width.max(0) as u32, height.max(0) as u32);
    }
    Ok(())
}

fn request_display<'local>(_env: &mut Env<'local>, _this: JObject<'local>, handle: jlong, index: jint) -> Result<(), Error> {
    if let Some(backend) = unsafe { backend(handle) } {
        backend.request_display(index);
    }
    Ok(())
}

/// `api::backend::Facts` as ints, in one crossing: width, height, the
/// current display, flags (1 can resize, 2 view only, 4 pointer relative),
/// how many monitors, then four ints per monitor and four per display:
/// x, y, width, height, `Monitor.fromFlat`'s format.
fn facts<'local>(env: &mut Env<'local>, _this: JObject<'local>, handle: jlong) -> Result<JIntArray<'local>, Error> {
    let Some(backend) = (unsafe { backend(handle) }) else {
        return Ok(JIntArray::null());
    };
    let f = backend.facts();
    let flags = f.can_resize as jint | (f.view_only as jint) << 1 | (f.pointer_relative as jint) << 2;
    let mut flat = vec![
        f.width as jint,
        f.height as jint,
        f.display,
        flags,
        f.monitors.len() as jint,
    ];
    for m in f.monitors.iter().chain(&f.displays) {
        flat.extend([m.x, m.y, m.width, m.height]);
    }
    let array = env.new_int_array(flat.len())?;
    array.set_region(env, 0, &flat)?;
    Ok(array)
}

/// The connection panel's rows, four strings each: the field's name as
/// `ConnectionFact.Field` spells it, the label, the value, and `"d"` for a
/// diagnostic or empty.
fn info<'local>(
    env: &mut Env<'local>,
    _this: JObject<'local>,
    handle: jlong,
) -> Result<JObjectArray<'local, JString<'local>>, Error> {
    let Some(h) = (unsafe { self::handle(handle) }) else {
        return Ok(JObjectArray::<JString>::null());
    };
    let mut rows = h.backend.info();
    // The session's sound, which the client holds rather than the backend,
    // in the words the Linux client's panel uses.
    if let Some(audio) = &h.audio
        && !rows.is_empty()
    {
        api::backend::with_sound(&mut rows, audio.controls());
    }
    // What carries the connections, where it is not this end's own: last,
    // in a group of its own.
    if !rows.is_empty() {
        api::backend::with_network(&mut rows, &*h.network);
    }
    let array = JObjectArray::<JString>::new(env, rows.len() * 4, JString::null())?;
    for (i, row) in rows.iter().enumerate() {
        let cells = [field(row.field), row.label, &row.value, if row.diagnostic { "d" } else { "" }];
        for (j, cell) in cells.iter().enumerate() {
            let s = env.new_string(*cell)?;
            array.set_element(env, i * 4 + j, &s)?;
        }
    }
    Ok(array)
}

/// The debug overlay's sound line, in the Linux client's format
/// (`runtime::media::audio::Sound::line`): what plays and how far behind, or
/// "nothing playing", and DISABLED or MUTED. Null where the session has no
/// sound switch, which is when an overlay has no such line.
fn sound_line<'local>(env: &mut Env<'local>, _this: JObject<'local>, handle: jlong) -> Result<JString<'local>, Error> {
    let line = unsafe { self::handle(handle) }
        .and_then(|h| h.audio.as_ref())
        .and_then(|audio| audio.controls().sound().line(audio.controls().playback()));
    match line {
        Some(line) => env.new_string(line),
        None => Ok(JString::null()),
    }
}

fn field(field: Field) -> &'static str {
    match field {
        Field::DesktopName => "DESKTOP_NAME",
        Field::Connection => "CONNECTION",
        Field::Security => "SECURITY",
        Field::Protocol => "PROTOCOL",
        Field::Encoding => "ENCODING",
        Field::LineSpeed => "LINE_SPEED",
        Field::Data => "DATA",
        Field::ServerPixels => "SERVER_PIXELS",
        Field::ViewerPixels => "VIEWER_PIXELS",
        Field::Other => "OTHER",
        Field::Tunnel => "TUNNEL",
    }
}

/// Received and sent, in bytes, with -1 for an uncounted direction; `null`
/// while there is nothing to count. Raw counts rather than the row, because
/// the row is worded in the phone's own number format.
fn traffic<'local>(env: &mut Env<'local>, _this: JObject<'local>, handle: jlong) -> Result<JLongArray<'local>, Error> {
    let Some(traffic) = (unsafe { backend(handle) }).and_then(|b| b.traffic()) else {
        return Ok(JLongArray::null());
    };
    let both = [traffic.received as jlong, traffic.sent.map_or(-1, |s| s as jlong)];
    let array = env.new_long_array(both.len())?;
    array.set_region(env, 0, &both)?;
    Ok(array)
}

/// Copy a desktop rectangle into `dst` at `(dst_x, dst_y)`, 1:1, leaving the
/// rest of `dst` alone: straight into the bitmap's rows, with no buffer in
/// between.
///
/// Called from the drawing thread, concurrently with everything else, which
/// every adapter is written for: the framebuffer is behind a reader-writer
/// lock and this takes the read side.
#[allow(clippy::too_many_arguments)]
fn read_region<'local>(
    env: jni::EnvUnowned<'local>,
    _this: JObject<'local>,
    handle: jlong,
    x: jint,
    y: jint,
    width: jint,
    height: jint,
    dst: JObject<'local>,
    dst_x: jint,
    dst_y: jint,
) -> jboolean {
    let Some(backend) = (unsafe { backend(handle) }) else {
        return false;
    };
    if x < 0 || y < 0 || width <= 0 || height <= 0 || dst_x < 0 || dst_y < 0 {
        return false;
    }
    let Some(mut locked) = (unsafe { bitmap::Locked::new(env.as_raw(), dst.as_raw()) }) else {
        return false;
    };
    let (width, height, dst_x, dst_y) = (width as usize, height as usize, dst_x as usize, dst_y as usize);
    if locked.width < dst_x + width || locked.height < dst_y + height {
        return false;
    }
    let stride = locked.stride;
    backend.read_region(x as usize, y as usize, width, height, locked.bytes(), stride, dst_x, dst_y)
}

/// The whole desktop at `1/step`, into a bitmap Java has already sized.
fn read_thumbnail<'local>(
    env: jni::EnvUnowned<'local>,
    _this: JObject<'local>,
    handle: jlong,
    step: jint,
    dst: JObject<'local>,
) -> jboolean {
    let Some(backend) = (unsafe { backend(handle) }) else {
        return false;
    };
    if step <= 0 {
        return false;
    }
    let Some(picture) = backend.thumbnail(step as usize) else {
        return false;
    };
    let Some(mut locked) = (unsafe { bitmap::Locked::new(env.as_raw(), dst.as_raw()) }) else {
        return false;
    };
    if locked.width < picture.width || locked.height < picture.height {
        return false;
    }
    let stride = locked.stride;
    let row = picture.width * 4;
    let bytes = locked.bytes();
    for (y, src) in picture.rgba.chunks_exact(row).enumerate() {
        bytes[y * stride..y * stride + row].copy_from_slice(src);
    }
    true
}

/// What a session's questions are asked about, worked out once at `create`.
struct Asks {
    /// What its far end's identity is pinned under, and whether a first
    /// sight is.
    pinned: Pinned,
    /// The user name a credentials prompt starts with.
    prompt_user: String,
}

/// Where a session's events leave for Java: `NativeBackend`'s `on…` methods,
/// resolved once, and the mailbox they are drained from.
struct Outlet {
    vm: JavaVM,
    target: Global<JObject<'static>>,
    asks: Asks,
    /// The session, once `create` has it, for the trust questions answered
    /// here; and an answer decided before then, kept for it.
    started: Mutex<Started>,
    known_hosts_pins: JMethodID,
    known_hosts_pin: JMethodID,
    connected: JMethodID,
    desktop_size: JMethodID,
    frame: JMethodID,
    cursor: JMethodID,
    pointer_mode: JMethodID,
    bell: JMethodID,
    status: JMethodID,
    sound: JMethodID,
    clipboard: JMethodID,
    credentials_needed: JMethodID,
    prompt_withdrawn: JMethodID,
    trust_needed: JMethodID,
    unverified: JMethodID,
    question: JMethodID,
    message: JMethodID,
    security_needed: JMethodID,
    closed: JMethodID,
    mailbox: OnceLock<Weak<Mailbox>>,
    /// Held while a thread drains, so that two threads with events never
    /// deliver them out of order.
    draining: Mutex<()>,
    /// Set by a wake that found another thread draining, and read by that
    /// thread once it has let go, so that what it was woken for is not left
    /// waiting for the next event.
    requested: AtomicBool,
}

impl Outlet {
    /// Resolved on the creating thread, against the object's own class: the
    /// protocol thread that delivers later has no class loader that could
    /// find `NativeBackend` by name, and resolving per call would be the most
    /// expensive thing in the path.
    fn new(env: &mut Env, target: &JObject, asks: Asks) -> jni::errors::Result<Outlet> {
        let class = env.get_object_class(target)?;
        let id = |env: &mut Env, name, sig| env.get_method_id(&class, name, sig);
        Ok(Outlet {
            vm: env.get_java_vm()?,
            target: env.new_global_ref(target)?,
            asks,
            started: Mutex::new(Started::default()),
            known_hosts_pins: id(env, jni_str!("knownHostsPins"), jni_sig!("()[Ljava/lang/String;"))?,
            known_hosts_pin: id(
                env,
                jni_str!("knownHostsPin"),
                jni_sig!("(Ljava/lang/String;Ljava/lang/String;)V"),
            )?,
            connected: id(env, jni_str!("onConnected"), jni_sig!("(II)V"))?,
            desktop_size: id(env, jni_str!("onDesktopSize"), jni_sig!("(II)V"))?,
            frame: id(env, jni_str!("onFrame"), jni_sig!("([I)V"))?,
            cursor: id(env, jni_str!("onCursor"), jni_sig!("([IIIIIJ)V"))?,
            pointer_mode: id(env, jni_str!("onPointerMode"), jni_sig!("(Z)V"))?,
            bell: id(env, jni_str!("onBell"), jni_sig!("()V"))?,
            status: id(env, jni_str!("onStatus"), jni_sig!("(Ljava/lang/String;)V"))?,
            sound: id(env, jni_str!("onSound"), jni_sig!("(IZZZ)V"))?,
            clipboard: id(env, jni_str!("onClipboard"), jni_sig!("(Ljava/lang/String;)V"))?,
            credentials_needed: id(env, jni_str!("onCredentialsNeeded"), jni_sig!("(ZLjava/lang/String;)V"))?,
            prompt_withdrawn: id(env, jni_str!("onPromptWithdrawn"), jni_sig!("()V"))?,
            trust_needed: id(
                env,
                jni_str!("onTrustNeeded"),
                jni_sig!("(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)V"),
            )?,
            unverified: id(
                env,
                jni_str!("onUnverified"),
                jni_sig!("(Ljava/lang/String;Ljava/lang/String;)V"),
            )?,
            question: id(
                env,
                jni_str!("onQuestion"),
                jni_sig!("(Ljava/lang/String;Ljava/lang/String;)V"),
            )?,
            message: id(
                env,
                jni_str!("onMessage"),
                jni_sig!("(Ljava/lang/String;IZLjava/lang/String;)V"),
            )?,
            security_needed: id(env, jni_str!("onSecurityNeeded"), jni_sig!("(Ljava/lang/String;)V"))?,
            closed: id(env, jni_str!("onClosed"), jni_sig!("(Ljava/lang/String;)V"))?,
            mailbox: OnceLock::new(),
            draining: Mutex::new(()),
            requested: AtomicBool::new(false),
        })
    }

    /// The mailbox's wake: deliver everything waiting, on this thread.
    ///
    /// Each turn takes the events waiting when it began, under one lock, and
    /// delivers them. A wake on a thread that is already delivering (a
    /// callback that answers straight back into the session, which can
    /// queue another event) finds the lock held and sets `requested`, and
    /// the delivering thread takes the new event on its next turn; so does
    /// one on another thread. Nothing here blocks, which a protocol thread
    /// holding its own locks needs.
    fn wake(&self) {
        let Some(mailbox) = self.mailbox.get().and_then(Weak::upgrade) else {
            return;
        };
        self.requested.store(true, Ordering::SeqCst);
        loop {
            let Ok(guard) = self.draining.try_lock() else {
                return;
            };
            self.requested.store(false, Ordering::SeqCst);
            for event in mailbox.take_all() {
                self.deliver(event);
            }
            drop(guard);
            if !self.requested.load(Ordering::SeqCst) {
                return;
            }
        }
    }

    /// One event, as one call. A Java exception thrown out of a callback
    /// becomes a log line rather than an abort: the protocol thread is in
    /// the middle of a framebuffer update and unwinding it would lose the
    /// connection over a bug on the other side of the boundary.
    fn deliver(&self, event: Event) {
        let result: jni::errors::Result<()> = self.vm.attach_current_thread(|env| {
            let int = |v: usize| jvalue { i: v as jint };
            match event {
                Event::Connected { width, height } => self.call(env, self.connected, &[int(width), int(height)]),
                Event::DesktopSize { width, height } => {
                    self.call(env, self.desktop_size, &[int(width), int(height)])
                }
                Event::Frame { rects, .. } => {
                    let flat: Vec<jint> = rects
                        .iter()
                        .flat_map(|r| [r.x, r.y, r.width, r.height].map(|v| v as jint))
                        .collect();
                    let array = env.new_int_array(flat.len())?;
                    array.set_region(env, 0, &flat)?;
                    self.call(env, self.frame, &[jvalue { l: array.as_raw() }])
                }
                Event::Cursor(cursor) => {
                    // `Color`-packed for `Bitmap.setPixels`, and hashed here
                    // because the pixels are already in cache from the swap.
                    let (argb, w, h, hot_x, hot_y) = match cursor {
                        Some(c) => {
                            let argb: Vec<u32> = c
                                .rgba
                                .chunks_exact(4)
                                .map(|p| runtime::cursor::swap_rb(u32::from_le_bytes([p[0], p[1], p[2], p[3]])))
                                .collect();
                            (argb, c.width, c.height, c.hot_x, c.hot_y)
                        }
                        None => (Vec::new(), 0, 0, 0, 0),
                    };
                    let hash = runtime::cursor::hash(&argb, w, h) as jlong;
                    let ints: Vec<jint> = argb.iter().map(|&p| p as jint).collect();
                    let array = env.new_int_array(ints.len())?;
                    if !ints.is_empty() {
                        array.set_region(env, 0, &ints)?;
                    }
                    self.call(
                        env,
                        self.cursor,
                        &[
                            jvalue { l: array.as_raw() },
                            int(w),
                            int(h),
                            jvalue { i: hot_x },
                            jvalue { i: hot_y },
                            jvalue { j: hash },
                        ],
                    )
                }
                Event::PointerMode { relative } => self.call(env, self.pointer_mode, &[jvalue { z: relative }]),
                Event::Bell => self.call(env, self.bell, &[]),
                // Null clears it.
                Event::Status(text) => match text {
                    Some(text) => self.text(env, self.status, &text),
                    None => self.call(env, self.status, &[jvalue { l: std::ptr::null_mut() }]),
                },
                // The state the session screen shows its sound switch from:
                // what it does (0 none, 1 mute, 2 enable, as
                // `VENTO_AUDIO_SWITCH_*`), whether the far end is asked for its
                // sound, whether it is muted here, and whether a stream plays.
                Event::Sound(sound) => self.call(
                    env,
                    self.sound,
                    &[
                        jvalue {
                            i: match sound.switch {
                                None => 0,
                                Some(Switch::Mute) => 1,
                                Some(Switch::Enable) => 2,
                            },
                        },
                        jvalue { z: sound.enabled },
                        jvalue { z: sound.muted },
                        jvalue { z: sound.playing },
                    ],
                ),
                Event::Clipboard(text) => self.text(env, self.clipboard, &text),
                Event::CredentialsNeeded { needs_user } => {
                    let user = env.new_string(if needs_user { self.asks.prompt_user.as_str() } else { "" })?;
                    self.call(
                        env,
                        self.credentials_needed,
                        &[jvalue { z: needs_user }, jvalue { l: user.as_raw() }],
                    )
                }
                Event::PromptWithdrawn => self.call(env, self.prompt_withdrawn, &[]),
                Event::TrustNeeded { fingerprint } => self.trust_needed(env, &fingerprint),
                Event::Unverified { why } => {
                    let key = env.new_string(&self.asks.pinned.key)?;
                    let why = env.new_string(&why)?;
                    self.call(env, self.unverified, &[jvalue { l: key.as_raw() }, jvalue { l: why.as_raw() }])
                }
                Event::Question { title, text } => {
                    let title = env.new_string(&title)?;
                    let text = env.new_string(&text)?;
                    self.call(env, self.question, &[jvalue { l: title.as_raw() }, jvalue { l: text.as_raw() }])
                }
                Event::Message {
                    text,
                    severity,
                    question,
                    confirm_label,
                } => {
                    let text = env.new_string(&text)?;
                    let confirm = env.new_string(&confirm_label)?;
                    self.call(
                        env,
                        self.message,
                        &[
                            jvalue { l: text.as_raw() },
                            jvalue { i: severity as jint },
                            jvalue { z: question },
                            jvalue { l: confirm.as_raw() },
                        ],
                    )
                }
                // RealVNC's, whose Android half is Java over its core and
                // never comes this way: Java refuses it, so that a backend
                // that did ask would not leave its protocol thread parked
                // behind a question this layer cannot ask.
                Event::SecurityNeeded { signature, .. } => self.text(env, self.security_needed, &signature),
                Event::Closed { detail } => self.text(env, self.closed, &detail),
            }
        });
        if let Err(e) = result {
            log::warn!("callback failed: {e}");
        }
    }

    /// An identity, against the pins: a match, or a first sight that is
    /// vouched for, answers the session from here, and anything else is
    /// `onTrustNeeded`'s question, whose answer comes back as `answerPin`.
    fn trust_needed(&self, env: &mut Env, fingerprint: &str) -> jni::errors::Result<()> {
        let pinned = &self.asks.pinned;
        let decision = trust::decide(&Pins::new(self, env), pinned, fingerprint);
        match decision {
            Decision::Matches => {
                log::info!("identity matches the pin for {}", pinned.key);
                self.answer_trust(true);
                Ok(())
            }
            Decision::FirstSight => {
                self.answer_trust(true);
                Ok(())
            }
            Decision::Ask {
                identity,
                matching_address,
            } => {
                log::info!("asking about the identity of {}: {identity:?}", pinned.key);
                let key = env.new_string(&pinned.key)?;
                let fingerprint = env.new_string(fingerprint)?;
                let identity = env.new_string(identity_name(identity))?;
                let matching = env.new_string(&matching_address)?;
                self.call(
                    env,
                    self.trust_needed,
                    &[
                        jvalue { l: key.as_raw() },
                        jvalue { l: fingerprint.as_raw() },
                        jvalue { l: identity.as_raw() },
                        jvalue { l: matching.as_raw() },
                    ],
                )
            }
        }
    }

    /// The session now exists: an answer decided before it did goes to it.
    fn started(&self, backend: &Arc<dyn Backend>) {
        let early = {
            let mut started = self.started.lock().unwrap();
            started.backend = Some(Arc::downgrade(backend));
            started.early.take()
        };
        if let Some(accept) = early {
            backend.answer_trust(accept);
        }
    }

    /// Answers the session's trust question from this side. One asked before
    /// `create` has returned the session (its thread is already running) is
    /// kept for [`Outlet::started`] rather than lost, which would leave the
    /// session waiting on it forever.
    fn answer_trust(&self, accept: bool) {
        let backend = {
            let mut started = self.started.lock().unwrap();
            if started.backend.is_none() {
                started.early = Some(accept);
                return;
            }
            started.backend.as_ref().and_then(Weak::upgrade)
        };
        if let Some(backend) = backend {
            backend.answer_trust(accept);
        }
    }

    fn call(&self, env: &mut Env, method: JMethodID, args: &[jvalue]) -> jni::errors::Result<()> {
        unsafe { env.call_method_unchecked(&self.target, method, ReturnType::Primitive(Primitive::Void), args) }?;
        Ok(())
    }

    fn text(&self, env: &mut Env, method: JMethodID, text: &str) -> jni::errors::Result<()> {
        let s = env.new_string(text)?;
        self.call(env, method, &[jvalue { l: s.as_raw() }])
    }
}

/// See [`Outlet::answer_trust`].
#[derive(Default)]
struct Started {
    backend: Option<Weak<dyn Backend>>,
    early: Option<bool>,
}

/// `Prompt.Trust.Identity`'s name for one, which Java reads back with
/// `valueOf`.
fn identity_name(identity: Identity) -> &'static str {
    match identity {
        Identity::Ok => "OK",
        Identity::New => "NEW",
        Identity::MatchesAnother => "MATCHES_ANOTHER",
        Identity::Changed => "CHANGED",
        Identity::Missing => "MISSING",
        Identity::Preshared => "PRESHARED",
        Identity::Ard => "ARD",
    }
}

/// The phone's pins, as a [`trust::Store`]: `KnownHosts`' preferences,
/// through the session's `NativeBackend`, on whatever thread asks (the
/// protocol thread for a decision, the main one for an answer). The
/// preferences are the ones the app has always kept, keys and values
/// unchanged, so a pin made before this layer took over the decision still
/// matches.
struct Pins<'a, 'local> {
    outlet: &'a Outlet,
    env: RefCell<&'a mut Env<'local>>,
}

impl<'a, 'local> Pins<'a, 'local> {
    fn new(outlet: &'a Outlet, env: &'a mut Env<'local>) -> Self {
        Pins {
            outlet,
            env: RefCell::new(env),
        }
    }

    fn read(&self) -> jni::errors::Result<Vec<(String, String)>> {
        let env = &mut **self.env.borrow_mut();
        let flat = unsafe {
            env.call_method_unchecked(&self.outlet.target, self.outlet.known_hosts_pins, ReturnType::Array, &[])
        }?
        .l()?;
        let flat = env.cast_local::<JObjectArray<JString>>(flat)?;
        let n = flat.len(env)?;
        let mut pins = Vec::with_capacity(n / 2);
        for i in (0..n.saturating_sub(1)).step_by(2) {
            let key = flat.get_element(env, i)?;
            let value = flat.get_element(env, i + 1)?;
            pins.push((
                string(env, &key)?.unwrap_or_default(),
                string(env, &value)?.unwrap_or_default(),
            ));
        }
        Ok(pins)
    }

    fn write(&self, key: &str, fingerprint: &str) -> jni::errors::Result<()> {
        let env = &mut **self.env.borrow_mut();
        let key = env.new_string(key)?;
        let fingerprint = env.new_string(fingerprint)?;
        unsafe {
            env.call_method_unchecked(
                &self.outlet.target,
                self.outlet.known_hosts_pin,
                ReturnType::Primitive(Primitive::Void),
                &[jvalue { l: key.as_raw() }, jvalue { l: fingerprint.as_raw() }],
            )
        }?;
        Ok(())
    }

    /// A Java exception from the store, taken off the thread so that the
    /// calls after it still work, and described.
    fn failure(&self, e: Error) -> String {
        let caught = self.env.borrow_mut().exception_catch();
        match caught {
            Err(Error::CaughtJavaException { name, msg, .. }) => format!("{name}: {msg}"),
            _ => e.to_string(),
        }
    }
}

impl trust::Store for Pins<'_, '_> {
    fn pins(&self) -> Vec<(String, String)> {
        self.read().unwrap_or_else(|e| {
            log::warn!("reading the known hosts: {}; treated as empty", self.failure(e));
            Vec::new()
        })
    }

    fn put(&self, key: &str, fingerprint: &str) -> Result<(), String> {
        self.write(key, fingerprint).map_err(|e| self.failure(e))
    }
}
