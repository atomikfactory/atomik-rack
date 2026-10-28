//! Tests of the granular blocks: window shapes and sums, the source read and its anti-alias
//! filter, the clock's timing, the pool's pitch, direction, cap and stealing, and the built-in
//! source.

use super::*;
use crate::rng::Rng;
use crate::test_util::{dominant_hz, rms, sine};
use proptest::prelude::*;

const SR: f64 = 48_000.0;

fn bank() -> WindowBank {
    WindowBank::new()
}

fn pool_with(p: GrainParams) -> GrainPool {
    let mut pool = GrainPool::new();
    pool.spawn(p);
    pool
}

fn params(len: u32) -> GrainParams {
    GrainParams {
        pos: 0.0,
        step: 1.0,
        len,
        window: Window::Hann,
        gain: [1.0, 1.0],
    }
}

/// The frequency of `y` from the interpolated rising zero crossings (exact for an amplitude
/// modulated sine, whose zeros are the sine's).
fn zero_crossing_hz(y: &[f32]) -> f64 {
    let mut first = None;
    let mut last = 0.0;
    let mut count = 0usize;
    for (i, w) in y.windows(2).enumerate() {
        if w[0] < 0.0 && w[1] >= 0.0 {
            let t = i as f64 + f64::from(-w[0]) / f64::from(w[1] - w[0]);
            first.get_or_insert(t);
            last = t;
            count += 1;
        }
    }
    let first = first.expect("no zero crossing");
    (count - 1) as f64 * SR / (last - first)
}

// ------------------------------------------------------------------------------------ windows

#[test]
fn window_value_is_zero_at_both_ends_and_never_above_one() {
    for w in [Window::Hann, Window::Tukey, Window::Percussive] {
        assert!(window_value(w, 0.0).abs() < 1e-12, "{w:?} start");
        assert!(window_value(w, 1.0).abs() < 1e-12, "{w:?} end");
        let peak = (0..=1000)
            .map(|i| window_value(w, f64::from(i) / 1000.0))
            .fold(0.0f64, f64::max);
        assert!(peak <= 1.0 + 1e-12, "{w:?} peak {peak}");
        assert!(peak > 0.99, "{w:?} peak {peak}");
    }
}

#[test]
fn hann_is_symmetric_and_peaks_in_the_middle() {
    for i in 0..=100 {
        let p = f64::from(i) / 100.0;
        assert!(
            (window_value(Window::Hann, p) - window_value(Window::Hann, 1.0 - p)).abs() < 1e-12
        );
    }
    assert!((window_value(Window::Hann, 0.5) - 1.0).abs() < 1e-12);
    // sin^2(pi p), checked at one point off the symmetry axis.
    let want = (std::f64::consts::PI * 0.1).sin().powi(2);
    assert!((window_value(Window::Hann, 0.1) - want).abs() < 1e-9);
}

#[test]
fn hann_grains_at_half_length_spacing_sum_to_one() {
    for i in 0..=50 {
        let p = 0.5 + f64::from(i) / 100.0;
        let sum = window_value(Window::Hann, p) + window_value(Window::Hann, p - 0.5);
        assert!((sum - 1.0).abs() < 1e-9, "p {p}: {sum}");
    }
}

#[test]
fn tukey_is_flat_in_the_middle_and_sums_to_one_at_three_quarter_spacing() {
    for i in 25..=75 {
        assert!((window_value(Window::Tukey, f64::from(i) / 100.0) - 1.0).abs() < 1e-12);
    }
    for i in 0..=25 {
        let p = 0.75 + f64::from(i) / 100.0;
        let sum = window_value(Window::Tukey, p) + window_value(Window::Tukey, p - 0.75);
        assert!((sum - 1.0).abs() < 1e-9, "p {p}: {sum}");
    }
}

