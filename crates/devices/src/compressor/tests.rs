//! Compressor tests. Everything runs through `DeviceHarness`, which guards every
//! `process` call against allocation.

use super::*;
use crate::test_support::{SR, assert_keys, harness, run_signal, sine, sweep_params_min_max};
use atomik_dsp::{Rng, static_gain_reduction_knee_db};
use atomik_testkit::harness::DeviceHarness;
use proptest::prelude::{ProptestConfig, prop_assert, proptest};

const IN: [&str; 2] = ["in_l", "in_r"];

fn comp() -> DeviceHarness {
    harness(Box::new(Compressor::new()))
}

fn comp_at(sr: f64) -> DeviceHarness {
    DeviceHarness::with_options(Box::new(Compressor::new()), sr, 120.0, 0)
}

/// Gain reduction in dB from the `gr_cv_out` value (`1 - g`).
fn gr_db(cv: f32) -> f64 {
    -20.0 * f64::from(1.0 - cv).max(1e-9).log10()
}

fn amp_of_db(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

/// Peak amplitude of the `hz` tone measured on a window of whole cycles.
fn measure(y: &[f32], hz: f64, start: usize) -> f64 {
    crate::test_support::tone_amplitude(y, hz, start)
}

/// Feeds `main` (both inputs) and `key` (both sidechain inputs, when `Some`) sample functions for
/// `batches` batches and returns the `gr_cv_out` of the last sample as GR in dB.
fn final_gr(
    h: &mut DeviceHarness,
    main: impl Fn(usize) -> f32,
    key: Option<&dyn Fn(usize) -> f32>,
    batches: usize,
) -> f64 {
    let mut gr = 0.0;
    for b in 0..batches {
        let mut x = [0.0f32; BATCH];
        let mut k = [0.0f32; BATCH];
        for i in 0..BATCH {
            x[i] = main(b * BATCH + i);
            if let Some(f) = key {
                k[i] = f(b * BATCH + i);
            }
        }
        *h.input_mut("in_l") = x;
        *h.input_mut("in_r") = x;
        if key.is_some() {
            *h.input_mut("sc_in_l") = k;
            *h.input_mut("sc_in_r") = k;
        }
        h.process();
        gr = gr_db(h.output("gr_cv_out")[BATCH - 1]);
    }
    gr
}

#[test]
fn descriptor_validates() {
    assert_keys(
        &DESCRIPTOR,
        &[
            "threshold",
            "ratio",
            "limit",
            "knee",
            "attack",
            "release",
            "auto_release",
            "lookahead",
            "makeup",
            "auto_makeup",
            "mix",
            "sc_enable",
            "sc_hp",
            "sc_lp",
            "stereo_link",
            "detector",
            "bypass",
        ],
        &[
            "in_l",
            "in_r",
            "sc_in_l",
            "sc_in_r",
            "out_l",
            "out_r",
            "gr_cv_out",
        ],
    );
    assert_eq!(DESCRIPTOR.type_id, "atomik.compressor");
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Effect);
    let p = DESCRIPTOR.param_by_key("lookahead").unwrap();
    assert_eq!((p.min, p.max), (0.0, 5.0));
}

/// Static curve: output level of a 1 kHz sine against the analytic soft-knee curve.
fn check_static_curve(rms: bool) {
    let mut worst = 0.0f64;
    for threshold in [-30.0f32, -12.0] {
        for (ratio, limit) in [(2.0f32, 0.0f32), (8.0, 0.0), (20.0, 1.0)] {
            for knee in [0.0f32, 6.0, 12.0] {
                for over in [-18.0f64, -9.0, -4.0, -1.0, 0.0, 1.0, 4.0, 9.0, 18.0] {
                    let mut h = comp();
                    h.set_param("threshold", threshold);
                    h.set_param("ratio", ratio);
                    h.set_param("limit", limit);
                    h.set_param("knee", knee);
                    h.set_param("attack", 2.0);
                    h.set_param("release", 100.0);
                    h.set_param("detector", if rms { 1.0 } else { 0.0 });
                    let level = f64::from(threshold) + over;
                    let amp = amp_of_db(level);
                    // 0.35 s of settling, then 4800 samples = 100 whole 1 kHz cycles (a window
                    // starting on a multiple of 48 spans whole cycles).
                    let start = 17_088;
                    let batches = (start + 4_800) / BATCH + 1;
                    let y = run_signal(&mut h, &IN, sine(1000.0, amp), batches, &["out_l"]);
                    let got =
                        20.0 * (measure(&y[0][start..start + 4_800], 1000.0, start) / amp).log10();
                    let r = if limit >= 0.5 { f32::INFINITY } else { ratio };
                    let want = -f64::from(static_gain_reduction_knee_db(
                        level as f32,
                        threshold,
                        r,
                        knee,
                    ));
                    let err = got - want;
                    worst = worst.max(err.abs());
                    assert!(
                        err.abs() < 0.1,
                        "rms={rms} T={threshold} R={ratio} lim={limit} knee={knee} level={level}: got {got:.3} want {want:.3}"
                    );
                }
            }
        }
    }
    println!("static curve (rms={rms}): worst error {worst:.4} dB");
}

