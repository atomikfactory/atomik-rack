//! Note router, player bypass and emitter tests.

use super::*;
use atomik_core::MAX_EVENTS_PER_BATCH;

#[derive(Debug, Clone, Copy, PartialEq)]
enum K {
    On(u8),
    Off(u8),
    AllOff,
    Other,
}

fn buf() -> NoteBuffer {
    NoteBuffer::with_capacity(MAX_EVENTS_PER_BATCH)
}

fn seen(b: &NoteBuffer) -> Vec<(u16, K)> {
    b.as_slice()
        .iter()
        .map(|e| {
            (
                e.offset,
                match e.kind {
                    NoteKind::On { key, .. } => K::On(key),
                    NoteKind::Off { key } => K::Off(key),
                    NoteKind::AllOff => K::AllOff,
                    _ => K::Other,
                },
            )
        })
        .collect()
}

fn ev(offset: u16, kind: NoteKind) -> NoteEvent {
    NoteEvent { offset, kind }
}

fn on(offset: u16, key: u8) -> NoteEvent {
    ev(offset, NoteKind::On { key, velocity: 0.5 })
}

fn off(offset: u16, key: u8) -> NoteEvent {
    ev(offset, NoteKind::Off { key })
}

#[test]
fn router_off_releases_what_the_on_emitted_after_a_param_change() {
    let mut r = NoteRouter::new();
    let mut out = buf();
    // Transpose +2 when the key goes down...
    r.on(&mut out, 3, 60, &[(62, 0.5)]);
    // ...and +7 by the time it goes up: the Off still releases 62.
    r.off(&mut out, 9, 60);
    assert_eq!(seen(&out), [(3, K::On(62)), (9, K::Off(62))]);
    assert!(!r.any_sounding());
}

#[test]
fn router_chord_input_releases_every_emitted_key() {
    let mut r = NoteRouter::new();
    let mut out = buf();
    r.on(&mut out, 0, 60, &[(60, 0.5), (64, 0.5), (67, 0.5)]);
    r.off(&mut out, 5, 60);
    assert_eq!(
        seen(&out),
        [
            (0, K::On(60)),
            (0, K::On(64)),
            (0, K::On(67)),
            (5, K::Off(60)),
            (5, K::Off(64)),
            (5, K::Off(67)),
        ]
    );
}

#[test]
fn router_two_inputs_on_one_output_key_close_only_on_the_last_off() {
    let mut r = NoteRouter::new();
    let mut out = buf();
    r.on(&mut out, 0, 60, &[(64, 0.5)]);
    r.on(&mut out, 2, 62, &[(64, 0.5)]);
    r.off(&mut out, 4, 60);
    r.off(&mut out, 6, 62);
    assert_eq!(
        seen(&out),
        [
            (0, K::On(64)),
            // The second user retriggers the key.
            (2, K::Off(64)),
            (2, K::On(64)),
            (6, K::Off(64)),
        ]
    );
    assert!(!r.any_sounding());
}

#[test]
fn router_on_of_a_held_input_key_releases_the_old_notes_first() {
    let mut r = NoteRouter::new();
    let mut out = buf();
    r.on(&mut out, 0, 60, &[(61, 0.5)]);
    r.on(&mut out, 1, 60, &[(65, 0.5)]);
    r.off(&mut out, 2, 60);
    assert_eq!(
        seen(&out),
        [
            (0, K::On(61)),
            (1, K::Off(61)),
            (1, K::On(65)),
            (2, K::Off(65))
        ]
    );
}

#[test]
fn router_duplicate_output_keys_of_one_input_count_once() {
    let mut r = NoteRouter::new();
    let mut out = buf();
    r.on(&mut out, 0, 60, &[(64, 0.5), (64, 0.7)]);
    r.off(&mut out, 1, 60);
    assert_eq!(
        seen(&out),
        [(0, K::On(64)), (1, K::Off(64))],
        "one On and one Off"
    );
}

