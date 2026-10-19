//! Offline onset (transient) detection for loop slicing.
//!
//! [`detect_onsets`] is pure and deterministic and runs off the real-time thread (it allocates).
//! Same samples, rate and sensitivity give the same frames on every platform: the decision path
//! uses only IEEE `+ - * /`, `sqrt`, integer arithmetic and sorting (no `ln`, `exp`, `pow`, no
//! FFT). The log-like compression is a rational function (see below).
//!
//! **Algorithm** (all lengths scale with the rate `sr`, written here for 48 kHz):
//! 1. *Mono sum:* the mean of the channels (non-finite samples count as 0); the shortest
//!    channel sets the length.
//! 2. *Energy:* window `w` = 1024 frames, hop `h` = 256 frames (`round(1024 * sr / 48000)` and
//!    `round(256 * sr / 48000)`). Frame `k` covers `k*h .. k*h + w` (zero padded past the end);
//!    `r_k` is its RMS.
//! 3. *Flux:* `f_k = max(0, (r_k - r_(k-1)) / (r_k + r_(k-1) + 5e-4))`, a rational stand-in for
//!    the half-wave rectified log ratio (it is about `tanh(ln(r_k / r_(k-1)) / 2)`), so the
//!    measure is level independent and levels near -66 dBFS and below do not count. `f_0 = 0`:
//!    sound that is already there at the start of the input is not an onset (the caller treats
//!    the start of the loop as an implicit slice boundary).
//! 4. *Adaptive threshold:* frame `k` is a candidate when `f_k` is a local flux peak
//!    (`f_k >= f_(k-1)` and `f_k > f_(k+1)`) and `f_k > thr_k`, with
//!    `thr_k = delta(s) + lambda(s) * mean(f over k-8..=k+8)`, `s` the sensitivity clamped to
//!    `0..=1` (NaN gives 0.5), `delta(s) = 0.02 + 0.28 * (1 - s)` and
//!    `lambda(s) = 1 + 3 * (1 - s)`.
//! 5. *Localisation* (independent of `s`): the new samples of frame `k` are the last hop of its
//!    window; the search region reaches back over up to three preceding frames whose flux is at
//!    least a quarter of the peak. Inside the region the first 0.5 ms block whose energy jump is at
//!    least half of the largest jump is taken, then moved back while the block energy keeps
//!    falling (at most 5 ms): the foot of the attack, the nearest preceding low-energy point.
//! 6. *Minimum gap:* candidates are sorted by position and thinned left to right so that kept
//!    onsets are at least 30 ms apart.
//!
//! **Monotonicity.** `delta` and `lambda` never increase with `s`, so `thr_k` never increases and
//! the candidate frames at a higher sensitivity are a superset of those at a lower one. Peak
//! picking and localisation do not depend on `s`, so the position list of a superset is a
//! superset. Thinning left to right keeps the most points any selection with that minimum gap
//! can keep (earliest-first is optimal for points on a line), and a superset never allows fewer.
//! So more sensitivity never gives fewer onsets (proptest in this module).

/// Added to the RMS sum in the flux denominator (about -66 dBFS).
const FLUX_EPS: f64 = 5.0e-4;
/// Half width in frames of the local mean of the threshold.
const MEAN_HALF: usize = 8;
/// Minimum distance between two onsets, in milliseconds.
const MIN_GAP_MS: u64 = 30;
/// Longest move back to the foot of an attack, in milliseconds.
const MAX_BACK_MS: u64 = 5;

fn threshold_terms(sensitivity: f32) -> (f64, f64) {
    let s = if sensitivity.is_nan() {
        0.5
    } else {
        f64::from(sensitivity.clamp(0.0, 1.0))
    };
    (0.02 + 0.28 * (1.0 - s), 1.0 + 3.0 * (1.0 - s))
}

