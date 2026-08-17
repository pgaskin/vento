// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The display channel: what the guest's screen is made of.
//!
//! SPICE draws in two ways at once and both are here. **Draw commands** carry
//! an image — one of six encodings — and a box to put it in, which is the
//! desktop. **Streams** are the part no other protocol in this app has: the
//! server notices a region behaving like video, declares it a stream, and sends
//! MJPEG or H.264 for that rectangle until it stops, with the framebuffer under
//! it keeping its own contents. A stream's rectangle is a damaged rectangle
//! like any other, which is why the seam took it without change.
//!
//! Only **surface 0** is drawn. The others are off-screen bitmaps a guest's
//! driver composes into, and a client that ignores them draws a desktop that is
//! right wherever the driver did not use one — which for QXL under the guests
//! this was built against is all of it. What arrives for another surface is
//! counted and logged once, so a guest that does use them says so rather than
//! quietly painting nothing.

use crate::client::Handler;
use crate::error::Result;
use crate::framebuffer::Framebuffer;
use crate::images::Images;
use shakenfist_spice_compression::{JpegDecoderRsDecoder, VideoDecoder, video};
use shakenfist_spice_protocol::constants::display_server;
use shakenfist_spice_protocol::messages::{
    DrawBase, ImageDescriptor, Notify, SpiceBrush, SpiceOpaque, SurfaceCreate,
};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// One live video stream: where it goes and what decodes it.
struct Stream {
    /// The destination in desktop coordinates, which is where its frames land
    /// whatever size they arrive at.
    dest: (usize, usize, usize, usize),
    decoder: Box<dyn VideoDecoder>,
}

pub struct Display {
    framebuffer: Arc<RwLock<Framebuffer>>,
    images: Images,
    streams: HashMap<u32, Stream>,
    /// The layout the server published, if it published one: `MONITORS_CONFIG`
    /// is how a multi-head guest says which part of this surface is which
    /// screen. Empty means it has not said, which is not the same as one.
    monitors: Vec<(i32, i32, i32, i32)>,
    /// Whether the desktop has a size yet, since a surface arrives after the
    /// channel and everything above waits for it.
    ready: bool,
    /// Whether anything has been drawn since the caller last asked, which is
    /// what decides whether a batch of messages ends in a repaint.
    dirty: bool,
    /// Draws for a surface this client does not keep, which is a fact about
    /// the guest's driver rather than an error — and worth a row in the panel
    /// where it happens, since it is the one thing that would make the picture
    /// incomplete without anything looking wrong.
    off_surface: u64,
}

impl Display {
    pub fn new(framebuffer: Arc<RwLock<Framebuffer>>, cache_bytes: usize) -> Display {
        Display {
            framebuffer,
            images: Images::new(cache_bytes),
            streams: HashMap::new(),
            monitors: Vec::new(),
            ready: false,
            dirty: false,
            off_surface: 0,
        }
    }

    pub fn monitors(&self) -> &[(i32, i32, i32, i32)] {
        &self.monitors
    }

