//! Distortion tests. Everything runs through `DeviceHarness`, which guards every
//! `process` call against allocation.

use super::*;
use crate::test_support::{
    assert_keys, harness, run_signal, sine, sine_gain_db, sweep_params_min_max,
};
use atomik_dsp::Rng;
use atomik_testkit::harness::DeviceHarness;
use atomik_testkit::signal::magnitude_spectrum;
use proptest::prelude::{ProptestConfig, prop_assert, proptest};

const IN: [&str; 2] = ["in_l", "in_r"];

fn dist() -> DeviceHarness {
    harness(Box::new(Distortion::new()))
}

fn dist_at(sr: f64) -> DeviceHarness {
    DeviceHarness::with_options(Box::new(Distortion::new()), sr, 120.0, 0)
}

fn set(h: &mut DeviceHarness, params: &[(&str, f32)]) {
    for (k, v) in params {
        h.set_param(k, *v);
    }
}

#[test]
fn descriptor_validates() {
    assert_keys(
        &DESCRIPTOR,
        &[
            "curve",
            "drive",
            "bias",
            "tone",
            "output",
            "mix",
            "crush.bits",
            "crush.rate",
            "oversample",
            "drive_cv_trim",
            "mix_cv_trim",
        ],
        &["in_l", "in_r", "out_l", "out_r", "drive_cv_in", "mix_cv_in"],
    );
    assert_eq!(DESCRIPTOR.type_id, "atomik.distortion");
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Effect);
    let p = DESCRIPTOR.param_by_key("oversample").unwrap();
    assert_eq!(p.default, 2.0, "4x by default");
}

/// The output of the first sample after a step from rest (the DC blocker has not acted yet), so
/// it is the pure curve value `curve(x * drive + bias) - curve(bias)`.
fn first_sample(params: &[(&str, f32)], x: f32) -> f32 {
    let mut h = dist();
    set(&mut h, &[("oversample", 0.0), ("mix", 1.0)]);
    set(&mut h, params);
    *h.input_mut("in_l") = [x; BATCH];
    *h.input_mut("in_r") = [x; BATCH];
    h.process();
    h.output("out_l")[0]
}

#[test]
fn curves_match_their_formulas() {
    let drive_db = 6.0206f32; // x2
    let g = 2.0f32;
    for x in [0.1f32, 0.3, 0.5, 0.8, 1.2, 1.7] {
        let u = x * g;
        let soft = first_sample(&[("curve", 0.0), ("drive", drive_db)], x);
        assert!((soft - u / (1.0 + u * u).sqrt()).abs() < 2e-4, "soft {x}");
        let hard = first_sample(&[("curve", 1.0), ("drive", drive_db)], x);
        assert!((hard - u.clamp(-1.0, 1.0)).abs() < 2e-4, "hard {x}");
        let tube = first_sample(&[("curve", 2.0), ("drive", drive_db)], x);
        assert!((tube - u / (1.0 + u * u).sqrt()).abs() < 2e-4, "tube+ {x}");
        let tube_n = first_sample(&[("curve", 2.0), ("drive", drive_db)], -x);
        let v = 1.25 * u;
        assert!(
            (tube_n - (-u) / (1.0 + v * v).sqrt()).abs() < 2e-4,
            "tube- {x}"
        );
        assert!(tube_n.abs() < tube.abs(), "negative side saturates lower");
    }
    // Foldback reflects at +-1: u = 1.5 -> 0.5, 2.5 -> -0.5, 3.5 -> -0.5 (triangle).
    for (x, want) in [
        (0.3f32, 0.3f32),
        (0.75, 0.75),
        (1.5, 0.5),
        (2.5, -0.5),
        (3.5, -0.5),
        (-1.5, -0.5),
    ] {
        let y = first_sample(&[("curve", 3.0), ("drive", 0.0)], x);
        assert!((y - want).abs() < 2e-4, "fold {x}: {y} vs {want}");
    }
    // Small-signal gain is unity for the three smooth curves.
    for c in [0.0f32, 1.0, 3.0] {
        let y = first_sample(&[("curve", c), ("drive", 0.0)], 0.01);
        assert!((y - 0.01).abs() < 1e-4, "curve {c}: {y}");
    }
    // The tube curve has a different positive and negative ceiling.
    let hi = first_sample(&[("curve", 2.0), ("drive", 40.0)], 1.0);
    let lo = first_sample(&[("curve", 2.0), ("drive", 40.0)], -1.0);
    assert!(hi > 0.99 && (lo + 0.8).abs() < 0.01, "{hi} {lo}");
}

