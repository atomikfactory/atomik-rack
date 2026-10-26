//! CV sequencer tests.
#![allow(clippy::float_cmp)]

use super::*;
use atomik_core::{BATCH, LoopWrap, NoteEvent, NoteKind};
use atomik_device_sdk::{
    CurveStep, CvSeqData, PatternData, PlayerData, PlayerDataRef, SeqResolution, StepData,
    StepSeqData,
};
use atomik_testkit::harness::DeviceHarness;
use proptest::prelude::*;
use std::sync::Arc;

use crate::step_sequencer::StepSequencer;
use crate::test_support::{off, on};

const SR: u64 = 48_000;
const BPM: u64 = 120;

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

fn notes(patterns: &[PatternData]) -> StepSeqData {
    let mut d = StepSeqData {
        count: patterns.len() as u8,
        ..StepSeqData::default()
    };
    for (i, p) in patterns.iter().enumerate() {
        d.patterns[i] = *p;
    }
    d
}

/// CV data with flat curves.
fn cv_data(patterns: &[PatternData]) -> CvSeqData {
    CvSeqData {
        notes: notes(patterns),
        ..CvSeqData::default()
    }
}

fn curve(value: i8, slide: bool) -> CurveStep {
    CurveStep { value, slide }
}

fn arc(d: CvSeqData) -> PlayerDataRef {
    Arc::new(PlayerData::CvSeq(d))
}

fn rig_with(sr: f64, bpm: f64, d: CvSeqData) -> DeviceHarness {
    let mut h = DeviceHarness::with_options(Box::new(CvSequencer::new()), sr, bpm, 7);
    let back = h.device_mut().swap_player_data(Some(arc(d)));
    assert!(back.is_none());
    h
}

fn rig(patterns: &[PatternData]) -> DeviceHarness {
    rig_with(SR as f64, BPM as f64, cv_data(patterns))
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

fn ons(evs: &[Ev]) -> Vec<(u64, u8)> {
    evs.iter()
        .filter_map(|e| match e.k {
            K::On(k, _) => Some((e.frame, k)),
            _ => None,
        })
        .collect()
}

/// First frame at or after the integer tick `tick` (exact integer arithmetic).
fn due(tick: u64) -> u64 {
    let num = u128::from(tick) * 60 * u128::from(SR);
    let den = u128::from(BPM) * 15_360;
    num.div_ceil(den) as u64
}

fn batches_for(ticks: u64) -> usize {
    (due(ticks) / BATCH as u64) as usize + 3
}

fn set(h: &mut DeviceHarness, key: &str, v: f32) {
    h.set_param(key, v);
}

/// Runs `batches` batches and returns one output's samples.
fn samples(h: &mut DeviceHarness, key: &str, batches: usize) -> Vec<f32> {
    let mut out = Vec::new();
    for _ in 0..batches {
        h.process();
        out.extend_from_slice(h.output(key));
    }
    out
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
            "chain",
            "transpose",
            "follow",
            "probability.amount",
            "random.seed",
            "velocity.scale",
            "gate.scale",
            "curve_a.polarity",
            "curve_b.polarity",
            "curve_a.amount",
            "curve_b.amount",
            "mute",
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
            "trig_cv_out",
            "curve_a_cv_out",
            "curve_b_cv_out"
        ]
    );
    assert_eq!(DESCRIPTOR.kind, atomik_device_sdk::DeviceKind::Utility);
}

/// A pattern with ties, probability, gates and an off step, on 1/16 with swing.
fn busy(seed: u8) -> PatternData {
    let mut p = pat(SeqResolution::Sixteenth, 12, 30 + seed, &[]);
    for i in 0..12usize {
        p.steps[i] = StepData {
            on: i % 5 != 3,
            key: 48 + ((i * 5 + usize::from(seed)) % 24) as u8,
            velocity: 40 + (i * 7) as u8,
            gate: 20 + (i * 11 % 80) as u8,
            prob: if i % 3 == 0 { 55 } else { 100 },
            tie: i % 4 == 2,
        };
    }
    p
}

