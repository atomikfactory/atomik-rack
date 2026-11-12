//! Chords and keys from the chroma of an [`Analysis`].
//!
//! **Units.** The caller cuts the song into units on its beat grid (a beat, half a bar or a bar:
//! the "changes" setting) given as source frames; [`pool_frames`] sums the chroma, bass chroma and
//! level of the frames centred in each unit. Chords change only between units and at most once
//! per unit.
//!
//! **Scoring.** Each unit's chroma is compared with a template of every chord quality on the
//! twelve roots by cosine (the template has a 1 on each chord tone). Triads come first: every
//! other quality pays a small penalty, so a seventh wins only when its seventh is clearly there,
//! and a sixth, which shares its notes with the minor seventh a third below, pays more. A root
//! that is also the unit's bass earns a small bonus, which tells a minor seventh from its sixth
//! and a sus2 from the sus4 a fifth above. "No chord" scores a constant that a flat chroma
//! (noise) beats every triad with; a silent unit is "no chord" for sure. With
//! [`ChordOptions::full`] off only the triads are tried.
//!
//! **Smoothing.** A Viterbi pass over the units maximises the summed scores plus a bonus for
//! every unit that keeps the chord of the one before, so a chord holds through a passing note
//! and changes only where the harmony really moves.
//!
//! **Bass.** With [`ChordOptions::slash`] each unit whose bass chroma (the lowest strong voice of
//! each frame) is at least half on one pitch class that is not the root, and is a chord tone or a
//! step of the root's scale (anything but a semitone or a tritone away), gives its chord that
//! bass: `C/G`. A unit with no clear bass keeps the bass of the unit before; a chord whose bass
//! moves is cut where it moves.
//!
//! **Key.** [`key_profile`] is the chroma summed over the whole take, which the key profiles of
//! the model compare with the major and minor keys.

use super::{Analysis, Frame};
use atomik_core::theory::ChordKind;

/// The score of "no chord" (a flat chroma scores 0.5 with every triad).
const NO_CHORD_SCORE: f64 = 0.55;
/// The bonus of a unit that keeps the chord of the unit before.
const STAY_BONUS: f64 = 0.08;
/// The bonus of a chord whose root is the unit's bass.
const BASS_BONUS: f64 = 0.04;
/// A unit quieter than the loudest unit by more than this (dB) is silent.
const SILENT_DB: f32 = 40.0;
/// A unit with voices in fewer than one frame in this many is silent (the edge of the sound of
/// the unit next to it).
const VOICED_SHARE_INV: u32 = 3;
/// The share of the bass chroma one pitch class needs to be the bass of a chord.
const BASS_HOLD: f32 = 0.5;

/// The qualities tried, with what each pays: the triads first.
const QUALITIES: [(ChordKind, f64); 13] = [
    (ChordKind::Major, 0.0),
    (ChordKind::Minor, 0.0),
    (ChordKind::Sus4, 0.03),
    (ChordKind::Dim, 0.03),
    (ChordKind::Sus2, 0.04),
    (ChordKind::Aug, 0.05),
    (ChordKind::Seventh, 0.04),
    (ChordKind::MinorSeventh, 0.04),
    (ChordKind::MajorSeventh, 0.045),
    (ChordKind::HalfDim, 0.05),
    (ChordKind::Sixth, 0.06),
    (ChordKind::MinorSixth, 0.06),
    (ChordKind::DimSeventh, 0.06),
];
/// How many of [`QUALITIES`] are triads.
const TRIADS: usize = 6;

/// What the recognition tries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChordOptions {
    /// Sevenths and sixths as well as triads (else triads only).
    pub full: bool,
    /// Give a chord its bass note when it is not the root (slash chords).
    pub slash: bool,
}

impl Default for ChordOptions {
    fn default() -> Self {
        Self {
            full: true,
            slash: true,
        }
    }
}

/// The summed analysis of one unit.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BeatChroma {
    /// Chroma, C first.
    pub chroma: [f32; 12],
    /// Bass chroma, C first.
    pub bass: [f32; 12],
    /// The sum of the frames' levels.
    pub level: f32,
    /// How many frames were added.
    pub frames: u32,
    /// How many of them had voices.
    pub voiced: u32,
}

impl BeatChroma {
    /// Adds one frame.
    pub fn add(&mut self, f: &Frame) {
        for i in 0..12 {
            self.chroma[i] += f.chroma[i];
            self.bass[i] += f.bass[i];
        }
        self.level += f.level;
        self.frames += 1;
        self.voiced += u32::from(!f.voices.is_empty());
    }

