//! EQ tests. Everything runs through `DeviceHarness`, which guards every
//! `process` call against allocation.

use super::*;
use crate::test_support::{
    SR, assert_keys, harness, run_signal, sine, sine_gain_db, sweep_params_min_max,
};
use atomik_dsp::Rng;
use atomik_testkit::harness::DeviceHarness;
use atomik_testkit::signal::max_step;
use proptest::prelude::{ProptestConfig, prop_assert, proptest};

const IN: [&str; 2] = ["in_l", "in_r"];

fn eq() -> DeviceHarness {
    harness(Box::new(Equalizer::new()))
}

fn eq_at(sr: f64) -> DeviceHarness {
    DeviceHarness::with_options(Box::new(Equalizer::new()), sr, 120.0, 0)
}

/// Gain in dB of a sine at `hz` (amplitude 0.25) through the EQ.
fn gain_at(h: &mut DeviceHarness, hz: f64) -> f64 {
    sine_gain_db(h, &IN, "out_l", hz, 0.25, 0.3)
}

fn set(h: &mut DeviceHarness, params: &[(&str, f32)]) {
    for (k, v) in params {
        h.set_param(k, *v);
    }
}

/// RBJ reference response of one section.
fn rbj_db(shape: BiquadShape, f0: f64, q: f64, gain_db: f64, f: f64) -> f64 {
    BiquadCoeffs::design(shape, f0, q, gain_db, SR).response_db(f, SR)
}

/// Butterworth response of order `n` on the bilinear-warped axis (independent of the biquads).
fn butter_db(low_pass: bool, order: u32, fc: f64, f: f64) -> f64 {
    let w = (std::f64::consts::PI * f / SR).tan() / (std::f64::consts::PI * fc / SR).tan();
    let x = if low_pass { w } else { 1.0 / w };
    -10.0 * (1.0 + x.powi(2 * order as i32)).log10()
}

const TEST_FREQS: [f64; 10] = [
    31.0, 63.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0,
];

#[test]
fn descriptor_validates() {
    assert_keys(
        &DESCRIPTOR,
        &[
            "band1.on",
            "band1.freq",
            "band1.q",
            "lowcut.slope",
            "band2.on",
            "band2.freq",
            "band2.gain",
            "band2.q",
            "band3.on",
            "band3.freq",
            "band3.gain",
            "band3.q",
            "band4.on",
            "band4.freq",
            "band4.gain",
            "band4.q",
            "band5.on",
            "band5.freq",
            "band5.gain",
            "band5.q",
            "band6.on",
            "band6.freq",
            "band6.q",
            "highcut.slope",
            "output",
            "band2.gain_cv_trim",
            "band3.gain_cv_trim",
            "band4.gain_cv_trim",
            "band5.gain_cv_trim",
            "bypass",
        ],
        &[
            "in_l",
            "in_r",
            "out_l",
            "out_r",
            "band2_gain_cv_in",
            "band3_gain_cv_in",
            "band4_gain_cv_in",
            "band5_gain_cv_in",
        ],
    );
    assert_eq!(DESCRIPTOR.type_id, "atomik.eq");
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Effect);
    assert_eq!(Equalizer::new().latency_frames(), 0);
}

#[test]
fn shelf_and_peak_responses_match_the_rbj_curves() {
    // (band key prefix, shape, f0, gain, q)
    let cases = [
        ("band2", BiquadShape::LowShelf, 200.0, 9.0, 0.707),
        ("band2", BiquadShape::LowShelf, 100.0, -12.0, 1.0),
        ("band3", BiquadShape::Peaking, 1000.0, 12.0, 2.0),
        ("band3", BiquadShape::Peaking, 3000.0, -9.0, 0.5),
        ("band4", BiquadShape::Peaking, 250.0, 6.0, 4.0),
        ("band5", BiquadShape::HighShelf, 4000.0, -8.0, 0.707),
        ("band5", BiquadShape::HighShelf, 8000.0, 10.0, 1.2),
    ];
    let mut worst = 0.0f64;
    for (band, shape, f0, g, q) in cases {
        let mut h = eq();
        set(
            &mut h,
            &[
                (&format!("{band}.freq"), f0 as f32),
                (&format!("{band}.gain"), g as f32),
                (&format!("{band}.q"), q as f32),
            ],
        );
        for f in TEST_FREQS {
            let got = gain_at(&mut h, f);
            let want = rbj_db(shape, f0, f64::from(q as f32), g, f);
            worst = worst.max((got - want).abs());
            assert!(
                (got - want).abs() < 0.1,
                "{shape:?} f0 {f0} gain {g} q {q} at {f} Hz: got {got:.3} want {want:.3}"
            );
        }
    }
    println!("shelf/peak worst error vs the RBJ response: {worst:.2e} dB");
}

