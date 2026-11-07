//! Kick and bass check: how the sub range of a kick and a bass add up, hit by hit.
//!
//! The inputs are the two signals as they reach the mix (mono sums of the kick alone and the
//! bass alone, same length, same rate). [`check`] works in the sub range (below [`SUB_HZ`]):
//!
//! 1. Both signals go through a fourth-order low-pass (two Butterworth sections) at
//!    [`SUB_HZ`]. The filter is linear, so the sub of the sum is the sum of the two subs.
//! 2. The hits are the onsets of the kick ([`crate::detect_onsets`]). The window of a hit runs
//!    from its onset to the next onset, at most [`WINDOW_SECONDS`] long; a window shorter than
//!    [`MIN_WINDOW_SECONDS`] is skipped.
//! 3. In each window the mean power of the kick sub `Pk`, of the bass sub `Pb` and of their sum
//!    `Ps` are measured. Two sources that do not care about each other add in power, so the
//!    expectation is `Pk + Pb`; the **delta** is `10 log10(Ps / (Pk + Pb))` in dB. Below
//!    `-`[`NOTICE_DB`] the sum is quieter than expected (the two **cancel** each other), above
//!    `+`[`NOTICE_DB`] it is louder (they **build up**: the two are in phase, which is why a
//!    sub that is too loud on those hits is also the one that clips the master). A window whose
//!    expected power is below [`FLOOR_POWER`] counts as silent and is not reported.
//! 4. The **fundamentals** are the median over the hits of the pitch of each sub in the second
//!    half of its window, measured from the spacing of the rising zero crossings (a sub is close
//!    to a sine, so the median spacing is the period). When the two are within
//!    [`BEAT_SEMITONES`] of each other and not identical they beat: the level of their sum
//!    swells and fades at the difference of the two frequencies, so the low end is uneven from
//!    note to note.
//!
//! The analysis is pure and deterministic (IEEE `+ - * /`, `sqrt`, `log10`, `powf`, `log2`) and
//! allocates; it runs on a worker, never on the audio thread.

use crate::biquad::{Biquad, BiquadCoeffs, BiquadShape};

/// The top of the sub range, Hz.
pub const SUB_HZ: f32 = 120.0;
/// The longest window of a hit, seconds.
pub const WINDOW_SECONDS: f64 = 0.30;
/// The shortest window of a hit that is looked at, seconds.
pub const MIN_WINDOW_SECONDS: f64 = 0.05;
/// A hit whose sum is this far (dB) from the power sum of the two counts as cancelling or
/// building up.
pub const NOTICE_DB: f32 = 1.5;
/// Two fundamentals this close (semitones) beat.
pub const BEAT_SEMITONES: f32 = 2.0;
/// Fundamentals closer than this (semitones) are the same pitch: no beat, only a phase.
pub const SAME_SEMITONES: f32 = 0.05;
/// Mean power below which a window is silent (about -80 dBFS).
pub const FLOOR_POWER: f64 = 1.0e-8;
/// How long after its onset the kick is left alone before its pitch is read, seconds.
const SETTLE_SECONDS: f64 = 0.02;
/// Onset sensitivity of the hit detection.
const SENSITIVITY: f32 = 0.5;

/// What a hit does to the sub range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The sum is quieter than the power sum of the two.
    Cancel,
    /// The sum is louder than the power sum of the two.
    BuildUp,
    /// The sum is about the power sum.
    Even,
}

/// One kick hit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HitCheck {
    /// The first frame of the hit (the kick onset).
    pub frame: u64,
    /// Frames of its window.
    pub frames: u64,
    /// Mean power of the kick sub in the window, dB.
    pub kick_db: f32,
    /// Mean power of the bass sub in the window, dB.
    pub bass_db: f32,
    /// Mean power of the sum, dB.
    pub sum_db: f32,
    /// Sum minus the power sum of the two, dB (negative: cancelling).
    pub delta_db: f32,
    /// What it comes to.
    pub verdict: Verdict,
}

/// The result of [`check`].
#[derive(Debug, Clone, PartialEq)]
pub struct Check {
    /// The rate of the signals.
    pub sample_rate: u32,
    /// Frames of the signals.
    pub frames: u64,
    /// The hits that carry sound, in time order.
    pub hits: Vec<HitCheck>,
    /// The fundamental of the kick, Hz.
    pub kick_hz: Option<f32>,
    /// The fundamental of the bass, Hz.
    pub bass_hz: Option<f32>,
    /// The distance between the two fundamentals, semitones (never negative).
    pub semitones: Option<f32>,
    /// The two fundamentals are within [`BEAT_SEMITONES`] and not the same pitch.
    pub beating: bool,
    /// The beat rate, Hz, when they beat.
    pub beat_hz: Option<f32>,
    /// Over all the hits: the sum against the power sum of the two, dB (0 without hits).
    pub overall_db: f32,
    /// The lowest delta of any hit, dB (0 without hits).
    pub worst_db: f32,
}

