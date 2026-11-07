//! Clipper tests, all through `DeviceHarness` (every `process` is allocation-guarded).
#![allow(clippy::float_cmp)]

use super::*;
use crate::test_support::{SR, assert_keys, harness, run_signal, sine, sweep_params_min_max};
use atomik_dsp::Rng;
use atomik_testkit::harness::DeviceHarness;

fn clipper() -> DeviceHarness {
    harness(Box::new(Clipper::new()))
}

/// The settings that make the clipper bit-transparent below the ceiling.
fn transparent() -> DeviceHarness {
    let mut h = clipper();
    h.set_param("curve", 0.0);
    h.set_param("oversample", 0.0);
    h.set_param("symmetry", 0.0);
    h.set_param("bands", 0.0);
    h.set_param("mix", 1.0);
    h.set_param("delta", 0.0);
    h.set_param("gain", 0.0);
    h.set_param("output", 0.0);
    h
}

fn secs(t: f64) -> usize {
    (t * SR).round() as usize
}

fn batches(samples: usize) -> usize {
    samples.div_ceil(BATCH)
}

/// Runs `samples` of `signal` through both inputs; returns `(out_l, out_r)`.
fn run(
    h: &mut DeviceHarness,
    signal: impl Fn(usize) -> f32,
    samples: usize,
) -> (Vec<f32>, Vec<f32>) {
    let mut y = run_signal(
        h,
        &["in_l", "in_r"],
        signal,
        batches(samples),
        &["out_l", "out_r"],
    );
    let r = y.remove(1);
    (y.remove(0), r)
}

/// Deterministic noise of peak `amp`.
fn noise(amp: f32, len: usize, seed: u64) -> Vec<f32> {
    let mut r = Rng::new(seed);
    (0..len).map(|_| r.next_bipolar() * amp).collect()
}

fn peak(y: &[f32]) -> f32 {
    y.iter().fold(0.0f32, |m, v| m.max(v.abs()))
}

fn ceiling_lin(db: f32) -> f32 {
    lin(db)
}

fn meters(h: &DeviceHarness) -> [f32; METERS] {
    let mut out = [0.0f32; METERS];
    assert_eq!(h.device().meter_values(&mut out), METERS);
    out
}

#[test]
fn descriptor_validates() {
    assert_keys(
        &DESCRIPTOR,
        &[
            "gain",
            "ceiling",
            "curve",
            "symmetry",
            "oversample",
            "output",
            "mix",
            "delta",
            "bands",
            "xover.low",
            "xover.high",
            "band1.drive",
            "band2.drive",
            "band3.drive",
            "guard",
            "bypass",
        ],
        &["in_l", "in_r", "out_l", "out_r"],
    );
    assert_eq!(DESCRIPTOR.type_id, "atomik.clipper");
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Effect);
}

#[test]
fn hard_symmetric_unoversampled_is_bit_transparent_below_the_ceiling_and_holds_it_above() {
    let mut h = transparent();
    h.set_param("ceiling", -6.0);
    let c = ceiling_lin(-6.0);
    let x = noise(1.0, secs(0.5), 7);
    let (l, r) = run(&mut h, |n| x[n], x.len());
    let mut below = 0;
    let mut above = 0;
    for (i, &v) in x.iter().enumerate() {
        let want = v.clamp(-c, c);
        assert_eq!(l[i].to_bits(), want.to_bits(), "L sample {i}");
        assert_eq!(r[i].to_bits(), want.to_bits(), "R sample {i}");
        if v.abs() <= c {
            below += 1;
        } else {
            above += 1;
        }
    }
    assert!(below > 1000 && above > 1000, "{below} below, {above} above");
}

#[test]
fn a_quiet_signal_passes_unchanged_at_every_curve_below_the_knee() {
    for curve in [0.0f32, 0.25, 0.5, 1.0] {
        let mut h = transparent();
        h.set_param("curve", curve);
        h.set_param("ceiling", 0.0);
        // Below the knee `(1 - curve) * ceiling`.
        let amp = 0.99 * (1.0 - curve);
        let x = noise(amp, secs(0.2), 3);
        let (l, _) = run(&mut h, |n| x[n], x.len());
        assert!(
            l.iter().zip(&x).all(|(a, b)| a.to_bits() == b.to_bits()),
            "curve {curve}"
        );
    }
}

