//! The encodings, each decoding one rectangle into scratch space.
//!
//! Everything here works in [`crate::PixelFormat::NATIVE`] and writes into a
//! `w × h` buffer of framebuffer words. Nothing touches the framebuffer itself
//! — the caller blits under the write lock, which is what keeps that lock held
//! for a memcpy rather than for a socket read (see [`crate::Framebuffer`]).
//!
//! `CopyRect` is the exception and is not here: it reads the framebuffer, so it
//! is the caller's business.

use crate::error::{Error, Result};
use crate::pixel::{PixelReader, pack};
use crate::proto::Reader;
use crate::zlib::ZlibStream;
use std::io::Read;

pub const ENC_RAW: i32 = 0;
pub const ENC_COPY_RECT: i32 = 1;
pub const ENC_RRE: i32 = 2;
pub const ENC_HEXTILE: i32 = 5;
pub const ENC_ZRLE: i32 = 16;
pub const ENC_CURSOR: i32 = -239;
pub const ENC_DESKTOP_SIZE: i32 = -223;
pub const ENC_LAST_RECT: i32 = -224;
/// `ExtendedMouseButtons`: a ninth button, and a byte to put it in. Offered in
/// `SetEncodings` and **acknowledged** by an empty rectangle of this encoding —
/// the extra byte may not be sent before that arrives, since to a server that
/// never agreed to it the marker bit means button 8 and the byte after it is
/// the next message.
pub const ENC_EXTENDED_MOUSE_BUTTONS: i32 = -316;
/// QEMU's `PointerTypeChange`: the rectangle's *x* is 1 for absolute and 0 for
/// relative, there are no pixels, and it arrives unprompted after every
/// `SetEncodings`.
pub const ENC_POINTER_TYPE_CHANGE: i32 = -257;
/// `ExtendedDesktopSize`: the desktop's size *and* who may change it. The
/// rectangle's *x* is why it arrived and its *y* is what came of it; the body is
/// a screen layout. A server that sends one at all will take a `SetDesktopSize`,
/// which is the only way this client ever learns that.
pub const ENC_EXTENDED_DESKTOP_SIZE: i32 = -308;

/// Why an [`ENC_EXTENDED_DESKTOP_SIZE`] rectangle arrived, and how it went.
/// Only a rectangle that is both this client's doing and a failure means the
/// size has not changed; everything else is the server stating the truth.
pub const RESIZE_REASON_CLIENT: usize = 1;
pub const RESIZE_RESULT_SUCCESS: usize = 0;

/// Whether a rectangle of this encoding carries pixels at all. The
/// pseudo-encodings use the same four header fields to mean other things — a
/// cursor's size, a new desktop's size, a pointer mode in *x* — so the caller's
/// "inside the desktop" test applies to the real ones and to nothing else.
pub fn is_pseudo(encoding: i32) -> bool {
    !matches!(
        encoding,
        ENC_RAW | ENC_COPY_RECT | ENC_RRE | ENC_HEXTILE | ENC_ZRLE
    )
}

/// What to call one in the connection panel.
pub fn encoding_name(encoding: i32) -> &'static str {
    match encoding {
        ENC_RAW => "Raw",
        ENC_COPY_RECT => "CopyRect",
        ENC_RRE => "RRE",
        ENC_HEXTILE => "Hextile",
        ENC_ZRLE => "ZRLE",
        _ => "unknown",
    }
}

/// Raw: the pixels, in order, and the reason every other encoding exists.
pub fn raw(r: &mut Reader<impl Read>, w: usize, h: usize, out: &mut Vec<u32>) -> Result<()> {
    out.clear();
    out.reserve(w * h);
    // One read for the whole rectangle rather than one per pixel: at 32 bpp a
    // full-screen update is a couple of million calls otherwise.
    let bytes = r.bytes(w * h * 4)?;
    out.extend(
        bytes
            .chunks_exact(4)
            .map(|c| pack(c[0], c[1], c[2])),
    );
    Ok(())
}

/// RRE: a background, then a list of flat rectangles over it.
pub fn rre(r: &mut Reader<impl Read>, w: usize, h: usize, out: &mut Vec<u32>) -> Result<()> {
    let count = r.u32()? as usize;
    let background = r.pixel()?;
    out.clear();
    out.resize(w * h, background);
    for _ in 0..count {
        let colour = r.pixel()?;
        let (sx, sy) = (r.u16()? as usize, r.u16()? as usize);
        let (sw, sh) = (r.u16()? as usize, r.u16()? as usize);
        fill(out, w, h, sx, sy, sw, sh, colour);
    }
    Ok(())
}

