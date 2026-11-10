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
//! batch offset, so loop wraps and locates are sample-accurate. Regions are sorted by start
//! with non-decreasing ends, so a segment finds its regions by binary search. Regions are
//! disjoint except where a crossfade mixes two of them (at most two at a time, validated by
//! `compile_audio`); the second region of an overlap *adds* to the first. For region frame `i`
//! (song frame `song_start_frame + i`) the output is
//!
//! `asset[source_offset + i] * gain(i)`   (`asset[source_offset - i]` when reversed)
//!
//! and silence once the source index leaves the asset (a forward region is cut at the asset's
//! end; a reversed one is silent before it reaches the asset's last frame). Mono assets play on
//! both channels; for assets with more than two channels the first two are played.
//!
//! `gain(i)` is `gain * fade(i)` in `f32`, then times the envelope value (only when the
//! region has an envelope) and the crossfade value (only inside a crossfade), in exactly this
//! order, so a region without envelope or crossfade is bit-identical to what the player has
//! always produced.
//!
//! # Fades, envelope and crossfades (bit-identical across platforms)
//!
//! `fade(i)` is the model's `AudioRegion::fade_gain`, evaluated in `f64` and rounded to `f32`:
//! with `n = clip_pos + i`, fade-in `curve(n / fade_in)` for `n < fade_in`, fade-out
//! `curve((clip_frames - n) / fade_out)` for `n >= clip_frames - fade_out`, multiplied. Linear
//! is `t`, Exponential `t * t`, SCurve `t * t * (3 - 2 * t)`: plain IEEE arithmetic, identical to
//! the model's. EqualPower is `sin(t * pi / 2)`, which the model computes with the platform's
//! `f64::sin`; the engine instead uses [`sin_quarter`], a 12-term Taylor polynomial in nested
//! Horner form (only IEEE `+ - * /` on `f64`, no libm call, no fused multiply-add), so every
//! platform rounds identically. Its truncation error is below `1e-20`, so after rounding to
//! `f32` it equals the model's value except, very rarely, one ulp when the exact value lies next
//! to a rounding boundary.
//!
//! The envelope is a list of `(clip frame, linear gain)` points per track (`env` of a region
//! indexes it): held before the first and after the last point, and between two points
//! `a + (b - a) * ((n - fa) / (fb - fa))` in `f64` rounded to `f32`, the model's
//! `envelope_gain`. A crossfade region has `x_in` frames of `curve(i / x_in)` at its start (the
//! later clip) or `x_out == frames` of `curve((x_out - i) / x_out)` (the tail of the earlier
//! clip); the pair is constant power with EqualPower.
//!
//! # Interim warps
//!
//! A warped clip normally plays a finished buffer rendered off the audio thread (a plain
//! region of that buffer). Until the buffer exists its region can carry an interim warp
//! (`SongAudioRegion::warp`): the engine reads the clip's *memory* asset through the clip's time
//! map (`(clip frame, asset frame)` knots, `SongAudioTrack::warp_maps`) with four-point Hermite
//! interpolation, so the timing follows the tempo and the markers at once. It is a resampling:
//! the pitch moves with the stretch ratio and a transposition is not heard. The read allocates
//! nothing (stack blocks) and streamed assets never take it. Offline renders are given the
//! finished buffer instead (the render crate builds interim regions only for the live song).
//!
//! When a song swap replaces a track that played an interim warp, the new track fades in over
//! 5 ms while the old one fades out (`Handoff`), so the finished buffer arrives without a click.
//!
//! # Streamed assets
//!
//! The streamer thread applies the same gains while it reads (see `stream.rs`): the ring
//! carries the *finished* output of the streamed regions (gain, fades, envelope, crossfade, mixed
//! where two overlap), and the audio thread copies it, then adds the memory regions.

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
    /// `t * t`.
    Exponential,
    /// `t * t * (3 - 2 * t)`.
    SCurve,
}

impl SongFadeCurve {
    /// The gain at position `t` in `[0, 1]`, in `f64` with platform-independent arithmetic.
    pub fn eval(self, t: f64) -> f64 {
        match self {
            Self::Linear => t,
            Self::EqualPower => sin_quarter(t),
            Self::Exponential => t * t,
            Self::SCurve => t * t * (3.0 - 2.0 * t),
        }
    }
}

