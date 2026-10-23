// SPDX-FileCopyrightText: 2026 Nick Haghiri
// SPDX-License-Identifier: GPL-3.0-or-later

//! SPICE's H.264 video streams, decoded by the phone through the shared MediaCodec
//! decoder. The frames go to the display as planes, so the decoder leaves them in
//! a slot and returns a size with nothing to draw.

use common::android::h264::Decoder;
use common::video::PlanesSlot;
use shakenfist_spice_compression::video::{DecodedFrame, VideoDecoder, VideoDecoderError};

pub struct MediaCodecVideo {
    decoder: Decoder,
    planes: PlanesSlot,
}

impl MediaCodecVideo {
    pub fn new(planes: PlanesSlot) -> Result<Self, VideoDecoderError> {
        // SPICE does not say which range its video is in, and what its encoders
        // produce is limited range, so that is the default.
        let decoder = Decoder::new(false).map_err(VideoDecoderError::Decode)?;
        Ok(Self { decoder, planes })
    }
}

impl VideoDecoder for MediaCodecVideo {
    fn decode(&mut self, packet: &[u8]) -> Result<Option<DecodedFrame>, VideoDecoderError> {
        let planes = self.decoder.decode(packet, None).map_err(VideoDecoderError::Decode)?;
        let (width, height) = (planes.width as u32, planes.height as u32);
        *self.planes.lock().unwrap() = Some(planes);
        Ok(Some(DecodedFrame { rgba: Vec::new(), width, height }))
    }

    fn name(&self) -> &'static str {
        "H.264 (MediaCodec)"
    }
}
