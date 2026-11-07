//! Dynamic EQ tests, all through `DeviceHarness` (every `process` is allocation-guarded).
#![allow(clippy::float_cmp)]

use super::*;
use crate::test_support::{SR, assert_keys, sine, sweep_params_min_max, tone_amplitude};
use atomik_dsp::Rng;
use atomik_testkit::harness::DeviceHarness;

fn eq() -> DeviceHarness {
    DeviceHarness::new(Box::new(DynamicEq::new()), SR)
}

fn set(h: &mut DeviceHarness, band: usize, key: &str, v: f32) {
    h.set_param(&format!("band{band}.{key}"), v);
}

fn ms(t: f64) -> usize {
    (t * 0.001 * SR).round() as usize
}

fn db(x: f64) -> f64 {
    20.0 * x.max(1e-12).log10()
}

/// Runs for `samples` with `l(n)`/`r(n)` on the inputs and, when given, `key(n)` on both key
/// sockets; returns `(out_l, out_r)`.
fn run_lr(
    h: &mut DeviceHarness,
    l: impl Fn(usize) -> f32,
    r: impl Fn(usize) -> f32,
    key: Option<&dyn Fn(usize) -> f32>,
    samples: usize,
) -> (Vec<f32>, Vec<f32>) {
    let (mut yl, mut yr) = (Vec::new(), Vec::new());
    for b in 0..samples.div_ceil(BATCH) {
        let (mut xl, mut xr, mut xk) = ([0.0f32; BATCH], [0.0f32; BATCH], [0.0f32; BATCH]);
        for i in 0..BATCH {
            let n = b * BATCH + i;
            xl[i] = l(n);
            xr[i] = r(n);
            if let Some(k) = key {
                xk[i] = k(n);
            }
        }
        *h.input_mut("in_l") = xl;
        *h.input_mut("in_r") = xr;
        if key.is_some() {
            *h.input_mut("sc_in_l") = xk;
            *h.input_mut("sc_in_r") = xk;
        }
        h.process();
        yl.extend_from_slice(h.output("out_l"));
        yr.extend_from_slice(h.output("out_r"));
    }
    (yl, yr)
}

/// The level in dB of the `hz` component over the last 100 ms (whole cycles at integer kHz).
fn tail_db(y: &[f32], hz: f64) -> f64 {
    let n = ms(100.0);
    let start = y.len() - n;
    db(tone_amplitude(&y[start..], hz, start))
}

/// The gain in dB of a steady sine `hz` at `amp` after `settle` ms, through the left channel.
fn gain_of(h: &mut DeviceHarness, hz: f64, amp: f64, settle: f64) -> f64 {
    let t = sine(hz, amp);
    let (y, _) = run_lr(h, &t, &t, None, ms(settle) + ms(100.0));
    tail_db(&y, hz) - db(amp)
}

fn noise(seed: u64, amp: f32) -> impl Fn(usize) -> f32 {
    let samples: Vec<f32> = {
        let mut r = Rng::new(seed);
        (0..ms(600.0)).map(|_| r.next_bipolar() * amp).collect()
    };
    move |n| samples[n % samples.len()]
}

/// Band 2 as a plain 1 kHz bell with the given dynamics and quick, equal ballistics.
fn bell(range: f32, threshold: f32) -> DeviceHarness {
    let mut h = eq();
    set(&mut h, 2, "on", 1.0);
    set(&mut h, 2, "freq", 1_000.0);
    set(&mut h, 2, "range", range);
    set(&mut h, 2, "threshold", threshold);
    set(&mut h, 2, "attack", 20.0);
    set(&mut h, 2, "release", 20.0);
    h
}

#[test]
fn descriptor_validates() {
    let mut keys: Vec<String> = Vec::new();
    for b in 1..=BANDS {
        for k in [
            "on",
            "freq",
            "q",
            "gain",
            "threshold",
            "range",
            "ratio",
            "attack",
            "release",
            "source",
            "channel",
        ] {
            keys.push(format!("band{b}.{k}"));
        }
    }
    for k in ["output", "delta", "bypass", "internal.analyser_live"] {
        keys.push(k.to_string());
    }
    let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
    assert_keys(
        &DESCRIPTOR,
        &keys,
        &["in_l", "in_r", "sc_in_l", "sc_in_r", "out_l", "out_r"],
    );
    assert_eq!(DESCRIPTOR.type_id, "atomik.dynamic-eq");
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Effect);
    assert_eq!(DESCRIPTOR.params.len(), BANDS * PER_BAND + 4);
}

