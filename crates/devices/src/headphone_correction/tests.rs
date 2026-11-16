//! Headphone correction tests, all through `DeviceHarness` (every `process` is allocation-guarded).
#![allow(clippy::float_cmp)]

use super::curve::{
    Curve, DataKind, Design, GRID_LEN, LIMIT_DB, Settings, TAPS, Target, grid_freq, snapshot,
};
use super::*;
use crate::test_support::{SR, assert_keys, harness, run_signal, sine};
use atomik_device_sdk::PlayerData;
use atomik_device_sdk::headphone::parse;
use atomik_dsp::{BiquadCoeffs, BiquadShape, Rng};
use atomik_testkit::harness::DeviceHarness;
use std::sync::Arc;

fn device() -> DeviceHarness {
    harness(Box::new(HeadphoneCorrection::new()))
}

fn data(text: &str) -> PlayerDataRef {
    Arc::new(PlayerData::Headphone(parse(text, "test.txt").unwrap()))
}

/// A device with `text` installed and the filter built and faded in.
fn with(text: &str) -> DeviceHarness {
    let mut h = device();
    assert!(h.device_mut().swap_player_data(Some(data(text))).is_none());
    settle(&mut h);
    h
}

fn meters(h: &DeviceHarness) -> [f32; METERS] {
    let mut m = [0.0; METERS];
    assert_eq!(h.device().meter_values(&mut m), METERS);
    m
}

/// Processes silence until nothing is being built, then 50 batches more (the fade-in).
fn settle(h: &mut DeviceHarness) {
    for _ in 0..600 {
        h.process();
        if meters(h)[meter::BUSY] == 0.0 {
            break;
        }
    }
    assert_eq!(meters(h)[meter::BUSY], 0.0, "the build did not finish");
    for _ in 0..50 {
        h.process();
    }
}

/// The impulse response of both channels (8192 samples), the input being an impulse on both.
fn impulse(h: &mut DeviceHarness) -> (Vec<f32>, Vec<f32>) {
    let mut y = run_signal(
        h,
        &["in_l", "in_r"],
        |n| if n == 0 { 1.0 } else { 0.0 },
        8192 / BATCH,
        &["out_l", "out_r"],
    );
    let r = y.remove(1);
    (y.remove(0), r)
}

/// Magnitude of `ir` at `hz` in dB (a single-bin DFT).
fn mag_db(ir: &[f32], hz: f64, sr: f64) -> f64 {
    let (mut re, mut im) = (0.0f64, 0.0f64);
    for (n, v) in ir.iter().enumerate() {
        let ph = std::f64::consts::TAU * hz * n as f64 / sr;
        re += f64::from(*v) * ph.cos();
        im -= f64::from(*v) * ph.sin();
    }
    20.0 * (re * re + im * im).sqrt().log10()
}

/// 60 log-spaced test frequencies in 30 Hz..16 kHz.
fn test_freqs() -> Vec<f64> {
    (0..60)
        .map(|i| 30.0 * (16_000.0f64 / 30.0).powf(f64::from(i) / 59.0))
        .collect()
}

fn assert_curve(ir: &[f32], sr: f64, expect: &dyn Fn(f64) -> f64, tol: f64, what: &str) {
    for f in test_freqs() {
        let got = mag_db(ir, f, sr);
        let want = expect(f);
        assert!(
            (got - want).abs() <= tol,
            "{what}: at {f:.0} Hz got {got:.2} dB, expected {want:.2} dB"
        );
    }
}

const PARAMETRIC: &str = "Preamp: -3.0 dB\n\
    Filter 1: ON LSC Fc 100 Hz Gain 5 dB Q 0.7\n\
    Filter 2: ON PK Fc 3000 Hz Gain -4 dB Q 1.5\n\
    Filter 3: ON HSC Fc 9000 Hz Gain -3 dB Q 0.7\n\
    Filter 4: OFF PK Fc 500 Hz Gain 12 dB Q 1\n";

