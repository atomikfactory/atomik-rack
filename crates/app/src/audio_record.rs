//! Audio recording rules: the pure parts, with no devices,
//! threads or files. The writer thread and the app glue build on them.
//!
//! # Latency and alignment
//!
//! The record tap tags every input frame with its host frame `h` (the engine reads it in the
//! batch that contains it). What the engine renders for host frame `h` is heard the engine's
//! 64-frame FIFO plus the output latency later, and what a performer plays in reaction to it
//! reaches the engine the input latency after that. So audio that shows up at input frame `h`
//! was played against the song as it was rendered at engine frame `h - compensation`, with
//!
//! ```text
//! compensation = input_latency + output_latency + 64 + user_offset      (record_offset_frames)
//! ```
//!
//! and [`record_offset_frames`] is that one formula. **Sign:** a positive value places the
//! recording *earlier* on the timeline (the usual case: latency makes the performer late); the
//! user offset adds to it, so a positive user offset moves recordings earlier and a negative
//! one later. The song position of input frame `h` is `position_at(h - compensation)` (see
//! [`split_laps`]).
//!
//! # Input gain and clip-safe
//!
//! An audio track has two stored params for its recording input ([`REC_GAIN_PARAM`], in dB, and
//! [`REC_CLIP_SAFE_PARAM`]). The gain is applied **to the recorded signal and to the monitor**
//! (the track's device scales the input it monitors by the same value), so the performer hears
//! what is written. The take's gain is the engine tap's ([`atomik_engine::TapControls`]), set
//! from these params when the take starts and every UI frame while it runs, so a turn of the
//! control is heard in the take from the next batch on and ramps without a step. Clip-safe puts
//! a limiter on the written take only; the monitor is not limited. A project written before the
//! params existed has none stored and records at 0 dB, clip-safe off, exactly as before.
//!
//! # Takes and laps
//!
//! A take's audio is one file per armed track holding every tapped frame from the first one to
//! the end, with gaps written as silence, so file frame `i` is engine frame `first + i`.
//! [`split_laps`] converts that range to *laps* (one per loop pass, like MIDI recording) of
//! consecutive song frames, dropping what lies before the transport started. A lap becomes one
//! asset and one clip ([`place_lap`] aligns it to whole ticks with a sub-tick `source_offset`).

use atomik_core::{BATCH, db_to_gain};
pub use atomik_core::{REC_CLIP_SAFE_PARAM, REC_GAIN_MAX_DB, REC_GAIN_MIN_DB, REC_GAIN_PARAM};
use atomik_device_sdk::Registry;
use atomik_engine::{SongPosition, TapChannel};
use atomik_model::{DeviceKey, INTERFACE_KEY, RackDoc};

/// Seconds of input the record tap's ring holds (the writer thread drains it continuously).
pub const TAP_SECONDS: usize = 4;

/// Frames of the engine's own FIFO between a batch's input and its output (one batch).
pub const ENGINE_FIFO_FRAMES: i64 = BATCH as i64;

/// The compensation, in frames, that moves a recording back onto the timeline: the input and
/// output latencies the host reports, the engine's 64-frame FIFO, and the user's offset (module
/// docs for the sign).
pub fn record_offset_frames(input_latency: u32, output_latency: u32, user_offset: i32) -> i64 {
    i64::from(input_latency)
        + i64::from(output_latency)
        + ENGINE_FIFO_FRAMES
        + i64::from(user_offset)
}

/// The recording input settings of an audio track.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InputSettings {
    /// Input gain in dB (the bottom of the range, [`REC_GAIN_MIN_DB`], is off).
    pub gain_db: f32,
    /// The take is limited so it never goes above 0 dBFS.
    pub clip_safe: bool,
}

impl Default for InputSettings {
    fn default() -> Self {
        Self {
            gain_db: 0.0,
            clip_safe: false,
        }
    }
}

impl InputSettings {
    /// The linear gain: 0 at the bottom of the range, otherwise the dB value clamped to the
    /// range (what the device's own param turns it into).
    pub fn gain(self) -> f32 {
        if self.gain_db.is_nan() {
            1.0
        } else if self.gain_db <= REC_GAIN_MIN_DB {
            0.0
        } else {
            db_to_gain(self.gain_db.min(REC_GAIN_MAX_DB))
        }
    }
}