#[test]
fn percussive_attacks_fast_then_decays_to_silence() {
    let w = |p: f64| window_value(Window::Percussive, p);
    assert!(
        (w(0.03) - 1.0).abs() < 1e-9,
        "peak at the end of the attack"
    );
    assert!(w(0.015) < 0.6 && w(0.015) > 0.3);
    let mut prev = w(0.03);
    for i in 4..=96 {
        let v = w(f64::from(i) / 100.0);
        assert!(v < prev, "falls at {i} %");
        prev = v;
    }
    // About 52 dB of decay over the grain.
    assert!(w(0.96) < 0.01);
}

#[test]
fn window_value_clamps_its_phase_and_survives_nan() {
    for w in [Window::Hann, Window::Tukey, Window::Percussive] {
        assert!(window_value(w, -3.0).abs() < 1e-12);
        assert!(window_value(w, 7.0).abs() < 1e-12);
        assert_eq!(window_value(w, f64::NAN), 0.0);
    }
}

#[test]
fn window_from_index_clamps_and_round_trips() {
    for (i, w) in [Window::Hann, Window::Tukey, Window::Percussive]
        .into_iter()
        .enumerate()
    {
        assert_eq!(Window::from_index(i), w);
        assert_eq!(w.index(), i);
    }
    assert_eq!(Window::from_index(99), Window::Percussive);
}

#[test]
fn bank_gain_follows_the_analytic_windows() {
    let b = bank();
    for w in [Window::Hann, Window::Tukey, Window::Percussive] {
        for i in 0..=2000 {
            let p = f64::from(i) / 2000.0;
            let err = (f64::from(b.gain(w, p as f32)) - window_value(w, p)).abs();
            assert!(err < 4e-3, "{w:?} at {p}: {err}");
        }
    }
}

#[test]
fn bank_gain_clamps_its_phase_and_survives_nan() {
    let b = bank();
    for w in [Window::Hann, Window::Tukey, Window::Percussive] {
        assert_eq!(b.gain(w, -1.0), 0.0);
        assert_eq!(b.gain(w, 2.0), 0.0);
        assert_eq!(b.gain(w, f32::NAN), 0.0);
    }
}

#[test]
fn bank_mean_square_of_hann_is_three_eighths() {
    assert!((bank().mean_square(Window::Hann) - 0.375).abs() < 2e-3);
    assert!(bank().mean_square(Window::Tukey) > bank().mean_square(Window::Hann));
    assert!(bank().mean_square(Window::Percussive) < bank().mean_square(Window::Hann));
}

// -------------------------------------------------------------------------------- the source

#[test]
fn source_new_limits_the_window_to_the_audio() {
    let x = vec![0.0f32; 100];
    assert_eq!(GrainSource::new(&x, &x, 10, 1000).len(), 90);
    assert_eq!(GrainSource::new(&x, &x[..50], 0, 100).len(), 50);
    assert!(GrainSource::new(&x, &x, 60, 60).is_empty());
    assert!(GrainSource::new(&x, &x, 70, 20).is_empty());
    assert!(GrainSource::new(&x, &x, 500, 600).is_empty());
    assert!(GrainSource::mono(&[], 0, 10).is_empty());
}

#[test]
fn empty_source_reads_silence() {
    let x = [1.0f32; 8];
    let s = GrainSource::new(&x, &x, 4, 4);
    assert_eq!(s.read(3.5, 1.0), (0.0, 0.0));
    assert_eq!(s.read(3.5, 4.0), (0.0, 0.0));
    assert_eq!(s.read_linear(1.0), (0.0, 0.0));
}

#[test]
fn hermite_read_is_exact_at_whole_frames_and_inside_the_window() {
    let x: Vec<f32> = (0..64).map(|i| (i * i) as f32 / 64.0).collect();
    let s = GrainSource::mono(&x, 8, 40);
    for j in 0..32 {
        assert_eq!(s.read_hermite(f64::from(j)).0, x[8 + j as usize]);
    }
    // A straight line is reproduced between frames.
    let line: Vec<f32> = (0..64).map(|i| i as f32).collect();
    let s = GrainSource::mono(&line, 0, 64);
    assert!((s.read_hermite(10.25).0 - 10.25).abs() < 1e-5);
}

