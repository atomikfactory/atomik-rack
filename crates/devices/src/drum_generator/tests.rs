//! Drum generator tests.
#![allow(clippy::float_cmp)]

use super::*;
use atomik_core::{LoopWrap, NoteEvent, NoteKind};
use atomik_device_sdk::{DrumData, DrumRole, DrumStep, PlayerData, SeqResolution};
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
        other => panic!("the generator never emits {other:?}"),
    }
}

/// A 1/16 pattern of `len` steps with the given `(lane, step, velocity)` hits.
fn pat(len: u8, hits: &[(usize, usize, u8)]) -> DrumPattern {
    let mut p = DrumPattern {
        length: len,
        ..DrumPattern::default()
    };
    for &(lane, step, v) in hits {
        p.steps[lane][step] = DrumStep {
            velocity: v,
            prob: 100,
        };
    }
    p
}

fn data(patterns: &[DrumPattern]) -> PlayerDataRef {
    let mut d = DrumData {
        count: patterns.len() as u8,
        ..DrumData::default()
    };
    for (i, p) in patterns.iter().enumerate() {
        d.patterns[i] = *p;
    }
    Arc::new(PlayerData::Drums(d))
}

fn rig(sr: f64, bpm: f64, patterns: &[DrumPattern]) -> DeviceHarness {
    let mut h = DeviceHarness::with_options(Box::new(DrumGenerator::new()), sr, bpm, 7);
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

/// The step (1/16) of an event's tick.
fn step_of(e: &Ev) -> i64 {
    (e.tick / 3_840.0).round() as i64
}

/// The `(step, key, velocity)` of every On.
fn hits(evs: &[Ev]) -> Vec<(i64, u8, u8)> {
    evs.iter()
        .filter_map(|e| match e.k {
            K::On(key, v) => Some((step_of(e), key, v)),
            _ => None,
        })
        .collect()
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
            "density",
            "variation",
            "fill.every",
            "fill.amount",
            "velocity.scale",
            "accent",
            "gate",
            "random.seed",
            "style",
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
            "trig_cv_out"
        ]
    );
    assert_eq!(DESCRIPTOR.kind, atomik_device_sdk::DeviceKind::Utility);
}

#[test]
fn written_pattern_plays_on_the_exact_frames() {
    // A kick on 0 and 4, a snare on 2, a hat on 3, at every rate and tempo; gate 25 % of 3 840.
    for sr in [44_100u64, 48_000, 96_000] {
        for bpm in [60u64, 133, 300] {
            let p = pat(8, &[(0, 0, 100), (0, 4, 90), (2, 2, 127), (6, 3, 64)]);
            let mut h = rig(sr as f64, bpm as f64, &[p]);
            let evs = collect(&mut h, batches_for(8 * 3_840, sr, bpm));
            let mut want = Vec::new();
            for (step, key, v) in [(0u64, 36u8, 100u8), (2, 38, 127), (3, 42, 64), (4, 36, 90)] {
                want.push((due(step * 3_840, sr, bpm), K::On(key, v)));
                want.push((due(step * 3_840 + 960, sr, bpm), K::Off(key)));
            }
            want.sort_by_key(|(f, _)| *f);
            let limit = due(8 * 3_840, sr, bpm);
            let got: Vec<_> = plain(&evs)
                .into_iter()
                .filter(|(f, _)| *f < limit)
                .collect();
            assert_eq!(got, want, "sr {sr} bpm {bpm}");
        }
    }
}

#[test]
fn lane_keys_and_mutes_apply() {
    let mut p = pat(4, &[(0, 0, 100), (1, 0, 90), (2, 0, 80)]);
    p.lanes[0].key = 60;
    p.lanes[1].mute = true;
    let mut h = rig(48_000.0, 120.0, &[p]);
    let evs = collect(&mut h, 20);
    let ons: Vec<K> = evs
        .iter()
        .map(|e| e.k)
        .filter(|k| matches!(k, K::On(..)))
        .collect();
    assert_eq!(
        ons,
        [K::On(60, 100), K::On(38, 80)],
        "lane 0 plays key 60, the muted lane 1 is silent, lane 2 keeps its pad key"
    );
}

