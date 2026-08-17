// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The desktop as pixels, and the one thing two threads share.

/// A framebuffer in [`crate::PixelFormat::NATIVE`]: one `u32` per pixel, laid
/// out in memory as `R, G, B, A` so a read is a row copy into an Android
/// `ARGB_8888` bitmap.
///
/// Written by the protocol thread under a write lock and read by the drawing
/// thread under a read lock — which is why every decoder assembles a rectangle
/// in scratch space first and holds the lock only for the blit. A 1920×1200 raw
/// update is nine megabytes off a socket, and stalling the drawing thread for
/// as long as that takes to arrive would be a frame or two of nothing.
pub struct Framebuffer {
    width: usize,
    height: usize,
    pixels: Vec<u32>,
}

impl Framebuffer {
    pub fn new() -> Framebuffer {
        Framebuffer {
            width: 0,
            height: 0,
            pixels: Vec::new(),
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    /// Nothing is preserved: a desktop that changes size has repainted anyway,
    /// and the server sends the whole thing again.
    pub fn resize(&mut self, width: usize, height: usize) {
        self.width = width;
        self.height = height;
        self.pixels.clear();
        self.pixels.resize(width * height, 0xff00_0000);
    }

    /// `src` is `w × h` pixels, row-major, and is clipped to the framebuffer.
    pub fn blit(&mut self, x: usize, y: usize, w: usize, h: usize, src: &[u32]) {
        if src.len() < w * h {
            return;
        }
        let Some((x, y, cw, ch)) = self.clip(x, y, w, h) else {
            return;
        };
        for row in 0..ch {
            // The source's stride is the rectangle's own width, not the clipped
            // one: clipping narrows what is copied out of each row and does not
            // move where the next row starts.
            let from = row * w;
            let to = (y + row) * self.width + x;
            self.pixels[to..to + cw].copy_from_slice(&src[from..from + cw]);
        }
    }

    /// `CopyRect`: the one encoding that reads the framebuffer as well as
    /// writing it, so it is the one that cannot be done in scratch space.
    /// Overlapping rectangles are the normal case (a window drag), so the row
    /// order follows the direction of the move.
    pub fn copy(&mut self, src_x: usize, src_y: usize, dst_x: usize, dst_y: usize,
                w: usize, h: usize) {
        let w = w.min(self.width.saturating_sub(src_x)).min(self.width.saturating_sub(dst_x));
        let h = h.min(self.height.saturating_sub(src_y)).min(self.height.saturating_sub(dst_y));
        if w == 0 || h == 0 {
            return;
        }
        let stride = self.width;
        let rows: Box<dyn Iterator<Item = usize>> = if dst_y > src_y {
            Box::new((0..h).rev())
        } else {
            Box::new(0..h)
        };
        for row in rows {
            let from = (src_y + row) * stride + src_x;
            let to = (dst_y + row) * stride + dst_x;
            self.pixels.copy_within(from..from + w, to);
        }
    }

    /// Copy a desktop rectangle out as `R, G, B, A` bytes, one row at a time
    /// into a destination of its own stride. False if there is nothing to read
    /// or the rectangle is not wholly inside the desktop — a partial answer
    /// would be a half-drawn tile on screen.
    pub fn read_rgba(&self, x: usize, y: usize, w: usize, h: usize,
                     dst: &mut [u8], dst_stride: usize) -> bool {
        if self.width == 0 || w == 0 || h == 0 {
            return false;
        }
        if x + w > self.width || y + h > self.height {
            return false;
        }
        if dst_stride < w * 4 || dst.len() < (h - 1) * dst_stride + w * 4 {
            return false;
        }
        for row in 0..h {
            let from = (y + row) * self.width + x;
            let src = &self.pixels[from..from + w];
            let to = row * dst_stride;
            // `u32` is `R, G, B, A` in memory by construction (PixelFormat::NATIVE),
            // so this is a straight reinterpretation and not a conversion.
            let (_, bytes, _) = unsafe { src.align_to::<u8>() };
            dst[to..to + w * 4].copy_from_slice(bytes);
        }
        true
    }

    /// The whole desktop at `1/step`, nearest neighbour: the home screen's
    /// thumbnail. Unlike RealVNC's scaler the step need not be a power of two —
    /// that constraint was a consequence of `(int)(1/scale)` in their library,
    /// and there is no float here at all.
    pub fn thumbnail(&self, step: usize) -> Option<(usize, usize, Vec<u32>)> {
        if self.width == 0 || step == 0 {
            return None;
        }
        let tw = self.width.div_ceil(step);
        let th = self.height.div_ceil(step);
        let mut out = Vec::with_capacity(tw * th);
        for row in 0..th {
            let sy = (row * step).min(self.height - 1);
            for col in 0..tw {
                let sx = (col * step).min(self.width - 1);
                out.push(self.pixels[sy * self.width + sx]);
            }
        }
        Some((tw, th, out))
    }

    fn clip(&self, x: usize, y: usize, w: usize, h: usize) -> Option<(usize, usize, usize, usize)> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let w = w.min(self.width - x);
        let h = h.min(self.height - y);
        if w == 0 || h == 0 { None } else { Some((x, y, w, h)) }
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

    #[test]
    fn blit_and_read() {
        let mut fb = Framebuffer::new();
        fb.resize(4, 3);
        fb.blit(1, 1, 2, 2, &[1, 2, 3, 4]);
        let mut out = [0u8; 2 * 2 * 4];
        assert!(fb.read_rgba(1, 1, 2, 2, &mut out, 8));
        assert_eq!(u32::from_le_bytes(out[0..4].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(out[12..16].try_into().unwrap()), 4);
    }

    /// A rectangle that hangs over the right edge is narrowed, and each row
    /// still comes from the source's own stride. Only a server that does not
    /// clip to what was asked for gets here, which is why it went unnoticed:
    /// what it produces is a skewed picture rather than a crash.
    #[test]
    fn a_clipped_blit_keeps_the_source_stride() {
        let mut fb = Framebuffer::new();
        fb.resize(4, 2);
        fb.blit(2, 0, 4, 2, &[1, 2, 3, 4, 5, 6, 7, 8]);
        let mut out = [0u8; 2 * 2 * 4];
        assert!(fb.read_rgba(2, 0, 2, 2, &mut out, 8));
        let at = |i: usize| u32::from_le_bytes(out[i * 4..][..4].try_into().unwrap());
        assert_eq!([at(0), at(1)], [1, 2]);
        assert_eq!([at(2), at(3)], [5, 6]);
    }

    #[test]
    fn read_outside_is_refused() {
        let mut fb = Framebuffer::new();
        fb.resize(4, 3);
        let mut out = [0u8; 64];
        assert!(!fb.read_rgba(3, 0, 2, 1, &mut out, 8));
    }

    /// The destination offset `nativeReadRegion` takes is a slice offset and
    /// nothing else, which is what lets it promise that the rest of the
    /// destination is left as it was — the promise the caller's tiles depend on,
    /// since a tile is refreshed a piece at a time.
    #[test]
    fn a_read_into_the_middle_leaves_the_rest_alone() {
        let mut fb = Framebuffer::new();
        fb.resize(4, 4);
        fb.blit(1, 1, 2, 2, &[1, 2, 3, 4]);

        const STRIDE: usize = 4 * 4;
        let mut dst = vec![0xeeu8; STRIDE * 4];
        let (dx, dy) = (1usize, 2usize);
        assert!(fb.read_rgba(1, 1, 2, 2, &mut dst[dy * STRIDE + dx * 4..], STRIDE));

        let at = |x: usize, y: usize| {
            u32::from_le_bytes(dst[y * STRIDE + x * 4..][..4].try_into().unwrap())
        };
        for y in 0..4 {
            for x in 0..4 {
                let expected = match (x, y) {
                    (1, 2) => 1,
                    (2, 2) => 2,
                    (1, 3) => 3,
                    (2, 3) => 4,
                    _ => 0xeeee_eeee,
                };
                assert_eq!(at(x, y), expected, "pixel {x},{y}");
            }
        }
    }

    /// A downward move must copy from the bottom up, or every row is the first
    /// one — the classic overlapping-`memcpy` bug, and the one a window drag
    /// hits every time.
    #[test]
    fn overlapping_copy_downwards() {
        let mut fb = Framebuffer::new();
        fb.resize(1, 4);
        fb.blit(0, 0, 1, 4, &[10, 20, 30, 40]);
        fb.copy(0, 0, 0, 1, 1, 3);
        let mut out = [0u8; 4 * 4];
        assert!(fb.read_rgba(0, 0, 1, 4, &mut out, 4));
        let px: Vec<u32> = out
            .chunks(4)
            .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
            .collect();
        assert_eq!(px, vec![10, 10, 20, 30]);
    }

    #[test]
    fn thumbnail_halves() {
        let mut fb = Framebuffer::new();
        fb.resize(4, 4);
        fb.blit(0, 0, 4, 1, &[1, 2, 3, 4]);
        let (w, h, px) = fb.thumbnail(2).unwrap();
        assert_eq!((w, h), (2, 2));
        assert_eq!(&px[0..2], &[1, 3]);
    }
}
