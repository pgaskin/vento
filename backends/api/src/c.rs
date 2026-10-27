// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! One adapter for all the C and C++ backends, through the vtable in
//! `vento/backend.h`.
//!
//! The vtable is the same for each, so [`CSession`] holds the raw session
//! and the mailbox and wraps each call once, and [`CBackend`] is the
//! [`Backend`] over it. What differs between them (their config, their live
//! options and the labels on their info rows) is a [`Behaviour`] of function
//! pointers filled in by each backend's crate: the libvncclient, TigerVNC,
//! FreeRDP and RealVNC crates beside their C sources.
//!
//! The callbacks are trampolined into the mailbox: `user` is an `Arc<Target>`
//! the session carries, borrowed by each trampoline and reclaimed in
//! [`CBackend::destroy`], which the header guarantees runs only after the
//! protocol thread has ended and no callback can still be running.
//!
//! Nothing here references a backend's own symbols, so linking this needs
//! none of their libraries; each backend crate loads its own ([`library`]).

use crate::backend::{Backend, ConnectionFact, Credentials, Facts, Monitor, Picture, Traffic};
use crate::events::{Encryption, Identity, Mailbox, Severity};
use crate::kind::Connect;
use crate::options::Options;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, ThreadId};

pub mod audio;
pub mod ffi;
pub mod library;
pub mod net;
pub mod video;

// The clipboard limit `vento/text.h` gives the C session files must equal
// the one every Rust backend uses; this fails the build if they drift apart.
const _: () = assert!(ffi::VENTO_MAX_CLIPBOARD as usize == runtime::MAX_CLIPBOARD);

/// A C string from the session as a `String`, UTF-8 as the header
/// guarantees.
///
/// # Safety
///
/// `ptr` is null or a NUL-terminated string that lives for the call.
pub unsafe fn string(ptr: *const c_char) -> String {
    if ptr.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned()
    }
}

/// Writes `text` into a C buffer of `len` bytes, with its NUL, truncated to
/// fit at a character boundary. False, with nothing written, when there is
/// no room for anything but the NUL.
///
/// # Safety
///
/// `out` is null or `len` writable bytes.
pub(crate) unsafe fn write_string(text: &str, out: *mut c_char, len: usize) -> bool {
    if out.is_null() || len < 2 {
        return false;
    }
    let mut end = text.len().min(len - 1);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let text = text[..end].replace('\0', " ");
    unsafe {
        std::ptr::copy_nonoverlapping(text.as_ptr(), out.cast::<u8>(), end);
        *out.add(end) = 0;
    }
    true
}

/// What the callbacks carry as `user`: the session's mailbox, and for an RDP
/// backend the domain the credentials were last sent with, which the advice
/// on a logon failure depends on ([`crate::credentials::logon_failed`]).
struct Target {
    mailbox: Arc<Mailbox>,
    domain: Option<Mutex<String>>,
}

/// What a callback was given, as a borrow: `user` is a live `Arc<Target>`
/// for the length of the session.
unsafe fn target<'a>(user: *mut c_void) -> &'a Target {
    unsafe { &*(user as *const Target) }
}

/// The mailbox a callback was given, as a borrow (see [`target`]).
unsafe fn mailbox<'a>(user: *mut c_void) -> &'a Mailbox {
    unsafe { &target(user).mailbox }
}

extern "C" fn cb_connected(user: *mut c_void, width: c_int, height: c_int) {
    unsafe { mailbox(user) }.on_connected(width.max(0) as usize, height.max(0) as usize);
}

extern "C" fn cb_desktop_size(user: *mut c_void, width: c_int, height: c_int) {
    unsafe { mailbox(user) }.on_desktop_size(width.max(0) as usize, height.max(0) as usize);
}

extern "C" fn cb_damaged(user: *mut c_void, x: c_int, y: c_int, width: c_int, height: c_int) {
    unsafe { mailbox(user) }.on_damaged(
        x.max(0) as usize,
        y.max(0) as usize,
        width.max(0) as usize,
        height.max(0) as usize,
    );
}

extern "C" fn cb_frame_end(user: *mut c_void) {
    unsafe { mailbox(user) }.on_frame_end();
}

extern "C" fn cb_cursor(
    user: *mut c_void,
    pixels: *const u32,
    width: c_int,
    height: c_int,
    hot_x: c_int,
    hot_y: c_int,
) {
    let (w, h) = (width.max(0) as usize, height.max(0) as usize);
    let words: &[u32] = if pixels.is_null() || w == 0 || h == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(pixels, w * h) }
    };
    unsafe { mailbox(user) }.on_cursor(words, w, h, hot_x, hot_y);
}

extern "C" fn cb_pointer_mode(user: *mut c_void, relative: bool) {
    unsafe { mailbox(user) }.on_pointer_mode(relative);
}

extern "C" fn cb_bell(user: *mut c_void) {
    unsafe { mailbox(user) }.on_bell();
}

/// NULL and "" both clear the status: `string` makes NULL empty, and the
/// mailbox takes empty as `None`.
extern "C" fn cb_status(user: *mut c_void, text: *const c_char) {
    unsafe { mailbox(user) }.on_status(Some(&unsafe { string(text) }));
}