/// Onset frames of the loop `channels` (planar) at `sample_rate`, sorted and unique, at least 30 ms
/// apart. `sensitivity` is `0..=1` (more gives more onsets, never fewer). Empty input, silence and
/// input shorter than two analysis frames give no onsets. See the module docs for the method.
pub fn detect_onsets(channels: &[&[f32]], sample_rate: u32, sensitivity: f32) -> Vec<u64> {
    let n = channels.iter().map(|c| c.len()).min().unwrap_or(0);
    if n == 0 || sample_rate == 0 {
        return Vec::new();
    }
    let sr = u64::from(sample_rate);
    let w = ((1024 * sr + 24_000) / 48_000).max(8) as usize;
    let h = ((256 * sr + 24_000) / 48_000).max(2) as usize;
    let b = (sr.div_ceil(2000)).max(1) as usize;
    let gap = ((MIN_GAP_MS * sr + 500) / 1000).max(1);
    let max_back = ((MAX_BACK_MS * sr / 1000) as usize / b).max(1);

    let inv = 1.0 / channels.len() as f64;
    let mono: Vec<f64> = (0..n)
        .map(|i| {
            let mut sum = 0.0f64;
            for c in channels {
                let x = c[i];
                if x.is_finite() {
                    sum += f64::from(x);
                }
            }
            sum * inv
        })
        .collect();

    let nf = n.div_ceil(h);
    if nf < 2 {
        return Vec::new();
    }
    let rms: Vec<f64> = (0..nf)
        .map(|k| {
            let lo = k * h;
            let hi = (lo + w).min(n);
            let mut acc = 0.0f64;
            for x in &mono[lo..hi] {
                acc += x * x;
            }
            (acc / w as f64).sqrt()
        })
        .collect();
    let mut flux = vec![0.0f64; nf];
    for k in 1..nf {
        let (a, p) = (rms[k], rms[k - 1]);
        flux[k] = ((a - p) / (a + p + FLUX_EPS)).max(0.0);
    }

    let nb = n.div_ceil(b);
    let energy: Vec<f64> = (0..nb)
        .map(|j| {
            let mut acc = 0.0f64;
            for x in &mono[j * b..((j + 1) * b).min(n)] {
                acc += x * x;
            }
            acc
        })
        .collect();

    let (delta, lambda) = threshold_terms(sensitivity);
    let mut found: Vec<u64> = Vec::new();
    for k in 1..nf {
        let f = flux[k];
        if f <= 0.0 || f < flux[k - 1] || (k + 1 < nf && f <= flux[k + 1]) {
            continue;
        }
        let lo = k.saturating_sub(MEAN_HALF);
        let hi = (k + MEAN_HALF).min(nf - 1);
        let mut sum = 0.0f64;
        for v in &flux[lo..=hi] {
            sum += v;
        }
        let thr = delta + lambda * (sum / (hi - lo + 1) as f64);
        if f > thr
            && let Some(p) = locate(k, &flux, &energy, (w, h, b), n, max_back)
        {
            found.push(p as u64);
        }
    }
    found.sort_unstable();
    found.dedup();
    let mut kept: Vec<u64> = Vec::with_capacity(found.len());
    for p in found {
        if kept.last().is_none_or(|&last| p - last >= gap) {
            kept.push(p);
        }
    }
    kept
}

