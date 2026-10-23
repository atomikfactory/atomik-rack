//! Note chase: notes that started before the play position sound from the position on play,
//! locate while playing and loop wrap, a snapshot swap keeps the keys a changed track still
//! covers (without retriggering), and a track with chase off plays as before. Every render is
//! allocation-guarded; the chase must not depend on the host block size.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::{NoteKind, PPQ, Ticks};
use atomik_engine::{DeviceInstanceId, SongNote, SongTrack, TransportCommand};
use common::song::*;
use common::*;

const PLAY: TransportCommand = TransportCommand::Play;

/// 64 ticks = exactly 100 frames at 120 BPM and 48 kHz (a quarter note is 24000 frames).
const U: i64 = 64;

fn locate(tick: i64) -> TransportCommand {
    TransportCommand::Locate(Ticks(tick))
}

fn looping(start: i64, end: i64) -> TransportCommand {
    TransportCommand::SetLoop {
        start: Ticks(start),
        end: Ticks(end),
        enabled: true,
    }
}

/// A note with an explicit velocity.
fn vnote(tick: i64, length: i64, key: u8, velocity: f32) -> SongNote {
    SongNote {
        tick: Ticks(tick),
        length,
        key,
        velocity,
    }
}

fn on_v(key: u8, velocity: f32) -> NoteKind {
    NoteKind::On { key, velocity }
}

type Maker = fn(DeviceInstanceId, Vec<SongNote>) -> SongTrack;

/// One track on a fresh engine; the commands are queued before the first render, so they all
/// land in the first batch. Renders `frames` frames and returns the track's events.
fn play_one(
    make: Maker,
    notes: Vec<SongNote>,
    commands: &[TransportCommand],
    frames: usize,
) -> Vec<(u64, NoteKind)> {
    let (mut e, mut h, logs) = note_logs(1, 0);
    h.submit_song(song(vec![make(logs[0].0, notes)], &[(0, 120.0)]))
        .unwrap();
    for c in commands {
        h.transport(*c).unwrap();
    }
    run(&mut e, frames, 64);
    logs[0].1.events()
}

#[test]
fn play_inside_a_note_starts_it_at_frame_zero_with_its_velocity() {
    let notes = vec![vnote(PPQ, 2 * PPQ, 60, 0.8)];
    let ev = play_one(strack, notes, &[locate(3 * PPQ / 2), PLAY], 40_064);
    // The note ends at tick 3 PPQ: 36000 frames after the play position.
    assert_eq!(ev, vec![(0, on_v(60, 0.8)), (36_000, off(60))]);
}

#[test]
fn play_exactly_at_a_note_start_plays_it_once_and_at_its_end_plays_nothing() {
    let notes = vec![vnote(PPQ, PPQ, 60, 0.8)];
    let ev = play_one(strack, notes.clone(), &[locate(PPQ), PLAY], 30_016);
    assert_eq!(ev, vec![(0, on_v(60, 0.8)), (24_000, off(60))]);
    let ev = play_one(strack, notes, &[locate(2 * PPQ), PLAY], 30_016);
    assert_eq!(ev, vec![]);
}

#[test]
fn play_inside_several_notes_chases_them_in_ascending_key_order() {
    let notes = vec![
        vnote(0, 4 * PPQ, 67, 0.3),
        vnote(0, 4 * PPQ, 60, 0.5),
        vnote(100, 4 * PPQ, 64, 0.9),
        vnote(PPQ, PPQ, 72, 0.4), // starts after the position
    ];
    let ev = play_one(strack, notes, &[locate(PPQ / 2), PLAY], 100);
    assert_eq!(
        ev,
        vec![(0, on_v(60, 0.5)), (0, on_v(64, 0.9)), (0, on_v(67, 0.3))]
    );
}

#[test]
fn locate_while_playing_into_a_note_sends_all_off_then_the_chased_on() {
    let notes = vec![vnote(PPQ, 2 * PPQ, 60, 0.8)];
    let (mut e, mut h, logs) = note_logs(1, 0);
    h.submit_song(song(vec![strack(logs[0].0, notes)], &[(0, 120.0)]))
        .unwrap();
    h.transport(PLAY).unwrap();
    run(&mut e, 6_400, 64);
    h.transport(locate(3 * PPQ / 2)).unwrap();
    run(&mut e, 40_000, 64);
    assert_eq!(
        logs[0].1.events(),
        vec![
            (6_400, NoteKind::AllOff),
            (6_400, on_v(60, 0.8)),
            (6_400 + 36_000, off(60)),
        ]
    );
}

