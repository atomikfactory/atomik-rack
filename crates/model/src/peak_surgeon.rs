//! Peak Surgeon: trim only the shortest, most irregular peaks of an audio clip, by a gain dip a
//! few samples wide, to win headroom nobody can hear.
//!
//! The ear needs a minimum time to register a change of level, so a dip of a few tenths of a dB
//! over a handful of samples lowers a peak measurably and is not heard. The work is the usual
//! hand loop, done by the machine: find the highest peak, dip it a little, find the highest peak
//! again, until the peaks are even.
//!
//! * [`Scanner`] reads the levels the clip plays (the highest absolute sample of any channel
//!   times the clip's gain, fades and envelope, one value per frame of the clip's own time) in
//!   chunks, and keeps the [`KEEP`] highest **peaks**: a peak is a frame at least as high as every
//!   frame within `width` frames of it on either side (the first of a flat top), so two peaks
//!   are always more than `width` frames apart.
//! * [`plan`] chooses the dips that bring every peak down to a threshold: peak by peak from the
//!   highest, each step dips the peak by [`Settings::step_db`] (0.2 to 0.4 dB) at its own frame; a
//!   second step on the same frame deepens that dip, and a dip is never deeper than
//!   [`MAX_DIP_DB`] (a peak that would need more is left, and counted). A dip is a triangle in
//!   linear gain `width` frames each side of the peak, so the neighbours of the peak are lowered
//!   by less and a dip never raises anything.
//! * [`apply_dips`] writes the dips into the clip's gain envelope (the clip is never
//!   re-rendered and the file is never touched): three breakpoints per dip, and the points of the
//!   existing envelope that fall inside a dip's span are replaced. The clip's own gain stays
//!   where it is. The result is one `SetAudioClip`, so the whole trim is one undo step.
//!
//! The planner works on the kept peaks only, so its `after` level is exact for them and an upper
//! bound for nothing else: the app measures the clip again after the edit and reports that
//! number.

use crate::song::{
    AUDIO_GAIN_DB_MAX, AUDIO_GAIN_DB_MIN, GainPoint, MAX_GAIN_POINTS, envelope_gain,
};
use crate::{AudioClipContent, ModelError};
use atomik_core::{db_to_gain, gain_to_db};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};

/// Smallest dip step in dB.
pub const MIN_STEP_DB: f32 = 0.2;
/// Largest dip step in dB.
pub const MAX_STEP_DB: f32 = 0.4;
/// The step the panel starts with, in dB.
pub const DEFAULT_STEP_DB: f32 = 0.3;
/// Narrowest dip half-width in frames.
pub const MIN_WIDTH: u32 = 2;
/// Widest dip half-width in frames.
pub const MAX_WIDTH: u32 = 64;
/// The half-width the panel starts with, in frames.
pub const DEFAULT_WIDTH: u32 = 8;
/// The deepest one dip goes, in dB.
pub const MAX_DIP_DB: f32 = 3.0;
/// Peaks a scan keeps.
pub const KEEP: usize = 4096;
/// Levels below this (linear) are never peaks of interest: about -80 dBFS.
const FLOOR: f32 = 1e-4;

/// One peak of the clip.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Peak {
    /// The frame in the clip's own time.
    pub frame: u64,
    /// The linear level the clip plays there.
    pub level: f32,
}

impl Peak {
    /// The level in dBFS.
    pub fn db(&self) -> f32 {
        gain_to_db(self.level)
    }
}

/// What a scan found.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Scan {
    /// The kept peaks, highest first (at most [`KEEP`]).
    pub peaks: Vec<Peak>,
    /// Frames scanned.
    pub frames: u64,
}

impl Scan {
    /// The highest peak in dBFS, `None` for silence.
    pub fn top_db(&self) -> Option<f32> {
        self.peaks.first().map(Peak::db)
    }
}

/// Finds the peaks of a clip, a chunk of levels at a time (see the module docs).
#[derive(Debug)]
pub struct Scanner {
    width: usize,
    /// Levels from the frame `base` on.
    buf: Vec<f32>,
    base: u64,
    /// The next frame to examine.
    next: u64,
    /// The kept peaks, lowest on top.
    kept: BinaryHeap<Reverse<(u32, Reverse<u64>)>>,
}