const HEXTILE_RAW: u8 = 1;
const HEXTILE_BACKGROUND: u8 = 2;
const HEXTILE_FOREGROUND: u8 = 4;
const HEXTILE_SUBRECTS: u8 = 8;
const HEXTILE_COLOURED: u8 = 16;

/// Hextile: 16×16 tiles, each either raw or a background with sub-rectangles.
///
/// The background and foreground colours **persist between tiles** — a tile
/// that specifies neither reuses the previous one's, which is where most of its
/// compression comes from and the one piece of state a naive decoder drops.
pub fn hextile(r: &mut Reader<impl Read>, w: usize, h: usize, out: &mut Vec<u32>) -> Result<()> {
    out.clear();
    out.resize(w * h, 0xff00_0000);
    let mut background = 0xff00_0000u32;
    let mut foreground = 0xff00_0000u32;
    let mut tile: Vec<u32> = Vec::new();

    for ty in (0..h).step_by(16) {
        let th = 16.min(h - ty);
        for tx in (0..w).step_by(16) {
            let tw = 16.min(w - tx);
            let flags = r.u8()?;
            if flags & HEXTILE_RAW != 0 {
                raw(r, tw, th, &mut tile)?;
                blit(out, w, tx, ty, tw, th, &tile);
                continue;
            }
            if flags & HEXTILE_BACKGROUND != 0 {
                background = r.pixel()?;
            }
            if flags & HEXTILE_FOREGROUND != 0 {
                foreground = r.pixel()?;
            }
            fill(out, w, h, tx, ty, tw, th, background);
            if flags & HEXTILE_SUBRECTS == 0 {
                continue;
            }
            let count = r.u8()? as usize;
            for _ in 0..count {
                let colour = if flags & HEXTILE_COLOURED != 0 {
                    r.pixel()?
                } else {
                    foreground
                };
                // Two bytes for the whole sub-rectangle: a nibble each for x,
                // y, width-1 and height-1, which is why a tile is 16 wide.
                let xy = r.u8()?;
                let wh = r.u8()?;
                let sx = (xy >> 4) as usize;
                let sy = (xy & 0x0f) as usize;
                let sw = (wh >> 4) as usize + 1;
                let sh = (wh & 0x0f) as usize + 1;
                // Bounded by the room left in the tile rather than by the
                // tile's size: a sub-rectangle at x=15 may still claim a width
                // of 16, and the neighbouring tile only survives it because it
                // is decoded afterwards and paints over the spill.
                let sw = sw.min(tw.saturating_sub(sx));
                let sh = sh.min(th.saturating_sub(sy));
                fill(out, w, h, tx + sx, ty + sy, sw, sh, colour);
            }
        }
    }
    Ok(())
}