    /// Adds another unit.
    pub fn merge(&mut self, o: &BeatChroma) {
        for i in 0..12 {
            self.chroma[i] += o.chroma[i];
            self.bass[i] += o.bass[i];
        }
        self.level += o.level;
        self.frames += o.frames;
        self.voiced += o.voiced;
    }

    /// The mean level of the frames.
    pub fn mean_level(&self) -> f32 {
        if self.frames == 0 {
            0.0
        } else {
            self.level / self.frames as f32
        }
    }
}

/// The units of `a` cut at `bounds` (source frames, ascending): unit `i` holds the frames
/// centred in `[bounds[i], bounds[i + 1])`, so there is one unit fewer than bounds.
pub fn pool_frames(a: &Analysis, bounds: &[u64]) -> Vec<BeatChroma> {
    let units = bounds.len().saturating_sub(1);
    let mut out = vec![BeatChroma::default(); units];
    if units == 0 {
        return out;
    }
    for (k, f) in a.frames.iter().enumerate() {
        let c = a.centre(k);
        let i = bounds.partition_point(|b| *b <= c);
        if i == 0 || i > units {
            continue;
        }
        out[i - 1].add(f);
    }
    out
}

/// A chord over units `first..end` (indexes of the units given to [`recognise_chords`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChordSpan {
    /// The first unit.
    pub first: usize,
    /// The unit after the last.
    pub end: usize,
    /// Root pitch class, `0..12`.
    pub root: u8,
    /// Quality.
    pub quality: ChordKind,
    /// Bass pitch class when it is not the root.
    pub bass: Option<u8>,
    /// How well the chroma fits the chord, `0..=1`.
    pub confidence: f32,
}

/// A chord over source frames.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChordGuess {
    /// First source frame.
    pub start_frame: u64,
    /// The source frame after the chord.
    pub end_frame: u64,
    /// Root pitch class, `0..12`.
    pub root: u8,
    /// Quality.
    pub quality: ChordKind,
    /// Bass pitch class when it is not the root.
    pub bass: Option<u8>,
    /// How well the chroma fits the chord, `0..=1`.
    pub confidence: f32,
}

/// The template of `quality` on `root`, normalised.
fn template(root: u8, quality: ChordKind) -> [f64; 12] {
    let mut t = [0.0f64; 12];
    for i in quality.intervals() {
        t[usize::from((root + i) % 12)] = 1.0;
    }
    let n = t.iter().map(|v| v * v).sum::<f64>().sqrt();
    for v in &mut t {
        *v /= n;
    }
    t
}

/// The pitch class with the most weight, and its share of the total (`None`: all zero).
fn strongest(v: &[f32; 12]) -> Option<(u8, f32)> {
    let total: f32 = v.iter().sum();
    if total <= 0.0 {
        return None;
    }
    let mut best = 0usize;
    for i in 1..12 {
        if v[i] > v[best] {
            best = i;
        }
    }
    Some((best as u8, v[best] / total))
}

/// The scores of every state (chords, then "no chord" last) for one unit.
fn scores(u: &BeatChroma, silent: bool, qualities: &[(ChordKind, f64)], out: &mut [f64]) {
    let states = out.len();
    if silent {
        out.fill(0.0);
        out[states - 1] = 1.0;
        return;
    }
    let norm = u
        .chroma
        .iter()
        .map(|v| f64::from(*v) * f64::from(*v))
        .sum::<f64>()
        .sqrt();
    let bass = strongest(&u.bass).map(|(pc, _)| pc);
    let mut i = 0;
    for root in 0..12u8 {
        for (q, penalty) in qualities {
            let t = template(root, *q);
            let dot: f64 = (0..12).map(|k| f64::from(u.chroma[k]) * t[k]).sum();
            let mut s = dot / norm - penalty;
            if bass == Some(root) {
                s += BASS_BONUS;
            }
            out[i] = s;
            i += 1;
        }
    }
    out[states - 1] = NO_CHORD_SCORE;
}

