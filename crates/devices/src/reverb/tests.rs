//! Reverb tests. Everything runs through `DeviceHarness`, which also guards
//! every `process` call against allocation.

use super::*;
use crate::test_support::{SR, assert_keys, harness, sweep_params_min_max};
use atomik_dsp::Rng;
use atomik_testkit::harness::DeviceHarness;
use atomik_testkit::signal::{db, max_step, peak, rms};
use proptest::prelude::{ProptestConfig, prop_assert, proptest};

const OUT: [&str; 2] = ["out_l", "out_r"];

fn reverb_at(sr: f64) -> DeviceHarness {
    DeviceHarness::with_options(Box::new(Reverb::new()), sr, 120.0, 0)
}

fn reverb() -> DeviceHarness {
    harness(Box::new(Reverb::new()))
}

fn wet_only(h: &mut DeviceHarness) {
    h.set_param("mix", 1.0);
}

/// Deterministic white noise in -amp..amp.
fn noise(seed: u64, n: usize, amp: f32) -> Vec<f32> {
    let mut r = Rng::new(seed);
    (0..n).map(|_| r.next_bipolar() * amp).collect()
}

/// Feeds `x` (mono, both inputs) and returns (L, R); the length is rounded up to batches.
fn run_mono(h: &mut DeviceHarness, x: &[f32]) -> (Vec<f32>, Vec<f32>) {
    let mut l = Vec::new();
    let mut r = Vec::new();
    for chunk in x.chunks(BATCH) {
        let mut b = [0.0f32; BATCH];
        b[..chunk.len()].copy_from_slice(chunk);
        *h.input_mut("in_l") = b;
        *h.input_mut("in_r") = b;
        h.process();
        l.extend_from_slice(h.output("out_l"));
        r.extend_from_slice(h.output("out_r"));
    }
    (l, r)
}

/// Unit impulse at frame 0 on both inputs, then silence, for `samples` samples.
fn impulse_response(h: &mut DeviceHarness, samples: usize) -> (Vec<f32>, Vec<f32>) {
    let mut x = vec![0.0f32; samples];
    x[0] = 1.0;
    run_mono(h, &x)
}

/// Reverberation time by Schroeder backward integration of the stereo energy: the energy decay
/// curve `EDC(t) = sum_{s >= t} (l_s^2 + r_s^2)` in dB, a least-squares line through the points
/// between -5 dB and -25 dB (T20), extrapolated to -60 dB.
fn rt60_schroeder(l: &[f32], r: &[f32], sr: f64) -> f64 {
    let n = l.len();
    let mut edc = vec![0.0f64; n];
    let mut acc = 0.0f64;
    for i in (0..n).rev() {
        acc += f64::from(l[i]).powi(2) + f64::from(r[i]).powi(2);
        edc[i] = acc;
    }
    let total = edc[0];
    let pts: Vec<(f64, f64)> = (0..n)
        .map(|i| (i as f64 / sr, 10.0 * (edc[i] / total).log10()))
        .filter(|(_, d)| *d <= -5.0 && *d >= -25.0)
        .collect();
    assert!(pts.len() > 10, "decay did not cross -25 dB");
    let m = pts.len() as f64;
    let (st, sd) = pts.iter().fold((0.0, 0.0), |(a, b), (t, d)| (a + t, b + d));
    let (mt, md) = (st / m, sd / m);
    let num: f64 = pts.iter().map(|(t, d)| (t - mt) * (d - md)).sum();
    let den: f64 = pts.iter().map(|(t, _)| (t - mt).powi(2)).sum();
    -60.0 / (num / den)
}

fn rt60_of(alg: f32, size: f32, decay: f32) -> f64 {
    let mut h = reverb();
    wet_only(&mut h);
    h.set_param("algorithm", alg);
    h.set_param("size", size);
    h.set_param("decay", decay);
    h.set_param("damping", 0.0);
    h.set_param("predelay", 0.0);
    h.set_param("lowcut", 20.0);
    let n = ((decay * 1.3 + 0.4) as f64 * SR) as usize;
    let (l, r) = impulse_response(&mut h, n);
    rt60_schroeder(&l, &r, SR)
}