/// ZRLE: 64×64 tiles down a persistent zlib stream, each tile raw, solid,
/// palettised, run-length encoded, or both at once.
pub fn zrle(
    r: &mut Reader<impl Read>,
    stream: &mut ZlibStream,
    w: usize,
    h: usize,
    out: &mut Vec<u32>,
) -> Result<()> {
    let length = r.u32()? as usize;
    // The compressed length is the server's, so it is bounded before it becomes
    // an allocation.
    if length > 64 << 20 {
        return Err(Error::Protocol(format!("ZRLE rectangle of {length} bytes")));
    }
    let compressed = r.bytes(length)?;
    // What the tiles can possibly be worth decompressed: the costliest tile is
    // run-length encoded with every run one pixel long, which is a three-byte
    // CPIXEL and a length byte each, plus a subencoding byte and a full palette
    // per tile.
    let tiles_over = w.div_ceil(64) * h.div_ceil(64);
    let data = stream.inflate(&compressed, w * h * 4 + tiles_over * (1 + 128 * 3) + 64)?;
    let mut tiles = Tiles { data, at: 0 };

    out.clear();
    out.resize(w * h, 0xff00_0000);
    let mut tile: Vec<u32> = Vec::new();
    let mut palette = [0u32; 128];

    for ty in (0..h).step_by(64) {
        let th = 64.min(h - ty);
        for tx in (0..w).step_by(64) {
            let tw = 64.min(w - tx);
            let n = tw * th;
            let subencoding = tiles.u8()?;
            let rle = subencoding & 0x80 != 0;
            let palette_size = (subencoding & 0x7f) as usize;

            tile.clear();
            match (rle, palette_size) {
                (false, 0) => {
                    for _ in 0..n {
                        tile.push(tiles.cpixel()?);
                    }
                }
                (_, 1) => {
                    // Solid, with or without the RLE bit: one colour, no data.
                    tile.resize(n, tiles.cpixel()?);
                }
                (false, size) => {
                    for slot in palette.iter_mut().take(size) {
                        *slot = tiles.cpixel()?;
                    }
                    // Packed indices, 1, 2 or 4 bits, each *row* padded to a
                    // byte boundary — not the tile.
                    let bits = match size {
                        2 => 1,
                        3..=4 => 2,
                        5..=16 => 4,
                        _ => return Err(Error::Protocol(format!("ZRLE palette of {size}"))),
                    };
                    let per_byte = 8 / bits;
                    let mask = (1u8 << bits) - 1;
                    for _ in 0..th {
                        let mut col = 0;
                        while col < tw {
                            let byte = tiles.u8()?;
                            for slot in 0..per_byte {
                                if col >= tw {
                                    break;
                                }
                                let shift = 8 - bits * (slot + 1);
                                let index = ((byte >> shift) & mask) as usize;
                                tile.push(*palette.get(index).unwrap_or(&0xff00_0000));
                                col += 1;
                            }
                        }
                    }
                }
                (true, 0) => {
                    // Plain RLE: colour, then a length in 255-sized instalments.
                    while tile.len() < n {
                        let colour = tiles.cpixel()?;
                        let run = tiles.run_length()?;
                        let take = run.min(n - tile.len());
                        tile.resize(tile.len() + take, colour);
                    }
                }
                (true, size) => {
                    for slot in palette.iter_mut().take(size) {
                        *slot = tiles.cpixel()?;
                    }
                    while tile.len() < n {
                        let index = tiles.u8()?;
                        // The top bit says "a run length follows"; without it
                        // the index stands for a single pixel.
                        let run = if index & 0x80 != 0 { tiles.run_length()? } else { 1 };
                        let colour = *palette.get((index & 0x7f) as usize).unwrap_or(&0xff00_0000);
                        let take = run.min(n - tile.len());
                        tile.resize(tile.len() + take, colour);
                    }
                }
            }
            if tile.len() < n {
                return Err(Error::Protocol(format!(
                    "ZRLE tile short: {} of {n}",
                    tile.len()
                )));
            }
            blit(out, w, tx, ty, tw, th, &tile);
        }
    }
    Ok(())
}

/// A cursor from the `Cursor` pseudo-encoding: pixels, then a 1-bit mask.
///
/// The alpha is the mask, so what comes back is drawable as it is — which is
/// what `Listener.cursor` wants, and what makes the client-side cursor the
/// whole touchpad design rests on possible.
pub fn cursor(r: &mut Reader<impl Read>, w: usize, h: usize) -> Result<Vec<u32>> {
    let mut pixels = Vec::with_capacity(w * h);
    for _ in 0..w * h {
        pixels.push(r.pixel()?);
    }
    let stride = w.div_ceil(8);
    let mask = r.bytes(stride * h)?;
    for y in 0..h {
        for x in 0..w {
            let bit = mask[y * stride + x / 8] >> (7 - (x % 8)) & 1;
            if bit == 0 {
                pixels[y * w + x] = 0;
            }
        }
    }
    Ok(pixels)
}

/// A cursor of zero size is how a server hides the pointer, and it is a
/// legitimate message rather than an error.
pub fn empty_cursor(w: usize, h: usize) -> bool {
    w == 0 || h == 0
}

/// A reader over the inflated ZRLE bytes. Not a `Read`: the tile loop wants to
/// fail loudly when the stream is short, rather than block.
struct Tiles<'a> {
    data: &'a [u8],
    at: usize,
}

impl Tiles<'_> {
    fn u8(&mut self) -> Result<u8> {
        let b = *self
            .data
            .get(self.at)
            .ok_or_else(|| Error::Protocol("ZRLE stream ran out".into()))?;
        self.at += 1;
        Ok(b)
    }

    fn cpixel(&mut self) -> Result<u32> {
        let end = self.at + 3;
        let b = self
            .data
            .get(self.at..end)
            .ok_or_else(|| Error::Protocol("ZRLE stream ran out".into()))?;
        let px = pack(b[0], b[1], b[2]);
        self.at = end;
        Ok(px)
    }

    /// 255 means "and more"; the total is one greater than the sum.
    fn run_length(&mut self) -> Result<usize> {
        let mut total = 1usize;
        loop {
            let b = self.u8()?;
            total += b as usize;
            if b != 255 {
                return Ok(total);
            }
        }
    }
}

