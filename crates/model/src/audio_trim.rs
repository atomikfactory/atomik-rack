//! Moving the edges of audio clips: the tempo map between ticks and frames, and the maths of
//! trimming a clip's start or growing and cutting its end, with the source offset, fades, gain
//! envelope and warp markers that follow.
//!
//! Everything here is pure. The arrangement view maps pixels to these functions while a clip
//! edge is dragged, and the model uses them to plan the commands that move an edge without a
//! pointer (Extend to Next and Previous Clip).

use crate::audio_edit::slice_envelope;
use crate::song::{AudioClipContent, GainPoint, TempoEvent};
use crate::warp::{WarpMarker, WarpSettings};
use atomik_core::{Tempo, TempoPoint, TempoTable};

/// The song tempo map (steps and ramps) for fractional tick / frame conversions, the maths of
/// [`atomik_core::TempoMap`]. Positions are fixed in time: a tick maps to `seconds * sample_rate`
/// frames and back through the tempo events. Integer conversions give the same frames as
/// [`crate::Song::frame_at`].
#[derive(Debug, Clone, PartialEq)]
pub struct TempoMap {
    table: TempoTable,
}

impl Default for TempoMap {
    fn default() -> Self {
        Self::new(&[TempoEvent::step(0, 120.0)])
    }
}

impl TempoMap {
    /// Builds the map from the song's tempo events (sorted by tick, first at tick 0); a map the
    /// core cannot compile reads as constant at the first tempo, or at 120 BPM.
    pub fn new(tempo: &[TempoEvent]) -> Self {
        let points: Vec<TempoPoint> = tempo
            .iter()
            .map(|e| TempoPoint {
                tick: e.tick,
                bpm: e.bpm,
                ramp: e.ramp,
            })
            .collect();
        let table = TempoTable::new(&points).unwrap_or_else(|_| {
            let bpm = tempo.first().map_or(120.0, |e| e.bpm);
            let tempo = Tempo::new(bpm)
                .or_else(|_| Tempo::new(120.0))
                .unwrap_or_else(|_| unreachable!("120 BPM is a valid tempo"));
            TempoTable::constant(tempo)
        });
        Self { table }
    }

    /// Seconds at a (fractional) tick. Negative ticks give 0.
    pub fn seconds_at_tick(&self, tick: f64) -> f64 {
        self.table.map().seconds(tick).max(0.0)
    }

    /// The (fractional) tick at a time in seconds; negative times give 0.
    pub fn tick_at_seconds(&self, seconds: f64) -> f64 {
        self.table.map().tick(seconds, 1.0).max(0.0)
    }

    /// Frames at a (fractional) tick.
    pub fn frame_at_tick(&self, tick: f64, sample_rate: u32) -> f64 {
        self.seconds_at_tick(tick) * f64::from(sample_rate)
    }

    /// The (fractional) tick at a frame.
    pub fn tick_at_frame(&self, frame: f64, sample_rate: u32) -> f64 {
        self.tick_at_seconds(frame / f64::from(sample_rate))
    }

    /// The song frame of a tick, rounded: identical to [`crate::Song::frame_at`].
    pub fn frame_at(&self, tick: i64, sample_rate: u32) -> u64 {
        let f = self.frame_at_tick(tick as f64, sample_rate).round();
        if f > 0.0 { f as u64 } else { 0 }
    }
}

/// The asset frame that plays at a (fractional) song tick for a warped clip (warp enabled) that
/// starts at `clip_start` and plays from `source_offset`: the offset plus the time map of the
/// clip's warp settings (see [`atomik_model::warp`]).
pub fn warped_source(
    warp: &WarpSettings,
    sample_rate: u32,
    clip_start: i64,
    source_offset: u64,
    tick: f64,
) -> f64 {
    source_offset as f64 + warp.source_frames_at(sample_rate, tick - clip_start as f64)
}

/// The song tick (fractional) at which a warped clip plays asset frame `frame`: the inverse of
/// [`warped_source`].
pub fn warped_tick(
    warp: &WarpSettings,
    sample_rate: u32,
    clip_start: i64,
    source_offset: u64,
    frame: f64,
) -> f64 {
    clip_start as f64 + warp.ticks_at_source(sample_rate, frame - source_offset as f64)
}

/// [`trim_left`] for a warped clip (warp enabled): the offset becomes the asset frame the time
/// map plays at the new start, so the audio stays where it is in time, and the warp markers are
/// re-based (see [`WarpSettings::markers_after`]). The start is limited to tick 0, to the
/// earliest tick that still reads frame 0 of the asset, to the latest tick inside the asset and
/// to `min_len` before the end.
pub fn trim_left_warped(
    sample_rate: u32,
    (start, length, content): (i64, i64, &AudioClipContent),
    asset_frames: u64,
    wanted_start: i64,
    min_len: i64,
) -> (Trim, Vec<WarpMarker>) {
    let w = &content.warp;
    let end = start + length;
    let offset = content.source_offset;
    let tick_of = |frame: f64| warped_tick(w, sample_rate, start, offset, frame);
    let lo = (tick_of(0.0).ceil() as i64).max(0).min(start);
    let latest = tick_of(asset_frames as f64).floor() as i64;
    let hi = latest.min(end - min_len).max(lo);
    let new_start = wanted_start.clamp(lo, hi);
    let at = warped_source(w, sample_rate, start, offset, new_start as f64);
    let new_offset = (at.round().max(0.0) as u64).min(asset_frames);
    let markers = w.markers_after(new_start - start, offset, new_offset);
    (
        Trim {
            start: new_start,
            length: end - new_start,
            source_offset: new_offset,
        },
        markers,
    )
}

