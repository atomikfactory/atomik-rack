//! Transport and RT player: play/stop/locate and the published
//! `TransportState`, sample-accurate notes and loops, note merging with the other sources,
//! snapshot swaps while playing, the metronome, tempo ownership, and the recording conversion
//! (`SongSnapshot::position_at`) across loop wraps. Every render is allocation-guarded.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::{NoteKind, PPQ, Ticks};
use atomik_engine::{
    Graph, ScheduledNote, SongPosition, TransportCommand, TransportState, new_engine,
};
use atomik_testkit::fakes::{NoteLog, TEMPO_PROBE, TempoProbe};
use common::song::*;
use common::*;

const PLAY: TransportCommand = TransportCommand::Play;
const STOP: TransportCommand = TransportCommand::Stop;

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

#[test]
fn transport_state_follows_play_stop_locate_and_loop() {
    let (mut e, mut h, _logs) = note_logs(1, 0);
    assert_eq!(h.transport_state(), TransportState::default());
    h.submit_song(song(Vec::new(), &[(0, 120.0)])).unwrap();
    run(&mut e, 64, 64);
    let s = h.transport_state();
    assert!(!s.playing);
    assert_eq!(
        (s.engine_frame, s.song_frame, s.tick, s.loop_count),
        (0, 0, 0, 0)
    );
    // Play lands at the next boundary (64); the state is published at each batch start.
    h.transport(PLAY).unwrap();
    run(&mut e, 128, 64);
    let s = h.transport_state();
    assert!(s.playing);
    assert_eq!((s.engine_frame, s.song_frame, s.tick), (128, 64, 41)); // 64 / 1.5625 = 40.96
    // Locate while playing.
    h.transport(locate(PPQ)).unwrap();
    run(&mut e, 64, 64);
    let s = h.transport_state();
    assert_eq!((s.engine_frame, s.song_frame, s.tick), (192, 24_000, PPQ));
    // Stop keeps the position reached; a stopped transport does not advance.
    h.transport(STOP).unwrap();
    run(&mut e, 128, 64);
    let s = h.transport_state();
    assert!(!s.playing);
    assert_eq!((s.engine_frame, s.song_frame), (320, 24_064));
    // Locate while stopped; loop settings are published.
    h.transport(locate(2 * PPQ)).unwrap();
    h.transport(looping(PPQ, 3 * PPQ)).unwrap();
    run(&mut e, 64, 64);
    let s = h.transport_state();
    assert_eq!(
        s,
        TransportState {
            playing: false,
            tick: 2 * PPQ,
            engine_frame: 384,
            loop_count: 0,
            song_frame: 48_000,
            loop_start: PPQ,
            loop_end: 3 * PPQ,
            loop_enabled: true,
            ..TransportState::default()
        }
    );
    // Play again: the loop wraps at 72000 back to 24000.
    h.transport(PLAY).unwrap();
    run(&mut e, 24_064 + 64, 64);
    let s = h.transport_state();
    assert_eq!(s.loop_count, 1);
    assert_eq!(s.song_frame, 24_064);
}

#[test]
fn notes_play_at_exact_frames_and_stop_sends_all_off() {
    let (mut e, mut h, logs) = note_logs(1, 0);
    let (dev, log) = &logs[0];
    let notes = vec![snote(PPQ, PPQ, 60), snote(PPQ / 2, 3 * PPQ, 62)];
    h.submit_song(song(vec![strack(*dev, notes)], &[(0, 120.0)]))
        .unwrap();
    h.transport(PLAY).unwrap();
    run(&mut e, 60_032, 333);
    h.transport(STOP).unwrap();
    run(&mut e, 64 * 100, 64);
    let events = log.events();
    assert_eq!(
        events,
        vec![
            (12_000, on(62)),
            (24_000, on(60)),
            (48_000, off(60)),
            (60_032, NoteKind::AllOff),
        ]
    );
    assert!(stuck(&events).is_empty());
}

