//! Key & Chord tests (scale stage, chords, voicings, param changes while held, hanging notes).
#![allow(clippy::float_cmp)]

use super::*;
use atomik_core::ExprKind;
use atomik_testkit::harness::DeviceHarness;
use proptest::prelude::*;

use crate::test_support::{off, on};

#[derive(Debug, Clone, Copy, PartialEq)]
enum K {
    On(u8, u8),
    Off(u8),
    AllOff,
    Bend,
    Poly(u8),
    Expr(u8),
}

fn rig() -> DeviceHarness {
    DeviceHarness::new(Box::new(KeyChord::new()), 48_000.0)
}

fn kind_of(e: &NoteEvent) -> K {
    match e.kind {
        NoteKind::On { key, velocity } => K::On(key, (velocity * 127.0).round() as u8),
        NoteKind::Off { key } => K::Off(key),
        NoteKind::AllOff => K::AllOff,
        NoteKind::PitchBend { .. } => K::Bend,
        NoteKind::PolyPressure { key, .. } => K::Poly(key),
        NoteKind::NoteExpr { key, .. } => K::Expr(key),
        _ => panic!("unexpected event"),
    }
}

/// Runs one batch and returns what came out.
fn batch(h: &mut DeviceHarness) -> Vec<K> {
    h.process();
    h.notes_out("notes_out").iter().map(kind_of).collect()
}

fn run(h: &mut DeviceHarness, events: &[NoteEvent]) -> Vec<K> {
    for e in events {
        h.push_note("notes_in", *e);
    }
    batch(h)
}

fn ev(offset: u16, kind: NoteKind) -> NoteEvent {
    NoteEvent { offset, kind }
}

fn set(h: &mut DeviceHarness, key: &str, v: f32) {
    h.set_param(key, v);
}

/// The keys of the `On`s in `out`, in output order.
fn on_keys(out: &[K]) -> Vec<u8> {
    out.iter()
        .filter_map(|k| match k {
            K::On(key, _) => Some(*key),
            _ => None,
        })
        .collect()
}

fn off_keys(out: &[K]) -> Vec<u8> {
    out.iter()
        .filter_map(|k| match k {
            K::Off(key) => Some(*key),
            _ => None,
        })
        .collect()
}

/// Plays `key` on a fresh rig set up by `setup` and returns the sorted keys that sound.
fn chord_of(key: u8, setup: &[(&str, f32)]) -> Vec<u8> {
    let mut h = rig();
    set(&mut h, "scale.mode", 0.0);
    for (k, v) in setup {
        set(&mut h, k, *v);
    }
    let mut keys = on_keys(&run(&mut h, &[on(0, key, 0.5)]));
    keys.sort_unstable();
    keys
}

/// The `chord` param value of a fixed chord kind.
fn fixed(kind: ChordKind) -> f32 {
    4.0 + ChordKind::ALL.iter().position(|k| *k == kind).unwrap() as f32
}

#[test]
fn descriptor_validates_and_orders_params_and_sockets() {
    assert_eq!(DESCRIPTOR.validate(), Ok(()));
    let keys: Vec<_> = DESCRIPTOR.params.iter().map(|p| p.key).collect();
    assert_eq!(
        keys,
        [
            "scale.root",
            "scale.kind",
            "scale.mode",
            "scale.snap",
            "chord",
            "chord.inversion",
            "chord.voicing",
            "chord.octave",
            "chord.velocity",
            "bypass",
        ]
    );
    let sockets: Vec<_> = DESCRIPTOR.sockets.iter().map(|s| s.key).collect();
    assert_eq!(sockets, ["notes_in", "notes_out"]);
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Utility);
}

#[test]
fn chord_choice_labels_match_core_theory() {
    let labels = |key: &str| match DESCRIPTOR.param_by_key(key).unwrap().unit {
        Unit::Choice(l) => l.to_vec(),
        _ => panic!("{key} is not a choice"),
    };
    let scale_kinds: Vec<_> = ScaleKind::ALL.iter().map(|k| k.label()).collect();
    assert_eq!(labels("scale.kind"), scale_kinds);
    assert_eq!(labels("scale.root"), PITCH_NAMES);
    let mut chords = vec!["Off"];
    chords.extend(DiatonicSize::ALL.iter().map(|s| s.label()));
    chords.extend(ChordKind::ALL.iter().map(|k| k.label()));
    assert_eq!(labels("chord"), chords);
    let chord = DESCRIPTOR.param_by_key("chord").unwrap();
    assert_eq!(chord.max as usize, chords.len() - 1);
    let kind = DESCRIPTOR.param_by_key("scale.kind").unwrap();
    assert_eq!(kind.max as usize, ScaleKind::ALL.len() - 1);
    assert_eq!(kind.default as usize, 1, "major");
}

