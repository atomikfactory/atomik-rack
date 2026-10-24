//! Phase and stereo meter device tests: the audio is untouched, the correlation, balance, width
//! and mid/side readouts follow the signal, the loudness matches the reference tone, holds and
//! the reset edge behave, and the scope tap sees the input.

use super::*;
use crate::test_support::{assert_keys, harness, sweep_params_min_max};
use atomik_dsp::Rng;
use atomik_testkit::harness::DeviceHarness;
use atomik_testkit::no_alloc::assert_no_alloc;

const SR: f64 = 48_000.0;

fn device() -> DeviceHarness {
    harness(Box::new(PhaseMeter::new()))
}

/// Runs `batches` batches of `signal(channel, absolute sample)`; returns (input, output).
fn run(
    h: &mut DeviceHarness,
    start: usize,
    batches: usize,
    mut signal: impl FnMut(usize, usize) -> f32,
) -> ([Vec<f32>; 2], [Vec<f32>; 2]) {
    let mut input = [Vec::new(), Vec::new()];
    let mut output = [Vec::new(), Vec::new()];
    for b in 0..batches {
        for (c, key) in ["in_l", "in_r"].into_iter().enumerate() {
            let batch: [f32; BATCH] = std::array::from_fn(|i| signal(c, start + b * BATCH + i));
            *h.input_mut(key) = batch;
            input[c].extend_from_slice(&batch);
        }
        h.process();
        output[0].extend_from_slice(h.output("out_l"));
        output[1].extend_from_slice(h.output("out_r"));
    }
    (input, output)
}

fn values(h: &DeviceHarness) -> [f32; PHASE_METERS] {
    let mut v = [f32::NAN; PHASE_METERS];
    assert_eq!(h.device().meter_values(&mut v), PHASE_METERS);
    v
}

fn bits(v: &[f32]) -> Vec<u32> {
    v.iter().map(|x| x.to_bits()).collect()
}

fn sine(hz: f64, amp: f64, phase: f64) -> impl Fn(usize) -> f32 {
    move |n| (amp * (std::f64::consts::TAU * hz * n as f64 / SR + phase).sin()) as f32
}

fn near(a: f32, b: f32, tol: f32) -> bool {
    (a - b).abs() <= tol
}

fn db(x: f32) -> f32 {
    20.0 * x.log10()
}

#[test]
fn descriptor_validates_and_follows_the_effect_conventions() {
    assert_keys(
        &DESCRIPTOR,
        &[
            "corr_time",
            "hold",
            "scope",
            "scope_gain",
            "loudness",
            "meter.reset",
            "bypass",
        ],
        &["in_l", "in_r", "out_l", "out_r"],
    );
    assert_eq!(DESCRIPTOR.type_id, "atomik.phase-meter");
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Effect);
    assert_eq!(HOLD_NAMES.len(), HOLD_SECONDS.len());
    assert_eq!(
        DESCRIPTOR.param_by_key("loudness").unwrap().default as i32,
        1
    );
    let mut r = atomik_device_sdk::Registry::new();
    r.register(factory()).unwrap();
    assert_eq!(r.create("atomik.phase-meter").unwrap().latency_frames(), 0);
}

#[test]
fn the_audio_passes_through_bit_for_bit_with_no_latency_whatever_the_settings() {
    let mut h = device();
    let mut noise = [Rng::new(1), Rng::new(2)];
    let (i, o) = run(&mut h, 0, 100, |c, _| noise[c].next_bipolar() * 0.9);
    assert_eq!(bits(&i[0]), bits(&o[0]));
    assert_eq!(bits(&i[1]), bits(&o[1]));
    for p in DESCRIPTOR.params.iter().filter(|p| p.key != "bypass") {
        for v in [p.min, p.max] {
            h.set_param(p.key, v);
            let mut noise = [Rng::new(3), Rng::new(4)];
            let (i, o) = run(&mut h, 0, 20, |c, _| noise[c].next_bipolar() * 2.0);
            assert_eq!(bits(&i[0]), bits(&o[0]), "{} = {v}", p.key);
            assert_eq!(bits(&i[1]), bits(&o[1]), "{} = {v}", p.key);
        }
    }
    // Non-finite samples pass, and do not poison the readouts.
    h.set_param("meter.reset", 0.0);
    h.set_param("meter.reset", 1.0);
    *h.input_mut("in_l") = [f32::NAN; BATCH];
    *h.input_mut("in_r") = [f32::INFINITY; BATCH];
    h.process();
    assert!(h.output("out_l").iter().all(|v| v.is_nan()));
    assert!(values(&h).iter().all(|v| v.is_finite()));
}