#[test]
fn locate_while_playing_sends_all_off_and_continues_from_the_new_position() {
    let (mut e, mut h, logs) = note_logs(1, 0);
    let (dev, log) = &logs[0];
    let notes = vec![snote(PPQ, PPQ, 60), snote(PPQ / 2, 3 * PPQ, 62)];
    h.submit_song(song(vec![strack(*dev, notes)], &[(0, 120.0)]))
        .unwrap();
    h.transport(PLAY).unwrap();
    run(&mut e, 30_016, 64);
    // Inside notes 60 and 62: AllOff, then both are chased from the new position (key order).
    h.transport(locate(3 * PPQ / 2)).unwrap();
    run(&mut e, 40_000 - 30_016, 7);
    h.transport(locate(0)).unwrap();
    run(&mut e, 90_048 - 40_000, 4096);
    assert_eq!(
        log.events(),
        vec![
            (12_000, on(62)),
            (24_000, on(60)),
            (30_016, NoteKind::AllOff),
            (30_016, on(60)),
            (30_016, on(62)),
            (40_000, NoteKind::AllOff),
            (52_000, on(62)),
            (64_000, on(60)),
            (88_000, off(60)),
        ]
    );
}

#[test]
fn loop_wraps_sample_accurately_and_releases_crossing_notes() {
    let (mut e, mut h, logs) = note_logs(1, 0);
    let (dev, log) = &logs[0];
    // Loop [12000, 36000) song frames; key 64 crosses the loop end, key 65 does not.
    let notes = vec![snote(5 * PPQ / 4, PPQ / 2, 64), snote(PPQ / 2, 960, 65)];
    h.submit_song(song(vec![strack(*dev, notes)], &[(0, 120.0)]))
        .unwrap();
    h.transport(looping(PPQ / 2, 3 * PPQ / 2)).unwrap();
    h.transport(PLAY).unwrap();
    run(&mut e, 90_048, 100);
    let mut want = vec![(12_000, on(65)), (13_500, off(65)), (30_000, on(64))];
    for lap in 1..=3u64 {
        let wrap = 12_000 + lap * 24_000; // wrap frame = start of this lap
        want.extend([
            (wrap, off(64)), // released at the loop end
            (wrap, on(65)),
            (wrap + 1_500, off(65)),
            (wrap + 18_000, on(64)),
        ]);
    }
    want.retain(|(f, _)| *f < 90_048);
    assert_eq!(log.events(), want);
    let s = h.transport_state();
    assert_eq!(s.loop_count, 3);
    // Wraps land mid-batch (36000 = 562.5 batches).
    assert_eq!(36_000 % 64, 32);
}

#[test]
fn song_notes_merge_with_scheduled_and_live_notes_in_frame_order() {
    let (mut e, mut h, logs) = note_logs(1, 0);
    let (dev, log) = &logs[0];
    let mut live = h.live_notes().unwrap();
    let sched = |frame, key| ScheduledNote {
        frame,
        device: *dev,
        socket: NoteLog::NOTES_IN,
        kind: on(key),
    };
    // Song note at song frame 100 (tick 64), playing from engine frame 0.
    h.submit_song(song(
        vec![strack(*dev, vec![snote(64, 10, 60)])],
        &[(0, 120.0)],
    ))
    .unwrap();
    h.transport(PLAY).unwrap();
    h.send_note(sched(70, 61)).unwrap();
    h.send_note(sched(100, 62)).unwrap();
    live.push(sched(80, 64)).unwrap();
    live.push(sched(100, 63)).unwrap();
    run(&mut e, 192, 64);
    assert_eq!(
        log.events(),
        vec![
            (70, on(61)),
            (80, on(64)),
            (100, on(60)), // ties: song, then scheduled, then live
            (100, on(62)),
            (100, on(63)),
            (116, off(60)), // tick 74 = 115.625 frames
        ]
    );
}

/// Song events of several tracks are merged with queued notes by frame, not by track order:
/// track 2's note (frame 25) must reach its device before a scheduled note at frame 40, even
/// though track 1 (frame 56) is generated first.
#[test]
fn song_events_of_several_tracks_merge_in_frame_order() {
    let (mut e, mut h, logs) = note_logs(2, 0);
    let (d1, d2) = (logs[0].0, logs[1].0);
    h.submit_song(song(
        vec![
            strack(d1, vec![snote(36, 10, 60)]), // frame 56.25 -> 56
            strack(d2, vec![snote(16, 10, 61)]), // frame 25
        ],
        &[(0, 120.0)],
    ))
    .unwrap();
    h.transport(PLAY).unwrap();
    h.send_note(ScheduledNote {
        frame: 40,
        device: d2,
        socket: NoteLog::NOTES_IN,
        kind: on(62),
    })
    .unwrap();
    run(&mut e, 128, 64);
    assert_eq!(logs[1].1.events()[..2], [(25, on(61)), (40, on(62))]);
    assert_eq!(logs[0].1.events()[0], (56, on(60)));
}

