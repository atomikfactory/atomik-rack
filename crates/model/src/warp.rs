//! Warp: audio clips that follow the song tempo and transpose without changing length.
//!
//! The clip model stays in ticks. A clip with warp settings plays its asset through a *time
//! map* from the song timeline to the asset: the source frame heard at a song tick is
//! `source_offset + source(tick - start)`, where `source` counts source frames from the clip
//! start in *beats*, not seconds (the clip's source tempo, [`WarpSettings::source_bpm`], says
//! how many frames a beat holds), so when the song tempo changes or ramps the audio keeps
//! locked to the beat grid. Warp markers ([`WarpMarker`]) pin a source frame to a tick; between
//! two markers the map is straight (the audio is stretched locally), before the first and
//! after the last it continues at the source tempo.
//!
//! Everything here is pure arithmetic: [`warp_job`] turns a clip into the [`WarpJob`] a worker
//! renders (the map as knots over song frames), [`source_at_tick`] and [`tick_at_source`] are
//! the map and its inverse for the editing commands, and [`quantize_audio`] places markers so
//! detected transients land on a grid.

use crate::song::{AudioClipContent, MAX_AUDIO_FRAMES, Song};
use atomik_core::PPQ;
use std::collections::BTreeMap;

/// Lowest source tempo.
pub const WARP_BPM_MIN: f64 = 20.0;
/// Highest source tempo.
pub const WARP_BPM_MAX: f64 = 999.0;
/// Largest transposition in cents (two octaves either way).
pub const WARP_TRANSPOSE_MAX_CENTS: i32 = 2400;
/// Most warp markers on one clip.
pub const MAX_WARP_MARKERS: usize = 512;
/// Frames between two knots inside a tempo ramp.
const RAMP_KNOT_FRAMES: f64 = 4096.0;

/// The stretch engine a warped clip uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WarpMode {
    /// Speed and pitch change together (resampling). Transposition has no effect.
    Repitch,
    /// Transient-preserving stretch for drums and percussive loops.
    Beats,
    /// Phase vocoder for sustained and polyphonic material.
    Tonal,
}

impl WarpMode {
    /// Every mode, in menu order.
    pub const ALL: [WarpMode; 3] = [WarpMode::Beats, WarpMode::Tonal, WarpMode::Repitch];

    /// The name used in the project file and by the commands.
    pub fn id(self) -> &'static str {
        match self {
            WarpMode::Repitch => "repitch",
            WarpMode::Beats => "beats",
            WarpMode::Tonal => "tonal",
        }
    }

    /// The mode called `id` (see [`WarpMode::id`]).
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.id() == id)
    }

    /// A short label for menus and the inspector.
    pub fn label(self) -> &'static str {
        match self {
            WarpMode::Repitch => "Repitch",
            WarpMode::Beats => "Beats",
            WarpMode::Tonal => "Tonal",
        }
    }
}

/// A warp marker: this source frame plays at this tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WarpMarker {
    /// Ticks from the clip start.
    pub tick: i64,
    /// Source frames from the clip's `source_offset`.
    pub source: u64,
}

/// The warp settings of an audio clip. The default is no warp: the clip plays its asset in
/// real time.
#[derive(Debug, Clone, PartialEq)]
pub struct WarpSettings {
    /// Follow the song tempo. Off keeps the mode, tempo and markers for later.
    pub enabled: bool,
    /// The stretch engine.
    pub mode: WarpMode,
    /// The tempo the source was recorded at, [`WARP_BPM_MIN`]..=[`WARP_BPM_MAX`].
    pub source_bpm: f64,
    /// Transposition in cents (100 per semitone), within ±[`WARP_TRANSPOSE_MAX_CENTS`]. It
    /// changes pitch without changing length (Beats and Tonal; Repitch ignores it), also when
    /// `enabled` is off.
    pub transpose_cents: i32,
    /// Markers, strictly increasing in tick and in source frame, at most [`MAX_WARP_MARKERS`].
    pub markers: Vec<WarpMarker>,
}

