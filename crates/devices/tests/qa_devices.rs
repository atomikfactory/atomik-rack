//! QA tests strengthening the device acceptance criteria.
//!
//! * the in-crate test passes its 0.1 bound with little margin against a delay that
//!   jumps instead of gliding (max step 0.114 on that mutant). The fully-wet scenario below
//!   separates the two (≤ 0.030 vs ≥ 0.092). Large changes exceed the literal bound: see the
//!   BUG-ignored test.
//! * the pan law is checked over the whole settled batch, from either input side, and
//!   at intermediate pan positions (constant power, not only the centre and the extremes).
//! * the bound holds at every supported sample rate, not only 48 kHz.
atomik_testkit::install_alloc_guard!();

use atomik_core::{BATCH, NoteEvent, NoteKind};
use atomik_devices::{
    fm_synth, granular, guitar_amp, line_mixer::LineMixer, poly_synth, stereo_delay::StereoDelay,
    wavetable_synth,
};
use atomik_testkit::harness::DeviceHarness;
use atomik_testkit::signal::{max_step, rms};
use proptest::prelude::*;

const SR: f64 = 48_000.0;

/// Feeds a 0.1-amplitude 1 kHz sine (input) into both delay inputs for one batch.
fn sine_batch(h: &mut DeviceHarness, phase: &mut f64) {
    let mut b = [0.0f32; BATCH];
    for s in &mut b {
        *s = (0.1 * (std::f64::consts::TAU * *phase).sin()) as f32;
        *phase = (*phase + 1_000.0 / SR).fract();
    }
    *h.input_mut("in_l") = b;
    *h.input_mut("in_r") = b;
}

/// Fully wet, no feedback: settles at `from_ms` (with the line filled up to `to_ms`), switches
/// to `to_ms`, and returns the max sample-to-sample step of `out_l` from just before the
/// change until 250 batches after it.
fn wet_max_step(from_ms: f32, to_ms: f32) -> f32 {
    let mut h = DeviceHarness::new(Box::new(StereoDelay::new()), SR);
    h.set_param("time.mode", 0.0);
    h.set_param("time.ms", from_ms);
    h.set_param("feedback", 0.0);
    h.set_param("mix", 1.0);
    let mut phase = 0.0;
    let history_ms = f64::from(from_ms.max(to_ms));
    let settle = (history_ms * SR / 1_000.0 / BATCH as f64) as usize + 20;
    let mut out = Vec::new();
    for i in 0..settle + 250 {
        if i == settle {
            h.set_param("time.ms", to_ms);
        }
        sine_batch(&mut h, &mut phase);
        h.process();
        if i + 5 >= settle {
            out.extend_from_slice(h.output("out_l"));
        }
    }
    assert!(rms(&out) > 0.05, "wet signal present: {}", rms(&out));
    max_step(&out)
}

/// Moderate time changes. Measured max steps: original glide ≤ 0.030; a delay that jumps
/// instead of gliding gives 0.092…0.129 on these same changes (mutation check M6).
const MODERATE_CHANGES: [(f32, f32); 4] = [
    (100.0, 100.25),
    (100.0, 37.25),
    (150.5, 100.25),
    (375.0, 500.5),
];

#[test]
fn qa_ac_d10_fully_wet_moderate_time_changes_are_click_free() {
    for (from, to) in MODERATE_CHANGES {
        let step = wet_max_step(from, to);
        assert!(
            step <= 0.1,
            "delay-change bound, {from} -> {to} ms: max step {step}"
        );
        assert!(
            step <= 0.05,
            "click-free with margin, {from} -> {to} ms: max step {step}"
        );
    }
}

/// Read literally ("a delay-time change", any size) with the delay fully wet.
#[test]
fn qa_ac_d10_fully_wet_large_time_changes_stay_within_0_1() {
    for (from, to) in [(37.25, 1_000.5), (1_000.5, 37.25)] {
        let step = wet_max_step(from, to);
        assert!(
            step <= 0.1,
            "delay-change bound, {from} -> {to} ms: max step {step}"
        );
    }
}

fn mixer() -> DeviceHarness {
    DeviceHarness::new(Box::new(LineMixer::new()), SR)
}

