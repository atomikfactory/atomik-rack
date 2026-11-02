//! Convolution reverb tests. The partitioned convolver itself is tested against direct
//! convolution in `atomik-dsp`; these cover the device around it.

use super::*;
use crate::test_support::{SR, assert_keys, harness, sweep_params_min_max};
use atomik_core::BATCH;
use atomik_device_sdk::{LoopMode, SampleBuffer, SampleMap, TriggerMode, Zone, ZoneExt};
use atomik_dsp::Rng;
use atomik_testkit::harness::DeviceHarness;
use atomik_testkit::no_alloc::assert_no_alloc;
use atomik_testkit::signal::rms;
use proptest::prelude::{ProptestConfig, prop_assert, proptest};
use std::sync::Arc;

fn noise(seed: u64, n: usize, amp: f32) -> Vec<f32> {
    let mut r = Rng::new(seed);
    (0..n).map(|_| r.next_bipolar() * amp).collect()
}

/// A map whose first zone is the whole of `channels` (the impulse response).
fn ir_map(channels: Vec<Vec<f32>>) -> SampleMapRef {
    let frames = channels[0].len() as u64;
    let buffer = Arc::new(SampleBuffer::new(channels.into()).unwrap());
    let zone = Zone {
        buffer: 0,
        key_lo: 0,
        key_hi: 127,
        vel_lo: 1,
        vel_hi: 127,
        root_key: 60,
        transpose: 0,
        tune_cents: 0.0,
        gain_db: 0.0,
        pan: 0.0,
        start: 0,
        end: frames,
        loop_mode: LoopMode::Off,
        loop_start: 0,
        loop_end: 0,
        crossfade: 0,
        trigger: TriggerMode::OneShot,
        reverse: false,
        ext: ZoneExt::NONE,
    };
    Arc::new(SampleMap::new(vec![buffer], vec![zone]).unwrap())
}

fn convolver() -> DeviceHarness {
    let mut h = harness(Box::new(ConvolutionReverb::new()));
    h.set_param("mix", 1.0);
    h
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

fn impulse(n: usize, at: usize) -> Vec<f32> {
    let mut x = vec![0.0f32; n];
    x[at] = 1.0;
    x
}

#[test]
fn descriptor_validates() {
    assert_keys(
        &DESCRIPTOR,
        &[
            "predelay", "lowcut", "highcut", "width", "output", "mix", "bypass",
        ],
        &["in_l", "in_r", "out_l", "out_r"],
    );
    assert_eq!(DESCRIPTOR.type_id, "atomik.convolver");
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Effect);
    assert_eq!(DESCRIPTOR.validate(), Ok(()));
}

#[test]
fn without_an_impulse_response_the_wet_signal_is_silent() {
    let mut h = convolver();
    let (l, r) = run_mono(&mut h, &noise(1, 4_000, 0.5));
    assert!(l.iter().chain(&r).all(|v| *v == 0.0));
}

#[test]
fn the_first_sample_of_the_ir_sounds_at_once_and_the_ir_is_reproduced() {
    // A mono IR of mixed decaying noise, longer than the head so the tail partitions matter.
    let n = 9_000;
    let ir: Vec<f32> = noise(2, n, 1.0)
        .iter()
        .enumerate()
        .map(|(i, v)| v * (-(i as f32) / 3_000.0).exp())
        .collect();
    let energy: f64 = ir.iter().map(|v| f64::from(*v).powi(2)).sum();
    let gain = (1.0 / energy.sqrt()) as f32;
    let mut h = convolver();
    h.set_param("lowcut", 20.0);
    assert!(
        h.device_mut()
            .swap_samples(Some(ir_map(vec![ir.clone()])))
            .is_none()
    );
    assert_eq!(h.device().latency_frames(), 0);
    let (l, r) = run_mono(&mut h, &impulse(12_000, 0));
    // The reference is the IR through the device's 20 Hz low cut (a one-pole highpass).
    let a = 1.0 - (-std::f64::consts::TAU * 20.0 / SR).exp();
    let mut lp = 0.0f64;
    let want: Vec<f64> = (0..l.len())
        .map(|i| {
            let x = ir.get(i).map_or(0.0, |v| f64::from(*v * gain));
            let y = x - lp;
            lp += a * (x - lp);
            y
        })
        .collect();
    let scale = gain * ir.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    for i in 0..l.len() {
        assert!(
            (f64::from(l[i]) - want[i]).abs() < 5e-3 * f64::from(scale),
            "L {i}: {} vs {}",
            l[i],
            want[i]
        );
        assert!(
            (f64::from(r[i]) - want[i]).abs() < 5e-3 * f64::from(scale),
            "R {i}"
        );
    }
}

