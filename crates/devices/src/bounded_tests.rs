//! Bounded-output property tests for the instruments and larger effects not covered by the
//! per-device files: any parameter values, random notes and loud random audio keep every output
//! finite and inside a generous bound, at 44.1, 48 and 96 kHz. Every `process` runs under the
//! allocation guard (the harness does it).

use crate::test_support::{demo_sample_map, drum_sample_map, slicer_sample_map};
use atomik_core::{BATCH, NoteEvent, NoteKind};
use atomik_device_sdk::{Direction, Registry, SocketKind};
use atomik_dsp::Rng;
use atomik_testkit::harness::DeviceHarness;
use proptest::prelude::*;

/// Output bound: far above any musical level, far below a runaway.
const BOUND: f32 = 16.0;

/// One random scene.
#[derive(Debug, Clone)]
struct Scene {
    sr_index: usize,
    norms: Vec<f32>,
    notes: Vec<(usize, u16, u8, f32)>,
    amp: f32,
    bpm: f64,
    seed: u64,
}

fn scene() -> impl Strategy<Value = Scene> {
    (
        0usize..3,
        prop::collection::vec(0.0f32..=1.0, 256),
        prop::collection::vec((0usize..200, 0u16..64, 0u8..=127, 0.01f32..=1.0), 1..40),
        0.0f32..=2.0,
        20.0f64..=300.0,
        any::<u64>(),
    )
        .prop_map(|(sr_index, norms, notes, amp, bpm, seed)| Scene {
            sr_index,
            norms,
            notes,
            amp,
            bpm,
            seed,
        })
}

/// Plays `scene` through device `id` for one second and checks every audio output.
fn check_bounded(id: &str, s: &Scene) -> Result<(), TestCaseError> {
    check_bounded_to(id, s, BOUND)
}

/// [`check_bounded`] with its own output bound.
fn check_bounded_to(id: &str, s: &Scene, bound: f32) -> Result<(), TestCaseError> {
    let sr = [44_100.0, 48_000.0, 96_000.0][s.sr_index];
    let mut r = Registry::new();
    crate::register_all(&mut r).unwrap();
    let mut h = DeviceHarness::with_options(r.create(id).unwrap(), sr, s.bpm, s.seed);
    if r.takes_samples(id) {
        let map = match id {
            "atomik.drum-machine" => drum_sample_map(),
            "atomik.loop-slicer" => slicer_sample_map(),
            _ => demo_sample_map(),
        };
        prop_assert!(h.device_mut().swap_samples(Some(map)).is_none());
    }
    let d = h.device().descriptor();
    for (p, n) in d.params.iter().zip(&s.norms) {
        h.set_param(p.key, p.from_normalized(*n));
    }
    let outs: Vec<&str> = d
        .sockets
        .iter()
        .filter(|k| k.direction == Direction::Output && k.kind == SocketKind::Audio)
        .map(|k| k.key)
        .collect();
    let ins: Vec<&str> = d
        .sockets
        .iter()
        .filter(|k| k.direction == Direction::Input && k.kind == SocketKind::Audio)
        .map(|k| k.key)
        .collect();
    let notes_in = d
        .sockets
        .iter()
        .find(|k| k.direction == Direction::Input && k.kind == SocketKind::Notes)
        .map(|k| k.key);
    let key_of = |key: u8| match id {
        "atomik.drum-machine" => 36 + key % 16,
        "atomik.loop-slicer" => 24 + key % 5,
        _ => key,
    };
    let mut notes = s.notes.clone();
    notes.sort_by_key(|(b, o, _, _)| (*b, *o));
    let mut rng = Rng::new(s.seed);
    let batches = (sr / BATCH as f64).ceil() as usize; // 1 s
    let mut next = 0;
    for b in 0..batches {
        while next < notes.len() && notes[next].0 == b {
            let (_, offset, key, velocity) = notes[next];
            if let Some(socket) = notes_in {
                let kind = NoteKind::On {
                    key: key_of(key),
                    velocity,
                };
                h.push_note(socket, NoteEvent { offset, kind });
            }
            next += 1;
        }
        for key in &ins {
            let mut x = [0.0f32; BATCH];
            for v in &mut x {
                *v = rng.next_bipolar() * s.amp;
            }
            *h.input_mut(key) = x;
        }
        h.process();
        for key in &outs {
            for v in h.output(key) {
                prop_assert!(
                    v.is_finite() && v.abs() <= bound,
                    "{id} {sr} Hz, {key}: {v}"
                );
            }
        }
    }
    Ok(())
}

