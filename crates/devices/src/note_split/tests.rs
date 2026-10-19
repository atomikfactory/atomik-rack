//! Split device tests.
#![allow(clippy::float_cmp)]

use super::*;
use atomik_core::{BATCH, NoteEvent, NoteKind};
use atomik_testkit::harness::DeviceHarness;
use proptest::prelude::*;

use crate::test_support::{harness, off, on};

const OUTS: [&str; 8] = [
    "out_1", "out_2", "out_3", "out_4", "out_5", "out_6", "out_7", "out_8",
];

fn rig() -> DeviceHarness {
    harness(Box::new(NoteSplit::new()))
}

fn slot(h: &mut DeviceHarness, n: usize, enabled: bool, key: (u8, u8), vel: (u8, u8), tr: i8) {
    h.set_param(&format!("s{n}.on"), f32::from(u8::from(enabled)));
    h.set_param(&format!("s{n}.key_lo"), f32::from(key.0));
    h.set_param(&format!("s{n}.key_hi"), f32::from(key.1));
    h.set_param(&format!("s{n}.vel_lo"), f32::from(vel.0));
    h.set_param(&format!("s{n}.vel_hi"), f32::from(vel.1));
    h.set_param(&format!("s{n}.transpose"), f32::from(tr));
}

fn vel(v: u8) -> f32 {
    f32::from(v) / 127.0
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum K {
    On(u8, u8),
    Off(u8),
    AllOff,
    /// Any other kind: compared whole.
    Other(NoteKind),
}

fn kinds(h: &DeviceHarness, out: &str) -> Vec<(u16, K)> {
    h.notes_out(out)
        .iter()
        .map(|e| {
            (
                e.offset,
                match e.kind {
                    NoteKind::On { key, velocity } => K::On(key, (velocity * 127.0).round() as u8),
                    NoteKind::Off { key } => K::Off(key),
                    NoteKind::AllOff => K::AllOff,
                    other => K::Other(other),
                },
            )
        })
        .collect()
}

fn feed(h: &mut DeviceHarness, ev: &[NoteEvent]) {
    for e in ev {
        h.push_note("notes_in", *e);
    }
    h.process();
}

fn all_off(offset: u16) -> NoteEvent {
    NoteEvent {
        offset,
        kind: NoteKind::AllOff,
    }
}

#[test]
fn descriptor_matches_the_contract() {
    assert_eq!(DESCRIPTOR.validate(), Ok(()));
    assert_eq!(DESCRIPTOR.type_id, "atomik.note-split");
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Utility);
    assert_eq!(DESCRIPTOR.sockets.len(), 9);
    assert_eq!(DESCRIPTOR.sockets[0].key, "notes_in");
    assert_eq!(DESCRIPTOR.sockets[0].direction, Direction::Input);
    assert_eq!(DESCRIPTOR.sockets[0].kind, SocketKind::Notes);
    for (i, s) in DESCRIPTOR.sockets[1..].iter().enumerate() {
        assert_eq!(s.key, format!("out_{}", i + 1));
        assert_eq!(s.kind, SocketKind::Notes);
        assert_eq!(s.direction, Direction::Output);
    }
    assert_eq!(DESCRIPTOR.params.len(), 48);
    let tail = [
        ("on", 0.0, 1.0, 0.0),
        ("key_lo", 0.0, 127.0, 0.0),
        ("key_hi", 0.0, 127.0, 127.0),
        ("vel_lo", 1.0, 127.0, 1.0),
        ("vel_hi", 1.0, 127.0, 127.0),
        ("transpose", -48.0, 48.0, 0.0),
    ];
    for k in 1..=8usize {
        for (j, (name, lo, hi, def)) in tail.iter().enumerate() {
            let p = &DESCRIPTOR.params[6 * (k - 1) + j];
            assert_eq!(p.id, ParamId((6 * (k - 1) + j) as u32));
            assert_eq!(p.key, format!("s{k}.{name}"));
            assert_eq!((p.min, p.max, p.default), (*lo, *hi, *def), "{}", p.key);
            assert!(p.discrete, "{}", p.key);
        }
    }
    assert_eq!(rig().device().latency_frames(), 0);
}

#[test]
fn a_disabled_slot_is_silent_and_defaults_are_off() {
    let mut h = rig();
    feed(&mut h, &[on(0, 60, 0.8), off(10, 60)]);
    for o in OUTS {
        assert!(h.notes_out(o).is_empty(), "{o}");
    }
}

