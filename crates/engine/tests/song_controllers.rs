//! Controller lanes in a song track: pitch bend, aftertouch and CC events reach the track's input
//! at their frame, ordered with the notes, restored on a seek, reset on stop, repeatable at any
//! host block size, and rendered without allocating (the guard below panics on any).
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::{NoteKind, PPQ, Ticks};
use atomik_engine::{
    SongController, SongControllerKind as K, SongCurve, SongNote, SongPoint, SongTrack,
    TransportCommand,
};
use common::song::*;
use common::*;

const PLAY: TransportCommand = TransportCommand::Play;

fn locate(tick: i64) -> TransportCommand {
    TransportCommand::Locate(Ticks(tick))
}

fn pt(tick: i64, value: f32, curve: SongCurve) -> SongPoint {
    SongPoint {
        tick: Ticks(tick),
        value,
        curve,
    }
}

fn step(tick: i64, value: f32) -> SongPoint {
    pt(tick, value, SongCurve::Step)
}

fn cc(controller: u8, value: f32) -> NoteKind {
    NoteKind::Cc { controller, value }
}

fn bend(value: f32) -> NoteKind {
    NoteKind::PitchBend { value }
}

fn lane(kind: K, points: Vec<SongPoint>) -> SongController {
    SongController { kind, points }
}

/// One track on a fresh engine; the commands are queued before the first render. Returns the
/// track's events after `frames` frames.
fn play_one(
    notes: Vec<SongNote>,
    controllers: Vec<SongController>,
    commands: &[TransportCommand],
    frames: usize,
) -> Vec<(u64, NoteKind)> {
    let (mut e, mut h, logs) = note_logs(1, 0);
    let track = SongTrack {
        controllers,
        ..strack(logs[0].0, notes)
    };
    h.submit_song(song(vec![track], &[(0, 120.0)])).unwrap();
    for c in commands {
        h.transport(*c).unwrap();
    }
    run(&mut e, frames, 64);
    logs[0].1.events()
}

#[test]
fn events_arrive_at_their_frame_with_the_value_of_their_point() {
    let ev = play_one(
        vec![],
        vec![
            lane(K::Cc(1), vec![step(PPQ, 0.5), step(2 * PPQ, 1.0)]),
            lane(K::ChannelPressure, vec![step(PPQ / 2, 0.25)]),
            lane(K::PolyPressure(60), vec![step(PPQ, 0.75)]),
        ],
        &[PLAY],
        60_000,
    );
    // 24000 frames per quarter note at 120 BPM and 48 kHz. Play at tick 0 zeroes the aftertouch
    // lanes (they have no value yet); the CC has none to restore and sends nothing.
    assert_eq!(
        ev,
        vec![
            (0, NoteKind::ChannelPressure { value: 0.0 }),
            (
                0,
                NoteKind::PolyPressure {
                    key: 60,
                    value: 0.0
                }
            ),
            (12_000, NoteKind::ChannelPressure { value: 0.25 }),
            (24_000, cc(1, 0.5)),
            (
                24_000,
                NoteKind::PolyPressure {
                    key: 60,
                    value: 0.75
                }
            ),
            (48_000, cc(1, 1.0)),
        ]
    );
}

#[test]
fn a_ramp_sends_rounded_steps_in_time_order() {
    let ev = play_one(
        vec![],
        vec![lane(
            K::Cc(7),
            vec![pt(0, 0.0, SongCurve::Linear), step(PPQ, 1.0)],
        )],
        &[PLAY],
        26_000,
    );
    assert!(ev.len() > 100 && ev.len() <= 128, "{}", ev.len());
    assert!(ev.windows(2).all(|w| w[0].0 < w[1].0), "ascending frames");
    let values: Vec<f32> = ev
        .iter()
        .map(|(_, k)| match k {
            NoteKind::Cc { value, .. } => *value,
            other => panic!("{other:?}"),
        })
        .collect();
    assert!(values.windows(2).all(|w| w[0] < w[1]));
    assert_eq!(*values.last().unwrap(), 1.0);
}

