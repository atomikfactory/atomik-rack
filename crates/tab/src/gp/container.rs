//! The `.gpx` container: a bit-compressed (`BCFZ`) or plain (`BCFS`) file system image that holds
//! the XML score as the entry `score.gpif` (and layout, style and sound files this reader ignores).
//!
//! `BCFZ` is the magic, a little-endian `u32` with the size of the image, and a bit stream (bits are
//! taken from the most significant end of each byte). Each token starts with one flag bit:
//!
//! * `1`: a copy from what was already written. A 4-bit width `w`, then an offset and a length of
//!   `w` bits each, **least significant bit first**; `min(offset, length)` bytes are copied from
//!   `offset` bytes back.
//! * `0`: literals. A count of 2 bits (least significant first), then that many bytes of 8 bits
//!   (most significant first).
//!
//! The decompressed image starts with `BCFS` and is cut in sectors of 4096 bytes. A sector that starts
//! with the number 2 is a file entry: the name at byte 4 (up to 127 characters), the size at byte
//! `0x8C` and from byte `0x94` the numbers of the sectors that hold the file, ended by a 0. Sector
//! numbers count from the start of the image.

use super::{GpError, limits};

const SECTOR: usize = 0x1000;
const SCORE_NAME: &str = "score.gpif";

/// Finds `score.gpif` in a `BCFZ` or `BCFS` file.
pub(super) fn extract_score(bytes: &[u8]) -> Result<Vec<u8>, GpError> {
    if bytes.starts_with(b"BCFZ") {
        find_score(&decompress(bytes)?)
    } else {
        find_score(bytes)
    }
}

/// Reads bits from the most significant end of each byte.
struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Bits<'_> {
    fn bit(&mut self) -> Option<u32> {
        let byte = *self.data.get(self.pos / 8)?;
        let b = (byte >> (7 - self.pos % 8)) & 1;
        self.pos += 1;
        Some(u32::from(b))
    }

    /// `n` bits, the first one read is the most significant.
    fn msb(&mut self, n: u32) -> Option<u32> {
        let mut v = 0;
        for _ in 0..n {
            v = (v << 1) | self.bit()?;
        }
        Some(v)
    }

    /// `n` bits, the first one read is the least significant.
    fn lsb(&mut self, n: u32) -> Option<u32> {
        let mut v = 0;
        for i in 0..n {
            v |= self.bit()? << i;
        }
        Some(v)
    }
}

/// Undoes the bit compression of a `BCFZ` file.
pub(super) fn decompress(bytes: &[u8]) -> Result<Vec<u8>, GpError> {
    let short = || GpError::Container("the compressed data ends early".into());
    let size = bytes.get(4..8).ok_or_else(short)?;
    let expected = u32::from_le_bytes([size[0], size[1], size[2], size[3]]) as usize;
    if expected > limits::MAX_DECOMPRESSED {
        return Err(GpError::TooBig(limits::MAX_DECOMPRESSED));
    }
    let mut bits = Bits {
        data: &bytes[8..],
        pos: 0,
    };
    let mut out: Vec<u8> = Vec::with_capacity(expected.min(8 * 1024 * 1024));
    while out.len() < expected {
        match bits.bit().ok_or_else(short)? {
            1 => {
                let w = bits.msb(4).ok_or_else(short)?;
                let offset = bits.lsb(w).ok_or_else(short)? as usize;
                let len = bits.lsb(w).ok_or_else(short)? as usize;
                if offset == 0 || offset > out.len() {
                    return Err(GpError::Container("a copy points before the start".into()));
                }
                let n = offset.min(len);
                if out.len() + n > limits::MAX_DECOMPRESSED {
                    return Err(GpError::TooBig(limits::MAX_DECOMPRESSED));
                }
                let from = out.len() - offset;
                out.extend_from_within(from..from + n);
            }
            _ => {
                let n = bits.lsb(2).ok_or_else(short)?;
                for _ in 0..n {
                    out.push(bits.msb(8).ok_or_else(short)? as u8);
                }
            }
        }
    }
    Ok(out)
}

