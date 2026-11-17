//! The practice speed (`TransportCommand::Speed`): the song, its notes, the click and the count-in
//! run on the slowed timeline, a change of speed keeps the musical position, devices see the
//! scaled tempo, audio clips are silent while the speed is not 1.0, and an offline bounce that
//! never sends the command is bit-identical to what it always was. Every render is
//! allocation-guarded.

// Justification: the speed factors used here (0.5, 0.25, 1.0) and the frames they give are exact.
#![allow(clippy::float_cmp)]

atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::{NoteKind, PPQ, Ticks, TimeInfo};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceKind, Direction, PrepareInfo, ProcessContext, SocketDescriptor,
    SocketId, SocketKind,
};
use atomik_engine::{
    DeviceInstanceId, Engine, EngineHandle, HotEdit, MAX_SPEED, MIN_SPEED, Topology,
    TransportCommand,
};
use common::audio::*;
use common::song::*;
use common::*;
use std::sync::{Arc, Mutex};

const BAR: i64 = 4 * PPQ;
/// One quarter note at 120 BPM, 48 kHz.
const BEAT_FRAMES: u64 = 24_000;

fn speed(factor: f32) -> TransportCommand {
    TransportCommand::Speed(factor)
}

fn ons(events: &[(u64, NoteKind)]) -> Vec<u64> {
    events
        .iter()
        .filter(|(_, k)| matches!(k, NoteKind::On { .. }))
        .map(|(f, _)| *f)
        .collect()
}

fn offs(events: &[(u64, NoteKind)]) -> Vec<u64> {
    events
        .iter()
        .filter(|(_, k)| matches!(k, NoteKind::Off { .. }))
        .map(|(f, _)| *f)
        .collect()
}

/// A song with a note on each beat of the first two bars, one quarter long.
fn beats() -> Vec<atomik_engine::SongNote> {
    (0..8).map(|i| snote(i * PPQ, PPQ, 60 + i as u8)).collect()
}

#[test]
fn half_speed_doubles_the_frame_of_every_note() {
    let (mut e, mut h, logs) = note_logs(1, 0);
    let (dev, log) = &logs[0];
    h.submit_song(song(vec![strack(*dev, beats())], &[(0, 120.0)]))
        .unwrap();
    h.transport(speed(0.5)).unwrap();
    h.transport(TransportCommand::Play).unwrap();
    run(&mut e, 8 * 2 * BEAT_FRAMES as usize + 640, 64);
    let events = log.events();
    let want: Vec<u64> = (0..8).map(|i| i * 2 * BEAT_FRAMES).collect();
    assert_eq!(ons(&events), want);
    // Each note is one quarter long: its Off is one slowed beat after its On.
    let off_want: Vec<u64> = (1..=8).map(|i| i * 2 * BEAT_FRAMES).collect();
    assert_eq!(offs(&events), off_want);
}

#[test]
fn the_transport_state_runs_on_the_slowed_timeline() {
    let (mut e, mut h, _logs) = note_logs(1, 0);
    h.submit_song(song(Vec::new(), &[(0, 120.0)])).unwrap();
    h.transport(speed(0.5)).unwrap();
    h.transport(TransportCommand::Play).unwrap();
    // One slowed beat (48000 frames) is one beat of the song.
    run(&mut e, 2 * BEAT_FRAMES as usize, 64);
    let s = h.transport_state();
    assert!(s.playing);
    assert_eq!(s.song_frame, 2 * BEAT_FRAMES - 64);
    let want = s.song_frame as f64 / 2.0 / 24_000.0 * PPQ as f64;
    assert!((s.tick as f64 - want).abs() <= 1.0, "{} {want}", s.tick);
}