/// The cookbook sum of `PARAMETRIC`'s active bands at `f`, dB.
fn parametric_db(f: f64, sr: f64) -> f64 {
    [
        (BiquadShape::LowShelf, 100.0, 0.7, 5.0),
        (BiquadShape::Peaking, 3000.0, 1.5, -4.0),
        (BiquadShape::HighShelf, 9000.0, 0.7, -3.0),
    ]
    .iter()
    .map(|(s, fc, q, g)| BiquadCoeffs::design(*s, *fc, *q, *g, sr).response_db(f, sr))
    .sum()
}

fn max_db(expect: impl Fn(f64) -> f64) -> f64 {
    (0..2000)
        .map(|i| expect(20.0 * (1000.0f64).powf(f64::from(i) / 1999.0)))
        .fold(f64::MIN, f64::max)
}

/// A synthetic measured response: a broad bump near 3 kHz and a bass lift.
fn synthetic(f: f64) -> f64 {
    let o = (f / 3000.0).log2();
    5.0 * (-(o * o) / (2.0 * 0.7 * 0.7)).exp() - 3.0 / (1.0 + (f / 150.0) * (f / 150.0))
}

fn synthetic_csv() -> String {
    let mut s = String::from("Frequency (Hz), SPL (dB)\n");
    // 1/12-octave nodes from 20 Hz to 20 kHz.
    for i in 0..=120 {
        let f = 20.0 * 2.0f64.powf(f64::from(i) / 12.0);
        if f > 20_000.0 {
            break;
        }
        // Raw SPL: 85 dB at 1 kHz, as a real measurement would be.
        s.push_str(&format!(
            "{f:.3}, {:.4}\n",
            85.0 + synthetic(f) - synthetic(1000.0)
        ));
    }
    s
}

#[test]
fn the_descriptor_keys_are_pinned() {
    assert_keys(
        &DESCRIPTOR,
        &[
            "amount",
            "target",
            "file_kind",
            "simulate",
            "auto_gain",
            "output",
            "bypass",
        ],
        &["in_l", "in_r", "out_l", "out_r"],
    );
    let h = device();
    assert_eq!(h.device().latency_frames(), 0);
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Effect);
}

#[test]
fn without_data_the_device_is_a_bit_exact_passthrough() {
    let mut h = device();
    let mut rng = Rng::new(7);
    let x: Vec<f32> = (0..BATCH * 40).map(|_| rng.next_bipolar() * 0.9).collect();
    let y = run_signal(&mut h, &["in_l", "in_r"], |n| x[n], 40, &["out_l", "out_r"]);
    for (a, b) in x.iter().zip(&y[0]) {
        assert_eq!(a.to_bits(), b.to_bits());
    }
    for (a, b) in x.iter().zip(&y[1]) {
        assert_eq!(a.to_bits(), b.to_bits());
    }
    assert_eq!(meters(&h)[meter::LOADED], 0.0);
}

#[test]
fn non_finite_and_huge_input_stay_finite_with_and_without_data() {
    for text in [None, Some(PARAMETRIC), Some("20 3\n1000 0\n20000 -6\n")] {
        let mut h = device();
        if let Some(t) = text {
            let _ = h.device_mut().swap_player_data(Some(data(t)));
            settle(&mut h);
        }
        let y = run_signal(
            &mut h,
            &["in_l", "in_r"],
            |n| match n % 7 {
                0 => f32::NAN,
                1 => f32::INFINITY,
                2 => f32::NEG_INFINITY,
                3 => 1.0e6,
                _ => 0.0,
            },
            40,
            &["out_l", "out_r"],
        );
        assert!(y[0].iter().all(|v| v.is_finite()), "{text:?}");
        assert!(y[1].iter().all(|v| v.is_finite()), "{text:?}");
        // After the burst the tail dies away to exact silence's neighbourhood and stays finite.
        let silent = run_signal(&mut h, &["in_l", "in_r"], |_| 0.0, 200, &["out_l"]);
        assert!(silent[0].iter().all(|v| v.is_finite()), "{text:?}");
        // Silence in is silence out.
        let mut h2 = device();
        if let Some(t) = text {
            let _ = h2.device_mut().swap_player_data(Some(data(t)));
            settle(&mut h2);
        }
        let z = run_signal(&mut h2, &["in_l", "in_r"], |_| 0.0, 100, &["out_l"]);
        assert!(z[0].iter().all(|v| *v == 0.0), "silence stays silent");
    }
}

