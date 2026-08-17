//! The phone's own decoder, through the NDK's C API.
//!
//! Declared here rather than taken from a crate: it is a dozen functions with a
//! stable ABI, and `#[link]` on the extern block is the whole of the build
//! configuration — the same argument `common::android::bitmap` makes for
//! `jnigraphics`.
//!
//! Three things are not obvious and each cost a measurement to find, two of
//! them in [49](../../../../../notes/49-h264.md)'s H.264 path before this
//! existed:
//!
//! - **No surface.** The pixels have to come back to the CPU, because what the
//!   seam promises is a region read out of a framebuffer.
//! - **`low-latency`, because a held frame is a desktop that lags the finger on
//!   it.** A decoder is free to hold frames back to reorder them, and a remote
//!   desktop has nothing to reorder.
//! - **The layout is read from the buffer's own format, per buffer**, and out of
//!   `image-data` rather than from the colour format: the first buffer of a
//!   session does not have the same stride as the rest, and a decoder asked for
//!   Flexible reports Flexible and nothing more.

use std::ffi::{CString, c_char, c_void};

use super::convert::{Layout, Plane};
use super::diff::{Changes, Previous};
use super::{Codec, convert};

#[repr(C)]
struct AMediaCodec {
    _private: [u8; 0],
}

#[repr(C)]
struct AMediaFormat {
    _private: [u8; 0],
}

#[repr(C)]
#[derive(Default)]
struct BufferInfo {
    offset: i32,
    size: i32,
    presentation_time_us: i64,
    flags: u32,
}

// `off_t` is the fourth argument's type and is 64 bits on every ABI this ships
// for; the build refuses a 32-bit one rather than guessing which of the two
// widths that platform's `off_t` has.
#[cfg(target_pointer_width = "32")]
compile_error!("the MediaCodec binding assumes a 64-bit off_t");

#[link(name = "mediandk")]
unsafe extern "C" {
    fn AMediaFormat_new() -> *mut AMediaFormat;
    fn AMediaFormat_delete(format: *mut AMediaFormat) -> i32;
    fn AMediaFormat_setString(format: *mut AMediaFormat, name: *const c_char, value: *const c_char);
    fn AMediaFormat_setInt32(format: *mut AMediaFormat, name: *const c_char, value: i32);
    fn AMediaFormat_getInt32(format: *mut AMediaFormat, name: *const c_char, out: *mut i32) -> bool;
    fn AMediaFormat_getRect(
        format: *mut AMediaFormat,
        name: *const c_char,
        left: *mut i32,
        top: *mut i32,
        right: *mut i32,
        bottom: *mut i32,
    ) -> bool;
    fn AMediaFormat_getBuffer(
        format: *mut AMediaFormat,
        name: *const c_char,
        data: *mut *mut c_void,
        size: *mut usize,
    ) -> bool;

    fn AMediaCodec_createDecoderByType(mime: *const c_char) -> *mut AMediaCodec;
    fn AMediaCodec_configure(
        codec: *mut AMediaCodec,
        format: *const AMediaFormat,
        surface: *mut c_void,
        crypto: *mut c_void,
        flags: u32,
    ) -> i32;
    fn AMediaCodec_start(codec: *mut AMediaCodec) -> i32;
    fn AMediaCodec_stop(codec: *mut AMediaCodec) -> i32;
    fn AMediaCodec_delete(codec: *mut AMediaCodec) -> i32;
    fn AMediaCodec_dequeueInputBuffer(codec: *mut AMediaCodec, timeout_us: i64) -> isize;
    fn AMediaCodec_getInputBuffer(
        codec: *mut AMediaCodec,
        index: usize,
        out_size: *mut usize,
    ) -> *mut u8;
    fn AMediaCodec_queueInputBuffer(
        codec: *mut AMediaCodec,
        index: usize,
        offset: i64,
        size: usize,
        time_us: u64,
        flags: u32,
    ) -> i32;
    fn AMediaCodec_dequeueOutputBuffer(
        codec: *mut AMediaCodec,
        info: *mut BufferInfo,
        timeout_us: i64,
    ) -> isize;
    fn AMediaCodec_getOutputBuffer(
        codec: *mut AMediaCodec,
        index: usize,
        out_size: *mut usize,
    ) -> *mut u8;
    fn AMediaCodec_getBufferFormat(codec: *mut AMediaCodec, index: usize) -> *mut AMediaFormat;
    fn AMediaCodec_releaseOutputBuffer(
        codec: *mut AMediaCodec,
        index: usize,
        render: bool,
    ) -> i32;
}

