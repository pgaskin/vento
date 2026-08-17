//! What one frame differs from the last one in, found in the decoder's planes.
//!
//! This protocol has no damage rectangles, so what moved can only be found by
//! comparing — and the planes are a third of the bytes the picture is, are what
//! the decoder has just written, and are still in this thread's cache. The
//! answer is used twice over: it is the damage the drawing thread is told
//! about, and it is which rows are worth converting at all.
//!
//! Two identical planes cannot make different pixels, so a row this calls
//! unchanged is unchanged. The converse is not owed: a chroma sample that moves
//! under a luma that is already saturated converts a row for nothing, which
//! costs a row and cannot cost a picture.

use super::convert::Plane;

/// How wide a column of [`Changes`] is. Sixteen pixels is a cache line's worth
/// of framebuffer words, and the caller's tiles are an order of magnitude wider
/// than that.
const COLUMN_GRAIN: usize = 16;

/// The rows a frame changed, and the columns they changed in.
///
/// Rows exactly, since a row is what the conversion skips; columns to
/// [`COLUMN_GRAIN`], since they only ever become a damage rectangle and a finer
/// answer would cost more to find than it could save.
///
/// One of these covers a whole update rather than one frame: a message may
/// carry several, each decoded into the same buffer, and what the caller needs
/// is the rows that moved since the picture on screen.
pub struct Changes {
    rows: Vec<bool>,
    top: usize, // usize::MAX where nothing has moved
    bottom: usize,
    left: usize, // in columns of COLUMN_GRAIN
    right: usize,
}

impl Changes {
    pub fn new() -> Changes {
        Changes {
            rows: Vec::new(),
            top: usize::MAX,
            bottom: 0,
            left: usize::MAX,
            right: 0,
        }
    }

    /// Nothing has moved, in a picture this tall.
    pub fn reset(&mut self, height: usize) {
        self.rows.clear();
        self.rows.resize(height, false);
        self.top = usize::MAX;
        self.bottom = 0;
        self.left = usize::MAX;
        self.right = 0;
    }

    /// All of it has, which is a first frame, a decoder that has changed its
    /// output layout, or a buffer that has lost track of what is on screen.
    pub fn all(&mut self, width: usize, height: usize) {
        self.rows.clear();
        self.rows.resize(height, true);
        if width == 0 || height == 0 {
            return;
        }
        self.top = 0;
        self.bottom = height - 1;
        self.left = 0;
        self.right = (width - 1) / COLUMN_GRAIN;
    }

    /// This row moved, in the pixels from `from` up to `to`.
    pub fn mark(&mut self, row: usize, from: usize, to: usize) {
        let Some(flag) = self.rows.get_mut(row) else {
            return;
        };
        *flag = true;
        self.top = self.top.min(row);
        self.bottom = self.bottom.max(row);
        if to > from {
            self.left = self.left.min(from / COLUMN_GRAIN);
            self.right = self.right.max((to - 1) / COLUMN_GRAIN);
        }
    }

    pub fn row(&self, row: usize) -> bool {
        self.rows.get(row).copied().unwrap_or(false)
    }

    /// How many rows moved, which is what a frame here costs.
    pub fn count(&self) -> usize {
        self.rows.iter().filter(|moved| **moved).count()
    }

    /// What moved as one rectangle, or `None` where nothing did.
    pub fn bounds(&self, width: usize) -> Option<(usize, usize, usize, usize)> {
        if self.top == usize::MAX || self.left == usize::MAX {
            return None;
        }
        let x = self.left * COLUMN_GRAIN;
        let right = ((self.right + 1) * COLUMN_GRAIN).min(width);
        Some((x, self.top, right - x, self.bottom - self.top + 1))
    }
}

impl Default for Changes {
    fn default() -> Changes {
        Changes::new()
    }
}