#[test]
fn song_swap_while_playing_keeps_position_and_releases_changed_tracks() {
    let (mut e, mut h, logs) = note_logs(3, 0);
    let (d1, d2, d3) = (logs[0].0, logs[1].0, logs[2].0);
    let long = |key| snote(0, 4 * PPQ, key);
    let first = song(
        vec![
            strack(d1, vec![long(60)]),
            strack(d2, vec![long(61)]),
            strack(d3, vec![long(62)]),
        ],
        &[(0, 120.0)],
    );
    h.submit_song(first).unwrap();
    h.transport(PLAY).unwrap();
    run(&mut e, 30_016, 64);
    assert_eq!(h.collect_garbage(), 0);
    // d1 unchanged, d2 removed, d3 changed (a new note at 2 beats; its note 62 still covers the
    // position, so it keeps sounding).
    let second = vec![
        strack(d1, vec![long(60)]),
        strack(d3, vec![long(62), snote(2 * PPQ, PPQ, 63)]),
    ];
    h.submit_song(song(second.clone(), &[(0, 120.0)])).unwrap();
    run(&mut e, 64, 64);
    let s = h.transport_state();
    assert_eq!(
        (s.engine_frame, s.song_frame),
        (30_016, 30_016),
        "position kept"
    );
    assert_eq!(h.collect_garbage(), 1, "the old snapshot comes back");
    run(&mut e, 100_032 - 30_080, 64);
    assert_eq!(logs[0].1.events(), vec![(0, on(60)), (96_000, off(60))]);
    assert_eq!(logs[1].1.events(), vec![(0, on(61)), (30_016, off(61))]);
    assert_eq!(
        logs[2].1.events(),
        vec![
            (0, on(62)),
            (48_000, on(63)),
            (72_000, off(63)),
            (96_000, off(62))
        ]
    );
    // A swap with another tempo map keeps the tick: song frame 30016 at 120 BPM (tick
    // 19210.24) becomes frame 60032 at 60 BPM; the unchanged track keeps holding its note.
    h.transport(locate(0)).unwrap(); // at 100032: AllOff, then the notes at tick 0 again
    run(&mut e, 30_016, 64);
    let before = h.transport_state();
    assert_eq!(before.song_frame, 29_952);
    h.submit_song(song(second, &[(0, 60.0)])).unwrap();
    run(&mut e, 64, 64);
    let after = h.transport_state();
    assert_eq!(
        (after.engine_frame, after.song_frame, after.tick),
        (130_048, 60_032, 19_210),
        "tick kept: frames double at half tempo"
    );
    let last = logs[0].1.events();
    assert_eq!(
        last[last.len() - 2..],
        [(100_032, NoteKind::AllOff), (100_032, on(60))]
    );
    run(&mut e, 200_000, 4096);
    // Its off at tick 4 * PPQ is now at song frame 192000.
    let last = logs[0].1.events();
    assert_eq!(*last.last().unwrap(), (130_048 + 192_000 - 60_032, off(60)));
}

/// Engine with no plan, 3 outputs: only the metronome sounds (outputs 1/2).
fn click_render(block: usize, enabled: bool) -> Vec<Vec<f32>> {
    let (mut e, mut h) = new_engine(cfg(0, 3));
    h.submit_song(song(Vec::new(), &[(0, 120.0)])).unwrap();
    h.transport(TransportCommand::Metronome { enabled, gain: 0.5 })
        .unwrap();
    let mut out = render_blocks(&mut e, &[], 3, 30_016, block);
    h.transport(PLAY).unwrap();
    let played = render_blocks(&mut e, &[], 3, 4 * 24_000 + 1_000, block);
    h.transport(STOP).unwrap();
    let stopped = render_blocks(&mut e, &[], 3, 30_000, block);
    for (o, (p, s)) in out.iter_mut().zip(played.into_iter().zip(stopped)) {
        o.extend(p);
        o.extend(s);
    }
    out
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, s| m.max(s.abs()))
}