/// One stretch of audio an audio track plays, in song frames (the model's `AudioRegion`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SongAudioRegion {
    /// Asset (index into `SongInput::assets`).
    pub asset: AssetId,
    /// Song frame where the region starts.
    pub song_start_frame: u64,
    /// Asset frame read at the first region frame (the highest one when `reverse`).
    pub source_offset: u64,
    /// Region length in frames (>= 1); cut earlier where the asset ends.
    pub frames: u64,
    /// Linear gain (finite).
    pub gain: f32,
    /// Fade-in length in frames of the original clip (at most `clip_frames`).
    pub fade_in: u64,
    /// Fade-out length in frames of the original clip (at most `clip_frames`).
    pub fade_out: u64,
    /// Shape of the fade-in.
    pub fade_curve: SongFadeCurve,
    /// Shape of the fade-out.
    pub fade_out_curve: SongFadeCurve,
    /// Frames from the original clip's start to this region's start.
    pub clip_pos: u64,
    /// Length in frames of the original clip (>= `clip_pos + frames`).
    pub clip_frames: u64,
    /// Plays the asset backwards: region frame `i` reads asset frame `source_offset - i`
    /// (needs `frames <= source_offset + 1`).
    pub reverse: bool,
    /// The gain envelope: an index into [`SongAudioTrack::envelopes`], or `None`.
    pub env: Option<u32>,
    /// Frames of crossfade-in at the region's start (0 for none, at most `frames`).
    pub x_in: u64,
    /// Frames of crossfade-out at the region's start (0 for none, at most `frames`).
    pub x_out: u64,
    /// Crossfade shape.
    pub x_curve: SongFadeCurve,
    /// An interim warp: an index into [`SongAudioTrack::warp_maps`], or `None`. The region then
    /// plays its (memory) asset through that time map (see the module docs, *Interim warps*).
    pub warp: Option<u32>,
}

impl SongAudioRegion {
    /// A plain forward region: no reverse, envelope or crossfade, and a clip equal to the
    /// region.
    pub fn plain(asset: AssetId, song_start_frame: u64, source_offset: u64, frames: u64) -> Self {
        Self {
            asset,
            song_start_frame,
            source_offset,
            frames,
            gain: 1.0,
            fade_in: 0,
            fade_out: 0,
            fade_curve: SongFadeCurve::Linear,
            fade_out_curve: SongFadeCurve::Linear,
            clip_pos: 0,
            clip_frames: frames,
            reverse: false,
            env: None,
            x_in: 0,
            x_out: 0,
            x_curve: SongFadeCurve::EqualPower,
            warp: None,
        }
    }

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
        let mut g = 1.0f64;
        if fading_in {
            g *= self.fade_curve.eval(n as f64 / self.fade_in as f64);
        }
        if fading_out {
            g *= self
                .fade_out_curve
                .eval(self.clip_frames.saturating_sub(n) as f64 / self.fade_out as f64);
        }
        g as f32
    }

    /// The crossfade gain at region frame `i` (the model's `AudioRegion::xfade_gain`).
    pub fn xfade_gain(&self, i: u64) -> f32 {
        let mut g = 1.0f64;
        if i < self.x_in {
            g *= self.x_curve.eval(i as f64 / self.x_in as f64);
        }
        if i < self.x_out {
            g *= self
                .x_curve
                .eval((self.x_out - i) as f64 / self.x_out as f64);
        }
        g as f32
    }

    /// Writes the whole gain (clip gain, fade, envelope, crossfade) of region frames
    /// `i0 .. i0 + out.len()` into `out`; `env` is this region's envelope (empty for none).
    pub(crate) fn gains(&self, env: &[(u64, f32)], i0: u64, out: &mut [f32]) {
        let crossfade = self.x_in > 0 || self.x_out > 0;
        let mut seg = env.partition_point(|p| p.0 <= self.clip_pos + i0);
        for (k, o) in out.iter_mut().enumerate() {
            let i = i0 + k as u64;
            let mut g = self.gain * self.fade_gain(i);
            if !env.is_empty() {
                let n = self.clip_pos + i;
                while seg < env.len() && env[seg].0 <= n {
                    seg += 1;
                }
                g *= if seg == 0 {
                    env[0].1
                } else if seg == env.len() {
                    env[seg - 1].1
                } else {
                    let (fa, ga) = env[seg - 1];
                    let (fb, gb) = env[seg];
                    let t = (n - fa) as f64 / (fb - fa) as f64;
                    (f64::from(ga) + (f64::from(gb) - f64::from(ga)) * t) as f32
                };
            }
            if crossfade {
                g *= self.xfade_gain(i);
            }
            *o = g;
        }
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
    /// Regions (any order; `compile_song` sorts them by start and end and rejects a mix of
    /// more than two at once or a region that ends before an earlier one does).
    pub regions: Vec<SongAudioRegion>,
    /// Gain envelopes as `(clip frame, linear gain)` points, strictly increasing in frame
    /// (`SongAudioRegion::env` indexes the list).
    pub envelopes: Vec<Vec<(u64, f32)>>,
    /// Time maps of the interim warps (`SongAudioRegion::warp` indexes the list): each is
    /// `(frame from the clip start, asset frame)` knots, at least two, finite and strictly
    /// increasing in the first value.
    pub warp_maps: Vec<Arc<[Knot]>>,
}

