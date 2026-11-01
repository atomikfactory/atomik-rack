//! 32-bit float WAV I/O and the seekable streaming reader.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::data::AudioData;
use crate::decode::Decoded;
use crate::error::{AudioFileError, Result};

fn map_hound(e: hound::Error) -> AudioFileError {
    match e {
        hound::Error::IoError(e) => AudioFileError::Io(e),
        hound::Error::Unsupported => AudioFileError::Unsupported("WAV format".into()),
        hound::Error::FormatError(m) => AudioFileError::Decode(format!("WAV: {m}")),
        other => AudioFileError::Decode(format!("WAV: {other}")),
    }
}

/// Writes interleaved 32-bit float WAV. The round trip through [`read_wav_f32`] is exact.
pub fn write_wav_f32(path: &Path, data: &AudioData) -> Result<()> {
    data.validate()?;
    let spec = hound::WavSpec {
        channels: data.channels.len() as u16,
        sample_rate: data.sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let frames = data.frames();
    let nch = data.channels.len();
    let data_bytes = frames as u64 * nch as u64 * 4;
    // The header comes from hound (an empty file), so its bytes are the ones hound would write;
    // the two size fields are patched here. Data that does not fit a 32-bit size keeps the
    // sample-at-a-time path and its error.
    let mut head = std::io::Cursor::new(Vec::new());
    hound::WavWriter::new(&mut head, spec)
        .and_then(hound::WavWriter::finalize)
        .map_err(map_hound)?;
    let mut head = head.into_inner();
    let head_len = head.len();
    if head_len < 8 || data_bytes + head_len as u64 - 8 > u64::from(u32::MAX) {
        return write_wav_f32_slow(path, data, spec);
    }
    head[4..8].copy_from_slice(&((head_len as u64 - 8 + data_bytes) as u32).to_le_bytes());
    head[head_len - 4..].copy_from_slice(&(data_bytes as u32).to_le_bytes());
    let mut file = File::create(path)?;
    file.write_all(&head)?;
    let mut block = vec![0u8; 4 * nch * WRITE_FRAMES.min(frames.max(1))];
    let mut at = 0;
    while at < frames {
        let take = (frames - at).min(WRITE_FRAMES);
        let bytes = &mut block[..take * nch * 4];
        for (c, ch) in data.channels.iter().enumerate() {
            for (i, s) in ch[at..at + take].iter().enumerate() {
                let o = (i * nch + c) * 4;
                bytes[o..o + 4].copy_from_slice(&s.to_le_bytes());
            }
        }
        file.write_all(bytes)?;
        at += take;
    }
    file.flush()?;
    Ok(())
}

/// Frames converted per write: 64 KiB of stereo.
const WRITE_FRAMES: usize = 8192;

fn write_wav_f32_slow(path: &Path, data: &AudioData, spec: hound::WavSpec) -> Result<()> {
    let mut w = hound::WavWriter::create(path, spec).map_err(map_hound)?;
    for i in 0..data.frames() {
        for ch in &data.channels {
            w.write_sample(ch[i]).map_err(map_hound)?;
        }
    }
    w.finalize().map_err(map_hound)
}

/// Reads a WAV file via `hound`. 32-bit float is exact; integer PCM is scaled by `2^-(bits-1)`.
/// A NaN or infinite sample becomes silence. The memory taken never exceeds what the file's
/// own size can fill, whatever its header claims.
pub fn read_wav_f32(path: &Path) -> Result<AudioData> {
    read_wav_f32_limited(path, u64::MAX).map(|d| d.audio)
}

/// [`read_wav_f32`] that refuses a file of more than `max_samples` samples (all channels) and
/// counts the non-finite samples it replaced.
pub fn read_wav_f32_limited(path: &Path, max_samples: u64) -> Result<Decoded> {
    let file_len = std::fs::metadata(path)?.len();
    let mut r = hound::WavReader::open(path).map_err(map_hound)?;
    let spec = r.spec();
    let nch = usize::from(spec.channels);
    if nch == 0 {
        return Err(AudioFileError::Decode("WAV has no channels".into()));
    }
    // The header's sample count is a claim. The file cannot hold more samples than its bytes
    // allow, so that bounds the allocation; a file that falls short of its claim ends in the
    // read error below.
    let bytes_per_sample = u64::from(spec.bits_per_sample.div_ceil(8)).max(1);
    let total = u64::from(r.len()).min(file_len / bytes_per_sample);
    if total > max_samples {
        return Err(AudioFileError::Unsupported(format!(
            "audio too long ({total} samples, all channels; the limit is {max_samples})"
        )));
    }
    let per_channel = usize::try_from(total / nch as u64).unwrap_or(usize::MAX);
    let mut channels = vec![Vec::new(); nch];
    for channel in &mut channels {
        channel.try_reserve_exact(per_channel).map_err(|_| {
            AudioFileError::Unsupported(format!(
                "audio too long ({total} samples, all channels) for the memory available"
            ))
        })?;
    }
    match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Float, 32) => {
            // The reader sits at the first sample: take the raw interleaved bytes in blocks.
            let claimed = r.len() as usize;
            let mut src = r.into_inner();
            read_interleaved_f32(&mut src, claimed, &mut channels)?;
        }
        (hound::SampleFormat::Int, bits @ 8..=32) => {
            let scale = 2f64.powi(i32::from(bits) - 1);
            for (i, s) in r.samples::<i32>().enumerate() {
                channels[i % nch].push((f64::from(s.map_err(map_hound)?) / scale) as f32);
            }
        }
        (f, b) => return Err(AudioFileError::Unsupported(format!("WAV {f:?} {b}-bit"))),
    }
    if channels.iter().any(|c| c.len() != channels[0].len()) {
        return Err(AudioFileError::Decode(
            "WAV data ends inside a frame".into(),
        ));
    }
    let (audio, replaced) = AudioData::new_counted(spec.sample_rate, channels)?;
    Ok(Decoded { audio, replaced })
}

