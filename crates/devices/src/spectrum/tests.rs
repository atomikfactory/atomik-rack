//! Spectrum analyser device tests: the audio is untouched, the tap sees the same frames, the
//! params are stored, and the analysis the UI runs on the tap finds the tone that went in.

use super::*;
use crate::test_support::{assert_keys, harness, sweep_params_min_max};
use atomik_core::BATCH;
use atomik_dsp::Rng;
use atomik_dsp::spectrum::{ChannelMode, Settings, Spectrum as Display, WindowKind};
use atomik_testkit::harness::DeviceHarness;
use atomik_testkit::no_alloc::assert_no_alloc;

fn device() -> DeviceHarness {
    harness(Box::new(Spectrum::new()))
}

/// Feeds `batches` batches of `signal(channel, absolute sample)` and returns (input, output)
/// per channel.
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

fn bits(v: &[f32]) -> Vec<u32> {
    v.iter().map(|x| x.to_bits()).collect()
}

#[test]
fn descriptor_validates_and_follows_the_effect_conventions() {
    assert_keys(
        &DESCRIPTOR,
        &[
            "size",
            "window",
            "channel",
            "mode",
            "averaging",
            "hold",
            "decay",
            "range",
            "tilt",
            "smoothing",
            "weighting",
            "reference",
            "reference.capture",
            "bypass",
            "stats.reset",
            "clip_level",
        ],
        &["in_l", "in_r", "out_l", "out_r"],
    );
    assert_eq!(DESCRIPTOR.type_id, "atomik.spectrum");
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Effect);
    assert_eq!(SIZES.len(), SIZE_NAMES.len());
    assert_eq!(SIZES[SIZES.len() - 1], 65_536);
    assert_eq!(SIZES[4], 16_384, "the first five sizes keep their places");
    let size_max = DESCRIPTOR.param_by_key("size").unwrap().max as usize;
    assert_eq!(size_max, SIZES.len() - 1);
    assert_eq!(
        DESCRIPTOR.param_by_key("mode").unwrap().max as usize,
        MODE_NAMES.len() - 1
    );
    assert_eq!(MODE_NAMES[2], "Spectrogram");
    for (key, names) in [
        ("smoothing", SMOOTHING_NAMES.len()),
        ("weighting", WEIGHTING_NAMES.len()),
        ("reference", REFERENCE_NAMES.len()),
    ] {
        let p = DESCRIPTOR.param_by_key(key).unwrap();
        assert_eq!(p.max as usize, names - 1, "{key}");
        assert_eq!(p.default, 0.0, "{key}");
    }
    let size = DESCRIPTOR.param_by_key("size").unwrap();
    assert_eq!(SIZES[size.default as usize], 4096);
    assert_eq!(
        DESCRIPTOR.param_by_key("channel").unwrap().default as usize,
        4
    );
    assert_eq!(CHANNEL_NAMES[4], "Sum");
    // The old choices keep their values; the overlays are appended.
    assert_eq!(&CHANNEL_NAMES[..4], &["Left", "Right", "Mid", "Side"]);
    assert_eq!(&CHANNEL_NAMES[5..], &["L + R", "M + S"]);
    assert_eq!(
        DESCRIPTOR.param_by_key("channel").unwrap().max as usize,
        CHANNEL_NAMES.len() - 1
    );
    // New params come after the old ones, with the old ids.
    for (key, id) in [
        ("reference.capture", 12),
        ("bypass", 13),
        ("stats.reset", 14),
        ("clip_level", 15),
    ] {
        assert_eq!(DESCRIPTOR.param_by_key(key).unwrap().id.0, id, "{key}");
    }
    assert_eq!(DESCRIPTOR.param_by_key("clip_level").unwrap().default, 0.0);
    assert_eq!(DESCRIPTOR.param_by_key("stats.reset").unwrap().default, 0.0);
    assert_eq!(TILT_NAMES.len(), TILT_DB_PER_OCT.len());
    // The factory wraps it with the bypass.
    let mut r = atomik_device_sdk::Registry::new();
    r.register(factory()).unwrap();
    assert_eq!(r.create("atomik.spectrum").unwrap().latency_frames(), 0);
}

