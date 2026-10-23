//! Tempo ramps and meter changes in the engine: notes land at the frames of the closed form,
//! the metronome follows the meter map and a ramp keeps the same frames at any host block.
//! The oracle is the log formula written out here, independent of `atomik_core`.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::{MeterEvent, NoteKind, PPQ};
use atomik_engine::{SongInput, TransportCommand, compile_song};
use common::song::*;
use common::*;

const BAR: i64 = 4 * PPQ;

/// A tempo map: `(tick, bpm, ramp)`.
type Map = [(i64, f64, bool)];

/// Seconds at `tick`: each segment either holds its BPM or ramps linearly to the next one,
/// `60 / PPQ * L / (b1 - b0) * ln(b(x) / b0)` for a ramp over `L` ticks.
fn seconds(map: &Map, tick: i64) -> f64 {
    let k = 60.0 / PPQ as f64;
    let mut total = 0.0;
    for (i, &(t0, b0, ramp)) in map.iter().enumerate() {
        let end = map.get(i + 1).map_or(i64::MAX, |n| n.0);
        let upto = tick.min(end);
        if upto <= t0 {
            break;
        }
        let x = (upto - t0) as f64;
        total += match map.get(i + 1) {
            Some(&(_, b1, _)) if ramp && (b1 - b0).abs() > 0.0 => {
                let len = (end - t0) as f64;
                k * len / (b1 - b0) * ((b0 + (b1 - b0) * x / len) / b0).ln()
            }
            _ => k * x / b0,
        };
    }
    total
}

fn frame(map: &Map, tick: i64) -> u64 {
    (seconds(map, tick) * SR).round() as u64
}

fn input(tracks: Vec<atomik_engine::SongTrack>, map: &Map) -> SongInput {
    let mut i = song_input(tracks, &[(0, 120.0)]);
    i.tempo = tempo_ramps(map);
    i
}

const RAMPS: [(i64, f64, bool); 4] = [
    (0, 60.0, true),
    (2 * BAR, 180.0, false),
    (3 * BAR, 100.0, true),
    (4 * BAR, 40.0, false),
];

/// Notes of key 60, 100 ticks long, every half beat plus an odd offset over five bars.
fn notes() -> Vec<atomik_engine::SongNote> {
    (0..=5 * 8)
        .map(|k| snote(k * PPQ / 2 + (k * 37) % 91, 100, 60))
        .collect()
}

fn play(map: &Map, block: usize) -> Vec<(u64, NoteKind)> {
    let (mut e, mut h, logs) = note_logs(1, 0);
    let tracks = vec![strack(logs[0].0, notes())];
    h.submit_song(compile_song(&input(tracks, map)).unwrap())
        .unwrap();
    h.transport(TransportCommand::Play).unwrap();
    let end = frame(map, 5 * BAR + PPQ) as usize;
    run(&mut e, end, block);
    logs[0].1.events()
}

#[test]
fn notes_across_ramps_land_on_the_closed_form_frames() {
    let events = play(&RAMPS, 64);
    let expected: Vec<(u64, NoteKind)> = notes()
        .iter()
        .flat_map(|n| {
            [
                (frame(&RAMPS, n.tick.0), on(60)),
                (frame(&RAMPS, n.tick.0 + 100), off(60)),
            ]
        })
        .collect();
    assert_eq!(events.len(), expected.len());
    assert_eq!(events, expected);
}

#[test]
fn ramped_notes_do_not_depend_on_the_host_block() {
    let reference = play(&RAMPS, 64);
    for block in [1, 63, 333, 4096] {
        assert_eq!(play(&RAMPS, block), reference, "block {block}");
    }
}