    pub fn encodings(&self) -> Vec<&'static str> {
        self.images.encodings()
    }

    pub fn streams(&self) -> usize {
        self.streams.len()
    }

    /// Whether the picture moved since this was last asked, which the session
    /// turns into one `frameEnd` for a burst of draws rather than one each.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    /// Draws this client dropped because they were for another surface.
    pub fn off_surface(&self) -> u64 {
        self.off_surface
    }

    /// One message off the display channel. Damage is reported per draw rather
    /// than per batch: the caller ends a frame when the queue drains, which is
    /// what makes a screenful of small draws one repaint instead of eleven.
    pub async fn message(
        &mut self,
        message_type: u16,
        body: &[u8],
        handler: &mut dyn Handler,
    ) -> Result<()> {
        match message_type {
            display_server::MODE => {
                // The pre-surface way of saying how big the screen is, which a
                // guest with no QXL driver — a BIOS, a bootloader, an installer
                // — is the whole reason this protocol is worth having.
                if body.len() >= 12 {
                    let width = u32::from_le_bytes(body[0..4].try_into().unwrap()) as usize;
                    let height = u32::from_le_bytes(body[4..8].try_into().unwrap()) as usize;
                    self.reset_surface(width, height, handler);
                }
            }
            display_server::SURFACE_CREATE => {
                let create = SurfaceCreate::read(body)?;
                if create.surface_id == 0 {
                    self.reset_surface(create.width as usize, create.height as usize, handler);
                }
            }
            display_server::SURFACE_DESTROY => {}
            display_server::RESET | display_server::INVAL_ALL_PIXMAPS => {
                // Everything this end remembers about what the server has sent
                // before is now wrong, and the server will send it again.
                self.images.forget();
            }
            display_server::DRAW_COPY | display_server::DRAW_BLEND => {
                // SpiceCopy, and SpiceBlend has the identical layout: the image
                // offset, the source rectangle inside it, then the rop, the
                // scale mode and a mask this client does not apply.
                let base = DrawBase::read(body)?;
                let at = base.end_offset;
                let Some(offset) = read_u32(body, at) else {
                    return Ok(());
                };
                let src = (
                    read_u32(body, at + 8).unwrap_or(0),
                    read_u32(body, at + 4).unwrap_or(0),
                    read_u32(body, at + 16).unwrap_or(0),
                    read_u32(body, at + 12).unwrap_or(0),
                );
                self.draw_image(&base, body, offset as usize, src, handler)
                    .await;
            }
            display_server::DRAW_OPAQUE => {
                let base = DrawBase::read(body)?;
                let (opaque, _) = SpiceOpaque::read(&body[base.end_offset..])?;
                let src = (
                    opaque.src_left,
                    opaque.src_top,
                    opaque.src_right,
                    opaque.src_bottom,
                );
                self.draw_image(&base, body, opaque.src_bitmap as usize, src, handler)
                    .await;
            }
            display_server::DRAW_FILL => {
                let base = DrawBase::read(body)?;
                let (brush, _) = SpiceBrush::read(&body[base.end_offset..])?;
                // Only a solid brush is a fill; a pattern brush is a bitmap
                // this client does not tile, and painting its first colour over
                // the box would be worse than leaving the box alone.
                if let SpiceBrush::Solid { color } = brush {
                    self.fill(&base, argb(color), handler);
                }
            }
            display_server::DRAW_BLACKNESS => {
                let base = DrawBase::read(body)?;
                self.fill(&base, 0xff00_0000, handler);
            }
            display_server::DRAW_WHITENESS => {
                let base = DrawBase::read(body)?;
                self.fill(&base, 0xffff_ffff, handler);
            }
            display_server::COPY_BITS => {
                // The one draw that reads the framebuffer as well as writing
                // it: a source point, and the base's box as the destination.
                let base = DrawBase::read(body)?;
                let at = base.end_offset;
                let (Some(src_x), Some(src_y)) = (read_u32(body, at), read_u32(body, at + 4)) else {
                    return Ok(());
                };
                if base.surface_id != 0 {
                    self.off_surface += 1;
                    return Ok(());
                }
                let (x, y, w, h) = box_of(&base);
                self.framebuffer.write().unwrap().copy(
                    src_x as usize,
                    src_y as usize,
                    x,
                    y,
                    w,
                    h,
                );
                self.dirty = true;
                handler.damaged(x, y, w, h);
            }
            display_server::STREAM_CREATE => {
                self.stream_create(body);
            }
            display_server::STREAM_DATA | display_server::STREAM_DATA_SIZED => {
                self.stream_data(message_type, body, handler);
            }
            display_server::STREAM_DESTROY => {
                if let Some(id) = read_u32(body, 0) {
                    self.streams.remove(&id);
                }
            }
            display_server::STREAM_DESTROY_ALL => self.streams.clear(),
            display_server::MONITORS_CONFIG => self.monitors_config(body),
            display_server::NOTIFY => {
                let notify = Notify::read(body)?;
                log::info!("display: {}", notify.message);
            }
            other => {
                // Every draw this client does not do is a QXL command a guest
                // driver produces and QEMU passes on: ROP3, stroke, text and
                // composite. They are logged rather than counted because a
                // guest that sends one sends thousands.
                log::debug!("display: message {other} is not handled here");
            }
        }
        Ok(())
    }

    /// A new screen: the framebuffer is thrown away and the server draws the
    /// whole of the new one.
    fn reset_surface(&mut self, width: usize, height: usize, handler: &mut dyn Handler) {
        if width == 0 || height == 0 {
            return;
        }
        self.framebuffer.write().unwrap().resize(width, height);
        self.streams.clear();
        self.ready = true;
        self.dirty = true;
        handler.desktop_size(width, height);
    }

    pub fn ready(&self) -> bool {
        self.ready
    }

    async fn draw_image(
        &mut self,
        base: &DrawBase,
        body: &[u8],
        offset: usize,
        src: (u32, u32, u32, u32),
        handler: &mut dyn Handler,
    ) {
        if base.surface_id != 0 {
            self.off_surface += 1;
            return;
        }
        // Zero is a null image rather than one at the start of the message.
        if offset == 0 || offset + ImageDescriptor::SIZE > body.len() {
            return;
        }
        let Ok(descriptor) = ImageDescriptor::read(&body[offset..]) else {
            return;
        };
        let (x, y, w, h) = box_of(base);
        let data = &body[offset + ImageDescriptor::SIZE..];
        let Some(image) = self.images.draw(&descriptor, data, src, w, h).await else {
            return;
        };
        self.framebuffer
            .write()
            .unwrap()
            .blit(x, y, image.width, image.height, &image.pixels);
        self.dirty = true;
        handler.damaged(x, y, w, h);
    }

    fn fill(&mut self, base: &DrawBase, colour: u32, handler: &mut dyn Handler) {
        if base.surface_id != 0 {
            self.off_surface += 1;
            return;
        }
        let (x, y, w, h) = box_of(base);
        self.framebuffer.write().unwrap().fill(x, y, w, h, colour);
        self.dirty = true;
        handler.damaged(x, y, w, h);
    }

    /// `SpiceMsgDisplayStreamCreate`: a rectangle the server is about to send
    /// video for, and which codec.
    fn stream_create(&mut self, body: &[u8]) {
        if body.len() < 50 {
            return;
        }
        let id = read_u32(body, 4).unwrap_or(0);
        let codec = body[9];
        let dest = (
            read_u32(body, 38).unwrap_or(0) as usize, // left
            read_u32(body, 34).unwrap_or(0) as usize, // top
            read_u32(body, 46).unwrap_or(0) as usize, // right
            read_u32(body, 42).unwrap_or(0) as usize, // bottom
        );
        match video::for_stream(codec, Arc::new(JpegDecoderRsDecoder::new())) {
            Ok(decoder) => {
                log::info!(
                    "display: stream {id} is {} at ({},{})",
                    decoder.name(),
                    dest.0,
                    dest.1
                );
                self.streams.insert(
                    id,
                    Stream {
                        dest: (
                            dest.0,
                            dest.1,
                            dest.2.saturating_sub(dest.0),
                            dest.3.saturating_sub(dest.1),
                        ),
                        decoder,
                    },
                );
            }
            // H.264 is the one that gets here, because the decoder that would
            // take it is a C++ one this build patches out: the phone has one of
            // its own and putting the stream through it is 46c's. Until then
            // the region under the stream keeps whatever was drawn there, which
            // is a still picture rather than a hole.
            Err(e) => log::warn!("display: stream {id} is not decoded here: {e}"),
        }
    }

    fn stream_data(&mut self, message_type: u16, body: &[u8], handler: &mut dyn Handler) {
        let sized = message_type == display_server::STREAM_DATA_SIZED;
        let header = if sized { 36 } else { 12 };
        if body.len() < header {
            return;
        }
        let id = read_u32(body, 0).unwrap_or(0);
        let size = read_u32(body, if sized { 32 } else { 8 }).unwrap_or(0) as usize;
        let packet = &body[header..header + size.min(body.len() - header)];
        // A sized frame carries its own destination, which is how a stream
        // whose region has moved or been scaled says so.
        let dest = if sized {
            let (top, left, bottom, right) = (
                read_u32(body, 16).unwrap_or(0) as usize,
                read_u32(body, 20).unwrap_or(0) as usize,
                read_u32(body, 24).unwrap_or(0) as usize,
                read_u32(body, 28).unwrap_or(0) as usize,
            );
            Some((left, top, right.saturating_sub(left), bottom.saturating_sub(top)))
        } else {
            None
        };

        let Some(stream) = self.streams.get_mut(&id) else {
            return;
        };
        if let Some(dest) = dest {
            stream.dest = dest;
        }
        let (x, y, w, h) = stream.dest;
        match stream.decoder.decode(packet) {
            Ok(Some(frame)) => {
                let mut framebuffer = self.framebuffer.write().unwrap();
                framebuffer.blit(
                    x,
                    y,
                    frame.width as usize,
                    frame.height as usize,
                    &frame.rgba,
                );
                drop(framebuffer);
                self.dirty = true;
                // The stream says where its picture goes; what came out of the
                // decoder may be bigger, since a codec rounds up to whole
                // macroblocks and the rest is not part of the rectangle.
                handler.damaged(x, y, w.min(frame.width as usize), h.min(frame.height as usize));
            }
            Ok(None) => {}
            Err(e) => log::warn!("display: stream {id}: {e}"),
        }
    }

    /// `MONITORS_CONFIG`: how many heads this surface is divided into, and
    /// where each one is. The first protocol here that says so rather than
    /// leaving it to be inferred.
    fn monitors_config(&mut self, body: &[u8]) {
        if body.len() < 4 {
            return;
        }
        let count = u16::from_le_bytes(body[0..2].try_into().unwrap()) as usize;
        let mut heads = Vec::with_capacity(count);
        for i in 0..count {
            let at = 4 + i * 28;
            if at + 28 > body.len() {
                break;
            }
            let (width, height) = (
                read_u32(body, at + 8).unwrap_or(0) as i32,
                read_u32(body, at + 12).unwrap_or(0) as i32,
            );
            let (x, y) = (
                read_u32(body, at + 16).unwrap_or(0) as i32,
                read_u32(body, at + 20).unwrap_or(0) as i32,
            );
            if width > 0 && height > 0 {
                heads.push((x, y, width, height));
            }
        }
        if heads != self.monitors {
            log::info!("display: {} monitor(s)", heads.len());
            self.monitors = heads;
        }
    }
}

/// The destination box a draw names, as an origin and a size.
fn box_of(base: &DrawBase) -> (usize, usize, usize, usize) {
    let (left, top) = (base.left as usize, base.top as usize);
    (
        left,
        top,
        (base.right as usize).saturating_sub(left),
        (base.bottom as usize).saturating_sub(top),
    )
}

/// A brush colour is `0x00RRGGBB`; the framebuffer's word is `R, G, B, A` in
/// memory, which is the same value with its ends swapped and an alpha put on.
fn argb(colour: u32) -> u32 {
    let (r, g, b) = ((colour >> 16) & 0xff, (colour >> 8) & 0xff, colour & 0xff);
    0xff00_0000 | (b << 16) | (g << 8) | r
}

fn read_u32(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The framebuffer's word is the bitmap's, so a brush colour has to come
    /// out with red where red goes rather than where SPICE puts it.
    #[test]
    fn a_brush_colour_lands_the_right_way_round() {
        // SPICE's 0x00RRGGBB, and the word we store is R, G, B, A in memory.
        let word = argb(0x00_11_22_33);
        assert_eq!(word.to_le_bytes(), [0x11, 0x22, 0x33, 0xff]);
    }
}