extern "C" fn cb_clipboard(user: *mut c_void, utf8: *const c_char) {
    unsafe { mailbox(user) }.on_clipboard(&unsafe { string(utf8) });
}

extern "C" fn cb_credentials_needed(user: *mut c_void, needs_user: bool) {
    unsafe { mailbox(user) }.on_credentials_needed(needs_user);
}

extern "C" fn cb_trust_needed(user: *mut c_void, fingerprint: *const c_char) {
    unsafe { mailbox(user) }.on_trust_needed(&unsafe { string(fingerprint) });
}

extern "C" fn cb_unverified(user: *mut c_void, why: *const c_char) {
    unsafe { mailbox(user) }.on_unverified(&unsafe { string(why) });
}

extern "C" fn cb_question(user: *mut c_void, title: *const c_char, text: *const c_char) {
    unsafe { mailbox(user) }.on_question(&unsafe { string(title) }, &unsafe { string(text) });
}

extern "C" fn cb_security_needed(
    user: *mut c_void,
    identity: c_int,
    encryption: c_int,
    signature: *const c_char,
    catchphrase: *const c_char,
    matching_address: *const c_char,
) {
    unsafe { mailbox(user) }.on_security_needed(
        Identity::from_c(identity),
        Encryption::from_c(encryption),
        &unsafe { string(signature) },
        &unsafe { string(catchphrase) },
        &unsafe { string(matching_address) },
    );
}

extern "C" fn cb_message(
    user: *mut c_void,
    text: *const c_char,
    severity: c_int,
    question: bool,
    confirm_label: *const c_char,
) {
    unsafe { mailbox(user) }.on_message(
        &unsafe { string(text) },
        Severity::from_c(severity),
        question,
        &unsafe { string(confirm_label) },
    );
}

/// `VENTO_LOGON_FAILED` from an RDP backend is shown with the advice for the
/// domain that was tried.
extern "C" fn cb_closed(user: *mut c_void, detail: *const c_char) {
    let target = unsafe { target(user) };
    let detail = unsafe { string(detail) };
    match &target.domain {
        Some(domain) if detail == crate::credentials::LOGON_FAILED => {
            let detail = crate::credentials::logon_failed(&domain.lock().unwrap());
            target.mailbox.on_closed(&detail);
        }
        _ => target.mailbox.on_closed(&detail),
    }
}

/// The callbacks table, pointed at `user`.
fn callbacks(user: *mut c_void) -> ffi::vento_callbacks {
    ffi::vento_callbacks {
        user,
        connected: Some(cb_connected),
        desktop_size: Some(cb_desktop_size),
        damaged: Some(cb_damaged),
        frame_end: Some(cb_frame_end),
        cursor: Some(cb_cursor),
        pointer_mode: Some(cb_pointer_mode),
        bell: Some(cb_bell),
        clipboard: Some(cb_clipboard),
        credentials_needed: Some(cb_credentials_needed),
        trust_needed: Some(cb_trust_needed),
        unverified: Some(cb_unverified),
        question: Some(cb_question),
        security_needed: Some(cb_security_needed),
        message: Some(cb_message),
        closed: Some(cb_closed),
        status: Some(cb_status),
    }
}

/// The length of a backend's table in bytes, from its `size` member: the
/// header grows the table at its end, and a backend built against an older
/// header has a shorter table whose missing members must not be read at
/// all.
///
/// # Safety
///
/// `ops` is a backend's table, which always has its `size`.
unsafe fn table_size(ops: *const ffi::vento_session_ops) -> usize {
    unsafe { (*ops).size }
}

/// The end of the members every table has: those up to `destroy`, the last
/// member of the table when `size` was added at its start.
const BASE_TABLE: usize =
    std::mem::offset_of!(ffi::vento_session_ops, destroy) + std::mem::size_of::<Option<unsafe extern "C" fn()>>();

/// One of the table's optional members (those after `destroy`), or `None`
/// when the backend's table ends before it (a table from an older header) or
/// the backend left it NULL. The member is not read at all unless the table
/// covers it.
macro_rules! optional {
    ($ops:expr, $member:ident) => {{
        let ops: *const ffi::vento_session_ops = $ops;
        let end = std::mem::offset_of!(ffi::vento_session_ops, $member)
            + std::mem::size_of::<Option<unsafe extern "C" fn()>>();
        if unsafe { table_size(ops) } >= end {
            unsafe { (*ops).$member }
        } else {
            None
        }
    }};
}

/// A started session and its vtable. The raw pointers belong to the C
/// library and may be used from any thread, as the header says of every
/// `vento_session_*` call.
pub struct CSession {
    /// For a backend's own calls outside the vtable (a live option, its own
    /// kind of answer).
    pub session: *mut ffi::vento_session,
    /// The `Arc<Target>` the callbacks carry, as a raw pointer to reclaim
    /// once the session is destroyed.
    user: *const Target,
    /// The calls in progress and whether `destroy` has begun.
    gate: Gate,
}

// SAFETY: the C session is documented as callable from any thread, and the
// user pointer is only reclaimed after `destroy` has joined the protocol
// thread.
unsafe impl Send for CSession {}
unsafe impl Sync for CSession {}