#[test]
fn it_is_real_time_safe() {
    let mut h = device();
    let mut noise = [Rng::new(5), Rng::new(6)];
    run(&mut h, 0, 8, |c, _| noise[c].next_bipolar());
    assert_no_alloc(|| {
        for p in DESCRIPTOR.params {
            h.set_param(p.key, p.max);
        }
        h.device_mut().reset();
    });
    sweep_params_min_max(&mut h, &DESCRIPTOR);
}

#[test]
fn identical_channels_read_plus_one_and_mono() {
    let mut h = device();
    let tone = sine(440.0, 0.5, 0.0);
    run(&mut h, 0, 1_500, |_, n| tone(n)); // 2 s
    let v = values(&h);
    assert!(
        near(v[value::CORRELATION], 1.0, 1e-4),
        "{}",
        v[value::CORRELATION]
    );
    assert!(near(v[value::BALANCE], 0.0, 1e-5));
    assert!(v[value::WIDTH] < 1e-3);
    assert!(near(v[value::MID_RMS], 0.5 / 2f32.sqrt(), 2e-3));
    assert!(v[value::SIDE_RMS] < 1e-4);
    assert!(near(v[value::RMS_L], v[value::RMS_R], 1e-6));
}

#[test]
fn inverted_channels_read_minus_one_and_all_side() {
    let mut h = device();
    let tone = sine(440.0, 0.5, 0.0);
    run(
        &mut h,
        0,
        1_500,
        |c, n| if c == 0 { tone(n) } else { -tone(n) },
    );
    let v = values(&h);
    assert!(
        near(v[value::CORRELATION], -1.0, 1e-4),
        "{}",
        v[value::CORRELATION]
    );
    assert!(near(v[value::WIDTH], 1.0, 1e-3));
    assert!(v[value::MID_RMS] < 1e-4);
}

#[test]
fn unrelated_noise_reads_about_zero_and_equal_mid_and_side() {
    let mut h = device();
    let mut noise = [Rng::new(11), Rng::new(12)];
    run(&mut h, 0, 2_000, |c, _| noise[c].next_bipolar() * 0.5);
    let v = values(&h);
    assert!(
        v[value::CORRELATION].abs() < 0.08,
        "{}",
        v[value::CORRELATION]
    );
    assert!(near(v[value::WIDTH], 0.5, 0.05), "{}", v[value::WIDTH]);
    assert!(near(v[value::MID_RMS], v[value::SIDE_RMS], 0.02));
}

#[test]
fn the_mid_side_balance_and_width_follow_the_matrix() {
    let mut h = device();
    // Left 0.8, right 0.2 (DC): M = 0.5, S = 0.3.
    run(&mut h, 0, 6_000, |c, _| if c == 0 { 0.8 } else { 0.2 });
    let v = values(&h);
    assert!(near(v[value::MID_RMS], 0.5, 1e-3), "{}", v[value::MID_RMS]);
    assert!(
        near(v[value::SIDE_RMS], 0.3, 1e-3),
        "{}",
        v[value::SIDE_RMS]
    );
    assert!(near(v[value::BALANCE], -0.6, 1e-3));
    assert!(near(v[value::WIDTH], 0.375, 1e-3));
    assert!(near(v[value::RMS_L], 0.8, 1e-3) && near(v[value::RMS_R], 0.2, 1e-3));
}

#[test]
fn the_correlation_time_sets_how_quickly_it_follows() {
    let tone = sine(440.0, 0.5, 0.0);
    let mut noise = [Rng::new(13), Rng::new(14)];
    let mut after = |ms: f32| {
        let mut h = device();
        h.set_param("corr_time", ms);
        // A second of unrelated noise, then 100 ms of mono.
        run(&mut h, 0, 750, |c, _| noise[c].next_bipolar() * 0.5);
        run(&mut h, 0, 75, |_, n| tone(n));
        values(&h)[value::CORRELATION]
    };
    let (fast, slow) = (after(50.0), after(2000.0));
    assert!(fast > 0.8, "{fast}");
    assert!(slow < 0.3, "{slow}");
}