#[test]
fn the_output_never_passes_the_ceiling_at_any_oversampling_curve_or_band_mode() {
    let c = ceiling_lin(-3.0);
    for os in 0..4 {
        for curve in [0.0f32, 0.5, 1.0] {
            for bands in 0..3 {
                for symmetry in [0.0f32, 0.6, -1.0] {
                    let mut h = clipper();
                    h.set_param("ceiling", -3.0);
                    h.set_param("gain", 12.0);
                    h.set_param("oversample", os as f32);
                    h.set_param("curve", curve);
                    h.set_param("bands", bands as f32);
                    h.set_param("symmetry", symmetry);
                    let x = noise(1.0, secs(0.3), 5 + os as u64);
                    let (l, r) = run(&mut h, |n| x[n], x.len());
                    // Multiband: each band holds the ceiling, so the sum is bounded by the sum.
                    let limit = if bands == 0 { c } else { 3.3 * c };
                    assert!(
                        peak(&l) <= limit && peak(&r) <= limit,
                        "os {os} curve {curve} bands {bands} sym {symmetry}: {} {}",
                        peak(&l),
                        peak(&r)
                    );
                }
            }
        }
    }
}

#[test]
fn the_soft_curve_squeezes_above_its_knee() {
    // Knee at half the ceiling: 0.8 of full scale is above it and ends below the hard clip.
    let mut hard = transparent();
    hard.set_param("ceiling", 0.0);
    let mut soft = transparent();
    soft.set_param("ceiling", 0.0);
    soft.set_param("curve", 0.5);
    let sig = |_n: usize| 0.8f32;
    let (a, _) = run(&mut hard, sig, 256);
    let (b, _) = run(&mut soft, sig, 256);
    assert_eq!(*a.last().unwrap(), 0.8);
    let y = *b.last().unwrap();
    assert!(y < 0.8 && y > 0.5, "{y}");
    // A louder input approaches the ceiling without passing it.
    let (c, _) = run(&mut soft, |_| 8.0, 256);
    assert!(*c.last().unwrap() > 0.97 && *c.last().unwrap() <= 1.0);
}

#[test]
fn symmetry_adds_even_harmonics_and_the_offset_is_removed() {
    let tone = |h: &mut DeviceHarness| {
        let n = secs(1.5);
        let (y, _) = run(h, sine(1000.0, 1.5), n);
        y
    };
    let amp = |y: &[f32], hz: f64| {
        let start = secs(0.5);
        crate::test_support::tone_amplitude(&y[start..start + secs(1.0)], hz, start)
    };
    let mut flat = transparent();
    flat.set_param("ceiling", -6.0);
    let y0 = tone(&mut flat);
    let mut skew = transparent();
    skew.set_param("ceiling", -6.0);
    skew.set_param("symmetry", 0.6);
    let y1 = tone(&mut skew);
    let (e0, e1) = (amp(&y0, 2000.0), amp(&y1, 2000.0));
    assert!(e0 < 1.0e-4, "symmetric: {e0}");
    assert!(e1 > 0.02, "asymmetric: {e1}");
    // The mean of the output is the offset: close to nothing with the blocker in.
    let tail = &y1[secs(1.0)..];
    let mean = tail.iter().map(|v| f64::from(*v)).sum::<f64>() / tail.len() as f64;
    assert!(mean.abs() < 2.0e-3, "{mean}");
    // Without the blocker the same curve would carry an offset of a few percent: check the
    // curve itself.
    let curve = ClipCurve::new(ceiling_lin(-6.0), 0.0, 0.6);
    let raw_mean = (0..4800)
        .map(|i| {
            let x = (1.5 * (std::f64::consts::TAU * i as f64 / 4800.0).sin()) as f32;
            f64::from(curve.apply(x))
        })
        .sum::<f64>()
        / 4800.0;
    assert!(raw_mean.abs() > 0.02, "{raw_mean}");
}