#[test]
fn device_mute_closes_notes_and_plays_nothing_but_the_clock_runs() {
    let p = pat(4, &[(0, 0, 100), (0, 2, 100)]);
    let mut h = rig(48_000.0, 120.0, &[p]);
    let first = batch(&mut h);
    assert_eq!(plain(&first), [(0, K::On(36, 100))]);
    set(&mut h, "mute", 1.0);
    let base = h.frame();
    let evs = batch(&mut h);
    assert_eq!(plain(&evs), [(base, K::Off(36))]);
    assert!(
        collect(&mut h, 400).is_empty(),
        "muted: nothing plays, nothing hangs"
    );
    set(&mut h, "mute", 0.0);
    let evs = collect(&mut h, 600);
    assert!(
        evs.iter().any(|e| matches!(e.k, K::On(36, 100))),
        "the clock kept running, the next pass plays"
    );
}

fn busy_pattern() -> DrumPattern {
    let mut hits = Vec::new();
    for step in 0..16 {
        if step % 4 == 0 {
            hits.push((0, step, 110));
        }
        if step % 8 == 4 {
            hits.push((2, step, 100));
        }
        if step % 2 == 0 {
            hits.push((6, step, 80));
        }
    }
    pat(16, &hits)
}

#[test]
fn density_zero_plays_exactly_what_is_written() {
    let mut h = rig(48_000.0, 120.0, &[busy_pattern()]);
    let evs = collect(&mut h, batches_for(32 * 3_840, 48_000, 120));
    let mut got = hits(&evs);
    got.retain(|(s, ..)| *s < 32);
    got.sort_unstable();
    let mut want = Vec::new();
    for bar in 0..2i64 {
        for step in 0..16i64 {
            if step % 4 == 0 {
                want.push((bar * 16 + step, 36, 110));
            }
            if step % 8 == 4 {
                want.push((bar * 16 + step, 38, 100));
            }
            if step % 2 == 0 {
                want.push((bar * 16 + step, 42, 80));
            }
        }
    }
    want.sort_unstable();
    assert_eq!(got, want);
}

#[test]
fn density_adds_only_on_empty_steps_and_removes_only_hits() {
    let run = |density: f32| {
        let mut h = rig(48_000.0, 120.0, &[busy_pattern()]);
        set(&mut h, "density", density);
        set(&mut h, "random.seed", 5.0);
        let evs = collect(&mut h, batches_for(64 * 3_840, 48_000, 120));
        let mut got = hits(&evs);
        got.retain(|(s, ..)| *s < 64);
        got
    };
    let base: std::collections::BTreeSet<(i64, u8)> =
        run(0.0).iter().map(|(s, k, _)| (*s, *k)).collect();
    let more = run(1.0);
    let more_set: std::collections::BTreeSet<(i64, u8)> =
        more.iter().map(|(s, k, _)| (*s, *k)).collect();
    assert!(base.is_subset(&more_set), "every written hit stays");
    assert!(more_set.len() > base.len() + 10, "and hits are added");
    for (s, k, v) in &more {
        if base.contains(&(*s, *k)) {
            let written = match *k {
                36 => 110,
                38 => 100,
                _ => 80,
            };
            assert_eq!(*v, written, "a written hit keeps its velocity");
        }
    }
    let fewer: std::collections::BTreeSet<(i64, u8)> =
        run(-1.0).iter().map(|(s, k, _)| (*s, *k)).collect();
    assert!(fewer.is_subset(&base), "only written hits remain");
    assert!(fewer.len() + 10 < base.len(), "and hits are removed");
    // The strong positions survive thinning more often than the weak ones: the kick on the
    // downbeat of the bar is never removed (weight 1).
    assert!(fewer.contains(&(0, 36)) && fewer.contains(&(16, 36)));
}

