//! Note Delay tests (exact frames, kept lengths, swing, humanize, queue limits, hanging notes).
#![allow(clippy::float_cmp)]

use super::*;
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
    DeviceHarness::new(Box::new(NoteDelay::new()), 48_000.0)
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
            "delay.mode",
            "delay.sync",
            "delay.ms",
            "swing",
            "swing.grid",
            "humanize.time",
            "humanize.velocity",
            "random.seed",
            "bypass",
        ]
    );
    let sockets: Vec<_> = DESCRIPTOR.sockets.iter().map(|s| s.key).collect();
    assert_eq!(sockets, ["notes_in", "notes_out"]);
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Utility);
    let d = |k: &str| DESCRIPTOR.param_by_key(k).unwrap();
    assert_eq!(d("delay.sync").default, 5.0, "1/16");
    assert_eq!(d("delay.sync").max, 8.0);
    assert_eq!((d("delay.ms").min, d("delay.ms").max), (0.0, 1000.0));
    assert_eq!((d("swing").min, d("swing").max), (0.0, 75.0));
    assert_eq!(d("swing.grid").default, 1.0, "1/16");
    assert_eq!(d("humanize.time").max, 20.0);
    assert_eq!(d("humanize.velocity").max, 32.0);
    assert_eq!(d("random.seed").max, 999.0);
    let Unit::Choice(sync) = d("delay.sync").unit else {
        panic!("not a choice")
    };
    assert_eq!(
        sync,
        [
            "0", "1/64", "1/32T", "1/32", "1/16T", "1/16", "1/8T", "1/8", "1/4"
        ]
    );
}

#[test]
fn delay_shifts_every_event_and_keeps_lengths() {
    // Ticks per frame is 0.64: frames = ticks * 25 / 16.
    let expected = [
        0u64, 1_500, 2_000, 3_000, 4_000, 6_000, 8_000, 12_000, 24_000,
    ];
    for (i, d) in expected.into_iter().enumerate() {
        let mut h = rig();
        h.set_param("delay.sync", i as f32);
        let seen = play(
            &mut h,
            batches_for(40_000 + d),
            &[
                (10, on_k(60, 0.5)),
                (20, NoteKind::PitchBend { value: 0.5 }),
                (5_010, off_k(60)),
                (
                    5_020,
                    NoteKind::PolyPressure {
                        key: 60,
                        value: 0.2,
                    },
                ),
            ],
        );
        // The pressure arrives after the Off: its note no longer sounds, so it is dropped.
        assert_eq!(
            seen,
            [
                (10 + d, K::On(60, 64)),
                (20 + d, K::Bend),
                (5_010 + d, K::Off(60)),
            ],
            "sync {i}"
        );
    }
}

#[test]
fn free_delay_in_ms_is_exact() {
    for (ms, frames) in [
        (0.0, 0u64),
        (100.0, 4_800),
        (333.0, 15_984),
        (1000.0, 48_000),
    ] {
        let mut h = rig();
        h.set_param("delay.mode", 1.0);
        h.set_param("delay.ms", ms);
        let seen = play(
            &mut h,
            batches_for(2_000 + frames),
            &[(100, on_k(60, 0.5)), (900, off_k(60))],
        );
        assert_eq!(
            seen,
            [(100 + frames, K::On(60, 64)), (900 + frames, K::Off(60))],
            "{ms} ms"
        );
    }
}

#[test]
fn delay_never_adds_notes() {
    let mut h = rig();
    h.set_param("humanize.time", 20.0);
    h.set_param("humanize.velocity", 32.0);
    let mut inputs = Vec::new();
    for i in 0..20u64 {
        inputs.push((i * 700, on_k(40 + i as u8, 0.6)));
        inputs.push((i * 700 + 300, off_k(40 + i as u8)));
    }
    let seen = play(&mut h, batches_for(20 * 700 + 20_000), &inputs);
    assert_eq!(ons(&seen).len(), 20);
    assert_eq!(offs(&seen).len(), 20);
}

#[test]
fn swing_moves_only_odd_offbeat_notes_while_playing() {
    // 1/16 grid is 3 840 ticks = 6 000 frames; swing 50 % shifts by 960 ticks = 1 500 frames.
    let notes = [
        (0u64, 0u64),    // even step
        (6_000, 1_500),  // odd step, first half
        (12_000, 0),     // even step
        (9_125, 0),      // odd step, second half (tick 5 840)
        (18_000, 1_500), // odd step (the 4th), first half
    ];
    let inputs: Vec<_> = notes
        .iter()
        .enumerate()
        .flat_map(|(i, (f, _))| {
            let key = 50 + i as u8;
            [(*f, on_k(key, 0.5)), (*f + 100, off_k(key))]
        })
        .collect();
    let run = |playing: bool, grid: f32| {
        let mut h = rig();
        h.time_mut().playing = playing;
        h.set_param("delay.sync", 0.0);
        h.set_param("swing", 50.0);
        h.set_param("swing.grid", grid);
        ons(&play(&mut h, batches_for(25_000), &inputs))
    };
    let moved: Vec<_> = run(true, 1.0).iter().map(|e| e.0).collect();
    let mut want: Vec<_> = notes.iter().map(|(f, d)| f + d).collect();
    want.sort_unstable();
    assert_eq!(moved, want);
    // Stopped: nothing moves.
    let stopped: Vec<_> = run(false, 1.0).iter().map(|e| e.0).collect();
    let mut plain: Vec<_> = notes.iter().map(|(f, _)| *f).collect();
    plain.sort_unstable();
    assert_eq!(stopped, plain);
    // The 1/8 grid (7 680 ticks = 12 000 frames, shift 3 000 frames): only the second step
    // (frames 12 000 to 18 000) moves.
    let eighth: Vec<_> = run(true, 0.0).iter().map(|e| e.0).collect();
    let mut want: Vec<_> = vec![0, 12_000 + 3_000, 9_125, 6_000, 18_000];
    want.sort_unstable();
    assert_eq!(eighth, want);
}

