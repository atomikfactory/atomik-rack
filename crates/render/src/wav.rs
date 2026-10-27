//! Interleaved WAV output (32-bit float, 24-bit or 16-bit PCM) and the reader used by tests.
//!
//! 16-bit output is dithered by default (seeded TPDF, [`atomik_dsp::Dither`]); 24-bit and float
//! output is not. [`write_wav_with`] picks the dither and the seed.

use atomik_dsp::{Dither, DitherMode};
use std::io::{Read, Seek, Write};
use std::path::Path;

/// Sample format of a WAV file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum SampleFormat {
    /// IEEE 32-bit float, written bit-exactly.
    F32,
    /// 24-bit signed PCM; see [`pcm24_from_f32`].
    Pcm24,
    /// 16-bit signed PCM, dithered by default (see the module docs).
    Pcm16,
}

/// A decoded WAV file.
#[derive(Debug, Clone, PartialEq)]
pub struct WavData {
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Sample format.
    pub format: SampleFormat,
    /// De-interleaved samples, one `Vec` per channel (PCM is scaled back by 2^-23 or 2^-15).
    pub channels: Vec<Vec<f32>>,
}

/// WAV writing or reading failed.
#[derive(Debug, thiserror::Error)]
pub enum WavError {
    /// I/O or encoding error from the WAV library.
    #[error(transparent)]
    Hound(#[from] hound::Error),
    /// There is nothing to write.
    #[error("at least one channel is required")]
    NoChannels,
    /// The channel buffers differ in length.
    #[error("every channel must have the same number of frames")]
    UnequalChannels,
    /// The file is not 32-bit float, 24-bit PCM or 16-bit PCM.
    #[error("unsupported WAV sample format: {0}")]
    Unsupported(String),
}

/// Converts a float sample to 24-bit PCM.
///
/// Rule: `x · 2^23` rounded half to even, then clamped to `-2^23..=2^23 − 1` (so +1.0 maps to
/// 8 388 607 and −1.0 to −8 388 608). NaN maps to 0. Inputs beyond ±1 clip.
pub fn pcm24_from_f32(x: f32) -> i32 {
    if x.is_nan() {
        return 0;
    }
    let scaled = (f64::from(x) * PCM24_SCALE).round_ties_even();
    scaled.clamp(-PCM24_SCALE, PCM24_SCALE - 1.0) as i32
}

/// 2^23: full scale of 24-bit PCM.
const PCM24_SCALE: f64 = 8_388_608.0;

/// 2^15: full scale of 16-bit PCM.
const PCM16_SCALE: f32 = 32_768.0;

fn spec(channels: u16, sample_rate: u32, format: SampleFormat) -> hound::WavSpec {
    let (bits_per_sample, sample_format) = match format {
        SampleFormat::F32 => (32, hound::SampleFormat::Float),
        SampleFormat::Pcm24 => (24, hound::SampleFormat::Int),
        SampleFormat::Pcm16 => (16, hound::SampleFormat::Int),
    };
    hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample,
        sample_format,
    }
}

/// The dither [`write_wav`] uses: seeded TPDF for 16-bit PCM, none for the others.
pub fn default_dither(format: SampleFormat) -> DitherMode {
    match format {
        SampleFormat::Pcm16 => DitherMode::Tpdf,
        SampleFormat::F32 | SampleFormat::Pcm24 => DitherMode::None,
    }
}

/// Writes `channels` (equal lengths) interleaved as a WAV stream, with the default dither of the
/// format ([`default_dither`], seed 0).
pub fn write_wav<W: Write + Seek>(
    writer: W,
    channels: &[Vec<f32>],
    sample_rate: u32,
    format: SampleFormat,
) -> Result<(), WavError> {
    write_wav_with(
        writer,
        channels,
        sample_rate,
        format,
        (default_dither(format), 0),
    )
}

