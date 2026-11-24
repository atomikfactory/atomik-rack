//! The FM synth says it is quiescent only when no voice is active or waiting to start, nothing
//! reads its LFO, its gains sit on their targets and the stages behind the voices (the
//! oversampler's decimators and the chorus) hold only zeros, and a skipped stretch leaves the two
//! LFOs where processing it would, for every LFO shape and oversampling factor.

use super::*;
use crate::test_support::{IdleRun, harness, off, on, skip_matches_process};
use atomik_core::{NoteEvent, NoteKind};
use atomik_testkit::harness::DeviceHarness;

fn quiet(h: &DeviceHarness) -> bool {
    h.device().quiescent()
}

fn synth() -> DeviceHarness {
    harness(Box::new(FmSynth::new()))
}

/// Processes silent batches until the synth says it is quiescent; returns how many it took.
fn batches_to_rest(h: &mut DeviceHarness, limit: usize) -> usize {
    for n in 0..limit {
        if quiet(h) {
            return n;
        }
        h.process();
    }
    panic!("the FM synth never came to rest in {limit} batches");
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
fn the_decimators_and_the_chorus_keep_it_awake_until_they_are_empty() {
    for oversample in [0.0, 1.0, 2.0] {
        for chorus in [0.0, 0.5] {
            let mut h = synth();
            h.set_param("oversample", oversample);
            h.set_param("chorus.mix", chorus);
            h.set_param("op1.release", 1.0);
            h.set_param("op2.release", 1.0);
            batches_to_rest(&mut h, 20);
            h.push_note("notes_in", on(0, 60, 1.0));
            for _ in 0..30 {
                h.process();
            }
            h.push_note("notes_in", off(0, 60));
            let n = batches_to_rest(&mut h, 2_000);
            // The release is 1 ms; the chorus ring (30 ms and a margin, about 24 batches) and
            // the decimators (a block of silence) are what is left.
            if chorus > 0.0 {
                assert!((20..=40).contains(&n), "chorus on: {n} batches");
            } else {
                assert!(n <= 4, "over {oversample}, chorus off: {n} batches");
            }
            h.process();
            for key in ["out_l", "out_r"] {
                assert!(h.output(key).iter().all(|x| x.to_bits() == 0), "{key}");
            }
        }
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
fn a_param_change_and_a_reset_clear_it() {
    let mut h = synth();
    batches_to_rest(&mut h, 10);
    h.set_param("master.level", -6.0);
    assert!(!quiet(&h));
    // The master gain glides over 5 ms.
    let n = batches_to_rest(&mut h, 20);
    assert!((1..=6).contains(&n), "{n}");
    h.set_param("lfo.to_pitch", 3.0);
    assert!(!quiet(&h));
    batches_to_rest(&mut h, 20);
    h.device_mut().reset();
    assert!(!quiet(&h));
    batches_to_rest(&mut h, 20);
    h.push_note("notes_in", cc(11, 0.3));
    h.process();
    assert!(!quiet(&h), "the expression gain glides over 5 ms");
    batches_to_rest(&mut h, 20);
}

/// The scenario: notes (a chord, with stealing at two voices, unison copies and a delayed
/// operator), a long silence with controller moves and a change of the master gain, and a vibrato,
/// a tremolo and a modulation index that depend on where the LFO has got to when the keys come
/// back.
fn fm_scenario(h: &mut DeviceHarness, n: usize) -> bool {
    let mut touched = true;
    match n {
        0 => {
            for (k, v) in [
                ("voices", 2.0),
                ("lfo.rate", 7.3),
                ("lfo.to_pitch", 2.0),
                ("lfo.to_amp", 0.6),
                ("lfo.to_index", 0.4),
                ("chorus.mix", 0.4),
                ("chorus.rate", 1.3),
                ("unison.voices", 3.0),
                ("pitch.env_amount", 5.0),
                ("op1.release", 40.0),
                ("op2.release", 40.0),
                ("op2.delay", 5.0),
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
fn skipping_the_idle_batches_changes_no_bit_for_any_lfo_shape_and_oversampling() {
    for wave in [0.0, 1.0, 2.0, 3.0, 4.0] {
        for oversample in [0.0, 1.0, 2.0] {
            for cabled in [false, true] {
                let make = || {
                    let mut h = synth();
                    h.set_param("lfo.wave", wave);
                    h.set_param("oversample", oversample);
                    h.set_connected("level_cv_in", cabled);
                    h.set_connected("pan_cv_in", cabled);
                    h
                };
                let skipped = skip_matches_process(
                    &make,
                    &IdleRun {
                        ins: &[],
                        outs: &["out_l", "out_r"],
                        batches: 1_200,
                        sound: &|_| false,
                        edit: &|n, h| fm_scenario(h, n),
                    },
                );
                assert!(skipped > 600, "wave {wave}: {skipped} batches skipped");
            }
        }
    }
}