/// The recording input settings of the audio track `device` as the document holds them (the
/// defaults for a device that is not an audio track or has none stored).
pub fn input_settings(doc: &RackDoc, device: &DeviceKey, registry: &Registry) -> InputSettings {
    let defaults = InputSettings::default();
    InputSettings {
        gain_db: doc
            .param_value(device, REC_GAIN_PARAM, registry)
            .unwrap_or(defaults.gain_db),
        clip_safe: doc
            .param_value(device, REC_CLIP_SAFE_PARAM, registry)
            .map_or(defaults.clip_safe, |v| v >= 0.5),
    }
}

/// The post-gain peak of the track's input: the largest of the host peaks (`host_peaks[k]` is
/// the peak of host input `k` since the last read) of its channels, times its gain. The peak of
/// a scaled signal is the scaled peak, so this is what the meter of the take would read.
pub fn input_peak(host_peaks: &[f32], input: TrackInput, settings: InputSettings) -> f32 {
    let pre = input
        .channels()
        .into_iter()
        .map(|c| host_peaks.get(c).copied().unwrap_or(0.0))
        .filter(|p| p.is_finite())
        .fold(0.0f32, f32::max);
    pre * settings.gain()
}

/// Where an audio track takes its recording from: host input channels (0-based).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackInput {
    /// One channel (also: both sockets cabled from the same channel, or only one cabled).
    Mono(usize),
    /// Left and right from two different channels.
    Stereo(usize, usize),
}

impl TrackInput {
    /// The host channels, left first.
    pub fn channels(self) -> Vec<usize> {
        match self {
            Self::Mono(a) => vec![a],
            Self::Stereo(a, b) => vec![a, b],
        }
    }
}

/// The input channels of the audio track `device` from its cables `interface:in_k` → `in_l` /
/// `in_r`: two different channels give stereo; the same channel on both, or only one socket
/// cabled, gives mono; none (or cables from other devices) gives `None`.
pub fn track_input(doc: &RackDoc, device: &DeviceKey) -> Option<TrackInput> {
    let from = |socket: &str| -> Option<usize> {
        let cable = doc
            .cables
            .iter()
            .find(|c| &c.to.device == device && c.to.socket == socket)?;
        if cable.from.device.0 != INTERFACE_KEY {
            return None;
        }
        let n: usize = cable.from.socket.strip_prefix("in_")?.parse().ok()?;
        n.checked_sub(1)
    };
    match (from("in_l"), from("in_r")) {
        (Some(l), Some(r)) if l != r => Some(TrackInput::Stereo(l, r)),
        (Some(c), _) | (None, Some(c)) => Some(TrackInput::Mono(c)),
        (None, None) => None,
    }
}

/// One armed track of a take.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedTrack {
    /// The audio track's device.
    pub device: DeviceKey,
    /// Its input.
    pub input: TrackInput,
    /// For each of the track's channels, its index in the tap's channel list.
    pub tap_index: Vec<usize>,
}

/// The channels a take taps and where each track finds its own.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TapPlan {
    /// The tapped channels: every track's channels in turn (left first), each through the strip
    /// of its track (its index in `tracks`). A host channel that two tracks record appears once
    /// per track, because each applies its own input gain.
    pub channels: Vec<TapChannel>,
    /// The tracks, in the order given.
    pub tracks: Vec<PlannedTrack>,
}

/// Plans one tap for `tracks`: each track's channels in turn, through the track's own strip.
pub fn plan_tap(tracks: &[(DeviceKey, TrackInput)]) -> TapPlan {
    let mut channels = Vec::new();
    let tracks = tracks
        .iter()
        .enumerate()
        .map(|(strip, (device, input))| {
            let first = channels.len();
            channels.extend(
                input
                    .channels()
                    .into_iter()
                    .map(|host| TapChannel { host, strip }),
            );
            PlannedTrack {
                device: device.clone(),
                input: *input,
                tap_index: (first..channels.len()).collect(),
            }
        })
        .collect();
    TapPlan { channels, tracks }
}

