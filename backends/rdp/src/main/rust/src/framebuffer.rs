//! The desktop as pixels, and the one thing two threads share.
//!
//! Unlike the RFB client's, this one does not own the pixels: IronRDP's
//! [`DecodedImage`] is what its decoders write into, and taking a copy of it
//! after every update would double the cost of every frame for nothing. So the
//! image itself lives behind the `RwLock` and this is the reading half — the
//! same contract `rfb::Framebuffer` offers, so `nativeReadRegion` is the same
//! call on either backend.
//!
//! The format is [`PixelFormat::RgbA32`], which is `R, G, B, A` in memory —
//! already an Android `ARGB_8888` pixel, so a region read is a row copy with no
//! conversion anywhere in the path. That is the same decision, and the same
//! payoff, as `rfb::PixelFormat::NATIVE`.

use ironrdp_graphics::image_processing::PixelFormat;
use ironrdp_session::image::DecodedImage;

/// The format everything in this client works in. Not a preference: the whole
/// point is that nothing converts.
pub const FORMAT: PixelFormat = PixelFormat::RgbA32;

pub struct Framebuffer {
    image: Option<DecodedImage>,
}

impl Framebuffer {
    pub fn new() -> Framebuffer {
        Framebuffer { image: None }
    }

    pub fn width(&self) -> usize {
        self.image.as_ref().map_or(0, |i| usize::from(i.width()))
    }

    pub fn height(&self) -> usize {
        self.image.as_ref().map_or(0, |i| usize::from(i.height()))
    }

    /// Nothing is preserved. A desktop that changes size has been through a
    /// Deactivation-Reactivation Sequence, which repaints all of it.
    pub fn resize(&mut self, width: u16, height: u16) {
        self.image = Some(DecodedImage::new(FORMAT, width, height));
    }

    /// The image the decoders write into. Held under the write lock, which is
    /// why every caller takes it for one update and no longer.
    pub fn image_mut(&mut self) -> Option<&mut DecodedImage> {
        self.image.as_mut()
    }

    /// Copy a desktop rectangle out as `R, G, B, A` bytes, one row at a time
    /// into a destination of its own stride. False if there is nothing to read
    /// or the rectangle is not wholly inside the desktop — a partial answer
    /// would be a half-drawn tile on screen.
    pub fn read_rgba(
        &self,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        dst: &mut [u8],
        dst_stride: usize,
    ) -> bool {
        let Some(image) = self.image.as_ref() else {
            return false;
        };
        let (width, height) = (self.width(), self.height());
        if w == 0 || h == 0 || x + w > width || y + h > height {
            return false;
        }
        if dst_stride < w * 4 || dst.len() < (h - 1) * dst_stride + w * 4 {
            return false;
        }
        copy_rows(image.data(), image.stride(), x, y, w, h, dst, dst_stride);
        true
    }

    /// The whole desktop at `1/step`, nearest neighbour: the home screen's
    /// thumbnail, as `R, G, B, A` words.
    pub fn thumbnail(&self, step: usize) -> Option<(usize, usize, Vec<u32>)> {
        let image = self.image.as_ref()?;
        let (width, height) = (self.width(), self.height());
        if width == 0 || step == 0 {
            return None;
        }
        let stride = image.stride();
        let data = image.data();
        let tw = width.div_ceil(step);
        let th = height.div_ceil(step);
        let mut out = Vec::with_capacity(tw * th);
        for row in 0..th {
            let sy = (row * step).min(height - 1);
            for col in 0..tw {
                let sx = (col * step).min(width - 1);
                let at = sy * stride + sx * 4;
                out.push(u32::from_le_bytes([
                    data[at],
                    data[at + 1],
                    data[at + 2],
                    data[at + 3],
                ]));
            }
        }
        Some((tw, th, out))
    }
}

/// The row arithmetic, on its own so that a test can see it: `DecodedImage`
/// has no way in from outside the crate that owns it, and two strides that are
/// not the same one is exactly the thing worth checking.
fn copy_rows(
    data: &[u8],
    stride: usize,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    dst: &mut [u8],
    dst_stride: usize,
) {
    for row in 0..h {
        let from = (y + row) * stride + x * 4;
        let to = row * dst_stride;
        dst[to..to + w * 4].copy_from_slice(&data[from..from + w * 4]);
    }
}

impl Default for Framebuffer {
    fn default() -> Framebuffer {
        Framebuffer::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A read takes the source's stride and writes at the destination's, which
    /// are different numbers whenever an Android bitmap is padded.
    #[test]
    fn rows_land_at_the_destination_stride() {
        // 3×2 source, one byte per channel, values numbered by pixel.
        let mut data = vec![0u8; 3 * 2 * 4];
        for (i, px) in data.chunks_mut(4).enumerate() {
            px.copy_from_slice(&[i as u8, 0, 0, 0xff]);
        }
        let mut dst = vec![0u8; 2 * 12];
        copy_rows(&data, 3 * 4, 1, 0, 2, 2, &mut dst, 12);
        assert_eq!(dst[0], 1, "top-left of the region");
        assert_eq!(dst[4], 2);
        assert_eq!(dst[12], 4, "second row, at the destination's stride");
        assert_eq!(&dst[8..12], &[0, 0, 0, 0], "padding is left alone");
    }

    /// The destination offset `nativeReadRegion` takes is a slice offset and
    /// nothing else, which is what lets it promise that the rest of the
    /// destination is left as it was — the promise the caller's tiles depend on,
    /// since a tile is refreshed a piece at a time.
    #[test]
    fn a_read_into_the_middle_leaves_the_rest_alone() {
        // 2×2 source, pixels numbered 1..4 in the red channel.
        let mut data = vec![0u8; 2 * 2 * 4];
        for (i, px) in data.chunks_mut(4).enumerate() {
            px.copy_from_slice(&[i as u8 + 1, 0, 0, 0xff]);
        }
        const STRIDE: usize = 4 * 4;
        let mut dst = vec![0xeeu8; STRIDE * 4];
        let (dx, dy) = (1usize, 2usize);
        copy_rows(&data, 2 * 4, 0, 0, 2, 2, &mut dst[dy * STRIDE + dx * 4..], STRIDE);

        for y in 0..4 {
            for x in 0..4 {
                let expected = match (x, y) {
                    (1, 2) => 1,
                    (2, 2) => 2,
                    (1, 3) => 3,
                    (2, 3) => 4,
                    _ => 0xee,
                };
                assert_eq!(dst[y * STRIDE + x * 4], expected, "pixel {x},{y}");
            }
        }
    }

    #[test]
    fn read_outside_is_refused() {
        let mut fb = Framebuffer::new();
        fb.resize(4, 3);
        let mut out = [0u8; 64];
        assert!(!fb.read_rgba(3, 0, 2, 1, &mut out, 8));
        assert!(fb.read_rgba(0, 0, 2, 1, &mut out, 8));
        assert!(!Framebuffer::new().read_rgba(0, 0, 1, 1, &mut out, 4));
    }

    #[test]
    fn thumbnail_halves() {
        let mut fb = Framebuffer::new();
        fb.resize(4, 4);
        let (w, h, px) = fb.thumbnail(2).unwrap();
        assert_eq!((w, h), (2, 2));
        assert_eq!(px.len(), 4);
        assert!(Framebuffer::new().thumbnail(2).is_none());
    }
}
