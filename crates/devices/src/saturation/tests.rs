//! Saturation tests, all through `DeviceHarness` (every `process` is allocation-guarded).
#![allow(clippy::float_cmp)]

use super::*;
use crate::test_support::{
    SR, assert_keys, harness, run_signal, sine, sweep_params_min_max, tone_amplitude,
};
use atomik_dsp::{PinkNoise, Rng};
use atomik_testkit::harness::DeviceHarness;

fn sat() -> DeviceHarness {
    harness(Box::new(Saturation::new()))
}

/// A device with the knob, type, oversampling and auto level set.
fn make(knob: f32, ty: f32, os: f32, auto: bool) -> DeviceHarness {
    let mut h = sat();
    h.set_param("saturation", knob);
    h.set_param("type", ty);
    h.set_param("oversampling", os);
    h.set_param("auto_level", if auto { 1.0 } else { 0.0 });
    h
}

fn secs(t: f64) -> usize {
    (t * SR).round() as usize
}

/// Runs `samples` of `signal` through both inputs; returns `(out_l, out_r)`.
fn run(
    h: &mut DeviceHarness,
    signal: impl Fn(usize) -> f32,
    samples: usize,
) -> (Vec<f32>, Vec<f32>) {
    let mut y = run_signal(
        h,
        &["in_l", "in_r"],
        signal,
        samples.div_ceil(BATCH),
        &["out_l", "out_r"],
    );
    let r = y.remove(1);
    (y.remove(0), r)
}

fn run_l(h: &mut DeviceHarness, signal: impl Fn(usize) -> f32, samples: usize) -> Vec<f32> {
    run(h, signal, samples).0
}

fn noise(amp: f32, len: usize, seed: u64) -> Vec<f32> {
    let mut r = Rng::new(seed);
    (0..len).map(|_| r.next_bipolar() * amp).collect()
}

/// Pink noise scaled to the given RMS (measured over the whole buffer).
fn pink(rms: f32, len: usize, seed: u64) -> Vec<f32> {
    let mut p = PinkNoise::new(seed);
    // Pink noise has a wandering baseline below any audio band: take it out with a 40 Hz
    // one-pole high-pass, as a music signal would have none.
    let r = (-std::f64::consts::TAU * 40.0 / SR).exp() as f32;
    let (mut x1, mut y1) = (0.0f32, 0.0f32);
    let x: Vec<f32> = (0..len)
        .map(|_| {
            let v = p.next_sample();
            y1 = v - x1 + r * y1;
            x1 = v;
            y1
        })
        .collect();
    // The last second is the one that is measured: give it the RMS (pink noise wanders).
    let k = rms / rms_of(&x[len - secs(1.0)..]);
    x.iter().map(|v| v * k).collect()
}

fn rms_of(y: &[f32]) -> f32 {
    (y.iter().map(|v| f64::from(*v) * f64::from(*v)).sum::<f64>() / y.len() as f64).sqrt() as f32
}

fn db(x: f32) -> f32 {
    20.0 * x.log10()
}

/// Amplitude of `which * hz` in the last whole second of a two-second render of a `hz` sine.
fn harmonic(h: &mut DeviceHarness, hz: f64, amp: f64, which: f64) -> f64 {
    let y = run_l(h, sine(hz, amp), secs(2.0));
    let start = secs(1.0);
    tone_amplitude(&y[start..start + secs(1.0)], hz * which, start)
}

fn meters(h: &DeviceHarness) -> [f32; METERS] {
    let mut out = [0.0f32; METERS];
    assert_eq!(h.device().meter_values(&mut out), METERS);
    out
}

#[test]
fn descriptor_validates() {
    assert_keys(
        &DESCRIPTOR,
        &[
            "saturation",
            "type",
            "auto_level",
            "mix",
            "oversampling",
            "output",
            "saturation_cv_trim",
            "bypass",
        ],
        &["in_l", "in_r", "out_l", "out_r", "saturation_cv_in"],
    );
    assert_eq!(DESCRIPTOR.type_id, "atomik.saturation");
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Effect);
    let p = |k: &str| DESCRIPTOR.params.iter().find(|p| p.key == k).unwrap();
    assert_eq!(p("saturation").default, 0.0);
    assert_eq!(p("type").default, 1.0);
    assert_eq!(p("auto_level").default, 1.0);
    assert_eq!(p("mix").default, 1.0);
    assert_eq!(p("oversampling").default, 1.0);
    assert_eq!(p("saturation_cv_trim").default, 0.0);
}