#[test]
fn reads_wrap_round_the_window() {
    let x: Vec<f32> = (0..16).map(|i| i as f32).collect();
    let s = GrainSource::mono(&x, 4, 12); // frames 4..12 -> window 0..8
    assert_eq!(s.read_hermite(8.0).0, 4.0);
    assert_eq!(s.read_hermite(-1.0).0, 11.0);
    assert_eq!(s.read_hermite(19.0).0, 7.0);
    assert_eq!(s.read_linear(7.5).0, 0.5 * (11.0 + 4.0));
}

#[test]
fn stereo_source_keeps_its_channels_apart_and_mono_duplicates() {
    let l: Vec<f32> = (0..32).map(|i| i as f32).collect();
    let r: Vec<f32> = (0..32).map(|i| -(i as f32)).collect();
    let s = GrainSource::new(&l, &r, 0, 32);
    assert_eq!(s.read_hermite(10.0), (10.0, -10.0));
    assert_eq!(s.read_linear(10.0), (10.0, -10.0));
    let m = GrainSource::mono(&l, 0, 32);
    assert_eq!(m.read_hermite(10.0), (10.0, 10.0));
    assert_eq!(m.read(10.0, 3.0).0, m.read(10.0, 3.0).1);
}

#[test]
fn reads_survive_a_non_finite_position() {
    let x = [0.5f32; 16];
    let s = GrainSource::mono(&x, 0, 16);
    for pos in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 1.0e30] {
        assert!(s.read(pos, 1.0).0.is_finite());
        assert!(s.read(pos, 5.0).0.is_finite());
    }
}

// ------------------------------------------------------------------------- aliasing

/// RMS of the source `x` (mono) read at `step` frames per sample with `read` and, as the
/// reference, with the plain Hermite read.
fn read_rms(x: &[f32], step: f64, filtered: bool) -> f32 {
    let s = GrainSource::mono(x, 0, x.len() as u64);
    let n = ((x.len() as f64 - 40.0) / step) as usize;
    let y: Vec<f32> = (0..n)
        .map(|i| {
            let pos = 20.0 + i as f64 * step;
            if filtered {
                s.read(pos, step).0
            } else {
                s.read_hermite(pos).0
            }
        })
        .collect();
    rms(&y[20..y.len() - 20])
}

#[test]
fn read_attenuates_content_that_would_alias_at_four_times_the_speed() {
    // 9 kHz at step 4 is 36 kHz: it folds to 12 kHz when read plainly.
    let x = sine(9_000.0, SR, 48_000, 1.0);
    let naive = read_rms(&x, 4.0, false);
    let filtered = read_rms(&x, 4.0, true);
    assert!(naive > 0.6, "the plain read keeps it: {naive}");
    assert!(
        filtered < 0.2 * naive,
        "filtered {filtered} against plain {naive}"
    );
}

#[test]
fn read_attenuates_content_that_would_alias_at_eight_times_the_speed() {
    let x = sine(4_800.0, SR, 48_000, 1.0);
    let naive = read_rms(&x, 8.0, false);
    let filtered = read_rms(&x, 8.0, true);
    assert!(filtered < 0.25 * naive, "{filtered} against {naive}");
}

#[test]
fn read_keeps_the_content_that_survives_the_speed_up() {
    let x = sine(1_000.0, SR, 48_000, 1.0);
    for step in [2.0, 4.0, 8.0] {
        let naive = read_rms(&x, step, false);
        let filtered = read_rms(&x, step, true);
        let db = 20.0 * (filtered / naive).log10();
        assert!(db > -1.5 && db < 0.3, "step {step}: {db} dB");
    }
}

#[test]
fn read_at_or_below_unit_step_is_the_hermite_read() {
    let x = sine(7_000.0, SR, 4_096, 1.0);
    let s = GrainSource::mono(&x, 0, 4_096);
    for step in [0.1, 0.5, 1.0] {
        for i in 0..200 {
            let pos = 30.0 + f64::from(i) * 0.37;
            assert_eq!(s.read(pos, step), s.read_hermite(pos));
        }
    }
}