#[test]
fn bitcrush_quantises_and_holds() {
    // Quantisation: 3 bits -> 4 levels per unit.
    for x in [0.1f32, 0.37, 0.6, -0.9] {
        let y = first_sample(&[("curve", 4.0), ("crush.bits", 3.0), ("drive", 0.0)], x);
        let want = (x * 4.0).round() / 4.0;
        assert!((y - want).abs() < 1e-6, "{x}: {y} vs {want}");
    }
    // Sample-and-hold at sr / 4: groups of four equal samples (the DC blocker leaves only a tiny
    // droop inside a group).
    let mut h = dist();
    set(
        &mut h,
        &[
            ("curve", 4.0),
            ("crush.bits", 16.0),
            ("crush.rate", 12_000.0),
            ("drive", 0.0),
            ("oversample", 0.0),
        ],
    );
    let y = run_signal(&mut h, &IN, sine(700.0, 0.5), 20, &["out_l"]);
    let y = &y[0];
    let mut within = 0.0f32;
    let mut across = 0.0f32;
    // The hold updates on samples 3, 7, 11, ... (the counter starts at 0).
    for g in 10..250 {
        let o = 3 + g * 4;
        within = within.max((y[o + 3] - y[o]).abs());
        across = across.max((y[o + 4] - y[o + 3]).abs());
    }
    println!("hold: max step inside a group {within:.5}, between groups {across:.4}");
    assert!(within < 0.002 && across > 0.02, "{within} {across}");
}

#[test]
fn tube_makes_even_harmonics_and_soft_clip_does_not() {
    let harmonic = |curve: f32, k: usize| {
        let mut h = dist();
        set(&mut h, &[("curve", curve), ("drive", 18.0)]);
        let y = run_signal(&mut h, &IN, sine(1000.0, 0.5), 800, &["out_l"]);
        let seg = &y[0][24_000..24_000 + 24_000];
        let m = magnitude_spectrum(seg);
        let bin = |hz: usize| {
            m[hz / 2 - 1..=hz / 2 + 1]
                .iter()
                .fold(0.0f64, |a, b| a.max(*b))
        };
        20.0 * (bin(1000 * k) / bin(1000)).log10()
    };
    let tube2 = harmonic(2.0, 2);
    let soft2 = harmonic(0.0, 2);
    let soft3 = harmonic(0.0, 3);
    println!("2nd harmonic: tube {tube2:.1} dBc, soft {soft2:.1} dBc; soft 3rd {soft3:.1} dBc");
    assert!(tube2 > -40.0, "{tube2}");
    assert!(soft2 < -90.0, "{soft2}");
    assert!(soft3 > -30.0, "{soft3}");
}