/// Prevents a call from reaching a session that `destroy` has freed.
///
/// A shared handle can be called from another thread while it is being
/// destroyed (a drawing thread reading pixels, or a thread holding the
/// backend through a weak reference to answer a question), so a flag read
/// only by `destroy` would let that call dereference a freed session. Every
/// call goes through [`CSession::enter`]: none starts once `destroy` has,
/// and the free waits for those already in progress.
#[derive(Default)]
struct Gate {
    state: Mutex<GateState>,
    idle: Condvar,
}

#[derive(Default)]
struct GateState {
    /// Whether `destroy` has begun: the session is freed and the mailbox
    /// reclaimed once, however many times `destroy` is called.
    destroyed: bool,
    /// The thread of each call in progress, once per call (a call can make
    /// another such call).
    inside: Vec<ThreadId>,
    /// `destroy` was called from inside a call on the same thread (from a
    /// callback delivered inline), which it cannot wait for, so the last
    /// call to leave frees the session instead.
    deferred: bool,
}

/// A call in progress; see [`Gate`].
struct Entered<'a>(&'a CSession);

impl Drop for Entered<'_> {
    fn drop(&mut self) {
        let mut state = self.0.gate.state.lock().unwrap();
        let me = thread::current().id();
        if let Some(i) = state.inside.iter().position(|t| *t == me) {
            state.inside.swap_remove(i);
        }
        if !state.inside.is_empty() {
            return;
        }
        if std::mem::take(&mut state.deferred) {
            drop(state);
            unsafe { self.0.free() };
        } else {
            self.0.gate.idle.notify_all();
        }
    }
}