#[test]
fn a_ramp_and_the_step_to_the_same_tempo_agree_after_it() {
    // Both maps are 120 BPM from bar 2 on; the ramp only changes bar 1.
    let ramp = [(0, 60.0, true), (BAR, 120.0, false)];
    let step = [(0, 60.0, false), (BAR, 120.0, false)];
    let (a, b) = (play(&ramp, 64), play(&step, 64));
    // The ramp makes bar 1 shorter than the 60 BPM step, by the same amount for later notes.
    let shift = frame(&step, BAR) as i64 - frame(&ramp, BAR) as i64;
    assert!(shift > 0);
    let late = |ev: &[(u64, NoteKind)]| ev.iter().rev().take(20).map(|e| e.0).collect::<Vec<_>>();
    for (x, y) in late(&a).iter().zip(late(&b)) {
        assert!((y as i64 - *x as i64 - shift).abs() <= 1, "{x} {y} {shift}");
    }
}

/// The outputs of an engine with only the metronome sounding (no plan, outputs 1/2).
fn clicks(meter: &[MeterEvent], frames: usize, block: usize) -> Vec<f32> {
    let (mut e, mut h) = new_engine_pair();
    let mut i = song_input(Vec::new(), &[(0, 120.0)]);
    i.meter = meter.to_vec();
    h.submit_song(compile_song(&i).unwrap()).unwrap();
    h.transport(TransportCommand::Metronome {
        enabled: true,
        gain: 0.5,
    })
    .unwrap();
    h.transport(TransportCommand::Play).unwrap();
    render_blocks(&mut e, &[], 3, frames, block).remove(0)
}

fn new_engine_pair() -> (atomik_engine::Engine, atomik_engine::EngineHandle) {
    engine_with(cfg(0, 3), 0, None)
}

/// `(onset frame, peak of the next 1000 frames)` of every click: an onset follows 200 silent
/// frames.
fn onsets(x: &[f32]) -> Vec<(usize, f32)> {
    let mut out = Vec::new();
    let mut quiet = 200;
    for (n, s) in x.iter().enumerate() {
        if s.abs() > 1e-6 {
            if quiet >= 200 {
                let peak = x[n..(n + 1000).min(x.len())]
                    .iter()
                    .fold(0.0f32, |m, v| m.max(v.abs()));
                out.push((n, peak));
            }
            quiet = 0;
        } else {
            quiet += 1;
        }
    }
    out
}

#[test]
fn the_metronome_follows_the_meter_map_with_an_accent_on_every_bar_line() {
    // Bar 1: 3/4 (three quarters), bar 2: 6/8 (six eighths), then 4/4.
    let meter = [
        MeterEvent::new(1, 3, 4),
        MeterEvent::new(2, 6, 8),
        MeterEvent::new(3, 4, 4),
    ];
    let beats: Vec<(i64, bool)> = {
        let mut v = vec![(0, true), (PPQ, false), (2 * PPQ, false), (3 * PPQ, true)];
        v.extend((1..6).map(|k| (3 * PPQ + k * PPQ / 2, false)));
        v.extend([(6 * PPQ, true), (7 * PPQ, false)]);
        v
    };
    let end = frame(&[(0, 120.0, false)], 8 * PPQ) as usize;
    let out = clicks(&meter, end, 64);
    let found = onsets(&out);
    assert_eq!(found.len(), beats.len(), "{found:?}");
    let accent = found.iter().map(|c| c.1).fold(0.0f32, f32::max);
    for ((tick, downbeat), (at, peak)) in beats.iter().zip(&found) {
        let want = frame(&[(0, 120.0, false)], *tick) as usize;
        // The output has 64 frames of latency; the first audible click sample is one later.
        assert!(
            (*at as i64 - 64 - want as i64).abs() <= 2,
            "{tick}: {at} {want}"
        );
        assert_eq!(*peak > 0.9 * accent, *downbeat, "tick {tick}: {peak}");
    }
}

#[test]
fn a_four_four_map_clicks_like_before() {
    let out = clicks(&[MeterEvent::new(1, 4, 4)], 4 * 24_000 + 1_000, 512);
    let found = onsets(&out);
    assert_eq!(found.len(), 5);
    let accent = found.iter().map(|c| c.1).fold(0.0f32, f32::max);
    for (k, (at, peak)) in found.iter().enumerate() {
        assert!((*at as i64 - 64 - (k * 24_000) as i64).abs() <= 2);
        assert_eq!(*peak > 0.9 * accent, k % 4 == 0);
    }
}