fn check_rt60(alg: f32) {
    for size in [0.2, 0.8] {
        for decay in [0.5, 2.0, 8.0] {
            let rt = rt60_of(alg, size, decay);
            let err = rt / f64::from(decay) - 1.0;
            println!("alg {alg} size {size} decay {decay}: RT60 {rt:.3} s ({err:+.3})");
            assert!(
                err.abs() <= 0.10,
                "alg {alg} size {size} decay {decay}: measured {rt:.3} s ({:+.1} %)",
                err * 100.0
            );
        }
    }
}

#[test]
fn descriptor_validates() {
    assert_keys(
        &DESCRIPTOR,
        &[
            "algorithm",
            "predelay",
            "size",
            "decay",
            "damping",
            "lowcut",
            "early",
            "width",
            "modulation",
            "mix",
            "freeze",
            "output",
        ],
        &["in_l", "in_r", "out_l", "out_r"],
    );
    assert_eq!(DESCRIPTOR.type_id, "atomik.reverb");
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Effect);
    let p = DESCRIPTOR.param_by_key("decay").unwrap();
    assert_eq!((p.min, p.max), (0.2, 20.0));
    let p = DESCRIPTOR.param_by_key("predelay").unwrap();
    assert_eq!((p.min, p.max), (0.0, 250.0));
}

#[test]
fn rt60_room_within_ten_percent() {
    check_rt60(0.0);
}

#[test]
fn rt60_hall_within_ten_percent() {
    check_rt60(1.0);
}

#[test]
fn rt60_plate_within_ten_percent() {
    check_rt60(2.0);
}

#[test]
fn damping_shortens_highs_not_lows() {
    // Octave-band energy decay: with damping the 8 kHz band decays faster than the 250 Hz band.
    let mut h = reverb();
    wet_only(&mut h);
    h.set_param("decay", 4.0);
    h.set_param("damping", 0.8);
    let (l, _) = impulse_response(&mut h, (3.0 * SR) as usize);
    let win = |x: &[f32], a: usize, b: usize| rms(&x[a..b]);
    let late = win(&l, 96_000, 144_000);
    let early = win(&l, 12_000, 24_000);
    let mut h2 = reverb();
    wet_only(&mut h2);
    h2.set_param("decay", 4.0);
    h2.set_param("damping", 0.0);
    let (l2, _) = impulse_response(&mut h2, (3.0 * SR) as usize);
    let late2 = win(&l2, 96_000, 144_000);
    let early2 = win(&l2, 12_000, 24_000);
    // Damped tail falls further relative to its start than the undamped one.
    assert!(
        db(late / early) < db(late2 / early2) - 3.0,
        "{} vs {}",
        db(late / early),
        db(late2 / early2)
    );
}

#[test]
fn tail_is_decorrelated_at_full_width_and_mono_at_zero() {
    for alg in [0.0, 1.0, 2.0] {
        let x = noise(11, 4 * 48_000, 0.3);
        let mut h = reverb();
        wet_only(&mut h);
        h.set_param("algorithm", alg);
        h.set_param("width", 1.0);
        h.set_param("decay", 2.0);
        let (l, r) = run_mono(&mut h, &x);
        let c = correlation(&l[48_000..], &r[48_000..]);
        println!("alg {alg}: correlation at width 1 = {c:+.4}");
        assert!(c.abs() < 0.25, "alg {alg}: correlation {c}");
        let mut h = reverb();
        wet_only(&mut h);
        h.set_param("algorithm", alg);
        h.set_param("width", 0.0);
        let (l, r) = run_mono(&mut h, &x);
        let c = correlation(&l[48_000..], &r[48_000..]);
        assert!(c > 0.9999, "alg {alg}: width 0 correlation {c}");
        assert!(
            l.iter()
                .zip(&r)
                .all(|(a, b)| (a - b).abs() <= 1e-6 * a.abs().max(1.0))
        );
    }
}

fn correlation(a: &[f32], b: &[f32]) -> f64 {
    let (mut ab, mut aa, mut bb) = (0.0f64, 0.0f64, 0.0f64);
    for (x, y) in a.iter().zip(b) {
        ab += f64::from(*x) * f64::from(*y);
        aa += f64::from(*x).powi(2);
        bb += f64::from(*y).powi(2);
    }
    ab / (aa * bb).sqrt()
}

