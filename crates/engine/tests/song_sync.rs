//! MIDI clock out (24 clocks per quarter note, Start, Stop, Continue and Song Position Pointer
//! stamped with engine frames) and following an external clock (tempo, start, stop, locate, a
//! lost clock). Every render is allocation-guarded.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::{NoteKind, PPQ, Ticks};
use atomik_device_sdk::MidiTap;
use atomik_engine::{
    CLOCK, CONTINUE, EngineHandle, ExternalClock, ExternalState, START, STOP, SyncConfig,
    TransportCommand,
};
use common::song::*;
use std::sync::Arc;

/// Every message the tap holds, as `(frame, bytes)`.
fn drain(tap: &MidiTap) -> Vec<(u64, Vec<u8>)> {
    let mut v = Vec::new();
    while let Some(e) = tap.pop() {
        v.push((e.frame, e.bytes().to_vec()));
    }
    v
}

fn on(config: SyncConfig) -> SyncConfig {
    SyncConfig {
        enabled: true,
        ..config
    }
}

fn clock_frames(messages: &[(u64, Vec<u8>)]) -> Vec<u64> {
    messages
        .iter()
        .filter(|(_, b)| b == &[CLOCK])
        .map(|(f, _)| *f)
        .collect()
}

fn others(messages: &[(u64, Vec<u8>)]) -> Vec<(u64, Vec<u8>)> {
    messages
        .iter()
        .filter(|(_, b)| b != &[CLOCK])
        .cloned()
        .collect()
}

/// An engine with a (note-less) song at 120 BPM whose clock out is on and has run one batch.
fn rig() -> (atomik_engine::Engine, EngineHandle, Arc<MidiTap>) {
    let (mut e, mut h, _logs) = note_logs(1, 0);
    h.submit_song(song(Vec::new(), &[(0, 120.0)])).unwrap();
    h.sync_out().configure(on(SyncConfig::default()));
    run(&mut e, 64, 64);
    let tap = h.sync_out().tap();
    // Switching clock out on tells the receiver the position.
    assert_eq!(drain(&tap), vec![(0, vec![0xF2, 0, 0])]);
    (e, h, tap)
}

#[test]
fn play_sends_start_then_a_clock_every_twenty_fourth_of_a_beat() {
    let (mut e, mut h, tap) = rig();
    assert!(drain(&tap).is_empty(), "nothing while stopped");
    h.transport(TransportCommand::Play).unwrap();
    run(&mut e, 48_000, 64);
    let got = drain(&tap);
    // The song starts at engine frame 64; at 120 BPM a clock is 1000 frames.
    assert_eq!(got[0], (64, vec![START]));
    let clocks = clock_frames(&got);
    assert_eq!(clocks.len(), 48);
    for (k, f) in clocks.iter().enumerate() {
        assert_eq!(*f, 64 + 1000 * k as u64, "clock {k}");
    }
}

#[test]
fn stop_sends_stop_and_the_clocks_end() {
    let (mut e, mut h, tap) = rig();
    h.transport(TransportCommand::Play).unwrap();
    run(&mut e, 5_000, 64);
    drain(&tap);
    h.transport(TransportCommand::Stop).unwrap();
    run(&mut e, 5_000, 64);
    let got = drain(&tap);
    assert_eq!(others(&got).len(), 1);
    assert_eq!(others(&got)[0].1, vec![STOP]);
    let stop_at = others(&got)[0].0;
    assert!(clock_frames(&got).iter().all(|f| *f < stop_at), "{got:?}");
}

#[test]
fn a_locate_while_stopped_sends_the_position_and_play_then_continues() {
    let (mut e, mut h, tap) = rig();
    h.transport(TransportCommand::Locate(Ticks(4 * PPQ)))
        .unwrap();
    run(&mut e, 64, 64);
    // Four beats are sixteen sixteenth notes.
    assert_eq!(drain(&tap), vec![(64, vec![0xF2, 16, 0])]);
    h.transport(TransportCommand::Play).unwrap();
    run(&mut e, 64, 64);
    let got = drain(&tap);
    assert_eq!(got[0], (128, vec![CONTINUE]));
    // Playing from beat 4: the first clock is the one on the beat itself.
    assert_eq!(clock_frames(&got)[0], 128);
}

