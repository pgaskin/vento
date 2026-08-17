// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The picture: what the far end encoded, decoded by the phone.
//!
//! Every frame in this protocol is a whole frame — there are no damage
//! rectangles anywhere in it — so what a session costs is one decode and one
//! conversion per update, and the decode is the phone's own hardware. That is
//! what keeps this backend from being a codec project: nothing here implements
//! VP9, and the NDK's MediaCodec is a C API, so a Rust module reaches it with
//! an `extern "C"` block and no JNI, no Java and no thread of its own.
//!
//! Off Android there is no decoder at all and [`decoder`] says so. The protocol
//! above it is host-testable; the picture is not, and pretending otherwise
//! would mean carrying a software VP9 decoder for the sake of a test.

// Off Android nothing in either is called — the decoder that calls them is the
// phone's — and the tests that check the arithmetic are the reason they are
// compiled on the host at all.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod convert;
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub mod diff;

#[cfg(target_os = "android")]
mod mediacodec;

pub use diff::Changes;

/// Which of their codecs a frame arrived in. The client asks for VP9 and their
/// server obeys unconditionally, but the arm a `VideoFrame` carries is the far
/// end's decision and this follows it rather than assuming.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    Vp9,
    Vp8,
    Av1,
    H264,
    H265,
}

#[cfg_attr(not(target_os = "android"), allow(dead_code))]
impl Codec {
    /// Every arm a `VideoFrame` can carry, which is what the far end is told
    /// this end can take.
    pub const ALL: [Codec; 5] = [
        Codec::Vp9,
        Codec::Vp8,
        Codec::Av1,
        Codec::H264,
        Codec::H265,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Codec::Vp9 => "VP9",
            Codec::Vp8 => "VP8",
            Codec::Av1 => "AV1",
            Codec::H264 => "H.264",
            Codec::H265 => "H.265",
        }
    }

    fn mime(self) -> &'static str {
        match self {
            Codec::Vp9 => "video/x-vnd.on2.vp9",
            Codec::Vp8 => "video/x-vnd.on2.vp8",
            Codec::Av1 => "video/av01",
            Codec::H264 => "video/avc",
            Codec::H265 => "video/hevc",
        }
    }
}

/// One decoder, for one codec at one size.
///
/// A frame goes in and what moved in it comes out into `out`, which is the
/// caller's back buffer rather than the framebuffer: every frame in this
/// protocol is a whole one, so the picture is swapped in afterwards rather than
/// converted in place under a lock the drawing thread wants.
pub trait Decoder {
    /// True when a frame arrived, with the rows and columns it changed added to
    /// `changed`. `out` is `width * height` words, row-major, `R, G, B, A` in
    /// memory, and **only the rows `changed` names are written**: the rest are
    /// left as they were, since converting a row nothing moved in is the bulk
    /// of what a frame costs here.
    ///
    /// Rows already marked in `changed` on the way in are converted whether
    /// they moved or not, which is how a caller whose buffer is out of step
    /// with the picture on screen asks for a whole frame.
    fn decode(
        &mut self,
        data: &[u8],
        out: &mut [u32],
        width: usize,
        height: usize,
        changed: &mut Changes,
    ) -> bool;
}

/// Whether this device can decode this codec at all.
///
/// Asked once and remembered, because the answer is the device's and cannot
/// change under a running app, and asked at all because it is what the far end
/// is told before it picks one: a peer told this end takes H.265 and sending it
/// to a phone with no HEVC decoder is a session with a login, a cursor and no
/// picture.
pub fn decodable(codec: Codec) -> bool {
    #[cfg(target_os = "android")]
    {
        use std::sync::OnceLock;
        static PROBED: OnceLock<[bool; Codec::ALL.len()]> = OnceLock::new();
        let probed = PROBED.get_or_init(|| Codec::ALL.map(mediacodec::can_decode));
        probed[Codec::ALL.iter().position(|c| *c == codec).unwrap()]
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = codec;
        false
    }
}

/// A decoder for this codec at this size, or `None` where the device has none —
/// which is a session with no picture and is reported as such rather than
/// retried.
pub fn decoder(codec: Codec, width: usize, height: usize) -> Option<Box<dyn Decoder>> {
    #[cfg(target_os = "android")]
    {
        mediacodec::Decoder::new(codec, width, height).map(|d| Box::new(d) as Box<dyn Decoder>)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (codec, width, height);
        None
    }
}