#[test]
fn oversampling_lowers_the_aliasing_of_a_loud_high_tone() {
    // 10 kHz clipped hard: the 3rd harmonic (30 kHz) folds to 18 kHz.
    let alias = |os: f32| {
        let mut h = transparent();
        h.set_param("ceiling", -6.0);
        h.set_param("oversample", os);
        h.set_param("guard", 0.0);
        let n = secs(1.5);
        let (y, _) = run(&mut h, sine(10_000.0, 2.0), n);
        let start = secs(0.5);
        crate::test_support::tone_amplitude(&y[start..start + secs(1.0)], 18_000.0, start)
    };
    let off = alias(0.0);
    assert!(off > 0.01, "the alias is there without oversampling: {off}");
    let each: Vec<f64> = [1.0, 2.0, 3.0].iter().map(|os| alias(*os)).collect();
    println!("alias at 18 kHz: off {off:.5}, 2x/4x/8x {each:?}");
    assert!(each[0] < off * 0.5, "2x: {} vs {off}", each[0]);
    assert!(each[1] < off * 0.1, "4x: {} vs {off}", each[1]);
    assert!(each[2] < off * 0.1, "8x: {} vs {off}", each[2]);
}

#[test]
fn the_guard_holds_the_ceiling_against_the_filter_overshoot() {
    let c = ceiling_lin(-6.0);
    let top = |guard: f32| {
        let mut h = transparent();
        h.set_param("ceiling", -6.0);
        h.set_param("oversample", 3.0);
        h.set_param("guard", guard);
        let (y, _) = run(&mut h, sine(3_000.0, 2.0), secs(0.5));
        peak(&y[secs(0.1)..])
    };
    assert!(top(0.0) > c * 1.01, "the filters overshoot a hard clip");
    assert!(top(1.0) <= c);
}

#[test]
fn the_latency_follows_the_oversampling_and_the_dry_path_matches_it() {
    for (os, want) in [(0.0, 0u32), (1.0, 29), (2.0, 35), (3.0, 37)] {
        let mut h = clipper();
        h.set_param("oversample", os);
        assert_eq!(h.device().latency_frames(), want, "os {os}");
        assert!(h.device().variable_latency());
        // Mix at 0 gives the input back, delayed by exactly the latency, bit for bit.
        h.set_param("mix", 0.0);
        let x = noise(0.7, secs(0.1), 21);
        let (l, _) = run(&mut h, |n| x[n], x.len());
        let lat = want as usize;
        for i in lat + 8..x.len() {
            assert_eq!(l[i].to_bits(), x[i - lat].to_bits(), "os {os} sample {i}");
        }
    }
}

#[test]
fn the_wet_path_has_the_reported_latency() {
    // A quiet band-limited burst through a clipper that does not clip: the wet output is the
    // input delayed by the latency (the oversampling filters are linear phase).
    for (os, lat) in [(1.0, 29usize), (2.0, 35), (3.0, 37)] {
        let mut h = clipper();
        h.set_param("oversample", os);
        h.set_param("ceiling", 0.0);
        let f = |n: usize| {
            if n < 480 {
                (0.2 * (std::f64::consts::TAU * 1000.0 * n as f64 / SR).sin()) as f32
            } else {
                0.0
            }
        };
        let (y, _) = run(&mut h, f, secs(0.1));
        let mut worst = 0.0f32;
        for i in 200..400 {
            worst = worst.max((y[i + lat] - f(i)).abs());
        }
        assert!(worst < 2.0e-3, "os {os}: {worst}");
    }
}

