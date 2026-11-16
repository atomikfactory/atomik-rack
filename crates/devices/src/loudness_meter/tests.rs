//! Loudness meter device tests: the audio is untouched, the loudness matches the reference tone,
//! the range, peaks and ratios follow their definitions, a reset clears, hold freezes, the
//! presets judge a programme, and the indices of the published values are pinned.

use super::*;
use crate::test_support::{assert_keys, harness, sweep_params_min_max};
use atomik_dsp::Rng;
use atomik_testkit::harness::DeviceHarness;
use atomik_testkit::no_alloc::assert_no_alloc;
use std::f64::consts::TAU;

const SR: f64 = 48_000.0;

fn device() -> DeviceHarness {
    harness(Box::new(LoudnessMeterDevice::new()))
}

fn device_at(sr: f64) -> DeviceHarness {
    DeviceHarness::new(Box::new(LoudnessMeterDevice::new()), sr)
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

/// Runs `seconds` of a signal without keeping the audio.
fn feed(
    h: &mut DeviceHarness,
    start: usize,
    seconds: f64,
    sr: f64,
    mut signal: impl FnMut(usize, usize) -> f32,
) -> usize {
    let batches = (seconds * sr / BATCH as f64).round() as usize;
    for b in 0..batches {
        for (c, key) in ["in_l", "in_r"].into_iter().enumerate() {
            let batch: [f32; BATCH] = std::array::from_fn(|i| signal(c, start + b * BATCH + i));
            *h.input_mut(key) = batch;
        }
        h.process();
    }
    start + batches * BATCH
}

fn values(h: &DeviceHarness) -> [f32; METERS] {
    let mut v = [f32::NAN; METERS];
    assert_eq!(h.device().meter_values(&mut v), METERS);
    v
}

fn bits(v: &[f32]) -> Vec<u32> {
    v.iter().map(|x| x.to_bits()).collect()
}

/// A sine of `level` dBFS peak.
fn tone(hz: f64, level: f64, sr: f64) -> impl Fn(usize) -> f32 {
    let amp = 10f64.powf(level / 20.0);
    move |n| (amp * (TAU * hz * n as f64 / sr).sin()) as f32
}

fn near(a: f32, b: f32, tol: f32) -> bool {
    (a - b).abs() <= tol
}

/// Both channels carry the tone.
fn stereo(level: f64) -> impl Fn(usize, usize) -> f32 {
    let t = tone(1_000.0, level, SR);
    move |_, n| t(n)
}

/// Only the left channel carries the tone.
fn left_only(level: f64) -> impl Fn(usize, usize) -> f32 {
    let t = tone(1_000.0, level, SR);
    move |c, n| if c == 0 { t(n) } else { 0.0 }
}

#[test]
fn descriptor_validates_and_follows_the_effect_conventions() {
    assert_keys(
        &DESCRIPTOR,
        &[
            "preset",
            "target_lufs",
            "max_tp",
            "history_span",
            "reset",
            "hold",
            "bypass",
        ],
        &["in_l", "in_r", "out_l", "out_r"],
    );
    assert_eq!(DESCRIPTOR.type_id, "atomik.loudness-meter");
    assert_eq!(DESCRIPTOR.name, "Loudness Meter");
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Effect);
    let preset = DESCRIPTOR.param_by_key("preset").unwrap();
    assert_eq!(preset.max as usize, PRESET_NAMES.len() - 1);
    assert_eq!(PRESET_NAMES[PRESET_CUSTOM], "Custom", "Custom is last");
    assert_eq!(PRESETS.len(), PRESET_CUSTOM);
    let span = DESCRIPTOR.param_by_key("history_span").unwrap();
    assert_eq!(span.max as usize, SPAN_NAMES.len() - 1);
    assert_eq!(SPAN_NAMES.len(), SPAN_SECONDS.len());
    assert!(SPAN_SECONDS.windows(2).all(|w| w[0] < w[1]));
    // The longest span fits in the history the device keeps (one point every half second).
    assert!(SPAN_SECONDS[SPAN_SECONDS.len() - 1] * 2.0 <= HISTORY_MAX as f32);
    let mut r = atomik_device_sdk::Registry::new();
    r.register(factory()).unwrap();
    assert_eq!(
        r.create("atomik.loudness-meter").unwrap().latency_frames(),
        0
    );
}