/// Plays `scene` through a CV utility for one second with loud random signals on every audio and
/// CV input (a CV input is random too, so gates and triggers chatter) and checks every output
/// socket, audio and CV alike.
fn check_cv_bounded(id: &str, s: &Scene) -> Result<(), TestCaseError> {
    let sr = [44_100.0, 48_000.0, 96_000.0][s.sr_index];
    let mut r = Registry::new();
    crate::register_all(&mut r).unwrap();
    let mut h = DeviceHarness::with_options(r.create(id).unwrap(), sr, s.bpm, s.seed);
    let d = h.device().descriptor();
    for (p, n) in d.params.iter().zip(&s.norms) {
        h.set_param(p.key, p.from_normalized(*n));
    }
    let outs: Vec<&str> = d
        .sockets
        .iter()
        .filter(|k| k.direction == Direction::Output && k.kind != SocketKind::Notes)
        .map(|k| k.key)
        .collect();
    let ins: Vec<&str> = d
        .sockets
        .iter()
        .filter(|k| k.direction.is_input() && k.kind != SocketKind::Notes)
        .map(|k| k.key)
        .collect();
    let mut rng = Rng::new(s.seed);
    let batches = (sr / BATCH as f64).ceil() as usize; // 1 s
    for _ in 0..batches {
        for key in &ins {
            let mut x = [0.0f32; BATCH];
            for v in &mut x {
                *v = rng.next_bipolar() * s.amp * 4.0;
            }
            *h.input_mut(key) = x;
        }
        h.process();
        for key in &outs {
            for v in h.output(key) {
                prop_assert!(
                    v.is_finite() && v.abs() <= BOUND,
                    "{id} {sr} Hz, {key}: {v}"
                );
            }
        }
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(12))]

    #[test]
    fn qa_semi_modular_any_params_bounded_at_every_sample_rate(s in scene()) {
        check_bounded("atomik.semi-modular", &s)?;
    }

    /// The sampler: any params and notes over a loaded map.
    #[test]
    fn qa_sampler_any_params_bounded_at_every_sample_rate(s in scene()) {
        check_bounded("atomik.sampler", &s)?;
    }

    /// The drum machine: any params and pad hits over a loaded map.
    #[test]
    fn qa_drum_machine_any_params_bounded_at_every_sample_rate(s in scene()) {
        check_bounded("atomik.drum-machine", &s)?;
    }

    /// The loop slicer: any params and slice keys over a loaded map.
    #[test]
    fn qa_loop_slicer_any_params_bounded_at_every_sample_rate(s in scene()) {
        check_bounded("atomik.loop-slicer", &s)?;
    }

    /// The mono bass: any params and notes.
    #[test]
    fn qa_mono_bass_any_params_bounded_at_every_sample_rate(s in scene()) {
        check_bounded("atomik.mono-bass", &s)?;
    }

    /// The organ: any params and notes.
    #[test]
    fn qa_organ_any_params_bounded_at_every_sample_rate(s in scene()) {
        check_bounded("atomik.organ", &s)?;
    }

    /// The dual layer: any params (both layers) and notes.
    #[test]
    fn qa_dual_layer_any_params_bounded_at_every_sample_rate(s in scene()) {
        check_bounded("atomik.dual-layer", &s)?;
    }

    /// The reverb: any params and loud noise.
    #[test]
    fn qa_reverb_any_params_bounded_at_every_sample_rate(s in scene()) {
        check_bounded("atomik.reverb", &s)?;
    }

    /// The mastering effect: any params and loud noise.
    #[test]
    fn qa_mastering_any_params_bounded_at_every_sample_rate(s in scene()) {
        check_bounded("atomik.mastering", &s)?;
    }

    /// Character: any params and loud noise.
    #[test]
    fn qa_character_any_params_bounded_at_every_sample_rate(s in scene()) {
        check_bounded("atomik.character", &s)?;
    }

    /// The auto-filter: any params, resonant sweeps and loud noise.
    #[test]
    fn qa_auto_filter_any_params_bounded_at_every_sample_rate(s in scene()) {
        check_bounded("atomik.auto-filter", &s)?;
    }

    /// The de-esser: any params and loud noise.
    #[test]
    fn qa_de_esser_any_params_bounded_at_every_sample_rate(s in scene()) {
        check_bounded("atomik.de-esser", &s)?;
    }

    /// The dynamic EQ: any params and loud noise (five boosted bands may stack up to a large
    /// but finite gain, so the bound is the stability bound).
    #[test]
    fn qa_dynamic_eq_any_params_bounded_at_every_sample_rate(s in scene()) {
        check_bounded_to("atomik.dynamic-eq", &s, 1.0e6)?;
    }

    /// The transient shaper: any params and loud noise.
    #[test]
    fn qa_transient_shaper_any_params_bounded_at_every_sample_rate(s in scene()) {
        check_bounded("atomik.transient-shaper", &s)?;
    }

    /// The rotary speaker: any params and loud noise.
    #[test]
    fn qa_rotary_speaker_any_params_bounded_at_every_sample_rate(s in scene()) {
        check_bounded("atomik.rotary-speaker", &s)?;
    }

    /// The pitch shifter: any params and loud noise.
    #[test]
    fn qa_pitch_shifter_any_params_bounded_at_every_sample_rate(s in scene()) {
        check_bounded("atomik.pitch-shifter", &s)?;
    }

    /// The tuner: any params and loud noise.
    #[test]
    fn qa_tuner_any_params_bounded_at_every_sample_rate(s in scene()) {
        check_bounded("atomik.tuner", &s)?;
    }

    /// The CV utilities: any params and loud random signals on every input.
    #[test]
    fn qa_cv_math_any_params_bounded_at_every_sample_rate(s in scene()) {
        check_cv_bounded("atomik.cv-math", &s)?;
    }

    #[test]
    fn qa_cv_quantizer_any_params_bounded_at_every_sample_rate(s in scene()) {
        check_cv_bounded("atomik.cv-quantizer", &s)?;
    }

    #[test]
    fn qa_sample_hold_any_params_bounded_at_every_sample_rate(s in scene()) {
        check_cv_bounded("atomik.sample-hold", &s)?;
    }

    #[test]
    fn qa_slew_limiter_any_params_bounded_at_every_sample_rate(s in scene()) {
        check_cv_bounded("atomik.slew-limiter", &s)?;
    }

    #[test]
    fn qa_gate_processor_any_params_bounded_at_every_sample_rate(s in scene()) {
        check_cv_bounded("atomik.gate-processor", &s)?;
    }

    #[test]
    fn qa_envelope_follower_any_params_bounded_at_every_sample_rate(s in scene()) {
        check_cv_bounded("atomik.envelope-follower", &s)?;
    }
}