#[test]
fn delta_plays_what_the_clipper_removes() {
    let c = ceiling_lin(-6.0);
    let x = noise(1.0, secs(0.3), 9);
    // Nothing removed below the ceiling.
    let mut quiet = transparent();
    quiet.set_param("ceiling", -6.0);
    quiet.set_param("delta", 1.0);
    let q = noise(0.4, secs(0.1), 4);
    let (l, _) = run(&mut quiet, |n| q[n], q.len());
    assert!(l.iter().all(|v| *v == 0.0));
    // Above it: the excess, so clipped plus delta is the input.
    let mut h = transparent();
    h.set_param("ceiling", -6.0);
    h.set_param("delta", 1.0);
    let (d, _) = run(&mut h, |n| x[n], x.len());
    for (i, &v) in x.iter().enumerate() {
        let excess = v - v.clamp(-c, c);
        assert_eq!(d[i].to_bits(), excess.to_bits(), "sample {i}");
    }
    // With oversampling the delta is lined up with the wet path.
    let mut os = clipper();
    os.set_param("ceiling", -6.0);
    os.set_param("oversample", 2.0);
    let mut wet = clipper();
    wet.set_param("ceiling", -6.0);
    wet.set_param("oversample", 2.0);
    os.set_param("delta", 1.0);
    let (delta, _) = run(&mut os, |n| x[n], x.len());
    let (clipped, _) = run(&mut wet, |n| x[n], x.len());
    let lat = 35;
    for i in lat + 8..x.len() {
        assert!(
            (clipped[i] + delta[i] - x[i - lat]).abs() < 1.0e-5,
            "sample {i}"
        );
    }
}

#[test]
fn the_input_gain_drives_the_curve_and_the_output_gain_follows_it() {
    let mut h = transparent();
    h.set_param("ceiling", -6.0);
    h.set_param("gain", 12.0);
    h.set_param("output", -6.0);
    let (y, _) = run(&mut h, |_| 0.3, 4096);
    // 0.3 + 12 dB is 1.19, above the ceiling (0.501): held there, then -6 dB.
    let want = ceiling_lin(-6.0) * lin(-6.0);
    assert!((y[4000] - want).abs() < 1.0e-6, "{} vs {want}", y[4000]);
}

#[test]
fn three_bands_add_up_flat_when_nothing_clips() {
    for bands in [1.0, 2.0] {
        for hz in [60.0, 150.0, 400.0, 1_000.0, 3_000.0, 8_000.0] {
            let mut h = clipper();
            h.set_param("oversample", 0.0);
            h.set_param("ceiling", 0.0);
            h.set_param("bands", bands);
            let n = secs(1.5);
            let (y, _) = run(&mut h, sine(hz, 0.4), n);
            let start = secs(0.5);
            let a = crate::test_support::tone_amplitude(&y[start..start + secs(1.0)], hz, start);
            assert!(
                (20.0 * (a / 0.4).log10()).abs() < 0.05,
                "bands {bands}, {hz} Hz: {a}"
            );
        }
    }
}

#[test]
fn a_band_drive_changes_only_its_band() {
    let gain = |bands: f32, drive: &str, hz: f64| {
        let mut h = clipper();
        h.set_param("oversample", 0.0);
        h.set_param("ceiling", 0.0);
        h.set_param("bands", bands);
        h.set_param(drive, 6.0);
        let n = secs(1.5);
        let (y, _) = run(&mut h, sine(hz, 0.1), n);
        let start = secs(0.5);
        20.0 * (crate::test_support::tone_amplitude(&y[start..start + secs(1.0)], hz, start) / 0.1)
            .log10()
    };
    // 3 bands: low below 150 Hz, mid in between, high above 3 kHz.
    assert!((gain(2.0, "band1.drive", 60.0) - 6.0).abs() < 0.3);
    assert!(gain(2.0, "band1.drive", 1_000.0).abs() < 0.3);
    assert!(gain(2.0, "band1.drive", 8_000.0).abs() < 0.3);
    assert!((gain(2.0, "band2.drive", 1_000.0) - 6.0).abs() < 0.3);
    assert!(gain(2.0, "band2.drive", 60.0).abs() < 0.3);
    assert!((gain(2.0, "band3.drive", 8_000.0) - 6.0).abs() < 0.3);
    // 2 bands: band 2 is everything above the low crossover; band 3 does nothing.
    assert!((gain(1.0, "band2.drive", 1_000.0) - 6.0).abs() < 0.3);
    assert!(gain(1.0, "band3.drive", 8_000.0).abs() < 0.3);
}

