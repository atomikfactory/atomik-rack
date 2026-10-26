//! Step sequencer tests.
#![allow(clippy::float_cmp)]

use super::*;
use atomik_core::{BATCH, LoopWrap, NoteEvent, NoteKind};
use atomik_device_sdk::{
    PatternData, PlayerData, PlayerDataRef, SeqResolution, StepData, StepSeqData,
};
use atomik_testkit::harness::DeviceHarness;
use proptest::prelude::*;
use std::sync::Arc;

use crate::test_support::{off, on};

/// A note event reduced to what the tests compare.
#[derive(Debug, Clone, Copy, PartialEq)]
enum K {
    On(u8, u8),
    Off(u8),
    AllOff,
}

/// An event with its absolute frame and its tick position.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Ev {
    frame: u64,
    tick: f64,
    k: K,
}

fn kind(e: &NoteEvent) -> K {
    match e.kind {
        NoteKind::On { key, velocity } => K::On(key, (velocity * 127.0).round() as u8),
        NoteKind::Off { key } => K::Off(key),
        NoteKind::AllOff => K::AllOff,
        other => panic!("the sequencer never emits {other:?}"),
    }
}

fn st(key: u8, vel: u8, gate: u8) -> StepData {
    StepData {
        on: true,
        key,
        velocity: vel,
        gate,
        prob: 100,
        tie: false,
    }
}

fn pat(res: SeqResolution, len: u8, swing: u8, steps: &[StepData]) -> PatternData {
    let mut p = PatternData {
        length: len,
        resolution: res,
        swing,
        ..PatternData::default()
    };
    for (i, s) in steps.iter().enumerate() {
        p.steps[i] = *s;
    }
    p
}

/// A pattern whose first `len` steps are all on with the given key/gate.
fn full(res: SeqResolution, len: u8, swing: u8, key: u8, gate: u8) -> PatternData {
    let mut p = pat(res, len, swing, &[]);
    for i in 0..usize::from(len) {
        p.steps[i] = st(key, 100, gate);
    }
    p
}

fn data(patterns: &[PatternData]) -> PlayerDataRef {
    let mut d = StepSeqData {
        count: patterns.len() as u8,
        ..StepSeqData::default()
    };
    for (i, p) in patterns.iter().enumerate() {
        d.patterns[i] = *p;
    }
    Arc::new(PlayerData::StepSeq(d))
}

fn rig(sr: f64, bpm: f64, patterns: &[PatternData]) -> DeviceHarness {
    let mut h = DeviceHarness::with_options(Box::new(StepSequencer::new()), sr, bpm, 7);
    let back = h.device_mut().swap_player_data(Some(data(patterns)));
    assert!(back.is_none());
    h
}