const AMEDIA_OK: i32 = 0;
const INFO_OUTPUT_FORMAT_CHANGED: isize = -2;
const INFO_OUTPUT_BUFFERS_CHANGED: isize = -3;

/// MediaCodec's own constants, which the NDK headers do not declare: they are
/// Java fields on `MediaCodecInfo.CodecCapabilities` and `MediaFormat`. Asking
/// for Flexible is what makes a decoder describe its output in terms anything
/// portable can read rather than name a vendor layout.
const COLOR_FORMAT_YUV420_FLEXIBLE: i32 = 0x7F42_0888;
const COLOR_STANDARD_BT709: i32 = 1;
const COLOR_RANGE_FULL: i32 = 1;

/// `image-data` is a `MediaImage2`, the same descriptor Java's `Image` is built
/// from: where each component starts, and what a row and a column of it cost.
const MEDIA_IMAGE2_HEADER: usize = 24; // type, planes, width, height, 2 × depth
const MEDIA_IMAGE2_PLANE: usize = 20; // offset, colInc, rowInc, 2 × subsampling
const MEDIA_IMAGE_TYPE_YUV: u32 = 1;

/// How long a decode waits for the frame it just fed. A frame that misses is
/// not lost — the next update draws one — so this bounds what a stall costs the
/// protocol thread rather than being a deadline. The first is longer because it
/// is the one the decoder is still working its output format out on.
const FIRST_FRAME_TIMEOUT_US: i64 = 200_000;
const FRAME_TIMEOUT_US: i64 = 30_000;

/// Whether the platform has a decoder for this codec, without configuring one:
/// a codec that exists here may still refuse a particular size, which is
/// [`Decoder::new`]'s answer rather than this one.
pub fn can_decode(codec: Codec) -> bool {
    let Ok(mime) = CString::new(codec.mime()) else {
        return false;
    };
    let handle = unsafe { AMediaCodec_createDecoderByType(mime.as_ptr()) };
    if handle.is_null() {
        return false;
    }
    unsafe { AMediaCodec_delete(handle) };
    true
}

pub struct Decoder {
    codec: *mut AMediaCodec,
    pts: u64,
    saw_frame: bool,
    failed: bool,
    planes: [Plane; 3],
    have_layout: bool,
    frame_width: i32,
    frame_height: i32,
    bt709: bool,
    full_range: bool,
    /// The layout the vectorised conversion was last checked against, and what
    /// it said. Checked once per layout rather than per frame.
    checked: Option<(Layout, bool)>,
    /// The last frame's planes, which is what says how much of this one is
    /// worth converting.
    previous: Previous,
    /// Buffers released without being converted, because a newer whole picture
    /// was already waiting behind them.
    pub skipped: u64,
}