#[test]
fn multiband_clipping_of_the_bass_leaves_the_highs_alone() {
    // A loud bass tone plus a quiet high tone: clipping only the low band's drive does not touch
    // the high tone, while a full-band clip squashes it with intermodulation.
    let sig = |n: usize| {
        let t = n as f64 / SR;
        (0.9 * (std::f64::consts::TAU * 60.0 * t).sin()
            + 0.05 * (std::f64::consts::TAU * 6_000.0 * t).sin()) as f32
    };
    let high = |y: &[f32]| {
        let start = secs(0.5);
        crate::test_support::tone_amplitude(&y[start..start + secs(1.0)], 6_000.0, start)
    };
    let mut multi = clipper();
    multi.set_param("oversample", 0.0);
    multi.set_param("ceiling", -6.0);
    multi.set_param("bands", 2.0);
    // The mid and high bands have a ceiling of their own that their content stays under.
    let (m, _) = run(&mut multi, sig, secs(1.5));
    assert!((high(&m) - 0.05).abs() < 0.004, "{}", high(&m));
    let mut full = transparent();
    full.set_param("ceiling", -6.0);
    let (f, _) = run(&mut full, sig, secs(1.5));
    assert!(high(&f) < 0.035, "{}", high(&f));
}

#[test]
fn clip_activity_reports_the_share_and_the_longest_run() {
    let mut h = transparent();
    h.set_param("ceiling", -6.0);
    let c = f64::from(ceiling_lin(-6.0));
    // Amplitude 2 at 1 kHz: clipped while |sin| > c / 2.
    let (_, _) = run(&mut h, sine(997.0, 2.0), secs(1.0));
    let m = meters(&h);
    let r = c / 2.0;
    let share = 100.0 * (1.0 - 2.0 * r.asin() / std::f64::consts::PI);
    assert!(
        (f64::from(m[meter::SHARE]) - share).abs() < 1.0,
        "{} vs {share}",
        m[meter::SHARE]
    );
    // One run per half cycle: a half period (0.5 ms) of which the clipped part is `share`.
    let longest = 0.5 * share / 100.0;
    assert!(
        (f64::from(m[meter::LONGEST]) - longest).abs() < 0.06,
        "{} vs {longest}",
        m[meter::LONGEST]
    );
    assert!(m[meter::DRIVEN] > 1.9 && m[meter::DRIVEN] <= 2.0);
    assert!((m[meter::OUTPUT] - c as f32).abs() < 1.0e-6);
    // Quiet audio clears it after the window has passed (two seconds).
    let (_, _) = run(&mut h, sine(1000.0, 0.1), secs(2.2));
    let m = meters(&h);
    assert_eq!(m[meter::SHARE], 0.0);
    assert_eq!(m[meter::LONGEST], 0.0);
}

#[test]
fn bad_input_stays_finite_and_the_outputs_are_deterministic() {
    let mut h = clipper();
    h.set_param("oversample", 3.0);
    h.set_param("bands", 2.0);
    h.set_param("symmetry", 0.5);
    let x = |n: usize| match n % 7 {
        0 => f32::NAN,
        1 => f32::INFINITY,
        2 => f32::NEG_INFINITY,
        _ => 0.5,
    };
    let (l, r) = run(&mut h, x, 2048);
    assert!(l.iter().chain(&r).all(|v| v.is_finite()));
    let play = || {
        let mut h = clipper();
        h.set_param("oversample", 2.0);
        h.set_param("curve", 0.4);
        h.set_param("bands", 1.0);
        h.set_param("gain", 9.0);
        let x = noise(0.8, 4096, 99);
        run(&mut h, |n| x[n], x.len()).0
    };
    let (a, b) = (play(), play());
    assert!(a.iter().zip(&b).all(|(p, q)| p.to_bits() == q.to_bits()));
}

#[test]
fn every_param_extreme_stays_finite_and_never_allocates() {
    let mut h = clipper();
    sweep_params_min_max(&mut h, &DESCRIPTOR);
}

#[test]
fn changing_the_oversampling_does_not_allocate() {
    let mut h = clipper();
    let x = noise(0.9, 4096, 2);
    let mut at = 0;
    for k in 0..8 {
        atomik_testkit::no_alloc::assert_no_alloc(|| h.set_param("oversample", (k % 4) as f32));
        let (_, _) = run(&mut h, |n| x[(at + n) % x.len()], 512);
        at += 512;
    }
}
