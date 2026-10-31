// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The phone's own decoder, through the NDK's C API: [`MediaCodec`] is this
//! platform's `runtime::media::video::Decoders`.
//!
//! The decode is the phone's hardware, which is what keeps a backend that
//! receives video from being a codec project: nothing here implements VP9,
//! and MediaCodec is a C API, so this reaches it with an `extern "C"` block
//! and no JNI, no Java and no thread of its own. Everything after the decode
//! (what moved, and the conversion of only that) is
//! `runtime::media::video::Output`, the same as libavcodec's on a desktop.
//!
//! Declared here rather than taken from a crate: it is a dozen functions with a
//! stable ABI, and `#[link]` on the extern block is the only build
//! configuration needed, the same argument [`bitmap`](crate::bitmap) makes for
//! `jnigraphics`.
//!
//! Three things are not obvious and each took a measurement to find:
//!
//! - **No surface.** The pixels have to come back to the CPU, because
//!   `Backend.readRegion` promises a region read out of a framebuffer.
//! - **`low-latency`, because a held frame is a desktop that lags the finger on
//!   it.** A decoder is free to hold frames back to reorder them, and a remote
//!   desktop has nothing to reorder.
//! - **The layout is read from the buffer's own format, per buffer**, and out of
//!   `image-data` rather than from the colour format: the first buffer of a
//!   session does not have the same stride as the rest, and a decoder asked for
//!   Flexible reports Flexible and nothing more.

use std::ffi::{CStr, CString, c_char, c_void};
use std::sync::OnceLock;

use runtime::media::video::convert::{Frame, Plane};
use runtime::media::video::{Changes, Codec, Decoders, Output, bitstream};

#[repr(C)]
pub(crate) struct AMediaCodec {
    _private: [u8; 0],
}

#[repr(C)]
pub(crate) struct AMediaFormat {
    _private: [u8; 0],
}

#[repr(C)]
#[derive(Default)]
pub(crate) struct BufferInfo {
    pub(crate) offset: i32,
    pub(crate) size: i32,
    pub(crate) presentation_time_us: i64,
    pub(crate) flags: u32,
}

// `off_t` is the fourth argument's type and is 64 bits on every ABI this ships
// for; the build refuses a 32-bit one rather than guessing which of the two
// widths that platform's `off_t` has.
#[cfg(target_pointer_width = "32")]
compile_error!("the MediaCodec binding assumes a 64-bit off_t");

#[link(name = "mediandk")]
unsafe extern "C" {
    pub(crate) fn AMediaFormat_new() -> *mut AMediaFormat;
    pub(crate) fn AMediaFormat_delete(format: *mut AMediaFormat) -> i32;
    pub(crate) fn AMediaFormat_setString(format: *mut AMediaFormat, name: *const c_char, value: *const c_char);
    pub(crate) fn AMediaFormat_setInt32(format: *mut AMediaFormat, name: *const c_char, value: i32);
    pub(crate) fn AMediaFormat_getInt32(format: *mut AMediaFormat, name: *const c_char, out: *mut i32) -> bool;
    pub(crate) fn AMediaFormat_getRect(
        format: *mut AMediaFormat,
        name: *const c_char,
        left: *mut i32,
        top: *mut i32,
        right: *mut i32,
        bottom: *mut i32,
    ) -> bool;
    pub(crate) fn AMediaFormat_setBuffer(
        format: *mut AMediaFormat,
        name: *const c_char,
        data: *const c_void,
        size: usize,
    );
    pub(crate) fn AMediaFormat_getBuffer(
        format: *mut AMediaFormat,
        name: *const c_char,
        data: *mut *mut c_void,
        size: *mut usize,
    ) -> bool;

    pub(crate) fn AMediaCodec_createDecoderByType(mime: *const c_char) -> *mut AMediaCodec;
    pub(crate) fn AMediaCodec_configure(
        codec: *mut AMediaCodec,
        format: *const AMediaFormat,
        surface: *mut c_void,
        crypto: *mut c_void,
        flags: u32,
    ) -> i32;
    pub(crate) fn AMediaCodec_start(codec: *mut AMediaCodec) -> i32;
    pub(crate) fn AMediaCodec_stop(codec: *mut AMediaCodec) -> i32;
    pub(crate) fn AMediaCodec_flush(codec: *mut AMediaCodec) -> i32;
    pub(crate) fn AMediaCodec_delete(codec: *mut AMediaCodec) -> i32;
    pub(crate) fn AMediaCodec_dequeueInputBuffer(codec: *mut AMediaCodec, timeout_us: i64) -> isize;
    pub(crate) fn AMediaCodec_getInputBuffer(
        codec: *mut AMediaCodec,
        index: usize,
        out_size: *mut usize,
    ) -> *mut u8;
    pub(crate) fn AMediaCodec_queueInputBuffer(
        codec: *mut AMediaCodec,
        index: usize,
        offset: i64,
        size: usize,
        time_us: u64,
        flags: u32,
    ) -> i32;
    pub(crate) fn AMediaCodec_dequeueOutputBuffer(
        codec: *mut AMediaCodec,
        info: *mut BufferInfo,
        timeout_us: i64,
    ) -> isize;
    pub(crate) fn AMediaCodec_getOutputBuffer(
        codec: *mut AMediaCodec,
        index: usize,
        out_size: *mut usize,
    ) -> *mut u8;
    pub(crate) fn AMediaCodec_getOutputFormat(codec: *mut AMediaCodec) -> *mut AMediaFormat;
    pub(crate) fn AMediaCodec_getBufferFormat(codec: *mut AMediaCodec, index: usize) -> *mut AMediaFormat;
    pub(crate) fn AMediaCodec_releaseOutputBuffer(
        codec: *mut AMediaCodec,
        index: usize,
        render: bool,
    ) -> i32;
    pub(crate) fn AMediaCodec_getName(codec: *mut AMediaCodec, out_name: *mut *mut c_char) -> i32;
    pub(crate) fn AMediaCodec_releaseName(codec: *mut AMediaCodec, name: *mut c_char);
}