#[test]
fn with_every_band_off_the_input_passes_bit_for_bit() {
    let mut h = eq();
    let (nl, nr) = (noise(1, 0.7), noise(2, 0.7));
    let (yl, yr) = run_lr(&mut h, &nl, &nr, None, ms(200.0));
    for n in 0..yl.len() {
        assert_eq!(yl[n].to_bits(), nl(n).to_bits(), "L {n}");
        assert_eq!(yr[n].to_bits(), nr(n).to_bits(), "R {n}");
    }
}

#[test]
fn a_band_that_is_on_with_no_gain_and_no_range_is_skipped() {
    let mut h = eq();
    set(&mut h, 3, "on", 1.0);
    set(&mut h, 3, "range", 0.0);
    set(&mut h, 3, "gain", 0.0);
    let (nl, nr) = (noise(3, 0.5), noise(4, 0.5));
    let (yl, yr) = run_lr(&mut h, &nl, &nr, None, ms(100.0));
    for n in 0..yl.len() {
        assert_eq!(yl[n].to_bits(), nl(n).to_bits(), "L {n}");
        assert_eq!(yr[n].to_bits(), nr(n).to_bits(), "R {n}");
    }
}

#[test]
fn a_band_switched_off_fades_out_and_the_input_returns_bit_for_bit() {
    let mut h = eq();
    set(&mut h, 2, "on", 1.0);
    set(&mut h, 2, "gain", 9.0);
    let (nl, nr) = (noise(5, 0.3), noise(6, 0.3));
    let (loud, _) = run_lr(&mut h, &nl, &nr, None, ms(150.0));
    assert!((0..loud.len()).any(|n| loud[n].to_bits() != nl(n).to_bits()));
    set(&mut h, 2, "on", 0.0);
    let (y, _) = run_lr(&mut h, &nl, &nr, None, ms(250.0));
    for (n, v) in y.iter().enumerate().skip(y.len() - ms(20.0)) {
        assert_eq!(v.to_bits(), nl(n).to_bits(), "sample {n}");
    }
}

#[test]
fn the_static_bell_and_the_shelves_follow_their_gain() {
    // Range 0: a plain static band.
    let mut h = bell(0.0, -60.0);
    set(&mut h, 2, "gain", 6.0);
    let g = gain_of(&mut h, 1_000.0, 0.1, 300.0);
    assert!((g - 6.0).abs() < 0.2, "{g} dB at the centre");
    let off = gain_of(&mut h, 100.0, 0.1, 100.0);
    assert!(off.abs() < 0.6, "{off} dB two octaves and more away");
    let mut h = eq();
    set(&mut h, 5, "on", 1.0);
    set(&mut h, 5, "range", 0.0);
    set(&mut h, 5, "gain", 6.0);
    let hi = gain_of(&mut h, 16_000.0, 0.1, 300.0);
    assert!((hi - 6.0).abs() < 0.6, "{hi} dB in the high shelf");
    let lo = gain_of(&mut h, 200.0, 0.1, 100.0);
    assert!(lo.abs() < 0.3, "{lo} dB below it");
    let mut h = eq();
    set(&mut h, 1, "on", 1.0);
    set(&mut h, 1, "range", 0.0);
    set(&mut h, 1, "gain", -6.0);
    let lo = gain_of(&mut h, 40.0, 0.1, 400.0);
    assert!((lo + 6.0).abs() < 0.6, "{lo} dB in the low shelf");
}

#[test]
fn a_loud_band_is_cut_by_the_range_and_a_quiet_one_is_left_alone() {
    // 0.5 is 24 dB over a -30 dB threshold, far past the 9 dB range.
    let mut h = bell(-9.0, -30.0);
    set(&mut h, 2, "ratio", 20.0);
    let loud = gain_of(&mut h, 1_000.0, 0.5, 600.0);
    assert!((loud + 9.0).abs() < 0.5, "{loud} dB");
    let mut h = bell(-9.0, -30.0);
    let quiet = gain_of(&mut h, 1_000.0, 0.005, 400.0);
    assert!(quiet.abs() < 0.1, "{quiet} dB below the threshold");
}