#[test]
fn read_is_continuous_across_a_unit_step() {
    let x = sine(3_000.0, SR, 4_096, 1.0);
    let s = GrainSource::mono(&x, 0, 4_096);
    let mut worst = 0.0f32;
    for i in 0..400 {
        let pos = 100.0 + f64::from(i) * 0.61;
        let a = s.read(pos, 1.0).0;
        let b = s.read(pos, 1.0 + 1e-6).0;
        worst = worst.max((a - b).abs());
    }
    assert!(worst < 1e-4, "jump {worst}");
}

#[test]
fn read_limits_the_step_to_the_most() {
    let x = sine(500.0, SR, 4_096, 1.0);
    let s = GrainSource::mono(&x, 0, 4_096);
    assert_eq!(s.read(200.0, 8.0), s.read(200.0, 1000.0));
}

// ------------------------------------------------------------------------------------ clock

#[test]
fn clock_fires_on_its_first_tick() {
    let mut rng = Rng::new(1);
    let mut c = GrainClock::new();
    assert!(c.tick(0.001, 0.0, &mut rng));
    assert!(!c.tick(0.001, 0.0, &mut rng));
}

#[test]
fn clock_spawns_the_density_over_ten_seconds() {
    for density in [1.0, 7.0, 24.0, 60.0, 133.7, 200.0] {
        let mut rng = Rng::new(3);
        let mut c = GrainClock::new();
        let rate = density / SR;
        let n = (0..480_000).filter(|_| c.tick(rate, 0.0, &mut rng)).count();
        let want = density * 10.0;
        assert!(
            (n as f64 - want).abs() <= 1.5,
            "{density}/s: {n} grains in 10 s, want {want}"
        );
    }
}

#[test]
fn clock_spaces_a_regular_stream_evenly() {
    let mut rng = Rng::new(3);
    let mut c = GrainClock::new();
    let rate = 100.0 / SR; // every 480 samples
    let at: Vec<usize> = (0..4_800).filter(|_| c.tick(rate, 0.0, &mut rng)).collect();
    assert_eq!(at.len(), 10);
    for w in at.windows(2) {
        assert_eq!(w[1] - w[0], 480);
    }
}

#[test]
fn clock_timing_randomises_the_spacing_but_keeps_the_rate() {
    let mut rng = Rng::new(9);
    let mut c = GrainClock::new();
    let rate = 40.0 / SR;
    let mut at = Vec::new();
    for i in 0..4_800_000 {
        if c.tick(rate, 1.0, &mut rng) {
            at.push(i);
        }
    }
    let gaps: Vec<usize> = at.windows(2).map(|w| w[1] - w[0]).collect();
    let (lo, hi) = (gaps.iter().min().unwrap(), gaps.iter().max().unwrap());
    assert!(*hi > *lo + 600, "irregular: {lo}..{hi}");
    let mean = gaps.iter().sum::<usize>() as f64 / gaps.len() as f64;
    // The mean of the random intervals is the regular one, to the statistics of 4000 draws.
    assert!((mean - 1_200.0).abs() < 30.0, "mean gap {mean}");
    // Intervals stay inside 5 % to 195 % of the mean (a little slack for the carried phase).
    assert!(*lo as f64 > 0.04 * 1_200.0 && (*hi as f64) < 2.0 * 1_200.0);
}

#[test]
fn clock_is_a_pure_function_of_its_seed() {
    let run = |seed| {
        let mut rng = Rng::new(seed);
        let mut c = GrainClock::new();
        (0..50_000)
            .map(|_| c.tick(30.0 / SR, 0.7, &mut rng))
            .collect::<Vec<_>>()
    };
    assert_eq!(run(5), run(5));
    assert_ne!(run(5), run(6));
}