#[test]
fn the_audio_passes_through_bit_for_bit_with_no_latency_whatever_the_settings() {
    let mut noise = [Rng::new(1), Rng::new(2)];
    let signal = move |c: usize, _n: usize| noise[c].next_bipolar() * 0.9;
    let mut h = device();
    let (i, o) = run(&mut h, 0, 100, signal);
    assert_eq!(bits(&i[0]), bits(&o[0]));
    assert_eq!(bits(&i[1]), bits(&o[1]));
    assert_eq!(h.device().latency_frames(), 0);
    // Every param at its extremes: the output is still the input.
    for p in DESCRIPTOR.params.iter().filter(|p| p.key != "bypass") {
        for v in [p.min, p.max] {
            h.set_param(p.key, v);
            let mut noise = [Rng::new(3), Rng::new(4)];
            let (i, o) = run(&mut h, 0, 20, move |c, _| noise[c].next_bipolar());
            assert_eq!(bits(&i[0]), bits(&o[0]), "{} = {v}", p.key);
            assert_eq!(bits(&i[1]), bits(&o[1]), "{} = {v}", p.key);
        }
    }
    // Non-finite samples pass too (the meters are not the place to hide them).
    *h.input_mut("in_l") = [f32::NAN; BATCH];
    h.process();
    assert!(h.output("out_l").iter().all(|v| v.is_nan()));
}

#[test]
fn it_is_real_time_safe() {
    let mut h = device();
    run(&mut h, 0, 4, |_, n| (n as f32 * 0.01).sin());
    // The harness guards `process`; guard the rest too.
    assert_no_alloc(|| {
        for p in DESCRIPTOR.params {
            h.set_param(p.key, p.max);
        }
        h.device_mut().reset();
    });
    sweep_params_min_max(&mut h, &DESCRIPTOR);
}

#[test]
fn the_tap_holds_the_frames_that_went_in() {
    let mut h = device();
    let tap = h.device().scope_tap().expect("a tap");
    assert!(
        Arc::ptr_eq(&tap, &h.device().scope_tap().unwrap()),
        "always the same tap"
    );
    let (i, _) = run(&mut h, 0, 50, |c, n| {
        (n as f32 * 0.001).sin() * if c == 0 { 1.0 } else { -0.5 }
    });
    assert_eq!(tap.head(), (50 * BATCH) as u64);
    let (mut l, mut r) = (vec![0.0; 1000], vec![0.0; 1000]);
    let (n, head) = tap.read_latest(&mut l, &mut r);
    assert_eq!((n, head), (1000, (50 * BATCH) as u64));
    assert_eq!(bits(&l), bits(&i[0][i[0].len() - 1000..]));
    assert_eq!(bits(&r), bits(&i[1][i[1].len() - 1000..]));
    // An unconnected input reads silence, so the tap gets silence for it.
    *h.input_mut("in_r") = [0.0; BATCH];
    h.process();
    let (mut l, mut r) = (vec![1.0; 4], vec![1.0; 4]);
    tap.read_latest(&mut l, &mut r);
    assert_eq!(bits(&r), vec![0; 4]);
}

#[test]
fn the_meter_values_are_the_recent_peaks_of_the_two_inputs() {
    let mut h = device();
    assert!(h.device().has_meter_values());
    run(&mut h, 0, 20, |c, n| {
        if n == 8 * BATCH + 5 {
            if c == 0 { 0.8 } else { -0.4 }
        } else {
            0.0
        }
    });
    let mut v = [0.0f32; 12];
    assert_eq!(h.device().meter_values(&mut v), METER_COUNT);
    // The peak was 16 batches ago or less: the window still holds it.
    assert!(
        (v[0] - 0.8).abs() < 1e-6 && (v[1] - 0.4).abs() < 1e-6,
        "{v:?}"
    );
    run(&mut h, 20 * BATCH, 20, |_, _| 0.0);
    h.device().meter_values(&mut v);
    assert!(v[0] < 1e-9 && v[1] < 1e-9, "gone after the window: {v:?}");
}

