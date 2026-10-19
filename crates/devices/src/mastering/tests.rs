//! Mastering tests. Everything runs through `DeviceHarness`, which guards every
//! `process` call against allocation.

use super::loudness::{DETECT_DELAY, PeakDetector, k_weighting};
use super::*;
use crate::test_support::{assert_keys, sweep_params_min_max, tone_amplitude};
use atomik_dsp::Rng;
use atomik_testkit::harness::DeviceHarness;
use proptest::prelude::{ProptestConfig, prop_assert, proptest};

const TAU: f64 = std::f64::consts::TAU;

fn dev_at(sr: f64) -> DeviceHarness {
    DeviceHarness::with_options(Box::new(Mastering::new()), sr, 120.0, 0)
}

fn dev() -> DeviceHarness {
    dev_at(48_000.0)
}

/// A device with the multiband bypassed and the ceiling at 0 dBTP (meter and limiter tests).
fn plain(sr: f64) -> DeviceHarness {
    let mut h = dev_at(sr);
    h.set_param("multiband.on", 0.0);
    h.set_param("ceiling", 0.0);
    h
}

type Sig<'a> = &'a dyn Fn(usize) -> f32;

/// Processes `n` samples (rounded up to whole batches) starting at sample index `start` and
/// calls `cb(end_index, &meters)` after every batch; returns the output if `keep`.
fn run_cb(
    h: &mut DeviceHarness,
    l: Sig,
    r: Sig,
    start: usize,
    n: usize,
    keep: bool,
    cb: &mut dyn FnMut(usize, &[f32; 8]),
) -> (Vec<f32>, Vec<f32>) {
    let batches = n.div_ceil(BATCH);
    let (mut ol, mut or) = (Vec::new(), Vec::new());
    for b in 0..batches {
        let (mut xl, mut xr) = ([0.0f32; BATCH], [0.0f32; BATCH]);
        for i in 0..BATCH {
            xl[i] = l(start + b * BATCH + i);
            xr[i] = r(start + b * BATCH + i);
        }
        *h.input_mut("in_l") = xl;
        *h.input_mut("in_r") = xr;
        h.process();
        if keep {
            ol.extend_from_slice(h.output("out_l"));
            or.extend_from_slice(h.output("out_r"));
        }
        cb(start + (b + 1) * BATCH, &meters(h));
    }
    (ol, or)
}

fn run(h: &mut DeviceHarness, l: Sig, r: Sig, start: usize, n: usize) -> (Vec<f32>, Vec<f32>) {
    run_cb(h, l, r, start, n, true, &mut |_, _| {})
}

fn meters(h: &DeviceHarness) -> [f32; 8] {
    let mut m = [0.0f32; 8];
    assert_eq!(h.device().meter_values(&mut m), 8);
    m
}

fn tone(hz: f64, amp_db: f64, sr: f64) -> impl Fn(usize) -> f32 {
    let a = 10f64.powf(amp_db / 20.0);
    move |n| (a * (TAU * hz * n as f64 / sr).sin()) as f32
}

fn silence(_: usize) -> f32 {
    0.0
}

/// Reference true peak: 16x windowed-sinc interpolation (65 taps, Hann), in `f64`, over the
/// samples `from..to` (the sample values themselves are phase 0).
fn true_peak_ref(y: &[f32], from: usize, to: usize) -> f64 {
    let half = 32i64;
    let pi = std::f64::consts::PI;
    // Weight table [phase 1..16][tap].
    let mut w = vec![[0.0f64; 65]; 16];
    for (k, row) in w.iter_mut().enumerate().skip(1) {
        let frac = k as f64 / 16.0;
        for (jj, wt) in row.iter_mut().enumerate() {
            let t = (jj as i64 - half) as f64 - frac;
            let sinc = (pi * t).sin() / (pi * t);
            *wt = sinc * 0.5 * (1.0 + (pi * t / (half as f64 + 1.0)).cos());
        }
    }
    let mut peak = 0.0f64;
    for i in from.max(32)..to.min(y.len() - 33) {
        peak = peak.max(f64::from(y[i]).abs());
        for row in w.iter().skip(1) {
            let mut acc = 0.0;
            for (jj, wt) in row.iter().enumerate() {
                acc += f64::from(y[i + jj - 32]) * wt;
            }
            peak = peak.max(acc.abs());
        }
    }
    peak
}

fn db(x: f64) -> f64 {
    20.0 * x.max(1e-12).log10()
}

fn noise(seed: u64, amp: f32) -> impl Fn(usize) -> f32 {
    let mut rng = Rng::new(seed);
    let v: Vec<f32> = (0..100_000)
        .map(|_| (rng.next_f32() * 2.0 - 1.0) * amp)
        .collect();
    move |n| v[n % v.len()]
}

const ALL_KEYS: [&str; 28] = [
    "input",
    "multiband.on",
    "xover.low",
    "xover.high",
    "band1.threshold",
    "band1.ratio",
    "band1.attack",
    "band1.release",
    "band1.gain",
    "band1.on",
    "band2.threshold",
    "band2.ratio",
    "band2.attack",
    "band2.release",
    "band2.gain",
    "band2.on",
    "band3.threshold",
    "band3.ratio",
    "band3.attack",
    "band3.release",
    "band3.gain",
    "band3.on",
    "width",
    "mono_below",
    "ceiling",
    "release",
    "output",
    "meter.reset",
];

