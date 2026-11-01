//! Envelope filter tests, all through `DeviceHarness` (every `process` is allocation-guarded).
#![allow(clippy::float_cmp)]

use super::*;
use crate::test_support::{SR, assert_keys, harness, off, on, sine, sweep_params_min_max};
use atomik_testkit::harness::DeviceHarness;

fn filt() -> DeviceHarness {
    harness(Box::new(EnvFilter::new()))
}

fn ms(t: f64) -> usize {
    (t * 0.001 * SR).round() as usize
}

/// Runs `samples` samples of `signal` on both inputs with the gate CV level `gate` (when `Some`)
/// and returns `(out_l, env_cv)`.
fn run(
    h: &mut DeviceHarness,
    signal: &dyn Fn(usize) -> f32,
    gate: Option<f32>,
    from: usize,
    samples: usize,
) -> (Vec<f32>, Vec<f32>) {
    let (mut out, mut env) = (Vec::new(), Vec::new());
    for b in 0..samples.div_ceil(BATCH) {
        let mut x = [0.0; BATCH];
        for (i, s) in x.iter_mut().enumerate() {
            *s = signal(from + b * BATCH + i);
        }
        *h.input_mut("in_l") = x;
        *h.input_mut("in_r") = x;
        if let Some(g) = gate {
            *h.input_mut("gate_cv_in") = [g; BATCH];
        }
        h.process();
        out.extend_from_slice(h.output("out_l"));
        env.extend_from_slice(h.output("env_cv_out"));
    }
    (out, env)
}

/// Peak of the last 20 ms of `y`.
fn tail_peak(y: &[f32]) -> f32 {
    y[y.len() - ms(20.0)..]
        .iter()
        .fold(0.0f32, |m, v| m.max(v.abs()))
}

#[test]
fn descriptor_validates() {
    assert_keys(
        &DESCRIPTOR,
        &[
            "mode",
            "cutoff",
            "resonance",
            "amount",
            "attack",
            "decay",
            "sustain",
            "release",
            "trigger",
            "threshold",
            "mix",
            "bypass",
        ],
        &[
            "in_l",
            "in_r",
            "notes_in",
            "gate_cv_in",
            "out_l",
            "out_r",
            "env_cv_out",
        ],
    );
    assert_eq!(DESCRIPTOR.type_id, "atomik.env-filter");
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Effect);
}

#[test]
fn without_a_gate_it_is_a_plain_filter_at_the_knob() {
    let mut h = filt();
    let (low, env) = run(&mut h, &sine(100.0, 0.5), None, 0, ms(300.0));
    assert!(tail_peak(&low) > 0.45, "{}", tail_peak(&low));
    assert!(env.iter().all(|e| *e == 0.0));
    let mut h = filt();
    let (high, _) = run(&mut h, &sine(5_000.0, 0.5), None, 0, ms(300.0));
    // Two octaves-plus above 400 Hz: well over 20 dB down.
    assert!(tail_peak(&high) < 0.05, "{}", tail_peak(&high));
}

#[test]
fn the_gate_cv_opens_the_filter_and_the_release_closes_it() {
    let mut h = filt();
    h.set_param("amount", 4.0);
    h.set_param("attack", 1.0);
    h.set_param("sustain", 1.0);
    h.set_param("release", 20.0);
    let tone = sine(3_000.0, 0.5);
    let (closed, _) = run(&mut h, &tone, Some(0.0), 0, ms(200.0));
    assert!(tail_peak(&closed) < 0.1, "{}", tail_peak(&closed));
    let (open, env) = run(&mut h, &tone, Some(1.0), ms(200.0), ms(300.0));
    assert!(tail_peak(&open) > 0.35, "{}", tail_peak(&open));
    assert!((env[env.len() - 1] - 1.0).abs() < 1e-3);
    let (shut, env) = run(&mut h, &tone, Some(0.0), ms(500.0), ms(300.0));
    assert!(tail_peak(&shut) < 0.1, "{}", tail_peak(&shut));
    assert_eq!(env[env.len() - 1], 0.0);
}

#[test]
fn the_gate_has_hysteresis() {
    let mut h = filt();
    h.set_param("sustain", 1.0);
    h.set_param("attack", 0.1);
    let (_, env) = run(&mut h, &|_| 0.0, Some(0.6), 0, ms(100.0));
    assert!(env[env.len() - 1] > 0.99);
    // 0.4 is under the rising threshold but above the falling one: stays open.
    let (_, env) = run(&mut h, &|_| 0.0, Some(0.4), ms(100.0), ms(100.0));
    assert!(env[env.len() - 1] > 0.99);
    let (_, env) = run(&mut h, &|_| 0.0, Some(0.1), ms(200.0), ms(2_000.0));
    assert!(env[env.len() - 1] < 0.01);
}