#[test]
fn param_sweep_stays_finite() {
    let mut h = sat();
    sweep_params_min_max(&mut h, &DESCRIPTOR);
}

#[test]
fn off_is_bit_exact_without_oversampling_in_neutral() {
    let x = noise(0.9, secs(0.3), 5);
    let mut h = make(0.0, 1.0, 0.0, true);
    let (l, r) = run(&mut h, |n| x[n], x.len());
    for (i, &v) in x.iter().enumerate() {
        assert_eq!(l[i].to_bits(), v.to_bits(), "L {i}");
        assert_eq!(r[i].to_bits(), v.to_bits(), "R {i}");
    }
}

#[test]
fn off_in_the_keep_types_is_the_crossover_all_pass() {
    // Keep low / keep high route the dry signal through the all-pass of the same crossover, so
    // OFF keeps the energy (flat magnitude) and the mix can never comb.
    let x = noise(0.5, secs(1.0), 5);
    for ty in [0.0, 2.0] {
        let mut h = make(0.0, ty, 0.0, true);
        let y = run_l(&mut h, |n| x[n], x.len());
        let (a, b) = (rms_of(&x[secs(0.2)..]), rms_of(&y[secs(0.2)..]));
        assert!(db(b / a).abs() < 0.05, "type {ty}: {} dB", db(b / a));
    }
}

#[test]
fn off_with_oversampling_is_the_input_delayed_by_the_latency() {
    let x = noise(0.9, secs(0.3), 6);
    for (os, lat) in [(1.0, 29usize), (2.0, 35)] {
        let mut h = make(0.0, 1.0, os, true);
        assert_eq!(h.device().latency_frames() as usize, lat);
        let l = run_l(&mut h, |n| x[n], x.len());
        for i in lat..x.len() {
            assert_eq!(l[i].to_bits(), x[i - lat].to_bits(), "os {os} sample {i}");
        }
    }
}

#[test]
fn harmonics_appear_and_grow_with_the_knob() {
    let ratio = |knob: f32, which: f64| {
        let mut h = make(knob, 1.0, 1.0, false);
        let y = run_l(&mut h, sine(1000.0, 0.3), secs(2.0));
        let start = secs(1.0);
        let w = &y[start..start + secs(1.0)];
        tone_amplitude(w, 1000.0 * which, start) / tone_amplitude(w, 1000.0, start)
    };
    let (mut last3, mut last2) = (0.0, 0.0);
    for knob in [0.0, 0.15, 0.4, 0.7, 1.0] {
        let (h3, h2) = (ratio(knob, 3.0), ratio(knob, 2.0));
        if knob == 0.0 {
            assert!(h3 < 1e-4 && h2 < 1e-4, "{h3} {h2}");
        } else {
            assert!(h3 > last3, "3rd at {knob}: {h3} vs {last3}");
            assert!(h2 > 0.005, "2nd (even) at {knob}: {h2}");
        }
        (last3, last2) = (h3, h2);
    }
    assert!(last3 > 0.1, "3rd at full knob {last3}");
    let _ = last2;
}

#[test]
fn the_curve_has_unit_slope_and_is_asymmetric() {
    for g in [0.01, 0.5, 5.0, 15.0] {
        let eps = 0.01 / g;
        let slope0 = (shape(eps, g) - shape(0.0, g)) / eps;
        assert!((slope0 - 1.0).abs() < 0.03, "g {g}: {slope0}");
        assert!(shape(0.0, g).abs() < 1e-6);
    }
    // Asymmetric: the positive and negative halves differ at high drive.
    assert!((shape(0.5, 10.0) + shape(-0.5, 10.0)).abs() > 1e-3);
    assert_eq!(drive_gain(0.0), 0.0);
    assert!((drive_gain(1.0) - G_MAX).abs() < 1e-3);
}