/// Renders 10 batches (well past the 5 ms smoothing) and returns the last batch of L and R.
fn settled_batch(h: &mut DeviceHarness) -> (Vec<f32>, Vec<f32>) {
    let o = h.render(10, &["master_l", "master_r"]);
    let tail = |x: &Vec<f32>| x[x.len() - BATCH..].to_vec();
    (tail(&o[0]), tail(&o[1]))
}

#[test]
fn qa_ac_d5_centre_pan_is_0_7071_over_the_whole_batch_from_either_side() {
    let k = std::f32::consts::FRAC_1_SQRT_2;
    for side in ["ch1_in_l", "ch1_in_r"] {
        let mut h = mixer();
        *h.input_mut(side) = [1.0; BATCH];
        let (l, r) = settled_batch(&mut h);
        for (n, (a, b)) in l.iter().zip(&r).enumerate() {
            assert!((a - k).abs() < 1e-4, "{side}: L[{n}] = {a}");
            assert!((b - k).abs() < 1e-4, "{side}: R[{n}] = {b}");
        }
    }
}

#[test]
fn qa_ac_d5_pan_law_is_constant_power_at_every_position() {
    for step in -10i8..=10 {
        let pan = f32::from(step) / 10.0;
        let mut h = mixer();
        *h.input_mut("ch1_in_l") = [1.0; BATCH];
        h.set_param("ch1.pan", pan);
        let (l, r) = settled_batch(&mut h);
        let theta = (pan + 1.0) * std::f32::consts::FRAC_PI_4;
        let (l, r) = (l[BATCH - 1], r[BATCH - 1]);
        assert!((l - theta.cos()).abs() < 1e-4, "pan {pan}: L {l}");
        assert!((r - theta.sin()).abs() < 1e-4, "pan {pan}: R {r}");
        assert!(
            (l * l + r * r - 1.0).abs() < 1e-4,
            "pan {pan}: power {}",
            l * l + r * r
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(12))]
    /// At 44.1 / 48 / 96 kHz: any params and random notes stay finite and |x| <= 4.
    #[test]
    fn qa_ac_d3_any_params_bounded_at_every_sample_rate(
        sr_index in 0usize..3,
        norms in prop::collection::vec(0.0f32..=1.0, poly_synth::DESCRIPTOR.params.len()),
        notes in prop::collection::vec((0usize..200, 0u16..64, 0u8..=127, 0.01f32..=1.0), 1..40),
        cv in -1.5f32..=1.5,
        seed in any::<u64>(),
    ) {
        let sr = [44_100.0, 48_000.0, 96_000.0][sr_index];
        let mut h = DeviceHarness::with_options(
            Box::new(poly_synth::PolySynth::new()), sr, 120.0, seed);
        for (p, n) in poly_synth::DESCRIPTOR.params.iter().zip(&norms) {
            h.set_param(p.key, p.from_normalized(*n));
        }
        *h.input_mut("cutoff_cv_in") = [cv; BATCH];
        let mut notes = notes;
        notes.sort_by_key(|(b, o, _, _)| (*b, *o));
        let batches = (sr / BATCH as f64).ceil() as usize; // 1 s
        let mut next = 0;
        for b in 0..batches {
            while next < notes.len() && notes[next].0 == b {
                let (_, offset, key, velocity) = notes[next];
                h.push_note("notes_in", NoteEvent { offset, kind: NoteKind::On { key, velocity } });
                next += 1;
            }
            h.process();
            for key in ["out_l", "out_r"] {
                for s in h.output(key) {
                    prop_assert!(s.is_finite() && s.abs() <= 4.0, "{} Hz, {}: {}", sr, key, s);
                }
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(12))]
    /// The wavetable synth at 44.1 / 48 / 96 kHz: any params, CVs and random notes stay finite
    /// and |x| <= 4.
    #[test]
    fn qa_wavetable_any_params_bounded_at_every_sample_rate(
        sr_index in 0usize..3,
        norms in prop::collection::vec(0.0f32..=1.0, wavetable_synth::DESCRIPTOR.params.len()),
        notes in prop::collection::vec((0usize..200, 0u16..64, 0u8..=127, 0.01f32..=1.0), 1..40),
        cv in -1.5f32..=1.5,
        seed in any::<u64>(),
    ) {
        let sr = [44_100.0, 48_000.0, 96_000.0][sr_index];
        let mut h = DeviceHarness::with_options(
            Box::new(wavetable_synth::WavetableSynth::new()), sr, 120.0, seed);
        for (p, n) in wavetable_synth::DESCRIPTOR.params.iter().zip(&norms) {
            h.set_param(p.key, p.from_normalized(*n));
        }
        for socket in ["cutoff_cv_in", "pitch_cv_in", "position_cv_in"] {
            *h.input_mut(socket) = [cv; BATCH];
        }
        let mut notes = notes;
        notes.sort_by_key(|(b, o, _, _)| (*b, *o));
        let batches = (sr / BATCH as f64).ceil() as usize; // 1 s
        let mut next = 0;
        for b in 0..batches {
            while next < notes.len() && notes[next].0 == b {
                let (_, offset, key, velocity) = notes[next];
                h.push_note("notes_in", NoteEvent { offset, kind: NoteKind::On { key, velocity } });
                next += 1;
            }
            h.process();
            for key in ["out_l", "out_r"] {
                for s in h.output(key) {
                    prop_assert!(s.is_finite() && s.abs() <= 4.0, "{} Hz, {}: {}", sr, key, s);
                }
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(12))]
    /// The FM synth at 44.1 / 48 / 96 kHz: any params, CVs and random notes stay finite and
    /// |x| <= 4.
    #[test]
    fn qa_fm_synth_any_params_bounded_at_every_sample_rate(
        sr_index in 0usize..3,
        norms in prop::collection::vec(0.0f32..=1.0, fm_synth::DESCRIPTOR.params.len()),
        notes in prop::collection::vec((0usize..200, 0u16..64, 0u8..=127, 0.01f32..=1.0), 1..40),
        cv in -1.5f32..=1.5,
        seed in any::<u64>(),
    ) {
        let sr = [44_100.0, 48_000.0, 96_000.0][sr_index];
        let mut h = DeviceHarness::with_options(
            Box::new(fm_synth::FmSynth::new()), sr, 120.0, seed);
        for (p, n) in fm_synth::DESCRIPTOR.params.iter().zip(&norms) {
            h.set_param(p.key, p.from_normalized(*n));
        }
        for socket in ["pitch_cv_in", "index_cv_in"] {
            *h.input_mut(socket) = [cv; BATCH];
        }
        let mut notes = notes;
        notes.sort_by_key(|(b, o, _, _)| (*b, *o));
        let batches = (sr / BATCH as f64).ceil() as usize; // 1 s
        let mut next = 0;
        for b in 0..batches {
            while next < notes.len() && notes[next].0 == b {
                let (_, offset, key, velocity) = notes[next];
                h.push_note("notes_in", NoteEvent { offset, kind: NoteKind::On { key, velocity } });
                next += 1;
            }
            h.process();
            for key in ["out_l", "out_r"] {
                for s in h.output(key) {
                    prop_assert!(s.is_finite() && s.abs() <= 4.0, "{} Hz, {}: {}", sr, key, s);
                }
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(12))]
    /// The granular sampler at 44.1 / 48 / 96 kHz, on its built-in source or on a stereo sample
    /// map: any params, CVs, random notes, pitch bends and mod wheel stay finite and |x| <= 4.
    #[test]
    fn qa_granular_any_params_bounded_at_every_sample_rate(
        sr_index in 0usize..3,
        with_map in any::<bool>(),
        norms in prop::collection::vec(0.0f32..=1.0, granular::DESCRIPTOR.params.len()),
        notes in prop::collection::vec((0usize..200, 0u16..64, 0u8..=127, 0.01f32..=1.0), 1..40),
        bend in -1.0f32..=1.0,
        wheel in 0.0f32..=1.0,
        cv in -1.5f32..=1.5,
        seed in any::<u64>(),
    ) {
        let sr = [44_100.0, 48_000.0, 96_000.0][sr_index];
        let mut h = DeviceHarness::with_options(
            Box::new(granular::Granular::new()), sr, 120.0, seed);
        if with_map {
            let data: Vec<Vec<f32>> = [220.0f32, 331.0]
                .iter()
                .map(|f| (0..9_000).map(|i| 0.8 * (i as f32 * f * 0.000_13).sin()).collect())
                .collect();
            let buffer = std::sync::Arc::new(
                atomik_device_sdk::SampleBuffer::new(data.into()).unwrap());
            let zone = atomik_device_sdk::Zone {
                buffer: 0, key_lo: 0, key_hi: 127, vel_lo: 1, vel_hi: 127, root_key: 60,
                transpose: 0, tune_cents: 0.0, gain_db: 0.0, pan: 0.0, start: 100, end: 8_900,
                loop_mode: atomik_device_sdk::LoopMode::Off, loop_start: 0, loop_end: 0,
                crossfade: 0, trigger: atomik_device_sdk::TriggerMode::Gate, reverse: false,
            };
            let map = std::sync::Arc::new(
                atomik_device_sdk::SampleMap::new(vec![buffer], vec![zone]).unwrap());
            prop_assert!(h.device_mut().swap_samples(Some(map)).is_none());
        }
        for (p, n) in granular::DESCRIPTOR.params.iter().zip(&norms) {
            h.set_param(p.key, p.from_normalized(*n));
        }
        for socket in ["pitch_cv_in", "position_cv_in", "density_cv_in", "cutoff_cv_in", "level_cv_in"] {
            *h.input_mut(socket) = [cv; BATCH];
        }
        h.push_note("notes_in", NoteEvent { offset: 0, kind: NoteKind::PitchBend { value: bend } });
        h.push_note("notes_in", NoteEvent {
            offset: 0, kind: NoteKind::Cc { controller: 1, value: wheel } });
        let mut notes = notes;
        notes.sort_by_key(|(b, o, _, _)| (*b, *o));
        let batches = (sr / BATCH as f64).ceil() as usize; // 1 s
        let mut next = 0;
        for b in 0..batches {
            while next < notes.len() && notes[next].0 == b {
                let (_, offset, key, velocity) = notes[next];
                h.push_note("notes_in", NoteEvent { offset, kind: NoteKind::On { key, velocity } });
                next += 1;
            }
            h.process();
            for key in ["out_l", "out_r"] {
                for s in h.output(key) {
                    prop_assert!(s.is_finite() && s.abs() <= 4.0, "{} Hz, {}: {}", sr, key, s);
                }
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(12))]
    /// The guitar amp at 44.1 / 48 / 96 kHz: any params and a loud random guitar-like input stay
    /// finite and within the amp's output bound.
    #[test]
    fn qa_guitar_amp_any_params_bounded_at_every_sample_rate(
        sr_index in 0usize..3,
        norms in prop::collection::vec(0.0f32..=1.0, guitar_amp::DESCRIPTOR.params.len()),
        amp in 0.0f32..=2.0,
        bpm in 20.0f64..=300.0,
        seed in any::<u64>(),
    ) {
        let sr = [44_100.0, 48_000.0, 96_000.0][sr_index];
        let mut h = DeviceHarness::with_options(
            Box::new(guitar_amp::GuitarAmp::new()), sr, bpm, seed);
        for (p, n) in guitar_amp::DESCRIPTOR.params.iter().zip(&norms) {
            h.set_param(p.key, p.from_normalized(*n));
        }
        let mut rng = atomik_dsp::Rng::new(seed);
        let batches = (sr / BATCH as f64).ceil() as usize; // 1 s
        for _ in 0..batches {
            let mut x = [0.0f32; BATCH];
            for s in &mut x {
                *s = rng.next_bipolar() * amp;
            }
            *h.input_mut("in_l") = x;
            *h.input_mut("in_r") = x;
            h.process();
            for key in ["out_l", "out_r"] {
                for s in h.output(key) {
                    prop_assert!(s.is_finite() && s.abs() <= 16.0, "{} Hz, {}: {}", sr, key, s);
                }
            }
        }
    }
}