#[test]
fn static_curve_matches_analytic_soft_knee_peak() {
    check_static_curve(false);
}

#[test]
fn static_curve_matches_analytic_soft_knee_rms() {
    check_static_curve(true);
}

/// GR (dB) per sample for a 1 kHz sine whose amplitude is `amp(n)`.
fn gr_trace(h: &mut DeviceHarness, amp: impl Fn(usize) -> f64, samples: usize) -> Vec<f64> {
    let batches = samples.div_ceil(BATCH);
    let y = run_signal(
        h,
        &IN,
        |n| (amp(n) * (std::f64::consts::TAU * 1000.0 * n as f64 / SR).sin()) as f32,
        batches,
        &["gr_cv_out"],
    );
    y[0].iter().map(|v| gr_db(*v)).collect()
}

/// First sample index at or after `from` where the trace reaches `level` (rising or falling).
fn crossing(tr: &[f64], from: usize, level: f64, rising: bool) -> usize {
    (from..tr.len())
        .find(|&i| {
            if rising {
                tr[i] >= level
            } else {
                tr[i] <= level
            }
        })
        .expect("no crossing")
}

#[test]
fn attack_and_release_time_constants_within_ten_percent() {
    // (attack ms, release ms)
    for (att, rel) in [(5.0f32, 100.0f32), (20.0, 300.0), (50.0, 800.0)] {
        let mut h = comp();
        h.set_param("threshold", -30.0);
        h.set_param("ratio", 8.0);
        h.set_param("knee", 0.0);
        h.set_param("attack", att);
        h.set_param("release", rel);
        let loud = amp_of_db(-10.0); // 20 dB over: GR = 17.5 dB
        let quiet = amp_of_db(-90.0);
        let t_loud = (SR * (f64::from(att) * 0.001 * 8.0 + 0.2)) as usize;
        let t_total = t_loud + (SR * (f64::from(rel) * 0.001 * 6.0)) as usize;
        let tr = gr_trace(&mut h, |n| if n < t_loud { loud } else { quiet }, t_total);
        let gr_ss = tr[t_loud - 1];
        assert!((gr_ss - 17.5).abs() < 0.1, "steady GR {gr_ss}");
        // Attack: 63.2 % of the final GR.
        let a = crossing(&tr, 0, gr_ss * (1.0 - (-1.0f64).exp()), true);
        let a_ms = a as f64 / SR * 1000.0;
        // Release: falls to 36.8 % of its starting value.
        let r = crossing(&tr, t_loud, gr_ss * (-1.0f64).exp(), false) - t_loud;
        let r_ms = r as f64 / SR * 1000.0;
        println!("attack {att} ms -> {a_ms:.2} ms; release {rel} ms -> {r_ms:.2} ms");
        assert!(
            (a_ms / f64::from(att) - 1.0).abs() < 0.1,
            "attack {att}: measured {a_ms}"
        );
        assert!(
            (r_ms / f64::from(rel) - 1.0).abs() < 0.1,
            "release {rel}: measured {r_ms}"
        );
    }
}

#[test]
fn rms_detector_attack_is_close_too() {
    // The RMS window adds its own 10 ms smoothing to the attack; with a 100 ms attack the
    // measured constant stays within 12 %.
    let mut h = comp();
    h.set_param("threshold", -30.0);
    h.set_param("ratio", 8.0);
    h.set_param("knee", 0.0);
    h.set_param("attack", 100.0);
    h.set_param("release", 400.0);
    h.set_param("detector", 1.0);
    let loud = amp_of_db(-10.0);
    let tr = gr_trace(&mut h, |_| loud, (SR * 1.5) as usize);
    let gr_ss = *tr.last().unwrap();
    assert!((gr_ss - 17.5).abs() < 0.15, "{gr_ss}");
    let a = crossing(&tr, 0, gr_ss * (1.0 - (-1.0f64).exp()), true);
    let a_ms = a as f64 / SR * 1000.0;
    println!("rms attack 100 ms -> {a_ms:.2} ms");
    assert!((a_ms / 100.0 - 1.0).abs() < 0.12, "{a_ms}");
}

