//! Note Repeater tests at 120 BPM and 48 kHz (0.64 ticks per frame, a quarter note is 24 000
//! frames): exact frames, decay and pitch steps, queue limits, hanging notes.
#![allow(clippy::float_cmp)]

use super::*;
use atomik_core::{ExprKind, LoopWrap};
use atomik_testkit::harness::DeviceHarness;
use proptest::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq)]
enum K {
    On(u8, u8),
    Off(u8),
    AllOff,
    Bend,
    Poly(u8),
}

fn rig() -> DeviceHarness {
    DeviceHarness::new(Box::new(NoteRepeater::new()), 48_000.0)
}

fn kind_of(e: &NoteEvent) -> K {
    match e.kind {
        NoteKind::On { key, velocity } => K::On(key, (velocity * 127.0).round() as u8),
        NoteKind::Off { key } => K::Off(key),
        NoteKind::AllOff => K::AllOff,
        NoteKind::PitchBend { .. } => K::Bend,
        NoteKind::PolyPressure { key, .. } => K::Poly(key),
        _ => panic!("unexpected event"),
    }
}

fn ev(offset: u16, kind: NoteKind) -> NoteEvent {
    NoteEvent { offset, kind }
}

/// Runs `batches` batches. `inputs` are `(absolute frame, event kind)`; returns every output
/// event at its absolute frame.
fn play(h: &mut DeviceHarness, batches: u64, inputs: &[(u64, NoteKind)]) -> Vec<(u64, K)> {
    let mut seen = Vec::new();
    for _ in 0..batches {
        let start = h.time_mut().frame;
        for (f, kind) in inputs {
            if (start..start + 64).contains(f) {
                h.push_note("notes_in", ev((f - start) as u16, *kind));
            }
        }
        h.process();
        seen.extend(
            h.notes_out("notes_out")
                .iter()
                .map(|e| (start + u64::from(e.offset), kind_of(e))),
        );
    }
    seen
}

fn on_k(key: u8, v: f32) -> NoteKind {
    NoteKind::On { key, velocity: v }
}

fn off_k(key: u8) -> NoteKind {
    NoteKind::Off { key }
}

fn ons(seen: &[(u64, K)]) -> Vec<(u64, u8, u8)> {
    seen.iter()
        .filter_map(|(f, k)| match k {
            K::On(key, v) => Some((*f, *key, *v)),
            _ => None,
        })
        .collect()
}

fn offs(seen: &[(u64, K)]) -> Vec<(u64, u8)> {
    seen.iter()
        .filter_map(|(f, k)| match k {
            K::Off(key) => Some((*f, *key)),
            _ => None,
        })
        .collect()
}

fn batches_for(frames: u64) -> u64 {
    frames / 64 + 2
}

#[test]
fn descriptor_validates_and_orders_params_and_sockets() {
    assert_eq!(DESCRIPTOR.validate(), Ok(()));
    let keys: Vec<_> = DESCRIPTOR.params.iter().map(|p| p.key).collect();
    assert_eq!(
        keys,
        [
            "time.sync",
            "time.mode",
            "time.ms",
            "repeats",
            "velocity.decay",
            "pitch.step",
            "pitch.scale",
            "scale.root",
            "scale.kind",
            "gate",
            "dry",
            "bypass",
        ]
    );
    let sockets: Vec<_> = DESCRIPTOR.sockets.iter().map(|s| s.key).collect();
    assert_eq!(sockets, ["notes_in", "notes_out"]);
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Utility);
    let d = |k: &str| DESCRIPTOR.param_by_key(k).unwrap();
    assert_eq!(d("time.sync").default, 1.0, "1/8D");
    assert_eq!(d("time.ms").taper, Taper::Exponential);
    assert_eq!((d("time.ms").min, d("time.ms").max), (10.0, 2000.0));
    assert_eq!((d("repeats").min, d("repeats").max), (1.0, 16.0));
    assert_eq!(d("repeats").default, 3.0);
    assert_eq!(d("velocity.decay").default, 0.25);
    assert_eq!(d("gate").default, 0.5);
    assert_eq!(d("dry").default, 1.0);
}