#[test]
fn note_row_matches_the_step_sequencer_for_the_same_data() {
    let pats = [
        busy(0),
        full(SeqResolution::EighthTriplet, 6, 0, 64, 100),
        pat(
            SeqResolution::Quarter,
            5,
            0,
            &[st(60, 90, 50), st(67, 90, 100)],
        ),
    ];
    for (pattern, transpose, follow) in [(1.0, 0.0, 0.0), (2.0, 5.0, 0.0), (3.0, -7.0, 1.0)] {
        let mut a =
            DeviceHarness::with_options(Box::new(StepSequencer::new()), SR as f64, BPM as f64, 7);
        let plain_data: PlayerDataRef = Arc::new(PlayerData::StepSeq(notes(&pats)));
        let _ = a.device_mut().swap_player_data(Some(plain_data));
        let mut b = rig(&pats);
        for h in [&mut a, &mut b] {
            set(h, "pattern", pattern);
            set(h, "pattern.switch", 0.0);
            set(h, "transpose", transpose);
            set(h, "follow", follow);
            set(h, "probability.amount", 0.8);
            set(h, "random.seed", 11.0);
            h.push_note("notes_in", on(3, 65, 0.7));
        }
        for n in 0..260 {
            if n == 100 {
                for h in [&mut a, &mut b] {
                    h.push_note("notes_in", on(9, 71, 0.7));
                }
            }
            a.process();
            b.process();
            assert_eq!(
                a.notes_out("notes_out"),
                b.notes_out("notes_out"),
                "batch {n}"
            );
            for key in [
                "gate_cv_out",
                "pitch_cv_out",
                "velocity_cv_out",
                "trig_cv_out",
            ] {
                assert_eq!(a.output(key), b.output(key), "{key} batch {n}");
            }
        }
    }
}

#[test]
fn curve_holds_per_step_and_slides_linearly() {
    let s = 3_840u64;
    let mut d = cv_data(&[full(SeqResolution::Sixteenth, 4, 0, 60, 50)]);
    d.curves[0][0][0] = curve(50, false);
    d.curves[0][0][1] = curve(-100, true);
    d.curves[0][0][2] = curve(100, false);
    d.curves[0][0][3] = curve(20, true); // slides into step 0 of the next pass: 50
    d.curves[0][1][1] = curve(80, false);
    let mut h = rig_with(SR as f64, BPM as f64, d);
    let n = batches_for(8 * s);
    let a = samples(&mut h, "curve_a_cv_out", n);
    let mut h = rig_with(SR as f64, BPM as f64, d);
    let b = samples(&mut h, "curve_b_cv_out", n);
    let tpf = BPM as f64 * 15_360.0 / (60.0 * SR as f64);
    let at = |t: u64| due(t) as usize;
    // Held values.
    assert!(a[at(0)..at(s)].iter().all(|v| *v == 0.5));
    assert!(a[at(2 * s)..at(3 * s)].iter().all(|v| *v == 1.0));
    // The slide of step 1 runs from -1 to the value of step 2 (1) over the step.
    for f in [at(s), at(s) + 100, at(s + s / 2), at(2 * s) - 1] {
        let want = -1.0 + 2.0 * (f as f64 * tpf - s as f64) / s as f64;
        assert!((f64::from(a[f]) - want).abs() < 1e-5, "frame {f}: {}", a[f]);
    }
    // The last step slides into the first step of the next pass.
    let f = at(3 * s + s / 2);
    let want = 0.2 + (0.5 - 0.2) * (f as f64 * tpf - (3 * s) as f64) / s as f64;
    assert!((f64::from(a[f]) - want).abs() < 1e-5);
    // Row B is independent: flat zero but step 1.
    assert!(b[at(0)..at(s)].iter().all(|v| *v == 0.0));
    assert!(b[at(s)..at(2 * s)].iter().all(|v| (*v - 0.8).abs() < 1e-6));
}