#[test]
fn a_locate_while_playing_stops_positions_and_continues() {
    let (mut e, mut h, tap) = rig();
    h.transport(TransportCommand::Play).unwrap();
    run(&mut e, 640, 64);
    drain(&tap);
    h.transport(TransportCommand::Locate(Ticks(2 * PPQ)))
        .unwrap();
    run(&mut e, 640, 64);
    let got = others(&drain(&tap));
    let frame = got[0].0;
    assert_eq!(
        got,
        vec![
            (frame, vec![STOP]),
            (frame, vec![0xF2, 8, 0]),
            (frame, vec![CONTINUE])
        ]
    );
    // Back to the start: Stop and Start, no position.
    h.transport(TransportCommand::Locate(Ticks(0))).unwrap();
    run(&mut e, 640, 64);
    let got = others(&drain(&tap));
    assert_eq!(
        got.iter().map(|m| m.1.clone()).collect::<Vec<_>>(),
        vec![vec![STOP], vec![START]]
    );
}

#[test]
fn a_locate_while_playing_can_send_only_the_position() {
    let (mut e, mut h, tap) = rig();
    h.sync_out().configure(on(SyncConfig {
        restart_on_locate: false,
        ..SyncConfig::default()
    }));
    h.transport(TransportCommand::Play).unwrap();
    run(&mut e, 640, 64);
    drain(&tap);
    h.transport(TransportCommand::Locate(Ticks(2 * PPQ)))
        .unwrap();
    run(&mut e, 640, 64);
    let got = others(&drain(&tap));
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].1, vec![0xF2, 8, 0]);
}

#[test]
fn the_position_can_be_left_out() {
    let (mut e, mut h, tap) = rig();
    h.sync_out().configure(on(SyncConfig {
        song_position: false,
        ..SyncConfig::default()
    }));
    h.transport(TransportCommand::Locate(Ticks(4 * PPQ)))
        .unwrap();
    run(&mut e, 64, 64);
    assert!(drain(&tap).is_empty());
}

#[test]
fn a_loop_wrap_sends_the_position_of_the_loop_start() {
    let (mut e, mut h, tap) = rig();
    h.transport(TransportCommand::SetLoop {
        start: Ticks(PPQ),
        end: Ticks(3 * PPQ),
        enabled: true,
    })
    .unwrap();
    h.transport(TransportCommand::Locate(Ticks(PPQ))).unwrap();
    run(&mut e, 64, 64);
    drain(&tap);
    h.transport(TransportCommand::Play).unwrap();
    // Two beats of loop at 120 BPM: 48_000 frames.
    run(&mut e, 50_000, 64);
    let got = drain(&tap);
    let spp: Vec<_> = got.iter().filter(|(_, b)| b[0] == 0xF2).collect();
    assert_eq!(spp.len(), 1, "{got:?}");
    assert_eq!(spp[0].1, vec![0xF2, 4, 0]);
    // The clocks go on across the wrap without a gap or a repeat: 1000 frames apart.
    let clocks = clock_frames(&got);
    assert!(clocks.windows(2).all(|w| w[1] - w[0] == 1000), "{clocks:?}");
}