// For the calls Android 16 added (API 36), looked up rather than linked:
// the app runs from API 31, where linking them would fail to load.
unsafe extern "C" {
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

/// bionic's `RTLD_DEFAULT`, which on a 64-bit ABI is the null handle:
/// every library already loaded, `libmediandk.so` among them.
const RTLD_DEFAULT: *mut c_void = std::ptr::null_mut();

pub(crate) const AMEDIA_OK: i32 = 0;
pub(crate) const INFO_OUTPUT_FORMAT_CHANGED: isize = -2;
pub(crate) const INFO_OUTPUT_BUFFERS_CHANGED: isize = -3;

/// MediaCodec's own constants, which the NDK headers do not declare: they are
/// Java fields on `MediaCodecInfo.CodecCapabilities` and `MediaFormat`. Asking
/// for Flexible is what makes a decoder describe its output in terms anything
/// portable can read rather than name a vendor layout.
const COLOR_FORMAT_YUV420_FLEXIBLE: i32 = 0x7F42_0888;
const COLOR_STANDARD_BT709: i32 = 1;
const COLOR_RANGE_FULL: i32 = 1;

/// `MediaCodec.BUFFER_FLAG_PARTIAL_FRAME`: this buffer holds part of a
/// frame, and the rest follows in the buffers up to one without the flag.
const BUFFER_FLAG_PARTIAL_FRAME: u32 = 8;

/// `image-data` is a `MediaImage2`, the same descriptor Java's `Image` is built
/// from: where each component starts, and what a row and a column of it cost.
const MEDIA_IMAGE2_HEADER: usize = 24; // type, planes, width, height, 2 × depth
const MEDIA_IMAGE2_PLANE: usize = 20; // offset, colInc, rowInc, 2 × subsampling
const MEDIA_IMAGE_TYPE_YUV: u32 = 1;

/// How long a decode waits for the frame it just fed. A frame that misses is
/// not lost (the next update draws one), so this bounds what a stall costs the
/// protocol thread rather than being a deadline. The first is longer because it
/// is the one the decoder is still working its output format out on.
const FIRST_FRAME_TIMEOUT_US: i64 = 200_000;
const FRAME_TIMEOUT_US: i64 = 30_000;

/// How long a decode of a frame's planes waits for its picture, which is
/// longer: a caller that asked for planes pairs each picture with its frame
/// (FreeRDP's AVC444), so a picture that misses is a frame lost rather than
/// one drawn late, and upstream's own MediaCodec decoder for it waits for as
/// long as it takes.
const PICTURE_TIMEOUT_US: i64 = 500_000;

/// One MediaCodec codec, as the NDK hands it out: what the video's
/// [`Decoder`] and the sound's (`mediacodec_audio`) are each made around,
/// so that both make, configure, start, stop and delete one the same way.
///
/// `Send`: in synchronous mode a codec is used from one thread at a time,
/// which is all either decoder trait promises, and the NDK's codec holds
/// nothing tied to the thread that made it: each call is posted to the
/// codec's own looper thread and waited for. TigerVNC's worker threads
/// feed one decoder from several threads in turn (see `api::c::video`).
/// Not `Sync`: nothing promises that two calls at once are safe.
pub(crate) struct Handle {
    raw: *mut AMediaCodec,
    /// Whether it has ever been started, which is when dropping it stops it
    /// before deleting it; one that never started is only deleted.
    started: bool,
}

// SAFETY: see above.
unsafe impl Send for Handle {}

impl Handle {
    /// A decoder of `mime`'s, not yet configured, or `None` where the
    /// platform has none.
    pub(crate) fn create(mime: &CStr) -> Option<Handle> {
        let raw = unsafe { AMediaCodec_createDecoderByType(mime.as_ptr()) };
        (!raw.is_null()).then_some(Handle { raw, started: false })
    }

    /// Configured with a format of `mime`, and of whatever else `keys` sets
    /// on it, and started; false where either fails.
    pub(crate) fn start(&mut self, mime: &CStr, keys: impl FnOnce(*mut AMediaFormat)) -> bool {
        let started = unsafe {
            let format = AMediaFormat_new();
            AMediaFormat_setString(format, c"mime".as_ptr(), mime.as_ptr());
            keys(format);
            let configured =
                AMediaCodec_configure(self.raw, format, std::ptr::null_mut(), std::ptr::null_mut(), 0);
            AMediaFormat_delete(format);
            configured == AMEDIA_OK && AMediaCodec_start(self.raw) == AMEDIA_OK
        };
        self.started |= started;
        started
    }

    /// Stopped, to be configured and started again.
    pub(crate) fn stop(&mut self) {
        unsafe { AMediaCodec_stop(self.raw) };
    }

    /// The codec, for the calls that feed it and drain it.
    pub(crate) fn raw(&self) -> *mut AMediaCodec {
        self.raw
    }

    /// The name MediaCodec gave the codec it picked, or `None` where it says
    /// nothing.
    pub(crate) fn name(&self) -> Option<String> {
        let mut name: *mut c_char = std::ptr::null_mut();
        if unsafe { AMediaCodec_getName(self.raw, &mut name) } != AMEDIA_OK || name.is_null() {
            return None;
        }
        let owned = unsafe { CStr::from_ptr(name) }.to_string_lossy().into_owned();
        unsafe { AMediaCodec_releaseName(self.raw, name) };
        Some(owned)
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            if self.started {
                AMediaCodec_stop(self.raw);
            }
            AMediaCodec_delete(self.raw);
        }
    }
}