/// Final GR (dB) for a quiet 1 kHz main input and a loud 100 Hz sidechain, with `sc_enable` set
/// and the sidechain sockets connected or not.
fn keyed_gr(enable: bool, connected: bool) -> f64 {
    let mut h = comp();
    h.set_param("threshold", -20.0);
    h.set_param("ratio", 10.0);
    h.set_param("knee", 0.0);
    h.set_param("attack", 1.0);
    h.set_param("release", 50.0);
    h.set_param("sc_enable", if enable { 1.0 } else { 0.0 });
    let main = sine(1000.0, amp_of_db(-40.0));
    let key = sine(100.0, amp_of_db(0.0));
    final_gr(
        &mut h,
        main,
        connected.then_some(&key as &dyn Fn(usize) -> f32),
        200,
    )
}

#[test]
fn sidechain_keys_the_gain_reduction() {
    let keyed = keyed_gr(true, true);
    println!("keyed GR {keyed:.2} dB");
    // 20 dB over the threshold at ratio 10 (soft knee 0): 18 dB.
    assert!((keyed - 18.0).abs() < 1.0, "keyed: {keyed}");
    assert!(keyed_gr(false, true) < 0.01, "sc disabled -> main detects");
    assert!(
        keyed_gr(true, false) < 0.01,
        "sc unconnected -> main detects"
    );
}

#[test]
fn a_lone_connected_sidechain_side_keys_both_channels() {
    let mut h = comp();
    h.set_param("threshold", -20.0);
    h.set_param("ratio", 10.0);
    h.set_param("knee", 0.0);
    h.set_param("attack", 1.0);
    h.set_param("sc_enable", 1.0);
    let main = sine(1000.0, amp_of_db(-40.0));
    let key = sine(100.0, 1.0);
    let mut out_r = [0.0f32; BATCH];
    for b in 0..200 {
        let mut x = [0.0f32; BATCH];
        let mut k = [0.0f32; BATCH];
        for i in 0..BATCH {
            x[i] = main(b * BATCH + i);
            k[i] = key(b * BATCH + i);
        }
        *h.input_mut("in_l") = x;
        *h.input_mut("in_r") = x;
        *h.input_mut("sc_in_l") = k; // only the left side is cabled
        h.process();
        out_r = *h.output("out_r");
    }
    let gr = gr_db(h.output("gr_cv_out")[BATCH - 1]);
    assert!(gr > 12.0, "{gr}");
    assert!(out_r.iter().all(|v| v.is_finite()));
}

#[test]
fn sidechain_filters_shape_the_key() {
    let gr_with = |hp: f32, lp: f32, hz: f64| {
        let mut h = comp();
        h.set_param("threshold", -20.0);
        h.set_param("ratio", 10.0);
        h.set_param("knee", 0.0);
        h.set_param("attack", 1.0);
        h.set_param("release", 50.0);
        h.set_param("sc_enable", 1.0);
        h.set_param("sc_hp", hp);
        h.set_param("sc_lp", lp);
        let quiet = sine(1000.0, amp_of_db(-60.0));
        let key = sine(hz, amp_of_db(-6.0));
        final_gr(&mut h, quiet, Some(&key), 300)
    };
    // A 50 Hz key is ~40 dB down through a 500 Hz high-pass: no reduction; open filter: heavy.
    assert!(gr_with(20.0, 20_000.0, 50.0) > 10.0);
    assert!(gr_with(500.0, 20_000.0, 50.0) < 0.5);
    // A 10 kHz key through a 1 kHz low-pass.
    assert!(gr_with(20.0, 20_000.0, 10_000.0) > 10.0);
    assert!(gr_with(20.0, 1_000.0, 10_000.0) < 0.5);
    // A 1 kHz key in the pass band of both still compresses.
    assert!(gr_with(100.0, 5_000.0, 1_000.0) > 10.0);
}

