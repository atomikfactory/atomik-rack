//! Learn a duck curve from a kick: the average envelope of the hits in a signal, simplified to a
//! few points with bends ([`crate::duck_curve`]).
//!
//! It runs offline on a worker, never on the audio thread, so it allocates freely. Everything is
//! plain sums, comparisons and the bend shape (no library maths call), so the same samples give the same curve on every platform.
//!
//! # Method
//!
//! 1. **Envelope.** The louder channel's absolute value, held for 24 ms (more than a cycle of a low
//!    kick, so the envelope does not ripple with the note) and read once per millisecond.
//! 2. **Hits.** The loudest value is the reference. A hit starts where the envelope first rises
//!    through 10 percent of it (about -20 dB), counts when it reaches 40 percent of the reference
//!    within 40 ms, and the next hit may start only after the envelope has fallen under 2.5
//!    percent and 80 ms have passed since the last start. A signal too quiet to measure (peak
//!    under -80 dBFS) or without a hit reads as an error.
//! 3. **Average.** Each hit's envelope from its start, for the length asked for, is divided by
//!    the hit's own peak (so a soft ghost note counts as much as a hard one) and cut where the
//!    next hit starts; the hits are averaged millisecond by millisecond over those that reach
//!    it. That average is the dip the bass should have: 1 where the kick is at its loudest.
//! 4. **Points.** The average is simplified to at most [`MAX_POINTS`] points: the two ends are
//!    kept, then the sample farthest from the curve so far becomes a point, every segment with
//!    the bend (in steps of eight) that fits it best, until the largest error is under 2 percent
//!    or the points are used up. The last point is set to 0, so the curve ends released.

use crate::duck_curve::{MAX_POINTS, Point};
use atomik_core::bend::{BEND_MAX, bend_shape};

/// Peak under which a signal is too quiet to learn from (about -80 dBFS).
const MIN_PEAK: f32 = 1.0e-4;
/// How long the envelope holds the loudest sample, ms: more than a cycle of a low kick, so the
/// envelope does not ripple with the note.
const HOLD_MS: usize = 24;
/// A hit starts when the envelope rises through this share of the reference.
const START_SHARE: f32 = 0.10;
/// A hit counts when it reaches this share of the reference.
const COUNT_SHARE: f32 = 0.40;
/// ...within this many ms of its start.
const REACH_MS: usize = 40;
/// The envelope must fall under this share before the next hit may start.
const REARM_SHARE: f32 = 0.025;
/// The least distance between two starts, ms.
const MIN_GAP_MS: usize = 80;
/// The shortest and longest window that can be learned, ms.
pub const MIN_WINDOW_MS: f32 = 20.0;
/// See [`MIN_WINDOW_MS`].
pub const MAX_WINDOW_MS: f32 = 4_000.0;
/// The largest error at which the simplification stops.
const GOOD_ENOUGH: f64 = 0.02;
/// The bend steps tried for a segment.
const BEND_STEP: i32 = 8;

/// A learned curve.
#[derive(Debug, Clone, PartialEq)]
pub struct Learned {
    /// The points, `x` over the window asked for.
    pub points: Vec<Point>,
    /// How many hits were averaged.
    pub hits: usize,
    /// The largest distance between the average and the points' curve (0 to 1).
    pub error: f32,
}

/// Why nothing was learned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LearnError {
    /// The signal is silent, or too quiet to tell hits from the noise.
    TooQuiet,
    /// No hit stands out of the rest of the signal.
    NoHits,
}

impl std::fmt::Display for LearnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooQuiet => f.write_str("the sound is too quiet to learn from"),
            Self::NoHits => f.write_str("no hit stands out in the sound"),
        }
    }
}

impl std::error::Error for LearnError {}

/// The envelope read once per millisecond: the loudest sample of the last [`HOLD_MS`].
fn envelope(left: &[f32], right: &[f32], rate: f32) -> Vec<f32> {
    let hop = (f64::from(rate) / 1000.0).round().max(1.0) as usize;
    let n = left.len().max(right.len());
    let at = |c: &[f32], i: usize| c.get(i).copied().filter(|v| v.is_finite()).unwrap_or(0.0);
    let mut peaks = Vec::with_capacity(n / hop + 1);
    let mut peak = 0.0f32;
    for i in 0..n {
        peak = peak.max(at(left, i).abs().max(at(right, i).abs()));
        if (i + 1) % hop == 0 {
            peaks.push(peak);
            peak = 0.0;
        }
    }
    (0..peaks.len())
        .map(|j| {
            peaks[j.saturating_sub(HOLD_MS - 1)..=j]
                .iter()
                .fold(0.0f32, |a, b| a.max(*b))
        })
        .collect()
}

