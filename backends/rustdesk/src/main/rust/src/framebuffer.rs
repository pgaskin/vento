//! The desktop as pixels: the picture two threads share, and the buffer the
//! next one is decoded into.

use crate::video::Changes;

/// A framebuffer of `R, G, B, A` words, so a read is a row copy into an Android
/// `ARGB_8888` bitmap.
///
/// Written by the protocol thread under a write lock and read by the drawing
/// thread under a read lock, and the write is a **pointer swap**: this protocol
/// has no damage rectangles, so a key frame is a whole 1920×1200 picture and
/// converting one into place under the lock would stop the drawing thread for
/// the length of that conversion — a third of a frame's time at 30 fps. The
/// decoder converts into a buffer of its own and [`swap`](Self::swap) puts it
/// in, which holds the lock for a `Vec` swap. What that buffer costs when a
/// frame converts only part of itself is [`Back`].
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

    /// Nothing is preserved: a desktop that has changed size is repainted from
    /// a key frame, which is the only kind of update this protocol has.
    pub fn resize(&mut self, width: usize, height: usize) {
        self.width = width;
        self.height = height;
        self.pixels.clear();
        self.pixels.resize(width * height, 0xff00_0000);
    }

    /// Take `next` as the picture, handing back the buffer that was there for
    /// the decoder to draw the following frame into.
    ///
    /// A size that does not match is refused rather than resized: the caller
    /// resizes when the far end says the desktop changed, and a frame that
    /// arrives between the two is one frame of the old size.
    pub fn swap(&mut self, next: &mut Vec<u32>) -> bool {
        if next.len() != self.width * self.height || self.width == 0 {
            return false;
        }
        std::mem::swap(&mut self.pixels, next);
        true
    }

    /// Copy the rows `behind` names out of the picture here into `dst`, which
    /// is the buffer the next [`swap`](Self::swap) will take. False, and
    /// nothing copied, where `dst` is not this desktop's size.
    ///
    /// This is what a frame that converts only part of itself owes the buffer
    /// it converts into: see [`Back`].
    pub fn carry_rows(&self, dst: &mut [u32], behind: impl Fn(usize) -> bool) -> bool {
        if self.width == 0 || dst.len() != self.width * self.height {
            return false;
        }
        for row in 0..self.height {
            if !behind(row) {
                continue;
            }
            let at = row * self.width;
            dst[at..at + self.width].copy_from_slice(&self.pixels[at..at + self.width]);
        }
        true
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
            // The word order is the bitmap's by construction, so this is a
            // reinterpretation rather than a conversion.
            let (_, bytes, _) = unsafe { src.align_to::<u8>() };
            dst[to..to + w * 4].copy_from_slice(bytes);
        }
        true
    }

    /// The whole desktop at `1/step`, nearest neighbour: the home screen's
    /// thumbnail.
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
}

impl Default for Framebuffer {
    fn default() -> Framebuffer {
        Framebuffer::new()
    }
}

/// The buffer a frame is decoded into, and what the last two frames wrote to
/// it.
///
/// [`Framebuffer::swap`] takes this buffer and hands back the one that was on
/// screen, so a buffer that has just been swapped in holds the frame **before**
/// last. That costs nothing while every frame is converted whole, and is the
/// whole of the difficulty once a frame converts only the rows that moved: a
/// row this frame does not touch is two pictures behind rather than one.
///
/// So the rows the *last* frame converted are brought forward out of the
/// picture now on screen, before the swap takes it away — every other row is
/// one the last two frames both left alone, and is therefore already right.
pub struct Back {
    pub pixels: Vec<u32>,
    /// What this frame converted, and what the last one did.
    pub changed: Changes,
    carried: Changes,
    /// Whether these pixels are related to the picture on screen at all. Set
    /// where a frame did not go on screen, and answered by converting the whole
    /// of the next one, since a buffer nothing is known about cannot be
    /// repaired a row at a time.
    stale: bool,
    /// Rows the last frame that went on screen was worth, which is what a frame
    /// costs and the one thing a session's own log cannot work out.
    converted: usize,
}

impl Back {
    pub fn new() -> Back {
        Back {
            pixels: Vec::new(),
            changed: Changes::new(),
            carried: Changes::new(),
            stale: true,
            converted: 0,
        }
    }

    /// Nothing is preserved, and the next frame is converted whole.
    pub fn resize(&mut self, width: usize, height: usize) {
        self.pixels.clear();
        self.pixels.resize(width * height, 0xff00_0000);
        self.stale = true;
    }

    /// Start an update: nothing has moved in it yet, unless these pixels have
    /// lost track of the picture on screen, in which case all of it has.
    pub fn begin(&mut self, width: usize, height: usize) {
        self.changed.reset(height);
        self.converted = 0;
        if self.stale {
            self.changed.all(width, height);
        }
    }

