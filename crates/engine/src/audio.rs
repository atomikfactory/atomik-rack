//! Audio clips: engine-native audio regions, assets, and the RT
//! renderer that turns them into per-batch *playback* buffers for `atomik.audio-track` devices.
//!
//! # Data
//!
//! * [`SongAsset`] is one decoded asset: [`SongAsset::Memory`] (planar `f32`, shared) or
//!   [`SongAsset::Stream`] (read from disk by the non-RT streamer, see `stream.rs`).
//!   `AssetId(n)` refers to `SongInput::assets[n]`. Offline bounces always use `Memory`
//!   (determinism).
//! * [`SongAudioTrack`] is the list of [`SongAudioRegion`]s one audio-track device plays. The
//!   fields mirror the model's `AudioRegion`: song-frame start, source offset, length, linear
//!   gain, fades measured inside the *original clip* (`clip_pos`, `clip_frames`).
//!
//! # Rendering (RT, allocation-free)
//!
//! For every batch segment `[a, b)` of song frames the player renders each audio track into a
//! preallocated stereo pair (`AudioRt::pb`), zeroed at the batch start; a segment lands at its
//! batch offset, so loop wraps and locates are sample-accurate. Regions are validated sorted
//! and non-overlapping, so a segment finds its regions by binary search. For region frame `i`
//! (song frame `song_start_frame + i`) the output is
//!
//! `asset[source_offset + i] * (gain * fade(i))`
//!
//! and silence once `source_offset + i` passes the asset's end (the region is cut at the
//! asset end). Mono assets play on both channels; for assets with more than two channels the
//! first two are played. The product is evaluated in `f32` in exactly this order.
//!
//! # Fades (bit-identical across platforms)
//!
//! `fade(i)` is the model's `AudioRegion::fade_gain`, evaluated in `f64` and rounded to `f32`:
//! with `n = clip_pos + i`, fade-in `curve(n / fade_in)` for `n < fade_in`, fade-out
//! `curve((clip_frames - n) / fade_out)` for `n >= clip_frames - fade_out`, multiplied. Linear
//! is `t` (identical to the model's). EqualPower is `sin(t * pi / 2)`, which the model computes
//! with the platform's `f64::sin`; the engine instead uses [`sin_quarter`], a 12-term Taylor
//! polynomial in nested Horner form (only IEEE `+ - * /` on `f64`, no libm call, no fused
//! multiply-add), so every platform rounds identically. Its truncation error is below
//! `1e-20`, so after rounding to `f32` it equals the model's value except, very rarely, one
//! ulp when the exact value lies next to a rounding boundary.

use crate::graph::DeviceInstanceId;
use crate::stream::StreamLaneRt;
use atomik_core::{BATCH, Batch};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Index of an asset in [`crate::SongInput::assets`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AssetId(pub u32);

/// A source of sample frames read on the streamer thread (never on the audio thread).
///
/// The app implements it over its file reader (`atomik_audio_file::WavStreamReader`, behind a
/// mutex, since `read` takes `&self`).
pub trait StreamSource: Send + Sync {
    /// Reads frames starting at `start_frame` into `out`, one `Vec` per channel (the asset's
    /// channel count). The number of frames wanted is the common length of the vectors, which
    /// are not resized. Returns the number of frames actually read (fewer at the end of the
    /// asset, 0 at or past it). The unread tail may be left untouched. May block (it only
    /// runs on the streamer thread).
    fn read(&self, start_frame: u64, out: &mut [Vec<f32>]) -> usize;
}

/// One decoded asset of a song.
#[derive(Clone)]
pub enum SongAsset {
    /// Fully loaded planar samples: `data.len() == channels`, every channel `frames` long.
    Memory {
        /// One `Vec` per channel (shared, immutable).
        data: Arc<[Vec<f32>]>,
        /// Frames per channel.
        frames: u64,
        /// Channel count (>= 1).
        channels: u16,
    },
    /// Read from disk through the streamer.
    Stream {
        /// Total frames of the asset.
        frames: u64,
        /// Channel count (>= 1).
        channels: u16,
        /// The reader.
        source: Arc<dyn StreamSource>,
    },
}

impl SongAsset {
    /// A memory asset from planar data (`frames` and `channels` are taken from `data`).
    pub fn memory(data: Arc<[Vec<f32>]>) -> Self {
        Self::Memory {
            frames: data.first().map_or(0, Vec::len) as u64,
            channels: u16::try_from(data.len()).unwrap_or(u16::MAX),
            data,
        }
    }

    /// A streamed asset.
    pub fn stream(frames: u64, channels: u16, source: Arc<dyn StreamSource>) -> Self {
        Self::Stream {
            frames,
            channels,
            source,
        }
    }

    /// Frames per channel.
    pub fn frames(&self) -> u64 {
        match self {
            Self::Memory { frames, .. } | Self::Stream { frames, .. } => *frames,
        }
    }

    /// Channel count.
    pub fn channels(&self) -> u16 {
        match self {
            Self::Memory { channels, .. } | Self::Stream { channels, .. } => *channels,
        }
    }

    /// True for a streamed asset.
    pub fn is_stream(&self) -> bool {
        matches!(self, Self::Stream { .. })
    }
}

impl std::fmt::Debug for SongAsset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Memory {
                frames, channels, ..
            } => write!(f, "Memory({channels} ch, {frames} frames)"),
            Self::Stream {
                frames, channels, ..
            } => write!(f, "Stream({channels} ch, {frames} frames)"),
        }
    }
}

