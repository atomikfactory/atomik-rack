//! Gate tests, all through `DeviceHarness`.
#![allow(clippy::float_cmp)]

use super::*;
use crate::test_support::{SR, assert_keys, harness, sweep_params_min_max};
use atomik_testkit::harness::DeviceHarness;

fn gate() -> DeviceHarness {
    harness(Box::new(Gate::new()))
}

fn ms(t: f64) -> usize {
    (t * 0.001 * SR) as usize
}

/// Feeds a constant `level` on both main inputs (and `key` on both sidechain inputs when given)
/// for `samples`, returns `(out_l, gr_cv)` per sample.
fn feed(
    h: &mut DeviceHarness,
    level: f32,
    key: Option<f32>,
    samples: usize,
) -> (Vec<f32>, Vec<f32>) {
    let (mut out, mut cv) = (Vec::new(), Vec::new());
    for _ in 0..samples.div_ceil(BATCH) {
        *h.input_mut("in_l") = [level; BATCH];
        *h.input_mut("in_r") = [level; BATCH];
        if let Some(k) = key {
            *h.input_mut("sc_in_l") = [k; BATCH];
            *h.input_mut("sc_in_r") = [k; BATCH];
        }
        h.process();
        out.extend_from_slice(h.output("out_l"));
        cv.extend_from_slice(h.output("gr_cv_out"));
    }
    (out, cv)
}

fn gain_db(cv: f32) -> f32 {
    20.0 * (1.0 - cv).max(1e-9).log10()
}

#[test]
fn descriptor_validates() {
    assert_keys(
        &DESCRIPTOR,
        &[
            "threshold",
            "range",
            "hold",
            "attack",
            "release",
            "expander",
            "sc_enable",
            "sc_hp",
            "sc_lp",
            "bypass",
        ],
        &[
            "in_l",
            "in_r",
            "sc_in_l",
            "sc_in_r",
            "out_l",
            "out_r",
            "gr_cv_out",
        ],
    );
    assert_eq!(DESCRIPTOR.type_id, "atomik.gate");
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Effect);
}

#[test]
fn an_open_gate_passes_the_signal_bit_for_bit() {
    let mut h = gate();
    let (out, cv) = feed(&mut h, 0.5, None, ms(500.0));
    assert!(out.iter().all(|x| x.to_bits() == 0.5f32.to_bits()));
    assert!(cv.iter().all(|x| *x == 0.0));
}

#[test]
fn a_closed_gate_attenuates_by_the_range() {
    let mut h = gate();
    h.set_param("range", 40.0);
    h.set_param("release", 20.0);
    let (out, _) = feed(&mut h, 0.001, None, ms(2_000.0)); // -60 dB, under the -40 dB threshold
    let want = 0.001 * 0.01;
    let got = out[out.len() - 1];
    assert!((got - want).abs() < 1e-7, "{got} vs {want}");
}

#[test]
fn it_holds_then_closes_with_the_release_time_constant() {
    let mut h = gate();
    h.set_param("hold", 50.0);
    h.set_param("release", 150.0);
    h.set_param("range", 60.0);
    feed(&mut h, 0.5, None, ms(300.0));
    let (_, cv) = feed(&mut h, 0.0, None, ms(800.0));
    // The 5 ms detector needs ln(0.5 / 0.01) = 3.9 time constants (about 19.6 ms) to fall
    // under the -40 dB threshold; then 50 ms of hold; then the release starts.
    let start = 19.6 + 50.0;
    assert!(
        gain_db(cv[ms(start - 10.0)]) > -0.1,
        "still open in the hold"
    );
    let at = start + 150.0;
    let want = -60.0 * (1.0 - (-1.0f32).exp());
    let got = gain_db(cv[ms(at)]);
    assert!((got - want).abs() < 1.5, "{got} dB vs {want}");
    assert!(gain_db(cv[ms(790.0)]) < -59.0);
}

#[test]
fn it_opens_with_the_attack_time_constant() {
    let mut h = gate();
    h.set_param("attack", 20.0);
    h.set_param("hold", 0.0);
    feed(&mut h, 0.0, None, ms(1_500.0)); // fully closed
    let (_, cv) = feed(&mut h, 0.5, None, ms(400.0));
    // One time constant after the key rises the gain has covered 63 % of the 60 dB.
    let got = gain_db(cv[ms(20.0)]);
    let want = -60.0 * (-1.0f32).exp();
    assert!((got - want).abs() < 2.0, "{got} dB vs {want}");
    assert!(gain_db(cv[ms(399.0)]) > -0.1);
}