#[test]
fn at_one_tick_the_off_comes_first_then_the_controller_then_the_on() {
    let note = |tick, length, key| SongNote {
        tick: Ticks(tick),
        length,
        key,
        velocity: 0.5,
    };
    let ev = play_one(
        vec![note(0, PPQ, 60), note(PPQ, PPQ, 62)],
        vec![lane(K::PitchBend, vec![step(PPQ, 0.5)])],
        &[PLAY],
        50_000,
    );
    let at: Vec<NoteKind> = ev
        .iter()
        .filter(|(f, _)| *f == 24_000)
        .map(|(_, k)| *k)
        .collect();
    assert_eq!(
        at,
        vec![
            NoteKind::Off { key: 60 },
            bend(0.5),
            NoteKind::On {
                key: 62,
                velocity: 0.5
            }
        ]
    );
}

#[test]
fn play_from_the_middle_restores_the_value_set_before_the_position() {
    let lanes = vec![
        lane(
            K::Cc(1),
            vec![step(0, 0.25), step(PPQ, 0.5), step(3 * PPQ, 1.0)],
        ),
        lane(K::Cc(7), vec![step(3 * PPQ, 0.9)]),
        lane(K::PitchBend, vec![step(3 * PPQ, 0.5)]),
    ];
    // Play at 2 PPQ (lanes in kind order): pitch bend has no value yet and is zeroed, Cc1 is
    // 0.5 there, Cc7 has none (nothing sent). Then the later points play.
    let ev = play_one(vec![], lanes.clone(), &[locate(2 * PPQ), PLAY], 30_000);
    assert_eq!(
        ev,
        vec![
            (0, bend(0.0)),
            (0, cc(1, 0.5)),
            (24_000, bend(0.5)),
            (24_000, cc(1, 1.0)),
            (24_000, cc(7, 0.9)),
        ]
    );
    // A point exactly at the play position is played by the walk, not chased.
    let ev = play_one(vec![], lanes, &[locate(3 * PPQ), PLAY], 100);
    assert_eq!(
        ev,
        vec![
            (0, bend(0.0)),
            (0, cc(1, 0.5)),
            (0, bend(0.5)),
            (0, cc(1, 1.0)),
            (0, cc(7, 0.9)),
        ]
    );
}

#[test]
fn a_track_with_note_chase_off_still_chases_its_controllers() {
    let (mut e, mut h, logs) = note_logs(1, 0);
    let track = SongTrack {
        controllers: vec![lane(K::Cc(11), vec![step(0, 0.5)])],
        ..strack_plain(logs[0].0, vec![])
    };
    h.submit_song(song(vec![track], &[(0, 120.0)])).unwrap();
    h.transport(locate(PPQ)).unwrap();
    h.transport(PLAY).unwrap();
    run(&mut e, 64, 64);
    assert_eq!(logs[0].1.events(), vec![(0, cc(11, 0.5))]);
}

#[test]
fn stop_and_locate_while_playing_zero_the_resetting_lanes_only() {
    let lanes = vec![
        lane(K::PitchBend, vec![step(0, 0.5)]),
        lane(K::ChannelPressure, vec![step(0, 0.5)]),
        lane(K::Cc(64), vec![step(0, 1.0)]),
        lane(K::Cc(1), vec![step(0, 0.5)]),
    ];
    let (mut e, mut h, logs) = note_logs(1, 0);
    let track = SongTrack {
        controllers: lanes,
        ..strack(logs[0].0, vec![])
    };
    h.submit_song(song(vec![track], &[(0, 120.0)])).unwrap();
    h.transport(PLAY).unwrap();
    run(&mut e, 640, 64);
    h.transport(TransportCommand::Stop).unwrap();
    run(&mut e, 640, 64);
    let ev = logs[0].1.events();
    let after_stop: Vec<(u64, NoteKind)> = ev.iter().copied().filter(|(f, _)| *f >= 640).collect();
    assert_eq!(
        after_stop,
        vec![
            (640, NoteKind::AllOff),
            (640, bend(0.0)),
            (640, NoteKind::ChannelPressure { value: 0.0 }),
            (640, cc(64, 0.0)),
        ]
    );
}

