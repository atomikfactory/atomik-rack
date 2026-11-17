//! A small zip reader for `.gp` files (Guitar Pro 7 and later), which are zip archives with the
//! score as `Content/score.gpif`.
//!
//! Only what that needs: the central directory, stored and deflated entries, no encryption, no
//! multi-disk archives. The directory is the authority for names and sizes (a data descriptor after the
//! entry may carry zeros in the local header). The deflate stream is read through a size limit, so
//! an entry that claims to be small and inflates to gigabytes is refused when it passes the cap.

use std::io::Read;

use flate2::read::DeflateDecoder;

use super::{GpError, limits};

const EOCD: [u8; 4] = [b'P', b'K', 5, 6];
const CENTRAL: [u8; 4] = [b'P', b'K', 1, 2];
const LOCAL: [u8; 4] = [b'P', b'K', 3, 4];

fn u16_at(d: &[u8], p: usize) -> Option<usize> {
    let b = d.get(p..p.checked_add(2)?)?;
    Some(usize::from(u16::from_le_bytes([b[0], b[1]])))
}

fn u32_at(d: &[u8], p: usize) -> Option<usize> {
    let b = d.get(p..p.checked_add(4)?)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
}

/// One entry of the central directory.
struct Entry<'a> {
    name: &'a [u8],
    method: usize,
    compressed: usize,
    local: usize,
}

/// The entries of the archive, at most [`limits::MAX_ENTRIES`].
fn entries(data: &[u8]) -> Result<Vec<Entry<'_>>, GpError> {
    let bad = || GpError::Container("the zip directory is damaged".into());
    // The end-of-directory record is in the last 64 KiB (the comment can be that long).
    let from = data.len().saturating_sub(22 + 65_535);
    let eocd = (from..=data.len().saturating_sub(22))
        .rev()
        .find(|&p| data.get(p..p + 4) == Some(&EOCD[..]))
        .ok_or_else(|| GpError::Container("this is not a zip file".into()))?;
    let count = u16_at(data, eocd + 10).ok_or_else(bad)?;
    let mut p = u32_at(data, eocd + 16).ok_or_else(bad)?;
    let mut out = Vec::with_capacity(count.min(limits::MAX_ENTRIES));
    for _ in 0..count.min(limits::MAX_ENTRIES) {
        if data.get(p..p + 4) != Some(&CENTRAL[..]) {
            return Err(bad());
        }
        let method = u16_at(data, p + 10).ok_or_else(bad)?;
        let compressed = u32_at(data, p + 20).ok_or_else(bad)?;
        let name_len = u16_at(data, p + 28).ok_or_else(bad)?;
        let extra_len = u16_at(data, p + 30).ok_or_else(bad)?;
        let comment_len = u16_at(data, p + 32).ok_or_else(bad)?;
        let local = u32_at(data, p + 42).ok_or_else(bad)?;
        let name = data.get(p + 46..p + 46 + name_len).ok_or_else(bad)?;
        out.push(Entry {
            name,
            method,
            compressed,
            local,
        });
        p += 46 + name_len + extra_len + comment_len;
    }
    Ok(out)
}

/// The score of a `.gp` file: `Content/score.gpif`.
pub(super) fn extract_score(data: &[u8]) -> Result<Vec<u8>, GpError> {
    let list = entries(data)?;
    let entry = list
        .iter()
        .find(|e| e.name.eq_ignore_ascii_case(b"Content/score.gpif"))
        .or_else(|| {
            list.iter()
                .find(|e| e.name.to_ascii_lowercase().ends_with(b"score.gpif"))
        })
        .ok_or(GpError::NoScore)?;
    read_entry(data, entry)
}

fn read_entry(data: &[u8], e: &Entry<'_>) -> Result<Vec<u8>, GpError> {
    let bad = || GpError::Container("a zip entry is damaged".into());
    if data.get(e.local..e.local + 4) != Some(&LOCAL[..]) {
        return Err(bad());
    }
    let name_len = u16_at(data, e.local + 26).ok_or_else(bad)?;
    let extra_len = u16_at(data, e.local + 28).ok_or_else(bad)?;
    let start = e.local + 30 + name_len + extra_len;
    let packed = data
        .get(start..start.checked_add(e.compressed).ok_or_else(bad)?)
        .ok_or_else(bad)?;
    let cap = limits::MAX_DECOMPRESSED;
    match e.method {
        0 => {
            if packed.len() > cap {
                return Err(GpError::TooBig(cap));
            }
            Ok(packed.to_vec())
        }
        8 => {
            let mut out = Vec::new();
            DeflateDecoder::new(packed)
                .take(cap as u64 + 1)
                .read_to_end(&mut out)
                .map_err(|_| bad())?;
            if out.len() > cap {
                return Err(GpError::TooBig(cap));
            }
            Ok(out)
        }
        m => Err(GpError::Container(format!(
            "a zip entry uses compression method {m}, which is not supported"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_a_zip() {
        assert!(matches!(
            extract_score(b"PK\x03\x04 junk"),
            Err(GpError::Container(_))
        ));
        assert!(matches!(extract_score(&[]), Err(GpError::Container(_))));
    }
}