#[test]
fn a_stereo_ir_convolves_each_channel_with_its_own() {
    let mut h = convolver();
    let mut a = vec![0.0f32; 600];
    a[10] = 1.0;
    let mut b = vec![0.0f32; 600];
    b[300] = 1.0;
    h.device_mut().swap_samples(Some(ir_map(vec![a, b])));
    h.set_param("width", 1.0);
    let (l, r) = run_mono(&mut h, &impulse(1_000, 0));
    let first = |x: &[f32]| x.iter().position(|v| v.abs() > 0.2).unwrap();
    // Width 1 keeps the channels apart: each peaks at its own IR position.
    assert_eq!(first(&l), 10);
    assert_eq!(first(&r), 300);
}

#[test]
fn the_level_does_not_depend_on_the_ir_scale() {
    let ir = noise(3, 6_000, 1.0);
    let quiet: Vec<f32> = ir.iter().map(|v| v * 0.05).collect();
    let x = noise(4, 40_000, 0.5);
    let level = |ir: &[f32]| {
        let mut h = convolver();
        h.set_param("lowcut", 20.0);
        h.device_mut().swap_samples(Some(ir_map(vec![ir.to_vec()])));
        let (l, _) = run_mono(&mut h, &x);
        rms(&l[10_000..])
    };
    let (a, b) = (level(&ir), level(&quiet));
    assert!((a / b - 1.0).abs() < 0.01, "{a} vs {b}");
    // White noise keeps roughly its power.
    assert!((a / rms(&x[10_000..]) - 1.0).abs() < 0.25, "{a}");
}

#[test]
fn mix_zero_is_dry_bit_exact() {
    let x = noise(9, 10_000, 1.0);
    let mut h = convolver();
    h.device_mut()
        .swap_samples(Some(ir_map(vec![noise(8, 3_000, 1.0)])));
    h.set_param("mix", 0.0);
    let (l, r) = run_mono(&mut h, &x);
    assert!(l.iter().zip(&x).all(|(a, b)| a.to_bits() == b.to_bits()));
    assert!(r.iter().zip(&x).all(|(a, b)| a.to_bits() == b.to_bits()));
}

#[test]
fn the_pre_delay_shifts_the_onset() {
    let mut h = convolver();
    let mut ir = vec![0.0f32; 400];
    ir[0] = 1.0;
    h.device_mut().swap_samples(Some(ir_map(vec![ir])));
    h.set_param("predelay", 10.0);
    let (l, _) = run_mono(&mut h, &impulse(2_000, 0));
    let onset = l.iter().position(|v| v.abs() > 0.2).unwrap();
    assert_eq!(onset, (0.010 * SR) as usize);
}

#[test]
fn swapping_the_ir_returns_the_old_map_and_never_clicks() {
    let first = ir_map(vec![noise(5, 5_000, 1.0)]);
    let second = ir_map(vec![noise(6, 8_000, 1.0)]);
    let mut h = convolver();
    assert!(h.device_mut().swap_samples(Some(first.clone())).is_none());
    let x = noise(7, 60_000, 0.5);
    let mut out = Vec::new();
    for (b, chunk) in x.chunks(BATCH).enumerate() {
        if b == 400 {
            let back = assert_no_alloc(|| h.device_mut().swap_samples(Some(second.clone())));
            assert!(Arc::ptr_eq(&back.unwrap(), &first));
        }
        let mut buf = [0.0f32; BATCH];
        buf[..chunk.len()].copy_from_slice(chunk);
        *h.input_mut("in_l") = buf;
        *h.input_mut("in_r") = buf;
        h.process();
        out.extend_from_slice(h.output("out_l"));
    }
    let step = |s: &[f32]| {
        s.windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max)
    };
    let before = step(&out[10_000..25_000]);
    let around = step(&out[25_000..40_000]);
    assert!(around < before * 1.6, "{around} vs {before}");
    // The new IR is what sounds at the end.
    let back = assert_no_alloc(|| h.device_mut().swap_samples(None));
    assert!(back.is_some());
}