#[test]
fn variation_and_fill_are_pure_functions_of_the_position() {
    use super::algo::{Meter, Tuning, hit_velocity};
    let p = busy_pattern();
    let t = Tuning {
        density: 0.3,
        variation: 0.7,
        fill_every: 2,
        fill_amount: 0.5,
        accent: 0.4,
        vel_scale: 1.0,
        seed_mix: 99,
    };
    let m = Meter::new(4, 4);
    let g = Grid::DEFAULT;
    let all = |order: &[i64]| -> Vec<(i64, usize, u8)> {
        let mut v = Vec::new();
        for &k in order {
            for lane in 0..DRUM_LANES {
                v.push((k, lane, hit_velocity(&p, lane, k, g, &t, &m)));
            }
        }
        v.sort_unstable();
        v
    };
    let forward: Vec<i64> = (0..200).collect();
    let backward: Vec<i64> = (0..200).rev().collect();
    assert_eq!(
        all(&forward),
        all(&backward),
        "the order of asking never matters"
    );
    // A different seed gives a different bar.
    let t2 = Tuning { seed_mix: 100, ..t };
    let a: Vec<u8> = (0..200)
        .map(|k| hit_velocity(&p, 6, k, g, &t, &m))
        .collect();
    let b: Vec<u8> = (0..200)
        .map(|k| hit_velocity(&p, 6, k, g, &t2, &m))
        .collect();
    assert_ne!(a, b);
    // Fill bars are the odd ones (every 2 bars: the last of each pair), in their second half.
    let quiet = Tuning {
        density: 0.0,
        variation: 0.0,
        accent: 0.0,
        ..t
    };
    let snare = |bar: i64, step: i64| hit_velocity(&p, 2, bar * 16 + step, g, &quiet, &m);
    assert_eq!(snare(0, 9), 0, "bar 0 is not a fill bar");
    assert_eq!(snare(1, 7), 0, "the first half of bar 1 is left alone");
    assert!(snare(1, 9) > 0 && snare(1, 15) > snare(1, 9), "rising hits");
    // The kick hits on the last beat of a fill bar.
    assert!(hit_velocity(&p, 0, 16 + 12, g, &quiet, &m) > 0);
}

#[test]
fn variation_moves_hits_one_step_later_and_adds_ghosts_only_in_varied_bars() {
    let mut h = rig(48_000.0, 120.0, &[busy_pattern()]);
    set(&mut h, "variation", 1.0);
    let evs = collect(&mut h, batches_for(64 * 3_840, 48_000, 120));
    let got = hits(&evs);
    let snare_steps: Vec<i64> = got
        .iter()
        .filter(|(s, k, v)| *k == 38 && *s < 64 && *v > 90)
        .map(|(s, ..)| *s)
        .collect();
    assert!(
        snare_steps.iter().any(|s| s % 8 == 5),
        "a snare hit moved one step later: {snare_steps:?}"
    );
    let ghosts = got.iter().filter(|(_, k, v)| *k == 38 && *v < 60).count();
    assert!(ghosts > 0, "soft ghost notes on the snare lane");
    let kicks: Vec<i64> = got
        .iter()
        .filter(|(s, k, _)| *k == 36 && *s < 64)
        .map(|(s, ..)| *s)
        .collect();
    assert!(kicks.iter().all(|s| s % 4 == 0), "the kick never moves");
}

#[test]
fn locate_lands_on_the_same_hits_as_a_continuous_run() {
    let (sr, bpm, s) = (48_000u64, 120u64, 3_840u64);
    let configure = |h: &mut DeviceHarness| {
        set(h, "density", 0.4);
        set(h, "variation", 0.8);
        set(h, "fill.every", 2.0); // 2 bars
        set(h, "accent", 0.5);
        set(h, "random.seed", 11.0);
    };
    let mut cont = rig(sr as f64, bpm as f64, &[busy_pattern()]);
    configure(&mut cont);
    let all = hits(&collect(&mut cont, batches_for(160 * s, sr, bpm)));
    let from = 70i64;
    let mut located = rig(sr as f64, bpm as f64, &[busy_pattern()]);
    configure(&mut located);
    located.time_mut().tick = (from as u64 * s) as f64 - 100.0;
    let got = hits(&collect(&mut located, batches_for(60 * s, sr, bpm)));
    let want: Vec<_> = all
        .iter()
        .copied()
        .filter(|(st, ..)| *st >= from && *st < from + 55)
        .collect();
    let got: Vec<_> = got
        .into_iter()
        .filter(|(st, ..)| *st >= from && *st < from + 55)
        .collect();
    assert!(!want.is_empty());
    assert_eq!(got, want);
}

