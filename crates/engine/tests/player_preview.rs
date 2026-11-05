//! The player preview: a note player runs on a clock of its own, at the song tempo, while the song
//! transport stands still. Notes come out of the previewing player alone, the transport does not
//! move, stopping closes what is held, and the song transport starting ends it. Every render runs
//! under the allocation guard (`render_block`), and the clock is the engine's own frame count, so
//! the frames below are exact for any host block size.

atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::NoteKind;
use atomik_device_sdk::{PatternData, PlayerData, PlayerDataRef, StepData, StepSeqData};
use atomik_devices::step_sequencer::StepSequencer;
use atomik_engine::{DeviceInstanceId, Graph, TransportCommand};
use atomik_testkit::fakes::{NoteEvents, NoteLog};
use common::song::song;
use common::*;
use std::sync::Arc;

/// The `notes_out` socket of a player.
const NOTES_OUT: atomik_device_sdk::SocketId = atomik_device_sdk::SocketId(1);

/// A 1/16 step is 3840 ticks, which is 6000 frames at 120 BPM and 48 kHz.
const STEP_FRAMES: u64 = 6000;

/// Sixteen steps on key 60 with the gate at half a step.
fn data() -> PlayerDataRef {
    let mut d = StepSeqData::default();
    let mut pattern = PatternData::default();
    for s in pattern.steps.iter_mut().take(16) {
        *s = StepData {
            on: true,
            key: 60,
            velocity: 100,
            gate: 50,
            prob: 100,
            tie: false,
        };
    }
    d.patterns[0] = pattern;
    Arc::new(PlayerData::StepSeq(d))
}

/// Two step sequencers, each feeding its own note log.
struct Rig {
    engine: atomik_engine::Engine,
    handle: atomik_engine::EngineHandle,
    seqs: [DeviceInstanceId; 2],
    logs: [NoteEvents; 2],
}

fn rig(threads: u16) -> Rig {
    let c = cfg(0, 2);
    let mut g = Graph::new(&c);
    let mut seqs = Vec::new();
    let mut logs = Vec::new();
    for _ in 0..2 {
        let seq = g.add_device(Box::new(StepSequencer::new()));
        g.set_device_player_data(seq, Some(data())).unwrap();
        let (log, view) = NoteLog::boxed(256);
        let log = g.add_device(log);
        g.connect(ep(seq, NOTES_OUT), ep(log, NoteLog::NOTES_IN))
            .unwrap();
        seqs.push(seq);
        logs.push(view);
    }
    let (engine, mut handle) = engine_with(c, threads, None);
    handle.submit_plan(g.compile().unwrap()).unwrap();
    // A song (with no tracks) is loaded and stopped: without one the transport counts as playing.
    handle.submit_song(song(Vec::new(), &[(0, 120.0)])).unwrap();
    Rig {
        engine,
        handle,
        seqs: [seqs[0], seqs[1]],
        logs: [logs.remove(0), logs.remove(0)],
    }
}

fn run(r: &mut Rig, frames: usize, block: usize) {
    render_blocks(&mut r.engine, &[], 2, frames, block);
}

fn ons(log: &NoteEvents) -> Vec<u64> {
    log.events()
        .into_iter()
        .filter(|(_, k)| matches!(k, NoteKind::On { .. }))
        .map(|(f, _)| f)
        .collect()
}

fn offs(log: &NoteEvents) -> Vec<u64> {
    log.events()
        .into_iter()
        .filter(|(_, k)| matches!(k, NoteKind::Off { .. }))
        .map(|(f, _)| f)
        .collect()
}

#[test]
fn a_stopped_song_plays_nothing_until_a_player_previews() {
    for threads in [0, 2] {
        let mut r = rig(threads);
        run(&mut r, 30_000, 64);
        assert!(r.logs[0].events().is_empty(), "{threads}");
        assert!(r.logs[1].events().is_empty(), "{threads}");
    }
}

