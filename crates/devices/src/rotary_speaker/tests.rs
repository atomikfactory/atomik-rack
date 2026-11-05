//! Rotary speaker tests, all through `DeviceHarness` (every `process` is allocation-guarded).
#![allow(clippy::float_cmp)]

use super::*;
use crate::test_support::{
    SR, assert_keys, harness, run_signal, sine, sweep_params_min_max, tone_amplitude,
};
use atomik_testkit::harness::DeviceHarness;

fn speaker() -> DeviceHarness {
    harness(Box::new(RotarySpeaker::new()))
}

fn secs(t: f64) -> usize {
    (t * SR).round() as usize
}

fn batches(samples: usize) -> usize {
    samples.div_ceil(BATCH)
}

/// Runs `samples` of `signal` through both inputs; returns `(out_l, out_r, horn_cv)`.
fn run(
    h: &mut DeviceHarness,
    signal: impl Fn(usize) -> f32,
    samples: usize,
) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
    let mut y = run_signal(
        h,
        &["in_l", "in_r"],
        signal,
        batches(samples),
        &["out_l", "out_r", "horn_cv_out"],
    );
    let cv = y.remove(2);
    let r = y.remove(1);
    (y.remove(0), r, cv)
}

/// The horn's rotations per second over `y` (rising zero crossings of the horn CV).
fn rate(cv: &[f32]) -> f64 {
    let turns = cv.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
    turns as f64 / (cv.len() as f64 / SR)
}

/// The peak of the signal in each window of `len` samples.
fn envelope(y: &[f32], len: usize) -> Vec<f32> {
    y.chunks(len)
        .map(|c| c.iter().fold(0.0f32, |m, v| m.max(v.abs())))
        .collect()
}

/// A device set up for a clean read of one effect: no drive, no mix change, one microphone
/// point.
fn plain() -> DeviceHarness {
    let mut h = speaker();
    h.set_param("drive", 0.0);
    h.set_param("doppler", 0.0);
    h.set_param("tremolo", 0.0);
    h.set_param("spread", 0.0);
    h
}

#[test]
fn descriptor_validates() {
    assert_keys(
        &DESCRIPTOR,
        &[
            "speed",
            "horn_slow",
            "horn_fast",
            "drum_slow",
            "drum_fast",
            "horn_ramp",
            "drum_ramp",
            "crossover",
            "drive",
            "doppler",
            "tremolo",
            "spread",
            "mix",
            "bypass",
        ],
        &[
            "in_l",
            "in_r",
            "speed_cv_in",
            "out_l",
            "out_r",
            "horn_cv_out",
        ],
    );
    assert_eq!(DESCRIPTOR.type_id, "atomik.rotary-speaker");
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Effect);
}

#[test]
fn with_no_movement_and_no_drive_the_two_bands_add_up_flat() {
    for hz in [100.0, 400.0, 800.0, 1_500.0, 6_000.0] {
        let mut h = plain();
        h.set_param("speed", 0.0);
        let (y, _, _) = run(&mut h, sine(hz, 0.3), secs(1.5));
        let start = secs(0.5);
        let level = tone_amplitude(&y[start..start + secs(1.0)], hz, start);
        assert!(
            (20.0 * (level / 0.3).log10()).abs() < 0.2,
            "{hz} Hz: {level}"
        );
    }
}

#[test]
fn the_rotors_turn_at_the_slow_and_the_fast_speed() {
    let turns = |speed: f32, horn: &str, hz: f32| {
        let mut h = plain();
        h.set_param("speed", speed);
        h.set_param(horn, hz);
        rate(&run(&mut h, |_| 0.0, secs(5.0)).2)
    };
    assert!((turns(1.0, "horn_slow", 1.0) - 1.0).abs() < 0.25);
    assert!((turns(2.0, "horn_fast", 7.0) - 7.0).abs() < 0.3);
    assert!(turns(0.0, "horn_slow", 1.0) < 0.01, "Stop does not turn");
}

#[test]
fn the_first_batch_starts_at_the_target_speed() {
    let mut h = plain();
    h.set_param("speed", 2.0);
    h.set_param("horn_fast", 8.0);
    let (_, _, cv) = run(&mut h, |_| 0.0, BATCH);
    let want = (std::f64::consts::TAU * 8.0 * 63.0 / SR).sin() as f32;
    assert!((cv[63] - want).abs() < 0.003, "{} vs {want}", cv[63]);
}