#[test]
fn a_tempo_ramp_keeps_every_clock_on_its_tick() {
    let (mut e, mut h, _logs) = note_logs(1, 0);
    let mut input = song_input(Vec::new(), &[(0, 100.0)]);
    input.tempo = tempo_ramps(&[(0, 100.0, true), (4 * PPQ, 140.0, false)]);
    let snap = atomik_engine::compile_song(&input).unwrap();
    h.submit_song(snap).unwrap();
    h.sync_out().configure(on(SyncConfig::default()));
    run(&mut e, 64, 64);
    let tap = h.sync_out().tap();
    h.transport(TransportCommand::Play).unwrap();
    run(&mut e, 200_000, 64);
    let clocks = clock_frames(&drain(&tap));
    assert!(clocks.len() > 100);
    // Clock k is on tick 640 k: through the same map the notes use.
    let map = atomik_core::TempoTable::new(&[
        atomik_core::TempoPoint {
            tick: 0,
            bpm: 100.0,
            ramp: true,
        },
        atomik_core::TempoPoint {
            tick: 4 * PPQ,
            bpm: 140.0,
            ramp: false,
        },
    ])
    .unwrap();
    for (k, f) in clocks.iter().enumerate() {
        let want = map.map().song_frame(k as i64 * 640, SR);
        assert_eq!(*f, 64 + want, "clock {k}");
    }
    // The tempo rises, so the gaps shrink.
    assert!(clocks[1] - clocks[0] > clocks[clocks.len() - 1] - clocks[clocks.len() - 2]);
}

#[test]
fn nothing_is_sent_while_clock_out_is_off() {
    let (mut e, mut h, _logs) = note_logs(1, 0);
    h.submit_song(song(Vec::new(), &[(0, 120.0)])).unwrap();
    let tap = h.sync_out().tap();
    h.transport(TransportCommand::Play).unwrap();
    run(&mut e, 10_000, 64);
    h.transport(TransportCommand::Locate(Ticks(PPQ))).unwrap();
    h.transport(TransportCommand::Stop).unwrap();
    run(&mut e, 1_000, 64);
    assert!(drain(&tap).is_empty());
}

#[test]
fn clocks_keep_running_while_stopped_when_asked_to() {
    let (mut e, mut h, tap) = rig();
    h.sync_out().configure(on(SyncConfig {
        while_stopped: true,
        ..SyncConfig::default()
    }));
    run(&mut e, 10_000, 64);
    let got = drain(&tap);
    let clocks = clock_frames(&got);
    assert_eq!(clocks.len(), 10, "{clocks:?}");
    for w in clocks.windows(2) {
        let gap = w[1] - w[0];
        assert!((999..=1001).contains(&gap), "{clocks:?}");
    }
    // Start finds the clock on the tick grid again.
    h.transport(TransportCommand::Play).unwrap();
    run(&mut e, 64, 64);
    let got = drain(&tap);
    assert_eq!(others(&got).len(), 1);
    assert_eq!(others(&got)[0].1, vec![START]);
}

#[test]
fn switching_clock_out_on_while_playing_joins_the_receiver_at_the_position() {
    let (mut e, mut h, _logs) = note_logs(1, 0);
    h.submit_song(song(Vec::new(), &[(0, 120.0)])).unwrap();
    let tap = h.sync_out().tap();
    h.transport(TransportCommand::Locate(Ticks(PPQ))).unwrap();
    h.transport(TransportCommand::Play).unwrap();
    run(&mut e, 640, 64);
    h.sync_out().configure(on(SyncConfig::default()));
    run(&mut e, 64, 64);
    let got = others(&drain(&tap));
    assert_eq!(
        got.iter().map(|m| m.1.clone()).collect::<Vec<_>>(),
        vec![vec![0xF2, 4, 0], vec![CONTINUE]]
    );
}

// ---- Following an external clock ----

struct Following {
    engine: atomik_engine::Engine,
    handle: EngineHandle,
    cell: Arc<ExternalClock>,
    log: atomik_testkit::fakes::NoteEvents,
    state: ExternalState,
}

impl Following {
    /// A song with a note per beat (keys 60..) at 120 BPM, followed from the first batch.
    fn new() -> Self {
        let (mut engine, mut handle, logs) = note_logs(1, 0);
        let (dev, log) = &logs[0];
        let notes = (0..16)
            .map(|k| snote(k * PPQ, PPQ / 2, 60 + k as u8))
            .collect();
        handle
            .submit_song(song(vec![strack(*dev, notes)], &[(0, 120.0)]))
            .unwrap();
        let cell = handle.external_clock();
        cell.set_following(true);
        run(&mut engine, 64, 64);
        Self {
            engine,
            handle,
            cell,
            log: log.clone(),
            state: ExternalState::default(),
        }
    }

