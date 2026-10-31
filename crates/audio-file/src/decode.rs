//! Decoding of WAV, AIFF, FLAC, MP3 and OGG/Vorbis through symphonia.
//!
//! Integer PCM becomes `f32` as `i / 2^(bits-1)` (symphonia's exact conversion). Lossless
//! sources (PCM, FLAC) must decode completely: a stream that ends early is an error. For the
//! lossy codecs (MP3, Vorbis) a packet that fails to decode is skipped, as is usual for damaged
//! files, but a file that yields no audio at all is an error.
//!
//! Encoder padding: MP3 and Vorbis encoders add priming samples. Symphonia trims what the
//! container declares (gapless info); anything the container does not declare stays in the
//! output, so lossy files may decode a little longer than the source.

use std::fs::File;
use std::io::{Cursor, Read, SeekFrom};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;

use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::codecs::audio::well_known::{
    CODEC_ID_MP1, CODEC_ID_MP2, CODEC_ID_MP3, CODEC_ID_VORBIS,
};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;

use crate::data::AudioData;
use crate::error::{AudioFileError, Result};

/// The most samples (all channels) [`decode_file`] and [`decode_bytes`] hold in memory: 4 GiB of
/// `f32`, about 3 hours of 96 kHz stereo. A longer file is refused before it can exhaust memory
/// (an allocation failure aborts the process, which no guard can catch); callers that really
/// want more, such as a render that loads long assets, pass their own limit to
/// [`decode_file_with`].
pub const DEFAULT_MAX_SAMPLES: u64 = 1 << 30;

/// A decoded file and what the decoder had to repair.
#[derive(Debug, Clone, PartialEq)]
pub struct Decoded {
    /// The audio.
    pub audio: AudioData,
    /// How many samples were NaN or infinite and became silence (zero for any sound file).
    pub replaced: u64,
}

/// Decodes a file. The extension is used as a probe hint only. Refuses more than
/// [`DEFAULT_MAX_SAMPLES`]; non-finite samples become silence (see [`decode_file_with`]).
pub fn decode_file(path: &Path) -> Result<AudioData> {
    decode_file_with(path, DEFAULT_MAX_SAMPLES).map(|d| d.audio)
}

/// [`decode_file`] with its own limit on the samples (all channels) it will hold, and a count
/// of the non-finite samples it replaced.
pub fn decode_file_with(path: &Path, max_samples: u64) -> Result<Decoded> {
    let file = File::open(path)?;
    let ext = path.extension().and_then(|e| e.to_str());
    decode_source(Box::new(file), ext, max_samples)
}

/// Decodes an in-memory file. `ext_hint` is an optional extension such as `"mp3"`; a wrong
/// hint is harmless because the probe falls back to the content.
pub fn decode_bytes(bytes: &[u8], ext_hint: Option<&str>) -> Result<AudioData> {
    decode_bytes_with(bytes, ext_hint, DEFAULT_MAX_SAMPLES).map(|d| d.audio)
}

/// [`decode_bytes`] with its own sample limit and a count of the repaired samples.
pub fn decode_bytes_with(
    bytes: &[u8],
    ext_hint: Option<&str>,
    max_samples: u64,
) -> Result<Decoded> {
    decode_source(Box::new(Cursor::new(bytes.to_vec())), ext_hint, max_samples)
}

fn decode_source(
    source: Box<dyn MediaSource>,
    ext: Option<&str>,
    max_samples: u64,
) -> Result<Decoded> {
    // Symphonia should never panic on bad data; if a decoder bug does, report it as an error.
    catch_unwind(AssertUnwindSafe(|| decode_inner(source, ext, max_samples))).unwrap_or_else(|_| {
        Err(AudioFileError::Decode(
            "decoder panicked on malformed data".into(),
        ))
    })
}

fn too_long(max_samples: u64) -> AudioFileError {
    AudioFileError::Unsupported(format!(
        "audio too long (more than {max_samples} samples, all channels)"
    ))
}

pub(crate) fn map_sym(e: SymError) -> AudioFileError {
    match e {
        SymError::IoError(e) => AudioFileError::Io(e),
        SymError::Unsupported(m) => AudioFileError::Unsupported(m.into()),
        other => AudioFileError::Decode(other.to_string()),
    }
}

/// Most channels accepted in a WAV header. Symphonia 0.6.1 overflows a shift while fixing the
/// channel mask of a corrupt header with more than 32 channels (a panic when overflow checks are
/// on), and a 32-bit channel mask cannot place more positions anyway.
const MAX_WAV_CHANNELS: u16 = 32;

