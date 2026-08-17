//! 4:2:0 planes to `R, G, B, A` words, which is what a decoded frame costs
//! after the decoder has finished with it.
//!
//! The arithmetic is the same expression twice — once a row at a time and once
//! sixteen pixels at a time — and the vectorised one is used only where it has
//! been shown to agree with the other on this decoder's own output. What is in
//! doubt is never the arithmetic; it is whether the planes are where the
//! decoder's descriptor says they are, and a row of real pixels answers that
//! where an assertion about a column increment does not.
//!
//! It is a second implementation of what TigerVNC's shim does in C++ for H.264,
//! and deliberately not a shared one: the two are in different languages inside
//! different modules, and what would be shared is thirty lines of arithmetic
//! rather than a decision.

/// Where a decoder put the three planes of one output buffer, out of its
/// `image-data` descriptor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Plane {
    pub offset: usize,
    pub col_inc: usize,
    pub row_inc: usize,
    pub horiz_subsampling: usize,
    pub vert_subsampling: usize,
}

/// The three increments a row conversion needs.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub y_col_inc: usize,
    pub uv_col_inc: usize,
    pub horiz_subsampling: usize,
}

/// Y, then the four chroma terms, in 1/256ths, and what Y is offset by.
#[derive(Clone, Copy)]
pub struct Coefficients {
    pub y: i32,
    pub rv: i32,
    pub gu: i32,
    pub gv: i32,
    pub bu: i32,
    pub y_offset: i32,
}

pub fn coefficients(bt709: bool, full_range: bool) -> Coefficients {
    let [y, rv, gu, gv, bu, y_offset] = match (full_range, bt709) {
        (true, true) => [256, 403, -48, -120, 475, 0],
        (true, false) => [256, 359, -88, -183, 454, 0],
        (false, true) => [298, 459, -55, -136, 541, -16],
        (false, false) => [298, 409, -100, -208, 516, -16],
    };
    Coefficients {
        y,
        rv,
        gu,
        gv,
        bu,
        y_offset,
    }
}

#[inline]
fn clamp(v: i32) -> u32 {
    v.clamp(0, 255) as u32
}

/// One row, scalar, from `col` to the end of it.
///
/// The chroma terms are computed once per chroma sample rather than once per
/// pixel, and the subsampling is a countdown rather than a division: a division
/// inside this loop, which is the obvious way to write it, cost 29 ms of a
/// decoder thread for one 1920×1200 frame in the C++ this is a second copy of —
/// more than the hardware decoder spends on the whole picture.
pub fn row_scalar(
    base: &[u8],
    y_at: usize,
    u_at: usize,
    v_at: usize,
    l: &Layout,
    out: &mut [u32],
    col: usize,
    w: usize,
    c: &Coefficients,
) {
    let mut yi = y_at + col * l.y_col_inc;
    let mut ui = u_at + (col / l.horiz_subsampling) * l.uv_col_inc;
    let mut vi = v_at + (col / l.horiz_subsampling) * l.uv_col_inc;
    let mut chroma = 0usize;
    let (mut rv, mut gu, mut bu) = (0i32, 0i32, 0i32);
    for out in out.iter_mut().take(w).skip(col) {
        if chroma == 0 {
            let uu = base[ui] as i32 - 128;
            let vv = base[vi] as i32 - 128;
            rv = c.rv * vv + 128;
            gu = c.gu * uu + c.gv * vv + 128;
            bu = c.bu * uu + 128;
            ui += l.uv_col_inc;
            vi += l.uv_col_inc;
            chroma = l.horiz_subsampling;
        }
        chroma -= 1;
        let yy = (base[yi] as i32 + c.y_offset) * c.y;
        yi += l.y_col_inc;
        // Red at shift 0, which is the framebuffer's word order and so the byte
        // order an Android `ARGB_8888` bitmap reads.
        *out = clamp((yy + rv) >> 8)
            | (clamp((yy + gu) >> 8) << 8)
            | (clamp((yy + bu) >> 8) << 16)
            | 0xff00_0000;
    }
}

#[cfg(target_arch = "aarch64")]
mod neon {
    use super::{Coefficients, Layout};
    use std::arch::aarch64::*;