#[test]
fn closed_form_landmarks() {
    // Peak: the centre gain is the gain. Shelves: half the gain at the corner (S = 1).
    let mut h = eq();
    set(
        &mut h,
        &[
            ("band3.freq", 1000.0),
            ("band3.gain", 10.0),
            ("band3.q", 3.0),
        ],
    );
    assert!((gain_at(&mut h, 1000.0) - 10.0).abs() < 0.05);
    let mut h = eq();
    set(&mut h, &[("band2.freq", 500.0), ("band2.gain", 12.0)]);
    assert!((gain_at(&mut h, 500.0) - 6.0).abs() < 0.1);
    assert!((gain_at(&mut h, 31.0) - 12.0).abs() < 0.2);
    let mut h = eq();
    set(&mut h, &[("band5.freq", 4000.0), ("band5.gain", -12.0)]);
    assert!((gain_at(&mut h, 4000.0) + 6.0).abs() < 0.15);
    assert!((gain_at(&mut h, 16000.0) + 12.0).abs() < 1.0);
}

fn check_cut(low: bool, slope: usize, fc: f64, freqs: &[f64]) -> f64 {
    let band = if low { "band1" } else { "band6" };
    let slope_key = if low { "lowcut.slope" } else { "highcut.slope" };
    let mut h = eq();
    set(
        &mut h,
        &[
            (&format!("{band}.on"), 1.0),
            (&format!("{band}.freq"), fc as f32),
            (slope_key, slope as f32),
        ],
    );
    let order = 2 * [1u32, 2, 4][slope];
    let mut worst = 0.0f64;
    for &f in freqs {
        let got = gain_at(&mut h, f);
        let want = butter_db(!low, order, fc, f);
        if want > -70.0 {
            worst = worst.max((got - want).abs());
            assert!(
                (got - want).abs() < 0.1,
                "low={low} slope {slope} fc {fc} at {f}: got {got:.3} want {want:.3}"
            );
        } else {
            assert!(got < -68.0, "low={low} slope {slope} at {f}: {got}");
        }
    }
    worst
}

#[test]
fn cut_filters_are_butterworth_at_every_slope() {
    let mut worst = 0.0f64;
    for slope in 0..3 {
        worst = worst.max(check_cut(
            true,
            slope,
            1000.0,
            &[250.0, 500.0, 800.0, 1000.0, 1250.0, 2000.0, 4000.0],
        ));
        worst = worst.max(check_cut(
            false,
            slope,
            2000.0,
            &[500.0, 1000.0, 1600.0, 2000.0, 2500.0, 4000.0, 8000.0],
        ));
    }
    println!("cut filters worst error vs Butterworth: {worst:.2e} dB");
}

#[test]
fn cut_slopes_are_12_24_48_db_per_octave() {
    // The attenuation gained between fc/2 and fc/4 (low cut) tends to the nominal slope.
    for (slope, nominal) in [(0usize, 12.0f64), (1, 24.0), (2, 48.0)] {
        let mut h = eq();
        set(
            &mut h,
            &[
                ("band1.on", 1.0),
                ("band1.freq", 1000.0),
                ("lowcut.slope", slope as f32),
            ],
        );
        let (a, b) = (gain_at(&mut h, 250.0), gain_at(&mut h, 125.0));
        println!("low cut slope {nominal}: {:.2} dB/oct", a - b);
        assert!(((a - b) - nominal).abs() < 0.6, "{nominal}: {}", a - b);
        // Bilinear warping steepens a high cut a little; allow 3 dB at 2 and 4 times the corner.
        let mut h = eq();
        set(
            &mut h,
            &[
                ("band6.on", 1.0),
                ("band6.freq", 1000.0),
                ("highcut.slope", slope as f32),
            ],
        );
        let (a, b) = (gain_at(&mut h, 2000.0), gain_at(&mut h, 4000.0));
        println!("high cut slope {nominal}: {:.2} dB/oct", a - b);
        assert!(((a - b) - nominal).abs() < 3.0, "{nominal}: {}", a - b);
    }
}