    /// Bring the rows this frame did not convert up to the picture on screen,
    /// which is every row the last frame converted and this one did not. False
    /// where that picture is not this buffer's size, and the buffer is then
    /// stale rather than half right.
    pub fn carry(&mut self, from: &Framebuffer) -> bool {
        let Back {
            pixels,
            changed,
            carried,
            stale,
            ..
        } = self;
        if !from.carry_rows(pixels, |row| carried.row(row) && !changed.row(row)) {
            *stale = true;
            return false;
        }
        true
    }

    /// The picture went on screen: what this frame converted is what the next
    /// one will have to bring forward.
    pub fn swapped(&mut self) {
        self.converted = self.changed.count();
        std::mem::swap(&mut self.changed, &mut self.carried);
        self.stale = false;
    }

    /// How many rows the last frame put on screen, and none for a frame that
    /// did not.
    pub fn converted(&self) -> usize {
        self.converted
    }

    /// The picture did not, so the next frame is converted whole.
    pub fn lost(&mut self) {
        self.stale = true;
    }
}

impl Default for Back {
    fn default() -> Back {
        Back::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_swapped_frame_reads_back() {
        let mut fb = Framebuffer::new();
        fb.resize(2, 2);
        let mut next = vec![1u32, 2, 3, 4];
        assert!(fb.swap(&mut next));
        // The old buffer comes back, which is what makes the next frame free.
        assert_eq!(next.len(), 4);
        let mut out = [0u8; 16];
        assert!(fb.read_rgba(0, 0, 2, 2, &mut out, 8));
        assert_eq!(u32::from_le_bytes(out[0..4].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(out[12..16].try_into().unwrap()), 4);
    }

    #[test]
    fn a_frame_of_the_wrong_size_is_refused() {
        let mut fb = Framebuffer::new();
        fb.resize(2, 2);
        let mut next = vec![0u32; 3];
        assert!(!fb.swap(&mut next));
    }

    const W: usize = 4;
    const H: usize = 4;

    /// One update, with a picture of one colour a row: the rows `moved` names
    /// are converted and the rest are left, which is what a decoder does.
    fn frame(fb: &mut Framebuffer, back: &mut Back, picture: &[u32; H], moved: &[usize]) {
        back.begin(W, H);
        for &row in moved {
            back.changed.mark(row, 0, W);
        }
        for (row, colour) in picture.iter().enumerate() {
            if back.changed.row(row) {
                back.pixels[row * W..row * W + W].fill(*colour);
            }
        }
        assert!(back.carry(fb));
        assert!(fb.swap(&mut back.pixels));
        back.swapped();
    }

    fn on_screen(fb: &Framebuffer) -> [u32; H] {
        std::array::from_fn(|row| fb.pixels[row * W])
    }

    /// The one thing converting part of a frame has to get right: the buffer
    /// that comes back from a swap is the frame *before* last, so a row the new
    /// frame does not convert has to be brought forward from the picture that
    /// has just gone on screen.
    #[test]
    fn a_row_that_is_not_converted_comes_forward() {
        let mut fb = Framebuffer::new();
        fb.resize(W, H);
        let mut back = Back::new();
        back.resize(W, H);

        let mut picture = [1u32; H];
        frame(&mut fb, &mut back, &picture, &[]); // the first one is all of it
        assert_eq!(on_screen(&fb), [1, 1, 1, 1]);

        picture[1] = 2;
        frame(&mut fb, &mut back, &picture, &[1]);
        assert_eq!(on_screen(&fb), [1, 2, 1, 1]);

        // Row 1 is the one at stake: it moved last frame and not this one, so
        // the buffer being drawn into still holds what it was before that.
        picture[2] = 3;
        frame(&mut fb, &mut back, &picture, &[2]);
        assert_eq!(on_screen(&fb), [1, 2, 3, 1]);
    }

    #[test]
    fn a_frame_that_did_not_go_on_screen_converts_the_next_one_whole() {
        let mut back = Back::new();
        back.resize(W, H);
        back.begin(W, H);
        back.swapped();

        back.lost();
        back.begin(W, H);
        assert!((0..H).all(|row| back.changed.row(row)));
    }

    #[test]
    fn read_outside_is_refused() {
        let mut fb = Framebuffer::new();
        fb.resize(4, 3);
        let mut out = [0u8; 64];
        assert!(!fb.read_rgba(3, 0, 2, 1, &mut out, 8));
    }

    #[test]
    fn thumbnail_halves() {
        let mut fb = Framebuffer::new();
        fb.resize(4, 4);
        let mut next = vec![0u32; 16];
        next[0..4].copy_from_slice(&[1, 2, 3, 4]);
        assert!(fb.swap(&mut next));
        let (w, h, px) = fb.thumbnail(2).unwrap();
        assert_eq!((w, h), (2, 2));
        assert_eq!(&px[0..2], &[1, 3]);
    }
}
