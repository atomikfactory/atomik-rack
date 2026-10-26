//! Arpeggiator tests (random mode, hanging notes).
#![allow(clippy::float_cmp)]

use super::*;
use atomik_core::{BATCH, LoopWrap, NoteEvent, NoteKind};
use atomik_device_sdk::SeqResolution;
use atomik_testkit::harness::DeviceHarness;
use proptest::prelude::*;

use crate::test_support::{off, on};

const SR: u64 = 48_000;
const BPM: u64 = 120;

#[derive(Debug, Clone, Copy, PartialEq)]
enum K {
    On(u8, u8),
    Off(u8),
    AllOff,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Ev {
    frame: u64,
    tick: f64,
    k: K,
}

fn rig() -> DeviceHarness {
    DeviceHarness::with_options(Box::new(Arpeggiator::new()), SR as f64, BPM as f64, 3)
}

fn batch(h: &mut DeviceHarness) -> Vec<Ev> {
    let base = h.frame();
    let t = *h.time_mut();
    h.process();
    h.notes_out("notes_out")
        .iter()
        .map(|e| Ev {
            frame: base + u64::from(e.offset),
            tick: t.tick + f64::from(e.offset) * t.ticks_per_frame,
            k: match e.kind {
                NoteKind::On { key, velocity } => K::On(key, (velocity * 127.0).round() as u8),
                NoteKind::Off { key } => K::Off(key),
                NoteKind::AllOff => K::AllOff,
                other => panic!("the arpeggiator never emits {other:?}"),
            },
        })
        .collect()
}

fn collect(h: &mut DeviceHarness, batches: usize) -> Vec<Ev> {
    let mut out = Vec::new();
    for _ in 0..batches {
        out.extend(batch(h));
    }
    out
}

fn due(tick: u64) -> u64 {
    let num = u128::from(tick) * 60 * u128::from(SR);
    num.div_ceil(u128::from(BPM) * 15_360) as u64
}

fn set(h: &mut DeviceHarness, key: &str, v: f32) {
    h.set_param(key, v);
}

fn press(h: &mut DeviceHarness, keys: &[u8]) {
    for k in keys {
        h.push_note("notes_in", on(0, *k, 0.8));
    }
}

/// Number of batches that cover `steps` steps of `s` ticks plus a margin.
fn batches_for(steps: u64, s: u64) -> usize {
    (due(steps * s) / BATCH as u64) as usize + 3
}

fn ons(evs: &[Ev]) -> Vec<(u64, u8)> {
    evs.iter()
        .filter_map(|e| {
            if let K::On(k, _) = e.k {
                Some((e.frame, k))
            } else {
                None
            }
        })
        .collect()
}

const MODES: [&str; 6] = ["up", "down", "up-down", "random", "as-played", "chord"];

fn mode_index(name: &str) -> f32 {
    MODES.iter().position(|m| *m == name).unwrap() as f32
}

/// An independent model of one cycle of the non-random, non-chord modes.
fn model_cycle(mode: &str, press_order: &[u8], octaves: u32) -> Vec<u8> {
    let mut sorted = press_order.to_vec();
    sorted.sort_unstable();
    let stack = |base: &[u8]| -> Vec<u8> {
        (0..octaves)
            .flat_map(|o| base.iter().map(move |k| k + 12 * o as u8))
            .collect()
    };
    match mode {
        "up" => stack(&sorted),
        "down" => {
            let mut v = stack(&sorted);
            v.reverse();
            v
        }
        "up-down" => {
            let up = stack(&sorted);
            let mut v = up.clone();
            if up.len() > 2 {
                v.extend(up[1..up.len() - 1].iter().rev());
            }
            v
        }
        "as-played" => stack(press_order),
        _ => unreachable!(),
    }
}

fn played_keys(mode: &str, keys: &[u8], octaves: u32, steps: usize) -> Vec<u8> {
    let mut h = rig();
    set(&mut h, "mode", mode_index(mode));
    set(&mut h, "octaves", octaves as f32);
    press(&mut h, keys);
    let evs = collect(&mut h, batches_for(steps as u64 + 1, 3_840));
    ons(&evs).into_iter().map(|(_, k)| k).take(steps).collect()
}

#[test]
fn descriptor_validates_and_orders_params_and_sockets() {
    assert_eq!(DESCRIPTOR.validate(), Ok(()));
    let keys: Vec<_> = DESCRIPTOR.params.iter().map(|p| p.key).collect();
    assert_eq!(
        keys,
        [
            "mode",
            "rate",
            "octaves",
            "gate",
            "swing",
            "hold",
            "velocity.mode",
            "velocity.fixed",
            "run",
            "random.seed",
            "bypass"
        ]
    );
    let sockets: Vec<_> = DESCRIPTOR.sockets.iter().map(|s| s.key).collect();
    assert_eq!(
        sockets,
        [
            "notes_in",
            "notes_out",
            "gate_cv_out",
            "pitch_cv_out",
            "velocity_cv_out",
            "trig_cv_out"
        ]
    );
    assert_eq!(DESCRIPTOR.type_id, "atomik.arpeggiator");
    assert_eq!(DESCRIPTOR.kind, atomik_device_sdk::DeviceKind::Utility);
}

#[test]
fn note_order_matches_the_model_for_up_down_updown_and_as_played() {
    // Several sets, in press order that differs from the sorted order.
    let sets: [&[u8]; 5] = [
        &[60],
        &[64, 60],
        &[67, 60, 64],
        &[72, 60, 67, 64],
        &[65, 72, 60, 67, 64],
    ];
    for mode in ["up", "down", "up-down", "as-played"] {
        for octaves in 1..=4u32 {
            for keys in sets {
                let cycle = model_cycle(mode, keys, octaves);
                let n = cycle.len() * 2 + 1;
                let want: Vec<u8> = (0..n).map(|i| cycle[i % cycle.len()]).collect();
                let got = played_keys(mode, keys, octaves, n);
                assert_eq!(got, want, "{mode} octaves {octaves} keys {keys:?}");
            }
        }
    }
}

#[test]
fn up_down_does_not_repeat_the_end_points() {
    assert_eq!(
        played_keys("up-down", &[60, 64, 67], 1, 8),
        [60, 64, 67, 64, 60, 64, 67, 64]
    );
    assert_eq!(played_keys("up-down", &[60], 2, 4), [60, 72, 60, 72]);
    assert_eq!(played_keys("up-down", &[60, 64], 1, 4), [60, 64, 60, 64]);
}

#[test]
fn chord_plays_the_whole_set_with_one_octave_per_step_cycling() {
    for octaves in 1..=4u32 {
        let mut h = rig();
        set(&mut h, "mode", mode_index("chord"));
        set(&mut h, "octaves", octaves as f32);
        press(&mut h, &[67, 60, 64]);
        let evs = collect(&mut h, batches_for(6, 3_840));
        let mut steps: Vec<Vec<u8>> = Vec::new();
        let mut last = u64::MAX;
        for (f, k) in ons(&evs) {
            if f != last {
                steps.push(Vec::new());
                last = f;
            }
            steps.last_mut().unwrap().push(k);
        }
        for (i, chord) in steps.iter().take(6).enumerate() {
            let o = 12 * (i as u32 % octaves) as u8;
            assert_eq!(
                chord,
                &[60 + o, 64 + o, 67 + o],
                "step {i} octaves {octaves}"
            );
        }
    }
}

#[test]
fn rate_periods_are_exact_for_all_seven_rates() {
    for (i, res) in SeqResolution::ALL.iter().enumerate() {
        let s = u64::from(res.step_ticks());
        let mut h = rig();
        set(&mut h, "rate", i as f32);
        press(&mut h, &[60]);
        let evs = collect(&mut h, batches_for(8, s));
        let frames: Vec<u64> = ons(&evs).iter().map(|(f, _)| *f).collect();
        for k in 0..8u64 {
            assert_eq!(frames[k as usize], due(k * s), "{res:?} step {k}");
        }
    }
}

#[test]
fn gate_from_five_percent_to_legato_and_swing() {
    let s = 3_840u64;
    for (gate, end) in [(5u64, 192u64), (50, 1_920), (80, 3_072), (100, 3_840)] {
        let mut h = rig();
        set(&mut h, "gate", gate as f32);
        press(&mut h, &[60]);
        let evs = collect(&mut h, batches_for(3, s));
        let offs: Vec<u64> = evs
            .iter()
            .filter(|e| matches!(e.k, K::Off(_)))
            .map(|e| e.frame)
            .collect();
        assert_eq!(offs[0], due(end), "gate {gate}");
        assert_eq!(offs[1], due(s + end), "gate {gate}");
    }
    // Legato: off then on at the same frame.
    let mut h = rig();
    set(&mut h, "gate", 100.0);
    press(&mut h, &[60]);
    let evs = collect(&mut h, batches_for(3, s));
    let at: Vec<K> = evs
        .iter()
        .filter(|e| e.frame == due(s))
        .map(|e| e.k)
        .collect();
    assert_eq!(at, [K::Off(60), K::On(60, 102)]);
    // Swing 50 % delays odd steps by (50 * s + 100) / 200 ticks.
    let mut h = rig();
    set(&mut h, "swing", 50.0);
    press(&mut h, &[60]);
    let evs = collect(&mut h, batches_for(5, s));
    let frames: Vec<u64> = ons(&evs).iter().map(|(f, _)| *f).collect();
    for k in 0..4u64 {
        let shift = if k % 2 == 1 { (50 * s + 100) / 200 } else { 0 };
        assert_eq!(frames[k as usize], due(k * s + shift), "step {k}");
    }
}

#[test]
fn a_key_pressed_mid_step_waits_for_the_next_boundary_and_the_cycle_restarts() {
    let mut h = rig();
    let mut evs = collect(&mut h, 10);
    assert!(evs.is_empty());
    h.push_note("notes_in", on(5, 60, 0.8));
    h.push_note("notes_in", on(6, 64, 0.8));
    evs.extend(collect(&mut h, batches_for(3, 3_840)));
    let o = ons(&evs);
    assert_eq!(
        o[0],
        (due(3_840), 60),
        "first note at the next boundary, from the first key"
    );
    assert_eq!(o[1], (due(7_680), 64));
    // Release everything: the cycle ends; pressing again restarts from the first note.
    h.push_note("notes_in", off(0, 60));
    h.push_note("notes_in", off(1, 64));
    let more = collect(&mut h, batches_for(3, 3_840));
    assert!(ons(&more).is_empty(), "no new notes without keys");
    h.push_note("notes_in", on(0, 64, 0.8));
    h.push_note("notes_in", on(1, 60, 0.8));
    let again = collect(&mut h, batches_for(3, 3_840));
    assert_eq!(ons(&again)[0].1, 60, "restarts from the lowest key (up)");
}

#[test]
fn released_key_stops_the_notes_and_nothing_hangs() {
    let mut h = rig();
    set(&mut h, "gate", 100.0);
    press(&mut h, &[60, 64]);
    let mut evs = collect(&mut h, 120);
    h.push_note("notes_in", off(0, 60));
    h.push_note("notes_in", off(0, 64));
    evs.extend(collect(&mut h, 400));
    let (on_n, off_n) = (
        evs.iter().filter(|e| matches!(e.k, K::On(..))).count(),
        evs.iter().filter(|e| matches!(e.k, K::Off(_))).count(),
    );
    assert_eq!(on_n, off_n);
}

#[test]
fn hold_latches_resets_on_the_next_press_and_releases_when_turned_off() {
    let s = 3_840u64;
    let mut h = rig();
    set(&mut h, "hold", 1.0);
    press(&mut h, &[60, 64]);
    collect(&mut h, 20);
    h.push_note("notes_in", off(0, 60));
    h.push_note("notes_in", off(0, 64));
    let held = collect(&mut h, batches_for(4, s));
    let keys: Vec<u8> = ons(&held).iter().map(|(_, k)| *k).collect();
    assert!(
        keys.len() >= 4 && keys.iter().all(|k| *k == 60 || *k == 64),
        "{keys:?}"
    );
    // The next press after every key is up starts a fresh set.
    h.push_note("notes_in", on(0, 72, 0.8));
    let fresh = collect(&mut h, batches_for(4, s));
    let keys: Vec<u8> = ons(&fresh).iter().map(|(_, k)| *k).collect();
    assert!(keys.iter().all(|k| *k == 72), "{keys:?}");
    // A second key pressed while one is down joins the set.
    h.push_note("notes_in", on(0, 76, 0.8));
    let joined = collect(&mut h, batches_for(4, s));
    let keys: Vec<u8> = ons(&joined).iter().map(|(_, k)| *k).collect();
    assert!(keys.contains(&72) && keys.contains(&76), "{keys:?}");
    // Hold off while the keys are released: the latch goes away.
    h.push_note("notes_in", off(0, 72));
    h.push_note("notes_in", off(0, 76));
    collect(&mut h, 20);
    set(&mut h, "hold", 0.0);
    collect(&mut h, 200);
    let after = collect(&mut h, batches_for(4, s));
    assert!(ons(&after).is_empty(), "latch released");
}

#[test]
fn hold_off_with_a_key_still_down_keeps_only_that_key() {
    let mut h = rig();
    set(&mut h, "hold", 1.0);
    press(&mut h, &[60, 64]);
    collect(&mut h, 20);
    h.push_note("notes_in", off(0, 60)); // latched
    collect(&mut h, 20);
    set(&mut h, "hold", 0.0);
    let evs = collect(&mut h, batches_for(4, 3_840));
    let keys: Vec<u8> = ons(&evs).iter().map(|(_, k)| *k).collect();
    assert!(
        !keys.is_empty() && keys.iter().all(|k| *k == 64),
        "{keys:?}"
    );
}

#[test]
fn all_off_clears_the_set_and_closes_the_notes() {
    let mut h = rig();
    set(&mut h, "gate", 100.0);
    press(&mut h, &[60]);
    collect(&mut h, 20);
    h.push_note(
        "notes_in",
        NoteEvent {
            offset: 7,
            kind: NoteKind::AllOff,
        },
    );
    let base = h.frame();
    let evs = batch(&mut h);
    assert_eq!(
        evs.iter().map(|e| (e.frame, e.k)).collect::<Vec<_>>(),
        [(base + 7, K::Off(60))]
    );
    assert!(ons(&collect(&mut h, 400)).is_empty());
}

#[test]
fn run_transport_is_silent_while_stopped_and_always_uses_the_internal_clock() {
    let s = 3_840u64;
    // run = transport, stopped: silent, and notes in the batch where it stops are closed.
    let mut h = rig();
    set(&mut h, "run", 0.0);
    set(&mut h, "gate", 100.0);
    press(&mut h, &[60]);
    collect(&mut h, 10);
    h.time_mut().playing = false;
    let evs = batch(&mut h);
    assert_eq!(evs.iter().map(|e| e.k).collect::<Vec<_>>(), [K::Off(60)]);
    assert!(collect(&mut h, 300).is_empty(), "silent while stopped");

    // run = always (default), stopped at tick 1000: the internal clock runs from there.
    let mut h = rig();
    h.time_mut().playing = false;
    h.time_mut().tick = 1_000.0;
    press(&mut h, &[60, 64]);
    let tpf = h.time_mut().ticks_per_frame;
    let evs = collect(&mut h, batches_for(4, s));
    let o = ons(&evs);
    assert_eq!(o[0].1, 60);
    let first = (((s as f64) - 1_000.0) / tpf - 1e-6).ceil() as u64;
    assert_eq!(o[0].0, first, "first step boundary after the playhead tick");
    let second = (((2 * s - 1_000) as f64) / tpf - 1e-6).ceil() as u64;
    assert_eq!(o[1].0, second, "the grid keeps the tick spacing");
}

#[test]
fn starting_the_transport_while_running_on_the_internal_clock_leaves_no_hanging_note() {
    let mut h = rig();
    h.time_mut().playing = false;
    set(&mut h, "gate", 100.0);
    press(&mut h, &[60]);
    let mut evs = collect(&mut h, 200);
    h.time_mut().playing = true;
    h.time_mut().tick = 20_000.0;
    evs.extend(collect(&mut h, 200));
    h.time_mut().playing = false;
    h.push_note("notes_in", off(0, 60));
    evs.extend(collect(&mut h, 300));
    let mut count = 0i32;
    for e in &evs {
        match e.k {
            K::On(..) => count += 1,
            K::Off(_) => count -= 1,
            K::AllOff => count = 0,
        }
        assert!((0..=1).contains(&count));
    }
    assert_eq!(count, 0);
}

fn random_picks(seed: f32, start_step: Option<u64>, steps: u64) -> Vec<(u64, u8)> {
    let s = 3_840u64;
    let mut h = rig();
    set(&mut h, "mode", mode_index("random"));
    set(&mut h, "octaves", 2.0);
    set(&mut h, "random.seed", seed);
    if let Some(k) = start_step {
        h.time_mut().tick = (k * s) as f64;
    }
    press(&mut h, &[60, 62, 64, 65]);
    let mut out = Vec::new();
    for _ in 0..(steps * s * 60 * SR / (BPM * 15_360) / BATCH as u64 + 3) {
        for e in batch(&mut h) {
            if let K::On(k, _) = e.k {
                out.push((((e.tick + 0.01) / s as f64).floor() as u64, k));
            }
        }
    }
    out
}

#[test]
fn random_is_reproducible_uniform_and_independent_of_the_start_position() {
    let a = random_picks(0.0, None, 400);
    let b = random_picks(0.0, None, 400);
    assert_eq!(a, b, "two runs agree");
    let c = random_picks(0.0, Some(100), 300);
    let a_tail: Vec<_> = a.iter().copied().filter(|(k, _)| *k >= 100).collect();
    let n = c.len().min(a_tail.len());
    assert!(n > 100);
    assert_eq!(
        &c[..n],
        &a_tail[..n],
        "a locate gives the same picks at the same steps"
    );
    let d = random_picks(1.0, None, 400);
    assert_ne!(a, d, "another seed re-rolls");
    // Uniform over the pool of 4 keys x 2 octaves = 8 notes.
    let big = random_picks(0.0, None, 8_000);
    let mut counts = std::collections::BTreeMap::new();
    for (_, k) in &big {
        *counts.entry(*k).or_insert(0usize) += 1;
    }
    assert_eq!(counts.len(), 8, "{counts:?}");
    let mean = big.len() as f64 / 8.0;
    for (k, c) in counts {
        assert!(
            ((c as f64) - mean).abs() < mean * 0.1,
            "key {k}: {c} vs {mean}"
        );
    }
}

#[test]
fn velocity_follows_the_input_or_a_fixed_value() {
    let mut h = rig();
    h.push_note("notes_in", on(0, 60, 0.5));
    h.push_note("notes_in", on(1, 64, 1.0));
    let evs = collect(&mut h, batches_for(3, 3_840));
    let v: Vec<K> = evs
        .iter()
        .map(|e| e.k)
        .filter(|k| matches!(k, K::On(..)))
        .collect();
    assert_eq!(v[0], K::On(60, 64));
    assert_eq!(v[1], K::On(64, 127));
    let mut h = rig();
    set(&mut h, "velocity.mode", 1.0);
    set(&mut h, "velocity.fixed", 90.0);
    h.push_note("notes_in", on(0, 60, 0.5));
    let evs = collect(&mut h, 20);
    assert_eq!(evs[0].k, K::On(60, 90));
}

#[test]
fn cv_outputs_follow_the_arpeggio() {
    let mut h = rig();
    set(&mut h, "gate", 50.0);
    press(&mut h, &[72]);
    let n = batches_for(2, 3_840) * BATCH;
    let (mut gate, mut pitch, mut vel, mut trig) = (vec![], vec![], vec![], vec![]);
    for _ in 0..n / BATCH {
        h.process();
        gate.extend_from_slice(h.output("gate_cv_out"));
        pitch.extend_from_slice(h.output("pitch_cv_out"));
        vel.extend_from_slice(h.output("velocity_cv_out"));
        trig.extend_from_slice(h.output("trig_cv_out"));
    }
    let off0 = due(1_920) as usize;
    let on1 = due(3_840) as usize;
    assert!(gate[..off0].iter().all(|v| *v == 1.0));
    assert!(gate[off0..on1].iter().all(|v| *v == 0.0));
    assert_eq!(gate[on1], 1.0);
    assert!((pitch[100] - 0.5).abs() < 1e-6);
    assert!((vel[100] - 0.8).abs() < 0.01);
    assert!(trig[..48].iter().all(|v| *v == 1.0));
    assert_eq!(trig[48], 0.0);
    assert!(trig[on1..on1 + 48].iter().all(|v| *v == 1.0));
}

#[test]
fn meter_values_show_the_held_count_and_the_next_keys() {
    let mut h = rig();
    assert!(h.device().has_meter_values());
    press(&mut h, &[64, 60]);
    h.process();
    let mut out = [0.0f32; 16];
    let n = h.device().meter_values(&mut out);
    assert!(n >= 9);
    assert_eq!(out[0], 2.0, "held keys");
    // Step 0 just played key 60; the next steps are 64, 60, 64, ...
    assert_eq!(
        &out[1..9],
        &[64.0, 60.0, 64.0, 60.0, 64.0, 60.0, 64.0, 60.0]
    );
    let mut idle = rig();
    idle.process();
    idle.device().meter_values(&mut out);
    assert_eq!(&out[..9], &[0.0; 9]);
}

#[test]
fn reset_closes_the_sounding_note_with_an_all_off() {
    let mut h = rig();
    set(&mut h, "gate", 100.0);
    press(&mut h, &[60]);
    collect(&mut h, 5);
    h.device_mut().reset();
    let base = h.frame();
    let evs = batch(&mut h);
    assert_eq!(evs[0].k, K::AllOff);
    assert_eq!(evs[0].frame, base);
}

#[test]
fn loop_wrap_and_locate_close_notes() {
    let mut h = rig();
    set(&mut h, "gate", 100.0);
    press(&mut h, &[60]);
    collect(&mut h, 10);
    h.time_mut().loop_wrap = Some(LoopWrap {
        offset: 30,
        to_tick: 0.0,
    });
    let base = h.frame();
    let evs = batch(&mut h);
    assert_eq!(
        evs.iter().map(|e| (e.frame, e.k)).collect::<Vec<_>>(),
        [(base + 30, K::Off(60)), (base + 30, K::On(60, 102))]
    );
}

#[derive(Debug, Clone)]
enum Act {
    Run,
    Stop,
    Play,
    Locate(u32),
    Wrap(u8, u32),
    Press(u8),
    Release(u8),
    AllOff,
    Hold(bool),
    Mode(u8),
    Rate(u8),
    Tempo(u16),
    Gate(u8),
    RunMode(bool),
}

fn act() -> impl Strategy<Value = Act> {
    prop_oneof![
        6 => Just(Act::Run),
        1 => Just(Act::Stop),
        1 => Just(Act::Play),
        2 => (0u32..400_000).prop_map(Act::Locate),
        2 => (1u8..64, 0u32..200_000).prop_map(|(o, t)| Act::Wrap(o, t)),
        4 => (48u8..80).prop_map(Act::Press),
        4 => (48u8..80).prop_map(Act::Release),
        1 => Just(Act::AllOff),
        1 => any::<bool>().prop_map(Act::Hold),
        1 => (0u8..6).prop_map(Act::Mode),
        1 => (0u8..7).prop_map(Act::Rate),
        1 => (60u16..300).prop_map(Act::Tempo),
        1 => (5u8..=100).prop_map(Act::Gate),
        1 => any::<bool>().prop_map(Act::RunMode),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn no_note_hangs_or_doubles_across_random_scripts(
        acts in proptest::collection::vec(act(), 5..160),
    ) {
        let mut h = rig();
        let mut count = [0i32; 128];
        for a in acts {
            match a {
                Act::Run => {}
                Act::Stop => h.time_mut().playing = false,
                Act::Play => h.time_mut().playing = true,
                Act::Locate(t) => h.time_mut().tick = f64::from(t),
                Act::Wrap(o, t) => h.time_mut().loop_wrap = Some(LoopWrap { offset: u16::from(o), to_tick: f64::from(t) }),
                Act::Press(k) => h.push_note("notes_in", on(0, k, 0.7)),
                Act::Release(k) => h.push_note("notes_in", off(0, k)),
                Act::AllOff => h.push_note("notes_in", NoteEvent { offset: 0, kind: NoteKind::AllOff }),
                Act::Hold(v) => set(&mut h, "hold", if v { 1.0 } else { 0.0 }),
                Act::Mode(m) => set(&mut h, "mode", f32::from(m)),
                Act::Rate(r) => set(&mut h, "rate", f32::from(r)),
                Act::Tempo(bpm) => h.time_mut().ticks_per_frame = f64::from(bpm) * 15_360.0 / (60.0 * 48_000.0),
                Act::Gate(g) => set(&mut h, "gate", f32::from(g)),
                Act::RunMode(a) => set(&mut h, "run", if a { 1.0 } else { 0.0 }),
            }
            let evs = batch(&mut h);
            for e in &evs {
                match e.k {
                    K::On(k, _) => {
                        count[usize::from(k)] += 1;
                        prop_assert!(count[usize::from(k)] <= 1, "double on {k} at {e:?}");
                    }
                    K::Off(k) => {
                        count[usize::from(k)] -= 1;
                        prop_assert!(count[usize::from(k)] >= 0, "off without on {k}");
                    }
                    K::AllOff => count = [0; 128],
                }
            }
            prop_assert!(evs.windows(2).all(|w| w[0].frame <= w[1].frame));
        }
        // Release everything and stop: nothing may hang.
        h.push_note("notes_in", NoteEvent { offset: 0, kind: NoteKind::AllOff });
        h.time_mut().playing = false;
        set(&mut h, "run", 0.0);
        let evs = batch(&mut h);
        for e in &evs {
            match e.k {
                K::On(k, _) => count[usize::from(k)] += 1,
                K::Off(k) => count[usize::from(k)] -= 1,
                K::AllOff => count = [0; 128],
            }
        }
        prop_assert!(count.iter().all(|c| *c == 0), "hanging: {count:?}");
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn outputs_are_finite_and_in_range_for_any_params(
        values in proptest::collection::vec(-1.0e6f32..1.0e6, 10),
        tick in 0.0f64..1.0e7,
        keys in proptest::collection::vec(0u8..128, 0..12),
    ) {
        let mut h = rig();
        for (i, v) in values.iter().enumerate() {
            h.set_param(DESCRIPTOR.params[i].key, *v);
        }
        h.time_mut().tick = tick;
        press(&mut h, &keys);
        for _ in 0..40 {
            for e in batch(&mut h) {
                if let K::On(k, v) = e.k {
                    prop_assert!(k < 128 && (1..=127).contains(&v));
                }
            }
            for key in ["gate_cv_out", "pitch_cv_out", "velocity_cv_out", "trig_cv_out"] {
                prop_assert!(h.output(key).iter().all(|s| s.is_finite() && (-1.0..=1.0).contains(s)));
            }
        }
    }
}

/// Regression (found by the QA mutation runs): a rate change while a long note still sounds must
/// close that note before the same key is played again, never send a second On.
#[test]
fn a_faster_rate_never_doubles_a_still_sounding_key() {
    let mut h = rig();
    set(&mut h, "gate", 100.0);
    set(&mut h, "rate", 0.0); // 1/4: the legato note lasts 15 360 ticks
    press(&mut h, &[48]);
    let mut evs = collect(&mut h, 3);
    assert_eq!(ons(&evs).len(), 1, "one note sounds");
    set(&mut h, "rate", 6.0); // 1/32T: the next step starts 1 280 ticks later
    evs.extend(collect(&mut h, 400));
    let mut open = 0i32;
    for e in &evs {
        match e.k {
            K::On(..) => {
                open += 1;
                assert!(open <= 1, "double On at {e:?}");
            }
            K::Off(_) => open -= 1,
            K::AllOff => open = 0,
        }
        assert!(open >= 0, "Off without On at {e:?}");
    }
    assert!(ons(&evs).len() > 10, "the fast rate keeps playing");
}

/// A jump closes the sounding note at the jump frame even when no step starts there.
#[test]
fn a_locate_closes_a_long_note_at_once() {
    let mut h = rig();
    set(&mut h, "gate", 100.0);
    set(&mut h, "rate", 0.0); // 1/4: a legato note lasts 15 360 ticks
    press(&mut h, &[48]);
    let evs = collect(&mut h, 1);
    assert_eq!(ons(&evs), [(0, 48)]);
    h.time_mut().tick = 1_000.0; // forward inside the step, long before its end
    let base = h.frame();
    let evs = batch(&mut h);
    assert_eq!(
        evs.iter().map(|e| (e.frame, e.k)).collect::<Vec<_>>(),
        [(base, K::Off(48))]
    );
}

/// Applies note events to a set of sounding keys, failing on a double On or an Off of a silent key.
fn track(sounding: &mut std::collections::BTreeSet<u8>, evs: &[Ev]) {
    for e in evs {
        match e.k {
            K::On(k, _) => assert!(sounding.insert(k), "double On of {k} at {}", e.frame),
            K::Off(k) => assert!(sounding.remove(&k), "Off of silent {k} at {}", e.frame),
            K::AllOff => sounding.clear(),
        }
    }
}

#[test]
fn arpeggiator_bypass_passes_notes_through_and_leaves_nothing_hanging() {
    let mut h = rig();
    let mut sounding = std::collections::BTreeSet::new();
    set(&mut h, "gate", 100.0);
    press(&mut h, &[60]);
    let evs = collect(&mut h, 20);
    track(&mut sounding, &evs);
    assert_eq!(sounding.iter().copied().collect::<Vec<_>>(), [60]);

    // Bypass while key 60 is held: the arpeggiated note closes at the start of the batch, the
    // held key is not retriggered and a new key passes straight through.
    set(&mut h, "bypass", 1.0);
    h.push_note("notes_in", on(5, 64, 0.8));
    let base = h.frame();
    let evs = batch(&mut h);
    assert_eq!(
        evs.iter().map(|e| (e.frame, e.k)).collect::<Vec<_>>(),
        [(base, K::Off(60)), (base + 5, K::On(64, 102))]
    );
    track(&mut sounding, &evs);
    assert!(
        collect(&mut h, 400).is_empty(),
        "no arpeggio while bypassed"
    );

    // Offs: the held key's is swallowed, the thru key's passes.
    h.push_note("notes_in", off(2, 60));
    h.push_note("notes_in", off(9, 64));
    let base = h.frame();
    let evs = batch(&mut h);
    assert_eq!(
        evs.iter().map(|e| (e.frame, e.k)).collect::<Vec<_>>(),
        [(base + 9, K::Off(64))]
    );
    track(&mut sounding, &evs);
    assert!(sounding.is_empty());

    // A key held on the thru when the bypass ends is closed, and the arpeggiator starts clean.
    h.push_note("notes_in", on(0, 67, 0.8));
    track(&mut sounding, &batch(&mut h));
    set(&mut h, "bypass", 0.0);
    let base = h.frame();
    let evs = batch(&mut h);
    assert_eq!(
        evs.iter().map(|e| (e.frame, e.k)).collect::<Vec<_>>(),
        [(base, K::Off(67))]
    );
    track(&mut sounding, &evs);
    h.push_note("notes_in", off(0, 67));
    assert!(
        collect(&mut h, 400).is_empty(),
        "the old key does not arpeggiate"
    );
    press(&mut h, &[72]);
    let evs = collect(&mut h, 400);
    track(&mut sounding, &evs);
    assert!(ons(&evs).iter().all(|(_, k)| *k == 72) && !ons(&evs).is_empty());
}