/// [`write_wav`] with the dither and its seed chosen (`dither`: mode and seed). The dither
/// applies to the PCM formats and is ignored for float output; it is the only place the samples
/// are requantised, so a float to 16-bit export is quantised exactly once.
pub fn write_wav_with<W: Write + Seek>(
    writer: W,
    channels: &[Vec<f32>],
    sample_rate: u32,
    format: SampleFormat,
    (dither, seed): (DitherMode, u64),
) -> Result<(), WavError> {
    let frames = channels.first().ok_or(WavError::NoChannels)?.len();
    if channels.iter().any(|c| c.len() != frames) {
        return Err(WavError::UnequalChannels);
    }
    let count = u16::try_from(channels.len())
        .map_err(|_| WavError::Unsupported(format!("{} channels", channels.len())))?;
    let mut w = hound::WavWriter::new(writer, spec(count, sample_rate, format))?;
    let mut dither = Dither::new(dither, seed, channels.len());
    for n in 0..frames {
        for (c, channel) in channels.iter().enumerate() {
            match format {
                SampleFormat::F32 => w.write_sample(channel[n])?,
                SampleFormat::Pcm24 => w.write_sample(dither.quantise(c, channel[n], 24))?,
                SampleFormat::Pcm16 => w.write_sample(dither.quantise(c, channel[n], 16) as i16)?,
            }
        }
    }
    w.finalize()?;
    Ok(())
}

/// Reads a 32-bit float, 24-bit PCM or 16-bit PCM WAV stream.
pub fn read_wav<R: Read>(reader: R) -> Result<WavData, WavError> {
    let mut r = hound::WavReader::new(reader)?;
    let spec = r.spec();
    let count = usize::from(spec.channels);
    let (format, interleaved): (SampleFormat, Vec<f32>) =
        match (spec.sample_format, spec.bits_per_sample) {
            (hound::SampleFormat::Float, 32) => (
                SampleFormat::F32,
                r.samples::<f32>().collect::<Result<_, _>>()?,
            ),
            (hound::SampleFormat::Int, 24) => (
                SampleFormat::Pcm24,
                r.samples::<i32>()
                    .map(|s| s.map(|v| (f64::from(v) / PCM24_SCALE) as f32))
                    .collect::<Result<_, _>>()?,
            ),
            (hound::SampleFormat::Int, 16) => (
                SampleFormat::Pcm16,
                r.samples::<i16>()
                    .map(|s| s.map(|v| f32::from(v) / PCM16_SCALE))
                    .collect::<Result<_, _>>()?,
            ),
            (f, bits) => return Err(WavError::Unsupported(format!("{bits}-bit {f:?}"))),
        };
    let mut channels = vec![Vec::with_capacity(interleaved.len() / count.max(1)); count];
    for frame in interleaved.chunks(count.max(1)) {
        for (channel, sample) in channels.iter_mut().zip(frame) {
            channel.push(*sample);
        }
    }
    Ok(WavData {
        sample_rate: spec.sample_rate,
        format,
        channels,
    })
}

/// [`write_wav`] to a file (created or truncated).
pub fn write_wav_file(
    path: &Path,
    channels: &[Vec<f32>],
    sample_rate: u32,
    format: SampleFormat,
) -> Result<(), WavError> {
    let file = std::io::BufWriter::new(std::fs::File::create(path).map_err(hound::Error::from)?);
    write_wav(file, channels, sample_rate, format)
}

/// [`write_wav_with`] to a file (created or truncated).
pub fn write_wav_file_with(
    path: &Path,
    channels: &[Vec<f32>],
    sample_rate: u32,
    format: SampleFormat,
    dither: (DitherMode, u64),
) -> Result<(), WavError> {
    let file = std::io::BufWriter::new(std::fs::File::create(path).map_err(hound::Error::from)?);
    write_wav_with(file, channels, sample_rate, format, dither)
}