impl Check {
    /// How many hits cancel.
    pub fn cancelling(&self) -> usize {
        self.count(Verdict::Cancel)
    }

    /// How many hits build up.
    pub fn building(&self) -> usize {
        self.count(Verdict::BuildUp)
    }

    fn count(&self, verdict: Verdict) -> usize {
        self.hits.iter().filter(|h| h.verdict == verdict).count()
    }

    /// The score a fix is judged by: the overall delta, with the worst hit counted in so that a
    /// fix that removes one deep cancellation beats one that nudges the average.
    pub fn score(&self) -> f32 {
        self.overall_db + 0.25 * self.worst_db
    }
}

/// The sub range of `x` at `sample_rate`: a fourth-order low-pass at [`SUB_HZ`].
pub fn sub_band(x: &[f32], sample_rate: u32) -> Vec<f32> {
    let c = BiquadCoeffs::design(
        BiquadShape::Lowpass,
        f64::from(SUB_HZ),
        std::f64::consts::FRAC_1_SQRT_2,
        0.0,
        f64::from(sample_rate),
    );
    let mut sections = [Biquad::default(); 2];
    x.iter()
        .map(|&s| {
            let mut v = if s.is_finite() { f64::from(s) } else { 0.0 };
            for section in &mut sections {
                v = section.process(&c, v);
            }
            v as f32
        })
        .collect()
}

/// The mono sum of a stereo pair (the shorter channel sets the length).
pub fn mono(left: &[f32], right: &[f32]) -> Vec<f32> {
    left.iter()
        .zip(right)
        .map(|(&l, &r)| {
            let (l, r) = (
                if l.is_finite() { l } else { 0.0 },
                if r.is_finite() { r } else { 0.0 },
            );
            0.5 * (l + r)
        })
        .collect()
}

fn power(x: &[f32]) -> f64 {
    if x.is_empty() {
        return 0.0;
    }
    x.iter().map(|&s| f64::from(s) * f64::from(s)).sum::<f64>() / x.len() as f64
}

fn db(p: f64) -> f32 {
    (10.0 * p.max(1.0e-12).log10()) as f32
}

/// The pitch in Hz of a near-sine `x`: the median spacing of its rising zero crossings (the
/// crossing instants are interpolated). `None` for a signal that is too quiet or has fewer
/// than three crossings.
pub fn tone_hz(x: &[f32], sample_rate: u32) -> Option<f32> {
    if power(x) < FLOOR_POWER {
        return None;
    }
    let mut crossings = Vec::new();
    for (i, w) in x.windows(2).enumerate() {
        if w[0] < 0.0 && w[1] >= 0.0 {
            let t = f64::from(-w[0]) / f64::from(w[1] - w[0]);
            crossings.push(i as f64 + t);
        }
    }
    if crossings.len() < 3 {
        return None;
    }
    let mut periods: Vec<f64> = crossings.windows(2).map(|c| c[1] - c[0]).collect();
    periods.sort_by(f64::total_cmp);
    let median = periods[periods.len() / 2];
    (median > 0.0).then(|| (f64::from(sample_rate) / median) as f32)
}

fn median(mut v: Vec<f32>) -> Option<f32> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f32::total_cmp);
    Some(v[v.len() / 2])
}

fn seconds_to_frames(seconds: f64, rate: u32) -> usize {
    (seconds * f64::from(rate)).round().max(0.0) as usize
}

/// The kick hits of `kick` (onset frames): see the module docs.
pub fn hit_frames(kick: &[f32], sample_rate: u32) -> Vec<u64> {
    crate::detect_onsets(&[kick], sample_rate, SENSITIVITY)
}

/// Checks the sub range of `kick` and `bass` (see the module docs). The two are cut to the
/// shorter length.
pub fn check(kick: &[f32], bass: &[f32], sample_rate: u32) -> Check {
    let frames = kick.len().min(bass.len());
    let kick = Kick::new(&kick[..frames], sample_rate);
    kick.against(&sub_band(&bass[..frames], sample_rate))
}

/// The kick side of a check: its sub range and its hits, found once for any number of basses.
#[derive(Debug, Clone)]
struct Kick {
    sub: Vec<f32>,
    onsets: Vec<u64>,
    sample_rate: u32,
}

impl Kick {
    fn new(kick: &[f32], sample_rate: u32) -> Self {
        Self {
            sub: sub_band(kick, sample_rate),
            onsets: hit_frames(kick, sample_rate),
            sample_rate,
        }
    }

    /// The check against a bass sub of the same length.
    fn against(&self, bass_sub: &[f32]) -> Check {
        analyse(&self.sub, bass_sub, &self.onsets, self.sample_rate)
    }
}