#[test]
fn locate_while_stopped_only_seeks_and_play_chases() {
    let notes = vec![vnote(0, 4 * PPQ, 60, 0.8)];
    let (mut e, mut h, logs) = note_logs(1, 0);
    h.submit_song(song(vec![strack(logs[0].0, notes)], &[(0, 120.0)]))
        .unwrap();
    h.transport(locate(PPQ)).unwrap();
    run(&mut e, 640, 64);
    assert_eq!(logs[0].1.events(), vec![]);
    h.transport(PLAY).unwrap();
    run(&mut e, 64, 64);
    assert_eq!(logs[0].1.events(), vec![(640, on_v(60, 0.8))]);
}

#[test]
fn loop_wrap_on_a_batch_boundary_chases_the_note_spanning_the_loop_start() {
    // Loop [PPQ, 2 PPQ) = frames [24000, 48000); 48000 = 750 batches: the wrap is pending at
    // offset 0 of the next batch.
    let notes = vec![vnote(0, 3 * PPQ, 60, 0.8)];
    let ev = play_one(strack, notes, &[looping(PPQ, 2 * PPQ), PLAY], 48_064);
    assert_eq!(
        ev,
        vec![
            (0, on_v(60, 0.8)),
            (48_000, off(60)),
            (48_000, on_v(60, 0.8)),
        ]
    );
}

#[test]
fn loop_wrap_inside_a_batch_chases_at_the_wrap_offset() {
    // Loop end at frame 48250, inside a batch. The note at the loop start plays from the event
    // list (not chased).
    let notes = vec![vnote(0, 3 * PPQ, 60, 0.8), vnote(PPQ, 25 * U, 62, 0.4)];
    let ev = play_one(strack, notes, &[looping(PPQ, 2 * PPQ + 160), PLAY], 48_320);
    assert_eq!(
        ev,
        vec![
            (0, on_v(60, 0.8)),
            (24_000, on_v(62, 0.4)),
            (26_500, off(62)),
            (48_250, off(60)),
            (48_250, on_v(60, 0.8)),
            (48_250, on_v(62, 0.4)),
        ]
    );
}

#[test]
fn loop_shorter_than_a_batch_chases_at_every_wrap() {
    // Loop [25 U, 25.5 U) = frames [2500, 2550): 50 frames, several wraps per batch.
    let notes = vec![vnote(0, PPQ, 60, 0.8)];
    let ev = play_one(
        strack,
        notes,
        &[looping(25 * U, 25 * U + U / 2), PLAY],
        2_944,
    );
    assert_eq!(ev[0], (0, on_v(60, 0.8)));
    let wraps: Vec<_> = ev[1..].chunks(2).collect();
    assert!(wraps.len() >= 7, "{ev:?}");
    for (i, w) in wraps.iter().enumerate() {
        let at = 2_550 + 50 * i as u64;
        assert_eq!(w, &[(at, off(60)), (at, on_v(60, 0.8))], "wrap {i}");
    }
}

#[test]
fn chase_off_track_keeps_a_spanning_note_silent_everywhere() {
    let notes = vec![vnote(PPQ, 2 * PPQ, 60, 0.8)];
    let ev = play_one(strack_plain, notes, &[locate(3 * PPQ / 2), PLAY], 40_064);
    assert_eq!(ev, vec![]);
    // Loop wrap: the note started before the loop start, only its release comes.
    let notes = vec![vnote(0, 3 * PPQ, 60, 0.8)];
    let ev = play_one(strack_plain, notes, &[looping(PPQ, 2 * PPQ), PLAY], 48_064);
    assert_eq!(ev, vec![(0, on_v(60, 0.8)), (48_000, off(60))]);
}

#[test]
fn overlapping_notes_of_one_key_chase_only_the_surviving_one() {
    // The first note is cut where the second starts (10 U).
    let notes = vec![vnote(0, 20 * U, 60, 0.3), vnote(10 * U, 20 * U, 60, 0.9)];
    let ev = play_one(strack, notes.clone(), &[locate(14 * U), PLAY], 50_048);
    assert_eq!(ev, vec![(0, on_v(60, 0.9)), (1_600, off(60))]);
    let ev = play_one(strack, notes, &[locate(6 * U), PLAY], 50_048);
    assert_eq!(
        ev,
        vec![
            (0, on_v(60, 0.3)),
            (400, off(60)),
            (400, on_v(60, 0.9)),
            (2_400, off(60)),
        ]
    );
}