#[test]
fn clock_follows_a_changing_rate_without_skipping_or_doubling() {
    let mut rng = Rng::new(2);
    let mut c = GrainClock::new();
    // Phase gained over the run is what fires: a ramp from 10 to 110 grains/s over 2 s.
    let mut gained = 0.0;
    let mut n = 0usize;
    for i in 0..96_000 {
        let rate = (10.0 + 100.0 * f64::from(i) / 96_000.0) / SR;
        gained += rate;
        if c.tick(rate, 0.0, &mut rng) {
            n += 1;
        }
    }
    assert!(
        (n as f64 - (gained + 1.0).floor()).abs() < 1.5,
        "{n} vs {gained}"
    );
}

#[test]
fn clock_sanitises_the_rate() {
    let mut rng = Rng::new(2);
    let mut c = GrainClock::new();
    assert!(c.tick(f64::NAN, 0.0, &mut rng));
    for _ in 0..1000 {
        assert!(!c.tick(f64::NAN, 0.0, &mut rng));
        assert!(!c.tick(-5.0, 0.0, &mut rng));
    }
    // A rate above one grain per sample fires once per sample at most.
    assert!(c.tick(50.0, 0.0, &mut rng));
    assert!(c.tick(50.0, 0.0, &mut rng));
    assert!(c.tick(50.0, 0.0, &mut rng));
    // A non-finite rate counts as none.
    assert!(!c.tick(f64::INFINITY, 0.0, &mut rng));
}

// ----------------------------------------------------------------------------------- pool

#[test]
fn one_grain_plays_its_window_and_then_ends() {
    let ones = vec![1.0f32; 2_048];
    let src = GrainSource::mono(&ones, 0, 2_048);
    let b = bank();
    let mut pool = pool_with(GrainParams {
        len: 480,
        gain: [0.5, 2.0],
        ..params(480)
    });
    assert_eq!(pool.active(), 1);
    for i in 0..480 {
        let (l, r) = pool.process(&b, &src);
        let w = b.gain(Window::Hann, i as f32 / 480.0);
        assert!(
            (l - 0.5 * w).abs() < 1e-6 && (r - 2.0 * w).abs() < 1e-6,
            "sample {i}"
        );
    }
    assert_eq!(pool.active(), 0);
    assert_eq!(pool.process(&b, &src), (0.0, 0.0));
}

#[test]
fn grain_length_is_limited_to_at_least_one_sample() {
    let ones = vec![1.0f32; 64];
    let src = GrainSource::mono(&ones, 0, 64);
    let mut pool = pool_with(params(0));
    pool.process(&bank(), &src);
    assert_eq!(pool.active(), 0);
}

#[test]
fn pool_sums_overlapping_grains() {
    let ones = vec![1.0f32; 4_096];
    let src = GrainSource::mono(&ones, 0, 4_096);
    let b = bank();
    let mut pool = GrainPool::new();
    pool.spawn(params(1_000));
    let mut out = Vec::new();
    for i in 0..1_500 {
        if i == 500 {
            pool.spawn(params(1_000));
        }
        out.push(pool.process(&b, &src).0);
    }
    // Hann grains half a length apart sum to 1 while both sound.
    for v in &out[500..1_000] {
        assert!((v - 1.0).abs() < 2e-3, "{v}");
    }
    assert_eq!(pool.active(), 0);
}

#[test]
fn grains_play_the_source_at_the_pitch_the_step_asks_for() {
    let x = sine(480.0, SR, 96_000, 0.8);
    let src = GrainSource::mono(&x, 0, 96_000);
    let b = bank();
    for semis in [-12.0f64, -5.0, 0.0, 7.0, 12.0, 19.0] {
        let step = (semis / 12.0).exp2();
        let mut pool = pool_with(GrainParams {
            pos: 100.0,
            step,
            len: 30_000,
            window: Window::Tukey,
            gain: [1.0, 1.0],
        });
        let y: Vec<f32> = (0..30_000).map(|_| pool.process(&b, &src).0).collect();
        let mid = &y[8_000..22_000];
        let hz = zero_crossing_hz(mid);
        let want = 480.0 * step;
        assert!(
            (hz / want - 1.0).abs() < 2e-4,
            "{semis} st: {hz} Hz, want {want}"
        );
    }
}

