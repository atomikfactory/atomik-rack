//! Pitch shifter tests, all through `DeviceHarness` (every `process` is allocation-guarded).
#![allow(clippy::float_cmp)]

use super::*;
use crate::test_support::{SR, assert_keys, harness, run_signal, sine, sweep_params_min_max};
use atomik_testkit::harness::DeviceHarness;

fn shifter() -> DeviceHarness {
    harness(Box::new(PitchShifter::new()))
}

fn ms(t: f64) -> usize {
    (t * 0.001 * SR).round() as usize
}

fn run(h: &mut DeviceHarness, signal: impl Fn(usize) -> f32, samples: usize) -> Vec<f32> {
    run_signal(
        h,
        &["in_l", "in_r"],
        signal,
        samples.div_ceil(BATCH),
        &["out_l"],
    )
    .remove(0)
}

/// The frequency of `y` in Hz from its rising zero crossings (linear interpolation between the
/// samples), over the whole slice.
fn frequency(y: &[f32]) -> f64 {
    let mut first = None;
    let mut last = 0.0;
    let mut count = 0usize;
    for (i, w) in y.windows(2).enumerate() {
        if w[0] < 0.0 && w[1] >= 0.0 {
            let t = i as f64 + f64::from(-w[0]) / f64::from(w[1] - w[0]);
            if first.is_none() {
                first = Some(t);
            } else {
                count += 1;
            }
            last = t;
        }
    }
    match first {
        Some(f) if count > 0 => count as f64 * SR / (last - f),
        _ => 0.0,
    }
}

fn cents(a: f64, b: f64) -> f64 {
    1200.0 * (a / b).log2()
}

fn peak(y: &[f32]) -> f32 {
    y.iter().fold(0.0f32, |m, v| m.max(v.abs()))
}

#[test]
fn descriptor_validates() {
    assert_keys(
        &DESCRIPTOR,
        &[
            "semitones",
            "cents",
            "quality",
            "mix",
            "pitch_cv_trim",
            "bypass",
        ],
        &["in_l", "in_r", "pitch_cv_in", "out_l", "out_r"],
    );
    assert_eq!(DESCRIPTOR.type_id, "atomik.pitch-shifter");
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Effect);
}

#[test]
fn the_latency_is_reported_exactly_for_each_quality_and_rate() {
    // 3, 6 and 10 ms cross-fades at 48 kHz: X = 144, 288, 480 and D = 5 X / 2 + 4.
    for (q, want) in [(0.0, 364), (1.0, 724), (2.0, 1_204)] {
        let mut h = shifter();
        h.set_param("quality", q);
        assert_eq!(h.device().latency_frames(), want, "quality {q}");
    }
    assert!(PitchShifter::new().variable_latency());
    assert_eq!(latency_samples(1, 44_100.0), 5 * 132 + 4);
    assert_eq!(latency_samples(1, 96_000.0), 5 * 288 + 4);
}

#[test]
fn at_zero_shift_the_output_is_the_input_delayed_by_the_latency_bit_for_bit() {
    for q in [0.0, 1.0, 2.0] {
        let mut h = shifter();
        h.set_param("quality", q);
        let d = h.device().latency_frames() as usize;
        let tone = sine(330.0, 0.6);
        let y = run(&mut h, &tone, d + 4_000);
        for (n, v) in y.iter().enumerate() {
            let want = if n < d { 0.0 } else { tone(n - d) };
            assert_eq!(v.to_bits(), want.to_bits(), "quality {q} sample {n}");
        }
    }
}

#[test]
fn an_octave_up_and_down_and_in_between_land_on_the_pitch() {
    for (st, base) in [(12.0, 220.0), (-12.0, 880.0), (7.0, 330.0), (-5.0, 440.0)] {
        let mut h = shifter();
        h.set_param("semitones", st);
        let y = run(&mut h, sine(base, 0.5), ms(1_500.0));
        let want = base * 2f64.powf(f64::from(st) / 12.0);
        let got = frequency(&y[ms(500.0)..]);
        assert!(
            cents(got, want).abs() < 15.0,
            "{st} st on {base} Hz: {got} Hz, wanted {want}"
        );
    }
}

#[test]
fn cents_and_semitones_add() {
    let mut h = shifter();
    h.set_param("semitones", 2.0);
    h.set_param("cents", -50.0);
    let y = run(&mut h, sine(300.0, 0.5), ms(1_500.0));
    let want = 300.0 * 2f64.powf(1.5 / 12.0);
    let got = frequency(&y[ms(500.0)..]);
    assert!(cents(got, want).abs() < 10.0, "{got} vs {want}");
}

#[test]
fn a_shifted_tone_keeps_its_level_there_are_no_dips_at_the_jumps() {
    for st in [12.0, -12.0, 3.0] {
        let mut h = shifter();
        h.set_param("semitones", st);
        let y = run(&mut h, sine(220.0, 0.5), ms(2_000.0));
        let y = &y[ms(500.0)..];
        let window = ms(4.0);
        let lowest = y
            .chunks(window)
            .filter(|c| c.len() == window)
            .map(peak)
            .fold(f32::MAX, f32::min);
        assert!(lowest > 0.38, "{st} st: the envelope dips to {lowest}");
        assert!(peak(y) < 0.56, "{st} st: peaks at {}", peak(y));
    }
}