#[test]
fn the_cut_grows_with_the_level_up_to_the_range() {
    let cut = |amp: f64| {
        let mut h = bell(-18.0, -30.0);
        -gain_of(&mut h, 1_000.0, amp, 600.0)
    };
    let (a, b, c) = (cut(0.04), cut(0.1), cut(0.3));
    assert!(a < b && b < c, "{a} {b} {c}");
    assert!(a > 0.5 && c < 18.1, "{a} {c}");
}

#[test]
fn a_positive_range_lifts_the_band_when_it_is_loud() {
    let mut h = bell(6.0, -40.0);
    set(&mut h, 2, "ratio", 20.0);
    let loud = gain_of(&mut h, 1_000.0, 0.1, 600.0);
    assert!((loud - 6.0).abs() < 0.5, "{loud} dB");
    let mut h = bell(6.0, -40.0);
    let quiet = gain_of(&mut h, 1_000.0, 0.001, 400.0);
    assert!(quiet.abs() < 0.1, "{quiet} dB");
}

#[test]
fn the_dynamic_gain_adds_to_the_static_gain() {
    let mut h = bell(-6.0, -40.0);
    set(&mut h, 2, "ratio", 20.0);
    set(&mut h, 2, "gain", 4.0);
    let g = gain_of(&mut h, 1_000.0, 0.3, 600.0);
    assert!((g + 2.0).abs() < 0.5, "{g} dB (+4 static, -6 dynamic)");
}

#[test]
fn only_the_listened_to_band_triggers_the_band() {
    let mut h = bell(-12.0, -30.0);
    set(&mut h, 2, "ratio", 20.0);
    let (lo, hi) = (sine(200.0, 0.3), sine(1_000.0, 0.5));
    let t = move |n: usize| lo(n) + hi(n);
    let (y, _) = run_lr(&mut h, &t, &t, None, ms(800.0));
    // The 200 Hz tone only meets the skirt of the cut bell (Q 1), never the detector.
    let skirt =
        BiquadCoeffs::design(BiquadShape::Peaking, 1_000.0, 1.0, -12.0, SR).response_db(200.0, SR);
    let low = tail_db(&y, 200.0) - db(0.3);
    assert!(
        (low - skirt).abs() < 0.15,
        "the 200 Hz tone moved {low} dB, the skirt is {skirt}"
    );
    let high = tail_db(&y, 1_000.0) - db(0.5);
    assert!((high + 12.0).abs() < 0.7, "{high} dB");
    // The low tone alone does not trip the band.
    let mut h = bell(-12.0, -30.0);
    let t = sine(200.0, 0.05);
    let (y, _) = run_lr(&mut h, &t, &t, None, ms(500.0));
    assert!((tail_db(&y, 200.0) - db(0.05)).abs() < 0.1, "untouched");
}

#[test]
fn the_stereo_detector_follows_the_louder_channel() {
    let mut h = bell(-9.0, -30.0);
    set(&mut h, 2, "ratio", 20.0);
    let (loud, quiet) = (sine(1_000.0, 0.5), sine(1_000.0, 0.005));
    let (_, yr) = run_lr(&mut h, &loud, &quiet, None, ms(800.0));
    let cut = tail_db(&yr, 1_000.0) - db(0.005);
    assert!((cut + 9.0).abs() < 0.6, "the quiet side moved {cut} dB");
}