#[test]
fn auto_level_table_is_sane() {
    let t = level_table();
    for row in t {
        assert_eq!(row[0], 1.0);
        for w in row.windows(2) {
            assert!(w[1] >= w[0] - 1e-4, "{} then {}", w[0], w[1]);
        }
        assert!(row[TABLE] > 1.5 && row[TABLE] < 20.0, "{}", row[TABLE]);
    }
}

/// RMS of the steady state of a render (the last second).
fn steady_rms(y: &[f32]) -> f32 {
    rms_of(&y[y.len() - secs(1.0)..])
}

#[test]
fn auto_level_keeps_the_rms_across_the_knob_on_sine_and_noise() {
    let amp = 0.18 * std::f64::consts::SQRT_2;
    let p = pink(0.18, secs(3.0), 11);
    let ty = 1.0f32;
    let mut base_sine = 0.0;
    let mut base_noise = 0.0;
    for knob in [0.0f32, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0] {
        let mut h = make(knob, ty, 1.0, true);
        let ys = run_l(&mut h, sine(997.0, amp), secs(2.0));
        let mut h = make(knob, ty, 1.0, true);
        let yn = run_l(&mut h, |n| p[n], p.len());
        let (rs, rn) = (steady_rms(&ys), steady_rms(&yn));
        if knob == 0.0 {
            base_sine = rs;
            base_noise = rn;
        }
        assert!(
            db(rs / base_sine).abs() < 1.5,
            "knob {knob} sine {} dB",
            db(rs / base_sine)
        );
        assert!(
            db(rn / base_noise).abs() < 1.5,
            "knob {knob} noise {} dB",
            db(rn / base_noise)
        );
    }
}

#[test]
fn auto_level_keeps_the_rms_for_the_other_types_on_noise() {
    let p = pink(0.18, secs(3.0), 12);
    for ty in [0.0f32, 2.0] {
        let mut base = 0.0;
        for knob in [0.0f32, 0.25, 0.5, 0.75, 1.0] {
            let mut h = make(knob, ty, 1.0, true);
            let yn = run_l(&mut h, |n| p[n], p.len());
            let rn = steady_rms(&yn);
            if knob == 0.0 {
                base = rn;
            }
            assert!(
                db(rn / base).abs() < 1.5,
                "type {ty} knob {knob}: {} dB",
                db(rn / base)
            );
        }
    }
}

#[test]
fn auto_level_adds_gain_to_the_shaped_signal() {
    let amp = 0.18 * std::f64::consts::SQRT_2;
    let mut a = make(0.9, 1.0, 1.0, false);
    let mut b = make(0.9, 1.0, 1.0, true);
    let ya = run_l(&mut a, sine(997.0, amp), secs(2.0));
    let yb = run_l(&mut b, sine(997.0, amp), secs(2.0));
    assert!(steady_rms(&ya) < steady_rms(&yb), "auto level adds gain");
}

#[test]
fn mix_zero_is_the_dry_input_latency_aligned() {
    let x = noise(0.8, secs(0.3), 9);
    for (os, lat) in [(0.0, 0usize), (1.0, 29), (2.0, 35)] {
        // Neutral: the dry path is the plain input (the keep types use the all-pass).
        let mut h = make(1.0, 1.0, os, true);
        h.set_param("mix", 0.0);
        let l = run_l(&mut h, |n| x[n], x.len());
        // The mix smoother glides from its default: skip the first 100 ms.
        for i in secs(0.1)..x.len() {
            assert_eq!(l[i].to_bits(), x[i - lat].to_bits(), "os {os} sample {i}");
        }
    }
}