#[test]
fn fill_every_four_bars_follows_the_time_signature() {
    let (sr, bpm) = (48_000u64, 240u64);
    for (num, den) in [(4u8, 4u8), (3, 4), (6, 8)] {
        let bar = u64::from(num) * 61_440 / u64::from(den);
        // An empty pattern: the only snares are the fill's.
        let mut h = rig(sr as f64, bpm as f64, &[pat(16, &[])]);
        h.time_mut().time_signature = (num, den);
        set(&mut h, "fill.every", 3.0); // 4 bars
        set(&mut h, "fill.amount", 0.5);
        let evs = collect(&mut h, batches_for(9 * bar, sr, bpm));
        let mut steps: Vec<u64> = evs
            .iter()
            .filter(|e| matches!(e.k, K::On(38, _)))
            .map(|e| (e.tick / 3_840.0).round() as u64 * 3_840)
            .collect();
        steps.dedup();
        assert!(!steps.is_empty(), "{num}/{den}");
        // Every snare lies in the second half of bar 3 (counted from 0), 4 bars of this meter
        // after the first group, and nowhere else in the first 9 bars except bar 7.
        for t in &steps {
            let b = t / bar;
            let off = t % bar;
            assert!(b == 3 || b == 7, "{num}/{den}: tick {t} is in bar {b}");
            assert!(
                off as f64 >= bar as f64 / 2.0 - 3_840.0,
                "{num}/{den}: tick {t} is in the first half of its bar"
            );
        }
        assert!(steps.iter().any(|t| t / bar == 3), "{num}/{den}");
        assert!(steps.iter().any(|t| t / bar == 7), "{num}/{den}");
    }
}

#[test]
fn pattern_switch_at_end_waits_for_the_pass() {
    let (sr, bpm, s) = (48_000u64, 120u64, 3_840u64);
    let a = pat(4, &[(0, 0, 100), (0, 1, 100), (0, 2, 100), (0, 3, 100)]);
    let b = pat(4, &[(1, 0, 90), (1, 1, 90), (1, 2, 90), (1, 3, 90)]);
    let mut h = rig(sr as f64, bpm as f64, &[a, b]);
    // pattern.switch defaults to at-end.
    let mut evs = collect(&mut h, batches_for(s + 100, sr, bpm));
    set(&mut h, "pattern", 2.0);
    evs.extend(collect(&mut h, batches_for(8 * s, sr, bpm)));
    let h2 = hits(&evs);
    let first_b = h2.iter().find(|(_, k, _)| *k == 37).unwrap().0;
    assert_eq!(first_b, 4, "pattern 2 starts at the end of the first pass");
    assert!(
        h2.iter()
            .filter(|(st, ..)| *st < 4)
            .all(|(_, k, _)| *k == 36),
        "pattern 1 plays out its pass: {h2:?}"
    );
    // `now` switches at the next batch.
    let mut h = rig(sr as f64, bpm as f64, &[a, b]);
    set(&mut h, "pattern.switch", 0.0);
    let _ = collect(&mut h, batches_for(s + 100, sr, bpm));
    set(&mut h, "pattern", 2.0);
    let evs = collect(&mut h, batches_for(2 * s, sr, bpm));
    let first = hits(&evs).first().copied().unwrap();
    assert_eq!(first.1, 37, "now: the next step already plays pattern 2");
}

#[test]
fn data_swap_never_cuts_a_sounding_note() {
    let (sr, bpm, s) = (48_000u64, 120u64, 3_840u64);
    let p = pat(4, &[(0, 0, 100)]);
    let mut h = rig(sr as f64, bpm as f64, &[p]);
    set(&mut h, "gate", 1.0);
    let mut evs = collect(&mut h, 20);
    let old = h.device_mut().swap_player_data(Some(data(&[pat(4, &[])])));
    assert!(old.is_some(), "the previous data comes back");
    evs.extend(collect(&mut h, batches_for(2 * s, sr, bpm)));
    let want = [(0, K::On(36, 100)), (due(s, sr, bpm), K::Off(36))];
    assert_eq!(plain(&evs), want, "one on, one off, no double");
    // A swap to nothing while a hit sounds.
    let mut h = rig(sr as f64, bpm as f64, &[p]);
    set(&mut h, "gate", 1.0);
    let mut evs = collect(&mut h, 20);
    drop(h.device_mut().swap_player_data(None));
    evs.extend(collect(&mut h, 400));
    let offs = evs.iter().filter(|e| matches!(e.k, K::Off(_))).count();
    let ons = evs.iter().filter(|e| matches!(e.k, K::On(..))).count();
    assert_eq!((ons, offs), (1, 1));
}