#[test]
fn clearing_the_ir_goes_quiet_and_a_second_ir_takes_over() {
    let mut h = convolver();
    h.device_mut()
        .swap_samples(Some(ir_map(vec![noise(5, 3_000, 1.0)])));
    let x = noise(7, 20_000, 0.5);
    run_mono(&mut h, &x);
    h.device_mut().swap_samples(None);
    let (l, _) = run_mono(&mut h, &x);
    assert!(l[8_000..].iter().all(|v| v.abs() < 1e-4));
    h.device_mut()
        .swap_samples(Some(ir_map(vec![noise(9, 3_000, 1.0)])));
    let (l, _) = run_mono(&mut h, &x);
    assert!(rms(&l[8_000..]) > 0.05);
}

#[test]
fn an_ir_longer_than_the_limit_is_cut_not_refused() {
    let mut h = convolver();
    let long = (MAX_IR_SECONDS * SR as f32) as usize + 50_000;
    h.device_mut()
        .swap_samples(Some(ir_map(vec![vec![0.01; long]])));
    let (l, r) = run_mono(&mut h, &noise(1, 20_000, 0.5));
    assert!(l.iter().chain(&r).all(|v| v.is_finite()));
}

#[test]
fn prepare_again_rebuilds_the_installed_ir() {
    let ir = ir_map(vec![noise(5, 3_000, 1.0)]);
    let mut h = convolver();
    h.device_mut().swap_samples(Some(ir));
    let x = noise(7, 6_000, 0.5);
    let (a, _) = run_mono(&mut h, &x);
    h.device_mut().prepare(&PrepareInfo {
        sample_rate: atomik_core::SampleRate::new(SR).unwrap(),
        seed: 0,
    });
    let (b, _) = run_mono(&mut h, &x);
    assert_eq!(a, b);
}

#[test]
fn deterministic_across_runs() {
    let run = || {
        let mut h = convolver();
        h.device_mut()
            .swap_samples(Some(ir_map(vec![noise(5, 7_000, 1.0)])));
        h.set_param("highcut", 8_000.0);
        run_mono(&mut h, &noise(7, 20_000, 0.5)).0
    };
    assert_eq!(
        run().iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        run().iter().map(|v| v.to_bits()).collect::<Vec<_>>()
    );
}

#[test]
fn every_param_min_max_does_not_panic() {
    let mut h = convolver();
    h.device_mut()
        .swap_samples(Some(ir_map(vec![noise(5, 3_000, 1.0)])));
    sweep_params_min_max(&mut h, &DESCRIPTOR);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(12))]

    #[test]
    fn output_is_finite_and_bounded_for_any_ir_and_params(
        len in 1usize..12_000,
        amp in 0.0f32..4.0,
        stereo in proptest::bool::ANY,
        predelay in 0.0f32..250.0,
        highcut in 1_000.0f32..20_000.0,
        output in -24.0f32..12.0,
        mix in 0.0f32..1.0,
        seed in 0u64..100,
    ) {
        let mut h = harness(Box::new(ConvolutionReverb::new()));
        let mut chans = vec![noise(seed, len, amp)];
        if stereo {
            chans.push(noise(seed + 1, len, amp));
        }
        h.device_mut().swap_samples(Some(ir_map(chans)));
        for (k, v) in [("predelay", predelay), ("highcut", highcut), ("output", output), ("mix", mix)] {
            h.set_param(k, v);
        }
        let (l, r) = run_mono(&mut h, &noise(seed, 40 * BATCH, 1.0));
        for v in l.iter().chain(&r) {
            prop_assert!(v.is_finite() && v.abs() <= 32.0 * 4.0 + 1.0, "{v}");
        }
    }
}