impl PartialEq for SongAsset {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Memory {
                    data: a,
                    frames: fa,
                    channels: ca,
                },
                Self::Memory {
                    data: b,
                    frames: fb,
                    channels: cb,
                },
            ) => fa == fb && ca == cb && (Arc::ptr_eq(a, b) || a == b),
            (
                Self::Stream {
                    frames: fa,
                    channels: ca,
                    source: a,
                },
                Self::Stream {
                    frames: fb,
                    channels: cb,
                    source: b,
                },
            ) => fa == fb && ca == cb && Arc::ptr_eq(a, b),
            _ => false,
        }
    }
}

/// Fade shape (the model's `FadeCurve`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SongFadeCurve {
    /// Gain ramps linearly.
    Linear,
    /// `sin(t * pi / 2)` (see the module docs for the platform-independent evaluation).
    EqualPower,
}

/// One stretch of audio an audio track plays, in song frames (the model's `AudioRegion`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SongAudioRegion {
    /// Asset (index into `SongInput::assets`).
    pub asset: AssetId,
    /// Song frame where the region starts.
    pub song_start_frame: u64,
    /// Asset frame read at the first region frame.
    pub source_offset: u64,
    /// Region length in frames (>= 1); cut earlier where the asset ends.
    pub frames: u64,
    /// Linear gain (finite).
    pub gain: f32,
    /// Fade-in length in frames of the original clip (at most `clip_frames`).
    pub fade_in: u64,
    /// Fade-out length in frames of the original clip (at most `clip_frames`).
    pub fade_out: u64,
    /// Fade shape.
    pub fade_curve: SongFadeCurve,
    /// Frames from the original clip's start to this region's start.
    pub clip_pos: u64,
    /// Length in frames of the original clip (>= `clip_pos + frames`).
    pub clip_frames: u64,
}

impl SongAudioRegion {
    /// First song frame after the region.
    pub fn end(&self) -> u64 {
        self.song_start_frame + self.frames
    }

    /// The fade gain at region frame `i`, without the clip gain: the model's
    /// `AudioRegion::fade_gain`, with the platform-independent EqualPower curve.
    pub fn fade_gain(&self, i: u64) -> f32 {
        let n = self.clip_pos + i;
        let fading_in = n < self.fade_in;
        let fading_out = self.fade_out > 0 && n + self.fade_out >= self.clip_frames;
        if !fading_in && !fading_out {
            return 1.0;
        }
        let curve = |t: f64| match self.fade_curve {
            SongFadeCurve::Linear => t,
            SongFadeCurve::EqualPower => sin_quarter(t),
        };
        let mut g = 1.0f64;
        if fading_in {
            g *= curve(n as f64 / self.fade_in as f64);
        }
        if fading_out {
            g *= curve(self.clip_frames.saturating_sub(n) as f64 / self.fade_out as f64);
        }
        g as f32
    }
}

