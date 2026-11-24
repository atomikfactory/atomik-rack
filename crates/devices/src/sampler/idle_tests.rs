//! The sampler says it is quiescent only when no voice is active or waiting to start and its gains
//! sit on their targets, and a skipped stretch leaves the LFO where processing it would (the
//! vibrato and the tremolo of the next note start from it), for every LFO shape.

use super::*;
use crate::test_support::{IdleRun, demo_sample_map, harness, off, on, skip_matches_process};
use atomik_core::{NoteEvent, NoteKind};
use atomik_testkit::harness::DeviceHarness;

fn quiet(h: &DeviceHarness) -> bool {
    h.device().quiescent()
}

/// A sampler with the demo map (three layered zones on keys 60 to 68: two loops and a one-shot of
/// 3,990 frames) and the default patch.
fn demo() -> DeviceHarness {
    let mut h = harness(Box::new(Sampler::new()));
    assert!(
        h.device_mut()
            .swap_samples(Some(demo_sample_map()))
            .is_none()
    );
    h
}

/// Processes silent batches until the sampler says it is quiescent; returns how many it took.
fn batches_to_rest(h: &mut DeviceHarness, limit: usize) -> usize {
    for n in 0..limit {
        if quiet(h) {
            return n;
        }
        h.process();
    }
    panic!("the sampler never came to rest in {limit} batches");
}

fn cc(controller: u8, value: f32) -> NoteEvent {
    NoteEvent {
        offset: 0,
        kind: NoteKind::Cc { controller, value },
    }
}

#[test]
fn it_is_quiescent_only_when_no_voice_is_active() {
    let mut h = demo();
    assert!(!quiet(&h), "nothing has run");
    let first = batches_to_rest(&mut h, 10);
    assert_eq!(first, 1, "the master gain is snapped by the first batch");
    h.push_note("notes_in", on(0, 64, 1.0));
    for _ in 0..40 {
        h.process();
        assert!(!quiet(&h), "the loops sound");
    }
    h.push_note("notes_in", off(0, 64));
    // The loops release over the default 150 ms (113 batches); the one-shot ignores the
    // note-off and plays on to its end (63 batches from its start), which is earlier.
    let n = batches_to_rest(&mut h, 1_000);
    assert!(
        (105..=125).contains(&n),
        "the default patch rests after {n} batches"
    );
    // A one-shot alone runs to its end whatever the note-off says.
    let mut h = demo();
    h.set_param("env.amp.release", 0.5);
    h.push_note("notes_in", on(0, 64, 1.0));
    h.push_note("notes_in", off(0, 64));
    let n = batches_to_rest(&mut h, 1_000);
    assert!((60..=70).contains(&n), "one-shot {n}");
}

#[test]
fn a_stolen_voice_and_its_waiting_note_keep_it_awake() {
    let mut h = demo();
    h.set_param("voices", 1.0);
    h.set_param("env.amp.release", 1.0);
    batches_to_rest(&mut h, 10);
    // Three zones on one voice: the second and third zone steal it and wait for its fade.
    h.push_note("notes_in", on(0, 64, 1.0));
    h.process();
    assert!(!quiet(&h));
    h.push_note("notes_in", off(0, 64));
    batches_to_rest(&mut h, 200);
    h.push_note("notes_in", on(0, 64, 1.0));
    h.push_note("notes_in", on(5, 62, 1.0));
    h.process();
    assert!(!quiet(&h));
    // Only the one-shot zone, the last to take the voice, plays on to its end (63 batches).
    let n = batches_to_rest(&mut h, 300);
    assert!((60..=90).contains(&n), "{n} batches");
    h.process();
    for key in ["out_l", "out_r"] {
        assert!(h.output(key).iter().all(|x| x.to_bits() == 0), "{key}");
    }
}

#[test]
fn a_param_change_a_reset_and_a_map_swap_clear_it() {
    let mut h = demo();
    batches_to_rest(&mut h, 10);
    h.set_param("master.level", -6.0);
    assert!(!quiet(&h));
    // The master gain glides over 5 ms.
    let n = batches_to_rest(&mut h, 20);
    assert!((1..=6).contains(&n), "{n}");
    h.set_param("lfo.to_pitch", 3.0);
    assert!(!quiet(&h));
    batches_to_rest(&mut h, 20);
    let kept = demo_sample_map();
    let _ = h.device_mut().swap_samples(Some(kept));
    assert!(
        !quiet(&h),
        "the engine asks again after a swap, and so does the device"
    );
    batches_to_rest(&mut h, 20);
    h.device_mut().reset();
    assert!(!quiet(&h));
    // A held volume or expression is a settled gain; one still gliding is not.
    batches_to_rest(&mut h, 20);
    h.push_note("notes_in", cc(7, 0.3));
    h.process();
    assert!(!quiet(&h), "the volume gain glides over 5 ms");
    batches_to_rest(&mut h, 20);
}

/// The scenario: layered notes that steal (two voices), a long silence with controller moves, a
/// swap of the map, a change of the master gain and a vibrato and tremolo that depend on where the LFO
/// has got to when the keys come back.
fn sampler_scenario(h: &mut DeviceHarness, n: usize) -> bool {
    let mut touched = true;
    match n {
        0 => {
            for (k, v) in [
                ("voices", 2.0),
                ("lfo.rate", 7.3),
                ("lfo.to_pitch", 4.0),
                ("lfo.to_amp", 0.8),
                ("env.amp.release", 40.0),
            ] {
                h.set_param(k, v);
            }
        }
        4 => {
            h.push_note("notes_in", on(10, 64, 0.9));
            h.push_note("notes_in", on(30, 62, 0.7));
        }
        50 => {
            h.push_note("notes_in", off(0, 64));
            h.push_note("notes_in", off(7, 62));
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
            h.push_note("notes_in", on(0, 64, 1.0));
            h.push_note("notes_in", on(0, 61, 1.0));
            h.push_note("notes_in", on(100 % 64, 66, 1.0));
        }
        860 => {
            h.push_note("notes_in", off(0, 64));
            h.push_note("notes_in", off(0, 61));
            h.push_note("notes_in", off(0, 66));
        }
        _ => touched = false,
    }
    touched
}

#[test]
fn skipping_the_idle_batches_changes_no_bit_for_any_lfo_shape_and_a_cabled_level_cv() {
    for wave in [0.0, 1.0, 2.0, 3.0, 4.0] {
        for cabled in [false, true] {
            let make = || {
                let mut h = demo();
                h.set_param("lfo.wave", wave);
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
                    edit: &|n, h| sampler_scenario(h, n),
                },
            );
            assert!(skipped > 600, "wave {wave}: {skipped} batches skipped");
        }
    }
}
