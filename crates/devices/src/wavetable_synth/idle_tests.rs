//! The wavetable synth says it is quiescent only when no voice is active or waiting to start, nothing
//! reads its LFO and its gains sit on their targets, and a skipped stretch leaves the LFO where
//! processing it would (the position sweep, the vibrato and the filter of the next note start from
//! it), for every LFO shape. The panel's published position is the idle value, as a processed
//! silent batch leaves it.

use super::*;
use crate::test_support::{IdleRun, harness, off, on, skip_matches_process};
use atomik_core::{NoteEvent, NoteKind};
use atomik_testkit::harness::DeviceHarness;

fn quiet(h: &DeviceHarness) -> bool {
    h.device().quiescent()
}

fn synth() -> DeviceHarness {
    harness(Box::new(WavetableSynth::new()))
}

/// Processes silent batches until the synth says it is quiescent; returns how many it took.
fn batches_to_rest(h: &mut DeviceHarness, limit: usize) -> usize {
    for n in 0..limit {
        if quiet(h) {
            return n;
        }
        h.process();
    }
    panic!("the wavetable synth never came to rest in {limit} batches");
}

fn cc(controller: u8, value: f32) -> NoteEvent {
    NoteEvent {
        offset: 0,
        kind: NoteKind::Cc { controller, value },
    }
}

#[test]
fn it_is_quiescent_only_when_no_voice_is_active() {
    let mut h = synth();
    assert!(!quiet(&h), "nothing has run");
    let first = batches_to_rest(&mut h, 10);
    assert_eq!(first, 1, "the master gain is snapped by the first batch");
    h.push_note("notes_in", on(0, 60, 1.0));
    for _ in 0..40 {
        h.process();
        assert!(!quiet(&h), "a note sounds");
    }
    h.push_note("notes_in", off(0, 60));
    let n = batches_to_rest(&mut h, 2_000);
    assert!(
        (200..=240).contains(&n),
        "the default patch rests after {n} batches"
    );
    h.process();
    for key in ["out_l", "out_r"] {
        assert!(h.output(key).iter().all(|x| x.to_bits() == 0), "{key}");
    }
}

#[test]
fn a_stolen_voice_and_its_waiting_note_keep_it_awake() {
    let mut h = synth();
    h.set_param("voices", 1.0);
    h.set_param("env.amp.release", 1.0);
    batches_to_rest(&mut h, 10);
    h.push_note("notes_in", on(0, 60, 1.0));
    h.push_note("notes_in", on(5, 64, 1.0));
    h.process();
    assert!(!quiet(&h), "the note waits for the steal fade");
    h.push_note("notes_in", off(0, 64));
    batches_to_rest(&mut h, 200);
    h.process();
    for key in ["out_l", "out_r"] {
        assert!(h.output(key).iter().all(|x| x.to_bits() == 0), "{key}");
    }
}

#[test]
fn a_cabled_lfo_output_keeps_it_awake() {
    let mut h = synth();
    h.set_connected("lfo_cv_out", true);
    for _ in 0..50 {
        h.process();
        assert!(!quiet(&h), "the LFO output is a source");
    }
    h.set_connected("lfo_cv_out", false);
    batches_to_rest(&mut h, 10);
}

#[test]
fn the_published_position_is_the_idle_value_when_it_rests() {
    let mut h = synth();
    h.push_note("notes_in", on(0, 60, 1.0));
    for _ in 0..20 {
        h.process();
    }
    let mut m = [0.0f32; METER_COUNT];
    assert_eq!(h.device().meter_values(&mut m), METER_COUNT);
    assert!(m.iter().all(|v| *v >= 0.0), "a voice publishes {m:?}");
    h.push_note("notes_in", off(0, 60));
    batches_to_rest(&mut h, 2_000);
    assert_eq!(h.device().meter_values(&mut m), METER_COUNT);
    assert!(
        m.iter().all(|v| v.to_bits() == (-1.0f32).to_bits()),
        "{m:?}"
    );
}

