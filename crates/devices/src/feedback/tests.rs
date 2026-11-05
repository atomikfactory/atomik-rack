//! Feedback route tests, all through `DeviceHarness` (which guards every `process` against allocation).
#![allow(clippy::float_cmp)]

use super::*;
use crate::test_support::{SR, assert_keys, harness, sweep_params_min_max};
use atomik_core::{NoteBuffer, SampleRate, Tempo, TimeInfo};
use atomik_testkit::harness::DeviceHarness;

fn line() -> DeviceHarness {
    harness(Box::new(FeedbackDelay::new()))
}

/// A deterministic test signal.
fn noise(n: usize) -> f32 {
    let mut x = (n as u32).wrapping_mul(2_654_435_761).wrapping_add(17);
    x ^= x >> 15;
    x = x.wrapping_mul(2_246_822_519);
    x ^= x >> 13;
    (x as f32 / u32::MAX as f32) * 1.6 - 0.8
}

/// Runs `batches` batches of `l(n)` on `in_l` and `r(n)` on `in_r`; returns the outputs.
fn run(
    h: &mut DeviceHarness,
    l: impl Fn(usize) -> f32,
    r: impl Fn(usize) -> f32,
    batches: usize,
) -> (Vec<f32>, Vec<f32>) {
    let (mut out_l, mut out_r) = (Vec::new(), Vec::new());
    for b in 0..batches {
        let mut xl = [0.0; BATCH];
        let mut xr = [0.0; BATCH];
        for i in 0..BATCH {
            xl[i] = l(b * BATCH + i);
            xr[i] = r(b * BATCH + i);
        }
        *h.input_mut("in_l") = xl;
        *h.input_mut("in_r") = xr;
        h.process();
        out_l.extend_from_slice(h.output("out_l"));
        out_r.extend_from_slice(h.output("out_r"));
    }
    (out_l, out_r)
}

#[test]
fn descriptor_validates_and_splits_the_stages() {
    assert_keys(
        &DESCRIPTOR,
        &["time", "gain"],
        &["in_l", "in_r", "out_l", "out_r"],
    );
    assert_eq!(DESCRIPTOR.type_id, "atomik.feedback");
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Utility);
    assert_eq!(DESCRIPTOR.stage_count(), 2);
    for s in DESCRIPTOR.sockets {
        let stage = if s.direction.is_input() { 1 } else { 0 };
        assert_eq!(DESCRIPTOR.stage_of(s.id), stage, "{}", s.key);
    }
}

#[test]
fn the_shortest_delay_is_one_batch() {
    let mut h = line();
    // An impulse at frame 5: it comes back at frame 5 + 64, never earlier.
    let (l, r) = run(
        &mut h,
        |n| f32::from(n == 5),
        |n| 0.5 * f32::from(n == 7),
        3,
    );
    let at: Vec<usize> = (0..l.len()).filter(|&i| l[i] != 0.0).collect();
    assert_eq!(at, vec![5 + BATCH]);
    assert_eq!(l[5 + BATCH], 1.0);
    let at: Vec<usize> = (0..r.len()).filter(|&i| r[i] != 0.0).collect();
    assert_eq!(at, vec![7 + BATCH]);
    assert_eq!(r[7 + BATCH], 0.5);
}

#[test]
fn the_first_batch_is_silent_and_the_signal_comes_back_bit_for_bit() {
    let mut h = line();
    let (l, r) = run(&mut h, noise, |n| noise(n + 1000), 8);
    assert!(l[..BATCH].iter().all(|x| *x == 0.0));
    assert!(r[..BATCH].iter().all(|x| *x == 0.0));
    for n in BATCH..8 * BATCH {
        assert_eq!(l[n].to_bits(), noise(n - BATCH).to_bits(), "{n}");
        assert_eq!(r[n].to_bits(), noise(n - BATCH + 1000).to_bits(), "{n}");
    }
}

#[test]
fn time_sets_the_delay_in_whole_frames() {
    let mut h = line();
    h.set_param("time", 10.0);
    let frames = (10.0 * 0.001 * SR) as usize; // 480
    let (l, _) = run(&mut h, |n| f32::from(n == 3), |_| 0.0, 12);
    let at: Vec<usize> = (0..l.len()).filter(|&i| l[i] != 0.0).collect();
    assert_eq!(at, vec![3 + frames]);
}

