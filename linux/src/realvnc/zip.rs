// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! Just enough of the zip format to read two named files out of an APK: the
//! end of central directory record, the central directory, and each entry's
//! local header, with the entries stored or deflated. No zip64, which an
//! APK of a few megabytes never needs; such an archive is not read.

use std::io::Read;

/// One file in the archive.
pub struct Entry<'a> {
    method: u16,
    data: &'a [u8],
    /// The size the central directory gives it once read.
    pub size: u64,
}

impl Entry<'_> {
    /// Its contents, inflated where they were deflated, up to `limit`
    /// bytes, which the size in the central directory cannot exceed either.
    pub fn reader(&self, limit: u64) -> Result<Box<dyn Read + '_>, String> {
        if self.size > limit {
            return Err(format!("it is {} bytes", self.size));
        }
        Ok(match self.method {
            0 => Box::new(self.data.take(limit)),
            8 => Box::new(flate2::read::DeflateDecoder::new(self.data).take(limit)),
            other => return Err(format!("compression method {other}")),
        })
    }
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// The entry named `name` in the archive `zip`, `Ok(None)` where there is
/// none, and an error where `zip` is not an archive this can read.
pub fn find<'a>(zip: &'a [u8], name: &str) -> Result<Option<Entry<'a>>, String> {
    // The end record is the last thing in the file, followed by a comment
    // of at most 65535 bytes.
    const END: u32 = 0x0605_4b50;
    let earliest = zip.len().saturating_sub(22 + 0xffff);
    let end = (earliest..zip.len().saturating_sub(21))
        .rev()
        .find(|&at| u32_at(zip, at) == Some(END))
        .ok_or("no end of central directory")?;
    let count = u16_at(zip, end + 10).ok_or("truncated")?;
    let mut at = u32_at(zip, end + 16).ok_or("truncated")? as usize;
    for _ in 0..count {
        if u32_at(zip, at) != Some(0x0201_4b50) {
            return Err("bad central directory".into());
        }
        let field = |offset| u16_at(zip, at + offset).ok_or("truncated");
        let method = field(10)?;
        let compressed = u32_at(zip, at + 20).ok_or("truncated")? as usize;
        let size = u32_at(zip, at + 24).ok_or("truncated")?;
        let (name_len, extra_len, comment_len) = (field(28)? as usize, field(30)? as usize, field(32)? as usize);
        let local = u32_at(zip, at + 42).ok_or("truncated")? as usize;
        let this = zip.get(at + 46..at + 46 + name_len).ok_or("truncated")?;
        at += 46 + name_len + extra_len + comment_len;
        if this != name.as_bytes() {
            continue;
        }
        if u32_at(zip, local) != Some(0x0403_4b50) {
            return Err("bad local header".into());
        }
        // The local header's own name and extra field lengths, which can
        // differ from the central directory's.
        let start = local
            + 30
            + u16_at(zip, local + 26).ok_or("truncated")? as usize
            + u16_at(zip, local + 28).ok_or("truncated")? as usize;
        let data = zip.get(start..start + compressed).ok_or("truncated")?;
        return Ok(Some(Entry {
            method,
            data,
            size: size.into(),
        }));
    }
    Ok(None)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Write;

    /// A zip of `files`, each as `(name, contents, deflated)`, with
    /// `comment` after the end record.
    pub fn zip(files: &[(&str, &[u8], bool)], comment: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, contents, deflated) in files {
            let data = if *deflated {
                let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
                e.write_all(contents).unwrap();
                e.finish().unwrap()
            } else {
                contents.to_vec()
            };
            let method: u16 = if *deflated { 8 } else { 0 };
            let offset = out.len() as u32;
            out.extend(0x0403_4b50u32.to_le_bytes());
            out.extend([20, 0, 0, 0]);
            out.extend(method.to_le_bytes());
            out.extend([0; 8]); // time, date, CRC (not checked here)
            out.extend((data.len() as u32).to_le_bytes());
            out.extend((contents.len() as u32).to_le_bytes());
            out.extend((name.len() as u16).to_le_bytes());
            out.extend(3u16.to_le_bytes());
            out.extend(name.as_bytes());
            out.extend([0; 3]);
            out.extend(&data);
            central.extend(0x0201_4b50u32.to_le_bytes());
            central.extend([20, 0, 20, 0, 0, 0]);
            central.extend(method.to_le_bytes());
            central.extend([0; 8]);
            central.extend((data.len() as u32).to_le_bytes());
            central.extend((contents.len() as u32).to_le_bytes());
            central.extend((name.len() as u16).to_le_bytes());
            central.extend([0; 12]);
            central.extend(offset.to_le_bytes());
            central.extend(name.as_bytes());
        }
        let start = out.len() as u32;
        out.extend(&central);
        out.extend(0x0605_4b50u32.to_le_bytes());
        out.extend([0; 4]);
        out.extend((files.len() as u16).to_le_bytes());
        out.extend((files.len() as u16).to_le_bytes());
        out.extend((central.len() as u32).to_le_bytes());
        out.extend(start.to_le_bytes());
        out.extend((comment.len() as u16).to_le_bytes());
        out.extend(comment);
        out
    }

    fn read(zip: &[u8], name: &str) -> Option<Vec<u8>> {
        let entry = find(zip, name).unwrap()?;
        let mut out = Vec::new();
        entry.reader(1 << 20).unwrap().read_to_end(&mut out).unwrap();
        Some(out)
    }

    #[test]
    fn stored_and_deflated_entries_are_read_by_name() {
        let archive = zip(
            &[("a", b"first", false), ("lib/x86_64/b.so", &[7; 5000], true), ("c", b"", false)],
            b"a comment",
        );
        assert_eq!(read(&archive, "a").unwrap(), b"first");
        assert_eq!(read(&archive, "lib/x86_64/b.so").unwrap(), vec![7; 5000]);
        assert_eq!(read(&archive, "c").unwrap(), b"");
        assert!(read(&archive, "lib/x86_64/a").is_none());
    }

    #[test]
    fn what_is_not_an_archive_or_too_large_is_refused() {
        assert!(find(b"<html>not a zip</html>", "a").is_err());
        let archive = zip(&[("a", &[1; 100], false)], b"");
        assert!(find(&archive[..archive.len() - 30], "a").is_err());
        assert!(find(&archive, "a").unwrap().unwrap().reader(99).is_err());
    }
}