#[test]
fn cut_q_adds_resonance_at_the_corner() {
    let mut h = eq();
    set(
        &mut h,
        &[("band1.on", 1.0), ("band1.freq", 1000.0), ("band1.q", 4.0)],
    );
    let g = gain_at(&mut h, 1000.0);
    assert!((g - 12.04).abs() < 0.1, "{g}");
    let mut h = eq();
    set(
        &mut h,
        &[
            ("band6.on", 1.0),
            ("band6.freq", 2000.0),
            ("band6.q", 2.0),
            ("highcut.slope", 1.0),
        ],
    );
    // 24 dB/oct: the last section's Q is scaled by 2 / 0.7071, a clear peak near the corner.
    let g = gain_at(&mut h, 2000.0);
    assert!(g > 3.0, "{g}");
}

#[test]
fn off_bands_are_bit_transparent() {
    let mut rng = Rng::new(5);
    let x: Vec<[f32; BATCH]> = (0..60)
        .map(|_| std::array::from_fn(|_| rng.next_bipolar() * 0.9))
        .collect();
    let exact = |h: &mut DeviceHarness, skip: usize| {
        for (b, batch) in x.iter().enumerate() {
            *h.input_mut("in_l") = *batch;
            *h.input_mut("in_r") = batch.map(|v| -v);
            h.process();
            if b >= skip {
                for (i, v) in batch.iter().enumerate() {
                    assert_eq!(h.output("out_l")[i].to_bits(), v.to_bits());
                    assert_eq!(h.output("out_r")[i].to_bits(), (-v).to_bits());
                }
            }
        }
    };
    // Defaults: the cuts are off and the shelves and peaks have 0 dB gain.
    exact(&mut eq(), 0);
    // Every band off with big gains and extreme settings.
    let mut h = eq();
    for band in 1..=6 {
        h.set_param(&format!("band{band}.on"), 0.0);
    }
    set(
        &mut h,
        &[
            ("band2.gain", 24.0),
            ("band3.gain", -24.0),
            ("band4.gain", 24.0),
            ("band5.gain", 24.0),
            ("band1.freq", 2000.0),
            ("band6.freq", 1000.0),
        ],
    );
    exact(&mut h, 0);
    // A band that was on and is switched off returns to bit-exact once the fade has finished.
    let mut h = eq();
    set(&mut h, &[("band3.gain", 12.0), ("band1.on", 1.0)]);
    for _ in 0..10 {
        h.process();
    }
    set(&mut h, &[("band3.on", 0.0), ("band1.on", 0.0)]);
    for _ in 0..150 {
        h.process(); // the fade (10 ms time constant) takes about 100 ms to finish
    }
    exact(&mut h, 0);
    // Output gain is the only thing left: +6 dB is no longer exact.
    let mut h = eq();
    h.set_param("output", 6.0);
    *h.input_mut("in_l") = x[0];
    h.process();
    h.process();
    assert_ne!(h.output("out_l")[5].to_bits(), x[0][5].to_bits());
}

