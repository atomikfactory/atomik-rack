//! Song notes land at the exact frame from any locate position and through loop
//! wraps, with no stuck notes. The engine's output (two `NoteLog` tracks) is compared event
//! for event against an independent per-sample oracle that walks the song timeline one frame
//! at a time; every render is allocation-guarded.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::{NoteKind, PPQ, Ticks};
use atomik_engine::{HotEdit, SongNote, SongSnapshot, TransportCommand};
use common::song::*;
use proptest::prelude::*;
use std::collections::BTreeSet;

/// One scripted transport command at an engine frame.
#[derive(Debug, Clone)]
struct Cmd {
    frame: u64,
    command: TransportCommand,
}

/// A flattened event of one track, as the oracle sees it.
#[derive(Debug, Clone, Copy)]
struct Ev {
    tick: i64,
    kind: NoteKind,
}

/// Flattens non-overlapping notes (distinct keys) and sorts them like the compiler does:
/// by (tick, offs before ons, key).
fn flatten(notes: &[SongNote]) -> Vec<Ev> {
    let mut ev: Vec<Ev> = notes
        .iter()
        .flat_map(|n| {
            [
                Ev {
                    tick: n.tick.0,
                    kind: on(n.key),
                },
                Ev {
                    tick: n.tick.0 + n.length,
                    kind: off(n.key),
                },
            ]
        })
        .collect();
    ev.sort_by_key(|e| match e.kind {
        NoteKind::Off { key } => (e.tick, 0, key),
        NoteKind::On { key, .. } => (e.tick, 1, key),
        _ => unreachable!(),
    });
    ev
}

/// Walks the timeline sample by sample from engine frame 0 to `end` and returns, per track,
/// the expected `(engine frame, event)` list. Commands apply at the first 64-frame boundary at
/// or after their frame, in order.
fn oracle(
    snap: &SongSnapshot,
    tracks: &[Vec<Ev>],
    cmds: &[Cmd],
    end: u64,
) -> Vec<Vec<(u64, NoteKind)>> {
    let f = |tick: i64| snap.song_frame(Ticks(tick), SR);
    let mut out = vec![Vec::new(); tracks.len()];
    let mut held: Vec<BTreeSet<u8>> = vec![BTreeSet::new(); tracks.len()];
    let (mut playing, mut pos) = (false, 0u64);
    let mut lp: Option<(u64, u64)> = None;
    let mut next = 0;
    for e in 0..end {
        if e % 64 == 0 {
            while let Some(c) = cmds.get(next).filter(|c| c.frame <= e) {
                next += 1;
                match c.command {
                    TransportCommand::Play => playing = true,
                    TransportCommand::Stop | TransportCommand::Locate(_) if playing => {
                        for (t, h) in held.iter_mut().enumerate() {
                            out[t].push((e, NoteKind::AllOff));
                            h.clear();
                        }
                        if let TransportCommand::Locate(t) = c.command {
                            pos = f(t.0.max(0));
                        } else {
                            playing = false;
                        }
                    }
                    TransportCommand::Locate(t) => pos = f(t.0.max(0)),
                    TransportCommand::Stop => {}
                    TransportCommand::SetLoop {
                        start,
                        end,
                        enabled,
                    } => {
                        let (ls, le) = (f(start.0), f(end.0));
                        lp = (enabled && le > ls).then_some((ls, le));
                    }
                    TransportCommand::Metronome { .. } => {}
                }
            }
        }
        if !playing {
            continue;
        }
        if let Some((ls, le)) = lp
            && pos == le
        {
            for (t, h) in held.iter_mut().enumerate() {
                for key in std::mem::take(h) {
                    out[t].push((e, off(key)));
                }
            }
            pos = ls;
        }
        for (t, events) in tracks.iter().enumerate() {
            for ev in events.iter().filter(|ev| f(ev.tick) == pos) {
                match ev.kind {
                    NoteKind::On { key, .. } => {
                        held[t].insert(key);
                        out[t].push((e, ev.kind));
                    }
                    NoteKind::Off { key } => {
                        if held[t].remove(&key) {
                            out[t].push((e, ev.kind));
                        }
                    }
                    _ => unreachable!(),
                }
            }
        }
        pos += 1;
    }
    out
}