#[test]
fn swing_shift_follows_the_step_rule_of_the_step_players() {
    // swing 75 on a 1/16 step: (75 * 3840 + 100) / 200 = 1440 ticks = 2 250 frames.
    let mut h = rig();
    h.set_param("delay.sync", 0.0);
    h.set_param("swing", 75.0);
    let seen = play(&mut h, batches_for(12_000), &[(6_000, on_k(60, 0.5))]);
    assert_eq!(ons(&seen)[0].0, 6_000 + 2_250);
}

#[test]
fn humanize_is_deterministic_for_a_seed() {
    let inputs: Vec<_> = (0..16u64)
        .flat_map(|i| {
            let key = 40 + i as u8;
            [(i * 3_000, on_k(key, 0.5)), (i * 3_000 + 1_000, off_k(key))]
        })
        .collect();
    let run = |seed: f32| {
        let mut h = rig();
        h.set_param("delay.sync", 0.0);
        h.set_param("humanize.time", 20.0);
        h.set_param("humanize.velocity", 32.0);
        h.set_param("random.seed", seed);
        play(&mut h, batches_for(16 * 3_000 + 2_000), &inputs)
    };
    let a = run(7.0);
    assert_eq!(a, run(7.0), "same seed, same result");
    assert_ne!(a, run(8.0), "another seed, another result");
    // Late only, by at most 20 ms (960 frames); the velocity moves at most 32 steps.
    for (i, (f, _, v)) in ons(&a).into_iter().enumerate() {
        let at = i as u64 * 3_000;
        assert!((at..=at + 960).contains(&f), "note {i} at {f}");
        assert!((v as i32 - 64).abs() <= 33, "velocity {v}");
    }
    assert!(ons(&a).iter().any(|e| e.2 != 64), "the velocity moved");
    assert!(
        ons(&a)
            .iter()
            .enumerate()
            .any(|(i, e)| e.0 != i as u64 * 3_000),
        "the time moved"
    );
}

#[test]
fn humanize_follows_the_transport_tick_not_the_device_clock() {
    // The same song position at a different device frame gets the same draw.
    let run = |idle_batches: u64| {
        let mut h = rig();
        h.set_param("delay.sync", 0.0);
        h.set_param("humanize.time", 20.0);
        h.set_param("humanize.velocity", 32.0);
        h.set_param("random.seed", 3.0);
        play(&mut h, idle_batches, &[]);
        h.time_mut().tick = 10_000.0;
        let start = h.time_mut().frame;
        let seen = play(&mut h, 40, &[(start + 5, on_k(60, 0.5))]);
        let (f, _, v) = ons(&seen)[0];
        (f - start, v)
    };
    assert_eq!(run(0), run(25));
}

#[test]
fn humanize_off_keeps_every_velocity_exact() {
    let mut h = rig();
    h.set_param("delay.sync", 0.0);
    let seen = play(&mut h, 2, &[(3, on_k(60, 0.3))]);
    assert_eq!(seen, [(3, K::On(60, 38))]);
    let mut h = rig();
    h.set_param("delay.sync", 0.0);
    h.push_note("notes_in", ev(3, on_k(60, 0.123_456)));
    h.process();
    let NoteKind::On { velocity, .. } = h.notes_out("notes_out")[0].kind else {
        panic!()
    };
    assert_eq!(velocity, 0.123_456);
}

#[test]
fn an_off_follows_the_delay_its_on_got_even_when_the_param_moved() {
    let mut h = rig();
    let mut seen = play(&mut h, 2, &[(10, on_k(60, 0.5))]);
    h.set_param("delay.sync", 8.0); // 1/4 = 24 000
    seen.extend(play(&mut h, batches_for(40_000), &[(3_000, off_k(60))]));
    assert_eq!(
        seen,
        [(6_010, K::On(60, 64)), (9_000, K::Off(60))],
        "the note keeps its 3 000 frames"
    );
}

