//! Gain tool tests, all through `DeviceHarness` (which guards every `process` against allocation).
#![allow(clippy::float_cmp)]

use super::*;
use crate::test_support::{SR, assert_keys, harness, sweep_params_min_max};
use atomik_testkit::harness::DeviceHarness;

fn tool() -> DeviceHarness {
    harness(Box::new(GainTool::new()))
}

/// A deterministic test signal.
fn noise(n: usize, seed: u32) -> f32 {
    let mut x = (n as u32).wrapping_mul(2_654_435_761).wrapping_add(seed);
    x ^= x >> 15;
    x = x.wrapping_mul(2_246_822_519);
    x ^= x >> 13;
    (x as f32 / u32::MAX as f32) * 1.6 - 0.8
}

/// Runs `batches` batches of `l(n)` / `r(n)` on `in_l` / `in_r` and returns the outputs.
fn run(
    h: &mut DeviceHarness,
    l: impl Fn(usize) -> f32,
    r: impl Fn(usize) -> f32,
    batches: usize,
) -> (Vec<f32>, Vec<f32>) {
    let (mut out_l, mut out_r) = (Vec::new(), Vec::new());
    for b in 0..batches {
        let mut xl = [0.0; BATCH];
        let mut xr = [0.0; BATCH];
        for i in 0..BATCH {
            xl[i] = l(b * BATCH + i);
            xr[i] = r(b * BATCH + i);
        }
        *h.input_mut("in_l") = xl;
        *h.input_mut("in_r") = xr;
        h.process();
        out_l.extend_from_slice(h.output("out_l"));
        out_r.extend_from_slice(h.output("out_r"));
    }
    (out_l, out_r)
}

fn last(v: &[f32]) -> f32 {
    v[v.len() - 1]
}

#[test]
fn descriptor_validates() {
    assert_keys(
        &DESCRIPTOR,
        &[
            "mode",
            "gain",
            "pan",
            "width",
            "blend",
            "mute",
            "gain_cv_trim",
            "pan_cv_trim",
            "bypass",
        ],
        &[
            "in_l",
            "in_r",
            "aux_l",
            "aux_r",
            "out_l",
            "out_r",
            "gain_cv_in",
            "pan_cv_in",
        ],
    );
    assert_eq!(DESCRIPTOR.type_id, "atomik.gain-tool");
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Effect);
}

#[test]
fn defaults_pass_the_input_bit_for_bit() {
    let mut h = tool();
    let (l, r) = run(&mut h, |n| noise(n, 1), |n| noise(n, 2), 40);
    for n in 0..l.len() {
        assert_eq!(l[n].to_bits(), noise(n, 1).to_bits());
        assert_eq!(r[n].to_bits(), noise(n, 2).to_bits());
    }
}

#[test]
fn gain_scales_by_the_decibels_and_settles() {
    let mut h = tool();
    h.set_param("gain", 6.020_6);
    let (l, r) = run(&mut h, |_| 0.25, |_| -0.1, 80);
    assert!((last(&l) - 0.5).abs() < 1e-4, "{}", last(&l));
    assert!((last(&r) + 0.2).abs() < 1e-4, "{}", last(&r));
    h.set_param("gain", -20.0);
    let (l, _) = run(&mut h, |_| 0.5, |_| 0.0, 200);
    assert!((last(&l) - 0.05).abs() < 1e-5, "{}", last(&l));
}

#[test]
fn mute_ramps_to_silence_and_back() {
    let mut h = tool();
    run(&mut h, |_| 0.5, |_| 0.5, 4);
    h.set_param("mute", 1.0);
    let (l, _) = run(&mut h, |_| 0.5, |_| 0.5, 120);
    assert_eq!(last(&l), 0.0);
    // The first batch after the switch does not jump to silence.
    assert!(l[0] > 0.3, "{}", l[0]);
    h.set_param("mute", 0.0);
    let (l, _) = run(&mut h, |_| 0.5, |_| 0.5, 120);
    assert_eq!(last(&l), 0.5);
}

#[test]
fn balance_silences_the_far_side() {
    let mut h = tool();
    h.set_param("pan", 1.0);
    let (l, r) = run(&mut h, |_| 0.5, |_| 0.5, 120);
    assert_eq!(last(&l), 0.0);
    assert_eq!(last(&r), 0.5);
    h.set_param("pan", -0.5);
    let (l, r) = run(&mut h, |_| 0.5, |_| 0.5, 120);
    assert_eq!(last(&l), 0.5);
    assert!((last(&r) - 0.25).abs() < 1e-6);
}

