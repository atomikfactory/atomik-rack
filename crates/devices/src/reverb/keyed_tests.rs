//! Tests of the key-driven duck and gate of both reverbs (`atomik.reverb`, `atomik.space-reverb`).

use crate::bypass::Bypassable;
use crate::space_reverb::SpaceReverb;
use crate::test_support::{SR, harness, sweep_params_min_max};
use atomik_core::BATCH;
use atomik_dsp::Rng;
use atomik_testkit::harness::DeviceHarness;
use atomik_testkit::signal::rms;

type Make = fn() -> DeviceHarness;

fn classic() -> DeviceHarness {
    harness(Box::new(super::Reverb::new()))
}

fn space() -> DeviceHarness {
    harness(Box::new(SpaceReverb::new()))
}

const BOTH: [(&str, Make); 2] = [("reverb", classic), ("space reverb", space)];

fn noise(seed: u64, n: usize, amp: f32) -> Vec<f32> {
    let mut r = Rng::new(seed);
    (0..n).map(|_| r.next_bipolar() * amp).collect()
}

fn tone(freq: f32, n: usize, amp: f32) -> Vec<f32> {
    (0..n)
        .map(|i| amp * (std::f32::consts::TAU * freq * i as f32 / SR as f32).sin())
        .collect()
}

/// Which key sockets are cabled in a run.
#[derive(Clone, Copy, PartialEq)]
enum Key {
    None,
    Both,
    LeftOnly,
}

/// Feeds `x` to both inputs and `key` to the key sockets as `cabling` says; returns (L, R).
fn run(h: &mut DeviceHarness, x: &[f32], key: &[f32], cabling: Key) -> (Vec<f32>, Vec<f32>) {
    let (mut l, mut r) = (Vec::new(), Vec::new());
    for (i, chunk) in x.chunks(BATCH).enumerate() {
        let mut b = [0.0f32; BATCH];
        b[..chunk.len()].copy_from_slice(chunk);
        *h.input_mut("in_l") = b;
        *h.input_mut("in_r") = b;
        if cabling != Key::None {
            let mut k = [0.0f32; BATCH];
            for (n, v) in k.iter_mut().enumerate() {
                *v = key.get(i * BATCH + n).copied().unwrap_or(0.0);
            }
            *h.input_mut("sc_in_l") = k;
            if cabling == Key::Both {
                *h.input_mut("sc_in_r") = k;
            }
        }
        h.process();
        l.extend_from_slice(h.output("out_l"));
        r.extend_from_slice(h.output("out_r"));
    }
    (l, r)
}

/// Magnitude of `x` at `freq` (a single Goertzel bin over the whole slice).
fn bin(x: &[f32], freq: f32) -> f64 {
    let w = std::f64::consts::TAU * f64::from(freq) / SR;
    let c = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    for v in x {
        let s = f64::from(*v) + c * s1 - s2;
        s2 = s1;
        s1 = s;
    }
    (s1 * s1 + s2 * s2 - c * s1 * s2).sqrt() / x.len() as f64
}

#[test]
fn a_cabled_key_with_the_duck_and_gate_off_changes_nothing() {
    let x = noise(1, 24_000, 0.5);
    let key = noise(2, 24_000, 0.8);
    for (name, make) in BOTH {
        let mut a = make();
        let mut b = make();
        let off = run(&mut a, &x, &key, Key::None);
        let on = run(&mut b, &x, &key, Key::Both);
        assert_eq!(off, on, "{name}: the key is ignored while neither is on");
    }
}

#[test]
fn the_duck_only_touches_the_wet_path() {
    // The wet path is silent for the first 100 ms (pre-delay), so up to there the output is the
    // dry signal and nothing else: the duck must not move one bit of it. With the mix at 0 the
    // whole output is the input, key or not.
    let x = noise(3, 48_000, 0.5);
    let key = noise(4, 48_000, 0.9);
    let predelay_samples = (0.100 * SR) as usize;
    for (name, make) in BOTH {
        for mode in [0.0, 1.0, 2.0] {
            let configure = |h: &mut DeviceHarness, duck: f32, mix: f32| {
                h.set_param("predelay", 100.0);
                h.set_param("mix", mix);
                h.set_param("duck", duck);
                h.set_param("duck_mode", mode);
            };
            let (mut quiet, mut loud) = (make(), make());
            configure(&mut quiet, 0.0, 0.5);
            configure(&mut loud, 1.0, 0.5);
            let (q, _) = run(&mut quiet, &x, &key, Key::Both);
            let (d, _) = run(&mut loud, &x, &key, Key::Both);
            assert_eq!(
                q[..predelay_samples],
                d[..predelay_samples],
                "{name} mode {mode}: the dry part is bit-identical"
            );
            assert_ne!(
                q[predelay_samples..],
                d[predelay_samples..],
                "{name}: the wet part moved"
            );

            let (mut dry_a, mut dry_b) = (make(), make());
            configure(&mut dry_a, 0.0, 0.0);
            configure(&mut dry_b, 1.0, 0.0);
            let (a, _) = run(&mut dry_a, &x, &key, Key::Both);
            let (b, _) = run(&mut dry_b, &x, &key, Key::Both);
            assert_eq!(a, b, "{name} mode {mode}: at mix 0 the output is the input");
            assert_eq!(a, x[..a.len().min(x.len())].to_vec());
        }
    }
}

