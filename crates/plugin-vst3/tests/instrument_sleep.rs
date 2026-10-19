//! The live bug of a commercial wavetable synth (debug fix): a VST3 instrument that reports a zero tail was
//! skipped after the one batch that carried its note-on, so a held note sounded for 64 frames
//! (a click) and the plugin saw `process` stop while a note was held ("the DAW suspended
//! processing"). The test plugin `Atomik Test Picky Synth` behaves like that plugin: tail 0
//! always, and silent unless `setProcessing(true)` came after the last `setProcessing(false)`.
//! Rules checked here: an instrument is processed continuously while a note is held and while
//! its release sounds, it is put to sleep (`setProcessing(false)`, state SLEEPING) only after a
//! second of silence with no open note, and a note-on wakes it with `setProcessing(true)` before
//! the first block so the first block already sounds.
// Justification: silence is exact zero.
#![allow(clippy::float_cmp)]

atomik_testkit::install_alloc_guard!();

mod support;

use atomik_core::{BATCH, NoteEvent, NoteKind};
use atomik_device_sdk::RunMode;
use support::*;

fn ev(kind: NoteKind) -> NoteEvent {
    NoteEvent { offset: 0, kind }
}

fn peak(h: &atomik_testkit::harness::DeviceHarness) -> f32 {
    h.output("out_l")
        .iter()
        .chain(h.output("out_r"))
        .fold(0.0, |m, s| m.max(s.abs()))
}

fn state(h: &atomik_testkit::harness::DeviceHarness) -> f32 {
    let mut m = [0.0f32; 5];
    h.device().meter_values(&mut m);
    m[2]
}

const SECOND: usize = SR as usize / BATCH;

#[test]
fn a_zero_tail_instrument_sounds_for_the_whole_note_and_is_woken_by_the_next_one() {
    let _serial = serial();
    let k = kit_variants(Format::Vst3);
    let lib = TestLib::open_path(&k.path);
    let mut rig = rig_for(
        &*k.backend,
        &vst3_ref(&variant_id(V_PICKY)),
        RunMode::Realtime,
    );
    let mut h = rig.harness();
    let calls = |rig: &Rig| lib.probe(&*rig.instance, probe::PROCESS_CALLS);
    let starts = |rig: &Rig| lib.probe(&*rig.instance, probe::PROCESSING_STARTS);
    let stops = |rig: &Rig| lib.probe(&*rig.instance, probe::PROCESSING_STOPS);

    // Idle: the plugin is started and processed from the first batch (no "suspended" message).
    for _ in 0..50 {
        h.process();
    }
    assert_eq!(
        starts(&rig),
        1,
        "setProcessing(true) once, before the first block"
    );
    assert_eq!(calls(&rig), 50, "processed while it is young");

    // A held note sounds in every block (the bug: one block, then silence).
    h.push_note(
        "notes_in",
        ev(NoteKind::On {
            key: 60,
            velocity: 1.0,
        }),
    );
    h.process();
    assert!(peak(&h) > 0.0, "the first block of the note sounds");
    for n in 0..SECOND * 2 {
        h.process();
        assert!(peak(&h) > 1e-4, "block {n} of the held note is silent");
    }
    assert_eq!(state(&h), 0.0, "running");
    assert_eq!(stops(&rig), 0, "never stopped while the note is held");

    // The release sounds until it is gone, then (after a second of silence) the plugin sleeps.
    h.push_note("notes_in", ev(NoteKind::Off { key: 60 }));
    let mut sounded_after_off = 0usize;
    let mut slept_after = None;
    for n in 0..SECOND * 6 {
        h.process();
        if peak(&h) > 1e-6 {
            sounded_after_off += 1;
        }
        if state(&h) == 1.0 {
            slept_after = Some(n);
            break;
        }
    }
    // The test synth's release is about 8 blocks; the old rule cut it after at most one.
    assert!(
        sounded_after_off > 4,
        "the release was cut: {sounded_after_off}"
    );
    let slept_after = slept_after.expect("the idle plugin goes to sleep");
    assert!(
        slept_after >= SECOND,
        "not before a second of silence: {slept_after}"
    );
    assert_eq!(stops(&rig), 1, "setProcessing(false) when it went to sleep");
    let asleep_calls = calls(&rig);
    for _ in 0..40 {
        h.process();
        assert_eq!(peak(&h), 0.0);
    }
    assert_eq!(calls(&rig), asleep_calls, "asleep means no process calls");
    assert_eq!(state(&h), 1.0);

    // The next note wakes it: setProcessing(true) first, so the very first block sounds.
    h.push_note(
        "notes_in",
        ev(NoteKind::On {
            key: 64,
            velocity: 1.0,
        }),
    );
    h.process();
    assert_eq!(starts(&rig), 2, "restarted before the block");
    assert!(peak(&h) > 0.0, "the first block after the wake sounds");
    assert_eq!(state(&h), 0.0, "RUNNING again");
    for n in 0..SECOND {
        h.process();
        assert!(peak(&h) > 1e-4, "block {n} after the wake is silent");
    }
}
