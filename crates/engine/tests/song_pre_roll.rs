//! The pre-roll (count-in): the position holds while the click counts, nothing of the song
//! plays or is published as playing until it ends, the song then starts from the held position
//! (loop semantics included), and a Stop or a Play cancels it. Every render is
//! allocation-guarded.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::{MeterEvent, NoteKind, PPQ, Tempo, Ticks};
use atomik_engine::{PreRoll, SongSnapshot, SongTempo, TransportCommand, compile_song, new_engine};
use common::song::*;
use common::*;

const BAR: i64 = 4 * PPQ;
/// Two bars of 4/4 at 120 BPM, 48 kHz.
const TWO_BARS: u64 = 192_000;

fn click_on(enabled: bool) -> TransportCommand {
    TransportCommand::Metronome { enabled, gain: 0.5 }
}

fn pre_roll(bars: u8) -> PreRoll {
    song(Vec::new(), &[(0, 120.0)])
        .pre_roll(Ticks(0), bars, SR)
        .unwrap()
}

/// Frame indices where a click starts: the first non-zero sample after at least 2000 silent
/// frames.
fn onsets(out: &[f32]) -> Vec<usize> {
    let mut found = Vec::new();
    let mut silent = usize::MAX / 2;
    for (i, s) in out.iter().enumerate() {
        if *s == 0.0 {
            silent += 1;
        } else {
            if silent >= 2_000 {
                found.push(i);
            }
            silent = 0;
        }
    }
    found
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, s| m.max(s.abs()))
}

fn count_ons(events: &[(u64, NoteKind)]) -> Vec<u64> {
    events
        .iter()
        .filter(|(_, k)| matches!(k, NoteKind::On { .. }))
        .map(|(f, _)| *f)
        .collect()
}

#[test]
fn the_plan_is_a_pure_function_of_the_maps_and_the_sample_rate() {
    assert_eq!(
        pre_roll(2),
        PreRoll {
            frames: TWO_BARS,
            beats: 8,
            per_bar: 4
        }
    );
    assert_eq!(pre_roll(4).frames, 2 * TWO_BARS);
    assert_eq!(pre_roll(1).onset(3), 72_000);
    assert!(
        song(Vec::new(), &[(0, 120.0)])
            .pre_roll(Ticks(0), 0, SR)
            .is_none()
    );
    // 3/4 from bar 2: the count follows the meter at the start position.
    let mut input = song_input(Vec::new(), &[(0, 120.0)]);
    input.meter = vec![MeterEvent::new(1, 4, 4), MeterEvent::new(2, 3, 4)];
    let snap = compile_song(&input).unwrap();
    let plan = snap.pre_roll(Ticks(BAR), 1, SR).unwrap();
    assert_eq!((plan.beats, plan.per_bar, plan.frames), (3, 3, 72_000));
    assert_eq!(snap.pre_roll(Ticks(BAR), 1, SR), Some(plan));
    // A whole number of batches whatever the tempo.
    let odd = song(Vec::new(), &[(0, 97.3)])
        .pre_roll(Ticks(0), 3, 44_100.0)
        .unwrap();
    assert_eq!(odd.frames % 64, 0);
    assert!(odd.frames > 0);
}

#[test]
fn a_ramp_counts_at_the_tempo_of_the_start_position() {
    let mut input = song_input(Vec::new(), &[(0, 120.0)]);
    input.tempo = vec![
        SongTempo {
            tick: Ticks(0),
            tempo: Tempo::new(120.0).unwrap(),
            ramp: true,
        },
        SongTempo {
            tick: Ticks(2 * BAR),
            tempo: Tempo::new(240.0).unwrap(),
            ramp: false,
        },
    ];
    let snap: SongSnapshot = compile_song(&input).unwrap();
    assert_eq!(snap.pre_roll(Ticks(0), 2, SR).unwrap().frames, TWO_BARS);
    let mid = snap.pre_roll(Ticks(BAR), 2, SR).unwrap();
    assert!(
        mid.frames < TWO_BARS && mid.frames.is_multiple_of(64),
        "{mid:?}"
    );
    assert_eq!(snap.pre_roll(Ticks(BAR), 2, SR), Some(mid));
}

#[test]
fn two_bars_give_eight_clicks_then_the_song_starts_at_the_held_position() {
    let (mut e, mut h, logs) = note_logs(1, 0);
    let (dev, log) = &logs[0];
    let snap = song(vec![strack(*dev, vec![snote(0, PPQ, 60)])], &[(0, 120.0)]);
    let plan = snap.pre_roll(Ticks(0), 2, SR).unwrap();
    h.submit_song(snap).unwrap();
    // The count sounds with the metronome off.
    h.transport(click_on(false)).unwrap();
    h.transport(TransportCommand::PlayWithPreRoll(plan))
        .unwrap();
    let out = render_blocks(&mut e, &[], 2, TWO_BARS as usize + 64 + 60_000, 64);
    let starts = onsets(&out[0]);
    assert_eq!(starts.len(), 8, "{starts:?}");
    let lead = starts[0];
    for (i, at) in starts.iter().enumerate() {
        assert_eq!(*at, lead + i * 24_000);
        let p = peak(&out[0][*at..*at + 1_000]);
        let want = if i % 4 == 0 { 0.5 } else { 0.3 };
        assert!((p - want).abs() < 0.07, "click {i}: {p}");
    }
    assert_eq!(
        peak(&out[1][lead..lead + 1_000]),
        peak(&out[0][lead..lead + 1_000])
    );
    // Nothing of the song before the count ends; the note starts right after it.
    assert_eq!(log.events().first().map(|e| e.0), Some(TWO_BARS));
    // Metronome off: no click after the count.
    assert_eq!(peak(&out[0][lead + 7 * 24_000 + 2_000..]), 0.0);
}