#[test]
fn scale_choice_labels_are_the_key_and_chord_tables() {
    let labels = |key: &str| match DESCRIPTOR.param_by_key(key).unwrap().unit {
        Unit::Choice(l) => l.to_vec(),
        _ => panic!("{key} is not a choice"),
    };
    assert_eq!(labels("scale.kind"), crate::key_chord::SCALE_KIND_LABELS);
    assert_eq!(labels("scale.root"), PITCH_NAMES);
    let kind = DESCRIPTOR.param_by_key("scale.kind").unwrap();
    assert_eq!(kind.max as usize, ScaleKind::ALL.len() - 1);
    assert_eq!(kind.default as usize, 1, "major");
    assert_eq!(
        labels("time.sync"),
        [
            "1/4", "1/8D", "1/8", "1/8T", "1/16D", "1/16", "1/16T", "1/32"
        ]
    );
}

#[test]
fn repeats_land_on_exact_frames_for_every_sync_value() {
    // Ticks per frame is 0.64, so the distance in frames is ticks * 25 / 16.
    let expected = [24_000u64, 18_000, 12_000, 8_000, 9_000, 6_000, 4_000, 3_000];
    for (i, d) in expected.into_iter().enumerate() {
        let mut h = rig();
        h.set_param("time.sync", i as f32);
        h.set_param("repeats", 2.0);
        h.set_param("velocity.decay", 0.0);
        h.set_param("gate", 0.5);
        let seen = play(
            &mut h,
            batches_for(3 * d),
            &[(10, on_k(60, 0.5)), (30, off_k(60))],
        );
        let frames: Vec<_> = ons(&seen).iter().map(|e| e.0).collect();
        assert_eq!(frames, [10, 10 + d, 10 + 2 * d], "sync {i}");
        assert_eq!(
            offs(&seen).iter().map(|e| e.0).collect::<Vec<_>>(),
            [30, 10 + d + d / 2, 10 + 2 * d + d / 2],
            "sync {i}"
        );
    }
}

#[test]
fn free_time_in_ms_is_exact() {
    for (ms, frames) in [(100.0, 4_800u64), (250.0, 12_000), (333.0, 15_984)] {
        let mut h = rig();
        h.set_param("time.mode", 1.0);
        h.set_param("time.ms", ms);
        h.set_param("repeats", 2.0);
        let seen = play(&mut h, batches_for(3 * frames), &[(0, on_k(60, 0.5))]);
        let at: Vec<_> = ons(&seen).iter().map(|e| e.0).collect();
        assert_eq!(at, [0, frames, 2 * frames], "{ms} ms");
    }
}

#[test]
fn velocity_decay_and_pitch_step_per_repeat() {
    let mut h = rig();
    h.set_param("time.mode", 1.0);
    h.set_param("time.ms", 100.0);
    h.set_param("velocity.decay", 0.5);
    h.set_param("pitch.step", 7.0);
    let seen = play(&mut h, batches_for(4 * 4_800), &[(0, on_k(60, 0.8))]);
    let o = ons(&seen);
    let keys: Vec<_> = o.iter().map(|e| e.1).collect();
    assert_eq!(keys, [60, 67, 74, 81]);
    let vels: Vec<_> = o.iter().map(|e| e.2).collect();
    // 0.8, 0.4, 0.2, 0.1 of 127.
    assert_eq!(vels, [102, 51, 25, 13]);
}

#[test]
fn pitch_step_in_key_moves_by_scale_degrees() {
    let mut h = rig();
    h.set_param("time.mode", 1.0);
    h.set_param("time.ms", 100.0);
    h.set_param("pitch.scale", 1.0);
    h.set_param("pitch.step", 2.0);
    h.set_param("velocity.decay", 0.0);
    // C major from C: two degrees per repeat gives E, G, B.
    let seen = play(&mut h, batches_for(4 * 4_800), &[(0, on_k(60, 0.5))]);
    let keys: Vec<_> = ons(&seen).iter().map(|e| e.1).collect();
    assert_eq!(keys, [60, 64, 67, 71]);
    // Another scale and root: D natural minor from D, one degree: E, F, G.
    let mut h = rig();
    h.set_param("time.mode", 1.0);
    h.set_param("time.ms", 100.0);
    h.set_param("pitch.scale", 1.0);
    h.set_param("pitch.step", 1.0);
    h.set_param("scale.root", 2.0);
    h.set_param("scale.kind", 2.0);
    let seen = play(&mut h, batches_for(4 * 4_800), &[(0, on_k(62, 0.9))]);
    let keys: Vec<_> = ons(&seen).iter().map(|e| e.1).collect();
    assert_eq!(keys, [62, 64, 65, 67]);
}

