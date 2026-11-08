//! Resonance finder: narrow peaks of the average spectrum of a signal that stand out of its
//! smoothed trend, ranked, each with an estimated frequency and Q.
//!
//! It runs offline on a worker, never on the audio thread, so it allocates freely. The maths is
//! the deterministic FFT and dB of [`crate::spectrum`], plus `f64` sums in a fixed order, so the
//! same samples give the same list on every platform.
//!
//! # Method
//!
//! 1. **Average spectrum.** The signal (left and right, the mean of their power spectra) is cut
//!    into windows of 8192 points (16384 from 88.2 kHz up, so the bin spacing stays near 6 Hz) with
//!    a hop of half a window, each through a Hann window and the calibrated power per bin of
//!    [`crate::spectrum::Analyser`]. Windows whose mean square is below -100 dB are skipped
//!    (silence says nothing about the sound), and the power of the others is averaged. A few
//!    seconds of sound are many windows, which keeps the scatter of a noisy bin well below 1 dB.
//! 2. **Trend.** The average in dB is smoothed over one third of an octave (the window grows with
//!    the frequency and never has fewer than 13 bins), twice: the second pass clips every bin to
//!    3 dB above the first trend before it averages, so a peak does not lift the curve it is
//!    measured against.
//! 3. **Peaks.** A bin is a candidate when it is a local maximum of the dB spectrum between
//!    [`Settings::low_hz`] and [`Settings::high_hz`] and stands at least
//!    [`Settings::min_prominence_db`] above the trend. Its frequency is refined with a parabola
//!    through the dB values of the bin and its two neighbours (good to a small part of a bin).
//! 4. **Width and Q.** The width is that of the peak 3 dB under its top (half the prominence for
//!    a peak that rises less than 6 dB), found by walking down both sides with linear
//!    interpolation; `Q = frequency / width`, clamped to `0.7..=18`. A pure tone is as wide as the
//!    main lobe of the window (1.4 bins), so its Q is the resolution limit, not its own.
//! 5. **Ranking.** Candidates are sorted by prominence (the louder frequency first on a tie). A
//!    candidate within a sixth of an octave of a better one is dropped, and at most
//!    [`Settings::max_found`] are kept.
//!
//! # Levels
//!
//! `level_db` is the level of the band `frequency / min(Q, 8)` wide around the peak (the widest
//! Q a dynamic band's detector uses) in the scale of that detector (a sine of peak 1.0 is 0 dB),
//! `trend_db` the level the trend has in the same band. `prominence_db` is measured on the single bin, so it is larger than `level_db -
//! trend_db` for a narrow peak.

use crate::spectrum::{Analyser, ChannelMode, WindowKind};
use crate::{exp2_det, log2_det};

/// The frequencies a peak must lie between by default, Hz.
pub const DEFAULT_LOW_HZ: f32 = 50.0;
/// See [`DEFAULT_LOW_HZ`].
pub const DEFAULT_HIGH_HZ: f32 = 16_000.0;
/// The smallest Q the finder reports.
pub const MIN_Q: f32 = 0.7;
/// The largest Q the finder reports.
pub const MAX_Q: f32 = 18.0;
/// The largest Q the band detector of the dynamic EQ listens with.
pub const DETECTOR_MAX_Q: f32 = 8.0;
/// Windows with a mean square below this are skipped.
const SILENT_MEAN_SQUARE: f32 = 1.0e-10;
/// The fewest bins half a trend window spans.
const MIN_HALF_WINDOW: usize = 6;
/// Half the width of a third of an octave, as a fraction of the frequency (`2^(1/6) - 1`).
const THIRD_OCTAVE_HALF: f64 = 0.122_462;
/// How far above the first trend a bin counts in the second.
const CLIP_DB: f32 = 3.0;
/// Candidates closer than this (octaves) to a better one are dropped.
const MERGE_OCTAVES: f32 = 1.0 / 6.0;
/// `10 * log10(2)`.
const DB_PER_OCTAVE_OF_POWER: f32 = 3.010_3;
/// `log2(10) / 10`.
const LOG2_10_10: f32 = 0.332_192_8;
/// A power below this is a floor, not a level.
const POWER_FLOOR: f32 = 1.0e-20;

/// How the finder looks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    /// A peak must stand this far above the trend, dB.
    pub min_prominence_db: f32,
    /// The lowest frequency of a peak, Hz.
    pub low_hz: f32,
    /// The highest frequency of a peak, Hz.
    pub high_hz: f32,
    /// The most peaks returned.
    pub max_found: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            min_prominence_db: 6.0,
            low_hz: DEFAULT_LOW_HZ,
            high_hz: DEFAULT_HIGH_HZ,
            max_found: 12,
        }
    }
}

