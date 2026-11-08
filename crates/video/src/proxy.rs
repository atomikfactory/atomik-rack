//! The proxy file: every frame of a film as a small independent JPEG, in one file with an index.
//!
//! The viewer and the thumbnails decode frames of the proxy, never of the film: any frame is one
//! seek and one small read, with no inter-frame decoding.
//!
//! ```text
//! offset 0   header, 40 bytes, little endian
//!              magic        8   "ATMKVPRX"
//!              version      4   PROXY_VERSION
//!              width        4   of every frame
//!              height       4
//!              rate_num     4   frame rate = rate_num / rate_den
//!              rate_den     4
//!              frame_count  4   0 while the file is unfinished
//!              index_offset 8   0 while the file is unfinished
//! offset 40  the frames, one JPEG file after the other
//! index_offset  frame_count + 1 offsets of 8 bytes: where each frame starts, then where the
//!              index starts (so frame n is the bytes between entries n and n + 1)
//! ```
//!
//! A file is finished when the writer has written the index and then the header with its count
//! and index offset; a reader refuses a file whose header says it is unfinished, whose length
//! does not fit its index, or whose version is not [`PROXY_VERSION`]. The builder writes to a
//! `.part` name and renames when the file is finished, so a proxy under its final name is whole.

use crate::VideoError;
use crate::jpeg::jpeg_size;
use crate::probe::Rational;
use std::io::{Read, Seek, SeekFrom, Write};

/// The version of the file format. A proxy of another version is rebuilt.
pub const PROXY_VERSION: u32 = 1;
const MAGIC: &[u8; 8] = b"ATMKVPRX";
const HEADER_LEN: u64 = 40;

/// What a proxy states about itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProxyInfo {
    /// The width of every frame in pixels.
    pub width: u32,
    /// The height of every frame in pixels.
    pub height: u32,
    /// The frame rate.
    pub rate: Rational,
    /// The number of frames.
    pub frames: u32,
}

impl ProxyInfo {
    /// The number of the frame shown at `seconds` from the first frame, clamped to the film.
    pub fn frame_at(&self, seconds: f64) -> u32 {
        let n = (seconds * self.rate.as_f64()).floor();
        if n.is_nan() || n <= 0.0 {
            0
        } else {
            (n as u64).min(u64::from(self.frames.saturating_sub(1))) as u32
        }
    }

    /// The time of the start of frame `n`, in seconds from the first frame.
    pub fn time_of(&self, n: u32) -> f64 {
        f64::from(n) / self.rate.as_f64()
    }
}

fn header(info: &ProxyInfo, index_offset: u64) -> [u8; HEADER_LEN as usize] {
    let mut h = [0u8; HEADER_LEN as usize];
    h[..8].copy_from_slice(MAGIC);
    for (i, v) in [
        PROXY_VERSION,
        info.width,
        info.height,
        info.rate.num,
        info.rate.den,
        info.frames,
    ]
    .iter()
    .enumerate()
    {
        h[8 + 4 * i..12 + 4 * i].copy_from_slice(&v.to_le_bytes());
    }
    h[32..].copy_from_slice(&index_offset.to_le_bytes());
    h
}

/// Writes a proxy: frames in order, then [`ProxyWriter::finish`].
pub struct ProxyWriter<W: Write + Seek> {
    out: W,
    rate: Rational,
    size: Option<(u32, u32)>,
    offsets: Vec<u64>,
    at: u64,
}

impl<W: Write + Seek> ProxyWriter<W> {
    /// Starts a proxy of frames at `rate` on `out` (which is written from its current start).
    ///
    /// # Errors
    /// [`VideoError::Io`] when the placeholder header cannot be written.
    pub fn new(mut out: W, rate: Rational) -> Result<Self, VideoError> {
        out.seek(SeekFrom::Start(0))?;
        out.write_all(&[0u8; HEADER_LEN as usize])?;
        Ok(Self {
            out,
            rate,
            size: None,
            offsets: Vec::new(),
            at: HEADER_LEN,
        })
    }

    /// The number of frames written so far.
    pub fn frames(&self) -> u32 {
        self.offsets.len() as u32
    }

    /// Appends one frame (a whole JPEG file). The first frame fixes the picture size; a frame
    /// of another size is refused.
    ///
    /// # Errors
    /// [`VideoError::Proxy`] for bytes that are no JPEG or of another size than the first frame,
    /// [`VideoError::Io`] when the write fails.
    pub fn push_frame(&mut self, jpeg: &[u8]) -> Result<(), VideoError> {
        let size = jpeg_size(jpeg).ok_or_else(|| VideoError::Proxy("a frame is no JPEG".into()))?;
        match self.size {
            None => self.size = Some(size),
            Some(first) if first != size => {
                return Err(VideoError::Proxy(format!(
                    "a frame is {}x{} but the first was {}x{}",
                    size.0, size.1, first.0, first.1
                )));
            }
            Some(_) => {}
        }
        if self.offsets.len() >= u32::MAX as usize {
            return Err(VideoError::Proxy("too many frames".into()));
        }
        self.out.write_all(jpeg)?;
        self.offsets.push(self.at);
        self.at += jpeg.len() as u64;
        Ok(())
    }

