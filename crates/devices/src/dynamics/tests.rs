//! Gate + compressor tests, all through `DeviceHarness`.
#![allow(clippy::float_cmp)]

use super::*;
use crate::test_support::{SR, assert_keys, harness, sine, sweep_params_min_max, tone_amplitude};
use atomik_testkit::harness::DeviceHarness;

fn dynamics() -> DeviceHarness {
    harness(Box::new(Dynamics::new()))
}

fn run(
    h: &mut DeviceHarness,
    signal: impl Fn(usize) -> f32,
    batches: usize,
) -> (Vec<f32>, Vec<f32>) {
    let (mut out, mut cv) = (Vec::new(), Vec::new());
    for b in 0..batches {
        let mut x = [0.0; BATCH];
        for (i, s) in x.iter_mut().enumerate() {
            *s = signal(b * BATCH + i);
        }
        *h.input_mut("in_l") = x;
        *h.input_mut("in_r") = x;
        h.process();
        out.extend_from_slice(h.output("out_l"));
        cv.extend_from_slice(h.output("gr_cv_out"));
    }
    (out, cv)
}

#[test]
fn descriptor_validates() {
    assert_keys(
        &DESCRIPTOR,
        &[
            "in_gain",
            "gate_threshold",
            "gate_range",
            "gate_release",
            "threshold",
            "ratio",
            "knee",
            "attack",
            "release",
            "makeup",
            "mix",
            "bypass",
        ],
        &["in_l", "in_r", "out_l", "out_r", "gr_cv_out"],
    );
    assert_eq!(DESCRIPTOR.type_id, "atomik.dynamics");
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Effect);
}

#[test]
fn the_defaults_pass_the_input_bit_for_bit() {
    let mut h = dynamics();
    let tone = sine(333.0, 0.9);
    let (out, cv) = run(&mut h, &tone, 200);
    for (n, y) in out.iter().enumerate() {
        assert_eq!(y.to_bits(), tone(n).to_bits(), "sample {n}");
    }
    assert!(cv.iter().all(|c| *c == 0.0));
}

#[test]
fn the_compressor_follows_the_static_curve() {
    let mut h = dynamics();
    h.set_param("threshold", -20.0);
    h.set_param("ratio", 4.0);
    h.set_param("knee", 0.0);
    h.set_param("attack", 1.0);
    // A 1 kHz sine at -8 dB: 12 dB over the threshold, 9 dB of reduction at 4:1.
    let amp = 10f64.powf(-8.0 / 20.0);
    let batches = (SR as usize) / BATCH;
    let (out, _) = run(&mut h, sine(1_000.0, amp), batches);
    let start = out.len() - 4_800;
    let got = tone_amplitude(&out[start..], 1_000.0, start);
    let want = amp * 10f64.powf(-9.0 / 20.0);
    assert!((got / want - 1.0).abs() < 0.03, "{got} vs {want}");
}

#[test]
fn the_gate_runs_before_the_compressor() {
    // A quiet signal under the gate threshold is closed down by the range even though the
    // makeup after it would lift it: the output is far under the input.
    let mut h = dynamics();
    h.set_param("gate_threshold", -30.0);
    h.set_param("gate_range", 60.0);
    h.set_param("gate_release", 5.0);
    h.set_param("makeup", 6.0);
    let (out, _) = run(&mut h, |_| 0.001, 400);
    assert!(out[out.len() - 1] < 0.000_01, "{}", out[out.len() - 1]);
}

#[test]
fn input_gain_scales_before_the_stages_and_mix_zero_is_dry() {
    let mut h = dynamics();
    h.set_param("in_gain", 6.020_6);
    let (out, _) = run(&mut h, |_| 0.2, 200);
    assert!((out[out.len() - 1] - 0.4).abs() < 1e-4);
    h.set_param("mix", 0.0);
    let (out, _) = run(&mut h, |_| 0.2, 200);
    assert_eq!(out[out.len() - 1], 0.2);
}

#[test]
fn gr_cv_rises_with_reduction() {
    let mut h = dynamics();
    h.set_param("threshold", -30.0);
    h.set_param("ratio", 10.0);
    let (_, cv) = run(&mut h, |_| 0.5, 400);
    assert!(cv[cv.len() - 1] > 0.3, "{}", cv[cv.len() - 1]);
}

#[test]
fn the_registered_device_bypasses_to_the_input() {
    let mut h = DeviceHarness::new((factory().create)(), SR);
    h.set_param("threshold", -40.0);
    h.set_param("ratio", 20.0);
    h.set_param("bypass", 1.0);
    let (out, _) = run(&mut h, |_| 0.5, 200);
    assert_eq!(out[out.len() - 1], 0.5);
}

#[test]
fn params_at_their_limits_keep_the_output_finite() {
    let mut h = dynamics();
    *h.input_mut("in_l") = [0.9; BATCH];
    *h.input_mut("in_r") = [-0.3; BATCH];
    sweep_params_min_max(&mut h, &DESCRIPTOR);
}