#[test]
fn unipolar_and_amount_map_the_curve() {
    let s = 3_840u64;
    let mut d = cv_data(&[full(SeqResolution::Sixteenth, 4, 0, 60, 50)]);
    for (row, vals) in [[-100, 0, 100, 50], [-100, 0, 100, 50]].iter().enumerate() {
        for (i, v) in vals.iter().enumerate() {
            d.curves[0][row][i] = curve(*v, false);
        }
    }
    let at = |t: u64| due(t) as usize + 5;
    let run = |params: &[(&str, f32)]| {
        let mut h = rig_with(SR as f64, BPM as f64, d);
        for (k, v) in params {
            set(&mut h, k, *v);
        }
        let a = samples(&mut h, "curve_a_cv_out", batches_for(4 * s));
        [a[at(0)], a[at(s)], a[at(2 * s)], a[at(3 * s)]]
    };
    assert_eq!(run(&[]), [-1.0, 0.0, 1.0, 0.5]);
    assert_eq!(run(&[("curve_a.polarity", 1.0)]), [0.0, 0.5, 1.0, 0.75]);
    assert_eq!(run(&[("curve_a.amount", 0.5)]), [-0.5, 0.0, 0.5, 0.25]);
    assert_eq!(
        run(&[("curve_a.polarity", 1.0), ("curve_a.amount", 0.5)]),
        [0.0, 0.25, 0.5, 0.375]
    );
    assert_eq!(
        run(&[("curve_b.amount", 0.0)]),
        [-1.0, 0.0, 1.0, 0.5],
        "B is its own row"
    );
    // Row B has its own polarity and amount.
    let mut h = rig_with(SR as f64, BPM as f64, d);
    set(&mut h, "curve_b.polarity", 1.0);
    set(&mut h, "curve_b.amount", 0.5);
    let b = samples(&mut h, "curve_b_cv_out", batches_for(4 * s));
    assert_eq!([b[at(0)], b[at(s)], b[at(2 * s)]], [0.0, 0.25, 0.5]);
}

#[test]
fn chain_plays_patterns_in_order_with_their_own_resolution() {
    // 2 sixteenths (key 60), 3 eighths (key 62), 2 quarters (key 64, the second swung by half a
    // step... with swing 50: (50 * 15 360 + 100) / 200 = 3 840 ticks).
    let pats = [
        full(SeqResolution::Sixteenth, 2, 0, 60, 50),
        full(SeqResolution::Eighth, 3, 0, 62, 50),
        full(SeqResolution::Quarter, 2, 50, 64, 50),
    ];
    let mut h = rig(&pats);
    set(&mut h, "chain", 3.0);
    // Pass 0: patterns at 0, 7 680 and 30 720; the pass is 7 680 + 23 040 + 30 720 = 61 440.
    let starts: [(u64, u8); 7] = [
        (0, 60),
        (3_840, 60),
        (7_680, 62),
        (15_360, 62),
        (23_040, 62),
        (30_720, 64),
        (30_720 + 15_360 + 3_840, 64),
    ];
    let evs = collect(&mut h, batches_for(61_440 + 7_680));
    let limit = due(61_440 + 7_680);
    let got: Vec<(u64, u8)> = ons(&evs).into_iter().filter(|(f, _)| *f < limit).collect();
    let mut want: Vec<(u64, u8)> = starts.iter().map(|(t, k)| (due(*t), *k)).collect();
    want.extend(starts.iter().take(2).map(|(t, k)| (due(t + 61_440), *k)));
    assert_eq!(got, want);
}

#[test]
fn chain_starts_at_the_selected_pattern_and_wraps_at_the_pattern_count() {
    let pats = [
        full(SeqResolution::Sixteenth, 1, 0, 60, 50),
        full(SeqResolution::Sixteenth, 1, 0, 62, 50),
        full(SeqResolution::Sixteenth, 1, 0, 64, 50),
    ];
    let mut h = rig(&pats);
    set(&mut h, "pattern", 3.0);
    set(&mut h, "pattern.switch", 0.0);
    set(&mut h, "chain", 3.0);
    let evs = collect(&mut h, batches_for(6 * 3_840));
    let keys: Vec<u8> = ons(&evs).iter().map(|(_, k)| *k).collect();
    assert_eq!(&keys[..6], [64, 60, 62, 64, 60, 62]);
}

/// The ons of a run that starts with a locate to `tick`, by the tick of each On.
fn ons_by_tick(d: CvSeqData, chain: f32, tick: f64, batches: usize) -> Vec<(f64, u8)> {
    let mut h = rig_with(SR as f64, BPM as f64, d);
    set(&mut h, "chain", chain);
    h.time_mut().tick = tick;
    collect(&mut h, batches)
        .iter()
        .filter_map(|e| match e.k {
            K::On(k, _) => Some((e.tick, k)),
            _ => None,
        })
        .collect()
}

fn chain_data() -> CvSeqData {
    let mut p0 = full(SeqResolution::Sixteenth, 3, 20, 60, 50);
    let mut p1 = full(SeqResolution::EighthTriplet, 4, 0, 62, 100);
    let mut p2 = full(SeqResolution::Quarter, 2, 50, 64, 50);
    for (i, p) in [&mut p0, &mut p1, &mut p2].into_iter().enumerate() {
        for (j, s) in p.steps.iter_mut().enumerate() {
            s.key += (j % 7) as u8 + i as u8;
        }
    }
    cv_data(&[p0, p1, p2])
}

