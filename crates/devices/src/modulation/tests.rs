//! Modulation tests.

use super::*;
use crate::test_support::{SR, assert_keys, harness, run_signal, sweep_params_min_max};
use atomik_core::{NoteBuffer, SampleRate, Tempo, TimeInfo};
use atomik_dsp::Rng;
use atomik_testkit::harness::DeviceHarness;
use proptest::prelude::{ProptestConfig, prop_assert, proptest};

const IN: [&str; 2] = ["in_l", "in_r"];
const OUT: [&str; 2] = ["out_l", "out_r"];

/// Uniform noise in -1..1 (a seeded table of 4 s, wrapped).
fn noise(seed: u64) -> impl Fn(usize) -> f32 {
    let mut rng = Rng::new(seed);
    let v: Vec<f32> = (0..200_000).map(|_| rng.next_f32() * 2.0 - 1.0).collect();
    move |n| v[n % v.len()]
}

/// Drives a `Modulation` directly with an explicit `TimeInfo` (frame, tempo, playing).
fn step(
    m: &mut Modulation,
    sr: f64,
    frame: u64,
    bpm: f64,
    playing: bool,
    l: &[f32; BATCH],
    r: &[f32; BATCH],
) -> ([f32; BATCH], [f32; BATCH]) {
    let mut signals = vec![[0.0f32; BATCH]; 4];
    signals[0] = *l;
    signals[1] = *r;
    let mut notes: Vec<NoteBuffer> = (0..4).map(|_| NoteBuffer::with_capacity(0)).collect();
    let connected = [true; 4];
    let time = TimeInfo::at(
        frame,
        SampleRate::new(sr).unwrap(),
        Tempo::new(bpm).unwrap(),
        playing,
    );
    let mut ctx = ProcessContext::new(time, &mut signals, &mut notes, &connected);
    m.process(&mut ctx);
    (signals[2], signals[3])
}

fn prepared(sr: f64) -> Modulation {
    let mut m = Modulation::new();
    m.prepare(&PrepareInfo {
        sample_rate: SampleRate::new(sr).unwrap(),
        seed: 0,
    });
    m
}

fn set(m: &mut Modulation, key: &str, v: f32) {
    let p = DESCRIPTOR.param_by_key(key).unwrap();
    m.set_param(p.id, p.clamp(v));
}

#[test]
fn descriptor_validates() {
    assert_keys(
        &DESCRIPTOR,
        &[
            "mode", "rate", "sync", "depth", "feedback", "delay", "stages", "spread", "mix",
            "output",
        ],
        &["in_l", "in_r", "out_l", "out_r"],
    );
    assert_eq!(DESCRIPTOR.type_id, "atomik.modulation");
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Effect);
    let f = DESCRIPTOR.param_by_key("feedback").unwrap();
    assert_eq!((f.min, f.max), (-0.95, 0.95));
}

#[test]
fn stages_are_even() {
    for (raw, want) in [
        (2.0, 2),
        (3.0, 4),
        (5.0, 6),
        (11.0, 12),
        (12.0, 12),
        (40.0, 12),
    ] {
        assert_eq!(even_stages(raw), want, "{raw}");
    }
}

#[test]
fn pade_tan_is_accurate() {
    let mut worst = 0.0f64;
    let mut x = 0.001f32;
    while x <= 0.7 {
        worst = worst.max((f64::from(tan_pade(x)) - f64::from(x).tan()).abs());
        x += 0.001;
    }
    assert!(worst < 1e-5, "{worst}");
}

#[test]
fn allocation_free_at_all_rates_and_modes() {
    for sr in [44_100.0, 48_000.0, 96_000.0] {
        for mode in 0..3 {
            let mut h = DeviceHarness::with_options(Box::new(Modulation::new()), sr, 133.0, 3);
            h.set_param("mode", mode as f32);
            h.set_param("sync", 3.0);
            h.set_param("feedback", 0.6);
            run_signal(&mut h, &IN, noise(1), 50, &OUT); // every process call is guarded
            h.set_param("sync", 0.0);
            run_signal(&mut h, &IN, noise(2), 20, &OUT);
            assert_eq!(h.device().latency_frames(), 0);
        }
    }
}