impl CSession {
    /// Admits a call, or returns `None` once `destroy` has begun.
    fn enter(&self) -> Option<Entered<'_>> {
        let mut state = self.gate.state.lock().unwrap();
        if state.destroyed {
            return None;
        }
        state.inside.push(thread::current().id());
        Some(Entered(self))
    }

    /// Runs `f` if the session has not been destroyed; returns `default`
    /// otherwise.
    fn call<R>(&self, default: R, f: impl FnOnce() -> R) -> R {
        match self.enter() {
            Some(_entered) => f(),
            None => default,
        }
    }

    /// The table, as a pointer rather than a reference: it may be shorter
    /// than this build's `vento_session_ops` (see [`table_size`]), so
    /// members are read through it individually, never as a whole.
    fn ops(&self) -> *const ffi::vento_session_ops {
        unsafe { (*self.session).ops }
    }

    fn pointer(&self, x: i32, y: i32, mask: i32) {
        self.call((), || unsafe { ((*self.ops()).pointer.unwrap())(self.session, x, y, mask) })
    }

    fn key_down(&self, keysym: u32, key_id: i64) {
        self.call((), || unsafe { ((*self.ops()).key_down.unwrap())(self.session, keysym, key_id) })
    }

    fn key_up(&self, key_id: i64) {
        self.call((), || unsafe { ((*self.ops()).key_up.unwrap())(self.session, key_id) })
    }

    fn release_all_keys(&self) {
        self.call((), || unsafe { ((*self.ops()).release_all_keys.unwrap())(self.session) })
    }

    fn focus(&self, focused: bool) {
        self.call((), || unsafe { ((*self.ops()).focus.unwrap())(self.session, focused) })
    }

    fn view_only(&self, view_only: bool) {
        self.call((), || unsafe { ((*self.ops()).view_only.unwrap())(self.session, view_only) })
    }

    fn can_resize(&self) -> bool {
        self.call(false, || unsafe { ((*self.ops()).can_resize.unwrap())(self.session) })
    }

    fn monitors(&self) -> Vec<i32> {
        let Some(_entered) = self.enter() else {
            return Vec::new();
        };
        let mut flat = vec![0i32; 4 * ffi::VENTO_MAX_MONITORS as usize];
        let n = unsafe { ((*self.ops()).monitors.unwrap())(self.session, flat.as_mut_ptr()) };
        flat.truncate((n.max(0) as usize * 4).min(flat.len()));
        flat
    }

    fn request_desktop_size(&self, width: i32, height: i32) {
        self.call((), || unsafe { ((*self.ops()).request_desktop_size.unwrap())(self.session, width, height) })
    }

    /// The layout with the far end's own ids, or `None` when the backend has
    /// no ids and the layout is [`monitors`](CSession::monitors) numbered by
    /// index.
    fn screens(&self) -> Option<Vec<Monitor>> {
        let _entered = self.enter()?;
        let screens = optional!(self.ops(), screens)?;
        let mut flat = vec![0i32; 5 * ffi::VENTO_MAX_MONITORS as usize];
        let n = unsafe { screens(self.session, flat.as_mut_ptr()) };
        flat.truncate((n.max(0) as usize * 5).min(flat.len()));
        Some(Monitor::from_flat5(&flat))
    }

    /// Whether the backend can send a layout at all, regardless of whether
    /// the far end accepts it.
    fn has_layout(&self) -> bool {
        self.call(false, || optional!(self.ops(), request_layout).is_some())
    }

    fn request_layout(&self, width: i32, height: i32, screens: &[Monitor]) {
        let Some(_entered) = self.enter() else {
            return;
        };
        if let Some(request_layout) = optional!(self.ops(), request_layout) {
            let flat = Monitor::flat5(screens);
            let n = screens.len().min(c_int::MAX as usize) as c_int;
            unsafe { request_layout(self.session, width, height, flat.as_ptr(), n) }
        }
    }

    fn clipboard(&self, text: &str) {
        if let Ok(c) = CString::new(text) {
            self.call((), || unsafe { ((*self.ops()).clipboard.unwrap())(self.session, c.as_ptr()) })
        }
    }

    /// The info rows, in the backend's own order, or `None` when there are
    /// none yet. Freed as the header's `vento_strings_free` frees them, with
    /// `free` on each item and the array.
    fn info(&self) -> Option<Vec<String>> {
        let _entered = self.enter()?;
        let mut strings = ffi::vento_strings {
            items: std::ptr::null_mut(),
            count: 0,
        };
        if !unsafe { ((*self.ops()).info.unwrap())(self.session, &mut strings) } {
            return None;
        }
        let rows = (0..strings.count)
            .map(|i| unsafe { string(*strings.items.add(i)) })
            .collect();
        unsafe {
            for i in 0..strings.count {
                ffi::free(*strings.items.add(i) as *mut c_void);
            }
            ffi::free(strings.items as *mut c_void);
        }
        Some(rows)
    }

    /// The decoder's overlay line, or `None` when the backend has none (no
    /// video, no picture yet, or a table from before the member).
    fn decoder_status(&self) -> Option<String> {
        let _entered = self.enter()?;
        let decoder_status = optional!(self.ops(), decoder_status)?;
        let mut line = [0 as c_char; 256];
        if !unsafe { decoder_status(self.session, line.as_mut_ptr(), line.len()) } {
            return None;
        }
        // The backend's NUL, or the end of the buffer if it left none.
        line[line.len() - 1] = 0;
        Some(unsafe { string(line.as_ptr()) })
    }

    /// Bytes received and sent, or `None` while there is nothing to count.
    fn traffic(&self) -> Option<[u64; 2]> {
        let mut out = [0u64; 2];
        self.call(false, || unsafe { ((*self.ops()).traffic.unwrap())(self.session, out.as_mut_ptr()) })
            .then_some(out)
    }

    #[allow(clippy::too_many_arguments)]
    fn read_region(
        &self,
        x: usize,
        y: usize,
        width: usize,
        height: usize,
        dst: &mut [u8],
        dst_stride: usize,
        dst_x: usize,
        dst_y: usize,
    ) -> bool {
        // The library writes at (dst_y, dst_x) into `dst` itself and checks
        // only the source; the header leaves checking the destination to the
        // caller.
        let Some([x, y, width, height, dst_x, dst_y]) =
            region_args(dst.len(), [x, y, width, height], dst_stride, dst_x, dst_y)
        else {
            return false;
        };
        let Some(_entered) = self.enter() else {
            return false;
        };
        unsafe {
            ((*self.ops()).read_region.unwrap())(
                self.session,
                x,
                y,
                width,
                height,
                dst.as_mut_ptr() as *mut c_void,
                dst_stride,
                dst_x,
                dst_y,
            )
        }
    }

    fn thumbnail(&self, step: usize, width: usize, height: usize) -> Option<Picture> {
        let step = step.max(1);
        if width == 0 || height == 0 {
            return None;
        }
        let tw = width.div_ceil(step);
        let th = height.div_ceil(step);
        let mut rgba = vec![0u8; tw * th * 4];
        let _entered = self.enter()?;
        let ok = unsafe {
            ((*self.ops()).read_thumbnail.unwrap())(
                self.session,
                step as c_int,
                rgba.as_mut_ptr() as *mut c_void,
                tw * 4,
                tw as c_int,
                th as c_int,
            )
        };
        ok.then_some(Picture {
            width: tw,
            height: th,
            rgba,
        })
    }

    fn answer_credentials(&self, user: &str, domain: &str, password: Option<&str>) {
        let user = CString::new(user).unwrap_or_default();
        let domain = CString::new(domain).unwrap_or_default();
        let password = password.and_then(|p| CString::new(p).ok());
        let Some(_entered) = self.enter() else {
            return;
        };
        unsafe {
            ((*self.ops()).answer_credentials.unwrap())(
                self.session,
                user.as_ptr(),
                domain.as_ptr(),
                password.as_ref().map_or(std::ptr::null(), |p| p.as_ptr()),
            )
        }
    }

    fn answer_trust(&self, accept: bool) {
        self.call((), || unsafe { ((*self.ops()).answer_trust.unwrap())(self.session, accept) })
    }

    fn disconnect(&self) {
        self.call((), || unsafe { ((*self.ops()).disconnect.unwrap())(self.session) })
    }

    /// Ends the session, waits for its thread, frees it, and then reclaims
    /// the mailbox the callbacks carried, in that order, because the
    /// callbacks are only guaranteed finished once `destroy` has returned.
    /// Only the first call does anything, as [`Backend::destroy`] requires:
    /// a second would free the session and drop the mailbox's count twice.
    /// No call starts once this has, and the free waits for calls in
    /// progress on other threads (see [`Gate`]).
    fn destroy(&self) {
        let mut state = self.gate.state.lock().unwrap();
        if std::mem::replace(&mut state.destroyed, true) {
            return;
        }
        if state.inside.contains(&thread::current().id()) {
            state.deferred = true;
            return;
        }
        while !state.inside.is_empty() {
            state = self.gate.idle.wait(state).unwrap();
        }
        drop(state);
        unsafe { self.free() }
    }

    /// Calls the C side's `destroy` and releases the mailbox reference.
    ///
    /// # Safety
    ///
    /// Once, after `destroy` has begun and no call is in flight.
    unsafe fn free(&self) {
        unsafe {
            ((*self.ops()).destroy.unwrap())(self.session);
            drop(Arc::from_raw(self.user));
        }
    }
}