#[test]
fn a_broadband_duck_removes_the_wet_signal_while_the_key_is_loud_and_it_comes_back() {
    let x = noise(5, 96_000, 0.4);
    // The key is loud for the first second and silent for the second.
    let key: Vec<f32> = noise(6, 96_000, 0.8)
        .into_iter()
        .enumerate()
        .map(|(i, v)| if i < 48_000 { v } else { 0.0 })
        .collect();
    for (name, make) in BOTH {
        let (mut plain, mut ducked) = (make(), make());
        for h in [&mut plain, &mut ducked] {
            h.set_param("mix", 1.0);
            h.set_param("duck_release", 100.0);
        }
        ducked.set_param("duck", 1.0);
        let (p, _) = run(&mut plain, &x, &key, Key::Both);
        let (d, _) = run(&mut ducked, &x, &key, Key::Both);
        // 0.2 s to 1 s: the follower has attacked, the duck is full.
        let loud = 9_600..48_000;
        assert!(
            rms(&d[loud.clone()]) < 0.01 * rms(&p[loud]),
            "{name}: the wet signal is gone under a loud key"
        );
        // Well after the key stops (1.6 s to 2 s) the reverb is back at its own level.
        let after = 76_800..96_000;
        let (rd, rp) = (rms(&d[after.clone()]), rms(&p[after]));
        assert!(rd > 0.97 * rp, "{name}: {rd} against {rp}");
    }
}

#[test]
fn a_half_duck_takes_half_the_wet_signal_and_a_quiet_key_none() {
    let x = noise(7, 48_000, 0.4);
    let loud = vec![0.8f32; 48_000];
    let quiet = vec![0.001f32; 48_000];
    for (name, make) in BOTH {
        let mut plain = make();
        plain.set_param("mix", 1.0);
        let (p, _) = run(&mut plain, &x, &loud, Key::Both);
        let window = 9_600..48_000;
        for (key, expected) in [(&loud, 0.5f64), (&quiet, 1.0)] {
            let mut h = make();
            h.set_param("mix", 1.0);
            h.set_param("duck", 0.5);
            let (d, _) = run(&mut h, &x, key, Key::Both);
            let ratio = f64::from(rms(&d[window.clone()])) / f64::from(rms(&p[window.clone()]));
            assert!(
                (ratio - expected).abs() < 0.02,
                "{name}: {ratio} for {expected}"
            );
        }
    }
}

#[test]
fn the_low_band_and_the_band_mode_duck_only_their_part_of_the_wet_signal() {
    let n = 96_000;
    let x: Vec<f32> = tone(80.0, n, 0.2)
        .iter()
        .zip(tone(1_000.0, n, 0.2))
        .zip(tone(6_000.0, n, 0.2))
        .map(|((a, b), c)| a + b + c)
        .collect();
    let key = vec![0.8f32; n];
    let window = 24_000..n;
    for (name, make) in BOTH {
        let mut plain = make();
        plain.set_param("mix", 1.0);
        let (p, _) = run(&mut plain, &x, &key, Key::Both);
        let reference = |f: f32| bin(&p[window.clone()], f);
        // (mode, frequency, ducked tone, untouched tones, lowest level left of those)
        for (mode, freq, ducked, kept, floor) in [
            (1.0, 250.0, 80.0, [1_000.0, 6_000.0], 0.85),
            (2.0, 1_000.0, 1_000.0, [80.0, 6_000.0], 0.7),
        ] {
            let mut h = make();
            h.set_param("mix", 1.0);
            h.set_param("duck", 1.0);
            h.set_param("duck_mode", mode);
            h.set_param("duck_freq", freq);
            let (d, _) = run(&mut h, &x, &key, Key::Both);
            let level = |f: f32| bin(&d[window.clone()], f) / reference(f);
            assert!(
                level(ducked) < 0.15,
                "{name} mode {mode}: {} left",
                level(ducked)
            );
            for f in kept {
                let l = level(f);
                assert!(
                    (floor..1.15).contains(&l),
                    "{name} mode {mode}: {f} Hz at {l}"
                );
            }
        }
    }
}