/// How a decoder's output is arranged, as far as a comparison cares: two
/// buffers of the same shape can be compared row for row, and anything else is
/// a fresh start.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Shape {
    width: usize,
    height: usize,
    horiz: usize,
    vert: usize,
    /// U and V in one plane, a byte apart, which is what a phone's decoder
    /// usually hands back. Their two rows are then one run of bytes and one
    /// comparison rather than two strided ones.
    interleaved: bool,
}

impl Shape {
    fn of(planes: &[Plane; 3], width: usize, height: usize) -> Shape {
        Shape {
            width,
            height,
            horiz: planes[1].horiz_subsampling.max(1),
            vert: planes[1].vert_subsampling.max(1),
            interleaved: planes[1].col_inc == 2
                && planes[2].col_inc == 2
                && planes[1].row_inc == planes[2].row_inc
                && planes[2].offset.abs_diff(planes[1].offset) == 1,
        }
    }

    fn chroma(&self) -> (usize, usize) {
        (self.width.div_ceil(self.horiz), self.height.div_ceil(self.vert))
    }
}

/// The planes of the last frame that was converted, kept so the next one can be
/// compared against them.
///
/// Packed rather than in the decoder's own strides: the buffer they are copied
/// out of belongs to the decoder and is handed back the moment a frame has been
/// read, so keeping them means keeping a copy. It is 1.5 bytes a pixel, against
/// the 4 that each of the two pictures costs.
///
/// A row is copied only where it differs, so what the copy costs tracks what
/// moved rather than what arrived.
pub struct Previous {
    luma: Vec<u8>,
    chroma: Vec<u8>,
    shape: Option<Shape>,
}

impl Previous {
    pub fn new() -> Previous {
        Previous {
            luma: Vec::new(),
            chroma: Vec::new(),
            shape: None,
        }
    }

    /// Which rows of this frame differ from the last one's, marked into
    /// `changed`, and this frame kept for the next comparison.
    ///
    /// `changed` is added to rather than replaced: a caller that already knows
    /// a row has to be converted keeps that answer, and the union of the two is
    /// what the conversion then writes.
    pub fn compare(
        &mut self,
        base: &[u8],
        planes: &[Plane; 3],
        width: usize,
        height: usize,
        changed: &mut Changes,
    ) {
        let shape = Shape::of(planes, width, height);
        let (cw, ch) = shape.chroma();
        if !self.fits(base, planes, &shape) {
            // Nothing here describes this buffer, so nothing here may be
            // trusted about it either: the whole frame is converted and the
            // next one starts again.
            self.shape = None;
            changed.all(width, height);
            return;
        }
        if self.shape != Some(shape) {
            self.luma.clear();
            self.luma.resize(width * height, 0);
            self.chroma.clear();
            self.chroma.resize(2 * cw * ch, 0);
            self.shape = Some(shape);
            changed.all(width, height);
            // And on through the comparison, which is what fills the copy: a
            // row that happens to match the zeros it was allocated with is a
            // row already in hand.
        }

        // What one unit of the chroma answer is: a byte of an interleaved pair,
        // or a sample where the two planes are separate — and how many of them
        // one column of the answer is worth.
        let per_sample = if shape.interleaved { 2 } else { 1 };
        let chroma_grain = (COLUMN_GRAIN * per_sample / shape.horiz).max(1);

        // Chroma first, because one of its rows decides `vert` luma rows and a
        // luma row that is already marked is one less range to work out.
        for crow in 0..ch {
            let cached = &mut self.chroma[crow * 2 * cw..][..2 * cw];
            let moved = if shape.interleaved {
                let at = planes[1].offset.min(planes[2].offset) + crow * planes[1].row_inc;
                differs(base, at, 1, chroma_grain, cached)
            } else {
                // U then V, and whichever of the two moved decides the row.
                let (u, v) = cached.split_at_mut(cw);
                let u_at = planes[1].offset + crow * planes[1].row_inc;
                let v_at = planes[2].offset + crow * planes[2].row_inc;
                span(
                    differs(base, u_at, planes[1].col_inc, chroma_grain, u),
                    differs(base, v_at, planes[2].col_inc, chroma_grain, v),
                )
            };
            let Some((first, last)) = moved else { continue };
            let from = first / per_sample * shape.horiz;
            let to = ((last / per_sample + 1) * shape.horiz).min(width);
            for row in crow * shape.vert..((crow + 1) * shape.vert).min(height) {
                changed.mark(row, from, to);
            }
        }

        for row in 0..height {
            let at = planes[0].offset + row * planes[0].row_inc;
            let cached = &mut self.luma[row * width..][..width];
            if let Some((first, last)) = differs(base, at, planes[0].col_inc, COLUMN_GRAIN, cached)
            {
                changed.mark(row, first, last + 1);
            }
        }
    }