/// Plays `tracks` (one per `NoteLog`) under `map` with the scripted commands; returns the logs.
fn play(
    notes: &[Vec<SongNote>],
    map: &[(i64, f64)],
    cmds: &[Cmd],
    end: u64,
    block: usize,
    threads: u16,
) -> (SongSnapshot, Vec<Vec<(u64, NoteKind)>>) {
    let (mut e, mut h, logs) = note_logs(notes.len(), threads);
    let tracks = logs
        .iter()
        .zip(notes)
        .map(|((dev, _), n)| strack(*dev, n.clone()))
        .collect();
    let snap = song(tracks, map);
    h.submit_song(snap.clone()).unwrap();
    let mut done = 0u64;
    for c in cmds {
        // Queue each command just before the host block containing its frame.
        let upto = c.frame.max(done);
        run(&mut e, (upto - done) as usize, block);
        done = upto;
        h.schedule(c.frame, HotEdit::Transport(c.command)).unwrap();
    }
    run(&mut e, (end - done) as usize, block);
    (snap, logs.iter().map(|(_, l)| l.events()).collect())
}

fn check(notes: &[Vec<SongNote>], map: &[(i64, f64)], cmds: &[Cmd], end: u64, block: usize) {
    let (snap, got) = play(notes, map, cmds, end, block, 0);
    let flat: Vec<Vec<Ev>> = notes.iter().map(|n| flatten(n)).collect();
    let want = oracle(&snap, &flat, cmds, end);
    for (t, (g, w)) in got.iter().zip(&want).enumerate() {
        assert_eq!(g, w, "track {t}, block {block}");
        assert!(
            stuck(g).is_empty()
                || cmds
                    .last()
                    .is_none_or(|c| c.command != TransportCommand::Stop),
            "track {t}: stuck {:?}",
            stuck(g)
        );
    }
}

fn cmd(frame: u64, command: TransportCommand) -> Cmd {
    Cmd { frame, command }
}

fn looping(start: i64, end: i64) -> TransportCommand {
    TransportCommand::SetLoop {
        start: Ticks(start),
        end: Ticks(end),
        enabled: true,
    }
}

#[test]
fn note_exactly_at_the_loop_end_never_sounds_and_a_zero_remainder_wraps_at_offset_0() {
    // Loop [0, PPQ) = song frames [0, 24000): 24000 is a batch boundary (zero remainder).
    let notes = vec![vec![
        snote(PPQ, 100, 60),         // at the loop end: never inside the loop
        snote(PPQ / 2, PPQ / 2, 61), // its off coincides with the loop end
        snote(0, 64, 62),            // at the loop start
    ]];
    let cmds = [cmd(0, looping(0, PPQ)), cmd(0, TransportCommand::Play)];
    let (snap, got) = play(&notes, &[(0, 120.0)], &cmds, 80_000, 64, 0);
    assert_eq!(snap.song_frame(Ticks(PPQ), SR) % 64, 0);
    let mut want = Vec::new();
    for lap in 0..4u64 {
        let base = lap * 24_000;
        if lap > 0 {
            want.push((base, off(61))); // released once, at the wrap (no second off)
        }
        want.extend([
            (base, on(62)),
            (base + 100, off(62)),
            (base + 12_000, on(61)),
        ]);
    }
    want.retain(|(f, _)| *f < 80_000);
    assert_eq!(got[0], want);
    check(&notes, &[(0, 120.0)], &cmds, 80_000, 333);
}

#[test]
fn tempo_change_inside_the_loop_moves_the_loop_end_and_the_notes() {
    // 120 BPM to PPQ / 2, then 60 BPM: the loop [0, PPQ) ends at song frame 36000 and a note
    // at 3/4 beat sounds at 24000.
    let map = [(0, 120.0), (PPQ / 2, 60.0)];
    let notes = vec![vec![snote(3 * PPQ / 4, PPQ, 60)]];
    let cmds = [cmd(0, looping(0, PPQ)), cmd(0, TransportCommand::Play)];
    let (snap, got) = play(&notes, &map, &cmds, 100_000, 64, 0);
    assert_eq!(snap.song_frame(Ticks(PPQ), SR), 36_000);
    assert_eq!(
        got[0],
        vec![
            (24_000, on(60)),
            (36_000, off(60)),
            (60_000, on(60)),
            (72_000, off(60)),
            (96_000, on(60)),
        ]
    );
    check(&notes, &map, &cmds, 100_000, 7);
}