#[test]
fn sweeping_params_stays_finite() {
    let mut h = harness(Box::new(Modulation::new()));
    *h.input_mut("in_l") = [0.5; BATCH];
    *h.input_mut("in_r") = [-0.5; BATCH];
    sweep_params_min_max(&mut h, &DESCRIPTOR);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]
    #[test]
    fn params_keep_output_finite_and_bounded(
        mode in 0u8..3, rate in 0.02f32..20.0, sync in 0u8..8, depth in 0.0f32..1.0,
        fb in -0.95f32..0.95, delay in 0.1f32..30.0, stages in 2u8..13, spread in 0.0f32..180.0,
        mix in 0.0f32..1.0, out in -24.0f32..12.0, seed in 0u64..1000,
    ) {
        let mut h = harness(Box::new(Modulation::new()));
        for (k, v) in [("mode", f32::from(mode)), ("rate", rate), ("sync", f32::from(sync)),
            ("depth", depth), ("feedback", fb), ("delay", delay), ("stages", f32::from(stages)),
            ("spread", spread), ("mix", mix), ("output", out)] {
            h.set_param(k, v);
        }
        let y = run_signal(&mut h, &IN, noise(seed), 150, &OUT);
        let bound = 12.0 * 10f32.powf(out / 20.0) * 8.0;
        for ch in &y {
            prop_assert!(ch.iter().all(|s| s.is_finite() && s.abs() <= bound));
        }
    }
}

#[test]
fn zero_latency_and_dry_passes_instantly() {
    let mut h = harness(Box::new(Modulation::new()));
    assert_eq!(h.device().latency_frames(), 0);
    h.set_param("mix", 0.0);
    let y = run_signal(&mut h, &IN, |n| if n == 5 { 1.0 } else { 0.0 }, 2, &OUT);
    assert_eq!(y[0][5], 1.0);
    assert!(y[0].iter().enumerate().all(|(i, v)| i == 5 || *v == 0.0));
}

#[test]
fn mix_zero_is_dry_bit_exact_in_every_mode() {
    for mode in 0..3 {
        let mut h = harness(Box::new(Modulation::new()));
        h.set_param("mode", mode as f32);
        h.set_param("mix", 0.0);
        h.set_param("feedback", 0.5);
        let x = noise(4);
        let y = run_signal(&mut h, &IN, &x, 40, &OUT);
        for (n, v) in y[0].iter().enumerate() {
            assert_eq!(v.to_bits(), x(n).to_bits());
        }
    }
}

#[test]
fn deterministic_across_instances() {
    for mode in 0..3 {
        let run = |seed| {
            let mut h = DeviceHarness::with_options(Box::new(Modulation::new()), SR, 120.0, seed);
            h.set_param("mode", mode as f32);
            h.set_param("feedback", 0.4);
            run_signal(&mut h, &IN, noise(9), 100, &OUT)
        };
        assert_eq!(run(1), run(1));
    }
}

#[test]
fn flanger_delay_is_exact_at_zero_depth() {
    // Fixed delay of 2 ms (96 samples at 48 kHz), mix 1: the impulse comes back at 96.
    let mut h = harness(Box::new(Modulation::new()));
    h.set_param("mode", 1.0);
    h.set_param("depth", 0.0);
    h.set_param("delay", 2.0);
    h.set_param("mix", 1.0);
    let y = run_signal(&mut h, &IN, |n| if n == 0 { 1.0 } else { 0.0 }, 4, &OUT);
    let (idx, _) = y[0]
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
        .unwrap();
    assert_eq!(idx, 96);
    assert!((y[0][96] - 1.0).abs() < 1e-6);
}

