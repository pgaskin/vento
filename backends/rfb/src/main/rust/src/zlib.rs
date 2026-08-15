//! The persistent inflate stream ZRLE is carried in.

use crate::error::{Error, Result};
use flate2::{Decompress, FlushDecompress, Status};

/// One zlib stream, kept across rectangles.
///
/// That persistence is the point and the trap: the server compresses every ZRLE
/// rectangle into the *same* stream, flushing between them, so its dictionary
/// carries over and a rectangle decoded out of a fresh `Decompress` is
/// gibberish. There is exactly one of these per session, and it lives as long
/// as the connection does.
pub struct ZlibStream {
    inflate: Decompress,
    out: Vec<u8>,
}

/// How much room to make available per inflate call.
const CHUNK: usize = 64 * 1024;

impl ZlibStream {
    pub fn new() -> ZlibStream {
        ZlibStream {
            inflate: Decompress::new(true),
            out: Vec::new(),
        }
    }

    /// Inflate one rectangle's worth of input, whole.
    ///
    /// The decompressed length is not on the wire — it is implied by the tiles
    /// — so this runs until the input is spent and nothing more comes out,
    /// which is well-defined only because the server ends each rectangle with a
    /// sync flush.
    ///
    /// `limit` is the most the caller could possibly consume. Bounding the
    /// input is not enough to bound this: sixty-four megabytes of zeros inflate
    /// to about sixty-four gigabytes, and the allocation that fails aborts.
    pub fn inflate(&mut self, input: &[u8], limit: usize) -> Result<&[u8]> {
        self.out.clear();
        let mut consumed = 0usize;
        loop {
            if self.out.len() > limit {
                return Err(Error::Protocol(format!(
                    "zlib produced more than the {limit} bytes the rectangle can hold"
                )));
            }
            self.out.reserve(CHUNK);
            let in_before = self.inflate.total_in();
            let out_before = self.inflate.total_out();
            let status = self
                .inflate
                .decompress_vec(&input[consumed..], &mut self.out, FlushDecompress::None)
                .map_err(|e| Error::Protocol(format!("zlib: {e}")))?;
            consumed += (self.inflate.total_in() - in_before) as usize;
            let produced = self.inflate.total_out() - out_before;
            if status == Status::StreamEnd {
                break;
            }
            if consumed >= input.len() && produced == 0 {
                break;
            }
        }
        if consumed != input.len() {
            return Err(Error::Protocol(format!(
                "zlib left {} of {} bytes",
                input.len() - consumed,
                input.len()
            )));
        }
        Ok(&self.out)
    }
}

impl Default for ZlibStream {
    fn default() -> ZlibStream {
        ZlibStream::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compression, Compress, FlushCompress};

    /// Two rectangles down one stream: the second only decodes if the first
    /// left its dictionary behind.
    #[test]
    fn stream_persists_across_rectangles() {
        let mut deflate = Compress::new(Compression::default(), true);
        let mut stream = ZlibStream::new();
        let mut compress = |data: &[u8]| {
            let mut out = Vec::new();
            let mut buf = vec![0u8; data.len() + 1024];
            let before_in = deflate.total_in();
            let before_out = deflate.total_out();
            deflate
                .compress(data, &mut buf, FlushCompress::Sync)
                .unwrap();
            assert_eq!(deflate.total_in() - before_in, data.len() as u64);
            let n = (deflate.total_out() - before_out) as usize;
            out.extend_from_slice(&buf[..n]);
            out
        };

        let first = b"the quick brown fox jumps over the lazy dog".repeat(4);
        let second = b"the quick brown fox jumps over the lazy dog".repeat(4);
        let a = compress(&first);
        let b = compress(&second);
        assert_eq!(stream.inflate(&a, 1 << 20).unwrap(), &first[..]);
        assert_eq!(stream.inflate(&b, 1 << 20).unwrap(), &second[..]);
        assert!(b.len() < a.len(), "the dictionary carried over");
    }
}