impl Default for WarpSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: WarpMode::Beats,
            source_bpm: 120.0,
            transpose_cents: 0,
            markers: Vec::new(),
        }
    }
}

impl WarpSettings {
    /// True when the clip is rendered through the stretcher: it follows the tempo, or it is
    /// transposed with an engine that can transpose.
    pub fn active(&self) -> bool {
        self.enabled || self.effective_cents() != 0
    }

    /// The transposition that sounds, in cents (0 for Repitch).
    pub fn effective_cents(&self) -> i32 {
        if self.mode == WarpMode::Repitch {
            0
        } else {
            self.transpose_cents
        }
    }

    /// Whole semitones of the transposition (towards zero).
    pub fn semitones(&self) -> i32 {
        self.transpose_cents / 100
    }

    /// The cents left after [`WarpSettings::semitones`], `-99..=99`.
    pub fn cents(&self) -> i32 {
        self.transpose_cents % 100
    }

    /// The source frames, counted from the clip's source offset, that play `dt` ticks after the
    /// clip start (the time map of a warped clip): `dt` times the source tempo's frames per
    /// tick, bent through the markers.
    pub fn source_frames_at(&self, sample_rate: u32, dt: f64) -> f64 {
        rel_source(self, frames_per_tick(self.source_bpm, sample_rate), dt)
    }

    /// The ticks after the clip start at which the source frame `src` (from the clip's source
    /// offset) plays: the inverse of [`WarpSettings::source_frames_at`].
    pub fn ticks_at_source(&self, sample_rate: u32, src: f64) -> f64 {
        rel_tick(self, frames_per_tick(self.source_bpm, sample_rate), src)
    }

