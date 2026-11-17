//! A minimal zip writer for `.mxl` files: two entries, the container description and the score,
//! deflated.

use std::io::Write;

use flate2::Compression;
use flate2::write::DeflateEncoder;

const CONTAINER: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<container>\n  <rootfiles>\n    <rootfile full-path=\"score.musicxml\" \
media-type=\"application/vnd.recordare.musicxml+xml\"/>\n  </rootfiles>\n</container>\n";

struct Entry<'a> {
    name: &'a str,
    data: &'a [u8],
    crc: u32,
    packed: Vec<u8>,
    offset: u32,
}

fn le16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn le32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn deflate(data: &[u8]) -> Vec<u8> {
    let mut enc = DeflateEncoder::new(Vec::new(), Compression::default());
    // Writing into a `Vec` cannot fail.
    let _ = enc.write_all(data);
    enc.finish().unwrap_or_default()
}

/// The bytes of an `.mxl` archive holding `score` as `score.musicxml`.
pub(super) fn mxl(score: &[u8]) -> Vec<u8> {
    let parts: [(&str, &[u8]); 2] = [
        ("META-INF/container.xml", CONTAINER.as_bytes()),
        ("score.musicxml", score),
    ];
    let mut entries: Vec<Entry<'_>> = parts
        .iter()
        .map(|(name, data)| {
            let mut crc = flate2::Crc::new();
            crc.update(data);
            Entry {
                name,
                data,
                crc: crc.sum(),
                packed: deflate(data),
                offset: 0,
            }
        })
        .collect();
    let mut out = Vec::new();
    for e in &mut entries {
        e.offset = out.len() as u32;
        out.extend_from_slice(b"PK\x03\x04");
        le16(&mut out, 20); // version needed
        le16(&mut out, 0); // flags
        le16(&mut out, 8); // deflate
        le16(&mut out, 0); // time
        le16(&mut out, 0x21); // date: 1980-01-01
        le32(&mut out, e.crc);
        le32(&mut out, e.packed.len() as u32);
        le32(&mut out, e.data.len() as u32);
        le16(&mut out, e.name.len() as u16);
        le16(&mut out, 0);
        out.extend_from_slice(e.name.as_bytes());
        out.extend_from_slice(&e.packed);
    }
    let dir_start = out.len() as u32;
    for e in &entries {
        out.extend_from_slice(b"PK\x01\x02");
        le16(&mut out, 20); // version made by
        le16(&mut out, 20); // version needed
        le16(&mut out, 0);
        le16(&mut out, 8);
        le16(&mut out, 0);
        le16(&mut out, 0x21);
        le32(&mut out, e.crc);
        le32(&mut out, e.packed.len() as u32);
        le32(&mut out, e.data.len() as u32);
        le16(&mut out, e.name.len() as u16);
        le16(&mut out, 0); // extra
        le16(&mut out, 0); // comment
        le16(&mut out, 0); // disk
        le16(&mut out, 0); // internal attributes
        le32(&mut out, 0); // external attributes
        le32(&mut out, e.offset);
        out.extend_from_slice(e.name.as_bytes());
    }
    let dir_len = out.len() as u32 - dir_start;
    out.extend_from_slice(b"PK\x05\x06");
    le16(&mut out, 0);
    le16(&mut out, 0);
    le16(&mut out, entries.len() as u16);
    le16(&mut out, entries.len() as u16);
    le32(&mut out, dir_len);
    le32(&mut out, dir_start);
    le16(&mut out, 0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gp::zip;

    #[test]
    fn the_archive_reads_back() {
        let score = b"<score-partwise/>".repeat(100);
        let z = mxl(&score);
        assert_eq!(
            zip::names(&z).unwrap(),
            ["META-INF/container.xml", "score.musicxml"]
        );
        assert_eq!(zip::read_named(&z, "score.musicxml").unwrap(), score);
        assert!(zip::read_named(&z, "META-INF/container.xml").is_ok());
    }
}