#[test]
fn the_gate_closes_the_tail_when_the_key_stops_and_leaves_the_rest_alone() {
    let n = 48_000;
    let burst = 4_800; // 100 ms
    let x: Vec<f32> = noise(8, n, 0.5)
        .into_iter()
        .enumerate()
        .map(|(i, v)| if i < burst { v } else { 0.0 })
        .collect();
    let key = x.clone();
    for (name, make) in BOTH {
        let (mut plain, mut gated) = (make(), make());
        for h in [&mut plain, &mut gated] {
            h.set_param("mix", 1.0);
            h.set_param("decay", 6.0);
            h.set_param("predelay", 0.0);
            h.set_param("duck_threshold", -40.0);
        }
        gated.set_param("key_gate", 1.0);
        gated.set_param("key_gate_hold", 40.0);
        let (p, _) = run(&mut plain, &x, &key, Key::Both);
        let (g, _) = run(&mut gated, &x, &key, Key::Both);
        assert_eq!(p[..burst], g[..burst], "{name}: open while the key plays");
        let late = 12_000..n; // 250 ms on: hold and fade are over
        assert!(
            rms(&p[late.clone()]) > 0.01,
            "{name}: the plain tail rings on"
        );
        assert!(
            rms(&g[late.clone()]) < 1e-3 * rms(&p[late]),
            "{name}: the gated tail is closed"
        );
    }
}

#[test]
fn a_gate_hold_of_zero_keeps_the_gate_open() {
    let x = noise(9, 24_000, 0.5);
    let key = vec![0.0f32; 24_000];
    for (name, make) in BOTH {
        let (mut plain, mut gated) = (make(), make());
        for h in [&mut plain, &mut gated] {
            h.set_param("mix", 1.0);
        }
        gated.set_param("key_gate", 1.0);
        gated.set_param("key_gate_hold", 0.0);
        let (p, _) = run(&mut plain, &x, &key, Key::Both);
        let (g, _) = run(&mut gated, &x, &key, Key::Both);
        assert_eq!(p, g, "{name}");
    }
}

#[test]
fn without_a_cable_the_main_input_is_the_key_and_one_side_feeds_both() {
    let x = noise(10, 48_000, 0.5);
    let window = 9_600..48_000;
    for (name, make) in BOTH {
        let mut plain = make();
        plain.set_param("mix", 1.0);
        let (p, _) = run(&mut plain, &x, &[], Key::None);
        let mut uncabled = make();
        uncabled.set_param("mix", 1.0);
        uncabled.set_param("duck", 1.0);
        let (u, _) = run(&mut uncabled, &x, &[], Key::None);
        assert!(
            rms(&u[window.clone()]) < 0.05 * rms(&p[window.clone()]),
            "{name}: self-ducked"
        );

        let key = vec![0.8f32; 48_000];
        let (mut both, mut left) = (make(), make());
        for h in [&mut both, &mut left] {
            h.set_param("mix", 1.0);
            h.set_param("duck", 1.0);
        }
        let (b, _) = run(&mut both, &x, &key, Key::Both);
        let (l, _) = run(&mut left, &x, &key, Key::LeftOnly);
        assert_eq!(b, l, "{name}: a lone left key counts for both sides");
    }
}

#[test]
fn the_keyed_params_sweep_stays_finite_and_survives_the_bypass_wrapper() {
    for (_, make) in BOTH {
        let mut h = make();
        let d = h.device().descriptor();
        sweep_params_min_max(&mut h, d);
    }
    // Wrapped like the factory does it: the duck survives the bypass wrapper.
    let mut h = harness(Box::new(Bypassable::new(SpaceReverb::new())));
    h.set_param("duck", 1.0);
    h.set_param("key_gate", 1.0);
    let x = noise(11, 4_800, 0.5);
    let (l, _) = run(&mut h, &x, &x, Key::Both);
    assert!(l.iter().all(|v| v.is_finite()));
}