/// Fades after the clip changed length: the fade-in first, the fade-out gets what is left, so
/// `fade_in + fade_out <= frames`.
pub fn clamp_fades(fade_in: u64, fade_out: u64, frames: u64) -> (u64, u64) {
    let fi = fade_in.min(frames);
    (fi, fade_out.min(frames - fi))
}

/// The result of trimming the left edge of an audio clip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Trim {
    /// New start tick.
    pub start: i64,
    /// New length in ticks (the end stays where it was).
    pub length: i64,
    /// New first asset frame played.
    pub source_offset: u64,
}

/// Trims the left edge of the clip `(start, length, source_offset)` to `wanted_start` (already
/// snapped): the end stays and the offset moves by the frames the start moved, so the audio
/// stays where it is in time. The start is limited to tick 0, to the earliest tick whose
/// offset is still frame 0 of the asset, to the latest tick whose offset is still inside the
/// asset (`asset_frames`) and to `min_len` before the end.
pub fn trim_left(
    map: &TempoMap,
    sample_rate: u32,
    (start, length, source_offset): (i64, i64, u64),
    asset_frames: u64,
    wanted_start: i64,
    min_len: i64,
) -> Trim {
    let end = start + length;
    let start_frame = map.frame_at(start, sample_rate);
    let tick_of = |frame: u64| map.tick_at_frame(frame as f64, sample_rate).round() as i64;
    let earliest = start_frame.saturating_sub(source_offset);
    let mut lo = tick_of(earliest);
    while lo > 0 && map.frame_at(lo - 1, sample_rate) >= earliest {
        lo -= 1;
    }
    while map.frame_at(lo, sample_rate) < earliest {
        lo += 1;
    }
    let lo = lo.max(0);
    let latest = start_frame + asset_frames.saturating_sub(source_offset);
    let mut hi = tick_of(latest);
    while map.frame_at(hi, sample_rate) > latest {
        hi -= 1;
    }
    let hi = hi.min(end - min_len).max(lo);
    let new_start = wanted_start.clamp(lo, hi);
    let moved = i128::from(map.frame_at(new_start, sample_rate)) - i128::from(start_frame);
    let offset = (i128::from(source_offset) + moved).max(0) as u64;
    Trim {
        start: new_start,
        length: end - new_start,
        source_offset: offset,
    }
}

/// The envelope after the clip's start moved `moved` frames later (earlier when negative) with
/// the audio staying where it is: the points move the other way, so each still sits on the same
/// sound. The part of the envelope before the new start is cut and the value at the new start
/// becomes the first point.
pub fn envelope_after_trim(points: &[GainPoint], moved: i64) -> Vec<GainPoint> {
    if moved > 0 {
        slice_envelope(points, moved as u64, u64::MAX)
    } else {
        points
            .iter()
            .map(|p| GainPoint {
                frame: p.frame + moved.unsigned_abs(),
                gain_db: p.gain_db,
            })
            .collect()
    }
}

/// Trims the left edge of a *reversed* clip `(start, length, source_offset)` to `wanted_start`
/// (already snapped). A reversed clip plays the asset frames from `source_offset` up to
/// `source_offset + frames` backwards, so moving the start only cuts or restores the top of
/// that span: the source offset stays and the clip can grow no longer than the asset
/// allows (`asset_frames - source_offset` frames). The start is limited to tick 0 and to
/// `min_len` before the end.
pub fn trim_left_reversed(
    map: &TempoMap,
    sample_rate: u32,
    (start, length, source_offset): (i64, i64, u64),
    asset_frames: u64,
    wanted_start: i64,
    min_len: i64,
) -> Trim {
    let end = start + length;
    let end_frame = map.frame_at(end, sample_rate);
    // The longest clip the asset holds above the offset, as the earliest start tick.
    let longest = asset_frames.saturating_sub(source_offset);
    let earliest = end_frame.saturating_sub(longest);
    let tick_of = |frame: u64| map.tick_at_frame(frame as f64, sample_rate).round() as i64;
    let mut lo = tick_of(earliest);
    while lo > 0 && map.frame_at(lo - 1, sample_rate) >= earliest {
        lo -= 1;
    }
    while map.frame_at(lo, sample_rate) < earliest {
        lo += 1;
    }
    let lo = lo.max(0);
    let hi = (end - min_len).max(lo);
    let new_start = wanted_start.clamp(lo, hi);
    Trim {
        start: new_start,
        length: end - new_start,
        source_offset,
    }
}