#[test]
fn a_reversed_grain_reads_the_source_backwards() {
    let ramp: Vec<f32> = (0..2_000).map(|i| i as f32 / 1_000.0).collect();
    let src = GrainSource::mono(&ramp, 0, 2_000);
    let b = bank();
    let mut pool = pool_with(GrainParams {
        pos: 1_500.0,
        step: -1.0,
        len: 400,
        window: Window::Tukey,
        gain: [1.0, 1.0],
    });
    let y: Vec<f32> = (0..400).map(|_| pool.process(&b, &src).0).collect();
    // The flat middle of the window follows the source downwards: sample 200 is frame 1300.
    assert!((y[200] - 1.3).abs() < 1e-3, "{}", y[200]);
    assert!(y[150] > y[200] && y[200] > y[250]);
}

#[test]
fn a_full_pool_replaces_the_grain_nearest_its_end() {
    let ones = vec![1.0f32; 64];
    let src = GrainSource::mono(&ones, 0, 64);
    let b = bank();
    let mut pool = GrainPool::new();
    pool.set_limit(4);
    // Four grains of different lengths, started together, aged 200 samples: progress 0.2, 0.4,
    // 0.5 and 0.8 (the second slot is the nearest to its end after the fourth is replaced).
    for len in [1_000, 500, 400, 250] {
        pool.spawn(params(len));
    }
    for _ in 0..200 {
        pool.process(&b, &src);
    }
    assert_eq!(pool.active(), 4);
    let (slot, stole) = pool.spawn(params(777));
    assert!(stole);
    assert_eq!(slot, 3, "the grain at 0.8 of its length goes");
    assert_eq!(pool.active(), 4);
    assert_eq!(pool.stolen(), 1);
}

#[test]
fn stealing_breaks_ties_towards_the_lowest_slot() {
    let ones = vec![1.0f32; 64];
    let src = GrainSource::mono(&ones, 0, 64);
    let b = bank();
    let mut pool = GrainPool::new();
    pool.set_limit(3);
    for _ in 0..3 {
        pool.spawn(params(600));
    }
    pool.process(&b, &src);
    assert_eq!(pool.spawn(params(600)), (0, true));
}

#[test]
fn the_limit_caps_the_grains_and_never_cuts_a_sounding_one() {
    let ones = vec![1.0f32; 64];
    let src = GrainSource::mono(&ones, 0, 64);
    let b = bank();
    let mut pool = GrainPool::new();
    for _ in 0..MAX_GRAINS {
        pool.spawn(params(10_000));
    }
    assert_eq!(pool.active(), MAX_GRAINS);
    pool.set_limit(10);
    assert_eq!(pool.limit(), 10);
    assert_eq!(pool.active(), MAX_GRAINS, "lowering the limit cuts nothing");
    pool.process(&b, &src);
    assert_eq!(pool.active(), MAX_GRAINS);
    // A new grain replaces one of the first ten slots.
    let (slot, stole) = pool.spawn(params(500));
    assert!(stole && slot < 10);
    // The limit is itself limited.
    pool.set_limit(0);
    assert_eq!(pool.limit(), 1);
    pool.set_limit(10_000);
    assert_eq!(pool.limit(), MAX_GRAINS);
}

#[test]
fn spawning_beyond_the_pool_never_exceeds_the_active_count() {
    let mut pool = GrainPool::new();
    for _ in 0..500 {
        pool.spawn(params(300));
        assert!(pool.active() <= MAX_GRAINS);
    }
    assert_eq!(pool.stolen(), 500 - MAX_GRAINS as u64);
}