fn fill(out: &mut [u32], stride: usize, height: usize,
        x: usize, y: usize, w: usize, h: usize, colour: u32) {
    if x >= stride || y >= height {
        return;
    }
    let w = w.min(stride - x);
    let h = h.min(height - y);
    for row in 0..h {
        let at = (y + row) * stride + x;
        out[at..at + w].fill(colour);
    }
}

fn blit(out: &mut [u32], stride: usize, x: usize, y: usize, w: usize, h: usize, src: &[u32]) {
    for row in 0..h {
        let from = row * w;
        let to = (y + row) * stride + x;
        out[to..to + w].copy_from_slice(&src[from..from + w]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::Reader;

    fn reader(bytes: Vec<u8>) -> Reader<std::io::Cursor<Vec<u8>>> {
        Reader::new(std::io::Cursor::new(bytes))
    }

    #[test]
    fn raw_is_rgba() {
        let mut out = Vec::new();
        raw(&mut reader(vec![1, 2, 3, 0, 4, 5, 6, 0]), 2, 1, &mut out).unwrap();
        assert_eq!(out, vec![0xff030201, 0xff060504]);
    }

    #[test]
    fn rre_paints_background_then_subrects() {
        let mut wire = Vec::new();
        wire.extend_from_slice(&1u32.to_be_bytes()); // one sub-rectangle
        wire.extend_from_slice(&[0, 0, 0, 0]); // black background
        wire.extend_from_slice(&[255, 0, 0, 0]); // red
        wire.extend_from_slice(&1u16.to_be_bytes());
        wire.extend_from_slice(&0u16.to_be_bytes());
        wire.extend_from_slice(&1u16.to_be_bytes());
        wire.extend_from_slice(&1u16.to_be_bytes());
        let mut out = Vec::new();
        rre(&mut reader(wire), 2, 2, &mut out).unwrap();
        assert_eq!(out, vec![0xff000000, 0xff0000ff, 0xff000000, 0xff000000]);
    }

    /// The persisting background is the piece worth a test: the second tile
    /// specifies nothing at all and must come out the first one's colour.
    #[test]
    fn hextile_background_persists() {
        let mut wire = Vec::new();
        wire.push(HEXTILE_BACKGROUND);
        wire.extend_from_slice(&[9, 8, 7, 0]);
        wire.push(0); // second tile: no flags
        let mut out = Vec::new();
        hextile(&mut reader(wire), 32, 1, &mut out).unwrap();
        assert!(out.iter().all(|&p| p == pack(9, 8, 7)));
    }

    /// A sub-rectangle may claim a width the tile has no room for. It used to
    /// paint into the next tile's columns, which nothing ever saw because that
    /// tile is decoded afterwards and paints over it — here there is no next
    /// tile, so the spill is visible.
    #[test]
    fn a_hextile_subrect_is_clipped_to_the_room_left_in_its_tile() {
        let mut wire = Vec::new();
        wire.push(HEXTILE_BACKGROUND | HEXTILE_FOREGROUND | HEXTILE_SUBRECTS);
        wire.extend_from_slice(&[0, 0, 0, 0]); // black background
        wire.extend_from_slice(&[255, 0, 0, 0]); // red foreground
        wire.push(1); // one sub-rectangle
        wire.push(0x30); // at x=3, y=0
        wire.push(0xf0); // sixteen wide, one high
        let mut out = Vec::new();
        hextile(&mut reader(wire), 4, 1, &mut out).unwrap();
        assert_eq!(
            out,
            vec![0xff000000, 0xff000000, 0xff000000, pack(255, 0, 0)]
        );
    }

    #[test]
    fn cursor_mask_becomes_alpha() {
        let mut wire = Vec::new();
        for _ in 0..2 {
            wire.extend_from_slice(&[1, 2, 3, 0]);
        }
        wire.push(0b1000_0000); // only the first pixel is opaque
        let px = cursor(&mut reader(wire), 2, 1).unwrap();
        assert_eq!(px, vec![pack(1, 2, 3), 0]);
    }

    #[test]
    fn zrle_run_lengths_are_one_more_than_the_sum() {
        let mut tiles = Tiles {
            data: &[255, 255, 3],
            at: 0,
        };
        assert_eq!(tiles.run_length().unwrap(), 1 + 255 + 255 + 3);
    }
}