/// One resonance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Resonance {
    /// Centre frequency, Hz.
    pub freq_hz: f32,
    /// Estimated Q (width at 3 dB under the top), within [`MIN_Q`]..=[`MAX_Q`].
    pub q: f32,
    /// Height of the peak bin over the trend, dB.
    pub prominence_db: f32,
    /// Level of the band around the peak, dB (detector scale, module docs).
    pub level_db: f32,
    /// Level of the trend in the same band, dB.
    pub trend_db: f32,
}

/// The transform size for a sample rate.
fn size_for(sample_rate: f32) -> usize {
    if sample_rate >= 88_200.0 {
        16_384
    } else {
        8_192
    }
}

/// `10 log10(p)` with a floor.
fn db_of(p: f32) -> f32 {
    log2_det(p.max(POWER_FLOOR)) * DB_PER_OCTAVE_OF_POWER
}

/// `10^(db / 10)`.
fn power_of(db: f32) -> f32 {
    exp2_det(db * LOG2_10_10)
}

/// The average power per bin of the sound in `left`/`right` (windows of `size` points, hop half a
/// window, silent windows skipped), or `None` when nothing sounds.
fn average_power(left: &[f32], right: &[f32], size: usize) -> Option<(Vec<f64>, f32)> {
    let frames = left.len().min(right.len());
    if frames == 0 {
        return None;
    }
    let mut analyser = Analyser::new(size, WindowKind::Hann);
    let enbw = analyser.enbw();
    let hop = size / 2;
    let windows = if frames <= size {
        1
    } else {
        (frames - size) / hop + 1
    };
    let mut sum = vec![0.0f64; analyser.bins()];
    let mut used = 0usize;
    for w in 0..windows {
        let from = w * hop;
        let to = (from + size).min(frames);
        let (l, r) = (&left[from..to], &right[from..to]);
        let energy: f32 = l
            .iter()
            .zip(r)
            .map(|(a, b)| {
                let (a, b) = (finite(*a), finite(*b));
                0.5 * (a * a + b * b)
            })
            .sum::<f32>()
            / (to - from) as f32;
        if energy < SILENT_MEAN_SQUARE {
            continue;
        }
        let power = analyser.analyse(l, r, ChannelMode::Sum);
        for (s, p) in sum.iter_mut().zip(power) {
            *s += f64::from(*p);
        }
        used += 1;
    }
    if used == 0 {
        return None;
    }
    for s in &mut sum {
        *s /= used as f64;
    }
    Some((sum, enbw))
}

fn finite(x: f32) -> f32 {
    if x.is_finite() { x } else { 0.0 }
}

/// Mean of `values` over `k - half(k) ..= k + half(k)` for every bin, with running sums.
fn smooth(values: &[f32]) -> Vec<f32> {
    let mut prefix = Vec::with_capacity(values.len() + 1);
    prefix.push(0.0f64);
    let mut acc = 0.0f64;
    for v in values {
        acc += f64::from(*v);
        prefix.push(acc);
    }
    let last = values.len() - 1;
    (0..values.len())
        .map(|k| {
            let half = ((k as f64 * THIRD_OCTAVE_HALF).round() as usize).max(MIN_HALF_WINDOW);
            let (lo, hi) = (k.saturating_sub(half), (k + half).min(last));
            ((prefix[hi + 1] - prefix[lo]) / (hi + 1 - lo) as f64) as f32
        })
        .collect()
}

/// The trend of an average spectrum in dB (module docs, step 2).
fn trend_of(db: &[f32]) -> Vec<f32> {
    let first = smooth(db);
    let clipped: Vec<f32> = db
        .iter()
        .zip(&first)
        .map(|(d, t)| d.min(t + CLIP_DB))
        .collect();
    smooth(&clipped)
}

/// The width in bins of the peak at bin `k` at `drop` dB under its top, by linear interpolation.
fn width_bins(db: &[f32], k: usize, drop: f32) -> f32 {
    let level = db[k] - drop;
    let mut left = k as f32;
    let mut i = k;
    while i > 0 {
        if db[i - 1] <= level {
            let (a, b) = (db[i - 1], db[i]);
            left = (i - 1) as f32 + (level - a) / (b - a).max(1e-6);
            break;
        }
        i -= 1;
        left = i as f32;
    }
    let mut right = k as f32;
    let mut i = k;
    while i + 1 < db.len() {
        if db[i + 1] <= level {
            let (a, b) = (db[i], db[i + 1]);
            right = i as f32 + (a - level) / (a - b).max(1e-6);
            break;
        }
        i += 1;
        right = i as f32;
    }
    right - left
}

