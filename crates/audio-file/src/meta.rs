//! Tempo metadata of an audio file: a bounded scan of the chunks of a WAV or AIFF file.
//!
//! The scan reads the chunk headers and the few chunks that carry loop information, never
//! more than [`MAX_CHUNKS`] headers and [`MAX_BODY`] bytes of one chunk, and never allocates
//! from a size the file states. It does not panic on any input: a file that is not RIFF/WAVE,
//! is truncated or lies about its sizes simply yields less (or nothing). It is separate from
//! decoding, so a file that cannot be decoded still reads as having no metadata.
//!
//! What is read:
//!
//! * `acid`: the loop's tempo, its beat count and the one-shot flag.
//! * `smpl`: the number of sample loops (a file with one is loop-like).
//! * `bext`: the description text, searched for a tempo written as `120 BPM`.
//!
//! In an AIFF or AIFF-C file (big-endian `FORM` chunks):
//!
//! * `basc`: the beat count of an Apple loop (a big-endian `u32` after a version `u32`); the
//!   tempo then follows from the length of the audio.
//! * `ANNO`, `NAME` and `COMT`: the text, searched for a tempo written as `120 BPM` (the
//!   comment chunk is read as a whole, its time stamps and marker ids skipped).
//! * `INST`: a sustain loop marks the file as loop-like.

use crate::error::Result;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Chunk headers one scan walks at most.
pub const MAX_CHUNKS: usize = 256;
/// Bytes read of one chunk at most.
pub const MAX_BODY: usize = 512;
/// The lowest tempo taken from a file, BPM.
pub const META_BPM_MIN: f64 = 20.0;
/// The highest tempo taken from a file, BPM.
pub const META_BPM_MAX: f64 = 999.0;

/// What a file says about its tempo and kind.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FileMeta {
    /// The tempo the file says it was made at, within [`META_BPM_MIN`]..=[`META_BPM_MAX`].
    pub tempo_bpm: Option<f64>,
    /// The number of beats the file says it holds (`acid`), at least 1.
    pub beats: Option<u32>,
    /// The file says it is a one-shot (the `acid` one-shot flag), not a loop.
    pub one_shot: bool,
    /// The file carries at least one sample loop (`smpl`).
    pub has_loop: bool,
}

impl FileMeta {
    /// True when the file says nothing a decision can use.
    pub fn is_empty(&self) -> bool {
        *self == FileMeta::default()
    }
}

/// Reads the metadata of the file at `path`; an unreadable or unrecognised file gives an
/// empty [`FileMeta`] (this never fails the import).
pub fn read_file_meta(path: &Path) -> FileMeta {
    let Ok(mut file) = File::open(path) else {
        return FileMeta::default();
    };
    let Ok(len) = file.metadata().map(|m| m.len()) else {
        return FileMeta::default();
    };
    scan_meta(&mut file, len).unwrap_or_default()
}

/// Reads the metadata of WAV bytes in memory.
pub fn parse_meta(bytes: &[u8]) -> FileMeta {
    scan_meta(&mut std::io::Cursor::new(bytes), bytes.len() as u64).unwrap_or_default()
}