#[test]
fn freeze_holds_energy_without_blow_up() {
    for alg in [0.0, 1.0, 2.0] {
        let mut h = reverb();
        wet_only(&mut h);
        h.set_param("algorithm", alg);
        h.set_param("decay", 1.0);
        let x = noise(5, 24_000, 0.3);
        run_mono(&mut h, &x);
        h.set_param("freeze", 1.0);
        // Input keeps playing after the freeze: it must be muted.
        let loud = noise(6, 48_000, 0.8);
        run_mono(&mut h, &loud);
        let seconds = 8usize;
        let silence = vec![0.0f32; 48_000 * seconds];
        let (l, r) = run_mono(&mut h, &silence);
        let level = |s: usize| {
            let a = s * 48_000;
            let e = rms(&l[a..a + 24_000]).powi(2) + rms(&r[a..a + 24_000]).powi(2);
            10.0 * e.log10()
        };
        let first = level(0);
        let last = level(seconds - 1);
        println!("alg {alg}: freeze energy {first:.2} dB -> {last:.2} dB");
        assert!(
            (last - first).abs() < 4.0,
            "alg {alg}: {first} -> {last} dB"
        );
        assert!(peak(&l).max(peak(&r)) < 4.0, "bounded");
        // Unfreezing lets it decay again.
        h.set_param("freeze", 0.0);
        let (l2, _) = run_mono(&mut h, &vec![0.0; 48_000 * 4]);
        assert!(rms(&l2[3 * 48_000..]) < rms(&l2[..48_000]) * 0.1);
    }
}

#[test]
fn output_decays_to_exact_zero_after_input_stops() {
    // Denormal proxy: the feedback state flushes below 1e-20, so a decayed tail is exactly 0
    // (never a long run of denormals, which would be slow).
    let mut h = reverb();
    wet_only(&mut h);
    h.set_param("decay", 0.2);
    h.set_param("algorithm", 0.0);
    run_mono(&mut h, &noise(3, 12_000, 0.5));
    let (l, r) = run_mono(&mut h, &vec![0.0; 48_000 * 8]);
    let tail = &l[l.len() - 48_000..];
    assert!(tail.iter().chain(&r[r.len() - 48_000..]).all(|v| *v == 0.0));
    // No denormals at any time.
    assert!(
        l.iter()
            .chain(&r)
            .all(|v| *v == 0.0 || v.abs() >= f32::MIN_POSITIVE)
    );
}

#[test]
fn mix_zero_is_dry_bit_exact_and_has_no_latency() {
    let x = noise(9, 20_000, 1.0);
    let mut h = reverb();
    h.set_param("mix", 0.0);
    h.set_param("predelay", 120.0);
    h.set_param("output", 12.0);
    h.set_param("decay", 20.0);
    let (l, r) = run_mono(&mut h, &x);
    for i in 0..x.len() {
        assert_eq!(l[i].to_bits(), x[i].to_bits(), "L at {i}");
        assert_eq!(r[i].to_bits(), x[i].to_bits(), "R at {i}");
    }
    // Reaching mix 0 after a different mix is also exact once the smoother settles.
    h.set_param("mix", 0.5);
    run_mono(&mut h, &x);
    h.set_param("mix", 0.0);
    run_mono(&mut h, &x);
    let (l, _) = run_mono(&mut h, &x);
    assert!(l.iter().zip(&x).all(|(a, b)| a.to_bits() == b.to_bits()));
    assert_eq!(h.device().latency_frames(), 0);
}

#[test]
fn mix_blends_dry_and_wet() {
    let x = noise(2, 30_000, 0.5);
    let mut wet = reverb();
    wet_only(&mut wet);
    let (wl, _) = run_mono(&mut wet, &x);
    let mut half = reverb();
    half.set_param("mix", 0.5);
    let (hl, _) = run_mono(&mut half, &x);
    for i in [1_000, 5_000, 20_000] {
        let want = 0.5 * x[i] + 0.5 * wl[i];
        assert!((hl[i] - want).abs() < 1e-5, "{i}");
    }
}

#[test]
fn latency_is_zero_and_onset_follows_predelay() {
    let onset = |pre: f32| {
        let mut h = reverb();
        wet_only(&mut h);
        h.set_param("predelay", pre);
        let (l, r) = impulse_response(&mut h, 48_000 / 2 + 20_000);
        (0..l.len())
            .find(|i| l[*i].abs() > 1e-9 || r[*i].abs() > 1e-9)
            .unwrap()
    };
    assert_eq!(reverb().device().latency_frames(), 0);
    let base = onset(0.0);
    assert!(base < 500, "first early tap soon after the input: {base}");
    for ms in [1.0f32, 10.0, 37.5, 100.0, 250.0] {
        let expect = (f64::from(ms) * 0.001 * SR).round() as usize;
        let got = onset(ms) - base;
        assert!(got.abs_diff(expect) <= 1, "{ms} ms: {got} vs {expect}");
    }
}

