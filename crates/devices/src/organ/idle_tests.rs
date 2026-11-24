//! The organ says it is quiescent only when nothing sounds and every state is at rest, and a
//! skipped stretch leaves the scanner phase where processing it would: the vibrato is heard the
//! moment a key returns, so the scenario runs it with the chorus up.

use super::*;
use crate::test_support::{harness, off, on};
use atomik_testkit::harness::DeviceHarness;

fn quiet(h: &DeviceHarness) -> bool {
    h.device().quiescent()
}

/// Processes silent batches until the organ says it is quiescent; returns how many it took.
fn batches_to_rest(h: &mut DeviceHarness, limit: usize) -> usize {
    for n in 0..limit {
        if quiet(h) {
            return n;
        }
        h.process();
    }
    panic!("the organ never came to rest in {limit} batches");
}

fn organ() -> DeviceHarness {
    harness(Box::new(Organ::new()))
}

#[test]
fn it_is_quiescent_only_when_no_key_sounds_and_every_tail_has_ended() {
    let mut h = organ();
    assert!(!quiet(&h), "nothing has run");
    let first = batches_to_rest(&mut h, 100);
    assert!(first <= 8, "a fresh organ rests after {first} batches");
    h.push_note("notes_in", on(0, 60, 1.0));
    h.push_note("notes_in", on(0, 64, 1.0));
    for _ in 0..40 {
        h.process();
        assert!(!quiet(&h), "a key sounds");
    }
    h.push_note("notes_in", off(0, 60));
    h.push_note("notes_in", off(0, 64));
    // The release is 4 ms down to 1e-4 (about 28 batches), the click is spent sooner and the
    // phasors and the scan lines are at rest a few batches later.
    let mut n = 0;
    while !quiet(&h) {
        h.process();
        n += 1;
        assert!(n < 100, "never at rest");
    }
    assert!(
        (24..=40).contains(&n),
        "the default patch rests after {n} batches"
    );
    // Held keys never rest, a sustained release neither.
    h.push_note("notes_in", on(0, 60, 1.0));
    h.process();
    assert!(!quiet(&h));
    let pedal = |value| NoteEvent {
        offset: 0,
        kind: NoteKind::Cc {
            controller: 64,
            value,
        },
    };
    h.push_note("notes_in", pedal(1.0));
    h.push_note("notes_in", off(0, 60));
    for _ in 0..200 {
        h.process();
        assert!(!quiet(&h), "the pedal holds the key");
    }
    h.push_note("notes_in", pedal(0.0));
    batches_to_rest(&mut h, 200);
}

#[test]
fn the_percussion_tail_does_not_keep_it_awake_but_the_release_does() {
    let mut h = organ();
    h.set_param("perc.mode", 1.0);
    h.set_param("perc.decay", 1.0);
    h.set_param("scanner.type", 6.0);
    h.push_note("notes_in", on(0, 60, 1.0));
    h.push_note("notes_in", off(10, 60));
    h.process();
    // The slow decay (spent at 1e-5 after 1.5 s, about 560 batches) is advanced by `idle_advance`.
    let n = batches_to_rest(&mut h, 2_000);
    assert!((14..=40).contains(&n), "{n} batches");
}

#[test]
fn a_param_change_and_a_reset_clear_it() {
    let mut h = organ();
    batches_to_rest(&mut h, 100);
    assert!(quiet(&h));
    h.set_param("scanner.rate", 5.0);
    assert!(!quiet(&h));
    batches_to_rest(&mut h, 100);
    h.set_param("drawbar.4", 6.0);
    assert!(!quiet(&h));
    // The drawbar glides over 10 ms.
    let n = batches_to_rest(&mut h, 100);
    assert!((1..=10).contains(&n), "{n}");
    h.device_mut().reset();
    assert!(!quiet(&h));
}

#[test]
fn a_held_mod_wheel_on_the_rotary_speed_is_motion_and_a_zero_one_is_not() {
    let wheel = |value| NoteEvent {
        offset: 0,
        kind: NoteKind::Cc {
            controller: 1,
            value,
        },
    };
    let mut h = organ();
    h.set_param("wheel.target", 2.0);
    h.set_connected("wheel_cv_out", true);
    batches_to_rest(&mut h, 100);
    h.push_note("notes_in", wheel(0.7));
    for _ in 0..100 {
        h.process();
        assert!(!quiet(&h), "the wheel output holds 0.7");
        assert!(h.output("wheel_cv_out").iter().all(|v| *v > 0.69));
    }
    h.push_note("notes_in", wheel(0.0));
    batches_to_rest(&mut h, 100);
    // On the scanner depth the held wheel is a settled smoother target: it rests.
    h.set_param("wheel.target", 1.0);
    h.set_param("scanner.type", 4.0);
    h.push_note("notes_in", wheel(0.7));
    batches_to_rest(&mut h, 100);
}

#[test]
fn a_changed_tuning_is_taken_by_the_next_batch_that_runs() {
    let mut h = organ();
    batches_to_rest(&mut h, 100);
    h.time_mut().tune_cents = 30.0;
    // The engine runs the batch of a changed transport; the device retunes there and rests again.
    h.process();
    batches_to_rest(&mut h, 100);
}

/// The organ scenario: three keys, a long silence with a mod wheel move, a scanner change and a
/// change of the percussion in it, then keys again whose vibrato depends on where the scanner
/// phase has got to.
fn organ_scenario(h: &mut DeviceHarness, n: usize) -> bool {
    let wheel = |value| NoteEvent {
        offset: 0,
        kind: NoteKind::Cc {
            controller: 1,
            value,
        },
    };
    let mut touched = true;
    match n {
        0 => {
            for (k, v) in [
                ("scanner.type", 6.0),
                ("scanner.rate", 7.3),
                ("out.drive", 0.4),
                ("key.click", 0.5),
                ("wheel.target", 1.0),
                ("drawbar.4", 5.0),
                ("perc.mode", 2.0),
                ("perc.decay", 1.0),
            ] {
                h.set_param(k, v);
            }
        }
        5 => {
            for k in [48u8, 55, 64] {
                h.push_note("notes_in", on(40, k, 0.9));
            }
        }
        60 => {
            for k in [48u8, 55, 64] {
                h.push_note("notes_in", off(7, k));
            }
        }
        400 => h.push_note("notes_in", wheel(0.0)),
        500 => h.push_note("notes_in", wheel(0.9)),
        600 => h.set_param("scanner.type", 4.0),
        700 => h.set_param("scanner.rate", 8.1),
        800 => {
            h.push_note("notes_in", on(0, 60, 1.0));
            h.push_note("notes_in", on(100, 67, 1.0));
        }
        860 => h.push_note("notes_in", off(0, 60)),
        1_100 => h.set_param("wheel.target", 2.0),
        1_200 => h.push_note("notes_in", wheel(0.0)),
        1_300 => h.push_note("notes_in", on(3, 52, 1.0)),
        _ => touched = false,
    }
    touched
}

#[test]
fn skipping_the_idle_batches_changes_no_bit_and_no_meter() {
    let skipped = crate::test_support::skip_matches_process(
        &organ,
        &crate::test_support::IdleRun {
            ins: &[],
            outs: &["out_l", "out_r", "wheel_cv_out"],
            batches: 1_600,
            sound: &|_| false,
            edit: &|n, h| organ_scenario(h, n),
        },
    );
    assert!(skipped > 600, "{skipped} batches skipped");
}