#[test]
fn a_speed_change_keeps_the_musical_position() {
    let (mut e, mut h, logs) = note_logs(1, 0);
    let (dev, log) = &logs[0];
    h.submit_song(song(vec![strack(*dev, beats())], &[(0, 120.0)]))
        .unwrap();
    h.transport(TransportCommand::Play).unwrap();
    // 1.5 beats at full speed, then half speed.
    run(&mut e, 36_000, 64);
    h.transport(speed(0.5)).unwrap();
    run(&mut e, 64, 64);
    let s = h.transport_state();
    let tick = s.tick as f64;
    assert!(
        (tick - 1.5 * PPQ as f64).abs() < 2.0 * 64.0 * 0.64,
        "the tick is where it was: {tick}"
    );
    run(&mut e, 4 * BEAT_FRAMES as usize, 64);
    let on = ons(&log.events());
    // Beats 0 and 1 at full speed, then beat 2 half a beat later (at 0.5 beat of slowed time, one
    // full beat of frames), and so on, a slowed beat apart.
    assert_eq!(&on[..2], &[0, BEAT_FRAMES]);
    assert!(on[2].abs_diff(36_000 + BEAT_FRAMES) <= 128, "{on:?}");
    assert_eq!(on[3], on[2] + 2 * BEAT_FRAMES);
    // And back: the position is kept again.
    h.transport(speed(1.0)).unwrap();
    run(&mut e, 64, 64);
    let back = h.transport_state().tick;
    assert!(back >= s.tick, "{back} < {}", s.tick);
}

#[test]
fn a_loop_wraps_on_the_slowed_timeline() {
    let (mut e, mut h, logs) = note_logs(1, 0);
    let (dev, log) = &logs[0];
    h.submit_song(song(
        vec![strack(*dev, vec![snote(0, PPQ, 60)])],
        &[(0, 120.0)],
    ))
    .unwrap();
    h.transport(TransportCommand::SetLoop {
        start: Ticks(0),
        end: Ticks(BAR),
        enabled: true,
    })
    .unwrap();
    h.transport(speed(0.25)).unwrap();
    h.transport(TransportCommand::Play).unwrap();
    // A bar at a quarter of the speed is 8 s: two laps.
    run(&mut e, 2 * 8 * 48_000 + 640, 64);
    assert_eq!(ons(&log.events()), [0, 8 * 48_000, 2 * 8 * 48_000]);
    assert!(h.transport_state().loop_count >= 2);
}

#[test]
fn the_count_in_lasts_as_long_as_the_slowed_beats() {
    let (mut e, mut h, logs) = note_logs(1, 0);
    let (dev, log) = &logs[0];
    let snap = song(vec![strack(*dev, vec![snote(0, PPQ, 60)])], &[(0, 120.0)]);
    let plan = snap.pre_roll(Ticks(0), 1, SR).unwrap();
    assert_eq!(plan.frames, 4 * BEAT_FRAMES);
    h.submit_song(snap).unwrap();
    h.transport(speed(0.5)).unwrap();
    h.transport(TransportCommand::PlayWithPreRoll(plan))
        .unwrap();
    run(&mut e, 64, 64);
    assert_eq!(h.transport_state().pre_roll_frames, 8 * BEAT_FRAMES);
    run(&mut e, 8 * BEAT_FRAMES as usize + 128, 64);
    assert_eq!(ons(&log.events()), [8 * BEAT_FRAMES]);
}

#[test]
fn the_click_follows_the_slowed_beats() {
    let (mut e, mut h, _logs) = note_logs(1, 0);
    h.submit_song(song(Vec::new(), &[(0, 120.0)])).unwrap();
    h.transport(TransportCommand::Metronome {
        enabled: true,
        gain: 0.5,
    })
    .unwrap();
    h.transport(speed(0.5)).unwrap();
    h.transport(TransportCommand::Play).unwrap();
    let out = render_blocks(&mut e, &[], 2, 4 * BEAT_FRAMES as usize, 64);
    let mut starts = Vec::new();
    let mut silent = usize::MAX / 2;
    for (i, s) in out[0].iter().enumerate() {
        if *s == 0.0 {
            silent += 1;
        } else {
            if silent >= 2_000 {
                starts.push(i);
            }
            silent = 0;
        }
    }
    assert_eq!(starts.len(), 2, "{starts:?}");
    assert_eq!(starts[1] - starts[0], 2 * BEAT_FRAMES as usize);
}