/// `read_region`'s arguments as the C call takes them (`x, y, width,
/// height, dst_x, dst_y`), or `None` when the call must not be made: a
/// value or far edge that an `int` cannot hold (it would wrap to a different
/// rectangle), an empty rectangle, or a destination of `len` bytes that does
/// not hold every row at `stride`. Checking the last row is enough, and a
/// row wider than the stride is rejected too.
fn region_args(
    len: usize,
    [x, y, width, height]: [usize; 4],
    stride: usize,
    dst_x: usize,
    dst_y: usize,
) -> Option<[c_int; 6]> {
    let int = |v: usize| c_int::try_from(v).ok();
    if width == 0 || height == 0 {
        return None;
    }
    int(x.checked_add(width)?)?;
    int(y.checked_add(height)?)?;
    int(dst_x.checked_add(width)?)?;
    int(dst_y.checked_add(height)?)?;
    let row = dst_x.checked_add(width)?.checked_mul(4)?;
    if row > stride {
        return None;
    }
    let end = (dst_y + height - 1).checked_mul(stride)?.checked_add(row)?;
    if end > len {
        return None;
    }
    Some([int(x)?, int(y)?, int(width)?, int(height)?, int(dst_x)?, int(dst_y)?])
}

/// What differs between the C backends.
pub struct Behaviour {
    /// Builds this backend's info rows with its own labels from the strings
    /// `info` returned and the already gathered traffic.
    pub info: fn(&[String], Option<Traffic>) -> Vec<ConnectionFact>,
    /// Whether the sent direction is counted. RealVNC's core counts only one
    /// way, so its row shows only "in" rather than a misleading zero out.
    pub sent_counted: bool,
    /// A live option change, after the value has been stored in `options`.
    /// View-only goes through the vtable; the rest use the backend's own
    /// live call.
    pub set_option: fn(&CSession, &Options, key: &str),
    /// Whether the credentials carry a domain, which only the RDP backend's
    /// do; the others send an empty one.
    pub rdp_domain: bool,
}

/// A C backend as a [`Backend`].
pub struct CBackend {
    session: CSession,
    mailbox: Arc<Mailbox>,
    options: Mutex<Options>,
    user: String,
    domain: String,
    /// What the callbacks carry, to record the domain each answer sends.
    target: Arc<Target>,
    behaviour: &'static Behaviour,
    security: Option<&'static Security>,
}

/// RealVNC's security prompt, which has a remember option and an identity to
/// return, so it has its own calls where the other backends have only the
/// plain yes or no.
pub struct Security {
    pub answer: fn(&CSession, accept: bool, remember: bool),
    /// The identity the library asked to have remembered, if any.
    pub saved_identity: fn(&CSession) -> Option<String>,
}

impl CBackend {
    fn traffic_raw(&self) -> Option<Traffic> {
        let [received, sent] = self.session.traffic()?;
        Some(Traffic {
            received,
            sent: self.behaviour.sent_counted.then_some(sent),
        })
    }
}

impl Backend for CBackend {
    fn pointer(&self, x: i32, y: i32, button_mask: i32) {
        self.session.pointer(x, y, button_mask & 0x1ff);
    }

    fn key_down(&self, keysym: u32, key_id: i64) {
        self.session.key_down(keysym, key_id);
    }

    fn key_up(&self, key_id: i64) {
        self.session.key_up(key_id);
    }

    fn release_all_keys(&self) {
        self.session.release_all_keys();
    }

    fn focus(&self, focused: bool) {
        self.session.focus(focused);
    }

    fn set_view_only(&self, view_only: bool) {
        self.set_option("ViewOnly", if view_only { "true" } else { "false" });
    }

    fn clipboard(&self, text: &str) {
        self.session.clipboard(text);
    }

    fn request_desktop_size(&self, width: u32, height: u32) {
        self.session
            .request_desktop_size(width.min(i32::MAX as u32) as i32, height.min(i32::MAX as u32) as i32);
    }

    fn request_layout(&self, width: u32, height: u32, screens: &[Monitor]) {
        if self.session.has_layout() {
            self.session.request_layout(
                width.min(i32::MAX as u32) as i32,
                height.min(i32::MAX as u32) as i32,
                screens,
            );
        } else if screens.len() == 1 {
            self.request_desktop_size(width, height);
        }
    }

    fn set_option(&self, key: &str, value: &str) {
        let mut options = self.options.lock().unwrap();
        options.set(key, value);
        if key == "ViewOnly" {
            self.session.view_only(options.bool(key));
        }
        // The backend's own live call uses the session directly.
        self.session.call((), || (self.behaviour.set_option)(&self.session, &options, key));
    }