#[test]
fn a_parametric_file_is_applied_with_the_automatic_preamp() {
    let mut h = with(PARAMETRIC);
    let (l, r) = impulse(&mut h);
    let boost = max_db(|f| parametric_db(f, SR)).max(0.0);
    assert!(boost > 1.0);
    assert_curve(
        &l,
        SR,
        &|f| parametric_db(f, SR) - boost,
        0.5,
        "parametric left",
    );
    assert_curve(
        &r,
        SR,
        &|f| parametric_db(f, SR) - boost,
        0.5,
        "parametric right",
    );
    let m = meters(&h);
    assert_eq!(m[meter::LOADED], 1.0);
    assert!((f64::from(m[meter::PREAMP]) + boost).abs() < 0.05, "{m:?}");
    assert!((f64::from(m[meter::BOOST]) - boost).abs() < 0.05, "{m:?}");
}

#[test]
fn auto_gain_off_uses_the_files_own_preamp() {
    let mut h = device();
    h.set_param("auto_gain", 0.0);
    let _ = h.device_mut().swap_player_data(Some(data(PARAMETRIC)));
    settle(&mut h);
    let (l, _) = impulse(&mut h);
    assert_curve(&l, SR, &|f| parametric_db(f, SR) - 3.0, 0.5, "file preamp");
    // Without the preamp line there is no gain either.
    let mut h = device();
    h.set_param("auto_gain", 0.0);
    let _ = h
        .device_mut()
        .swap_player_data(Some(data("Filter 1: ON PK Fc 1000 Hz Gain 6 dB Q 1\n")));
    settle(&mut h);
    let (l, _) = impulse(&mut h);
    assert!((mag_db(&l, 1000.0, SR) - 6.0).abs() < 0.3);
}

#[test]
fn a_measured_response_is_flattened_to_the_flat_target() {
    let csv = synthetic_csv();
    let mut h = with(&csv);
    let (l, _) = impulse(&mut h);
    let corr = |f: f64| -(synthetic(f) - synthetic(1000.0));
    let boost = max_db(corr).max(0.0);
    assert_curve(&l, SR, &|f| corr(f) - boost, 0.5, "csv flatten");
    // The 85 dB level of the raw data does not matter.
    let m = meters(&h);
    assert!((f64::from(m[meter::PREAMP]) + boost).abs() < 0.2, "{m:?}");
}

#[test]
fn the_studio_target_tilts_a_flat_response() {
    let mut h = device();
    h.set_param("target", 1.0);
    h.set_param("auto_gain", 0.0);
    let _ = h
        .device_mut()
        .swap_player_data(Some(data("20 80\n1000 80\n20000 80\n")));
    settle(&mut h);
    let (l, _) = impulse(&mut h);
    let studio = |f: f64| f64::from(Target::Studio.db_at(f));
    assert_curve(&l, SR, &studio, 0.5, "studio");
    assert!((studio(1000.0)).abs() < 0.1);
    assert!((studio(10_000.0) + 3.32).abs() < 0.05);
    assert!((studio(60.0) - 2.4).abs() < 0.05);
    assert!((studio(5000.0) + 2.32).abs() < 0.05);
    // Held beyond 10 kHz.
    assert!((studio(20_000.0) - studio(10_000.0)).abs() < 1e-3);
}