    /// Checks the ranges; `Err` says what is wrong.
    pub fn validate(&self) -> Result<(), String> {
        if !self.source_bpm.is_finite() || !(WARP_BPM_MIN..=WARP_BPM_MAX).contains(&self.source_bpm)
        {
            return Err(format!(
                "source tempo {} is outside {WARP_BPM_MIN}..={WARP_BPM_MAX} BPM",
                self.source_bpm
            ));
        }
        if self.transpose_cents.abs() > WARP_TRANSPOSE_MAX_CENTS {
            return Err(format!(
                "transpose {} cents is outside ±{WARP_TRANSPOSE_MAX_CENTS}",
                self.transpose_cents
            ));
        }
        if self.markers.len() > MAX_WARP_MARKERS {
            return Err(format!(
                "{} warp markers is above the {MAX_WARP_MARKERS} allowed",
                self.markers.len()
            ));
        }
        for (i, m) in self.markers.iter().enumerate() {
            if m.tick < 0 || m.tick > crate::song::MAX_TICK || m.source > MAX_AUDIO_FRAMES {
                return Err(format!("warp marker {i} is out of range"));
            }
            if i > 0 {
                let p = &self.markers[i - 1];
                if p.tick >= m.tick || p.source >= m.source {
                    return Err(format!(
                        "warp markers must increase in tick and source (marker {i})"
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Source frames per tick at `bpm`.
fn frames_per_tick(bpm: f64, sample_rate: u32) -> f64 {
    60.0 * f64::from(sample_rate) / (bpm * PPQ as f64)
}

/// Source frames from the clip's source offset at `dt` ticks from the clip start.
fn rel_source(w: &WarpSettings, spt: f64, dt: f64) -> f64 {
    let m = &w.markers;
    let k = m.partition_point(|p| p.tick as f64 <= dt);
    match (k.checked_sub(1).map(|i| m[i]), m.get(k)) {
        (None, None) => dt * spt,
        (None, Some(b)) => b.source as f64 - (b.tick as f64 - dt) * spt,
        (Some(a), None) => a.source as f64 + (dt - a.tick as f64) * spt,
        (Some(a), Some(b)) => {
            let t = (dt - a.tick as f64) / (b.tick - a.tick) as f64;
            a.source as f64 + t * (b.source - a.source) as f64
        }
    }
}

/// The ticks from the clip start at which source frame `src` (from the offset) plays.
fn rel_tick(w: &WarpSettings, spt: f64, src: f64) -> f64 {
    let m = &w.markers;
    let k = m.partition_point(|p| p.source as f64 <= src);
    match (k.checked_sub(1).map(|i| m[i]), m.get(k)) {
        (None, None) => src / spt,
        (None, Some(b)) => b.tick as f64 - (b.source as f64 - src) / spt,
        (Some(a), None) => a.tick as f64 + (src - a.source as f64) / spt,
        (Some(a), Some(b)) => {
            let t = (src - a.source as f64) / (b.source - a.source) as f64;
            a.tick as f64 + t * (b.tick - a.tick) as f64
        }
    }
}

/// The asset frame (fractional) a clip that starts at `start` plays at song `tick`. Without
/// warp this is the 1:1 reading (the real time since the start); with warp the time map.
pub fn source_at_tick(
    song: &Song,
    sample_rate: u32,
    start: i64,
    a: &AudioClipContent,
    tick: i64,
) -> f64 {
    if !a.warp.enabled {
        let table = song.tempo_table();
        let map = table.map();
        let sr = f64::from(sample_rate);
        return a.source_offset as f64 + map.frame(tick as f64, sr) - map.frame(start as f64, sr);
    }
    let spt = frames_per_tick(a.warp.source_bpm, sample_rate);
    a.source_offset as f64 + rel_source(&a.warp, spt, (tick - start) as f64)
}

/// The song tick (fractional) at which the clip plays asset frame `src`: the inverse of
/// [`source_at_tick`].
pub fn tick_at_source(
    song: &Song,
    sample_rate: u32,
    start: i64,
    a: &AudioClipContent,
    src: f64,
) -> f64 {
    if !a.warp.enabled {
        let table = song.tempo_table();
        let map = table.map();
        let sr = f64::from(sample_rate);
        let frame = map.frame(start as f64, sr) + (src - a.source_offset as f64);
        return map.tick(frame.max(0.0), sr);
    }
    let spt = frames_per_tick(a.warp.source_bpm, sample_rate);
    start as f64 + rel_tick(&a.warp, spt, src - a.source_offset as f64)
}

impl AudioClipContent {
    /// The source offset and warp markers of the part of this clip (which starts at tick
    /// `start`) that begins at tick `tick`; `plain_delta` is the frames between the two ticks
    /// (what a clip without warp advances by) and the offset stays within `asset_frames`.
    ///
    /// Without warp the offset moves by `plain_delta` and the markers stay. With warp the
    /// offset is the source frame the time map plays at `tick`, and the markers are re-based on
    /// the new start, with one at its tick 0 so the stretch before the first remaining marker
    /// stays what it was.
    pub fn piece_from(
        &self,
        song: &Song,
        sample_rate: u32,
        start: i64,
        tick: i64,
        plain_delta: u64,
        asset_frames: u64,
    ) -> (u64, Vec<WarpMarker>) {
        if !self.warp.enabled {
            return (
                self.source_offset
                    .saturating_add(plain_delta)
                    .min(asset_frames),
                self.warp.markers.clone(),
            );
        }
        let at = source_at_tick(song, sample_rate, start, self, tick)
            .round()
            .max(0.0) as u64;
        let offset = at.min(asset_frames);
        (
            offset,
            self.warp
                .markers_after(tick - start, self.source_offset, offset),
        )
    }
}

impl WarpSettings {
    /// The markers of the part of a clip that starts `shift` ticks after the clip's start (a
    /// negative shift: before it) when its source offset moves from `old_offset` to
    /// `new_offset`: every marker re-based on the new start, with one at its tick 0 so the
    /// stretch before the first remaining marker stays what it was (markers that fall before
    /// the new start or the new offset are dropped). Empty without markers.
    pub fn markers_after(&self, shift: i64, old_offset: u64, new_offset: u64) -> Vec<WarpMarker> {
        if self.markers.is_empty() {
            return Vec::new();
        }
        let mut markers = vec![WarpMarker { tick: 0, source: 0 }];
        for m in &self.markers {
            let t = m.tick - shift;
            let s = i128::from(m.source) + i128::from(old_offset) - i128::from(new_offset);
            if t > 0 && s > 0 && markers.len() < MAX_WARP_MARKERS {
                markers.push(WarpMarker {
                    tick: t,
                    source: s as u64,
                });
            }
        }
        // Keep the list strictly increasing in source as well.
        let mut clean: Vec<WarpMarker> = Vec::with_capacity(markers.len());
        for m in markers {
            if clean
                .last()
                .is_none_or(|p| p.tick < m.tick && p.source < m.source)
            {
                clean.push(m);
            }
        }
        clean
    }
}

/// What a worker renders for a warped clip: the clip's whole span as one buffer. It is plain
/// data and carries everything the result depends on, so [`WarpJob::key`] identifies the
/// buffer (a change of tempo, setting or clip geometry gives another key).
#[derive(Debug, Clone, PartialEq)]
pub struct WarpJob {
    /// The asset (id in the pool).
    pub asset: String,
    /// The stretch engine.
    pub mode: WarpMode,
    /// The transposition in cents that sounds (0 for Repitch).
    pub transpose_cents: i32,
    /// The time map as `(frame from the clip start, asset frame)` knots, with the first frame
    /// at about 0: piecewise linear between knots.
    pub knots: Vec<(f64, f64)>,
    /// The buffer length: the clip's frames at the song's tempo map.
    pub frames: u64,
    /// The document sample rate.
    pub sample_rate: u32,
}

impl WarpJob {
    /// A 64-bit key of everything the rendered buffer depends on (FNV-1a over the fields).
    pub fn key(&self) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        let mut eat = |bytes: &[u8]| {
            for b in bytes {
                h ^= u64::from(*b);
                h = h.wrapping_mul(0x0100_0000_01b3);
            }
        };
        eat(self.asset.as_bytes());
        eat(self.mode.id().as_bytes());
        eat(&self.transpose_cents.to_le_bytes());
        eat(&self.frames.to_le_bytes());
        eat(&self.sample_rate.to_le_bytes());
        for (a, b) in &self.knots {
            eat(&a.to_bits().to_le_bytes());
            eat(&b.to_bits().to_le_bytes());
        }
        h
    }

    /// The asset frames the render reads: `(first, end)` with a margin of 8192 frames on both
    /// sides for the stretchers' windows, `first >= 0`.
    pub fn source_span(&self) -> (u64, u64) {
        let lo = self.knots.iter().map(|k| k.1).fold(f64::INFINITY, f64::min);
        let hi = self
            .knots
            .iter()
            .map(|k| k.1)
            .fold(f64::NEG_INFINITY, f64::max);
        let first = (lo - 8192.0).floor().max(0.0) as u64;
        let end = (hi + 8192.0).ceil().max(0.0) as u64;
        (first, end.max(first))
    }
}

/// The [`WarpJob`] of the audio clip `clip` (`None` when the clip is not warped).
pub fn warp_job(
    song: &Song,
    sample_rate: u32,
    start: i64,
    length: i64,
    a: &AudioClipContent,
) -> Option<WarpJob> {
    if !a.warp.active() {
        return None;
    }
    let table = song.tempo_table();
    let map = table.map();
    let sr = f64::from(sample_rate);
    let start_frame = map.song_frame(start, sr);
    let frames = map
        .song_frame(start + length, sr)
        .saturating_sub(start_frame);
    let offset = a.source_offset as f64;
    let knots = if a.warp.enabled {
        let spt = frames_per_tick(a.warp.source_bpm, sample_rate);
        // Breakpoints (ticks from the clip start): the ends, the markers and the tempo events.
        let mut cuts: Vec<i64> = vec![0, length];
        cuts.extend(
            a.warp
                .markers
                .iter()
                .map(|m| m.tick)
                .filter(|t| *t > 0 && *t < length),
        );
        cuts.extend(
            song.tempo
                .iter()
                .map(|e| e.tick - start)
                .filter(|t| *t > 0 && *t < length),
        );
        cuts.sort_unstable();
        cuts.dedup();
        let mut knots: Vec<(f64, f64)> = Vec::new();
        let frame_at = |dt: f64| map.frame(start as f64 + dt, sr) - start_frame as f64;
        for pair in cuts.windows(2) {
            let (ta, tb) = (pair[0] as f64, pair[1] as f64);
            // A ramp bends the frame axis: add knots inside it.
            let ramp = (map.bpm(start as f64 + ta + (tb - ta) / 3.0) - map.bpm(start as f64 + ta))
                .abs()
                > 1.0e-9;
            let pieces = if ramp {
                (((frame_at(tb) - frame_at(ta)) / RAMP_KNOT_FRAMES).ceil() as usize).clamp(2, 4096)
            } else {
                1
            };
            for p in 0..pieces {
                let dt = ta + (tb - ta) * p as f64 / pieces as f64;
                knots.push((frame_at(dt), offset + rel_source(&a.warp, spt, dt)));
            }
        }
        let end = length as f64;
        knots.push((frame_at(end), offset + rel_source(&a.warp, spt, end)));
        knots.dedup_by(|b, a| b.0 <= a.0);
        knots
    } else {
        vec![
            (0.0, offset),
            (frames.max(1) as f64, offset + frames.max(1) as f64),
        ]
    };
    if knots.len() < 2 {
        return None;
    }
    Some(WarpJob {
        asset: a.asset.clone(),
        mode: a.warp.mode,
        transpose_cents: a.warp.effective_cents(),
        knots,
        frames,
        sample_rate,
    })
}

/// A tempo for a loop of `frames` frames that holds `beats` beats, or `None` when it falls
/// outside the allowed range.
pub fn bpm_for_beats(frames: u64, beats: f64, sample_rate: u32) -> Option<f64> {
    if frames == 0 || !beats.is_finite() || beats <= 0.0 {
        return None;
    }
    let bpm = beats * 60.0 * f64::from(sample_rate) / frames as f64;
    (WARP_BPM_MIN..=WARP_BPM_MAX).contains(&bpm).then_some(bpm)
}

/// A rough tempo from transient positions (frames, ascending): the BPM in `70..=180`
/// (step 0.1) at which the onsets line up best on a beat grid, found as the comb-filter
/// strength `|sum e^(2 pi i onset / period)|` over candidate beat periods, counting an onset
/// on an eighth note as half a hit. `None` for fewer than 4 onsets or no clear peak.
pub fn estimate_bpm(onsets: &[u64], sample_rate: u32) -> Option<f64> {
    if onsets.len() < 4 || sample_rate == 0 {
        return None;
    }
    let sr = f64::from(sample_rate);
    let phase = |o: u64, period: f64| std::f64::consts::TAU * (o as f64 / period);
    let (mut best, mut best_bpm, mut sum_all) = (0.0f64, 0.0f64, 0.0f64);
    let mut count = 0.0f64;
    for step in 700..=1800 {
        let bpm = f64::from(step) / 10.0;
        let period = 60.0 * sr / bpm;
        let (mut re, mut im) = (0.0f64, 0.0f64);
        let (mut re2, mut im2) = (0.0f64, 0.0f64);
        for &o in onsets {
            let p = phase(o, period);
            re += p.cos();
            im += p.sin();
            let p2 = phase(o, period / 2.0);
            re2 += p2.cos();
            im2 += p2.sin();
        }
        let n = onsets.len() as f64;
        let score = ((re * re + im * im).sqrt() + 0.5 * (re2 * re2 + im2 * im2).sqrt()) / n;
        sum_all += score;
        count += 1.0;
        if score > best {
            best = score;
            best_bpm = bpm;
        }
    }
    let mean = sum_all / count;
    (best > 1.5 * mean && best > 0.5).then_some(best_bpm)
}

/// What [`quantize_audio`] aims for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioQuantize {
    /// Grid length in ticks (`>= 1`); the grid starts at tick 0 of the song.
    pub grid: i64,
    /// How far each transient moves towards the grid, `0..=1`.
    pub strength: f64,
}

/// Quantizes the audio of a clip that starts at `start` and is `length` ticks long: every
/// transient in `onsets` (asset frames) moves to the nearest grid line by `strength` of the
/// distance. This sets warp markers (replacing any), so the audio is stretched locally between
/// them; the clip's start and end stay pinned, so its span of the asset does not change. A
/// clip without warp is switched on at the song tempo at its start (Beats mode, unless it has
/// another mode set) so it still plays as it did apart from the moved transients.
///
/// `Err` says why nothing can be done (reversed clip, bad grid, no usable transient).
pub fn quantize_audio(
    song: &Song,
    sample_rate: u32,
    start: i64,
    length: i64,
    a: &AudioClipContent,
    onsets: &[u64],
    q: &AudioQuantize,
) -> Result<AudioClipContent, String> {
    if a.reverse {
        return Err("a reversed clip cannot be warped".into());
    }
    if q.grid < 1 || !q.strength.is_finite() {
        return Err("the quantize grid and strength must be valid".into());
    }
    let strength = q.strength.clamp(0.0, 1.0);
    let mut base = a.clone();
    if !base.warp.enabled {
        let bpm = song.tempo_table().map().bpm(start as f64);
        base.warp.enabled = true;
        base.warp.source_bpm = bpm.clamp(WARP_BPM_MIN, WARP_BPM_MAX);
        base.warp.markers.clear();
    }
    let offset = base.source_offset;
    let lo = base.source_offset as f64;
    let hi = source_at_tick(song, sample_rate, start, &base, start + length);
    let spt = frames_per_tick(base.warp.source_bpm, sample_rate);
    let mut markers: Vec<WarpMarker> = vec![WarpMarker { tick: 0, source: 0 }];
    let mut sorted: Vec<u64> = onsets.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mut moved = 0usize;
    for o in sorted {
        let of = o as f64;
        if of <= lo + 1.0 || of >= hi - 1.0 {
            continue;
        }
        let t_cur = tick_at_source(song, sample_rate, start, &base, of);
        let grid = q.grid as f64;
        let target = (t_cur / grid).round() * grid;
        let t_new = (t_cur + strength * (target - t_cur)).round() as i64 - start;
        let src = o - offset;
        let last = markers[markers.len() - 1];
        if t_new > last.tick
            && t_new < length
            && src > last.source
            && markers.len() < MAX_WARP_MARKERS - 1
        {
            markers.push(WarpMarker {
                tick: t_new,
                source: src,
            });
            moved += 1;
        }
    }
    if moved == 0 {
        return Err("no transient to quantize in the clip".into());
    }
    let end_src = rel_source(&base.warp, spt, length as f64).round().max(0.0) as u64;
    let last = markers[markers.len() - 1];
    if end_src > last.source && length > last.tick {
        markers.push(WarpMarker {
            tick: length,
            source: end_src,
        });
    }
    let mut out = base;
    out.warp.markers = markers;
    if out.warp.mode == WarpMode::Repitch && !a.warp.enabled {
        out.warp.mode = WarpMode::Beats;
    }
    out.warp.validate()?;
    Ok(out)
}

/// The jobs of a song: the job of every warped audio clip that is audible (muted tracks and
/// clips and hidden spans leave none), keyed by [`WarpJob::key`] (equal jobs share one entry).
pub fn warp_jobs(doc: &crate::RackDoc) -> BTreeMap<u64, WarpJob> {
    let mut out = BTreeMap::new();
    for track in crate::song::effective_tracks(&doc.song) {
        for region in crate::song::flatten_audio(track, &doc.song, doc.sample_rate) {
            if let Some(job) = region.warp {
                out.insert(job.key(), (*job).clone());
            }
        }
    }
    out
}