/// Whether the platform has a decoder of `mime`'s at all, without
/// configuring one: what both decoders' `decodable` asks once.
pub(crate) fn exists(mime: &str) -> bool {
    CString::new(mime).ok().and_then(|mime| Handle::create(&mime)).is_some()
}

/// How much of a frame one input buffer is asked to hold: the picture
/// uncompressed, at 4:2:0, which a frame of video does not reach in
/// practice: a key frame of a 3840x2160 desktop of random colours was 10 MB
/// of the 12.4 this asks for. Asked for rather than left to the decoder,
/// whose own choice need not grow with the picture: the Pixel 8's H.264
/// decoder gives 7 MiB at any size (and keeps to it when asked for less),
/// which a key frame of a busy 4K desktop does not fit. A frame larger still
/// starts the decoder over with room for it ([`Decoder::feed`]). Never less
/// than a megabyte, for a small picture's headers.
fn input_size(width: usize, height: usize) -> usize {
    (width * height * 3 / 2).max(1 << 20)
}

/// The phone's decoders, as a session is handed them.
pub struct MediaCodec;

impl Decoders for MediaCodec {
    /// Asked once and remembered, because the answer is the device's and
    /// cannot change under a running app.
    fn decodable(&self, codec: Codec) -> bool {
        static PROBED: OnceLock<[bool; Codec::ALL.len()]> = OnceLock::new();
        let probed = PROBED.get_or_init(|| Codec::ALL.map(can_decode));
        probed[Codec::ALL.iter().position(|c| *c == codec).unwrap()]
    }

    fn decoder(
        &self,
        codec: Codec,
        width: usize,
        height: usize,
    ) -> Option<Box<dyn runtime::media::video::Decoder>> {
        Decoder::new(codec, width, height).map(|d| Box::new(d) as Box<dyn runtime::media::video::Decoder>)
    }
}

/// What MediaCodec calls a codec, which is a MIME type.
fn mime(codec: Codec) -> &'static str {
    match codec {
        Codec::Vp9 => "video/x-vnd.on2.vp9",
        Codec::Vp8 => "video/x-vnd.on2.vp8",
        Codec::Av1 => "video/av01",
        Codec::H264 => "video/avc",
        Codec::H265 => "video/hevc",
    }
}

