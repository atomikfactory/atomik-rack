//! A held automation lane (a take is writing it) is not applied by the song player, the other
//! lanes still are, releasing the hold brings the lane back, and holding or releasing mid-play
//! never allocates on the audio thread.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::{PPQ, ParamId, Ticks};
use atomik_engine::{
    EngineError, Graph, HOLD_CAPACITY, SongAutomation, SongCurve, SongInput, SongPoint, SongTrack,
    TransportCommand, compile_song,
};
use atomik_testkit::fakes::ParamLog;
use common::song::*;
use common::*;

fn lane(param: ParamId, from: f32, to: f32) -> SongAutomation {
    let p = |tick, value| SongPoint {
        tick: Ticks(tick),
        value,
        curve: SongCurve::Linear,
    };
    SongAutomation {
        param,
        points: vec![p(0, from), p(8 * PPQ, to)],
    }
}

#[test]
fn a_held_lane_is_skipped_and_the_others_play_until_it_is_released() {
    let c = cfg(0, 2);
    let mut g = Graph::new(&c);
    let (dev, log) = ParamLog::boxed(20_000);
    let id = g.add_device(dev);
    let (mut e, mut h) = engine_with(c, 0, None);
    h.submit_plan(g.compile().unwrap()).unwrap();
    let input = SongInput {
        tracks: vec![SongTrack {
            device: id,
            notes_socket: ParamLog::NOTES_IN,
            chase: true,
            notes: Vec::new(),
            controllers: Vec::new(),
            automation: vec![lane(ParamLog::A, 0.1, 0.9), lane(ParamLog::B, -4.0, 4.0)],
        }],
        tempo: tempo(&[(0, 120.0)]),
        meter: vec![atomik_core::MeterEvent::new(1, 4, 4)],
        audio: Vec::new(),
        assets: Vec::new(),
    };
    h.submit_song(compile_song(&input).unwrap()).unwrap();
    h.hold_automation(&[(id, ParamLog::A)]).unwrap();
    h.transport(TransportCommand::Play).unwrap();
    run(&mut e, 64 * 100, 64);
    assert!(
        log.applied(ParamLog::A).is_empty(),
        "a held lane is not applied"
    );
    assert_eq!(log.applied(ParamLog::B).len(), 100, "the other lane plays");
    // Release while playing: the lane is back from the next batch on.
    h.hold_automation(&[]).unwrap();
    run(&mut e, 64 * 50, 64);
    assert_eq!(log.applied(ParamLog::A).len(), 50);
    assert_eq!(log.applied(ParamLog::B).len(), 150);
    // Holding again stops it again, and the lane that is held is the one named.
    h.hold_automation(&[(id, ParamLog::B)]).unwrap();
    run(&mut e, 64 * 20, 64);
    assert_eq!(log.applied(ParamLog::A).len(), 70);
    assert_eq!(log.applied(ParamLog::B).len(), 150);
}

#[test]
fn the_hold_has_room_for_its_capacity_and_refuses_more_without_changing() {
    let c = cfg(0, 2);
    let (_e, h) = engine_with(c, 0, None);
    let some: Vec<_> = (0..HOLD_CAPACITY as u32)
        .map(|i| (atomik_engine::DeviceInstanceId(i + 1), ParamId(i)))
        .collect();
    assert_eq!(h.hold_automation(&some), Ok(()));
    let too_many: Vec<_> = (0..=HOLD_CAPACITY as u32)
        .map(|i| (atomik_engine::DeviceInstanceId(i + 1), ParamId(i)))
        .collect();
    assert_eq!(h.hold_automation(&too_many), Err(EngineError::QueueFull));
    assert_eq!(h.hold_automation(&[]), Ok(()));
}