/// Reads `samples` interleaved little-endian `f32` from `src` into `channels` (the sample at
/// index `i` goes to channel `i % channels.len()`), 64 KiB at a time. A source that ends early
/// is an I/O error, as it is for the sample-at-a-time reader.
fn read_interleaved_f32(
    src: &mut impl Read,
    samples: usize,
    channels: &mut [Vec<f32>],
) -> Result<()> {
    let nch = channels.len();
    let frames = samples / nch;
    let mut block = vec![0u8; 4 * nch * READ_FRAMES.min(frames.max(1))];
    let mut done = 0;
    while done < frames {
        let take = (frames - done).min(READ_FRAMES);
        let bytes = &mut block[..take * nch * 4];
        src.read_exact(bytes)?;
        let (raw, _) = bytes.as_chunks::<4>();
        for (c, ch) in channels.iter_mut().enumerate() {
            ch.extend(
                raw.iter()
                    .skip(c)
                    .step_by(nch)
                    .map(|b| f32::from_le_bytes(*b)),
            );
        }
        done += take;
    }
    // A trailing partial frame: the channels end up of unequal length and the caller reports it.
    for ch in channels.iter_mut().take(samples % nch) {
        let mut b = [0u8; 4];
        src.read_exact(&mut b)?;
        ch.push(f32::from_le_bytes(b));
    }
    Ok(())
}

/// Frames converted per block when reading: 64 KiB of stereo.
const READ_FRAMES: usize = 8192;

/// Frames read from the file per system call by [`WavStreamReader`].
const STREAM_CHUNK_FRAMES: usize = 4096;

/// Format tags of the `fmt ` chunk.
const TAG_FLOAT: u16 = 3;
const TAG_EXTENSIBLE: u16 = 0xFFFE;

/// Where the samples of a WAV file are.
struct WavLayout {
    channels: usize,
    sample_rate: u32,
    /// Offset of the first sample in the file.
    data_start: u64,
    /// Whole frames of data the file really holds.
    frames: u64,
}

fn le_u16(b: &[u8]) -> u16 {
    u16::from_le_bytes([b[0], b[1]])
}