impl Scanner {
    /// A scanner with dips of `width` frames each side (clamped to [`MIN_WIDTH`]..=[`MAX_WIDTH`]).
    pub fn new(width: u32) -> Self {
        Self {
            width: width.clamp(MIN_WIDTH, MAX_WIDTH) as usize,
            buf: Vec::new(),
            base: 0,
            next: 0,
            kept: BinaryHeap::new(),
        }
    }

    /// Takes the next `levels` (linear, non-negative; a NaN counts as 0).
    pub fn push(&mut self, levels: &[f32]) {
        self.buf.extend(
            levels
                .iter()
                .map(|&l| if l.is_finite() { l.abs() } else { 0.0 }),
        );
        let end = self.base + self.buf.len() as u64;
        let w = self.width as u64;
        while self.next + w < end {
            self.examine(self.next);
            self.next += 1;
        }
        // Keep the frames still needed as left context.
        let keep_from = self.next.saturating_sub(w);
        if keep_from > self.base {
            self.buf.drain(..(keep_from - self.base) as usize);
            self.base = keep_from;
        }
    }

    /// Ends the scan and returns the peaks.
    pub fn finish(mut self) -> Scan {
        let frames = self.base + self.buf.len() as u64;
        // The tail has no right context: zeros stand in for it.
        self.buf.extend(std::iter::repeat_n(0.0, self.width));
        while self.next < frames {
            self.examine(self.next);
            self.next += 1;
        }
        let mut peaks: Vec<Peak> = self
            .kept
            .into_iter()
            .map(|Reverse((bits, Reverse(frame)))| Peak {
                frame,
                level: f32::from_bits(bits),
            })
            .collect();
        peaks.sort_by(|a, b| {
            b.level
                .total_cmp(&a.level)
                .then_with(|| a.frame.cmp(&b.frame))
        });
        Scan { peaks, frames }
    }

    fn examine(&mut self, at: u64) {
        let i = (at - self.base) as usize;
        let level = self.buf[i];
        if level < FLOOR {
            return;
        }
        // Cheap rejection against the lowest kept peak when the list is full.
        if self.kept.len() >= KEEP
            && self
                .kept
                .peek()
                .is_some_and(|Reverse((bits, _))| level.to_bits() <= *bits)
        {
            return;
        }
        let w = self.width;
        let from = i.saturating_sub(w);
        // The first of a flat top: strictly above what is before it, at least as high as what
        // follows.
        let before = &self.buf[from..i];
        let after = &self.buf[i + 1..(i + 1 + w).min(self.buf.len())];
        if before.iter().any(|&b| b >= level) || after.iter().any(|&a| a > level) {
            return;
        }
        self.kept.push(Reverse((level.to_bits(), Reverse(at))));
        if self.kept.len() > KEEP {
            self.kept.pop();
        }
    }
}

/// What [`plan`] is asked.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    /// Every peak above this level (dBFS) is dipped down to it.
    pub threshold_db: f32,
    /// How much one step dips, [`MIN_STEP_DB`]..=[`MAX_STEP_DB`].
    pub step_db: f32,
    /// Frames each side of a dip, [`MIN_WIDTH`]..=[`MAX_WIDTH`].
    pub width: u32,
    /// Most dips to make.
    pub max_dips: usize,
}

impl Settings {
    /// The settings the panel starts with for a clip whose envelope already has `existing`
    /// points: the default step and width, and as many dips as the envelope has room for.
    pub fn new(threshold_db: f32, existing: usize) -> Self {
        Self {
            threshold_db,
            step_db: DEFAULT_STEP_DB,
            width: DEFAULT_WIDTH,
            max_dips: dips_that_fit(existing),
        }
    }
}

/// How many dips fit in an envelope that already has `existing` points (three points each).
pub fn dips_that_fit(existing: usize) -> usize {
    MAX_GAIN_POINTS.saturating_sub(existing) / 3
}

/// One dip: a triangle of gain, `depth_db` deep at `frame`, gone `width` frames away.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Dip {
    /// The frame of the peak, in the clip's own time.
    pub frame: u64,
    /// How deep the dip is there, in dB (positive).
    pub depth_db: f32,
}

