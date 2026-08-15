//! The Java side of a session, called from the protocol thread.
//!
//! Every method id is looked up once, at create time, on the thread that has an
//! `Env` already. `damaged` runs hundreds of times a frame, and resolving a
//! method by name and signature on each of those would be the most expensive
//! thing in the path.
//!
//! One class per backend declares the interface and they are identical, so this
//! takes the name: `RfbNative$Callbacks` and `IronRdpNative$Callbacks` are the same
//! shape because they are answering the same `Backend`. Where the two protocols
//! genuinely differ — RDP's credentials carry a domain — the difference is in
//! what is sent *back* through `nativeAnswerCredentials`, not in what is
//! called here.

use jni::objects::{Global, JObject};
use jni::signature::{Primitive, ReturnType};
use jni::sys::{jvalue, jint};
use jni::{Env, JavaVM, jni_sig, jni_str};

/// The interface `RfbNative.Callbacks` declares, resolved.
pub struct Callbacks {
    vm: JavaVM,
    target: Global<JObject<'static>>,
    connected: jni::objects::JMethodID,
    desktop_size: jni::objects::JMethodID,
    damaged: jni::objects::JMethodID,
    frame_end: jni::objects::JMethodID,
    cursor: jni::objects::JMethodID,
    pointer_mode: jni::objects::JMethodID,
    bell: jni::objects::JMethodID,
    clipboard: jni::objects::JMethodID,
    credentials_needed: jni::objects::JMethodID,
    trust_needed: jni::objects::JMethodID,
    closed: jni::objects::JMethodID,
}

impl Callbacks {
    pub fn new(
        env: &mut Env,
        target: &JObject,
        class_name: &jni::strings::JNIStr,
    ) -> jni::errors::Result<Callbacks> {
        let class = env.find_class(class_name)?;
        Ok(Callbacks {
            vm: env.get_java_vm()?,
            target: env.new_global_ref(target)?,
            connected: env.get_method_id(&class, jni_str!("onConnected"), jni_sig!("(II)V"))?,
            desktop_size: env.get_method_id(&class, jni_str!("onDesktopSize"), jni_sig!("(II)V"))?,
            damaged: env.get_method_id(&class, jni_str!("onDamage"), jni_sig!("(IIII)V"))?,
            frame_end: env.get_method_id(&class, jni_str!("onFrameEnd"), jni_sig!("()V"))?,
            cursor: env.get_method_id(&class, jni_str!("onCursor"), jni_sig!("([IIIIIJ)V"))?,
            pointer_mode: env.get_method_id(&class, jni_str!("onPointerMode"), jni_sig!("(Z)V"))?,
            bell: env.get_method_id(&class, jni_str!("onBell"), jni_sig!("()V"))?,
            clipboard: env.get_method_id(
                &class,
                jni_str!("onClipboard"),
                jni_sig!("(Ljava/lang/String;)V"),
            )?,
            credentials_needed: env.get_method_id(
                &class,
                jni_str!("onCredentialsNeeded"),
                jni_sig!("(Z)V"),
            )?,
            trust_needed: env.get_method_id(
                &class,
                jni_str!("onTrustNeeded"),
                jni_sig!("(Ljava/lang/String;)V"),
            )?,
            closed: env.get_method_id(
                &class,
                jni_str!("onClosed"),
                jni_sig!("(Ljava/lang/String;)V"),
            )?,
        })
    }

    /// Attach this thread and make one void call.
    ///
    /// A Java exception thrown out of a callback becomes a log line rather than
    /// an abort: the protocol thread is in the middle of a framebuffer update
    /// and unwinding it would lose the connection over a bug on the other side
    /// of the boundary. The same argument `jni`'s `LogErrorAndDefault` makes for
    /// the inbound direction.
    fn call(&self, method: jni::objects::JMethodID, args: &[jvalue]) {
        let result: jni::errors::Result<()> = self.vm.attach_current_thread(|env| {
            unsafe {
                env.call_method_unchecked(
                    &self.target,
                    method,
                    ReturnType::Primitive(Primitive::Void),
                    args,
                )
            }?;
            Ok(())
        });
        if let Err(e) = result {
            log::warn!("callback failed: {e}");
        }
    }

    pub fn connected(&self, width: i32, height: i32) {
        self.call(self.connected, &[int(width), int(height)]);
    }