#[test]
fn key_range_edges_are_inclusive() {
    let mut h = rig();
    slot(&mut h, 1, true, (36, 59), (1, 127), 0);
    slot(&mut h, 2, true, (60, 96), (1, 127), 0);
    feed(
        &mut h,
        &[
            on(0, 35, 0.5),
            on(1, 36, 0.5),
            on(2, 59, 0.5),
            on(3, 60, 0.5),
            on(4, 96, 0.5),
            on(5, 97, 0.5),
        ],
    );
    let v = (0.5f32 * 127.0).round() as u8;
    assert_eq!(kinds(&h, "out_1"), [(1, K::On(36, v)), (2, K::On(59, v))]);
    assert_eq!(kinds(&h, "out_2"), [(3, K::On(60, v)), (4, K::On(96, v))]);
}

#[test]
fn velocity_range_edges_are_inclusive() {
    let mut h = rig();
    slot(&mut h, 1, true, (0, 127), (20, 100), 0);
    feed(
        &mut h,
        &[
            on(0, 60, vel(19)),
            on(1, 61, vel(20)),
            on(2, 62, vel(100)),
            on(3, 63, vel(101)),
        ],
    );
    assert_eq!(
        kinds(&h, "out_1"),
        [(1, K::On(61, 20)), (2, K::On(62, 100))]
    );
}

#[test]
fn transpose_shifts_and_clamps_to_the_key_range() {
    let mut h = rig();
    slot(&mut h, 1, true, (0, 127), (1, 127), 12);
    slot(&mut h, 2, true, (0, 127), (1, 127), -48);
    feed(&mut h, &[on(0, 120, 0.5), on(1, 20, 0.5)]);
    let v = (0.5f32 * 127.0).round() as u8;
    assert_eq!(kinds(&h, "out_1"), [(0, K::On(127, v)), (1, K::On(32, v))]);
    assert_eq!(kinds(&h, "out_2"), [(0, K::On(72, v)), (1, K::On(0, v))]);
}

#[test]
fn the_velocity_is_forwarded_unchanged() {
    let mut h = rig();
    slot(&mut h, 1, true, (0, 127), (1, 127), 0);
    feed(&mut h, &[on(0, 60, 0.3217)]);
    match h.notes_out("out_1")[0].kind {
        NoteKind::On { velocity, .. } => assert_eq!(velocity.to_bits(), 0.3217f32.to_bits()),
        k => panic!("{k:?}"),
    }
}

#[test]
fn overlapping_zones_layer() {
    let mut h = rig();
    slot(&mut h, 3, true, (0, 127), (1, 127), 0);
    slot(&mut h, 5, true, (50, 70), (1, 127), 7);
    feed(&mut h, &[on(2, 60, vel(89))]);
    assert_eq!(kinds(&h, "out_3"), [(2, K::On(60, 89))]);
    assert_eq!(kinds(&h, "out_5"), [(2, K::On(67, 89))]);
    for o in ["out_1", "out_2", "out_4", "out_6", "out_7", "out_8"] {
        assert!(h.notes_out(o).is_empty(), "{o}");
    }
}

#[test]
fn offsets_are_preserved_and_non_decreasing() {
    let mut h = rig();
    slot(&mut h, 1, true, (0, 127), (1, 127), 0);
    feed(
        &mut h,
        &[on(0, 60, 0.5), off(0, 60), on(5, 61, 0.5), off(63, 61)],
    );
    let offs: Vec<u16> = kinds(&h, "out_1").iter().map(|e| e.0).collect();
    assert_eq!(offs, [0, 0, 5, 63]);
}

#[test]
fn an_off_follows_its_on_after_the_zone_params_changed() {
    let mut h = rig();
    slot(&mut h, 1, true, (0, 63), (1, 127), 5);
    slot(&mut h, 2, true, (64, 127), (1, 127), 0);
    feed(&mut h, &[on(0, 60, vel(64))]);
    assert_eq!(kinds(&h, "out_1"), [(0, K::On(65, 64))]);
    // The zones are rearranged, a transpose changed and slot 1 switched off while the key is held.
    slot(&mut h, 1, false, (100, 110), (1, 127), -12);
    slot(&mut h, 2, true, (0, 127), (1, 127), 24);
    feed(&mut h, &[off(3, 60)]);
    assert_eq!(kinds(&h, "out_1"), [(3, K::Off(65))]);
    assert!(h.notes_out("out_2").is_empty());
    // A second Off of the same key is dropped (it has no route any more).
    feed(&mut h, &[off(0, 60)]);
    assert!(h.notes_out("out_1").is_empty());
}