    fn publish(&mut self, f: impl FnOnce(&mut ExternalState)) {
        f(&mut self.state);
        self.cell.publish(&self.state);
    }

    /// Runs `frames` with the clock alive: a tick arrives every 960 frames.
    fn run(&mut self, frames: usize) {
        let mut left = frames;
        while left > 0 {
            let n = left.min(960);
            self.publish(|s| s.clocks += 1);
            run(&mut self.engine, n, 64);
            left -= n;
        }
    }

    /// Runs `frames` without a clock tick.
    fn run_silent(&mut self, frames: usize) {
        run(&mut self.engine, frames, 64);
    }

    fn start_at(&mut self, bpm: f64, position: i64) {
        self.publish(|s| {
            s.tempo = Some(bpm);
            s.running = true;
            s.position = position;
            s.locates += 1;
            s.clocks += 1;
        });
    }
}

#[test]
fn a_started_clock_plays_the_song_at_its_tempo() {
    let mut f = Following::new();
    f.start_at(90.0, 0);
    f.run(64);
    assert!(f.handle.transport_state().playing);
    f.run(120_000);
    let ons: Vec<u64> = f
        .log
        .events()
        .iter()
        .filter(|(_, k)| matches!(k, NoteKind::On { .. }))
        .map(|(fr, _)| *fr)
        .collect();
    // Playing from engine frame 64: a beat is 32_000 frames at 90 BPM.
    for (k, fr) in ons.iter().enumerate().take(3) {
        let want = 64 + k as u64 * 32_000;
        assert!(fr.abs_diff(want) <= 1, "beat {k}: {fr} vs {want}");
    }
}

#[test]
fn a_tempo_change_moves_the_rest_of_the_song_not_the_position() {
    let mut f = Following::new();
    f.start_at(100.0, 0);
    // The song starts at engine frame 64; the change is taken at engine frame 29_952, 29_888
    // frames (1.037 beats at 100 BPM) into the song.
    f.run(64 + 29_824);
    f.publish(|s| {
        s.tempo = Some(140.0);
        s.clocks += 1;
    });
    f.run(60_000);
    let ons: Vec<u64> = f
        .log
        .events()
        .iter()
        .filter(|(_, k)| matches!(k, NoteKind::On { .. }))
        .map(|(fr, _)| *fr)
        .collect();
    assert!(ons[0].abs_diff(64) <= 1, "{ons:?}");
    assert!(ons[1].abs_diff(64 + 28_800) <= 1, "{ons:?}");
    // Beat 2 comes after the rest of the beat, at the new tempo.
    let new_beat = 60.0 * 48_000.0 / 140.0;
    let expect = 29_952.0 + (2.0 - 29_888.0 / 28_800.0) * new_beat;
    assert!(
        (ons[2] as f64 - expect).abs() <= 3.0,
        "{} vs {expect}",
        ons[2]
    );
}

#[test]
fn a_stop_from_the_clock_releases_the_notes_and_stops_the_transport() {
    let mut f = Following::new();
    f.start_at(120.0, 0);
    f.run(12_500); // inside the first note
    assert!(f.handle.transport_state().playing);
    f.publish(|s| {
        s.running = false;
        s.clocks += 1;
    });
    f.run(64);
    assert!(!f.handle.transport_state().playing);
    assert!(stuck(&f.log.events()).is_empty(), "{:?}", f.log.events());
}