/// The start of every hit in the 1 ms envelope.
fn hit_starts(env: &[f32], reference: f32) -> Vec<usize> {
    let (start, count, rearm) = (
        reference * START_SHARE,
        reference * COUNT_SHARE,
        reference * REARM_SHARE,
    );
    let mut starts: Vec<usize> = Vec::new();
    let mut armed = true;
    let mut last: Option<usize> = None;
    for (j, v) in env.iter().enumerate() {
        if !armed {
            if *v < rearm && last.is_none_or(|l| j >= l + MIN_GAP_MS) {
                armed = true;
            }
            continue;
        }
        if *v >= start {
            armed = false;
            last = Some(j);
            let reach = env[j..(j + REACH_MS).min(env.len())]
                .iter()
                .fold(0.0f32, |a, b| a.max(*b));
            if reach >= count {
                starts.push(j);
            }
        }
    }
    starts
}

/// The average envelope of the hits, each divided by its own peak, `window` ms long.
fn average(env: &[f32], starts: &[usize], window: usize) -> Vec<f64> {
    let mut sum = vec![0.0f64; window];
    let mut n = vec![0u32; window];
    for (h, &s) in starts.iter().enumerate() {
        let end = starts.get(h + 1).copied().unwrap_or(env.len());
        let peak = env[s..(s + REACH_MS).min(env.len())]
            .iter()
            .fold(0.0f32, |a, b| a.max(*b));
        if peak <= 0.0 {
            continue;
        }
        for t in 0..window {
            let j = s + t;
            if j >= end || j >= env.len() {
                break;
            }
            sum[t] += f64::from((env[j] / peak).min(1.0));
            n[t] += 1;
        }
    }
    sum.iter()
        .zip(&n)
        .map(|(s, n)| if *n > 0 { s / f64::from(*n) } else { 0.0 })
        .collect()
}

/// The error of a segment from sample `a` to `b` with `bend`: `(sum of squares, largest, where)`.
fn segment_error(avg: &[f64], a: usize, b: usize, bend: i8) -> (f64, f64, usize) {
    let (ya, yb) = (avg[a], avg[b]);
    let span = (b - a) as f64;
    let (mut sum, mut worst, mut at) = (0.0, 0.0, a);
    for (i, v) in avg.iter().enumerate().take(b + 1).skip(a) {
        let u = (i - a) as f64 / span;
        let y = ya + (yb - ya) * bend_shape(u, bend);
        let e = (v - y).abs();
        sum += e * e;
        if e > worst {
            worst = e;
            at = i;
        }
    }
    (sum, worst, at)
}

/// The bend that fits the segment best, with its largest error and where that is.
fn best_bend(avg: &[f64], a: usize, b: usize) -> (i8, f64, usize) {
    let mut best = (0i8, f64::INFINITY, 0.0, a);
    let step = BEND_STEP;
    let mut bend = -i32::from(BEND_MAX);
    while bend <= i32::from(BEND_MAX) {
        let b8 = bend as i8;
        let (sum, worst, at) = segment_error(avg, a, b, b8);
        if sum < best.1 {
            best = (b8, sum, worst, at);
        }
        bend += step;
    }
    (best.0, best.2, best.3)
}

/// Simplifies `avg` (one value per ms, the last forced to 0) to at most [`MAX_POINTS`] points.
fn simplify(avg: &mut [f64]) -> (Vec<Point>, f64) {
    let last = avg.len() - 1;
    avg[last] = 0.0;
    let mut keys: Vec<usize> = vec![0, last];
    loop {
        // The worst segment.
        let mut worst: Option<(f64, usize)> = None;
        for w in keys.windows(2) {
            if w[1] - w[0] < 2 {
                continue;
            }
            let (_, err, at) = best_bend(avg, w[0], w[1]);
            if worst.is_none_or(|(e, _)| err > e) {
                worst = Some((err, at));
            }
        }
        match worst {
            Some((err, at)) if err > GOOD_ENOUGH && keys.len() < MAX_POINTS => {
                keys.push(at);
                keys.sort_unstable();
            }
            _ => break,
        }
    }
    let mut points = Vec::with_capacity(keys.len());
    let mut error = 0.0f64;
    for (n, &k) in keys.iter().enumerate() {
        let bend = match keys.get(n + 1) {
            Some(&next) if next - k >= 2 => {
                let (b, err, _) = best_bend(avg, k, next);
                error = error.max(err);
                b
            }
            _ => 0,
        };
        #[allow(clippy::cast_precision_loss)] // a few thousand samples
        let x = k as f64 / last as f64;
        points.push(Point::new(x as f32, avg[k] as f32, bend));
    }
    (points, error)
}

