//! Tonal balance meter device tests: the audio is untouched, the tap sees the same frames, the
//! params are stored, and the analysis of the tap reads the balance of what went in.

use super::*;
use crate::test_support::{assert_keys, harness, sweep_params_min_max};
use atomik_core::BATCH;
use atomik_dsp::Rng;
use atomik_testkit::harness::DeviceHarness;
use atomik_testkit::no_alloc::assert_no_alloc;

fn device() -> DeviceHarness {
    harness(Box::new(TonalBalance::new()))
}

fn run(
    h: &mut DeviceHarness,
    batches: usize,
    mut signal: impl FnMut(usize, usize) -> f32,
) -> ([Vec<f32>; 2], [Vec<f32>; 2]) {
    let mut input = [Vec::new(), Vec::new()];
    let mut output = [Vec::new(), Vec::new()];
    for b in 0..batches {
        for (c, key) in ["in_l", "in_r"].into_iter().enumerate() {
            let batch: [f32; BATCH] = std::array::from_fn(|i| signal(c, b * BATCH + i));
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
            "response",
            "view",
            "channel",
            "learn",
            "bypass",
            "hidden.target",
        ],
        &["in_l", "in_r", "out_l", "out_r"],
    );
    assert_eq!(DESCRIPTOR.type_id, "atomik.tonal-balance");
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Effect);
    assert_eq!(RESPONSE_NAMES.len(), RESPONSE_MS.len());
    for (key, names) in [
        ("response", RESPONSE_NAMES.len()),
        ("view", VIEW_NAMES.len()),
        ("channel", CHANNEL_NAMES.len()),
    ] {
        assert_eq!(
            DESCRIPTOR.param_by_key(key).unwrap().max as usize,
            names - 1
        );
    }
    let target = DESCRIPTOR.param_by_key("hidden.target").unwrap();
    assert_eq!(target.default, 0.0, "Balanced");
    assert_eq!(target.max, TARGET_MAX);
    assert!(target.discrete);
    assert_eq!(DESCRIPTOR.param_by_key("response").unwrap().default, 1.0);
    assert_eq!(DESCRIPTOR.param_by_key("view").unwrap().default, 2.0);
    assert_eq!(DESCRIPTOR.param_by_key("learn").unwrap().default, 0.0);
    let mut r = atomik_device_sdk::Registry::new();
    r.register(factory()).unwrap();
    assert_eq!(
        r.create("atomik.tonal-balance").unwrap().latency_frames(),
        0
    );
}

#[test]
fn the_audio_passes_through_bit_for_bit_with_no_latency_whatever_the_settings() {
    let mut noise = [Rng::new(1), Rng::new(2)];
    let mut h = device();
    let (i, o) = run(&mut h, 100, move |c, _| noise[c].next_bipolar() * 0.9);
    assert_eq!(bits(&i[0]), bits(&o[0]));
    assert_eq!(bits(&i[1]), bits(&o[1]));
    assert_eq!(h.device().latency_frames(), 0);
    for p in DESCRIPTOR.params.iter().filter(|p| p.key != "bypass") {
        for v in [p.min, p.max] {
            h.set_param(p.key, v);
            let mut noise = [Rng::new(3), Rng::new(4)];
            let (i, o) = run(&mut h, 20, move |c, _| noise[c].next_bipolar());
            assert_eq!(bits(&i[0]), bits(&o[0]), "{} = {v}", p.key);
            assert_eq!(bits(&i[1]), bits(&o[1]), "{} = {v}", p.key);
        }
    }
    *h.input_mut("in_l") = [f32::NAN; BATCH];
    h.process();
    assert!(h.output("out_l").iter().all(|v| v.is_nan()));
}

#[test]
fn it_is_real_time_safe() {
    let mut h = device();
    run(&mut h, 4, |_, n| (n as f32 * 0.01).sin());
    assert_no_alloc(|| {
        for p in DESCRIPTOR.params {
            h.set_param(p.key, p.max);
        }
        h.device_mut().reset();
    });
    sweep_params_min_max(&mut h, &DESCRIPTOR);
}

#[test]
fn the_tap_holds_the_frames_that_went_in_and_the_meters_are_the_recent_peaks() {
    let mut h = device();
    let tap = h.device().scope_tap().expect("a tap");
    assert!(Arc::ptr_eq(&tap, &h.device().scope_tap().unwrap()));
    let (i, _) = run(&mut h, 50, |c, n| {
        (n as f32 * 0.001).sin() * if c == 0 { 1.0 } else { -0.5 }
    });
    assert_eq!(tap.head(), (50 * BATCH) as u64);
    let (mut l, mut r) = (vec![0.0; 1000], vec![0.0; 1000]);
    let (n, head) = tap.read_latest(&mut l, &mut r);
    assert_eq!((n, head), (1000, (50 * BATCH) as u64));
    assert_eq!(bits(&l), bits(&i[0][i[0].len() - 1000..]));
    assert_eq!(bits(&r), bits(&i[1][i[1].len() - 1000..]));
    assert!(
        tap.frames() >= atomik_dsp::tonal::FFT_SIZE * 2,
        "room for the analysis"
    );
    let mut v = [0.0f32; 8];
    assert!(h.device().has_meter_values());
    assert_eq!(h.device().meter_values(&mut v), 2);
    assert!(v[0] > 0.8 && v[1] > 0.4, "{v:?}");
}

#[test]
fn the_analysis_of_the_tap_reads_the_balance_that_went_in() {
    use atomik_dsp::spectrum::band_centres;
    use atomik_dsp::tonal::{FFT_SIZE, SHAPES, builtin, compare, shape_db};
    let sr = 48_000.0f64;
    // A sine at each band centre, shaped like the Warm target.
    let tones: Vec<(f64, f32)> = band_centres()
        .iter()
        .map(|f| {
            let bin = (f64::from(*f) / sr * FFT_SIZE as f64).round().max(1.0);
            let amp = 10f32.powf(shape_db(&SHAPES[2], *f) / 20.0) * 0.02;
            (bin * sr / FFT_SIZE as f64, amp)
        })
        .collect();
    let mut h = device();
    run(&mut h, 400, |_, n| {
        tones
            .iter()
            .map(|(hz, a)| a * (std::f64::consts::TAU * hz * n as f64 / sr).sin() as f32)
            .sum()
    });
    let tap = h.device().scope_tap().unwrap();
    let (mut l, mut r) = (vec![0.0; FFT_SIZE], vec![0.0; FFT_SIZE]);
    let (n, _) = tap.read_latest(&mut l, &mut r);
    assert_eq!(n, FFT_SIZE);
    let levels = atomik_dsp::tonal::band_levels_of(&l, &r, sr as f32);
    let warm = compare(&levels, &builtin(2)).unwrap();
    for d in warm.deviation {
        assert!(d.abs() < 1.0, "{:?}", warm.deviation);
    }
    let bright = compare(&levels, &builtin(1)).unwrap();
    assert!(
        bright.deviation[3] < -2.0,
        "less top than Bright: {:?}",
        bright.deviation
    );
}