#[test]
fn a_reference_tone_reads_its_level_in_lufs() {
    // EBU Tech 3341: a stereo 1 kHz sine at -23 dBFS reads -23 LUFS. The meter is the one of
    // atomik.mastering, so this also checks that this device wired it up correctly.
    let mut h = device();
    let amp = 10f64.powf(-23.0 / 20.0);
    let tone = sine(1000.0, amp, 0.0);
    run(&mut h, 0, 750, |_, n| tone(n)); // 1 s
    let v = values(&h);
    assert!(
        near(v[value::MOMENTARY], -23.0, 0.15),
        "{}",
        v[value::MOMENTARY]
    );
    run(&mut h, 750 * BATCH, 3_000, |_, n| tone(n)); // 4 s more
    let v = values(&h);
    assert!(
        near(v[value::MOMENTARY], -23.0, 0.15),
        "{}",
        v[value::MOMENTARY]
    );
    assert!(
        near(v[value::SHORT_TERM], -23.0, 0.15),
        "{}",
        v[value::SHORT_TERM]
    );
    assert!(
        near(v[value::INTEGRATED], -23.0, 0.15),
        "{}",
        v[value::INTEGRATED]
    );
    // One channel only is 3 dB quieter.
    let mut mono = device();
    run(
        &mut mono,
        0,
        3_000,
        |c, n| if c == 0 { tone(n) } else { 0.0 },
    );
    assert!(near(values(&mono)[value::SHORT_TERM], -26.0, 0.2));
    // The K-weighting rolls off the bass: 100 Hz at the same level reads lower.
    let mut bass = device();
    let low = sine(100.0, amp, 0.0);
    run(&mut bass, 0, 3_000, |_, n| low(n));
    assert!(
        values(&bass)[value::SHORT_TERM] < -23.0 - 0.3,
        "{}",
        values(&bass)[value::SHORT_TERM]
    );
}

#[test]
fn peaks_read_the_sample_peak_and_the_true_peak() {
    // A sine at a quarter of the sample rate, 45 degrees in: every sample is +-0.7071 of the
    // peak, so the sample peak is 3 dB down and the true peak is not.
    let mut h = device();
    let tone = sine(SR / 4.0, 1.0, std::f64::consts::FRAC_PI_4);
    run(&mut h, 0, 200, |_, n| tone(n));
    let v = values(&h);
    assert!(
        near(v[value::SAMPLE_PEAK_DB], -3.01, 0.05),
        "{}",
        v[value::SAMPLE_PEAK_DB]
    );
    assert!(
        near(v[value::TRUE_PEAK_DB], 0.0, 0.3),
        "{}",
        v[value::TRUE_PEAK_DB]
    );
    assert!(near(db(v[value::PEAK_L]), -3.01, 0.05));
    // Quiet again: the maxima stay, the recent peak and the hold have fallen.
    run(&mut h, 0, 2_000, |_, _| 0.0);
    let v = values(&h);
    assert!(near(v[value::SAMPLE_PEAK_DB], -3.01, 0.05));
    assert!(v[value::PEAK_L] < 1e-9 && v[value::HOLD_L] < 1e-9, "{v:?}");
}

