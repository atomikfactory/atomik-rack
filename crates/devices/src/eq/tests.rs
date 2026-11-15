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

/// An EQ with its analyser on (a panel with the display is on screen).
fn eq_live() -> DeviceHarness {
    let mut h = eq();
    h.set_param("internal.analyser_live", 1.0);
    h
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
            "internal.analyser_live",
            "band7.on",
            "band7.freq",
            "band7.gain",
            "band7.q",
            "band2.shape",
            "band3.shape",
            "band4.shape",
            "band5.shape",
            "band7.shape",
            "band7.gain_cv_trim",
            "stereo.mode",
            "spectrum.on",
            "display.range",
            "band1.freq_cv_trim",
            "band2.freq_cv_trim",
            "band3.freq_cv_trim",
            "band4.freq_cv_trim",
            "band5.freq_cv_trim",
            "band6.freq_cv_trim",
            "band7.freq_cv_trim",
            "band1.q_cv_trim",
            "band2.q_cv_trim",
            "band3.q_cv_trim",
            "band4.q_cv_trim",
            "band5.q_cv_trim",
            "band6.q_cv_trim",
            "band7.q_cv_trim",
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
            "band7_gain_cv_in",
            "band1_freq_cv_in",
            "band2_freq_cv_in",
            "band3_freq_cv_in",
            "band4_freq_cv_in",
            "band5_freq_cv_in",
            "band6_freq_cv_in",
            "band7_freq_cv_in",
            "band1_q_cv_in",
            "band2_q_cv_in",
            "band3_q_cv_in",
            "band4_q_cv_in",
            "band5_q_cv_in",
            "band6_q_cv_in",
            "band7_q_cv_in",
        ],
    );
    // Params keep their ids (the index) and the first 31 params and 8 sockets keep their keys.
    for (i, p) in DESCRIPTOR.params.iter().enumerate() {
        assert_eq!(p.id.0 as usize, i);
    }
    assert_eq!(DESCRIPTOR.params[30].key, "internal.analyser_live");
    assert_eq!(DESCRIPTOR.sockets[7].key, "band5_gain_cv_in");
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
    let mut h = eq_live();
    assert!(h.device().has_meter_values());
    // Silence reads zero.
    h.process();
    let mut out = [1.0f32; 32];
    assert_eq!(h.device().meter_values(&mut out), METER_COUNT);
    assert!(out[..METER_COUNT].iter().all(|v| v.to_bits() == 0));
    for k in [2usize, 7, 12] {
        let mut h = eq_live();
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
        let mut h = eq_live();
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
    let mut a = eq_live();
    let mut b = eq_live();
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

/// Noise through an EQ with a few bands moving, returning both outputs and the meter values.
fn noise_run(h: &mut DeviceHarness) -> (Vec<f32>, [f32; METER_COUNT]) {
    set(h, &[("band3.gain", 7.0), ("band1.on", 1.0)]);
    let mut rng = Rng::new(21);
    let mut all = Vec::new();
    for _ in 0..80 {
        let mut x = [0.0f32; BATCH];
        for v in &mut x {
            *v = rng.next_bipolar() * 0.5;
        }
        *h.input_mut("in_l") = x;
        *h.input_mut("in_r") = x;
        h.process();
        all.extend_from_slice(h.output("out_l"));
        all.extend_from_slice(h.output("out_r"));
    }
    let mut m = [0.0f32; METER_COUNT];
    h.device().meter_values(&mut m);
    (all, m)
}

#[test]
fn the_analyser_is_off_by_default_and_the_audio_never_depends_on_it() {
    let (off, off_meters) = noise_run(&mut eq());
    let (on, on_meters) = noise_run(&mut eq_live());
    assert!(off.iter().zip(&on).all(|(a, b)| a.to_bits() == b.to_bits()));
    // Off: the band energies read zero, the output peaks still move.
    assert!(off_meters[..ANALYSER_BANDS].iter().all(|v| *v == 0.0));
    assert!(off_meters[ANALYSER_BANDS] > 0.0 && off_meters[ANALYSER_BANDS + 1] > 0.0);
    // On: the same peaks and live band energies.
    assert_eq!(off_meters[ANALYSER_BANDS..], on_meters[ANALYSER_BANDS..]);
    assert!(on_meters[..ANALYSER_BANDS].iter().any(|v| *v > 0.0));
}

#[test]
fn switching_the_analyser_off_and_on_starts_the_bands_from_silence() {
    let mut h = eq_live();
    let (_, live) = noise_run(&mut h);
    assert!(live[..ANALYSER_BANDS].iter().any(|v| *v > 0.0));
    h.set_param("internal.analyser_live", 0.0);
    h.process();
    // The last values stay until the bank is switched on again, then restart from zero.
    h.set_param("internal.analyser_live", 1.0);
    let mut m = [1.0f32; METER_COUNT];
    h.device().meter_values(&mut m);
    assert!(m[..ANALYSER_BANDS].iter().all(|v| *v == 0.0));
}

/// Cost of the analyser: `cargo test -p atomik-devices --release --lib eq::tests::cost -- --ignored
/// --nocapture`. Not an assertion on speed (CI timing is noisy), a measurement for the baseline.
#[test]
#[ignore = "timing measurement"]
#[allow(clippy::disallowed_methods)] // Justification: wall-clock measurement.
fn cost_of_the_analyser_off_and_on() {
    let time = |live: bool| {
        let mut h = if live { eq_live() } else { eq() };
        let mut rng = Rng::new(5);
        let mut x = [0.0f32; BATCH];
        for v in &mut x {
            *v = rng.next_bipolar() * 0.3;
        }
        *h.input_mut("in_l") = x;
        *h.input_mut("in_r") = x;
        let mut best = f64::MAX;
        for _ in 0..5 {
            let t = std::time::Instant::now();
            for _ in 0..20_000 {
                h.process();
            }
            best = best.min(t.elapsed().as_secs_f64() / 20_000.0);
        }
        best * 1e9
    };
    let (off, on) = (time(false), time(true));
    println!("eq per batch: analyser off {off:.0} ns, on {on:.0} ns");
}

/// FNV-1a over the bits of every sample.
fn fnv(samples: &[f32]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for v in samples {
        for b in v.to_bits().to_le_bytes() {
            h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

/// Renders noise through the EQ with only the params that existed before the upgrade, moving a
/// few of them mid-run (slope change, frequency jump, an old gain CV), and hashes both outputs.
fn legacy_render(scenario: usize) -> u64 {
    let mut h = eq_live();
    let setups: [&[(&str, f32)]; 4] = [
        &[
            ("band1.on", 1.0),
            ("band1.freq", 90.0),
            ("band2.gain", 5.0),
            ("band3.gain", -7.0),
            ("band3.q", 2.5),
            ("band4.gain", 4.0),
            ("band5.gain", -3.0),
            ("output", -2.0),
        ],
        &[
            ("band1.on", 1.0),
            ("lowcut.slope", 2.0),
            ("band1.q", 1.5),
            ("band6.on", 1.0),
            ("band6.freq", 9_000.0),
            ("highcut.slope", 1.0),
            ("band6.q", 1.2),
            ("band3.gain", 6.0),
        ],
        &[
            ("band2.freq", 300.0),
            ("band2.gain", -9.0),
            ("band2.q", 1.4),
            ("band5.freq", 3_000.0),
            ("band5.gain", 8.0),
            ("band5.q", 0.5),
            ("band4.freq", 5_000.0),
            ("band4.gain", 11.0),
            ("band4.q", 9.0),
        ],
        &[
            ("band3.gain_cv_trim", 0.5),
            ("band5.gain_cv_trim", -1.0),
            ("band6.on", 1.0),
            ("highcut.slope", 2.0),
        ],
    ];
    set(&mut h, setups[scenario]);
    let mut rng = Rng::new(77 + scenario as u64);
    let mut all = Vec::new();
    for b in 0..200 {
        if b == 60 {
            set(
                &mut h,
                &[
                    ("lowcut.slope", 1.0),
                    ("band3.freq", 3_500.0),
                    ("band2.on", 0.0),
                ],
            );
        }
        if b == 120 {
            set(&mut h, &[("band6.freq", 4_000.0), ("highcut.slope", 0.0)]);
        }
        let mut x = [0.0f32; BATCH];
        let mut y = [0.0f32; BATCH];
        for (a, c) in x.iter_mut().zip(&mut y) {
            *a = rng.next_bipolar() * 0.5;
            *c = rng.next_bipolar() * 0.5;
        }
        *h.input_mut("in_l") = x;
        *h.input_mut("in_r") = y;
        if scenario == 3 {
            *h.input_mut("band3_gain_cv_in") = [0.3 * (b as f32 / 40.0).sin(); BATCH];
            *h.input_mut("band5_gain_cv_in") = [0.2; BATCH];
        }
        h.process();
        all.extend_from_slice(h.output("out_l"));
        all.extend_from_slice(h.output("out_r"));
    }
    let mut m = [0.0f32; METER_COUNT];
    h.device().meter_values(&mut m);
    all.extend_from_slice(&m);
    fnv(&all)
}

/// Hashes of `legacy_render` captured from the EQ before the upgrade. Any change to the sound of
/// an existing project changes them.
const LEGACY_HASHES: [u64; 4] = [
    8_035_127_733_858_046_531,
    4_963_387_877_835_014_772,
    10_032_941_447_465_546_332,
    2_993_201_266_270_898_146,
];

#[test]
fn existing_projects_sound_bit_identical_after_the_upgrade() {
    let got: Vec<u64> = (0..4).map(legacy_render).collect();
    println!("legacy hashes: {got:#018x?}");
    assert_eq!(got, LEGACY_HASHES);
}

// ---------------------------------------------------------------------- the upgrade

/// Steady-state peak amplitude of both outputs after `batches` of the signals `l(n)` and `r(n)`.
fn stereo_peaks(
    h: &mut DeviceHarness,
    l: impl Fn(usize) -> f32,
    r: impl Fn(usize) -> f32,
    batches: usize,
) -> (f64, f64) {
    let (mut pl, mut pr) = (0.0f64, 0.0f64);
    for b in 0..batches {
        let (mut x, mut y) = ([0.0f32; BATCH], [0.0f32; BATCH]);
        for i in 0..BATCH {
            x[i] = l(b * BATCH + i);
            y[i] = r(b * BATCH + i);
        }
        *h.input_mut("in_l") = x;
        *h.input_mut("in_r") = y;
        h.process();
        if b >= batches - 20 {
            for v in h.output("out_l") {
                pl = pl.max(f64::from(v.abs()));
            }
            for v in h.output("out_r") {
                pr = pr.max(f64::from(v.abs()));
            }
        }
    }
    (pl, pr)
}

/// Analytic notch magnitude in dB (bilinear-warped second-order prototype).
fn notch_db(f0: f64, q: f64, f: f64) -> f64 {
    let w = (std::f64::consts::PI * f / SR).tan() / (std::f64::consts::PI * f0 / SR).tan();
    let d = (1.0 - w * w).powi(2);
    10.0 * (d / (d + w * w / (q * q))).log10()
}

#[test]
fn every_shape_matches_the_rbj_response_on_every_middle_band() {
    // (band, shape value, RBJ shape, f0, gain, q)
    let cases = [
        ("band2", 0, BiquadShape::Peaking, 300.0, 8.0, 3.0),
        ("band2", 2, BiquadShape::HighShelf, 900.0, -7.0, 0.9),
        ("band3", 1, BiquadShape::LowShelf, 700.0, 10.0, 0.8),
        ("band3", 2, BiquadShape::HighShelf, 2_500.0, 6.0, 1.5),
        ("band4", 1, BiquadShape::LowShelf, 150.0, -9.0, 1.2),
        ("band5", 0, BiquadShape::Peaking, 6_000.0, 9.0, 4.0),
        ("band5", 0, BiquadShape::Peaking, 1_000.0, -12.0, 0.15),
        ("band7", 0, BiquadShape::Peaking, 3_000.0, 11.0, 2.0),
        ("band7", 1, BiquadShape::LowShelf, 400.0, 7.0, 0.707),
        ("band7", 2, BiquadShape::HighShelf, 5_000.0, -10.0, 1.9),
    ];
    for (band, shape, rbj, f0, g, q) in cases {
        let mut h = eq();
        set(
            &mut h,
            &[
                (&format!("{band}.shape"), shape as f32),
                (&format!("{band}.freq"), f0 as f32),
                (&format!("{band}.gain"), g as f32),
                (&format!("{band}.q"), q as f32),
            ],
        );
        for _ in 0..6 {
            h.process(); // the shape change fades the band out and in
        }
        for f in TEST_FREQS {
            let got = gain_at(&mut h, f);
            let want = rbj_db(rbj, f0, f64::from(q as f32), g, f);
            let helper = middle_response_db(shape, f0, f64::from(q as f32), g, f, SR);
            assert!(
                (got - want).abs() < 0.12,
                "{band} shape {shape} f0 {f0} gain {g} q {q} at {f} Hz: got {got:.3} want {want:.3}"
            );
            assert!((helper - want).abs() < 1e-9, "helper {helper} vs {want}");
        }
    }
}

#[test]
fn shelves_clamp_q_to_0_3_to_2_and_bells_do_not() {
    assert_eq!(effective_q(1, 10.0), 2.0);
    assert_eq!(effective_q(2, 0.1), 0.3);
    assert_eq!(effective_q(0, 10.0), 10.0);
    assert_eq!(effective_q(3, 0.1), 0.1);
    // Through the device: band2 as a shelf at q 18 behaves as q 2; as a bell it is narrow.
    let mut h = eq();
    set(
        &mut h,
        &[
            ("band2.freq", 1_000.0),
            ("band2.gain", 12.0),
            ("band2.q", 18.0),
        ],
    );
    let want = rbj_db(BiquadShape::LowShelf, 1_000.0, 2.0, 12.0, 500.0);
    assert!((gain_at(&mut h, 500.0) - want).abs() < 0.1);
    set(&mut h, &[("band2.shape", 0.0)]);
    for _ in 0..6 {
        h.process();
    }
    assert!(gain_at(&mut h, 250.0).abs() < 1.0);
    assert!((gain_at(&mut h, 1_000.0) - 12.0).abs() < 0.1);
}

#[test]
fn the_notch_is_deep_at_the_centre_ignores_gain_and_is_on_at_zero_gain() {
    for (gain, q) in [(0.0f32, 2.0f32), (12.0, 2.0), (-20.0, 0.5), (0.0, 8.0)] {
        let mut h = eq();
        set(
            &mut h,
            &[
                ("band3.shape", 3.0),
                ("band3.freq", 1_000.0),
                ("band3.gain", gain),
                ("band3.q", q),
            ],
        );
        for _ in 0..6 {
            h.process();
        }
        let centre = gain_at(&mut h, 1_000.0);
        assert!(centre < -45.0, "gain {gain} q {q}: centre {centre} dB");
        for f in [200.0, 700.0, 1_400.0, 5_000.0] {
            let got = gain_at(&mut h, f);
            let want = notch_db(1_000.0, f64::from(q), f);
            assert!(
                (got - want).abs() < 0.15,
                "q {q} at {f} Hz: got {got:.3} want {want:.3}"
            );
        }
    }
    // Off when the band is off, whatever the shape.
    let mut h = eq();
    set(&mut h, &[("band3.shape", 3.0), ("band3.on", 0.0)]);
    for _ in 0..150 {
        h.process();
    }
    assert!(gain_at(&mut h, 500.0).abs() < 1e-3);
    assert!(middle_active(3, 0.0) && !middle_active(0, 0.0) && !middle_active(2, 0.0));
}

#[test]
fn band7_is_flat_by_default_and_runs_as_the_fourth_bell() {
    let mut h = eq();
    assert!(gain_at(&mut h, 6_000.0).abs() < 1e-3);
    set(&mut h, &[("band7.gain", 12.0), ("band7.q", 3.0)]);
    assert!((gain_at(&mut h, 6_000.0) - 12.0).abs() < 0.1);
    set(&mut h, &[("band7.on", 0.0)]);
    for _ in 0..150 {
        h.process();
    }
    assert!(gain_at(&mut h, 6_000.0).abs() < 1e-3);
}

#[test]
fn six_db_per_octave_cuts_are_first_order() {
    for low in [true, false] {
        let band = if low { "band1" } else { "band6" };
        let key = if low { "lowcut.slope" } else { "highcut.slope" };
        let fc = if low { 1_000.0 } else { 2_000.0 };
        let mut h = eq();
        set(
            &mut h,
            &[
                (&format!("{band}.on"), 1.0),
                (&format!("{band}.freq"), fc as f32),
                (&format!("{band}.q"), 4.0), // ignored at 6 dB/oct
                (key, 3.0),
            ],
        );
        for _ in 0..8 {
            h.process();
        }
        // One and two octaves past the corner.
        let (one, two) = if low {
            (fc / 2.0, fc / 4.0)
        } else {
            (fc * 2.0, fc * 4.0)
        };
        let corner = gain_at(&mut h, fc);
        assert!((corner + 3.01).abs() < 0.1, "corner {corner}");
        let (a, b) = (gain_at(&mut h, one), gain_at(&mut h, two));
        for (f, got) in [(one, a), (two, b)] {
            let want = butter_db(!low, 1, fc, f);
            assert!(
                (got - want).abs() < 0.1,
                "low {low} at {f}: {got} vs {want}"
            );
            let helper = cut_response_db(low, 3, fc, 4.0, f, SR);
            assert!((helper - want).abs() < 0.01, "helper {helper} vs {want}");
        }
        assert!(((a - b) - 6.0).abs() < 1.5, "slope {}", a - b);
        assert!(a > -8.0 && a < -6.0 && b < -11.0 && b > -14.5, "{a} {b}");
    }
}

#[test]
fn thirty_six_db_per_octave_cuts_are_sixth_order_butterworth() {
    for low in [true, false] {
        let band = if low { "band1" } else { "band6" };
        let key = if low { "lowcut.slope" } else { "highcut.slope" };
        let fc = if low { 1_000.0 } else { 2_000.0 };
        let mut h = eq();
        set(
            &mut h,
            &[
                (&format!("{band}.on"), 1.0),
                (&format!("{band}.freq"), fc as f32),
                (key, 4.0),
            ],
        );
        for _ in 0..8 {
            h.process();
        }
        let freqs = if low {
            [250.0, 500.0, 800.0, 1_000.0, 1_250.0, 2_000.0]
        } else {
            [1_000.0, 1_600.0, 2_000.0, 2_500.0, 4_000.0, 8_000.0]
        };
        for f in freqs {
            let got = gain_at(&mut h, f);
            let want = butter_db(!low, 6, fc, f);
            if want > -70.0 {
                assert!(
                    (got - want).abs() < 0.1,
                    "low {low} at {f}: {got} vs {want}"
                );
            }
            let helper = cut_response_db(low, 4, fc, std::f64::consts::FRAC_1_SQRT_2, f, SR);
            assert!((helper - want).abs() < 0.01, "helper {helper} vs {want}");
        }
        let (one, two) = if low {
            (fc / 2.0, fc / 4.0)
        } else {
            (fc * 2.0, fc * 4.0)
        };
        let (a, b) = (gain_at(&mut h, one), gain_at(&mut h, two));
        let slope = a - b;
        println!("low {low} 36 dB/oct: {a:.2} {b:.2} slope {slope:.2}");
        let tolerance = if low { 1.0 } else { 6.0 };
        assert!((slope - 36.0).abs() < tolerance, "slope {slope}");
        assert!(a < -33.0, "one octave past the corner: {a}");
    }
}

#[test]
fn slope_order_lists_the_steepness_ascending() {
    let nominal = [12.0, 24.0, 48.0, 6.0, 36.0];
    let ordered: Vec<f64> = SLOPE_ORDER.iter().map(|&s| nominal[s]).collect();
    assert_eq!(ordered, [6.0, 12.0, 24.0, 36.0, 48.0]);
    assert_eq!(SLOPE_NAMES[SLOPE_ORDER[0]], "6 dB/oct");
    assert_eq!(SLOPE_NAMES[SLOPE_ORDER[3]], "36 dB/oct");
    assert_eq!(DISPLAY_RANGE_DB, [6.0, 12.0, 24.0]);
    assert_eq!(STEREO_MODE_NAMES.len(), 5);
    assert_eq!(SHAPE_NAMES, ["Bell", "Low shelf", "High shelf", "Notch"]);
}

/// A +12 dB bell at 1 kHz, Q 2, in the given stereo mode.
fn mode_eq(mode: f32) -> DeviceHarness {
    let mut h = eq();
    set(
        &mut h,
        &[
            ("band3.freq", 1_000.0),
            ("band3.gain", 12.0),
            ("band3.q", 2.0),
            ("stereo.mode", mode),
        ],
    );
    h
}

#[test]
fn stereo_modes_filter_the_right_part_of_the_signal() {
    let tone = |n: usize| (0.25 * (std::f64::consts::TAU * 1_000.0 * n as f64 / SR).sin()) as f32;
    let neg = |n: usize| -tone(n);
    let zero = |_: usize| 0.0f32;
    let g = 10f64.powf(12.0 / 20.0);
    let a = 0.25f64;
    let near = |got: f64, want: f64, what: &str| {
        assert!(
            (got - want).abs() < want.abs() * 0.02 + 2e-3,
            "{what}: {got} vs {want}"
        );
    };
    // L = R (pure mid): only Mid and Mid solo see it.
    let (l, r) = stereo_peaks(&mut mode_eq(0.0), tone, tone, 80);
    near(l, a * g, "stereo L=R");
    near(r, a * g, "stereo L=R R");
    let (l, r) = stereo_peaks(&mut mode_eq(1.0), tone, tone, 80);
    near(l, a * g, "mid L=R");
    near(r, a * g, "mid L=R R");
    let (l, r) = stereo_peaks(&mut mode_eq(2.0), tone, tone, 80);
    near(l, a, "side L=R");
    near(r, a, "side L=R R");
    let (l, r) = stereo_peaks(&mut mode_eq(3.0), tone, tone, 80);
    near(l, a * g, "mid solo L=R");
    near(r, a * g, "mid solo L=R R");
    let (l, r) = stereo_peaks(&mut mode_eq(4.0), tone, tone, 80);
    assert!(l < 1e-3 && r < 1e-3, "side solo of a mono signal: {l} {r}");
    // L = -R (pure side).
    let (l, r) = stereo_peaks(&mut mode_eq(1.0), tone, neg, 80);
    near(l, a, "mid L=-R");
    near(r, a, "mid L=-R R");
    let (l, r) = stereo_peaks(&mut mode_eq(2.0), tone, neg, 80);
    near(l, a * g, "side L=-R");
    near(r, a * g, "side L=-R R");
    let (l, r) = stereo_peaks(&mut mode_eq(3.0), tone, neg, 80);
    assert!(l < 1e-3 && r < 1e-3, "mid solo of a pure side: {l} {r}");
    let (l, r) = stereo_peaks(&mut mode_eq(4.0), tone, neg, 80);
    near(l, a * g, "side solo L=-R");
    near(r, a * g, "side solo L=-R R");
    // Left only: M = S = x / 2. Mid: L = (G + 1) x / 2, R = (G - 1) x / 2.
    let (l, r) = stereo_peaks(&mut mode_eq(1.0), tone, zero, 80);
    near(l, a * (g + 1.0) / 2.0, "mid left only");
    near(r, a * (g - 1.0) / 2.0, "mid left only R");
    let (l, r) = stereo_peaks(&mut mode_eq(2.0), tone, zero, 80);
    near(l, a * (1.0 + g) / 2.0, "side left only");
    near(r, a * (g - 1.0) / 2.0, "side left only R");
    let (l, r) = stereo_peaks(&mut mode_eq(3.0), tone, zero, 80);
    near(l, a * g / 2.0, "mid solo left only");
    near(r, a * g / 2.0, "mid solo left only R");
    // Stereo: left only stays on the left.
    let (l, r) = stereo_peaks(&mut mode_eq(0.0), tone, zero, 80);
    near(l, a * g, "stereo left only");
    assert!(r < 1e-6);
}

#[test]
fn stereo_mode_zero_is_bit_identical_to_the_original_path() {
    // Mode 0 again after a trip through the others (and a reset) is the stereo path as it was.
    let run = |trip: bool| {
        let mut h = eq();
        set(&mut h, &[("band3.gain", 6.0), ("band1.on", 1.0)]);
        if trip {
            for m in [1.0, 4.0, 2.0, 0.0] {
                h.set_param("stereo.mode", m);
                for _ in 0..4 {
                    h.process();
                }
            }
        }
        h.device_mut().reset();
        let mut rng = Rng::new(3);
        let mut all = Vec::new();
        for _ in 0..40 {
            let mut x = [0.0f32; BATCH];
            for v in &mut x {
                *v = rng.next_bipolar() * 0.5;
            }
            *h.input_mut("in_l") = x;
            *h.input_mut("in_r") = x.map(|v| v * 0.5);
            h.process();
            all.extend_from_slice(h.output("out_l"));
            all.extend_from_slice(h.output("out_r"));
        }
        all
    };
    let (a, b) = (run(false), run(true));
    assert!(a.iter().zip(&b).all(|(p, q)| p.to_bits() == q.to_bits()));
}

/// Largest sample-to-sample jump of `out_l` from just before batch 40 when `change` is applied
/// then, for a sine of amplitude 0.5 at `hz` on both inputs.
fn jump_after(setup: &[(&str, f32)], change: &[(&str, f32)], hz: f64) -> f32 {
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
    max_step(&all[40 * BATCH - 8..])
}

type Params<'a> = &'a [(&'a str, f32)];

#[test]
fn shape_slope_and_mode_changes_do_not_click() {
    let cases: [(Params, Params, f64, f32); 9] = [
        // A bell turns into a shelf and a notch at the signal frequency.
        (
            &[("band3.freq", 500.0), ("band3.gain", 12.0)],
            &[("band3.shape", 1.0)],
            500.0,
            0.5,
        ),
        (
            &[("band3.freq", 500.0), ("band3.gain", 12.0)],
            &[("band3.shape", 3.0)],
            500.0,
            0.2,
        ),
        // A notch (no gain) back to a flat bell.
        (
            &[("band4.freq", 700.0), ("band4.shape", 3.0)],
            &[("band4.shape", 0.0)],
            700.0,
            0.2,
        ),
        // Band 7 shape change.
        (
            &[("band7.freq", 3_000.0), ("band7.gain", 9.0)],
            &[("band7.shape", 2.0)],
            2_000.0,
            0.5,
        ),
        // Slopes 6 and 36 in and out of use, on both cuts.
        (
            &[("band1.on", 1.0), ("band1.freq", 400.0)],
            &[("lowcut.slope", 3.0)],
            300.0,
            0.1,
        ),
        (
            &[
                ("band1.on", 1.0),
                ("band1.freq", 400.0),
                ("lowcut.slope", 3.0),
            ],
            &[("lowcut.slope", 4.0)],
            300.0,
            0.1,
        ),
        (
            &[
                ("band6.on", 1.0),
                ("band6.freq", 2_000.0),
                ("highcut.slope", 4.0),
            ],
            &[("highcut.slope", 3.0)],
            1_500.0,
            0.15,
        ),
        // Stereo modes (no EQ gain: the output stays about the input).
        (&[], &[("stereo.mode", 1.0)], 300.0, 0.06),
        (
            &[("stereo.mode", 3.0)],
            &[("stereo.mode", 0.0)],
            300.0,
            0.06,
        ),
    ];
    for (setup, change, hz, bound) in cases {
        let step = jump_after(setup, change, hz);
        assert!(step < bound, "{change:?}: max step {step} (bound {bound})");
    }
}

#[test]
fn a_mode_change_leaves_no_burst_from_the_other_signals_filter_state() {
    // L = -R with a strong low shelf: in stereo the shelf state is large. In Mid mode the
    // mid signal is silence, so a filter that kept that state would ring.
    let mut h = eq();
    set(
        &mut h,
        &[
            ("band2.freq", 1_000.0),
            ("band2.gain", 24.0),
            ("band2.shape", 1.0),
        ],
    );
    let src = sine(100.0, 0.3);
    let mut tail = Vec::new();
    for b in 0..120 {
        if b == 40 {
            h.set_param("stereo.mode", 1.0);
        }
        let mut x = [0.0f32; BATCH];
        for (i, v) in x.iter_mut().enumerate() {
            *v = src(b * BATCH + i);
        }
        *h.input_mut("in_l") = x;
        *h.input_mut("in_r") = x.map(|v| -v);
        h.process();
        if b >= 43 {
            tail.extend_from_slice(h.output("out_l"));
        }
    }
    // Mid of a pure side signal is 0: the output is the side signal, 0.3 at most.
    let peak = tail.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    assert!(peak < 0.3001, "burst after the switch: {peak}");
    assert!(peak > 0.25, "the side signal is gone: {peak}");
}

/// Gain in dB at `hz` with `setup` applied and `cvs` on the sockets.
fn cv_gain(setup: &[(&str, f32)], cvs: &[(&str, f32)], hz: f64) -> f64 {
    let mut h = eq();
    set(&mut h, setup);
    for (k, c) in cvs {
        *h.input_mut(k) = [*c; BATCH];
    }
    gain_at(&mut h, hz)
}

#[test]
fn freq_cv_moves_a_band_by_four_octaves_at_full_scale() {
    let setup = [
        ("band3.freq", 500.0),
        ("band3.gain", 12.0),
        ("band3.q", 3.0),
    ];
    // Unconnected: peak at 500 Hz.
    assert!((cv_gain(&setup, &[], 500.0) - 12.0).abs() < 0.1);
    // +0.25 -> +1 octave: the peak sits at 1 kHz.
    let cvs = [("band3_freq_cv_in", 0.25)];
    assert!((cv_gain(&setup, &cvs, 1_000.0) - 12.0).abs() < 0.15);
    assert!(cv_gain(&setup, &cvs, 500.0) < 6.0);
    // Negative CV and half trim: -0.5 * 0.5 * 4 = -1 octave: 250 Hz.
    let half = [
        ("band3.freq", 500.0),
        ("band3.gain", 12.0),
        ("band3.q", 3.0),
        ("band3.freq_cv_trim", 0.5),
    ];
    let cvs = [("band3_freq_cv_in", -0.5)];
    assert!((cv_gain(&half, &cvs, 250.0) - 12.0).abs() < 0.15);
    // Clamped to the param range: +1 on a band whose frequency is already at the top.
    let top = [
        ("band3.freq", 20_000.0),
        ("band3.gain", 12.0),
        ("band3.q", 1.0),
    ];
    let g = cv_gain(&top, &[("band3_freq_cv_in", 1.0)], 16_000.0);
    let want = rbj_db(BiquadShape::Peaking, 20_000.0, 1.0, 12.0, 16_000.0);
    assert!((g - want).abs() < 0.2, "{g} vs {want}");
    // A cut's corner follows the CV too: 1 kHz low cut + 1 octave = 2 kHz.
    let cut = [("band1.on", 1.0), ("band1.freq", 1_000.0)];
    let g = cv_gain(&cut, &[("band1_freq_cv_in", 0.25)], 2_000.0);
    assert!((g + 3.01).abs() < 0.15, "{g}");
    // The high cut and band 7 have their own inputs.
    let g = cv_gain(
        &[("band6.on", 1.0), ("band6.freq", 2_000.0)],
        &[("band6_freq_cv_in", -0.25)],
        1_000.0,
    );
    assert!((g + 3.01).abs() < 0.15, "{g}");
    let g = cv_gain(
        &[
            ("band7.freq", 3_000.0),
            ("band7.gain", 12.0),
            ("band7.q", 3.0),
        ],
        &[("band7_freq_cv_in", -0.25)],
        1_500.0,
    );
    assert!((g - 12.0).abs() < 0.15, "{g}");
}

#[test]
fn q_cv_scales_q_by_two_to_the_cv_times_two() {
    let setup = [
        ("band3.freq", 1_000.0),
        ("band3.gain", 12.0),
        ("band3.q", 1.0),
    ];
    let at = |cv: Option<f32>| {
        let cvs: Vec<(&str, f32)> = cv.map(|c| ("band3_q_cv_in", c)).into_iter().collect();
        cv_gain(&setup, &cvs, 1_500.0)
    };
    let flat = at(None);
    assert!((flat - rbj_db(BiquadShape::Peaking, 1_000.0, 1.0, 12.0, 1_500.0)).abs() < 0.1);
    // cv 0.5 * trim 1 * 2 = one octave of Q: Q 2.
    let narrower = at(Some(0.5));
    assert!((narrower - rbj_db(BiquadShape::Peaking, 1_000.0, 2.0, 12.0, 1_500.0)).abs() < 0.1);
    assert!(narrower < flat - 1.0);
    // Negative: Q 0.5, wider.
    let wider = at(Some(-0.5));
    assert!((wider - rbj_db(BiquadShape::Peaking, 1_000.0, 0.5, 12.0, 1_500.0)).abs() < 0.1);
    // Clamped to the bell range 0.1..18: full scale on Q 10 gives 18, not 40.
    let high = [
        ("band3.freq", 1_000.0),
        ("band3.gain", 12.0),
        ("band3.q", 10.0),
    ];
    let g = cv_gain(&high, &[("band3_q_cv_in", 1.0)], 1_100.0);
    assert!((g - rbj_db(BiquadShape::Peaking, 1_000.0, 18.0, 12.0, 1_100.0)).abs() < 0.2);
    // A shelf clamps to 2.
    let shelf = [
        ("band2.freq", 1_000.0),
        ("band2.gain", 12.0),
        ("band2.q", 1.5),
    ];
    let g = cv_gain(&shelf, &[("band2_q_cv_in", 1.0)], 700.0);
    assert!((g - rbj_db(BiquadShape::LowShelf, 1_000.0, 2.0, 12.0, 700.0)).abs() < 0.1);
    // A cut's Q is its resonance: one octave of Q on Q 1 gives Q 2, a 6 dB corner at 12 dB/oct.
    let cut = [("band1.on", 1.0), ("band1.freq", 1_000.0), ("band1.q", 1.0)];
    let g = cv_gain(&cut, &[("band1_q_cv_in", 0.5)], 1_000.0);
    assert!((g - 6.02).abs() < 0.15, "{g}");
}

#[test]
fn band7_gain_cv_adds_cv_times_trim_times_24_db() {
    let setup = [("band7.freq", 6_000.0), ("band7.q", 2.0)];
    let g = cv_gain(&setup, &[("band7_gain_cv_in", 0.25)], 6_000.0);
    assert!((g - 6.0).abs() < 0.1, "{g}");
    let mut half = setup.to_vec();
    half.push(("band7.gain_cv_trim", -0.5));
    let g = cv_gain(&half, &[("band7_gain_cv_in", 0.5)], 6_000.0);
    assert!((g + 6.0).abs() < 0.1, "{g}");
}

#[test]
fn unconnected_new_cv_inputs_and_trims_change_nothing() {
    let run = |trims: bool| {
        let mut h = eq_live();
        set(
            &mut h,
            &[("band3.gain", 7.0), ("band1.on", 1.0), ("band7.gain", 3.0)],
        );
        if trims {
            for n in 1..=7 {
                set(
                    &mut h,
                    &[
                        (&format!("band{n}.freq_cv_trim"), -0.3),
                        (&format!("band{n}.q_cv_trim"), 0.2),
                    ],
                );
            }
            set(&mut h, &[("spectrum.on", 0.0), ("display.range", 2.0)]);
        }
        let mut rng = Rng::new(9);
        let mut all = Vec::new();
        for _ in 0..80 {
            let mut x = [0.0f32; BATCH];
            for v in &mut x {
                *v = rng.next_bipolar() * 0.5;
            }
            *h.input_mut("in_l") = x;
            *h.input_mut("in_r") = x;
            h.process();
            all.extend_from_slice(h.output("out_l"));
            all.extend_from_slice(h.output("out_r"));
        }
        all
    };
    let (a, b) = (run(false), run(true));
    assert!(a.iter().zip(&b).all(|(p, q)| p.to_bits() == q.to_bits()));
}

#[test]
fn the_cv_and_new_features_survive_non_finite_input() {
    let mut h = eq();
    set(
        &mut h,
        &[
            ("band3.shape", 3.0),
            ("band7.gain", 6.0),
            ("lowcut.slope", 4.0),
            ("band1.on", 1.0),
            ("highcut.slope", 3.0),
            ("band6.on", 1.0),
            ("stereo.mode", 1.0),
        ],
    );
    let mut bad = [0.1f32; BATCH];
    bad[3] = f32::NAN;
    bad[9] = f32::NEG_INFINITY;
    for b in 0..80 {
        *h.input_mut("in_l") = if b % 7 == 0 { bad } else { [0.1; BATCH] };
        *h.input_mut("in_r") = bad;
        *h.input_mut("band7_freq_cv_in") = bad;
        *h.input_mut("band3_q_cv_in") = bad;
        *h.input_mut("band1_freq_cv_in") = [f32::NAN; BATCH];
        h.process();
        for k in ["out_l", "out_r"] {
            assert!(h.output(k).iter().all(|v| v.is_finite()));
        }
        if b == 30 {
            h.set_param("stereo.mode", 4.0);
        }
    }
}

#[test]
fn the_spectrum_tap_writes_the_output_only_while_live() {
    let mut h = eq();
    set(&mut h, &[("band3.gain", 6.0), ("output", -3.0)]);
    let tap = h.device().scope_tap().expect("the EQ owns a tap");
    let src = sine(440.0, 0.4);
    let push = |h: &mut DeviceHarness, b: usize| {
        let mut x = [0.0f32; BATCH];
        for (i, v) in x.iter_mut().enumerate() {
            *v = src(b * BATCH + i);
        }
        *h.input_mut("in_l") = x;
        *h.input_mut("in_r") = x.map(|v| -v);
        h.process();
    };
    for b in 0..5 {
        push(&mut h, b);
    }
    assert_eq!(tap.head(), 0, "nothing is written while nobody looks");
    tap.set_live(true);
    let mut sent_l = Vec::new();
    let mut sent_r = Vec::new();
    for b in 5..10 {
        push(&mut h, b);
        sent_l.extend_from_slice(h.output("out_l"));
        sent_r.extend_from_slice(h.output("out_r"));
    }
    assert_eq!(tap.head(), 5 * BATCH as u64);
    let (mut l, mut r) = (vec![0.0f32; 5 * BATCH], vec![0.0f32; 5 * BATCH]);
    assert_eq!(tap.read_latest(&mut l, &mut r).0, 5 * BATCH);
    assert!(
        l.iter()
            .zip(&sent_l)
            .all(|(a, b)| a.to_bits() == b.to_bits())
    );
    assert!(
        r.iter()
            .zip(&sent_r)
            .all(|(a, b)| a.to_bits() == b.to_bits())
    );
    tap.set_live(false);
    push(&mut h, 10);
    assert_eq!(tap.head(), 5 * BATCH as u64);
}

#[test]
fn the_tap_does_not_change_the_audio() {
    let run = |live: bool| {
        let mut h = eq();
        if live {
            h.device().scope_tap().unwrap().set_live(true);
        }
        noise_run(&mut h).0
    };
    let (a, b) = (run(false), run(true));
    assert!(a.iter().zip(&b).all(|(p, q)| p.to_bits() == q.to_bits()));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    #[test]
    fn output_stays_finite_with_any_shape_slope_mode_and_cv(
        shapes in proptest::collection::vec(0u8..4, 5),
        freqs in proptest::collection::vec(20.0f32..20000.0, 7),
        gains in proptest::collection::vec(-24.0f32..24.0, 5),
        qs in proptest::collection::vec(0.1f32..18.0, 7),
        slopes in proptest::collection::vec(0u8..5, 2),
        mode in 0u8..5,
        cvs in proptest::collection::vec(-3.0f32..3.0, 3),
        seed in 0u64..100,
    ) {
        let mut h = eq();
        let bands = [2, 3, 4, 5, 7];
        for (k, b) in bands.iter().enumerate() {
            h.set_param(&format!("band{b}.shape"), f32::from(shapes[k]));
            h.set_param(&format!("band{b}.gain"), gains[k]);
        }
        for n in 1..=7usize {
            h.set_param(&format!("band{n}.on"), 1.0);
            h.set_param(&format!("band{n}.freq"), freqs[n - 1]);
            h.set_param(&format!("band{n}.q"), qs[n - 1]);
        }
        h.set_param("lowcut.slope", f32::from(slopes[0]));
        h.set_param("highcut.slope", f32::from(slopes[1]));
        h.set_param("stereo.mode", f32::from(mode));
        let mut rng = Rng::new(seed);
        for i in 0..60 {
            let mut x = [0.0f32; BATCH];
            for v in &mut x {
                *v = rng.next_bipolar();
            }
            *h.input_mut("in_l") = x;
            *h.input_mut("in_r") = x.map(|v| v * 0.3);
            *h.input_mut("band3_freq_cv_in") = [cvs[0]; BATCH];
            *h.input_mut("band5_q_cv_in") = [cvs[1]; BATCH];
            *h.input_mut("band7_gain_cv_in") = [cvs[2]; BATCH];
            h.process();
            if i == 30 {
                h.set_param("stereo.mode", f32::from((mode + 1) % 5));
            }
            for k in ["out_l", "out_r"] {
                for v in h.output(k) {
                    prop_assert!(v.is_finite());
                    prop_assert!(v.abs() < 1.0e7, "{}", v);
                }
            }
        }
    }
}