/// Sample position of the onset behind flux peak `k`, or `None` when its region is empty.
fn locate(
    k: usize,
    flux: &[f64],
    energy: &[f64],
    (w, h, b): (usize, usize, usize),
    n: usize,
    max_back: usize,
) -> Option<usize> {
    let f = flux[k];
    let mut lo = k;
    let mut back = 0;
    while back < 3 && lo > 1 && flux[lo - 1] >= 0.25 * f {
        lo -= 1;
        back += 1;
    }
    let start = (lo - 1) * h + w;
    let end = (k * h + w).min(n);
    if start >= end {
        return None;
    }
    let first = start / b;
    let last = end.div_ceil(b).min(energy.len());
    let jump = |j: usize| energy[j] - if j > 0 { energy[j - 1] } else { 0.0 };
    let mut best = 0.0f64;
    for j in first..last {
        best = best.max(jump(j));
    }
    let mut j = first;
    if best > 0.0 {
        j = (first..last)
            .find(|&j| jump(j) >= 0.5 * best)
            .unwrap_or(first);
    }
    let mut steps = 0;
    while j > 0 && steps < max_back && energy[j - 1] < energy[j] {
        j -= 1;
        steps += 1;
    }
    Some(j * b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rng;
    use proptest::prelude::*;

    const SR: u32 = 48_000;

    fn noise(rng: &mut Rng) -> f32 {
        rng.next_f32() * 2.0 - 1.0
    }

    /// Adds a noise burst of `len` frames at `at`: amplitude `amp`, exponential decay when
    /// `tau` frames > 0.
    fn burst(x: &mut [f32], at: usize, len: usize, amp: f32, tau: f32, rng: &mut Rng) {
        for i in 0..len.min(x.len() - at) {
            let env = if tau > 0.0 {
                (-(i as f32) / tau).exp()
            } else {
                1.0
            };
            x[at + i] += amp * env * noise(rng);
        }
    }

    fn tone(x: &mut [f32], at: usize, len: usize, hz: f64, amp: f32, tau: f64, sr: u32) {
        for i in 0..len.min(x.len() - at) {
            let t = i as f64 / f64::from(sr);
            let env = (-t / tau).exp();
            x[at + i] += amp * (env * (std::f64::consts::TAU * hz * t).sin()) as f32;
        }
    }

    fn within(found: &[u64], want: &[usize], sr: u32) {
        let tol = u64::from(sr) * 5 / 1000;
        assert_eq!(found.len(), want.len(), "found {found:?}, want {want:?}");
        for (f, w) in found.iter().zip(want) {
            let w = *w as u64;
            assert!(f.abs_diff(w) <= tol, "found {f}, want {w} (tol {tol})");
        }
    }

    const HITS: [usize; 5] = [9_600, 38_400, 72_000, 105_600, 134_400];

    #[test]
    fn clicks_are_found_within_five_ms() {
        for sr in [44_100u32, 48_000, 96_000] {
            let scale = f64::from(sr) / 48_000.0;
            let at: Vec<usize> = HITS.iter().map(|&p| (p as f64 * scale) as usize).collect();
            let mut rng = Rng::new(1);
            let mut x = vec![0.0f32; (144_000.0 * scale) as usize];
            for &p in &at {
                burst(
                    &mut x,
                    p,
                    (96.0 * scale) as usize,
                    0.8,
                    30.0 * scale as f32,
                    &mut rng,
                );
            }
            within(&detect_onsets(&[&x], sr, 0.5), &at, sr);
        }
    }

    #[test]
    fn decaying_tones_are_found_within_five_ms() {
        let mut x = vec![0.0f32; 160_000];
        for (i, &p) in HITS.iter().enumerate() {
            tone(&mut x, p, 24_000, 110.0 * (i + 1) as f64, 0.7, 0.08, SR);
        }
        within(&detect_onsets(&[&x], SR, 0.5), &HITS, SR);
    }

    #[test]
    fn noise_bursts_over_a_noise_floor_are_found() {
        let mut rng = Rng::new(7);
        let mut x = vec![0.0f32; 160_000];
        for v in x.iter_mut() {
            *v = 1.0e-4 * noise(&mut rng);
        }
        for (i, &p) in HITS.iter().enumerate() {
            burst(&mut x, p, 3_840 + 480 * i, 0.4, 0.0, &mut rng);
        }
        within(&detect_onsets(&[&x], SR, 0.5), &HITS, SR);
    }

    #[test]
    fn stereo_input_is_summed_and_channels_may_differ_in_length() {
        let mut rng = Rng::new(3);
        let mut l = vec![0.0f32; 60_000];
        burst(&mut l, 20_000, 2_000, 0.6, 400.0, &mut rng);
        let r = l.clone();
        let longer = vec![0.0f32; 70_000];
        within(&detect_onsets(&[&l, &r], SR, 0.5), &[20_000], SR);
        within(&detect_onsets(&[&l, &longer], SR, 0.5), &[20_000], SR);
        // Out-of-phase channels cancel.
        let neg: Vec<f32> = l.iter().map(|v| -v).collect();
        assert!(detect_onsets(&[&l, &neg], SR, 0.5).is_empty());
    }

    #[test]
    fn a_sustained_tone_has_no_onsets() {
        for hz in [100.0f64, 440.0, 1_000.0] {
            let mut x = vec![0.0f32; 96_000];
            for (i, v) in x.iter_mut().enumerate() {
                *v = 0.5 * (std::f64::consts::TAU * hz * i as f64 / 48_000.0).sin() as f32;
            }
            for s in [0.0f32, 0.5, 0.8] {
                assert_eq!(
                    detect_onsets(&[&x], SR, s),
                    Vec::<u64>::new(),
                    "{hz} Hz s {s}"
                );
            }
        }
    }

    #[test]
    fn silence_empty_and_tiny_inputs_give_no_onsets() {
        assert!(detect_onsets(&[&vec![0.0f32; 50_000]], SR, 1.0).is_empty());
        assert!(detect_onsets(&[], SR, 0.5).is_empty());
        assert!(detect_onsets(&[&[]], SR, 0.5).is_empty());
        assert!(detect_onsets(&[&[1.0f32]], SR, 1.0).is_empty());
        assert!(detect_onsets(&[&[1.0f32; 255]], SR, 1.0).is_empty());
        assert!(detect_onsets(&[&[1.0f32; 300]], 0, 1.0).is_empty());
        let bad = [f32::NAN, f32::INFINITY, 0.5];
        let _ = detect_onsets(&[&bad.repeat(2000)], SR, f32::NAN);
    }

    #[test]
    fn results_are_sorted_unique_and_repeatable() {
        let mut rng = Rng::new(11);
        let mut x = vec![0.0f32; 200_000];
        for p in (5_000..190_000).step_by(7_919) {
            burst(&mut x, p, 700, 0.5, 150.0, &mut rng);
        }
        let a = detect_onsets(&[&x], SR, 0.7);
        let b = detect_onsets(&[&x], SR, 0.7);
        assert_eq!(a, b);
        assert!(a.windows(2).all(|w| w[1] - w[0] >= 1_440), "{a:?}");
        assert!(a.iter().all(|&p| (p as usize) < x.len()));
    }

    #[test]
    fn sensitivity_changes_the_number_of_markers() {
        // Loud hits with quieter ghost notes between them.
        let mut rng = Rng::new(5);
        let mut x = vec![0.0f32; 192_000];
        for v in x.iter_mut() {
            *v = 0.02 * noise(&mut rng);
        }
        for p in (4_800..180_000).step_by(48_000) {
            burst(&mut x, p, 4_000, 0.8, 600.0, &mut rng);
            burst(&mut x, p + 24_000, 3_000, 0.025, 0.0, &mut rng);
        }
        let low = detect_onsets(&[&x], SR, 0.0).len();
        let high = detect_onsets(&[&x], SR, 1.0).len();
        assert!(low < high, "{low} vs {high}");
        assert_eq!(low, 4, "only the loud hits at the lowest sensitivity");
    }

    fn random_signal(seed: u64, hits: usize) -> Vec<f32> {
        let mut rng = Rng::new(seed);
        let mut x = vec![0.0f32; 120_000];
        for v in x.iter_mut() {
            *v = 0.01 * noise(&mut rng);
        }
        for _ in 0..hits {
            let at = (rng.next_u64() % 110_000) as usize;
            let len = 200 + (rng.next_u64() % 4_000) as usize;
            let amp = 0.02 + 0.8 * rng.next_f32();
            let tau = if rng.next_u64() & 1 == 0 {
                0.0
            } else {
                100.0 + 800.0 * rng.next_f32()
            };
            burst(&mut x, at, len, amp, tau, &mut rng);
        }
        x
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(24))]
        #[test]
        fn more_sensitivity_never_gives_fewer_onsets(
            seed in 0u64..1_000_000,
            hits in 0usize..14,
            a in 0.0f32..=1.0,
            b in 0.0f32..=1.0,
        ) {
            let x = random_signal(seed, hits);
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            let few = detect_onsets(&[&x], SR, lo);
            let many = detect_onsets(&[&x], SR, hi);
            prop_assert!(few.len() <= many.len(), "{} at {lo} vs {} at {hi}", few.len(), many.len());
            prop_assert_eq!(detect_onsets(&[&x], SR, lo), few);
            // And over the whole range of a ladder.
            let mut prev = 0;
            for i in 0..=10 {
                let n = detect_onsets(&[&x], SR, i as f32 / 10.0).len();
                prop_assert!(n >= prev);
                prev = n;
            }
        }
    }
}