    /// Whether every byte the comparison would read is inside the buffer. The
    /// caller checks the planes it converts from, and this checks the runs this
    /// reads, which are the same bytes said a different way.
    fn fits(&self, base: &[u8], planes: &[Plane; 3], shape: &Shape) -> bool {
        if shape.width == 0 || shape.height == 0 {
            return false;
        }
        let (cw, ch) = shape.chroma();
        let last = |plane: &Plane, rows: usize, run: usize| {
            plane.offset + (rows - 1) * plane.row_inc + (run - 1) * plane.col_inc.max(1)
        };
        let mut end = last(&planes[0], shape.height, shape.width);
        if shape.interleaved {
            let at = planes[1].offset.min(planes[2].offset);
            end = end.max(at + (ch - 1) * planes[1].row_inc + 2 * cw - 1);
        } else {
            end = end.max(last(&planes[1], ch, cw)).max(last(&planes[2], ch, cw));
        }
        end < base.len()
    }
}

impl Default for Previous {
    fn default() -> Previous {
        Previous::new()
    }
}

/// Where a plane row differs from the copy kept of it, as the first and last
/// unit that does — and the copy brought up to date, since a row that differs
/// is a row that has to be kept.
///
/// The whole row is compared first and most rows of most frames stop there;
/// only one that differs is walked, in units of `grain`, which is whatever
/// number of them the caller's column is worth.
fn differs(
    base: &[u8],
    at: usize,
    inc: usize,
    grain: usize,
    cached: &mut [u8],
) -> Option<(usize, usize)> {
    let n = cached.len();
    if inc != 1 {
        let mut first = usize::MAX;
        let mut last = 0;
        for i in 0..n {
            let byte = base[at + i * inc];
            if byte != cached[i] {
                cached[i] = byte;
                first = first.min(i);
                last = i;
            }
        }
        return (first != usize::MAX).then_some((first, last));
    }
    let src = &base[at..at + n];
    if src == cached {
        return None;
    }
    let mut first = usize::MAX;
    let mut last = 0;
    for from in (0..n).step_by(grain) {
        let to = (from + grain).min(n);
        if src[from..to] != cached[from..to] {
            first = first.min(from);
            last = to - 1;
        }
    }
    cached.copy_from_slice(src);
    Some((first, last))
}