fn le_u32(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

fn le_u64(b: &[u8]) -> u64 {
    u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
}

/// Walks the chunks of a RIFF or RF64 WAV file (32-bit float only) with 64-bit offsets: the
/// size fields of RIFF are 32-bit, so hound cannot reach past 4 GiB, and RF64 keeps the real
/// size of the data chunk in its `ds64` chunk. A data size the file cannot hold (a recording cut
/// short, a size field of `0xFFFF_FFFF` left by a writer that streams) is cut to the file.
fn read_layout(file: &mut File) -> Result<WavLayout> {
    let file_len = file.metadata()?.len();
    let short = |_: std::io::Error| AudioFileError::Decode("WAV header is cut short".into());
    let mut head = [0u8; 12];
    file.read_exact(&mut head).map_err(short)?;
    let rf64 = match &head[..4] {
        b"RIFF" => false,
        b"RF64" | b"BW64" => true,
        _ => return Err(AudioFileError::Decode("WAV: not a RIFF file".into())),
    };
    if &head[8..12] != b"WAVE" {
        return Err(AudioFileError::Decode("WAV: not a WAVE file".into()));
    }
    let mut pos = 12u64;
    let mut ds64_data = None;
    let mut fmt: Option<(u16, usize, u32, u16)> = None;
    loop {
        if pos.saturating_add(8) > file_len {
            return Err(AudioFileError::Decode("WAV has no data chunk".into()));
        }
        let mut chunk = [0u8; 8];
        file.seek(SeekFrom::Start(pos))?;
        file.read_exact(&mut chunk).map_err(short)?;
        let id = [chunk[0], chunk[1], chunk[2], chunk[3]];
        let size = u64::from(le_u32(&chunk[4..]));
        let body = pos + 8;
        match &id {
            b"ds64" if rf64 => {
                let mut b = [0u8; 16];
                file.read_exact(&mut b).map_err(short)?;
                ds64_data = Some(le_u64(&b[8..]));
            }
            b"fmt " => {
                if size < 16 {
                    return Err(AudioFileError::Decode("WAV fmt chunk is too short".into()));
                }
                let mut b = [0u8; 40];
                let want = usize::try_from(size).unwrap_or(40).min(40);
                file.read_exact(&mut b[..want]).map_err(short)?;
                let mut tag = le_u16(&b);
                if tag == TAG_EXTENSIBLE && want >= 26 {
                    tag = le_u16(&b[24..]);
                }
                fmt = Some((
                    tag,
                    usize::from(le_u16(&b[2..])),
                    le_u32(&b[4..]),
                    le_u16(&b[14..]),
                ));
            }
            b"data" => {
                let Some((tag, channels, sample_rate, bits)) = fmt else {
                    return Err(AudioFileError::Decode(
                        "WAV data before its fmt chunk".into(),
                    ));
                };
                if tag != TAG_FLOAT || bits != 32 {
                    return Err(AudioFileError::Unsupported(
                        "streaming needs 32-bit float WAV".into(),
                    ));
                }
                if channels == 0 {
                    return Err(AudioFileError::Decode("WAV has no channels".into()));
                }
                let claimed = match ds64_data {
                    Some(n) if rf64 && size == u64::from(u32::MAX) => n,
                    _ => size,
                };
                let available = file_len.saturating_sub(body);
                let bytes = claimed.min(available);
                return Ok(WavLayout {
                    channels,
                    sample_rate,
                    data_start: body,
                    frames: bytes / (4 * channels as u64),
                });
            }
            _ => {}
        }
        pos = body.saturating_add(size).saturating_add(size & 1);
    }
}

/// Seekable chunked reader over a 32-bit float WAV file (the engine's disk-streaming thread
/// uses it). Not realtime-safe to open; `read_frames` itself never allocates.
///
/// Offsets are 64-bit: assets above 4 GiB (RF64 files, or plain WAV files whose size field was
/// left at its maximum) stream like any other.
pub struct WavStreamReader {
    file: File,
    channels: usize,
    sample_rate: u32,
    frames: u64,
    data_start: u64,
    /// The frame the file position is at, when known (the next sequential read needs no seek).
    position: Option<u64>,
    /// Bytes read from the file before they are split into channels: a read-ahead block that
    /// holds the frames `ahead_start..ahead_start + ahead_frames`.
    scratch: Vec<u8>,
    ahead_start: u64,
    ahead_frames: usize,
    /// Read system calls made.
    reads: u64,
}

impl std::fmt::Debug for WavStreamReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WavStreamReader")
            .field("channels", &self.channels)
            .field("sample_rate", &self.sample_rate)
            .field("frames", &self.frames)
            .finish()
    }
}

impl WavStreamReader {
    /// Opens a WAV file written by this crate (32-bit float). Other formats are `Unsupported`.
    pub fn open(path: &Path) -> Result<Self> {
        let mut file = File::open(path)?;
        let layout = read_layout(&mut file)?;
        Ok(Self {
            file,
            channels: layout.channels,
            sample_rate: layout.sample_rate,
            frames: layout.frames,
            data_start: layout.data_start,
            position: None,
            scratch: vec![0; 4 * layout.channels * STREAM_CHUNK_FRAMES],
            ahead_start: 0,
            ahead_frames: 0,
            reads: 0,
        })
    }

    /// Sample rate in Hz.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Channel count.
    pub fn channel_count(&self) -> usize {
        self.channels
    }