#[test]
fn repeats_stop_when_the_velocity_falls_below_one_step() {
    let mut h = rig();
    h.set_param("time.mode", 1.0);
    h.set_param("time.ms", 10.0);
    h.set_param("velocity.decay", 0.5);
    h.set_param("repeats", 16.0);
    // 0.02, 0.01 (still above 1/127), 0.005 (below): one repeat.
    let seen = play(&mut h, 40, &[(0, on_k(60, 0.02))]);
    assert_eq!(ons(&seen).len(), 2);
}

#[test]
fn repeats_outside_key_range_are_dropped() {
    let mut h = rig();
    h.set_param("time.mode", 1.0);
    h.set_param("time.ms", 100.0);
    h.set_param("pitch.step", 5.0);
    h.set_param("velocity.decay", 0.0);
    let seen = play(&mut h, batches_for(4 * 4_800), &[(0, on_k(120, 0.5))]);
    let keys: Vec<_> = ons(&seen).iter().map(|e| e.1).collect();
    assert_eq!(keys, [120, 125]);
    // Downwards too: key 3 with step -2 gives 1 and then nothing.
    let mut h = rig();
    h.set_param("time.mode", 1.0);
    h.set_param("time.ms", 100.0);
    h.set_param("pitch.step", -2.0);
    h.set_param("velocity.decay", 0.0);
    let seen = play(&mut h, batches_for(4 * 4_800), &[(0, on_k(3, 0.5))]);
    let keys: Vec<_> = ons(&seen).iter().map(|e| e.1).collect();
    assert_eq!(keys, [3, 1]);
}

#[test]
fn gate_sets_the_echo_length() {
    for (gate, len) in [(0.25f32, 3_000u64), (0.5, 6_000), (1.0, 12_000)] {
        let mut h = rig();
        h.set_param("time.sync", 2.0); // 1/8 = 12 000 frames
        h.set_param("repeats", 1.0);
        h.set_param("gate", gate);
        let seen = play(
            &mut h,
            batches_for(30_000),
            &[(0, on_k(60, 0.5)), (5, off_k(60))],
        );
        assert_eq!(offs(&seen), [(5, 60), (12_000 + len, 60)], "gate {gate}");
    }
}

#[test]
fn a_repeat_of_a_key_that_still_sounds_retriggers_it_with_an_off_first() {
    // The dry note is held while the repeat of the same key starts (pitch step 0).
    let mut h = rig();
    h.set_param("time.mode", 1.0);
    h.set_param("time.ms", 100.0);
    h.set_param("repeats", 1.0);
    let seen = play(
        &mut h,
        batches_for(20_000),
        &[(0, on_k(60, 0.5)), (10_000, off_k(60))],
    );
    // On at 0; at 4 800 the repeat retriggers (Off, On); its end does not close the held key;
    // the key goes up at 10 000.
    assert_eq!(
        seen.iter().map(|e| e.1).collect::<Vec<_>>(),
        [K::On(60, 64), K::Off(60), K::On(60, 48), K::Off(60)]
    );
    assert_eq!(seen[1].0, 4_800);
    assert_eq!(seen[3].0, 10_000);
}

#[test]
fn dry_off_removes_the_original() {
    let mut h = rig();
    h.set_param("dry", 0.0);
    h.set_param("repeats", 1.0);
    h.set_param("time.mode", 1.0);
    h.set_param("time.ms", 100.0);
    let seen = play(
        &mut h,
        batches_for(8_000),
        &[(0, on_k(60, 0.5)), (50, off_k(60))],
    );
    assert_eq!(ons(&seen).iter().map(|e| e.0).collect::<Vec<_>>(), [4_800]);
    assert_eq!(offs(&seen), [(4_800 + 2_400, 60)]);
}

#[test]
fn dry_switched_off_while_held_still_ends_the_dry_note() {
    let mut h = rig();
    h.set_param("repeats", 1.0);
    let mut seen = play(&mut h, 2, &[(0, on_k(60, 0.5))]);
    h.set_param("dry", 0.0);
    seen.extend(play(&mut h, 2, &[(200, off_k(60))]));
    assert!(offs(&seen).iter().any(|e| e.0 == 200), "{seen:?}");
}

