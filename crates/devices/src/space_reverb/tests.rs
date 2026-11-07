//! Space reverb tests (the shared engine is covered by the classic reverb's tests).

use super::*;
use crate::test_support::{SR, assert_keys, harness, sweep_params_min_max};
use atomik_core::BATCH;
use atomik_dsp::Rng;
use atomik_testkit::harness::DeviceHarness;
use atomik_testkit::signal::{peak, rms};
use proptest::prelude::{ProptestConfig, prop_assert, proptest};

fn space() -> DeviceHarness {
    harness(Box::new(SpaceReverb::new()))
}

fn noise(seed: u64, n: usize, amp: f32) -> Vec<f32> {
    let mut r = Rng::new(seed);
    (0..n).map(|_| r.next_bipolar() * amp).collect()
}

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

/// Reverberation time (T20 extrapolated to 60 dB) of the stereo energy decay curve.
fn rt60(l: &[f32], r: &[f32], sr: f64) -> f64 {
    let n = l.len();
    let mut edc = vec![0.0f64; n];
    let mut acc = 0.0f64;
    for i in (0..n).rev() {
        acc += f64::from(l[i]).powi(2) + f64::from(r[i]).powi(2);
        edc[i] = acc;
    }
    let pts: Vec<(f64, f64)> = (0..n)
        .map(|i| (i as f64 / sr, 10.0 * (edc[i] / edc[0]).log10()))
        .filter(|(_, d)| (-25.0..=-5.0).contains(d))
        .collect();
    assert!(pts.len() > 10, "decay did not cross -25 dB");
    let m = pts.len() as f64;
    let (st, sd) = pts.iter().fold((0.0, 0.0), |(a, b), (t, d)| (a + t, b + d));
    let (mt, md) = (st / m, sd / m);
    let num: f64 = pts.iter().map(|(t, d)| (t - mt) * (d - md)).sum();
    let den: f64 = pts.iter().map(|(t, _)| (t - mt).powi(2)).sum();
    -60.0 / (num / den)
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
            "mix_cv_trim",
            "highcut",
            "gate",
            "gate_level",
            "duck",
            "duck_release",
            "duck_mode",
            "duck_freq",
            "duck_threshold",
            "key_gate",
            "key_gate_hold",
            "bypass",
        ],
        &[
            "in_l",
            "in_r",
            "out_l",
            "out_r",
            "mix_cv_in",
            "sc_in_l",
            "sc_in_r",
        ],
    );
    assert_eq!(DESCRIPTOR.type_id, "atomik.space-reverb");
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Effect);
    assert_eq!(DESCRIPTOR.validate(), Ok(()));
}

#[test]
fn every_algorithm_decays_at_the_set_time() {
    for alg in 0..4 {
        for decay in [1.0f32, 4.0] {
            let mut h = space();
            h.set_param("mix", 1.0);
            h.set_param("algorithm", alg as f32);
            h.set_param("decay", decay);
            h.set_param("damping", 0.0);
            h.set_param("early", 0.0);
            h.set_param("predelay", 0.0);
            h.set_param("lowcut", 20.0);
            let mut x = vec![0.0f32; ((decay * 1.4 + 0.5) as f64 * SR) as usize];
            x[0] = 1.0;
            let (l, r) = run_mono(&mut h, &x);
            let rt = rt60(&l, &r, SR);
            let err = rt / f64::from(decay) - 1.0;
            assert!(err.abs() <= 0.15, "alg {alg} decay {decay}: {rt:.3} s");
        }
    }
}

#[test]
fn the_algorithms_sound_different() {
    let x = noise(3, 20_000, 0.5);
    let outs: Vec<Vec<f32>> = (0..4)
        .map(|alg| {
            let mut h = space();
            h.set_param("mix", 1.0);
            h.set_param("algorithm", alg as f32);
            run_mono(&mut h, &x).0
        })
        .collect();
    for a in 0..4 {
        for b in a + 1..4 {
            assert_ne!(outs[a], outs[b], "{a} and {b}");
        }
    }
}

#[test]
fn mix_zero_is_dry_bit_exact_with_no_latency() {
    let x = noise(9, 20_000, 1.0);
    let mut h = space();
    h.set_param("mix", 0.0);
    h.set_param("gate", 20.0);
    h.set_param("highcut", 2_000.0);
    let (l, r) = run_mono(&mut h, &x);
    assert!(l.iter().zip(&x).all(|(a, b)| a.to_bits() == b.to_bits()));
    assert!(r.iter().zip(&x).all(|(a, b)| a.to_bits() == b.to_bits()));
    assert_eq!(h.device().latency_frames(), 0);
}

