//! Grading a played pass against the tab: pure and headless.
//!
//! The listen-and-grade practice feature hears the instrument (see `atomik_dsp::listen`), turns the
//! notes it heard into [`Played`] events on the song's tick axis and hands them, with the
//! [`Expected`] notes of the looped range, to [`grade_pass`]. Nothing here knows about audio.
//!
//! **Matching.** Expected notes that start together are one group (a chord when it holds two keys
//! or more). A played event may answer a group when it lies within the window
//! ([`Params::window_ticks`]) of it. Pairs are taken in two rounds, nearest in time first, each
//! expected group and each event used at most once:
//!
//! 1. events whose *pitch is right* ([`pitch_matches`]): a single note by its pitch class (an
//!    octave off counts: tracking an octave wrong is a detector habit, and the same class is the
//!    note), without a pitch by the chroma; a chord by [`chord_score`] of the event's chroma;
//! 2. events left over inside the window of a group still unmatched: *wrong* notes.
//!
//! A group nobody answered is *missed*; an event nobody claimed is an *extra*. The verdict of a
//! right note is [`Verdict::Hit`] within [`Params::hit_ticks`] (half the window), else
//! [`Verdict::Early`] or [`Verdict::Late`].
//!
//! **Accuracy** is `(hits + 0.5 * (early + late)) / (groups + extras)`: a wrong note, a missed
//! one and an extra all cost the same, and a right note a little off the beat gets half.
//!
//! **Windows in ticks.** A player's timing error is in wall-clock time, so the window is given in
//! milliseconds ([`DEFAULT_WINDOW_MS`]) and turned to ticks with the tick rate of the pass
//! ([`Params::from_ms`]): at half speed the same window spans half as many song ticks.
//!
//! **Calibration** ([`calibration_offset_ms`]): the median of how far the notes played along a click
//! lie after the beats; added to the manual offset it centres the setup's latency (a positive
//! offset places played notes earlier).

/// The default half-width of the window, milliseconds of wall-clock time.
pub const DEFAULT_WINDOW_MS: f64 = 80.0;
/// The default share of the chroma energy a chord must have (see [`chord_score`]).
pub const DEFAULT_CHORD_THRESHOLD: f32 = 0.6;
/// The default accuracy a pass needs to be clean, as a fraction.
pub const DEFAULT_CLEAN_ACCURACY: f32 = 0.9;
/// Passes the session history keeps.
pub const HISTORY_LEN: usize = 12;
/// A chroma bin at or above this (the largest is 1) counts as a pitch class that is there.
const PRESENT: f32 = 0.2;

/// What the grader hears of a played note (the host builds it from the tracker's output).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Played {
    /// Where on the song's tick axis the note started (fractional, latency already removed).
    pub tick: f64,
    /// The MIDI key of its pitch, when it has one.
    pub key: Option<i32>,
    /// The chroma (pitch class 0 = C; the largest value 1; all zero when unknown).
    pub chroma: [f32; 12],
}

/// A note group the tab asks for: the notes that start at one tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expected {
    /// Song tick the group starts at.
    pub tick: i64,
    /// The MIDI keys, sorted and without repeats.
    pub keys: Vec<u8>,
}

impl Expected {
    /// A group at `tick`.
    pub fn new(tick: i64, keys: impl IntoIterator<Item = u8>) -> Self {
        let mut keys: Vec<u8> = keys.into_iter().collect();
        keys.sort_unstable();
        keys.dedup();
        Self { tick, keys }
    }

    /// Whether the group is a chord (two keys or more).
    pub fn is_chord(&self) -> bool {
        self.keys.len() >= 2
    }
}

/// Groups notes `(tick, key)` that start together into [`Expected`] groups, in time order.
pub fn group_notes(notes: impl IntoIterator<Item = (i64, u8)>) -> Vec<Expected> {
    let mut all: Vec<(i64, u8)> = notes.into_iter().collect();
    all.sort_unstable();
    let mut out: Vec<Expected> = Vec::new();
    for (tick, key) in all {
        match out.last_mut() {
            Some(g) if g.tick == tick => {
                if !g.keys.contains(&key) {
                    g.keys.push(key);
                }
            }
            _ => out.push(Expected {
                tick,
                keys: vec![key],
            }),
        }
    }
    out
}

/// How a group was played.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Verdict {
    /// The right notes, on time.
    Hit,
    /// The right notes, before the beat.
    Early,
    /// The right notes, after the beat.
    Late,
    /// Something was played in time, but not the right notes.
    Wrong,
    /// Nothing was played.
    Missed,
}

impl Verdict {
    /// Whether the notes were right (a hit, early or late).
    pub fn is_right(self) -> bool {
        matches!(self, Verdict::Hit | Verdict::Early | Verdict::Late)
    }

    /// Points towards the accuracy.
    pub fn points(self) -> f32 {
        match self {
            Verdict::Hit => 1.0,
            Verdict::Early | Verdict::Late => 0.5,
            Verdict::Wrong | Verdict::Missed => 0.0,
        }
    }
}

