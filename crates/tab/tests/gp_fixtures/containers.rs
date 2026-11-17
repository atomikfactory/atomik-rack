//! Writers for the containers: a zip archive (stored or deflated) and the `.gpx` file system with
//! its bit-compressed form.

use std::io::Write;

use flate2::Compression;
use flate2::write::DeflateEncoder;

use super::{W, crc32};

/// A zip with the given entries; `deflate` picks the method.
pub fn zip(entries: &[(&str, &[u8])], deflate: bool) -> Vec<u8> {
    let mut out = W::default();
    let mut central = W::default();
    for (name, data) in entries {
        let (method, packed) = if deflate {
            let mut e = DeflateEncoder::new(Vec::new(), Compression::default());
            e.write_all(data).unwrap();
            (8u16, e.finish().unwrap())
        } else {
            (0u16, data.to_vec())
        };
        let local = out.0.len() as i32;
        let crc = crc32(data);
        out.0.extend_from_slice(b"PK\x03\x04");
        out.i16(20);
        out.i16(0);
        out.i16(method as i16);
        out.i16(0);
        out.i16(0);
        out.i32(crc as i32);
        out.i32(packed.len() as i32);
        out.i32(data.len() as i32);
        out.i16(name.len() as i16);
        out.i16(0);
        out.0.extend_from_slice(name.as_bytes());
        out.0.extend_from_slice(&packed);

        central.0.extend_from_slice(b"PK\x01\x02");
        central.i16(20);
        central.i16(20);
        central.i16(0);
        central.i16(method as i16);
        central.i16(0);
        central.i16(0);
        central.i32(crc as i32);
        central.i32(packed.len() as i32);
        central.i32(data.len() as i32);
        central.i16(name.len() as i16);
        central.i16(0);
        central.i16(0);
        central.i16(0);
        central.i16(0);
        central.i32(0);
        central.i32(local);
        central.0.extend_from_slice(name.as_bytes());
    }
    let cd_offset = out.0.len() as i32;
    let cd_size = central.0.len() as i32;
    out.0.extend_from_slice(&central.0);
    out.0.extend_from_slice(b"PK\x05\x06");
    out.i16(0);
    out.i16(0);
    out.i16(entries.len() as i16);
    out.i16(entries.len() as i16);
    out.i32(cd_size);
    out.i32(cd_offset);
    out.i16(0);
    out.0
}

/// A `.gp` file: a zip with the score as `Content/score.gpif`.
pub fn gp(score: &str, deflate: bool) -> Vec<u8> {
    zip(
        &[
            ("VERSION", b"7"),
            ("Content/score.gpif", score.as_bytes()),
            ("Content/BinaryStylesheet", &[1, 2, 3]),
        ],
        deflate,
    )
}

const SECTOR: usize = 0x1000;

/// A `BCFS` image with the files; each gets an entry sector followed by its data sectors, and a
/// non-score file comes first so that the reader has to look.
pub fn bcfs(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut image = vec![0u8; SECTOR];
    image[..4].copy_from_slice(b"BCFS");
    for (name, data) in files {
        let entry = image.len();
        image.resize(entry + SECTOR, 0);
        let blocks = data.len().div_ceil(SECTOR);
        let first = image.len() / SECTOR;
        image[entry..entry + 4].copy_from_slice(&2i32.to_le_bytes());
        image[entry + 4..entry + 4 + name.len()].copy_from_slice(name.as_bytes());
        image[entry + 0x8C..entry + 0x90].copy_from_slice(&(data.len() as i32).to_le_bytes());
        for b in 0..blocks {
            let at = entry + 0x94 + 4 * b;
            image[at..at + 4].copy_from_slice(&((first + b) as i32).to_le_bytes());
        }
        image.extend_from_slice(data);
        image.resize(image.len().div_ceil(SECTOR) * SECTOR, 0);
    }
    image
}

/// Writes bits: `msb` first-bit-is-most-significant, `lsb` the other way.
#[derive(Default)]
struct BitW {
    bytes: Vec<u8>,
    n: usize,
}

impl BitW {
    fn bit(&mut self, b: u32) {
        if self.n.is_multiple_of(8) {
            self.bytes.push(0);
        }
        if b != 0 {
            let last = self.bytes.len() - 1;
            self.bytes[last] |= 0x80 >> (self.n % 8);
        }
        self.n += 1;
    }
    fn msb(&mut self, v: u32, n: u32) {
        for i in (0..n).rev() {
            self.bit((v >> i) & 1);
        }
    }
    fn lsb(&mut self, v: u32, n: u32) {
        for i in 0..n {
            self.bit((v >> i) & 1);
        }
    }
}

/// `BCFZ`: the image compressed. With `lz` the writer looks for repeats; without it only literals
/// are written (still a valid stream).
pub fn bcfz(image: &[u8], lz: bool) -> Vec<u8> {
    let mut bw = BitW::default();
    let mut pos = 0usize;
    let mut literals: Vec<u8> = Vec::new();
    let flush = |bw: &mut BitW, lits: &mut Vec<u8>| {
        for chunk in lits.chunks(3) {
            bw.bit(0);
            bw.lsb(chunk.len() as u32, 2);
            for b in chunk {
                bw.msb(u32::from(*b), 8);
            }
        }
        lits.clear();
    };
    while pos < image.len() {
        let mut best = (0usize, 0usize);
        if lz && pos > 0 {
            let window = pos.min(4095);
            for off in 1..=window {
                let mut len = 0;
                while len < off
                    && len < 4095
                    && pos + len < image.len()
                    && image[pos + len] == image[pos - off + len]
                {
                    len += 1;
                }
                if len > best.1 {
                    best = (off, len);
                }
            }
        }
        if best.1 >= 5 {
            flush(&mut bw, &mut literals);
            bw.bit(1);
            bw.msb(12, 4);
            bw.lsb(best.0 as u32, 12);
            bw.lsb(best.1 as u32, 12);
            pos += best.1;
        } else {
            literals.push(image[pos]);
            pos += 1;
        }
    }
    flush(&mut bw, &mut literals);
    let mut out = b"BCFZ".to_vec();
    out.extend_from_slice(&(image.len() as u32).to_le_bytes());
    out.extend_from_slice(&bw.bytes);
    out
}

/// A `.gpx` file with the score inside a compressed file system.
pub fn gpx(score: &str, compressed: bool, lz: bool) -> Vec<u8> {
    let image = bcfs(&[("misc.xml", b"<misc/>"), ("score.gpif", score.as_bytes())]);
    if compressed { bcfz(&image, lz) } else { image }
}