#[test]
fn a_time_below_one_batch_is_one_batch() {
    let mut h = line();
    h.set_param("time", 0.5); // 24 frames: under the minimum
    let (l, _) = run(&mut h, |n| f32::from(n == 0), |_| 0.0, 3);
    let at: Vec<usize> = (0..l.len()).filter(|&i| l[i] != 0.0).collect();
    assert_eq!(at, vec![BATCH]);
}

#[test]
fn gain_scales_what_comes_back() {
    let mut h = line();
    h.set_param("gain", -20.0);
    let (l, _) = run(&mut h, |_| 1.0, |_| 0.0, 6);
    assert!((l[5 * BATCH] - 0.1).abs() < 1e-5, "{}", l[5 * BATCH]);
    let mut h = line();
    h.set_param("gain", -60.0);
    let (l, _) = run(&mut h, |_| 1.0, |_| 0.0, 6);
    assert!((l[5 * BATCH] - 0.001).abs() < 1e-6);
}

#[test]
fn what_enters_is_bounded_and_non_finite_samples_count_as_zero() {
    let mut h = line();
    let (l, r) = run(
        &mut h,
        |n| match n % 4 {
            0 => 1.0e9,
            1 => f32::NAN,
            2 => f32::INFINITY,
            _ => -1.0e9,
        },
        |_| 0.0,
        3,
    );
    for (n, got) in l.iter().enumerate().skip(BATCH) {
        let want = match (n - BATCH) % 4 {
            0 => LIMIT,
            1 | 2 => 0.0,
            _ => -LIMIT,
        };
        assert_eq!(*got, want, "{n}");
    }
    assert!(r.iter().all(|x| *x == 0.0));
}

#[test]
fn reset_clears_the_line() {
    let mut h = line();
    run(&mut h, |_| 1.0, |_| 1.0, 4);
    h.device_mut().reset();
    let (l, r) = run(&mut h, |_| 0.0, |_| 0.0, 3);
    assert!(l.iter().chain(&r).all(|x| *x == 0.0));
}

#[test]
fn a_moving_time_glides_without_a_jump() {
    let mut h = line();
    let sig = |n: usize| (n as f32 * 0.05).sin() * 0.5;
    run(&mut h, sig, sig, 8);
    h.set_param("time", 20.0);
    let later = |n: usize| sig(n + 8 * BATCH);
    let (l, _) = run(&mut h, later, later, 80);
    // The sine moves at most 0.025 per frame; the gliding read adds little to that.
    let worst = l
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0f32, f32::max);
    assert!(worst < 0.06, "{worst}");
}

#[test]
fn the_two_stages_in_turn_are_one_process_call() {
    let sr = SampleRate::new(SR).unwrap();
    let time = TimeInfo::at(0, sr, Tempo::new(120.0).unwrap(), true);
    let info = PrepareInfo {
        sample_rate: sr,
        seed: 0,
    };
    let mut whole = FeedbackDelay::new();
    let mut staged = FeedbackDelay::new();
    whole.prepare(&info);
    staged.prepare(&info);
    let mut notes = vec![NoteBuffer::with_capacity(0); 4];
    let connected = vec![true; 4];
    for b in 0..6 {
        let mut input = [0.0f32; BATCH];
        for (i, s) in input.iter_mut().enumerate() {
            *s = noise(b * BATCH + i);
        }
        let mut a = vec![[0.0f32; BATCH]; 4];
        a[0] = input;
        let mut s = a.clone();
        whole.process(&mut ProcessContext::new(
            time, &mut a, &mut notes, &connected,
        ));
        // Stage 0 sees no input of stage 1 yet: the output comes from earlier batches only.
        let mut early = vec![[0.0f32; BATCH]; 4];
        staged.process_stage(
            0,
            &mut ProcessContext::new(time, &mut early, &mut notes, &connected),
        );
        s[2] = early[2];
        staged.process_stage(
            1,
            &mut ProcessContext::new(time, &mut s, &mut notes, &connected),
        );
        assert_eq!(a[2], s[2], "batch {b}");
        assert_eq!(a[3], s[3], "batch {b}");
    }
}

#[test]
fn the_sweep_stays_finite() {
    let mut h = line();
    *h.input_mut("in_l") = [0.25; BATCH];
    *h.input_mut("in_r") = [0.25; BATCH];
    sweep_params_min_max(&mut h, &DESCRIPTOR);
}