fn int_at(data: &[u8], pos: usize) -> Option<i32> {
    let b = data.get(pos..pos.checked_add(4)?)?;
    Some(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// Looks through the sectors of a `BCFS` image for the score entry.
fn find_score(image: &[u8]) -> Result<Vec<u8>, GpError> {
    if !image.starts_with(b"BCFS") {
        return Err(GpError::Container(
            "the file system image is not recognised".into(),
        ));
    }
    let mut off = SECTOR;
    let mut looked = 0;
    while off + SECTOR <= image.len() && looked < limits::MAX_ENTRIES {
        looked += 1;
        if int_at(image, off) == Some(2) {
            let name_field = &image[off + 4..off + 4 + 127];
            let end = name_field
                .iter()
                .position(|b| *b == 0)
                .unwrap_or(name_field.len());
            if &name_field[..end] == SCORE_NAME.as_bytes() {
                return read_entry(image, off);
            }
        }
        off += SECTOR;
    }
    Err(GpError::NoScore)
}

fn read_entry(image: &[u8], off: usize) -> Result<Vec<u8>, GpError> {
    let size = int_at(image, off + 0x8C)
        .and_then(|s| usize::try_from(s).ok())
        .ok_or_else(|| GpError::Container("an entry has an impossible size".into()))?;
    if size > limits::MAX_DECOMPRESSED {
        return Err(GpError::TooBig(limits::MAX_DECOMPRESSED));
    }
    let mut data = Vec::with_capacity(size.min(image.len()));
    let mut i = 0;
    while off + 0x94 + 4 * i + 4 <= off + SECTOR && data.len() < size {
        let block = int_at(image, off + 0x94 + 4 * i).unwrap_or(0);
        if block <= 0 {
            break;
        }
        let start = (block as usize)
            .checked_mul(SECTOR)
            .filter(|s| *s < image.len())
            .ok_or_else(|| GpError::Container("an entry points outside the file".into()))?;
        let end = (start + SECTOR).min(image.len());
        data.extend_from_slice(&image[start..end]);
        i += 1;
    }
    if data.len() < size {
        return Err(GpError::Container(
            "the score is shorter than its entry says".into(),
        ));
    }
    data.truncate(size);
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bits_read_from_the_top_of_the_byte() {
        let mut b = Bits {
            data: &[0b1010_0001, 0b1000_0000],
            pos: 0,
        };
        assert_eq!(b.bit(), Some(1));
        assert_eq!(b.msb(3), Some(0b010));
        assert_eq!(b.lsb(4), Some(0b1000));
        assert_eq!(b.msb(2), Some(0b10));
    }

    #[test]
    fn short_or_looping_input_is_refused() {
        assert!(decompress(b"BCFZ").is_err());
        // Expects 100 bytes and ends at once.
        assert!(decompress(&[b'B', b'C', b'F', b'Z', 100, 0, 0, 0]).is_err());
        // A copy at the very start has nothing to copy from.
        let mut d = vec![b'B', b'C', b'F', b'Z', 10, 0, 0, 0];
        d.extend_from_slice(&[0b1010_0100, 0b1000_0000]);
        assert!(decompress(&d).is_err());
    }

    #[test]
    fn a_huge_size_is_refused_before_anything_is_allocated() {
        let d = [b'B', b'C', b'F', b'Z', 0xFF, 0xFF, 0xFF, 0xFF, 0];
        assert_eq!(
            decompress(&d),
            Err(GpError::TooBig(limits::MAX_DECOMPRESSED))
        );
    }

    #[test]
    fn not_a_container() {
        assert!(matches!(find_score(b"nope"), Err(GpError::Container(_))));
        assert_eq!(find_score(b"BCFS"), Err(GpError::NoScore));
    }
}