#[test]
fn default_in_scale_notes_pass_with_their_exact_velocity() {
    let mut h = rig();
    assert_eq!(
        run(&mut h, &[on(0, 60, 0.5), off(10, 60)]),
        [K::On(60, 64), K::Off(60)]
    );
    h.push_note("notes_in", on(0, 62, 0.8));
    h.process();
    let NoteKind::On { velocity, .. } = h.notes_out("notes_out")[0].kind else {
        panic!("an On")
    };
    assert_eq!(velocity, 0.8);
}

#[test]
fn snap_nearest_ties_down_like_the_piano_roll() {
    let mut h = rig();
    // C major: C# is between C and D and goes down; F# is between F and G and goes down.
    assert_eq!(run(&mut h, &[on(0, 61, 0.5)]), [K::On(60, 64)]);
    assert_eq!(run(&mut h, &[on(0, 66, 0.5)]), [K::On(65, 64)]);
    run(&mut h, &[ev(0, NoteKind::AllOff)]);
    // The same answer as the piano roll's scale for every key and a few scales.
    for (root, kind) in [(0u8, 1.0f32), (3, 2.0), (7, 10.0), (11, 12.0)] {
        set(&mut h, "scale.root", f32::from(root));
        set(&mut h, "scale.kind", kind);
        let scale = Scale::new(root, ScaleKind::ALL[kind as usize]);
        for key in 0..=127u8 {
            let out = run(&mut h, &[on(0, key, 0.5), off(1, key)]);
            assert_eq!(on_keys(&out), [scale.nearest(key)], "{key} in {scale:?}");
        }
    }
}

#[test]
fn snap_up_and_down_stay_in_range() {
    let mut h = rig();
    set(&mut h, "scale.snap", 1.0);
    assert_eq!(run(&mut h, &[on(0, 61, 0.5)]), [K::On(62, 64)]);
    assert_eq!(run(&mut h, &[on(0, 66, 0.5)]), [K::On(67, 64)]);
    run(&mut h, &[ev(0, NoteKind::AllOff)]);
    set(&mut h, "scale.snap", 2.0);
    assert_eq!(run(&mut h, &[on(0, 61, 0.5)]), [K::On(60, 64)]);
    run(&mut h, &[ev(0, NoteKind::AllOff)]);
    // C# major has no G (pitch class 7): 127 has no in-scale key at or above it, so `up` falls
    // back to the key below. The bottom end is the mirror image: B major has no C.
    set(&mut h, "scale.root", 1.0);
    set(&mut h, "scale.snap", 1.0);
    assert_eq!(on_keys(&run(&mut h, &[on(0, 127, 0.5)])), [126]);
    run(&mut h, &[ev(0, NoteKind::AllOff)]);
    set(&mut h, "scale.root", 11.0);
    set(&mut h, "scale.snap", 2.0);
    assert_eq!(on_keys(&run(&mut h, &[on(0, 0, 0.5)])), [1]);
    set(&mut h, "scale.root", 1.0);
    run(&mut h, &[ev(0, NoteKind::AllOff)]);
    // Every key of every scale lands in range and in the scale.
    for kind in 0..16u8 {
        set(&mut h, "scale.kind", f32::from(kind));
        for snap in [0.0, 1.0, 2.0] {
            set(&mut h, "scale.snap", snap);
            let scale = Scale::new(1, ScaleKind::ALL[usize::from(kind)]);
            for key in 0..=127u8 {
                let out = run(&mut h, &[on(0, key, 0.5), off(1, key)]);
                let got = on_keys(&out);
                assert_eq!(got.len(), 1);
                assert!(got[0] <= 127 && scale.contains(got[0]), "{key} -> {got:?}");
            }
        }
    }
}

#[test]
fn filter_drops_out_of_scale_notes_and_their_offs() {
    let mut h = rig();
    set(&mut h, "scale.mode", 2.0);
    let out = run(
        &mut h,
        &[on(0, 60, 0.5), on(1, 61, 0.5), off(2, 61), off(3, 60)],
    );
    assert_eq!(out, [K::On(60, 64), K::Off(60)]);
}

