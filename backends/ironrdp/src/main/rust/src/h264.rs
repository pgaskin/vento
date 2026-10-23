// SPDX-FileCopyrightText: 2026 Nick Haghiri
// SPDX-License-Identifier: GPL-3.0-or-later

//! The graphics pipeline's H.264 decoder: the phone's own, through the shared
//! MediaCodec decoder. Its frames go to the display as planes, so what this hands
//! back to the pipeline is a size and nothing to draw.

use common::android::h264::Decoder;
use ironrdp_egfx::decode::{DecodedFrame, DecoderError, DecoderResult, H264Decoder};

use crate::client::PlanesSlot;

pub struct MediaCodecH264 {
    decoder: Decoder,
    /// Where each decoded frame goes, for the display to draw.
    planes: PlanesSlot,
}

impl MediaCodecH264 {
    pub fn new(planes: PlanesSlot, default_full_range: bool) -> DecoderResult<Self> {
        let decoder = Decoder::new(default_full_range).map_err(DecoderError::msg)?;
        Ok(Self { decoder, planes })
    }

    /// One frame in, its planes into the slot. `region` is the top-left part of
    /// the picture that will be drawn, or the whole of it.
    fn decode_frame(&mut self, data: &[u8], region: Option<(usize, usize)>) -> DecoderResult<DecodedFrame> {
        let planes = self.decoder.decode(data, region).map_err(DecoderError::msg)?;
        let (w, h) = (planes.width, planes.height);
        *self.planes.lock().unwrap() = Some(planes);
        // No pixels: the planes are in the slot, and the size is all the caller needs.
        Ok(DecodedFrame::new(Vec::new(), w as u32, h as u32))
    }
}

impl H264Decoder for MediaCodecH264 {
    fn decode(&mut self, data: &[u8]) -> DecoderResult<DecodedFrame> {
        self.decode_frame(data, None)
    }

    fn decode_region(&mut self, data: &[u8], width: u32, height: u32) -> DecoderResult<DecodedFrame> {
        self.decode_frame(data, Some((width as usize, height as usize)))
    }

    fn reset(&mut self) {
        if let Err(e) = self.decoder.flush() {
            log::warn!("{e}");
        }
    }
}
