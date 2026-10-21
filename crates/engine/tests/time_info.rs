//! The engine fills `TimeInfo::tick`, `ticks_per_frame` and
//! `loop_wrap` from the player at every batch, with and without a song.

// Justification: a stopped transport must report bit-identical ticks, and the tempo values are exact.
#![allow(clippy::float_cmp)]

atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::{PPQ, Ticks, TimeInfo};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceKind, Direction, PrepareInfo, ProcessContext, SocketDescriptor,
    SocketId, SocketKind,
};
use atomik_engine::{DeviceInstanceId, Engine, EngineHandle, Topology, TransportCommand};
use common::song::*;
use common::*;
use std::sync::{Arc, Mutex};

static PROBE: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "test.time-probe",
    name: "Time Probe",
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

type Log = Arc<Mutex<Vec<TimeInfo>>>;

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

fn rig() -> (Engine, EngineHandle, Log) {
    let c = cfg(0, 2);
    let (engine, mut handle) = engine_with(c, 0, None);
    // Preallocated: pushing inside the capacity does not allocate (renders run under the guard).
    let log: Log = Arc::new(Mutex::new(Vec::with_capacity(2_000)));
    // Apple's `std` mutex boxes its pthread mutex on the first lock: take it once outside the
    // no-alloc render section.
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

/// Renders `n` batches, one at a time.
fn batches(e: &mut Engine, n: usize) {
    for _ in 0..n {
        render_blocks(e, &[], 2, 64, 64);
    }
}

const TPF_120: f64 = 120.0 * PPQ as f64 / (60.0 * 48_000.0); // 0.64

fn tpf_of(t: &TimeInfo) -> f64 {
    t.tempo.bpm() * PPQ as f64 / (60.0 * 48_000.0)
}

#[test]
fn without_a_song_the_tick_follows_the_engine_frames() {
    let (mut e, _h, log) = rig();
    batches(&mut e, 20);
    let log = log.lock().unwrap();
    assert_eq!(log.len(), 20);
    for (n, t) in log.iter().enumerate() {
        assert_eq!(t.frame, 64 * n as u64);
        assert!(
            (t.tick - 64.0 * n as f64 * TPF_120).abs() < 1e-9,
            "{n}: {}",
            t.tick
        );
        assert_eq!(t.ticks_per_frame, TPF_120);
        assert!(t.playing);
        assert_eq!(t.loop_wrap, None);
    }
}

#[test]
fn a_tempo_change_keeps_the_song_less_tick_continuous() {
    let (mut e, mut h, log) = rig();
    batches(&mut e, 5);
    h.set_tempo(atomik_core::Tempo::new(240.0).unwrap())
        .unwrap();
    batches(&mut e, 5);
    let log = log.lock().unwrap();
    for pair in log.windows(2) {
        let expect = pair[0].tick + 64.0 * pair[0].ticks_per_frame;
        assert!((pair[1].tick - expect).abs() < 1e-9, "{pair:?}");
    }
    assert_eq!(log[9].ticks_per_frame, 2.0 * TPF_120);
}

#[test]
fn with_a_song_the_tick_is_the_players_tick_through_play_locate_stop_and_tempo_changes() {
    let (mut e, mut h, log) = rig();
    h.submit_song(song(
        Vec::new(),
        &[(0, 120.0), (PPQ, 240.0), (3 * PPQ, 90.0)],
    ))
    .unwrap();
    // Stopped: a constant tick at the playhead, playing false.
    batches(&mut e, 3);
    h.transport(TransportCommand::Play).unwrap();
    batches(&mut e, 800);
    h.transport(TransportCommand::Locate(Ticks(PPQ / 2)))
        .unwrap();
    batches(&mut e, 10);
    h.transport(TransportCommand::Stop).unwrap();
    batches(&mut e, 4);
    let log = log.lock().unwrap();
    for (n, t) in log.iter().enumerate() {
        assert!(t.tick >= 0.0, "{n}");
        assert!((t.ticks_per_frame - tpf_of(t)).abs() < 1e-12, "{n}");
        assert_eq!(t.loop_wrap, None);
    }
    assert!(log[..3].iter().all(|t| !t.playing && t.tick == 0.0));
    // Play lands at batch 3 (position 0) and advances by 64 * tpf within a tempo segment.
    assert!(log[3].playing && log[3].tick == 0.0);
    assert!((log[4].tick - 64.0 * TPF_120).abs() < 1e-9);
    // Past tick PPQ the tempo is 240 BPM, past 3 * PPQ it is 90.
    let fast = log
        .iter()
        .find(|t| t.tempo.bpm() == 240.0)
        .expect("240 BPM reached");
    assert!(fast.tick >= PPQ as f64 - 1.0);
    assert!((fast.ticks_per_frame - 2.0 * TPF_120).abs() < 1e-12);
    assert!(log.iter().any(|t| t.tempo.bpm() == 90.0), "90 BPM reached");
    // Locate while playing (applied at batch 803): that batch starts at the located tick.
    assert!(log[803].playing);
    assert!(
        (log[803].tick - (PPQ / 2) as f64).abs() < 1e-9,
        "{}",
        log[803].tick
    );
    // Stopped again: constant tick, playing false.
    let last = &log[log.len() - 4..];
    assert!(last.iter().all(|t| !t.playing));
    assert!(last.windows(2).all(|p| p[0].tick == p[1].tick));
}

#[test]
fn every_batch_tick_equals_the_transport_state_tick() {
    let (mut e, mut h, log) = rig();
    h.submit_song(song(Vec::new(), &[(0, 120.0), (PPQ, 250.0)]))
        .unwrap();
    h.transport(TransportCommand::Play).unwrap();
    for n in 0..600 {
        render_blocks(&mut e, &[], 2, 64, 64);
        let s = h.transport_state();
        let t = *log.lock().unwrap().last().unwrap();
        assert!(s.playing);
        assert!(
            (t.tick - s.tick as f64).abs() <= 0.5 + 1e-9,
            "batch {n}: {} vs {}",
            t.tick,
            s.tick
        );
    }
}

#[test]
fn a_loop_wrap_inside_a_batch_reports_its_offset_and_target() {
    let (mut e, mut h, log) = rig();
    h.submit_song(song(Vec::new(), &[(0, 120.0)])).unwrap();
    h.transport(TransportCommand::SetLoop {
        start: Ticks(480),
        end: Ticks(1000),
        enabled: true,
    })
    .unwrap();
    h.transport(TransportCommand::Locate(Ticks(480))).unwrap();
    h.transport(TransportCommand::Play).unwrap();
    batches(&mut e, 80);
    let log = log.lock().unwrap();
    let wraps: Vec<usize> = (0..log.len() - 1)
        .filter(|&i| log[i].loop_wrap.is_some())
        .collect();
    assert!(wraps.len() >= 3, "{wraps:?}");
    for &i in &wraps {
        let t = log[i];
        let w = t.loop_wrap.unwrap();
        assert!((1..64).contains(&w.offset), "{w:?}");
        // The wrap offset is where the old position reaches the loop end...
        let at_wrap = t.tick + f64::from(w.offset) * t.ticks_per_frame;
        // (the first frame at or after the loop end: within one frame of ticks, never early).
        assert!(
            at_wrap >= 1000.0 - 1e-6 && at_wrap - 1000.0 < t.ticks_per_frame + 1e-6,
            "{t:?}"
        );
        // ...and the transport is back at the loop start there.
        assert!((w.to_tick - 480.0).abs() < 1.0, "{w:?}");
        let expect = w.to_tick + f64::from(64 - w.offset) * t.ticks_per_frame;
        assert!(
            (log[i + 1].tick - expect).abs() < 1e-3,
            "{t:?} {:?}",
            log[i + 1]
        );
    }
}

#[test]
fn a_wrap_on_a_batch_boundary_is_a_tick_jump_without_loop_wrap() {
    let (mut e, mut h, log) = rig();
    h.submit_song(song(Vec::new(), &[(0, 120.0)])).unwrap();
    // 24 000 frames = 375 batches: the loop end falls exactly on a batch start.
    h.transport(TransportCommand::SetLoop {
        start: Ticks(0),
        end: Ticks(PPQ),
        enabled: true,
    })
    .unwrap();
    h.transport(TransportCommand::Play).unwrap();
    batches(&mut e, 380);
    let log = log.lock().unwrap();
    assert!(log.iter().all(|t| t.loop_wrap.is_none()));
    assert!(log[374].tick > 0.0);
    assert_eq!(log[375].tick, 0.0, "back at the loop start");
}

#[test]
fn the_loop_region_is_reported_only_while_enabled_with_a_song() {
    let (mut e, mut h, log) = rig();
    h.submit_song(song(Vec::new(), &[(0, 120.0)])).unwrap();
    batches(&mut e, 2);
    h.transport(TransportCommand::SetLoop {
        start: Ticks(480),
        end: Ticks(1000),
        enabled: true,
    })
    .unwrap();
    batches(&mut e, 2);
    h.transport(TransportCommand::SetLoop {
        start: Ticks(480),
        end: Ticks(1000),
        enabled: false,
    })
    .unwrap();
    batches(&mut e, 2);
    let log = log.lock().unwrap();
    assert!(log[..2].iter().all(|t| t.loop_ticks.is_none()));
    assert_eq!(log[2].loop_ticks, Some((480.0, 1000.0)));
    assert_eq!(log[3].loop_ticks, Some((480.0, 1000.0)));
    assert!(log[4..].iter().all(|t| t.loop_ticks.is_none()));
}

#[test]
fn a_song_less_engine_has_no_loop_region_and_is_four_four() {
    let (mut e, _h, log) = rig();
    batches(&mut e, 3);
    let log = log.lock().unwrap();
    assert!(
        log.iter()
            .all(|t| t.loop_ticks.is_none() && t.time_signature == (4, 4))
    );
}

#[test]
fn the_time_signature_follows_the_songs_beats_per_bar() {
    let (mut e, mut h, log) = rig();
    batches(&mut e, 1);
    let mut input = song_input(Vec::new(), &[(0, 120.0)]);
    input.beats_per_bar = 7;
    h.submit_song(atomik_engine::compile_song(&input).unwrap())
        .unwrap();
    batches(&mut e, 2);
    let log = log.lock().unwrap();
    assert_eq!(log[0].time_signature, (4, 4), "before the song arrives");
    assert_eq!(log[1].time_signature, (7, 4));
    assert_eq!(log[2].time_signature, (7, 4));
}