#[test]
fn scale_mode_off_passes_every_note() {
    assert_eq!(chord_of(61, &[]), [61]);
}

#[test]
fn diatonic_triad_on_every_degree_of_c_major() {
    let expect: [(u8, [u8; 3]); 7] = [
        (60, [60, 64, 67]),
        (62, [62, 65, 69]),
        (64, [64, 67, 71]),
        (65, [65, 69, 72]),
        (67, [67, 71, 74]),
        (69, [69, 72, 76]),
        (71, [71, 74, 77]),
    ];
    for (key, notes) in expect {
        let mut h = rig();
        set(&mut h, "chord", 1.0);
        let mut got = on_keys(&run(&mut h, &[on(0, key, 0.5)]));
        got.sort_unstable();
        assert_eq!(got, notes, "triad on {key}");
    }
}

#[test]
fn diatonic_seventh_ninth_and_chromatic_fallback() {
    assert_eq!(
        chord_of(62, &[("scale.mode", 1.0), ("chord", 2.0)]),
        [62, 65, 69, 72]
    );
    assert_eq!(
        chord_of(60, &[("scale.mode", 1.0), ("chord", 3.0)]),
        [60, 64, 67, 71, 74]
    );
    // A snapped key is the chord root: C# snaps down to C.
    assert_eq!(
        chord_of(61, &[("scale.mode", 1.0), ("chord", 1.0)]),
        [60, 64, 67]
    );
    // The chromatic scale has no degrees: the major shapes stack on the key itself.
    let chromatic = [("scale.mode", 1.0), ("scale.kind", 0.0)];
    let with = |chord: f32| {
        let mut s = chromatic.to_vec();
        s.push(("chord", chord));
        chord_of(61, &s)
    };
    assert_eq!(with(1.0), [61, 65, 68]);
    assert_eq!(with(2.0), [61, 65, 68, 72]);
    assert_eq!(with(3.0), [61, 65, 68, 72, 75]);
}

#[test]
fn fixed_chord_kinds_match_core_intervals() {
    for kind in ChordKind::ALL {
        let want: Vec<u8> = kind.intervals().iter().map(|i| 60 + *i).collect();
        assert_eq!(chord_of(60, &[("chord", fixed(kind))]), want, "{kind:?}");
    }
}

#[test]
fn inversion_is_limited_to_notes_minus_one() {
    let major = fixed(ChordKind::Major);
    assert_eq!(
        chord_of(60, &[("chord", major), ("chord.inversion", 1.0)]),
        [64, 67, 72]
    );
    assert_eq!(
        chord_of(60, &[("chord", major), ("chord.inversion", 2.0)]),
        [67, 72, 76]
    );
    // Three inversions of a triad stop at the second.
    assert_eq!(
        chord_of(60, &[("chord", major), ("chord.inversion", 3.0)]),
        [67, 72, 76]
    );
    // A two-note chord keeps its position.
    let power = fixed(ChordKind::Power);
    assert_eq!(
        chord_of(60, &[("chord", power), ("chord.inversion", 3.0)]),
        [60, 67]
    );
}

#[test]
fn voicings_open_drop2_bass() {
    let major = fixed(ChordKind::Major);
    let seventh = fixed(ChordKind::Seventh);
    let case =
        |chord: f32, voicing: f32| chord_of(60, &[("chord", chord), ("chord.voicing", voicing)]);
    // Open: every second note (the 2nd, 4th, ...) goes up an octave.
    assert_eq!(case(major, 1.0), [60, 67, 76]);
    assert_eq!(case(seventh, 1.0), [60, 67, 76, 82]);
    // Drop 2: the second highest note goes down an octave.
    assert_eq!(case(major, 2.0), [52, 60, 67]);
    assert_eq!(case(seventh, 2.0), [55, 60, 64, 70]);
    // Bass: the root an octave below.
    assert_eq!(case(major, 3.0), [48, 60, 64, 67]);
    // A single note gets its octave below; drop 2 needs three notes.
    assert_eq!(case(0.0, 3.0), [48, 60]);
    assert_eq!(case(fixed(ChordKind::Power), 2.0), [60, 67]);
}

#[test]
fn octave_shifts_the_chord_and_drops_notes_outside_the_range() {
    let major = fixed(ChordKind::Major);
    assert_eq!(
        chord_of(60, &[("chord", major), ("chord.octave", 1.0)]),
        [72, 76, 79]
    );
    assert_eq!(
        chord_of(60, &[("chord", major), ("chord.octave", -2.0)]),
        [36, 40, 43]
    );
    // 124, 128 and 131: only the first fits.
    assert_eq!(
        chord_of(100, &[("chord", major), ("chord.octave", 2.0)]),
        [124]
    );
    assert_eq!(
        chord_of(110, &[("chord", major), ("chord.octave", 1.0)]),
        [122, 126]
    );
}

