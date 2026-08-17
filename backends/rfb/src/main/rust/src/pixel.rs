// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The pixel format on the wire, and the one we ask every server for.

use crate::proto::{Reader, Writer};
use std::io::{Read, Result, Write};

/// RFB's `PIXEL_FORMAT`: 16 bytes at the front of `ServerInit` and the whole
/// body of `SetPixelFormat`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PixelFormat {
    pub bits_per_pixel: u8,
    pub depth: u8,
    pub big_endian: bool,
    pub true_colour: bool,
    pub red_max: u16,
    pub green_max: u16,
    pub blue_max: u16,
    pub red_shift: u8,
    pub green_shift: u8,
    pub blue_shift: u8,
}

impl PixelFormat {
    /// What we ask for, and the only format the decoders handle.
    ///
    /// Chosen so that no conversion exists at all: 32 bits little-endian with
    /// red at shift 0 puts the bytes in memory as `R, G, B, _`, which is
    /// exactly what Android's `ARGB_8888` bitmap holds (`AndroidBitmap`'s
    /// `RGBA_8888`). A framebuffer word is therefore `0xFF_BB_GG_RR` and a
    /// region read is a row copy — the thing `Backend.readRegion` promises to
    /// be cheap.
    ///
    /// It also makes ZRLE's `CPIXEL` the three low bytes in wire order, which
    /// is `r, g, b` and needs no shifting either.
    pub const NATIVE: PixelFormat = PixelFormat {
        bits_per_pixel: 32,
        depth: 24,
        big_endian: false,
        true_colour: true,
        red_max: 255,
        green_max: 255,
        blue_max: 255,
        red_shift: 0,
        green_shift: 8,
        blue_shift: 16,
    };

    pub fn read(r: &mut impl Read) -> Result<PixelFormat> {
        let mut buf = [0u8; 16];
        r.read_exact(&mut buf)?;
        Ok(PixelFormat {
            bits_per_pixel: buf[0],
            depth: buf[1],
            big_endian: buf[2] != 0,
            true_colour: buf[3] != 0,
            red_max: u16::from_be_bytes([buf[4], buf[5]]),
            green_max: u16::from_be_bytes([buf[6], buf[7]]),
            blue_max: u16::from_be_bytes([buf[8], buf[9]]),
            red_shift: buf[10],
            green_shift: buf[11],
            blue_shift: buf[12],
        })
    }

    pub fn write(&self, w: &mut impl Write) -> Result<()> {
        let mut buf = [0u8; 16];
        buf[0] = self.bits_per_pixel;
        buf[1] = self.depth;
        buf[2] = self.big_endian as u8;
        buf[3] = self.true_colour as u8;
        buf[4..6].copy_from_slice(&self.red_max.to_be_bytes());
        buf[6..8].copy_from_slice(&self.green_max.to_be_bytes());
        buf[8..10].copy_from_slice(&self.blue_max.to_be_bytes());
        buf[10] = self.red_shift;
        buf[11] = self.green_shift;
        buf[12] = self.blue_shift;
        w.write_all(&buf)
    }

    /// One line for the connection panel's diagnostics
    /// (`ConnectionFact.Field.SERVER_PIXELS`), in the shape RealVNC's own
    /// `getServerPixelFormat` uses.
    pub fn describe(&self) -> String {
        if !self.true_colour {
            return format!("depth {} ({} bpp) colour map", self.depth, self.bits_per_pixel);
        }
        let bits = |max: u16| (max as u32 + 1).trailing_zeros();
        format!(
            "depth {} ({} bpp) rgb{}{}{} {}-endian",
            self.depth,
            self.bits_per_pixel,
            bits(self.red_max),
            bits(self.green_max),
            bits(self.blue_max),
            if self.big_endian { "big" } else { "little" },
        )
    }
}

/// Trait-free helpers so the decoders can read pixels without carrying a
/// format around: everything runs in [`PixelFormat::NATIVE`].
pub trait PixelReader {
    /// One `PIXEL`, opaque.
    fn pixel(&mut self) -> Result<u32>;
}

impl<R: Read> PixelReader for Reader<R> {
    fn pixel(&mut self) -> Result<u32> {
        let mut b = [0u8; 4];
        self.read_exact(&mut b)?;
        Ok(pack(b[0], b[1], b[2]))
    }
}

/// `r, g, b` as they arrive, into a framebuffer word.
#[inline]
pub fn pack(r: u8, g: u8, b: u8) -> u32 {
    0xff00_0000 | ((b as u32) << 16) | ((g as u32) << 8) | (r as u32)
}

impl<W: Write> Writer<W> {
    pub fn pixel_format(&mut self, f: &PixelFormat) -> Result<()> {
        f.write(self)
    }
}