/// Learns a curve of `window_ms` (clamped to [`MIN_WINDOW_MS`]..=[`MAX_WINDOW_MS`]) from the
/// hits in `left` and `right`, sampled at `rate`.
///
/// # Errors
/// The signal is too quiet, or has no hit.
pub fn learn(
    left: &[f32],
    right: &[f32],
    rate: f32,
    window_ms: f32,
) -> Result<Learned, LearnError> {
    let env = envelope(left, right, rate);
    let reference = env.iter().fold(0.0f32, |a, b| a.max(*b));
    if reference < MIN_PEAK {
        return Err(LearnError::TooQuiet);
    }
    let starts = hit_starts(&env, reference);
    if starts.is_empty() {
        return Err(LearnError::NoHits);
    }
    let window = window_ms.clamp(MIN_WINDOW_MS, MAX_WINDOW_MS).round() as usize;
    let mut avg = average(&env, &starts, window);
    // A three-point smoothing takes the ripple of a low note out of the average.
    let raw = avg.clone();
    for i in 1..avg.len() - 1 {
        avg[i] = (raw[i - 1] + 2.0 * raw[i] + raw[i + 1]) * 0.25;
    }
    let (points, error) = simplify(&mut avg);
    Ok(Learned {
        points,
        hits: starts.len(),
        error: error as f32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::duck_curve::value_at;

    const RATE: f32 = 48_000.0;

    /// A kick: a 55 Hz sine under an exponential decay, `len_ms` long, at `start`.
    fn kick(out: &mut [f32], start: usize, decay_ms: f32, amp: f32) {
        let len = (decay_ms * 6.0 * 0.001 * RATE) as usize;
        for n in 0..len.min(out.len().saturating_sub(start)) {
            let t = n as f32 / RATE;
            let env = (-t * 1000.0 / decay_ms).exp();
            out[start + n] += amp * env * (std::f32::consts::TAU * 55.0 * t).sin();
        }
    }

    fn four_on_the_floor(seconds: f32, decay_ms: f32) -> Vec<f32> {
        let mut x = vec![0.0f32; (seconds * RATE) as usize];
        for b in 0..(seconds * 2.0) as usize {
            kick(&mut x, b * (RATE as usize / 2), decay_ms, 0.9);
        }
        x
    }

    #[test]
    fn a_steady_kick_gives_a_curve_that_rises_fast_and_falls_to_nothing() {
        let x = four_on_the_floor(4.0, 60.0);
        let got = learn(&x, &x, RATE, 400.0).unwrap();
        assert!(got.hits >= 7, "{}", got.hits);
        let p = &got.points;
        assert!(p.len() >= 3 && p.len() <= MAX_POINTS, "{p:?}");
        assert_eq!(p[0].x, 0.0);
        assert_eq!(p[p.len() - 1].x, 1.0);
        assert_eq!(p[p.len() - 1].y, 0.0);
        // The top comes early (within 15 percent of 400 ms) and near 1.
        let top = p.iter().fold(
            Point::new(0.0, 0.0, 0),
            |a, b| if b.y > a.y { *b } else { a },
        );
        assert!(top.y > 0.85 && top.x < 0.15, "{top:?}");
        // A decay of 60 ms: halfway through the window (200 ms) it is nearly gone (the bend
        // of eight points does not quite reach an exponential tail).
        assert!(value_at(p, 0.5) < 0.2, "{p:?}");
        // And at 60 ms (x = 0.15) it is still ducking.
        assert!(value_at(p, 0.15) > 0.15, "{}", value_at(p, 0.15));
        assert!(got.error < 0.2, "{}", got.error);
    }

    #[test]
    fn a_longer_kick_gives_a_longer_curve() {
        let short = learn(&four_on_the_floor(4.0, 40.0), &[], RATE, 400.0).unwrap();
        let long = learn(&four_on_the_floor(4.0, 120.0), &[], RATE, 400.0).unwrap();
        assert!(value_at(&long.points, 0.4) > value_at(&short.points, 0.4) + 0.1);
    }

    #[test]
    fn a_soft_ghost_note_counts_as_much_as_a_hard_hit() {
        let mut x = vec![0.0f32; (4.0 * RATE) as usize];
        for b in 0..8 {
            let amp = if b % 2 == 0 { 0.9 } else { 0.45 };
            kick(&mut x, b * (RATE as usize / 2), 60.0, amp);
        }
        let mixed = learn(&x, &x, RATE, 400.0).unwrap();
        let steady = learn(&four_on_the_floor(4.0, 60.0), &[], RATE, 400.0).unwrap();
        assert!((value_at(&mixed.points, 0.1) - value_at(&steady.points, 0.1)).abs() < 0.15);
    }

    #[test]
    fn silence_and_a_hum_teach_nothing() {
        assert_eq!(
            learn(&[0.0; 48_000], &[0.0; 48_000], RATE, 400.0),
            Err(LearnError::TooQuiet)
        );
        assert_eq!(learn(&[], &[], RATE, 400.0), Err(LearnError::TooQuiet));
        let hum: Vec<f32> = (0..96_000)
            .map(|n| 0.5 * (std::f32::consts::TAU * 50.0 * n as f32 / RATE).sin())
            .collect();
        // A steady tone starts once and never falls: one hit at most, and the curve stays up.
        match learn(&hum, &hum, RATE, 400.0) {
            Ok(l) => assert!(value_at(&l.points, 0.5) > 0.5),
            Err(e) => assert_eq!(e, LearnError::NoHits),
        }
    }

    #[test]
    fn the_same_samples_give_the_same_curve() {
        let x = four_on_the_floor(3.0, 70.0);
        let a = learn(&x, &x, RATE, 500.0).unwrap();
        let b = learn(&x, &x, RATE, 500.0).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn non_finite_samples_count_as_silence() {
        let mut x = four_on_the_floor(2.0, 60.0);
        x[1000] = f32::NAN;
        x[2000] = f32::INFINITY;
        assert!(learn(&x, &x, RATE, 300.0).is_ok());
    }
}