#[test]
fn half_mix_is_the_sum_of_dry_and_wet_halves() {
    let x = sine(1000.0, 0.4);
    let mut full = make(0.8, 1.0, 1.0, true);
    let mut half = make(0.8, 1.0, 1.0, true);
    half.set_param("mix", 0.5);
    let yf = run_l(&mut full, &x, secs(1.0));
    let yh = run_l(&mut half, &x, secs(1.0));
    let lat = 29;
    for i in secs(0.5)..secs(1.0) {
        let want = 0.5 * yf[i] + 0.5 * x(i - lat);
        assert!((yh[i] - want).abs() < 1e-5, "{i}: {} vs {want}", yh[i]);
    }
}

#[test]
fn keep_low_leaves_a_low_sine_clean_and_saturates_a_high_one() {
    let low_hz = 80.0;
    let high_hz = 3000.0;
    let third = |ty: f32, hz: f64| {
        let mut h = make(0.9, ty, 1.0, false);
        harmonic(&mut h, hz, 0.4, 3.0)
    };
    let (low_kl, low_n, low_kh) = (third(2.0, low_hz), third(1.0, low_hz), third(0.0, low_hz));
    assert!(low_n > 0.03, "neutral saturates the low sine: {low_n}");
    assert!(
        low_kl < low_n * 0.05,
        "keep low {low_kl} vs neutral {low_n}"
    );
    assert!(
        low_kh > low_n * 0.5,
        "keep high saturates the low sine: {low_kh}"
    );
    let (hi_kl, hi_n, hi_kh) = (
        third(2.0, high_hz),
        third(1.0, high_hz),
        third(0.0, high_hz),
    );
    assert!(hi_n > 0.03, "neutral saturates the high sine: {hi_n}");
    assert!(
        hi_kl > hi_n * 0.5,
        "keep low saturates the high sine: {hi_kl} vs {hi_n}"
    );
    assert!(hi_kh < hi_n * 0.05, "keep high {hi_kh} vs neutral {hi_n}");
}

#[test]
fn the_three_types_differ_on_a_chord() {
    let chord = |n: usize| sine(110.0, 0.2)(n) + sine(2500.0, 0.2)(n);
    let mut outs = Vec::new();
    for ty in [0.0, 1.0, 2.0] {
        let mut h = make(0.7, ty, 1.0, true);
        outs.push(run_l(&mut h, chord, secs(1.0)));
    }
    for (a, b) in [(0, 1), (1, 2), (0, 2)] {
        let diff: f32 = outs[a]
            .iter()
            .zip(&outs[b])
            .skip(secs(0.5))
            .map(|(x, y)| (x - y).abs())
            .sum();
        assert!(diff > 10.0, "types {a} and {b} differ by {diff}");
    }
}

#[test]
fn a_type_switch_does_not_click() {
    for (from, to) in [(1.0, 2.0), (2.0, 0.0), (0.0, 1.0)] {
        let mut h = make(0.8, from, 1.0, true);
        let x = sine(220.0, 0.3);
        let mut y = run_l(&mut h, &x, secs(0.5));
        h.set_param("type", to);
        let b = run_signal(
            &mut h,
            &["in_l", "in_r"],
            |n| x(n + secs(0.5)),
            secs(0.5).div_ceil(BATCH),
            &["out_l"],
        );
        y.extend_from_slice(&b[0]);
        let step = |s: &[f32]| {
            s.windows(2)
                .map(|w| (w[1] - w[0]).abs())
                .fold(0.0f32, f32::max)
        };
        let steady = step(&y[secs(0.3)..secs(0.5) - 1]);
        let across = step(&y[secs(0.5) - 4..secs(0.5) + secs(0.1)]);
        assert!(
            across < steady * 2.5 + 0.01,
            "{from}->{to}: {across} vs {steady}"
        );
    }
}

#[test]
fn a_knob_step_does_not_click() {
    let mut h = make(0.0, 1.0, 1.0, true);
    let x = sine(220.0, 0.3);
    let mut y = run_l(&mut h, &x, secs(0.3));
    h.set_param("saturation", 1.0);
    let b = run_signal(
        &mut h,
        &["in_l", "in_r"],
        |n| x(n + secs(0.3)),
        secs(0.3).div_ceil(BATCH),
        &["out_l"],
    );
    y.extend_from_slice(&b[0]);
    let step = |s: &[f32]| {
        s.windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max)
    };
    let steady = step(&y[secs(0.2)..secs(0.3) - 1]);
    let across = step(&y[secs(0.3)..secs(0.4)]);
    assert!(across < steady * 3.0 + 0.02, "{across} vs {steady}");
}