#[test]
fn descriptor_validates() {
    assert_keys(
        &DESCRIPTOR,
        &ALL_KEYS,
        &["in_l", "in_r", "out_l", "out_r", "gr_cv_out"],
    );
    assert_eq!(DESCRIPTOR.type_id, "atomik.mastering");
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Effect);
    let c = DESCRIPTOR.param_by_key("ceiling").unwrap();
    assert_eq!((c.min, c.max, c.default), (-12.0, 0.0, -1.0));
}

#[test]
fn k_weighting_matches_the_published_48k_coefficients() {
    let (shelf, rlb) = k_weighting(48_000.0);
    let near = |a: f64, b: f64| assert!((a - b).abs() < 1e-9, "{a} vs {b}");
    near(shelf.b0, 1.535_124_859_586_97);
    near(shelf.b1, -2.691_696_189_406_38);
    near(shelf.b2, 1.198_392_810_852_85);
    near(shelf.a1, -1.690_659_293_182_41);
    near(shelf.a2, 0.732_480_774_215_85);
    near(rlb.a1, -1.990_047_454_833_98);
    near(rlb.a2, 0.990_072_250_366_21);
    assert_eq!((rlb.b0, rlb.b1, rlb.b2), (1.0, -2.0, 1.0));
}

#[test]
fn peak_detector_delay_is_17_groups() {
    for refine in [false, true] {
        let mut d = PeakDetector::new(BATCH);
        let mut peaks = Vec::new();
        for b in 0..4 {
            let mut x = [0.0f32; BATCH];
            if b == 0 {
                x[40] = 1.0;
            }
            let mut p = [0.0f32; BATCH];
            d.process(&x, &mut p, refine);
            peaks.extend_from_slice(&p);
        }
        let (idx, v) = peaks
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap();
        assert_eq!(idx, 40 + DETECT_DELAY, "refine {refine}");
        assert!((v - 1.0).abs() < 0.02, "{v}");
    }
}

#[test]
fn allocation_free_at_all_rates() {
    for sr in [44_100.0, 48_000.0, 96_000.0] {
        let mut h = dev_at(sr);
        let n = noise(1, 0.9);
        h.set_param("width", 1.4);
        h.set_param("mono_below", 120.0);
        run_cb(&mut h, &n, &n, 0, 50 * BATCH, false, &mut |_, _| {}); // guarded per batch
        h.set_param("meter.reset", 1.0);
        h.set_param("meter.reset", 0.0);
        run_cb(&mut h, &n, &n, 0, 10 * BATCH, false, &mut |_, _| {});
    }
}

#[test]
fn sweeping_params_stays_finite() {
    let mut h = dev();
    *h.input_mut("in_l") = [0.7; BATCH];
    *h.input_mut("in_r") = [-0.3; BATCH];
    sweep_params_min_max(&mut h, &DESCRIPTOR);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]
    #[test]
    fn params_keep_output_finite_and_below_the_ceiling(
        norm in proptest::collection::vec(0.0f32..1.0, 28), seed in 0u64..100,
    ) {
        let mut h = dev();
        for (p, n) in DESCRIPTOR.params.iter().zip(&norm) {
            if p.key != "meter.reset" {
                h.set_param(p.key, p.from_normalized(*n));
            }
        }
        let n = noise(seed, 1.0);
        let (l, r) = run(&mut h, &n, &n, 0, 120 * BATCH);
        let c = DESCRIPTOR.param_by_key("ceiling").unwrap();
        let o = DESCRIPTOR.param_by_key("output").unwrap();
        let ceil = 10f32.powf(c.from_normalized(norm[24]) / 20.0);
        let og = 10f32.powf(o.from_normalized(norm[26]) / 20.0);
        for ch in [&l, &r] {
            prop_assert!(ch.iter().all(|s| s.is_finite() && s.abs() <= ceil * og * 1.001));
        }
    }
}

#[test]
fn deterministic() {
    let go = || {
        let mut h = dev();
        h.set_param("width", 1.3);
        h.set_param("band2.threshold", -30.0);
        let n = noise(5, 0.8);
        run(&mut h, &n, &n, 0, 400 * BATCH)
    };
    assert_eq!(go(), go());
}

#[test]
fn latency_is_detector_plus_lookahead_and_matches_the_impulse_delay() {
    for sr in [44_100.0, 48_000.0, 96_000.0] {
        let mut h = plain(sr);
        let la = (1.5e-3 * sr).round() as u32;
        let want = DETECT_DELAY as u32 + la;
        assert_eq!(h.device().latency_frames(), want, "{sr}");
        let imp = |n: usize| if n == 3 { 0.5 } else { 0.0 };
        let (l, _) = run(&mut h, &imp, &imp, 0, 400 * BATCH / 4);
        let idx = l.iter().position(|v| *v != 0.0).unwrap();
        assert_eq!(idx as u32, 3 + want, "{sr}");
        assert_eq!(l[idx], 0.5);
        assert_eq!(h.device().latency_frames(), want);
    }
}