/// Processes one batch and returns its notes with absolute frames.
fn batch(h: &mut DeviceHarness) -> Vec<Ev> {
    let base = h.frame();
    let t = *h.time_mut();
    h.process();
    h.notes_out("notes_out")
        .iter()
        .map(|e| Ev {
            frame: base + u64::from(e.offset),
            tick: t.tick + f64::from(e.offset) * t.ticks_per_frame,
            k: kind(e),
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

fn plain(evs: &[Ev]) -> Vec<(u64, K)> {
    evs.iter().map(|e| (e.frame, e.k)).collect()
}

/// First frame at or after the integer tick `tick` at `bpm`/`sr` (exact integer arithmetic).
fn due(tick: u64, sr: u64, bpm: u64) -> u64 {
    let num = u128::from(tick) * 60 * u128::from(sr);
    let den = u128::from(bpm) * 15_360;
    num.div_ceil(den) as u64
}

fn set(h: &mut DeviceHarness, key: &str, v: f32) {
    h.set_param(key, v);
}

fn batches_for(ticks: u64, sr: u64, bpm: u64) -> usize {
    (due(ticks, sr, bpm) / BATCH as u64) as usize + 3
}

fn swing_shift(s: u64, swing: u64) -> u64 {
    (swing * s + 100) / 200
}

#[test]
fn descriptor_validates_and_orders_params_and_sockets() {
    assert_eq!(DESCRIPTOR.validate(), Ok(()));
    let keys: Vec<_> = DESCRIPTOR.params.iter().map(|p| p.key).collect();
    assert_eq!(
        keys,
        [
            "pattern",
            "pattern.switch",
            "transpose",
            "follow",
            "probability.amount",
            "random.seed",
            "mute",
            "velocity.scale",
            "gate.scale",
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
    assert_eq!(DESCRIPTOR.kind, atomik_device_sdk::DeviceKind::Utility);
}

#[test]
fn step_on_and_off_land_on_the_integer_tick_rule_for_every_rate_and_tempo() {
    for sr in [44_100u64, 48_000, 96_000] {
        for bpm in [60u64, 133, 300] {
            for res in SeqResolution::ALL {
                let s = u64::from(res.step_ticks());
                let mut h = rig(sr as f64, bpm as f64, &[full(res, 4, 0, 60, 50)]);
                let evs = collect(&mut h, batches_for(7 * s, sr, bpm));
                let mut want = Vec::new();
                for k in 0..6u64 {
                    want.push((due(k * s, sr, bpm), K::On(60, 100)));
                    want.push((due(k * s + s * 50 / 100, sr, bpm), K::Off(60)));
                }
                want.sort_by_key(|(f, _)| *f);
                let limit = due(6 * s, sr, bpm);
                let got: Vec<_> = plain(&evs)
                    .into_iter()
                    .filter(|(f, _)| *f < limit)
                    .collect();
                assert_eq!(got, want, "sr {sr} bpm {bpm} {res:?}");
            }
        }
    }
}

#[test]
fn swing_delays_odd_steps_by_the_integer_rule() {
    let (sr, bpm) = (48_000u64, 120u64);
    // 3, 13 and 74 give shifts whose exact value is not a whole number of ticks, so the half-up
    // rounding of the rule is part of what is checked.
    for swing in [0u64, 3, 13, 25, 50, 74, 75] {
        for res in [SeqResolution::Sixteenth, SeqResolution::EighthTriplet] {
            let s = u64::from(res.step_ticks());
            let mut h = rig(sr as f64, bpm as f64, &[full(res, 4, swing as u8, 60, 25)]);
            let evs = collect(&mut h, batches_for(7 * s, sr, bpm));
            let ons: Vec<u64> = evs
                .iter()
                .filter(|e| matches!(e.k, K::On(..)))
                .map(|e| e.frame)
                .collect();
            for k in 0..6u64 {
                let shift = if k % 2 == 1 { swing_shift(s, swing) } else { 0 };
                assert_eq!(
                    ons[k as usize],
                    due(k * s + shift, sr, bpm),
                    "k {k} {swing}"
                );
            }
        }
    }
    // Spot value of the integer rule: 50 % of a 1/16 is 960 ticks.
    assert_eq!(swing_shift(3_840, 50), 960);
    // Rounds half up: 3 % of a 1/16 is 57.6 ticks.
    assert_eq!(swing_shift(3_840, 3), 58);
}

#[test]
fn gate_lengths_one_percent_fifty_and_legato() {
    let (sr, bpm, s) = (48_000u64, 120u64, 3_840u64);
    for (gate, end) in [(1u64, 38u64), (50, 1_920), (100, 3_840)] {
        let mut h = rig(
            sr as f64,
            bpm as f64,
            &[full(SeqResolution::Sixteenth, 4, 0, 60, gate as u8)],
        );
        let evs = collect(&mut h, batches_for(3 * s, sr, bpm));
        let offs: Vec<u64> = evs
            .iter()
            .filter(|e| matches!(e.k, K::Off(_)))
            .map(|e| e.frame)
            .collect();
        assert_eq!(offs[0], due(end, sr, bpm), "gate {gate}");
        assert_eq!(offs[1], due(s + end, sr, bpm), "gate {gate}");
    }
    // At 100 % the off comes before the next on at the same frame (no overlap of one key).
    let mut h = rig(
        48_000.0,
        120.0,
        &[full(SeqResolution::Sixteenth, 4, 0, 60, 100)],
    );
    let evs = collect(&mut h, 200);
    let at: Vec<K> = evs
        .iter()
        .filter(|e| e.frame == due(3_840, sr, bpm))
        .map(|e| e.k)
        .collect();
    assert_eq!(at, [K::Off(60), K::On(60, 100)]);
}

#[test]
fn gate_never_runs_into_the_next_swung_step() {
    // An odd step with swing 75 and gate 100 % ends at the next step, not after it.
    let (sr, bpm, s) = (48_000u64, 120u64, 3_840u64);
    let mut h = rig(
        sr as f64,
        bpm as f64,
        &[full(SeqResolution::Sixteenth, 4, 75, 60, 100)],
    );
    let evs = collect(&mut h, batches_for(5 * s, sr, bpm));
    let mut balance = 0i32;
    for e in &evs {
        match e.k {
            K::On(..) => balance += 1,
            K::Off(_) => balance -= 1,
            K::AllOff => balance = 0,
        }
        assert!((0..=1).contains(&balance), "overlap at {e:?}");
    }
}

#[test]
fn tie_keeps_the_note_and_a_different_key_goes_legato() {
    let (sr, bpm, s) = (48_000u64, 120u64, 3_840u64);
    let mut tied = st(60, 100, 80);
    tied.tie = true;
    // Same key: one on, no off at the second step, trig fires at both.
    let p = pat(SeqResolution::Sixteenth, 4, 0, &[tied, st(60, 100, 50)]);
    let mut h = rig(sr as f64, bpm as f64, &[p]);
    let mut trig = Vec::new();
    let mut evs = Vec::new();
    for _ in 0..batches_for(2 * s, sr, bpm) {
        let base = h.frame();
        evs.extend(batch(&mut h));
        for (i, v) in h.output("trig_cv_out").iter().enumerate() {
            if *v > 0.5 {
                trig.push(base + i as u64);
            }
        }
    }
    let first: Vec<_> = plain(&evs).into_iter().take(2).collect();
    assert_eq!(first[0], (0, K::On(60, 100)));
    assert_eq!(
        first[1],
        (due(s + s / 2, sr, bpm), K::Off(60)),
        "no off between"
    );
    assert!(
        trig.contains(&0) && trig.contains(&due(s, sr, bpm)),
        "{trig:?}"
    );

    // Different key: the new on comes before the old off, at the same frame.
    let p = pat(SeqResolution::Sixteenth, 4, 0, &[tied, st(64, 100, 50)]);
    let mut h = rig(sr as f64, bpm as f64, &[p]);
    let evs = collect(&mut h, batches_for(2 * s, sr, bpm));
    let f1 = due(s, sr, bpm);
    let at: Vec<K> = evs.iter().filter(|e| e.frame == f1).map(|e| e.k).collect();
    assert_eq!(at, [K::On(64, 100), K::Off(60)]);
}

#[test]
fn tie_into_an_unplayed_step_ends_at_that_step() {
    let (sr, bpm, s) = (48_000u64, 120u64, 3_840u64);
    let mut tied = st(60, 100, 80);
    tied.tie = true;
    let p = pat(SeqResolution::Sixteenth, 4, 0, &[tied]);
    let mut h = rig(sr as f64, bpm as f64, &[p]);
    let evs = collect(&mut h, batches_for(3 * s, sr, bpm));
    assert_eq!(
        plain(&evs),
        [(0, K::On(60, 100)), (due(s, sr, bpm), K::Off(60))]
    );
}

#[test]
fn cv_outputs_follow_the_notes() {
    let (sr, bpm, s) = (48_000u64, 120u64, 3_840u64);
    let p = pat(
        SeqResolution::Sixteenth,
        4,
        0,
        &[st(72, 127, 50), st(48, 64, 50)],
    );
    let mut h = rig(sr as f64, bpm as f64, &[p]);
    let n = batches_for(2 * s, sr, bpm) * BATCH;
    let (mut gate, mut pitch, mut vel, mut trig) = (vec![], vec![], vec![], vec![]);
    for _ in 0..n / BATCH {
        h.process();
        gate.extend_from_slice(h.output("gate_cv_out"));
        pitch.extend_from_slice(h.output("pitch_cv_out"));
        vel.extend_from_slice(h.output("velocity_cv_out"));
        trig.extend_from_slice(h.output("trig_cv_out"));
    }
    let on1 = due(s, sr, bpm) as usize;
    let off0 = due(s / 2, sr, bpm) as usize;
    assert!(gate[..off0].iter().all(|v| *v == 1.0));
    assert!(gate[off0..on1].iter().all(|v| *v == 0.0));
    assert_eq!(gate[on1], 1.0);
    assert!((pitch[10] - 0.5).abs() < 1e-6, "(72-60)/24");
    assert!((pitch[on1 + 10] + 0.5).abs() < 1e-6, "(48-60)/24");
    assert!((vel[10] - 1.0).abs() < 1e-6);
    assert!((vel[on1 + 10] - 64.0 / 127.0).abs() < 1e-6);
    // Pitch and velocity hold after the note ends.
    assert!((pitch[off0 + 5] - 0.5).abs() < 1e-6);
    assert!((vel[off0 + 5] - 1.0).abs() < 1e-6);
    // Trig: a 1 ms (48 sample) pulse at every played step start.
    assert!(trig[..48].iter().all(|v| *v == 1.0));
    assert_eq!(trig[48], 0.0);
    assert!(trig[on1..on1 + 48].iter().all(|v| *v == 1.0));
    assert_eq!(trig[on1 + 48], 0.0);
}

#[test]
fn pitch_cv_is_clamped_to_plus_minus_one() {
    let p = pat(
        SeqResolution::Sixteenth,
        2,
        0,
        &[st(127, 100, 50), st(0, 100, 50)],
    );
    let mut h = rig(48_000.0, 120.0, &[p]);
    h.process();
    assert_eq!(h.output("pitch_cv_out")[5], 1.0);
    let mut h = rig(
        48_000.0,
        120.0,
        &[pat(SeqResolution::Sixteenth, 2, 0, &[st(0, 100, 50)])],
    );
    h.process();
    assert_eq!(h.output("pitch_cv_out")[5], -1.0);
}

#[test]
fn length_changes_rewrap_on_the_global_step_number() {
    let (sr, bpm, s) = (48_000u64, 120u64, 3_840u64);
    let steps: Vec<StepData> = (0..8).map(|i| st(60 + i, 100, 50)).collect();
    let mut h = rig(
        sr as f64,
        bpm as f64,
        &[pat(SeqResolution::Sixteenth, 3, 0, &steps)],
    );
    let evs = collect(&mut h, batches_for(8 * s, sr, bpm));
    let keys: Vec<u8> = evs
        .iter()
        .filter_map(|e| if let K::On(k, _) = e.k { Some(k) } else { None })
        .take(8)
        .collect();
    assert_eq!(keys, [60, 61, 62, 60, 61, 62, 60, 61]);

    // Growing the length mid-run (data swap): the step index is the global step mod length.
    let mut h = rig(
        sr as f64,
        bpm as f64,
        &[pat(SeqResolution::Sixteenth, 3, 0, &steps)],
    );
    let mut keys = Vec::new();
    let swap_at = due(3 * s + 100, sr, bpm) as usize / BATCH + 1;
    for b in 0..batches_for(10 * s, sr, bpm) {
        if b == swap_at {
            let new = data(&[pat(SeqResolution::Sixteenth, 8, 0, &steps)]);
            drop(h.device_mut().swap_player_data(Some(new)));
        }
        for e in batch(&mut h) {
            if let K::On(k, _) = e.k {
                keys.push(((e.tick + 0.01) / s as f64).floor() as u64 % 8 + 60 == u64::from(k));
            }
        }
    }
    // After the swap every on follows `60 + step mod 8` (the first three steps follow mod 3).
    assert!(keys.iter().skip(4).all(|ok| *ok), "{keys:?}");
}

#[test]
fn pattern_switch_now_and_at_end() {
    let (sr, bpm, s) = (48_000u64, 120u64, 3_840u64);
    let a = full(SeqResolution::Sixteenth, 4, 0, 60, 50);
    let b = full(SeqResolution::Sixteenth, 4, 0, 72, 50);
    for (mode, want) in [
        (0.0f32, [60u8, 60, 72, 72, 72, 72]),
        (1.0, [60, 60, 60, 60, 72, 72]),
    ] {
        let mut h = rig(sr as f64, bpm as f64, &[a, b]);
        set(&mut h, "pattern.switch", mode);
        let mut keys = Vec::new();
        let switch_at = due(s + 100, sr, bpm) as usize / BATCH + 1;
        for bt in 0..batches_for(6 * s, sr, bpm) {
            if bt == switch_at {
                set(&mut h, "pattern", 2.0);
            }
            for e in batch(&mut h) {
                if let K::On(k, _) = e.k {
                    keys.push(k);
                }
            }
        }
        assert_eq!(&keys[..6], &want, "switch mode {mode}");
    }
}

#[test]
fn pattern_with_another_resolution_regrids_the_clock() {
    let a = full(SeqResolution::Quarter, 2, 0, 60, 50);
    let b = full(SeqResolution::Sixteenth, 4, 0, 72, 50);
    let mut h = rig(48_000.0, 120.0, &[a, b]);
    set(&mut h, "pattern", 2.0);
    let evs = collect(&mut h, 400);
    // The queued pattern is adopted at the start: 16ths, 3840 ticks = 6000 frames.
    let ons: Vec<u64> = evs
        .iter()
        .filter(|e| matches!(e.k, K::On(..)))
        .map(|e| e.frame)
        .collect();
    assert_eq!(&ons[..3], &[0, 6_000, 12_000]);
}

#[test]
fn transpose_follow_velocity_and_gate_scale() {
    let (sr, bpm, s) = (48_000u64, 120u64, 3_840u64);
    let p = pat(SeqResolution::Sixteenth, 4, 0, &[st(60, 100, 40)]);
    let run = |h: &mut DeviceHarness| collect(h, batches_for(s, sr, bpm));
    let mut h = rig(sr as f64, bpm as f64, &[p]);
    set(&mut h, "transpose", 7.0);
    set(&mut h, "velocity.scale", 0.5);
    set(&mut h, "gate.scale", 0.5);
    let evs = run(&mut h);
    assert_eq!(
        plain(&evs),
        [(0, K::On(67, 50)), (due(s * 20 / 100, sr, bpm), K::Off(67))]
    );
    // Velocity scale clamps at 127, and key transposition at 127.
    let mut h = rig(
        sr as f64,
        bpm as f64,
        &[pat(SeqResolution::Sixteenth, 4, 0, &[st(120, 100, 40)])],
    );
    set(&mut h, "velocity.scale", 2.0);
    set(&mut h, "transpose", 24.0);
    assert_eq!(plain(&run(&mut h))[0].1, K::On(127, 127));
    // Follow: a held key 64 transposes by +4 (most recent key wins), none held = 0.
    let mut h = rig(
        sr as f64,
        bpm as f64,
        &[full(SeqResolution::Sixteenth, 4, 0, 60, 40)],
    );
    set(&mut h, "follow", 1.0);
    h.push_note("notes_in", on(0, 64, 0.8));
    let ons: Vec<u8> = collect(&mut h, batches_for(s, sr, bpm))
        .iter()
        .filter_map(|e| if let K::On(k, _) = e.k { Some(k) } else { None })
        .collect();
    assert_eq!(ons[0], 64, "held key 64 transposes by +4");
}

#[test]
fn follow_uses_the_most_recent_held_key_and_zero_when_none() {
    let (sr, bpm, s) = (48_000u64, 120u64, 3_840u64);
    let mut h = rig(
        sr as f64,
        bpm as f64,
        &[full(SeqResolution::Sixteenth, 8, 0, 60, 40)],
    );
    set(&mut h, "follow", 1.0);
    let f = due(s, sr, bpm) as usize / BATCH; // the batch holding step 1
    let mut keys = Vec::new();
    for b in 0..batches_for(4 * s, sr, bpm) {
        if b == 1 {
            h.push_note("notes_in", on(0, 64, 0.8)); // +4
        }
        if b == f + 1 {
            h.push_note("notes_in", on(0, 55, 0.8)); // -5, most recent
        }
        if b == due(2 * s, sr, bpm) as usize / BATCH + 1 {
            h.push_note("notes_in", off(0, 55));
            h.push_note("notes_in", off(1, 64));
        }
        for e in batch(&mut h) {
            if let K::On(k, _) = e.k {
                keys.push(k);
            }
        }
    }
    assert_eq!(&keys[..4], &[60, 64, 55, 60]);
}

#[test]
fn mute_closes_notes_and_plays_nothing_but_the_clock_runs() {
    let (sr, bpm, s) = (48_000u64, 120u64, 3_840u64);
    let mut h = rig(
        sr as f64,
        bpm as f64,
        &[full(SeqResolution::Sixteenth, 4, 0, 60, 100)],
    );
    let mut evs = Vec::new();
    let mute_at = due(s + 1_000, sr, bpm) as usize / BATCH + 1;
    let unmute_at = due(2 * s + 1_000, sr, bpm) as usize / BATCH;
    for b in 0..batches_for(5 * s, sr, bpm) {
        if b == mute_at {
            set(&mut h, "mute", 1.0);
        }
        if b == unmute_at {
            set(&mut h, "mute", 0.0);
        }
        evs.extend(batch(&mut h));
    }
    let off = evs
        .iter()
        .find(|e| e.k == K::Off(60) && e.frame >= (mute_at * BATCH) as u64)
        .unwrap();
    assert_eq!(
        off.frame,
        (mute_at * BATCH) as u64,
        "closed at the start of the batch"
    );
    let ons_after: Vec<u64> = evs
        .iter()
        .filter(|e| matches!(e.k, K::On(..)) && e.frame > off.frame)
        .map(|e| e.frame)
        .collect();
    assert_eq!(
        ons_after[0],
        due(3 * s, sr, bpm),
        "unmuting resumes on the next step"
    );
}

#[test]
fn no_pattern_data_plays_nothing() {
    let mut h = DeviceHarness::new(Box::new(StepSequencer::new()), 48_000.0);
    assert!(collect(&mut h, 300).is_empty());
}

#[test]
fn stop_closes_the_note_and_start_resumes_in_the_grid() {
    let (sr, bpm, s) = (48_000u64, 120u64, 3_840u64);
    let mut h = rig(
        sr as f64,
        bpm as f64,
        &[full(SeqResolution::Sixteenth, 4, 0, 60, 100)],
    );
    let evs = collect(&mut h, 10);
    assert_eq!(plain(&evs), [(0, K::On(60, 100))]);
    h.time_mut().playing = false;
    let evs = batch(&mut h);
    assert_eq!(plain(&evs), [(640, K::Off(60))]);
    h.time_mut().playing = false;
    assert!(collect(&mut h, 5).is_empty());
    // Start again from tick 2 * s (the transport tick stays where the playhead was).
    h.time_mut().playing = true;
    h.time_mut().tick = (2 * s) as f64;
    let evs = batch(&mut h);
    assert_eq!(evs[0].k, K::On(60, 100));
    assert_eq!(evs[0].frame, h.frame() - BATCH as u64);
}

#[test]
fn locate_lands_on_the_step_a_continuous_run_reaches() {
    let (sr, bpm, s) = (48_000u64, 120u64, 3_840u64);
    let steps: Vec<StepData> = (0..8).map(|i| st(60 + i, 100, 50)).collect();
    let p = pat(SeqResolution::Sixteenth, 8, 0, &steps);
    let mut h = rig(sr as f64, bpm as f64, &[p]);
    collect(&mut h, 20);
    let t0 = 11.5 * s as f64;
    h.time_mut().tick = t0;
    let tpf = h.time_mut().ticks_per_frame;
    let base = h.frame();
    let evs = collect(&mut h, 200);
    let ons: Vec<&Ev> = evs.iter().filter(|e| matches!(e.k, K::On(..))).collect();
    // Step 12 mod 8 = 4 is next: key 64 at ceil((12 s - t0) / tpf) frames into the batch.
    assert_eq!(ons[0].k, K::On(64, 100));
    assert_eq!(
        ons[0].frame,
        base + (((12 * s) as f64 - t0) / tpf - 1e-6).ceil() as u64
    );
    assert_eq!(ons[1].k, K::On(65, 100));
    // A locate closes the sounding note at the first frame of the batch.
    let mut h = rig(
        sr as f64,
        bpm as f64,
        &[full(SeqResolution::Sixteenth, 4, 0, 60, 100)],
    );
    collect(&mut h, 10);
    h.time_mut().tick = 3.5 * s as f64;
    let evs = batch(&mut h);
    assert_eq!(plain(&evs)[0], (h.frame() - BATCH as u64, K::Off(60)));
}

#[test]
fn loop_wrap_inside_a_batch_closes_notes_at_the_wrap_and_restarts() {
    let (sr, bpm) = (48_000u64, 120u64);
    let mut h = rig(
        sr as f64,
        bpm as f64,
        &[full(SeqResolution::Sixteenth, 4, 0, 60, 100)],
    );
    collect(&mut h, 10);
    let tpf = h.time_mut().ticks_per_frame;
    h.time_mut().loop_wrap = Some(LoopWrap {
        offset: 20,
        to_tick: 0.0,
    });
    let base = h.frame();
    let evs = batch(&mut h);
    assert_eq!(
        plain(&evs),
        [(base + 20, K::Off(60)), (base + 20, K::On(60, 100))]
    );
    // The next batch continues from the wrapped position without a second jump.
    h.time_mut().tick = 44.0 * tpf;
    let evs = batch(&mut h);
    assert!(evs.is_empty(), "{evs:?}");
}

#[test]
fn loop_wrap_on_a_batch_boundary_is_a_tick_jump() {
    let (sr, bpm, s) = (48_000u64, 120u64, 3_840u64);
    let mut h = rig(
        sr as f64,
        bpm as f64,
        &[full(SeqResolution::Sixteenth, 4, 0, 60, 100)],
    );
    collect(&mut h, 10);
    h.time_mut().tick = 0.0; // wrapped exactly at the batch boundary
    let base = h.frame();
    let evs = batch(&mut h);
    assert_eq!(plain(&evs), [(base, K::Off(60)), (base, K::On(60, 100))]);
    let _ = s;
}

#[test]
fn tempo_change_inside_a_batch_is_not_a_jump() {
    let (sr, bpm, s) = (48_000u64, 120u64, 3_840u64);
    let mut h = rig(
        sr as f64,
        bpm as f64,
        &[full(SeqResolution::Sixteenth, 4, 0, 60, 100)],
    );
    collect(&mut h, 10);
    // The tempo map changes inside the previous batch: the new batch's tick is not
    // `previous + 64 * old_tpf`, and its ticks_per_frame differs.
    let old = h.time_mut().ticks_per_frame;
    let new = old * 1.25;
    let tick = h.time_mut().tick + 64.0 * (old + new) / 2.0 - 64.0 * old;
    h.time_mut().ticks_per_frame = new;
    h.time_mut().tick += 64.0 * (old + new) / 2.0 - 64.0 * old;
    let base = h.frame();
    let evs = batch(&mut h);
    // No premature off (a jump would close the note at the first frame of the batch).
    assert!(evs.iter().all(|e| e.frame != base), "{evs:?}");
    let end = (s) as f64; // legato off at the next step start
    let want = base + ((end - tick) / new).ceil() as u64;
    let more = collect(&mut h, 140);
    let off = more
        .iter()
        .chain(evs.iter())
        .find(|e| e.k == K::Off(60))
        .unwrap();
    assert!(off.frame.abs_diff(want) <= 1, "{} vs {want}", off.frame);
}

#[test]
fn swapping_data_during_a_sounding_note_leaves_it_intact() {
    let (sr, bpm, s) = (48_000u64, 120u64, 3_840u64);
    let mut h = rig(
        sr as f64,
        bpm as f64,
        &[full(SeqResolution::Quarter, 4, 0, 60, 100)],
    );
    let mut evs = collect(&mut h, 20);
    let old =
        h.device_mut()
            .swap_player_data(Some(data(&[pat(SeqResolution::Quarter, 4, 0, &[])])));
    assert!(old.is_some(), "the previous data comes back");
    evs.extend(collect(&mut h, batches_for(4 * s, sr, bpm)));
    let want = [(0, K::On(60, 100)), (due(4 * s, sr, bpm), K::Off(60))];
    assert_eq!(plain(&evs), want, "one on, one off, no double");
    // A swap to None while a tie holds the note: the note ends at the next step boundary.
    let mut tied = st(60, 100, 80);
    tied.tie = true;
    let mut h = rig(
        sr as f64,
        bpm as f64,
        &[pat(SeqResolution::Sixteenth, 4, 0, &[tied, tied])],
    );
    let mut evs = collect(&mut h, 20);
    drop(h.device_mut().swap_player_data(None));
    evs.extend(collect(&mut h, 400));
    let offs = evs.iter().filter(|e| matches!(e.k, K::Off(_))).count();
    let ons = evs.iter().filter(|e| matches!(e.k, K::On(..))).count();
    assert_eq!((ons, offs), (1, 1));
}

#[test]
fn reset_closes_sounding_notes_with_an_all_off() {
    let mut h = rig(
        48_000.0,
        120.0,
        &[full(SeqResolution::Sixteenth, 4, 0, 60, 100)],
    );
    collect(&mut h, 3);
    h.device_mut().reset();
    let evs = batch(&mut h);
    assert!(
        plain(&evs).contains(&(h.frame() - BATCH as u64, K::AllOff)),
        "{evs:?}"
    );
    // And after reset it plays again from the transport position.
    h.time_mut().tick = 0.0;
    let evs = collect(&mut h, 3);
    assert!(evs.iter().any(|e| matches!(e.k, K::On(..))));
}

#[test]
fn all_off_on_notes_in_releases_the_sounding_note() {
    let mut h = rig(
        48_000.0,
        120.0,
        &[full(SeqResolution::Sixteenth, 4, 0, 60, 100)],
    );
    collect(&mut h, 3);
    h.push_note(
        "notes_in",
        NoteEvent {
            offset: 5,
            kind: NoteKind::AllOff,
        },
    );
    let base = h.frame();
    let evs = batch(&mut h);
    assert_eq!(plain(&evs), [(base + 5, K::Off(60))]);
}

fn played_steps(h: &mut DeviceHarness, s: u64, batches: usize) -> Vec<u64> {
    let mut out = Vec::new();
    for _ in 0..batches {
        for e in batch(h) {
            if matches!(e.k, K::On(..)) {
                out.push(((e.tick + 0.01) / s as f64).floor() as u64);
            }
        }
    }
    out
}

fn prob_pattern(prob: u8) -> PatternData {
    let mut p = full(SeqResolution::ThirtySecond, 64, 0, 60, 50);
    for s in &mut p.steps {
        s.prob = prob;
    }
    p
}

#[test]
fn probability_zero_never_and_hundred_always_and_fifty_is_near_half() {
    let s = 1_920u64;
    let n = 10_000u64;
    let batches = |sr: u64, bpm: u64| batches_for(n * s, sr, bpm);
    let mut h = rig(8_000.0, 300.0, &[prob_pattern(0)]);
    assert!(played_steps(&mut h, s, batches(8_000, 300)).is_empty());
    let mut h = rig(8_000.0, 300.0, &[prob_pattern(100)]);
    let all = played_steps(&mut h, s, batches(8_000, 300));
    assert!(all.len() as u64 >= n);
    let mut h = rig(8_000.0, 300.0, &[prob_pattern(50)]);
    let half = played_steps(&mut h, s, batches(8_000, 300));
    let in_range = half.iter().filter(|k| **k < n).count();
    assert!((4_800..=5_200).contains(&in_range), "{in_range}");
}

#[test]
fn probability_is_the_same_across_runs_start_positions_and_changes_with_the_seed() {
    let s = 1_920u64;
    let run = |seed_param: f32, start: Option<u64>| {
        let mut h = rig(8_000.0, 300.0, &[prob_pattern(50)]);
        set(&mut h, "random.seed", seed_param);
        if let Some(k) = start {
            h.time_mut().tick = (k * s) as f64;
        }
        played_steps(&mut h, s, 20_000)
    };
    let a = run(0.0, None);
    let b = run(0.0, None);
    assert_eq!(a, b, "two runs agree");
    // Starting at step 3000: the same steps play from there on.
    let c = run(0.0, Some(3_000));
    let a_tail: Vec<u64> = a.iter().copied().filter(|k| *k >= 3_000).collect();
    let n = c.len().min(a_tail.len());
    assert!(n > 100);
    assert_eq!(&c[..n], &a_tail[..n]);
    let d = run(1.0, None);
    assert_ne!(a, d, "another seed re-rolls");
}

#[test]
fn probability_amount_scales_every_step() {
    let s = 1_920u64;
    let n = 10_000u64;
    for (amount, lo, hi) in [(0.0f32, 0, 0), (0.5, 4_800, 5_200), (1.0, 10_000, 10_000)] {
        let mut h = rig(8_000.0, 300.0, &[prob_pattern(100)]);
        set(&mut h, "probability.amount", amount);
        let k = played_steps(&mut h, s, batches_for(n * s, 8_000, 300));
        let c = k.iter().filter(|x| **x < n).count();
        assert!((lo..=hi).contains(&c), "amount {amount}: {c}");
    }
}

#[test]
fn meter_values_publish_the_pattern_and_the_step() {
    let (sr, bpm, s) = (48_000u64, 120u64, 3_840u64);
    let a = full(SeqResolution::Sixteenth, 4, 0, 60, 50);
    let mut h = rig(sr as f64, bpm as f64, &[a, a]);
    assert!(h.device().has_meter_values());
    set(&mut h, "pattern", 2.0);
    for _ in 0..batches_for(2 * s + 100, sr, bpm) {
        h.process();
    }
    let mut out = [0.0f32; 8];
    let n = h.device().meter_values(&mut out);
    assert!(n >= 2);
    assert_eq!(out[0], 1.0, "0-based pattern index");
    assert_eq!(out[1], 2.0, "step index of the last started step");
}

/// One transport/automation action of the hang-note proptest.
#[derive(Debug, Clone)]
enum Act {
    Run,
    Stop,
    Play,
    Locate(u32),
    Wrap(u8, u32),
    Mute(bool),
    Pattern(u8, bool),
    Tempo(u16),
    Swap(u8),
}

fn act() -> impl Strategy<Value = Act> {
    prop_oneof![
        6 => Just(Act::Run),
        1 => Just(Act::Stop),
        1 => Just(Act::Play),
        2 => (0u32..400_000).prop_map(Act::Locate),
        2 => (1u8..64, 0u32..200_000).prop_map(|(o, t)| Act::Wrap(o, t)),
        1 => any::<bool>().prop_map(Act::Mute),
        2 => (1u8..=3, any::<bool>()).prop_map(|(p, n)| Act::Pattern(p, n)),
        1 => (60u16..300).prop_map(Act::Tempo),
        1 => (0u8..3).prop_map(Act::Swap),
    ]
}

fn random_pattern(seed: u8, res: SeqResolution) -> PatternData {
    let mut p = pat(res, 5 + seed % 9, ((u16::from(seed) * 17) % 76) as u8, &[]);
    for i in 0..64usize {
        p.steps[i] = StepData {
            on: (i + usize::from(seed)) % 3 != 1,
            key: 40 + ((i * 7 + usize::from(seed)) % 30) as u8,
            velocity: 1 + ((i * 13) % 127) as u8,
            gate: 1 + ((i * 29 + usize::from(seed)) % 100) as u8,
            prob: if i % 4 == 0 { 60 } else { 100 },
            tie: (i + usize::from(seed)) % 4 == 2,
        };
    }
    p
}

fn check_balance(count: &mut [i32; 128], evs: &[Ev]) {
    for e in evs {
        match e.k {
            K::On(k, _) => {
                count[usize::from(k)] += 1;
                assert!(count[usize::from(k)] <= 1, "double on of {k} at {e:?}");
            }
            K::Off(k) => {
                count[usize::from(k)] -= 1;
                assert!(count[usize::from(k)] >= 0, "off without on of {k} at {e:?}");
            }
            K::AllOff => *count = [0; 128],
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn no_note_hangs_or_doubles_across_random_transport_scripts(
        acts in proptest::collection::vec(act(), 5..140),
        res in 0usize..7,
        seed in 0u8..255,
    ) {
        let resn = SeqResolution::ALL[res];
        let pats = [random_pattern(seed, resn), random_pattern(seed.wrapping_add(5), SeqResolution::Eighth), random_pattern(seed.wrapping_add(9), SeqResolution::ThirtySecondTriplet)];
        let mut h = rig(48_000.0, 140.0, &pats);
        let mut count = [0i32; 128];
        for a in acts {
            match a {
                Act::Run => {}
                Act::Stop => h.time_mut().playing = false,
                Act::Play => h.time_mut().playing = true,
                Act::Locate(t) => h.time_mut().tick = f64::from(t),
                Act::Wrap(o, t) => h.time_mut().loop_wrap = Some(LoopWrap { offset: u16::from(o), to_tick: f64::from(t) }),
                Act::Mute(m) => set(&mut h, "mute", if m { 1.0 } else { 0.0 }),
                Act::Pattern(p, now) => {
                    set(&mut h, "pattern.switch", if now { 0.0 } else { 1.0 });
                    set(&mut h, "pattern", f32::from(p));
                }
                Act::Tempo(bpm) => {
                    let tpf = f64::from(bpm) * 15_360.0 / (60.0 * 48_000.0);
                    h.time_mut().ticks_per_frame = tpf;
                }
                Act::Swap(i) => {
                    let d = data(&pats[..=usize::from(i)]);
                    drop(h.device_mut().swap_player_data(Some(d)));
                }
            }
            let evs = batch(&mut h);
            check_balance(&mut count, &evs);
            // Events leave in non-decreasing offset order (NoteBuffer debug-asserts it too).
            prop_assert!(evs.windows(2).all(|w| w[0].frame <= w[1].frame));
        }
        h.time_mut().playing = false;
        let evs = batch(&mut h);
        check_balance(&mut count, &evs);
        prop_assert!(count.iter().all(|c| *c == 0), "hanging notes: {count:?}");
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn outputs_are_finite_and_in_range_for_any_params(
        values in proptest::collection::vec(-1.0e6f32..1.0e6, 9),
        tick in 0.0f64..1.0e7,
        bpm in 20u32..999,
    ) {
        let mut h = rig(44_100.0, 120.0, &[random_pattern(3, SeqResolution::Sixteenth)]);
        for (i, v) in values.iter().enumerate() {
            let p = &DESCRIPTOR.params[i];
            h.set_param(p.key, *v);
        }
        h.time_mut().tick = tick;
        h.time_mut().ticks_per_frame = f64::from(bpm) * 15_360.0 / (60.0 * 44_100.0);
        for _ in 0..40 {
            let evs = batch(&mut h);
            for e in evs {
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

/// Regression (found by the QA mutation runs): switching to a pattern with a finer grid while a
/// long note still sounds must close it before the same key is played again.
#[test]
fn a_finer_pattern_never_doubles_a_still_sounding_key() {
    let a = full(SeqResolution::Quarter, 2, 0, 60, 100);
    let b = full(SeqResolution::ThirtySecondTriplet, 8, 0, 60, 100);
    let mut h = rig(48_000.0, 120.0, &[a, b]);
    set(&mut h, "pattern.switch", 0.0); // now
    let mut evs = collect(&mut h, 3);
    set(&mut h, "pattern", 2.0);
    evs.extend(collect(&mut h, 400));
    let mut open = 0i32;
    let mut ons = 0;
    for e in &evs {
        match e.k {
            K::On(..) => {
                ons += 1;
                open += 1;
                assert!(open <= 1, "double On at {e:?}");
            }
            K::Off(_) => open -= 1,
            K::AllOff => open = 0,
        }
        assert!(open >= 0, "Off without On at {e:?}");
    }
    assert!(ons > 10, "the finer pattern keeps playing");
}

/// A jump closes the sounding note at the jump frame even when no step starts there (with the
/// same key replayed, the next On would hide a missing close).
#[test]
fn a_locate_closes_a_long_note_when_no_step_follows() {
    let mut p = full(SeqResolution::Quarter, 2, 0, 60, 100);
    p.steps[1].on = false;
    let mut h = rig(48_000.0, 120.0, &[p]);
    let evs = collect(&mut h, 1);
    assert_eq!(plain(&evs), [(0, K::On(60, 100))]);
    h.time_mut().tick = 1_000.0; // forward inside step 0, long before its legato end
    let base = h.frame();
    let evs = batch(&mut h);
    assert_eq!(plain(&evs), [(base, K::Off(60))]);
    assert!(collect(&mut h, 50).is_empty(), "nothing hangs or plays");
}

#[test]
fn step_sequencer_bypass_passes_notes_through_and_stops_the_pattern() {
    let (sr, bpm, s) = (48_000u64, 120u64, 3_840u64);
    let mut h = rig(
        sr as f64,
        bpm as f64,
        &[full(SeqResolution::Sixteenth, 4, 0, 60, 100)],
    );
    let mut all = Vec::new();
    let bypass_at = due(s + 1_000, sr, bpm) as usize / BATCH + 1;
    for _ in 0..bypass_at {
        all.extend(batch(&mut h));
    }
    // The playing note closes at the start of the bypass batch; a key played now passes through
    // with its offset.
    set(&mut h, "bypass", 1.0);
    h.push_note("notes_in", on(5, 72, 0.8));
    let base = h.frame();
    let evs = batch(&mut h);
    assert_eq!(
        plain(&evs),
        [(base, K::Off(60)), (base + 5, K::On(72, 102))]
    );
    all.extend(evs);
    assert!(
        collect(&mut h, batches_for(4 * s, sr, bpm)).is_empty(),
        "the pattern stops while bypassed"
    );

    // Leaving the bypass closes the thru key; the pattern resumes on the next step.
    set(&mut h, "bypass", 0.0);
    let base = h.frame();
    let evs = batch(&mut h);
    assert_eq!(plain(&evs), [(base, K::Off(72))]);
    all.extend(evs);
    h.push_note("notes_in", off(0, 72));
    all.extend(collect(&mut h, batches_for(2 * s, sr, bpm)));
    assert!(
        all.iter()
            .filter(|e| e.frame > base && matches!(e.k, K::On(..)))
            .all(|e| e.k == K::On(60, 100)),
        "only the pattern plays again, the old key's Off is ignored"
    );
    assert!(
        all.iter()
            .any(|e| e.frame > base && matches!(e.k, K::On(..)))
    );
    let mut down = std::collections::BTreeSet::new();
    for e in &all {
        match e.k {
            K::On(k, _) => assert!(down.insert(k), "double On of {k}"),
            K::Off(k) => assert!(down.remove(&k), "Off of silent {k}"),
            K::AllOff => down.clear(),
        }
    }
}