#[test]
fn the_ballistics_set_how_fast_the_gain_comes_and_goes() {
    let gain = |h: &DeviceHarness| {
        let mut m = [0.0f32; METER_COUNT];
        assert_eq!(h.device().meter_values(&mut m), METER_COUNT);
        m[METER_GAINS + 1]
    };
    let tone = sine(1_000.0, 0.5);
    let after_10_ms = |attack: f32| {
        let mut h = bell(-9.0, -30.0);
        set(&mut h, 2, "ratio", 20.0);
        set(&mut h, 2, "attack", attack);
        run_lr(&mut h, &tone, &tone, None, ms(10.0));
        gain(&h)
    };
    let (slow, fast) = (after_10_ms(80.0), after_10_ms(1.0));
    assert!(slow > -4.5 && fast < slow - 1.5, "slow {slow}, fast {fast}");
    let mut h = bell(-9.0, -30.0);
    set(&mut h, 2, "ratio", 20.0);
    set(&mut h, 2, "attack", 20.0);
    set(&mut h, 2, "release", 300.0);
    run_lr(&mut h, &tone, &tone, None, ms(400.0));
    assert!(gain(&h) < -8.5, "reached: {}", gain(&h));
    run_lr(&mut h, |_| 0.0, |_| 0.0, None, ms(50.0));
    assert!(
        gain(&h) < -5.0,
        "still held 50 ms after the stop: {}",
        gain(&h)
    );
    run_lr(&mut h, |_| 0.0, |_| 0.0, None, ms(2_500.0));
    assert!(gain(&h) > -0.5, "released: {}", gain(&h));
}

#[test]
fn the_key_input_drives_a_band_set_to_key() {
    let main = sine(1_000.0, 0.01);
    let key_loud = sine(1_000.0, 0.5);
    let key_off_band = sine(8_000.0, 0.1);
    let cut_with = |key: Option<&dyn Fn(usize) -> f32>| {
        let mut h = bell(-9.0, -30.0);
        set(&mut h, 2, "ratio", 20.0);
        set(&mut h, 2, "source", 1.0);
        let (y, _) = run_lr(&mut h, &main, &main, key, ms(800.0));
        tail_db(&y, 1_000.0) - db(0.01)
    };
    // The main tone is under the threshold: only the key can trip the band.
    assert!(cut_with(Some(&key_loud)) < -8.5);
    assert!(
        cut_with(Some(&key_off_band)).abs() < 0.2,
        "outside the band"
    );
    assert!(cut_with(Some(&|_| 0.0)).abs() < 0.1, "a silent key");
    // No cable: the band listens to the input again, which is under the threshold.
    assert!(cut_with(None).abs() < 0.1);
}

#[test]
fn a_band_set_to_key_without_a_cable_listens_to_its_input() {
    let mut h = bell(-9.0, -30.0);
    set(&mut h, 2, "ratio", 20.0);
    set(&mut h, 2, "source", 1.0);
    let t = sine(1_000.0, 0.5);
    let (y, _) = run_lr(&mut h, &t, &t, None, ms(800.0));
    assert!(tail_db(&y, 1_000.0) - db(0.5) < -8.5);
}

#[test]
fn a_lone_key_side_feeds_both_channels() {
    let main = sine(1_000.0, 0.01);
    let key = sine(1_000.0, 0.5);
    for side in ["sc_in_l", "sc_in_r"] {
        let mut h = bell(-9.0, -30.0);
        set(&mut h, 2, "ratio", 20.0);
        set(&mut h, 2, "source", 1.0);
        let mut yl = Vec::new();
        let mut yr = Vec::new();
        for b in 0..ms(800.0).div_ceil(BATCH) {
            let (mut x, mut k) = ([0.0f32; BATCH], [0.0f32; BATCH]);
            for i in 0..BATCH {
                x[i] = main(b * BATCH + i);
                k[i] = key(b * BATCH + i);
            }
            *h.input_mut("in_l") = x;
            *h.input_mut("in_r") = x;
            *h.input_mut(side) = k;
            h.process();
            yl.extend_from_slice(h.output("out_l"));
            yr.extend_from_slice(h.output("out_r"));
        }
        assert!(tail_db(&yl, 1_000.0) - db(0.01) < -8.5, "{side} L");
        assert!(tail_db(&yr, 1_000.0) - db(0.01) < -8.5, "{side} R");
    }
}