#[test]
fn a_correction_file_is_used_as_it_is() {
    let csv = "20, 0\n100, 3\n500, 3\n2000, -2\n8000, -2\n20000, 0\n";
    let mut h = device();
    h.set_param("file_kind", 1.0);
    h.set_param("auto_gain", 0.0);
    let _ = h.device_mut().swap_player_data(Some(data(csv)));
    settle(&mut h);
    let (l, _) = impulse(&mut h);
    // Away from the corners (the data is smoothed over 1/12 octave).
    for (f, want) in [(250.0, 3.0), (4000.0, -2.0), (12_000.0, -1.0)] {
        let got = mag_db(&l, f, SR);
        assert!((got - want).abs() < 0.6, "{f}: {got} vs {want}");
    }
    // As a measured response the same file is inverted (levelled at 1 kHz).
    let mut h = device();
    h.set_param("auto_gain", 0.0);
    let _ = h.device_mut().swap_player_data(Some(data(csv)));
    settle(&mut h);
    let (l, _) = impulse(&mut h);
    let at_1k = 3.0 + (-2.0 - 3.0) * (1000.0f64 / 500.0).ln() / (2000.0f64 / 500.0).ln();
    let want = -(-2.0 - at_1k);
    assert!((mag_db(&l, 4000.0, SR) - want).abs() < 0.6);
}

#[test]
fn amount_scales_the_correction_in_db() {
    let text =
        "Filter 1: ON PK Fc 1000 Hz Gain 4 dB Q 1\nFilter 2: ON LSC Fc 120 Hz Gain -3 dB Q 0.7\n";
    let measure = |amount: f32| {
        let mut h = device();
        h.set_param("auto_gain", 0.0);
        h.set_param("amount", amount);
        let _ = h.device_mut().swap_player_data(Some(data(text)));
        settle(&mut h);
        impulse(&mut h).0
    };
    let (one, two, half, zero) = (measure(1.0), measure(2.0), measure(0.5), measure(0.0));
    for f in test_freqs() {
        let (a, b, c) = (
            mag_db(&one, f, SR),
            mag_db(&two, f, SR),
            mag_db(&half, f, SR),
        );
        assert!((b - 2.0 * a).abs() < 0.5, "{f}: {b} vs 2 * {a}");
        assert!((c - 0.5 * a).abs() < 0.5, "{f}: {c} vs 0.5 * {a}");
        assert!(mag_db(&zero, f, SR).abs() < 0.01, "{f}");
    }
}

#[test]
fn amount_zero_is_a_passthrough_within_tolerance() {
    let mut h = device();
    h.set_param("amount", 0.0);
    let _ = h.device_mut().swap_player_data(Some(data(PARAMETRIC)));
    settle(&mut h);
    let mut rng = Rng::new(3);
    let x: Vec<f32> = (0..BATCH * 100).map(|_| rng.next_bipolar() * 0.5).collect();
    let y = run_signal(&mut h, &["in_l", "in_r"], |n| x[n], 100, &["out_l"]);
    for (a, b) in x.iter().zip(&y[0]) {
        assert!((a - b).abs() < 1e-5, "{a} {b}");
    }
    assert_eq!(meters(&h)[meter::PREAMP], 0.0);
}

#[test]
fn simulate_gives_the_inverse() {
    let measure = |simulate: f32| {
        let mut h = device();
        h.set_param("auto_gain", 0.0);
        h.set_param("simulate", simulate);
        let _ = h
            .device_mut()
            .swap_player_data(Some(data(&synthetic_csv())));
        settle(&mut h);
        impulse(&mut h).0
    };
    let (normal, inverse) = (measure(0.0), measure(1.0));
    for f in test_freqs() {
        let (a, b) = (mag_db(&normal, f, SR), mag_db(&inverse, f, SR));
        assert!((a + b).abs() < 0.5, "{f}: {a} + {b}");
    }
    // The simulation is the response levelled at 1 kHz, against the (flat) target.
    assert_curve(
        &inverse,
        SR,
        &|f| synthetic(f) - synthetic(1000.0),
        0.5,
        "simulate",
    );
}