#[test]
fn speed_is_clamped_and_a_non_finite_value_is_full_speed() {
    let (mut e, mut h, logs) = note_logs(1, 0);
    let (dev, log) = &logs[0];
    h.submit_song(song(vec![strack(*dev, beats())], &[(0, 120.0)]))
        .unwrap();
    assert_eq!((MIN_SPEED, MAX_SPEED), (0.25, 1.5));
    h.transport(speed(0.01)).unwrap();
    h.transport(TransportCommand::Play).unwrap();
    run(&mut e, 4 * BEAT_FRAMES as usize + 640, 64);
    // Clamped to a quarter: the second note is 4 beats of frames in.
    assert_eq!(ons(&log.events()), [0, 4 * BEAT_FRAMES]);
    h.transport(TransportCommand::Stop).unwrap();
    h.transport(TransportCommand::Locate(Ticks(0))).unwrap();
    h.transport(speed(f32::NAN)).unwrap();
    h.transport(TransportCommand::Play).unwrap();
    let before = log.events().len();
    run(&mut e, 2 * BEAT_FRAMES as usize, 64);
    let events = log.events();
    let later: Vec<u64> = ons(&events[before..]);
    assert_eq!(later.len(), 2, "full speed again: {later:?}");
    assert_eq!(later[1] - later[0], BEAT_FRAMES);
}

type Log = Arc<Mutex<Vec<TimeInfo>>>;

static PROBE: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "test.speed-probe",
    name: "Speed Probe",
    kind: DeviceKind::Utility,
    version: 1,
    params: &[],
    sockets: &[SocketDescriptor {
        id: SocketId(0),
        key: "audio_out",
        name: "audio_out",
        kind: SocketKind::Audio,
        direction: Direction::Output,
    }],
};

struct Probe(Log);

impl Device for Probe {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &PROBE
    }
    fn prepare(&mut self, _: &PrepareInfo) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _: atomik_core::ParamId, _: f32) {}
    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        self.0.lock().unwrap().push(ctx.time);
    }
}

fn probe_rig() -> (Engine, EngineHandle, Log) {
    let c = cfg(0, 2);
    let (engine, mut handle) = engine_with(c, 0, None);
    let log: Log = Arc::new(Mutex::new(Vec::with_capacity(2_000)));
    // Apple's `std` mutex boxes its pthread mutex on the first lock: take it outside the guard.
    drop(log.lock());
    let id = DeviceInstanceId(1);
    handle
        .insert_device(id, Box::new(Probe(Arc::clone(&log))), &[])
        .unwrap();
    let mut topo = Topology::new(&c);
    topo.add_node(id, &PROBE).unwrap();
    handle.submit_routing(topo.compile().unwrap()).unwrap();
    (engine, handle, log)
}

#[test]
fn devices_see_the_scaled_tempo_and_ticks_per_frame() {
    let (mut e, mut h, log) = probe_rig();
    h.submit_song(song(Vec::new(), &[(0, 120.0)])).unwrap();
    h.transport(TransportCommand::Play).unwrap();
    run(&mut e, 64 * 4, 64);
    h.transport(speed(0.5)).unwrap();
    run(&mut e, 64 * 4, 64);
    let log = log.lock().unwrap();
    let full = &log[1];
    let slow = &log[7];
    assert_eq!(full.tempo.bpm(), 120.0);
    assert_eq!(slow.tempo.bpm(), 60.0);
    assert!((slow.ticks_per_frame - full.ticks_per_frame / 2.0).abs() < 1e-12);
    // The tick advances by the slowed rate from batch to batch.
    let step = log[7].tick - log[6].tick;
    assert!((step - 64.0 * slow.ticks_per_frame).abs() < 1e-9, "{step}");
}