#[test]
fn chain_position_is_a_pure_function_of_tick() {
    let d = chain_data();
    let run = ons_by_tick(d, 3.0, 0.0, 5_000);
    assert!(run.len() > 20);
    for t0 in [1_000.0, 8_000.0, 20_000.0, 33_333.0, 61_000.0, 150_000.0] {
        let tpf = BPM as f64 * 15_360.0 / (60.0 * SR as f64);
        let located = ons_by_tick(d, 3.0, t0, 600);
        assert!(!located.is_empty());
        let expect: Vec<(f64, u8)> = run
            .iter()
            .copied()
            .filter(|(t, _)| *t >= t0)
            .take(located.len())
            .collect();
        assert_eq!(expect.len(), located.len(), "t0 {t0}");
        for (e, g) in expect.iter().zip(&located) {
            assert_eq!(e.1, g.1, "key after locate to {t0}");
            assert!(
                (e.0 - g.0).abs() <= tpf,
                "tick after locate to {t0}: {e:?} {g:?}"
            );
        }
    }
}

#[test]
fn locate_into_the_third_pattern_of_a_chain() {
    let pats = [
        full(SeqResolution::Sixteenth, 2, 0, 60, 50),
        full(SeqResolution::Eighth, 3, 0, 62, 50),
        pat(
            SeqResolution::Quarter,
            3,
            0,
            &[st(70, 100, 50), st(71, 100, 50), st(72, 100, 50)],
        ),
    ];
    let mut h = rig(&pats);
    set(&mut h, "chain", 3.0);
    collect(&mut h, 5);
    // The third pattern starts at 7 680 + 23 040 = 30 720; its step 1 at 46 080. Locate halfway
    // through its step 0: the next On is step 1, key 71.
    let t0 = 30_720.0 + 7_000.0;
    h.time_mut().tick = t0;
    let base = h.frame();
    let tpf = h.time_mut().ticks_per_frame;
    let evs = collect(&mut h, 400);
    let first = ons(&evs)[0];
    assert_eq!(first.1, 71);
    assert_eq!(first.0, base + ((46_080.0 - t0) / tpf - 1e-6).ceil() as u64);
}

#[test]
fn curves_follow_the_chain_and_are_the_same_after_a_locate() {
    let mut d = chain_data();
    for (p, base) in [(0usize, 10i8), (1, -30), (2, 60)] {
        for j in 0..8 {
            d.curves[p][0][j] = curve(base + j as i8 * 3, j % 3 == 1);
            d.curves[p][1][j] = curve(-base + j as i8 * 2, j % 2 == 0);
        }
    }
    let n = 900;
    let mut h = rig_with(SR as f64, BPM as f64, d);
    set(&mut h, "chain", 3.0);
    let all = samples(&mut h, "curve_a_cv_out", n);
    // A locate to a batch boundary reads the same samples as the continuous run.
    for first in [37usize, 211, 640] {
        let mut h = rig_with(SR as f64, BPM as f64, d);
        set(&mut h, "chain", 3.0);
        h.time_mut().tick = (first * BATCH) as f64 * (BPM as f64 * 15_360.0 / (60.0 * SR as f64));
        let part = samples(&mut h, "curve_a_cv_out", 30);
        assert_eq!(
            part,
            all[first * BATCH..(first + 30) * BATCH],
            "from batch {first}"
        );
    }
}

