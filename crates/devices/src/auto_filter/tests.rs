//! Auto-filter tests, all through `DeviceHarness` (every `process` is allocation-guarded).
#![allow(clippy::float_cmp)]

use super::*;
use crate::test_support::{SR, assert_keys, harness, sine, sweep_params_min_max, tone_amplitude};
use atomik_testkit::harness::DeviceHarness;

fn filt() -> DeviceHarness {
    harness(Box::new(AutoFilter::new()))
}

fn ms(t: f64) -> usize {
    (t * 0.001 * SR).round() as usize
}

fn db(x: f64) -> f64 {
    20.0 * x.max(1e-12).log10()
}

/// Runs `signal` on both inputs for `samples` and returns `(out_l, out_r, lfo_cv_out)`.
fn run(
    h: &mut DeviceHarness,
    signal: &dyn Fn(usize) -> f32,
    from: usize,
    samples: usize,
) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
    let (mut l, mut r, mut lfo) = (Vec::new(), Vec::new(), Vec::new());
    for b in 0..samples.div_ceil(BATCH) {
        let mut x = [0.0f32; BATCH];
        for (i, s) in x.iter_mut().enumerate() {
            *s = signal(from + b * BATCH + i);
        }
        *h.input_mut("in_l") = x;
        *h.input_mut("in_r") = x;
        h.process();
        l.extend_from_slice(h.output("out_l"));
        r.extend_from_slice(h.output("out_r"));
        lfo.extend_from_slice(h.output("lfo_cv_out"));
    }
    (l, r, lfo)
}

/// Peak of each 10 ms window of `y`.
fn envelope(y: &[f32]) -> Vec<f32> {
    y.chunks(ms(10.0))
        .map(|c| c.iter().fold(0.0f32, |m, v| m.max(v.abs())))
        .collect()
}

#[test]
fn descriptor_validates() {
    assert_keys(
        &DESCRIPTOR,
        &[
            "mode",
            "slope",
            "cutoff",
            "resonance",
            "depth",
            "rate",
            "sync",
            "division",
            "shape",
            "spread",
            "follow",
            "mix",
            "cutoff_cv_trim",
            "depth_cv_trim",
            "rate_cv_trim",
            "bypass",
        ],
        &[
            "in_l",
            "in_r",
            "cutoff_cv_in",
            "depth_cv_in",
            "rate_cv_in",
            "out_l",
            "out_r",
            "lfo_cv_out",
        ],
    );
    assert_eq!(DESCRIPTOR.type_id, "atomik.auto-filter");
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Effect);
}

#[test]
fn without_depth_it_is_a_plain_filter_at_the_knob() {
    let gain = |mode: f32, hz: f64| {
        let mut h = filt();
        h.set_param("mode", mode);
        h.set_param("depth", 0.0);
        let (y, _, _) = run(&mut h, &sine(hz, 0.5), 0, ms(1_000.0));
        db(tone_amplitude(&y[ms(500.0)..], hz, ms(500.0)) / 0.5)
    };
    assert!(gain(0.0, 100.0).abs() < 1.0, "{}", gain(0.0, 100.0));
    assert!(gain(0.0, 5_000.0) < -20.0, "{}", gain(0.0, 5_000.0));
    assert!(gain(2.0, 5_000.0).abs() < 1.0);
    assert!(gain(2.0, 100.0) < -20.0);
    assert!(gain(1.0, 800.0).abs() < 1.5, "unity at the centre");
    assert!(gain(1.0, 100.0) < -10.0);
    assert!(gain(3.0, 800.0) < -10.0, "a notch");
    assert!(gain(3.0, 100.0).abs() < 1.5);
}

#[test]
fn the_steeper_slope_cuts_more() {
    let gain = |slope: f32| {
        let mut h = filt();
        h.set_param("depth", 0.0);
        h.set_param("slope", slope);
        let (y, _, _) = run(&mut h, &sine(4_000.0, 0.5), 0, ms(500.0));
        db(tone_amplitude(&y[ms(250.0)..], 4_000.0, ms(250.0)) / 0.5)
    };
    assert!(
        gain(1.0) < gain(0.0) - 15.0,
        "{} vs {}",
        gain(1.0),
        gain(0.0)
    );
}

