//! The song player applies an automation lane only when its value changed (or something else may
//! have touched the device): a flat lane costs one `set_param` instead of one per batch, and the
//! lane still wins over a value set from outside, after a locate and after a loop wrap.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::{PPQ, ParamId, Ticks};
use atomik_engine::{
    DeviceInstanceId, Engine, EngineHandle, Graph, SongAutomation, SongCurve, SongInput, SongPoint,
    SongTrack, TransportCommand, compile_song,
};
use atomik_testkit::fakes::{ParamEvents, ParamLog};
use common::song::*;
use common::*;

fn points(param: ParamId, pts: &[(i64, f32)]) -> SongAutomation {
    SongAutomation {
        param,
        points: pts
            .iter()
            .map(|&(tick, value)| SongPoint {
                tick: Ticks(tick),
                value,
                curve: SongCurve::Linear,
            })
            .collect(),
    }
}

/// A playing engine with a flat lane on `A` (0.5) and a ramp on `B`.
fn rig() -> (Engine, EngineHandle, DeviceInstanceId, ParamEvents) {
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
            automation: vec![
                points(ParamLog::A, &[(0, 0.5), (8 * PPQ, 0.5)]),
                points(ParamLog::B, &[(0, -4.0), (8 * PPQ, 4.0)]),
            ],
        }],
        tempo: tempo(&[(0, 120.0)]),
        meter: vec![atomik_core::MeterEvent::new(1, 4, 4)],
        audio: Vec::new(),
        assets: Vec::new(),
    };
    h.submit_song(compile_song(&input).unwrap()).unwrap();
    h.transport(TransportCommand::Play).unwrap();
    run(&mut e, 64, 64);
    (e, h, id, log)
}

#[test]
fn a_flat_lane_is_applied_once_and_a_moving_lane_every_batch() {
    let (mut e, _h, _id, log) = rig();
    run(&mut e, 64 * 99, 64);
    assert_eq!(log.applied(ParamLog::A).len(), 1, "flat: first batch only");
    assert_eq!(log.applied(ParamLog::B).len(), 100, "moving: every batch");
}

#[test]
fn a_value_set_from_outside_is_overwritten_by_the_lane_on_the_next_batch() {
    let (mut e, mut h, id, log) = rig();
    run(&mut e, 64 * 10, 64);
    h.set_param(id, ParamLog::A, 0.9).unwrap();
    run(&mut e, 64 * 10, 64);
    let a = log.applied(ParamLog::A);
    // The outside value lands first, the forced lane write replaces it before the batch runs:
    // the device never renders 0.9, exactly as when every lane was applied every batch.
    assert_eq!(a.len(), 2, "first batch, then the forced re-apply");
    assert_eq!(a[1].1, 0.5);
}

#[test]
fn a_locate_applies_every_lane_again() {
    let (mut e, mut h, _id, log) = rig();
    run(&mut e, 64 * 10, 64);
    h.transport(TransportCommand::Locate(Ticks(PPQ))).unwrap();
    run(&mut e, 64 * 10, 64);
    assert_eq!(log.applied(ParamLog::A).len(), 2);
}

#[test]
fn a_lane_that_was_held_is_applied_when_released() {
    let (mut e, h, id, log) = rig();
    h.hold_automation(&[(id, ParamLog::A)]).unwrap();
    run(&mut e, 64 * 10, 64);
    h.hold_automation(&[]).unwrap();
    run(&mut e, 64 * 10, 64);
    assert_eq!(
        log.applied(ParamLog::A).len(),
        2,
        "first batch, then on release"
    );
}