#[test]
fn curves_hold_and_gate_drops_on_stop() {
    let s = 3_840u64;
    let mut d = cv_data(&[full(SeqResolution::Sixteenth, 4, 0, 60, 100)]);
    d.curves[0][0][1] = curve(40, false);
    d.curves[0][1][1] = curve(-60, false);
    let mut h = rig_with(SR as f64, BPM as f64, d);
    // Run into step 1 (tick s .. 2 s), then stop mid-step.
    let n = due(s + s / 2) as usize / BATCH + 1;
    for _ in 0..n {
        h.process();
    }
    assert!((h.output("curve_a_cv_out")[63] - 0.4).abs() < 1e-6);
    h.time_mut().playing = false;
    let mut gate = Vec::new();
    for _ in 0..6 {
        h.process();
        assert!(
            h.output("curve_a_cv_out")
                .iter()
                .all(|v| (*v - 0.4).abs() < 1e-6)
        );
        assert!(
            h.output("curve_b_cv_out")
                .iter()
                .all(|v| (*v + 0.6).abs() < 1e-6)
        );
        gate.extend_from_slice(h.output("gate_cv_out"));
    }
    assert!(
        gate.iter().all(|v| *v == 0.0),
        "the gate drops with the notes"
    );
    // A changed amount applies to the held value too.
    set(&mut h, "curve_a.amount", 0.5);
    h.process();
    assert!(
        h.output("curve_a_cv_out")
            .iter()
            .all(|v| (*v - 0.2).abs() < 1e-6)
    );
}

#[test]
fn curves_are_zero_before_anything_has_played() {
    let mut h = rig(&[full(SeqResolution::Sixteenth, 4, 0, 60, 50)]);
    h.time_mut().playing = false;
    h.process();
    assert!(h.output("curve_a_cv_out").iter().all(|v| *v == 0.0));
    assert!(h.output("curve_b_cv_out").iter().all(|v| *v == 0.0));
}

#[test]
fn loop_wrap_inside_a_batch_resets_position() {
    let s = 3_840u64;
    let mut d = cv_data(&[full(SeqResolution::Sixteenth, 4, 0, 60, 100)]);
    for j in 0..4 {
        d.curves[0][0][j] = curve(10 * (j as i8 + 1), false);
    }
    let mut h = rig_with(SR as f64, BPM as f64, d);
    // Into step 2 (value 30).
    let n = due(2 * s + 100) as usize / BATCH + 1;
    for _ in 0..n {
        h.process();
    }
    assert!((h.output("curve_a_cv_out")[63] - 0.3).abs() < 1e-6);
    h.time_mut().loop_wrap = Some(LoopWrap {
        offset: 20,
        to_tick: 0.0,
    });
    let base = h.frame();
    h.process();
    let evs: Vec<(u64, u8)> = h
        .notes_out("notes_out")
        .iter()
        .map(|e| (base + u64::from(e.offset), kind(e)))
        .map(|(f, k)| {
            (
                f,
                match k {
                    K::On(key, _) => key,
                    K::Off(_) => 0,
                    K::AllOff => 255,
                },
            )
        })
        .collect();
    assert_eq!(
        evs,
        [(base + 20, 0), (base + 20, 60)],
        "close then restart at the wrap"
    );
    let a = h.output("curve_a_cv_out");
    assert!(
        a[..20].iter().all(|v| (*v - 0.3).abs() < 1e-6),
        "before the wrap: step 2"
    );
    assert!(
        a[20..].iter().all(|v| (*v - 0.1).abs() < 1e-6),
        "after it: step 0"
    );
}

#[test]
fn pattern_and_chain_change_at_end_adopt_at_the_start_of_the_next_pass() {
    let pats = [
        full(SeqResolution::Sixteenth, 2, 0, 60, 50),
        full(SeqResolution::Sixteenth, 2, 0, 62, 50),
    ];
    let mut h = rig(&pats);
    set(&mut h, "pattern.switch", 1.0); // at-end
    let mut evs = collect(&mut h, 2); // inside step 0
    set(&mut h, "chain", 2.0);
    evs.extend(collect(&mut h, batches_for(8 * 3_840)));
    let keys: Vec<u8> = ons(&evs).iter().map(|(_, k)| *k).collect();
    // The pass of pattern 1 (steps 0 and 1) finishes; the chain 60, 60, 62, 62 is adopted at
    // tick 7 680, where its own period (15 360) is at its step 2: 62, 62, then 60, 60.
    assert_eq!(&keys[..6], [60, 60, 62, 62, 60, 60]);
}

#[test]
fn data_without_curves_gives_flat_zero_curves() {
    let mut h = DeviceHarness::with_options(Box::new(CvSequencer::new()), SR as f64, BPM as f64, 7);
    let plain_data: PlayerDataRef = Arc::new(PlayerData::StepSeq(notes(&[full(
        SeqResolution::Sixteenth,
        4,
        0,
        60,
        50,
    )])));
    let _ = h.device_mut().swap_player_data(Some(plain_data));
    let a = samples(&mut h, "curve_a_cv_out", 20);
    assert!(a.iter().all(|v| *v == 0.0));
    let mut h = DeviceHarness::new(Box::new(CvSequencer::new()), SR as f64);
    assert!(collect(&mut h, 10).is_empty(), "no data plays nothing");
}