#[test]
fn a_new_on_of_a_held_key_ends_the_old_note_first() {
    let mut h = rig();
    h.set_param("delay.sync", 1.0);
    let seen = play(
        &mut h,
        batches_for(4_000),
        &[(0, on_k(60, 0.5)), (100, on_k(60, 0.7)), (200, off_k(60))],
    );
    let kinds: Vec<_> = seen.iter().map(|e| e.1).collect();
    assert_eq!(
        kinds,
        [K::On(60, 64), K::Off(60), K::On(60, 89), K::Off(60)]
    );
}

#[test]
fn full_queue_drops_ons_never_offs() {
    let mut h = rig();
    h.set_param("delay.sync", 8.0);
    let mut inputs = Vec::new();
    for k in 0..128u8 {
        inputs.push((u64::from(k), on_k(k, 0.5)));
    }
    for k in 0..128u8 {
        inputs.push((200 + u64::from(k), off_k(k)));
    }
    // A second round of 128 notes: the 256 notes use every slot of the queue.
    for k in 0..128u8 {
        inputs.push((1_000 + u64::from(k), on_k(k, 0.5)));
        inputs.push((1_300 + u64::from(k), off_k(k)));
    }
    let seen = play(&mut h, batches_for(60_000), &inputs);
    for k in 0..128u8 {
        let n_on = ons(&seen).iter().filter(|e| e.1 == k).count();
        let n_off = offs(&seen).iter().filter(|e| e.1 == k).count();
        assert_eq!(n_on, n_off, "key {k} is balanced");
    }
    // 128 + 128 notes of two entries each fit exactly; every one is delivered.
    assert_eq!(ons(&seen).len(), 256);
}

#[test]
fn a_flood_of_notes_beyond_the_queue_is_dropped_without_hanging() {
    let mut h = rig();
    h.set_param("delay.sync", 8.0);
    let mut inputs = Vec::new();
    // 400 note pairs inside the delay: only the first 256 fit.
    for i in 0..400u64 {
        inputs.push((i * 10, on_k((i % 100) as u8 + 10, 0.5)));
        inputs.push((i * 10 + 5, off_k((i % 100) as u8 + 10)));
    }
    let seen = play(&mut h, batches_for(80_000), &inputs);
    assert!(ons(&seen).len() <= 256);
    assert!(ons(&seen).len() >= 250);
    assert_eq!(ons(&seen).len(), offs(&seen).len());
}

#[test]
fn all_off_clears_the_queue_and_closes_what_sounded() {
    let mut h = rig();
    let mut seen = play(&mut h, 2, &[(0, on_k(60, 0.5)), (10, on_k(62, 0.5))]);
    // Nothing has sounded yet (1/16 = 6 000 frames); the AllOff voids both.
    seen.extend(play(&mut h, 2, &[(130, NoteKind::AllOff)]));
    seen.extend(play(&mut h, batches_for(20_000), &[]));
    assert_eq!(seen, [(130, K::AllOff)]);
    // A note that sounds is closed.
    let mut h = rig();
    h.set_param("delay.sync", 0.0);
    let seen = play(&mut h, 4, &[(0, on_k(60, 0.5)), (130, NoteKind::AllOff)]);
    assert_eq!(
        seen,
        [(0, K::On(60, 64)), (130, K::Off(60)), (130, K::AllOff)]
    );
}

#[test]
fn runs_while_stopped() {
    let mut h = rig();
    h.time_mut().playing = false;
    let seen = play(
        &mut h,
        batches_for(10_000),
        &[(7, on_k(60, 0.5)), (70, off_k(60))],
    );
    assert_eq!(seen, [(6_007, K::On(60, 64)), (6_070, K::Off(60))]);
}

#[test]
fn bypass_clears_the_queue_and_leaves_nothing_hanging() {
    let mut h = rig();
    h.set_param("delay.sync", 0.0);
    let mut seen = play(&mut h, 2, &[(0, on_k(60, 0.5))]);
    h.set_param("delay.sync", 5.0);
    seen.extend(play(&mut h, 2, &[(130, on_k(62, 0.5))]));
    // The note sounding at the switch is closed; the later one never sounded.
    h.set_param("bypass", 1.0);
    seen.extend(play(&mut h, 2, &[]));
    assert_eq!(
        seen,
        [(0, K::On(60, 64)), (256, K::Off(60))],
        "the pending note is cleared"
    );
    // Bypassed: events pass untouched, the held key's Off is swallowed.
    let seen = play(&mut h, 2, &[(390, off_k(60)), (400, on_k(64, 0.5))]);
    assert_eq!(seen, [(400, K::On(64, 64))]);
    h.set_param("bypass", 0.0);
    let seen = play(&mut h, batches_for(20_000), &[]);
    assert_eq!(seen, [(512, K::Off(64))]);
}

#[test]
fn reset_closes_the_sounding_notes_with_an_all_off() {
    let mut h = rig();
    h.set_param("delay.sync", 0.0);
    play(&mut h, 1, &[(0, on_k(60, 0.5))]);
    h.device_mut().reset();
    let seen = play(&mut h, 1, &[]);
    assert_eq!(seen, [(64, K::AllOff)]);
    assert!(play(&mut h, 2_000, &[]).is_empty());
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
        prop_assert!(play(&mut h, 4_000, &[]).is_empty());
    }
}