/// Whether the platform has a decoder for this codec, without configuring one:
/// a codec that exists here may still refuse a particular size, which is
/// [`Decoder::new`]'s answer rather than this one.
fn can_decode(codec: Codec) -> bool {
    exists(mime(codec))
}

pub struct Decoder {
    codec: Handle,
    kind: Codec,
    /// The codec's own name, `c2.qti.avc.decoder` or `c2.android.avc.decoder`,
    /// and what the platform says it is: hardware or software, decided once
    /// when the decoder is made, since a codec does not change its nature.
    name: String,
    hardware: Option<bool>,
    /// What one input buffer was asked to hold, and whether the codec takes
    /// a frame in parts where one does not hold it.
    max_input: usize,
    partial_frames: bool,
    pts: u64,
    saw_frame: bool,
    /// Whether the next picture is waited for as the first is: true from
    /// when the decoder starts, and again after it starts over.
    fresh: bool,
    failed: bool,
    /// A frame was dropped or the decoder started over without its key
    /// frame, so the stream refers back to pictures it no longer has: for
    /// [`take_key_frame_request`](runtime::media::video::Decoder::take_key_frame_request).
    key_frame_wanted: bool,
    planes: [Plane; 3],
    have_layout: bool,
    frame_width: i32,
    frame_height: i32,
    bt709: bool,
    full_range: bool,
    /// The last frame's planes and the vectorised conversion's verdict, which
    /// together decide how little of this one is converted.
    output: Output,
    /// Frames fed for their planes whose pictures have not come out yet.
    owed: u32,
}

impl Decoder {
    pub fn new(codec: Codec, width: usize, height: usize) -> Option<Decoder> {
        let mime = CString::new(mime(codec)).ok()?;
        let Some(mut handle) = Handle::create(&mime) else {
            log::warn!("no {} decoder on this device", codec.name());
            return None;
        };
        let max_input = input_size(width, height);
        if !start(&mut handle, codec, width, height, max_input) {
            log::warn!("could not start a {} decoder for {width}x{height}", codec.name());
            return None;
        }
        let name = handle.name();
        let hardware = name.as_deref().and_then(is_hardware);
        let partial_frames = name.as_deref().is_some_and(takes_partial_frames);
        log::info!(
            "{} decoder started for {width}x{height}: {}, {}",
            codec.name(),
            name.as_deref().unwrap_or("unnamed"),
            match hardware {
                Some(true) => "hardware",
                Some(false) => "software",
                None => "not known whether hardware",
            }
        );
        Some(Decoder {
            codec: handle,
            kind: codec,
            name: name.unwrap_or_default(),
            hardware,
            max_input,
            partial_frames,
            pts: 0,
            saw_frame: false,
            fresh: true,
            failed: false,
            key_frame_wanted: false,
            planes: [Plane::default(); 3],
            have_layout: false,
            frame_width: width as i32,
            frame_height: height as i32,
            bt709: false,
            full_range: false,
            output: Output::new(),
            owed: 0,
        })
    }

    /// The frame into the decoder's input: one buffer for one frame, which is
    /// what a decoder takes a buffer to be.
    ///
    /// A frame larger than an input buffer (a key frame of a large, busy
    /// desktop) starts the decoder over with buffers large enough, and is
    /// fed to it if it is one a fresh decoder can start from; otherwise it is
    /// dropped and a key frame asked for. Where the codec will not give
    /// buffers that large, a codec that takes a frame in parts is given it in
    /// parts, and any other drops it (and asks for a key frame, unless it was
    /// one). A frame is never cut into several buffers queued as frames: a
    /// decoder takes each for a whole one, and decodes the parts of a key
    /// frame as two broken pictures, or none.
    fn feed(&mut self, data: &[u8]) -> bool {
        loop {
            let index = unsafe { AMediaCodec_dequeueInputBuffer(self.codec.raw(), FRAME_TIMEOUT_US) };
            if index < 0 {
                log::warn!("no input buffer, dropping {} bytes", data.len());
                return false;
            }
            let mut capacity = 0usize;
            let buffer =
                unsafe { AMediaCodec_getInputBuffer(self.codec.raw(), index as usize, &mut capacity) };
            if buffer.is_null() || capacity == 0 {
                return false;
            }
            if data.len() <= capacity {
                unsafe {
                    std::ptr::copy_nonoverlapping(data.as_ptr(), buffer, data.len());
                    AMediaCodec_queueInputBuffer(self.codec.raw(), index as usize, 0, data.len(), self.pts, 0);
                }
                self.pts += 1000;
                return true;
            }
            if data.len() > self.max_input {
                // A quarter more than this frame, since the next key frame of
                // the same desktop is no smaller. The buffer just taken goes
                // back with the stop.
                let wanted = data.len() + data.len() / 4;
                log::info!(
                    "a {}-byte {} frame does not fit the decoder's {capacity}-byte input buffers, \
                     starting it over with room for {wanted}",
                    data.len(),
                    self.kind.name()
                );
                if !self.restart(wanted) {
                    return false;
                }
                if bitstream::starts_stream(self.kind, data) {
                    continue;
                }
                log::warn!("the frame was not a key frame, so it is dropped and one asked for");
                self.key_frame_wanted = true;
                return false;
            }
            // As large as was asked for, and still the codec gave less.
            if self.partial_frames {
                return self.feed_parts(index as usize, buffer, capacity, data);
            }
            // Dropped, and a key frame asked for, unless this was one, since
            // the next would be as large, and asking would only have the far
            // end send that again and again. The picture then waits for a key
            // frame that fits.
            let key = bitstream::starts_stream(self.kind, data);
            log::warn!(
                "a {}-byte{} frame does not fit the decoder's {capacity}-byte input buffers, dropped{}",
                data.len(),
                if key { " key" } else { "" },
                if key { "" } else { "; asking for a key frame" }
            );
            // Given back empty, which a decoder takes as no frame at all.
            unsafe { AMediaCodec_queueInputBuffer(self.codec.raw(), index as usize, 0, 0, self.pts, 0) };
            self.key_frame_wanted = !key;
            return false;
        }
    }