/// Looks at the first bytes of a RIFF/WAVE stream and rejects headers symphonia mishandles.
pub(crate) fn precheck_wav(source: &mut dyn MediaSource) -> Result<()> {
    let mut head = Vec::with_capacity(4096);
    (&mut *source).take(4096).read_to_end(&mut head)?;
    source.seek(SeekFrom::Start(0))?;
    if head.len() < 12 || &head[..4] != b"RIFF" || &head[8..12] != b"WAVE" {
        return Ok(());
    }
    let mut pos = 12usize;
    while let Some(chunk) = head.get(pos..pos + 8) {
        let size = u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]) as usize;
        if &chunk[..4] == b"fmt " {
            if let Some(f) = head.get(pos + 8..pos + 12) {
                let channels = u16::from_le_bytes([f[2], f[3]]);
                if channels == 0 {
                    return Err(AudioFileError::Decode("WAV has no channels".into()));
                }
                if channels > MAX_WAV_CHANNELS {
                    return Err(AudioFileError::Unsupported(format!(
                        "WAV with {channels} channels"
                    )));
                }
            }
            return Ok(());
        }
        pos = pos
            .saturating_add(8)
            .saturating_add(size)
            .saturating_add(size & 1);
    }
    Ok(())
}

fn decode_inner(
    mut source: Box<dyn MediaSource>,
    ext: Option<&str>,
    max_samples: u64,
) -> Result<Decoded> {
    precheck_wav(source.as_mut())?;
    let mss = MediaSourceStream::new(source, Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = ext {
        hint.with_extension(ext);
    }
    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(map_sym)?;

    let track = format
        .default_track(TrackType::Audio)
        .ok_or_else(|| AudioFileError::Unsupported("no audio track".into()))?;
    let track_id = track.id;
    let declared_frames = track.num_frames;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or_else(|| AudioFileError::Unsupported("no audio codec parameters".into()))?;
    let lossy = [CODEC_ID_MP1, CODEC_ID_MP2, CODEC_ID_MP3, CODEC_ID_VORBIS].contains(&params.codec);
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(params, &AudioDecoderOptions::default())
        .map_err(map_sym)?;

    let mut rate = 0u32;
    let mut channels: Vec<Vec<f32>> = Vec::new();
    let mut scratch: Vec<Vec<f32>> = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(SymError::ResetRequired) => {
                return Err(AudioFileError::Unsupported("chained streams".into()));
            }
            Err(e) => return Err(map_sym(e)),
        };
        if packet.track_id != track_id {
            continue;
        }
        let buf = match decoder.decode(&packet) {
            Ok(b) => b,
            Err(SymError::DecodeError(_)) if lossy => continue,
            Err(e) => return Err(map_sym(e)),
        };
        if buf.frames() == 0 {
            continue;
        }
        let spec = buf.spec();
        let (r, n) = (spec.rate(), buf.num_planes());
        if channels.is_empty() {
            rate = r;
            channels = vec![Vec::new(); n];
        } else if r != rate || n != channels.len() {
            return Err(AudioFileError::InvalidData(
                "stream changes format mid-way".into(),
            ));
        }
        buf.copy_to_vecs_planar::<f32>(&mut scratch);
        // Check the size before growing: the growth itself is what aborts on a lack of memory.
        let wanted = (channels[0].len() as u64 + scratch.first().map_or(0, Vec::len) as u64)
            .saturating_mul(n as u64);
        if wanted > max_samples {
            return Err(too_long(max_samples));
        }
        for (dst, src) in channels.iter_mut().zip(&scratch) {
            dst.try_reserve(src.len())
                .map_err(|_| too_long(max_samples))?;
            dst.extend_from_slice(src);
        }
    }

    if channels.is_empty() || channels[0].is_empty() {
        return Err(AudioFileError::Empty);
    }
    if !lossy
        && let Some(declared) = declared_frames
        && (channels[0].len() as u64) < declared
    {
        return Err(AudioFileError::Decode(format!(
            "truncated: decoded {} of {declared} frames",
            channels[0].len()
        )));
    }
    let (audio, replaced) = AudioData::new_counted(rate, channels)?;
    Ok(Decoded { audio, replaced })
}

#[cfg(test)]
mod panic_tests {
    use super::*;
    use std::io::Seek;

    /// A source that panics when read, standing in for a decoder bug on malformed data.
    struct PanickingSource;

    impl Read for PanickingSource {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            panic!("decoder bug");
        }
    }

    impl Seek for PanickingSource {
        fn seek(&mut self, _: SeekFrom) -> std::io::Result<u64> {
            Ok(0)
        }
    }

    impl MediaSource for PanickingSource {
        fn is_seekable(&self) -> bool {
            false
        }
        fn byte_len(&self) -> Option<u64> {
            None
        }
    }

    #[test]
    fn a_panicking_decoder_returns_error() {
        let result = decode_source(Box::new(PanickingSource), Some("wav"), DEFAULT_MAX_SAMPLES);
        assert!(
            matches!(result, Err(AudioFileError::Decode(_))),
            "{result:?}"
        );
    }
}