#[test]
fn mid_and_side_bands_move_only_their_part_of_the_image() {
    let tone = sine(1_000.0, 0.1);
    let neg = |n: usize| -tone(n);
    let gain_in = |channel: f32, l: &dyn Fn(usize) -> f32, r: &dyn Fn(usize) -> f32| {
        let mut h = bell(0.0, -60.0);
        set(&mut h, 2, "gain", 6.0);
        set(&mut h, 2, "channel", channel);
        let (yl, _) = run_lr(&mut h, l, r, None, ms(500.0));
        tail_db(&yl, 1_000.0) - db(0.1)
    };
    // A centred tone (L = R) is all mid.
    assert!((gain_in(1.0, &tone, &tone) - 6.0).abs() < 0.3);
    assert!(gain_in(2.0, &tone, &tone).abs() < 0.05);
    // An out-of-phase tone (L = -R) is all side.
    assert!((gain_in(2.0, &tone, &neg) - 6.0).abs() < 0.3);
    assert!(gain_in(1.0, &tone, &neg).abs() < 0.05);
    // Stereo moves both.
    assert!((gain_in(0.0, &tone, &neg) - 6.0).abs() < 0.3);
}

#[test]
fn a_side_band_listens_to_the_side_signal() {
    let tone = sine(1_000.0, 0.5);
    let neg = |n: usize| -tone(n);
    let cut = |channel: f32, l: &dyn Fn(usize) -> f32, r: &dyn Fn(usize) -> f32| {
        let mut h = bell(-9.0, -30.0);
        set(&mut h, 2, "ratio", 20.0);
        set(&mut h, 2, "channel", channel);
        let (yl, _) = run_lr(&mut h, l, r, None, ms(800.0));
        tail_db(&yl, 1_000.0) - db(0.5)
    };
    assert!(cut(2.0, &tone, &neg) < -8.5, "side tone, side band");
    assert!(
        cut(2.0, &tone, &tone).abs() < 0.1,
        "centred tone, side band"
    );
    assert!(cut(1.0, &tone, &tone) < -8.5, "centred tone, mid band");
    assert!(cut(1.0, &tone, &neg).abs() < 0.1, "side tone, mid band");
}

#[test]
fn there_is_no_latency() {
    let h = eq();
    assert_eq!(h.device().latency_frames(), 0);
}

#[test]
fn the_meters_report_the_band_gains_and_the_peaks() {
    let mut h = bell(0.0, -60.0);
    set(&mut h, 2, "gain", 6.0);
    let t = sine(1_000.0, 0.2);
    run_lr(&mut h, &t, &t, None, ms(300.0));
    let mut m = [0.0f32; METER_COUNT];
    assert_eq!(h.device().meter_values(&mut m), METER_COUNT);
    assert!((m[METER_GAINS + 1] - 6.0).abs() < 0.05, "{:?}", &m[..5]);
    for k in [0, 2, 3, 4] {
        assert_eq!(m[METER_GAINS + k], 0.0);
    }
    assert!(
        m[METER_PEAKS] > 0.35 && m[METER_PEAKS] < 0.45,
        "{}",
        m[METER_PEAKS]
    );
    assert_eq!(m[METER_KEY_CONNECTED], 0.0);
}

#[test]
fn the_analysers_run_only_while_asked_and_read_input_and_key() {
    let main = sine(1_000.0, 0.3);
    let key = sine(4_000.0, 0.3);
    let energies = |live: bool, with_key: bool| {
        let mut h = eq();
        h.set_param("internal.analyser_live", if live { 1.0 } else { 0.0 });
        let k: &dyn Fn(usize) -> f32 = &key;
        run_lr(&mut h, &main, &main, with_key.then_some(k), ms(300.0));
        let mut m = [0.0f32; METER_COUNT];
        h.device().meter_values(&mut m);
        m
    };
    let idle = energies(false, true);
    assert!(idle[METER_INPUT..METER_PEAKS].iter().all(|e| *e == 0.0));
    assert_eq!(idle[METER_KEY_CONNECTED], 1.0);
    let m = energies(true, true);
    let centres = crate::eq::analyser_centres(SR);
    let top = |range: std::ops::Range<usize>| {
        let at = range
            .clone()
            .max_by(|a, b| m[*a].total_cmp(&m[*b]))
            .unwrap();
        (at - range.start, m[at])
    };
    let (i, ei) = top(METER_INPUT..METER_KEY);
    assert!(
        (centres[i] / 1_000.0).log2().abs() < 0.5,
        "input peak at {}",
        centres[i]
    );
    assert!(ei > 0.01);
    let (k, ek) = top(METER_KEY..METER_PEAKS);
    assert!(
        (centres[k] / 4_000.0).log2().abs() < 0.5,
        "key peak at {}",
        centres[k]
    );
    assert!(ek > 0.01);
    // Without a cable the key bank reads nothing.
    let m = energies(true, false);
    assert!(m[METER_KEY..METER_PEAKS].iter().all(|e| *e == 0.0));
    assert_eq!(m[METER_KEY_CONNECTED], 0.0);
}