#[test]
fn both_channels_jump_together_and_a_silent_side_stays_silent() {
    let mut h = shifter();
    h.set_param("semitones", 7.0);
    let tone = sine(250.0, 0.5);
    let n = ms(1_000.0);
    let (mut l, mut r) = (Vec::new(), Vec::new());
    for b in 0..n.div_ceil(BATCH) {
        let mut x = [0.0f32; BATCH];
        for (i, s) in x.iter_mut().enumerate() {
            *s = tone(b * BATCH + i);
        }
        *h.input_mut("in_l") = x;
        *h.input_mut("in_r") = [0.0; BATCH];
        h.process();
        l.extend_from_slice(h.output("out_l"));
        r.extend_from_slice(h.output("out_r"));
    }
    assert!(peak(&l) > 0.4);
    assert!(r.iter().all(|v| *v == 0.0));
    // A mono source comes out as two identical channels.
    let mut h = shifter();
    h.set_param("semitones", -4.0);
    for b in 0..80 {
        let mut x = [0.0f32; BATCH];
        for (i, s) in x.iter_mut().enumerate() {
            *s = tone(b * BATCH + i);
        }
        *h.input_mut("in_l") = x;
        *h.input_mut("in_r") = x;
        h.process();
        assert_eq!(h.output("out_l"), h.output("out_r"));
    }
}

#[test]
fn mix_blends_the_dry_input_delayed_by_the_latency() {
    let mut h = shifter();
    h.set_param("semitones", 5.0);
    h.set_param("mix", 0.0);
    let d = h.device().latency_frames() as usize;
    let tone = sine(440.0, 0.5);
    let y = run(&mut h, &tone, d + 2_000);
    for (n, v) in y.iter().enumerate().skip(d) {
        assert_eq!(v.to_bits(), tone(n - d).to_bits(), "sample {n}");
    }
}

#[test]
fn the_shift_going_back_to_zero_brings_the_delay_back_to_the_latency() {
    let mut h = shifter();
    let d = h.device().latency_frames() as usize;
    let tone = sine(300.0, 0.5);
    h.set_param("semitones", 9.0);
    let first = run(&mut h, &tone, ms(700.0));
    assert!(peak(&first) > 0.3);
    h.set_param("semitones", 0.0);
    let from = ms(700.0).div_ceil(BATCH) * BATCH;
    let y = run(&mut h, |n| tone(from + n), ms(300.0));
    // After the cross-fade in progress and the one that brings the head home (at most `2 X`
    // samples) the output is exactly the delayed input.
    let settled = 2 * fade_samples(1, SR as f32) + BATCH;
    for (n, v) in y.iter().enumerate().skip(settled) {
        assert_eq!(v.to_bits(), tone(from + n - d).to_bits(), "sample {n}");
    }
}

#[test]
fn the_pitch_cv_adds_semitones_through_its_trim() {
    let run_cv = |cv: f32, trim: f32| {
        let mut h = shifter();
        h.set_param("pitch_cv_trim", trim);
        let tone = sine(300.0, 0.5);
        let mut y = Vec::new();
        for b in 0..ms(1_500.0).div_ceil(BATCH) {
            let mut x = [0.0f32; BATCH];
            for (i, s) in x.iter_mut().enumerate() {
                *s = tone(b * BATCH + i);
            }
            *h.input_mut("in_l") = x;
            *h.input_mut("in_r") = x;
            *h.input_mut("pitch_cv_in") = [cv; BATCH];
            h.process();
            y.extend_from_slice(h.output("out_l"));
        }
        frequency(&y[ms(500.0)..])
    };
    // CV 0.5 at full trim is six semitones up, a negative trim goes down, zero trim does nothing.
    assert!(cents(run_cv(0.5, 1.0), 300.0 * 2f64.powf(0.5)).abs() < 15.0);
    assert!(cents(run_cv(0.5, -1.0), 300.0 * 2f64.powf(-0.5)).abs() < 15.0);
    assert!(cents(run_cv(1.0, 0.0), 300.0).abs() < 1.0);
}

#[test]
fn changing_the_quality_changes_the_latency_and_keeps_the_pitch() {
    let mut h = shifter();
    h.set_param("semitones", 5.0);
    h.set_param("quality", 0.0);
    assert_eq!(h.device().latency_frames(), 364);
    let y = run(&mut h, sine(400.0, 0.5), ms(1_500.0));
    let want = 400.0 * 2f64.powf(5.0 / 12.0);
    assert!(cents(frequency(&y[ms(500.0)..]), want).abs() < 20.0);
    h.set_param("quality", 2.0);
    assert_eq!(h.device().latency_frames(), 1_204);
}

#[test]
fn a_pulse_train_comes_out_with_pulses_not_noise() {
    // Shifting a train of pulses an octave up keeps pulses (twice as many per second).
    let mut h = shifter();
    h.set_param("semitones", 12.0);
    let period = ms(10.0);
    let y = run(
        &mut h,
        |n| if n % period == 0 { 0.8 } else { 0.0 },
        ms(1_500.0),
    );
    let y = &y[ms(500.0)..];
    let strong = y
        .windows(3)
        .filter(|w| w[1] > 0.3 && w[1] >= w[0] && w[1] >= w[2])
        .count();
    // 1 s of 100 pulses per second becomes about 200; allow the cross-fades their share.
    assert!((120..=260).contains(&strong), "{strong}");
}

#[test]
fn non_finite_input_counts_as_silence() {
    let mut h = shifter();
    h.set_param("semitones", 3.0);
    let y = run(
        &mut h,
        |n| if n % 3 == 0 { f32::NAN } else { f32::INFINITY },
        8_000,
    );
    assert!(y.iter().all(|v| *v == 0.0));
}

#[test]
fn every_parameter_extreme_stays_finite() {
    let mut h = shifter();
    sweep_params_min_max(&mut h, &DESCRIPTOR);
}