/// Alias energy (power in the bins that are not a harmonic of the tone, from 20 Hz to 19.2 kHz =
/// 0.4 fs, the oversampler's passband: a harmonic that lands between 0.4 fs and 0.6 fs is only
/// partly filtered and reflects to just below Nyquist) relative to the fundamental, in dB, for a
/// 5 kHz sine through `curve` at `drive`.
fn alias_dbc(oversample: f32, curve: f32, drive: f32) -> f64 {
    let mut h = dist();
    set(
        &mut h,
        &[
            ("curve", curve),
            ("drive", drive),
            ("oversample", oversample),
        ],
    );
    let y = run_signal(&mut h, &IN, sine(5000.0, 0.5), 1_000, &["out_l"]);
    // 24000 samples (2 Hz bins) = 2500 whole cycles of 5 kHz.
    let seg = &y[0][24_000..48_000];
    let m = magnitude_spectrum(seg);
    let mut fund = 0.0f64;
    let mut alias = 0.0f64;
    for (k, v) in m.iter().enumerate() {
        let nearest = (k as f64 / 2500.0).round() as usize * 2500;
        let near = k.abs_diff(nearest) <= 4;
        if near && nearest == 2500 {
            fund += v * v;
        } else if !near && k > 10 && k <= 9_600 {
            alias += v * v;
        }
    }
    10.0 * (alias / fund).log10()
}

#[test]
fn oversampling_cuts_aliasing_of_a_hard_clipped_5khz_sine() {
    // +24 dB of drive into a hard clip: nearly a square wave.
    let off = alias_dbc(0.0, 1.0, 24.0);
    let x2 = alias_dbc(1.0, 1.0, 24.0);
    let x4 = alias_dbc(2.0, 1.0, 24.0);
    println!(
        "hard clip +24 dB, 5 kHz at 48 kHz: alias {off:.1} dBc (off), {x2:.1} dBc (2x), {x4:.1} dBc (4x); reduction {:.1} dB (2x), {:.1} dB (4x)",
        off - x2,
        off - x4
    );
    // A hard clip has a slowly decaying (1/k^2) harmonic tail, so oversampling helps less than
    // for the smooth curves; the stated levels are what the chain achieves (measured above:
    // 15.0 dB for 2x and 24.7 dB for 4x, -39 dBc).
    assert!(x2 < off - 12.0, "2x: {} dB lower", off - x2);
    assert!(x4 < off - 20.0, "4x: {} dB lower", off - x4);
    assert!(x4 < x2, "4x is cleaner than 2x");
    assert!(x4 < -35.0, "4x alias level {x4} dBc");
    // The same order of reduction for the other curves, and more at a moderate drive.
    for (curve, min_gain) in [(0.0f32, 30.0), (2.0, 30.0), (3.0, 20.0)] {
        let a = alias_dbc(0.0, curve, 24.0);
        let b = alias_dbc(2.0, curve, 24.0);
        println!(
            "curve {curve}: {a:.1} dBc off, {b:.1} dBc 4x ({:.1} dB lower)",
            a - b
        );
        assert!(b < a - min_gain, "curve {curve}: {a} vs {b}");
    }
    for curve in [0.0f32, 2.0] {
        let a = alias_dbc(0.0, curve, 12.0);
        let b = alias_dbc(2.0, curve, 12.0);
        println!(
            "curve {curve} at +12 dB: {a:.1} dBc off, {b:.1} dBc 4x ({:.1} dB lower)",
            a - b
        );
        assert!(b < a - 50.0, "curve {curve} at +12 dB: {a} vs {b}");
    }
}

#[test]
fn dc_is_removed() {
    // A bias plus an asymmetric curve generates DC; a DC input is blocked as well.
    let mut h = dist();
    set(&mut h, &[("curve", 2.0), ("bias", 0.4), ("drive", 12.0)]);
    let y = run_signal(&mut h, &IN, sine(300.0, 0.4), 750, &["out_l"]);
    let seg = &y[0][24_000..];
    let mean = seg.iter().map(|v| f64::from(*v)).sum::<f64>() / seg.len() as f64;
    println!("mean after tube + bias: {mean:.2e}");
    assert!(mean.abs() < 2e-3, "{mean}");
    let mut h = dist();
    set(&mut h, &[("curve", 0.0), ("drive", 0.0)]);
    let y = run_signal(&mut h, &IN, |_| 0.3, 750, &["out_l"]);
    let first = y[0][..100].iter().fold(0.0f32, |m, v| m.max(*v));
    assert!(first > 0.2, "the step passes at first: {first}");
    let tail = y[0][40_000..].iter().fold(0.0f32, |m, v| m.max(v.abs()));
    assert!(tail < 1e-3, "DC still present: {tail}");
}