#[test]
fn the_analyser_does_not_change_the_audio() {
    let run = |live: f32| {
        let mut h = bell(-9.0, -30.0);
        h.set_param("internal.analyser_live", live);
        let t = sine(1_000.0, 0.4);
        run_lr(&mut h, &t, &t, None, ms(200.0)).0
    };
    let (a, b) = (run(0.0), run(1.0));
    assert!(a.iter().zip(&b).all(|(x, y)| x.to_bits() == y.to_bits()));
}

#[test]
fn the_output_gain_scales_the_signal() {
    let mut h = eq();
    h.set_param("output", -6.0);
    let t = sine(1_000.0, 0.5);
    let (y, _) = run_lr(&mut h, &t, &t, None, ms(300.0));
    assert!((tail_db(&y, 1_000.0) - db(0.5) + 6.0).abs() < 0.05);
}

#[test]
fn two_runs_give_the_same_bits() {
    let run = || {
        let mut h = bell(-9.0, -30.0);
        set(&mut h, 4, "on", 1.0);
        set(&mut h, 4, "gain", 3.0);
        set(&mut h, 4, "channel", 1.0);
        let (nl, nr) = (noise(7, 0.4), noise(8, 0.4));
        run_lr(&mut h, &nl, &nr, None, ms(300.0))
    };
    let (a, b) = (run(), run());
    assert!(
        a.0.iter()
            .zip(&b.0)
            .all(|(x, y)| x.to_bits() == y.to_bits())
    );
    assert!(
        a.1.iter()
            .zip(&b.1)
            .all(|(x, y)| x.to_bits() == y.to_bits())
    );
}

#[test]
fn every_param_at_its_ends_keeps_the_output_finite() {
    let mut h = eq();
    for b in 1..=BANDS {
        set(&mut h, b, "on", 1.0);
    }
    sweep_params_min_max(&mut h, &DESCRIPTOR);
}

#[test]
fn a_changing_static_gain_does_not_jump() {
    // Moving the gain by 12 dB at once is smoothed: no sample-to-sample step beyond the
    // signal's own slope.
    let mut h = eq();
    set(&mut h, 2, "on", 1.0);
    set(&mut h, 2, "range", 0.0);
    set(&mut h, 2, "gain", 0.5);
    let t = sine(100.0, 0.2);
    run_lr(&mut h, &t, &t, None, ms(100.0));
    set(&mut h, 2, "gain", 12.5);
    let (y, _) = run_lr(
        &mut h,
        |n| t(n + ms(100.0)),
        |n| t(n + ms(100.0)),
        None,
        ms(100.0),
    );
    let worst = y.windows(2).fold(0.0f32, |m, w| m.max((w[1] - w[0]).abs()));
    assert!(worst < 0.2, "largest step {worst}");
}

#[test]
fn the_delta_of_a_neutral_device_is_silence() {
    let mut h = eq();
    h.set_param("delta", 1.0);
    let (nl, nr) = (noise(1, 0.7), noise(2, 0.7));
    let (yl, yr) = run_lr(&mut h, &nl, &nr, None, ms(300.0));
    // The switch crossfades from the sound over 10 ms (a 100 ms margin lets it finish); then the
    // delta of an untouched signal is exactly zero.
    let from = ms(100.0);
    assert!(
        yl[from..]
            .iter()
            .chain(&yr[from..])
            .all(|v| v.to_bits() == 0.0f32.to_bits())
    );
    // Bands that are on but do nothing (no gain, no range) are neutral as well.
    set(&mut h, 2, "on", 1.0);
    set(&mut h, 2, "range", 0.0);
    let (yl, _) = run_lr(&mut h, &nl, &nr, None, ms(300.0));
    assert!(yl.iter().all(|v| *v == 0.0));
}