#[test]
fn the_automatic_preamp_keeps_the_gain_at_or_below_zero() {
    for text in [
        PARAMETRIC,
        "Filter 1: ON PK Fc 2000 Hz Gain 12 dB Q 2\n",
        "20 -10\n1000 0\n20000 8\n",
    ] {
        let mut h = with(text);
        let (l, _) = impulse(&mut h);
        let peak = (0..400)
            .map(|i| mag_db(&l, 25.0 * 640.0f64.powf(f64::from(i) / 399.0), SR))
            .fold(f64::MIN, f64::max);
        assert!(peak < 0.3, "{text}: peak {peak} dB");
        assert!(meters(&h)[meter::PREAMP] <= 0.0);
    }
    // With auto gain off the same boost comes through.
    let mut h = device();
    h.set_param("auto_gain", 0.0);
    let _ = h
        .device_mut()
        .swap_player_data(Some(data("Filter 1: ON PK Fc 2000 Hz Gain 12 dB Q 2\n")));
    settle(&mut h);
    let (l, _) = impulse(&mut h);
    assert!((mag_db(&l, 2000.0, SR) - 12.0).abs() < 0.5);
}

#[test]
fn the_correction_is_limited_to_twenty_db() {
    let mut h = device();
    h.set_param("auto_gain", 0.0);
    h.set_param("amount", 2.0);
    let _ = h
        .device_mut()
        .swap_player_data(Some(data("Filter 1: ON PK Fc 1000 Hz Gain 18 dB Q 1\n")));
    settle(&mut h);
    let (l, _) = impulse(&mut h);
    let g = mag_db(&l, 1000.0, SR);
    assert!((g - f64::from(LIMIT_DB)).abs() < 0.7, "{g}");
}

#[test]
fn output_gain_is_applied_last() {
    let mut h = device();
    h.set_param("output", -6.0);
    let x = run_signal(&mut h, &["in_l", "in_r"], |_| 0.5, 100, &["out_l"]);
    assert!((x[0][BATCH * 99] - 0.5 * 0.501_187).abs() < 1e-4);
    let mut h = with(PARAMETRIC);
    let (l0, _) = impulse(&mut h);
    h.set_param("output", 6.0);
    for _ in 0..600 {
        h.process();
    }
    let (l1, _) = impulse(&mut h);
    assert!((mag_db(&l1, 1000.0, SR) - mag_db(&l0, 1000.0, SR) - 6.0).abs() < 0.05);
}

#[test]
fn a_third_column_gives_each_side_its_own_curve() {
    let text = "freq, left, right\n20, 0, 0\n200, 6, -6\n2000, 6, -6\n20000, 0, 0\n";
    let mut h = device();
    h.set_param("file_kind", 1.0);
    h.set_param("auto_gain", 0.0);
    let _ = h.device_mut().swap_player_data(Some(data(text)));
    settle(&mut h);
    let (l, r) = impulse(&mut h);
    assert!((mag_db(&l, 800.0, SR) - 6.0).abs() < 0.5);
    assert!((mag_db(&r, 800.0, SR) + 6.0).abs() < 0.5);
}

