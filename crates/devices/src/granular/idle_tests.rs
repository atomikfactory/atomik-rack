//! The granular synth says it is quiescent only when no voice is active or waiting to start and its
//! gains sit on their targets, and a skipped stretch leaves the device LFO, the frame counter and
//! the panel values (every meter, the grain trail included) where processing it would, for every
//! LFO shape and with the LFO in each scope.

use super::*;
use crate::test_support::{IdleRun, demo_sample_map, harness, off, on, skip_matches_process};
use atomik_core::{NoteEvent, NoteKind};
use atomik_testkit::harness::DeviceHarness;

fn quiet(h: &DeviceHarness) -> bool {
    h.device().quiescent()
}

/// A granular synth with the built-in source and the default patch.
fn granular() -> DeviceHarness {
    harness(Box::new(Granular::new()))
}

/// Processes silent batches until the synth says it is quiescent; returns how many it took.
fn batches_to_rest(h: &mut DeviceHarness, limit: usize) -> usize {
    for n in 0..limit {
        if quiet(h) {
            return n;
        }
        h.process();
    }
    panic!("the granular synth never came to rest in {limit} batches");
}

fn cc(controller: u8, value: f32) -> NoteEvent {
    NoteEvent {
        offset: 0,
        kind: NoteKind::Cc { controller, value },
    }
}

#[test]
fn it_is_quiescent_only_when_no_voice_is_active() {
    let mut h = granular();
    assert!(!quiet(&h), "nothing has run");
    let first = batches_to_rest(&mut h, 10);
    assert_eq!(first, 1, "the master gain is snapped by the first batch");
    h.push_note("notes_in", on(0, 60, 1.0));
    for _ in 0..40 {
        h.process();
        assert!(!quiet(&h), "a note sounds");
    }
    h.push_note("notes_in", off(0, 60));
    // The default release is 400 ms (300 batches).
    let n = batches_to_rest(&mut h, 1_000);
    assert!(
        (290..=310).contains(&n),
        "the default patch rests after {n} batches"
    );
}

#[test]
fn a_stolen_voice_and_its_waiting_note_keep_it_awake() {
    let mut h = granular();
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
fn a_param_change_a_reset_and_a_map_swap_clear_it() {
    let mut h = granular();
    batches_to_rest(&mut h, 10);
    h.set_param("master.level", -6.0);
    assert!(!quiet(&h));
    // The master gain glides over 5 ms.
    let n = batches_to_rest(&mut h, 20);
    assert!((1..=6).contains(&n), "{n}");
    h.set_param("lfo.to_position", 0.3);
    assert!(!quiet(&h));
    batches_to_rest(&mut h, 20);
    let _ = h.device_mut().swap_samples(Some(demo_sample_map()));
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

#[test]
fn the_panel_values_of_a_skipped_stretch_are_those_of_a_processed_one() {
    let meters = |h: &DeviceHarness| {
        let mut m = [0.0f32; METER_COUNT];
        let n = h.device().meter_values(&mut m);
        assert_eq!(n, METER_COUNT);
        m.map(f32::to_bits)
    };
    let (mut full, mut skip) = (granular(), granular());
    for h in [&mut full, &mut skip] {
        h.set_param("env.amp.release", 5.0);
        h.set_param("grain.size", 400.0);
        h.push_note("notes_in", on(3, 60, 1.0));
    }
    for _ in 0..20 {
        full.process();
        skip.process();
    }
    for h in [&mut full, &mut skip] {
        h.push_note("notes_in", off(3, 60));
    }
    // Grains of up to 400 ms outlive the voice's release in the trail; the instance that skips
    // takes the quiescent batches from the moment it says so.
    let mut skipped = 0;
    for n in 0..1_200 {
        full.process();
        if quiet(&skip) && n > 20 {
            skip.device_mut().idle_advance(BATCH);
            skipped += 1;
        } else {
            skip.process();
        }
        assert_eq!(meters(&full), meters(&skip), "batch {n}");
    }
    assert!(skipped > 1_000, "{skipped}");
}

/// The scenario: notes that steal (two voices), a long silence with controller moves, a swap of
/// the map and a change of the master gain, and a note whose grains follow the LFO position,
/// size and pitch, which depend on where the device LFO has got to when the keys come back.
fn granular_scenario(h: &mut DeviceHarness, n: usize) -> bool {
    let mut touched = true;
    match n {
        0 => {
            for (k, v) in [
                ("voices", 2.0),
                ("lfo.rate", 7.3),
                ("lfo.to_position", 0.3),
                ("lfo.to_size", 0.5),
                ("lfo.to_pitch", 3.0),
                ("lfo.to_density", 0.4),
                ("pos.spray", 0.3),
                ("pitch.random", 20.0),
                ("grain.reverse", 0.3),
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
        600 => drop(h.device_mut().swap_samples(Some(demo_sample_map()))),
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
fn skipping_the_idle_batches_changes_no_bit_for_any_lfo_shape_and_scope() {
    for wave in [0.0, 1.0, 2.0, 3.0, 4.0] {
        for scope in [0.0, 1.0] {
            for cabled in [false, true] {
                let make = || {
                    let mut h = granular();
                    h.set_param("lfo.wave", wave);
                    h.set_param("lfo.scope", scope);
                    h.set_connected("level_cv_in", cabled);
                    h
                };
                let skipped = skip_matches_process(
                    &make,
                    &IdleRun {
                        ins: &[],
                        outs: &["out_l", "out_r"],
                        batches: 1_200,
                        sound: &|_| false,
                        edit: &|n, h| granular_scenario(h, n),
                    },
                );
                assert!(skipped > 600, "wave {wave}: {skipped} batches skipped");
            }
        }
    }
}