/// The tolerances of a pass.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    /// Half-width of the window, in ticks.
    pub window_ticks: f64,
    /// Half-width of the on-time zone, in ticks.
    pub hit_ticks: f64,
    /// The least [`chord_score`] that counts as the right chord.
    pub chord_threshold: f32,
}

impl Params {
    /// A window of `window_ms` at `ticks_per_ms` song ticks per millisecond of wall-clock time
    /// (the tempo, times the practice speed, times the tick resolution): the on-time zone is half
    /// of it.
    pub fn from_ms(window_ms: f64, ticks_per_ms: f64) -> Self {
        let window = (window_ms * ticks_per_ms).max(1.0);
        Self {
            window_ticks: window,
            hit_ticks: window / 2.0,
            chord_threshold: DEFAULT_CHORD_THRESHOLD,
        }
    }
}

/// The grade of one expected group.
#[derive(Debug, Clone, PartialEq)]
pub struct NoteGrade {
    /// The group.
    pub expected: Expected,
    /// What happened to it.
    pub verdict: Verdict,
    /// How far the played note was from it, ticks (positive = late); none when nothing answered
    /// or the answer was wrong and not timed.
    pub offset_ticks: Option<f64>,
}

/// Counts of the verdicts of a pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Tally {
    /// Right and on time.
    pub hit: u32,
    /// Right, early.
    pub early: u32,
    /// Right, late.
    pub late: u32,
    /// Played, but wrong.
    pub wrong: u32,
    /// Not played.
    pub missed: u32,
    /// Played but not asked for.
    pub extra: u32,
}

impl Tally {
    /// Groups graded.
    pub fn expected(&self) -> u32 {
        self.hit + self.early + self.late + self.wrong + self.missed
    }
}

/// The grade of a whole pass.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PassGrade {
    /// One entry per expected group, in time order.
    pub notes: Vec<NoteGrade>,
    /// Events no group claimed.
    pub extras: u32,
}

impl PassGrade {
    /// The counts.
    pub fn tally(&self) -> Tally {
        let mut t = Tally {
            extra: self.extras,
            ..Tally::default()
        };
        for n in &self.notes {
            match n.verdict {
                Verdict::Hit => t.hit += 1,
                Verdict::Early => t.early += 1,
                Verdict::Late => t.late += 1,
                Verdict::Wrong => t.wrong += 1,
                Verdict::Missed => t.missed += 1,
            }
        }
        t
    }

    /// The accuracy as a fraction (module docs); `None` when the pass had nothing to play and
    /// nothing was played.
    pub fn accuracy(&self) -> Option<f32> {
        let t = self.tally();
        let total = t.expected() + t.extra;
        if total == 0 {
            return None;
        }
        let points = t.hit as f32 + 0.5 * (t.early + t.late) as f32;
        Some(points / total as f32)
    }

    /// The accuracy in whole percent, 0 for a pass with nothing in it.
    pub fn percent(&self) -> u8 {
        self.accuracy()
            .map_or(0, |a| (a * 100.0).round().clamp(0.0, 100.0) as u8)
    }
}

/// The bit mask of the pitch classes of `keys` (bit 0 = C).
pub fn pc_mask(keys: &[u8]) -> u16 {
    keys.iter().fold(0u16, |m, k| m | 1 << (k % 12))
}

/// How well `chroma` (largest value 1) is the chord of pitch classes `mask`, 0 to 1: the share of
/// the chroma energy on the chord's classes, times `0.25 + 0.75 * coverage`, where coverage is the
/// share of the chord's classes that are present. One note of a triad gets about 0.5, two thirds
/// of it 0.7, the whole chord 0.9, another chord with one shared class 0.2.
pub fn chord_score(chroma: &[f32; 12], mask: u16) -> f32 {
    let total: f32 = chroma.iter().sum();
    if total <= 1e-6 || mask == 0 {
        return 0.0;
    }
    let (mut inside, mut present, mut wanted) = (0.0f32, 0u32, 0u32);
    for (pc, v) in chroma.iter().enumerate() {
        if mask >> pc & 1 == 1 {
            inside += v;
            wanted += 1;
            if *v >= PRESENT {
                present += 1;
            }
        }
    }
    let coverage = present as f32 / wanted as f32;
    inside / total * (0.25 + 0.75 * coverage)
}

/// Whether `played` is what `expected` asks for (module docs).
pub fn pitch_matches(expected: &Expected, played: &Played, chord_threshold: f32) -> bool {
    let mask = pc_mask(&expected.keys);
    if let (false, Some(key)) = (expected.is_chord(), played.key) {
        return mask >> key.rem_euclid(12) & 1 == 1;
    }
    chord_score(&played.chroma, mask) >= chord_threshold
}

fn verdict_for(dt: f64, hit_ticks: f64) -> Verdict {
    if dt.abs() <= hit_ticks {
        Verdict::Hit
    } else if dt < 0.0 {
        Verdict::Early
    } else {
        Verdict::Late
    }
}