/// The power of the bins between `lo_hz` and `hi_hz`, a bin that straddles an edge counting in
/// proportion (the sum of `power(k)` over the band).
fn band_sum(bins: usize, df: f64, lo_hz: f64, hi_hz: f64, power: impl Fn(usize) -> f64) -> f64 {
    let k0 = (lo_hz / df - 0.5).floor().max(0.0) as usize;
    let k1 = ((hi_hz / df + 0.5).ceil() as usize).min(bins - 1);
    let mut sum = 0.0;
    for k in k0..=k1 {
        let (b_lo, b_hi) = ((k as f64 - 0.5) * df, (k as f64 + 0.5) * df);
        let overlap = (hi_hz.min(b_hi) - lo_hz.max(b_lo)).max(0.0) / df;
        sum += overlap * power(k);
    }
    sum
}

/// Finds the resonances of the sound in `left` and `right` at `sample_rate` (module docs).
/// Silence, or a signal shorter than a few windows of nothing, gives an empty list.
pub fn find_resonances(
    left: &[f32],
    right: &[f32],
    sample_rate: f32,
    settings: &Settings,
) -> Vec<Resonance> {
    if !(sample_rate.is_finite() && sample_rate >= 8_000.0) || settings.max_found == 0 {
        return Vec::new();
    }
    let size = size_for(sample_rate);
    let Some((avg, enbw)) = average_power(left, right, size) else {
        return Vec::new();
    };
    let df = f64::from(sample_rate) / size as f64;
    let db: Vec<f32> = avg.iter().map(|p| db_of(*p as f32)).collect();
    let trend = trend_of(&db);
    let bins = db.len();
    let lo = ((f64::from(settings.low_hz) / df).ceil() as usize).max(2);
    let hi = ((f64::from(settings.high_hz) / df).floor() as usize).min(bins - 2);
    let mut found: Vec<Resonance> = Vec::new();
    for k in lo..=hi {
        let prominence = db[k] - trend[k];
        if prominence < settings.min_prominence_db || !(db[k] > db[k - 1] && db[k] >= db[k + 1]) {
            continue;
        }
        let (a, b, c) = (db[k - 1], db[k], db[k + 1]);
        let denom = a - 2.0 * b + c;
        let shift = if denom < -1e-6 {
            (0.5 * (a - c) / denom).clamp(-0.5, 0.5)
        } else {
            0.0
        };
        let freq = (k as f64 + f64::from(shift)) * df;
        let drop = 3.0f32.min(0.5 * prominence);
        let width = f64::from(width_bins(&db, k, drop)).max(1.0) * df;
        let q = ((freq / width) as f32).clamp(MIN_Q, MAX_Q);
        let half = 0.5 / f64::from(q.min(DETECTOR_MAX_Q));
        let (band_lo, band_hi) = (freq * (1.0 - half), freq * (1.0 + half));
        let peak_sum = band_sum(bins, df, band_lo, band_hi, |i| avg[i]);
        let trend_sum = band_sum(bins, df, band_lo, band_hi, |i| {
            f64::from(power_of(trend[i]))
        });
        found.push(Resonance {
            freq_hz: freq as f32,
            q,
            prominence_db: prominence,
            level_db: db_of((peak_sum / f64::from(enbw)) as f32),
            trend_db: db_of((trend_sum / f64::from(enbw)) as f32),
        });
    }
    found.sort_by(|a, b| {
        b.prominence_db
            .total_cmp(&a.prominence_db)
            .then(a.freq_hz.total_cmp(&b.freq_hz))
    });
    let mut kept: Vec<Resonance> = Vec::new();
    for r in found {
        let near = kept
            .iter()
            .any(|k| (log2_det(r.freq_hz) - log2_det(k.freq_hz)).abs() < MERGE_OCTAVES);
        if !near {
            kept.push(r);
            if kept.len() == settings.max_found {
                break;
            }
        }
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rng;

    const SR: f32 = 48_000.0;

    /// Noise with a tilt of about -3 dB per octave (a one-pole low-pass of white noise is close
    /// enough for a trend) plus sines `(hz, amp)`.
    fn signal(seconds: f32, seed: u64, tones: &[(f64, f32)]) -> (Vec<f32>, Vec<f32>) {
        let n = (seconds * SR) as usize;
        let mut rng = Rng::new(seed);
        let (mut l, mut r) = (Vec::with_capacity(n), Vec::with_capacity(n));
        let (mut sl, mut sr) = (0.0f32, 0.0f32);
        for i in 0..n {
            sl += 0.3 * (rng.next_bipolar() * 0.2 - sl);
            sr += 0.3 * (rng.next_bipolar() * 0.2 - sr);
            let t: f32 = tones
                .iter()
                .map(|&(hz, amp)| {
                    amp * (2.0 * std::f64::consts::PI * hz * i as f64 / f64::from(SR)).sin() as f32
                })
                .sum();
            l.push(sl + t);
            r.push(sr + t);
        }
        (l, r)
    }

    fn near(r: &Resonance, hz: f64) -> bool {
        let df = f64::from(SR) / 8192.0;
        (f64::from(r.freq_hz) - hz).abs() <= (0.02 * hz).max(df)
    }

    #[test]
    fn known_tones_are_found_within_two_percent_and_ranked_by_height() {
        let tones = [(310.0, 0.02), (1_250.0, 0.06), (4_100.0, 0.03)];
        let (l, r) = signal(6.0, 7, &tones);
        let found = find_resonances(&l, &r, SR, &Settings::default());
        assert_eq!(found.len(), 3, "{found:?}");
        for (hz, _) in tones {
            assert!(found.iter().any(|f| near(f, hz)), "{hz}: {found:?}");
        }
        assert!(
            near(&found[0], 1_250.0),
            "the strongest is first: {found:?}"
        );
        assert!(
            found
                .windows(2)
                .all(|w| w[0].prominence_db >= w[1].prominence_db),
            "{found:?}"
        );
        for f in &found {
            assert!(f.prominence_db >= 6.0 && (MIN_Q..=MAX_Q).contains(&f.q));
            assert!(f.level_db > f.trend_db, "{f:?}");
        }
    }

    #[test]
    fn a_signal_without_peaks_gives_nothing() {
        let (l, r) = signal(6.0, 3, &[]);
        assert!(find_resonances(&l, &r, SR, &Settings::default()).is_empty());
    }

    #[test]
    fn silence_and_empty_input_give_nothing() {
        let z = vec![0.0f32; 48_000];
        assert!(find_resonances(&z, &z, SR, &Settings::default()).is_empty());
        assert!(find_resonances(&[], &[], SR, &Settings::default()).is_empty());
        assert!(find_resonances(&z, &z, 0.0, &Settings::default()).is_empty());
    }

    #[test]
    fn a_wider_peak_gets_a_lower_q() {
        // A row of sines 3 Hz apart (closer than a bin) reads as one plateau about 200 Hz wide.
        let narrow = signal(6.0, 5, &[(2_000.0, 0.05)]);
        let row: Vec<(f64, f32)> = (0..70)
            .map(|i| (1_900.0 + 3.0 * f64::from(i), 0.01))
            .collect();
        let wide = signal(6.0, 5, &row);
        let q_of =
            |s: &(Vec<f32>, Vec<f32>)| find_resonances(&s.0, &s.1, SR, &Settings::default())[0].q;
        assert!(q_of(&wide) < q_of(&narrow));
    }

    #[test]
    fn the_result_is_the_same_every_time_and_respects_the_limits() {
        let tones = [
            (500.0, 0.05),
            (900.0, 0.05),
            (1_800.0, 0.05),
            (3_600.0, 0.05),
        ];
        let (l, r) = signal(6.0, 11, &tones);
        let a = find_resonances(&l, &r, SR, &Settings::default());
        assert_eq!(a, find_resonances(&l, &r, SR, &Settings::default()));
        let s = Settings {
            max_found: 2,
            ..Settings::default()
        };
        assert_eq!(find_resonances(&l, &r, SR, &s).len(), 2);
        let s = Settings {
            low_hz: 1_000.0,
            ..Settings::default()
        };
        assert!(
            find_resonances(&l, &r, SR, &s)
                .iter()
                .all(|f| f.freq_hz >= 1_000.0)
        );
    }

    #[test]
    fn non_finite_samples_count_as_silence() {
        let (mut l, r) = signal(2.0, 2, &[(1_000.0, 0.05)]);
        l[100] = f32::NAN;
        l[200] = f32::INFINITY;
        let _ = find_resonances(&l, &r, SR, &Settings::default());
    }

    #[test]
    fn a_high_rate_uses_the_larger_transform() {
        let n = 96_000 * 4;
        let mut rng = Rng::new(9);
        let x: Vec<f32> = (0..n)
            .map(|i| {
                0.05 * rng.next_bipolar()
                    + 0.05
                        * (2.0 * std::f64::consts::PI * 2_500.0 * i as f64 / 96_000.0).sin() as f32
            })
            .collect();
        let found = find_resonances(&x, &x, 96_000.0, &Settings::default());
        assert!(
            found.iter().any(|f| (f.freq_hz - 2_500.0).abs() < 50.0),
            "{found:?}"
        );
    }
}
