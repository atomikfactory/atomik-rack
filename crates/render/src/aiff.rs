//! AIFF output: 16-bit and 24-bit PCM (plain AIFF) and 32-bit float (AIFF-C `fl32`).
//!
//! The quantiser is the one of the WAV writer ([`atomik_dsp::Dither`]), so a file of either
//! container holds the same codes for the same audio, and a 16-bit file is quantised exactly once.
//! The samples are big-endian and interleaved; the sample rate is the 80-bit extended float the
//! format asks for. The file is `FORM`, the common chunk and the sound data, plus the version
//! chunk of an AIFF-C file.

use crate::wav::{SampleFormat, WavError};
use atomik_dsp::{Dither, DitherMode};
use std::io::Write;
use std::path::Path;

/// The AIFF-C format version chunk's timestamp (version 1).
const AIFC_VERSION_1: u32 = 0xA280_5140;

/// The sample rate as an 80-bit IEEE extended float (sign and exponent, then the 64-bit mantissa
/// with its integer bit set), big-endian.
pub fn extended_from_rate(rate: u32) -> [u8; 10] {
    if rate == 0 {
        return [0; 10];
    }
    let shift = rate.leading_zeros();
    let mantissa = u64::from(rate) << (32 + shift);
    // The exponent of a value whose top mantissa bit is bit 63.
    let exponent = 16_383 + 31 - u16::try_from(shift).unwrap_or(0);
    let mut out = [0u8; 10];
    out[..2].copy_from_slice(&exponent.to_be_bytes());
    out[2..].copy_from_slice(&mantissa.to_be_bytes());
    out
}

/// The whole number an 80-bit extended float holds (`0` when it is negative, not a number or
/// does not fit 32 bits). The inverse of [`extended_from_rate`] for the rates that fit.
pub fn rate_from_extended(bytes: &[u8; 10]) -> u32 {
    let exponent = i32::from(u16::from_be_bytes([bytes[0], bytes[1]]) & 0x7FFF);
    if bytes[0] & 0x80 != 0 {
        return 0;
    }
    let mut m = [0u8; 8];
    m.copy_from_slice(&bytes[2..]);
    let mantissa = u64::from_be_bytes(m);
    let shift = 16_383 + 63 - exponent;
    if !(32..64).contains(&shift) {
        return 0;
    }
    u32::try_from(mantissa >> shift).unwrap_or(0)
}

fn chunk(out: &mut Vec<u8>, id: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(id);
    out.extend_from_slice(&u32::try_from(body.len()).unwrap_or(u32::MAX).to_be_bytes());
    out.extend_from_slice(body);
    if body.len() % 2 == 1 {
        out.push(0);
    }
}

/// The bytes of the headers of an AIFF file of `frames` frames (everything before the samples)
/// and the size in bytes of the sound data.
fn header(
    channels: u16,
    frames: u32,
    sample_rate: u32,
    format: SampleFormat,
) -> Result<(Vec<u8>, u32), WavError> {
    let (bits, float) = match format {
        SampleFormat::Pcm16 => (16u16, false),
        SampleFormat::Pcm24 => (24, false),
        SampleFormat::F32 => (32, true),
    };
    let data_len = u64::from(frames) * u64::from(channels) * u64::from(bits / 8);
    let data_len = u32::try_from(data_len)
        .ok()
        .filter(|&n| n < u32::MAX - 4096)
        .ok_or_else(|| WavError::Unsupported("an AIFF file holds at most 4 GiB".to_owned()))?;
    let mut common = Vec::new();
    common.extend_from_slice(&channels.to_be_bytes());
    common.extend_from_slice(&frames.to_be_bytes());
    common.extend_from_slice(&bits.to_be_bytes());
    common.extend_from_slice(&extended_from_rate(sample_rate));
    let mut body = Vec::new();
    body.extend_from_slice(if float { b"AIFC" } else { b"AIFF" });
    if float {
        chunk(&mut body, b"FVER", &AIFC_VERSION_1.to_be_bytes());
        common.extend_from_slice(b"fl32");
        // The compression name, a Pascal string padded to an even length.
        common.extend_from_slice(&[12]);
        common.extend_from_slice(b"32-bit float\0");
    }
    chunk(&mut body, b"COMM", &common);
    // The sound data chunk's header: its size counts the offset and block size words.
    body.extend_from_slice(b"SSND");
    body.extend_from_slice(&(data_len + 8).to_be_bytes());
    body.extend_from_slice(&[0; 8]);
    let total = u32::try_from(body.len())
        .ok()
        .and_then(|n| n.checked_add(data_len))
        .and_then(|n| n.checked_add(data_len % 2))
        .ok_or_else(|| WavError::Unsupported("an AIFF file holds at most 4 GiB".to_owned()))?;
    let mut out = Vec::with_capacity(body.len() + 8);
    out.extend_from_slice(b"FORM");
    out.extend_from_slice(&total.to_be_bytes());
    out.extend_from_slice(&body);
    Ok((out, data_len))
}