#[test]
fn a_change_of_speed_runs_up_over_the_ramp_time() {
    let mut h = plain();
    h.set_param("horn_slow", 1.0);
    h.set_param("horn_fast", 8.0);
    h.set_param("horn_ramp", 2_000.0);
    let (_, _, before) = run(&mut h, |_| 0.0, secs(2.0));
    h.set_param("speed", 2.0);
    let (_, _, first) = run(&mut h, |_| 0.0, secs(1.0));
    let (_, _, _middle) = run(&mut h, |_| 0.0, secs(8.0));
    let (_, _, last) = run(&mut h, |_| 0.0, secs(3.0));
    let (before, first, last) = (rate(&before), rate(&first), rate(&last));
    assert!((before - 1.0).abs() < 0.6, "{before}");
    assert!(first > before && first < 5.0, "{first}");
    assert!((last - 8.0).abs() < 0.5, "{last}");
}

#[test]
fn a_shorter_ramp_gets_there_sooner() {
    let reached = |ramp: f32| {
        let mut h = plain();
        h.set_param("horn_fast", 8.0);
        h.set_param("horn_ramp", ramp);
        let _ = run(&mut h, |_| 0.0, secs(0.5));
        h.set_param("speed", 2.0);
        rate(&run(&mut h, |_| 0.0, secs(1.0)).2)
    };
    assert!(reached(50.0) > reached(2_000.0) + 1.0);
}

#[test]
fn the_speed_cv_switches_to_fast_with_hysteresis() {
    let mut h = plain();
    h.set_param("horn_ramp", 50.0);
    h.set_param("horn_fast", 8.0);
    h.set_param("horn_slow", 1.0);
    let turn = |h: &mut DeviceHarness, level: f32, s: f64| {
        let n = batches(secs(s));
        let mut cv = Vec::new();
        for _ in 0..n {
            *h.input_mut("speed_cv_in") = [level; BATCH];
            h.process();
            cv.extend_from_slice(h.output("horn_cv_out"));
        }
        rate(&cv)
    };
    let _ = turn(&mut h, 0.0, 1.0);
    assert!(turn(&mut h, 0.0, 3.0) < 2.0, "low CV leaves it slow");
    let _ = turn(&mut h, 1.0, 1.0);
    assert!(turn(&mut h, 1.0, 3.0) > 6.5, "high CV makes it fast");
    // Between the two thresholds the switch keeps its state.
    assert!(turn(&mut h, 0.4, 3.0) > 6.5, "0.4 keeps fast");
    let _ = turn(&mut h, 0.0, 1.0);
    assert!(turn(&mut h, 0.4, 3.0) < 2.0, "0.4 keeps slow");
}

#[test]
fn doppler_moves_the_pitch_and_zero_doppler_does_not() {
    let sideband = |doppler: f32| {
        let mut h = plain();
        h.set_param("speed", 2.0);
        h.set_param("horn_fast", 7.0);
        h.set_param("doppler", doppler);
        let (y, _, _) = run(&mut h, sine(3_000.0, 0.3), secs(2.0));
        let start = secs(1.0);
        let window = &y[start..start + secs(1.0)];
        let up = tone_amplitude(window, 3_007.0, start);
        let down = tone_amplitude(window, 2_993.0, start);
        up.min(down)
    };
    assert!(sideband(0.0) < 0.002, "{}", sideband(0.0));
    assert!(sideband(1.0) > 0.05, "{}", sideband(1.0));
    assert!(sideband(0.1) > 0.03, "{}", sideband(0.1));
}

#[test]
fn tremolo_moves_the_level_with_the_rotor() {
    let swing = |tremolo: f32| {
        let mut h = plain();
        h.set_param("speed", 2.0);
        h.set_param("horn_fast", 7.0);
        h.set_param("tremolo", tremolo);
        let (y, _, _) = run(&mut h, sine(3_000.0, 0.3), secs(2.0));
        let env = envelope(&y[secs(1.0)..], 48);
        let max = env.iter().fold(0.0f32, |m, v| m.max(*v));
        let min = env.iter().fold(f32::MAX, |m, v| m.min(*v));
        (max, min)
    };
    let (max, min) = swing(0.0);
    assert!(max - min < 0.02, "{max} {min}");
    let (max, min) = swing(1.0);
    assert!(max > 0.27 && min < 0.05, "{max} {min}");
    let (_, half) = swing(0.5);
    assert!(half > 0.1 && half < 0.2, "{half}");
}

#[test]
fn the_microphone_spread_makes_the_channels_differ() {
    let outputs = |spread: f32| {
        let mut h = plain();
        h.set_param("speed", 2.0);
        h.set_param("tremolo", 1.0);
        h.set_param("doppler", 0.5);
        h.set_param("spread", spread);
        let (l, r, _) = run(&mut h, sine(3_000.0, 0.3), secs(1.0));
        (l, r)
    };
    let (l, r) = outputs(0.0);
    assert!(l.iter().zip(&r).all(|(a, b)| a.to_bits() == b.to_bits()));
    let (l, r) = outputs(1.0);
    let diff = l
        .iter()
        .zip(&r)
        .fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
    assert!(diff > 0.1, "{diff}");
    // At full spread the two microphones are opposite: while one is loud the other is quiet.
    let (el, er) = (
        envelope(&l[secs(0.5)..], 480),
        envelope(&r[secs(0.5)..], 480),
    );
    let mean = |e: &[f32]| e.iter().sum::<f32>() / e.len() as f32;
    let (ml, mr) = (mean(&el), mean(&er));
    let cov: f32 = el.iter().zip(&er).map(|(a, b)| (a - ml) * (b - mr)).sum();
    assert!(cov < 0.0, "{cov}");
}