    /// Writes the index and the real header, and returns the output (not yet flushed to disk:
    /// the caller syncs it).
    ///
    /// # Errors
    /// [`VideoError::Proxy`] when no frame was written, [`VideoError::Io`] when a write fails.
    pub fn finish(mut self) -> Result<W, VideoError> {
        let (width, height) = self
            .size
            .ok_or_else(|| VideoError::Proxy("the video has no frames".into()))?;
        let index_offset = self.at;
        let mut index = Vec::with_capacity(8 * (self.offsets.len() + 1));
        for offset in self.offsets.iter().chain(std::iter::once(&index_offset)) {
            index.extend_from_slice(&offset.to_le_bytes());
        }
        self.out.write_all(&index)?;
        let info = ProxyInfo {
            width,
            height,
            rate: self.rate,
            frames: self.offsets.len() as u32,
        };
        self.out.seek(SeekFrom::Start(0))?;
        self.out.write_all(&header(&info, index_offset))?;
        self.out.flush()?;
        Ok(self.out)
    }
}

/// Reads frames of a finished proxy at random.
pub struct ProxyReader<R: Read + Seek> {
    input: R,
    info: ProxyInfo,
    offsets: Vec<u64>,
}

impl<R: Read + Seek> ProxyReader<R> {
    /// Opens a proxy: checks the header, the version and that the length fits the index, then
    /// reads the index.
    ///
    /// # Errors
    /// [`VideoError::ProxyVersion`] for another version (rebuild it), [`VideoError::Proxy`] for a
    /// file that is not a finished proxy.
    pub fn open(mut input: R) -> Result<Self, VideoError> {
        let bad = |why: &str| VideoError::Proxy(why.to_owned());
        let len = input.seek(SeekFrom::End(0))?;
        if len < HEADER_LEN {
            return Err(bad("the file is too short"));
        }
        input.seek(SeekFrom::Start(0))?;
        let mut h = [0u8; HEADER_LEN as usize];
        input.read_exact(&mut h)?;
        if &h[..8] != MAGIC {
            return Err(bad("it is not a video proxy"));
        }
        let word = |i: usize| {
            u32::from_le_bytes([h[8 + 4 * i], h[9 + 4 * i], h[10 + 4 * i], h[11 + 4 * i]])
        };
        if word(0) != PROXY_VERSION {
            return Err(VideoError::ProxyVersion {
                found: word(0),
                wanted: PROXY_VERSION,
            });
        }
        let index_offset = u64::from_le_bytes(h[32..40].try_into().unwrap_or([0; 8]));
        let frames = word(5);
        let rate = Rational {
            num: word(3),
            den: word(4),
        };
        if frames == 0 || index_offset == 0 {
            return Err(bad("it was not finished"));
        }
        if rate.num == 0 || rate.den == 0 || word(1) == 0 || word(2) == 0 {
            return Err(bad("its header is damaged"));
        }
        let index_len = 8 * (u64::from(frames) + 1);
        if index_offset < HEADER_LEN || index_offset.checked_add(index_len) != Some(len) {
            return Err(bad("its length does not fit its index"));
        }
        input.seek(SeekFrom::Start(index_offset))?;
        let mut raw = vec![0u8; index_len as usize];
        input.read_exact(&mut raw)?;
        let offsets: Vec<u64> = raw
            .as_chunks::<8>()
            .0
            .iter()
            .map(|c| u64::from_le_bytes(*c))
            .collect();
        let ordered = offsets.windows(2).all(|w| w[0] < w[1]);
        if !ordered || offsets[0] != HEADER_LEN || offsets[offsets.len() - 1] != index_offset {
            return Err(bad("its index is damaged"));
        }
        Ok(Self {
            input,
            info: ProxyInfo {
                width: word(1),
                height: word(2),
                rate,
                frames,
            },
            offsets,
        })
    }

    /// What the proxy states.
    pub fn info(&self) -> ProxyInfo {
        self.info
    }

    /// The number of bytes of frame `n`, `None` past the end.
    pub fn frame_len(&self, n: u32) -> Option<u64> {
        let n = n as usize;
        Some(self.offsets.get(n + 1)? - self.offsets.get(n)?)
    }