#[test]
fn width_zero_is_mono_and_two_doubles_the_side() {
    let mut h = tool();
    h.set_param("width", 0.0);
    let (l, r) = run(&mut h, |_| 0.8, |_| 0.2, 120);
    assert!((last(&l) - 0.5).abs() < 1e-6 && (last(&r) - 0.5).abs() < 1e-6);
    h.set_param("width", 2.0);
    let (l, r) = run(&mut h, |_| 0.8, |_| 0.2, 120);
    // M = 0.5, S = 0.3 -> 0.5 +/- 0.6.
    assert!((last(&l) - 1.1).abs() < 1e-6 && (last(&r) + 0.1).abs() < 1e-6);
}

#[test]
fn mix_adds_the_aux_and_xfade_blends_to_it() {
    let mut h = tool();
    h.set_param("mode", 1.0);
    h.set_param("blend", 0.5);
    *h.input_mut("aux_l") = [0.4; BATCH];
    *h.input_mut("aux_r") = [-0.4; BATCH];
    let (l, r) = run(&mut h, |_| 0.2, |_| 0.2, 120);
    assert!((last(&l) - 0.4).abs() < 1e-6 && (last(&r) - 0.0).abs() < 1e-6);

    let mut h = tool();
    h.set_param("mode", 2.0);
    h.set_param("blend", 0.25);
    *h.input_mut("aux_l") = [1.0; BATCH];
    *h.input_mut("aux_r") = [1.0; BATCH];
    let (l, _) = run(&mut h, |_| 0.0, |_| 0.0, 120);
    assert!((last(&l) - 0.25).abs() < 1e-6);
    // Blend 0 is the input alone, bit for bit.
    h.set_param("blend", 0.0);
    let (l, _) = run(&mut h, |n| noise(n, 3), |_| 0.0, 120);
    assert_eq!(last(&l).to_bits(), noise(120 * BATCH - 1, 3).to_bits());
}

#[test]
fn cv_moves_the_gain_by_twenty_four_decibels_at_full_trim() {
    let mut h = tool();
    *h.input_mut("gain_cv_in") = [0.5; BATCH];
    let (l, _) = run(&mut h, |_| 0.1, |_| 0.1, 120);
    // +12 dB.
    assert!((last(&l) - 0.1 * 3.981_07).abs() < 1e-3, "{}", last(&l));
    h.set_param("gain_cv_trim", 0.0);
    let (l, _) = run(&mut h, |_| 0.1, |_| 0.1, 120);
    assert!((last(&l) - 0.1).abs() < 1e-6);
    // The balance CV: +1 at full trim is hard right.
    h.set_param("pan_cv_trim", 1.0);
    *h.input_mut("pan_cv_in") = [1.0; BATCH];
    let (l, r) = run(&mut h, |_| 0.3, |_| 0.3, 120);
    assert_eq!(last(&l), 0.0);
    assert!((last(&r) - 0.3).abs() < 1e-6);
}

#[test]
fn non_finite_input_is_treated_as_silence() {
    let mut h = tool();
    h.set_param("gain", 12.0);
    let (l, r) = run(&mut h, |_| f32::NAN, |_| f32::INFINITY, 4);
    assert!(l.iter().chain(&r).all(|x| *x == 0.0));
}

#[test]
fn the_registered_device_has_a_bypass_that_passes_the_input() {
    let f = factory();
    let mut h = DeviceHarness::new((f.create)(), SR);
    h.set_param("gain", 12.0);
    h.set_param("bypass", 1.0);
    let (l, _) = run(&mut h, |n| noise(n, 5), |_| 0.0, 40);
    assert_eq!(last(&l).to_bits(), noise(40 * BATCH - 1, 5).to_bits());
}

#[test]
fn params_at_their_limits_keep_the_output_finite() {
    let mut h = tool();
    *h.input_mut("in_l") = [0.9; BATCH];
    *h.input_mut("in_r") = [-0.9; BATCH];
    sweep_params_min_max(&mut h, &DESCRIPTOR);
}