#[test]
fn no_pattern_data_plays_nothing() {
    let mut h = DeviceHarness::with_options(Box::new(DrumGenerator::new()), 48_000.0, 120.0, 0);
    assert!(collect(&mut h, 200).is_empty());
}

#[test]
fn cv_outputs_follow_the_loudest_hit_of_a_step() {
    let p = pat(4, &[(0, 0, 60), (2, 0, 127)]);
    let mut h = rig(48_000.0, 120.0, &[p]);
    set(&mut h, "gate", 1.0);
    h.process();
    let gate = h.output("gate_cv_out").to_vec();
    assert!(gate.iter().all(|g| *g == 1.0), "gate high while hits sound");
    let vel = h.output("velocity_cv_out").to_vec();
    assert!(vel.iter().all(|v| *v == 1.0), "the loudest hit (127)");
    let pitch = h.output("pitch_cv_out").to_vec();
    assert_eq!(pitch[63], ((38.0f32 - 60.0) / 24.0).clamp(-1.0, 1.0));
    let trig = h.output("trig_cv_out").to_vec();
    assert_eq!(trig[0], 1.0);
}

#[test]
fn stop_closes_notes_and_start_resumes_in_the_grid() {
    let (sr, bpm, s) = (48_000u64, 120u64, 3_840u64);
    let p = pat(4, &[(0, 0, 100), (0, 1, 100), (0, 2, 100), (0, 3, 100)]);
    let mut h = rig(sr as f64, bpm as f64, &[p]);
    set(&mut h, "gate", 1.0);
    let evs = collect(&mut h, 10);
    assert_eq!(plain(&evs), [(0, K::On(36, 100))]);
    h.time_mut().playing = false;
    let evs = batch(&mut h);
    assert_eq!(plain(&evs), [(640, K::Off(36))]);
    assert!(collect(&mut h, 5).is_empty());
    h.time_mut().playing = true;
    h.time_mut().tick = (2 * s) as f64;
    let evs = batch(&mut h);
    assert_eq!(evs[0].k, K::On(36, 100));
}

#[test]
fn loop_wrap_inside_a_batch_closes_notes_at_the_wrap_and_restarts() {
    let p = pat(4, &[(0, 0, 100), (0, 1, 100), (0, 2, 100), (0, 3, 100)]);
    let mut h = rig(48_000.0, 120.0, &[p]);
    set(&mut h, "gate", 1.0);
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
        [(base + 20, K::Off(36)), (base + 20, K::On(36, 100))]
    );
    h.time_mut().tick = 44.0 * tpf;
    assert!(batch(&mut h).is_empty(), "no second jump");
}

#[test]
fn reset_closes_sounding_notes_with_an_all_off() {
    let p = pat(4, &[(0, 0, 100), (0, 1, 100)]);
    let mut h = rig(48_000.0, 120.0, &[p]);
    set(&mut h, "gate", 1.0);
    collect(&mut h, 3);
    h.device_mut().reset();
    let evs = batch(&mut h);
    assert!(
        plain(&evs).contains(&(h.frame() - BATCH as u64, K::AllOff)),
        "{evs:?}"
    );
    h.time_mut().tick = 0.0;
    let evs = collect(&mut h, 3);
    assert!(evs.iter().any(|e| matches!(e.k, K::On(..))));
}

#[test]
fn all_off_on_notes_in_releases_the_sounding_hits() {
    let p = pat(4, &[(0, 0, 100), (2, 0, 100)]);
    let mut h = rig(48_000.0, 120.0, &[p]);
    set(&mut h, "gate", 1.0);
    collect(&mut h, 3);
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
        plain(&evs),
        [(base + 7, K::Off(36)), (base + 7, K::Off(38))]
    );
}