/// [`read_wav`] from a file.
pub fn read_wav_file(path: &Path) -> Result<WavData, WavError> {
    let file = std::io::BufReader::new(std::fs::File::open(path).map_err(hound::Error::from)?);
    read_wav(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const SCALE: f32 = 8_388_608.0;

    fn roundtrip(channels: &[Vec<f32>], format: SampleFormat) -> (Vec<u8>, WavData) {
        let mut buf = Cursor::new(Vec::new());
        write_wav(&mut buf, channels, 48_000, format).unwrap();
        let bytes = buf.into_inner();
        let data = read_wav(Cursor::new(&bytes)).unwrap();
        (bytes, data)
    }

    #[test]
    fn wav_writer_f32_and_24bit_roundtrip() {
        let left: Vec<f32> = (0..1000).map(|i| ((i as f32) * 0.01).sin() * 0.9).collect();
        let right: Vec<f32> = left.iter().map(|x| -x * 0.5).collect();
        let channels = vec![left, right];

        let (bytes, f32_data) = roundtrip(&channels, SampleFormat::F32);
        assert_eq!(f32_data.sample_rate, 48_000);
        assert_eq!(f32_data.format, SampleFormat::F32);
        assert_eq!(f32_data.channels.len(), 2);
        for (a, b) in channels.iter().zip(&f32_data.channels) {
            assert!(a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits()));
        }
        // Interleaved: the first two data samples are L[0], R[0].
        assert_eq!(&bytes[..4], b"RIFF");

        let (_, pcm) = roundtrip(&channels, SampleFormat::Pcm24);
        assert_eq!(pcm.format, SampleFormat::Pcm24);
        for (a, b) in channels.iter().zip(&pcm.channels) {
            assert_eq!(a.len(), b.len());
            for (x, y) in a.iter().zip(b) {
                assert!((x - y).abs() <= 0.5 / SCALE + f32::EPSILON, "{x} vs {y}");
            }
        }
    }

    #[test]
    fn pcm24_rounds_half_to_even_and_clamps() {
        let at = |k: f32| pcm24_from_f32(k / SCALE);
        assert_eq!(at(0.5), 0);
        assert_eq!(at(1.5), 2);
        assert_eq!(at(2.5), 2);
        assert_eq!(at(-0.5), 0);
        assert_eq!(at(-1.5), -2);
        assert_eq!(at(1.25), 1);
        assert_eq!(pcm24_from_f32(1.0), 8_388_607);
        assert_eq!(pcm24_from_f32(-1.0), -8_388_608);
        assert_eq!(pcm24_from_f32(3.0), 8_388_607);
        assert_eq!(pcm24_from_f32(-3.0), -8_388_608);
        assert_eq!(pcm24_from_f32(f32::NAN), 0);
        assert_eq!(pcm24_from_f32(f32::INFINITY), 8_388_607);
    }

    #[test]
    fn write_wav_rejects_bad_channel_sets() {
        let mut buf = Cursor::new(Vec::new());
        assert!(matches!(
            write_wav(&mut buf, &[], 48_000, SampleFormat::F32),
            Err(WavError::NoChannels)
        ));
        assert!(matches!(
            write_wav(
                &mut buf,
                &[vec![0.0; 3], vec![0.0; 2]],
                48_000,
                SampleFormat::F32
            ),
            Err(WavError::UnequalChannels)
        ));
    }

    #[test]
    fn read_wav_rejects_8bit() {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 8,
            sample_format: hound::SampleFormat::Int,
        };
        let mut buf = Cursor::new(Vec::new());
        let mut w = hound::WavWriter::new(&mut buf, spec).unwrap();
        w.write_sample(0i16).unwrap();
        w.finalize().unwrap();
        let r = read_wav(Cursor::new(buf.into_inner()));
        assert!(matches!(r, Err(WavError::Unsupported(_))), "{r:?}");
    }

    fn tone() -> Vec<Vec<f32>> {
        let left: Vec<f32> = (0..4800).map(|i| ((i as f32) * 0.05).sin() * 0.8).collect();
        let right: Vec<f32> = left.iter().map(|x| x * -0.4).collect();
        vec![left, right]
    }

    #[test]
    fn pcm16_roundtrips_within_the_dither_and_has_a_16_bit_header() {
        let channels = tone();
        let (bytes, back) = roundtrip(&channels, SampleFormat::Pcm16);
        assert_eq!(back.format, SampleFormat::Pcm16);
        assert_eq!(back.sample_rate, 48_000);
        assert_eq!(back.channels.len(), 2);
        // fmt chunk: PCM (1), 2 channels, 48 kHz, block align 4, 16 bits.
        assert_eq!(&bytes[20..22], &1u16.to_le_bytes());
        assert_eq!(&bytes[22..24], &2u16.to_le_bytes());
        assert_eq!(&bytes[32..34], &4u16.to_le_bytes());
        assert_eq!(&bytes[34..36], &16u16.to_le_bytes());
        // TPDF of ±1 LSB plus the rounding: at most 1.5 LSB from the input.
        for (a, b) in channels.iter().zip(&back.channels) {
            assert_eq!(a.len(), b.len());
            for (x, y) in a.iter().zip(b) {
                assert!(
                    (x - y).abs() <= 1.5 / PCM16_SCALE + f32::EPSILON,
                    "{x} vs {y}"
                );
            }
        }
    }

    #[test]
    fn pcm16_without_dither_rounds_to_half_an_lsb() {
        let channels = tone();
        let mut buf = Cursor::new(Vec::new());
        write_wav_with(
            &mut buf,
            &channels,
            48_000,
            SampleFormat::Pcm16,
            (DitherMode::None, 0),
        )
        .unwrap();
        let back = read_wav(Cursor::new(buf.into_inner())).unwrap();
        for (a, b) in channels.iter().zip(&back.channels) {
            for (x, y) in a.iter().zip(b) {
                assert!(
                    (x - y).abs() <= 0.5 / PCM16_SCALE + f32::EPSILON,
                    "{x} vs {y}"
                );
            }
        }
    }

    #[test]
    fn the_dither_is_seeded_and_only_touches_the_pcm_words() {
        let channels = tone();
        let bytes = |format, mode, seed| {
            let mut buf = Cursor::new(Vec::new());
            write_wav_with(&mut buf, &channels, 48_000, format, (mode, seed)).unwrap();
            buf.into_inner()
        };
        let a = bytes(SampleFormat::Pcm16, DitherMode::Tpdf, 9);
        assert_eq!(a, bytes(SampleFormat::Pcm16, DitherMode::Tpdf, 9));
        assert_ne!(a, bytes(SampleFormat::Pcm16, DitherMode::Tpdf, 10));
        assert_ne!(a, bytes(SampleFormat::Pcm16, DitherMode::None, 9));
        // write_wav is the 16-bit default: TPDF, seed 0.
        let mut buf = Cursor::new(Vec::new());
        write_wav(&mut buf, &channels, 48_000, SampleFormat::Pcm16).unwrap();
        assert_eq!(
            buf.into_inner(),
            bytes(SampleFormat::Pcm16, DitherMode::Tpdf, 0)
        );
        // Float output ignores it.
        assert_eq!(
            bytes(SampleFormat::F32, DitherMode::Tpdf, 1),
            bytes(SampleFormat::F32, DitherMode::None, 2)
        );
    }

    #[test]
    fn pcm24_is_not_dithered_by_default_and_matches_pcm24_from_f32() {
        let channels = tone();
        let mut buf = Cursor::new(Vec::new());
        write_wav(&mut buf, &channels, 48_000, SampleFormat::Pcm24).unwrap();
        let back = read_wav(Cursor::new(buf.into_inner())).unwrap();
        for (a, b) in channels.iter().zip(&back.channels) {
            for (x, y) in a.iter().zip(b) {
                let code = pcm24_from_f32(*x);
                assert_eq!(*y, (f64::from(code) / PCM24_SCALE) as f32);
            }
        }
    }

    #[test]
    fn the_audio_file_reader_reads_the_16_and_24_bit_files() {
        let channels = tone();
        let dir = std::env::temp_dir().join(format!("atomik-wav-depth-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (format, tolerance) in [
            (SampleFormat::Pcm16, 1.5 / PCM16_SCALE),
            (SampleFormat::Pcm24, 0.5 / SCALE),
        ] {
            let path = dir.join(format!("{format:?}.wav"));
            write_wav_file(&path, &channels, 44_100, format).unwrap();
            let decoded = atomik_audio_file::decode_file(&path).unwrap();
            assert_eq!(decoded.sample_rate, 44_100);
            assert_eq!(decoded.channels.len(), 2);
            assert_eq!(decoded.frames(), channels[0].len());
            for (a, b) in channels.iter().zip(&decoded.channels) {
                for (x, y) in a.iter().zip(b) {
                    assert!((x - y).abs() <= tolerance + 1e-6, "{format:?}: {x} vs {y}");
                }
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