#[test]
fn the_lfo_sweeps_the_cutoff() {
    let mut h = filt();
    h.set_param("depth", 1.0);
    h.set_param("rate", 2.0);
    // 2 kHz is over the 800 Hz knob and under the top of a four octave sweep.
    let (y, _, _) = run(&mut h, &sine(2_000.0, 0.5), 0, ms(2_000.0));
    let e = envelope(&y[ms(500.0)..]);
    let hi = e.iter().fold(0.0f32, |m, v| m.max(*v));
    let lo = e.iter().fold(f32::MAX, |m, v| m.min(*v));
    assert!(hi > 0.3, "{hi}");
    assert!(db(f64::from(hi)) - db(f64::from(lo)) > 15.0, "{hi} {lo}");
}

#[test]
fn the_lfo_runs_at_the_rate_and_every_shape_stays_in_range() {
    for shape in 0..5 {
        let mut h = filt();
        h.set_param("shape", shape as f32);
        h.set_param("rate", 4.0);
        let (_, _, lfo) = run(&mut h, &|_| 0.0, 0, ms(2_000.0));
        assert!(
            lfo.iter().all(|v| (-1.0..=1.0).contains(v)),
            "shape {shape}"
        );
        if shape == 0 {
            // A sine rising through zero 8 times in 2 s.
            let rising = lfo.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
            assert!((7..=9).contains(&rising), "{rising}");
        }
        if shape == 4 {
            let mut distinct = lfo.clone();
            distinct.dedup();
            assert!((7..=10).contains(&distinct.len()), "{}", distinct.len());
        }
    }
}

#[test]
fn sync_runs_at_a_note_value_of_the_tempo() {
    let mut h = filt();
    h.set_param("sync", 1.0);
    h.set_param("division", 8.0); // 1/4
    h.time_mut().tempo = atomik_core::Tempo::new(120.0).unwrap();
    // Two quarter notes a second: a cycle every 24000 samples.
    let (_, _, lfo) = run(&mut h, &|_| 0.0, 0, ms(2_000.0));
    let rising = lfo.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
    assert!((3..=5).contains(&rising), "{rising}");
}

#[test]
fn a_synced_lfo_follows_the_song_position_while_playing() {
    let at = |tick: f64, warm: usize| {
        let mut h = filt();
        h.set_param("sync", 1.0);
        h.set_param("division", 8.0);
        let _ = run(&mut h, &|_| 0.0, 0, warm);
        let t = h.time_mut();
        t.playing = true;
        t.tick = tick;
        let (_, _, lfo) = run(&mut h, &|_| 0.0, 0, BATCH);
        lfo
    };
    let a = at(15_360.0 * 3.37, 0);
    let b = at(15_360.0 * 3.37, ms(777.0));
    for (x, y) in a.iter().zip(&b) {
        assert_eq!(x.to_bits(), y.to_bits());
    }
    // A quarter into the cycle a sine is at its peak.
    let c = at(15_360.0 * 0.25, 0);
    assert!(c[0] > 0.99, "{}", c[0]);
}

#[test]
fn the_spread_offsets_the_right_channel() {
    let mut h = filt();
    h.set_param("depth", 1.0);
    h.set_param("rate", 2.0);
    let tone = sine(1_500.0, 0.5);
    let (l, r, _) = run(&mut h, &tone, 0, ms(1_000.0));
    let same = l.iter().zip(&r).all(|(a, b)| a == b);
    assert!(same, "no spread: identical channels");
    let mut h = filt();
    h.set_param("depth", 1.0);
    h.set_param("rate", 2.0);
    h.set_param("spread", 1.0);
    let (l, r, _) = run(&mut h, &tone, 0, ms(1_000.0));
    let (el, er) = (envelope(&l), envelope(&r));
    let apart = el
        .iter()
        .zip(&er)
        .any(|(a, b)| (db(f64::from(*a)) - db(f64::from(*b))).abs() > 6.0);
    assert!(apart, "the channels sweep at different moments");
}