#[test]
fn meter_values_publish_the_pattern_and_the_step() {
    let (sr, bpm, s) = (48_000u64, 120u64, 3_840u64);
    let a = pat(4, &[(0, 0, 100)]);
    let mut h = rig(sr as f64, bpm as f64, &[a, a]);
    assert!(h.device().has_meter_values());
    set(&mut h, "pattern", 2.0);
    for _ in 0..batches_for(2 * s + 100, sr, bpm) {
        h.process();
    }
    let mut out = [0.0f32; 8];
    let n = h.device().meter_values(&mut out);
    assert!(n >= 3);
    assert_eq!(out[0], 1.0, "0-based pattern index");
    assert_eq!(out[1], 2.0, "step index of the last started step");
    assert_eq!(out[2], 1.0, "running");
}

#[test]
fn bypass_passes_notes_through_and_stops_the_pattern() {
    let (sr, bpm, s) = (48_000u64, 120u64, 3_840u64);
    let p = pat(4, &[(0, 0, 100), (0, 1, 100), (0, 2, 100), (0, 3, 100)]);
    let mut h = rig(sr as f64, bpm as f64, &[p]);
    set(&mut h, "gate", 1.0);
    collect(&mut h, batches_for(s + 1_000, sr, bpm) - 3);
    set(&mut h, "bypass", 1.0);
    h.push_note("notes_in", on(5, 72, 0.8));
    let base = h.frame();
    let evs = batch(&mut h);
    assert_eq!(
        plain(&evs),
        [(base, K::Off(36)), (base + 5, K::On(72, 102))]
    );
    assert!(
        collect(&mut h, batches_for(4 * s, sr, bpm)).is_empty(),
        "the pattern stops while bypassed"
    );
    set(&mut h, "bypass", 0.0);
    h.push_note("notes_in", off(0, 72));
    let evs = collect(&mut h, batches_for(2 * s, sr, bpm));
    assert!(evs.iter().any(|e| e.k == K::Off(72)), "the thru key closes");
    assert!(
        evs.iter().any(|e| matches!(e.k, K::On(36, 100))),
        "and the pattern resumes"
    );
}

#[test]
fn role_weights_follow_the_roles_and_stay_in_range() {
    use super::algo::{Meter, Position, role_weight};
    let m = Meter::new(4, 4);
    assert_eq!(m.position(0), Position::Down);
    assert_eq!(m.position(15_360), Position::Back);
    assert_eq!(m.position(30_720), Position::Strong);
    assert_eq!(m.position(7_680), Position::Eighth);
    assert_eq!(m.position(3_840), Position::Sixteenth);
    assert_eq!(m.position(1_000), Position::Other);
    let w = |r, p, late| role_weight(r, p, late);
    assert!(w(DrumRole::Kick, Position::Down, 0.0) > w(DrumRole::Kick, Position::Back, 0.25));
    assert!(w(DrumRole::Snare, Position::Back, 0.25) > w(DrumRole::Snare, Position::Down, 0.0));
    assert!(w(DrumRole::Clap, Position::Back, 0.25) > w(DrumRole::Clap, Position::Eighth, 0.125));
    assert!(
        w(DrumRole::ClosedHat, Position::Eighth, 0.125)
            > w(DrumRole::ClosedHat, Position::Sixteenth, 0.0625)
    );
    assert!(
        w(DrumRole::TomLow, Position::Eighth, 0.9) > w(DrumRole::TomLow, Position::Eighth, 0.1),
        "toms favour the end of the bar"
    );
    for role in DrumRole::ALL {
        for pos in [
            Position::Down,
            Position::Strong,
            Position::Back,
            Position::Eighth,
            Position::Sixteenth,
            Position::Other,
        ] {
            for late in [0.0, 0.5, 0.99] {
                assert!((0.0..=1.0).contains(&w(role, pos, late)));
            }
        }
    }
    // An odd meter: 7/8 has a beat of an eighth; the bar is seven of them.
    let m = Meter::new(7, 8);
    assert_eq!((m.beat_ticks, m.bar_ticks), (7_680, 53_760));
    assert_eq!(m.locate(53_760 * 2 + 7_680), (2, 7_680));
    assert_eq!(Meter::new(0, 0), Meter::new(4, 4));
}