#[test]
fn oversampling_reports_its_latency_and_cuts_aliasing() {
    let mut lat = Vec::new();
    for os in [0.0, 1.0, 2.0] {
        lat.push(make(1.0, 1.0, os, false).device().latency_frames());
    }
    assert_eq!(lat, [0, 29, 35]);
    assert!(sat().device().variable_latency());
    // 10 kHz: the third harmonic (30 kHz) folds to 18 kHz at 48 kHz.
    let alias = |os: f32| {
        let mut h = make(1.0, 1.0, os, false);
        harmonic(&mut h, 10_000.0, 0.5, 1.8)
    };
    let (off, x2, x4) = (alias(0.0), alias(1.0), alias(2.0));
    assert!(off > 0.01, "alias without oversampling {off}");
    assert!(x4 < off * 0.1, "4x {x4} vs off {off}");
    assert!(
        x4 <= x2 * 1.5 && x2 < off * 0.3,
        "2x {x2}, 4x {x4}, off {off}"
    );
}

/// The 3 kHz amplitude of a 1 kHz sine through a device whose knob is `knob`, with a constant
/// `cv` on the CV input and `trim` set.
fn third_with_cv(trim: f32, cv: f32, knob: f32) -> f64 {
    let x = sine(1000.0, 0.3);
    let mut h = make(knob, 1.0, 1.0, false);
    h.set_param("saturation_cv_trim", trim);
    h.set_connected("saturation_cv_in", true);
    let mut y = Vec::new();
    for b in 0..secs(2.0).div_ceil(BATCH) {
        let mut s = [0.0f32; BATCH];
        for (i, v) in s.iter_mut().enumerate() {
            *v = x(b * BATCH + i);
        }
        *h.input_mut("in_l") = s;
        *h.input_mut("in_r") = s;
        *h.input_mut("saturation_cv_in") = [cv; BATCH];
        h.process();
        y.extend_from_slice(h.output("out_l"));
    }
    let start = secs(1.0);
    tone_amplitude(&y[start..start + secs(1.0)], 3000.0, start)
}

#[test]
fn cv_and_trim_move_the_knob() {
    let mut plain = make(0.5, 1.0, 1.0, false);
    let y = run_l(&mut plain, sine(1000.0, 0.3), secs(2.0));
    let start = secs(1.0);
    let reference = tone_amplitude(&y[start..start + secs(1.0)], 3000.0, start);
    // cv 0.5 at full trim on a knob at 0 is the same as a knob at 0.5.
    let a = third_with_cv(1.0, 0.5, 0.0);
    assert!(
        (a - reference).abs() < reference * 0.01,
        "{a} vs {reference}"
    );
    // Trim 0 ignores the cv.
    let b = third_with_cv(0.0, 1.0, 0.0);
    assert!(b < 1e-4, "{b}");
    // A negative trim pulls a raised knob down, and the sum clamps at 0.
    let c = third_with_cv(-1.0, 0.5, 0.5);
    assert!(c < 1e-4, "{c}");
    let d = third_with_cv(-0.5, 1.0, 1.0);
    assert!(d > 1e-3 && d < third_with_cv(0.0, 0.0, 1.0));
    // The cv clamps at +1 and the sum at 1.
    let e = third_with_cv(1.0, 5.0, 0.5);
    let f = third_with_cv(0.0, 0.0, 1.0);
    assert!((e - f).abs() < f * 0.01, "{e} vs {f}");
}