#[test]
fn a_param_change_a_reset_and_a_new_table_clear_it() {
    let mut h = synth();
    batches_to_rest(&mut h, 10);
    h.set_param("master.level", -6.0);
    assert!(!quiet(&h));
    // The master gain glides over 5 ms.
    let n = batches_to_rest(&mut h, 20);
    assert!((1..=6).contains(&n), "{n}");
    h.set_param("lfo.to_pos1", 0.3);
    assert!(!quiet(&h));
    batches_to_rest(&mut h, 20);
    let _ = h.device_mut().swap_samples(None);
    assert!(!quiet(&h), "a swap clears it");
    batches_to_rest(&mut h, 20);
    h.device_mut().reset();
    assert!(!quiet(&h));
    batches_to_rest(&mut h, 20);
    h.push_note("notes_in", cc(11, 0.3));
    h.process();
    assert!(!quiet(&h), "the expression gain glides over 5 ms");
    batches_to_rest(&mut h, 20);
}

/// The scenario: notes (a chord with stealing at two voices, noise and sub), a long silence with
/// controller moves and a change of the master gain, and a position sweep, a vibrato and a filter
/// sweep that depend on where the LFO has got to when the keys come back.
fn wavetable_scenario(h: &mut DeviceHarness, n: usize) -> bool {
    let mut touched = true;
    match n {
        0 => {
            for (k, v) in [
                ("voices", 2.0),
                ("lfo.rate", 7.3),
                ("lfo.to_pitch", 2.0),
                ("lfo.to_cutoff", 0.5),
                ("lfo.to_pos1", 0.4),
                ("lfo.to_pos2", 0.3),
                ("mix.sub", 0.3),
                ("mix.noise", 0.1),
                ("unison.voices", 3.0),
                ("env.amp.release", 40.0),
            ] {
                h.set_param(k, v);
            }
        }
        4 => {
            h.push_note("notes_in", on(10, 60, 0.9));
            h.push_note("notes_in", on(30, 64, 0.7));
            h.push_note("notes_in", on(50, 67, 0.7));
        }
        50 => {
            h.push_note("notes_in", off(0, 60));
            h.push_note("notes_in", off(7, 64));
            h.push_note("notes_in", off(9, 67));
        }
        400 => h.push_note("notes_in", cc(1, 0.8)),
        450 => h.push_note("notes_in", cc(7, 0.5)),
        500 => h.push_note("notes_in", cc(64, 1.0)),
        520 => h.push_note("notes_in", cc(64, 0.0)),
        540 => h.push_note(
            "notes_in",
            NoteEvent {
                offset: 3,
                kind: NoteKind::PitchBend { value: 0.5 },
            },
        ),
        700 => h.set_param("master.level", -3.0),
        800 => {
            h.push_note("notes_in", on(0, 62, 1.0));
            h.push_note("notes_in", on(0, 65, 1.0));
        }
        860 => {
            h.push_note("notes_in", off(0, 62));
            h.push_note("notes_in", off(0, 65));
        }
        _ => touched = false,
    }
    touched
}

#[test]
fn skipping_the_idle_batches_changes_no_bit_for_any_lfo_shape() {
    for wave in [0.0, 1.0, 2.0, 3.0, 4.0] {
        for cabled in [false, true] {
            let make = || {
                let mut h = synth();
                h.set_param("lfo.wave", wave);
                h.set_connected("level_cv_in", cabled);
                h.set_connected("pan_cv_in", cabled);
                h.set_connected("resonance_cv_in", cabled);
                h
            };
            let skipped = skip_matches_process(
                &make,
                &IdleRun {
                    ins: &[],
                    outs: &["out_l", "out_r"],
                    batches: 1_200,
                    sound: &|_| false,
                    edit: &|n, h| wavetable_scenario(h, n),
                },
            );
            assert!(skipped > 600, "wave {wave}: {skipped} batches skipped");
        }
    }
}