/// Writes `channels` (equal lengths) interleaved as an AIFF stream: `format` is the sample
/// format (16-bit and 24-bit PCM through the dither `(mode, seed)`, 32-bit float untouched).
///
/// # Errors
/// No channels, channels of different lengths, more than 4 GiB of audio, or an I/O error.
pub fn write_aiff<W: Write>(
    mut writer: W,
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
    let frame_count = u32::try_from(frames)
        .map_err(|_| WavError::Unsupported("an AIFF file holds at most 4 GiB".to_owned()))?;
    let (head, data_len) = header(count, frame_count, sample_rate, format)?;
    let io = |e: std::io::Error| WavError::Hound(hound::Error::from(e));
    writer.write_all(&head).map_err(io)?;
    let mut dither = Dither::new(dither, seed, channels.len());
    // A block of frames at a time keeps the writes large without holding the whole file.
    let mut block = Vec::with_capacity(4096 * channels.len() * 4);
    for start in (0..frames).step_by(4096) {
        block.clear();
        for n in start..(start + 4096).min(frames) {
            for (c, channel) in channels.iter().enumerate() {
                match format {
                    SampleFormat::F32 => block.extend_from_slice(&channel[n].to_be_bytes()),
                    SampleFormat::Pcm24 => {
                        let v = dither.quantise(c, channel[n], 24);
                        block.extend_from_slice(&v.to_be_bytes()[1..]);
                    }
                    SampleFormat::Pcm16 => {
                        let v = dither.quantise(c, channel[n], 16) as i16;
                        block.extend_from_slice(&v.to_be_bytes());
                    }
                }
            }
        }
        writer.write_all(&block).map_err(io)?;
    }
    if data_len % 2 == 1 {
        writer.write_all(&[0]).map_err(io)?;
    }
    writer.flush().map_err(io)?;
    Ok(())
}

/// [`write_aiff`] to a file (created or truncated).
///
/// # Errors
/// As [`write_aiff`].
pub fn write_aiff_file(
    path: &Path,
    channels: &[Vec<f32>],
    sample_rate: u32,
    format: SampleFormat,
    dither: (DitherMode, u64),
) -> Result<(), WavError> {
    let file = std::fs::File::create(path).map_err(|e| WavError::Hound(hound::Error::from(e)))?;
    write_aiff(
        std::io::BufWriter::new(file),
        channels,
        sample_rate,
        format,
        dither,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sample_rate_is_an_extended_float() {
        // 44100 Hz is 0x400E AC44 0000 0000 0000 in the format's own documentation.
        assert_eq!(
            extended_from_rate(44_100),
            [0x40, 0x0E, 0xAC, 0x44, 0, 0, 0, 0, 0, 0]
        );
        assert_eq!(
            extended_from_rate(48_000),
            [0x40, 0x0E, 0xBB, 0x80, 0, 0, 0, 0, 0, 0]
        );
        for rate in [8_000, 22_050, 44_100, 48_000, 88_200, 96_000, 192_000, 1] {
            assert_eq!(rate_from_extended(&extended_from_rate(rate)), rate);
        }
    }

    #[test]
    fn the_headers_have_the_right_sizes() {
        let channels = vec![vec![0.0f32; 10], vec![0.0f32; 10]];
        for (format, form, bytes) in [
            (SampleFormat::Pcm16, b"AIFF", 94),
            (SampleFormat::Pcm24, b"AIFF", 114),
            (SampleFormat::F32, b"AIFC", 164),
        ] {
            let mut out = Vec::new();
            write_aiff(&mut out, &channels, 48_000, format, (DitherMode::None, 0)).unwrap();
            assert_eq!(&out[..4], b"FORM");
            assert_eq!(&out[8..12], form);
            let size = u32::from_be_bytes([out[4], out[5], out[6], out[7]]) as usize;
            assert_eq!(size + 8, out.len(), "{format:?}");
            assert_eq!(out.len(), bytes, "{format:?}");
        }
    }

    #[test]
    fn nothing_or_ragged_channels_are_refused() {
        let mut out = Vec::new();
        let none: Vec<Vec<f32>> = Vec::new();
        assert!(matches!(
            write_aiff(
                &mut out,
                &none,
                48_000,
                SampleFormat::Pcm16,
                (DitherMode::None, 0)
            ),
            Err(WavError::NoChannels)
        ));
        let ragged = vec![vec![0.0; 3], vec![0.0; 2]];
        assert!(matches!(
            write_aiff(
                &mut out,
                &ragged,
                48_000,
                SampleFormat::Pcm16,
                (DitherMode::None, 0)
            ),
            Err(WavError::UnequalChannels)
        ));
    }
}