/// Walks the chunks of `r` (`len` bytes long).
fn scan_meta<R: Read + Seek>(r: &mut R, len: u64) -> Result<FileMeta> {
    let mut meta = FileMeta::default();
    let mut head = [0u8; 12];
    r.seek(SeekFrom::Start(0))?;
    if len < 12 || r.read_exact(&mut head).is_err() {
        return Ok(meta);
    }
    if &head[0..4] == b"FORM" && (&head[8..12] == b"AIFF" || &head[8..12] == b"AIFC") {
        return scan_aiff(r, len);
    }
    if &head[0..4] != b"RIFF" || &head[8..12] != b"WAVE" {
        return Ok(meta);
    }
    let mut pos = 12u64;
    for _ in 0..MAX_CHUNKS {
        if pos.saturating_add(8) > len {
            break;
        }
        let mut h = [0u8; 8];
        r.seek(SeekFrom::Start(pos))?;
        if r.read_exact(&mut h).is_err() {
            break;
        }
        let id = [h[0], h[1], h[2], h[3]];
        let size = u64::from(u32::from_le_bytes([h[4], h[5], h[6], h[7]]));
        let body = pos + 8;
        let avail = len.saturating_sub(body).min(size);
        let take = usize::try_from(avail.min(MAX_BODY as u64)).unwrap_or(0);
        match &id {
            b"acid" | b"smpl" | b"bext" => {
                let mut buf = vec![0u8; take];
                if r.read_exact(&mut buf).is_err() {
                    break;
                }
                match &id {
                    b"acid" => read_acid(&buf, &mut meta),
                    b"smpl" => read_smpl(&buf, &mut meta),
                    _ => read_bext(&buf, &mut meta),
                }
            }
            _ => {}
        }
        // Chunks are padded to an even size.
        pos = body.saturating_add(size).saturating_add(size & 1);
    }
    Ok(meta)
}

/// Walks the chunks of an AIFF or AIFF-C file (big-endian sizes, padded to an even size).
fn scan_aiff<R: Read + Seek>(r: &mut R, len: u64) -> Result<FileMeta> {
    let mut meta = FileMeta::default();
    let mut pos = 12u64;
    for _ in 0..MAX_CHUNKS {
        if pos.saturating_add(8) > len {
            break;
        }
        let mut h = [0u8; 8];
        r.seek(SeekFrom::Start(pos))?;
        if r.read_exact(&mut h).is_err() {
            break;
        }
        let id = [h[0], h[1], h[2], h[3]];
        let size = u64::from(u32::from_be_bytes([h[4], h[5], h[6], h[7]]));
        let body = pos + 8;
        let avail = len.saturating_sub(body).min(size);
        let take = usize::try_from(avail.min(MAX_BODY as u64)).unwrap_or(0);
        if matches!(&id, b"basc" | b"ANNO" | b"NAME" | b"COMT" | b"INST") {
            let mut buf = vec![0u8; take];
            if r.read_exact(&mut buf).is_err() {
                break;
            }
            match &id {
                b"basc" => meta.beats = u32_be(&buf, 4).filter(|n| (1..=65_536).contains(n)),
                b"INST" => read_inst(&buf, &mut meta),
                b"COMT" => read_comments(&buf, &mut meta),
                _ => read_text_tempo(&buf, &mut meta),
            }
        }
        pos = body.saturating_add(size).saturating_add(size & 1);
    }
    Ok(meta)
}

fn u32_be(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4)
        .map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

/// `INST`: eight bytes of note data, then the play mode (`u16`) of the sustain loop: not zero
/// when the file loops.
fn read_inst(b: &[u8], meta: &mut FileMeta) {
    if b.get(8..10).is_some_and(|m| m != [0, 0]) {
        meta.has_loop = true;
    }
}

