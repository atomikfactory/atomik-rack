//! The poly synth says it is quiescent only when no voice is active or waiting to start, nothing
//! reads LFO 1 through a cable, its gains sit on their targets and the stages behind the voices (the
//! chorus and the echo) hold only zeros, and a skipped stretch leaves the three LFOs where
//! processing it would: LFO 2 moves only while it has a destination, so a stretch skipped before
//! a destination is set must not have moved it. The mono bass is this engine under another
//! descriptor and is checked too.

use super::*;
use crate::test_support::{IdleRun, harness, off, on, skip_matches_process};
use atomik_core::{NoteEvent, NoteKind};
use atomik_testkit::harness::DeviceHarness;

fn quiet(h: &DeviceHarness) -> bool {
    h.device().quiescent()
}

fn synth() -> DeviceHarness {
    harness(Box::new(PolySynth::new()))
}

/// Processes silent batches until the synth says it is quiescent; returns how many it took.
fn batches_to_rest(h: &mut DeviceHarness, limit: usize) -> usize {
    for n in 0..limit {
        if quiet(h) {
            return n;
        }
        h.process();
    }
    panic!("the poly synth never came to rest in {limit} batches");
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
fn the_chorus_and_the_echo_keep_it_awake_until_their_lines_are_empty() {
    for (chorus, echo) in [(0.0, 0.0), (0.5, 0.0), (0.0, 0.4), (0.5, 0.4)] {
        let mut h = synth();
        h.set_param("chorus.mix", chorus);
        h.set_param("delay.mix", echo);
        // One pass through the ring: the repeats of the default feedback are tested below.
        h.set_param("delay.feedback", 0.0);
        h.set_param("env.amp.release", 1.0);
        batches_to_rest(&mut h, 20);
        h.push_note("notes_in", on(0, 60, 1.0));
        for _ in 0..30 {
            h.process();
        }
        h.push_note("notes_in", off(0, 60));
        let n = batches_to_rest(&mut h, 5_000);
        // The release is 1 ms; the chorus ring (30 ms, about 25 batches) and the echo ring
        // (65,536 samples, 1,024 batches) are what is left.
        let (lo, hi) = match (chorus > 0.0, echo > 0.0) {
            (false, false) => (0, 4),
            (true, false) => (20, 40),
            (_, true) => (1_000, 1_100),
        };
        assert!(
            (lo..=hi).contains(&n),
            "chorus {chorus}, echo {echo}: {n} batches"
        );
        h.process();
        for key in ["out_l", "out_r"] {
            assert!(h.output(key).iter().all(|x| x.to_bits() == 0), "{key}");
        }
    }
}

#[test]
fn the_repeats_of_the_echo_keep_it_awake_until_they_are_flushed_to_zero() {
    let mut h = synth();
    h.set_param("delay.mix", 0.4);
    h.set_param("env.amp.release", 1.0);
    batches_to_rest(&mut h, 20);
    h.push_note("notes_in", on(0, 60, 1.0));
    for _ in 0..30 {
        h.process();
    }
    h.push_note("notes_in", off(0, 60));
    // Every repeat is 0.35 of the one before and the line flushes below 1e-20: about 43 repeats of
    // 0.3 s (9,700 batches) after the last note.
    let n = batches_to_rest(&mut h, 20_000);
    assert!((9_000..=10_500).contains(&n), "{n} batches");
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

/// The scenario: a chord (with stealing at two voices), a long silence with controller moves, a
/// change of the master gain and the LFO 2 depths, and a chord whose vibrato, filter sweep, tremolo
/// and chorus depend on where the LFOs have got to when the keys come back.
fn poly_scenario(h: &mut DeviceHarness, n: usize, late_lfo2: bool) -> bool {
    let mut touched = true;
    match n {
        0 => {
            for (k, v) in [
                ("voices", 2.0),
                ("lfo.rate", 7.3),
                ("lfo.to_pitch", 2.0),
                ("lfo.to_cutoff", 0.5),
                ("lfo2.wave", 4.0),
                ("lfo2.rate", 5.1),
                ("noise.level", 0.1),
                ("drift", 0.5),
                ("spread", 0.4),
                ("chorus.mix", 0.4),
                ("chorus.rate", 1.3),
                ("delay.mix", 0.3),
                ("delay.time", 40.0),
                ("delay.feedback", 0.01),
                ("env.amp.release", 40.0),
            ] {
                h.set_param(k, v);
            }
            if !late_lfo2 {
                h.set_param("lfo2.to_pitch", 1.0);
                h.set_param("lfo2.to_level", 0.3);
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
        1_500 => h.push_note("notes_in", cc(1, 0.8)),
        1_700 => h.push_note("notes_in", cc(7, 0.5)),
        1_800 => h.push_note("notes_in", cc(64, 1.0)),
        1_850 => h.push_note("notes_in", cc(64, 0.0)),
        1_900 => h.push_note(
            "notes_in",
            NoteEvent {
                offset: 3,
                kind: NoteKind::PitchBend { value: 0.5 },
            },
        ),
        2_000 => h.set_param("master.level", -3.0),
        2_200 if late_lfo2 => {
            // LFO 2 waited while it had no destination; from here it runs.
            h.set_param("lfo2.to_pitch", 1.0);
            h.set_param("lfo2.to_level", 0.3);
        }
        2_600 => {
            h.push_note("notes_in", on(0, 62, 1.0));
            h.push_note("notes_in", on(0, 65, 1.0));
        }
        2_660 => {
            h.push_note("notes_in", off(0, 62));
            h.push_note("notes_in", off(0, 65));
        }
        _ => touched = false,
    }
    touched
}

fn run_scenario(make: &dyn Fn() -> DeviceHarness, late_lfo2: bool) -> usize {
    skip_matches_process(
        make,
        &IdleRun {
            ins: &[],
            outs: &["out_l", "out_r"],
            batches: 4_200,
            sound: &|_| false,
            edit: &|n, h| poly_scenario(h, n, late_lfo2),
        },
    )
}

#[test]
fn skipping_the_idle_batches_changes_no_bit_for_any_lfo_shape_and_lfo_2_that_waits() {
    for wave in [0.0, 1.0, 2.0, 3.0, 4.0] {
        for late_lfo2 in [false, true] {
            for cabled in [false, true] {
                let make = || {
                    let mut h = synth();
                    h.set_param("lfo.wave", wave);
                    h.set_connected("level_cv_in", cabled);
                    h.set_connected("pan_cv_in", cabled);
                    h.set_connected("resonance_cv_in", cabled);
                    h
                };
                let skipped = run_scenario(&make, late_lfo2);
                assert!(
                    skipped > 1_000,
                    "wave {wave}, late LFO 2 {late_lfo2}: {skipped} batches skipped"
                );
            }
        }
    }
}

#[test]
fn the_mono_bass_skips_the_same_way() {
    let make = || harness(Box::new(crate::mono_bass::create()));
    for late_lfo2 in [false, true] {
        let skipped = run_scenario(&make, late_lfo2);
        assert!(skipped > 1_000, "{skipped} batches skipped");
    }
}