#[test]
fn input_off_does_not_touch_the_echoes() {
    let mut h = rig();
    h.set_param("time.sync", 5.0); // 1/16 = 6 000 frames
    h.set_param("repeats", 2.0);
    let seen = play(
        &mut h,
        batches_for(20_000),
        &[(0, on_k(60, 0.5)), (1, off_k(60))],
    );
    assert_eq!(ons(&seen).len(), 3);
    assert_eq!(offs(&seen).len(), 3);
}

#[test]
fn bend_cc_and_pressure_pass_in_order() {
    let mut h = rig();
    let seen = play(
        &mut h,
        2,
        &[
            (3, NoteKind::PitchBend { value: 0.5 }),
            (4, on_k(60, 0.5)),
            (
                5,
                NoteKind::PolyPressure {
                    key: 60,
                    value: 0.3,
                },
            ),
            (
                6,
                NoteKind::NoteExpr {
                    key: 90,
                    kind: ExprKind::Pressure,
                    value: 0.3,
                },
            ),
        ],
    );
    // The expression of a key that does not sound is dropped.
    assert_eq!(seen, [(3, K::Bend), (4, K::On(60, 64)), (5, K::Poly(60))]);
}

#[test]
fn full_queue_drops_ons_never_offs() {
    let mut h = rig();
    h.set_param("repeats", 16.0);
    h.set_param("velocity.decay", 0.0);
    h.set_param("time.mode", 1.0);
    h.set_param("time.ms", 10.0);
    // 60 keys at once: far more than 512 entries.
    let mut inputs = Vec::new();
    for k in 20..80u8 {
        inputs.push((0, on_k(k, 0.5)));
    }
    for k in 20..80u8 {
        inputs.push((30, off_k(k)));
    }
    let seen = play(&mut h, 700, &inputs);
    let on_n = ons(&seen).len();
    assert!(on_n < 60 * 17, "some notes were dropped ({on_n})");
    assert!(on_n >= 15 * 17, "the queue was used ({on_n})");
    for k in 20..80u8 {
        let n_on = ons(&seen).iter().filter(|e| e.1 == k).count();
        let n_off = offs(&seen).iter().filter(|e| e.1 == k).count();
        assert_eq!(n_on, n_off, "key {k} is balanced");
    }
}

#[test]
fn all_off_clears_pending_echoes() {
    let mut h = rig();
    let mut seen = play(&mut h, 2, &[(0, on_k(60, 0.5))]);
    seen.extend(play(&mut h, 2, &[(130, NoteKind::AllOff)]));
    seen.extend(play(&mut h, 2_000, &[]));
    assert_eq!(
        seen.iter().map(|e| e.1).collect::<Vec<_>>(),
        [K::On(60, 64), K::Off(60), K::AllOff]
    );
}

#[test]
fn echoes_continue_across_a_loop_wrap() {
    let mut h = rig();
    h.set_param("time.mode", 1.0);
    h.set_param("time.ms", 100.0);
    h.set_param("repeats", 2.0);
    let mut seen = play(&mut h, 1, &[(0, on_k(60, 0.5))]);
    // A loop wrap at frame 74: the engine closes the note and chases it at the loop start.
    h.time_mut().loop_wrap = Some(LoopWrap {
        offset: 10,
        to_tick: 0.0,
    });
    seen.extend(play(
        &mut h,
        batches_for(20_000),
        &[(74, off_k(60)), (74, on_k(60, 0.5))],
    ));
    // Both notes echo; nothing was cleared at the wrap.
    assert_eq!(ons(&seen).len(), 6);
    assert!(!seen.iter().any(|e| e.1 == K::AllOff));
}

#[test]
fn runs_while_stopped() {
    let mut h = rig();
    h.time_mut().playing = false;
    h.set_param("time.sync", 2.0);
    h.set_param("repeats", 2.0);
    let seen = play(&mut h, batches_for(30_000), &[(7, on_k(60, 0.5))]);
    let at: Vec<_> = ons(&seen).iter().map(|e| e.0).collect();
    assert_eq!(at, [7, 7 + 12_000, 7 + 24_000]);
}