#[test]
fn lookahead_latency_equals_the_impulse_delay() {
    for sr in [44_100.0f64, 48_000.0, 96_000.0] {
        for ms in [0.0f32, 1.0, 2.5, 5.0] {
            let mut h = comp_at(sr);
            h.set_param("threshold", 0.0); // never compresses a small impulse
            h.set_param("lookahead", ms);
            let want = (f64::from(ms) * 0.001 * sr).round() as usize;
            assert_eq!(h.device().latency_frames() as usize, want, "{sr} {ms}");
            let mut x = vec![0.0f32; 1024];
            x[5] = 0.5;
            let mut out = Vec::new();
            for chunk in x.chunks(BATCH) {
                let mut b = [0.0f32; BATCH];
                b.copy_from_slice(chunk);
                *h.input_mut("in_l") = b;
                *h.input_mut("in_r") = b;
                h.process();
                out.extend_from_slice(h.output("out_l"));
            }
            let at = out.iter().position(|v| v.abs() > 0.1).unwrap();
            assert_eq!(at, 5 + want, "sr {sr} lookahead {ms}");
            assert_eq!(out.iter().filter(|v| v.abs() > 1e-6).count(), 1);
            assert_eq!(
                out[at].to_bits(),
                0.5f32.to_bits(),
                "unity gain is bit exact"
            );
        }
    }
}

#[test]
fn latency_follows_the_lookahead_param_after_prepare() {
    let mut h = comp();
    assert_eq!(h.device().latency_frames(), 0);
    h.set_param("lookahead", 5.0);
    assert_eq!(h.device().latency_frames(), 240);
    h.set_param("lookahead", 0.0);
    assert_eq!(h.device().latency_frames(), 0);
}

#[test]
fn lookahead_lets_the_gain_arrive_before_the_peak() {
    let peak_after_onset = |lookahead: f32| {
        let mut h = comp();
        h.set_param("threshold", -20.0);
        h.set_param("limit", 1.0);
        h.set_param("knee", 0.0);
        h.set_param("attack", 3.0);
        h.set_param("lookahead", lookahead);
        let delay = (f64::from(lookahead) * 0.001 * SR).round() as usize;
        let onset = 4_800;
        let burst = sine(2000.0, 1.0);
        let y = run_signal(
            &mut h,
            &IN,
            |n| if n >= onset { burst(n) } else { 0.0 },
            (onset + 2_400) / BATCH + 1,
            &["out_l"],
        );
        // Largest output sample in the first 2 ms of the (delayed) burst.
        let from = onset + delay;
        y[0][from..from + 96]
            .iter()
            .fold(0.0f32, |m, v| m.max(v.abs()))
    };
    let without = peak_after_onset(0.0);
    let with = peak_after_onset(5.0);
    println!("onset peak: no lookahead {without:.3}, 5 ms lookahead {with:.3}");
    assert!(
        without > 0.7,
        "without lookahead the transient passes: {without}"
    );
    assert!(with < without * 0.5, "with lookahead it is caught: {with}");
}

#[test]
fn mix_zero_is_the_delayed_input_bit_exact() {
    for ms in [0.0f32, 2.0] {
        let mut h = comp();
        h.set_param("threshold", -50.0);
        h.set_param("mix", 0.0);
        h.set_param("makeup", 12.0);
        h.set_param("lookahead", ms);
        let delay = (f64::from(ms) * 0.001 * SR).round() as usize;
        let mut rng = Rng::new(3);
        let x: Vec<f32> = (0..BATCH * 40).map(|_| rng.next_bipolar() * 0.8).collect();
        let mut out = Vec::new();
        for chunk in x.chunks(BATCH) {
            let mut b = [0.0f32; BATCH];
            b.copy_from_slice(chunk);
            *h.input_mut("in_l") = b;
            *h.input_mut("in_r") = b;
            h.process();
            out.extend_from_slice(h.output("out_l"));
        }
        for i in delay..out.len() {
            assert_eq!(
                out[i].to_bits(),
                x[i - delay].to_bits(),
                "lookahead {ms} i {i}"
            );
        }
    }
}