#[test]
fn drive_zero_is_exact_and_drive_saturates() {
    let mut s = RotarySpeaker::new();
    s.set_param(ParamId(P_DRIVE), 0.0);
    for x in [0.0, 0.3, -0.9, 5.0] {
        assert_eq!(s.saturate(x).to_bits(), x.to_bits());
    }
    s.set_param(ParamId(P_DRIVE), 1.0);
    let top = s.saturate(100.0);
    assert!(top > 0.0 && top < 1.0, "{top}");
    assert_eq!(s.saturate(-100.0), -top);
    assert!(s.saturate(0.1) > 0.1, "a small signal gains");
    assert!(s.saturate(f32::MAX).is_finite());
}

#[test]
fn drive_adds_harmonics() {
    let third = |drive: f32| {
        let mut h = plain();
        h.set_param("speed", 0.0);
        h.set_param("drive", drive);
        let (y, _, _) = run(&mut h, sine(100.0, 0.4), secs(1.5));
        let start = secs(0.5);
        let window = &y[start..start + secs(1.0)];
        tone_amplitude(window, 300.0, start) / tone_amplitude(window, 100.0, start)
    };
    assert!(third(0.0) < 0.01);
    assert!(third(1.0) > 0.03);
}

#[test]
fn mix_zero_is_the_dry_input_bit_for_bit() {
    let mut h = speaker();
    h.set_param("mix", 0.0);
    let tone = sine(440.0, 0.4);
    let (l, r, _) = run(&mut h, &tone, secs(0.2));
    for n in 0..l.len() {
        assert_eq!(l[n].to_bits(), tone(n).to_bits());
        assert_eq!(r[n].to_bits(), tone(n).to_bits());
    }
}

#[test]
fn the_crossover_moves_which_rotor_gets_the_tone() {
    // Two seconds with the drum parked (slow, long ramp) and the horn fast: a 300 Hz tone is
    // in the drum band under a high crossover and in the horn band under a low one.
    let swing = |crossover: f32| {
        let mut h = plain();
        h.set_param("tremolo", 1.0);
        h.set_param("speed", 2.0);
        h.set_param("horn_fast", 7.0);
        h.set_param("drum_fast", 3.0);
        h.set_param("crossover", crossover);
        let (y, _, _) = run(&mut h, sine(300.0, 0.3), secs(3.0));
        let env = envelope(&y[secs(1.0)..], 480);
        let max = env.iter().fold(0.0f32, |m, v| m.max(*v));
        let min = env.iter().fold(f32::MAX, |m, v| m.min(*v));
        max - min
    };
    // The drum swings the level at 3 Hz by 60 % at most; the horn by all of it.
    assert!(swing(200.0) > swing(1_600.0) + 0.05);
}

#[test]
fn the_output_is_the_same_bits_on_every_run() {
    let go = || {
        let mut h = speaker();
        h.set_param("speed", 2.0);
        let noise = |n: usize| ((n * 2_654_435_761) % 1_000) as f32 / 500.0 - 1.0;
        run(&mut h, noise, secs(0.3))
    };
    let (a, b) = (go(), go());
    assert_eq!(
        a.0.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        b.0.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
    );
    assert_eq!(a.1.len(), b.1.len());
}

#[test]
fn non_finite_input_counts_as_silence() {
    let mut h = speaker();
    let (l, r, _) = run(
        &mut h,
        |n| if n % 7 == 0 { f32::NAN } else { f32::INFINITY },
        2_000,
    );
    assert!(l.iter().chain(&r).all(|v| *v == 0.0));
}

#[test]
fn every_parameter_extreme_stays_finite() {
    let mut h = speaker();
    sweep_params_min_max(&mut h, &DESCRIPTOR);
}

#[test]
fn loud_noise_at_extreme_settings_stays_bounded() {
    let mut h = speaker();
    for (key, v) in [
        ("speed", 2.0),
        ("drive", 1.0),
        ("doppler", 1.0),
        ("tremolo", 1.0),
        ("horn_ramp", 50.0),
    ] {
        h.set_param(key, v);
    }
    let noise = |n: usize| (((n * 2_654_435_761) % 2_000) as f32 / 1_000.0 - 1.0) * 4.0;
    let (l, r, _) = run(&mut h, noise, secs(1.0));
    assert!(l.iter().chain(&r).all(|v| v.is_finite() && v.abs() < 4.0));
}