#[test]
fn bypassed_chain_is_a_bit_exact_delay() {
    // multiband off, width 100 %, signal below the ceiling: pure delay, bit for bit.
    let mut h = plain(48_000.0);
    let n = noise(3, 0.5);
    let (l, r) = run(&mut h, &n, &n, 0, 300 * BATCH);
    let d = h.device().latency_frames() as usize;
    for i in d..l.len() {
        assert_eq!(l[i].to_bits(), n(i - d).to_bits(), "sample {i}");
        assert_eq!(r[i].to_bits(), n(i - d).to_bits());
    }
}

#[test]
fn crossovers_sum_flat_within_a_tenth_of_a_db() {
    // Ratio 1 everywhere: no compression, unity band gains. Sine gain at many frequencies
    // around and between both crossovers, for default and moved crossovers.
    let freqs = [
        30.0, 60.0, 100.0, 150.0, 200.0, 250.0, 400.0, 800.0, 1_500.0, 2_500.0, 3_000.0, 3_500.0,
        5_000.0, 8_000.0, 12_000.0, 17_000.0,
    ];
    let mut worst = 0.0f64;
    for (lo, hi) in [
        (200.0, 3_000.0),
        (80.0, 600.0),
        (1_500.0, 500.0),
        (500.0, 5_000.0),
    ] {
        let mut h = dev();
        h.set_param("xover.low", lo);
        h.set_param("xover.high", hi);
        for k in 1..=3 {
            h.set_param(&format!("band{k}.ratio"), 1.0);
        }
        let (eff_lo, eff_hi) = {
            let mut m = Mastering::new();
            m.set_param(ParamId(P_XLOW), lo);
            m.set_param(ParamId(P_XHIGH), hi);
            m.effective_crossovers()
        };
        assert!(eff_hi > eff_lo, "{lo}/{hi}: {eff_lo}/{eff_hi}");
        for f in freqs {
            let a = 0.1;
            let s = tone(f, db(a), 48_000.0);
            let settle = 24_000;
            let len = 48_000;
            let (y, _) = run(&mut h, &s, &s, 0, settle + len);
            let amp = tone_amplitude(&y[settle..settle + len], f, settle);
            let g = db(amp / a);
            worst = worst.max(g.abs());
            assert!(g.abs() < 0.1, "xover {lo}/{hi} at {f} Hz: {g} dB");
            h.device_mut().reset();
        }
    }
    eprintln!("crossover flatness: worst deviation {worst:.6} dB");
}

#[test]
fn band_compressors_act_on_their_own_band() {
    // A 100 Hz tone lives in band 1: only band 1 reduces gain.
    let mut h = plain(48_000.0);
    h.set_param("multiband.on", 1.0);
    for k in 1..=3 {
        h.set_param(&format!("band{k}.threshold"), -30.0);
        h.set_param(&format!("band{k}.ratio"), 10.0);
        h.set_param(&format!("band{k}.attack"), 1.0);
    }
    let s = tone(100.0, -12.0, 48_000.0);
    let mut last = [0.0f32; 8];
    run_cb(&mut h, &s, &s, 0, 48_000, false, &mut |_, m| last = *m);
    assert!(last[4] > 14.0, "low band GR {}", last[4]);
    assert!(last[5] < 0.5 && last[6] < 0.5, "{last:?}");
    // And a 6 kHz tone only in band 3.
    let mut h = plain(48_000.0);
    h.set_param("multiband.on", 1.0);
    for k in 1..=3 {
        h.set_param(&format!("band{k}.threshold"), -30.0);
        h.set_param(&format!("band{k}.ratio"), 10.0);
        h.set_param(&format!("band{k}.attack"), 1.0);
    }
    let s = tone(6_000.0, -12.0, 48_000.0);
    run_cb(&mut h, &s, &s, 0, 48_000, false, &mut |_, m| last = *m);
    assert!(last[6] > 14.0 && last[4] < 0.5 && last[5] < 0.5, "{last:?}");
}

#[test]
fn band_static_curve_gain_and_bypass() {
    // 1 kHz (band 2) at -10 dBFS, threshold -30, ratio 4, knee 6: GR = 0.75 * 20 = 15 dB.
    let run_with = |on: f32, gain: f32| {
        let mut h = dev();
        h.set_param("band2.threshold", -30.0);
        h.set_param("band2.ratio", 4.0);
        h.set_param("band2.attack", 1.0);
        h.set_param("band2.release", 2_000.0);
        h.set_param("band2.on", on);
        h.set_param("band2.gain", gain);
        let s = tone(1_000.0, -10.0, 48_000.0);
        let mut last = [0.0f32; 8];
        let (y, _) = run_cb(&mut h, &s, &s, 0, 96_000, true, &mut |_, m| last = *m);
        let amp = tone_amplitude(&y[48_000..96_000], 1_000.0, 48_000);
        (last[5], db(amp))
    };
    let (gr, lvl) = run_with(1.0, 0.0);
    assert!((gr - 15.0).abs() < 0.3, "GR {gr}");
    // The bands leak a little into each other (LR4 skirts), the uncompressed high band
    // adds about 0.6 dB to the compressed mid tone.
    assert!((lvl - (-10.0 - 15.0)).abs() < 1.0, "level {lvl}");
    let (gr, lvl) = run_with(0.0, 0.0);
    assert!(
        gr == 0.0 && (lvl + 10.0).abs() < 0.1,
        "bypassed: {gr} {lvl}"
    );
    let (_, lvl) = run_with(0.0, 6.0);
    assert!((lvl - (-10.0 + 6.0)).abs() < 0.1, "band gain {lvl}");
}