#[test]
fn a_new_filter_never_clicks() {
    // A 1 kHz sine through the device; the correction changes mid-stream in several ways. The
    // output is a smooth sine whose gain moves, so no step may exceed the slope of a sine
    // of the loudest gain.
    let amp = 0.4f64;
    let slope = |gain: f64| amp * gain * std::f64::consts::TAU * 1000.0 / SR;
    let mut h = with(PARAMETRIC);
    h.set_param("auto_gain", 0.0);
    let mut worst = 0.0f64;
    let mut last = 0.0f32;
    let mut n0 = 0usize;
    let mut run = |h: &mut DeviceHarness, batches: usize, worst: &mut f64| {
        let y = run_signal(
            h,
            &["in_l", "in_r"],
            |n| (amp * (std::f64::consts::TAU * 1000.0 * (n0 + n) as f64 / SR).sin()) as f32,
            batches,
            &["out_l"],
        );
        n0 += batches * BATCH;
        for v in &y[0] {
            *worst = worst.max(f64::from((v - last).abs()));
            last = *v;
        }
    };
    run(&mut h, 200, &mut worst);
    // The first batches fade the filter in: measure the steady state after it.
    worst = 0.0;
    run(&mut h, 200, &mut worst);
    assert!(worst <= slope(1.6) * 1.02, "{worst}");
    worst = 0.0;
    // amount, target, kind, simulate, auto gain and a data swap, each a rebuild.
    h.set_param("amount", 0.3);
    run(&mut h, 120, &mut worst);
    h.set_param("amount", 1.7);
    run(&mut h, 120, &mut worst);
    h.set_param("simulate", 1.0);
    run(&mut h, 120, &mut worst);
    h.set_param("simulate", 0.0);
    h.set_param("auto_gain", 1.0);
    run(&mut h, 120, &mut worst);
    let _ = h
        .device_mut()
        .swap_player_data(Some(data(&synthetic_csv())));
    run(&mut h, 120, &mut worst);
    h.set_param("target", 1.0);
    run(&mut h, 120, &mut worst);
    // Many changes in a row (a knob drag).
    for i in 0..60 {
        h.set_param("amount", 0.5 + 0.02 * i as f32);
        run(&mut h, 3, &mut worst);
    }
    run(&mut h, 120, &mut worst);
    // The data going away and coming back fades.
    let _ = h.device_mut().swap_player_data(None);
    run(&mut h, 120, &mut worst);
    let _ = h.device_mut().swap_player_data(Some(data(PARAMETRIC)));
    run(&mut h, 200, &mut worst);
    assert!(worst <= slope(3.0), "largest step {worst} is a click");
}

#[test]
fn removing_the_data_returns_to_the_exact_passthrough() {
    let mut h = with(PARAMETRIC);
    let old = h.device_mut().swap_player_data(None);
    assert!(old.is_some());
    for _ in 0..200 {
        h.process();
    }
    let mut rng = Rng::new(11);
    let x: Vec<f32> = (0..BATCH * 20).map(|_| rng.next_bipolar() * 0.7).collect();
    let y = run_signal(&mut h, &["in_l", "in_r"], |n| x[n], 20, &["out_l", "out_r"]);
    for (a, b) in x.iter().zip(&y[0]) {
        assert_eq!(a.to_bits(), b.to_bits());
    }
    assert_eq!(meters(&h)[meter::LOADED], 0.0);
    assert_eq!(meters(&h)[meter::PREAMP], 0.0);
}

#[test]
fn data_that_is_not_headphone_data_is_handed_back() {
    let mut h = device();
    let drums = crate::test_support::drum_player_data(1);
    let back = h.device_mut().swap_player_data(Some(drums));
    assert!(back.is_some());
    // The device still has no data.
    h.process();
    assert_eq!(meters(&h)[meter::LOADED], 0.0);
    let mut h = device();
    assert!(h.device_mut().swap_player_data(None).is_none());
    assert!(
        h.device_mut()
            .swap_player_data(Some(data(PARAMETRIC)))
            .is_none()
    );
    let old = h.device_mut().swap_player_data(Some(data(PARAMETRIC)));
    assert!(old.is_some());
}