/// Grades one pass: `expected` (any order) against `played` (any order). Events outside every
/// window are extras.
pub fn grade_pass(expected: &[Expected], played: &[Played], params: &Params) -> PassGrade {
    let mut order: Vec<usize> = (0..expected.len()).collect();
    order.sort_by_key(|&i| expected[i].tick);
    let mut verdicts: Vec<Option<(Verdict, Option<f64>)>> = vec![None; expected.len()];
    let mut used = vec![false; played.len()];

    // Every pair inside the window, nearest first.
    let mut pairs: Vec<(f64, usize, usize, bool)> = Vec::new();
    for (i, e) in expected.iter().enumerate() {
        for (j, p) in played.iter().enumerate() {
            let dt = p.tick - e.tick as f64;
            if dt.abs() <= params.window_ticks {
                pairs.push((dt.abs(), i, j, pitch_matches(e, p, params.chord_threshold)));
            }
        }
    }
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    for right_round in [true, false] {
        for &(_, i, j, right) in &pairs {
            if right != right_round || verdicts[i].is_some() || used[j] {
                continue;
            }
            let dt = played[j].tick - expected[i].tick as f64;
            used[j] = true;
            verdicts[i] = Some(if right {
                (verdict_for(dt, params.hit_ticks), Some(dt))
            } else {
                (Verdict::Wrong, Some(dt))
            });
        }
    }
    let notes = order
        .into_iter()
        .map(|i| {
            let (verdict, offset_ticks) = verdicts[i].unwrap_or((Verdict::Missed, None));
            NoteGrade {
                expected: expected[i].clone(),
                verdict,
                offset_ticks,
            }
        })
        .collect();
    PassGrade {
        notes,
        extras: used.iter().filter(|u| !**u).count() as u32,
    }
}

/// One finished pass in the session history.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PassSummary {
    /// Accuracy in whole percent.
    pub percent: u8,
    /// The counts.
    pub tally: Tally,
    /// The practice speed it was played at, percent.
    pub speed: u16,
}

/// The passes of a practice session: history, streak and best streak. Not saved anywhere.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Session {
    history: Vec<PassSummary>,
    passes: u32,
    streak: u32,
    best: u32,
}

impl Session {
    /// Adds a graded pass played at `speed` percent; returns its summary. The streak counts the
    /// groups played right in a row, across passes, in time order; a wrong or missed one ends it.
    pub fn record(&mut self, grade: &PassGrade, speed: u16) -> PassSummary {
        for n in &grade.notes {
            if n.verdict.is_right() {
                self.streak += 1;
                self.best = self.best.max(self.streak);
            } else {
                self.streak = 0;
            }
        }
        let summary = PassSummary {
            percent: grade.percent(),
            tally: grade.tally(),
            speed,
        };
        self.passes += 1;
        self.history.push(summary);
        if self.history.len() > HISTORY_LEN {
            self.history.remove(0);
        }
        summary
    }

    /// The newest passes, oldest first (at most [`HISTORY_LEN`]).
    pub fn history(&self) -> &[PassSummary] {
        &self.history
    }

    /// Passes graded since the session was cleared.
    pub fn passes(&self) -> u32 {
        self.passes
    }

    /// The groups right in a row right now.
    pub fn streak(&self) -> u32 {
        self.streak
    }

    /// The longest streak.
    pub fn best_streak(&self) -> u32 {
        self.best
    }

    /// The last pass, if any.
    pub fn last(&self) -> Option<&PassSummary> {
        self.history.last()
    }

    /// Forgets everything.
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

/// How far each of `ticks` lies from the nearest beat (a multiple of `beat_ticks`), positive when
/// after it.
pub fn beat_offsets(ticks: &[f64], beat_ticks: f64) -> Vec<f64> {
    if beat_ticks <= 0.0 {
        return Vec::new();
    }
    ticks
        .iter()
        .map(|t| t - (t / beat_ticks).round() * beat_ticks)
        .collect()
}

/// The correction, milliseconds, to add to the manual offset after a player played along a click:
/// the median of how far the played notes lie after the beats (`offsets_ms`, positive = late, the
/// notes already placed with the current offset). Values further than 250 ms from the beat are not
/// taken as playing along. `None` with fewer than three usable values.
pub fn calibration_offset_ms(offsets_ms: &[f64]) -> Option<f64> {
    let mut v: Vec<f64> = offsets_ms
        .iter()
        .copied()
        .filter(|o| o.is_finite() && o.abs() <= 250.0)
        .collect();
    if v.len() < 3 {
        return None;
    }
    v.sort_by(f64::total_cmp);
    let mid = v.len() / 2;
    let median = if v.len() % 2 == 1 {
        v[mid]
    } else {
        (v[mid - 1] + v[mid]) / 2.0
    };
    Some(median)
}

#[cfg(test)]
mod tests;