#[test]
fn notes_hold_the_gate_until_the_last_one_is_released() {
    let mut h = filt();
    h.set_param("sustain", 1.0);
    h.set_param("attack", 1.0);
    h.set_param("release", 10.0);
    h.push_note("notes_in", on(0, 60, 0.8));
    h.push_note("notes_in", on(10, 64, 0.8));
    let (_, env) = run(&mut h, &|_| 0.0, None, 0, ms(100.0));
    assert!(env[env.len() - 1] > 0.99);
    h.push_note("notes_in", off(0, 60));
    let (_, env) = run(&mut h, &|_| 0.0, None, ms(100.0), ms(100.0));
    assert!(env[env.len() - 1] > 0.99, "one key is still held");
    h.push_note("notes_in", off(0, 64));
    let (_, env) = run(&mut h, &|_| 0.0, None, ms(200.0), ms(300.0));
    assert_eq!(env[env.len() - 1], 0.0);
}

#[test]
fn a_note_on_inside_the_batch_starts_at_its_offset() {
    let mut h = filt();
    h.set_param("attack", 0.1);
    h.push_note("notes_in", on(32, 60, 0.8));
    let (_, env) = run(&mut h, &|_| 0.0, None, 0, BATCH);
    assert_eq!(env[31], 0.0);
    assert!(env[40] > 0.0);
}

#[test]
fn the_envelope_follows_the_adsr_times() {
    let mut h = filt();
    h.set_param("attack", 10.0);
    h.set_param("decay", 100.0);
    h.set_param("sustain", 0.5);
    let (_, env) = run(&mut h, &|_| 0.0, Some(1.0), 0, ms(600.0));
    // The attack reaches 1.0 at the attack time, then it decays to the sustain.
    assert!(env[ms(9.0)] < 1.0 && env[ms(11.0)] > 0.9);
    assert!((env[ms(500.0)] - 0.5).abs() < 0.02, "{}", env[ms(500.0)]);
}

#[test]
fn the_input_level_can_trigger_the_envelope() {
    let mut h = filt();
    h.set_param("trigger", 1.0);
    h.set_param("threshold", -30.0);
    h.set_param("sustain", 1.0);
    h.set_param("attack", 1.0);
    h.set_param("release", 10.0);
    let (_, env) = run(&mut h, &|_| 0.0, None, 0, ms(50.0));
    assert_eq!(env[env.len() - 1], 0.0);
    let (_, env) = run(&mut h, &sine(200.0, 0.3), None, 0, ms(100.0));
    assert!(env[env.len() - 1] > 0.9);
    let (_, env) = run(&mut h, &|_| 0.0, None, ms(100.0), ms(500.0));
    assert_eq!(env[env.len() - 1], 0.0);
}

#[test]
fn a_negative_amount_sweeps_the_cutoff_down() {
    let mut h = filt();
    h.set_param("cutoff", 4_000.0);
    h.set_param("amount", -4.0);
    h.set_param("attack", 1.0);
    h.set_param("sustain", 1.0);
    let tone = sine(2_000.0, 0.5);
    let (before, _) = run(&mut h, &tone, Some(0.0), 0, ms(200.0));
    let (after, _) = run(&mut h, &tone, Some(1.0), ms(200.0), ms(300.0));
    assert!(tail_peak(&before) > 0.4);
    assert!(tail_peak(&after) < 0.1, "{}", tail_peak(&after));
}

#[test]
fn highpass_and_notch_modes_do_what_they_say() {
    let tone_at = |mode: f32, hz: f64| {
        let mut h = filt();
        h.set_param("mode", mode);
        h.set_param("resonance", 0.1);
        let (y, _) = run(&mut h, &sine(hz, 0.5), None, 0, ms(300.0));
        tail_peak(&y)
    };
    assert!(tone_at(2.0, 100.0) < 0.05, "high-pass cuts the low tone");
    let hi = tone_at(2.0, 8_000.0);
    assert!(hi > 0.4, "and passes the high one: {hi}");
    assert!(tone_at(3.0, 400.0) < 0.1, "the notch cuts the cutoff");
    assert!(tone_at(3.0, 4_000.0) > 0.4, "and leaves the rest");
}

#[test]
fn mix_zero_is_the_input_bit_for_bit() {
    let mut h = filt();
    h.set_param("mix", 0.0);
    let tone = sine(777.0, 0.4);
    let (y, _) = run(&mut h, &tone, Some(1.0), 0, ms(50.0));
    for (n, v) in y.iter().enumerate() {
        assert_eq!(v.to_bits(), tone(n).to_bits());
    }
}

#[test]
fn the_registered_device_bypasses_to_the_input() {
    let mut h = DeviceHarness::new((factory().create)(), SR);
    h.set_param("bypass", 1.0);
    let tone = sine(5_000.0, 0.4);
    let (y, _) = run(&mut h, &tone, None, 0, ms(50.0));
    assert_eq!(y[ms(40.0)].to_bits(), tone(ms(40.0)).to_bits());
}

#[test]
fn params_at_their_limits_keep_the_output_finite() {
    let mut h = filt();
    *h.input_mut("in_l") = [0.9; BATCH];
    *h.input_mut("in_r") = [-0.3; BATCH];
    *h.input_mut("gate_cv_in") = [1.0; BATCH];
    sweep_params_min_max(&mut h, &DESCRIPTOR);
}
