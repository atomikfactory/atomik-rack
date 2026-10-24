//! Audio clip maths for the arrangement: the tempo map between ticks and
//! frames, waveform columns, fade / gain / trim mapping and the input selector cable sets.
//!
//! Everything here is pure and headless; [`crate::ArrangementView`] only maps pixels to the
//! inputs of these functions.

use atomik_audio_file::Peaks;
use atomik_core::{Tempo, TempoPoint, TempoTable};
use atomik_model::song::{AUDIO_GAIN_DB_MAX, AUDIO_GAIN_DB_MIN};
use atomik_model::{
    AudioClipContent, Cable, Command, DeviceKey, GainPoint, INTERFACE_KEY, MAX_GAIN_POINTS,
    SocketRef, TempoEvent, WarpMarker, WarpSettings, slice_envelope,
};

/// The song tempo map (steps and ramps) for fractional tick / frame conversions, the maths of
/// [`atomik_core::TempoMap`]. Positions are fixed in time: a tick maps to `seconds * sample_rate`
/// frames and back through the tempo events. Integer conversions give the same frames as
/// [`atomik_model::Song::frame_at`].
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

    /// The song frame of a tick, rounded: identical to [`atomik_model::Song::frame_at`].
    pub fn frame_at(&self, tick: i64, sample_rate: u32) -> u64 {
        let f = self.frame_at_tick(tick as f64, sample_rate).round();
        if f > 0.0 { f as u64 } else { 0 }
    }
}

/// The asset frame that plays at a (fractional) song tick for a clip starting at `clip_start`
/// that plays from `source_offset`: the offset plus the real time since the clip start
/// (audio is never stretched, so tempo changes only move where frames land on the
/// grid). Below the offset (even negative) for ticks before the clip start.
pub fn source_frame(
    map: &TempoMap,
    sample_rate: u32,
    clip_start: i64,
    source_offset: u64,
    tick: f64,
) -> f64 {
    source_offset as f64 + map.frame_at_tick(tick, sample_rate)
        - map.frame_at_tick(clip_start as f64, sample_rate)
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

/// Min / max of one waveform pixel column: the peaks of `channel` over the source frames
/// `[from, to)`, merging every bucket the range touches (at least one). Frames at or after
/// `limit` (the asset end) are not drawn; `None` when nothing of the range is left, the
/// channel is missing or the range lies before frame 0.
pub fn column_peak(
    peaks: &Peaks,
    channel: usize,
    from: f64,
    to: f64,
    limit: u64,
) -> Option<(f32, f32)> {
    let buckets = peaks.channels.get(channel)?;
    let to = to.min(limit as f64);
    if to <= 0.0 || from >= to {
        return None;
    }
    let size = f64::from(peaks.frames_per_bucket.max(1));
    let first = (from.max(0.0) / size).floor() as usize;
    let last = ((to / size).ceil() as usize)
        .max(first + 1)
        .min(buckets.len());
    let slice = buckets.get(first..last)?;
    if slice.is_empty() {
        return None;
    }
    Some(
        slice
            .iter()
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), (a, b)| {
                (lo.min(*a), hi.max(*b))
            }),
    )
}

/// Lowest gain the gain handle reaches, in dB (the model accepts -120 and below).
pub const GAIN_MIN_DB: f32 = -60.0;
/// Highest gain the gain handle reaches, in dB (the model accepts up to +24).
pub const GAIN_MAX_DB: f32 = 12.0;
/// Slowdown of a gain drag with Shift held.
const FINE_FACTOR: f64 = 0.1;

/// The y of a gain on the handle scale: [`GAIN_MAX_DB`] at `top`, [`GAIN_MIN_DB`] at `bottom`
/// (linear in dB). Gains outside the range sit on the nearer end.
pub fn gain_to_y(db: f32, top: f64, bottom: f64) -> f64 {
    let range = f64::from(GAIN_MAX_DB - GAIN_MIN_DB);
    let n = (f64::from(GAIN_MAX_DB) - f64::from(db)).clamp(0.0, range) / range;
    top + n * (bottom - top)
}