#[test]
fn silence_huge_and_non_finite_input_stay_finite() {
    for os in [0.0, 1.0, 2.0] {
        for ty in [0.0, 1.0, 2.0] {
            let mut h = make(1.0, ty, os, true);
            let (l, _) = run(&mut h, |_| 0.0, secs(0.2));
            assert!(l.iter().all(|v| *v == 0.0), "silence in, silence out");
            let (l, r) = run(
                &mut h,
                |n| if n % 3 == 0 { 1.0e6 } else { -1.0e6 },
                secs(0.2),
            );
            assert!(l.iter().chain(&r).all(|v| v.is_finite()));
            let (l, r) = run(
                &mut h,
                |n| match n % 4 {
                    0 => f32::NAN,
                    1 => f32::INFINITY,
                    2 => f32::NEG_INFINITY,
                    _ => 0.3,
                },
                secs(0.2),
            );
            assert!(l.iter().chain(&r).all(|v| v.is_finite()));
            let (l, _) = run(&mut h, |_| 0.0, secs(0.5));
            assert!(l.iter().all(|v| v.is_finite()));
            assert!(l[l.len() - 1].abs() < 1e-3, "settles to silence");
        }
    }
}

#[test]
fn a_sample_rate_change_keeps_working() {
    let mut dev = Saturation::new();
    dev.set_param(ParamId(P_SATURATION), 0.6);
    for sr in [44_100.0, 96_000.0, 48_000.0] {
        let mut h = DeviceHarness::new(Box::new(dev.clone()), sr);
        assert_eq!(h.device().latency_frames(), 29);
        let n = (sr * 0.5) as usize;
        let y = run_signal(
            &mut h,
            &["in_l", "in_r"],
            |i| (0.3 * (std::f64::consts::TAU * 1000.0 * i as f64 / sr).sin()) as f32,
            n.div_ceil(BATCH),
            &["out_l"],
        );
        assert!(y[0].iter().all(|v| v.is_finite()));
        assert!(rms_of(&y[0][n / 2..]) > 0.1);
        // Prepare again on the same instance at another rate, with state already running.
        dev.prepare(&PrepareInfo {
            sample_rate: atomik_core::SampleRate::new(sr).unwrap(),
            seed: 0,
        });
    }
}

#[test]
fn the_drive_lamp_follows_how_hard_the_curve_works() {
    let lamp_at = |knob: f32, amp: f64| {
        let mut h = make(knob, 1.0, 1.0, true);
        run_l(&mut h, sine(1000.0, amp), secs(0.5));
        meters(&h)[meter::DRIVE]
    };
    assert_eq!(lamp_at(0.0, 0.5), 0.0, "off");
    let (a, b, c) = (lamp_at(0.2, 0.5), lamp_at(0.5, 0.5), lamp_at(1.0, 0.5));
    assert!(a < b && b < c && c > 0.5, "{a} {b} {c}");
    assert!(
        lamp_at(1.0, 0.002) < c * 0.3,
        "a quiet signal barely saturates"
    );
    // Silence lets it fall back to 0.
    let mut h = make(1.0, 1.0, 1.0, true);
    run_l(&mut h, sine(1000.0, 0.5), secs(0.3));
    run_l(&mut h, |_| 0.0, secs(1.5));
    assert_eq!(meters(&h)[meter::DRIVE], 0.0);
    assert!(meters(&h).iter().all(|v| v.is_finite()));
}

#[test]
fn input_and_output_peaks_are_reported() {
    let mut h = make(0.0, 1.0, 0.0, true);
    run_l(&mut h, sine(1000.0, 0.5), secs(0.2));
    let m = meters(&h);
    assert!((m[meter::INPUT] - 0.5).abs() < 0.01, "{m:?}");
    assert!((m[meter::OUTPUT] - 0.5).abs() < 0.01, "{m:?}");
}

#[test]
fn output_gain_scales_the_result() {
    let mut a = make(0.0, 1.0, 0.0, true);
    let mut b = make(0.0, 1.0, 0.0, true);
    b.set_param("output", -6.0);
    let x = sine(500.0, 0.5);
    let ya = run_l(&mut a, &x, secs(0.5));
    let yb = run_l(&mut b, &x, secs(0.5));
    let r = rms_of(&yb[secs(0.3)..]) / rms_of(&ya[secs(0.3)..]);
    assert!((db(r) + 6.0).abs() < 0.05, "{}", db(r));
}