#[test]
fn clear_ends_every_grain_and_resets_the_count() {
    let mut pool = GrainPool::new();
    for _ in 0..100 {
        pool.spawn(params(300));
    }
    pool.clear();
    assert_eq!((pool.active(), pool.stolen()), (0, 0));
}

#[test]
fn pool_is_deterministic() {
    let x = sine(333.0, SR, 20_000, 0.9);
    let src = GrainSource::mono(&x, 0, 20_000);
    let b = bank();
    let run = || {
        let mut pool = GrainPool::new();
        pool.set_limit(12);
        let mut rng = Rng::new(4);
        let mut clock = GrainClock::new();
        let mut out = Vec::new();
        for _ in 0..30_000 {
            if clock.tick(90.0 / SR, 0.5, &mut rng) {
                pool.spawn(GrainParams {
                    pos: f64::from(rng.next_f32()) * 15_000.0,
                    step: if rng.next_f32() < 0.3 { -1.2 } else { 0.9 },
                    len: 2_000 + (rng.next_f32() * 4_000.0) as u32,
                    window: Window::from_index((rng.next_f32() * 3.0) as usize),
                    gain: [rng.next_f32(), rng.next_f32()],
                });
            }
            let (l, r) = pool.process(&b, &src);
            out.push(l.to_bits());
            out.push(r.to_bits());
        }
        out
    };
    assert_eq!(run(), run());
}

#[test]
fn spawn_sanitises_a_non_finite_position_or_step() {
    let ones = vec![1.0f32; 64];
    let src = GrainSource::mono(&ones, 0, 64);
    let b = bank();
    let mut pool = GrainPool::new();
    pool.spawn(GrainParams {
        pos: f64::NAN,
        step: f64::NAN,
        len: 100,
        window: Window::Hann,
        gain: [1.0, 1.0],
    });
    pool.spawn(GrainParams {
        pos: f64::INFINITY,
        step: 1.0e9,
        len: u32::MAX,
        window: Window::Hann,
        gain: [1.0, 1.0],
    });
    for _ in 0..200 {
        let (l, r) = pool.process(&b, &src);
        assert!(l.is_finite() && r.is_finite());
    }
}

// ---------------------------------------------------------------------------- positions

#[test]
fn fit_start_keeps_a_fitting_grain_inside_the_window() {
    assert_eq!(fit_start(900.0, 200.0, 1_000.0, false), 800.0);
    assert_eq!(fit_start(-50.0, 200.0, 1_000.0, false), 0.0);
    assert_eq!(fit_start(500.0, 200.0, 1_000.0, false), 500.0);
    assert_eq!(fit_start(100.0, 200.0, 1_000.0, true), 200.0);
    assert_eq!(fit_start(1_200.0, 200.0, 1_000.0, true), 1_000.0);
    assert_eq!(fit_start(500.0, 200.0, 1_000.0, true), 500.0);
    // A grain longer than the window keeps its start (the read wraps).
    assert_eq!(fit_start(500.0, 1_000.0, 1_000.0, false), 500.0);
    assert_eq!(fit_start(500.0, 3_000.0, 1_000.0, true), 500.0);
    assert!(fit_start(f64::NAN, 10.0, 100.0, false).is_nan());
}

