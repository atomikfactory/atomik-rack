//! Per-note expression in a song track: the curves of a note reach the track's input at their
//! frame after the note-on, only while the note sounds, are restored when a seek lands inside
//! the note, repeat identically at any block size, and play without allocating (the guard below
//! panics on any).
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::{ExprKind, NoteKind, PPQ, Ticks};
use atomik_engine::{
    SongExprPoint, SongExpression, SongNote, SongTrack, TransportCommand, compile_song,
};
use common::song::*;

const PLAY: TransportCommand = TransportCommand::Play;

fn locate(tick: i64) -> TransportCommand {
    TransportCommand::Locate(Ticks(tick))
}

fn x(note_tick: i64, key: u8, kind: ExprKind, points: &[(i64, f32)]) -> SongExpression {
    SongExpression {
        tick: Ticks(note_tick),
        key,
        kind,
        points: points
            .iter()
            .map(|&(t, v)| SongExprPoint {
                tick: Ticks(t),
                value: v,
            })
            .collect(),
    }
}

fn expr(key: u8, kind: ExprKind, value: f32) -> NoteKind {
    NoteKind::NoteExpr { key, kind, value }
}

fn on(key: u8) -> NoteKind {
    NoteKind::On { key, velocity: 0.5 }
}

fn play_one(
    notes: Vec<SongNote>,
    expression: Vec<SongExpression>,
    commands: &[TransportCommand],
    frames: usize,
    block: usize,
) -> Vec<(u64, NoteKind)> {
    let (mut e, mut h, logs) = note_logs(1, 0);
    let track = SongTrack {
        expression,
        ..strack(logs[0].0, notes)
    };
    h.submit_song(song(vec![track], &[(0, 120.0)])).unwrap();
    for c in commands {
        h.transport(*c).unwrap();
    }
    run(&mut e, frames, block);
    logs[0].1.events()
}

#[test]
fn the_points_follow_the_note_on_at_their_frames() {
    let ev = play_one(
        vec![snote(0, 2 * PPQ, 60)],
        vec![
            x(0, 60, ExprKind::Tuning, &[(0, 0.5), (PPQ, 2.0)]),
            x(0, 60, ExprKind::Pressure, &[(0, 0.25)]),
        ],
        &[PLAY],
        50_000,
        64,
    );
    // 24000 frames per quarter note at 120 BPM and 48 kHz. At one tick the note-on comes first,
    // then its expression (kinds in the order the curves were given).
    assert_eq!(
        ev,
        vec![
            (0, on(60)),
            (0, expr(60, ExprKind::Tuning, 0.5)),
            (0, expr(60, ExprKind::Pressure, 0.25)),
            (24_000, expr(60, ExprKind::Tuning, 2.0)),
            (48_000, NoteKind::Off { key: 60 }),
        ]
    );
}

#[test]
fn points_outside_the_note_and_curves_of_missing_notes_play_nothing() {
    let ev = play_one(
        vec![snote(0, PPQ, 60)],
        vec![
            // The second point is at the note's end: the note is off by then.
            x(
                0,
                60,
                ExprKind::Slide,
                &[(0, 0.5), (PPQ, 0.9), (2 * PPQ, 1.0)],
            ),
            // No note starts at 1 tick on key 60, none on key 61.
            x(1, 60, ExprKind::Tuning, &[(1, 1.0)]),
            x(0, 61, ExprKind::Tuning, &[(0, 1.0)]),
        ],
        &[PLAY],
        60_000,
        64,
    );
    assert_eq!(
        ev,
        vec![
            (0, on(60)),
            (0, expr(60, ExprKind::Slide, 0.5)),
            (24_000, NoteKind::Off { key: 60 }),
        ]
    );
}

#[test]
fn a_seek_into_a_note_restores_the_value_in_force() {
    let curves = vec![x(
        0,
        60,
        ExprKind::Tuning,
        &[(0, 1.0), (PPQ, 2.0), (3 * PPQ, 3.0)],
    )];
    let ev = play_one(
        vec![snote(0, 4 * PPQ, 60)],
        curves.clone(),
        &[locate(2 * PPQ), PLAY],
        30_000,
        64,
    );
    assert_eq!(
        ev,
        vec![
            (0, on(60)),
            (0, expr(60, ExprKind::Tuning, 2.0)),
            (24_000, expr(60, ExprKind::Tuning, 3.0)),
        ]
    );
    // Before the first point the curve holds its first value.
    let curves = vec![x(0, 60, ExprKind::Pressure, &[(PPQ, 0.5)])];
    let ev = play_one(
        vec![snote(0, 4 * PPQ, 60)],
        curves,
        &[locate(PPQ / 2), PLAY],
        30_000,
        64,
    );
    assert_eq!(
        ev,
        vec![
            (0, on(60)),
            (0, expr(60, ExprKind::Pressure, 0.5)),
            (12_000, expr(60, ExprKind::Pressure, 0.5)),
        ]
    );
}

#[test]
fn a_track_without_expression_plays_exactly_as_before() {
    let notes = vec![snote(0, PPQ, 60), snote(PPQ, PPQ, 62)];
    let ev = play_one(notes, Vec::new(), &[PLAY], 60_000, 64);
    assert!(
        ev.iter()
            .all(|(_, k)| !matches!(k, NoteKind::NoteExpr { .. }))
    );
    assert_eq!(ev.len(), 4);
}

#[test]
fn the_events_do_not_depend_on_the_block_size() {
    let notes = vec![snote(0, 3 * PPQ, 60), snote(PPQ, 2 * PPQ, 64)];
    let curves = vec![
        x(
            0,
            60,
            ExprKind::Tuning,
            &[(0, 0.0), (PPQ / 3, 1.0), (PPQ, -1.0), (2 * PPQ, 0.5)],
        ),
        x(PPQ, 64, ExprKind::Pressure, &[(PPQ, 0.1), (PPQ + 777, 0.9)]),
    ];
    let a = play_one(notes.clone(), curves.clone(), &[PLAY], 120_000, 64);
    let b = play_one(notes.clone(), curves.clone(), &[PLAY], 120_000, 512);
    let c = play_one(notes, curves, &[PLAY], 120_000, 100);
    assert_eq!(a, b);
    assert_eq!(a, c);
    assert!(
        a.iter()
            .any(|(_, k)| matches!(k, NoteKind::NoteExpr { .. }))
    );
}

#[test]
fn a_bad_curve_is_refused() {
    let (_e, _h, logs) = note_logs(1, 0);
    let bad = SongTrack {
        expression: vec![x(0, 60, ExprKind::Pressure, &[(0, 1.5)])],
        ..strack(logs[0].0, vec![snote(0, PPQ, 60)])
    };
    assert!(compile_song(&song_input(vec![bad], &[(0, 120.0)])).is_err());
}