#[test]
fn parallel_mix_blends_dry_and_compressed() {
    let mut full = comp();
    let mut half = comp();
    for h in [&mut full, &mut half] {
        h.set_param("threshold", -30.0);
        h.set_param("ratio", 8.0);
        h.set_param("attack", 1.0);
    }
    half.set_param("mix", 0.5);
    let x = sine(500.0, 0.5);
    let a = run_signal(&mut full, &IN, &x, 400, &["out_l"]);
    let b = run_signal(&mut half, &IN, &x, 400, &["out_l"]);
    let start = 400 * BATCH - 2_000; // after the mix smoother has settled
    for i in start..400 * BATCH {
        let want = 0.5 * x(i) + 0.5 * a[0][i];
        assert!(
            (b[0][i] - want).abs() < 1e-5,
            "i {i}: {} vs {want}",
            b[0][i]
        );
    }
}

#[test]
fn stereo_link_shares_or_separates_the_reduction() {
    let quiet_r_gain = |link: f32| {
        let mut h = comp();
        h.set_param("threshold", -20.0);
        h.set_param("ratio", 10.0);
        h.set_param("knee", 0.0);
        h.set_param("attack", 1.0);
        h.set_param("stereo_link", link);
        let (l, r) = (sine(1000.0, 1.0), sine(1000.0, amp_of_db(-40.0)));
        let mut out_r = Vec::new();
        for b in 0..300 {
            let mut bl = [0.0f32; BATCH];
            let mut br = [0.0f32; BATCH];
            for i in 0..BATCH {
                bl[i] = l(b * BATCH + i);
                br[i] = r(b * BATCH + i);
            }
            *h.input_mut("in_l") = bl;
            *h.input_mut("in_r") = br;
            h.process();
            out_r.extend_from_slice(h.output("out_r"));
        }
        let n = out_r.len();
        let amp = measure(&out_r[n - 4_800..], 1000.0, n - 4_800);
        20.0 * (amp / amp_of_db(-40.0)).log10()
    };
    let linked = quiet_r_gain(1.0);
    let unlinked = quiet_r_gain(0.0);
    println!("quiet R channel: linked {linked:.2} dB, unlinked {unlinked:.2} dB");
    assert!(linked < -12.0, "linked R is reduced with L: {linked}");
    assert!(unlinked > -0.05, "unlinked R is untouched: {unlinked}");
}

#[test]
fn cv_out_is_one_minus_the_gain() {
    let mut h = comp();
    h.set_param("threshold", -20.0);
    h.set_param("ratio", 4.0);
    h.set_param("knee", 0.0);
    h.set_param("attack", 1.0);
    *h.input_mut("in_l") = [0.5; BATCH];
    *h.input_mut("in_r") = [0.5; BATCH];
    for _ in 0..300 {
        h.process();
    }
    let cv = h.output("gr_cv_out")[BATCH - 1];
    let g = h.output("out_l")[BATCH - 1] / 0.5;
    assert!((1.0 - cv - g).abs() < 1e-6, "cv {cv} gain {g}");
    // 0.5 is -6.02 dB: GR = (20 - 6.02) * 0.75 = 10.4845 dB.
    assert!((gr_db(cv) - 10.4845).abs() < 0.005, "{}", gr_db(cv));
    // At rest the CV is exactly 0.
    let mut rest = comp();
    *rest.input_mut("in_l") = [0.001; BATCH];
    rest.process();
    assert!(rest.output("gr_cv_out").iter().all(|v| v.to_bits() == 0));
}

#[test]
fn makeup_and_auto_makeup() {
    // A signal below the threshold sees only the makeup.
    let gain = |manual: f32, auto: f32| {
        let mut h = comp();
        h.set_param("threshold", -20.0);
        h.set_param("ratio", 4.0);
        h.set_param("knee", 0.0);
        h.set_param("makeup", manual);
        h.set_param("auto_makeup", auto);
        let x = sine(1000.0, amp_of_db(-60.0));
        let y = run_signal(&mut h, &IN, &x, 400, &["out_l"]);
        let n = y[0].len();
        20.0 * (measure(&y[0][n - 4_800..], 1000.0, n - 4_800) / amp_of_db(-60.0)).log10()
    };
    assert!((gain(6.0, 0.0) - 6.0).abs() < 0.01);
    // Auto: GR of a 0 dB input = 20 * 0.75 = 15 dB.
    assert!((gain(0.0, 1.0) - 15.0).abs() < 0.01);
    assert!((gain(3.0, 1.0) - 18.0).abs() < 0.01);
}