#[test]
fn tone_tilts_the_spectrum() {
    let gain = |tone: f32, hz: f64| {
        let mut h = dist();
        set(&mut h, &[("curve", 0.0), ("drive", 0.0), ("tone", tone)]);
        sine_gain_db(&mut h, &IN, "out_l", hz, 0.01, 0.3)
    };
    assert!(gain(0.0, 100.0).abs() < 0.1 && gain(0.0, 8000.0).abs() < 0.1);
    let (lo, hi) = (gain(1.0, 100.0), gain(1.0, 8000.0));
    println!("tone +1: {lo:.2} dB at 100 Hz, {hi:+.2} dB at 8 kHz");
    assert!(lo < -4.5 && lo > -6.5, "{lo}");
    assert!(hi > 4.5 && hi < 6.5, "{hi}");
    let (lo, hi) = (gain(-1.0, 100.0), gain(-1.0, 8000.0));
    assert!(lo > 4.5 && hi < -4.5, "{lo} {hi}");
}

#[test]
fn latency_equals_the_impulse_delay_for_every_setting_and_rate() {
    for sr in [44_100.0f64, 48_000.0, 96_000.0] {
        for (os, want) in [(0.0f32, 0usize), (1.0, 29), (2.0, 35)] {
            for curve in [0.0f32, 1.0, 3.0, 4.0] {
                let mut h = dist_at(sr);
                set(
                    &mut h,
                    &[
                        ("curve", curve),
                        ("drive", 0.0),
                        ("oversample", os),
                        ("crush.bits", 16.0),
                    ],
                );
                assert_eq!(h.device().latency_frames() as usize, want);
                let mut x = vec![0.0f32; 512];
                x[9] = 0.1;
                let mut out = Vec::new();
                for chunk in x.chunks(BATCH) {
                    let mut b = [0.0f32; BATCH];
                    b.copy_from_slice(chunk);
                    *h.input_mut("in_l") = b;
                    *h.input_mut("in_r") = b;
                    h.process();
                    out.extend_from_slice(h.output("out_l"));
                }
                let at = out
                    .iter()
                    .enumerate()
                    .fold(
                        (0, 0.0f32),
                        |m, (i, v)| if v.abs() > m.1 { (i, v.abs()) } else { m },
                    )
                    .0;
                assert_eq!(at, 9 + want, "sr {sr} os {os} curve {curve}");
            }
        }
    }
}

#[test]
fn mix_zero_is_the_aligned_dry_input_bit_exact() {
    for (os, lat) in [(0.0f32, 0usize), (1.0, 29), (2.0, 35)] {
        let mut h = dist();
        set(
            &mut h,
            &[
                ("mix", 0.0),
                ("oversample", os),
                ("drive", 40.0),
                ("curve", 3.0),
            ],
        );
        let mut rng = Rng::new(4);
        let x: Vec<f32> = (0..BATCH * 30).map(|_| rng.next_bipolar() * 0.7).collect();
        let mut out = Vec::new();
        for chunk in x.chunks(BATCH) {
            let mut b = [0.0f32; BATCH];
            b.copy_from_slice(chunk);
            *h.input_mut("in_l") = b;
            *h.input_mut("in_r") = b;
            h.process();
            out.extend_from_slice(h.output("out_l"));
        }
        for i in lat..out.len() {
            assert_eq!(out[i].to_bits(), x[i - lat].to_bits(), "os {os} i {i}");
        }
    }
}