#[test]
fn parameter_changes_do_not_click() {
    // Bounds are the natural maximum step of a full-scale sine of the final amplitude.
    let click = |setup: &[(&str, f32)], change: &[(&str, f32)], hz: f64, bound: f32| {
        let mut h = eq();
        set(&mut h, setup);
        let src = sine(hz, 0.5);
        let mut all = Vec::new();
        for b in 0..160 {
            if b == 40 {
                set(&mut h, change);
            }
            let mut x = [0.0f32; BATCH];
            for (i, v) in x.iter_mut().enumerate() {
                *v = src(b * BATCH + i);
            }
            *h.input_mut("in_l") = x;
            *h.input_mut("in_r") = x;
            h.process();
            all.extend_from_slice(h.output("out_l"));
        }
        let step = max_step(&all[40 * BATCH - 8..]);
        assert!(step < bound, "{change:?}: max step {step} (bound {bound})");
    };
    // A +18 dB peak switched in at the signal frequency: amplitude 0.5 -> 4.0 over ~10 ms.
    click(
        &[("band3.freq", 500.0)],
        &[("band3.gain", 18.0)],
        500.0,
        0.4,
    );
    // A hard frequency jump of a +12 dB peak across the sine.
    click(
        &[("band3.freq", 500.0), ("band3.gain", 12.0)],
        &[("band3.freq", 4000.0)],
        500.0,
        0.4,
    );
    // Switching a low cut on and off.
    click(&[("band1.freq", 400.0)], &[("band1.on", 1.0)], 300.0, 0.1);
    click(
        &[("band1.freq", 400.0), ("band1.on", 1.0)],
        &[("band1.on", 0.0)],
        300.0,
        0.1,
    );
    // Changing a cut's slope while it is running.
    click(
        &[("band6.on", 1.0), ("band6.freq", 2000.0)],
        &[("highcut.slope", 2.0)],
        1500.0,
        0.15,
    );
}

#[test]
fn analyser_reports_the_band_energies() {
    let centres = analyser_centres(SR);
    assert!(centres.windows(2).all(|w| w[1] > w[0]));
    assert!((centres[0] - 40.0).abs() < 1e-6 && (centres[15] - 16_000.0).abs() < 1e-3);
    let mut h = eq();
    assert!(h.device().has_meter_values());
    // Silence reads zero.
    h.process();
    let mut out = [1.0f32; 32];
    assert_eq!(h.device().meter_values(&mut out), METER_COUNT);
    assert!(out[..METER_COUNT].iter().all(|v| v.to_bits() == 0));
    for k in [2usize, 7, 12] {
        let mut h = eq();
        let f = centres[k];
        let y = run_signal(
            &mut h,
            &IN,
            |n| (0.5 * (std::f64::consts::TAU * f * n as f64 / SR).sin()) as f32,
            400,
            &["out_l"],
        );
        let mut out = [0.0f32; 32];
        let n = h.device().meter_values(&mut out);
        assert_eq!(n, 18);
        let e = &out[..16];
        let top = e
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap()
            .0;
        assert_eq!(top, k, "{e:?}");
        // A sine of peak 0.5 at the centre: 0.125 (power), within 10 %.
        assert!((e[k] - 0.125).abs() < 0.0125, "band {k}: {}", e[k]);
        for other in [k - 1, k + 1] {
            assert!(
                e[other] < e[k] * 0.4,
                "band {other}: {} vs {}",
                e[other],
                e[k]
            );
        }
        let n = y[0].len();
        let peak = y[0][n - BATCH..].iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((out[16] - peak).abs() < 1e-6 && (out[17] - peak).abs() < 1e-6);
    }
    // A short buffer is filled partially.
    let mut small = [0.0f32; 4];
    assert_eq!(h.device().meter_values(&mut small), 4);
}

#[test]
fn analyser_follows_the_output_of_the_eq() {
    // A +24 dB peak on the tone lifts its band energy by about 24 dB.
    let measure = |gain: f32| {
        let mut h = eq();
        let f = analyser_centres(SR)[7];
        set(&mut h, &[("band3.freq", f as f32), ("band3.gain", gain)]);
        run_signal(
            &mut h,
            &IN,
            |n| (0.05 * (std::f64::consts::TAU * f * n as f64 / SR).sin()) as f32,
            600,
            &["out_l"],
        );
        let mut out = [0.0f32; 32];
        h.device().meter_values(&mut out);
        10.0 * f64::from(out[7]).log10()
    };
    let lift = measure(24.0) - measure(0.0);
    assert!((lift - 24.0).abs() < 1.0, "{lift}");
}