/// `sin(t * pi / 2)` for `t` in `[0, 1]` using only IEEE `f64` arithmetic in a fixed order
/// (nested Horner Taylor series to `x^23`; error below `1e-20`), so it is bit-identical on
/// every platform. Values outside `[0, 1]` are clamped.
pub fn sin_quarter(t: f64) -> f64 {
    let x = t.clamp(0.0, 1.0) * std::f64::consts::FRAC_PI_2;
    let x2 = x * x;
    let mut acc = 1.0f64;
    let mut k = 11u32;
    while k >= 1 {
        let d = f64::from(2 * k * (2 * k + 1));
        acc = 1.0 - acc * x2 / d;
        k -= 1;
    }
    x * acc
}

/// The audio regions of one audio-track device.
#[derive(Debug, Clone, PartialEq)]
pub struct SongAudioTrack {
    /// The `atomik.audio-track` device.
    pub device: DeviceInstanceId,
    /// Regions (any order; `compile_song` sorts by `song_start_frame` and rejects overlaps).
    pub regions: Vec<SongAudioRegion>,
}

/// The immutable, shared part of a compiled audio track (also read by the streamer).
pub(crate) struct LaneData {
    /// Sorted by start, non-overlapping.
    pub(crate) regions: Vec<SongAudioRegion>,
    pub(crate) assets: Arc<[SongAsset]>,
}

/// A compiled audio track.
#[derive(Clone)]
pub(crate) struct CompiledAudio {
    pub(crate) device: DeviceInstanceId,
    pub(crate) data: Arc<LaneData>,
    /// True if any of its regions reads a streamed asset.
    pub(crate) streams: bool,
}

/// Per-track RT scratch: the playback pair and the stream lane (attached on submit).
pub(crate) struct AudioRt {
    pub(crate) pb: [Batch; 2],
    pub(crate) stream: Option<StreamLaneRt>,
}

impl Clone for AudioRt {
    /// A clone carries no stream lane (the lane belongs to the submitted snapshot).
    fn clone(&self) -> Self {
        Self {
            pb: self.pb,
            stream: None,
        }
    }
}

impl AudioRt {
    pub(crate) fn new() -> Self {
        Self {
            pb: [[0.0; BATCH]; 2],
            stream: None,
        }
    }
}

/// Validates `audio` against `assets` and builds the compiled tracks (non-RT).
pub(crate) fn compile_audio(
    audio: &[SongAudioTrack],
    assets: &[SongAsset],
) -> Result<Vec<CompiledAudio>, crate::SongError> {
    use crate::SongError;
    for (k, a) in assets.iter().enumerate() {
        let ok = match a {
            SongAsset::Memory {
                data,
                frames,
                channels,
            } => {
                *channels >= 1
                    && data.len() == usize::from(*channels)
                    && data.iter().all(|c| c.len() as u64 == *frames)
            }
            SongAsset::Stream { channels, .. } => *channels >= 1,
        };
        if !ok {
            return Err(SongError::BadAsset(format!(
                "asset {k}: channel data does not match its channel count and frames"
            )));
        }
    }
    let shared: Arc<[SongAsset]> = assets.to_vec().into();
    let mut devices = std::collections::BTreeSet::new();
    let mut out = Vec::with_capacity(audio.len());
    for track in audio {
        if !devices.insert(track.device) {
            return Err(SongError::DuplicateAudioTrack(track.device));
        }
        let mut regions = track.regions.clone();
        regions.sort_by_key(|r| r.song_start_frame); // stable
        for r in &regions {
            let asset = assets
                .get(r.asset.0 as usize)
                .ok_or(SongError::UnknownAsset(r.asset))?;
            let _ = asset;
            let valid = r.frames >= 1
                && r.gain.is_finite()
                && r.song_start_frame.checked_add(r.frames).is_some()
                && r.source_offset.checked_add(r.frames).is_some()
                && r.clip_pos
                    .checked_add(r.frames)
                    .is_some_and(|e| e <= r.clip_frames)
                && r.fade_in <= r.clip_frames
                && r.fade_out <= r.clip_frames;
            if !valid {
                return Err(SongError::BadEvent(format!("audio region {r:?}")));
            }
        }
        if let Some(w) = regions
            .windows(2)
            .find(|w| w[0].end() > w[1].song_start_frame)
        {
            return Err(SongError::BadEvent(format!(
                "overlapping audio regions {:?} and {:?}",
                w[0], w[1]
            )));
        }
        let streams = regions
            .iter()
            .any(|r| assets[r.asset.0 as usize].is_stream());
        out.push(CompiledAudio {
            device: track.device,
            data: Arc::new(LaneData {
                regions,
                assets: Arc::clone(&shared),
            }),
            streams,
        });
    }
    Ok(out)
}