#[test]
fn no_name_is_a_third_party_product() {
    let all = PRESET_NAMES
        .iter()
        .chain(&SPAN_NAMES)
        .copied()
        .chain([DESCRIPTOR.name])
        .chain(DESCRIPTOR.params.iter().map(|p| p.name))
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    for brand in [
        "spotify", "apple", "youtube", "tidal", "amazon", "deezer", "tiktok",
    ] {
        assert!(!all.contains(brand), "{brand}");
    }
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
    h.set_param("reset", 0.0);
    h.set_param("reset", 1.0);
    *h.input_mut("in_l") = [f32::NAN; BATCH];
    *h.input_mut("in_r") = [f32::INFINITY; BATCH];
    h.process();
    assert!(h.output("out_l").iter().all(|v| v.is_nan()));
    assert!(values(&h).iter().all(|v| v.is_finite()));
    // ... and the detectors still work afterwards.
    h.set_param("reset", 0.0);
    h.set_param("hold", 0.0);
    feed(&mut h, 0, 5.0, SR, stereo(-20.0));
    assert!(near(values(&h)[value::TRUE_PEAK], -20.0, 0.3));
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
    h.set_param("hold", 0.0);
    // A long run crosses every once-a-second and half-second path, history included.
    let t = stereo(-18.0);
    assert_no_alloc(|| {
        feed(&mut h, 0, 20.0, SR, &t);
    });
    assert!(values(&h)[value::HISTORY_POINTS] > 20.0);
    sweep_params_min_max(&mut h, &DESCRIPTOR);
}

#[test]
fn a_reference_tone_reads_its_level_in_every_measure() {
    // 1 kHz, -23 dBFS peak in both channels: -23 LUFS (EBU Tech 3341 case 1).
    let mut h = device();
    feed(&mut h, 0, 20.0, SR, stereo(-23.0));
    let v = values(&h);
    assert!(
        near(v[value::MOMENTARY], -23.0, 0.1),
        "{}",
        v[value::MOMENTARY]
    );
    assert!(
        near(v[value::SHORT_TERM], -23.0, 0.1),
        "{}",
        v[value::SHORT_TERM]
    );
    assert!(
        near(v[value::INTEGRATED], -23.0, 0.1),
        "{}",
        v[value::INTEGRATED]
    );
    // A steady tone has no range.
    assert!(v[value::LRA] < 0.2, "{}", v[value::LRA]);
    // Peaks: the sine's own, both channels alike.
    for i in [
        value::TRUE_PEAK,
        value::TRUE_PEAK_L,
        value::TRUE_PEAK_R,
        value::SAMPLE_PEAK,
    ] {
        assert!(near(v[i], -23.0, 0.2), "{i}: {}", v[i]);
    }
    // Ratios of a tone at its loudness: PLR 0, PSR 0, crest 3.01 dB.
    assert!(near(v[value::PLR], 0.0, 0.25), "{}", v[value::PLR]);
    assert!(near(v[value::PSR], 0.0, 0.25), "{}", v[value::PSR]);
    assert!(near(v[value::DR], 3.01, 0.1), "{}", v[value::DR]);
    assert!(near(v[value::TIME], 20.0, 0.1), "{}", v[value::TIME]);
}