proptest! {
    #[test]
    fn fit_start_result_fits_when_the_grain_fits(
        start in -2_000.0f64..3_000.0,
        span in 0.0f64..900.0,
        window in 1_000.0f64..2_000.0,
        reverse in any::<bool>(),
    ) {
        let s = fit_start(start, span, window, reverse);
        if reverse {
            prop_assert!(s - span >= -1e-9 && s <= window + 1e-9);
        } else {
            prop_assert!(s >= -1e-9 && s + span <= window + 1e-9);
        }
    }

    #[test]
    fn fold01_stays_in_unit_range_and_is_the_identity_inside_it(x in -50.0f64..50.0) {
        let f = fold01(x);
        prop_assert!((0.0..=1.0).contains(&f));
        if (0.0..=1.0).contains(&x) {
            prop_assert!((f - x).abs() < 1e-15);
        }
    }

    #[test]
    fn pool_output_is_finite_and_bounded_for_any_grains(
        grains in prop::collection::vec(
            (-5_000.0f64..50_000.0, -9.0f64..9.0, 1u32..6_000, 0usize..3, 0.0f32..1.5, 0.0f32..1.5),
            1..100,
        ),
        limit in 1usize..=64,
    ) {
        let x = sine(777.0, SR, 8_000, 1.0);
        let src = GrainSource::mono(&x, 100, 7_900);
        let b = bank();
        let mut pool = GrainPool::new();
        pool.set_limit(limit);
        let mut bound = 0.0f32;
        for (pos, step, len, w, gl, gr) in grains {
            pool.spawn(GrainParams { pos, step, len, window: Window::from_index(w), gain: [gl, gr] });
            bound += gl.max(gr);
        }
        for _ in 0..800 {
            let (l, r) = pool.process(&b, &src);
            prop_assert!(l.is_finite() && r.is_finite());
            prop_assert!(l.abs() <= bound * 1.01 + 1e-3 && r.abs() <= bound * 1.01 + 1e-3);
        }
    }
}

#[test]
fn fold01_reflects_at_both_ends() {
    assert!((fold01(-0.1) - 0.1).abs() < 1e-12);
    assert!((fold01(1.2) - 0.8).abs() < 1e-12);
    assert!((fold01(2.3) - 0.3).abs() < 1e-12);
    assert_eq!(fold01(f64::NAN), 0.0);
    assert_eq!(fold01(f64::INFINITY), 0.0);
}

// ------------------------------------------------------------------------- default source

#[test]
fn default_source_has_the_documented_length_and_level() {
    for sr in [44_100.0f32, 48_000.0, 96_000.0] {
        let x = default_source(sr);
        assert_eq!(x.len(), (3.0 * sr).round() as usize);
        let peak = x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((peak - 0.7).abs() < 1e-4, "{sr}: peak {peak}");
        assert!(x.iter().all(|v| v.is_finite()));
        assert!(rms(&x) > 0.1, "not silent: {}", rms(&x));
        assert!(
            x[0].abs() < 1e-3 && x[x.len() - 1].abs() < 1e-3,
            "faded edges"
        );
    }
}

#[test]
fn default_source_is_bit_identical_on_every_call() {
    let a = default_source(48_000.0);
    let b = default_source(48_000.0);
    assert_eq!(
        a.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        b.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
    );
}

#[test]
fn default_source_is_a_tone_at_middle_c() {
    let x = default_source(48_000.0);
    // The first second of the first vowel: the strongest bin is a harmonic of 261.6 Hz.
    let hz = dominant_hz(&x[12_000..28_384], SR);
    let h = hz / 261.625_565;
    assert!(
        (h - h.round()).abs() < 0.03 && h.round() >= 1.0,
        "peak at {hz} Hz"
    );
}

#[test]
fn default_source_changes_its_timbre_along_the_position() {
    let x = default_source(48_000.0);
    // The second formant of an "ee" (2.3 kHz) against the first of an "ah" (0.8 kHz).
    let ratio = |from: usize| {
        let seg = &x[from..from + 8_192];
        let m = crate::test_util::spectrum(seg);
        let k = |hz: f64| (hz * seg.len() as f64 / SR) as usize;
        let band = |a: f64, b: f64| m[k(a)..k(b)].iter().map(|v| v * v).sum::<f64>();
        band(2_000.0, 2_600.0) / band(500.0, 1_100.0)
    };
    let ah = ratio(4_000);
    let ee = ratio(3 * 48_000 - 12_000);
    assert!(ee > 5.0 * ah, "ee {ee} against ah {ah}");
}

#[test]
fn default_source_survives_an_odd_sample_rate() {
    assert_eq!(default_source(f32::NAN).len(), 24_000);
    assert_eq!(default_source(100.0).len(), 24_000);
    assert!(default_source(192_000.0).iter().all(|v| v.is_finite()));
}