#[test]
fn the_state_shows_the_count_and_the_song_start() {
    let (mut e, mut h, _logs) = note_logs(1, 0);
    let snap = song(Vec::new(), &[(0, 120.0)]);
    let plan = snap.pre_roll(Ticks(0), 1, SR).unwrap();
    h.submit_song(snap).unwrap();
    h.transport(TransportCommand::PlayWithPreRoll(plan))
        .unwrap();
    run(&mut e, 64, 64);
    let s = h.transport_state();
    assert!(!s.playing);
    assert_eq!(
        (s.pre_roll_frames, s.pre_roll_beat, s.song_frame),
        (96_000, 1, 0)
    );
    // Batch 750 (engine frame 48000): beat 3 of the bar.
    run(&mut e, 48_000, 64);
    let s = h.transport_state();
    assert!(!s.playing);
    assert_eq!((s.pre_roll_frames, s.pre_roll_beat), (48_000, 3));
    assert_eq!(s.song_frame, 0);
    // The song starts at the batch after the count: engine frame 96000, song frame 0.
    run(&mut e, 48_000, 64);
    let s = h.transport_state();
    assert!(s.playing, "{s:?}");
    assert_eq!((s.pre_roll_frames, s.pre_roll_beat), (0, 0));
    assert_eq!((s.engine_frame, s.song_frame), (96_000, 0));
}

#[test]
fn stop_and_play_cancel_the_count() {
    let (mut e, mut h, logs) = note_logs(1, 0);
    let (dev, log) = &logs[0];
    let snap = song(vec![strack(*dev, vec![snote(0, PPQ, 60)])], &[(0, 120.0)]);
    let plan = snap.pre_roll(Ticks(0), 2, SR).unwrap();
    h.submit_song(snap).unwrap();
    h.transport(click_on(false)).unwrap();
    h.transport(TransportCommand::PlayWithPreRoll(plan))
        .unwrap();
    run(&mut e, 30_016, 64);
    h.transport(TransportCommand::Stop).unwrap();
    let out = render_blocks(&mut e, &[], 2, TWO_BARS as usize, 64);
    assert_eq!(peak(&out[0]), 0.0, "the count stops sounding");
    assert!(log.events().is_empty(), "the song never started");
    assert!(!h.transport_state().playing);
    // A Play during a count starts the song at once.
    h.transport(TransportCommand::PlayWithPreRoll(plan))
        .unwrap();
    run(&mut e, 10_048, 64);
    h.transport(TransportCommand::Play).unwrap();
    run(&mut e, 128, 64);
    assert!(h.transport_state().playing);
    assert_eq!(h.transport_state().pre_roll_frames, 0);
    assert_eq!(count_ons(&log.events()).len(), 1);
}

#[test]
fn a_loop_counts_once_and_a_later_position_counts_in_before_it() {
    let (mut e, mut h, logs) = note_logs(1, 0);
    let (dev, log) = &logs[0];
    let snap = song(vec![strack(*dev, vec![snote(BAR, PPQ, 60)])], &[(0, 120.0)]);
    let plan = snap.pre_roll(Ticks(BAR), 1, SR).unwrap();
    h.submit_song(snap).unwrap();
    h.transport(click_on(false)).unwrap();
    h.transport(TransportCommand::SetLoop {
        start: Ticks(BAR),
        end: Ticks(2 * BAR),
        enabled: true,
    })
    .unwrap();
    h.transport(TransportCommand::Locate(Ticks(BAR))).unwrap();
    h.transport(TransportCommand::PlayWithPreRoll(plan))
        .unwrap();
    // The count (one bar), then three laps of the one-bar loop.
    let out = render_blocks(&mut e, &[], 2, 4 * 96_000 + 64, 64);
    assert_eq!(onsets(&out[0]).len(), 4, "one count, not one per lap");
    assert_eq!(
        count_ons(&log.events()),
        [96_000, 192_000, 288_000, 384_000]
    );
    assert!(h.transport_state().loop_count >= 2);
}

#[test]
fn a_count_in_is_independent_of_the_host_block_size() {
    let render = |block: usize| {
        let (mut e, mut h) = new_engine(cfg(0, 2));
        let snap = song(Vec::new(), &[(0, 120.0)]);
        let plan = snap.pre_roll(Ticks(0), 1, SR).unwrap();
        h.submit_song(snap).unwrap();
        h.transport(click_on(true)).unwrap();
        h.transport(TransportCommand::PlayWithPreRoll(plan))
            .unwrap();
        render_blocks(&mut e, &[], 2, 110_000, block)
    };
    let reference = render(64);
    for block in [7, 333, 4096] {
        assert_eq!(render(block), reference, "block {block}");
    }
}