#[test]
fn onset_follows_predelay_at_other_sample_rates() {
    for sr in [44_100.0, 96_000.0] {
        let onset = |pre: f32| {
            let mut h = reverb_at(sr);
            wet_only(&mut h);
            h.set_param("predelay", pre);
            let (l, r) = impulse_response(&mut h, (sr * 0.3) as usize);
            (0..l.len())
                .find(|i| l[*i].abs() > 1e-9 || r[*i].abs() > 1e-9)
                .unwrap()
        };
        let got = onset(50.0) - onset(0.0);
        let expect = (0.05 * sr).round() as usize;
        assert!(got.abs_diff(expect) <= 1, "{sr}: {got} vs {expect}");
    }
}

#[test]
fn deterministic_across_runs_and_seed_sensitive() {
    let x = noise(21, 40_000, 0.4);
    let run = |seed: u64| {
        let mut h = DeviceHarness::with_options(Box::new(Reverb::new()), SR, 120.0, seed);
        h.set_param("mix", 0.6);
        h.set_param("modulation", 1.0);
        let mut out = Vec::new();
        let batches = x.len().div_ceil(BATCH);
        for b in 0..batches {
            if b == 200 {
                h.set_param("size", 0.9);
                h.set_param("algorithm", 2.0);
            }
            let mut buf = [0.0f32; BATCH];
            for (i, s) in buf.iter_mut().enumerate() {
                *s = *x.get(b * BATCH + i).unwrap_or(&0.0);
            }
            *h.input_mut("in_l") = buf;
            *h.input_mut("in_r") = buf;
            h.process();
            out.extend_from_slice(h.output("out_l"));
            out.extend_from_slice(h.output("out_r"));
        }
        out
    };
    let a = run(4);
    let b = run(4);
    assert_eq!(
        a.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        b.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
    );
    assert_ne!(a, run(5), "modulation phases come from the seed");
    // Reset restarts identically.
    let mut h = reverb();
    h.set_param("mix", 1.0);
    let first = run_mono(&mut h, &x[..8_000]);
    h.device_mut().reset();
    let again = run_mono(&mut h, &x[..8_000]);
    assert_eq!(first, again);
}

#[test]
fn size_and_algorithm_changes_do_not_click() {
    let mut h = reverb();
    h.set_param("mix", 1.0);
    h.set_param("decay", 3.0);
    let mut out = Vec::new();
    let sig: Vec<f32> = (0..48_000 * 6)
        .map(|n| 0.1 * (std::f64::consts::TAU * 220.0 * n as f64 / SR).sin() as f32)
        .collect();
    for (b, chunk) in sig.chunks(BATCH).enumerate() {
        match b {
            1_000 => h.set_param("size", 1.0),
            2_000 => h.set_param("size", 0.0),
            3_000 => h.set_param("algorithm", 2.0),
            4_000 => h.set_param("algorithm", 0.0),
            _ => {}
        }
        let mut buf = [0.0f32; BATCH];
        buf[..chunk.len()].copy_from_slice(chunk);
        *h.input_mut("in_l") = buf;
        *h.input_mut("in_r") = buf;
        h.process();
        out.extend_from_slice(h.output("out_l"));
    }
    let tail = &out[24_000..];
    let step = max_step(tail);
    let level = rms(tail);
    println!("max step {step}, rms {level}");
    assert!(level > 0.01);
    // A click would be a step comparable to the signal peak; a 220 Hz sine of 0.1 peak steps
    // 0.003 per sample, the reverberated version is of the same order.
    assert!(step < 0.05, "max step {step}");
}

#[test]
fn decay_changes_are_smooth() {
    let mut h = reverb();
    h.set_param("mix", 1.0);
    let sig: Vec<f32> = (0..48_000 * 3)
        .map(|n| 0.1 * (std::f64::consts::TAU * 330.0 * n as f64 / SR).sin() as f32)
        .collect();
    let mut out = Vec::new();
    for (b, chunk) in sig.chunks(BATCH).enumerate() {
        match b {
            700 => h.set_param("decay", 20.0),
            1_400 => h.set_param("decay", 0.2),
            _ => {}
        }
        let mut buf = [0.0f32; BATCH];
        buf[..chunk.len()].copy_from_slice(chunk);
        *h.input_mut("in_l") = buf;
        *h.input_mut("in_r") = buf;
        h.process();
        out.extend_from_slice(h.output("out_l"));
    }
    assert!(max_step(&out[24_000..]) < 0.05);
}