/// One knot of an interim warp's time map: `(frame from the clip start, asset frame)`.
pub type Knot = (f64, f64);

/// The immutable, shared part of a compiled audio track (also read by the streamer).
pub(crate) struct LaneData {
    /// Sorted by start with non-decreasing ends; at most two overlap at a frame.
    pub(crate) regions: Vec<SongAudioRegion>,
    pub(crate) envelopes: Vec<Vec<(u64, f32)>>,
    pub(crate) warp_maps: Vec<Arc<[Knot]>>,
    pub(crate) assets: Arc<[SongAsset]>,
}

impl LaneData {
    /// The envelope points of a region (empty for none).
    pub(crate) fn envelope(&self, r: &SongAudioRegion) -> &[(u64, f32)] {
        r.env.map_or(&[][..], |k| &self.envelopes[k as usize][..])
    }
}

/// A compiled audio track.
#[derive(Clone)]
pub(crate) struct CompiledAudio {
    pub(crate) device: DeviceInstanceId,
    pub(crate) data: Arc<LaneData>,
    /// True if any of its regions reads a streamed asset.
    pub(crate) streams: bool,
    /// True if any of its regions plays an interim warp.
    pub(crate) interim: bool,
}

/// Length of the crossfade from an interim warp to the buffer that replaces it, in seconds.
const HANDOFF_SECONDS: f64 = 0.005;

/// A crossfade in progress from the previous snapshot's version of a track: the old lane is
/// rendered next to the new one for `left` more frames and the two are mixed. The old lane data
/// is shared (an `Arc` clone taken at the swap), and it is dropped with the snapshot that
/// carries it, never on the audio thread.
pub(crate) struct Handoff {
    data: Arc<LaneData>,
    total: u32,
    left: u32,
}

impl Handoff {
    /// Mixes the old lane into song frames `[a, b)` (at batch offset `off`) of `pb`: the new
    /// output weighs `i / total` and the old one the rest, `i` counting frames since the swap.
    fn cross(&mut self, pb: &mut [Batch; 2], a: u64, b: u64, off: usize) {
        let n = (b - a) as usize;
        let mut old = [[0.0f32; BATCH]; 2];
        memory_regions(&self.data, &mut old, a, b, off, a);
        let done = (self.total - self.left) as usize;
        for k in 0..n.min(self.left as usize) {
            let t = (done + k) as f32 / self.total as f32;
            for (p, o) in pb.iter_mut().zip(&old) {
                p[off + k] = p[off + k] * t + o[off + k] * (1.0 - t);
            }
        }
        self.left = self.left.saturating_sub(n as u32);
    }
}

/// Per-track RT scratch: the playback pair, the stream lane (attached on submit) and the
/// crossfade from the previous snapshot, if one is running.
pub(crate) struct AudioRt {
    pub(crate) pb: [Batch; 2],
    pub(crate) stream: Option<StreamLaneRt>,
    pub(crate) handoff: Option<Handoff>,
    /// The regions playing while the player follows an external clock.
    pub(crate) follow: crate::audio_follow::FollowRt,
    /// A launcher slot plays on this track: the arrangement clips are silent.
    pub(crate) silent: bool,
}