#[test]
fn router_close_all_emits_offs_ascending_and_clears() {
    let mut r = NoteRouter::new();
    let mut out = buf();
    r.on(&mut out, 0, 60, &[(72, 0.5), (48, 0.5)]);
    r.on(&mut out, 0, 61, &[(55, 0.5)]);
    out.clear();
    r.close_all(&mut out, 7);
    assert_eq!(
        seen(&out),
        [(7, K::Off(48)), (7, K::Off(55)), (7, K::Off(72))]
    );
    assert!(!r.any_sounding());
    // The held input keys' Offs now emit nothing.
    out.clear();
    r.off(&mut out, 8, 60);
    r.off(&mut out, 8, 61);
    assert!(out.is_empty());
}

#[test]
fn router_press_of_a_swallowed_key_that_still_sounds_releases_it_first() {
    let mut r = NoteRouter::new();
    let mut out = buf();
    r.on(&mut out, 0, 51, &[(51, 0.5)]);
    r.swallow_held();
    // A fresh press while the first note still sounds: close it, then play again (a stream
    // of such presses would otherwise outgrow the per-key table).
    for _ in 0..12 {
        r.on(&mut out, 1, 51, &[(51, 0.5)]);
        r.swallow_held();
    }
    r.on(&mut out, 1, 51, &[(51, 0.5)]);
    r.off(&mut out, 2, 51);
    assert!(!r.any_sounding());
    assert_eq!(seen(&out).last(), Some(&(2, K::Off(51))));
}

#[test]
fn router_swallow_held_ignores_the_off_of_keys_down() {
    let mut r = NoteRouter::new();
    let mut out = buf();
    r.on(&mut out, 0, 60, &[(60, 0.5)]);
    r.close_all(&mut out, 1);
    r.swallow_held();
    out.clear();
    r.off(&mut out, 2, 60);
    assert!(out.is_empty());
    // A new press of the key works again.
    r.on(&mut out, 3, 60, &[(60, 0.5)]);
    r.off(&mut out, 4, 60);
    assert_eq!(seen(&out), [(3, K::On(60)), (4, K::Off(60))]);
}

#[test]
fn router_all_off_closes_everything_and_forgets_held_keys() {
    let mut r = NoteRouter::new();
    let mut out = buf();
    r.on(&mut out, 0, 60, &[(60, 0.5)]);
    r.all_off(&mut out, 3);
    assert!(!r.is_held(60));
    assert_eq!(seen(&out), [(0, K::On(60)), (3, K::Off(60))]);
}

#[test]
fn router_follow_gives_the_first_output_key_of_a_sounding_input() {
    let mut r = NoteRouter::new();
    let mut out = buf();
    assert_eq!(r.follow(60), None);
    r.on(&mut out, 0, 60, &[(67, 0.5), (71, 0.5)]);
    assert_eq!(r.follow(60), Some(67));
    r.off(&mut out, 1, 60);
    assert_eq!(r.follow(60), None);
}

#[test]
fn push_sorted_raises_offsets_and_keeps_room_for_offs() {
    let mut out = NoteBuffer::with_capacity(ON_HEADROOM + 1);
    assert!(push_sorted(&mut out, on(10, 60)));
    assert!(
        push_sorted(&mut out, off(3, 60)),
        "an earlier frame is raised"
    );
    assert_eq!(out.as_slice()[1].offset, 10);
    // No room left for an On's headroom: an On is refused, an Off is not.
    assert!(!push_sorted(&mut out, on(11, 61)));
    assert!(push_sorted(&mut out, off(11, 61)));
    let mut tiny = NoteBuffer::with_capacity(1);
    assert!(push_sorted(&mut tiny, off(0, 60)));
    assert!(!push_sorted(&mut tiny, off(0, 61)), "full buffer");
}

#[test]
fn router_refused_on_is_not_recorded_so_its_off_emits_nothing() {
    let mut r = NoteRouter::new();
    let mut out = NoteBuffer::with_capacity(ON_HEADROOM - 1);
    r.on(&mut out, 0, 60, &[(60, 0.5)]);
    assert!(out.is_empty());
    r.off(&mut out, 1, 60);
    assert!(out.is_empty());
    assert!(!r.any_sounding());
}