#[test]
fn the_reference_tone_reads_right_at_other_sample_rates() {
    for sr in [44_100.0, 96_000.0] {
        let mut h = device_at(sr);
        let t = tone(1_000.0, -23.0, sr);
        feed(&mut h, 0, 15.0, sr, |_, n| t(n));
        let v = values(&h);
        assert!(
            near(v[value::INTEGRATED], -23.0, 0.1),
            "{sr}: {}",
            v[value::INTEGRATED]
        );
        assert!(near(v[value::SHORT_TERM], -23.0, 0.1), "{sr}");
        assert!(near(v[value::DR], 3.01, 0.1), "{sr}: {}", v[value::DR]);
    }
}

#[test]
fn the_gated_integrated_loudness_follows_ebu_tech_3341() {
    // Case 3: 10 s at -36, 60 s at -23, 10 s at -36 -> -23.0 (the quiet parts are gated out).
    let mut h = device();
    let mut at = 0;
    for (secs, level) in [(10.0, -36.0), (60.0, -23.0), (10.0, -36.0)] {
        at = feed(&mut h, at, secs, SR, stereo(level));
    }
    let i = values(&h)[value::INTEGRATED];
    assert!(near(i, -23.0, 0.1), "{i}");
}

#[test]
fn the_loudness_range_of_a_two_level_programme() {
    // 30 s at -33 then 30 s at -23: the 10th to 95th percentile of the short-term values
    // spans the 10 LU between the levels.
    let mut h = device();
    let at = feed(&mut h, 0, 30.0, SR, stereo(-33.0));
    feed(&mut h, at, 30.0, SR, stereo(-23.0));
    let lra = values(&h)[value::LRA];
    assert!(near(lra, 10.0, 0.5), "{lra}");
    // One level alone has none. A fade into silence adds the few short-term values it passes
    // through, and the silence itself (below the absolute gate) adds nothing more.
    let mut h = device();
    let at = feed(&mut h, 0, 20.0, SR, stereo(-23.0));
    let at = feed(&mut h, at, 8.0, SR, |_, _| 0.0);
    let after_fade = values(&h)[value::LRA];
    assert!(after_fade < 6.0, "{after_fade}");
    feed(&mut h, at, 20.0, SR, |_, _| 0.0);
    assert_eq!(values(&h)[value::LRA], after_fade);
    // Nothing yet reads as none.
    assert_eq!(values(&device())[value::LRA], -120.0);
}

#[test]
fn the_true_peak_reads_an_inter_sample_peak() {
    // fs/4 sine at 45 degrees with a -6 dBFS sample peak: true peak -3.01 dBTP.
    let a = 0.5 * std::f64::consts::SQRT_2;
    let s =
        move |n: usize| (a * (TAU * 0.25 * n as f64 + std::f64::consts::FRAC_PI_4).sin()) as f32;
    let mut h = device();
    feed(&mut h, 0, 2.0, SR, |c, n| if c == 0 { s(n) } else { 0.0 });
    let v = values(&h);
    assert!(
        near(v[value::SAMPLE_PEAK], -6.02, 0.05),
        "{}",
        v[value::SAMPLE_PEAK]
    );
    assert!(
        near(v[value::TRUE_PEAK], -3.01, 0.25),
        "{}",
        v[value::TRUE_PEAK]
    );
    // Per channel: the left one carries it, the right one nothing.
    assert!(
        near(v[value::TRUE_PEAK_L], -3.01, 0.25),
        "{}",
        v[value::TRUE_PEAK_L]
    );
    assert_eq!(v[value::TRUE_PEAK_R], -120.0);
    assert_eq!(
        v[value::TRUE_PEAK],
        v[value::TRUE_PEAK_L].max(v[value::TRUE_PEAK_R])
    );
}