#[test]
fn an_off_works_across_batches() {
    let mut h = rig();
    slot(&mut h, 4, true, (0, 127), (1, 127), -3);
    feed(&mut h, &[on(0, 60, 0.5)]);
    for _ in 0..5 {
        h.process();
        assert!(h.notes_out("out_4").is_empty());
    }
    feed(&mut h, &[off(9, 60)]);
    assert_eq!(kinds(&h, "out_4"), [(9, K::Off(57))]);
}

#[test]
fn a_retriggered_key_closes_the_old_note_first() {
    let mut h = rig();
    slot(&mut h, 1, true, (0, 127), (1, 127), 0);
    feed(&mut h, &[on(0, 60, vel(64))]);
    slot(&mut h, 1, true, (0, 127), (1, 127), 12);
    feed(&mut h, &[on(4, 60, vel(64))]);
    assert_eq!(kinds(&h, "out_1"), [(4, K::Off(60)), (4, K::On(72, 64))]);
    feed(&mut h, &[off(0, 60)]);
    assert_eq!(kinds(&h, "out_1"), [(0, K::Off(72))]);
}

#[test]
fn a_retrigger_that_leaves_the_zone_only_closes() {
    let mut h = rig();
    slot(&mut h, 1, true, (0, 127), (1, 127), 0);
    feed(&mut h, &[on(0, 60, 0.5)]);
    slot(&mut h, 1, true, (0, 10), (1, 127), 0);
    feed(&mut h, &[on(2, 60, 0.5)]);
    assert_eq!(kinds(&h, "out_1"), [(2, K::Off(60))]);
}

#[test]
fn all_off_reaches_every_output_and_clears_the_tables() {
    let mut h = rig();
    slot(&mut h, 1, true, (0, 127), (1, 127), 0);
    slot(&mut h, 8, true, (0, 127), (1, 127), 0);
    feed(&mut h, &[on(0, 60, 0.5)]);
    feed(&mut h, &[all_off(6)]);
    for o in OUTS {
        assert_eq!(kinds(&h, o), [(6, K::AllOff)], "{o}");
    }
    feed(&mut h, &[off(0, 60)]);
    for o in OUTS {
        assert!(h.notes_out(o).is_empty(), "{o}");
    }
}

#[test]
fn reset_flushes_an_all_off_when_a_note_is_held() {
    let mut h = rig();
    slot(&mut h, 2, true, (0, 127), (1, 127), 0);
    feed(&mut h, &[on(0, 60, 0.5)]);
    h.device_mut().reset();
    h.process();
    for o in OUTS {
        assert_eq!(kinds(&h, o), [(0, K::AllOff)], "{o}");
    }
    h.process();
    for o in OUTS {
        assert!(h.notes_out(o).is_empty(), "{o}");
    }
    // Nothing held: reset stays silent.
    h.device_mut().reset();
    h.process();
    for o in OUTS {
        assert!(h.notes_out(o).is_empty(), "{o}");
    }
}

#[test]
fn no_slot_connected_is_harmless() {
    let mut h = rig();
    slot(&mut h, 1, true, (0, 127), (1, 127), 0);
    h.set_connected("out_1", false);
    feed(&mut h, &[on(0, 60, 0.5), off(1, 60)]);
    h.process();
}

#[test]
fn process_is_allocation_free_under_a_busy_script() {
    let mut h = rig();
    for k in 1..=8 {
        slot(&mut h, k, true, (0, 127), (1, 127), k as i8);
    }
    for b in 0..64u32 {
        for i in 0..24u8 {
            h.push_note("notes_in", on(u16::from(i) * 2, 20 + i, 0.6));
            h.push_note("notes_in", off(u16::from(i) * 2 + 1, 20 + i));
        }
        if b % 7 == 0 {
            h.set_param("s3.transpose", (b % 20) as f32);
        }
        h.process(); // guarded inside the harness
    }
}

fn ev(offset: u16, kind: NoteKind) -> NoteEvent {
    NoteEvent { offset, kind }
}

#[test]
fn channel_messages_go_to_every_enabled_slot_and_only_to_those() {
    let mut h = rig();
    slot(&mut h, 1, true, (0, 127), (1, 127), 0);
    // A narrow key zone does not matter for channel messages.
    slot(&mut h, 2, true, (100, 110), (1, 127), 5);
    slot(&mut h, 3, false, (0, 127), (1, 127), 0);
    let bend = NoteKind::PitchBend { value: -0.25 };
    let cc = NoteKind::Cc {
        controller: 1,
        value: 0.5,
    };
    let press = NoteKind::ChannelPressure { value: 0.75 };
    feed(&mut h, &[ev(3, bend), ev(7, cc), ev(7, press)]);
    for out in ["out_1", "out_2"] {
        assert_eq!(
            kinds(&h, out),
            vec![(3, K::Other(bend)), (7, K::Other(cc)), (7, K::Other(press))],
            "{out}"
        );
    }
    assert!(
        kinds(&h, "out_3").is_empty(),
        "a disabled slot gets nothing"
    );
    assert!(kinds(&h, "out_4").is_empty());
}