#[test]
fn mute_closes_notes_and_plays_nothing_but_the_curves_run() {
    let s = 3_840u64;
    let mut d = cv_data(&[full(SeqResolution::Sixteenth, 4, 0, 60, 100)]);
    d.curves[0][0][2] = curve(70, false);
    let mut h = rig_with(SR as f64, BPM as f64, d);
    collect(&mut h, 3);
    set(&mut h, "mute", 1.0);
    let base = h.frame();
    let evs = batch(&mut h);
    assert_eq!(plain(&evs), [(base, K::Off(60))]);
    let n = due(3 * s) as usize / BATCH + 1;
    let mut max = 0.0f32;
    for _ in 0..n {
        let evs = batch(&mut h);
        assert!(ons(&evs).is_empty());
        max = max.max(
            h.output("curve_a_cv_out")
                .iter()
                .copied()
                .fold(0.0, f32::max),
        );
    }
    assert!((max - 0.7).abs() < 1e-6, "the curve of step 2 still plays");
}

#[test]
fn bypass_passes_notes_through_stops_the_pattern_and_holds_the_curves() {
    let s = 3_840u64;
    let mut d = cv_data(&[full(SeqResolution::Sixteenth, 4, 0, 60, 100)]);
    d.curves[0][0][1] = curve(40, false);
    let mut h = rig_with(SR as f64, BPM as f64, d);
    for _ in 0..due(s + 1_000) as usize / BATCH + 1 {
        batch(&mut h);
    }
    assert!((h.output("curve_a_cv_out")[63] - 0.4).abs() < 1e-6);
    set(&mut h, "bypass", 1.0);
    h.push_note("notes_in", on(5, 72, 0.8));
    let base = h.frame();
    let evs = batch(&mut h);
    assert_eq!(
        plain(&evs),
        [(base, K::Off(60)), (base + 5, K::On(72, 102))]
    );
    assert!(
        h.output("curve_a_cv_out")
            .iter()
            .all(|v| (*v - 0.4).abs() < 1e-6)
    );
    assert!(collect(&mut h, batches_for(4 * s)).is_empty());
    assert!(
        h.output("curve_a_cv_out")
            .iter()
            .all(|v| (*v - 0.4).abs() < 1e-6)
    );
    // Leaving the bypass closes the thru key and the curves run again from the position.
    set(&mut h, "bypass", 0.0);
    let evs = batch(&mut h);
    assert_eq!(evs[0].k, K::Off(72));
    h.push_note("notes_in", off(0, 72));
    collect(&mut h, 20);
}

#[test]
fn reset_closes_sounding_notes_with_an_all_off() {
    let mut h = rig(&[full(SeqResolution::Sixteenth, 4, 0, 60, 100)]);
    collect(&mut h, 3);
    h.device_mut().reset();
    let evs = batch(&mut h);
    assert_eq!(evs[0].k, K::AllOff);
    assert_eq!(evs[0].frame, h.frame() - BATCH as u64);
}

#[test]
fn swapping_data_during_a_sounding_note_leaves_it_intact() {
    let mut h = rig(&[full(SeqResolution::Sixteenth, 4, 0, 60, 100)]);
    let first = collect(&mut h, 3);
    assert_eq!(plain(&first), [(0, K::On(60, 100))]);
    let other = cv_data(&[full(SeqResolution::Sixteenth, 4, 0, 72, 100)]);
    let back = h.device_mut().swap_player_data(Some(arc(other)));
    assert!(back.is_some(), "the old data comes back");
    let evs = collect(&mut h, 120);
    let kinds: Vec<K> = evs.iter().map(|e| e.k).take(2).collect();
    assert_eq!(
        kinds,
        [K::Off(60), K::On(72, 100)],
        "the note ends in its own time"
    );
}

#[test]
fn meter_values_publish_the_pattern_and_the_step() {
    let a = full(SeqResolution::Sixteenth, 4, 0, 60, 50);
    let mut h = rig(&[a, a]);
    assert!(h.device().has_meter_values());
    set(&mut h, "pattern", 2.0);
    for _ in 0..batches_for(2 * 3_840 + 100) {
        h.process();
    }
    let mut out = [0.0f32; 8];
    let n = h.device().meter_values(&mut out);
    assert!(n >= 3);
    assert_eq!((out[0], out[1], out[2]), (1.0, 2.0, 1.0));
}