impl Decoder {
    pub fn new(codec: Codec, width: usize, height: usize) -> Option<Decoder> {
        let mime = CString::new(codec.mime()).ok()?;
        let handle = unsafe { AMediaCodec_createDecoderByType(mime.as_ptr()) };
        if handle.is_null() {
            log::warn!("no {} decoder on this device", codec.name());
            return None;
        }
        let started = unsafe {
            let format = AMediaFormat_new();
            AMediaFormat_setString(format, c"mime".as_ptr(), mime.as_ptr());
            AMediaFormat_setInt32(format, c"width".as_ptr(), width as i32);
            AMediaFormat_setInt32(format, c"height".as_ptr(), height as i32);
            AMediaFormat_setInt32(format, c"color-format".as_ptr(), COLOR_FORMAT_YUV420_FLEXIBLE);
            AMediaFormat_setInt32(format, c"low-latency".as_ptr(), 1);
            let configured = AMediaCodec_configure(
                handle,
                format,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                0,
            );
            AMediaFormat_delete(format);
            configured == AMEDIA_OK && AMediaCodec_start(handle) == AMEDIA_OK
        };
        if !started {
            log::warn!("could not start a {} decoder for {width}x{height}", codec.name());
            unsafe { AMediaCodec_delete(handle) };
            return None;
        }
        log::info!("{} decoder started for {width}x{height}", codec.name());
        Some(Decoder {
            codec: handle,
            pts: 0,
            saw_frame: false,
            failed: false,
            planes: [Plane::default(); 3],
            have_layout: false,
            frame_width: width as i32,
            frame_height: height as i32,
            bt709: true,
            full_range: false,
            checked: None,
            previous: Previous::new(),
            skipped: 0,
        })
    }

