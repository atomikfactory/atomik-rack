//! Progressive decoding for previews: open a file and pull decoded chunks one at a time, so a
//! long file starts playing after its first packets instead of after a full decode.
//!
//! Nothing is read ahead beyond symphonia's own buffered reader, so memory stays flat however
//! long the file is, and the first chunk costs one header parse plus a few packets.
//!
//! The decoder is forgiving where [`crate::decode_file`] is strict: damaged packets are skipped
//! and a truncated or unreadable tail ends the stream normally. Only a failure before any audio
//! is available (unsupported file, header problems) or a decoder panic is reported as an error.

use crate::decode::{map_sym, precheck_wav};
use crate::{AudioFileError, Result};
use atomik_library::AudioInfo;
use std::fs::File;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;

use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;

/// Header data of `path` (rate, channels, length when stated) without decoding the audio.
/// Fast: reads the container header and codec parameters only. When a format leaves the
/// channel count or rate out of its header (some MP3 files), the first packet is decoded.
pub fn probe_file(path: &Path) -> Result<AudioInfo> {
    StreamDecoder::open(path).map(|d| d.info())
}

/// A decoder that yields a file's audio in chunks, at the file's own rate, planar `f32`.
///
/// Unlike [`crate::decode_file`] it is forgiving: a damaged or truncated tail simply ends the
/// stream (the preview plays what could be decoded). A decoder panic inside symphonia ends the
/// stream as an error, never unwinds into the caller.
pub struct StreamDecoder {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    info: AudioInfo,
    /// Decoded frames not yet handed out (also the first packet read while discovering the
    /// stream parameters).
    pending: Vec<Vec<f32>>,
    scratch: Vec<Vec<f32>>,
    done: bool,
}

/// The error reported when symphonia panics.
fn panicked() -> AudioFileError {
    AudioFileError::Decode("decoder panicked on malformed data".into())
}

impl StreamDecoder {
    /// Opens `path` and reads its header. Fails for unsupported or unreadable files.
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        let ext = path.extension().and_then(|e| e.to_str());
        catch_unwind(AssertUnwindSafe(|| Self::open_source(Box::new(file), ext)))
            .unwrap_or_else(|_| Err(panicked()))
    }

    fn open_source(mut source: Box<dyn MediaSource>, ext: Option<&str>) -> Result<Self> {
        precheck_wav(source.as_mut())?;
        let mss = MediaSourceStream::new(source, Default::default());
        let mut hint = Hint::new();
        if let Some(ext) = ext {
            hint.with_extension(ext);
        }
        let format = symphonia::default::get_probe()
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
        let frames = track.num_frames;
        let params = track
            .codec_params
            .as_ref()
            .and_then(|p| p.audio())
            .ok_or_else(|| AudioFileError::Unsupported("no audio codec parameters".into()))?;
        let rate = params.sample_rate.filter(|r| *r > 0);
        let channels = params
            .channels
            .as_ref()
            .map(|c| c.count())
            .filter(|c| *c > 0);
        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(params, &AudioDecoderOptions::default())
            .map_err(map_sym)?;

        let mut this = Self {
            format,
            decoder,
            track_id,
            info: AudioInfo {
                sample_rate: rate.unwrap_or(0),
                channels: channels.map_or(0, |c| u16::try_from(c).unwrap_or(u16::MAX)),
                frames,
            },
            pending: Vec::new(),
            scratch: Vec::new(),
            done: false,
        };
        if rate.is_none() || channels.is_none() {
            // The header does not state everything: learn it from the first decodable packet.
            if !this.decode_packet()? {
                return Err(AudioFileError::Empty);
            }
        }
        Ok(this)
    }

    /// Stream parameters (as [`probe_file`] would return).
    pub fn info(&self) -> AudioInfo {
        self.info
    }

    /// Decodes until at least `min_frames` frames are available (fewer only at the end of
    /// the stream) and returns them planar (`channels` vectors of equal length).
    /// `Ok(None)` once the stream is exhausted.
    ///
    /// A chunk holds at most about `min_frames` plus one packet. After an `Err` the stream is
    /// finished and every later call returns `Ok(None)`.
    pub fn next_chunk(&mut self, min_frames: usize) -> Result<Option<Vec<Vec<f32>>>> {
        let want = min_frames.max(1);
        let r = catch_unwind(AssertUnwindSafe(|| -> Result<()> {
            while !self.done && self.pending.first().is_none_or(|p| p.len() < want) {
                self.decode_packet()?;
            }
            Ok(())
        }));
        match r {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                self.done = true;
                self.pending.clear();
                return Err(e);
            }
            Err(_) => {
                self.done = true;
                self.pending.clear();
                return Err(panicked());
            }
        }
        if self.pending.is_empty() {
            return Ok(None);
        }
        Ok(Some(std::mem::take(&mut self.pending)))
    }

    /// Decodes the next packet of the track and appends it to `pending`. Returns `false` at the
    /// end of the stream (and sets `done`).
    fn decode_packet(&mut self) -> Result<bool> {
        loop {
            let packet = match self.format.next_packet() {
                Ok(Some(p)) => p,
                // End of file, or a truncated or unreadable tail: the stream simply ends.
                Ok(None) | Err(SymError::IoError(_) | SymError::DecodeError(_)) => {
                    self.done = true;
                    return Ok(false);
                }
                Err(SymError::ResetRequired) => {
                    return Err(AudioFileError::Unsupported("chained streams".into()));
                }
                Err(e) => return Err(map_sym(e)),
            };
            if packet.track_id != self.track_id {
                continue;
            }
            let buf = match self.decoder.decode(&packet) {
                Ok(b) => b,
                Err(SymError::DecodeError(_)) => continue,
                Err(SymError::IoError(_)) => {
                    self.done = true;
                    return Ok(false);
                }
                Err(e) => return Err(map_sym(e)),
            };
            if buf.frames() == 0 {
                continue;
            }
            let (rate, planes) = (buf.spec().rate(), buf.num_planes());
            if self.info.channels == 0 {
                self.info.channels = u16::try_from(planes).unwrap_or(u16::MAX);
            }
            if self.info.sample_rate == 0 {
                self.info.sample_rate = rate;
            }
            if rate != self.info.sample_rate || planes != usize::from(self.info.channels) {
                return Err(AudioFileError::InvalidData(
                    "stream changes format mid-way".into(),
                ));
            }
            buf.copy_to_vecs_planar::<f32>(&mut self.scratch);
            if self.pending.is_empty() {
                self.pending = vec![Vec::new(); planes];
            }
            for (dst, src) in self.pending.iter_mut().zip(&self.scratch) {
                dst.extend_from_slice(src);
            }
            return Ok(true);
        }
    }
}