#[test]
fn loops_shorter_than_a_batch_wrap_several_times_per_batch() {
    // Loop [0, 10 ticks) = 16 frames; the note covers frames [0, 8).
    let notes = vec![vec![snote(0, 5, 60)]];
    let cmds = [cmd(0, looping(0, 10)), cmd(0, TransportCommand::Play)];
    let (_, got) = play(&notes, &[(0, 120.0)], &cmds, 640, 64, 0);
    let want: Vec<_> = (0..40u64)
        .flat_map(|k| [(16 * k, on(60)), (16 * k + 8, off(60))])
        .collect();
    assert_eq!(got[0], want);
    check(&notes, &[(0, 120.0)], &cmds, 640, 1);
}

#[test]
fn locate_inside_a_note_skips_it_and_never_sends_its_off() {
    let notes = vec![vec![snote(0, PPQ, 60), snote(PPQ / 4, PPQ, 61)]];
    let cmds = [
        cmd(0, TransportCommand::Locate(Ticks(PPQ / 2))),
        cmd(100, TransportCommand::Play),
        cmd(30_000, TransportCommand::Stop),
    ];
    let (_, got) = play(&notes, &[(0, 120.0)], &cmds, 31_000, 64, 0);
    assert_eq!(got[0], vec![(30_016, NoteKind::AllOff)]);
    check(&notes, &[(0, 120.0)], &cmds, 31_000, 64);
}

fn arb_notes() -> impl Strategy<Value = Vec<SongNote>> {
    // Distinct keys per track, so the oracle needs no overlap handling.
    proptest::collection::vec((0..2 * PPQ, 1..PPQ), 0..6).prop_map(|v| {
        v.into_iter()
            .enumerate()
            .map(|(k, (tick, len))| snote(tick, len, 40 + k as u8))
            .collect()
    })
}

fn arb_map() -> impl Strategy<Value = Vec<(i64, f64)>> {
    proptest::collection::vec((1..2 * PPQ, 60.0..400.0f64), 0..3).prop_flat_map(|changes| {
        (120.0..400.0f64).prop_map(move |first| {
            let mut map = vec![(0, first)];
            let mut ticks: Vec<_> = changes.clone();
            ticks.sort_by_key(|c| c.0);
            ticks.dedup_by_key(|c| c.0);
            map.extend(ticks);
            map
        })
    })
}

fn arb_cmds() -> impl Strategy<Value = Vec<Cmd>> {
    (
        0..2 * PPQ,        // first locate
        0u64..500,         // play frame
        any::<bool>(),     // loop on
        0..2 * PPQ,        // loop start
        1..PPQ,            // loop length
        0..2 * PPQ,        // second locate
        2_000u64..12_000,  // second locate frame (after play)
        12_000u64..20_000, // stop frame
    )
        .prop_map(|(l1, p, lp, ls, len, l2, f2, stop)| {
            let mut v = vec![cmd(0, TransportCommand::Locate(Ticks(l1)))];
            if lp {
                v.push(cmd(0, looping(ls, ls + len)));
            }
            v.push(cmd(p, TransportCommand::Play));
            v.push(cmd(p + f2, TransportCommand::Locate(Ticks(l2))));
            v.push(cmd(p + stop, TransportCommand::Stop));
            v
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]
    /// Any tempo map, notes, locate positions and loop, at any host block size: every
    /// event lands on the oracle's frame and nothing is left held after Stop.
    #[test]
    fn ac_s3_notes_land_exactly_from_any_locate_and_through_loop_wraps(
        a in arb_notes(),
        b in arb_notes(),
        map in arb_map(),
        cmds in arb_cmds(),
        block in prop_oneof![Just(1usize), Just(7), Just(64), Just(100), Just(333)],
    ) {
        let end = cmds.last().unwrap().frame + 1_000;
        check(&[a, b], &map, &cmds, end, block);
    }
}