#[test]
fn bypass_switch_closes_sent_notes_and_swallows_held_keys() {
    let mut b = PlayerBypass::new();
    let mut out = buf();
    // A key goes down before the bypass: the device sees it.
    assert!(b.route(&mut out, on(0, 60)).is_some());
    b.set_param(1.0);
    assert!(b.begin_batch(&mut out));
    assert!(b.is_active());
    // Its Off is swallowed: downstream never heard of the On from the thru.
    assert!(b.route(&mut out, off(1, 60)).is_none());
    // New keys and other events pass through.
    assert!(b.route(&mut out, on(2, 62)).is_none());
    assert!(
        b.route(&mut out, ev(3, NoteKind::PitchBend { value: 0.5 }))
            .is_none()
    );
    // Leaving the bypass closes the key that is still down on the thru.
    b.set_param(0.0);
    assert!(b.begin_batch(&mut out));
    assert!(!b.is_active());
    assert_eq!(
        seen(&out),
        [(2, K::On(62)), (3, K::Other), (3, K::Off(62))],
        "the Off sits at the last offset so the order holds"
    );
    // The thru key's real Off is now ignored: the device never saw its On.
    out.clear();
    assert!(b.route(&mut out, off(1, 62)).is_none());
    assert!(out.is_empty());
}

#[test]
fn bypass_without_a_change_routes_to_the_device() {
    let mut b = PlayerBypass::new();
    let mut out = buf();
    assert!(!b.begin_batch(&mut out));
    for e in [on(0, 60), off(1, 60), ev(2, NoteKind::AllOff)] {
        assert_eq!(b.route(&mut out, e), Some(e));
    }
    assert!(out.is_empty());
}

#[test]
fn bypass_thru_passes_all_off_and_forgets_the_passed_keys() {
    let mut b = PlayerBypass::new();
    let mut out = buf();
    b.set_param(1.0);
    b.begin_batch(&mut out);
    b.route(&mut out, on(0, 60));
    assert!(b.route(&mut out, ev(1, NoteKind::AllOff)).is_none());
    b.set_param(0.0);
    b.begin_batch(&mut out);
    assert_eq!(seen(&out), [(0, K::On(60)), (1, K::AllOff)]);
}

#[test]
fn bypass_reset_reports_keys_still_passed_through() {
    let mut b = PlayerBypass::new();
    let mut out = buf();
    assert!(!b.reset());
    b.set_param(1.0);
    b.begin_batch(&mut out);
    b.route(&mut out, on(0, 60));
    assert!(b.reset());
    assert!(!b.reset());
}

#[test]
fn bypass_does_not_pass_the_off_of_a_refused_on() {
    let mut b = PlayerBypass::new();
    let mut out = NoteBuffer::with_capacity(ON_HEADROOM - 1);
    b.set_param(1.0);
    b.begin_batch(&mut out);
    assert!(b.route(&mut out, on(0, 60)).is_none());
    assert!(b.route(&mut out, off(1, 60)).is_none());
    assert!(out.is_empty());
}

#[test]
fn router_all_off_from_a_stop_or_locate_then_a_chase_starts_clean() {
    // The engine sends an AllOff on stop and on a locate while playing, then chased Ons.
    let mut r = NoteRouter::new();
    let mut out = buf();
    r.on(&mut out, 0, 60, &[(60, 0.5), (64, 0.5)]);
    out.clear();
    r.all_off(&mut out, 0);
    r.on(&mut out, 0, 60, &[(62, 0.5)]);
    r.off(&mut out, 9, 60);
    assert_eq!(
        seen(&out),
        [
            (0, K::Off(60)),
            (0, K::Off(64)),
            (0, K::On(62)),
            (9, K::Off(62))
        ]
    );
}

#[test]
fn router_loop_wrap_offs_then_chased_ons_keep_the_pairs() {
    // At a loop wrap the engine sends explicit Offs, then chased Ons at the wrap offset.
    let mut r = NoteRouter::new();
    let mut out = buf();
    r.on(&mut out, 0, 60, &[(67, 0.5)]);
    r.off(&mut out, 30, 60);
    r.on(&mut out, 30, 60, &[(67, 0.5)]);
    r.off(&mut out, 50, 60);
    assert_eq!(
        seen(&out),
        [
            (0, K::On(67)),
            (30, K::Off(67)),
            (30, K::On(67)),
            (50, K::Off(67))
        ]
    );
    assert!(!r.any_sounding());
}