#[test]
fn mix_blends_the_aligned_dry_and_wet() {
    let run = |mix: f32| {
        let mut h = dist();
        set(&mut h, &[("mix", mix), ("drive", 20.0), ("curve", 1.0)]);
        run_signal(&mut h, &IN, sine(440.0, 0.5), 400, &["out_l"]).remove(0)
    };
    let (wet, half, dry) = (run(1.0), run(0.5), run(0.0));
    for i in 20_000..25_000 {
        let want = 0.5 * dry[i] + 0.5 * wet[i];
        assert!((half[i] - want).abs() < 1e-5, "i {i}");
    }
}

#[test]
fn output_gain_scales_the_wet_signal() {
    let level = |out_db: f32| {
        let mut h = dist();
        set(
            &mut h,
            &[("curve", 1.0), ("drive", 30.0), ("output", out_db)],
        );
        let y = run_signal(&mut h, &IN, sine(1000.0, 0.5), 400, &["out_l"]);
        y[0][20_000..].iter().fold(0.0f32, |m, v| m.max(v.abs()))
    };
    let (a, b) = (level(0.0), level(-12.0));
    assert!((20.0 * (a / b).log10() - 12.0).abs() < 0.2, "{a} {b}");
}

#[test]
fn meter_values_report_input_and_output_peaks() {
    let mut h = dist();
    assert!(h.device().has_meter_values());
    set(&mut h, &[("curve", 1.0), ("drive", 24.0)]);
    *h.input_mut("in_l") = [0.25; BATCH];
    *h.input_mut("in_r") = [-0.4; BATCH];
    for _ in 0..100 {
        h.process();
    }
    let mut out = [0.0f32; 32];
    assert_eq!(h.device().meter_values(&mut out), 2);
    assert!((out[0] - 0.4).abs() < 1e-6);
    assert!(out[1] > 0.0 && out[1] <= 1.2, "{}", out[1]);
}

#[test]
fn non_finite_input_is_harmless() {
    let mut h = dist();
    let mut bad = [0.1f32; BATCH];
    bad[3] = f32::NAN;
    bad[9] = f32::INFINITY;
    *h.input_mut("in_l") = bad;
    *h.input_mut("in_r") = bad;
    h.process();
    *h.input_mut("in_l") = [0.1; BATCH];
    *h.input_mut("in_r") = [0.1; BATCH];
    let o = h.render(80, &["out_l", "out_r"]);
    assert!(o.iter().flatten().all(|v| v.is_finite()));
}

#[test]
fn oversample_changes_stay_finite_and_report_the_new_latency() {
    let mut h = dist();
    let mut rng = Rng::new(6);
    for i in 0..60 {
        let mut x = [0.0f32; BATCH];
        for v in &mut x {
            *v = rng.next_bipolar();
        }
        *h.input_mut("in_l") = x;
        *h.input_mut("in_r") = x;
        h.set_param("oversample", (i % 3) as f32);
        assert_eq!(h.device().latency_frames(), [0, 29, 35][i % 3]);
        h.process();
        assert!(h.output("out_l").iter().all(|v| v.is_finite()));
    }
}

#[test]
fn is_deterministic_across_instances_and_resets() {
    let run = |h: &mut DeviceHarness| {
        let mut rng = Rng::new(12);
        let mut all = Vec::new();
        for b in 0..100 {
            let mut x = [0.0f32; BATCH];
            for v in &mut x {
                *v = rng.next_bipolar() * 0.8;
            }
            if b == 30 {
                set(
                    h,
                    &[
                        ("curve", 3.0),
                        ("tone", 0.5),
                        ("bias", 0.2),
                        ("oversample", 1.0),
                    ],
                );
            }
            if b == 60 {
                set(
                    h,
                    &[("curve", 4.0), ("crush.bits", 5.0), ("crush.rate", 9_000.0)],
                );
            }
            *h.input_mut("in_l") = x;
            *h.input_mut("in_r") = x;
            h.process();
            all.extend_from_slice(h.output("out_l"));
            all.extend_from_slice(h.output("out_r"));
        }
        all
    };
    let mut a = dist();
    let mut b = dist();
    let ya = run(&mut a);
    let yb = run(&mut b);
    assert!(ya.iter().zip(&yb).all(|(p, q)| p.to_bits() == q.to_bits()));
    set(
        &mut a,
        &[
            ("curve", 0.0),
            ("tone", 0.0),
            ("bias", 0.0),
            ("oversample", 2.0),
            ("crush.bits", 8.0),
            ("crush.rate", 96_000.0),
        ],
    );
    a.device_mut().reset();
    let yc = run(&mut a);
    assert!(ya.iter().zip(&yc).all(|(p, q)| p.to_bits() == q.to_bits()));
}

