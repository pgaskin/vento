// SPDX-FileCopyrightText: 2026 Nick Haghiri
// SPDX-License-Identifier: GPL-3.0-or-later

//! H.264 decoded by the phone's own hardware, through MediaCodec.
//!
//! Frames go in as Annex B, which both RDP's and SPICE's H.264 are. The decoder's
//! YUV is packed as NV12 and handed back as [`VideoPlanes`] for the display to
//! convert on the GPU, so nothing here converts pixels.

use std::mem::MaybeUninit;
use std::time::Duration;

use ndk::media::media_codec::{
    DequeuedInputBufferResult, DequeuedOutputBufferInfoResult, MediaCodec, MediaCodecDirection,
};
use ndk::media::media_format::MediaFormat;

use crate::video::VideoPlanes;

/// MediaCodec's own colour formats, by the numbers it reports them as.
const COLOR_FORMAT_YUV420_PLANAR: i32 = 19; // I420: Y, then U, then V
const COLOR_FORMAT_YUV420_SEMI_PLANAR: i32 = 21; // NV12: Y, then interleaved UV

/// How long one attempt waits on the codec, and how many attempts a frame gets.
/// A frame that has not come out after this long is dropped, and the next one
/// that does come out is the one drawn.
const WAIT: Duration = Duration::from_millis(10);
const ATTEMPTS: u32 = 20;

/// The layout of the decoder's output, as it last reported it.
#[derive(Clone, Copy, Debug, Default)]
struct Layout {
    width: usize,
    height: usize,
    stride: usize,
    slice_height: usize,
    color_format: i32,
    color_standard: i32,
    /// Whether the planes are full range: as the codec says, or as the default says when it does not.
    full_range: bool,
}

/// A codec, which is not `Send` only because it holds a raw pointer. It is made on
/// one thread, moved to the thread that decodes, and used from that thread alone,
/// so the pointer is never touched from two threads at once.
struct SingleThreaded(MediaCodec);

// SAFETY: see the type's comment; nothing ever uses this codec from two threads.
unsafe impl Send for SingleThreaded {}

impl std::ops::Deref for SingleThreaded {
    type Target = MediaCodec;

    fn deref(&self) -> &MediaCodec {
        &self.0
    }
}

pub struct Decoder {
    codec: SingleThreaded,
    layout: Layout,
    timestamp_us: i64,
    /// The range to assume when the codec does not say which it is.
    default_full_range: bool,
}

impl Decoder {
    /// A decoder for the phone's AVC codec. `default_full_range` is the range to
    /// assume when the codec does not say which one its output is in.
    pub fn new(default_full_range: bool) -> Result<Self, String> {
        let codec = MediaCodec::from_decoder_type("video/avc")
            .ok_or_else(|| "no AVC decoder on this device".to_string())?;
        let mut format = MediaFormat::new();
        format.set_str("mime", "video/avc");
        // The real size comes from the stream, and the codec reports it once it
        // has one; these only size its input buffers.
        format.set_i32("width", 1920);
        format.set_i32("height", 1200);
        format.set_i32("max-input-size", 2 * 1024 * 1024);
        // Low latency, or the codec holds frames back to reorder them (three, on the
        // device this was written on), and this decoder has to answer each frame
        // with its own picture before the next one is sent.
        format.set_i32("low-latency", 1);
        codec
            .configure(&format, None, MediaCodecDirection::Decoder)
            .map_err(|e| format!("configuring the AVC decoder: {e:?}"))?;
        codec
            .start()
            .map_err(|e| format!("starting the AVC decoder: {e:?}"))?;
        Ok(Self {
            codec: SingleThreaded(codec),
            layout: Layout::default(),
            timestamp_us: 0,
            default_full_range,
        })
    }

    /// Drops whatever the codec holds. The codec keeps no picture the next frame
    /// would depend on once it has been flushed, and a new stream starts at an
    /// IDR, so flushing is enough.
    pub fn flush(&mut self) -> Result<(), String> {
        self.codec
            .flush()
            .map_err(|e| format!("flushing the AVC decoder: {e:?}"))
    }

    /// One Annex B frame in, its picture out. `region` is the top-left part of the
    /// picture that will be drawn, or the whole of it.
    pub fn decode(&mut self, annex_b: &[u8], region: Option<(usize, usize)>) -> Result<VideoPlanes, String> {
        self.queue(annex_b)?;

        for _ in 0..ATTEMPTS {
            match self
                .codec
                .dequeue_output_buffer(WAIT)
                .map_err(|e| format!("reading the AVC decoder: {e:?}"))?
            {
                DequeuedOutputBufferInfoResult::Buffer(buffer) => {
                    let info = *buffer.info();
                    let start = info.offset().max(0) as usize;
                    let end = start + info.size().max(0) as usize;
                    let packed = {
                        let bytes = buffer.buffer();
                        if end > bytes.len() {
                            return Err("decoded frame past the end of its buffer".into());
                        }
                        pack_nv12(&bytes[start..end], self.layout, region)?
                    };
                    self.codec
                        .release_output_buffer(buffer, false)
                        .map_err(|e| format!("releasing a decoded frame: {e:?}"))?;
                    return Ok(packed);
                }
                DequeuedOutputBufferInfoResult::OutputFormatChanged => self.read_format_change(),
                DequeuedOutputBufferInfoResult::OutputBuffersChanged
                | DequeuedOutputBufferInfoResult::TryAgainLater => {}
            }
        }
        Err("the AVC decoder produced no frame for this one".into())
    }