#[test]
fn metronome_clicks_on_beats_only_while_playing_and_is_deterministic() {
    let out = click_render(64, true);
    // Play lands at engine frame 30016; output has 64 frames of latency.
    let start = 30_016 + 64;
    assert_eq!(peak(&out[0][..start]), 0.0, "silent while stopped");
    for beat in 0..4usize {
        let onset = start + beat * 24_000;
        assert_eq!(peak(&out[0][onset - 200..onset]), 0.0, "beat {beat}");
        let click = peak(&out[0][onset..onset + 1_000]);
        assert!(click > 0.05, "beat {beat}: {click}");
        assert!(click <= 0.5, "gain 0.5");
        if beat == 0 {
            assert!(
                click > peak(&out[0][onset + 24_000..onset + 25_000]),
                "accent"
            );
        }
    }
    assert_eq!(out[0], out[1], "outputs 1 and 2");
    assert_eq!(peak(&out[2]), 0.0, "output 3 untouched");
    let stop = start + 4 * 24_000 + 1_000 + 64; // Stop lands at the next boundary
    let stop = stop.next_multiple_of(64);
    assert_eq!(peak(&out[0][stop..]), 0.0, "silent after stop");
    for block in [1, 333] {
        assert_eq!(
            bits(&click_render(block, true)),
            bits(&out),
            "block {block}"
        );
    }
    let quiet = click_render(64, false);
    assert!(quiet.iter().all(|c| peak(c) == 0.0), "disabled: silent");
}

#[test]
fn song_tempo_map_wins_over_set_tempo_while_loaded() {
    let c = cfg(0, 2);
    let mut g = Graph::new(&c);
    let probe = g.add_device(TempoProbe::boxed());
    let out1 = iface(&g, "out_1");
    g.connect(ep(probe, TempoProbe::AUDIO_OUT), out1).unwrap();
    assert_eq!(g.descriptor(probe).unwrap().type_id, TEMPO_PROBE.type_id);
    let (mut e, mut h) = new_engine(c);
    h.submit_plan(g.compile().unwrap()).unwrap();
    h.set_tempo(atomik_core::Tempo::new(150.0).unwrap())
        .unwrap();
    // Host output frame n shows batch n - 64 (FIFO latency).
    let out = render_blocks(&mut e, &[], 2, 128, 64); // host 0..128
    assert_eq!(out[0][64], 150.0, "no song: set_tempo");
    h.submit_song(song(Vec::new(), &[(0, 120.0), (PPQ, 90.0)]))
        .unwrap();
    h.set_tempo(atomik_core::Tempo::new(150.0).unwrap())
        .unwrap();
    let out = render_blocks(&mut e, &[], 2, 128, 64); // host 128..256
    assert_eq!(out[0][0], 150.0);
    assert_eq!(
        out[0][64], 120.0,
        "song loaded: its tempo at the position wins"
    );
    // Play lands at batch 256; the song reaches tick PPQ (frame 24000) at batch 24256,
    // shown at host 24320.
    h.transport(PLAY).unwrap();
    let out = render_blocks(&mut e, &[], 2, 24_192, 64); // host 256..24448
    let first_90 = out[0]
        .iter()
        .position(|&s| s.to_bits() == 90.0f32.to_bits())
        .unwrap();
    assert_eq!(first_90 + 256, 24_320);
    assert!(
        out[0][..first_90]
            .iter()
            .all(|&s| s.to_bits() == 120.0f32.to_bits())
    );
}

#[test]
fn songless_transport_follows_set_tempo_and_keeps_the_tick() {
    let (mut e, mut h) = new_engine(cfg(0, 2));
    h.transport(PLAY).unwrap();
    render_blocks(&mut e, &[], 2, 24_000 + 64, 64);
    let s = h.transport_state();
    assert!(s.playing);
    assert_eq!((s.song_frame, s.tick), (24_000, PPQ));
    h.set_tempo(atomik_core::Tempo::new(60.0).unwrap()).unwrap();
    render_blocks(&mut e, &[], 2, 64, 64);
    let s = h.transport_state();
    assert_eq!((s.song_frame, s.tick), (48_128, PPQ + 41));
}