#[test]
fn key_messages_follow_the_per_key_route_table() {
    let mut h = rig();
    slot(&mut h, 1, true, (0, 50), (1, 127), 0);
    slot(&mut h, 2, true, (0, 127), (1, 127), 12);
    let tuning = NoteKind::NoteExpr {
        key: 60,
        kind: atomik_core::ExprKind::Tuning,
        value: 1.5,
    };
    let pp = NoteKind::PolyPressure {
        key: 60,
        value: 0.4,
    };
    // Key 60 is outside slot 1's zone (no route there) and routed to 72 on slot 2.
    feed(&mut h, &[on(0, 60, 0.5), ev(2, tuning), ev(4, pp)]);
    assert_eq!(
        kinds(&h, "out_1"),
        vec![],
        "slot 1 never opened key 60, so it gets no expression"
    );
    assert_eq!(
        kinds(&h, "out_2"),
        vec![
            (0, K::On(72, 64)),
            (
                2,
                K::Other(NoteKind::NoteExpr {
                    key: 72,
                    kind: atomik_core::ExprKind::Tuning,
                    value: 1.5
                })
            ),
            (
                4,
                K::Other(NoteKind::PolyPressure {
                    key: 72,
                    value: 0.4
                })
            ),
        ],
        "expressions use the transposed output key"
    );
    // After the Off the route is gone: further expressions are dropped.
    feed(&mut h, &[off(0, 60), ev(1, pp)]);
    assert_eq!(kinds(&h, "out_2"), vec![(0, K::Off(72))]);
}

#[test]
fn an_expression_for_a_key_that_never_started_is_dropped() {
    let mut h = rig();
    slot(&mut h, 1, true, (0, 127), (1, 127), 0);
    feed(
        &mut h,
        &[ev(
            0,
            NoteKind::NoteExpr {
                key: 40,
                kind: atomik_core::ExprKind::Pan,
                value: 0.5,
            },
        )],
    );
    assert!(kinds(&h, "out_1").is_empty());
}

#[derive(Debug, Clone)]
enum Step {
    On(u8, u8),
    Off(u8),
    AllOff,
    Param(u8, f32),
    Idle,
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        4 => (0u8..128, 1u8..128).prop_map(|(k, v)| Step::On(k, v)),
        4 => (0u8..128).prop_map(Step::Off),
        1 => Just(Step::AllOff),
        3 => (0u8..48, any::<u16>()).prop_map(|(p, x)| {
            let d = &DESCRIPTOR.params[usize::from(p)];
            Step::Param(p, (d.min + (d.max - d.min) * (f32::from(x) / 65535.0)).round())
        }),
        1 => Just(Step::Idle),
    ]
}