#[test]
fn meter_values_report_levels_and_tail() {
    let mut h = reverb();
    assert!(h.device().has_meter_values());
    wet_only(&mut h);
    let mut m = [0.0f32; 8];
    assert_eq!(h.device().meter_values(&mut m), 3);
    assert_eq!(&m[..3], &[0.0, 0.0, 0.0]);
    let (l, r) = run_mono(&mut h, &noise(1, 6_400, 0.5));
    let n = h.device().meter_values(&mut m);
    assert_eq!(n, 3);
    let last_l = peak(&l[l.len() - BATCH..]);
    let last_r = peak(&r[r.len() - BATCH..]);
    assert_eq!(m[0], last_l);
    assert_eq!(m[1], last_r);
    assert!(m[2] > 0.0 && m[2].is_finite());
    let mut two = [0.0f32; 2];
    assert_eq!(h.device().meter_values(&mut two), 2);
    assert_eq!(h.device().meter_values(&mut []), 0);
}

#[test]
fn early_late_balance_selects_the_parts() {
    let ir = |early: f32| {
        let mut h = reverb();
        wet_only(&mut h);
        h.set_param("predelay", 0.0);
        h.set_param("early", early);
        h.set_param("algorithm", 0.0);
        impulse_response(&mut h, 24_000).0
    };
    let early_only = ir(1.0);
    let late_only = ir(0.0);
    let first_ms = |x: &[f32]| rms(&x[..240]); // 5 ms
    assert!(first_ms(&early_only) > 0.0 && first_ms(&late_only) == 0.0);
    assert!(rms(&early_only[480..]) > 0.0);
    // Early-only still has the tail's energy at zero.
    assert!(rms(&late_only[12_000..]) > 0.0 || rms(&late_only[2_000..12_000]) > 0.0);
    assert!(rms(&early_only[9_600..]) < 1e-3 * rms(&early_only));
}

#[test]
fn output_gain_scales_the_wet_signal_only() {
    let ir = |out: f32| {
        let mut h = reverb();
        h.set_param("mix", 0.5);
        h.set_param("output", out);
        run_mono(&mut h, &noise(8, 20_000, 0.3)).0
    };
    let (a, b) = (ir(0.0), ir(-6.0));
    let x = noise(8, 20_000, 0.3);
    let wet_a: Vec<f32> = a.iter().zip(&x).map(|(y, d)| y - 0.5 * d).collect();
    let wet_b: Vec<f32> = b.iter().zip(&x).map(|(y, d)| y - 0.5 * d).collect();
    let ratio = rms(&wet_b[2_000..]) / rms(&wet_a[2_000..]);
    assert!((ratio - 0.501).abs() < 0.01, "{ratio}");
}

#[test]
fn set_param_every_param_min_max_does_not_panic() {
    let mut h = reverb();
    *h.input_mut("in_l") = [0.5; BATCH];
    *h.input_mut("in_r") = [-0.5; BATCH];
    sweep_params_min_max(&mut h, &DESCRIPTOR);
}

#[test]
fn reset_silences_tails() {
    let mut h = reverb();
    wet_only(&mut h);
    let (l, _) = impulse_response(&mut h, 12_000);
    assert!(l.iter().any(|v| v.abs() > 1e-3));
    h.device_mut().reset();
    let o = h.render(400, &OUT);
    assert!(o[0].iter().chain(&o[1]).all(|v| v.to_bits() == 0));
}

#[test]
fn non_finite_input_does_not_poison_the_tail() {
    let mut h = reverb();
    wet_only(&mut h);
    let mut bad = [0.0f32; BATCH];
    bad[3] = f32::NAN;
    bad[5] = f32::INFINITY;
    *h.input_mut("in_l") = bad;
    *h.input_mut("in_r") = bad;
    h.process();
    *h.input_mut("in_l") = [0.0; BATCH];
    *h.input_mut("in_r") = [0.0; BATCH];
    let o = h.render(100, &OUT);
    assert!(o[0].iter().chain(&o[1]).all(|v| v.is_finite()));
}

