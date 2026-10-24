//! Helpers for song tests: song builders, an engine with routed `NoteLog`s, and a
//! stuck-note check.

use super::{cfg, engine_with, render_blocks};
use atomik_core::{NoteKind, Tempo, Ticks};
use atomik_engine::{
    DeviceInstanceId, Engine, EngineHandle, Graph, SongInput, SongNote, SongSnapshot, SongTempo,
    SongTrack, compile_song,
};
use atomik_testkit::fakes::{NoteEvents, NoteLog};
use std::collections::BTreeSet;

/// Sample rate of [`cfg`].
pub const SR: f64 = 48_000.0;

/// A tempo map from `(tick, bpm)` pairs.
pub fn tempo(map: &[(i64, f64)]) -> Vec<SongTempo> {
    map.iter()
        .map(|&(tick, bpm)| SongTempo {
            tick: Ticks(tick),
            tempo: Tempo::new(bpm).unwrap(),
            ramp: false,
        })
        .collect()
}

/// A note with velocity 0.5.
pub fn snote(tick: i64, length: i64, key: u8) -> SongNote {
    SongNote {
        tick: Ticks(tick),
        length,
        key,
        velocity: 0.5,
    }
}

/// A track into `device`'s `NoteLog::NOTES_IN` (socket 0, like most Notes inputs here).
pub fn strack(device: DeviceInstanceId, notes: Vec<SongNote>) -> SongTrack {
    SongTrack {
        device,
        notes_socket: NoteLog::NOTES_IN,
        chase: true,
        notes,
        controllers: Vec::new(),
        automation: Vec::new(),
    }
}

/// Like [`strack`] with note chase off: notes spanning a play position stay silent.
pub fn strack_plain(device: DeviceInstanceId, notes: Vec<SongNote>) -> SongTrack {
    SongTrack {
        chase: false,
        ..strack(device, notes)
    }
}

/// Song input in 4/4.
pub fn song_input(tracks: Vec<SongTrack>, map: &[(i64, f64)]) -> SongInput {
    SongInput {
        tracks,
        tempo: tempo(map),
        meter: vec![atomik_core::MeterEvent::new(1, 4, 4)],
        audio: Vec::new(),
        assets: Vec::new(),
    }
}

/// A compiled song in 4/4.
pub fn song(tracks: Vec<SongTrack>, map: &[(i64, f64)]) -> SongSnapshot {
    compile_song(&song_input(tracks, map)).unwrap()
}

/// An engine (2 outputs, `threads` workers) whose plan holds `n` routed `NoteLog`s with ids
/// `1..=n`, and their logs.
pub fn note_logs(
    n: usize,
    threads: u16,
) -> (Engine, EngineHandle, Vec<(DeviceInstanceId, NoteEvents)>) {
    let c = cfg(0, 2);
    let mut g = Graph::new(&c);
    let logs = (0..n)
        .map(|_| {
            let (dev, log) = NoteLog::boxed(200_000);
            (g.add_device(dev), log)
        })
        .collect();
    let (engine, mut handle) = engine_with(c, threads, None);
    // Control messages apply in queue order, so a song and transport commands queued next
    // land in the same first batch, after the plan.
    handle.submit_plan(g.compile().unwrap()).unwrap();
    (engine, handle, logs)
}

/// Renders `frames` frames (allocation-guarded) in blocks of `block`, discarding the output.
pub fn run(engine: &mut Engine, frames: usize, block: usize) {
    if frames > 0 {
        render_blocks(engine, &[], 2, frames, block);
    }
}

/// Keys still held after replaying `events` (On adds, Off removes, AllOff clears).
pub fn stuck(events: &[(u64, NoteKind)]) -> BTreeSet<u8> {
    let mut held = BTreeSet::new();
    for (_, kind) in events {
        match *kind {
            NoteKind::On { key, .. } => {
                held.insert(key);
            }
            NoteKind::Off { key } => {
                held.remove(&key);
            }
            NoteKind::AllOff => held.clear(),
            // Channel messages and expressions never hold a key.
            _ => {}
        }
    }
    held
}

/// `On { key, velocity: 0.5 }`.
pub fn on(key: u8) -> NoteKind {
    NoteKind::On { key, velocity: 0.5 }
}

/// `Off { key }`.
pub fn off(key: u8) -> NoteKind {
    NoteKind::Off { key }
}

/// A tempo map from `(tick, bpm, ramp)` triples.
pub fn tempo_ramps(map: &[(i64, f64, bool)]) -> Vec<SongTempo> {
    map.iter()
        .map(|&(tick, bpm, ramp)| SongTempo {
            tick: Ticks(tick),
            tempo: Tempo::new(bpm).unwrap(),
            ramp,
        })
        .collect()
}