#[test]
fn params_sweep_stays_finite_at_all_rates() {
    for sr in [44_100.0, 48_000.0, 96_000.0] {
        let mut h = dist_at(sr);
        *h.input_mut("in_l") = [0.4; BATCH];
        *h.input_mut("in_r") = [-0.4; BATCH];
        sweep_params_min_max(&mut h, &DESCRIPTOR);
    }
}

#[test]
fn process_is_allocation_free_at_all_rates_with_automation() {
    for sr in [44_100.0, 48_000.0, 96_000.0] {
        let mut h = dist_at(sr);
        let mut rng = Rng::new(3);
        for i in 0..300 {
            let mut x = [0.0f32; BATCH];
            for v in &mut x {
                *v = rng.next_bipolar() * 0.5;
            }
            *h.input_mut("in_l") = x;
            *h.input_mut("in_r") = x;
            h.set_param("curve", (i / 20 % 5) as f32);
            h.set_param("drive", (i % 49) as f32);
            h.set_param("bias", ((i % 21) as f32 - 10.0) / 10.0);
            h.set_param("tone", ((i % 11) as f32 - 5.0) / 5.0);
            h.set_param("oversample", (i / 7 % 3) as f32);
            h.set_param("crush.bits", 1.0 + (i % 16) as f32);
            h.set_param("crush.rate", 500.0 + (i % 40) as f32 * 2_000.0);
            h.set_param("mix", (i % 5) as f32 / 4.0);
            h.process(); // guarded by the harness
        }
        h.device_mut().reset();
        h.process();
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    #[test]
    fn output_is_finite_and_bounded_for_any_params(
        curve in 0u8..5,
        drive in 0.0f32..48.0,
        bias in -1.0f32..1.0,
        tone in -1.0f32..1.0,
        output in -24.0f32..12.0,
        mix in 0.0f32..1.0,
        bits in 1.0f32..16.0,
        rate in 500.0f32..96000.0,
        os in 0u8..3,
        seed in 0u64..100,
    ) {
        let mut h = dist();
        for (k, v) in [
            ("curve", f32::from(curve)), ("drive", drive), ("bias", bias), ("tone", tone),
            ("output", output), ("mix", mix), ("crush.bits", bits), ("crush.rate", rate),
            ("oversample", f32::from(os)),
        ] {
            h.set_param(k, v);
        }
        let mut rng = Rng::new(seed);
        for _ in 0..40 {
            let mut x = [0.0f32; BATCH];
            for v in &mut x {
                *v = rng.next_bipolar();
            }
            *h.input_mut("in_l") = x;
            *h.input_mut("in_r") = x;
            h.process();
            for k in ["out_l", "out_r"] {
                for v in h.output(k) {
                    prop_assert!(v.is_finite());
                    // Curves <= 1, +12 dB output, DC blocker and tilt headroom, plus the dry input.
                    prop_assert!(v.abs() < 30.0, "{}", v);
                }
            }
        }
    }
}

fn max_diff(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0, f32::max)
}

/// Left output for a 440 Hz sine with the given params and optional constant CVs
/// (`drive_cv_in`, `mix_cv_in`).
fn cv_run(params: &[(&str, f32)], drive_cv: Option<f32>, mix_cv: Option<f32>) -> Vec<f32> {
    let mut h = dist();
    for (k, v) in params {
        h.set_param(k, *v);
    }
    if let Some(c) = drive_cv {
        *h.input_mut("drive_cv_in") = [c; BATCH];
    }
    if let Some(c) = mix_cv {
        *h.input_mut("mix_cv_in") = [c; BATCH];
    }
    run_signal(&mut h, &IN, sine(440.0, 0.3), 40, &["out_l"])[0].clone()
}

