//! Interleaved WAV output (32-bit float or 24-bit PCM) and the reader used by tests.

use std::io::{Read, Seek, Write};
use std::path::Path;

/// Sample format of a WAV file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum SampleFormat {
    /// IEEE 32-bit float, written bit-exactly.
    F32,
    /// 24-bit signed PCM; see [`pcm24_from_f32`].
    Pcm24,
}

/// A decoded WAV file.
#[derive(Debug, Clone, PartialEq)]
pub struct WavData {
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Sample format.
    pub format: SampleFormat,
    /// De-interleaved samples, one `Vec` per channel (PCM is scaled back by 2^-23).
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
    /// The file is neither 32-bit float nor 24-bit PCM.
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

fn spec(channels: u16, sample_rate: u32, format: SampleFormat) -> hound::WavSpec {
    let (bits_per_sample, sample_format) = match format {
        SampleFormat::F32 => (32, hound::SampleFormat::Float),
        SampleFormat::Pcm24 => (24, hound::SampleFormat::Int),
    };
    hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample,
        sample_format,
    }
}

/// Writes `channels` (equal lengths) interleaved as a WAV stream.
pub fn write_wav<W: Write + Seek>(
    writer: W,
    channels: &[Vec<f32>],
    sample_rate: u32,
    format: SampleFormat,
) -> Result<(), WavError> {
    let frames = channels.first().ok_or(WavError::NoChannels)?.len();
    if channels.iter().any(|c| c.len() != frames) {
        return Err(WavError::UnequalChannels);
    }
    let count = u16::try_from(channels.len())
        .map_err(|_| WavError::Unsupported(format!("{} channels", channels.len())))?;
    let mut w = hound::WavWriter::new(writer, spec(count, sample_rate, format))?;
    for n in 0..frames {
        for channel in channels {
            match format {
                SampleFormat::F32 => w.write_sample(channel[n])?,
                SampleFormat::Pcm24 => w.write_sample(pcm24_from_f32(channel[n]))?,
            }
        }
    }
    w.finalize()?;
    Ok(())
}

/// Reads a 32-bit float or 24-bit PCM WAV stream.
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
    fn read_wav_rejects_16bit() {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut buf = Cursor::new(Vec::new());
        let mut w = hound::WavWriter::new(&mut buf, spec).unwrap();
        w.write_sample(0i16).unwrap();
        w.finalize().unwrap();
        let r = read_wav(Cursor::new(buf.into_inner()));
        assert!(matches!(r, Err(WavError::Unsupported(_))), "{r:?}");
    }
}