/// The linear gain of a dip of `depth_db` at `centre`, at frame `n`, `width` frames each side.
fn dip_factor(centre: u64, depth_db: f32, width: u32, n: u64) -> f64 {
    let d = n.abs_diff(centre);
    if d >= u64::from(width) {
        return 1.0;
    }
    let deep = 1.0 - f64::from(db_to_gain(-depth_db));
    1.0 - deep * (1.0 - d as f64 / f64::from(width))
}

/// What [`plan`] chose.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Plan {
    /// The dips, by frame.
    pub dips: Vec<Dip>,
    /// The highest kept peak before, in dBFS.
    pub before_db: Option<f32>,
    /// The highest kept peak after the dips, in dBFS.
    pub after_db: Option<f32>,
    /// Peaks still above the threshold that no dip could bring down (too deep, or out of dips).
    pub left: usize,
}

impl Plan {
    /// The headroom the dips win, in dB (0 when there is nothing to trim).
    pub fn gained_db(&self) -> f32 {
        match (self.before_db, self.after_db) {
            (Some(b), Some(a)) => (b - a).max(0.0),
            _ => 0.0,
        }
    }
}

/// Chooses the dips for `peaks` (highest first, as [`Scan::peaks`]); see the module docs.
pub fn plan(peaks: &[Peak], settings: &Settings) -> Plan {
    let width = settings.width.clamp(MIN_WIDTH, MAX_WIDTH);
    let step = settings.step_db.clamp(MIN_STEP_DB, MAX_STEP_DB);
    let limit = db_to_gain(settings.threshold_db);
    let mut out = Plan {
        before_db: peaks.first().map(Peak::db),
        ..Plan::default()
    };
    let mut depths: BTreeMap<u64, f32> = BTreeMap::new();
    // The gain of every dip at `frame`.
    let factor = |depths: &BTreeMap<u64, f32>, frame: u64| -> f32 {
        let lo = frame.saturating_sub(u64::from(width) - 1);
        let hi = frame + u64::from(width) - 1;
        depths
            .range(lo..=hi)
            .fold(1.0f64, |g, (&c, &d)| g * dip_factor(c, d, width, frame)) as f32
    };
    let mut heap: BinaryHeap<(u32, usize)> = peaks
        .iter()
        .enumerate()
        .filter(|(_, p)| p.level > limit)
        .map(|(i, p)| (p.level.to_bits(), i))
        .collect();
    while let Some((bits, i)) = heap.pop() {
        let p = peaks[i];
        let now = p.level * factor(&depths, p.frame);
        if now.to_bits() != bits {
            // Another dip lowered it since it was queued.
            if now > limit {
                heap.push((now.to_bits(), i));
            }
            continue;
        }
        if now <= limit {
            break;
        }
        let depth = depths.get(&p.frame).copied();
        let deeper = depth.unwrap_or(0.0) + step;
        if deeper > MAX_DIP_DB + 1e-4 || (depth.is_none() && depths.len() >= settings.max_dips) {
            out.left += 1;
            continue;
        }
        depths.insert(p.frame, deeper);
        heap.push(((p.level * factor(&depths, p.frame)).to_bits(), i));
    }
    out.after_db = peaks
        .iter()
        .map(|p| p.level * factor(&depths, p.frame))
        .reduce(f32::max)
        .map(gain_to_db);
    out.dips = depths
        .into_iter()
        .map(|(frame, depth_db)| Dip { frame, depth_db })
        .collect();
    out
}

/// The envelope of a clip with `dips` written into `existing` (see the module docs). `span` is
/// the clip's length in frames: no point lies past it.
///
/// # Errors
/// [`ModelError::BadSong`] when the result would have more than [`MAX_GAIN_POINTS`] points.
pub fn apply_dips(
    existing: &[GainPoint],
    dips: &[Dip],
    width: u32,
    span: u64,
) -> Result<Vec<GainPoint>, ModelError> {
    if dips.is_empty() {
        return Ok(existing.to_vec());
    }
    let width = width.clamp(MIN_WIDTH, MAX_WIDTH);
    let w = u64::from(width);
    let linear: Vec<(u64, f32)> = existing
        .iter()
        .map(|p| (p.frame, db_to_gain(p.gain_db)))
        .collect();
    let inside = |frame: u64| {
        dips.iter()
            .any(|d| frame > d.frame.saturating_sub(w) && frame < d.frame + w)
    };
    let mut points: BTreeMap<u64, f32> = existing
        .iter()
        .filter(|p| !inside(p.frame) || p.frame >= span)
        .map(|p| (p.frame, p.gain_db))
        .collect();
    for d in dips {
        for frame in [d.frame.saturating_sub(w), d.frame, (d.frame + w).min(span)] {
            let dip = dips.iter().fold(1.0f64, |g, e| {
                g * dip_factor(e.frame, e.depth_db, width, frame)
            });
            let base = f64::from(envelope_gain(&linear, frame).unwrap_or(1.0));
            let db = gain_to_db((base * dip) as f32).clamp(AUDIO_GAIN_DB_MIN, AUDIO_GAIN_DB_MAX);
            points.insert(frame, db);
        }
    }
    if points.len() > MAX_GAIN_POINTS {
        return Err(ModelError::BadSong(format!(
            "the trim needs {} envelope points, at most {MAX_GAIN_POINTS} are allowed",
            points.len()
        )));
    }
    Ok(points
        .into_iter()
        .map(|(frame, gain_db)| GainPoint { frame, gain_db })
        .collect())
}