#[test]
fn added_notes_scale_the_velocity_and_the_root_keeps_it() {
    let mut h = rig();
    set(&mut h, "scale.mode", 0.0);
    set(&mut h, "chord", fixed(ChordKind::Major));
    set(&mut h, "chord.velocity", 0.5);
    h.push_note("notes_in", on(0, 60, 0.8));
    h.process();
    let vels: Vec<f32> = h
        .notes_out("notes_out")
        .iter()
        .map(|e| match e.kind {
            NoteKind::On { velocity, .. } => velocity,
            _ => panic!("an On"),
        })
        .collect();
    assert_eq!(vels, [0.8, 0.8 * 0.5, 0.8 * 0.5]);
    // At 0 only the played note sounds, and its Off is the only Off.
    run(&mut h, &[off(0, 60)]);
    set(&mut h, "chord.velocity", 0.0);
    assert_eq!(run(&mut h, &[on(0, 60, 0.5)]), [K::On(60, 64)]);
    assert_eq!(run(&mut h, &[off(0, 60)]), [K::Off(60)]);
    // A very soft added note still sounds.
    set(&mut h, "chord.velocity", 0.01);
    assert_eq!(
        run(&mut h, &[on(0, 60, 0.1)]),
        [K::On(60, 13), K::On(64, 1), K::On(67, 1)]
    );
}

#[test]
fn overlapping_chords_share_keys_without_cutting() {
    let mut h = rig();
    set(&mut h, "scale.mode", 0.0);
    set(&mut h, "chord", fixed(ChordKind::Major));
    assert_eq!(on_keys(&run(&mut h, &[on(0, 60, 0.5)])), [60, 64, 67]);
    // E major shares E (64) with C major: that key is struck again, the others are new.
    let out = run(&mut h, &[on(0, 64, 0.5)]);
    assert_eq!(
        out,
        [K::Off(64), K::On(64, 64), K::On(68, 64), K::On(71, 64)]
    );
    // Releasing C major leaves the shared E sounding.
    assert_eq!(off_keys(&run(&mut h, &[off(0, 60)])), [60, 67]);
    assert_eq!(off_keys(&run(&mut h, &[off(0, 64)])), [64, 68, 71]);
}

#[test]
fn scale_change_while_held_releases_the_old_chord() {
    let mut h = rig();
    set(&mut h, "chord", 1.0);
    assert_eq!(on_keys(&run(&mut h, &[on(0, 60, 0.5)])), [60, 64, 67]);
    set(&mut h, "scale.kind", 2.0);
    set(&mut h, "chord", fixed(ChordKind::Octave));
    set(&mut h, "scale.root", 5.0);
    assert!(batch(&mut h).is_empty(), "a param change emits nothing");
    assert_eq!(
        run(&mut h, &[off(0, 60)]),
        [K::Off(60), K::Off(64), K::Off(67)]
    );
    // The next press uses the new scale and chord (C is in F minor).
    assert_eq!(on_keys(&run(&mut h, &[on(0, 60, 0.5)])), [60, 72]);
}

#[test]
fn poly_pressure_follows_the_lowest_note_of_the_chord() {
    let mut h = rig();
    set(&mut h, "chord", fixed(ChordKind::Major));
    set(&mut h, "chord.octave", 1.0);
    run(&mut h, &[on(0, 60, 0.5)]);
    let out = run(
        &mut h,
        &[
            ev(
                0,
                NoteKind::PolyPressure {
                    key: 60,
                    value: 0.5,
                },
            ),
            ev(
                1,
                NoteKind::NoteExpr {
                    key: 60,
                    kind: ExprKind::Pressure,
                    value: 0.5,
                },
            ),
            ev(
                2,
                NoteKind::PolyPressure {
                    key: 61,
                    value: 0.5,
                },
            ),
            ev(3, NoteKind::PitchBend { value: 0.2 }),
        ],
    );
    assert_eq!(out, [K::Poly(72), K::Expr(72), K::Bend]);
}