impl Clone for AudioRt {
    /// A clone carries no stream lane (the lane belongs to the submitted snapshot) and no
    /// crossfade.
    fn clone(&self) -> Self {
        Self {
            pb: self.pb,
            stream: None,
            handoff: None,
            follow: Default::default(),
            silent: self.silent,
        }
    }
}

impl AudioRt {
    pub(crate) fn new() -> Self {
        Self {
            pb: [[0.0; BATCH]; 2],
            stream: None,
            handoff: None,
            follow: Default::default(),
            silent: false,
        }
    }
}

/// At a snapshot swap: every track of `new` that replaces a track of `old` playing an interim
/// warp starts a [`HANDOFF_SECONDS`] crossfade from it (a click-free swap to the finished
/// buffer). Tracks that read streamed assets are left out: the streamed output is not
/// rendered here. Allocation-free.
pub(crate) fn start_handoffs(
    old: &[CompiledAudio],
    new: &[CompiledAudio],
    rts: &mut [AudioRt],
    sr: f64,
) {
    let total = (sr * HANDOFF_SECONDS).round().max(1.0) as u32;
    for (n, rt) in new.iter().zip(rts.iter_mut()) {
        if n.streams || rt.handoff.is_some() {
            continue;
        }
        if let Some(o) = old
            .iter()
            .find(|o| o.device == n.device && o.interim && !o.streams)
            && !Arc::ptr_eq(&o.data, &n.data)
        {
            rt.handoff = Some(Handoff {
                data: Arc::clone(&o.data),
                total,
                left: total,
            });
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
        regions.sort_by_key(|r| (r.song_start_frame, r.end())); // stable
        for e in &track.envelopes {
            let sorted = e.windows(2).all(|w| w[0].0 < w[1].0);
            if !sorted || e.iter().any(|p| !p.1.is_finite()) {
                return Err(SongError::BadEvent(format!("audio envelope {e:?}")));
            }
        }
        for m in &track.warp_maps {
            let ordered = m.windows(2).all(|w| w[0].0 < w[1].0);
            let finite = m.iter().all(|k| k.0.is_finite() && k.1.is_finite());
            if m.len() < 2 || !ordered || !finite {
                return Err(SongError::BadEvent(format!("audio warp map {m:?}")));
            }
        }
        for r in &regions {
            let asset = assets
                .get(r.asset.0 as usize)
                .ok_or(SongError::UnknownAsset(r.asset))?;
            let warp_ok = r.warp.is_none_or(|w| {
                (w as usize) < track.warp_maps.len() && !r.reverse && !asset.is_stream()
            });
            let valid = warp_ok
                && r.frames >= 1
                && r.gain.is_finite()
                && r.song_start_frame.checked_add(r.frames).is_some()
                && r.source_offset.checked_add(r.frames).is_some()
                && (!r.reverse || r.frames <= r.source_offset + 1)
                && r.clip_pos
                    .checked_add(r.frames)
                    .is_some_and(|e| e <= r.clip_frames)
                && r.fade_in <= r.clip_frames
                && r.fade_out <= r.clip_frames
                && r.x_in <= r.frames
                && r.x_out <= r.frames
                && r.env.is_none_or(|k| (k as usize) < track.envelopes.len());
            if !valid {
                return Err(SongError::BadEvent(format!("audio region {r:?}")));
            }
        }
        // Overlaps mix at most two regions, and a region never ends before an earlier one
        // (the segment search relies on both).
        if let Some(w) = regions.windows(2).find(|w| w[0].end() > w[1].end()) {
            return Err(SongError::BadEvent(format!(
                "audio region {:?} lies inside {:?}",
                w[1], w[0]
            )));
        }
        if let Some(w) = regions
            .windows(3)
            .find(|w| w[0].end() > w[2].song_start_frame)
        {
            return Err(SongError::BadEvent(format!(
                "three overlapping audio regions {:?}, {:?} and {:?}",
                w[0], w[1], w[2]
            )));
        }
        let streams = regions
            .iter()
            .any(|r| assets[r.asset.0 as usize].is_stream());
        let interim = regions.iter().any(|r| r.warp.is_some());
        out.push(CompiledAudio {
            device: track.device,
            data: Arc::new(LaneData {
                regions,
                envelopes: track.envelopes.clone(),
                warp_maps: track.warp_maps.clone(),
                assets: Arc::clone(&shared),
            }),
            streams,
            interim,
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
        let AudioRt {
            pb,
            stream,
            handoff,
            silent,
            ..
        } = rt;
        let n_seg = (b - a) as usize;
        if *silent {
            // A launcher slot plays on this track instead: the stream stays in step, nothing sounds.
            if let Some(lane) = stream.as_mut() {
                lane.skip(n_seg);
            }
            return;
        }
        let live = data.regions[data.regions.partition_point(|r| r.end() <= a)..]
            .iter()
            .take_while(|r| r.song_start_frame < b);
        // Song frames below `written` already hold a region's output in `pb`: a later region
        // adds to them (a crossfade) and assigns to the rest, so a lone region is exactly what
        // it always was.
        let mut written = a;
        if let Some(lane) = stream.as_mut() {
            // The ring carries the finished streamed output in song order: take the whole
            // segment (or skip it) so it stays in step.
            let touches = live
                .clone()
                .any(|r| data.assets[r.asset.0 as usize].is_stream());
            if touches {
                let mut l = [0.0f32; BATCH];
                let mut r = [0.0f32; BATCH];
                if lane.take(n_seg, &mut l[..n_seg], &mut r[..n_seg]) > 0 {
                    self.underruns.fetch_add(1, Ordering::Relaxed);
                }
                pb[0][off..off + n_seg].copy_from_slice(&l[..n_seg]);
                pb[1][off..off + n_seg].copy_from_slice(&r[..n_seg]);
                written = b;
            } else {
                lane.skip(n_seg);
            }
        }
        memory_regions(data, pb, a, b, off, written);
        if let Some(h) = handoff.as_mut()
            && h.left > 0
        {
            h.cross(pb, a, b, off);
        }
    }
}

/// Renders the memory regions of `data` over song frames `[a, b)` into `pb` at batch offset
/// `off`. Song frames below `written` already hold output: a region adds to them and assigns
/// to the rest.
pub(crate) fn memory_regions(
    data: &LaneData,
    pb: &mut [Batch; 2],
    a: u64,
    b: u64,
    off: usize,
    mut written: u64,
) {
    let first = data.regions.partition_point(|r| r.end() <= a);
    for r in data.regions[first..]
        .iter()
        .take_while(|r| r.song_start_frame < b)
    {
        let lo = a.max(r.song_start_frame);
        let hi = b.min(r.end());
        let n = (hi - lo) as usize;
        let i0 = lo - r.song_start_frame;
        let at = off + (lo - a) as usize;
        let added = (written.saturating_sub(lo)).min(n as u64) as usize;
        if render_region(data, r, pb, i0, n, at, added) {
            written = written.max(hi);
        }
    }
}

/// Renders `n` frames of region `r`, from its frame `i0`, into `pb` at batch offset `at`: the
/// first `added` of them add to what is there and the rest assign. Returns false for a region
/// of a streamed asset (nothing is rendered). A frame outside the asset is silent.
pub(crate) fn render_region(
    data: &LaneData,
    r: &SongAudioRegion,
    pb: &mut [Batch; 2],
    i0: u64,
    n: usize,
    at: usize,
    added: usize,
) -> bool {
    let SongAsset::Memory {
        data: planes,
        frames: asset_frames,
        ..
    } = &data.assets[r.asset.0 as usize]
    else {
        return false;
    };
    let l = &planes[0];
    let rr = &planes[planes.len().min(2) - 1];
    if let Some(w) = r.warp {
        // An interim warp reads the asset through its time map for every region frame
        // (frames outside the asset are silent).
        let mut g = [0.0f32; BATCH];
        r.gains(data.envelope(r), i0, &mut g[..n]);
        let map = &data.warp_maps[w as usize];
        let mut seg = 0usize;
        for k in 0..n {
            let clip_frame = (r.clip_pos + i0 + k as u64) as f64;
            let s = map_source(map, clip_frame, &mut seg);
            let (vl, vr) = (hermite_at(l, s) * g[k], hermite_at(rr, s) * g[k]);
            if k < added {
                pb[0][at + k] += vl;
                pb[1][at + k] += vr;
            } else {
                pb[0][at + k] = vl;
                pb[1][at + k] = vr;
            }
        }
        return true;
    }
    // The region frames `k0..k1` that read inside the asset, and their source index.
    let (k0, k1, s0) = if r.reverse {
        let s0 = r.source_offset - i0;
        let lead = if *asset_frames == 0 {
            n as u64
        } else {
            s0.saturating_sub(asset_frames - 1).min(n as u64)
        };
        (lead as usize, n, s0)
    } else {
        let s0 = r.source_offset + i0;
        let valid = (n as u64).min(asset_frames.saturating_sub(s0)) as usize;
        (0, valid, s0)
    };
    if k1 > k0 {
        let mut g = [0.0f32; BATCH];
        r.gains(data.envelope(r), i0 + k0 as u64, &mut g[..k1 - k0]);
        for k in k0..k1 {
            let s = if r.reverse {
                (s0 - k as u64) as usize
            } else {
                s0 as usize + k
            };
            let gk = g[k - k0];
            let (vl, vr) = (l[s] * gk, rr[s] * gk);
            if k < added {
                pb[0][at + k] += vl;
                pb[1][at + k] += vr;
            } else {
                pb[0][at + k] = vl;
                pb[1][at + k] = vr;
            }
        }
    }
    true
}

/// The asset frame a warp map reads at clip frame `c`: linear between knots, continuing the
/// first or last segment outside them. `seg` is a cursor (the segment of the previous call:
/// start at 0 and call with ascending `c`; it is re-found when `c` moves backwards).
fn map_source(map: &[Knot], c: f64, seg: &mut usize) -> f64 {
    let last = map.len() - 2;
    *seg = (*seg).min(last);
    while *seg > 0 && c < map[*seg].0 {
        *seg -= 1;
    }
    while *seg < last && c >= map[*seg + 1].0 {
        *seg += 1;
    }
    let (k0, k1) = (map[*seg], map[*seg + 1]);
    k0.1 + (c - k0.0) * (k1.1 - k0.1) / (k1.0 - k0.0)
}

/// `plane` at the fractional frame `s` by four-point Hermite interpolation; frames outside the
/// plane read 0.
fn hermite_at(plane: &[f32], s: f64) -> f32 {
    let i = s.floor();
    let frames = plane.len() as f64;
    if i < -2.0 || i >= frames + 1.0 {
        return 0.0;
    }
    let at = |j: f64| {
        if j >= 0.0 && j < frames {
            plane[j as usize]
        } else {
            0.0
        }
    };
    let (y0, y1, y2, y3) = (at(i - 1.0), at(i), at(i + 1.0), at(i + 2.0));
    let t = (s - i) as f32;
    if t == 0.0 {
        return y1;
    }
    let c1 = 0.5 * (y2 - y0);
    let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
    ((c3 * t + c2) * t + c1) * t + y1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(clip_pos: u64, frames: u64, clip_frames: u64) -> SongAudioRegion {
        SongAudioRegion {
            fade_in: 100,
            fade_out: 200,
            clip_pos,
            clip_frames,
            ..SongAudioRegion::plain(AssetId(0), 1000, 0, frames)
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
    fn the_fade_out_has_its_own_shape() {
        let r = SongAudioRegion {
            fade_in: 100,
            fade_out: 100,
            fade_curve: SongFadeCurve::Linear,
            fade_out_curve: SongFadeCurve::Exponential,
            ..SongAudioRegion::plain(AssetId(0), 0, 0, 1000)
        };
        // Halfway through each fade: the linear one is a half, the squared one a quarter.
        assert_eq!(r.fade_gain(50), 0.5);
        assert_eq!(r.fade_gain(950), 0.25);
        assert_eq!(r.fade_gain(500), 1.0);
    }

    #[test]
    fn the_new_curves_are_plain_polynomials_and_the_gain_block_follows_the_pieces() {
        assert_eq!(
            SongFadeCurve::Exponential.eval(0.5).to_bits(),
            0.25f64.to_bits()
        );
        assert_eq!(SongFadeCurve::SCurve.eval(0.5).to_bits(), 0.5f64.to_bits());
        assert_eq!(
            SongFadeCurve::SCurve.eval(0.25).to_bits(),
            0.15625f64.to_bits()
        );
        let env = [(100u64, 0.5f32), (300, 1.0)];
        let r = SongAudioRegion {
            gain: 0.8,
            fade_in: 50,
            fade_out: 80,
            fade_curve: SongFadeCurve::SCurve,
            fade_out_curve: SongFadeCurve::SCurve,
            clip_pos: 20,
            clip_frames: 400,
            frames: 380,
            env: Some(0),
            x_in: 60,
            x_curve: SongFadeCurve::EqualPower,
            ..SongAudioRegion::plain(AssetId(0), 0, 0, 380)
        };
        let mut g = vec![0.0f32; 380];
        r.gains(&env, 0, &mut g);
        let mut tail = vec![0.0f32; 100];
        r.gains(&env, 280, &mut tail);
        for i in 0..380u64 {
            let n = r.clip_pos + i;
            let e = if n < 100 {
                0.5
            } else if n >= 300 {
                1.0
            } else {
                let t = (n - 100) as f64 / 200.0;
                (f64::from(0.5f32) + (1.0 - f64::from(0.5f32)) * t) as f32
            };
            let want = r.gain * r.fade_gain(i) * e * r.xfade_gain(i);
            assert_eq!(g[i as usize].to_bits(), want.to_bits(), "frame {i}");
            if i >= 280 {
                assert_eq!(
                    tail[(i - 280) as usize].to_bits(),
                    want.to_bits(),
                    "block {i}"
                );
            }
        }
        // Without envelope or crossfade the gain is exactly `gain * fade`.
        let plain = SongAudioRegion {
            gain: 0.3,
            fade_in: 10,
            ..SongAudioRegion::plain(AssetId(0), 0, 0, 40)
        };
        let mut g = [0.0f32; 40];
        plain.gains(&[], 0, &mut g);
        for (i, v) in g.iter().enumerate() {
            assert_eq!(v.to_bits(), (0.3f32 * plain.fade_gain(i as u64)).to_bits());
        }
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
        let good = SongAudioRegion::plain(AssetId(0), 0, 0, 50);
        let track = |regions: Vec<SongAudioRegion>| SongAudioTrack {
            device: DeviceInstanceId(1),
            regions,
            envelopes: vec![vec![(0, 1.0), (10, 0.5)]],
            warp_maps: Vec::new(),
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
        assert!(matches!(
            bad(&|r| {
                r.reverse = true; // 50 frames need a source offset of at least 49
                r.source_offset = 10;
            }),
            Err(SongError::BadEvent(_))
        ));
        assert!(matches!(bad(&|r| r.x_in = 51), Err(SongError::BadEvent(_))));
        assert!(matches!(
            bad(&|r| r.env = Some(1)),
            Err(SongError::BadEvent(_))
        ));
        assert!(bad(&|r| r.env = Some(0)).is_ok());
        let out_of_order = SongAudioTrack {
            regions: vec![SongAudioRegion {
                env: Some(0),
                ..good
            }],
            envelopes: vec![vec![(5, 1.0), (5, 0.5)]],
            ..track(Vec::new())
        };
        assert!(matches!(
            compile_audio(&[out_of_order], &assets),
            Err(SongError::BadEvent(_))
        ));
        // Two regions may overlap (a crossfade mixes them); a third at the same frame, or a
        // region that ends before an earlier one, is refused.
        let second = SongAudioRegion {
            song_start_frame: 49,
            ..good
        };
        assert!(compile_audio(&[track(vec![good, second])], &assets).is_ok());
        let third = SongAudioRegion {
            song_start_frame: 49,
            ..good
        };
        assert!(matches!(
            compile_audio(&[track(vec![good, second, third])], &assets),
            Err(SongError::BadEvent(_))
        ));
        let inside = SongAudioRegion {
            song_start_frame: 10,
            frames: 10,
            clip_frames: 10,
            ..good
        };
        assert!(matches!(
            compile_audio(&[track(vec![good, inside])], &assets),
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