#[test]
fn delay_modulation_stays_in_range() {
    for sr in [44_100.0f32, 48_000.0, 96_000.0] {
        let max_line = (MAX_DELAY_MS * MAX_DELAY_FACTOR * 0.001 * sr).ceil() + 8.0;
        for ms in [0.1f32, 0.5, 2.0, 7.0, 15.0, 30.0] {
            let base = ms * 0.001 * sr;
            for depth in [0.0f32, 0.3, 1.0] {
                for k in 0..=200 {
                    let lfo = -1.0 + k as f32 / 100.0;
                    let f = flanger_delay(base, depth, lfo);
                    assert!(f >= 0.1 * base - 1e-3 && f <= 1.9 * base + 1e-3);
                    assert!(f <= max_line, "flanger {f} > {max_line}");
                    for v in 0..3 {
                        let c = chorus_delay(base, v, depth, lfo);
                        assert!(c >= 0.5 * 0.79 * base - 1e-3 && c <= 1.5 * 1.27 * base + 1e-3);
                        assert!(c <= max_line, "chorus {c} > {max_line}");
                    }
                }
            }
        }
        // The LFO itself stays in -1..=1 for any phase.
        for k in 0..2_000 {
            let l = lfo_shape(f64::from(k) * 0.00137 - 1.3);
            assert!((-1.0..=1.0).contains(&l));
        }
    }
}

#[test]
fn feedback_is_stable_at_maximum_for_30_seconds() {
    for (mode, fb) in [
        (0.0, 0.95),
        (1.0, 0.95),
        (1.0, -0.95),
        (2.0, 0.95),
        (2.0, -0.95),
    ] {
        let mut h = harness(Box::new(Modulation::new()));
        h.set_param("mode", mode);
        h.set_param("feedback", fb);
        h.set_param("depth", 1.0);
        h.set_param("delay", 1.5);
        h.set_param("rate", 7.0);
        h.set_param("mix", 1.0);
        h.set_param("stages", 12.0);
        let x = noise(21);
        let batches = (30.0 * SR) as usize / BATCH;
        let mut peak = 0.0f32;
        for b in 0..batches {
            let mut xb = [0.0f32; BATCH];
            for (i, s) in xb.iter_mut().enumerate() {
                *s = x(b * BATCH + i);
            }
            *h.input_mut("in_l") = xb;
            *h.input_mut("in_r") = xb;
            h.process();
            for k in OUT {
                for s in h.output(k) {
                    assert!(s.is_finite());
                    peak = peak.max(s.abs());
                }
            }
        }
        // Uniform noise through a 0.95 loop stays far below the 20x worst case.
        assert!(peak < 25.0, "mode {mode} fb {fb}: peak {peak}");
    }
}

#[test]
fn feedback_decays_after_input_stops() {
    for mode in 0..3 {
        let mut h = harness(Box::new(Modulation::new()));
        h.set_param("mode", mode as f32);
        h.set_param("feedback", 0.95);
        h.set_param("mix", 1.0);
        h.set_param("depth", 0.7);
        let x = noise(3);
        let y = run_signal(
            &mut h,
            &IN,
            |n| if n < 4_800 { x(n) } else { 0.0 },
            (8.0 * SR) as usize / BATCH,
            &OUT,
        );
        let tail = &y[0][y[0].len() - 4_800..];
        assert!(
            tail.iter().all(|v| v.abs() < 1e-3),
            "mode {mode} tail rings"
        );
    }
}

/// Expected LFO phase of the left channel at absolute sample `frame` (f64 math).
fn want_phase(frame: u64, sr: f64, bpm: f64, beats: f64) -> f64 {
    let x = frame as f64 / (beats * sr * 60.0 / bpm);
    x - x.floor()
}

#[test]
fn sync_periods_are_exact() {
    let zero = [0.0f32; BATCH];
    for (idx, beats) in SYNC_BEATS.iter().enumerate() {
        for (sr, bpm) in [(48_000.0, 120.0), (44_100.0, 133.0), (96_000.0, 90.5)] {
            let mut m = prepared(sr);
            set(&mut m, "sync", (idx + 1) as f32);
            let period = beats * sr * 60.0 / bpm;
            // Phase at the last sample of a batch starting at `frame`, for jumped frames.
            for frame in [0u64, 64, 12_800, 1_234_560] {
                step(&mut m, sr, frame, bpm, true, &zero, &zero);
                let want = want_phase(frame + 63, sr, bpm, *beats);
                let got = m.lfo_phase();
                assert!(
                    (got - want).abs() < 1e-9,
                    "sync {idx} frame {frame}: {got} vs {want}"
                );
            }
            // One (rounded) period apart the phase differs only by the rounding drift.
            let f0 = 640u64;
            let f1 = f0 + period.round() as u64;
            step(&mut m, sr, f0, bpm, true, &zero, &zero);
            let p0 = m.lfo_phase();
            step(&mut m, sr, f1, bpm, true, &zero, &zero);
            let p1 = m.lfo_phase();
            let drift = (period.round() - period) / period;
            let d = (p1 - p0 + 0.5).rem_euclid(1.0) - 0.5;
            assert!((d - drift).abs() < 1e-9, "sync {idx}: d {d} drift {drift}");
        }
    }
}