#[test]
fn the_follower_opens_the_filter_with_the_level() {
    let gain = |amp: f64| {
        let mut h = filt();
        h.set_param("depth", 0.0);
        h.set_param("follow", 1.0);
        let (y, _, _) = run(&mut h, &sine(6_000.0, amp), 0, ms(1_000.0));
        db(tone_amplitude(&y[ms(500.0)..], 6_000.0, ms(500.0)) / amp)
    };
    assert!(
        gain(0.9) > gain(0.02) + 12.0,
        "{} vs {}",
        gain(0.9),
        gain(0.02)
    );
}

#[test]
fn the_cutoff_cv_moves_the_cutoff_through_its_trim() {
    let gain = |cv: f32, trim: f32| {
        let mut h = filt();
        h.set_param("depth", 0.0);
        h.set_param("cutoff_cv_trim", trim);
        let tone = sine(3_000.0, 0.5);
        let mut y = Vec::new();
        for b in 0..ms(500.0).div_ceil(BATCH) {
            let mut x = [0.0f32; BATCH];
            for (i, s) in x.iter_mut().enumerate() {
                *s = tone(b * BATCH + i);
            }
            *h.input_mut("in_l") = x;
            *h.input_mut("in_r") = x;
            *h.input_mut("cutoff_cv_in") = [cv; BATCH];
            h.process();
            y.extend_from_slice(h.output("out_l"));
        }
        db(tone_amplitude(&y[ms(250.0)..], 3_000.0, ms(250.0)) / 0.5)
    };
    assert!(gain(1.0, 1.0) > gain(0.0, 1.0) + 15.0);
    assert!((gain(1.0, 0.0) - gain(0.0, 1.0)).abs() < 0.01);
    assert!(gain(1.0, -1.0) < gain(0.0, 1.0));
}

#[test]
fn mix_zero_is_the_dry_input_and_a_resonant_sweep_stays_bounded() {
    let mut h = filt();
    h.set_param("mix", 0.0);
    h.set_param("depth", 1.0);
    let tone = sine(1_000.0, 0.5);
    let (y, _, _) = run(&mut h, &tone, 0, ms(100.0));
    assert!(y.iter().enumerate().all(|(n, v)| *v == tone(n)));
    let mut h = filt();
    h.set_param("resonance", 1.0);
    h.set_param("slope", 1.0);
    h.set_param("cutoff", 1_000.0);
    h.set_param("depth", 0.0);
    let (y, _, _) = run(&mut h, &sine(1_000.0, 1.0), 0, ms(500.0));
    let peak = y.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    assert!(peak > 4.0 && peak < 8.0, "{peak}");
}

#[test]
fn the_soft_limit_is_exact_below_the_knee_and_bounded_above() {
    for x in [0.0f32, 0.5, -3.9, 4.0, -4.0] {
        assert_eq!(soft_limit(x), x);
    }
    assert!(soft_limit(4.5) > 4.0 && soft_limit(4.5) < 5.0);
    assert!(soft_limit(1.0e9) <= 8.0 && soft_limit(-1.0e9) >= -8.0);
    assert!(soft_limit(5.0) > soft_limit(4.5));
}

#[test]
fn non_finite_input_counts_as_silence() {
    let mut h = filt();
    let (y, _, _) = run(
        &mut h,
        &|n| if n % 5 == 0 { f32::NAN } else { f32::INFINITY },
        0,
        2_000,
    );
    assert!(y.iter().all(|v| *v == 0.0));
}

#[test]
fn every_parameter_extreme_stays_finite() {
    let mut h = filt();
    sweep_params_min_max(&mut h, &DESCRIPTOR);
}