#[test]
fn limit_holds_the_threshold() {
    let mut h = comp();
    h.set_param("threshold", -12.0);
    h.set_param("limit", 1.0);
    h.set_param("knee", 0.0);
    h.set_param("attack", 0.5);
    let y = run_signal(&mut h, &IN, sine(1000.0, amp_of_db(6.0)), 400, &["out_l"]);
    let n = y[0].len();
    let lvl = 20.0 * measure(&y[0][n - 4_800..], 1000.0, n - 4_800).log10();
    println!("limit: output level {lvl:.3} dB for a threshold of -12 dB");
    assert!((lvl + 12.0).abs() < 0.15, "{lvl}");
}

/// Time (ms) for the GR to fall to half its peak after a burst of `burst_ms`.
fn release_half_time(burst_ms: f64, auto: bool) -> f64 {
    let mut h = comp();
    h.set_param("threshold", -30.0);
    h.set_param("ratio", 8.0);
    h.set_param("knee", 0.0);
    h.set_param("attack", 1.0);
    h.set_param("release", 800.0);
    h.set_param("auto_release", if auto { 1.0 } else { 0.0 });
    let (loud, quiet) = (amp_of_db(-10.0), amp_of_db(-90.0));
    let t_loud = (SR * burst_ms * 0.001) as usize;
    let tr = gr_trace(
        &mut h,
        |n| if n < t_loud { loud } else { quiet },
        t_loud + (SR * 4.0) as usize,
    );
    let peak = tr[..t_loud].iter().fold(0.0f64, |m, v| m.max(*v));
    (crossing(&tr, t_loud, peak * 0.5, false) - t_loud) as f64 / SR * 1000.0
}

#[test]
fn auto_release_is_program_dependent() {
    let short = release_half_time(30.0, true);
    let long = release_half_time(2_000.0, true);
    println!("auto release half-time: after 30 ms {short:.0} ms, after 2 s {long:.0} ms");
    assert!(long > 3.0 * short, "short {short} long {long}");
    // A fixed release is the same regardless of the history (after the attack has finished).
    let (f1, f2) = (
        release_half_time(300.0, false),
        release_half_time(2_000.0, false),
    );
    assert!((f1 / f2 - 1.0).abs() < 0.05, "fixed release {f1} vs {f2}");
}

#[test]
fn meter_values_report_levels_and_gain_reduction() {
    let mut h = comp();
    assert!(h.device().has_meter_values());
    h.set_param("threshold", -20.0);
    h.set_param("ratio", 4.0);
    h.set_param("knee", 0.0);
    h.set_param("attack", 1.0);
    *h.input_mut("in_l") = [0.5; BATCH];
    *h.input_mut("in_r") = [0.25; BATCH];
    for _ in 0..300 {
        h.process();
    }
    let mut out = [0.0f32; 32];
    let n = h.device().meter_values(&mut out);
    assert_eq!(n, 3);
    assert!((out[0] - 0.5).abs() < 1e-6);
    assert!(out[1] < out[0] && out[1] > 0.0);
    assert!((out[2] - 10.5).abs() < 0.05, "{}", out[2]);
    let mut small = [0.0f32; 2];
    assert_eq!(h.device().meter_values(&mut small), 2);
}

#[test]
fn non_finite_input_is_harmless() {
    let mut h = comp();
    let mut bad = [0.1f32; BATCH];
    bad[3] = f32::NAN;
    bad[9] = f32::INFINITY;
    *h.input_mut("in_l") = bad;
    *h.input_mut("in_r") = bad;
    h.process();
    *h.input_mut("in_l") = [0.1; BATCH];
    *h.input_mut("in_r") = [0.1; BATCH];
    let o = h.render(50, &["out_l", "out_r", "gr_cv_out"]);
    assert!(o.iter().flatten().all(|v| v.is_finite()));
}

#[test]
fn is_deterministic_across_instances_and_resets() {
    let run = |h: &mut DeviceHarness| {
        let mut rng = Rng::new(21);
        let mut all = Vec::new();
        for b in 0..120 {
            let mut x = [0.0f32; BATCH];
            for v in &mut x {
                *v = rng.next_bipolar() * 0.7;
            }
            if b == 40 {
                h.set_param("ratio", 12.0);
                h.set_param("lookahead", 3.0);
                h.set_param("auto_release", 1.0);
            }
            *h.input_mut("in_l") = x;
            *h.input_mut("in_r") = x;
            h.process();
            all.extend_from_slice(h.output("out_l"));
            all.extend_from_slice(h.output("gr_cv_out"));
        }
        all
    };
    let mut a = comp();
    let mut b = comp();
    let ya = run(&mut a);
    let yb = run(&mut b);
    assert!(ya.iter().zip(&yb).all(|(p, q)| p.to_bits() == q.to_bits()));
    a.device_mut().reset();
    a.set_param("ratio", 4.0);
    a.set_param("lookahead", 0.0);
    a.set_param("auto_release", 0.0);
    let yc = run(&mut a);
    assert!(ya.iter().zip(&yc).all(|(p, q)| p.to_bits() == q.to_bits()));
}