#[test]
fn a_previewing_player_plays_from_its_pattern_start_at_the_song_tempo_for_any_block_size() {
    for block in [1, 64, 333, 4096] {
        for threads in [0, 2] {
            let mut r = rig(threads);
            r.handle.preview_player(r.seqs[0], true).unwrap();
            run(&mut r, 3 * STEP_FRAMES as usize, block);
            let got = ons(&r.logs[0]);
            assert_eq!(
                got,
                vec![0, STEP_FRAMES, 2 * STEP_FRAMES],
                "{block}/{threads}"
            );
            // The other player and the transport are untouched.
            assert!(r.logs[1].events().is_empty(), "{block}/{threads}");
            let state = r.handle.transport_state();
            assert!(!state.playing && state.tick == 0, "{block}/{threads}");
        }
    }
}

#[test]
fn a_preview_started_later_starts_at_the_pattern_start_too() {
    let mut r = rig(0);
    run(&mut r, 6400, 64);
    r.handle.preview_player(r.seqs[0], true).unwrap();
    run(&mut r, 2 * STEP_FRAMES as usize, 64);
    // The message lands at the batch boundary at frame 6400; the first step is there.
    assert_eq!(ons(&r.logs[0]), vec![6400, 6400 + STEP_FRAMES]);
}

#[test]
fn stopping_a_preview_closes_the_held_note_at_the_next_batch_for_any_block_size() {
    for block in [64, 333] {
        let mut r = rig(0);
        r.handle.preview_player(r.seqs[0], true).unwrap();
        // The first note sounds at 0 and its gate would end at 3000: stop at 1280 (20 batches).
        run(&mut r, 1280, 64);
        assert!(offs(&r.logs[0]).is_empty());
        r.handle.preview_player(r.seqs[0], false).unwrap();
        run(&mut r, 4 * STEP_FRAMES as usize, block);
        assert_eq!(offs(&r.logs[0]), vec![1280], "{block}: closed at once");
        assert_eq!(
            ons(&r.logs[0]),
            vec![0],
            "{block}: nothing plays afterwards"
        );
    }
}

#[test]
fn starting_a_preview_again_restarts_the_pattern() {
    let mut r = rig(0);
    r.handle.preview_player(r.seqs[0], true).unwrap();
    run(&mut r, 1280, 64);
    r.handle.preview_player(r.seqs[0], false).unwrap();
    run(&mut r, 1280, 64);
    r.handle.preview_player(r.seqs[0], true).unwrap();
    run(&mut r, 640, 64);
    assert_eq!(ons(&r.logs[0]), vec![0, 2560]);
}

#[test]
fn the_song_transport_starting_ends_the_preview_and_stopping_it_stays_silent() {
    let mut r = rig(0);
    r.handle.preview_player(r.seqs[0], true).unwrap();
    run(&mut r, 1280, 64);
    assert!(r.logs[1].events().is_empty());
    r.handle.transport(TransportCommand::Play).unwrap();
    run(&mut r, 6400, 64);
    // Playing, both players follow the song clock, which started at tick 0 at frame 1280.
    let first = ons(&r.logs[1]);
    assert_eq!(first, vec![1280, 1280 + STEP_FRAMES]);
    assert!(
        ons(&r.logs[0]).ends_with(&first),
        "the preview gave way to the song"
    );
    r.handle.transport(TransportCommand::Stop).unwrap();
    run(&mut r, 64, 64);
    let (before0, before1) = (r.logs[0].events().len(), r.logs[1].events().len());
    run(&mut r, 3 * STEP_FRAMES as usize, 64);
    // The song stands still again: the preview does not come back and nothing else plays.
    assert_eq!(r.logs[0].events().len(), before0);
    assert_eq!(r.logs[1].events().len(), before1);
}

#[test]
fn previews_of_two_players_run_independently_and_a_removed_device_is_ignored() {
    let mut r = rig(0);
    r.handle.preview_player(r.seqs[0], true).unwrap();
    r.handle.preview_player(r.seqs[1], true).unwrap();
    r.handle
        .preview_player(DeviceInstanceId(9999), true)
        .unwrap();
    run(&mut r, 2 * STEP_FRAMES as usize, 64);
    assert_eq!(ons(&r.logs[0]), ons(&r.logs[1]));
    assert_eq!(ons(&r.logs[0]), vec![0, STEP_FRAMES]);
}