#[test]
fn chord_helpers_never_allocate() {
    use atomik_core::theory::{
        ChordKind, DiatonicSize, MAX_CHORD_KEYS, Scale, ScaleKind, chord_into, diatonic_chord_into,
    };
    use atomik_testkit::no_alloc::assert_no_alloc;
    let mut out = [0u8; MAX_CHORD_KEYS];
    let scale = Scale::new(2, ScaleKind::Dorian);
    assert_no_alloc(|| {
        for kind in ChordKind::ALL {
            for inversion in 0..4 {
                chord_into(60, kind, inversion, &mut out);
            }
        }
        for size in DiatonicSize::ALL {
            diatonic_chord_into(scale, 61, size, 1, &mut out);
        }
    });
}

#[test]
fn router_and_bypass_never_allocate() {
    use atomik_testkit::no_alloc::assert_no_alloc;
    let mut r = NoteRouter::new();
    let mut b = PlayerBypass::new();
    let mut out = buf();
    assert_no_alloc(|| {
        r.on(&mut out, 0, 60, &[(60, 0.5), (64, 0.5)]);
        r.swallow_held();
        r.off(&mut out, 1, 60);
        r.close_all(&mut out, 2);
        b.set_param(1.0);
        b.begin_batch(&mut out);
        b.route(&mut out, on(3, 61));
        b.reset();
    });
}

mod props {
    use super::*;
    use proptest::prelude::*;
    use std::collections::BTreeMap;

    #[derive(Debug, Clone)]
    enum Step {
        On(u8, u8),
        Off(u8),
        AllOff,
        CloseAll,
        Swallow,
    }

    fn step() -> impl Strategy<Value = Step> {
        prop_oneof![
            4 => (0u8..12, 0u8..48).prop_map(|(k, shape)| Step::On(48 + k, shape)),
            4 => (0u8..12).prop_map(|k| Step::Off(48 + k)),
            1 => Just(Step::AllOff),
            1 => Just(Step::CloseAll),
            1 => Just(Step::Swallow),
        ]
    }

    /// A param-dependent map: the output keys of `key` under `shape` (changes between On and Off).
    fn map(key: u8, shape: u8) -> Vec<(u8, f32)> {
        let n = 1 + usize::from(shape % 3);
        (0..n)
            .map(|i| (key + shape / 4 + 3 * i as u8, 0.5))
            .collect()
    }

    proptest! {
        #[test]
        fn router_never_hangs_or_doubles_across_random_scripts(
            script in prop::collection::vec(step(), 1..120)
        ) {
            let mut r = NoteRouter::new();
            let mut sim: BTreeMap<u8, u32> = BTreeMap::new();
            let mut frame = 0usize;
            for s in &script {
                let mut out = buf();
                frame = (frame + 1) % 60;
                match s {
                    Step::On(k, shape) => r.on(&mut out, frame, *k, &map(*k, *shape)),
                    Step::Off(k) => r.off(&mut out, frame, *k),
                    Step::AllOff => r.all_off(&mut out, frame),
                    Step::CloseAll => r.close_all(&mut out, frame),
                    Step::Swallow => r.swallow_held(),
                }
                for e in out.as_slice() {
                    match e.kind {
                        NoteKind::On { key, .. } => {
                            let c = sim.entry(key).or_insert(0);
                            prop_assert_eq!(*c, 0, "double On of {}", key);
                            *c = 1;
                        }
                        NoteKind::Off { key } => {
                            let c = sim.entry(key).or_insert(0);
                            prop_assert_eq!(*c, 1, "Off of silent {}", key);
                            *c = 0;
                        }
                        _ => {}
                    }
                }
            }
            // Releasing every input key leaves nothing sounding.
            let mut out = buf();
            for k in 48..60 {
                r.off(&mut out, 0, k);
            }
            r.close_all(&mut out, 0);
            for e in out.as_slice() {
                match e.kind {
                    NoteKind::On { key, .. } => {
                        let c = sim.entry(key).or_insert(0);
                        prop_assert_eq!(*c, 0);
                        *c = 1;
                    }
                    NoteKind::Off { key } => {
                        let c = sim.entry(key).or_insert(0);
                        prop_assert_eq!(*c, 1, "Off of silent {}", key);
                        *c = 0;
                    }
                    _ => {}
                }
            }
            prop_assert!(sim.values().all(|c| *c == 0), "hanging: {:?}", sim);
            prop_assert!(!r.any_sounding());
        }
    }
}