#[test]
fn non_finite_input_is_harmless() {
    let mut h = eq();
    set(&mut h, &[("band1.on", 1.0), ("band3.gain", 6.0)]);
    let mut bad = [0.1f32; BATCH];
    bad[3] = f32::NAN;
    bad[9] = f32::NEG_INFINITY;
    *h.input_mut("in_l") = bad;
    *h.input_mut("in_r") = bad;
    h.process();
    *h.input_mut("in_l") = [0.1; BATCH];
    *h.input_mut("in_r") = [0.1; BATCH];
    let o = h.render(50, &["out_l", "out_r"]);
    assert!(o.iter().flatten().all(|v| v.is_finite()));
    let mut m = [0.0f32; 32];
    h.device().meter_values(&mut m);
    assert!(m.iter().all(|v| v.is_finite()));
}

#[test]
fn is_deterministic_across_instances_and_resets() {
    let run = |h: &mut DeviceHarness| {
        let mut rng = Rng::new(8);
        let mut all = Vec::new();
        for b in 0..120 {
            let mut x = [0.0f32; BATCH];
            for v in &mut x {
                *v = rng.next_bipolar() * 0.5;
            }
            if b == 30 {
                set(
                    h,
                    &[
                        ("band3.gain", 9.0),
                        ("band1.on", 1.0),
                        ("lowcut.slope", 2.0),
                    ],
                );
            }
            if b == 70 {
                set(h, &[("band3.freq", 3000.0), ("band6.on", 1.0)]);
            }
            *h.input_mut("in_l") = x;
            *h.input_mut("in_r") = x;
            h.process();
            all.extend_from_slice(h.output("out_l"));
            all.extend_from_slice(h.output("out_r"));
        }
        let mut m = [0.0f32; 32];
        h.device().meter_values(&mut m);
        all.extend_from_slice(&m);
        all
    };
    let mut a = eq();
    let mut b = eq();
    let ya = run(&mut a);
    let yb = run(&mut b);
    assert!(ya.iter().zip(&yb).all(|(p, q)| p.to_bits() == q.to_bits()));
    a.device_mut().reset();
    set(
        &mut a,
        &[
            ("band3.gain", 0.0),
            ("band1.on", 0.0),
            ("lowcut.slope", 0.0),
            ("band3.freq", 500.0),
            ("band6.on", 0.0),
        ],
    );
    a.device_mut().reset();
    let yc = run(&mut a);
    assert!(ya.iter().zip(&yc).all(|(p, q)| p.to_bits() == q.to_bits()));
}

#[test]
fn params_sweep_stays_finite_at_all_rates() {
    for sr in [44_100.0, 48_000.0, 96_000.0] {
        let mut h = eq_at(sr);
        *h.input_mut("in_l") = [0.4; BATCH];
        *h.input_mut("in_r") = [-0.4; BATCH];
        sweep_params_min_max(&mut h, &DESCRIPTOR);
    }
}

#[test]
fn process_is_allocation_free_at_all_rates_with_automation() {
    for sr in [44_100.0, 48_000.0, 96_000.0] {
        let mut h = eq_at(sr);
        let mut rng = Rng::new(2);
        for i in 0..300 {
            let mut x = [0.0f32; BATCH];
            for v in &mut x {
                *v = rng.next_bipolar() * 0.5;
            }
            *h.input_mut("in_l") = x;
            *h.input_mut("in_r") = x;
            h.set_param("band1.on", (i / 40 % 2) as f32);
            h.set_param("lowcut.slope", (i / 25 % 3) as f32);
            h.set_param("band3.freq", 100.0 + (i % 50) as f32 * 300.0);
            h.set_param("band3.gain", (i % 25) as f32 - 12.0);
            h.set_param("band4.q", 0.3 + (i % 9) as f32);
            h.set_param("band5.gain", (i % 7) as f32 * 3.0);
            h.set_param("band6.on", (i / 30 % 2) as f32);
            h.set_param("highcut.slope", (i / 20 % 3) as f32);
            h.set_param("output", (i % 5) as f32);
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
        ons in proptest::collection::vec(proptest::bool::ANY, 6),
        freqs in proptest::collection::vec(20.0f32..20000.0, 6),
        gains in proptest::collection::vec(-24.0f32..24.0, 4),
        qs in proptest::collection::vec(0.1f32..18.0, 6),
        slopes in proptest::collection::vec(0u8..3, 2),
        output in -24.0f32..24.0,
        seed in 0u64..100,
    ) {
        let mut h = eq();
        for b in 0..6 {
            let n = b + 1;
            h.set_param(&format!("band{n}.on"), f32::from(ons[b]));
            h.set_param(&format!("band{n}.freq"), freqs[b]);
            h.set_param(&format!("band{n}.q"), qs[b]);
        }
        for (i, b) in [2, 3, 4, 5].iter().enumerate() {
            h.set_param(&format!("band{b}.gain"), gains[i]);
        }
        h.set_param("lowcut.slope", f32::from(slopes[0]));
        h.set_param("highcut.slope", f32::from(slopes[1]));
        h.set_param("output", output);
        let mut rng = Rng::new(seed);
        for _ in 0..60 {
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
                    // Four +24 dB bands (x16 each is bounded far below 1e6) and resonant cuts.
                    prop_assert!(v.abs() < 1.0e6, "{}", v);
                }
            }
        }
        let mut m = [0.0f32; 32];
        h.device().meter_values(&mut m);
        prop_assert!(m.iter().all(|v| v.is_finite()));
    }
}