    fn read_format_change(&mut self) {
        let format = self.codec.output_format();
        self.layout = Layout {
            width: format.i32("width").unwrap_or(0).max(0) as usize,
            height: format.i32("height").unwrap_or(0).max(0) as usize,
            stride: format.i32("stride").unwrap_or(0).max(0) as usize,
            slice_height: format.i32("slice-height").unwrap_or(0).max(0) as usize,
            color_format: format.i32("color-format").unwrap_or(0),
            color_standard: format.i32("color-standard").unwrap_or(0),
            full_range: false,
        };
        // MediaCodec's colour range: 1 is full, 2 is limited, and anything else is
        // unreported, which takes the default.
        self.layout.full_range = match format.i32("color-range").unwrap_or(0) {
            1 => true,
            2 => false,
            _ => self.default_full_range,
        };
    }

    /// Hands one frame to the codec, waiting for an input buffer to be free.
    fn queue(&mut self, annex_b: &[u8]) -> Result<(), String> {
        for _ in 0..ATTEMPTS {
            match self
                .codec
                .dequeue_input_buffer(WAIT)
                .map_err(|e| format!("feeding the AVC decoder: {e:?}"))?
            {
                DequeuedInputBufferResult::Buffer(mut buffer) => {
                    let dst: &mut [MaybeUninit<u8>] = buffer.buffer_mut();
                    if annex_b.len() > dst.len() {
                        return Err("a frame is larger than the decoder's input".into());
                    }
                    for (slot, byte) in dst.iter_mut().zip(annex_b) {
                        slot.write(*byte);
                    }
                    self.timestamp_us += 33_333;
                    self.codec
                        .queue_input_buffer(buffer, 0, annex_b.len(), self.timestamp_us as u64, 0)
                        .map_err(|e| format!("queueing a frame: {e:?}"))?;
                    return Ok(());
                }
                DequeuedInputBufferResult::TryAgainLater => {}
            }
        }
        Err("the AVC decoder has no room for a frame".into())
    }
}

/// The decoder's output as packed NV12, the top-left region only.
fn pack_nv12(yuv: &[u8], layout: Layout, region: Option<(usize, usize)>) -> Result<VideoPlanes, String> {
    if layout.width == 0 || layout.height == 0 {
        return Err("decoded frame has no size yet".into());
    }
    let (w, h) = match region {
        Some((rw, rh)) => (rw.min(layout.width), rh.min(layout.height)),
        None => (layout.width, layout.height),
    };
    let stride = if layout.stride == 0 { layout.width } else { layout.stride };
    let slice = if layout.slice_height == 0 { layout.height } else { layout.slice_height };
    let y_size = stride * slice;
    let chroma_stride = stride / 2;
    let chroma_rows = slice / 2;
    if yuv.len() < y_size + 2 * chroma_stride * chroma_rows {
        return Err("decoded frame is smaller than its layout".into());
    }
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let mut data = Vec::with_capacity(w * h + 2 * cw * ch);
    for row in 0..h {
        data.extend_from_slice(&yuv[row * stride..row * stride + w]);
    }
    match layout.color_format {
        COLOR_FORMAT_YUV420_SEMI_PLANAR => {
            for row in 0..ch {
                let at = y_size + row * stride;
                data.extend_from_slice(&yuv[at..at + 2 * cw]);
            }
        }
        COLOR_FORMAT_YUV420_PLANAR => {
            let v_base = y_size + chroma_stride * chroma_rows;
            for row in 0..ch {
                for x in 0..cw {
                    data.push(yuv[y_size + row * chroma_stride + x]);
                    data.push(yuv[v_base + row * chroma_stride + x]);
                }
            }
        }
        other => return Err(format!("unsupported decoder colour format {other}")),
    }
    // MediaCodec's colour standard: 1 is BT.709, and anything unreported is assumed
    // to be BT.709 for HD. The range was settled when the format was read.
    let bt709 = match layout.color_standard {
        1 => true,
        2 | 4 | 6 => false,
        _ => h >= 720,
    };
    let color_mode = i32::from(bt709) | (i32::from(layout.full_range) << 1);
    Ok(VideoPlanes { width: w, height: h, data, color_mode })
}