#[test]
fn the_held_peak_stays_for_the_hold_time_then_falls() {
    // 1 s by default: a single peak, then silence.
    let blip = |c: usize, n: usize| if n == 100 && c == 0 { 0.5 } else { 0.0 };
    let mut h = device();
    run(&mut h, 0, 20, blip);
    // Gone from the recent peak after 16 batches, held on.
    run(&mut h, 20 * BATCH, 400, |_, _| 0.0); // ~0.5 s
    let v = values(&h);
    assert!(v[value::PEAK_L] < 1e-9);
    assert!(near(v[value::HOLD_L], 0.5, 1e-6), "{}", v[value::HOLD_L]);
    run(&mut h, 420 * BATCH, 400, |_, _| 0.0); // ~1.1 s in all
    assert!(values(&h)[value::HOLD_L] < 1e-9, "released after the hold");
    // Off: the hold follows the recent peak.
    let mut off = device();
    off.set_param("hold", 0.0);
    run(&mut off, 0, 10, blip);
    assert!(
        near(values(&off)[value::HOLD_L], 0.5, 1e-6),
        "while it is recent"
    );
    run(&mut off, 10 * BATCH, 20, |_, _| 0.0);
    assert!(values(&off)[value::HOLD_L] < 1e-9);
    // Infinite: held until reset, and the reset edge clears it.
    let mut inf = device();
    inf.set_param("hold", 4.0);
    run(&mut inf, 0, 20, blip);
    run(&mut inf, 20 * BATCH, 3_000, |_, _| 0.0);
    assert!(near(values(&inf)[value::HOLD_L], 0.5, 1e-6));
    inf.set_param("meter.reset", 1.0);
    run(&mut inf, 0, 1, |_, _| 0.0);
    let v = values(&inf);
    assert!(v[value::HOLD_L] < 1e-9, "reset clears the hold");
    assert!(near(v[value::SAMPLE_PEAK_DB], -120.0, 0.0) && near(v[value::INTEGRATED], -120.0, 0.0));
}

#[test]
fn the_reset_acts_on_the_rising_edge_only() {
    let tone = sine(1000.0, 0.1, 0.0);
    let mut h = device();
    run(&mut h, 0, 3_000, |_, n| tone(n));
    let before = values(&h)[value::INTEGRATED];
    assert!(before > -40.0);
    h.set_param("meter.reset", 1.0);
    run(&mut h, 3_000 * BATCH, 1, |_, n| tone(n));
    // Held high: the meter restarts but stays running (the integrated is still empty or tiny).
    let after = values(&h)[value::INTEGRATED];
    assert!(near(after, -120.0, 0.0), "{after}");
    run(&mut h, 0, 3_000, |_, n| tone(n));
    let level = values(&h)[value::INTEGRATED];
    assert!(
        near(level, before, 0.3),
        "it measures again: {level} vs {before}"
    );
    // Staying at 1 does not reset again.
    h.set_param("meter.reset", 1.0);
    run(&mut h, 0, 10, |_, n| tone(n));
    assert!(values(&h)[value::INTEGRATED] > -40.0);
}

#[test]
fn loudness_off_reads_the_floor_and_skips_the_work() {
    let mut h = device();
    h.set_param("loudness", 0.0);
    let tone = sine(1000.0, 0.5, 0.0);
    run(&mut h, 0, 3_000, |_, n| tone(n));
    let v = values(&h);
    for i in [
        value::TRUE_PEAK_DB,
        value::MOMENTARY,
        value::SHORT_TERM,
        value::INTEGRATED,
    ] {
        assert!(near(v[i], -120.0, 0.0), "value {i}: {}", v[i]);
    }
    assert!(v[value::SAMPLE_PEAK_DB] > -7.0, "the peaks still work");
    // Switching it on starts from nothing.
    h.set_param("loudness", 1.0);
    run(&mut h, 0, 750, |_, n| tone(n));
    assert!(values(&h)[value::MOMENTARY] > -20.0);
}

#[test]
fn the_tap_holds_the_input_for_the_goniometer() {
    let mut h = device();
    let tap = h.device().scope_tap().expect("a tap");
    let mut noise = [Rng::new(21), Rng::new(22)];
    let (i, _) = run(&mut h, 0, 40, |c, _| noise[c].next_bipolar());
    let (mut l, mut r) = (vec![0.0; 2048], vec![0.0; 2048]);
    let (n, head) = tap.read_latest(&mut l, &mut r);
    assert_eq!((n, head), (2048, (40 * BATCH) as u64));
    assert_eq!(bits(&l), bits(&i[0][i[0].len() - 2048..]));
    assert_eq!(bits(&r), bits(&i[1][i[1].len() - 2048..]));
}

#[test]
fn the_results_do_not_depend_on_the_sample_rate_setup_order() {
    // Prepared at 44.1 kHz the hold is still a second.
    let info = PrepareInfo {
        sample_rate: atomik_core::SampleRate::new(44_100.0).unwrap(),
        seed: 0,
    };
    let mut d = PhaseMeter::new();
    d.set_param(ParamId(P_HOLD), 2.0);
    d.prepare(&info);
    assert_eq!(d.hold, Some(3 * 44_100));
}