    /// The same row, sixteen pixels at a time. Returns how many it did, which
    /// is a multiple of sixteen and may be none — the tail, and every layout
    /// not covered here, goes back through the scalar loop.
    ///
    /// Sixteen rather than eight because that is what makes both chroma loads
    /// whole: eight chroma samples is one `vld1_u8` where the planes are
    /// separate and one `vld2_u8` where they are interleaved, and neither reads
    /// a byte the sixteen luma pixels do not account for.
    ///
    /// # Safety
    ///
    /// Every index this touches is inside `base` when the caller has checked
    /// the last pixel of each plane, which `Decoder::convert` does per buffer.
    pub unsafe fn row(
        base: &[u8],
        y_at: usize,
        u_at: usize,
        v_at: usize,
        l: &Layout,
        out: &mut [u32],
        w: usize,
        c: &Coefficients,
    ) -> usize {
        if l.y_col_inc != 1 || l.horiz_subsampling != 2 {
            return 0;
        }
        // NV12 has U first and NV21 V first; one load covers both planes either
        // way, and which half is which is the only difference.
        let interleaved = l.uv_col_inc == 2 && (v_at == u_at + 1 || u_at == v_at + 1);
        if l.uv_col_inc != 1 && !interleaved {
            return 0;
        }
        let u_first = v_at == u_at + 1;

        unsafe {
            let y_off = vdupq_n_s16(c.y_offset as i16);
            let cy = vdup_n_s16(c.y as i16);
            let crv = vdup_n_s16(c.rv as i16);
            let cgu = vdup_n_s16(c.gu as i16);
            let cgv = vdup_n_s16(c.gv as i16);
            let cbu = vdup_n_s16(c.bu as i16);
            let half = vdupq_n_s32(128);
            let opaque = vdup_n_u8(0xff);
            let mid = vdupq_n_s16(128);

            let mut col = 0usize;
            while col + 16 <= w {
                let y_ptr = base.as_ptr().add(y_at + col);
                let u_ptr = base.as_ptr().add(u_at + col / 2 * l.uv_col_inc);
                let v_ptr = base.as_ptr().add(v_at + col / 2 * l.uv_col_inc);

                let (ub, vb) = if interleaved {
                    let pair = vld2_u8(if u_first { u_ptr } else { v_ptr });
                    if u_first {
                        (pair.0, pair.1)
                    } else {
                        (pair.1, pair.0)
                    }
                } else {
                    (vld1_u8(u_ptr), vld1_u8(v_ptr))
                };
                let uu = vsubq_s16(vreinterpretq_s16_u16(vmovl_u8(ub)), mid);
                let vv = vsubq_s16(vreinterpretq_s16_u16(vmovl_u8(vb)), mid);

                // Eight chroma terms, then each duplicated onto the two luma
                // pixels it covers, which is what the zip below does.
                let mut rv = [vdupq_n_s32(0); 2];
                let mut gu = [vdupq_n_s32(0); 2];
                let mut bu = [vdupq_n_s32(0); 2];
                for i in 0..2 {
                    let uh = if i == 0 {
                        vget_low_s16(uu)
                    } else {
                        vget_high_s16(uu)
                    };
                    let vh = if i == 0 {
                        vget_low_s16(vv)
                    } else {
                        vget_high_s16(vv)
                    };
                    rv[i] = vaddq_s32(vmull_s16(vh, crv), half);
                    gu[i] = vaddq_s32(vaddq_s32(vmull_s16(uh, cgu), vmull_s16(vh, cgv)), half);
                    bu[i] = vaddq_s32(vmull_s16(uh, cbu), half);
                }

                let yraw = vld1q_u8(y_ptr);
                let ylo = vaddq_s16(
                    vreinterpretq_s16_u16(vmovl_u8(vget_low_u8(yraw))),
                    y_off,
                );
                let yhi = vaddq_s16(
                    vreinterpretq_s16_u16(vmovl_u8(vget_high_u8(yraw))),
                    y_off,
                );
                let yy = [
                    vmull_s16(vget_low_s16(ylo), cy),
                    vmull_s16(vget_high_s16(ylo), cy),
                    vmull_s16(vget_low_s16(yhi), cy),
                    vmull_s16(vget_high_s16(yhi), cy),
                ];

                for block in 0..2 {
                    let r = (vzip1q_s32(rv[block], rv[block]), vzip2q_s32(rv[block], rv[block]));
                    let g = (vzip1q_s32(gu[block], gu[block]), vzip2q_s32(gu[block], gu[block]));
                    let b = (vzip1q_s32(bu[block], bu[block]), vzip2q_s32(bu[block], bu[block]));
                    // `vqshrun` saturates the negative side to zero and
                    // `vqmovn` the high side to 255, so the scalar clamp is two
                    // instructions here.
                    let pack = |lo: int32x4_t, hi: int32x4_t| {
                        vqmovn_u16(vcombine_u16(
                            vqshrun_n_s32::<8>(vaddq_s32(yy[block * 2], lo)),
                            vqshrun_n_s32::<8>(vaddq_s32(yy[block * 2 + 1], hi)),
                        ))
                    };
                    let px = uint8x8x4_t(
                        pack(r.0, r.1),
                        pack(g.0, g.1),
                        pack(b.0, b.1),
                        opaque,
                    );
                    vst4_u8(out.as_mut_ptr().add(col + block * 8) as *mut u8, px);
                }
                col += 16;
            }
            col
        }
    }
}