/// `content` with `dips` written into its envelope; the rest of the clip is as it was.
///
/// # Errors
/// As [`apply_dips`].
pub fn trimmed(
    content: &AudioClipContent,
    dips: &[Dip],
    width: u32,
    span: u64,
) -> Result<AudioClipContent, ModelError> {
    Ok(AudioClipContent {
        envelope: apply_dips(&content.envelope, dips, width, span)?,
        ..content.clone()
    })
}

/// The levels `content` plays from `source` frames in clip time (the highest absolute sample of
/// any channel, times [`AudioClipContent::played_gain`]); `reverse` is the clip's flag. Test and
/// reference helper: the app feeds the [`Scanner`] chunk by chunk from the asset instead.
pub fn played_levels(
    content: &AudioClipContent,
    channels: &[Vec<f32>],
    clip_frames: u64,
) -> Vec<f32> {
    let frames = channels.first().map_or(0, Vec::len) as u64;
    let from = content.source_offset.min(frames);
    let n = clip_frames.min(frames - from);
    let gain = content.played_gain(clip_frames);
    (0..n)
        .map(|k| {
            let at = if content.reverse {
                from + n - 1 - k
            } else {
                from + k
            };
            let top = channels
                .iter()
                .map(|c| c[at as usize].abs())
                .fold(0.0f32, f32::max);
            top * gain.at(k)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A quiet signal at 0.25 with peaks at the given frames and levels.
    fn signal(len: usize, peaks: &[(usize, f32)]) -> Vec<f32> {
        let mut v = vec![0.25f32; len];
        for &(f, l) in peaks {
            v[f] = l;
        }
        v
    }

    fn scan(levels: &[f32], width: u32) -> Scan {
        let mut s = Scanner::new(width);
        // Odd chunk sizes: the scanner must not care where the chunks cut.
        for chunk in levels.chunks(37) {
            s.push(chunk);
        }
        s.finish()
    }

    fn at(db: f32) -> f32 {
        db_to_gain(db)
    }

    #[test]
    fn the_scan_ranks_the_peaks_highest_first_whatever_the_chunks() {
        let levels = signal(5_000, &[(100, 0.9), (2_000, 0.7), (4_990, 0.8), (0, 0.6)]);
        let a = scan(&levels, 8);
        let b = {
            let mut s = Scanner::new(8);
            s.push(&levels);
            s.finish()
        };
        assert_eq!(a, b);
        let top: Vec<(u64, f32)> = a.peaks.iter().take(4).map(|p| (p.frame, p.level)).collect();
        assert_eq!(top, vec![(100, 0.9), (4_990, 0.8), (2_000, 0.7), (0, 0.6)]);
        assert_eq!(a.frames, 5_000);
    }

    #[test]
    fn two_peaks_are_never_within_the_width_and_a_flat_top_counts_once() {
        let mut levels = vec![0.1f32; 400];
        // A plateau of five equal frames and a lower neighbour close to it.
        levels[100..105].fill(0.8);
        levels[106] = 0.79;
        let s = scan(&levels, 8);
        let near: Vec<u64> = s
            .peaks
            .iter()
            .map(|p| p.frame)
            .filter(|f| (95..115).contains(f))
            .collect();
        assert_eq!(near, vec![100]);
        for pair in s.peaks.windows(2) {
            let (a, b) = (pair[0].frame, pair[1].frame);
            assert!(a.abs_diff(b) > 8, "{a} and {b} are too close");
        }
    }

    #[test]
    fn silence_and_garbage_have_no_peaks() {
        assert!(scan(&[0.0; 1_000], 8).peaks.is_empty());
        let nan = vec![f32::NAN; 100];
        assert!(scan(&nan, 8).peaks.is_empty());
        assert_eq!(scan(&[], 8).frames, 0);
    }

    #[test]
    fn the_scan_keeps_only_the_highest_when_there_are_too_many() {
        let n = KEEP * 3;
        let mut levels = vec![0.01f32; n * 20];
        for k in 0..n {
            levels[k * 20 + 10] = 0.1 + 0.8 * (k as f32 / n as f32);
        }
        let s = scan(&levels, 8);
        assert_eq!(s.peaks.len(), KEEP);
        assert!(s.peaks[0].level > s.peaks[KEEP - 1].level);
        assert!(s.peaks[KEEP - 1].level >= 0.1 + 0.8 * (2.0 / 3.0) - 1e-3);
    }

    #[test]
    fn the_plan_dips_each_peak_down_to_the_threshold_in_steps() {
        let levels = signal(
            2_000,
            &[(300, at(-0.2)), (900, at(-0.6)), (1_500, at(-3.0))],
        );
        let s = scan(&levels, 8);
        let settings = Settings {
            threshold_db: -1.0,
            step_db: 0.3,
            width: 8,
            max_dips: 100,
        };
        let p = plan(&s.peaks, &settings);
        assert_eq!(p.left, 0);
        let frames: Vec<u64> = p.dips.iter().map(|d| d.frame).collect();
        assert_eq!(frames, vec![300, 900], "the -3 dB peak is left alone");
        // -0.2 needs 0.8 dB: three steps of 0.3; -0.6 needs 0.4: two steps.
        assert!((p.dips[0].depth_db - 0.9).abs() < 1e-4);
        assert!((p.dips[1].depth_db - 0.6).abs() < 1e-4);
        assert!((p.before_db.unwrap() + 0.2).abs() < 1e-3);
        assert!(p.after_db.unwrap() <= -1.0 + 1e-3, "{:?}", p.after_db);
        assert!(p.gained_db() > 0.79);
    }

    #[test]
    fn nothing_above_the_threshold_means_no_dips() {
        let levels = signal(500, &[(100, at(-6.0))]);
        let p = plan(&scan(&levels, 8).peaks, &Settings::new(-1.0, 0));
        assert!(p.dips.is_empty());
        assert_eq!(p.gained_db(), 0.0);
        assert_eq!(p.left, 0);
        assert_eq!(plan(&[], &Settings::new(-1.0, 0)), Plan::default());
    }

    #[test]
    fn a_peak_too_deep_to_trim_or_past_the_dip_budget_is_counted() {
        let levels = signal(2_000, &[(300, 1.0), (900, at(-0.5)), (1_500, at(-0.5))]);
        let s = scan(&levels, 8);
        // 0 dBFS to -10 dBFS would need more than a dip may take.
        let deep = plan(
            &s.peaks,
            &Settings {
                threshold_db: -10.0,
                step_db: 0.4,
                width: 8,
                max_dips: 100,
            },
        );
        assert!(deep.left >= 1, "{deep:?}");
        assert!(deep.dips.iter().all(|d| d.depth_db <= MAX_DIP_DB + 1e-4));
        // Two dips allowed, three peaks to trim: one is left.
        let few = plan(
            &s.peaks,
            &Settings {
                threshold_db: -1.0,
                step_db: 0.4,
                width: 8,
                max_dips: 2,
            },
        );
        assert_eq!(few.dips.len(), 2);
        assert_eq!(few.left, 1);
    }

    #[test]
    fn neighbouring_peaks_share_the_dip_that_covers_both() {
        // Two peaks 6 frames apart (width 4: neither dominates the other's window).
        let levels = signal(200, &[(100, at(-0.1)), (106, at(-0.15))]);
        let s = scan(&levels, 4);
        assert_eq!(s.peaks.iter().filter(|p| p.level > 0.5).count(), 2);
        let p = plan(
            &s.peaks,
            &Settings {
                threshold_db: -1.0,
                step_db: 0.4,
                width: 4,
                max_dips: 100,
            },
        );
        assert_eq!(p.left, 0);
        assert!(p.after_db.unwrap() <= -1.0 + 1e-3);
    }

    #[test]
    fn dips_in_the_envelope_lower_the_played_peak_and_nothing_else() {
        let mut clip = AudioClipContent::new("a", 0);
        clip.gain_db = -2.0;
        let channels = vec![signal(3_000, &[(500, 0.95), (2_000, 0.9)])];
        let levels = played_levels(&clip, &channels, 3_000);
        let s = scan(&levels, 8);
        let settings = Settings {
            threshold_db: -3.0,
            step_db: 0.3,
            width: 8,
            max_dips: 100,
        };
        let p = plan(&s.peaks, &settings);
        assert_eq!(p.dips.len(), 2);
        let trimmed = trimmed(&clip, &p.dips, 8, 3_000).unwrap();
        assert_eq!(trimmed.gain_db, -2.0, "the clip gain is not touched");
        assert_eq!(trimmed.envelope.len(), 6);
        // Measure again with the new envelope: the peaks are at the threshold, the rest is
        // the same as before.
        let after = played_levels(&trimmed, &channels, 3_000);
        let top = after.iter().copied().fold(0.0f32, f32::max);
        assert!(gain_to_db(top) <= -3.0 + 0.02, "{}", gain_to_db(top));
        let changed = levels
            .iter()
            .zip(&after)
            .filter(|(a, b)| (*a - *b).abs() > 1e-6)
            .count();
        assert!(changed <= 2 * 15, "{changed} frames changed");
        assert!(levels.iter().zip(&after).all(|(a, b)| *b <= *a + 1e-7));
    }

    #[test]
    fn a_reversed_clip_dips_the_clip_time_frame_of_the_peak() {
        let mut clip = AudioClipContent::new("a", 0);
        clip.reverse = true;
        let channels = vec![signal(1_000, &[(100, 0.9)])];
        let levels = played_levels(&clip, &channels, 1_000);
        // Frame 100 of the source plays at frame 899 of the reversed clip.
        let s = scan(&levels, 8);
        assert_eq!(s.peaks[0].frame, 899);
    }

    #[test]
    fn dips_keep_the_existing_envelope_outside_their_span() {
        let existing = vec![
            GainPoint {
                frame: 0,
                gain_db: -6.0,
            },
            GainPoint {
                frame: 500,
                gain_db: -6.0,
            },
            GainPoint {
                frame: 1_000,
                gain_db: 0.0,
            },
        ];
        let dips = [Dip {
            frame: 500,
            depth_db: 1.0,
        }];
        let out = apply_dips(&existing, &dips, 8, 2_000).unwrap();
        let frames: Vec<u64> = out.iter().map(|p| p.frame).collect();
        assert_eq!(frames, vec![0, 492, 500, 508, 1_000]);
        assert!((out[2].gain_db + 7.0).abs() < 0.01, "{}", out[2].gain_db);
        assert!((out[1].gain_db + 6.0).abs() < 0.2);
        assert_eq!(out[0], existing[0]);
        assert_eq!(out[4], existing[2]);
    }

    #[test]
    fn a_dip_at_the_ends_of_the_clip_stays_inside_it() {
        let dips = [
            Dip {
                frame: 0,
                depth_db: 0.6,
            },
            Dip {
                frame: 99,
                depth_db: 0.6,
            },
        ];
        let out = apply_dips(&[], &dips, 8, 100).unwrap();
        assert_eq!(out.first().unwrap().frame, 0);
        assert_eq!(out.last().unwrap().frame, 100);
        assert!(out.windows(2).all(|w| w[0].frame < w[1].frame));
        assert!(out.iter().all(|p| p.frame <= 100));
    }

    #[test]
    fn too_many_points_are_refused() {
        let dips: Vec<Dip> = (0..200u64)
            .map(|k| Dip {
                frame: 100 + k * 100,
                depth_db: 0.3,
            })
            .collect();
        assert!(apply_dips(&[], &dips, 8, 100_000).is_err());
        assert_eq!(dips_that_fit(0), 170);
        assert_eq!(dips_that_fit(MAX_GAIN_POINTS), 0);
        assert!(apply_dips(&[], &dips[..dips_that_fit(0)], 8, 100_000).is_ok());
    }
}