#[test]
fn generate_is_deterministic_for_a_seed_and_follows_the_style() {
    let base = DrumPattern::default();
    for style in DrumStyle::ALL {
        let a = generate_pattern(&base, style, 0.0, 3);
        assert_eq!(a, generate_pattern(&base, style, 0.0, 3), "{style:?}");
        let kick: Vec<usize> = (0..16).filter(|i| a.steps[0][*i].velocity > 0).collect();
        match style {
            DrumStyle::FourOnTheFloor => assert_eq!(kick, [0, 4, 8, 12]),
            DrumStyle::BrokenBeat => assert_eq!(kick, [0, 3, 6, 10, 13]),
            DrumStyle::HalfTime | DrumStyle::Straight => assert_eq!(kick, [0, 8, 10]),
        }
        assert!(
            a.steps
                .iter()
                .flatten()
                .all(|s| s.prob == 100 && s.velocity <= 127),
            "{style:?}"
        );
    }
    let a = generate_pattern(&base, DrumStyle::Straight, 0.0, 1);
    let b = generate_pattern(&base, DrumStyle::Straight, 0.0, 2);
    assert_ne!(a.steps, b.steps, "the seed jitters the velocities");
    let snare: Vec<usize> = (0..16).filter(|i| a.steps[2][*i].velocity > 0).collect();
    assert_eq!(snare, [4, 12]);
    assert_eq!(a.lanes, base.lanes);
    assert_eq!((a.length, a.swing), (base.length, base.swing));
}

#[test]
fn generate_density_adds_or_removes_hits_around_the_template() {
    let base = DrumPattern::default();
    let count = |p: &DrumPattern| p.steps.iter().flatten().filter(|s| s.velocity > 0).count();
    let plain = generate_pattern(&base, DrumStyle::Straight, 0.0, 9);
    let busy = generate_pattern(&base, DrumStyle::Straight, 1.0, 9);
    let thin = generate_pattern(&base, DrumStyle::Straight, -1.0, 9);
    assert!(count(&busy) > count(&plain) + 5);
    assert!(count(&thin) < count(&plain));
    // The template hits survive in the busy pattern.
    for lane in 0..16 {
        for step in 0..16 {
            if plain.steps[lane][step].velocity > 0 {
                assert!(busy.steps[lane][step].velocity > 0);
            }
        }
    }
    // A second lane of the same role gets no template (two snares do not double).
    let mut two = base;
    two.lanes[4].role = DrumRole::Snare;
    let g = generate_pattern(&two, DrumStyle::Straight, 0.0, 1);
    assert!(g.steps[4].iter().all(|s| s.velocity == 0));
    assert!(g.steps[2].iter().any(|s| s.velocity > 0));
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
    Density(i8),
    Gate(u8),
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
        2 => (-100i8..=100).prop_map(Act::Density),
        2 => (5u8..=100).prop_map(Act::Gate),
    ]
}

fn random_pattern(seed: u8, res: SeqResolution) -> DrumPattern {
    let mut p = DrumPattern {
        length: 5 + seed % 12,
        resolution: res,
        swing: ((u16::from(seed) * 17) % 76) as u8,
        ..DrumPattern::default()
    };
    // Two lanes share a key on purpose: the same key can be hit by two lanes.
    p.lanes[5].key = p.lanes[4].key;
    for lane in 0..16usize {
        for i in 0..64usize {
            if (i * 7 + lane * 3 + usize::from(seed)) % 3 == 0 {
                p.steps[lane][i] = DrumStep {
                    velocity: 1 + ((i * 13 + lane) % 127) as u8,
                    prob: if i % 4 == 0 { 60 } else { 100 },
                };
            }
        }
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
        set(&mut h, "variation", 0.6);
        set(&mut h, "fill.every", 1.0);
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
                Act::Density(d) => set(&mut h, "density", f32::from(d) / 100.0),
                Act::Gate(g) => set(&mut h, "gate", f32::from(g) / 100.0),
            }
            let evs = batch(&mut h);
            check_balance(&mut count, &evs);
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
        values in proptest::collection::vec(-1.0e6f32..1.0e6, 13),
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