#[test]
fn a_position_pointer_moves_the_song_and_continue_plays_on_from_it() {
    let mut f = Following::new();
    f.publish(|s| {
        s.tempo = Some(120.0);
        s.locates += 1;
        s.position = 4 * PPQ;
        s.clocks += 1;
    });
    f.run(64);
    assert!(!f.handle.transport_state().playing);
    assert_eq!(f.handle.transport_state().tick, 4 * PPQ);
    f.publish(|s| {
        s.running = true;
        s.clocks += 1;
    });
    f.run(64);
    f.run(24_000 + 64);
    let ons: Vec<_> = f
        .log
        .events()
        .into_iter()
        .filter_map(|(fr, k)| match k {
            NoteKind::On { key, .. } => Some((fr, key)),
            _ => None,
        })
        .collect();
    // The note on beat 4 starts at once, the one on beat 5 a beat later.
    assert_eq!(ons[0].1, 64);
    assert_eq!(ons[1].1, 65);
    assert!(ons[1].0 - ons[0].0 >= 23_900 && ons[1].0 - ons[0].0 <= 24_100);
}

#[test]
fn a_lost_clock_stops_the_notes_and_the_last_tempo_stays() {
    let mut f = Following::new();
    f.start_at(120.0, 0);
    f.run(12_500);
    assert!(f.handle.transport_state().playing);
    // No clock for more than half a second.
    f.run_silent(30_016);
    assert!(!f.handle.transport_state().playing);
    assert!(stuck(&f.log.events()).is_empty());
    // It returns: the transport continues on the same tempo.
    f.publish(|s| s.clocks += 1);
    f.run(64);
    assert!(f.handle.transport_state().playing);
}

#[test]
fn the_transport_commands_are_ignored_while_following() {
    let mut f = Following::new();
    f.handle.transport(TransportCommand::Play).unwrap();
    f.run(640);
    assert!(!f.handle.transport_state().playing);
    f.handle
        .transport(TransportCommand::Locate(Ticks(PPQ)))
        .unwrap();
    f.run(64);
    assert_eq!(f.handle.transport_state().tick, 0);
    // Not following any more: they work again.
    f.cell.set_following(false);
    f.run(64);
    f.handle.transport(TransportCommand::Play).unwrap();
    f.run(640);
    assert!(f.handle.transport_state().playing);
}

#[test]
fn leaving_follow_stops_and_goes_back_to_the_songs_own_tempo() {
    let mut f = Following::new();
    f.start_at(60.0, 0);
    f.run(48_000 + 64); // one beat at 60 BPM
    f.cell.set_following(false);
    f.run(64);
    let s = f.handle.transport_state();
    assert!(!s.playing);
    // The musical position is kept: beat 1.
    assert!((s.tick - PPQ).abs() < 40, "{s:?}");
    f.handle.transport(TransportCommand::Play).unwrap();
    f.run(24_000);
    // From beat 1 at the song's 120 BPM: the next beat is 24_000 frames away.
    let keys: Vec<u8> = f
        .log
        .events()
        .iter()
        .filter_map(|(_, k)| match k {
            NoteKind::On { key, .. } => Some(*key),
            _ => None,
        })
        .collect();
    assert_eq!(keys.last(), Some(&62));
}

#[test]
fn a_followed_clock_is_forwarded_as_clock_out() {
    let mut f = Following::new();
    let out = f.handle.sync_out();
    out.configure(on(SyncConfig::default()));
    f.run(64);
    let tap = out.tap();
    f.start_at(150.0, 0);
    f.run(64 + 48_000);
    let clocks = clock_frames(&drain(&tap));
    // 150 BPM: a clock every 800 frames.
    assert!(clocks.windows(2).all(|w| w[1] - w[0] == 800), "{clocks:?}");
}

#[test]
fn switching_clock_out_off_while_playing_stops_the_receiver_once() {
    let (mut e, mut h, tap) = rig();
    h.transport(TransportCommand::Play).unwrap();
    run(&mut e, 2_000, 64);
    drain(&tap);
    h.sync_out().configure(SyncConfig::default());
    run(&mut e, 2_000, 64);
    let got = drain(&tap);
    assert_eq!(
        got.iter().filter(|(_, b)| b == &[STOP]).count(),
        1,
        "{got:?}"
    );
    assert!(clock_frames(&got).is_empty());
    run(&mut e, 2_000, 64);
    assert!(drain(&tap).is_empty());
}