    /// Reads frame `n` into `out` (replacing what it held): one seek and one read.
    ///
    /// # Errors
    /// [`VideoError::Proxy`] for a frame past the end, [`VideoError::Io`] when the read fails.
    pub fn read_frame(&mut self, n: u32, out: &mut Vec<u8>) -> Result<(), VideoError> {
        let len = self
            .frame_len(n)
            .ok_or_else(|| VideoError::Proxy(format!("there is no frame {n}")))?;
        self.input.seek(SeekFrom::Start(self.offsets[n as usize]))?;
        out.clear();
        out.resize(len as usize, 0);
        self.input.read_exact(out)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::tiny_jpeg;
    use std::io::Cursor;

    const RATE: Rational = Rational { num: 25, den: 1 };

    fn write(frames: &[Vec<u8>]) -> Vec<u8> {
        let mut w = ProxyWriter::new(Cursor::new(Vec::new()), RATE).unwrap();
        for f in frames {
            w.push_frame(f).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    fn frames(n: u8) -> Vec<Vec<u8>> {
        (0..n).map(|i| tiny_jpeg(64, 36, i)).collect()
    }

    #[test]
    fn frames_read_back_at_random() {
        let source = frames(12);
        let bytes = write(&source);
        let mut r = ProxyReader::open(Cursor::new(bytes)).unwrap();
        assert_eq!(
            r.info(),
            ProxyInfo {
                width: 64,
                height: 36,
                rate: RATE,
                frames: 12
            }
        );
        let mut got = Vec::new();
        for n in [7u32, 0, 11, 3, 3, 10] {
            r.read_frame(n, &mut got).unwrap();
            assert_eq!(got, source[n as usize], "frame {n}");
            assert_eq!(r.frame_len(n), Some(source[n as usize].len() as u64));
        }
        assert!(r.read_frame(12, &mut got).is_err());
        assert_eq!(r.frame_len(12), None);
    }

    #[test]
    fn frames_of_different_lengths_keep_their_boundaries() {
        let mut source = frames(3);
        let at = source[1].len() - 2;
        source[1].splice(at..at, [7u8; 100]);
        // Keep it a frame: the extra bytes sit in the entropy data.
        let bytes = write(&source);
        let mut r = ProxyReader::open(Cursor::new(bytes)).unwrap();
        let mut got = Vec::new();
        for (n, f) in source.iter().enumerate() {
            r.read_frame(n as u32, &mut got).unwrap();
            assert_eq!(&got, f);
        }
    }

    #[test]
    fn an_unfinished_file_is_refused() {
        let mut w = ProxyWriter::new(Cursor::new(Vec::new()), RATE).unwrap();
        w.push_frame(&tiny_jpeg(64, 36, 1)).unwrap();
        // The header is still the placeholder: take the bytes without finishing.
        let ProxyWriter { out, .. } = w;
        let err = ProxyReader::open(Cursor::new(out.into_inner()))
            .err()
            .unwrap();
        assert!(matches!(err, VideoError::Proxy(_)), "{err}");
    }

    #[test]
    fn damaged_files_are_refused_without_panicking() {
        let good = write(&frames(4));
        for cut in [0, 5, 39, 40, good.len() / 2, good.len() - 1] {
            assert!(
                ProxyReader::open(Cursor::new(good[..cut].to_vec())).is_err(),
                "cut {cut}"
            );
        }
        // One flipped byte in the header's magic, the frame count and the index.
        for at in [0usize, 28, good.len() - 3, good.len() - 20] {
            let mut bad = good.clone();
            bad[at] ^= 0x55;
            let _ = ProxyReader::open(Cursor::new(bad));
        }
        let mut bad = good.clone();
        bad[0] = b'X';
        assert!(ProxyReader::open(Cursor::new(bad)).is_err());
        let mut bad = good;
        let n = bad.len();
        bad[n - 1] ^= 0xFF;
        assert!(ProxyReader::open(Cursor::new(bad)).is_err());
    }

    #[test]
    fn another_version_asks_for_a_rebuild() {
        let mut bytes = write(&frames(2));
        bytes[8..12].copy_from_slice(&(PROXY_VERSION + 1).to_le_bytes());
        match ProxyReader::open(Cursor::new(bytes)).err().unwrap() {
            VideoError::ProxyVersion { found, wanted } => {
                assert_eq!((found, wanted), (PROXY_VERSION + 1, PROXY_VERSION));
            }
            other => panic!("{other}"),
        }
    }

    #[test]
    fn the_writer_refuses_no_frames_other_sizes_and_non_jpeg() {
        let w = ProxyWriter::new(Cursor::new(Vec::new()), RATE).unwrap();
        assert!(w.finish().is_err());
        let mut w = ProxyWriter::new(Cursor::new(Vec::new()), RATE).unwrap();
        w.push_frame(&tiny_jpeg(64, 36, 1)).unwrap();
        assert!(w.push_frame(&tiny_jpeg(32, 36, 1)).is_err());
        assert!(w.push_frame(b"junk").is_err());
        assert_eq!(w.frames(), 1);
    }

    #[test]
    fn time_and_frame_numbers_map_both_ways() {
        let info = ProxyInfo {
            width: 2,
            height: 2,
            rate: Rational { num: 25, den: 1 },
            frames: 100,
        };
        assert_eq!(info.frame_at(0.0), 0);
        assert_eq!(info.frame_at(0.039), 0);
        assert_eq!(info.frame_at(0.04), 1);
        assert_eq!(info.frame_at(1.0), 25);
        assert_eq!(info.frame_at(-3.0), 0);
        assert_eq!(info.frame_at(f64::NAN), 0);
        assert_eq!(info.frame_at(1e9), 99);
        assert!((info.time_of(50) - 2.0).abs() < 1e-12);
    }
}