#[test]
fn a_tempo_change_does_not_move_scheduled_echoes() {
    let mut h = rig();
    h.set_param("time.sync", 2.0);
    h.set_param("repeats", 2.0);
    let mut seen = play(&mut h, 2, &[(0, on_k(60, 0.5))]);
    h.time_mut().ticks_per_frame *= 2.0;
    seen.extend(play(&mut h, batches_for(30_000), &[]));
    let at: Vec<_> = ons(&seen).iter().map(|e| e.0).collect();
    assert_eq!(at, [0, 12_000, 24_000]);
}

#[test]
fn bypass_clears_the_queue_and_leaves_nothing_hanging() {
    let mut h = rig();
    let mut seen = play(&mut h, 2, &[(0, on_k(60, 0.5))]);
    h.set_param("bypass", 1.0);
    seen.extend(play(&mut h, 2, &[]));
    // The sounding dry note closes at the switch; the held key is ignored until its Off.
    assert_eq!(offs(&seen), [(128, 60)]);
    assert_eq!(ons(&seen).len(), 1, "the queued echoes are gone");
    // Bypassed: the Off of the held key is swallowed, a new note passes untouched.
    let seen = play(&mut h, 2, &[(260, off_k(60)), (270, on_k(62, 0.5))]);
    assert_eq!(seen, [(270, K::On(62, 64))]);
    // Back on: the key passed while bypassed closes, and no echo ever comes back.
    h.set_param("bypass", 0.0);
    let seen = play(&mut h, batches_for(60_000), &[]);
    assert_eq!(seen, [(384, K::Off(62))]);
}

#[test]
fn reset_closes_the_sounding_notes_with_an_all_off() {
    let mut h = rig();
    play(&mut h, 1, &[(0, on_k(60, 0.5))]);
    h.device_mut().reset();
    let seen = play(&mut h, 1, &[]);
    assert_eq!(seen.iter().map(|e| e.1).collect::<Vec<_>>(), [K::AllOff]);
    assert!(play(&mut h, 2_000, &[]).is_empty(), "the queue is gone");
}

#[test]
fn process_is_allocation_free_with_every_param_extreme() {
    let mut h = rig();
    for p in DESCRIPTOR.params {
        for v in [p.min, p.max, p.default] {
            h.set_param(p.key, v);
            play(
                &mut h,
                2,
                &[(0, on_k(60, 0.7)), (9, off_k(60)), (70, NoteKind::AllOff)],
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
        (36u8..84, 1u8..=127).prop_map(|(k, v)| Act::On(k, v)),
        (36u8..84).prop_map(Act::Off),
        Just(Act::AllOff),
        (0usize..DESCRIPTOR.params.len() - 1, 0.0f32..=1.0).prop_map(|(p, n)| {
            let d = &DESCRIPTOR.params[p];
            Act::Param(p, d.min + n * (d.max - d.min))
        }),
        Just(Act::Reset),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn no_note_hangs_or_doubles_across_random_scripts(
        acts in proptest::collection::vec(act(), 5..120),
    ) {
        let mut h = rig();
        let mut count = [0i32; 128];
        let apply = |out: &[(u64, K)], count: &mut [i32; 128]| {
            for (_, k) in out {
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
            let start = h.time_mut().frame;
            let out = match a {
                Act::On(k, v) => play(&mut h, 3, &[(start, on_k(k, f32::from(v) / 127.0))]),
                Act::Off(k) => play(&mut h, 3, &[(start, off_k(k))]),
                Act::AllOff => play(&mut h, 3, &[(start, NoteKind::AllOff)]),
                Act::Param(p, v) => {
                    h.set_param(DESCRIPTOR.params[p].key, v);
                    play(&mut h, 3, &[])
                }
                Act::Reset => {
                    h.device_mut().reset();
                    play(&mut h, 3, &[])
                }
            };
            apply(&out, &mut count)?;
        }
        let start = h.time_mut().frame;
        let out = play(&mut h, 2, &[(start, NoteKind::AllOff)]);
        apply(&out, &mut count)?;
        prop_assert!(count.iter().all(|c| *c == 0));
        // And nothing comes after the AllOff.
        prop_assert!(play(&mut h, 4_000, &[]).is_empty());
    }
}