#[test]
fn malformed_data_is_a_passthrough_and_never_panics() {
    use atomik_device_sdk::headphone::{HeadphoneBody, HeadphoneData, ResponseData};
    let bad = HeadphoneData {
        name: "bad".into(),
        body: HeadphoneBody::Response(ResponseData {
            freq: vec![100.0],
            left: vec![],
            right: None,
        }),
    };
    let mut h = device();
    let _ = h
        .device_mut()
        .swap_player_data(Some(Arc::new(PlayerData::Headphone(bad))));
    let y = run_signal(&mut h, &["in_l", "in_r"], |_| 0.25, 50, &["out_l"]);
    assert!(y[0].iter().all(|v| *v == 0.25));
}

#[test]
fn the_filter_follows_the_sample_rate() {
    for sr in [44_100.0, 96_000.0] {
        let mut h = DeviceHarness::new(Box::new(HeadphoneCorrection::new()), sr);
        let _ = h.device_mut().swap_player_data(Some(data(PARAMETRIC)));
        settle(&mut h);
        let (l, _) = impulse(&mut h);
        let boost = max_db(|f| parametric_db(f, sr)).max(0.0);
        assert_curve(
            &l,
            sr,
            &|f| parametric_db(f, sr) - boost,
            0.5,
            "sample rate",
        );
    }
    // A running device that is prepared again at another rate builds the filter again.
    let mut h = with(PARAMETRIC);
    let info = PrepareInfo {
        sample_rate: atomik_core::SampleRate::new(96_000.0).unwrap(),
        seed: 0,
    };
    h.device_mut().prepare(&info);
    for _ in 0..600 {
        h.process();
        if meters(&h)[meter::BUSY] == 0.0 && meters(&h)[meter::PREAMP] != 0.0 {
            break;
        }
    }
    for _ in 0..50 {
        h.process();
    }
    let (l, _) = impulse(&mut h);
    let boost = max_db(|f| parametric_db(f, 96_000.0)).max(0.0);
    assert_curve(
        &l,
        96_000.0,
        &|f| parametric_db(f, 96_000.0) - boost,
        0.5,
        "re-prepared",
    );
}

#[test]
fn a_bounce_is_the_same_every_time() {
    let run = || {
        let mut h = device();
        let _ = h.device_mut().swap_player_data(Some(data(PARAMETRIC)));
        let mut rng = Rng::new(5);
        let x: Vec<f32> = (0..BATCH * 300).map(|_| rng.next_bipolar() * 0.3).collect();
        let y = run_signal(&mut h, &["in_l", "in_r"], |n| x[n], 300, &["out_l"]);
        y[0].iter().map(|v| v.to_bits()).collect::<Vec<_>>()
    };
    assert_eq!(run(), run());
}

#[test]
fn the_snapshot_gives_the_curves_a_panel_draws() {
    let d = parse(&synthetic_csv(), "x").unwrap();
    let s = snapshot(&d, 0, &Settings::default(), 48_000.0, 200);
    assert_eq!(s.freq.len(), 200);
    assert!((s.freq[0] - 20.0).abs() < 1e-3);
    assert!((s.freq[199] - 20_000.0).abs() < 20.0);
    // The response is levelled at 1 kHz; the correction is its mirror under a flat target.
    let i = s.freq.iter().position(|f| *f >= 1000.0).unwrap();
    assert!(s.response[i].abs() < 0.4);
    for k in 0..200 {
        assert!((s.response[k] + s.correction[k]).abs() < 1e-3);
        assert!(s.target[k].abs() < 1e-6);
    }
    assert!(s.max_boost_db >= 0.0);
    assert!((s.preamp_db + s.max_boost_db).abs() < 1e-6);
    // Simulate negates it; the studio target shows up.
    let sim = Settings {
        simulate: true,
        target: Target::Studio,
        ..Settings::default()
    };
    let t = snapshot(&d, 0, &sim, 48_000.0, 200);
    assert!(t.target.iter().any(|v| v.abs() > 1.0));
    // A parametric file has no target.
    let p = parse(PARAMETRIC, "p").unwrap();
    let ps = snapshot(&p, 0, &Settings::default(), 48_000.0, 50);
    assert!(ps.target.iter().all(|v| *v == 0.0));
}