/// The engine range of one take's file and how to place it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TakeRange {
    /// Engine frame of file frame 0.
    pub first_engine_frame: u64,
    /// Frames in the file.
    pub frames: u64,
    /// The compensation ([`record_offset_frames`]).
    pub compensation: i64,
    /// Frames whose compensated engine frame is before this are dropped (the transport had not
    /// started yet).
    pub valid_from: u64,
}

/// A run of consecutive frames of one loop pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    /// Loop pass, counted from the first kept frame (0, 1, 2, ...).
    pub lap: u64,
    /// First file frame.
    pub first: u64,
    /// Frames.
    pub frames: u64,
    /// Song frame of the first frame.
    pub song_frame: u64,
}

/// Splits a take's file into laps. `position` converts an engine frame to a song position
/// (`SongSnapshot::position_at` with the latest playing state). Frame `i` of the file lies at
/// `position(first + i - compensation)`; frames whose compensated engine frame is before
/// `valid_from` (or negative) are dropped. Within a lap the song frames are consecutive.
pub fn split_laps(range: &TakeRange, position: &dyn Fn(u64) -> SongPosition) -> Vec<Segment> {
    let first = i128::from(range.first_engine_frame);
    let compensated = |i: u64| first + i128::from(i) - i128::from(range.compensation);
    // The first file frame whose compensated engine frame is not before `valid_from`.
    let lowest = i128::from(range.valid_from) + i128::from(range.compensation) - first;
    let Ok(start) = u64::try_from(lowest.max(0)) else {
        return Vec::new();
    };
    if start >= range.frames {
        return Vec::new();
    }
    let at = |i: u64| position(u64::try_from(compensated(i).max(0)).unwrap_or(0));
    let base = at(start).loop_count;
    let last = range.frames - 1;
    let mut out = Vec::new();
    let mut start = start;
    while start <= last {
        let here = at(start);
        let lap_end = if at(last).loop_count <= here.loop_count {
            range.frames
        } else {
            // Loop counts never decrease: bisect for the first frame of the next pass.
            let (mut lo, mut hi) = (start, last);
            while hi - lo > 1 {
                let mid = lo + (hi - lo) / 2;
                if at(mid).loop_count > here.loop_count {
                    hi = mid;
                } else {
                    lo = mid;
                }
            }
            hi
        };
        out.push(Segment {
            lap: here.loop_count.saturating_sub(base),
            first: start,
            frames: lap_end - start,
            song_frame: here.song_frame,
        });
        start = lap_end;
    }
    out
}

/// A lap placed on the song's tick grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LapPlacement {
    /// Clip start tick.
    pub start_tick: i64,
    /// Frames of the asset to skip so the audio starts on its song frame.
    pub source_offset: u64,
    /// Frames the clip plays from `source_offset`.
    pub frames: u64,
}

/// Places a lap that starts at song frame `song_frame` and has `frames` frames: the clip starts
/// at the first tick whose frame is at or after `song_frame` (`frame_at` maps a tick to its song
/// frame and never decreases), and `source_offset` skips the few frames before it, so every
/// sample plays at exactly its song frame. `None` when the lap ends before that tick.
pub fn place_lap(
    song_frame: u64,
    frames: u64,
    frame_at: &dyn Fn(i64) -> u64,
) -> Option<LapPlacement> {
    let tick = if frame_at(0) >= song_frame {
        0
    } else {
        // Grow until a tick reaches the frame, then bisect for the first one.
        let (mut lo, mut hi) = (0i64, 1i64);
        while frame_at(hi) < song_frame {
            lo = hi;
            hi = hi.checked_mul(2).filter(|h| *h < MAX_PLACE_TICK)?;
        }
        while hi - lo > 1 {
            let mid = lo + (hi - lo) / 2;
            if frame_at(mid) >= song_frame {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        hi
    };
    let source_offset = frame_at(tick) - song_frame;
    (source_offset < frames).then(|| LapPlacement {
        start_tick: tick,
        source_offset,
        frames: frames - source_offset,
    })
}

/// Search limit of [`place_lap`] (ticks): far beyond any song.
const MAX_PLACE_TICK: i64 = 1 << 40;

#[cfg(test)]
#[path = "audio_record_tests.rs"]
mod tests;
