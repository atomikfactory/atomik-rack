//! De-esser tests, all through `DeviceHarness` (every `process` is allocation-guarded).
#![allow(clippy::float_cmp)]

use super::*;
use crate::test_support::{
    SR, assert_keys, harness, run_signal, sine, sweep_params_min_max, tone_amplitude,
};
use atomik_testkit::harness::DeviceHarness;

fn esser() -> DeviceHarness {
    harness(Box::new(DeEsser::new()))
}

fn ms(t: f64) -> usize {
    (t * 0.001 * SR).round() as usize
}

fn db(x: f64) -> f64 {
    20.0 * x.max(1e-12).log10()
}

/// Runs `signal` (both channels) for `samples` and returns `(out_l, gr_cv_out)`.
fn run(
    h: &mut DeviceHarness,
    signal: impl Fn(usize) -> f32,
    samples: usize,
) -> (Vec<f32>, Vec<f32>) {
    let mut o = run_signal(
        h,
        &["in_l", "in_r"],
        signal,
        samples.div_ceil(BATCH),
        &["out_l", "gr_cv_out"],
    );
    let gr = o.pop().unwrap();
    (o.pop().unwrap(), gr)
}

/// The level in dB of the `hz` component over the last 100 ms (whole cycles at integer kHz).
fn tail_db(y: &[f32], hz: f64) -> f64 {
    let n = ms(100.0);
    let start = y.len() - n;
    db(tone_amplitude(&y[start..], hz, start))
}

fn two(a: f64, hz_a: f64, b: f64, hz_b: f64) -> impl Fn(usize) -> f32 {
    let (x, y) = (sine(hz_a, a), sine(hz_b, b));
    move |n| x(n) + y(n)
}

#[test]
fn descriptor_validates() {
    assert_keys(
        &DESCRIPTOR,
        &[
            "freq",
            "q",
            "threshold",
            "ratio",
            "range",
            "attack",
            "release",
            "mode",
            "listen",
            "freq_cv_trim",
            "bypass",
        ],
        &["in_l", "in_r", "freq_cv_in", "out_l", "out_r", "gr_cv_out"],
    );
    assert_eq!(DESCRIPTOR.type_id, "atomik.de-esser");
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Effect);
}

#[test]
fn a_signal_under_the_threshold_passes_bit_for_bit() {
    let mut h = esser();
    let tone = sine(6_500.0, 0.01); // -40 dB, threshold -30
    let (y, gr) = run(&mut h, &tone, ms(300.0));
    for (n, v) in y.iter().enumerate() {
        assert_eq!(v.to_bits(), tone(n).to_bits(), "sample {n}");
    }
    assert!(gr.iter().all(|g| *g == 0.0));
}

#[test]
fn a_loud_sibilant_tone_is_turned_down_by_the_curve_up_to_the_range() {
    // 0.5 is -6 dB: 24 dB over the threshold, ratio 6 -> 20 dB, limited to the 12 dB range.
    let mut h = esser();
    let (y, _) = run(&mut h, sine(6_500.0, 0.5), ms(500.0));
    let cut = db(0.5) - tail_db(&y, 6_500.0);
    assert!((cut - 12.0).abs() < 1.5, "{cut} dB");
    // With a smaller range it stops there.
    let mut h = esser();
    h.set_param("range", 5.0);
    let (y, _) = run(&mut h, sine(6_500.0, 0.5), ms(500.0));
    let cut = db(0.5) - tail_db(&y, 6_500.0);
    assert!((cut - 5.0).abs() < 1.0, "{cut} dB");
    // A mild overshoot follows the ratio: -20 dB is 10 dB over -> 10 * (1 - 1/3) = 6.7 dB.
    let mut h = esser();
    h.set_param("ratio", 3.0);
    let (y, _) = run(&mut h, sine(6_500.0, 0.1), ms(500.0));
    let cut = db(0.1) - tail_db(&y, 6_500.0);
    assert!((cut - 6.7).abs() < 1.2, "{cut} dB");
}

#[test]
fn split_mode_leaves_the_rest_of_the_spectrum_alone() {
    let mut h = esser();
    let (y, _) = run(&mut h, two(0.5, 6_500.0, 0.3, 200.0), ms(500.0));
    let low = db(0.3) - tail_db(&y, 200.0);
    assert!(low.abs() < 0.5, "the 200 Hz tone moved {low} dB");
    let high = db(0.5) - tail_db(&y, 6_500.0);
    assert!(high > 8.0, "{high} dB");
}