/// `COMT`: a comment count (`u16`), then per comment a time stamp (`u32`), a marker id (`u16`),
/// a text length (`u16`) and the text padded to an even size. The first comment that names a
/// tempo wins.
fn read_comments(b: &[u8], meta: &mut FileMeta) {
    let count = b
        .get(0..2)
        .map_or(0, |c| usize::from(u16::from_be_bytes([c[0], c[1]])));
    let mut at = 2usize;
    for _ in 0..count.min(64) {
        let Some(n) = b
            .get(at + 6..at + 8)
            .map(|c| usize::from(u16::from_be_bytes([c[0], c[1]])))
        else {
            return;
        };
        let end = (at + 8 + n).min(b.len());
        read_text_tempo(&b[at + 8..end], meta);
        at = at + 8 + n + (n & 1);
        if meta.tempo_bpm.is_some() {
            return;
        }
    }
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn plausible(bpm: f64) -> Option<f64> {
    (bpm.is_finite() && (META_BPM_MIN..=META_BPM_MAX).contains(&bpm)).then_some(bpm)
}

/// `acid`: flags (bit 0: one-shot), root note, two unused fields, beats, meter, tempo (f32).
fn read_acid(b: &[u8], meta: &mut FileMeta) {
    if b.len() < 24 {
        return;
    }
    let flags = u32_at(b, 0).unwrap_or(0);
    meta.one_shot = flags & 1 != 0;
    meta.beats = u32_at(b, 12).filter(|n| (1..=65_536).contains(n));
    let tempo = f32::from_bits(u32_at(b, 20).unwrap_or(0));
    meta.tempo_bpm = plausible(f64::from(tempo)).or(meta.tempo_bpm);
}

/// `smpl`: nine u32 fields, the ninth is the loop count.
fn read_smpl(b: &[u8], meta: &mut FileMeta) {
    if u32_at(b, 28).is_some_and(|n| n > 0) {
        meta.has_loop = true;
    }
}

/// `bext`: a 256 byte description first; a tempo is read from text like `120 BPM` or
/// `BPM: 120`.
fn read_bext(b: &[u8], meta: &mut FileMeta) {
    read_text_tempo(&b[..b.len().min(256)], meta);
}

/// A tempo written in `text` as `120 BPM` or `BPM: 120`, unless `meta` has one already.
fn read_text_tempo(text: &[u8], meta: &mut FileMeta) {
    if meta.tempo_bpm.is_some() {
        return;
    }
    let end = text.iter().position(|c| *c == 0).unwrap_or(text.len());
    let lower: Vec<u8> = text[..end].iter().map(u8::to_ascii_lowercase).collect();
    let Some(at) = lower.windows(3).position(|w| w == b"bpm") else {
        return;
    };
    let number = |digits: &[u8]| -> Option<f64> {
        let s: String = digits.iter().map(|c| char::from(*c)).collect();
        s.parse::<f64>().ok().and_then(plausible)
    };
    // The number before the unit, then the one after it.
    let before = &lower[..at];
    let trimmed_end = before
        .iter()
        .rposition(|c| !c.is_ascii_whitespace())
        .map_or(0, |i| i + 1);
    let start = before[..trimmed_end]
        .iter()
        .rposition(|c| !(c.is_ascii_digit() || *c == b'.'))
        .map_or(0, |i| i + 1);
    let found = number(&before[start..trimmed_end]).or_else(|| {
        let after = &lower[at + 3..];
        let skip = after
            .iter()
            .position(|c| c.is_ascii_digit())
            .unwrap_or(after.len());
        let len = after[skip..]
            .iter()
            .position(|c| !(c.is_ascii_digit() || *c == b'.'))
            .unwrap_or(after.len() - skip);
        number(&after[skip..skip + len])
    });
    meta.tempo_bpm = found;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = id.to_vec();
        v.extend((body.len() as u32).to_le_bytes());
        v.extend(body);
        if body.len() % 2 == 1 {
            v.push(0);
        }
        v
    }

    fn wav(chunks: &[Vec<u8>]) -> Vec<u8> {
        let body: Vec<u8> = chunks.concat();
        let mut v = b"RIFF".to_vec();
        v.extend((body.len() as u32 + 4).to_le_bytes());
        v.extend(b"WAVE");
        v.extend(body);
        v
    }

    fn acid(flags: u32, beats: u32, tempo: f32) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend(flags.to_le_bytes());
        b.extend(60u16.to_le_bytes());
        b.extend(0u16.to_le_bytes());
        b.extend(0f32.to_le_bytes());
        b.extend(beats.to_le_bytes());
        b.extend(4u16.to_le_bytes());
        b.extend(4u16.to_le_bytes());
        b.extend(tempo.to_le_bytes());
        chunk(b"acid", &b)
    }

    #[test]
    fn an_acid_chunk_gives_the_tempo_and_the_beats() {
        let bytes = wav(&[chunk(b"fmt ", &[0; 16]), acid(0, 16, 100.0)]);
        let m = parse_meta(&bytes);
        assert_eq!(m.tempo_bpm, Some(100.0));
        assert_eq!(m.beats, Some(16));
        assert!(!m.one_shot);
    }

    #[test]
    fn the_one_shot_flag_and_odd_chunks_before_the_acid_chunk_are_handled() {
        let bytes = wav(&[chunk(b"junk", &[1, 2, 3]), acid(1, 1, 90.0)]);
        let m = parse_meta(&bytes);
        assert!(m.one_shot);
        assert_eq!(m.tempo_bpm, Some(90.0));
    }

    #[test]
    fn an_implausible_tempo_is_dropped() {
        for t in [0.0f32, 5.0, 5000.0, f32::NAN, f32::INFINITY, -120.0] {
            let m = parse_meta(&wav(&[acid(0, 4, t)]));
            assert_eq!(m.tempo_bpm, None, "{t}");
        }
    }

    #[test]
    fn a_smpl_loop_marks_the_file_as_loop_like() {
        let mut body = vec![0u8; 36];
        body[28] = 1;
        let m = parse_meta(&wav(&[chunk(b"smpl", &body)]));
        assert!(m.has_loop);
        let m = parse_meta(&wav(&[chunk(b"smpl", &[0u8; 36])]));
        assert!(!m.has_loop);
    }

    #[test]
    fn the_bext_description_is_searched_for_a_tempo() {
        for (text, want) in [
            ("Drum loop 128 BPM", Some(128.0)),
            ("bpm: 93.5 tight", Some(93.5)),
            ("a loop", None),
            ("BPM", None),
            ("0 bpm", None),
        ] {
            let mut body = vec![0u8; 602];
            body[..text.len()].copy_from_slice(text.as_bytes());
            let m = parse_meta(&wav(&[chunk(b"bext", &body)]));
            assert_eq!(m.tempo_bpm, want, "{text}");
        }
    }

    fn aiff_chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = id.to_vec();
        v.extend((body.len() as u32).to_be_bytes());
        v.extend(body);
        if body.len() % 2 == 1 {
            v.push(0);
        }
        v
    }

    fn aiff(form: &[u8; 4], chunks: &[Vec<u8>]) -> Vec<u8> {
        let body: Vec<u8> = chunks.concat();
        let mut v = b"FORM".to_vec();
        v.extend((body.len() as u32 + 4).to_be_bytes());
        v.extend(form);
        v.extend(body);
        v
    }

    fn comment(text: &str) -> Vec<u8> {
        let mut b = 1u16.to_be_bytes().to_vec();
        b.extend(0u32.to_be_bytes());
        b.extend(0u16.to_be_bytes());
        b.extend((text.len() as u16).to_be_bytes());
        b.extend(text.as_bytes());
        if text.len() % 2 == 1 {
            b.push(0);
        }
        aiff_chunk(b"COMT", &b)
    }

    #[test]
    fn an_aiff_loop_gives_its_beat_count() {
        let mut basc = 1u32.to_be_bytes().to_vec();
        basc.extend(8u32.to_be_bytes());
        basc.extend([0u8; 8]);
        for form in [b"AIFF", b"AIFC"] {
            let bytes = aiff(
                form,
                &[aiff_chunk(b"COMM", &[0; 18]), aiff_chunk(b"basc", &basc)],
            );
            let m = parse_meta(&bytes);
            assert_eq!(m.beats, Some(8));
            assert_eq!(m.tempo_bpm, None);
        }
        // A beat count of zero or beyond the cap is dropped.
        for n in [0u32, 70_000] {
            let mut b = 1u32.to_be_bytes().to_vec();
            b.extend(n.to_be_bytes());
            assert!(parse_meta(&aiff(b"AIFF", &[aiff_chunk(b"basc", &b)])).is_empty());
        }
    }

    #[test]
    fn an_aiff_text_chunk_names_a_tempo() {
        for (chunk, want) in [
            (aiff_chunk(b"ANNO", b"Drum loop 128 BPM"), Some(128.0)),
            (aiff_chunk(b"NAME", b"bpm: 93.5 tight"), Some(93.5)),
            (comment("groove at 110 bpm"), Some(110.0)),
            (comment("a loop"), None),
            (aiff_chunk(b"ANNO", b"0 bpm"), None),
        ] {
            let m = parse_meta(&aiff(b"AIFF", &[chunk]));
            assert_eq!(m.tempo_bpm, want);
        }
    }

    #[test]
    fn an_aiff_instrument_loop_marks_the_file_as_loop_like() {
        let mut inst = vec![0u8; 20];
        inst[9] = 1;
        assert!(parse_meta(&aiff(b"AIFF", &[aiff_chunk(b"INST", &inst)])).has_loop);
        assert!(!parse_meta(&aiff(b"AIFF", &[aiff_chunk(b"INST", &[0; 20])])).has_loop);
    }

    #[test]
    fn truncated_and_hostile_aiff_headers_never_panic() {
        let full = aiff(
            b"AIFF",
            &[comment("100 bpm"), aiff_chunk(b"basc", &[0; 12])],
        );
        for cut in 0..full.len() {
            let _ = parse_meta(&full[..cut]);
        }
        // A comment count and lengths that run past the chunk.
        let mut b = u16::MAX.to_be_bytes().to_vec();
        b.extend([0u8; 6]);
        b.extend(u16::MAX.to_be_bytes());
        let _ = parse_meta(&aiff(b"AIFF", &[aiff_chunk(b"COMT", &b)]));
        for size in [u32::MAX, 0, 1 << 31] {
            let mut v = b"FORM    AIFF".to_vec();
            v.extend(b"COMT");
            v.extend(size.to_be_bytes());
            v.extend([7u8; 30]);
            let _ = parse_meta(&v);
        }
    }

    #[test]
    fn files_without_metadata_and_other_formats_read_as_empty() {
        assert!(parse_meta(&[]).is_empty());
        assert!(parse_meta(b"RIFF").is_empty());
        assert!(parse_meta(&wav(&[chunk(b"fmt ", &[0; 16])])).is_empty());
        assert!(parse_meta(b"FORM\0\0\0\x04AIFF").is_empty());
        assert!(parse_meta(&[0xff; 64]).is_empty());
    }

    #[test]
    fn truncated_and_hostile_headers_never_panic_or_read_past_the_data() {
        let full = wav(&[acid(0, 8, 120.0)]);
        for cut in 0..full.len() {
            let _ = parse_meta(&full[..cut]);
        }
        // A chunk that claims four gigabytes, one that claims zero, a size that wraps.
        for size in [u32::MAX, 0, u32::MAX - 1, 1 << 31] {
            let mut v = b"RIFF\0\0\0\0WAVE".to_vec();
            v.extend(b"acid");
            v.extend(size.to_le_bytes());
            v.extend([0u8; 30]);
            let _ = parse_meta(&v);
            v.extend(b"acid");
            v.extend(size.to_le_bytes());
            let _ = parse_meta(&v);
        }
        // Many chunks stop at the cap; the acid chunk behind them is not found.
        let many: Vec<Vec<u8>> = (0..MAX_CHUNKS + 8)
            .map(|_| chunk(b"junk", &[0; 2]))
            .chain([acid(0, 4, 120.0)])
            .collect();
        assert!(parse_meta(&wav(&many)).is_empty());
        // Deterministic noise.
        let mut x = 0x9e37_79b9u32;
        for round in 0..200 {
            let n = 12 + round % 90;
            let mut v = b"RIFF\0\0\0\0WAVE".to_vec();
            for _ in 0..n {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                v.push(x as u8);
            }
            let _ = parse_meta(&v);
        }
    }

    #[test]
    fn a_missing_file_reads_as_empty() {
        assert!(read_file_meta(Path::new("definitely/not/here.wav")).is_empty());
    }
}
