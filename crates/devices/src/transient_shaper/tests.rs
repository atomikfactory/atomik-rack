//! Transient shaper tests, all through `DeviceHarness` (every `process` is allocation-guarded).
#![allow(clippy::float_cmp)]

use super::*;
use crate::test_support::{SR, assert_keys, harness, run_signal, sine, sweep_params_min_max};
use atomik_testkit::harness::DeviceHarness;

fn shaper() -> DeviceHarness {
    harness(Box::new(TransientShaper::new()))
}

fn ms(t: f64) -> usize {
    (t * 0.001 * SR).round() as usize
}

fn batches(samples: usize) -> usize {
    samples.div_ceil(BATCH)
}

/// A 1 kHz tone of amplitude `amp` that starts at sample 0 and stops after `on` samples
/// (`usize::MAX`: never).
fn burst(amp: f64, on: usize) -> impl Fn(usize) -> f32 {
    let tone = sine(1_000.0, amp);
    move |n| if n < on { tone(n) } else { 0.0 }
}

/// A tone that starts at 0 with an exponential decay of time constant `tau_ms`.
fn decaying(amp: f64, tau_ms: f64) -> impl Fn(usize) -> f32 {
    let tone = sine(1_000.0, amp);
    move |n| tone(n) * (-(n as f64) / (tau_ms * 0.001 * SR)).exp() as f32
}

fn peak(y: &[f32]) -> f32 {
    y.iter().fold(0.0f32, |m, v| m.max(v.abs()))
}

fn db(x: f32) -> f32 {
    20.0 * x.max(1e-9).log10()
}

fn out(h: &mut DeviceHarness, signal: impl Fn(usize) -> f32, samples: usize) -> Vec<f32> {
    run_signal(h, &["in_l", "in_r"], signal, batches(samples), &["out_l"]).remove(0)
}

#[test]
fn descriptor_validates() {
    assert_keys(
        &DESCRIPTOR,
        &[
            "attack",
            "sustain",
            "fast",
            "slow",
            "threshold",
            "output",
            "mix",
            "attack_cv_trim",
            "sustain_cv_trim",
            "bypass",
        ],
        &[
            "in_l",
            "in_r",
            "attack_cv_in",
            "sustain_cv_in",
            "out_l",
            "out_r",
            "transient_cv_out",
        ],
    );
    assert_eq!(DESCRIPTOR.type_id, "atomik.transient-shaper");
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Effect);
}

#[test]
fn with_both_amounts_at_zero_it_is_the_input_bit_for_bit() {
    let mut h = shaper();
    let x = out(&mut h, burst(0.5, ms(100.0)), ms(300.0));
    let tone = burst(0.5, ms(100.0));
    for (n, y) in x.iter().enumerate() {
        assert_eq!(y.to_bits(), tone(n).to_bits(), "sample {n}");
    }
}

#[test]
fn a_steady_tone_is_left_alone_whatever_the_amounts() {
    for (a, s) in [(1.0, 1.0), (-1.0, -1.0), (1.0, -1.0)] {
        let mut h = shaper();
        h.set_param("attack", a);
        h.set_param("sustain", s);
        let y = out(&mut h, sine(1_000.0, 0.3), ms(2_000.0));
        let tail = &y[y.len() - ms(100.0)..];
        assert!(
            (db(peak(tail)) - db(0.3)).abs() < 0.2,
            "attack {a} sustain {s}: {} dB",
            db(peak(tail))
        );
    }
}

#[test]
fn raising_the_attack_lifts_the_onset_and_lowering_it_dips_the_onset() {
    let onset = |attack: f32| {
        let mut h = shaper();
        h.set_param("attack", attack);
        let y = out(&mut h, burst(0.3, usize::MAX), ms(400.0));
        (peak(&y[..ms(30.0)]), peak(&y[y.len() - ms(20.0)..]))
    };
    let (flat, flat_end) = onset(0.0);
    let (up, up_end) = onset(1.0);
    let (down, down_end) = onset(-1.0);
    assert!(db(up) - db(flat) > 3.0, "{} dB", db(up) - db(flat));
    assert!(db(flat) - db(down) > 3.0, "{} dB", db(flat) - db(down));
    // The steady part after the onset is the same.
    assert!((db(up_end) - db(flat_end)).abs() < 0.3);
    assert!((db(down_end) - db(flat_end)).abs() < 0.3);
}