    /// A frame in as many buffers as it takes, the first already taken, each
    /// but the last flagged as part of a frame and all with the frame's one
    /// timestamp: for a codec that says it reassembles them.
    fn feed_parts(&mut self, mut index: usize, mut buffer: *mut u8, mut capacity: usize, data: &[u8]) -> bool {
        let mut rest = data;
        loop {
            let take = rest.len().min(capacity);
            let last = take == rest.len();
            let flags = if last { 0 } else { BUFFER_FLAG_PARTIAL_FRAME };
            unsafe {
                std::ptr::copy_nonoverlapping(rest.as_ptr(), buffer, take);
                AMediaCodec_queueInputBuffer(self.codec.raw(), index, 0, take, self.pts, flags);
            }
            rest = &rest[take..];
            if last {
                self.pts += 1000;
                return true;
            }
            let next = unsafe { AMediaCodec_dequeueInputBuffer(self.codec.raw(), FRAME_TIMEOUT_US) };
            buffer = if next >= 0 {
                unsafe { AMediaCodec_getInputBuffer(self.codec.raw(), next as usize, &mut capacity) }
            } else {
                std::ptr::null_mut()
            };
            if buffer.is_null() || capacity == 0 {
                // Part of a frame is queued, and the next frame would be taken
                // for its rest: the input is flushed instead, which loses the
                // pictures the stream refers back to.
                log::warn!("no input buffer for the rest of a {}-byte frame, dropped", data.len());
                unsafe { AMediaCodec_flush(self.codec.raw()) };
                self.owed = 0;
                self.key_frame_wanted = true;
                return false;
            }
            index = next as usize;
        }
    }

    /// The decoder stopped and configured again for the same picture with
    /// input buffers of `max_input` bytes. What it held is gone: the pictures
    /// it was still working on and the ones the stream refers back to.
    fn restart(&mut self, max_input: usize) -> bool {
        self.codec.stop();
        self.max_input = max_input;
        self.owed = 0;
        self.fresh = true;
        self.have_layout = false;
        let (width, height) = (self.frame_width.max(1) as usize, self.frame_height.max(1) as usize);
        if !start(&mut self.codec, self.kind, width, height, max_input) {
            log::warn!("the {} decoder would not start again", self.kind.name());
            self.failed = true;
            return false;
        }
        true
    }

    /// The next decoded buffer's index, or a negative number when there is
    /// none waiting.
    fn next_buffer(&mut self, info: &mut BufferInfo, timeout: i64) -> isize {
        loop {
            let index = unsafe { AMediaCodec_dequeueOutputBuffer(self.codec.raw(), info, timeout) };
            if index == INFO_OUTPUT_FORMAT_CHANGED || index == INFO_OUTPUT_BUFFERS_CHANGED {
                continue;
            }
            return index;
        }
    }

