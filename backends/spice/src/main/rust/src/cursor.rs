//! The cursor channel: the shape the guest is drawing, on a connection of its
//! own.
//!
//! It is a channel rather than a message because SPICE keeps the pointer's
//! picture apart from the screen's — the cursor moves without the desktop
//! changing, and a client that owns the pointer draws it here rather than
//! waiting for a frame. What this end needs of it is the shape and the hotspot;
//! where the cursor is is the guest's business in server mouse mode and this
//! end's in client mode, and neither of them is this channel's.

use crate::client::Handler;
use crate::error::Result;
use shakenfist_spice_protocol::constants::cursor_server;
use shakenfist_spice_protocol::messages::{CursorInit, CursorSet, SpiceCursorHeader};
use std::collections::HashMap;

/// The shapes the server has asked this end to keep, by its own id: SPICE
/// sends a cursor once and then names it, so a client that does not keep them
/// gets a cursor that goes blank the second time a window is resized.
///
/// Bounded by count rather than by bytes, unlike the image cache: a cursor is
/// at most 256×256 and a guest has a few dozen, so the bound is there to stop
/// a hostile server rather than to fit a real one.
const MAX_SHAPES: usize = 64;

pub struct Cursors {
    shapes: HashMap<u64, Shape>,
    order: Vec<u64>,
}

struct Shape {
    width: i32,
    height: i32,
    hot_x: i32,
    hot_y: i32,
    /// The framebuffer's own word order, which is what the callback into Java
    /// takes: it does the swap to `Color`'s `0xAARRGGBB` itself.
    pixels: Vec<u32>,
}

impl Cursors {
    pub fn new() -> Cursors {
        Cursors {
            shapes: HashMap::new(),
            order: Vec::new(),
        }
    }

    pub fn message(
        &mut self,
        message_type: u16,
        body: &[u8],
        handler: &mut dyn Handler,
    ) -> Result<()> {
        match message_type {
            // Both carry a SpiceCursor; INIT has nine bytes of its own in front
            // of it and SET has five.
            cursor_server::INIT => self.cursor(&body[CursorInit::SIZE..], handler),
            cursor_server::SET => self.cursor(&body[CursorSet::SIZE..], handler),
            cursor_server::HIDE => handler.cursor(&[], 0, 0, 0, 0),
            cursor_server::INVALIDATE_ALL => {
                self.shapes.clear();
                self.order.clear();
            }
            _ => {}
        }
        Ok(())
    }

    fn cursor(&mut self, data: &[u8], handler: &mut dyn Handler) {
        let Ok(Some(header)) = SpiceCursorHeader::read(data) else {
            return; // FLAG_NONE, or a body too short to hold one
        };
        if header.flags & SpiceCursorHeader::FLAG_FROM_CACHE != 0 {
            if let Some(shape) = self.shapes.get(&header.unique_id) {
                handler.cursor(&shape.pixels, shape.width, shape.height, shape.hot_x, shape.hot_y);
            } else {
                log::warn!("cursor: {} is not one this end kept", header.unique_id);
            }
            return;
        }

        let Some(pixels) = pixels(&header, &data[SpiceCursorHeader::SIZE..]) else {
            return;
        };
        let shape = Shape {
            width: header.width as i32,
            height: header.height as i32,
            hot_x: header.hot_spot_x as i32,
            hot_y: header.hot_spot_y as i32,
            pixels,
        };
        handler.cursor(&shape.pixels, shape.width, shape.height, shape.hot_x, shape.hot_y);
        if header.flags & SpiceCursorHeader::FLAG_CACHE_ME != 0 {
            if self.order.len() >= MAX_SHAPES && let Some(oldest) = self.order.first().copied() {
                self.order.remove(0);
                self.shapes.remove(&oldest);
            }
            self.order.push(header.unique_id);
            self.shapes.insert(header.unique_id, shape);
        }
    }
}

impl Default for Cursors {
    fn default() -> Cursors {
        Cursors::new()
    }
}

/// A cursor's pixels as `R, G, B, A` words, which is the framebuffer's order
/// and what the callback into Java expects.
///
/// Three of SPICE's cursor types are 32 bits or 24 and are the ones a guest
/// with a driver sends; the monochrome ones belong to a text-mode guest and
/// are two bitplanes rather than pixels, which is a different shape of work
/// for a cursor nothing here has produced.
fn pixels(header: &SpiceCursorHeader, data: &[u8]) -> Option<Vec<u32>> {
    let count = (header.width as usize).checked_mul(header.height as usize)?;
    if count == 0 {
        return None;
    }
    let mut out = Vec::with_capacity(count);
    match header.cursor_type {
        // ALPHA: BGRA, and the alpha is the cursor's own shape.
        0 => {
            let bytes = data.get(..count * 4)?;
            for px in bytes.chunks_exact(4) {
                out.push(u32::from_le_bytes([px[2], px[1], px[0], px[3]]));
            }
        }
        // COLOR24: BGR, every pixel opaque.
        5 => {
            let bytes = data.get(..count * 3)?;
            for px in bytes.chunks_exact(3) {
                out.push(u32::from_le_bytes([px[2], px[1], px[0], 0xff]));
            }
        }
        // COLOR32: BGRX, where the fourth byte is padding rather than alpha.
        6 => {
            let bytes = data.get(..count * 4)?;
            for px in bytes.chunks_exact(4) {
                out.push(u32::from_le_bytes([px[2], px[1], px[0], 0xff]));
            }
        }
        other => {
            log::warn!("cursor: type {other} is not built here");
            return None;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(cursor_type: u8, width: u16, height: u16) -> SpiceCursorHeader {
        SpiceCursorHeader {
            flags: 0,
            unique_id: 1,
            cursor_type,
            width,
            height,
            hot_spot_x: 0,
            hot_spot_y: 0,
        }
    }

    /// The colour order is the one thing here that is silently wrong when it is
    /// wrong: a red cursor comes out blue and nothing else changes.
    #[test]
    fn an_alpha_cursor_comes_out_in_the_framebuffers_order() {
        // One pixel, BGRA on the wire: blue 0x11, green 0x22, red 0x33.
        let pixels = pixels(&header(0, 1, 1), &[0x11, 0x22, 0x33, 0x80]).unwrap();
        assert_eq!(pixels[0].to_le_bytes(), [0x33, 0x22, 0x11, 0x80]);
    }

    #[test]
    fn a_24_bit_cursor_is_opaque() {
        let pixels = pixels(&header(5, 1, 1), &[0x11, 0x22, 0x33]).unwrap();
        assert_eq!(pixels[0].to_le_bytes(), [0x33, 0x22, 0x11, 0xff]);
    }

    /// A truncated cursor is refused rather than read past — the header's size
    /// is the server's word and the body is what actually arrived.
    #[test]
    fn a_cursor_shorter_than_its_header_says_is_refused() {
        assert!(pixels(&header(0, 4, 4), &[0; 8]).is_none());
    }
}