#[test]
fn synced_output_is_independent_of_block_history() {
    // A fresh device that starts at frame 12_800 and one that ran from 0 agree bit for bit when
    // both only ever saw silence before (so the delay lines and allpasses hold nothing).
    for mode in 0..3 {
        let (sr, bpm) = (48_000.0, 120.0);
        let mut a = prepared(sr);
        let mut b = prepared(sr);
        for m in [&mut a, &mut b] {
            set(m, "mode", mode as f32);
            set(m, "sync", 3.0);
            set(m, "feedback", 0.5);
            set(m, "depth", 0.8);
        }
        let zero = [0.0f32; BATCH];
        let mut frame = 0u64;
        while frame < 12_800 {
            step(&mut a, sr, frame, bpm, true, &zero, &zero);
            frame += BATCH as u64;
        }
        let x = noise(5);
        for k in 0..200usize {
            let mut xb = [0.0f32; BATCH];
            for (i, s) in xb.iter_mut().enumerate() {
                *s = x(k * BATCH + i);
            }
            let f = 12_800 + (k * BATCH) as u64;
            let ya = step(&mut a, sr, f, bpm, true, &xb, &xb);
            let yb = step(&mut b, sr, f, bpm, true, &xb, &xb);
            assert_eq!(ya, yb, "mode {mode} batch {k}");
        }
    }
}

#[test]
fn free_runs_when_stopped_and_at_the_set_rate() {
    let sr = 48_000.0;
    let zero = [0.0f32; BATCH];
    let mut m = prepared(sr);
    set(&mut m, "sync", 3.0); // 1/4 note: 24000 samples at 120 BPM
    let mut frame = 0u64;
    // 375 batches of 64 = 24000 samples: one cycle while stopped (the frame is ignored).
    for _ in 0..375 {
        step(&mut m, sr, frame, 120.0, false, &zero, &zero);
        frame += 777;
    }
    let p = m.lfo_phase();
    assert!(p < 1e-9 || (1.0 - p) < 1e-9, "one cycle: {p}");
    // Unsynced at 2 Hz for 0.5 s: one cycle.
    let mut m = prepared(sr);
    set(&mut m, "rate", 2.0);
    for k in 0..375 {
        step(&mut m, sr, k * 64, 120.0, true, &zero, &zero);
    }
    let p = m.lfo_phase();
    assert!(p < 1e-9 || (1.0 - p) < 1e-9, "free 2 Hz: {p}");
}

#[test]
fn stereo_spread_offsets_the_right_lfo() {
    let run = |spread: f32| {
        let mut h = harness(Box::new(Modulation::new()));
        h.set_param("mode", 2.0);
        h.set_param("spread", spread);
        h.set_param("depth", 1.0);
        h.set_param("mix", 1.0);
        h.set_param("rate", 3.0);
        run_signal(&mut h, &IN, noise(8), 100, &OUT)
    };
    let a = run(0.0);
    assert_eq!(a[0], a[1]);
    let b = run(180.0);
    assert_ne!(b[0], b[1]);
}

#[test]
fn phaser_has_unit_magnitude_without_modulation() {
    // Depth 0, mix 1, no feedback: a chain of allpasses has unit gain at every frequency.
    let mut h = harness(Box::new(Modulation::new()));
    h.set_param("mode", 2.0);
    h.set_param("depth", 0.0);
    h.set_param("mix", 1.0);
    h.set_param("stages", 8.0);
    for hz in [100.0, 1_000.0, 5_000.0] {
        let g = crate::test_support::sine_gain_db(&mut h, &IN, "out_l", hz, 0.5, 0.2);
        assert!(g.abs() < 0.01, "{hz} Hz: {g} dB");
    }
}
