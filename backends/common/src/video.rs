// SPDX-FileCopyrightText: 2026 Nick Haghiri
// SPDX-License-Identifier: GPL-3.0-or-later

//! A decoded video frame, as the display takes it: NV12 planes, not pixels, so
//! the colour conversion happens on the GPU. Any protocol that decodes H.264 on
//! the phone hands its frames over in these.

/// One decoded frame, as NV12 for the display to convert on the GPU.
pub struct VideoPlanes {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
    /// Bit 0: BT.709 rather than BT.601. Bit 1: full range rather than limited.
    pub color_mode: i32,
}

/// Where a decoder leaves the frame it has just produced, for the caller to take.
pub type PlanesSlot = std::sync::Arc<std::sync::Mutex<Option<VideoPlanes>>>;