/// Recording: a live event's engine frame converts back to the exact song frame and tick,
/// before and after a loop wrap, from states published earlier or later.
#[test]
fn recording_conversion_is_exact_across_loop_wraps() {
    let (mut e, mut h, logs) = note_logs(1, 0);
    let (dev, log) = &logs[0];
    let mut live = h.live_notes().unwrap();
    let snapshot = song(
        vec![strack(*dev, vec![snote(4 * PPQ / 3, 100, 60)])],
        &[(0, 120.0)],
    );
    let conv = snapshot.clone();
    h.submit_song(snapshot).unwrap();
    h.transport(looping(PPQ, 2 * PPQ)).unwrap(); // song frames [24000, 48000)
    h.transport(PLAY).unwrap();
    // Live notes: around the first wrap (engine 48000) and 7 frames after each song note.
    let live_frames = [47_999u64, 48_000, 32_007, 56_007, 80_007];
    let mut sorted = live_frames;
    sorted.sort_unstable();
    for f in sorted {
        live.push(ScheduledNote {
            frame: f,
            device: *dev,
            socket: NoteLog::NOTES_IN,
            kind: on(100),
        })
        .unwrap();
    }
    let mut states = Vec::new();
    for _ in 0..(130_048 / 64) {
        run(&mut e, 64, 64);
        states.push(h.transport_state());
    }
    let events = log.events();
    let song_ons: Vec<u64> = events
        .iter()
        .filter(|(_, k)| *k == on(60))
        .map(|(f, _)| *f)
        .collect();
    assert_eq!(song_ons, vec![32_000, 56_000, 80_000, 104_000, 128_000]);
    for (k, s) in states.iter().enumerate() {
        assert_eq!(s.engine_frame, k as u64 * 64);
    }
    // Every state (from before, during or after the lap) maps every song note back.
    for (lap, &frame) in song_ons.iter().enumerate() {
        for s in states.iter().step_by(97).chain(states.last()) {
            let p = conv.position_at(s, frame, SR);
            assert_eq!(
                p,
                SongPosition {
                    song_frame: 32_000,
                    tick: Ticks(4 * PPQ / 3),
                    loop_count: lap as u64
                },
                "note {frame} from state at {}",
                s.engine_frame
            );
        }
    }
    // Live notes are delivered at their frames; their positions are exact.
    let live_got: Vec<u64> = events
        .iter()
        .filter(|(_, k)| *k == on(100))
        .map(|(f, _)| *f)
        .collect();
    assert_eq!(live_got, sorted.to_vec());
    let s = states[700]; // engine frame 44800, lap 0
    let at = |f| conv.position_at(&s, f, SR);
    assert_eq!(
        at(47_999),
        SongPosition {
            song_frame: 47_999,
            tick: Ticks(30_719),
            loop_count: 0
        }
    );
    assert_eq!(
        at(48_000),
        SongPosition {
            song_frame: 24_000,
            tick: Ticks(PPQ),
            loop_count: 1
        }
    );
    assert_eq!(at(56_007).song_frame, 32_007);
    assert_eq!(at(56_007).tick, Ticks(20_484));
    assert_eq!(at(80_007).loop_count, 2);
}

/// A silent source through a 1,000-frame effect into output 1; the metronome is on and the
/// transport plays from the start. Returns output 1.
fn latent_click_render(align: bool, block: usize) -> Vec<f32> {
    use atomik_testkit::fakes::{ConstantSource, LatencyPassthrough};
    let c = cfg(0, 2);
    let mut g = Graph::new(&c);
    let src = g.add_device(ConstantSource::boxed(0.0));
    let fx = g.add_device(LatencyPassthrough::boxed(1_000));
    g.connect(
        ep(src, ConstantSource::AUDIO_OUT),
        ep(fx, LatencyPassthrough::AUDIO_IN),
    )
    .unwrap();
    g.connect(ep(fx, LatencyPassthrough::AUDIO_OUT), iface(&g, "out_1"))
        .unwrap();
    g.set_align_inputs(align);
    let (mut e, mut h) = new_engine(c);
    h.submit_plan(g.compile().unwrap()).unwrap();
    h.submit_song(song(Vec::new(), &[(0, 120.0)])).unwrap();
    h.transport(TransportCommand::Metronome {
        enabled: true,
        gain: 0.5,
    })
    .unwrap();
    render_blocks(&mut e, &[], 2, 256, block);
    h.transport(PLAY).unwrap();
    render_blocks(&mut e, &[], 2, 6_000, block).swap_remove(0)
}

fn first_sound(x: &[f32]) -> usize {
    x.iter().position(|s| s.abs() > 1e-6).unwrap()
}

#[test]
fn the_click_waits_for_the_latency_of_the_audio_when_inputs_are_aligned() {
    let straight = first_sound(&latent_click_render(false, 64));
    let held = first_sound(&latent_click_render(true, 64));
    assert_eq!(
        held,
        straight + 1_000,
        "the click lands with the delayed audio"
    );
    for block in [1, 333] {
        assert_eq!(
            bits(&[latent_click_render(true, block)]),
            bits(&[latent_click_render(true, 64)]),
            "block {block}"
        );
    }
}