#[test]
fn width_zero_is_mono_and_one_is_unchanged() {
    let nl = noise(1, 0.4);
    let nr = noise(2, 0.4);
    // 0 %: bit-identical channels (through the whole chain).
    let mut h = dev();
    h.set_param("width", 0.0);
    let (l, r) = run(&mut h, &nl, &nr, 0, 300 * BATCH);
    assert_eq!(l, r);
    assert!(l.iter().any(|v| v.abs() > 0.01));
    // 100 % with the multiband bypassed: bit-exact delayed input (no matrix at all).
    let mut h = plain(48_000.0);
    let (l, r) = run(&mut h, &nl, &nr, 0, 300 * BATCH);
    let lat = h.device().latency_frames() as usize;
    for i in lat..l.len() {
        assert_eq!(l[i].to_bits(), nl(i - lat).to_bits());
        assert_eq!(r[i].to_bits(), nr(i - lat).to_bits());
    }
    // 200 %: the side doubles (L = x, R = -x becomes 2x, -2x; mid unchanged).
    let mut h = plain(48_000.0);
    h.set_param("width", 2.0);
    let s = tone(1_000.0, -20.0, 48_000.0);
    let neg = |n: usize| -s(n);
    let (l, r) = run(&mut h, &s, &neg, 0, 48_000);
    let a = tone_amplitude(&l[24_000..48_000], 1_000.0, 24_000);
    assert!((db(a / 0.1) - 6.02).abs() < 0.02, "{}", db(a / 0.1));
    for i in 24_000..48_000 {
        assert!((l[i] + r[i]).abs() < 1e-6);
    }
}

#[test]
fn mono_below_removes_the_side_signal_below_the_frequency() {
    let side_gain = |hz: f64, mono_below: f32| {
        let mut h = plain(48_000.0);
        h.set_param("mono_below", mono_below);
        let s = tone(hz, -20.0, 48_000.0);
        let neg = |n: usize| -s(n);
        let (l, _) = run(&mut h, &s, &neg, 0, 96_000);
        db(tone_amplitude(&l[48_000..96_000], hz, 48_000) / 0.1)
    };
    // Corner 400 Hz: LR4 high-pass on the side.
    assert!(
        side_gain(100.0, 400.0) < -40.0,
        "{}",
        side_gain(100.0, 400.0)
    );
    assert!((side_gain(400.0, 400.0) + 6.02).abs() < 0.2);
    assert!(side_gain(4_000.0, 400.0).abs() < 0.1);
    assert!(side_gain(100.0, 0.0).abs() < 0.01, "off passes the side");
    // Mono content passes unchanged at any frequency.
    let mut h = plain(48_000.0);
    h.set_param("mono_below", 400.0);
    let s = tone(100.0, -20.0, 48_000.0);
    let (l, r) = run(&mut h, &s, &s, 0, 48_000);
    assert_eq!(l, r);
    let a = tone_amplitude(&l[24_000..48_000], 100.0, 24_000);
    assert!((db(a / 0.1)).abs() < 0.01);
}

/// Output of the limiter for a stereo test signal: (peak reference dBTP after the settle, output).
fn limited_peak(h: &mut DeviceHarness, l: Sig, r: Sig, n: usize) -> (f64, Vec<f32>, Vec<f32>) {
    let mut last = [0.0f32; 8];
    let (ol, or) = run_cb(h, l, r, 0, n, true, &mut |_, m| last = *m);
    // The reference skips the first 3000 samples: the step from digital silence has
    // reconstruction (Gibbs) content above the 0.4 fs the 4x detector passes (the device's own
    // meter reading, which does include the onset, is checked separately by the callers).
    let end = ol.len() - 64; // and the last samples (their interpolation window is cut off)
    let p = true_peak_ref(&ol, 3_000, end).max(true_peak_ref(&or, 3_000, end));
    LAST_METER_TP.with(|c| c.set(last[3]));
    (db(p), ol, or)
}

thread_local! {
    static LAST_METER_TP: std::cell::Cell<f32> = const { std::cell::Cell::new(0.0) };
}