/// The gain after dragging the handle `dy` pixels down (negative: up) from `orig_db` on a
/// scale `height` pixels tall. Rounded to 0.1 dB (0.01 dB with `fine`, which also slows the
/// drag to a tenth), clamped to [`GAIN_MIN_DB`]..=[`GAIN_MAX_DB`].
pub fn gain_drag(orig_db: f32, dy: f64, height: f64, fine: bool) -> f32 {
    if height <= 0.0 {
        return orig_db;
    }
    let per_px = f64::from(GAIN_MAX_DB - GAIN_MIN_DB) / height;
    let speed = if fine { FINE_FACTOR } else { 1.0 };
    let step = if fine { 100.0 } else { 10.0 };
    let db = f64::from(orig_db) - dy * per_px * speed;
    let db = (db * step).round() / step;
    (db as f32).clamp(GAIN_MIN_DB, GAIN_MAX_DB)
}

/// A fade-in of `orig` frames dragged by `delta` frames: within `0..=frames - fade_out`.
pub fn drag_fade_in(orig: u64, delta: i64, frames: u64, fade_out: u64) -> u64 {
    let max = frames.saturating_sub(fade_out);
    let v = i128::from(orig) + i128::from(delta);
    v.clamp(0, i128::from(max)) as u64
}

/// A fade-out of `orig` frames dragged by `delta` frames in the direction of the time axis
/// (dragging left lengthens it): within `0..=frames - fade_in`.
pub fn drag_fade_out(orig: u64, delta: i64, frames: u64, fade_in: u64) -> u64 {
    let max = frames.saturating_sub(fade_in);
    let v = i128::from(orig) - i128::from(delta);
    v.clamp(0, i128::from(max)) as u64
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

/// The gain in dB at a y on the handle scale (the inverse of [`gain_to_y`]), rounded to
/// 0.1 dB and clamped to [`GAIN_MIN_DB`]..=[`GAIN_MAX_DB`].
pub fn y_to_gain(y: f64, top: f64, bottom: f64) -> f32 {
    if bottom <= top {
        return 0.0;
    }
    let range = f64::from(GAIN_MAX_DB - GAIN_MIN_DB);
    let n = ((y - top) / (bottom - top)).clamp(0.0, 1.0);
    let db = f64::from(GAIN_MAX_DB) - n * range;
    ((db * 10.0).round() / 10.0) as f32
}

/// The envelope after adding a point at `frame` with `gain_db` (a point already at that frame
/// takes the new gain), and the position of the new point. Full envelopes ([`MAX_GAIN_POINTS`])
/// are returned unchanged with the nearest point's position.
pub fn envelope_insert(points: &[GainPoint], frame: u64, gain_db: f32) -> (Vec<GainPoint>, usize) {
    let gain_db = gain_db.clamp(AUDIO_GAIN_DB_MIN, AUDIO_GAIN_DB_MAX);
    let mut list = points.to_vec();
    let at = list.partition_point(|p| p.frame < frame);
    if list.get(at).is_some_and(|p| p.frame == frame) {
        list[at].gain_db = gain_db;
    } else if list.len() < MAX_GAIN_POINTS {
        list.insert(at, GainPoint { frame, gain_db });
    } else {
        return (list, at.min(points.len().saturating_sub(1)));
    }
    (list, at)
}

/// The envelope after moving point `index` to `frame` and `gain_db`: the frame stays strictly
/// between its neighbours (and at least 0), the gain inside the handle scale.
pub fn envelope_move(
    points: &[GainPoint],
    index: usize,
    frame: i64,
    gain_db: f32,
) -> Vec<GainPoint> {
    let mut list = points.to_vec();
    let Some(p) = list.get_mut(index) else {
        return list;
    };
    let lo = if index == 0 {
        0
    } else {
        points[index - 1].frame.saturating_add(1)
    };
    let hi = points
        .get(index + 1)
        .map_or(u64::MAX >> 24, |n| n.frame.saturating_sub(1))
        .max(lo);
    p.frame = frame.clamp(lo as i64, hi as i64) as u64;
    p.gain_db = gain_db.clamp(GAIN_MIN_DB, GAIN_MAX_DB);
    list
}

/// The envelope without point `index`.
pub fn envelope_remove(points: &[GainPoint], index: usize) -> Vec<GainPoint> {
    let mut list = points.to_vec();
    if index < list.len() {
        list.remove(index);
    }
    list
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

/// The source offset of a *reversed* clip after its end moved so it lasts `new_frames` instead
/// of `old_frames`: the end of a reversed clip is the *low* end of its source span, so the offset
/// moves by what was cut (and can go no lower than frame 0, which also limits how long
/// the clip can grow). Returns the new offset and the frames the clip can last.
pub fn reversed_end_resize(source_offset: u64, old_frames: u64, new_frames: u64) -> (u64, u64) {
    let frames = new_frames.min(old_frames.saturating_add(source_offset));
    let offset = i128::from(source_offset) + i128::from(old_frames) - i128::from(frames);
    (offset.max(0) as u64, frames)
}

/// Vertical bands `(centre y, half height)` of the waveform channels in `top..bottom`: one
/// band for mono, two stacked halves for stereo (channel 0 on top). Channels beyond the
/// second are not drawn.
pub fn channel_bands(top: f64, bottom: f64, channels: usize) -> Vec<(f64, f64)> {
    let n = channels.min(2);
    let h = (bottom - top) / n.max(1) as f64;
    (0..n)
        .map(|i| (top + h * (i as f64 + 0.5), h / 2.0))
        .collect()
}

/// What feeds an audio track from the host inputs, derived from the cables into its `in_l` /
/// `in_r` sockets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputSel {
    /// Nothing is cabled.
    Off,
    /// One input (1-based) feeds both sides.
    Mono(u16),
    /// Two inputs feed left and right.
    Pair(u16, u16),
    /// Only one side is cabled, or the cables do not come from the host inputs as a whole.
    Partial {
        /// Input cabled to `in_l`.
        left: Option<u16>,
        /// Input cabled to `in_r`.
        right: Option<u16>,
    },
}

impl InputSel {
    /// The inputs feeding `(in_l, in_r)`.
    pub fn sides(self) -> (Option<u16>, Option<u16>) {
        match self {
            InputSel::Off => (None, None),
            InputSel::Mono(k) => (Some(k), Some(k)),
            InputSel::Pair(a, b) => (Some(a), Some(b)),
            InputSel::Partial { left, right } => (left, right),
        }
    }
}

/// The input selection of `track` from the document cables (interface `in_k` into `in_l` /
/// `in_r`; other cables are ignored).
pub fn input_selection(cables: &[Cable], track: &DeviceKey) -> InputSel {
    let side = |socket: &str| {
        cables
            .iter()
            .find(|c| {
                &c.to.device == track && c.to.socket == socket && c.from.device.0 == INTERFACE_KEY
            })
            .and_then(|c| c.from.socket.strip_prefix("in_")?.parse::<u16>().ok())
    };
    match (side("in_l"), side("in_r")) {
        (None, None) => InputSel::Off,
        (Some(l), Some(r)) if l == r => InputSel::Mono(l),
        (Some(l), Some(r)) => InputSel::Pair(l, r),
        (left, right) => InputSel::Partial { left, right },
    }
}

/// The choices of the input menu for `channels` host inputs: off, every input as mono, then
/// the pairs 1+2, 3+4, ... that fit.
pub fn input_options(channels: u16) -> Vec<InputSel> {
    let mut out = vec![InputSel::Off];
    out.extend((1..=channels).map(InputSel::Mono));
    out.extend((1..=channels / 2).map(|p| InputSel::Pair(2 * p - 1, 2 * p)));
    out
}

/// The menu / header label of an input selection (`Off`, `In 3`, `In 1+2`, `In 2 R`).
pub fn input_label(sel: InputSel) -> String {
    match sel {
        InputSel::Mono(k) => format!("In {k}"),
        InputSel::Pair(a, b) => format!("In {a}+{b}"),
        InputSel::Partial {
            left: Some(k),
            right: None,
        } => format!("In {k} L"),
        InputSel::Partial {
            left: None,
            right: Some(k),
        } => format!("In {k} R"),
        InputSel::Partial { left, right } => match (left, right) {
            (Some(a), Some(b)) => format!("In {a}+{b}"),
            _ => "Off".to_string(),
        },
        InputSel::Off => "Off".to_string(),
    }
}

/// The commands that turn the input selection `current` of `track` into `target`: the cables
/// that differ are disconnected first, then the new ones connected (a mono input feeds both
/// sockets). Empty when nothing differs.
pub fn input_change(track: &DeviceKey, current: InputSel, target: InputSel) -> Vec<Command> {
    let at = |socket: &str| SocketRef {
        device: track.clone(),
        socket: socket.to_string(),
    };
    let from = |k: u16| SocketRef {
        device: DeviceKey(INTERFACE_KEY.to_string()),
        socket: format!("in_{k}"),
    };
    let (cl, cr) = current.sides();
    let (tl, tr) = target.sides();
    let sides = [("in_l", cl, tl), ("in_r", cr, tr)];
    let mut out: Vec<Command> = sides
        .iter()
        .filter(|(_, cur, want)| cur.is_some() && cur != want)
        .map(|(socket, _, _)| Command::Disconnect { to: at(socket) })
        .collect();
    out.extend(
        sides
            .iter()
            .filter(|(_, cur, want)| want.is_some() && cur != want)
            .filter_map(|(socket, _, want)| {
                want.map(|k| {
                    Command::Connect(Cable {
                        from: from(k),
                        to: at(socket),
                    })
                })
            }),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_core::PPQ;
    use atomik_model::TempoEvent;

    fn map(events: &[(i64, f64)]) -> TempoMap {
        let ev: Vec<TempoEvent> = events
            .iter()
            .map(|&(tick, bpm)| TempoEvent::step(tick, bpm))
            .collect();
        TempoMap::new(&ev)
    }

    #[test]
    fn frames_follow_the_tempo_map() {
        // 120 BPM: one beat is half a second.
        let m = map(&[(0, 120.0)]);
        assert_eq!(m.frame_at(PPQ, 48_000), 24_000);
        assert_eq!(m.frame_at(-5, 48_000), 0);
        // Tempo change at beat 4 to 60 BPM: the next beat takes a whole second.
        let m = map(&[(0, 120.0), (4 * PPQ, 60.0)]);
        assert_eq!(m.frame_at(4 * PPQ, 48_000), 96_000);
        assert_eq!(m.frame_at(5 * PPQ, 48_000), 144_000);
    }

    #[test]
    fn ticks_invert_frames_across_tempo_changes() {
        let m = map(&[(0, 120.0), (4 * PPQ, 60.0), (8 * PPQ, 90.0)]);
        for tick in [
            0,
            1,
            PPQ,
            4 * PPQ - 1,
            4 * PPQ,
            4 * PPQ + 7,
            8 * PPQ,
            9 * PPQ + 123,
        ] {
            let f = m.frame_at(tick, 48_000);
            let back = m.tick_at_frame(f as f64, 48_000).round() as i64;
            assert_eq!(back, tick, "tick {tick} frame {f}");
        }
        assert_eq!(m.tick_at_frame(-100.0, 48_000), 0.0);
    }

    #[test]
    fn seconds_and_ticks_are_continuous_at_a_tempo_change() {
        let m = map(&[(0, 120.0), (4 * PPQ, 60.0)]);
        let before = m.seconds_at_tick(4.0 * PPQ as f64 - 1e-6);
        let after = m.seconds_at_tick(4.0 * PPQ as f64 + 1e-6);
        assert!((after - before).abs() < 1e-6);
        assert!((m.tick_at_seconds(3.0) - 5.0 * PPQ as f64).abs() < 1e-6);
    }

    #[test]
    fn source_frames_follow_the_offset_and_the_tempo_map() {
        let m = map(&[(0, 120.0), (6 * PPQ, 60.0)]);
        let sr = 48_000;
        let at = |tick: f64| source_frame(&m, sr, 4 * PPQ, 1_000, tick);
        // The clip start plays the offset.
        assert_eq!(at(4.0 * PPQ as f64), 1_000.0);
        // Two beats at 120 BPM = 48000 frames.
        assert_eq!(at(6.0 * PPQ as f64), 49_000.0);
        // Then one beat per second: one more beat = 48000 frames.
        assert_eq!(at(7.0 * PPQ as f64), 97_000.0);
        // Fractional ticks interpolate; before the clip start the frame is below the offset.
        assert!((at(4.5 * PPQ as f64) - (1_000.0 + 12_000.0)).abs() < 1e-6);
        assert!(at(3.0 * PPQ as f64) < 1_000.0);
        // A clip starting inside the slow part maps its own start to the offset.
        let slow = source_frame(&m, sr, 7 * PPQ, 500, 8.0 * PPQ as f64);
        assert_eq!(slow, 500.0 + 48_000.0);
    }

    #[test]
    fn column_peak_merges_the_buckets_a_pixel_covers() {
        let peaks = atomik_audio_file::Peaks {
            frames_per_bucket: 100,
            channels: vec![vec![(-0.1, 0.2), (-0.5, 0.3), (-0.2, 0.9), (0.0, 0.1)]],
        };
        // One bucket.
        assert_eq!(column_peak(&peaks, 0, 0.0, 100.0, 400), Some((-0.1, 0.2)));
        // A range inside the second bucket still returns that bucket.
        assert_eq!(column_peak(&peaks, 0, 120.0, 130.0, 400), Some((-0.5, 0.3)));
        // Two and a half buckets.
        assert_eq!(column_peak(&peaks, 0, 50.0, 250.0, 400), Some((-0.5, 0.9)));
        // Clipped to the asset end: frames past `limit` are not drawn.
        assert_eq!(column_peak(&peaks, 0, 200.0, 400.0, 250), Some((-0.2, 0.9)));
        assert_eq!(column_peak(&peaks, 0, 250.0, 300.0, 250), None);
        // Missing channel and empty range.
        assert_eq!(column_peak(&peaks, 1, 0.0, 100.0, 400), None);
        assert_eq!(column_peak(&peaks, 0, -50.0, -10.0, 400), None);
    }

    #[test]
    fn gain_maps_to_y_and_drags_with_fine_control() {
        assert_eq!(gain_to_y(GAIN_MAX_DB, 10.0, 82.0), 10.0);
        assert_eq!(gain_to_y(GAIN_MIN_DB, 10.0, 82.0), 82.0);
        assert!((gain_to_y(0.0, 10.0, 82.0) - 22.0).abs() < 1e-9);
        // Out-of-range gains clamp to the handle range.
        assert_eq!(gain_to_y(40.0, 10.0, 82.0), 10.0);
        assert_eq!(gain_to_y(-100.0, 10.0, 82.0), 82.0);
        // 1 px = 1 dB over a 72 px track; up is louder; coarse rounds to 0.1 dB.
        assert!((gain_drag(0.0, -7.23, 72.0, false) - 7.2).abs() < 1e-5);
        assert!((gain_drag(-3.0, 4.0, 72.0, false) + 7.0).abs() < 1e-5);
        // Shift = a tenth of the speed, 0.01 dB steps.
        assert!((gain_drag(0.0, -7.23, 72.0, true) - 0.72).abs() < 1e-5);
        // Clamped to the range.
        assert_eq!(gain_drag(10.0, -100.0, 72.0, false), GAIN_MAX_DB);
        assert_eq!(gain_drag(-50.0, 500.0, 72.0, false), GAIN_MIN_DB);
        assert_eq!(gain_drag(0.0, 5.0, 0.0, false), 0.0, "no height, no change");
    }

    #[test]
    fn fades_clamp_to_the_clip_and_each_other() {
        assert_eq!(drag_fade_in(100, 50, 1000, 0), 150);
        assert_eq!(drag_fade_in(100, -500, 1000, 0), 0);
        assert_eq!(drag_fade_in(100, 5000, 1000, 300), 700);
        assert_eq!(drag_fade_in(100, 5000, 1000, 5000), 0, "over-long fade-out");
        // Dragging the out handle left (negative x delta) lengthens the fade-out.
        assert_eq!(drag_fade_out(100, -50, 1000, 0), 150);
        assert_eq!(drag_fade_out(100, 500, 1000, 0), 0);
        assert_eq!(drag_fade_out(100, -5000, 1000, 400), 600);
        assert_eq!(clamp_fades(600, 600, 1000), (600, 400));
        assert_eq!(clamp_fades(2000, 5, 1000), (1000, 0));
        assert_eq!(clamp_fades(10, 20, 1000), (10, 20));
    }

    #[test]
    fn left_trim_keeps_the_audio_in_place() {
        let m = map(&[(0, 120.0)]);
        let sr = 48_000;
        let (start, len, off) = (4 * PPQ, 4 * PPQ, 48_000);
        // One beat earlier: one beat (24000 frames) more of the file at the front.
        let t = trim_left(&m, sr, (start, len, off), 1_000_000, 3 * PPQ, 960);
        assert_eq!(
            (t.start, t.length, t.source_offset),
            (3 * PPQ, 5 * PPQ, 24_000)
        );
        // Never before frame 0 of the asset: the earliest start is 2 beats.
        let t = trim_left(&m, sr, (start, len, off), 1_000_000, 0, 960);
        assert_eq!((t.start, t.length, t.source_offset), (2 * PPQ, 6 * PPQ, 0));
        // Inward trim moves the offset forward.
        let t = trim_left(&m, sr, (start, len, off), 1_000_000, 6 * PPQ, 960);
        assert_eq!(
            (t.start, t.length, t.source_offset),
            (6 * PPQ, 2 * PPQ, 96_000)
        );
        // Not past the asset end: 100000 frames leaves 52000 after the offset.
        let t = trim_left(&m, sr, (start, len, off), 100_000, 7 * PPQ, 960);
        assert_eq!((t.start, t.source_offset), (94_720, 100_000));
        assert_eq!(t.length, 8 * PPQ - 94_720);
        // Not past the clip end minus the minimum length.
        let t = trim_left(&m, sr, (start, len, off), 10_000_000, 9 * PPQ, 960);
        assert_eq!((t.start, t.length), (8 * PPQ - 960, 960));
        // Not before tick 0 even when the offset would allow it.
        let t = trim_left(&m, sr, (PPQ, len, 1_000_000), 2_000_000, -100, 960);
        assert_eq!(t.start, 0);
        assert_eq!(t.source_offset, 1_000_000 - 24_000);
    }

    #[test]
    fn stereo_bands_split_the_body() {
        assert_eq!(channel_bands(10.0, 50.0, 1), vec![(30.0, 20.0)]);
        assert_eq!(
            channel_bands(10.0, 50.0, 2),
            vec![(20.0, 10.0), (40.0, 10.0)]
        );
        assert_eq!(
            channel_bands(10.0, 50.0, 6).len(),
            2,
            "extra channels are not drawn"
        );
        assert!(channel_bands(10.0, 50.0, 0).is_empty());
    }

    fn cable(from: u16, to: &str) -> atomik_model::Cable {
        let sref = |d: &str, s: String| atomik_model::SocketRef {
            device: atomik_model::DeviceKey(d.into()),
            socket: s,
        };
        atomik_model::Cable {
            from: sref("interface", format!("in_{from}")),
            to: sref("audio-track", to.into()),
        }
    }

    fn track() -> atomik_model::DeviceKey {
        atomik_model::DeviceKey("audio-track".into())
    }

    #[test]
    fn input_selection_is_derived_from_the_cables() {
        assert_eq!(input_selection(&[], &track()), InputSel::Off);
        let mono = [cable(3, "in_l"), cable(3, "in_r")];
        assert_eq!(input_selection(&mono, &track()), InputSel::Mono(3));
        let pair = [cable(1, "in_l"), cable(2, "in_r")];
        assert_eq!(input_selection(&pair, &track()), InputSel::Pair(1, 2));
        let half = [cable(4, "in_r")];
        assert_eq!(
            input_selection(&half, &track()),
            InputSel::Partial {
                left: None,
                right: Some(4)
            }
        );
        // Cables of other tracks and from other devices are ignored.
        let mut other = cable(1, "in_l");
        other.to.device = atomik_model::DeviceKey("other".into());
        let mut synth = cable(1, "in_r");
        synth.from.device = atomik_model::DeviceKey("synth".into());
        assert_eq!(input_selection(&[other, synth], &track()), InputSel::Off);
    }

    #[test]
    fn input_options_and_labels() {
        assert_eq!(input_options(0), vec![InputSel::Off]);
        assert_eq!(
            input_options(2),
            vec![
                InputSel::Off,
                InputSel::Mono(1),
                InputSel::Mono(2),
                InputSel::Pair(1, 2)
            ]
        );
        let five = input_options(5);
        assert_eq!(five.len(), 1 + 5 + 2, "off, 5 monos and pairs 1+2 and 3+4");
        assert_eq!(five.last(), Some(&InputSel::Pair(3, 4)));
        assert_eq!(input_label(InputSel::Off), "Off");
        assert_eq!(input_label(InputSel::Mono(7)), "In 7");
        assert_eq!(input_label(InputSel::Pair(3, 4)), "In 3+4");
        let l = InputSel::Partial {
            left: Some(1),
            right: None,
        };
        assert_eq!(input_label(l), "In 1 L");
        let r = InputSel::Partial {
            left: None,
            right: Some(2),
        };
        assert_eq!(input_label(r), "In 2 R");
        let none = InputSel::Partial {
            left: None,
            right: None,
        };
        assert_eq!(input_label(none), "Off");
    }

    fn cmds(current: InputSel, target: InputSel) -> Vec<String> {
        input_change(&track(), current, target)
            .into_iter()
            .map(|c| match c {
                atomik_model::Command::Disconnect { to } => format!("-{}", to.socket),
                atomik_model::Command::Connect(c) => format!("{}>{}", c.from.socket, c.to.socket),
                other => panic!("unexpected {other:?}"),
            })
            .collect()
    }

    #[test]
    fn input_changes_replace_only_what_differs() {
        use InputSel::*;
        assert_eq!(cmds(Off, Mono(3)), ["in_3>in_l", "in_3>in_r"]);
        assert_eq!(cmds(Off, Pair(1, 2)), ["in_1>in_l", "in_2>in_r"]);
        assert_eq!(cmds(Pair(1, 2), Off), ["-in_l", "-in_r"]);
        assert_eq!(
            cmds(Pair(1, 2), Pair(3, 4)),
            ["-in_l", "-in_r", "in_3>in_l", "in_4>in_r"]
        );
        assert_eq!(cmds(Pair(1, 2), Mono(1)), ["-in_r", "in_1>in_r"]);
        assert!(cmds(Mono(2), Mono(2)).is_empty());
        let right4 = Partial {
            left: None,
            right: Some(4),
        };
        assert_eq!(
            cmds(right4, Pair(1, 2)),
            ["-in_r", "in_1>in_l", "in_2>in_r"]
        );
        let left5 = Partial {
            left: Some(5),
            right: None,
        };
        assert_eq!(cmds(Off, left5), ["in_5>in_l"]);
    }
}