#[test]
fn the_analysis_of_the_tap_finds_the_tone_that_went_in() {
    for (f, size) in [
        (220.0f32, 4096usize),
        (1000.0, 8192),
        (7000.0, 2048),
        (330.0, 32_768),
        (110.0, 65_536),
    ] {
        let mut h = device();
        let tap = h.device().scope_tap().unwrap();
        run(&mut h, 0, 1100, |_, n| {
            (0.25 * (std::f64::consts::TAU * f64::from(f) * n as f64 / 48_000.0).sin()) as f32
        });
        let (mut l, mut r) = (vec![0.0; size], vec![0.0; size]);
        let (n, _) = tap.read_latest(&mut l, &mut r);
        assert_eq!(n, size);
        let mut display = Display::new();
        let s = Settings {
            size,
            window: WindowKind::Hann,
            channel: ChannelMode::Sum,
            averaging_ms: 0.0,
            ..Settings::default()
        };
        display.update(&l, &r, 0, 48_000.0, &s, 200);
        let bin = 48_000.0 / size as f32;
        assert!(
            (display.peak_hz() - f).abs() < bin,
            "{f} Hz: read {} Hz",
            display.peak_hz()
        );
        assert!(
            (display.peak_db() - (-12.04)).abs() < 1.5,
            "{}",
            display.peak_db()
        );
    }
}

// ------------------------------------------------------------------ measurements

fn values(h: &DeviceHarness) -> [f32; METER_COUNT] {
    let mut v = [0.0f32; METER_COUNT];
    assert_eq!(h.device().meter_values(&mut v), METER_COUNT);
    v
}

fn pulse_reset(h: &mut DeviceHarness) {
    h.set_param("stats.reset", 1.0);
    run(h, 0, 1, |_, _| 0.0);
    h.set_param("stats.reset", 0.0);
}

#[test]
fn the_meter_indices_are_pinned() {
    assert_eq!(METER_COUNT, 9);
    assert_eq!(
        [
            value::PEAK_L,
            value::PEAK_R,
            value::CORRELATION,
            value::CORRELATION_LONG,
            value::SAMPLE_PEAK_L_DB,
            value::SAMPLE_PEAK_R_DB,
            value::TRUE_PEAK_L_DB,
            value::TRUE_PEAK_R_DB,
            value::CLIPS,
        ],
        [0, 1, 2, 3, 4, 5, 6, 7, 8]
    );
}

#[test]
fn the_correlation_is_plus_one_for_equal_sides_minus_one_for_inverted_and_zero_for_noise() {
    let tone = |n: usize| 0.5 * (n as f32 * 0.05).sin();
    let mut h = device();
    run(&mut h, 0, 400, |_, n| tone(n));
    let v = values(&h);
    assert!(v[value::CORRELATION] > 0.999, "{v:?}");
    assert!(v[value::CORRELATION_LONG] > 0.999, "{v:?}");
    let mut h = device();
    run(
        &mut h,
        0,
        400,
        |c, n| if c == 0 { tone(n) } else { -tone(n) },
    );
    let v = values(&h);
    assert!(v[value::CORRELATION] < -0.999, "{v:?}");
    assert!(v[value::CORRELATION_LONG] < -0.999, "{v:?}");
    let mut noise = [Rng::new(11), Rng::new(12)];
    let mut h = device();
    run(&mut h, 0, 2500, move |c, _| noise[c].next_bipolar() * 0.5);
    let v = values(&h);
    assert!(v[value::CORRELATION].abs() < 0.15, "{v:?}");
    assert!(v[value::CORRELATION_LONG].abs() < 0.08, "{v:?}");
    // Silence reads zero, not a stale value.
    let mut h = device();
    run(&mut h, 0, 10, |_, _| 0.0);
    assert_eq!(values(&h)[value::CORRELATION], 0.0);
}