/// Noise low-passed at about 0.35 fs (8th-order Butterworth) and normalised to a +-1 peak: the
/// band the 4x detector passes flat (programme material has little above 0.4 fs).
fn band_limited_noise(seed: u64, len: usize) -> Vec<f32> {
    use atomik_dsp::{Biquad, BiquadCoeffs, BiquadShape};
    let n = noise(seed, 1.0);
    let sr = 48_000.0;
    let qs = [0.5098, 0.6013, 0.8999, 2.5629];
    let c: Vec<BiquadCoeffs> = qs
        .iter()
        .map(|q| BiquadCoeffs::design(BiquadShape::Lowpass, 0.35 * sr, *q, 0.0, sr))
        .collect();
    let mut st = [Biquad::default(); 4];
    let mut v: Vec<f32> = (0..len)
        .map(|i| {
            let mut x = f64::from(n(i));
            for (b, co) in st.iter_mut().zip(&c) {
                x = b.process(co, x);
            }
            x as f32
        })
        .collect();
    let peak = v.iter().fold(0.0f32, |p, x| p.max(x.abs()));
    v.iter_mut().for_each(|x| *x /= peak);
    v
}

#[test]
fn limiter_holds_the_ceiling_on_an_inter_sample_peak_signal() {
    // fs/4 sine at 45 degrees: samples are +-0.7071 A, the true peak is A. +6 dB gain on a
    // -6 dBFS sample peak input pushes the true peak to +3 dBTP; ceilings -1, -3, -6.
    let sr = 48_000.0;
    let a = 0.5 * std::f64::consts::SQRT_2;
    let s =
        move |n: usize| (a * (TAU * 0.25 * n as f64 + std::f64::consts::FRAC_PI_4).sin()) as f32;
    for ceiling in [-1.0f32, -3.0, -6.0] {
        let mut h = dev_at(sr);
        h.set_param("multiband.on", 0.0);
        h.set_param("input", 6.0);
        h.set_param("ceiling", ceiling);
        let (tp, y, _) = limited_peak(&mut h, &s, &s, 48_000);
        let meter = LAST_METER_TP.with(std::cell::Cell::get);
        eprintln!(
            "fs/4 ceiling {ceiling}: reference true peak {tp:.3} dBTP (overshoot {:.3} dB), meter incl. onset {meter:.3}",
            tp - f64::from(ceiling)
        );
        assert!(
            tp <= f64::from(ceiling) + 0.1,
            "ceiling {ceiling}: {tp:.3} dBTP"
        );
        assert!(
            f64::from(meter) <= f64::from(ceiling) + 0.1,
            "meter {meter}"
        );
        assert!(tp > f64::from(ceiling) - 0.6, "not over-limited: {tp:.3}");
        assert!(y.iter().all(|v| v.is_finite()));
    }
}