#[test]
fn a_song_less_transport_slows_too() {
    let (mut e, mut h, log) = probe_rig();
    h.transport(speed(0.5)).unwrap();
    run(&mut e, 64 * 10, 64);
    let log = log.lock().unwrap();
    // 120 BPM engine tempo at half speed: 0.32 ticks per frame.
    let tpf = 120.0 * PPQ as f64 / (60.0 * 48_000.0) / 2.0;
    for t in log.iter() {
        assert_eq!(t.ticks_per_frame, tpf);
        assert_eq!(t.tempo.bpm(), 60.0);
    }
}

#[test]
fn audio_clips_are_silent_while_the_speed_is_not_one() {
    let plain = || {
        Bounce::new(
            audio_input(
                vec![audio_track_of(1, vec![region(0, 0, 4_000)])],
                vec![asset(2, 20_000)],
            ),
            6_000,
        )
    };
    let normal = plain().run();
    assert!(normal[0][..4_000].iter().any(|s| *s != 0.0));
    // Full speed sent explicitly: bit-identical to not sending anything.
    let mut same = plain();
    same.edits.push(edit(0, HotEdit::Transport(speed(1.0))));
    assert_eq!(bits(&same.run()), bits(&normal));
    // Slowed: the clip is silent, and it comes back at 1.0.
    let mut slow = plain();
    slow.edits.push(edit(0, HotEdit::Transport(speed(0.5))));
    assert!(slow.run().iter().all(|c| c.iter().all(|s| *s == 0.0)));
}

#[test]
fn a_bounce_that_sends_no_speed_is_not_affected_by_a_slowed_live_engine() {
    let input = || {
        let mut i = audio_input(
            vec![audio_track_of(1, vec![region(0, 0, 4_000)])],
            vec![asset(2, 20_000)],
        );
        i.tempo = tempo(&[(0, 120.0)]);
        i
    };
    let baseline = Bounce::new(input(), 5_000).run();
    // A live engine at a quarter speed does not touch an offline render's own engine.
    let (mut live, mut h, _logs) = note_logs(1, 0);
    h.transport(speed(0.25)).unwrap();
    run(&mut live, 640, 64);
    let again = Bounce::new(input(), 5_000).run();
    assert_eq!(bits(&again), bits(&baseline));
}

#[test]
fn speed_changes_under_the_guard_do_not_allocate() {
    let (mut e, mut h, logs) = note_logs(1, 0);
    let (dev, log) = &logs[0];
    h.submit_song(song(
        vec![strack(*dev, beats())],
        &[(0, 100.0), (2 * BAR, 140.0)],
    ))
    .unwrap();
    h.transport(TransportCommand::SetLoop {
        start: Ticks(0),
        end: Ticks(BAR),
        enabled: true,
    })
    .unwrap();
    h.transport(TransportCommand::Metronome {
        enabled: true,
        gain: 0.3,
    })
    .unwrap();
    h.transport(TransportCommand::Play).unwrap();
    for factor in [0.5, 0.75, 1.0, 1.5, 0.25, 1.0] {
        h.transport(speed(factor)).unwrap();
        run(&mut e, 20_000, 64);
    }
    assert!(!log.events().is_empty());
}

#[test]
fn the_slowed_render_is_independent_of_the_host_block_size() {
    let render = |block: usize, threads: u16| {
        let (mut e, mut h, logs) = note_logs(1, threads);
        let (dev, log) = &logs[0];
        h.submit_song(song(vec![strack(*dev, beats())], &[(0, 120.0)]))
            .unwrap();
        h.transport(speed(0.75)).unwrap();
        h.transport(TransportCommand::Play).unwrap();
        run(&mut e, 200_000, block);
        log.events()
    };
    assert_eq!(render(64, 0), render(512, 0));
}
