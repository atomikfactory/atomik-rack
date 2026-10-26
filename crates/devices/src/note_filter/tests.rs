//! Note filter tests (stages, param changes while held, passthrough, hanging notes).
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
    Cc,
    Channel,
    Poly(u8),
    Expr(u8),
}

fn rig() -> DeviceHarness {
    DeviceHarness::new(Box::new(NoteFilter::new()), 48_000.0)
}

fn kind_of(e: &NoteEvent) -> K {
    match e.kind {
        NoteKind::On { key, velocity } => K::On(key, (velocity * 127.0).round() as u8),
        NoteKind::Off { key } => K::Off(key),
        NoteKind::AllOff => K::AllOff,
        NoteKind::PitchBend { .. } => K::Bend,
        NoteKind::Cc { .. } => K::Cc,
        NoteKind::ChannelPressure { .. } => K::Channel,
        NoteKind::PolyPressure { key, .. } => K::Poly(key),
        NoteKind::NoteExpr { key, .. } => K::Expr(key),
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

#[test]
fn descriptor_validates_and_orders_params_and_sockets() {
    assert_eq!(DESCRIPTOR.validate(), Ok(()));
    let keys: Vec<_> = DESCRIPTOR.params.iter().map(|p| p.key).collect();
    assert_eq!(
        keys,
        [
            "transpose",
            "octave",
            "key.low",
            "key.high",
            "key.mode",
            "velocity.low",
            "velocity.high",
            "velocity.scale",
            "velocity.offset",
            "velocity.curve",
            "velocity.fixed",
            "pass.bend",
            "pass.cc",
            "pass.pressure",
            "bypass",
        ]
    );
    let sockets: Vec<_> = DESCRIPTOR.sockets.iter().map(|s| s.key).collect();
    assert_eq!(sockets, ["notes_in", "notes_out"]);
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Utility);
}

#[test]
fn default_params_pass_notes_unchanged() {
    let mut h = rig();
    let out = run(&mut h, &[on(0, 60, 0.5), off(10, 60)]);
    assert_eq!(out, [K::On(60, 64), K::Off(60)]);
}

#[test]
fn default_velocity_passes_exactly_not_rounded_to_a_midi_step() {
    let mut h = rig();
    h.push_note("notes_in", on(0, 60, 0.8));
    h.process();
    let NoteKind::On { velocity, .. } = h.notes_out("notes_out")[0].kind else {
        panic!("an On")
    };
    assert_eq!(velocity, 0.8);
}

#[test]
fn transpose_and_octave_shift_on_and_off_together() {
    let mut h = rig();
    set(&mut h, "transpose", 7.0);
    set(&mut h, "octave", -1.0);
    let out = run(&mut h, &[on(0, 60, 0.5), off(10, 60)]);
    assert_eq!(out, [K::On(55, 64), K::Off(55)]);
}

#[test]
fn out_of_range_result_is_dropped_not_clamped() {
    let mut h = rig();
    set(&mut h, "transpose", 12.0);
    let out = run(
        &mut h,
        &[on(0, 120, 0.5), on(0, 100, 0.5), off(10, 120), off(10, 100)],
    );
    assert_eq!(out, [K::On(112, 64), K::Off(112)]);
    set(&mut h, "transpose", -12.0);
    let out = run(&mut h, &[on(0, 5, 0.5), off(10, 5)]);
    assert!(out.is_empty());
}

#[test]
fn key_range_pass_inside_outside_and_fold() {
    let mut h = rig();
    set(&mut h, "key.low", 48.0);
    set(&mut h, "key.high", 59.0);
    let evs = [on(0, 40, 0.5), on(0, 50, 0.5), on(0, 70, 0.5)];
    assert_eq!(run(&mut h, &evs), [K::On(50, 64)]);
    run(&mut h, &[ev(0, NoteKind::AllOff)]);
    set(&mut h, "key.mode", 1.0);
    assert_eq!(run(&mut h, &evs), [K::On(40, 64), K::On(70, 64)]);
    run(&mut h, &[ev(0, NoteKind::AllOff)]);
    set(&mut h, "key.mode", 2.0);
    // Every input passes; the output folds into 48..=59 by octaves.
    assert_eq!(
        run(&mut h, &evs),
        [K::On(52, 64), K::On(50, 64), K::On(58, 64)]
    );
    // A range narrower than an octave drops the keys no octave reaches.
    run(&mut h, &[ev(0, NoteKind::AllOff)]);
    set(&mut h, "key.high", 52.0);
    assert_eq!(
        run(&mut h, &[on(0, 58, 0.5), on(0, 50, 0.5)]),
        [K::On(50, 64)]
    );
}

#[test]
fn velocity_range_drops_outside_notes_and_their_offs() {
    let mut h = rig();
    set(&mut h, "velocity.low", 20.0);
    set(&mut h, "velocity.high", 80.0);
    let out = run(
        &mut h,
        &[
            on(0, 60, 10.0 / 127.0),
            on(0, 62, 50.0 / 127.0),
            on(0, 64, 100.0 / 127.0),
            off(5, 60),
            off(5, 62),
            off(5, 64),
        ],
    );
    assert_eq!(out, [K::On(62, 50), K::Off(62)]);
}

#[test]
fn velocity_scale_offset_curve_and_fixed() {
    let mut h = rig();
    set(&mut h, "velocity.scale", 0.5);
    assert_eq!(run(&mut h, &[on(0, 60, 100.0 / 127.0)]), [K::On(60, 50)]);
    set(&mut h, "velocity.scale", 1.0);
    set(&mut h, "velocity.offset", 30.0);
    assert_eq!(run(&mut h, &[on(0, 61, 100.0 / 127.0)]), [K::On(61, 127)]);
    set(&mut h, "velocity.offset", -200.0);
    assert_eq!(run(&mut h, &[on(0, 62, 100.0 / 127.0)]), [K::On(62, 1)]);
    set(&mut h, "velocity.offset", 0.0);
    set(&mut h, "velocity.curve", 1.0);
    let K::On(_, lifted) = run(&mut h, &[on(0, 63, 0.25)])[0] else {
        panic!("an On")
    };
    assert!(lifted > 32, "a positive curve lifts soft notes: {lifted}");
    set(&mut h, "velocity.curve", -1.0);
    let K::On(_, pressed) = run(&mut h, &[on(0, 64, 0.25)])[0] else {
        panic!("an On")
    };
    assert!(
        pressed < 32,
        "a negative curve presses them down: {pressed}"
    );
    set(&mut h, "velocity.curve", 0.0);
    set(&mut h, "velocity.fixed", 100.0);
    assert_eq!(run(&mut h, &[on(0, 65, 0.1)]), [K::On(65, 100)]);
}

#[test]
fn transpose_change_while_held_releases_the_old_key() {
    let mut h = rig();
    set(&mut h, "transpose", 12.0);
    assert_eq!(run(&mut h, &[on(0, 60, 0.5)]), [K::On(72, 64)]);
    set(&mut h, "transpose", -5.0);
    assert_eq!(run(&mut h, &[off(0, 60)]), [K::Off(72)]);
    // The next press uses the new transpose.
    assert_eq!(run(&mut h, &[on(0, 60, 0.5)]), [K::On(55, 64)]);
}

#[test]
fn two_keys_landing_on_one_output_close_on_the_last_off() {
    let mut h = rig();
    set(&mut h, "transpose", 2.0);
    assert_eq!(run(&mut h, &[on(0, 60, 0.5)]), [K::On(62, 64)]);
    set(&mut h, "transpose", 0.0);
    // 62 played at transpose 0 lands on the sounding 62: retrigger, then one Off at the end.
    assert_eq!(run(&mut h, &[on(0, 62, 0.5)]), [K::Off(62), K::On(62, 64)]);
    assert!(run(&mut h, &[off(0, 60)]).is_empty());
    assert_eq!(run(&mut h, &[off(0, 62)]), [K::Off(62)]);
}

#[test]
fn pass_toggles_filter_bend_cc_and_pressure() {
    let mut h = rig();
    let evs = [
        ev(0, NoteKind::PitchBend { value: 0.5 }),
        ev(
            0,
            NoteKind::Cc {
                controller: 1,
                value: 0.5,
            },
        ),
        ev(0, NoteKind::ChannelPressure { value: 0.5 }),
    ];
    assert_eq!(run(&mut h, &evs), [K::Bend, K::Cc, K::Channel]);
    set(&mut h, "pass.bend", 0.0);
    assert_eq!(run(&mut h, &evs), [K::Cc, K::Channel]);
    set(&mut h, "pass.cc", 0.0);
    assert_eq!(run(&mut h, &evs), [K::Channel]);
    set(&mut h, "pass.pressure", 0.0);
    assert!(run(&mut h, &evs).is_empty());
}

#[test]
fn poly_pressure_follows_the_transposed_key() {
    let mut h = rig();
    set(&mut h, "transpose", 12.0);
    let evs = [
        ev(
            0,
            NoteKind::PolyPressure {
                key: 60,
                value: 0.5,
            },
        ),
        on(1, 60, 0.5),
        ev(
            2,
            NoteKind::PolyPressure {
                key: 60,
                value: 0.5,
            },
        ),
        ev(
            3,
            NoteKind::NoteExpr {
                key: 60,
                kind: ExprKind::Slide,
                value: 0.2,
            },
        ),
        ev(
            4,
            NoteKind::PolyPressure {
                key: 61,
                value: 0.5,
            },
        ),
    ];
    // Pressure before the On or for a key that is not down has nothing to follow.
    assert_eq!(run(&mut h, &evs), [K::On(72, 64), K::Poly(72), K::Expr(72)]);
    set(&mut h, "pass.pressure", 0.0);
    let out = run(
        &mut h,
        &[ev(
            0,
            NoteKind::PolyPressure {
                key: 60,
                value: 0.5,
            },
        )],
    );
    assert!(out.is_empty());
}

#[test]
fn events_keep_their_offsets() {
    let mut h = rig();
    set(&mut h, "transpose", 1.0);
    h.push_note("notes_in", on(5, 60, 0.5));
    h.push_note("notes_in", off(50, 60));
    h.process();
    let offsets: Vec<u16> = h.notes_out("notes_out").iter().map(|e| e.offset).collect();
    assert_eq!(offsets, [5, 50]);
}

#[test]
fn all_off_closes_and_passes_on() {
    let mut h = rig();
    run(&mut h, &[on(0, 60, 0.5), on(0, 64, 0.5)]);
    let out = run(&mut h, &[ev(3, NoteKind::AllOff)]);
    assert_eq!(out, [K::Off(60), K::Off(64), K::AllOff]);
    // The released keys' Offs that follow emit nothing.
    assert!(run(&mut h, &[off(0, 60)]).is_empty());
}

#[test]
fn reset_closes_the_sounding_notes_with_an_all_off() {
    let mut h = rig();
    run(&mut h, &[on(0, 60, 0.5)]);
    h.device_mut().reset();
    assert_eq!(batch(&mut h), [K::AllOff]);
    assert!(batch(&mut h).is_empty());
}

#[test]
fn bypass_passes_notes_through_and_leaves_nothing_hanging() {
    let mut h = rig();
    set(&mut h, "transpose", 12.0);
    assert_eq!(run(&mut h, &[on(0, 60, 0.5)]), [K::On(72, 64)]);
    set(&mut h, "bypass", 1.0);
    // The switch closes what the filter sent; the key held across it is not retriggered.
    assert_eq!(batch(&mut h), [K::Off(72)]);
    assert!(run(&mut h, &[off(0, 60)]).is_empty());
    // Bypassed: the notes pass through untransposed.
    assert_eq!(run(&mut h, &[on(0, 62, 0.5)]), [K::On(62, 64)]);
    set(&mut h, "bypass", 0.0);
    assert_eq!(batch(&mut h), [K::Off(62)]);
    assert!(run(&mut h, &[off(0, 62)]).is_empty());
    assert_eq!(run(&mut h, &[on(0, 62, 0.5)]), [K::On(74, 64)]);
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
        3 => (0usize..14, -200.0f32..200.0).prop_map(|(p, v)| Act::Param(p, v)),
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