#[test]
fn the_expander_follows_its_slope_above_the_range() {
    let mut h = gate();
    h.set_param("expander", 1.0);
    h.set_param("release", 5.0);
    h.set_param("hold", 0.0);
    // 10 dB under the threshold: 10 dB of attenuation at the 2:1 slope.
    let (_, cv) = feed(&mut h, 10f32.powf(-50.0 / 20.0), None, ms(1_000.0));
    let got = gain_db(cv[cv.len() - 1]);
    assert!((got + 10.0).abs() < 0.2, "{got}");
}

#[test]
fn the_sidechain_key_opens_the_gate_for_a_quiet_input() {
    let mut h = gate();
    h.set_param("sc_enable", 1.0);
    h.set_param("release", 20.0);
    // Quiet main, loud key: open.
    let (out, _) = feed(&mut h, 0.001, Some(0.5), ms(300.0));
    assert_eq!(out[out.len() - 1].to_bits(), 0.001f32.to_bits());
    // Quiet key: closes.
    let (out, _) = feed(&mut h, 0.001, Some(0.0), ms(2_000.0));
    assert!(out[out.len() - 1] < 0.000_01);
    // With the sidechain switch off the main input is the key again.
    h.set_param("sc_enable", 0.0);
    let (out, _) = feed(&mut h, 0.5, Some(0.0), ms(300.0));
    assert_eq!(out[out.len() - 1].to_bits(), 0.5f32.to_bits());
}

#[test]
fn the_key_high_pass_ignores_low_frequencies() {
    // A loud 40 Hz key does not open a gate whose key is high-passed at 1 kHz.
    let tone = |n: usize| 0.5 * (std::f64::consts::TAU * 40.0 * n as f64 / SR).sin() as f32;
    let run = |hp: f32| {
        let mut h = gate();
        h.set_param("sc_enable", 1.0);
        h.set_param("sc_hp", hp);
        h.set_param("release", 20.0);
        let mut cv_last = 0.0;
        for b in 0..ms(1_500.0) / BATCH {
            let mut k = [0.0; BATCH];
            for (i, s) in k.iter_mut().enumerate() {
                *s = tone(b * BATCH + i);
            }
            *h.input_mut("in_l") = [0.2; BATCH];
            *h.input_mut("in_r") = [0.2; BATCH];
            *h.input_mut("sc_in_l") = k;
            *h.input_mut("sc_in_r") = k;
            h.process();
            cv_last = h.output("gr_cv_out")[BATCH - 1];
        }
        gain_db(cv_last)
    };
    assert!(run(20.0) > -1.0, "open without the filter");
    assert!(run(1_000.0) < -50.0, "closed with it");
}

#[test]
fn two_runs_of_the_same_signal_give_the_same_bits() {
    // Two devices fed the same samples give the same bits (the maths is deterministic).
    let sig = |n: usize| 0.3 * (std::f64::consts::TAU * 330.0 * n as f64 / SR).sin() as f32;
    let render = || {
        let mut h = gate();
        h.set_param("threshold", -20.0);
        let mut v = Vec::new();
        for b in 0..400 {
            let mut x = [0.0; BATCH];
            for (i, s) in x.iter_mut().enumerate() {
                let n = b * BATCH + i;
                *s = sig(n)
                    * if (n / 4_000).is_multiple_of(2) {
                        1.0
                    } else {
                        0.03
                    };
            }
            *h.input_mut("in_l") = x;
            *h.input_mut("in_r") = x;
            h.process();
            v.extend_from_slice(h.output("out_l"));
        }
        v
    };
    let (a, b) = (render(), render());
    assert!(a.iter().zip(&b).all(|(x, y)| x.to_bits() == y.to_bits()));
    assert!(a.iter().any(|x| x.abs() > 0.1));
}

#[test]
fn the_registered_device_bypasses_to_the_input() {
    let mut h = DeviceHarness::new((factory().create)(), SR);
    h.set_param("bypass", 1.0);
    let (out, _) = feed(&mut h, 0.001, None, ms(1_000.0));
    assert_eq!(out[out.len() - 1].to_bits(), 0.001f32.to_bits());
}

#[test]
fn params_at_their_limits_keep_the_output_finite() {
    let mut h = gate();
    *h.input_mut("in_l") = [0.9; BATCH];
    *h.input_mut("in_r") = [-0.3; BATCH];
    sweep_params_min_max(&mut h, &DESCRIPTOR);
}