#[test]
fn events_keep_their_offsets() {
    let mut h = rig();
    set(&mut h, "chord", 1.0);
    h.push_note("notes_in", on(5, 60, 0.5));
    h.push_note("notes_in", off(40, 60));
    h.process();
    let offsets: Vec<u16> = h.notes_out("notes_out").iter().map(|e| e.offset).collect();
    assert_eq!(offsets, [5, 5, 5, 40, 40, 40]);
}

#[test]
fn all_off_closes_and_passes_on() {
    let mut h = rig();
    set(&mut h, "chord", 1.0);
    run(&mut h, &[on(0, 60, 0.5)]);
    let out = run(&mut h, &[ev(3, NoteKind::AllOff)]);
    assert_eq!(out, [K::Off(60), K::Off(64), K::Off(67), K::AllOff]);
    assert!(run(&mut h, &[off(0, 60)]).is_empty());
}

#[test]
fn reset_closes_the_sounding_notes_with_an_all_off() {
    let mut h = rig();
    set(&mut h, "chord", 1.0);
    run(&mut h, &[on(0, 60, 0.5)]);
    h.device_mut().reset();
    assert_eq!(batch(&mut h), [K::AllOff]);
    assert!(batch(&mut h).is_empty());
}

#[test]
fn bypass_passes_notes_through_and_leaves_nothing_hanging() {
    let mut h = rig();
    set(&mut h, "chord", 1.0);
    assert_eq!(on_keys(&run(&mut h, &[on(0, 61, 0.5)])), [60, 64, 67]);
    set(&mut h, "bypass", 1.0);
    // The chord closes at the switch; the held key is ignored until its own Off.
    assert_eq!(off_keys(&batch(&mut h)), [60, 64, 67]);
    assert!(run(&mut h, &[off(0, 61)]).is_empty());
    // Bypassed: the note passes untouched, out of scale and without a chord.
    assert_eq!(run(&mut h, &[on(0, 61, 0.5)]), [K::On(61, 64)]);
    set(&mut h, "bypass", 0.0);
    assert_eq!(batch(&mut h), [K::Off(61)]);
}

#[test]
fn process_is_allocation_free_with_every_param_extreme() {
    let mut h = rig();
    for p in DESCRIPTOR.params {
        for v in [p.min, p.max, p.default] {
            h.set_param(p.key, v);
            run(
                &mut h,
                &[on(0, 60, 0.7), off(9, 60), ev(10, NoteKind::AllOff)],
            );
        }
    }
}

#[derive(Debug, Clone)]
enum Act {
    On(u8, u8),
    Off(u8),
    AllOff,
    Param(usize, f32),
    Reset,
}

fn act() -> impl Strategy<Value = Act> {
    prop_oneof![
        4 => (0u8..128, 1u8..128).prop_map(|(k, v)| Act::On(k, v)),
        4 => (0u8..128).prop_map(Act::Off),
        1 => Just(Act::AllOff),
        4 => (0usize..9, -30.0f32..30.0).prop_map(|(p, v)| Act::Param(p, v)),
        1 => Just(Act::Reset),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn no_note_hangs_or_doubles_across_random_scripts(
        acts in proptest::collection::vec(act(), 5..200),
    ) {
        let mut h = rig();
        let mut count = [0i32; 128];
        let apply = |out: &[K], count: &mut [i32; 128]| {
            for k in out {
                match *k {
                    K::On(key, _) => {
                        count[usize::from(key)] += 1;
                        prop_assert!(count[usize::from(key)] <= 1, "doubled {key}");
                    }
                    K::Off(key) => {
                        count[usize::from(key)] -= 1;
                        prop_assert!(count[usize::from(key)] >= 0, "stray off {key}");
                    }
                    K::AllOff => *count = [0; 128],
                    _ => {}
                }
            }
            Ok(())
        };
        for a in acts {
            let out = match a {
                Act::On(k, v) => run(&mut h, &[on(0, k, f32::from(v) / 127.0)]),
                Act::Off(k) => run(&mut h, &[off(0, k)]),
                Act::AllOff => run(&mut h, &[ev(0, NoteKind::AllOff)]),
                Act::Param(p, v) => {
                    h.set_param(DESCRIPTOR.params[p].key, v);
                    batch(&mut h)
                }
                Act::Reset => {
                    h.device_mut().reset();
                    batch(&mut h)
                }
            };
            apply(&out, &mut count)?;
        }
        let out = run(&mut h, &[ev(0, NoteKind::AllOff)]);
        apply(&out, &mut count)?;
        prop_assert!(count.iter().all(|c| *c == 0));
    }
}