/// Whether the two agree on this decoder's first row, which is what decides
/// that the vectorised one may be used at all. One row of one frame, once per
/// layout.
pub fn vectorised_agrees(
    base: &[u8],
    y_at: usize,
    u_at: usize,
    v_at: usize,
    l: &Layout,
    w: usize,
    c: &Coefficients,
) -> bool {
    let mut scalar = vec![0u32; w];
    let mut vector = vec![0u32; w];
    row_scalar(base, y_at, u_at, v_at, l, &mut scalar, 0, w, c);
    let done = row_vectorised(base, y_at, u_at, v_at, l, &mut vector, w, c);
    if done == 0 {
        return false;
    }
    row_scalar(base, y_at, u_at, v_at, l, &mut vector, done, w, c);
    scalar == vector
}

/// The vectorised row where there is one, and nothing done where there is not.
pub fn row_vectorised(
    base: &[u8],
    y_at: usize,
    u_at: usize,
    v_at: usize,
    l: &Layout,
    out: &mut [u32],
    w: usize,
    c: &Coefficients,
) -> usize {
    #[cfg(target_arch = "aarch64")]
    {
        unsafe { neon::row(base, y_at, u_at, v_at, l, out, w, c) }
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        let _ = (base, y_at, u_at, v_at, l, out, w, c);
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A planar and a semi-planar buffer of the same picture, so that a wrong
    /// column increment shows up as a wrong colour rather than as nothing.
    fn planes(w: usize, h: usize, interleaved: bool) -> (Vec<u8>, Layout, [usize; 3]) {
        let mut base = vec![0u8; w * h + w * h / 2 + 16];
        for i in 0..w * h {
            base[i] = (i * 7 % 256) as u8;
        }
        let chroma = w * h;
        for i in 0..w * h / 4 {
            if interleaved {
                base[chroma + i * 2] = (i * 11 % 256) as u8;
                base[chroma + i * 2 + 1] = (i * 13 % 256) as u8;
            } else {
                base[chroma + i] = (i * 11 % 256) as u8;
                base[chroma + w * h / 4 + i] = (i * 13 % 256) as u8;
            }
        }
        if interleaved {
            (
                base,
                Layout {
                    y_col_inc: 1,
                    uv_col_inc: 2,
                    horiz_subsampling: 2,
                },
                [0, chroma, chroma + 1],
            )
        } else {
            (
                base,
                Layout {
                    y_col_inc: 1,
                    uv_col_inc: 1,
                    horiz_subsampling: 2,
                },
                [0, chroma, chroma + w * h / 4],
            )
        }
    }

    /// Only where there is a second path to agree with: everywhere else the
    /// scalar loop is the whole of the conversion, and the phone checks this
    /// again on its own decoder's first row of every layout.
    #[cfg(target_arch = "aarch64")]
    #[test]
    fn the_two_paths_agree() {
        let c = coefficients(true, false);
        for interleaved in [false, true] {
            let (base, layout, at) = planes(64, 4, interleaved);
            // A width that is not a multiple of sixteen, so the tail is
            // exercised as well as the block.
            assert!(vectorised_agrees(&base, at[0], at[1], at[2], &layout, 61, &c));
        }
    }

    /// Black and white through the full-range coefficients, which is the one
    /// case the arithmetic can be checked against by hand.
    #[test]
    fn the_ends_of_the_range() {
        let c = coefficients(true, true);
        let base = [0u8, 255, 128, 128, 128, 128];
        let layout = Layout {
            y_col_inc: 1,
            uv_col_inc: 1,
            horiz_subsampling: 2,
        };
        let mut out = [0u32; 2];
        row_scalar(&base, 0, 2, 4, &layout, &mut out, 0, 2, &c);
        assert_eq!(out[0], 0xff00_0000);
        assert_eq!(out[1], 0xffff_ffff);
    }
}