    /// Total frames in the file.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Number of read system calls made so far (diagnostics: sequential playback in chunks
    /// smaller than the read-ahead block costs one call per block, not one per chunk).
    pub fn read_calls(&self) -> u64 {
        self.reads
    }

    /// Reads the block of frames starting at `frame` into `scratch` (the read-ahead). Seeks only
    /// when the file cursor is elsewhere. Never allocates.
    fn fill(&mut self, frame: u64) -> Result<()> {
        // Nothing is buffered, and the cursor is unknown, until the read succeeds.
        self.ahead_frames = 0;
        let want = usize::try_from(self.frames - frame)
            .unwrap_or(usize::MAX)
            .min(STREAM_CHUNK_FRAMES);
        if self.position != Some(frame) {
            self.position = None;
            self.file.seek(SeekFrom::Start(
                self.data_start + frame * 4 * self.channels as u64,
            ))?;
        }
        self.position = None;
        let bytes = &mut self.scratch[..want * 4 * self.channels];
        self.reads += 1;
        self.file.read_exact(bytes).map_err(|e| {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                AudioFileError::Decode("WAV data truncated".into())
            } else {
                AudioFileError::Io(e)
            }
        })?;
        self.ahead_start = frame;
        self.ahead_frames = want;
        self.position = Some(frame + want as u64);
        Ok(())
    }

    /// Reads frames starting at `start_frame` into `out` (one `Vec` per channel).
    ///
    /// The number of frames wanted is the common length of the `out` vectors, which are never
    /// resized. Returns the frames actually read (fewer at the end of the file, 0 at or past it);
    /// the unread tail of every vector is zero-filled. A NaN or infinite sample reads as
    /// silence. Does not allocate.
    pub fn read_frames(&mut self, start_frame: u64, out: &mut [Vec<f32>]) -> Result<usize> {
        if out.len() != self.channels {
            return Err(AudioFileError::InvalidData(format!(
                "expected {} channel buffers, got {}",
                self.channels,
                out.len()
            )));
        }
        let want = out[0].len();
        if out.iter().any(|c| c.len() != want) {
            return Err(AudioFileError::InvalidData(
                "channel buffers differ in length".into(),
            ));
        }
        let avail = self.frames.saturating_sub(start_frame);
        let n = want.min(usize::try_from(avail).unwrap_or(usize::MAX));
        if n > 0 {
            let frame_bytes = 4 * self.channels;
            let mut done = 0;
            while done < n {
                let frame = start_frame + done as u64;
                let in_ahead = frame >= self.ahead_start
                    && frame < self.ahead_start + self.ahead_frames as u64;
                if !in_ahead {
                    self.fill(frame)?;
                }
                let first = (frame - self.ahead_start) as usize;
                let take = (n - done).min(self.ahead_frames - first);
                let bytes = &self.scratch[first * frame_bytes..(first + take) * frame_bytes];
                for (i, frame) in bytes.chunks_exact(frame_bytes).enumerate() {
                    let (samples, _) = frame.as_chunks::<4>();
                    for (ch, raw) in out.iter_mut().zip(samples) {
                        let s = f32::from_le_bytes(*raw);
                        ch[done + i] = if s.is_finite() { s } else { 0.0 };
                    }
                }
                done += take;
            }
        }
        for ch in out.iter_mut() {
            ch[n..].fill(0.0);
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::AudioFileError;

    fn tmp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("atomik-audio-file-wav-tests");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(format!("{}-{name}", std::process::id()))
    }

    /// Deterministic odd-valued test signal including special floats.
    fn signal(channels: usize, frames: usize) -> AudioData {
        let chans = (0..channels)
            .map(|c| {
                (0..frames)
                    .map(|i| match i % 7 {
                        0 => 0.0,
                        1 => -0.0,
                        2 => f32::MIN_POSITIVE,
                        3 => 1.0 + f32::EPSILON,
                        _ => ((i * 31 + c * 17) as f32).sin() * 1.5,
                    })
                    .collect()
            })
            .collect();
        AudioData::new(48_000, chans).unwrap()
    }

    #[test]
    fn round_trip_is_bit_exact() {
        for (ch, frames) in [(1, 1000), (2, 777), (6, 33)] {
            let data = signal(ch, frames);
            let p = tmp(&format!("rt{ch}.wav"));
            write_wav_f32(&p, &data).unwrap();
            let back = read_wav_f32(&p).unwrap();
            assert_eq!(back.sample_rate, 48_000);
            assert_eq!(back.channel_count(), ch);
            for (a, b) in data
                .channels
                .iter()
                .flatten()
                .zip(back.channels.iter().flatten())
            {
                assert_eq!(a.to_bits(), b.to_bits());
            }
            assert_eq!(back.frames(), frames);
            std::fs::remove_file(p).unwrap();
        }
    }

    #[test]
    fn written_file_is_32_bit_float_interleaved() {
        let data = AudioData::new(44_100, vec![vec![0.25, 0.5], vec![-0.25, -0.5]]).unwrap();
        let p = tmp("layout.wav");
        write_wav_f32(&p, &data).unwrap();
        let mut r = hound::WavReader::open(&p).unwrap();
        let spec = r.spec();
        assert_eq!(
            (
                spec.channels,
                spec.sample_rate,
                spec.bits_per_sample,
                spec.sample_format
            ),
            (2, 44_100, 32, hound::SampleFormat::Float)
        );
        let s: Vec<f32> = r.samples::<f32>().map(Result::unwrap).collect();
        assert_eq!(s, [0.25, -0.25, 0.5, -0.5]);
        std::fs::remove_file(p).unwrap();
    }

    #[test]
    fn missing_and_garbage_files_are_errors() {
        assert!(matches!(
            read_wav_f32(&tmp("nope.wav")),
            Err(AudioFileError::Io(_))
        ));
        let p = tmp("garbage.wav");
        std::fs::write(&p, b"definitely not a wav file").unwrap();
        assert!(read_wav_f32(&p).is_err());
        assert!(WavStreamReader::open(&p).is_err());
        std::fs::remove_file(p).unwrap();
    }

    #[test]
    fn reads_integer_pcm_scaled_exactly() {
        let p = tmp("pcm16.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 8000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&p, spec).unwrap();
        for s in [-32768i16, 0, 16384, 32767] {
            w.write_sample(s).unwrap();
        }
        w.finalize().unwrap();
        let d = read_wav_f32(&p).unwrap();
        assert_eq!(d.channels[0], [-1.0, 0.0, 0.5, 32767.0 / 32768.0]);
        std::fs::remove_file(p).unwrap();
    }

    #[test]
    fn stream_reader_matches_full_load_on_random_ranges() {
        let data = signal(2, 5000);
        let p = tmp("stream.wav");
        write_wav_f32(&p, &data).unwrap();
        let mut r = WavStreamReader::open(&p).unwrap();
        assert_eq!(
            (r.sample_rate(), r.channel_count(), r.frames()),
            (48_000, 2, 5000)
        );
        // Deterministic pseudo-random (start, len) pairs, in non-monotonic order.
        let mut x: u64 = 0x1234_5678_9abc_def0;
        for _ in 0..50 {
            x = x
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let start = (x >> 33) % 5000;
            let len = ((x >> 12) % 700 + 1) as usize;
            let mut out = vec![vec![9.0f32; len]; 2];
            let n = r.read_frames(start, &mut out).unwrap();
            let expect_n = len.min(5000 - start as usize);
            assert_eq!(n, expect_n, "start {start} len {len}");
            for (ch, got) in out.iter().enumerate() {
                let want = &data.channels[ch][start as usize..start as usize + n];
                for (g, w) in got.iter().zip(want) {
                    assert_eq!(g.to_bits(), w.to_bits());
                }
                assert!(
                    got[n..].iter().all(|v| v.to_bits() == 0),
                    "tail must be zero-filled"
                );
                assert_eq!(got.len(), len, "read_frames never resizes");
            }
        }
        std::fs::remove_file(p).unwrap();
    }

    #[test]
    fn stream_reader_edge_cases() {
        let data = signal(2, 100);
        let p = tmp("stream-edge.wav");
        write_wav_f32(&p, &data).unwrap();
        let mut r = WavStreamReader::open(&p).unwrap();
        let mut out = vec![vec![1.0f32; 10]; 2];
        assert_eq!(r.read_frames(100, &mut out).unwrap(), 0, "start at end");
        assert_eq!(r.read_frames(5000, &mut out).unwrap(), 0, "start past end");
        assert!(out.iter().all(|c| c.iter().all(|v| *v == 0.0)));
        let mut wrong = vec![vec![0.0f32; 10]; 3];
        assert!(matches!(
            r.read_frames(0, &mut wrong),
            Err(AudioFileError::InvalidData(_))
        ));
        let mut ragged = vec![vec![0.0f32; 10], vec![0.0f32; 9]];
        assert!(matches!(
            r.read_frames(0, &mut ragged),
            Err(AudioFileError::InvalidData(_))
        ));
        std::fs::remove_file(p).unwrap();
    }
}