/// Plays `first`, swaps in `second` (built with `make`) at frame 30016 (a batch boundary) and
/// renders on; returns the track's events.
fn swap_run(make: Maker, first: Vec<SongNote>, second: Vec<SongNote>) -> Vec<(u64, NoteKind)> {
    let (mut e, mut h, logs) = note_logs(1, 0);
    let d = logs[0].0;
    h.submit_song(song(vec![strack(d, first)], &[(0, 120.0)]))
        .unwrap();
    h.transport(PLAY).unwrap();
    run(&mut e, 30_016, 64);
    h.submit_song(song(vec![make(d, second)], &[(0, 120.0)]))
        .unwrap();
    run(&mut e, 100_032 - 30_016, 64);
    logs[0].1.events()
}

#[test]
fn swap_of_a_changed_track_keeps_covered_keys_and_releases_the_others() {
    let first = vec![vnote(0, 4 * PPQ, 60, 0.5), vnote(0, 4 * PPQ, 62, 0.5)];
    // Key 62 is gone; key 64 is new under the playhead (start < position < end): not started.
    let second = vec![vnote(0, 4 * PPQ, 60, 0.5), vnote(1000, 2000, 64, 0.5)];
    for make in [strack as Maker, strack_plain] {
        let ev = swap_run(make, first.clone(), second.clone());
        assert_eq!(
            ev,
            vec![
                (0, on(60)),
                (0, on(62)),
                (30_016, off(62)),
                (96_000, off(60)),
            ]
        );
        assert!(stuck(&ev).is_empty());
    }
}

#[test]
fn swap_of_a_changed_track_releases_a_key_whose_note_now_ends_before_the_position() {
    let first = vec![vnote(0, 4 * PPQ, 60, 0.5)];
    let second = vec![vnote(0, PPQ, 60, 0.5)];
    let ev = swap_run(strack, first, second);
    assert_eq!(ev, vec![(0, on(60)), (30_016, off(60))]);
}

#[test]
fn swap_of_an_unchanged_track_keeps_its_held_set() {
    let first = vec![vnote(0, 4 * PPQ, 60, 0.5)];
    let ev = swap_run(strack, first.clone(), first);
    assert_eq!(ev, vec![(0, on(60)), (96_000, off(60))]);
}

#[test]
fn swap_toggling_chase_keeps_the_covered_key_sounding() {
    // Chase on -> off with the same notes: a changed track, but the covered key still sounds.
    let first = vec![vnote(0, 4 * PPQ, 60, 0.5)];
    let ev = swap_run(strack_plain, first.clone(), first);
    assert_eq!(ev, vec![(0, on(60)), (96_000, off(60))]);
}

/// A scenario mixing play, a loop and locates while playing, rendered with host blocks of
/// `block` frames; every command lands at a multiple of 64. Returns each device's events.
fn scenario(block: usize) -> Vec<Vec<(u64, NoteKind)>> {
    let (mut e, mut h, logs) = note_logs(2, 0);
    let (d1, d2) = (logs[0].0, logs[1].0);
    let a = vec![
        vnote(0, 6 * PPQ, 60, 0.5),
        vnote(100, 3 * PPQ, 64, 0.7),
        vnote(2 * PPQ, PPQ, 67, 0.9),
    ];
    let b = vec![vnote(PPQ, 5 * PPQ, 48, 0.6), vnote(3 * PPQ, PPQ, 52, 0.4)];
    h.submit_song(song(
        vec![strack(d1, a), strack_plain(d2, b)],
        &[(0, 120.0)],
    ))
    .unwrap();
    h.transport(looping(PPQ, 3 * PPQ + 7)).unwrap();
    h.transport(locate(PPQ + 33)).unwrap();
    h.transport(PLAY).unwrap();
    render_blocks(&mut e, &[], 2, 12_800, block);
    h.transport(locate(2 * PPQ + 500)).unwrap();
    render_blocks(&mut e, &[], 2, 12_800, block);
    h.transport(locate(10)).unwrap();
    render_blocks(&mut e, &[], 2, 76_800, block);
    logs.iter().map(|l| l.1.events()).collect()
}

#[test]
fn chase_events_are_independent_of_the_host_block_size() {
    let base = scenario(64);
    let ons = base[0]
        .iter()
        .filter(|(_, k)| matches!(k, NoteKind::On { .. }))
        .count();
    assert!(ons > 6, "{base:?}");
    for block in [1, 63, 512, 4096] {
        assert_eq!(scenario(block), base, "block {block}");
    }
}