#[test]
fn peaks_and_clips_are_counted_exactly() {
    let mut h = device();
    let v = values(&h);
    assert_eq!(v[value::SAMPLE_PEAK_L_DB], -120.0);
    assert_eq!(v[value::CLIPS], 0.0);
    // A single full-scale sample on the left, half scale on the right.
    run(&mut h, 0, 20, |c, n| match (c, n) {
        (0, 777) => 1.0,
        (1, 100) => -0.5,
        _ => 0.0,
    });
    let v = values(&h);
    assert_eq!(v[value::SAMPLE_PEAK_L_DB], 0.0, "{v:?}");
    assert!((v[value::SAMPLE_PEAK_R_DB] + 6.02).abs() < 0.01, "{v:?}");
    assert_eq!(v[value::CLIPS], 1.0);
    assert!(v[value::TRUE_PEAK_L_DB] >= -0.0001, "{v:?}");
    // 0 dBFS minus the largest peak is what the UI calls headroom.
    assert_eq!(
        -v[value::SAMPLE_PEAK_L_DB].max(v[value::SAMPLE_PEAK_R_DB]),
        0.0
    );
    // The peaks hold, the clip count grows: an over (-1.5) is one clipped sample per hit, both
    // channels count, and the audio still passes.
    let (i, o) = run(&mut h, 0, 4, |c, n| {
        if n % 100 == 0 {
            if c == 0 { -1.5 } else { 1.0 }
        } else {
            0.0
        }
    });
    assert_eq!(bits(&i[0]), bits(&o[0]));
    let v = values(&h);
    assert_eq!(v[value::CLIPS], 1.0 + 2.0 * 3.0, "{v:?}"); // n = 0, 100, 200 of 256 frames
    assert!((v[value::SAMPLE_PEAK_L_DB] - 3.52).abs() < 0.01, "{v:?}");
    // The clip level moves the threshold: -6 dB counts 0.6 but not 0.4.
    let mut h = device();
    h.set_param("clip_level", -6.0);
    run(&mut h, 0, 2, |c, n| match (c, n) {
        (0, 5) => 0.6,
        (1, 5) => 0.4,
        _ => 0.0,
    });
    assert_eq!(values(&h)[value::CLIPS], 1.0);
}

#[test]
fn the_true_peak_sees_the_peak_between_the_samples() {
    // A quarter-sample-rate tone at 45 degrees: every sample is 0.7071 and the waveform
    // between them reaches 1.0.
    let mut h = device();
    run(&mut h, 0, 40, |_, n| {
        (std::f32::consts::FRAC_PI_2 * n as f32 + std::f32::consts::FRAC_PI_4).sin()
    });
    let v = values(&h);
    assert!((v[value::SAMPLE_PEAK_L_DB] + 3.01).abs() < 0.05, "{v:?}");
    assert!(
        v[value::TRUE_PEAK_L_DB] > -0.5 && v[value::TRUE_PEAK_L_DB] < 0.5,
        "{v:?}"
    );
    assert!(
        v[value::TRUE_PEAK_L_DB] > v[value::SAMPLE_PEAK_L_DB] + 2.0,
        "{v:?}"
    );
    // Never below the sample peak.
    assert_eq!(v[value::TRUE_PEAK_R_DB], v[value::TRUE_PEAK_L_DB]);
}

#[test]
fn reset_clears_the_peaks_and_the_clips_and_a_held_toggle_does_it_once() {
    let mut h = device();
    run(&mut h, 0, 5, |_, n| if n == 10 { 1.0 } else { 0.0 });
    assert_eq!(values(&h)[value::CLIPS], 2.0);
    h.set_param("stats.reset", 1.0);
    run(&mut h, 0, 1, |_, _| 0.0);
    let v = values(&h);
    assert_eq!(v[value::CLIPS], 0.0);
    assert_eq!(v[value::SAMPLE_PEAK_L_DB], -120.0);
    assert_eq!(v[value::TRUE_PEAK_L_DB], -120.0);
    // Held at 1 it is one edge: new peaks stay.
    run(&mut h, 0, 2, |_, n| if n == 3 { 0.5 } else { 0.0 });
    assert!((values(&h)[value::SAMPLE_PEAK_L_DB] + 6.02).abs() < 0.01);
    h.set_param("stats.reset", 0.0);
    pulse_reset(&mut h);
    assert_eq!(values(&h)[value::SAMPLE_PEAK_L_DB], -120.0);
    // A transport reset leaves the peaks alone.
    run(&mut h, 0, 2, |_, n| if n == 3 { 0.5 } else { 0.0 });
    h.device_mut().reset();
    assert!((values(&h)[value::SAMPLE_PEAK_L_DB] + 6.02).abs() < 0.01);
}

#[test]
fn non_finite_samples_do_not_poison_the_measurements() {
    let mut h = device();
    *h.input_mut("in_l") = [f32::NAN; BATCH];
    *h.input_mut("in_r") = [f32::INFINITY; BATCH];
    h.process();
    run(&mut h, 0, 400, |_, n| 0.25 * (n as f32 * 0.05).sin());
    let v = values(&h);
    assert!(v.iter().all(|x| x.is_finite()), "{v:?}");
    assert!(
        v[value::CORRELATION] > 0.99 && v[value::TRUE_PEAK_L_DB] < -11.0,
        "{v:?}"
    );
}