#[test]
fn params_sweep_stays_finite_at_all_rates() {
    for sr in [44_100.0, 48_000.0, 96_000.0] {
        let mut h = comp_at(sr);
        *h.input_mut("in_l") = [0.4; BATCH];
        *h.input_mut("in_r") = [-0.4; BATCH];
        sweep_params_min_max(&mut h, &DESCRIPTOR);
    }
}

#[test]
fn process_is_allocation_free_at_all_rates_with_automation() {
    for sr in [44_100.0, 48_000.0, 96_000.0] {
        let mut h = comp_at(sr);
        *h.input_mut("in_l") = [0.3; BATCH];
        *h.input_mut("in_r") = [-0.3; BATCH];
        *h.input_mut("sc_in_l") = [0.9; BATCH];
        *h.input_mut("sc_in_r") = [0.9; BATCH];
        for i in 0..300 {
            h.set_param("threshold", -((i % 40) as f32));
            h.set_param("ratio", 1.0 + (i % 19) as f32);
            h.set_param("lookahead", (i % 6) as f32);
            h.set_param("sc_enable", (i / 50 % 2) as f32);
            h.set_param("sc_hp", 20.0 + (i % 9) as f32 * 50.0);
            h.set_param("auto_release", (i / 30 % 2) as f32);
            h.set_param("detector", (i / 20 % 2) as f32);
            h.process(); // guarded by the harness
        }
        h.device_mut().reset();
        h.process();
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    #[test]
    fn output_is_finite_and_bounded_for_any_params(
        threshold in -60.0f32..0.0,
        ratio in 1.0f32..20.0,
        limit in proptest::bool::ANY,
        knee in 0.0f32..24.0,
        attack in 0.1f32..200.0,
        release in 5.0f32..2000.0,
        auto_release in proptest::bool::ANY,
        lookahead in 0.0f32..5.0,
        makeup in 0.0f32..24.0,
        auto_makeup in proptest::bool::ANY,
        mix in 0.0f32..1.0,
        sc_enable in proptest::bool::ANY,
        sc_hp in 20.0f32..1000.0,
        sc_lp in 1000.0f32..20000.0,
        link in 0.0f32..1.0,
        rms in proptest::bool::ANY,
        seed in 0u64..100,
    ) {
        let mut h = comp();
        for (k, v) in [
            ("threshold", threshold), ("ratio", ratio), ("limit", f32::from(limit)),
            ("knee", knee), ("attack", attack), ("release", release),
            ("auto_release", f32::from(auto_release)), ("lookahead", lookahead),
            ("makeup", makeup), ("auto_makeup", f32::from(auto_makeup)), ("mix", mix),
            ("sc_enable", f32::from(sc_enable)), ("sc_hp", sc_hp), ("sc_lp", sc_lp),
            ("stereo_link", link), ("detector", f32::from(rms)),
        ] {
            h.set_param(k, v);
        }
        let mut rng = Rng::new(seed);
        let mut worst = 0.0f32;
        for _ in 0..40 {
            let mut l = [0.0f32; BATCH];
            let mut r = [0.0f32; BATCH];
            for i in 0..BATCH {
                l[i] = rng.next_bipolar();
                r[i] = rng.next_bipolar() * 0.3;
            }
            *h.input_mut("in_l") = l;
            *h.input_mut("in_r") = r;
            *h.input_mut("sc_in_l") = r;
            *h.input_mut("sc_in_r") = l;
            h.process();
            for k in ["out_l", "out_r", "gr_cv_out"] {
                for v in h.output(k) {
                    prop_assert!(v.is_finite());
                    worst = worst.max(v.abs());
                }
            }
        }
        // Input peak 1, makeup at most 36 dB (+ a rounding margin).
        prop_assert!(worst <= 64.0, "{}", worst);
    }
}