#[test]
fn plr_psr_and_dr_follow_their_definitions() {
    // One channel at -20 dBFS: loudness -20 - 3.01 = -23.0 LUFS.
    let mut h = device();
    feed(&mut h, 0, 20.0, SR, left_only(-20.0));
    let v = values(&h);
    assert!(
        near(v[value::INTEGRATED], -23.0, 0.1),
        "{}",
        v[value::INTEGRATED]
    );
    // PLR = true peak - integrated = -20 - -23 = 3.0.
    assert!(near(v[value::PLR], 3.0, 0.3), "{}", v[value::PLR]);
    assert!(near(
        v[value::PLR],
        v[value::TRUE_PEAK] - v[value::INTEGRATED],
        1e-3
    ));
    // PSR = the true peak of the last 3 s - the short-term loudness: also 3.0 here.
    assert!(near(v[value::PSR], 3.0, 0.3), "{}", v[value::PSR]);
    // DR = sample peak - the RMS of both channels together: the sine's RMS is 3.01 below its
    // peak, and one silent channel takes another 3.01.
    assert!(near(v[value::DR], 6.02, 0.1), "{}", v[value::DR]);
    // A louder, shorter burst raises PSR while it lasts, PLR and DR keep the programme.
    let at = feed(&mut h, 48_000 * 20, 4.0, SR, left_only(-10.0));
    let v = values(&h);
    assert!(
        near(v[value::TRUE_PEAK], -10.0, 0.3),
        "{}",
        v[value::TRUE_PEAK]
    );
    assert!(v[value::PLR] > 3.0, "the maximum rose: {}", v[value::PLR]);
    // Quiet again for longer than the 3 s window: the burst's peak leaves PSR, not PLR.
    feed(&mut h, at, 8.0, SR, left_only(-30.0));
    let w = values(&h);
    assert!(
        near(w[value::TRUE_PEAK], v[value::TRUE_PEAK], 0.01),
        "{}",
        w[value::TRUE_PEAK]
    );
    assert!(
        near(w[value::PSR], 3.0, 0.4),
        "PSR follows the signal: {}",
        w[value::PSR]
    );
    assert!(w[value::PLR] > 3.0);
    // Silence for 4 s: the window holds no peak and the loudness is gone, so PSR is none.
    feed(&mut h, 0, 4.0, SR, |_, _| 0.0);
    assert_eq!(values(&h)[value::PSR], -120.0);
}

#[test]
fn reset_clears_everything_that_accumulates() {
    let mut h = device();
    let at = feed(&mut h, 0, 15.0, SR, stereo(-30.0));
    feed(&mut h, at, 15.0, SR, stereo(-20.0));
    let before = values(&h);
    assert!(before[value::INTEGRATED] > -30.0 && before[value::LRA] > 5.0);
    assert!(before[value::HISTORY_POINTS] > 10.0);
    assert!(before[value::BINS..].iter().any(|b| *b > 0.0));
    h.set_param("reset", 1.0);
    feed(&mut h, 0, 0.01, SR, |_, _| 0.0);
    let v = values(&h);
    for i in [
        value::MOMENTARY,
        value::SHORT_TERM,
        value::INTEGRATED,
        value::LRA,
        value::TRUE_PEAK,
        value::TRUE_PEAK_L,
        value::TRUE_PEAK_R,
        value::SAMPLE_PEAK,
        value::PLR,
        value::PSR,
        value::DR,
    ] {
        assert_eq!(v[i], -120.0, "value {i}");
    }
    assert_eq!(v[value::HISTORY_POINTS], 0.0);
    assert!(v[value::TIME] < 0.05);
    assert!(v[value::BINS..].iter().all(|b| *b == 0.0));
    assert_ne!(
        v[value::EPOCH],
        before[value::EPOCH],
        "the history starts over"
    );
    // Holding the toggle high does not keep resetting: the next tone is measured.
    feed(&mut h, 0, 10.0, SR, stereo(-23.0));
    let v = values(&h);
    assert!(
        near(v[value::INTEGRATED], -23.0, 0.1),
        "{}",
        v[value::INTEGRATED]
    );
    // The device's own reset clears too, and counts as one.
    let epoch = v[value::EPOCH];
    h.device_mut().reset();
    feed(&mut h, 0, 0.01, SR, |_, _| 0.0);
    let v = values(&h);
    assert_eq!(v[value::INTEGRATED], -120.0);
    assert_ne!(v[value::EPOCH], epoch);
    // Settings survive a reset.
    h.set_param("preset", 3.0);
    h.set_param("reset", 0.0);
    h.set_param("reset", 1.0);
    feed(&mut h, 0, 0.01, SR, |_, _| 0.0);
    assert_eq!(values(&h)[value::TARGET], -24.0);
}