fn analyse(kick_sub: &[f32], bass_sub: &[f32], onsets: &[u64], sample_rate: u32) -> Check {
    let frames = kick_sub.len().min(bass_sub.len());
    let sum: Vec<f32> = kick_sub.iter().zip(bass_sub).map(|(k, b)| k + b).collect();
    let longest = seconds_to_frames(WINDOW_SECONDS, sample_rate);
    let shortest = seconds_to_frames(MIN_WINDOW_SECONDS, sample_rate);
    let settle = seconds_to_frames(SETTLE_SECONDS, sample_rate);
    let mut hits = Vec::new();
    let (mut kick_tones, mut bass_tones) = (Vec::new(), Vec::new());
    let (mut total_sum, mut total_expected) = (0.0f64, 0.0f64);
    for (i, &start) in onsets.iter().enumerate() {
        let start = start as usize;
        let next = onsets.get(i + 1).map_or(frames, |&n| n as usize);
        let end = next.min(start + longest).min(frames);
        if end <= start || end - start < shortest {
            continue;
        }
        let (pk, pb, ps) = (
            power(&kick_sub[start..end]),
            power(&bass_sub[start..end]),
            power(&sum[start..end]),
        );
        if pk + pb < FLOOR_POWER {
            continue;
        }
        let delta = db(ps) - db(pk + pb);
        let verdict = if delta < -NOTICE_DB {
            Verdict::Cancel
        } else if delta > NOTICE_DB {
            Verdict::BuildUp
        } else {
            Verdict::Even
        };
        let weight = (end - start) as f64;
        total_sum += ps * weight;
        total_expected += (pk + pb) * weight;
        hits.push(HitCheck {
            frame: start as u64,
            frames: (end - start) as u64,
            kick_db: db(pk),
            bass_db: db(pb),
            sum_db: db(ps),
            delta_db: delta,
            verdict,
        });
        let from = (start + settle).min(end);
        let half = from + (end - from) / 2;
        if let Some(hz) = tone_hz(&kick_sub[half..end], sample_rate) {
            kick_tones.push(hz);
        }
        if let Some(hz) = tone_hz(&bass_sub[half..end], sample_rate) {
            bass_tones.push(hz);
        }
    }
    let (kick_hz, bass_hz) = (median(kick_tones), median(bass_tones));
    let semitones = match (kick_hz, bass_hz) {
        (Some(k), Some(b)) if k > 0.0 && b > 0.0 => Some((12.0 * (k / b).log2()).abs()),
        _ => None,
    };
    let beating = semitones.is_some_and(|s| (SAME_SEMITONES..=BEAT_SEMITONES).contains(&s));
    let beat_hz = match (beating, kick_hz, bass_hz) {
        (true, Some(k), Some(b)) => Some((k - b).abs()),
        _ => None,
    };
    let overall_db = if hits.is_empty() {
        0.0
    } else {
        db(total_sum) - db(total_expected)
    };
    let worst_db = hits
        .iter()
        .map(|h| h.delta_db)
        .fold(f32::INFINITY, f32::min);
    Check {
        sample_rate,
        frames: frames as u64,
        hits,
        kick_hz,
        bass_hz,
        semitones,
        beating,
        beat_hz,
        overall_db,
        worst_db: if worst_db.is_finite() { worst_db } else { 0.0 },
    }
}

/// The delay in frames (0 to `max_frames`) of `bass` that gives the best [`Check::score`]
/// against `kick`, found by shifting the bass sub (the filter is linear and time invariant, so
/// delaying the bass and filtering it give the same sub). The smaller delay wins a tie, and
/// `None` when no delay beats no delay by at least `0.5` of score.
pub fn best_bass_delay(
    kick: &[f32],
    bass: &[f32],
    sample_rate: u32,
    max_frames: usize,
) -> Option<usize> {
    let frames = kick.len().min(bass.len());
    let kick = Kick::new(&kick[..frames], sample_rate);
    let bass_sub = sub_band(&bass[..frames], sample_rate);
    let base = kick.against(&bass_sub).score();
    let mut best = (0usize, base);
    let mut delayed = vec![0.0f32; frames];
    let mut tried = std::collections::BTreeSet::new();
    let mut try_delay = |d: usize, best: &mut (usize, f32)| {
        if d > max_frames || d >= frames || !tried.insert(d) {
            return;
        }
        delayed[..d].fill(0.0);
        delayed[d..].copy_from_slice(&bass_sub[..frames - d]);
        let s = kick.against(&delayed).score();
        if s > best.1 + 1.0e-4 {
            *best = (d, s);
        }
    };
    // A coarse walk, then a fine one around the best of it.
    let step = (max_frames / 48).max(1);
    let mut d = step;
    while d <= max_frames {
        try_delay(d, &mut best);
        d += step;
    }
    let centre = best.0;
    if centre > 0 {
        for d in centre.saturating_sub(step)..=(centre + step) {
            try_delay(d, &mut best);
        }
    }
    (best.1 >= base + 0.5 && best.0 > 0).then_some(best.0)
}

#[cfg(test)]
mod tests;