#[test]
fn the_high_cut_darkens_the_wet_signal() {
    let tone: Vec<f32> = (0..30_000)
        .map(|i| 0.4 * (std::f32::consts::TAU * 8_000.0 * i as f32 / SR as f32).sin())
        .collect();
    let level = |cut: f32| {
        let mut h = space();
        h.set_param("mix", 1.0);
        h.set_param("highcut", cut);
        h.set_param("damping", 0.0);
        let (l, _) = run_mono(&mut h, &tone);
        rms(&l[10_000..])
    };
    let open = level(20_000.0);
    let cut = level(2_000.0);
    assert!(cut < open * 0.3, "{cut} vs {open}");
}

/// A 100 ms burst of noise, then silence; the wet level 300 ms after the burst.
fn gated_tail_level(hold_ms: f32) -> (f32, f32) {
    let mut h = space();
    h.set_param("mix", 1.0);
    h.set_param("decay", 6.0);
    h.set_param("gate", hold_ms);
    h.set_param("gate_level", -40.0);
    let burst = (0.1 * SR) as usize;
    let mut x = noise(5, burst, 0.5);
    x.resize((0.6 * SR) as usize, 0.0);
    let (l, _) = run_mono(&mut h, &x);
    let at = |ms: f64| (ms * 0.001 * SR) as usize;
    (rms(&l[at(40.0)..at(90.0)]), rms(&l[at(400.0)..at(500.0)]))
}

#[test]
fn the_gate_cuts_the_tail_after_the_hold() {
    let (open_in, open_tail) = gated_tail_level(0.0);
    let (gated_in, gated_tail) = gated_tail_level(40.0);
    assert!(open_tail > 0.01, "the ungated tail rings: {open_tail}");
    assert!(
        gated_in > open_in * 0.5,
        "the gate is open while the input plays"
    );
    assert!(gated_tail < open_tail * 0.01, "{gated_tail} vs {open_tail}");
}

#[test]
fn the_gate_opens_again_for_the_next_hit() {
    let mut h = space();
    h.set_param("mix", 1.0);
    h.set_param("gate", 30.0);
    let mut x = noise(6, 4_800, 0.5);
    x.resize(48_000, 0.0);
    x.extend(noise(7, 4_800, 0.5));
    x.resize(60_000, 0.0);
    let (l, _) = run_mono(&mut h, &x);
    assert!(peak(&l[48_000..48_000 + 2_000]) > 0.01);
}

#[test]
fn deterministic_and_allocation_free_under_automation() {
    let x = noise(21, 30_000, 0.4);
    let run = || {
        let mut h = DeviceHarness::with_options(Box::new(SpaceReverb::new()), SR, 120.0, 4);
        h.set_param("mix", 0.6);
        let mut out = Vec::new();
        for (b, chunk) in x.chunks(BATCH).enumerate() {
            if b % 40 == 0 {
                h.set_param("algorithm", (b / 40 % 4) as f32);
                h.set_param("size", (b % 7) as f32 / 6.0);
                h.set_param("gate", (b % 3) as f32 * 40.0);
                h.set_param("highcut", 2_000.0 + (b % 5) as f32 * 3_000.0);
                h.set_param("freeze", (b / 120 % 2) as f32);
            }
            let mut buf = [0.0f32; BATCH];
            buf[..chunk.len()].copy_from_slice(chunk);
            *h.input_mut("in_l") = buf;
            *h.input_mut("in_r") = buf;
            h.process(); // guarded by the harness
            out.extend_from_slice(h.output("out_l"));
        }
        out
    };
    let (a, b) = (run(), run());
    assert_eq!(
        a.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        b.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
    );
}

#[test]
fn every_param_min_max_does_not_panic() {
    let mut h = space();
    sweep_params_min_max(&mut h, &DESCRIPTOR);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(16))]

    #[test]
    fn output_is_finite_and_bounded_for_any_params(
        alg in 0u8..4,
        size in 0.0f32..1.0,
        decay in 0.2f32..20.0,
        highcut in 1_000.0f32..20_000.0,
        gate in 0.0f32..500.0,
        level in -60.0f32..0.0,
        mix in 0.0f32..1.0,
        seed in 0u64..100,
    ) {
        let mut h = DeviceHarness::with_options(Box::new(SpaceReverb::new()), SR, 120.0, seed);
        for (k, v) in [
            ("algorithm", f32::from(alg)), ("size", size), ("decay", decay),
            ("highcut", highcut), ("gate", gate), ("gate_level", level), ("mix", mix),
        ] {
            h.set_param(k, v);
        }
        let (l, r) = run_mono(&mut h, &noise(seed, 30 * BATCH, 1.0));
        for v in l.iter().chain(&r) {
            prop_assert!(v.is_finite() && v.abs() <= 32.0 * 4.0 + 1.0, "{v}");
        }
    }
}