fn random_pattern(seed: u8, res: SeqResolution) -> PatternData {
    let mut p = pat(res, 3 + seed % 9, ((u16::from(seed) * 17) % 76) as u8, &[]);
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

fn random_data(seed: u8, patterns: usize) -> CvSeqData {
    let res = SeqResolution::ALL[usize::from(seed) % 7];
    let pats = [
        random_pattern(seed, res),
        random_pattern(seed.wrapping_add(5), SeqResolution::Eighth),
        random_pattern(seed.wrapping_add(9), SeqResolution::ThirtySecondTriplet),
    ];
    let mut d = cv_data(&pats[..patterns]);
    for p in 0..3 {
        for r in 0..2 {
            for j in 0..64 {
                d.curves[p][r][j] = curve(
                    (((j * 37 + p * 11 + r * 5 + usize::from(seed)) % 201) as i16 - 100) as i8,
                    (j + r) % 3 == 0,
                );
            }
        }
    }
    d
}

#[derive(Debug, Clone)]
enum Act {
    Run,
    Stop,
    Play,
    Locate(u32),
    Wrap(u8, u32),
    Mute(bool),
    Pattern(u8, bool),
    Chain(u8),
    Tempo(u16),
    Swap(u8),
    Bypass(bool),
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
        2 => (1u8..=4).prop_map(Act::Chain),
        1 => (60u16..300).prop_map(Act::Tempo),
        1 => (1u8..=3).prop_map(Act::Swap),
        1 => any::<bool>().prop_map(Act::Bypass),
    ]
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
        seed in 0u8..255,
    ) {
        let mut h = rig_with(48_000.0, 140.0, random_data(seed, 3));
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
                Act::Chain(c) => set(&mut h, "chain", f32::from(c)),
                Act::Tempo(bpm) => {
                    h.time_mut().ticks_per_frame = f64::from(bpm) * 15_360.0 / (60.0 * 48_000.0);
                }
                Act::Swap(i) => {
                    drop(h.device_mut().swap_player_data(Some(arc(random_data(seed, usize::from(i))))));
                }
                Act::Bypass(b) => set(&mut h, "bypass", if b { 1.0 } else { 0.0 }),
            }
            let evs = batch(&mut h);
            check_balance(&mut count, &evs);
            prop_assert!(evs.windows(2).all(|w| w[0].frame <= w[1].frame));
            for key in ["curve_a_cv_out", "curve_b_cv_out"] {
                prop_assert!(h.output(key).iter().all(|s| s.is_finite() && (-1.0..=1.0).contains(s)));
            }
        }
        h.time_mut().playing = false;
        set(&mut h, "bypass", 0.0);
        for _ in 0..2 {
            let evs = batch(&mut h);
            check_balance(&mut count, &evs);
        }
        // A key a bypassed device passed through is not one of ours: only the script's own
        // notes could hang, and the script sends none.
        prop_assert!(count.iter().all(|c| *c == 0), "hanging notes: {count:?}");
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn outputs_are_finite_and_in_range_for_any_params(
        values in proptest::collection::vec(-1.0e6f32..1.0e6, 15),
        tick in 0.0f64..1.0e7,
        bpm in 20u32..999,
    ) {
        let mut h = rig_with(44_100.0, 120.0, random_data(3, 3));
        for (i, v) in values.iter().enumerate() {
            let p = &DESCRIPTOR.params[i];
            h.set_param(p.key, *v);
        }
        h.set_param("bypass", 0.0);
        h.time_mut().tick = tick;
        h.time_mut().ticks_per_frame = f64::from(bpm) * 15_360.0 / (60.0 * 44_100.0);
        for _ in 0..40 {
            let evs = batch(&mut h);
            for e in evs {
                if let K::On(k, v) = e.k {
                    prop_assert!(k < 128 && (1..=127).contains(&v));
                }
            }
            for key in ["gate_cv_out", "pitch_cv_out", "velocity_cv_out", "trig_cv_out", "curve_a_cv_out", "curve_b_cv_out"] {
                prop_assert!(h.output(key).iter().all(|s| s.is_finite() && (-1.0..=1.0).contains(s)));
            }
        }
    }
}