#[test]
fn a_loop_wrap_restores_the_controllers_at_the_loop_start() {
    let (mut e, mut h, logs) = note_logs(1, 0);
    let track = SongTrack {
        controllers: vec![lane(
            K::Cc(1),
            vec![step(0, 0.25), step(PPQ + PPQ / 2, 0.75)],
        )],
        ..strack(logs[0].0, vec![])
    };
    h.submit_song(song(vec![track], &[(0, 120.0)])).unwrap();
    // Loop [PPQ, 2 PPQ): the lane is 0.25 at the loop start, then 0.75 half way through.
    h.transport(TransportCommand::SetLoop {
        start: Ticks(PPQ),
        end: Ticks(2 * PPQ),
        enabled: true,
    })
    .unwrap();
    h.transport(locate(PPQ)).unwrap();
    h.transport(PLAY).unwrap();
    run(&mut e, 60_032, 64);
    let ev = logs[0].1.events();
    assert_eq!(
        ev,
        vec![
            (0, cc(1, 0.25)),
            (12_000, cc(1, 0.75)),
            (24_000, cc(1, 0.25)),
            (36_000, cc(1, 0.75)),
            (48_000, cc(1, 0.25)),
            (60_000, cc(1, 0.75)),
        ]
    );
}

#[test]
fn bad_lanes_are_refused_at_compile_time() {
    let tr = |controllers| SongTrack {
        controllers,
        ..strack(atomik_engine::DeviceInstanceId(1), vec![])
    };
    let bad = [
        vec![lane(K::Cc(1), vec![step(-1, 0.5)])],
        vec![lane(K::Cc(1), vec![step(0, 2.0)])],
        vec![lane(K::PitchBend, vec![step(0, f32::NAN)])],
        vec![
            lane(K::Cc(1), vec![step(0, 0.5)]),
            lane(K::Cc(1), vec![step(5, 0.5)]),
        ],
    ];
    for (i, c) in bad.into_iter().enumerate() {
        let input = song_input(vec![tr(c)], &[(0, 120.0)]);
        assert!(atomik_engine::compile_song(&input).is_err(), "case {i}");
    }
}

/// Play, locates while playing and a loop with bend and CC lanes, at several block sizes.
fn scenario(block: usize) -> Vec<(u64, NoteKind)> {
    let (mut e, mut h, logs) = note_logs(1, 0);
    let track = SongTrack {
        controllers: vec![
            lane(
                K::PitchBend,
                vec![pt(0, -1.0, SongCurve::Linear), step(2 * PPQ, 1.0)],
            ),
            lane(K::Cc(64), vec![step(PPQ, 1.0), step(3 * PPQ, 0.0)]),
        ],
        ..strack(logs[0].0, vec![])
    };
    h.submit_song(song(vec![track], &[(0, 120.0)])).unwrap();
    h.transport(TransportCommand::SetLoop {
        start: Ticks(PPQ),
        end: Ticks(3 * PPQ + 7),
        enabled: true,
    })
    .unwrap();
    h.transport(locate(PPQ + 33)).unwrap();
    h.transport(PLAY).unwrap();
    render_blocks(&mut e, &[], 2, 12_800, block);
    h.transport(locate(2 * PPQ + 500)).unwrap();
    render_blocks(&mut e, &[], 2, 12_800, block);
    h.transport(locate(10)).unwrap();
    render_blocks(&mut e, &[], 2, 76_800, block);
    logs[0].1.events()
}

#[test]
fn controller_events_are_independent_of_the_host_block_size() {
    let base = scenario(64);
    assert!(base.len() > 300, "{}", base.len());
    for block in [1, 63, 512, 4096] {
        assert_eq!(scenario(block), base, "block {block}");
    }
}