    fn facts(&self) -> Facts {
        let (width, height) = self.mailbox.size();
        let connected = self.mailbox.connected();
        let monitors = self
            .session
            .screens()
            .unwrap_or_else(|| Monitor::from_flat(&self.session.monitors()));
        // For a backend that sends layouts, `can_resize` says whether the far
        // end accepts one, whatever its number of screens; a plain resize
        // would still merge several screens into one, so it is not offered
        // for more than one.
        let layout = self.session.has_layout();
        let takes = connected && self.session.can_resize();
        Facts {
            width,
            height,
            can_resize: takes && (!layout || monitors.len() <= 1),
            can_layout: takes && layout,
            monitors,
            displays: Vec::new(),
            display: -1,
            view_only: self.options.lock().unwrap().bool("ViewOnly"),
            pointer_relative: false,
        }
    }

    fn info(&self) -> Vec<ConnectionFact> {
        if !self.mailbox.connected() {
            return Vec::new();
        }
        match self.session.info() {
            Some(strings) => (self.behaviour.info)(&strings, self.traffic_raw()),
            None => Vec::new(),
        }
    }

    fn traffic(&self) -> Option<Traffic> {
        self.traffic_raw()
    }

    fn decoder_status(&self) -> Option<String> {
        self.session.decoder_status()
    }

    fn read_region(
        &self,
        x: usize,
        y: usize,
        width: usize,
        height: usize,
        dst: &mut [u8],
        dst_stride: usize,
        dst_x: usize,
        dst_y: usize,
    ) -> bool {
        self.session
            .read_region(x, y, width, height, dst, dst_stride, dst_x, dst_y)
    }

    fn thumbnail(&self, step: usize) -> Option<Picture> {
        let (width, height) = self.mailbox.size();
        self.session.thumbnail(step, width, height)
    }

    fn answer_credentials(&self, credentials: Option<Credentials>) {
        match credentials {
            None => self.session.answer_credentials("", "", None),
            Some(c) => {
                // What was typed, with an empty user name replaced by the
                // one the session was created with. Only RDP sends a domain.
                let (domain, c) = if self.behaviour.rdp_domain {
                    let (domain, user) = crate::credentials::rdp_identity(&c, &self.domain, &self.user);
                    if let Some(sent) = &self.target.domain {
                        sent.lock().unwrap().clone_from(&domain);
                    }
                    (domain, Credentials { user, ..c })
                } else {
                    (String::new(), crate::credentials::fill_user(c, &self.user))
                };
                self.session.answer_credentials(&c.user, &domain, Some(&c.password));
            }
        }
    }

    fn answer_trust(&self, accept: bool) {
        self.session.answer_trust(accept);
    }

    fn answer_security(&self, accept: bool, remember: bool) {
        match self.security {
            Some(security) => self.session.call((), || (security.answer)(&self.session, accept, remember)),
            None => self.session.answer_trust(accept),
        }
    }

    fn saved_identity(&self) -> Option<String> {
        let security = self.security?;
        self.session.call(None, || (security.saved_identity)(&self.session))
    }

    fn disconnect(&self) {
        self.session.disconnect();
    }

    fn destroy(&self) {
        self.session.destroy();
    }
}