#[test]
fn channels_are_independent() {
    let mut h = make(1.0, 1.0, 1.0, true);
    let mut y = run_signal(
        &mut h,
        &["in_l"],
        sine(1000.0, 0.5),
        secs(0.3).div_ceil(BATCH),
        &["out_l", "out_r"],
    );
    let r = y.remove(1);
    let l = y.remove(0);
    assert!(rms_of(&l[secs(0.2)..]) > 0.1);
    assert!(r.iter().all(|v| *v == 0.0), "a silent right stays silent");
}

/// The expected linear response of keep low (`ty` 2) / keep high (`ty` 0) at full knob: the clean
/// band plus the level gain times the saturated band, from the same crossover. Returns the
/// wet and the all-pass dry signals.
fn keep_reference(ty: usize, hz: f64, amp: f64, len: usize) -> (Vec<f32>, Vec<f32>) {
    let mut split = Crossover::new(SR as f32);
    split.set_freq(SPLIT_HZ);
    let level = level_gain(level_table(), ty, 1.0);
    let x = sine(hz, amp);
    let (mut wet, mut dry) = (Vec::new(), Vec::new());
    for n in 0..len {
        let (low, high) = split.process(x(n));
        let (clean, sat) = if ty == 2 { (low, high) } else { (high, low) };
        wet.push(clean + level * sat);
        dry.push(low + high);
    }
    (wet, dry)
}

#[test]
fn keep_types_have_no_notch_at_the_crossover() {
    // -40 dBFS: the curve is nearly linear, so the response is the linear one.
    let amp = 0.01;
    for ty in [0.0f32, 2.0] {
        for hz in [
            50.0, 100.0, 200.0, 300.0, 400.0, 500.0, 800.0, 1500.0, 3000.0, 5000.0,
        ] {
            let mut h = make(1.0, ty, 0.0, true);
            let y = run_l(&mut h, sine(hz, amp), secs(1.5));
            let start = secs(1.0);
            let got = tone_amplitude(&y[start..], hz, start);
            let (want_wet, _) = keep_reference(ty as usize, hz, amp, secs(1.5));
            let want = tone_amplitude(&want_wet[start..], hz, start);
            assert!(
                db((got / want) as f32).abs() < 1.0,
                "type {ty} {hz} Hz: {got} vs {want}"
            );
            // Never a notch: at least the lower of the two band gains.
            let floor = amp * f64::from(level_gain(level_table(), ty as usize, 1.0)).min(1.0);
            assert!(got > 0.7 * floor, "type {ty} {hz} Hz notch: {got}");
        }
    }
}

#[test]
fn keep_types_at_half_mix_do_not_comb() {
    let amp = 0.01;
    for ty in [0.0f32, 2.0] {
        for hz in [100.0, 300.0, 400.0, 500.0, 1000.0, 3000.0] {
            let mut h = make(1.0, ty, 0.0, true);
            h.set_param("mix", 0.5);
            let y = run_l(&mut h, sine(hz, amp), secs(1.5));
            let start = secs(1.0);
            let got = tone_amplitude(&y[start..], hz, start);
            let (wet, dry) = keep_reference(ty as usize, hz, amp, secs(1.5));
            let mixed: Vec<f32> = wet
                .iter()
                .zip(&dry)
                .map(|(w, d)| 0.5 * w + 0.5 * d)
                .collect();
            let want = tone_amplitude(&mixed[start..], hz, start);
            assert!(
                db((got / want) as f32).abs() < 1.0,
                "type {ty} {hz} Hz: {got} vs {want}"
            );
        }
    }
}