#[test]
fn hold_freezes_the_accumulation_and_not_the_live_readings() {
    let mut h = device();
    let at = feed(&mut h, 0, 15.0, SR, stereo(-23.0));
    let frozen = values(&h);
    h.set_param("hold", 1.0);
    feed(&mut h, at, 15.0, SR, stereo(-13.0));
    let v = values(&h);
    // Momentary and short-term follow the louder tone ...
    assert!(
        near(v[value::MOMENTARY], -13.0, 0.1),
        "{}",
        v[value::MOMENTARY]
    );
    assert!(
        near(v[value::SHORT_TERM], -13.0, 0.1),
        "{}",
        v[value::SHORT_TERM]
    );
    // ... everything accumulated stays as it was.
    for i in [
        value::INTEGRATED,
        value::LRA,
        value::TRUE_PEAK,
        value::TRUE_PEAK_L,
        value::TRUE_PEAK_R,
        value::SAMPLE_PEAK,
        value::PLR,
        value::DR,
        value::HISTORY_POINTS,
        value::TIME,
    ] {
        assert_eq!(v[i].to_bits(), frozen[i].to_bits(), "value {i}");
    }
    assert_eq!(bits(&v[value::BINS..]), bits(&frozen[value::BINS..]));
    // Letting go resumes the accumulation.
    h.set_param("hold", 0.0);
    feed(&mut h, 0, 15.0, SR, stereo(-13.0));
    let v = values(&h);
    assert!(
        v[value::INTEGRATED] > frozen[value::INTEGRATED] + 3.0,
        "{}",
        v[value::INTEGRATED]
    );
    assert!(
        near(v[value::TRUE_PEAK], -13.0, 0.3),
        "{}",
        v[value::TRUE_PEAK]
    );
    assert!(v[value::TIME] > frozen[value::TIME] + 14.0);
}

#[test]
fn the_history_goes_to_the_tap_every_half_second_after_the_settle() {
    let mut h = device();
    let tap = h.device().scope_tap().expect("a tap");
    feed(&mut h, 0, 2.5, SR, stereo(-23.0));
    assert_eq!(values(&h)[value::HISTORY_POINTS], 0.0, "settling");
    assert_eq!(tap.head(), 0);
    feed(&mut h, 0, 17.5, SR, stereo(-23.0));
    let v = values(&h);
    // 20 s - 3 s settle, two points a second.
    assert!(
        near(v[value::HISTORY_POINTS], 34.0, 1.0),
        "{}",
        v[value::HISTORY_POINTS]
    );
    assert_eq!(tap.head(), v[value::HISTORY_POINTS] as u64);
    let (mut s, mut m) = (vec![0.0; 2048], vec![0.0; 2048]);
    let (n, head) = tap.read_latest(&mut s, &mut m);
    assert_eq!(n as u64, head);
    // Left is short-term, right is momentary, in LUFS.
    assert!(
        near(s[n - 1], -23.0, 0.1) && near(m[n - 1], -23.0, 0.1),
        "{} {}",
        s[n - 1],
        m[n - 1]
    );
    // The tap holds the whole longest span (the largest read is half the ring).
    assert!(tap.frames() / 2 >= HISTORY_MAX);
    // Hold records nothing.
    h.set_param("hold", 1.0);
    feed(&mut h, 0, 5.0, SR, stereo(-23.0));
    assert_eq!(tap.head(), head);
}