/// The chords of `units` (see the module docs). Units with no chord are gaps; equal neighbours are
/// one chord.
pub fn recognise_chords(units: &[BeatChroma], opts: &ChordOptions) -> Vec<ChordSpan> {
    if units.is_empty() {
        return Vec::new();
    }
    let qualities = if opts.full {
        &QUALITIES[..]
    } else {
        &QUALITIES[..TRIADS]
    };
    let per = qualities.len();
    let states = 12 * per + 1;
    let top = units.iter().fold(0.0f32, |m, u| m.max(u.mean_level()));
    let floor = top * crate::det_math::exp2_det(-SILENT_DB / 6.020_6);
    let silent = |u: &BeatChroma| {
        u.frames == 0
            || u.mean_level() <= floor
            || u.chroma.iter().sum::<f32>() <= 0.0
            || u.voiced * VOICED_SHARE_INV < u.frames
    };
    let mut emit = vec![0.0f64; units.len() * states];
    for (t, u) in units.iter().enumerate() {
        scores(
            u,
            silent(u),
            qualities,
            &mut emit[t * states..(t + 1) * states],
        );
    }
    // Viterbi with a uniform change cost.
    let mut delta: Vec<f64> = emit[..states].to_vec();
    let mut back: Vec<u16> = vec![0; units.len() * states];
    for t in 1..units.len() {
        let mut best_any = 0usize;
        for s in 1..states {
            if delta[s] > delta[best_any] {
                best_any = s;
            }
        }
        let mut next = vec![0.0f64; states];
        for s in 0..states {
            let stay = delta[s] + STAY_BONUS;
            let (from, base) = if stay >= delta[best_any] {
                (s, stay)
            } else {
                (best_any, delta[best_any])
            };
            next[s] = base + emit[t * states + s];
            back[t * states + s] = from as u16;
        }
        delta = next;
    }
    let mut last = 0usize;
    for s in 1..states {
        if delta[s] > delta[last] {
            last = s;
        }
    }
    let mut path = vec![0usize; units.len()];
    path[units.len() - 1] = last;
    for t in (1..units.len()).rev() {
        path[t - 1] = usize::from(back[t * states + path[t]]);
    }
    // The bass of each unit: a clear pitch class, else the bass of the unit before when it has
    // the same chord.
    let mut basses: Vec<Option<u8>> = vec![None; units.len()];
    for t in 0..units.len() {
        let s = path[t];
        if !opts.slash || s == states - 1 {
            continue;
        }
        let root = (s / per) as u8;
        basses[t] = match strongest(&units[t].bass) {
            Some((pc, share)) if share >= BASS_HOLD => {
                let step = (pc + 12 - root) % 12;
                (pc != root && step != 1 && step != 6).then_some(pc)
            }
            _ if t > 0 && path[t - 1] == s => basses[t - 1],
            _ => None,
        };
    }
    let mut out: Vec<ChordSpan> = Vec::new();
    let mut t = 0;
    while t < units.len() {
        let s = path[t];
        let bass = basses[t];
        let mut end = t + 1;
        while end < units.len() && path[end] == s && basses[end] == bass {
            end += 1;
        }
        if s != states - 1 {
            let (root, quality) = ((s / per) as u8, qualities[s % per].0);
            let fit: f32 = (t..end)
                .map(|i| ((emit[i * states + s] - 0.5) / 0.5).clamp(0.0, 1.0) as f32)
                .sum::<f32>()
                / (end - t) as f32;
            out.push(ChordSpan {
                first: t,
                end,
                root,
                quality,
                bass,
                confidence: fit,
            });
        }
        t = end;
    }
    out
}

/// The chords of `a` with a change allowed every `beats_per_change` beats of `beats` (the source
/// frames each beat starts at, ascending); the last unit runs to the end of the take.
pub fn detect_chords(
    a: &Analysis,
    beats: &[u64],
    beats_per_change: usize,
    opts: &ChordOptions,
) -> Vec<ChordGuess> {
    let end = a.centre(a.frames.len());
    let mut bounds: Vec<u64> = beats
        .iter()
        .step_by(beats_per_change.max(1))
        .copied()
        .filter(|b| *b < end)
        .collect();
    if bounds.is_empty() {
        return Vec::new();
    }
    bounds.push(end);
    let units = pool_frames(a, &bounds);
    recognise_chords(&units, opts)
        .into_iter()
        .map(|c| ChordGuess {
            start_frame: bounds[c.first],
            end_frame: bounds[c.end],
            root: c.root,
            quality: c.quality,
            bass: c.bass,
            confidence: c.confidence,
        })
        .collect()
}

/// The chroma of the whole take (louder and longer sound counts more), C first: what the key
/// profiles are compared with.
pub fn key_profile(a: &Analysis) -> [f64; 12] {
    let mut out = [0.0f64; 12];
    for f in &a.frames {
        for (o, c) in out.iter_mut().zip(f.chroma) {
            *o += f64::from(c);
        }
    }
    out
}
