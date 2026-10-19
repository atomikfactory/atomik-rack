//! 32-bit float WAV I/O and the seekable streaming reader.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use crate::data::AudioData;
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
    let mut w = hound::WavWriter::create(path, spec).map_err(map_hound)?;
    for i in 0..data.frames() {
        for ch in &data.channels {
            w.write_sample(ch[i]).map_err(map_hound)?;
        }
    }
    w.finalize().map_err(map_hound)
}

/// Reads a WAV file via `hound`. 32-bit float is exact; integer PCM is scaled by `2^-(bits-1)`.
pub fn read_wav_f32(path: &Path) -> Result<AudioData> {
    let mut r = hound::WavReader::open(path).map_err(map_hound)?;
    let spec = r.spec();
    let nch = usize::from(spec.channels);
    if nch == 0 {
        return Err(AudioFileError::Decode("WAV has no channels".into()));
    }
    let total = r.len() as usize;
    let mut channels = vec![Vec::with_capacity(total / nch); nch];
    match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Float, 32) => {
            for (i, s) in r.samples::<f32>().enumerate() {
                channels[i % nch].push(s.map_err(map_hound)?);
            }
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
    AudioData::new(spec.sample_rate, channels)
}

/// Seekable chunked reader over a 32-bit float WAV file (the engine's disk-streaming thread
/// uses it). Not realtime-safe to open; `read_frames` itself never allocates.
pub struct WavStreamReader {
    reader: hound::WavReader<BufReader<File>>,
    channels: usize,
    sample_rate: u32,
    frames: u64,
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
        let reader = hound::WavReader::open(path).map_err(map_hound)?;
        let spec = reader.spec();
        if spec.sample_format != hound::SampleFormat::Float || spec.bits_per_sample != 32 {
            return Err(AudioFileError::Unsupported(
                "streaming needs 32-bit float WAV".into(),
            ));
        }
        if spec.channels == 0 {
            return Err(AudioFileError::Decode("WAV has no channels".into()));
        }
        let channels = usize::from(spec.channels);
        let frames = u64::from(reader.duration());
        Ok(Self {
            reader,
            channels,
            sample_rate: spec.sample_rate,
            frames,
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

    /// Reads frames starting at `start_frame` into `out` (one `Vec` per channel).
    ///
    /// The number of frames wanted is the common length of the `out` vectors, which are never
    /// resized. Returns the frames actually read (fewer at the end of the file, 0 at or past it);
    /// the unread tail of every vector is zero-filled. Does not allocate.
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
            let start = u32::try_from(start_frame)
                .map_err(|_| AudioFileError::InvalidData("start frame beyond 2^32".into()))?;
            self.reader.seek(start)?;
            let mut samples = self.reader.samples::<f32>();
            for i in 0..n {
                for ch in out.iter_mut() {
                    let s = samples
                        .next()
                        .ok_or_else(|| AudioFileError::Decode("WAV data truncated".into()))?;
                    ch[i] = s.map_err(map_hound)?;
                }
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