    pub fn desktop_size(&self, width: i32, height: i32) {
        self.call(self.desktop_size, &[int(width), int(height)]);
    }

    pub fn damaged(&self, x: i32, y: i32, width: i32, height: i32) {
        self.call(
            self.damaged,
            &[int(x), int(y), int(width), int(height)],
        );
    }

    pub fn frame_end(&self) {
        self.call(self.frame_end, &[]);
    }

    /// `pixels` is the framebuffer's own `R, G, B, A` word order; Java's
    /// `Bitmap.setPixels` wants `Color`'s `0xAARRGGBB`, so red and blue swap
    /// here rather than on a thread that is drawing.
    pub fn cursor(&self, pixels: &[u32], width: i32, height: i32, hot_x: i32, hot_y: i32) {
        let argb: Vec<jint> = pixels.iter().map(|&p| swap_rb(p) as jint).collect();
        let hash = cursor_hash(&argb, width, height);
        let result: jni::errors::Result<()> = self.vm.attach_current_thread(|env| {
            let array = env.new_int_array(argb.len())?;
            if !argb.is_empty() {
                array.set_region(env, 0, &argb)?;
            }
            unsafe {
                env.call_method_unchecked(
                    &self.target,
                    self.cursor,
                    ReturnType::Primitive(Primitive::Void),
                    &[
                        jvalue { l: array.as_raw() },
                        int(width),
                        int(height),
                        int(hot_x),
                        int(hot_y),
                        jvalue { j: hash },
                    ],
                )
            }?;
            Ok(())
        });
        if let Err(e) = result {
            log::warn!("cursor callback failed: {e}");
        }
    }

    /// What a pointer event's coordinates now mean. RFB's only; an RDP session
    /// is absolute for the length of its life and never calls this.
    pub fn pointer_mode(&self, relative: bool) {
        self.call(self.pointer_mode, &[jvalue { z: relative }]);
    }

    pub fn bell(&self) {
        self.call(self.bell, &[]);
    }

    pub fn clipboard(&self, text: &str) {
        self.with_string(self.clipboard, text);
    }

    pub fn closed(&self, detail: &str) {
        self.with_string(self.closed, detail);
    }

    /// Ask for credentials. Returns nothing itself — the answer arrives through
    /// the protocol's own `Session::answer_credentials`, because on the other
    /// side of this is a dialog and a person.
    pub fn credentials_needed(&self, needs_user: bool) {
        self.call(self.credentials_needed, &[jvalue { z: needs_user }]);
    }

    /// Ask whether the server's certificate is the right one. Answered through
    /// the protocol's own `Session::answer_trust`; the app is what holds the
    /// pin store, so the decision is not ours to take here.
    pub fn trust_needed(&self, fingerprint: &str) {
        self.with_string(self.trust_needed, fingerprint);
    }

    fn with_string(&self, method: jni::objects::JMethodID, text: &str) {
        let result: jni::errors::Result<()> = self.vm.attach_current_thread(|env| {
            let s = env.new_string(text)?;
            unsafe {
                env.call_method_unchecked(
                    &self.target,
                    method,
                    ReturnType::Primitive(Primitive::Void),
                    &[jvalue { l: s.as_raw() }],
                )
            }?;
            Ok(())
        });
        if let Err(e) = result {
            log::warn!("callback failed: {e}");
        }
    }
}

fn int(v: i32) -> jvalue {
    jvalue { i: v }
}

/// FNV-1a over a cursor's pixels, which is its identity in `CursorCache` — so
/// this and the two C shims' copies of it and the Java one all have to agree.
/// Computed here because the pixels are already in cache from the conversion
/// above, and because what it saves on the other side is a bitmap and a texture
/// per cursor change rather than anything on the wire.
fn cursor_hash(argb: &[jint], width: i32, height: i32) -> i64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    h = (h ^ width as u32 as u64).wrapping_mul(0x100_0000_01b3);
    h = (h ^ height as u32 as u64).wrapping_mul(0x100_0000_01b3);
    for &p in argb {
        h = (h ^ p as u32 as u64).wrapping_mul(0x100_0000_01b3);
    }
    h as i64
}

/// `0xAA_BB_GG_RR` (memory order R, G, B, A) to `0xAA_RR_GG_BB`.
fn swap_rb(p: u32) -> u32 {
    (p & 0xff00_ff00) | ((p & 0x00ff_0000) >> 16) | ((p & 0x0000_00ff) << 16)
}