#[test]
fn the_delta_is_the_input_minus_the_output() {
    let t = sine(1_000.0, 0.5);
    let mut cut = bell(0.0, -60.0);
    set(&mut cut, 2, "gain", -9.0);
    let (processed, _) = run_lr(&mut cut, &t, &t, None, ms(500.0));
    let mut h = bell(0.0, -60.0);
    set(&mut h, 2, "gain", -9.0);
    h.set_param("delta", 1.0);
    let (delta, _) = run_lr(&mut h, &t, &t, None, ms(500.0));
    // After the fade the delta is what the cut removed: x - y sample for sample.
    let from = ms(100.0);
    for n in from..delta.len() {
        let want = t(n) - processed[n];
        assert!(
            (delta[n] - want).abs() < 1e-6,
            "{n}: {} vs {want}",
            delta[n]
        );
    }
    // A 9 dB cut of a sine removes 1 - 10^(-9/20) = 0.65 of its amplitude.
    let removed = db(tone_amplitude(
        &delta[delta.len() - ms(100.0)..],
        1_000.0,
        delta.len() - ms(100.0),
    ));
    assert!((removed - db(0.5 * 0.646)).abs() < 0.3, "{removed}");
}

#[test]
fn the_delta_ignores_the_output_gain_and_switches_without_a_jump() {
    let t = sine(500.0, 0.4);
    let mut h = bell(0.0, -60.0);
    set(&mut h, 2, "gain", -6.0);
    h.set_param("output", 12.0);
    run_lr(&mut h, &t, &t, None, ms(200.0));
    h.set_param("delta", 1.0);
    let (y, _) = run_lr(
        &mut h,
        |n| t(n + ms(200.0)),
        |n| t(n + ms(200.0)),
        None,
        ms(200.0),
    );
    let worst = y.windows(2).fold(0.0f32, |m, w| m.max((w[1] - w[0]).abs()));
    assert!(worst < 0.4, "largest step {worst}");
    assert!(
        y[y.len() - 1].abs() < 0.4,
        "the 12 dB output gain is not applied to the delta"
    );
}

#[test]
fn cuts_from_found_resonances_act_only_while_the_peak_is_there() {
    use atomik_dsp::resonance::{Settings, find_resonances};
    // Find a 1.2 kHz resonance over a quiet noise floor.
    let n = (6.0 * SR) as usize;
    let mut r = Rng::new(21);
    let floor: Vec<f32> = (0..n).map(|_| 0.02 * r.next_bipolar()).collect();
    let tone = sine(1_200.0, 0.1);
    let mix: Vec<f32> = (0..n).map(|i| floor[i] + tone(i)).collect();
    let found = find_resonances(&mix, &mix, SR as f32, &Settings::default());
    assert_eq!(found.len(), 1, "{found:?}");
    let free = cuts::free_bells(&[false; BANDS]);
    assert_eq!(free, vec![1, 2, 3]);
    let plan = cuts::plan(&found, &free);
    assert_eq!(plan.len(), 1);
    let (band, cut) = plan[0];
    assert_eq!(band, 1);
    assert!(cut.range_db <= -3.0 && cut.range_db >= -12.0);
    assert!((cut.freq_hz - 1_200.0).abs() < 25.0);
    let apply = |h: &mut DeviceHarness| {
        for (k, v) in cut.params(band) {
            h.set_param(&k, v);
        }
    };
    // With the resonance present the band cuts it.
    let mut h = eq();
    apply(&mut h);
    let (y, _) = run_lr(&mut h, &tone, &tone, None, ms(800.0));
    let cut_db = tail_db(&y, 1_200.0) - db(0.1);
    assert!(cut_db < -3.0, "{cut_db}");
    // At the level of the surrounding sound it leaves the band alone.
    let quiet = sine(1_200.0, 0.0015);
    let mut h = eq();
    apply(&mut h);
    let (y, _) = run_lr(&mut h, &quiet, &quiet, None, ms(800.0));
    let quiet_db = tail_db(&y, 1_200.0) - db(0.0015);
    assert!(quiet_db > -0.5, "{quiet_db}");
    // Bands that are on are not touched: nothing is free.
    assert!(cuts::free_bells(&[true; BANDS]).is_empty());
    assert!(cuts::plan(&found, &[]).is_empty());
}