    fn feed(&mut self, data: &[u8]) -> bool {
        let index = unsafe { AMediaCodec_dequeueInputBuffer(self.codec, FRAME_TIMEOUT_US) };
        if index < 0 {
            log::warn!("no input buffer, dropping {} bytes", data.len());
            return false;
        }
        let mut capacity = 0usize;
        let buffer =
            unsafe { AMediaCodec_getInputBuffer(self.codec, index as usize, &mut capacity) };
        if buffer.is_null() || capacity < data.len() {
            // Unlike H.264 there is nothing to split at: a VP9 frame is one
            // access unit and half of one is not a picture.
            log::warn!("a {} byte frame does not fit {capacity} bytes", data.len());
            return false;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr(), buffer, data.len());
            AMediaCodec_queueInputBuffer(
                self.codec,
                index as usize,
                0,
                data.len(),
                self.pts,
                0,
            );
        }
        self.pts += 1000;
        true
    }

    /// The next decoded buffer's index, or a negative number when there is
    /// none waiting.
    fn next_buffer(&mut self, info: &mut BufferInfo, timeout: i64) -> isize {
        loop {
            let index = unsafe { AMediaCodec_dequeueOutputBuffer(self.codec, info, timeout) };
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
    /// picture — so converting each in turn is 9 MB of writes for a picture the
    /// next one overwrites before anybody sees it. The earlier buffers are
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
            unsafe { AMediaCodec_releaseOutputBuffer(self.codec, index as usize, false) };
            index = next;
            info = newer;
            self.skipped += 1;
        }
        let index = index as usize;

        let format = unsafe { AMediaCodec_getBufferFormat(self.codec, index) };
        if !format.is_null() {
            self.read_format(format);
            unsafe { AMediaFormat_delete(format) };
        }

        // One `D/Codec2Buffer: ConstGraphicBlockBuffer::canCopy: wrapped ;
        // buffer ref doesn't exist` a frame comes out of the platform here, and
        // it is the good case rather than a fault: the framework is saying it
        // will not copy this decoded block into a client buffer, so what comes
        // back below wraps the codec's own memory and the conversion reads it
        // in place. Every no-surface `getOutputBuffer` user prints it.
        let mut size = 0usize;
        let buffer = unsafe { AMediaCodec_getOutputBuffer(self.codec, index, &mut size) };
        if !buffer.is_null() && info.size > 0 {
            let bytes = unsafe {
                std::slice::from_raw_parts(
                    buffer.add(info.offset.max(0) as usize),
                    size.saturating_sub(info.offset.max(0) as usize),
                )
            };
            self.convert(bytes, out, width, height, changed);
        }
        unsafe { AMediaCodec_releaseOutputBuffer(self.codec, index, false) };
        true
    }

    fn read_format(&mut self, format: *mut AMediaFormat) {
        unsafe {
            AMediaFormat_getInt32(format, c"width".as_ptr(), &mut self.frame_width);
            AMediaFormat_getInt32(format, c"height".as_ptr(), &mut self.frame_height);
            // The visible rectangle, where the coded one is padded up to a
            // macroblock.
            let (mut l, mut t, mut r, mut b) = (0, 0, 0, 0);
            if AMediaFormat_getRect(format, c"crop".as_ptr(), &mut l, &mut t, &mut r, &mut b) {
                self.frame_width = r - l + 1;
                self.frame_height = b - t + 1;
            }

            // Both out of the bitstream's own description of itself. A stream
            // that says nothing is taken for what the encoders this has met
            // produce.
            let mut standard = 0;
            let mut range = 0;
            self.bt709 = !AMediaFormat_getInt32(format, c"color-standard".as_ptr(), &mut standard)
                || standard == COLOR_STANDARD_BT709
                || standard == 0;
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
                let plane = Plane {
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
                self.planes[i] = plane;
            }
            self.have_layout = true;
        }
    }

    /// The buffer's planes into `out`, and what moved in them into `changed` —
    /// which is also what decides how much of the buffer is converted at all.
    fn convert(
        &mut self,
        base: &[u8],
        out: &mut [u32],
        width: usize,
        height: usize,
        changed: &mut Changes,
    ) {
        let w = width.min(self.frame_width.max(0) as usize);
        let h = height.min(self.frame_height.max(0) as usize);
        if !self.have_layout || w == 0 || h == 0 {
            return;
        }
        // Every read below is inside the buffer if the last one of each plane
        // is, and a decoder that has just changed resolution is where that
        // matters.
        for plane in &self.planes {
            let last = plane.offset
                + ((h - 1) / plane.vert_subsampling) * plane.row_inc
                + ((w - 1) / plane.horiz_subsampling) * plane.col_inc;
            if last >= base.len() {
                log::warn!("a {w}x{h} frame does not fit {} bytes", base.len());
                return;
            }
        }

        let c = convert::coefficients(self.bt709, self.full_range);
        let layout = Layout {
            y_col_inc: self.planes[0].col_inc,
            uv_col_inc: self.planes[1].col_inc,
            horiz_subsampling: self.planes[1].horiz_subsampling,
        };
        let vectorise = match self.checked {
            Some((checked, ok)) if checked == layout => ok,
            _ => {
                let ok = convert::vectorised_agrees(
                    base,
                    self.planes[0].offset,
                    self.planes[1].offset,
                    self.planes[2].offset,
                    &layout,
                    w,
                    &c,
                );
                log::info!(
                    "the vectorised conversion is {} for this decoder's layout",
                    if ok { "in use" } else { "off" }
                );
                self.checked = Some((layout, ok));
                ok
            }
        };

        self.previous.compare(base, &self.planes, w, h, changed);

        for row in 0..h {
            if !changed.row(row) {
                continue;
            }
            let y_at = self.planes[0].offset + row * self.planes[0].row_inc;
            let u_at = self.planes[1].offset
                + (row / self.planes[1].vert_subsampling) * self.planes[1].row_inc;
            let v_at = self.planes[2].offset
                + (row / self.planes[2].vert_subsampling) * self.planes[2].row_inc;
            let line = &mut out[row * width..row * width + w];
            let mut col = 0;
            if vectorise {
                col = convert::row_vectorised(base, y_at, u_at, v_at, &layout, line, w, &c);
            }
            convert::row_scalar(base, y_at, u_at, v_at, &layout, line, col, w, &c);
        }
    }
}

impl super::Decoder for Decoder {
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
        let timeout = if self.saw_frame {
            FRAME_TIMEOUT_US
        } else {
            FIRST_FRAME_TIMEOUT_US
        };
        if !self.draw_next(out, width, height, timeout, changed) {
            return false;
        }
        self.saw_frame = true;
        true
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe {
            AMediaCodec_stop(self.codec);
            AMediaCodec_delete(self.codec);
        }
    }
}

fn read_i32(bytes: &[u8], at: usize) -> i32 {
    i32::from_ne_bytes(bytes[at..at + 4].try_into().unwrap_or([0; 4]))
}