#[test]
fn the_history_never_grows_past_what_the_tap_keeps() {
    let mut h = device_at(8_000.0);
    // A cheap rate: 40 minutes of a quiet tone.
    let t = tone(1_000.0, -30.0, 8_000.0);
    feed(&mut h, 0, 40.0 * 60.0, 8_000.0, |_, n| t(n));
    assert_eq!(values(&h)[value::HISTORY_POINTS], HISTORY_MAX as f32);
}

#[test]
fn the_distribution_counts_the_short_term_loudness_in_one_lu_bins() {
    let mut h = device();
    let at = feed(&mut h, 0, 30.0, SR, stereo(-23.0));
    let v = values(&h);
    let bins = &v[value::BINS..];
    assert_eq!(bins.len(), BINS);
    let peak = bins
        .iter()
        .position(|b| (*b - 1.0).abs() < 1e-6)
        .expect("a tallest bin");
    // -23 LUFS sits in the bin [-23, -22): index 23 from -46.
    assert_eq!(peak, 23);
    assert!(bins.iter().enumerate().all(|(i, b)| i == peak || *b < 0.1));
    // A second level makes a second hump, half as tall when it lasts half as long.
    feed(&mut h, at, 15.0, SR, stereo(-33.0));
    let v = values(&h);
    let bins = &v[value::BINS..];
    assert_eq!(bins[23], 1.0);
    let second = bins[13];
    assert!(second > 0.3 && second < 0.7, "{second}");
    // Quiet and loud values land in the end bins; silence is not counted.
    assert_eq!(bin_of(-80.0), 0);
    assert_eq!(bin_of(5.0), BINS - 1);
    assert_eq!(bin_of(DIST_LOW), 0);
    assert_eq!(bin_of(-0.01), BINS - 1);
    let mut h = device();
    feed(&mut h, 0, 10.0, SR, |_, _| 0.0);
    assert!(values(&h)[value::BINS..].iter().all(|b| *b == 0.0));
}

#[test]
fn the_presets_name_a_target_and_a_peak_limit() {
    let mut h = device();
    feed(&mut h, 0, 0.01, SR, |_, _| 0.0);
    for (i, (target, tp)) in PRESETS.iter().enumerate() {
        h.set_param("preset", i as f32);
        feed(&mut h, 0, 0.01, SR, |_, _| 0.0);
        let v = values(&h);
        assert_eq!(
            (v[value::TARGET], v[value::MAX_TP]),
            (*target, *tp),
            "{}",
            PRESET_NAMES[i]
        );
    }
    // The names carry the numbers they mean.
    assert!(PRESET_NAMES[0].contains("-14") && PRESET_NAMES[0].contains("-1 dBTP"));
    assert!(PRESET_NAMES[2].contains("R128") && PRESET_NAMES[2].contains("-23"));
    assert!(PRESET_NAMES[3].contains("A/85") && PRESET_NAMES[3].contains("-24"));
    assert!(PRESET_NAMES[5].contains("-9"));
    // Custom takes the two params; the others ignore them.
    h.set_param("target_lufs", -18.0);
    h.set_param("max_tp", -3.0);
    h.set_param("preset", PRESET_CUSTOM as f32);
    feed(&mut h, 0, 0.01, SR, |_, _| 0.0);
    let v = values(&h);
    assert_eq!((v[value::TARGET], v[value::MAX_TP]), (-18.0, -3.0));
    h.set_param("preset", 0.0);
    feed(&mut h, 0, 0.01, SR, |_, _| 0.0);
    assert_eq!(values(&h)[value::TARGET], -14.0);
    assert_eq!(target_of(PRESET_CUSTOM, (-20.0, -2.0)), (-20.0, -2.0));
    assert_eq!(target_of(2, (-20.0, -2.0)), (-23.0, -1.0));
}