/// Builds a session: the config built by `make` from a mailbox's `user`
/// pointer and the callbacks, wrapped in a [`CBackend`] with `behaviour`.
/// `make` returns null where the session could not be set up.
pub fn start(
    c: &Connect,
    behaviour: &'static Behaviour,
    make: impl FnOnce(*const ffi::vento_callbacks) -> *mut ffi::vento_session,
    security: Option<&'static Security>,
) -> Result<Arc<dyn Backend>, String> {
    let mailbox = Arc::clone(&c.mailbox);
    let (domain, user_name) = c.domain_and_user();
    let target = Arc::new(Target {
        mailbox: Arc::clone(&mailbox),
        domain: behaviour.rdp_domain.then(|| Mutex::new(domain.clone())),
    });
    // The reference the callbacks carry, owned by the session.
    let user = Arc::into_raw(Arc::clone(&target));
    let table = callbacks(user as *mut c_void);
    let session = make(&table);
    if session.is_null() {
        // Nothing started: reclaim the reference meant for the callbacks.
        unsafe { drop(Arc::from_raw(user)) };
        return Err("could not start the connection".into());
    }
    // A table claiming to be shorter than the members every table has never
    // had its `size` filled in, so every optional member would be read as
    // absent. Logged here, as the backend's fault, rather than showing up as
    // a feature that silently does nothing.
    let size = unsafe { table_size((*session).ops) };
    if size < BASE_TABLE {
        log::warn!("the backend's vento_session_ops says it is {size} bytes, which no table is");
    }
    Ok(Arc::new(CBackend {
        session: CSession {
            session,
            user,
            gate: Gate::default(),
        },
        mailbox,
        options: Mutex::new(c.options.clone()),
        user: user_name,
        domain,
        target,
        behaviour,
        security,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "C" fn no_layout(_: *mut ffi::vento_session, _: c_int, _: c_int, _: *const i32, _: c_int) {}

    /// A session with only a table behind it: the tested code reads only
    /// the table.
    fn with_table<T>(ops: &ffi::vento_session_ops, f: impl FnOnce(&CSession) -> T) -> T {
        let mut raw = ffi::vento_session { ops };
        // Never destroyed, so the null mailbox is never reclaimed.
        let session = CSession {
            session: &mut raw,
            user: std::ptr::null(),
            gate: Gate::default(),
        };
        f(&session)
    }

    /// A session whose table records what reached it: `entered` is set while
    /// a pointer call is inside the backend, which waits there for `hold`.
    #[repr(C)]
    struct Recorder {
        raw: ffi::vento_session,
        log: Mutex<Vec<&'static str>>,
        hold: Mutex<bool>,
        released: Condvar,
        entered: std::sync::Barrier,
        /// Called from inside the pointer call, like a listener destroying
        /// the session from a callback delivered inline.
        inline: Mutex<Option<Box<dyn Fn() + Send>>>,
    }

    fn recorder(s: *mut ffi::vento_session) -> &'static Recorder {
        unsafe { &*(s as *const Recorder) }
    }

    unsafe extern "C" fn rec_pointer(s: *mut ffi::vento_session, _: c_int, _: c_int, _: c_int) {
        let r = recorder(s);
        r.log.lock().unwrap().push("pointer in");
        if let Some(inline) = r.inline.lock().unwrap().take() {
            inline();
        } else {
            r.entered.wait();
            let mut hold = r.hold.lock().unwrap();
            while *hold {
                hold = r.released.wait(hold).unwrap();
            }
        }
        r.log.lock().unwrap().push("pointer out");
    }

    unsafe extern "C" fn rec_destroy(s: *mut ffi::vento_session) {
        recorder(s).log.lock().unwrap().push("destroy");
    }

    unsafe extern "C" fn rec_key_up(s: *mut ffi::vento_session, _: i64) {
        recorder(s).log.lock().unwrap().push("key_up");
    }

    static TABLE: ffi::vento_session_ops = ffi::vento_session_ops {
        size: std::mem::size_of::<ffi::vento_session_ops>(),
        pointer: Some(rec_pointer),
        key_up: Some(rec_key_up),
        destroy: Some(rec_destroy),
        ..unsafe { std::mem::zeroed() }
    };

    /// A leaked recorder, in a session holding a real mailbox for `free` to
    /// reclaim.
    fn recorded() -> (&'static Recorder, Arc<CSession>, Arc<Mailbox>) {
        let r: &'static Recorder = Box::leak(Box::new(Recorder {
            raw: ffi::vento_session { ops: &TABLE },
            log: Mutex::new(Vec::new()),
            hold: Mutex::new(true),
            released: Condvar::new(),
            entered: std::sync::Barrier::new(2),
            inline: Mutex::new(None),
        }));
        let mailbox = Mailbox::new(None);
        let session = Arc::new(CSession {
            session: &r.raw as *const _ as *mut _,
            user: Arc::into_raw(Arc::new(Target {
                mailbox: Arc::clone(&mailbox),
                domain: None,
            })),
            gate: Gate::default(),
        });
        (r, session, mailbox)
    }

    /// Nothing reaches the backend once `destroy` has run, and a second
    /// `destroy` does nothing: one free, one reference released.
    #[test]
    fn nothing_reaches_a_destroyed_session() {
        let (r, session, mailbox) = recorded();
        session.key_up(1);
        session.destroy();
        session.key_up(2);
        session.destroy();
        assert!(!session.has_layout());
        assert_eq!(*r.log.lock().unwrap(), ["key_up", "destroy"]);
        assert_eq!(Arc::strong_count(&mailbox), 1);
    }

    /// A call already inside the backend when another thread destroys the
    /// session is waited for: the free comes after it, never during it.
    #[test]
    fn destroy_waits_for_a_call_in_flight() {
        let (r, session, mailbox) = recorded();
        let drawing = {
            let session = Arc::clone(&session);
            thread::spawn(move || session.pointer(1, 2, 0))
        };
        r.entered.wait();
        let destroying = {
            let session = Arc::clone(&session);
            thread::spawn(move || session.destroy())
        };
        // Long enough for a destroy that did not wait to have freed.
        thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(*r.log.lock().unwrap(), ["pointer in"]);
        *r.hold.lock().unwrap() = false;
        r.released.notify_all();
        drawing.join().unwrap();
        destroying.join().unwrap();
        assert_eq!(*r.log.lock().unwrap(), ["pointer in", "pointer out", "destroy"]);
        assert_eq!(Arc::strong_count(&mailbox), 1);
    }

    /// `destroy` from inside a call on its own thread cannot wait for that
    /// call, so the call frees the session on its way out.
    #[test]
    fn destroy_inside_a_call_frees_on_the_way_out() {
        let (r, session, mailbox) = recorded();
        let inner = Arc::clone(&session);
        *r.inline.lock().unwrap() = Some(Box::new(move || inner.destroy()));
        session.pointer(1, 2, 0);
        assert_eq!(*r.log.lock().unwrap(), ["pointer in", "pointer out", "destroy"]);
        session.key_up(1);
        assert_eq!(r.log.lock().unwrap().len(), 3);
        assert_eq!(Arc::strong_count(&mailbox), 1);
    }

    #[test]
    fn a_member_past_the_tables_size_is_not_there() {
        let full = ffi::vento_session_ops {
            size: std::mem::size_of::<ffi::vento_session_ops>(),
            request_layout: Some(no_layout),
            ..Default::default()
        };
        assert!(with_table(&full, |s| s.has_layout()));
        // The same member, in a table that ends before it: an older
        // backend's, where those bytes are something else.
        let old = ffi::vento_session_ops {
            size: BASE_TABLE,
            ..full
        };
        assert!(!with_table(&old, |s| s.has_layout()));
        // In range but NULL: the backend does not support it.
        let none = ffi::vento_session_ops {
            request_layout: None,
            ..full
        };
        assert!(!with_table(&none, |s| s.has_layout()));
        assert!(with_table(&none, |s| s.screens()).is_none());
    }

    #[test]
    fn a_region_is_passed_on_only_when_the_destination_holds_it() {
        // 64x64 at the origin of a 256-byte stride: 63 rows and one more row.
        let need = 63 * 256 + 256;
        assert_eq!(
            region_args(need, [0, 0, 64, 64], 256, 0, 0),
            Some([0, 0, 64, 64, 0, 0])
        );
        assert_eq!(region_args(need - 1, [0, 0, 64, 64], 256, 0, 0), None);
        assert_eq!(region_args(16, [0, 0, 64, 64], 256, 0, 0), None);
        // Placed at (2, 3) in a bigger picture: the last row ends further in.
        let need = (3 + 63) * 1024 + (2 + 64) * 4;
        assert_eq!(
            region_args(need, [5, 6, 64, 64], 1024, 2, 3),
            Some([5, 6, 64, 64, 2, 3])
        );
        assert_eq!(region_args(need - 1, [5, 6, 64, 64], 1024, 2, 3), None);
        // A row wider than the stride.
        assert_eq!(region_args(1 << 20, [0, 0, 65, 1], 256, 0, 0), None);
        assert_eq!(region_args(1 << 20, [0, 0, 0, 1], 256, 0, 0), None);
    }

    #[test]
    fn a_string_is_cut_to_its_buffer_at_a_character() {
        let mut buf = [1 as c_char; 6];
        assert!(unsafe { write_string("H.264 sw", buf.as_mut_ptr(), buf.len()) });
        assert_eq!(unsafe { string(buf.as_ptr()) }, "H.264");
        // "é" is two bytes, and half of it is not UTF-8.
        let mut buf = [1 as c_char; 3];
        assert!(unsafe { write_string("aé", buf.as_mut_ptr(), buf.len()) });
        assert_eq!(unsafe { string(buf.as_ptr()) }, "a");
        assert!(!unsafe { write_string("a", buf.as_mut_ptr(), 1) });
        assert!(!unsafe { write_string("a", std::ptr::null_mut(), 8) });
    }

    #[test]
    fn a_region_an_int_cannot_hold_is_not_passed_on() {
        let big = usize::MAX;
        let max = c_int::MAX as usize;
        assert_eq!(region_args(big, [0x7fff_ff00, 0, 0x200, 1], 0x800, 0, 0), None);
        assert_eq!(region_args(big, [max, 0, 1, 1], 4, 0, 0), None);
        assert_eq!(region_args(big, [0, max + 1, 1, 1], 4, 0, 0), None);
        assert_eq!(region_args(big, [0, 0, 1, 1], big, 0, big), None);
        assert_eq!(region_args(big, [0, 0, big, 1], big, 0, 0), None);
        assert_eq!(region_args(big, [0, 0, 1, 2], big, 0, 0), None);
    }

    #[test]
    fn the_header_has_the_same_logon_failure_text() {
        assert_eq!(&ffi::VENTO_LOGON_FAILED[..], format!("{}\0", crate::credentials::LOGON_FAILED).as_bytes());
    }

    /// A logon failure from an RDP backend gets the advice for the domain
    /// last tried; any other detail, or any other backend's, is passed on.
    #[test]
    fn a_logon_failure_is_closed_with_advice_for_its_domain() {
        let closed = |domain: Option<&str>, detail: &str| {
            let target = Target {
                mailbox: Mailbox::new(None),
                domain: domain.map(|d| Mutex::new(d.to_string())),
            };
            let detail = CString::new(detail).unwrap();
            cb_closed(&target as *const Target as *mut c_void, detail.as_ptr());
            match target.mailbox.take() {
                Some(crate::events::Event::Closed { detail }) => detail,
                other => panic!("{other:?}"),
            }
        };
        assert_eq!(closed(Some("MicrosoftAccount"), "Logon failed."), crate::credentials::logon_failed("MicrosoftAccount"));
        assert_eq!(closed(Some("example.invalid"), "Logon failed."), crate::credentials::logon_failed("example.invalid"));
        assert_eq!(closed(Some("CORP"), "Logon failed."), "Logon failed.");
        assert_eq!(closed(Some("MicrosoftAccount"), "The connection failed"), "The connection failed");
        assert_eq!(closed(None, "Logon failed."), "Logon failed.");
    }
}