#[test]
fn switching_the_oversampling_live_does_not_drop_out_or_step() {
    let x = sine(200.0, 0.3);
    let seg = secs(0.2);
    let mut h = make(0.5, 1.0, 0.0, true);
    let mut y = run_l(&mut h, &x, seg);
    let before = meters(&h)[meter::DRIVE];
    let mut lamp_after = Vec::new();
    for (i, os) in [1.0, 2.0, 0.0].into_iter().enumerate() {
        h.set_param("oversampling", os);
        let b = run_signal(
            &mut h,
            &["in_l", "in_r"],
            |n| x(n + (i + 1) * seg),
            seg.div_ceil(BATCH),
            &["out_l"],
        );
        lamp_after.push(meters(&h)[meter::DRIVE]);
        y.extend_from_slice(&b[0]);
    }
    let step = |s: &[f32]| {
        s.windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max)
    };
    let steady = step(&y[seg - secs(0.05)..seg - 1]);
    let steady_rms = rms_of(&y[seg - secs(0.05)..seg]);
    for k in 1..=3 {
        let at = k * seg;
        let across = step(&y[at - 4..at + secs(0.01)]);
        assert!(
            across < steady * 2.5 + 0.01,
            "switch {k}: step {across} vs {steady}"
        );
        // No dropout: every 4 ms window after the switch keeps most of the level.
        let win = secs(0.004);
        for w in (at..at + secs(0.02)).step_by(win) {
            let r = rms_of(&y[w..w + win]);
            assert!(
                r > 0.75 * steady_rms,
                "switch {k} window {w}: {r} vs {steady_rms}"
            );
        }
    }
    // The lamp does not jump to 1 on a switch.
    for l in lamp_after {
        assert!((l - before).abs() < 0.2 && l < 0.9, "lamp {l} vs {before}");
    }
}

#[test]
fn a_quiet_tail_passes_at_a_low_knob() {
    // -90 dBFS: the curve must be a straight line there, with no rounding noise from `tanh`.
    for knob in [0.005f32, 0.0003] {
        let mut h = make(knob, 1.0, 0.0, false);
        let x = sine(1000.0, 3.16e-5);
        let y = run_l(&mut h, &x, secs(0.5));
        let mut worst = 0.0f32;
        for (n, &v) in y.iter().enumerate().skip(secs(0.1)) {
            worst = worst.max((v - x(n)).abs());
        }
        // Better than -140 dBFS absolute (-50 dB relative to the tail).
        assert!(worst < 1.0e-7, "knob {knob}: error {worst}");
        let xs: Vec<f32> = (secs(0.2)..secs(0.5)).map(&x).collect();
        let ratio = rms_of(&y[secs(0.2)..]) / rms_of(&xs);
        assert!(
            (ratio - 1.0).abs() < 1e-3,
            "knob {knob}: muted or boosted, {ratio}"
        );
    }
}

#[test]
fn off_with_a_partial_mix_is_still_bit_exact() {
    let x = noise(0.7, secs(0.3), 21);
    for (os, lat) in [(0.0, 0usize), (1.0, 29)] {
        let mut h = make(0.0, 1.0, os, true);
        h.set_param("mix", 0.3);
        let l = run_l(&mut h, |n| x[n], x.len());
        for i in lat..x.len() {
            assert_eq!(l[i].to_bits(), x[i - lat].to_bits(), "os {os} sample {i}");
        }
    }
}

#[test]
fn settling_into_off_does_not_step() {
    let x = sine(220.0, 0.3);
    let mut h = make(0.8, 1.0, 0.0, true);
    let mut y = run_l(&mut h, &x, secs(0.3));
    h.set_param("saturation", 0.0);
    let b = run_signal(
        &mut h,
        &["in_l", "in_r"],
        |n| x(n + secs(0.3)),
        secs(0.7).div_ceil(BATCH),
        &["out_l"],
    );
    y.extend_from_slice(&b[0]);
    // The error against the dry input, from the knob going down on.
    let d: Vec<f32> = y.iter().enumerate().map(|(n, v)| v - x(n)).collect();
    let tail = secs(0.3);
    let snap = (tail..d.len())
        .rev()
        .find(|&n| d[n] != 0.0)
        .map_or(tail, |n| n + 1);
    assert!(snap < d.len() - BATCH, "never settled on the dry path");
    let worst = (snap.saturating_sub(2 * BATCH)..snap + 2)
        .map(|n| (d[n] - d[n - 1]).abs())
        .fold(0.0f32, f32::max);
    assert!(
        worst < 1.0e-6,
        "step at the snap: {worst} (-120 dB is 1e-6)"
    );
}