#[test]
fn the_verdict_is_ok_within_one_lu_and_names_the_side_outside() {
    assert_eq!(verdict(-14.0, -14.0), Verdict::Ok);
    assert_eq!(
        verdict(-13.0, -14.0),
        Verdict::Ok,
        "+1 LU is still on target"
    );
    assert_eq!(verdict(-15.0, -14.0), Verdict::Ok, "-1 LU too");
    assert_eq!(verdict(-12.9, -14.0), Verdict::TooLoud);
    assert_eq!(verdict(-15.1, -14.0), Verdict::TooQuiet);
    assert_eq!(verdict(-120.0, -14.0), Verdict::NoData);
    assert_eq!(verdict(f32::NAN, -14.0), Verdict::NoData);
    // The true peak fails only above its limit.
    assert!(peak_passes(-1.0, -1.0));
    assert!(peak_passes(-2.5, -1.0));
    assert!(!peak_passes(-0.9, -1.0));
    assert!(peak_passes(-120.0, -1.0), "nothing measured is no failure");
    // End to end: a tone at -23 against the three targets.
    let mut h = device();
    feed(&mut h, 0, 15.0, SR, stereo(-23.0));
    let v = values(&h);
    for (preset, want) in [
        (0, Verdict::TooQuiet),
        (2, Verdict::Ok),
        (5, Verdict::TooQuiet),
    ] {
        let (target, max_tp) = target_of(preset, (-14.0, -1.0));
        assert_eq!(
            verdict(v[value::INTEGRATED], target),
            want,
            "{}",
            PRESET_NAMES[preset]
        );
        assert!(peak_passes(v[value::TRUE_PEAK], max_tp));
    }
    assert_eq!(verdict(v[value::INTEGRATED], -30.0), Verdict::TooLoud);
}

#[test]
fn the_published_indices_are_pinned() {
    assert_eq!(METERS, 62);
    // The bypass wrapper adds two values after the effect's own: they must fit the bank.
    const { assert!(METERS + 2 <= atomik_device_sdk::MAX_METER_VALUES) };
    assert_eq!(SCALARS + BINS, METERS);
    let order = [
        value::MOMENTARY,
        value::SHORT_TERM,
        value::INTEGRATED,
        value::LRA,
        value::TRUE_PEAK,
        value::TRUE_PEAK_L,
        value::TRUE_PEAK_R,
        value::SAMPLE_PEAK,
        value::PLR,
        value::PSR,
        value::DR,
        value::TARGET,
        value::MAX_TP,
        value::EPOCH,
        value::HISTORY_POINTS,
        value::TIME,
    ];
    assert_eq!(order, std::array::from_fn::<usize, SCALARS, _>(|i| i));
    assert_eq!(value::BINS, SCALARS);
    // A device at rest publishes its defaults: no data, the first preset's target.
    let v = values(&device());
    assert_eq!(v[value::MOMENTARY], -120.0);
    assert_eq!(v[value::INTEGRATED], -120.0);
    assert_eq!((v[value::TARGET], v[value::MAX_TP]), (-14.0, -1.0));
    // The wrapped device (what the registry builds) publishes the same ones in the same places.
    let mut r = atomik_device_sdk::Registry::new();
    r.register(factory()).unwrap();
    let mut w = DeviceHarness::new(r.create("atomik.loudness-meter").unwrap(), SR);
    feed(&mut w, 0, 12.0, SR, stereo(-23.0));
    let mut out = [0.0f32; atomik_device_sdk::MAX_METER_VALUES];
    let n = w.device().meter_values(&mut out);
    assert_eq!(n, METERS);
    assert!(near(out[value::INTEGRATED], -23.0, 0.1));
}

#[test]
fn it_is_deterministic() {
    let run_once = || {
        let mut h = device();
        let mut noise = [Rng::new(7), Rng::new(8)];
        feed(&mut h, 0, 12.0, SR, |c, _| noise[c].next_bipolar() * 0.3);
        values(&h)
    };
    assert_eq!(bits(&run_once()), bits(&run_once()));
}