#[test]
fn the_grid_and_the_design_hold_their_contracts() {
    assert_eq!(GRID_LEN, 961);
    assert!((grid_freq(0) - 20.0).abs() < 1e-9);
    assert!((grid_freq(960) - 20_480.0).abs() < 1e-3);
    // A flat curve is an exact unit impulse.
    let d = parse("20 5\n20000 5\n", "flat").unwrap();
    let mut design = Design::new();
    design.begin(&d, 0, &Settings::default(), 48_000.0);
    let mut ir = vec![0.0f32; TAPS];
    let mut steps = 0;
    while !design.step(&d, &mut ir) {
        steps += 1;
        assert!(steps < 100);
    }
    assert!(!design.busy());
    assert_eq!(ir[0], 1.0);
    assert!(ir[1..].iter().all(|v| *v == 0.0));
    // A curve takes a bounded number of steps and its energy is at the start (minimum phase).
    let d = parse(PARAMETRIC, "p").unwrap();
    design.begin(&d, 0, &Settings::default(), 48_000.0);
    steps = 0;
    while !design.step(&d, &mut ir) {
        steps += 1;
    }
    assert!(steps <= 25, "{steps} steps");
    let head: f64 = ir[..256].iter().map(|v| f64::from(*v).powi(2)).sum();
    let tail: f64 = ir[2048..].iter().map(|v| f64::from(*v).powi(2)).sum();
    assert!(tail < head * 1e-4, "{head} {tail}");
}

#[test]
fn curve_settings_follow_the_documented_order() {
    let d = parse("20 0\n1000 0\n20000 6\n", "x").unwrap();
    let mut c = Curve::new();
    // Measured: a response that rises 6 dB at the top is corrected by a 6 dB cut there.
    c.compute(&d, 0, &Settings::default(), 48_000.0);
    assert!((c.curve_at(20_000.0) + 6.0).abs() < 0.1);
    assert!(c.curve_at(100.0).abs() < 0.1);
    assert!(c.max_boost_db < 0.1);
    assert!((c.preamp_db + c.max_boost_db).abs() < 1e-6);
    // As a correction it is used as it is: a boost, and the preamp takes it back.
    let s = Settings {
        kind: DataKind::Correction,
        ..Settings::default()
    };
    c.compute(&d, 0, &s, 48_000.0);
    assert!((c.curve_at(20_000.0) - 6.0).abs() < 0.1);
    assert!((c.max_boost_db - 6.0).abs() < 0.1);
    assert!((c.preamp_db + c.max_boost_db).abs() < 1e-5);
    // Simulate negates; the amount scales before the limit.
    let s = Settings {
        kind: DataKind::Correction,
        simulate: true,
        amount: 2.0,
        auto_gain: false,
        ..Settings::default()
    };
    c.compute(&d, 0, &s, 48_000.0);
    assert!((c.curve_at(20_000.0) + 12.0).abs() < 0.2);
    assert_eq!(c.preamp_db, 0.0);
    assert!(!c.flat);
    let flat = Settings {
        amount: 0.0,
        ..Settings::default()
    };
    c.compute(&d, 0, &flat, 48_000.0);
    assert!(c.flat);
}

#[test]
fn a_tone_through_a_loaded_device_has_the_expected_gain() {
    let mut h = device();
    h.set_param("auto_gain", 0.0);
    let _ = h
        .device_mut()
        .swap_player_data(Some(data("Filter 1: ON PK Fc 1000 Hz Gain 6 dB Q 1\n")));
    settle(&mut h);
    let y = run_signal(
        &mut h,
        &["in_l", "in_r"],
        sine(1000.0, 0.25),
        400,
        &["out_l"],
    );
    let tail = &y[0][y[0].len() - 4800..];
    let peak = tail.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    assert!((peak - 0.25 * 1.995).abs() < 0.02, "{peak}");
}