#[test]
fn mix_cv_adds_to_mix_scaled_by_trim_and_clamps() {
    let p = |m: f32, t: f32| [("mix", m), ("mix_cv_trim", t)];
    let a = cv_run(&p(0.2, 1.0), None, Some(0.5));
    assert!(max_diff(&a, &cv_run(&p(0.7, 1.0), None, None)) < 1e-4);
    assert!(
        max_diff(&a, &cv_run(&p(0.2, 1.0), None, None)) > 1e-3,
        "CV has an effect"
    );
    let h = cv_run(&p(0.2, 0.5), None, Some(0.5));
    assert!(max_diff(&h, &cv_run(&p(0.45, 1.0), None, None)) < 1e-4);
    assert_eq!(
        cv_run(&p(0.2, 1.0), None, Some(5.0)),
        cv_run(&p(0.2, 1.0), None, Some(1.0))
    );
    assert!(
        max_diff(
            &cv_run(&p(0.5, -1.0), None, Some(1.0)),
            &cv_run(&p(0.0, 1.0), None, None)
        ) < 1e-4
    );
    assert_eq!(
        cv_run(&p(0.3, 0.0), None, Some(1.0)),
        cv_run(&p(0.3, 1.0), None, None)
    );
}

#[test]
fn drive_cv_adds_in_normalised_space_over_0_to_48_db_and_clamps() {
    let p = |d: f32, t: f32| [("drive", d), ("drive_cv_trim", t)];
    // 12 dB = 0.25 normalised; + 0.25 -> 0.5 -> 24 dB.
    let a = cv_run(&p(12.0, 1.0), Some(0.25), None);
    assert!(max_diff(&a, &cv_run(&p(24.0, 1.0), None, None)) < 1e-3);
    assert!(
        max_diff(&a, &cv_run(&p(12.0, 1.0), None, None)) > 1e-2,
        "CV has an effect"
    );
    // Half trim: 0.25 + 0.5 * 0.5 = 0.5 -> 24 dB.
    let h = cv_run(&p(12.0, 0.5), Some(0.5), None);
    assert!(max_diff(&h, &cv_run(&p(24.0, 1.0), None, None)) < 1e-3);
    // Clamped: cv 5 == cv 1, and the sum clamps at 48 dB / 0 dB.
    assert_eq!(
        cv_run(&p(12.0, 1.0), Some(5.0), None),
        cv_run(&p(12.0, 1.0), Some(1.0), None)
    );
    assert!(
        max_diff(
            &cv_run(&p(12.0, 1.0), Some(1.0), None),
            &cv_run(&p(48.0, 1.0), None, None)
        ) < 1e-3
    );
    assert!(
        max_diff(
            &cv_run(&p(12.0, -1.0), Some(1.0), None),
            &cv_run(&p(0.0, 1.0), None, None)
        ) < 1e-3
    );
    // Amount 0 cancels.
    assert_eq!(
        cv_run(&p(12.0, 0.0), Some(1.0), None),
        cv_run(&p(12.0, 1.0), None, None)
    );
}

#[test]
fn cv_inputs_are_allocation_free_and_finite() {
    let mut h = dist();
    for i in 0..300 {
        *h.input_mut("drive_cv_in") = [((i % 9) as f32 - 4.0) * 0.7; BATCH];
        *h.input_mut("mix_cv_in") = [((i % 7) as f32 - 3.0) * 0.7; BATCH];
        *h.input_mut("in_l") = [0.2; BATCH];
        h.process();
        assert!(h.output("out_l").iter().all(|v| v.is_finite()));
    }
}