#[test]
fn limiter_holds_the_ceiling_on_full_scale_noise_and_sines() {
    let bl = band_limited_noise(1, 200_000);
    let bl2 = band_limited_noise(2, 200_000);
    let bln = move |n: usize| bl[n % bl.len()];
    let bln2 = move |n: usize| bl2[n % bl2.len()];
    type Case = (&'static str, Box<dyn Fn(usize) -> f32>, f32);
    let cases: Vec<Case> = vec![
        ("band-limited noise 0 dBFS", Box::new(bln), 0.0),
        ("band-limited noise +12 dB", Box::new(bln2), 12.0),
        (
            "sine 997 Hz 0 dBFS",
            Box::new(tone(997.0, 0.0, 48_000.0)),
            6.0,
        ),
        ("sine 19 kHz", Box::new(tone(19_000.0, -1.0, 48_000.0)), 6.0),
        (
            "sine 11.5 kHz",
            Box::new(tone(11_500.0, 0.0, 48_000.0)),
            3.0,
        ),
        ("white noise 0 dBFS", Box::new(noise(1, 1.0)), 0.0),
        ("white noise +12 dB", Box::new(noise(2, 1.0)), 12.0),
    ];
    for (name, sig, gain) in &cases {
        let mut h = dev();
        h.set_param("multiband.on", 0.0);
        h.set_param("input", *gain);
        let (tp, _, _) = limited_peak(&mut h, &**sig, &**sig, 96_000);
        let meter = f64::from(LAST_METER_TP.with(std::cell::Cell::get));
        eprintln!(
            "{name}: reference true peak {tp:.3} dBTP, overshoot {:+.3} dB; BS.1770 meter {meter:.3}",
            tp + 1.0
        );
        // The BS.1770 (4x) reading never exceeds the ceiling; the 16x reference stays within
        // 0.1 dB for everything up to 0.4 fs. White noise has content in the 0.4 to 0.5 fs band
        // that a 4x halfband detector attenuates; its reference overshoot is only bounded here.
        assert!(meter <= -1.0 + 0.1, "{name}: meter {meter}");
        if name.starts_with("white") {
            assert!(tp <= -1.0 + 1.5, "{name}: {tp:.3} dBTP");
        } else {
            assert!(tp <= -1.0 + 0.1, "{name}: {tp:.3} dBTP");
        }
    }
}

#[test]
fn limiter_holds_the_ceiling_through_the_whole_chain() {
    // Default chain (multiband on, compression, width 150 %), loud band-limited noise.
    let bl = band_limited_noise(5, 200_000);
    let bl2 = band_limited_noise(6, 200_000);
    let (nl, nr) = (
        move |n: usize| bl[n % bl.len()],
        move |n: usize| bl2[n % bl2.len()],
    );
    let mut h = dev();
    h.set_param("width", 1.5);
    h.set_param("input", 9.0);
    let (tp, _, _) = limited_peak(&mut h, &nl, &nr, 96_000);
    eprintln!("whole chain: reference true peak {tp:.3} dBTP");
    assert!(tp <= -1.0 + 0.1, "{tp}");
}

#[test]
fn limiter_does_not_distort_sustained_tones() {
    for hz in [100.0, 1_000.0, 5_000.0] {
        let mut h = dev();
        h.set_param("multiband.on", 0.0);
        h.set_param("input", 6.0); // 0 dBFS tone -> +6 dBFS, ceiling -1
        let s = tone(hz, 0.0, 48_000.0);
        let (y, _) = run(&mut h, &s, &s, 0, 2 * 48_000);
        // Residual after removing the fitted fundamental over whole cycles.
        let n = 48_000;
        let w = &y[48_000..48_000 + n];
        let (mut sn, mut cs) = (0.0f64, 0.0f64);
        for (i, v) in w.iter().enumerate() {
            let ph = TAU * hz * (48_000 + i) as f64 / 48_000.0;
            sn += f64::from(*v) * ph.sin();
            cs += f64::from(*v) * ph.cos();
        }
        let (a, b) = (2.0 * sn / n as f64, 2.0 * cs / n as f64);
        let mut res = 0.0f64;
        let mut tot = 0.0f64;
        for (i, v) in w.iter().enumerate() {
            let ph = TAU * hz * (48_000 + i) as f64 / 48_000.0;
            let fit = a * ph.sin() + b * ph.cos();
            res += (f64::from(*v) - fit).powi(2);
            tot += fit * fit;
        }
        let thd = 10.0 * (res / tot).log10();
        eprintln!("{hz} Hz sustained limiting: residual {thd:.1} dB");
        assert!(
            thd < if hz < 500.0 { -45.0 } else { -60.0 },
            "{hz} Hz: residual {thd} dB"
        );
        let amp = (a * a + b * b).sqrt();
        assert!((db(amp) + 1.0).abs() < 0.25, "level {}", db(amp));
    }
}

#[test]
fn limiter_release_is_the_set_time_constant() {
    // Loud burst, then silence: the gain recovers exponentially with the release constant.
    let mut h = dev();
    h.set_param("multiband.on", 0.0);
    h.set_param("input", 12.0);
    h.set_param("release", 100.0);
    let s = |n: usize| {
        if n < 24_000 {
            tone(1_000.0, 0.0, 48_000.0)(n)
        } else {
            0.0
        }
    };
    let mut gr = Vec::new();
    run_cb(&mut h, &s, &s, 0, 48_000, false, &mut |i, m| {
        gr.push((i, m[7]))
    });
    let env = |g: f32| 10f64.powf(-f64::from(g) / 20.0);
    let (g0, t0) = {
        // The first batch whose GR starts falling after the peak.
        let peak = gr.iter().map(|x| x.1).fold(0.0f32, f32::max);
        let i = gr.iter().rposition(|x| x.1 >= peak - 0.05).unwrap();
        (env(gr[i].1), gr[i].0)
    };
    let target = 1.0 - (1.0 - g0) * (-1.0f64).exp();
    let t = gr
        .iter()
        .find(|x| x.0 > t0 && env(x.1) >= target)
        .map(|x| x.0)
        .unwrap();
    let tau_ms = (t - t0) as f64 / 48.0;
    eprintln!("limiter release: gain 0.368 point after {tau_ms:.1} ms (set 100)");
    assert!((tau_ms - 100.0).abs() < 15.0, "{tau_ms}");
}

#[test]
fn gr_cv_out_follows_the_limiter() {
    let mut h = dev();
    h.set_param("multiband.on", 0.0);
    let quiet = tone(1_000.0, -30.0, 48_000.0);
    run(&mut h, &quiet, &quiet, 0, 4 * BATCH);
    assert!(h.output("gr_cv_out").iter().all(|v| *v == 0.0));
    h.set_param("input", 12.0);
    let loud = tone(1_000.0, 0.0, 48_000.0);
    run(&mut h, &loud, &loud, 0, 3_000);
    let cv = h.output("gr_cv_out");
    assert!(cv.iter().all(|v| *v > 0.1 && *v < 1.0), "{cv:?}");
}

/// Integrated loudness after feeding `segments` of (seconds, level dBFS of a 1 kHz sine on both
/// channels or one channel).
fn integrated_of(sr: f64, segments: &[(f64, f64)], both: bool) -> f32 {
    let mut h = plain(sr);
    let mut m = [0.0f32; 8];
    let mut start = 0usize;
    for (secs, lvl) in segments {
        let n = (secs * sr) as usize;
        // Phase-continuous across segments: the sample index is absolute.
        let s = tone(1_000.0, *lvl, sr);
        let r: Sig = if both { &s } else { &silence };
        run_cb(&mut h, &s, r, start, n, false, &mut |_, mm| m = *mm);
        start += n.div_ceil(BATCH) * BATCH;
    }
    m[2]
}

#[test]
fn lufs_reference_values_for_sine_signals() {
    // Stereo 1 kHz sine: -23 dBFS peak on both channels reads -23.0 LUFS (EBU Tech 3341
    // case 1); -33 reads -33.0 (case 2); -20 dBFS peak reads -20.0 (the loudness of a stereo
    // sine in LUFS equals its peak dBFS).
    for (lvl, want) in [(-23.0, -23.0), (-33.0, -33.0), (-20.0, -20.0)] {
        let got = integrated_of(48_000.0, &[(20.0, lvl)], true);
        eprintln!("stereo 1 kHz {lvl} dBFS: {got:.3} LUFS (want {want})");
        assert!((f64::from(got) - want).abs() < 0.1, "{lvl}: {got}");
    }
    // One channel only: 997 Hz at 0 dBFS reads -3.01 LUFS.
    let mut h = plain(48_000.0);
    let s = tone(997.0, 0.0, 48_000.0);
    let mut m = [0.0f32; 8];
    run_cb(&mut h, &s, &silence, 0, 20 * 48_000, false, &mut |_, mm| {
        m = *mm
    });
    eprintln!("one channel 997 Hz 0 dBFS: {:.3} LUFS (want -3.01)", m[2]);
    assert!((m[2] + 3.01).abs() < 0.1, "{}", m[2]);
    // And the other channel alone reads the same (L and R have the same weight).
    let mut h = plain(48_000.0);
    run_cb(&mut h, &silence, &s, 0, 20 * 48_000, false, &mut |_, mm| {
        m = *mm
    });
    assert!((m[2] + 3.01).abs() < 0.1, "{}", m[2]);
}

#[test]
fn lufs_accuracy_at_other_sample_rates() {
    for sr in [44_100.0, 96_000.0] {
        let got = integrated_of(sr, &[(15.0, -23.0)], true);
        eprintln!("{sr} Hz stereo 1 kHz -23 dBFS: {got:.3} LUFS");
        assert!((f64::from(got) + 23.0).abs() < 0.1, "{sr}: {got}");
    }
}

#[test]
fn lufs_gating_follows_ebu_tech_3341() {
    // Case 3: 10 s at -36, 60 s at -23, 10 s at -36 -> -23.0. Case 4 adds -72 LUFS segments
    // (absolute gate) around them.
    let c3 = integrated_of(
        48_000.0,
        &[(10.0, -36.0), (60.0, -23.0), (10.0, -36.0)],
        true,
    );
    eprintln!("case 3: {c3:.3} LUFS");
    assert!((f64::from(c3) + 23.0).abs() < 0.1, "{c3}");
    let c4 = integrated_of(
        48_000.0,
        &[
            (10.0, -72.0),
            (10.0, -36.0),
            (60.0, -23.0),
            (10.0, -36.0),
            (10.0, -72.0),
        ],
        true,
    );
    eprintln!("case 4: {c4:.3} LUFS");
    assert!((f64::from(c4) + 23.0).abs() < 0.1, "{c4}");
}

#[test]
fn gating_removes_silence() {
    // Half of the signal is digital silence: only the transition blocks differ.
    let with_gaps = integrated_of(48_000.0, &[(20.0, -23.0), (20.0, -140.0)], true);
    let tone_only = integrated_of(48_000.0, &[(20.0, -23.0)], true);
    eprintln!("tone {tone_only:.3} vs tone+silence {with_gaps:.3}");
    assert!((tone_only - with_gaps).abs() < 0.1);
    // Alternating 5 s tone / 10 s silence: every tone edge adds three partly filled blocks
    // (which the relative gate keeps, as the standard does), so the reading is slightly lower.
    let alt = integrated_of(
        48_000.0,
        &[(5.0, -23.0), (10.0, -140.0), (5.0, -23.0), (10.0, -140.0)],
        true,
    );
    eprintln!("alternating: {alt:.3}");
    assert!((tone_only - alt).abs() < 0.3);
    // Pure silence never leaves the floor.
    let mut h = plain(48_000.0);
    let mut m = [0.0f32; 8];
    run_cb(
        &mut h,
        &silence,
        &silence,
        0,
        5 * 48_000,
        false,
        &mut |_, mm| m = *mm,
    );
    assert_eq!(&m[..4], &[-120.0, -120.0, -120.0, -120.0]);
}

#[test]
fn momentary_and_short_term_windows_on_tone_bursts() {
    let sr = 48_000.0;
    let mut h = plain(sr);
    // 4 s of silence, a 1 s burst at -23 dBFS stereo, then 5 s of silence.
    let burst = tone(1_000.0, -23.0, sr);
    let s = move |n: usize| {
        if (4 * 48_000..5 * 48_000).contains(&n) {
            burst(n)
        } else {
            0.0
        }
    };
    let mut series = Vec::new();
    run_cb(&mut h, &s, &s, 0, 10 * 48_000, false, &mut |i, m| {
        series.push((i as f64 / sr, m[0], m[1]));
    });
    let at = |t: f64| {
        *series
            .iter()
            .min_by(|a, b| (a.0 - t).abs().total_cmp(&(b.0 - t).abs()))
            .unwrap()
    };
    // Before the burst: floor.
    assert_eq!(at(3.9).1, -120.0);
    // 0.2 s into the burst the 400 ms window is half full: -23 - 3.01.
    let m = at(4.2).1;
    assert!((m + 26.0).abs() < 0.4, "momentary 0.2 s in: {m}");
    // 0.5 s in the window is full: -23.0.
    let m = at(4.5).1;
    assert!((m + 23.0).abs() < 0.1, "momentary 0.5 s in: {m}");
    // 0.2 s after the burst ends the window holds 200 ms of tone again: -26.
    let m = at(5.2).1;
    assert!((m + 26.0).abs() < 0.4, "momentary 0.2 s after: {m}");
    // 0.5 s after: gone.
    assert_eq!(at(5.5).1, -120.0);
    // Short-term: 1 s of 3 s window: -23 - 4.77 = -27.8 at the end of the burst, back to the
    // floor 3 s later.
    let s1 = at(5.0).2;
    assert!((s1 + 27.8).abs() < 0.4, "short-term at burst end: {s1}");
    assert!(at(7.9).2 > -40.0, "still in the window");
    assert_eq!(at(8.2).2, -120.0);
    // A long tone fills the short-term window: half at 1.5 s, full at 3 s.
    let mut h = plain(sr);
    let t = tone(1_000.0, -23.0, sr);
    let mut series = Vec::new();
    run_cb(&mut h, &t, &t, 0, 5 * 48_000, false, &mut |i, m| {
        series.push((i as f64 / sr, m[1]))
    });
    let at2 = |x: f64| {
        series
            .iter()
            .min_by(|a, b| (a.0 - x).abs().total_cmp(&(b.0 - x).abs()))
            .unwrap()
            .1
    };
    assert!((at2(1.5) + 26.0).abs() < 0.4, "{}", at2(1.5));
    assert!((at2(3.1) + 23.0).abs() < 0.1, "{}", at2(3.1));
}

#[test]
fn true_peak_reads_inter_sample_peaks() {
    // fs/4 sine at 45 degrees with a -6 dBFS sample peak: true peak -3.01 dBTP.
    let a = 0.5 * std::f64::consts::SQRT_2;
    let s =
        move |n: usize| (a * (TAU * 0.25 * n as f64 + std::f64::consts::FRAC_PI_4).sin()) as f32;
    let mut h = plain(48_000.0);
    let mut m = [0.0f32; 8];
    let (y, _) = run_cb(&mut h, &s, &s, 0, 48_000, true, &mut |_, mm| m = *mm);
    let sample_peak = y.iter().fold(0.0f32, |p, v| p.max(v.abs()));
    assert!((db(f64::from(sample_peak)) + 6.02).abs() < 0.05);
    eprintln!(
        "fs/4 45 deg -6 dBFS sample peak: true peak {:.3} dBTP (want -3.01)",
        m[3]
    );
    assert!((f64::from(m[3]) + 3.01).abs() < 0.2, "{}", m[3]);
    // A 997 Hz sine at 0 dBFS reads about 0 dBTP.
    let mut h = plain(48_000.0);
    let t = tone(997.0, 0.0, 48_000.0);
    run_cb(&mut h, &t, &t, 0, 48_000, false, &mut |_, mm| m = *mm);
    assert!(m[3].abs() < 0.2, "{}", m[3]);
}

#[test]
fn meter_reset_clears_integrated_loudness_and_true_peak() {
    let mut h = plain(48_000.0);
    let s = tone(1_000.0, -20.0, 48_000.0);
    let mut m = [0.0f32; 8];
    run_cb(&mut h, &s, &s, 0, 5 * 48_000, false, &mut |_, mm| m = *mm);
    assert!(m[2] > -21.0 && m[3] > -21.0, "{m:?}");
    // Flush the latency tail with silence (the true-peak maximum holds), then reset.
    run_cb(&mut h, &silence, &silence, 0, 4_800, false, &mut |_, mm| {
        m = *mm
    });
    assert!(m[3] > -21.0, "true peak is held: {m:?}");
    h.set_param("meter.reset", 1.0);
    run_cb(&mut h, &silence, &silence, 0, BATCH, false, &mut |_, mm| {
        m = *mm
    });
    assert_eq!(&m[..4], &[-120.0; 4]);
    // Holding the toggle high does not keep resetting: the next tone is measured.
    run_cb(&mut h, &s, &s, 0, 5 * 48_000, false, &mut |_, mm| m = *mm);
    assert!((m[2] + 20.0).abs() < 0.2, "{m:?}");
    // The device reset also clears.
    h.device_mut().reset();
    run_cb(&mut h, &silence, &silence, 0, BATCH, false, &mut |_, mm| {
        m = *mm
    });
    assert_eq!(&m[..4], &[-120.0; 4]);
}

#[test]
fn meter_values_layout_and_defaults() {
    let h = dev();
    assert!(h.device().has_meter_values());
    let m = meters(&h);
    assert_eq!(m, [-120.0, -120.0, -120.0, -120.0, 0.0, 0.0, 0.0, 0.0]);
    let mut small = [9.0f32; 3];
    assert_eq!(h.device().meter_values(&mut small), 3);
    assert_eq!(small, [-120.0; 3]);
}