/// Gain in dB at 1 kHz with peak 1 at `gain` dB / `trim` and `cv` on `band3_gain_cv_in`.
fn peak_cv_gain(gain: f32, trim: f32, cv: Option<f32>) -> f64 {
    let mut h = eq();
    set(
        &mut h,
        &[
            ("band3.freq", 1_000.0),
            ("band3.gain", gain),
            ("band3.gain_cv_trim", trim),
        ],
    );
    if let Some(c) = cv {
        *h.input_mut("band3_gain_cv_in") = [c; BATCH];
    }
    gain_at(&mut h, 1_000.0)
}

#[test]
fn gain_cv_adds_cv_times_trim_times_24_db_and_clamps() {
    // cv 0.25 * 24 dB = +6 dB on a flat band.
    assert!((peak_cv_gain(0.0, 1.0, Some(0.25)) - 6.0).abs() < 0.1);
    // Adds to the knob: 6 dB - 6 dB = flat.
    assert!(peak_cv_gain(6.0, 1.0, Some(-0.25)).abs() < 0.1);
    // Half trim: 0.5 * 0.5 * 24 = 6 dB; negative trim inverts.
    assert!((peak_cv_gain(0.0, 0.5, Some(0.5)) - 6.0).abs() < 0.1);
    assert!((peak_cv_gain(0.0, -1.0, Some(0.25)) + 6.0).abs() < 0.1);
    // CV beyond +-1 clamps to +-24 dB; knob + CV clamps to the +-24 dB range.
    assert!((peak_cv_gain(0.0, 1.0, Some(5.0)) - 24.0).abs() < 0.2);
    assert!((peak_cv_gain(12.0, 1.0, Some(1.0)) - 24.0).abs() < 0.2);
    assert!((peak_cv_gain(0.0, 1.0, Some(-5.0)) + 24.0).abs() < 0.2);
    // Amount 0 cancels the CV.
    assert!(peak_cv_gain(0.0, 0.0, Some(1.0)).abs() < 1e-3);
}

#[test]
fn gain_cv_works_on_every_gain_band_and_ignores_the_cut_bands() {
    for (band, hz) in [(2, 100.0), (3, 500.0), (4, 2_500.0), (5, 12_000.0)] {
        let mut h = eq();
        *h.input_mut(&format!("band{band}_gain_cv_in")) = [0.25; BATCH];
        let g = gain_at(&mut h, hz);
        assert!(g > 3.0, "band {band} moved by {g} dB");
    }
}

#[test]
fn gain_cv_is_allocation_free_and_finite() {
    let mut h = eq();
    for i in 0..300 {
        for b in 2..=5 {
            *h.input_mut(&format!("band{b}_gain_cv_in")) =
                [((i + b) % 9) as f32 * 0.7 - 2.8; BATCH];
        }
        *h.input_mut("in_l") = [0.2; BATCH];
        h.process();
        assert!(h.output("out_l").iter().all(|v| v.is_finite()));
    }
}