#[test]
fn wideband_mode_turns_everything_down_when_the_band_is_loud() {
    let mut h = esser();
    h.set_param("mode", 1.0);
    let (y, _) = run(&mut h, two(0.5, 6_500.0, 0.3, 200.0), ms(500.0));
    let low = db(0.3) - tail_db(&y, 200.0);
    assert!((low - 12.0).abs() < 1.5, "{low} dB");
    // The low tone alone does not trip the detector.
    let mut h = esser();
    h.set_param("mode", 1.0);
    let (y, gr) = run(&mut h, sine(200.0, 0.5), ms(500.0));
    assert!(db(0.5) - tail_db(&y, 200.0) < 0.5);
    assert!(gr[gr.len() - 1] < 0.05);
}

#[test]
fn listen_outputs_the_detector_band() {
    let mut h = esser();
    h.set_param("listen", 1.0);
    let (y, _) = run(&mut h, two(0.5, 6_500.0, 0.5, 200.0), ms(300.0));
    assert!(tail_db(&y, 6_500.0) > db(0.5) - 0.5, "the centre passes");
    assert!(tail_db(&y, 200.0) < db(0.5) - 25.0, "the low tone does not");
}

#[test]
fn the_reduction_comes_in_with_the_attack_and_goes_with_the_release() {
    // A slow attack: a mild overshoot (-24 dB against -30) takes its time to be reached.
    let mut h = esser();
    h.set_param("attack", 20.0);
    let tone = sine(6_500.0, 0.063);
    let (_, gr) = run(&mut h, &tone, ms(300.0));
    assert!(gr[ms(5.0)] < gr[ms(100.0)] * 0.6, "builds up");
    assert!(gr[ms(250.0)] > 0.1);
    let mut h = esser();
    h.set_param("attack", 1.0);
    h.set_param("release", 100.0);
    let tone = sine(6_500.0, 0.5);
    let on = ms(200.0);
    let (_, gr) = run(&mut h, |n| if n < on { tone(n) } else { 0.0 }, ms(700.0));
    assert!(gr[ms(150.0)] > 0.5, "reduced while loud");
    assert!(
        gr[on + ms(10.0)] > 0.3,
        "still reduced right after the stop"
    );
    assert!(gr[ms(650.0)] < 0.02, "released");
}

#[test]
fn the_frequency_cv_moves_the_band() {
    // A 3.25 kHz tone is outside the 6.5 kHz band but inside the band moved down an octave.
    let cut = |cv: f32| {
        let mut h = esser();
        let tone = sine(3_250.0, 0.5);
        let mut y = Vec::new();
        for b in 0..ms(500.0).div_ceil(BATCH) {
            let mut x = [0.0f32; BATCH];
            for (i, s) in x.iter_mut().enumerate() {
                *s = tone(b * BATCH + i);
            }
            *h.input_mut("in_l") = x;
            *h.input_mut("in_r") = x;
            *h.input_mut("freq_cv_in") = [cv; BATCH];
            h.process();
            y.extend_from_slice(h.output("out_l"));
        }
        db(0.5) - tail_db(&y, 3_250.0)
    };
    assert!(cut(0.0) < 3.0, "{}", cut(0.0));
    assert!(cut(-0.5) > 8.0, "{}", cut(-0.5));
}

#[test]
fn the_meter_and_the_cv_output_report_the_reduction() {
    let mut h = esser();
    let (_, gr) = run(&mut h, sine(6_500.0, 0.5), ms(300.0));
    let g = gr[gr.len() - 1];
    assert!((20.0 * (1.0 - g).log10() + 12.0).abs() < 1.5, "{g}");
    let mut m = [0.0f32; 3];
    assert_eq!(h.device().meter_values(&mut m), 3);
    assert!(
        m[0] > 0.4 && m[1] < m[0] && (m[2] - 12.0).abs() < 1.5,
        "{m:?}"
    );
}

#[test]
fn non_finite_input_counts_as_silence() {
    let mut h = esser();
    let (y, _) = run(
        &mut h,
        |n| {
            if n % 3 == 0 {
                f32::NAN
            } else {
                f32::NEG_INFINITY
            }
        },
        2_000,
    );
    assert!(y.iter().all(|v| *v == 0.0));
}

#[test]
fn every_parameter_extreme_stays_finite() {
    let mut h = esser();
    sweep_params_min_max(&mut h, &DESCRIPTOR);
}
