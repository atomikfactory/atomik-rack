//! (events): notes, MIDI messages and expressions become plugin events at their exact
//! batch offsets with host note ids, params are coalesced into events at the batch start, and
//! a note effect's output goes to `notes_out`.
// Justification: velocities and values are copied unchanged; the tests compare them exactly.
#![allow(clippy::float_cmp)]

atomik_testkit::install_alloc_guard!();

mod support;

use atomik_core::{ExprKind, NoteEvent, NoteKind};
use atomik_plugin_host::{PluginEvent, PluginEventKind as K};
use atomik_test_plugin_core as core;
use support::*;

fn ev(offset: u16, kind: NoteKind) -> NoteEvent {
    NoteEvent { offset, kind }
}

fn on(offset: u16, key: u8, velocity: f32) -> NoteEvent {
    ev(offset, NoteKind::On { key, velocity })
}

fn off(offset: u16, key: u8) -> NoteEvent {
    ev(offset, NoteKind::Off { key })
}

fn synth(
    id: &str,
) -> (
    atomik_testkit::harness::DeviceHarness,
    Rest,
    std::sync::Arc<ProbeShared>,
) {
    let (probe, shared) = Probe::new();
    let (h, r) = harness(rig(id, &synth_layout(), Box::new(probe)));
    (h, r, shared)
}

fn events(shared: &ProbeShared, call: usize) -> Vec<PluginEvent> {
    shared.log.lock().unwrap().events_of_call(call).to_vec()
}

fn note_id(e: &PluginEvent) -> i32 {
    match e.kind {
        K::NoteOn { note_id, .. } | K::NoteOff { note_id, .. } => note_id,
        other => panic!("not a note event: {other:?}"),
    }
}

#[test]
fn notes_arrive_at_their_exact_offsets_and_an_off_reuses_the_id_of_its_on() {
    let (mut h, _r, shared) = synth("ev.notes");
    h.push_note("notes_in", on(17, 60, 0.5));
    h.push_note("notes_in", off(40, 60));
    h.process();
    let e = events(&shared, 0);
    assert_eq!(e.len(), 2);
    assert_eq!(e[0].offset, 17);
    assert_eq!(e[1].offset, 40);
    assert!(matches!(e[0].kind, K::NoteOn { key: 60, velocity, .. } if velocity == 0.5));
    assert!(matches!(e[1].kind, K::NoteOff { key: 60, .. }));
    assert_eq!(note_id(&e[0]), note_id(&e[1]));
    assert!(note_id(&e[0]) >= 0);
}

#[test]
fn each_note_gets_its_own_id_and_an_off_across_batches_still_matches() {
    let (mut h, _r, shared) = synth("ev.ids");
    h.push_note("notes_in", on(0, 60, 1.0));
    h.push_note("notes_in", on(1, 64, 1.0));
    h.process();
    h.push_note("notes_in", off(5, 64));
    h.push_note("notes_in", off(9, 60));
    h.process();
    let a = events(&shared, 0);
    let b = events(&shared, 1);
    assert_ne!(note_id(&a[0]), note_id(&a[1]));
    assert_eq!(note_id(&b[0]), note_id(&a[1]), "the Off of key 64");
    assert_eq!(note_id(&b[1]), note_id(&a[0]), "the Off of key 60");
}

#[test]
fn a_retriggered_key_closes_its_old_note_first() {
    let (mut h, _r, shared) = synth("ev.retrigger");
    h.push_note("notes_in", on(0, 60, 1.0));
    h.process();
    h.push_note("notes_in", on(8, 60, 0.7));
    h.push_note("notes_in", off(20, 60));
    h.process();
    let first = events(&shared, 0);
    let second = events(&shared, 1);
    assert_eq!(second.len(), 3);
    assert!(matches!(second[0].kind, K::NoteOff { key: 60, .. }));
    assert_eq!(second[0].offset, 8);
    assert_eq!(note_id(&second[0]), note_id(&first[0]));
    assert!(matches!(second[1].kind, K::NoteOn { key: 60, .. }));
    assert_ne!(note_id(&second[1]), note_id(&first[0]));
    assert_eq!(note_id(&second[2]), note_id(&second[1]));
}

#[test]
fn an_off_without_an_on_is_dropped() {
    let (mut h, _r, shared) = synth("ev.stray-off");
    h.push_note("notes_in", off(3, 60));
    h.process();
    assert!(events(&shared, 0).is_empty());
}