fn check(h: &DeviceHarness, open: &mut [[bool; 128]; 8]) -> Result<(), TestCaseError> {
    for (i, o) in OUTS.iter().enumerate() {
        let mut last = 0u16;
        for e in h.notes_out(o) {
            prop_assert!(e.offset >= last);
            last = e.offset;
            match e.kind {
                NoteKind::On { key, .. } => {
                    prop_assert!(!open[i][usize::from(key)], "double on");
                    open[i][usize::from(key)] = true;
                }
                NoteKind::Off { key } => {
                    prop_assert!(open[i][usize::from(key)], "off without on");
                    open[i][usize::from(key)] = false;
                }
                NoteKind::AllOff => open[i] = [false; 128],
                // Channel messages and expressions do not open or close notes.
                _ => {}
            }
        }
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Whatever the script and the zone edits, every output stays balanced: an Off only for an On
    /// that is open, never two Ons of one key without an Off between them, and after a final
    /// `AllOff` nothing is open (no hung notes). Also after an upstream `AllOff` in the middle.
    #[test]
    fn no_hung_notes_under_random_scripts(steps in proptest::collection::vec(step(), 1..160)) {
        let mut h = rig();
        for k in 1..=8 {
            h.set_param(&format!("s{k}.on"), 1.0);
        }
        let mut open = [[false; 128]; 8];
        let mut offset = 0u16;
        for s in steps {
            match s {
                Step::On(k, v) => {
                    h.push_note("notes_in", on(offset, k, vel(v)));
                    offset = (offset + 1).min(BATCH as u16 - 1);
                }
                Step::Off(k) => {
                    h.push_note("notes_in", off(offset, k));
                    offset = (offset + 1).min(BATCH as u16 - 1);
                }
                Step::AllOff => h.push_note("notes_in", all_off(offset)),
                Step::Param(p, v) => h.set_param(DESCRIPTOR.params[usize::from(p)].key, v),
                Step::Idle => {
                    h.process();
                    check(&h, &mut open)?;
                    offset = 0;
                }
            }
        }
        h.process();
        check(&h, &mut open)?;
        feed(&mut h, &[all_off(0)]);
        check(&h, &mut open)?;
        prop_assert!(open.iter().all(|o| o.iter().all(|x| !x)));
    }
}

#[test]
fn two_input_keys_on_one_output_key_never_leave_two_open_notes() {
    let mut h = rig();
    slot(&mut h, 1, true, (0, 127), (1, 127), 0);
    slot(&mut h, 2, true, (0, 127), (1, 127), 0);
    // Both keys clamp onto output key 127 of slot 1.
    h.set_param("s1.transpose", 48.0);
    feed(&mut h, &[on(0, 120, 0.5), on(1, 125, 0.5)]);
    assert_eq!(
        kinds(&h, "out_1"),
        [(0, K::On(127, 64)), (1, K::Off(127)), (1, K::On(127, 64))]
    );
    // The first key no longer owns the note: its Off is dropped, the second one closes it.
    feed(&mut h, &[off(2, 120)]);
    assert!(h.notes_out("out_1").is_empty());
    feed(&mut h, &[off(3, 125)]);
    assert_eq!(kinds(&h, "out_1"), [(3, K::Off(127))]);
}

/// QA (mutation M30): every MIDI velocity 1..=127, as the float the host sends, lands exactly on
/// its own velocity zone and on neither neighbouring zone.
#[test]
fn every_velocity_maps_to_its_own_zone() {
    for v in 1..=127u8 {
        let mut h = rig();
        slot(&mut h, 1, true, (0, 127), (v, v), 0);
        if v > 1 {
            slot(&mut h, 2, true, (0, 127), (1, v - 1), 0);
        }
        if v < 127 {
            slot(&mut h, 3, true, (0, 127), (v + 1, 127), 0);
        }
        feed(&mut h, &[on(0, 60, vel(v))]);
        assert_eq!(kinds(&h, "out_1"), [(0, K::On(60, v))], "velocity {v}");
        assert!(h.notes_out("out_2").is_empty(), "below, velocity {v}");
        assert!(h.notes_out("out_3").is_empty(), "above, velocity {v}");
    }
}

/// QA (mutations M27 and M28 together): a second Off of a key that is already closed is dropped,
/// so a receiver never sees more Offs than Ons.
#[test]
fn a_repeated_off_is_dropped() {
    let mut h = rig();
    slot(&mut h, 1, true, (0, 127), (1, 127), 3);
    feed(
        &mut h,
        &[on(0, 60, 0.5), off(1, 60), off(2, 60), off(3, 60)],
    );
    assert_eq!(
        kinds(&h, "out_1"),
        [(0, K::On(63, 64)), (1, K::Off(63))],
        "one On, one Off"
    );
    // The same across batches.
    feed(&mut h, &[off(0, 60)]);
    assert!(h.notes_out("out_1").is_empty());
}

/// QA (coverage): a full output buffer drops events instead of panicking, an unknown param id is
/// ignored and `Default` is the same device as `new`.
#[test]
fn a_full_output_buffer_drops_events_and_unknown_params_are_ignored() {
    let mut h = rig();
    slot(&mut h, 1, true, (0, 127), (1, 127), 0);
    h.device_mut().set_param(ParamId(48), 1.0);
    h.device_mut().set_param(ParamId(u32::MAX), 1.0);
    // Every retrigger writes an Off and an On, so 1024 inputs overflow the 1024-event output.
    let events: Vec<NoteEvent> = (0..atomik_core::MAX_EVENTS_PER_BATCH)
        .map(|_| on(0, 60, 0.5))
        .collect();
    feed(&mut h, &events);
    assert_eq!(
        h.notes_out("out_1").len(),
        atomik_core::MAX_EVENTS_PER_BATCH
    );
    // Nothing is wrong afterwards: a clean Off and On pair goes through.
    feed(&mut h, &[off(0, 60), on(1, 61, 0.5)]);
    assert!(!h.notes_out("out_1").is_empty());
    let d = NoteSplit::default();
    assert!(d.slots.iter().all(|s| !s.on && !s.holds()));
}