/// The RT view of one audio track while rendering a batch.
pub(crate) struct Renderer<'a> {
    pub(crate) underruns: &'a AtomicU64,
}

impl Renderer<'_> {
    /// Renders song frames `[a, b)` of `track` into `rt.pb` at batch offset `off`.
    pub(crate) fn segment(
        &self,
        track: &CompiledAudio,
        rt: &mut AudioRt,
        a: u64,
        b: u64,
        off: usize,
    ) {
        let data = &*track.data;
        let AudioRt { pb, stream } = rt;
        let mut cursor = a; // the stream lane has consumed song-order frames up to here
        let first = data.regions.partition_point(|r| r.end() <= a);
        for r in &data.regions[first..] {
            if r.song_start_frame >= b {
                break;
            }
            let lo = a.max(r.song_start_frame);
            let hi = b.min(r.end());
            let n = (hi - lo) as usize;
            let i0 = lo - r.song_start_frame;
            let src0 = r.source_offset + i0;
            let asset = &data.assets[r.asset.0 as usize];
            let valid = (n as u64).min(asset.frames().saturating_sub(src0)) as usize;
            let at = off + (lo - a) as usize;
            match asset {
                SongAsset::Memory { data: planes, .. } => {
                    let l = &planes[0];
                    let rr = &planes[planes.len().min(2) - 1];
                    for k in 0..valid {
                        let g = r.gain * r.fade_gain(i0 + k as u64);
                        let s = src0 as usize + k;
                        pb[0][at + k] = l[s] * g;
                        pb[1][at + k] = rr[s] * g;
                    }
                }
                SongAsset::Stream { .. } => {
                    if let Some(lane) = stream.as_mut() {
                        lane.skip((lo - cursor) as usize);
                        let missing = {
                            let (p0, p1) = pb.split_at_mut(1);
                            lane.take(
                                valid,
                                &mut p0[0][at..at + valid],
                                &mut p1[0][at..at + valid],
                            )
                        };
                        if missing > 0 {
                            self.underruns.fetch_add(1, Ordering::Relaxed);
                        }
                        lane.skip(n - valid);
                        cursor = hi;
                        for k in 0..valid {
                            let g = r.gain * r.fade_gain(i0 + k as u64);
                            pb[0][at + k] *= g;
                            pb[1][at + k] *= g;
                        }
                    }
                }
            }
        }
        if let Some(lane) = stream.as_mut() {
            lane.skip((b - cursor) as usize);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(clip_pos: u64, frames: u64, clip_frames: u64) -> SongAudioRegion {
        SongAudioRegion {
            asset: AssetId(0),
            song_start_frame: 1000,
            source_offset: 0,
            frames,
            gain: 1.0,
            fade_in: 100,
            fade_out: 200,
            fade_curve: SongFadeCurve::Linear,
            clip_pos,
            clip_frames,
        }
    }

    #[test]
    fn sin_quarter_matches_libm_to_double_precision() {
        for k in 0..=1000 {
            let t = f64::from(k) / 1000.0;
            let want = (t * std::f64::consts::FRAC_PI_2).sin();
            assert!((sin_quarter(t) - want).abs() < 1e-15, "t = {t}");
        }
        assert_eq!(sin_quarter(0.0).to_bits(), 0.0f64.to_bits());
        assert!((sin_quarter(1.0) - 1.0).abs() < 1e-15);
        assert_eq!(sin_quarter(7.0).to_bits(), sin_quarter(1.0).to_bits());
    }

    #[test]
    fn linear_fades_follow_the_reference_ramp() {
        let r = region(0, 1000, 1000);
        assert_eq!(r.fade_gain(0).to_bits(), 0.0f32.to_bits());
        assert_eq!(r.fade_gain(50).to_bits(), 0.5f32.to_bits());
        assert_eq!(r.fade_gain(100).to_bits(), 1.0f32.to_bits());
        assert_eq!(r.fade_gain(799).to_bits(), 1.0f32.to_bits());
        assert_eq!(r.fade_gain(800).to_bits(), 1.0f32.to_bits());
        assert_eq!(r.fade_gain(900).to_bits(), 0.5f32.to_bits());
        assert_eq!(r.fade_gain(999).to_bits(), (1.0f32 / 200.0).to_bits());
    }

    #[test]
    fn cut_regions_fade_within_the_original_clip() {
        // A region starting 600 frames into a 1000-frame clip: no fade-in, fade-out at the end.
        let r = region(600, 400, 1000);
        assert_eq!(r.fade_gain(0).to_bits(), 1.0f32.to_bits());
        assert_eq!(r.fade_gain(300).to_bits(), 0.5f32.to_bits());
    }

    #[test]
    fn compile_rejects_bad_regions_assets_and_overlaps() {
        use crate::SongError;
        let data: Arc<[Vec<f32>]> = vec![vec![0.0; 100]].into();
        let assets = [SongAsset::memory(data)];
        let good = SongAudioRegion {
            asset: AssetId(0),
            song_start_frame: 0,
            source_offset: 0,
            frames: 50,
            gain: 1.0,
            fade_in: 0,
            fade_out: 0,
            fade_curve: SongFadeCurve::Linear,
            clip_pos: 0,
            clip_frames: 50,
        };
        let track = |regions: Vec<SongAudioRegion>| SongAudioTrack {
            device: DeviceInstanceId(1),
            regions,
        };
        assert!(compile_audio(&[track(vec![good])], &assets).is_ok());
        let bad = |f: &dyn Fn(&mut SongAudioRegion)| {
            let mut r = good;
            f(&mut r);
            compile_audio(&[track(vec![r])], &assets)
        };
        assert!(matches!(
            bad(&|r| r.asset = AssetId(3)),
            Err(SongError::UnknownAsset(AssetId(3)))
        ));
        assert!(matches!(
            bad(&|r| r.frames = 0),
            Err(SongError::BadEvent(_))
        ));
        assert!(matches!(
            bad(&|r| r.gain = f32::NAN),
            Err(SongError::BadEvent(_))
        ));
        assert!(matches!(
            bad(&|r| r.fade_in = 51),
            Err(SongError::BadEvent(_))
        ));
        assert!(matches!(
            bad(&|r| r.clip_frames = 10),
            Err(SongError::BadEvent(_))
        ));
        let second = SongAudioRegion {
            song_start_frame: 49,
            ..good
        };
        assert!(matches!(
            compile_audio(&[track(vec![good, second])], &assets),
            Err(SongError::BadEvent(_))
        ));
        let touching = SongAudioRegion {
            song_start_frame: 50,
            ..good
        };
        let sorted = compile_audio(&[track(vec![touching, good])], &assets).unwrap();
        assert_eq!(sorted[0].data.regions[0].song_start_frame, 0);
        assert!(matches!(
            compile_audio(&[track(vec![good]), track(vec![good])], &assets),
            Err(SongError::DuplicateAudioTrack(DeviceInstanceId(1)))
        ));
        let broken = [SongAsset::Memory {
            data: vec![vec![0.0; 3]].into(),
            frames: 4,
            channels: 1,
        }];
        assert!(matches!(
            compile_audio(&[], &broken),
            Err(SongError::BadAsset(_))
        ));
    }
}