#[test]
fn the_sustain_amount_changes_the_tail_of_a_decaying_note() {
    let tail_energy = |sustain: f32| {
        let mut h = shaper();
        h.set_param("sustain", sustain);
        let y = out(&mut h, decaying(0.5, 120.0), ms(900.0));
        y[ms(200.0)..ms(600.0)]
            .iter()
            .map(|v| f64::from(*v).powi(2))
            .sum::<f64>()
    };
    let flat = tail_energy(0.0);
    let up = tail_energy(1.0);
    let down = tail_energy(-1.0);
    assert!(up > flat * 1.5, "{up} vs {flat}");
    assert!(down < flat * 0.7, "{down} vs {flat}");
}

#[test]
fn the_effect_does_not_depend_on_the_level() {
    let lift = |amp: f64| {
        let mut h = shaper();
        h.set_param("attack", 1.0);
        h.set_param("threshold", -90.0);
        let shaped = out(&mut h, burst(amp, usize::MAX), ms(60.0));
        let mut h = shaper();
        let plain = out(&mut h, burst(amp, usize::MAX), ms(60.0));
        db(peak(&shaped[..ms(30.0)])) - db(peak(&plain[..ms(30.0)]))
    };
    let (loud, quiet) = (lift(0.5), lift(0.02));
    assert!(loud > 3.0, "{loud}");
    assert!((loud - quiet).abs() < 1.0, "{loud} vs {quiet}");
}

#[test]
fn a_signal_under_the_threshold_is_not_shaped() {
    let mut h = shaper();
    h.set_param("attack", 1.0);
    h.set_param("threshold", -40.0);
    // 0.001 is -60 dB.
    let y = out(&mut h, burst(0.001, usize::MAX), ms(100.0));
    assert!((db(peak(&y)) - db(0.001)).abs() < 0.1, "{}", db(peak(&y)));
}

#[test]
fn the_cv_inputs_add_to_the_amounts_through_the_trims() {
    let lifted = |cv: f32, trim: f32| {
        let mut h = shaper();
        h.set_param("attack_cv_trim", trim);
        let n = batches(ms(60.0));
        let tone = burst(0.3, usize::MAX);
        let mut y = Vec::new();
        for b in 0..n {
            let mut x = [0.0f32; BATCH];
            for (i, s) in x.iter_mut().enumerate() {
                *s = tone(b * BATCH + i);
            }
            *h.input_mut("in_l") = x;
            *h.input_mut("in_r") = x;
            *h.input_mut("attack_cv_in") = [cv; BATCH];
            h.process();
            y.extend_from_slice(h.output("out_l"));
        }
        peak(&y[..ms(30.0)])
    };
    let none = lifted(0.0, 1.0);
    assert!(db(lifted(1.0, 1.0)) - db(none) > 3.0);
    assert!(db(none) - db(lifted(1.0, -1.0)) > 3.0);
    assert!((db(lifted(1.0, 0.0)) - db(none)).abs() < 1e-3);
}

#[test]
fn the_transient_output_follows_the_onsets_and_is_zero_for_a_steady_tone() {
    let mut h = shaper();
    let cv = run_signal(
        &mut h,
        &["in_l", "in_r"],
        burst(0.3, usize::MAX),
        batches(ms(1_500.0)),
        &["transient_cv_out"],
    )
    .remove(0);
    assert!(cv[..ms(10.0)].iter().any(|v| *v > 0.5), "an onset reads");
    assert!(cv[cv.len() - ms(50.0)..].iter().all(|v| *v < 0.01));
    assert!(cv.iter().all(|v| (0.0..=1.0).contains(v)));
}

#[test]
fn mix_blends_the_dry_input_and_zero_is_the_dry_input() {
    let mut h = shaper();
    h.set_param("attack", 1.0);
    h.set_param("mix", 0.0);
    let y = out(&mut h, burst(0.3, usize::MAX), ms(60.0));
    let tone = burst(0.3, usize::MAX);
    assert!(y.iter().enumerate().all(|(n, v)| *v == tone(n)));
}

#[test]
fn the_gain_never_goes_over_18_db() {
    let mut h = shaper();
    h.set_param("attack", 1.0);
    h.set_param("sustain", 1.0);
    h.set_param("output", 12.0);
    let y = out(&mut h, burst(0.05, usize::MAX), ms(200.0));
    assert!(peak(&y) <= 0.05 * 7.95 * 1.001, "{}", peak(&y));
}

#[test]
fn non_finite_input_counts_as_silence() {
    let mut h = shaper();
    h.set_param("attack", 1.0);
    let y = out(
        &mut h,
        |n| if n % 7 == 0 { f32::NAN } else { f32::INFINITY },
        1_000,
    );
    assert!(y.iter().all(|v| *v == 0.0));
}

#[test]
fn every_parameter_extreme_stays_finite() {
    let mut h = shaper();
    sweep_params_min_max(&mut h, &DESCRIPTOR);
}