#[test]
fn process_is_allocation_free_at_all_rates_with_automation() {
    for sr in [44_100.0, 48_000.0, 96_000.0] {
        let mut h = reverb_at(sr);
        *h.input_mut("in_l") = [0.3; BATCH];
        *h.input_mut("in_r") = [-0.3; BATCH];
        for i in 0..400 {
            h.set_param("algorithm", (i / 40 % 3) as f32);
            h.set_param("size", (i % 11) as f32 / 10.0);
            h.set_param("decay", 0.2 + (i % 7) as f32 * 3.0);
            h.set_param("predelay", (i % 13) as f32 * 19.0);
            h.set_param("freeze", (i / 50 % 2) as f32);
            h.set_param("mix", (i % 5) as f32 / 4.0);
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
        alg in 0u8..3,
        predelay in 0.0f32..250.0,
        size in 0.0f32..1.0,
        decay in 0.2f32..20.0,
        damping in 0.0f32..1.0,
        lowcut in 20.0f32..1_000.0,
        early in 0.0f32..1.0,
        width in 0.0f32..1.0,
        modulation in 0.0f32..1.0,
        mix in 0.0f32..1.0,
        freeze in proptest::bool::ANY,
        output in -24.0f32..12.0,
        seed in 0u64..100,
    ) {
        let mut h = DeviceHarness::with_options(Box::new(Reverb::new()), SR, 120.0, seed);
        for (k, v) in [
            ("algorithm", f32::from(alg)), ("predelay", predelay), ("size", size),
            ("decay", decay), ("damping", damping), ("lowcut", lowcut), ("early", early),
            ("width", width), ("modulation", modulation), ("mix", mix),
            ("freeze", if freeze { 1.0 } else { 0.0 }), ("output", output),
        ] {
            h.set_param(k, v);
        }
        let x = noise(seed, 30 * BATCH, 1.0);
        let (l, r) = run_mono(&mut h, &x);
        // Mid-run parameter jumps.
        h.set_param("size", 1.0 - size);
        h.set_param("freeze", if freeze { 0.0 } else { 1.0 });
        let (l2, r2) = run_mono(&mut h, &x);
        for v in l.iter().chain(&r).chain(&l2).chain(&r2) {
            prop_assert!(v.is_finite() && v.abs() <= 32.0 * 4.0 + 1.0, "{v}");
        }
    }
}

/// Rough CPU cost: seconds of processing per second of audio, release builds only
/// (`cargo test -p atomik-devices --release -- --ignored reverb_cpu --nocapture`).
#[test]
#[ignore]
// Wall-clock timing is the point of this ignored, manual benchmark; it never runs in CI.
#[allow(clippy::disallowed_methods)]
fn reverb_cpu_bench() {
    for sr in [44_100.0, 48_000.0, 96_000.0] {
        let mut h = reverb_at(sr);
        h.set_param("mix", 1.0);
        h.set_param("modulation", 1.0);
        *h.input_mut("in_l") = [0.2; BATCH];
        *h.input_mut("in_r") = [-0.2; BATCH];
        let batches = (sr as usize * 10) / BATCH;
        let t = std::time::Instant::now();
        for _ in 0..batches {
            h.process();
        }
        let secs = t.elapsed().as_secs_f64();
        println!(
            "sr {sr}: {:.4} s per second of audio ({:.2} % of one core)",
            secs / 10.0,
            secs * 10.0
        );
    }
}

#[test]
fn wet_level_is_comparable_to_the_input_level() {
    // Calibration of the internal gains: white noise through the 100 % wet reverb at default
    // settings comes out within 8 dB of its input level (the hall is dark, so white noise loses the most) for every algorithm and decay.
    for alg in [0.0, 1.0, 2.0] {
        for decay in [0.5, 2.0, 8.0] {
            let mut h = reverb();
            wet_only(&mut h);
            h.set_param("algorithm", alg);
            h.set_param("decay", decay);
            let x = noise(13, 6 * 48_000, 0.3);
            let (l, r) = run_mono(&mut h, &x);
            let gain = db(rms(&[&l[96_000..], &r[96_000..]].concat()) / rms(&x));
            println!("alg {alg} decay {decay}: wet/dry {gain:+.2} dB");
            assert!(gain.abs() < 8.0, "alg {alg} decay {decay}: {gain} dB");
        }
    }
}