    /// The latest decoded buffer, converted into `out`. False when there is
    /// none waiting.
    ///
    /// **Only the last one is converted.** A decoder that has fallen behind
    /// hands back several buffers at once, and every frame here is a whole
    /// picture, so converting each in turn is 9 MB of writes for a picture the
    /// next one overwrites before it is shown. The earlier buffers are
    /// released unread, which is what lets a session that fell behind catch up
    /// inside one update instead of over the next several.
    fn draw_next(
        &mut self,
        out: &mut [u32],
        width: usize,
        height: usize,
        timeout: i64,
        changed: &mut Changes,
    ) -> bool {
        let mut info = BufferInfo::default();
        let mut index = self.next_buffer(&mut info, timeout);
        if index < 0 {
            return false; // nothing yet, or an error this cannot act on
        }
        loop {
            let mut newer = BufferInfo::default();
            let next = self.next_buffer(&mut newer, 0);
            if next < 0 {
                break;
            }
            unsafe { AMediaCodec_releaseOutputBuffer(self.codec.raw(), index as usize, false) };
            index = next;
            info = newer;
        }
        self.take_buffer(index as usize, &info, &mut |frame, output| {
            output.write(frame, out, width, height, changed)
        });
        true
    }

    /// The next decoded buffer in the order they were fed, for
    /// [`decode_planes`](runtime::media::video::Decoder::decode_planes): one
    /// picture per frame, none passed over. A wait that runs out leaves its
    /// picture owed, and an owed picture is the frame before's, so it is
    /// released unread when it comes rather than handed over as this frame's.
    fn planes_next(&mut self, planes: &mut dyn FnMut(&Frame<'_>)) -> bool {
        loop {
            let mut info = BufferInfo::default();
            let index = self.next_buffer(&mut info, PICTURE_TIMEOUT_US);
            if index < 0 {
                log::warn!("no picture after {} ms", PICTURE_TIMEOUT_US / 1000);
                return false;
            }
            self.owed = self.owed.saturating_sub(1);
            if self.owed > 0 {
                unsafe { AMediaCodec_releaseOutputBuffer(self.codec.raw(), index as usize, false) };
                continue;
            }
            return self.take_buffer(index as usize, &info, &mut |frame, _| planes(frame));
        }
    }

    /// The decoded buffer at `index`, its format read and its planes handed
    /// to `picture` with the decoder's [`Output`], and then released. True
    /// when there was a picture to hand over.
    fn take_buffer(
        &mut self,
        index: usize,
        info: &BufferInfo,
        picture: &mut dyn FnMut(&Frame<'_>, &mut Output),
    ) -> bool {
        let format = unsafe { AMediaCodec_getBufferFormat(self.codec.raw(), index) };
        if !format.is_null() {
            self.read_format(format);
            unsafe { AMediaFormat_delete(format) };
        }

        // The platform logs one `D/Codec2Buffer: ConstGraphicBlockBuffer::
        // canCopy: wrapped ; buffer ref doesn't exist` per frame here, and it is
        // the good case rather than a fault: the framework is saying it will not
        // copy this decoded block into a client buffer, so what comes back
        // below wraps the codec's own memory and the conversion reads it in
        // place. It is logged for every no-surface `getOutputBuffer` user.
        let mut size = 0usize;
        let buffer = unsafe { AMediaCodec_getOutputBuffer(self.codec.raw(), index, &mut size) };
        let readable = !buffer.is_null() && info.size > 0 && self.have_layout;
        if readable {
            let bytes = unsafe {
                std::slice::from_raw_parts(
                    buffer.add(info.offset.max(0) as usize),
                    size.saturating_sub(info.offset.max(0) as usize),
                )
            };
            // One buffer with all three planes in it, which is the shape the
            // descriptor describes.
            let frame = Frame::one(
                bytes,
                self.planes,
                self.frame_width.max(0) as usize,
                self.frame_height.max(0) as usize,
                self.bt709,
                self.full_range,
            );
            picture(&frame, &mut self.output);
        }
        unsafe { AMediaCodec_releaseOutputBuffer(self.codec.raw(), index, false) };
        readable
    }

    fn read_format(&mut self, format: *mut AMediaFormat) {
        unsafe {
            AMediaFormat_getInt32(format, c"width".as_ptr(), &mut self.frame_width);
            AMediaFormat_getInt32(format, c"height".as_ptr(), &mut self.frame_height);
            // The visible rectangle, where the coded one is padded up to a
            // macroblock. The planes' offsets are the coded picture's, so
            // where the rectangle does not start at its corner they are
            // moved to where it does, as Java's `Image` moves them.
            let (mut l, mut t, mut r, mut b) = (0, 0, 0, 0);
            if AMediaFormat_getRect(format, c"crop".as_ptr(), &mut l, &mut t, &mut r, &mut b) {
                self.frame_width = r - l + 1;
                self.frame_height = b - t + 1;
            } else {
                (l, t) = (0, 0);
            }
            let (left, top) = (l.max(0) as usize, t.max(0) as usize);

            // Both out of the bitstream's own description of itself. A stream
            // that says nothing is taken for BT.601, which is what the encoders
            // this has met produce untagged (a RustDesk peer's libvpx and
            // libaom are handed libyuv's BT.601 and told nothing), and what
            // `platform_linux`'s libavcodec path assumes for an untagged stream
            // too.
            // (A framework that fills in a default of its own by picture size
            // answers before this does.)
            let mut standard = 0;
            let mut range = 0;
            self.bt709 = AMediaFormat_getInt32(format, c"color-standard".as_ptr(), &mut standard)
                && standard == COLOR_STANDARD_BT709;
            self.full_range = AMediaFormat_getInt32(format, c"color-range".as_ptr(), &mut range)
                && range == COLOR_RANGE_FULL;

            self.have_layout = false;
            let mut data: *mut c_void = std::ptr::null_mut();
            let mut size = 0usize;
            if !AMediaFormat_getBuffer(format, c"image-data".as_ptr(), &mut data, &mut size)
                || data.is_null()
                || size < MEDIA_IMAGE2_HEADER + 3 * MEDIA_IMAGE2_PLANE
            {
                log::warn!("the decoder did not describe its output planes");
                self.failed = true;
                return;
            }
            let described = std::slice::from_raw_parts(data as *const u8, size);
            if read_i32(described, 0) as u32 != MEDIA_IMAGE_TYPE_YUV
                || (read_i32(described, 4) as u32) < 3
            {
                log::warn!("the decoder's output is not three-plane YUV");
                self.failed = true;
                return;
            }
            for i in 0..3 {
                let at = MEDIA_IMAGE2_HEADER + i * MEDIA_IMAGE2_PLANE;
                let mut plane = Plane {
                    offset: read_i32(described, at).max(0) as usize,
                    col_inc: read_i32(described, at + 4).max(0) as usize,
                    row_inc: read_i32(described, at + 8).max(0) as usize,
                    horiz_subsampling: read_i32(described, at + 12).max(0) as usize,
                    vert_subsampling: read_i32(described, at + 16).max(0) as usize,
                };
                if plane.horiz_subsampling == 0
                    || plane.vert_subsampling == 0
                    || plane.col_inc == 0
                    || plane.row_inc == 0
                {
                    log::warn!("plane {i} has a layout this cannot read");
                    self.failed = true;
                    return;
                }
                plane.offset += top / plane.vert_subsampling * plane.row_inc
                    + left / plane.horiz_subsampling * plane.col_inc;
                self.planes[i] = plane;
            }
            self.have_layout = true;
        }
    }
}

impl runtime::media::video::Decoder for Decoder {
    fn decode(
        &mut self,
        data: &[u8],
        out: &mut [u32],
        width: usize,
        height: usize,
        changed: &mut Changes,
    ) -> bool {
        if self.failed || data.is_empty() || out.len() < width * height {
            return false;
        }
        if !self.feed(data) {
            return false;
        }
        let timeout = if self.fresh {
            FIRST_FRAME_TIMEOUT_US
        } else {
            FRAME_TIMEOUT_US
        };
        if !self.draw_next(out, width, height, timeout, changed) {
            return false;
        }
        self.saw_frame = true;
        self.fresh = false;
        true
    }

    fn decode_planes(&mut self, data: &[u8], planes: &mut dyn FnMut(&Frame<'_>)) -> bool {
        if self.failed || data.is_empty() || !self.feed(data) {
            return false;
        }
        self.owed += 1;
        if !self.planes_next(planes) {
            return false;
        }
        self.saw_frame = true;
        self.fresh = false;
        true
    }

    /// The platform's answer for this codec, once there is a picture to have
    /// decoded: hardware stays MediaCodec's choice on a phone, so this only
    /// says which one it made.
    fn hardware(&self) -> Option<bool> {
        self.hardware.filter(|_| self.saw_frame)
    }

    fn status(&self) -> Option<String> {
        (!self.name.is_empty()).then(|| format!("{} {}", self.kind.name(), self.name))
    }

    fn take_key_frame_request(&mut self) -> bool {
        std::mem::take(&mut self.key_frame_wanted)
    }
}

/// `handle` configured for a picture of this size, with input buffers of
/// `max_input` bytes, and started.
fn start(handle: &mut Handle, codec: Codec, width: usize, height: usize, max_input: usize) -> bool {
    let Ok(mime) = CString::new(mime(codec)) else {
        return false;
    };
    handle.start(&mime, |format| unsafe {
        AMediaFormat_setInt32(format, c"width".as_ptr(), width as i32);
        AMediaFormat_setInt32(format, c"height".as_ptr(), height as i32);
        AMediaFormat_setInt32(format, c"color-format".as_ptr(), COLOR_FORMAT_YUV420_FLEXIBLE);
        AMediaFormat_setInt32(format, c"low-latency".as_ptr(), 1);
        AMediaFormat_setInt32(format, c"max-input-size".as_ptr(), max_input.min(i32::MAX as usize) as i32);
    })
}

/// Whether the codec of this name decodes in hardware.
///
    /// The platform's own answer where it has one to give (Android 16's
/// codec store says hardware-accelerated, software-only or vendor software
/// for a codec by name, which is what `MediaCodecInfo.isHardwareAccelerated`
/// reads in Java), and before that, the rule apps had to use: the
/// platform's own codecs are named `c2.android.`, `c2.google.` or
/// `omx.google.`, and a few vendors mark a software one in its name.
pub(crate) fn is_hardware(name: &str) -> Option<bool> {
    type GetCodecInfo = unsafe extern "C" fn(*const c_char, *mut *const c_void) -> i32;
    type GetInfoType = unsafe extern "C" fn(*const c_void) -> i32;
    static STORE: OnceLock<Option<(GetCodecInfo, GetInfoType)>> = OnceLock::new();
    let store = STORE.get_or_init(|| unsafe {
        let info = dlsym(RTLD_DEFAULT, c"AMediaCodecStore_getCodecInfo".as_ptr());
        let kind = dlsym(RTLD_DEFAULT, c"AMediaCodecInfo_getMediaCodecInfoType".as_ptr());
        (!info.is_null() && !kind.is_null()).then(|| {
            (
                std::mem::transmute::<*mut c_void, GetCodecInfo>(info),
                std::mem::transmute::<*mut c_void, GetInfoType>(kind),
            )
        })
    });
    if let (Some((get_info, get_type)), Ok(c_name)) = (store, CString::new(name)) {
        let mut info: *const c_void = std::ptr::null();
        if unsafe { get_info(c_name.as_ptr(), &mut info) } == AMEDIA_OK && !info.is_null() {
            // AMediaCodecType: software only, hardware accelerated, and
            // software with device access, which is a vendor's software.
            match unsafe { get_type(info) } {
                1 | 3 => return Some(false),
                2 => return Some(true),
                _ => {}
            }
        }
    }
    Some(!software_by_name(name))
}

/// Whether the codec of this name takes a frame in several input buffers
/// (`FEATURE_PartialFrame`, `BUFFER_FLAG_PARTIAL_FRAME` on all but the last),
/// which native code can ask only of Android 16's codec store: before it, and
/// for a codec the store does not know, the answer is no. The Pixel 8's
/// H.264 decoder lists no such feature.
fn takes_partial_frames(name: &str) -> bool {
    type GetCodecInfo = unsafe extern "C" fn(*const c_char, *mut *const c_void) -> i32;
    type IsFeatureSupported = unsafe extern "C" fn(*const c_void, *const c_char) -> i32;
    static STORE: OnceLock<Option<(GetCodecInfo, IsFeatureSupported)>> = OnceLock::new();
    let store = STORE.get_or_init(|| unsafe {
        let info = dlsym(RTLD_DEFAULT, c"AMediaCodecStore_getCodecInfo".as_ptr());
        let feature = dlsym(RTLD_DEFAULT, c"AMediaCodecInfo_isFeatureSupported".as_ptr());
        (!info.is_null() && !feature.is_null()).then(|| {
            (
                std::mem::transmute::<*mut c_void, GetCodecInfo>(info),
                std::mem::transmute::<*mut c_void, IsFeatureSupported>(feature),
            )
        })
    });
    let (Some((get_info, supported)), Ok(c_name)) = (store, CString::new(name)) else {
        return false;
    };
    let mut info: *const c_void = std::ptr::null();
    unsafe {
        get_info(c_name.as_ptr(), &mut info) == AMEDIA_OK
            && !info.is_null()
            && supported(info, c"partial-frame".as_ptr()) == 1
    }
}

/// Whether a codec's name marks it as software, as ExoPlayer and the
/// platform's own tests decided it before there was an API for it.
fn software_by_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    if name.starts_with("arc.") {
        // ChromeOS's, which hand the stream to the host's hardware.
        return false;
    }
    name.starts_with("omx.google.")
        || name.starts_with("omx.ffmpeg.")
        || (name.starts_with("omx.sec.") && name.contains(".sw."))
        || name == "omx.qcom.video.decoder.hevcswvdec"
        || name.starts_with("c2.android.")
        || name.starts_with("c2.google.")
        || (!name.starts_with("omx.") && !name.starts_with("c2."))
}

fn read_i32(bytes: &[u8], at: usize) -> i32 {
    i32::from_ne_bytes(bytes[at..at + 4].try_into().unwrap_or([0; 4]))
}