#[test]
fn all_off_closes_every_open_key_in_key_order_then_sends_all_notes_off() {
    let (mut h, _r, shared) = synth("ev.alloff");
    h.push_note("notes_in", on(0, 72, 1.0));
    h.push_note("notes_in", on(1, 60, 1.0));
    h.push_note("notes_in", on(2, 64, 1.0));
    h.process();
    h.push_note("notes_in", ev(11, NoteKind::AllOff));
    h.process();
    // Another Off for an already closed key is dropped.
    h.push_note("notes_in", off(0, 60));
    h.process();
    let e = events(&shared, 1);
    let keys: Vec<u8> = e
        .iter()
        .filter_map(|e| match e.kind {
            K::NoteOff { key, .. } => Some(key),
            _ => None,
        })
        .collect();
    assert_eq!(keys, vec![60, 64, 72]);
    assert!(matches!(
        e.last().unwrap().kind,
        K::Cc {
            controller: 123,
            value
        } if value == 0.0
    ));
    assert!(e.iter().all(|e| e.offset == 11));
    assert!(events(&shared, 2).is_empty());
}

#[test]
fn all_off_with_nothing_open_still_sends_the_controller() {
    let (mut h, _r, shared) = synth("ev.alloff-empty");
    h.push_note("notes_in", ev(4, NoteKind::AllOff));
    h.process();
    let e = events(&shared, 0);
    assert_eq!(e.len(), 1);
    assert!(matches!(
        e[0].kind,
        K::Cc {
            controller: 123,
            ..
        }
    ));
}

#[test]
fn velocity_is_kept_in_the_open_unit_interval() {
    let (mut h, _r, shared) = synth("ev.velocity");
    h.push_note("notes_in", on(0, 60, 0.0));
    h.push_note("notes_in", on(1, 61, 2.0));
    h.push_note("notes_in", on(2, 62, f32::NAN));
    h.process();
    let v: Vec<f32> = events(&shared, 0)
        .iter()
        .map(|e| match e.kind {
            K::NoteOn { velocity, .. } => velocity,
            _ => panic!(),
        })
        .collect();
    assert!(v[0] > 0.0 && v[0] < 0.1, "{}", v[0]);
    assert_eq!(v[1], 1.0);
    assert_eq!(v[2], 1.0);
}

#[test]
fn channel_messages_pass_through_in_order() {
    let (mut h, _r, shared) = synth("ev.channel");
    h.push_note("notes_in", ev(2, NoteKind::PitchBend { value: -0.5 }));
    h.push_note(
        "notes_in",
        ev(
            2,
            NoteKind::Cc {
                controller: 1,
                value: 0.25,
            },
        ),
    );
    h.push_note("notes_in", ev(9, NoteKind::ChannelPressure { value: 0.75 }));
    h.process();
    let e = events(&shared, 0);
    assert_eq!(
        e,
        vec![
            PluginEvent {
                offset: 2,
                kind: K::PitchBend { value: -0.5 }
            },
            PluginEvent {
                offset: 2,
                kind: K::Cc {
                    controller: 1,
                    value: 0.25
                }
            },
            PluginEvent {
                offset: 9,
                kind: K::ChannelPressure { value: 0.75 }
            },
        ]
    );
}

#[test]
fn key_messages_use_the_note_id_of_their_key_and_are_dropped_for_closed_keys() {
    let (mut h, _r, shared) = synth("ev.expr");
    h.push_note("notes_in", on(0, 60, 1.0));
    h.push_note(
        "notes_in",
        ev(
            3,
            NoteKind::NoteExpr {
                key: 60,
                kind: ExprKind::Tuning,
                value: 1.5,
            },
        ),
    );
    h.push_note(
        "notes_in",
        ev(
            4,
            NoteKind::PolyPressure {
                key: 60,
                value: 0.4,
            },
        ),
    );
    h.push_note(
        "notes_in",
        ev(
            5,
            NoteKind::PolyPressure {
                key: 61,
                value: 0.4,
            },
        ),
    );
    h.process();
    let e = events(&shared, 0);
    assert_eq!(e.len(), 3, "the pressure on the closed key 61 is dropped");
    let id = note_id(&e[0]);
    assert!(matches!(
        e[1].kind,
        K::NoteExpr { key: 60, note_id, kind: ExprKind::Tuning, value } if note_id == id && value == 1.5
    ));
    assert!(matches!(
        e[2].kind,
        K::PolyPressure { key: 60, note_id, value } if note_id == id && value == 0.4
    ));
}

#[test]
fn offsets_are_clamped_to_the_batch() {
    let (mut h, _r, shared) = synth("ev.clamp");
    h.push_note("notes_in", on(200, 60, 1.0));
    h.process();
    assert_eq!(events(&shared, 0)[0].offset, 63);
}

#[test]
fn params_arrive_as_one_coalesced_event_each_at_the_batch_start_before_the_notes() {
    let (mut h, _r, shared) = synth("ev.params");
    // A burst on one param costs one event with the last value; two params keep their order of
    // first change.
    for i in 0..100 {
        h.set_param("p2", i as f32 / 100.0);
    }
    h.set_param("p1", 1.0);
    h.set_param("p2", 0.25);
    h.push_note("notes_in", on(0, 60, 1.0));
    h.process();
    let e = events(&shared, 0);
    assert_eq!(e.len(), 3);
    assert_eq!(
        e[0],
        PluginEvent {
            offset: 0,
            kind: K::Param { id: 2, value: 0.25 }
        }
    );
    assert_eq!(
        e[1],
        PluginEvent {
            offset: 0,
            kind: K::Param { id: 1, value: 1.0 }
        }
    );
    assert!(matches!(e[2].kind, K::NoteOn { .. }));
    // Nothing pending afterwards: the next batch carries no param event.
    h.process();
    assert!(events(&shared, 1).is_empty());
    // A param set again is sent again, even to the same value.
    h.set_param("p1", 1.0);
    h.process();
    assert_eq!(events(&shared, 2).len(), 1);
}

