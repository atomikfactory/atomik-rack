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
            "bypass",
        ],
        &["in_l", "in_r", "out_l", "out_r"],
    );
    assert_eq!(DESCRIPTOR.type_id, "atomik.spectrum");
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Effect);
    assert_eq!(SIZES.len(), SIZE_NAMES.len());
    let size = DESCRIPTOR.param_by_key("size").unwrap();
    assert_eq!(SIZES[size.default as usize], 4096);
    assert_eq!(
        DESCRIPTOR.param_by_key("channel").unwrap().default as usize,
        4
    );
    assert_eq!(CHANNEL_NAMES[4], "Sum");
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
    let mut v = [0.0f32; 8];
    assert_eq!(h.device().meter_values(&mut v), 2);
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
    for (f, size) in [(220.0f32, 4096usize), (1000.0, 8192), (7000.0, 2048)] {
        let mut h = device();
        let tap = h.device().scope_tap().unwrap();
        run(&mut h, 0, 200, |_, n| {
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