fn span(a: Option<(usize, usize)>, b: Option<(usize, usize)>) -> Option<(usize, usize)> {
    match (a, b) {
        (Some(a), Some(b)) => Some((a.0.min(b.0), a.1.max(b.1))),
        (found, None) | (None, found) => found,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame's worth of planes, in both of the layouts a decoder hands back.
    fn frame(w: usize, h: usize, interleaved: bool) -> (Vec<u8>, [Plane; 3]) {
        let base = vec![0x40u8; w * h + w * h / 2];
        let luma = Plane {
            offset: 0,
            col_inc: 1,
            row_inc: w,
            horiz_subsampling: 1,
            vert_subsampling: 1,
        };
        let chroma = |offset: usize, col_inc: usize| Plane {
            offset,
            col_inc,
            row_inc: if col_inc == 2 { w } else { w / 2 },
            horiz_subsampling: 2,
            vert_subsampling: 2,
        };
        let planes = if interleaved {
            [luma, chroma(w * h, 2), chroma(w * h + 1, 2)]
        } else {
            [luma, chroma(w * h, 1), chroma(w * h + w * h / 4, 1)]
        };
        (base, planes)
    }

    fn compare(p: &mut Previous, base: &[u8], planes: &[Plane; 3], w: usize, h: usize) -> Changes {
        let mut changed = Changes::new();
        changed.reset(h);
        p.compare(base, planes, w, h, &mut changed);
        changed
    }

    #[test]
    fn a_first_frame_is_all_of_it() {
        let (base, planes) = frame(64, 8, true);
        let mut previous = Previous::new();
        let changed = compare(&mut previous, &base, &planes, 64, 8);
        assert_eq!(changed.bounds(64), Some((0, 0, 64, 8)));
        // And the second one, which is the same picture, is none of it.
        let changed = compare(&mut previous, &base, &planes, 64, 8);
        assert_eq!(changed.bounds(64), None);
    }

    #[test]
    fn a_luma_pixel_is_one_row() {
        for interleaved in [false, true] {
            let (mut base, planes) = frame(64, 8, interleaved);
            let mut previous = Previous::new();
            compare(&mut previous, &base, &planes, 64, 8);
            base[3 * 64 + 20] = 0x41;
            let changed = compare(&mut previous, &base, &planes, 64, 8);
            // The row exactly, the columns to the grain either side.
            assert_eq!(changed.bounds(64), Some((16, 3, 16, 1)));
            assert!(!changed.row(2) && changed.row(3) && !changed.row(4));
        }
    }

    #[test]
    fn a_chroma_sample_is_the_rows_it_covers() {
        for interleaved in [false, true] {
            let (mut base, planes) = frame(64, 8, interleaved);
            let mut previous = Previous::new();
            compare(&mut previous, &base, &planes, 64, 8);
            // The V sample of chroma row 2, column 10, which is the luma rows
            // 4 and 5 and the pixels 20 and 21.
            let at = if interleaved {
                planes[2].offset + 2 * planes[2].row_inc + 10 * 2
            } else {
                planes[2].offset + 2 * planes[2].row_inc + 10
            };
            base[at] = 0x41;
            let changed = compare(&mut previous, &base, &planes, 64, 8);
            assert_eq!(changed.bounds(64), Some((16, 4, 16, 2)));
        }
    }

    /// The copy is what the comparison is against, so a frame that goes back to
    /// what it was is a change both times rather than a change and then
    /// nothing.
    #[test]
    fn a_row_that_returns_moves_twice() {
        let (mut base, planes) = frame(64, 8, true);
        let mut previous = Previous::new();
        compare(&mut previous, &base, &planes, 64, 8);
        base[64] = 0x41;
        assert!(compare(&mut previous, &base, &planes, 64, 8).row(1));
        base[64] = 0x40;
        assert!(compare(&mut previous, &base, &planes, 64, 8).row(1));
        assert_eq!(compare(&mut previous, &base, &planes, 64, 8).bounds(64), None);
    }

    #[test]
    fn a_layout_that_changes_starts_again() {
        let (base, planar) = frame(64, 8, false);
        let (_, semi) = frame(64, 8, true);
        let mut previous = Previous::new();
        compare(&mut previous, &base, &planar, 64, 8);
        assert_eq!(compare(&mut previous, &base, &planar, 64, 8).bounds(64), None);
        let changed = compare(&mut previous, &base, &semi, 64, 8);
        assert_eq!(changed.bounds(64), Some((0, 0, 64, 8)));
    }

    /// A buffer the planes do not fit in is a whole frame and no memory of it,
    /// rather than a panic on the protocol thread.
    #[test]
    fn a_buffer_that_is_too_small_is_all_of_it_twice() {
        let (base, planes) = frame(64, 8, true);
        let mut previous = Previous::new();
        let short = &base[..base.len() / 2];
        assert_eq!(compare(&mut previous, short, &planes, 64, 8).bounds(64), Some((0, 0, 64, 8)));
        assert_eq!(compare(&mut previous, short, &planes, 64, 8).bounds(64), Some((0, 0, 64, 8)));
    }
}