#[test]
fn a_note_effect_sends_its_notes_to_notes_out_and_drops_other_events() {
    let (h_probe, shared) = Probe::new();
    let (mut h, _r) = harness(rig("ev.shift-out", &shift_layout(), Box::new(h_probe)));
    {
        let mut emit = shared.emit.lock().unwrap();
        emit.push(PluginEvent {
            offset: 3,
            kind: K::NoteOn {
                key: 65,
                velocity: 0.5,
                note_id: 100,
            },
        });
        emit.push(PluginEvent {
            offset: 4,
            kind: K::PitchBend { value: 0.5 },
        });
        emit.push(PluginEvent {
            offset: 9,
            kind: K::NoteOff {
                key: 65,
                note_id: 100,
            },
        });
    }
    h.process();
    let out = h.notes_out("notes_out");
    assert_eq!(out.len(), 2, "expressions and messages out are dropped");
    assert_eq!(
        out[0],
        NoteEvent {
            offset: 3,
            kind: NoteKind::On {
                key: 65,
                velocity: 0.5
            }
        }
    );
    assert_eq!(
        out[1],
        NoteEvent {
            offset: 9,
            kind: NoteKind::Off { key: 65 }
        }
    );
}

#[test]
fn the_reference_note_effect_through_the_device_transposes() {
    let (mut h, _r) = harness(rig(
        "ev.shift-core",
        &shift_layout(),
        Box::new(CoreShift::new()),
    ));
    h.set_param("p1", 5.0);
    h.push_note("notes_in", on(2, 60, 0.5));
    h.process();
    assert_eq!(
        h.notes_out("notes_out"),
        &[NoteEvent {
            offset: 2,
            kind: NoteKind::On {
                key: 65,
                velocity: 0.5
            }
        }]
    );
    h.push_note("notes_in", off(4, 60));
    h.process();
    assert_eq!(
        h.notes_out("notes_out"),
        &[NoteEvent {
            offset: 4,
            kind: NoteKind::Off { key: 65 }
        }]
    );
}

#[test]
fn the_synth_through_the_device_equals_the_reference_run_directly() {
    let (mut h, _r) = harness(rig(
        "ev.synth-ref",
        &synth_layout(),
        Box::new(CoreSynth::new()),
    ));
    let mut reference = core::TestSynth::new(SR);
    h.set_param("p1", 1.0);
    reference.set_param(core::TestSynth::WAVE, 1.0);
    let mut note = 0;
    for b in 0..50 {
        let mut direct: Vec<core::TimedEvent> = Vec::new();
        if b == 0 {
            h.push_note("notes_in", on(11, 60, 0.8));
            h.push_note("notes_in", ev(30, NoteKind::PitchBend { value: 0.5 }));
            direct.push(core::TimedEvent {
                offset: 11,
                event: core::Event::NoteOn {
                    key: 60,
                    velocity: 0.8,
                    id: note,
                },
            });
            direct.push(core::TimedEvent {
                offset: 30,
                event: core::Event::PitchBend(0.5),
            });
            note += 1;
        }
        if b == 20 {
            h.push_note("notes_in", off(5, 60));
            direct.push(core::TimedEvent {
                offset: 5,
                event: core::Event::NoteOff { key: 60, id: 0 },
            });
        }
        h.process();
        let mut want = [[0.0; core::BLOCK]; 2];
        reference.process(&direct, &mut want);
        assert_eq!(*h.output("out_l"), want[0], "batch {b}");
        assert_eq!(*h.output("out_r"), want[1], "batch {b}");
    }
}

#[test]
fn a_full_events_out_list_drops_and_counts() {
    let (probe, shared) = Probe::new();
    let (mut h, _r) = harness(rig("ev.overflow", &shift_layout(), Box::new(probe)));
    {
        let mut emit = shared.emit.lock().unwrap();
        for i in 0..1_100u32 {
            emit.push(PluginEvent {
                offset: 0,
                kind: K::NoteOn {
                    key: (i % 128) as u8,
                    velocity: 1.0,
                    note_id: i as i32,
                },
            });
        }
    }
    h.process();
    let mut out = [0.0f32; 8];
    h.device().meter_values(&mut out);
    let sent = h.notes_out("notes_out").len();
    assert!(sent <= 1_024);
    assert_eq!(
        out[4] as usize,
        1_100 - sent,
        "the rest is counted as dropped"
    );
}