/// Trims the left edge of a warped *reversed* clip `(start, length, source_offset)` to
/// `wanted_start` (already snapped). Such a clip plays its stretched span from the end, so its
/// start is the end of the forward stretch: moving it only cuts or restores that end, and the
/// source offset and the markers (which count from the span's other end) stay. The start is
/// limited to tick 0 and to `min_len` before the end.
pub fn trim_left_stretched_reversed(
    (start, length, source_offset): (i64, i64, u64),
    wanted_start: i64,
    min_len: i64,
) -> Trim {
    let end = start + length;
    let hi = (end - min_len).max(0);
    let new_start = wanted_start.clamp(0, hi);
    Trim {
        start: new_start,
        length: end - new_start,
        source_offset,
    }
}

/// The source offset of a *reversed* clip after its end moved so it lasts `new_frames` instead
/// of `old_frames`: the end of a reversed clip is the *low* end of its source span, so the offset
/// moves by what was cut (and can go no lower than frame 0, which also limits how long
/// the clip can grow). Returns the new offset and the frames the clip can last.
pub fn reversed_end_resize(source_offset: u64, old_frames: u64, new_frames: u64) -> (u64, u64) {
    let frames = new_frames.min(old_frames.saturating_add(source_offset));
    let offset = i128::from(source_offset) + i128::from(old_frames) - i128::from(frames);
    (offset.max(0) as u64, frames)
}

/// The frames an audio clip of `length` ticks starting at tick `start` lasts at `rate`.
pub fn clip_frames(tempo: &TempoMap, start: i64, length: i64, rate: u32) -> u64 {
    tempo
        .frame_at(start + length, rate)
        .saturating_sub(tempo.frame_at(start, rate))
}

/// The geometry and content an audio clip gets when its start is trimmed to song tick
/// `wanted`: the source offset follows (warped and reversed clips by their own rules), the
/// fades are clamped to the new length and the envelope stays on the sound it was drawn on.
pub fn trim_start_audio(
    tempo: &TempoMap,
    rate: u32,
    asset_frames: u64,
    (start, length, content): (i64, i64, &AudioClipContent),
    wanted: i64,
    min: i64,
) -> (i64, i64, AudioClipContent) {
    let orig = (start, length, content.source_offset);
    let mut markers = None;
    let t = if content.warp.enabled && content.reverse {
        // A warped clip played backwards plays its stretched span from the end: trimming its
        // start cuts the end of that span, so the source offset and the markers stay.
        trim_left_stretched_reversed(orig, wanted, min)
    } else if content.warp.enabled {
        let (t, m) = trim_left_warped(rate, (start, length, content), asset_frames, wanted, min);
        markers = Some(m);
        t
    } else if content.reverse {
        trim_left_reversed(tempo, rate, orig, asset_frames, wanted, min)
    } else {
        trim_left(tempo, rate, orig, asset_frames, wanted, min)
    };
    let frames = clip_frames(tempo, t.start, t.length, rate);
    let (fade_in, fade_out) = clamp_fades(content.fade_in, content.fade_out, frames);
    let moved = i128::from(tempo.frame_at(t.start, rate)) - i128::from(tempo.frame_at(start, rate));
    let mut trimmed = content.clone();
    if let Some(m) = markers {
        trimmed.warp.markers = m;
    }
    (
        t.start,
        t.length,
        AudioClipContent {
            source_offset: t.source_offset,
            fade_in,
            fade_out,
            envelope: envelope_after_trim(
                &content.envelope,
                moved.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64,
            ),
            ..trimmed
        },
    )
}

/// The length (in ticks, at most `wanted`) and the content an audio clip starting at tick
/// `start` and `length` ticks long gets when its end moves so it is `wanted` ticks long. The
/// fades follow the length, and a reversed clip moves its source offset with its end and grows
/// no further than frame 0 of its source (a warped reversed clip plays its stretched span from
/// the other end and keeps its offset).
pub fn resize_end_audio(
    tempo: &TempoMap,
    rate: u32,
    (start, length, content): (i64, i64, &AudioClipContent),
    wanted: i64,
    min: i64,
) -> (i64, AudioClipContent) {
    let mut new = wanted;
    let mut frames = clip_frames(tempo, start, new, rate);
    let mut source_offset = content.source_offset;
    if content.reverse && !content.warp.enabled {
        // The end of a reversed clip is the low end of its source span: the offset moves with
        // it, and the clip grows no further than frame 0.
        let old = clip_frames(tempo, start, length, rate);
        let (_, allowed) = reversed_end_resize(content.source_offset, old, frames);
        if allowed < frames {
            let end_frame = tempo.frame_at(start, rate) + allowed;
            let end = tempo.tick_at_frame(end_frame as f64, rate).round() as i64;
            new = (end - start).max(min);
            frames = clip_frames(tempo, start, new, rate);
        }
        source_offset = reversed_end_resize(content.source_offset, old, frames).0;
    }
    let (fade_in, fade_out) = clamp_fades(content.fade_in, content.fade_out, frames);
    (
        new,
        AudioClipContent {
            source_offset,
            fade_in,
            fade_out,
            ..content.clone()
        },
    )
}
