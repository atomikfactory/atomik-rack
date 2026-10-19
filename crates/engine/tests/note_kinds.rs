//! The new `NoteKind` variants travel through the engine's note
//! path like notes do (exact frames, in order, any host block), and the split device forwards
//! them by its rules.

atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::{ExprKind, NoteKind};
use atomik_device_sdk::SocketId;
use atomik_devices::note_split;
use atomik_engine::{DeviceInstanceId, Endpoint, Graph, ScheduledNote};
use atomik_testkit::fakes::{NoteEvents, NoteLog};
use common::*;

fn sched(frame: u64, device: DeviceInstanceId, socket: SocketId, kind: NoteKind) -> ScheduledNote {
    ScheduledNote {
        frame,
        device,
        socket,
        kind,
    }
}

fn all_kinds() -> Vec<(u64, NoteKind)> {
    vec![
        (
            3,
            NoteKind::On {
                key: 60,
                velocity: 0.5,
            },
        ),
        (5, NoteKind::PitchBend { value: -0.5 }),
        (
            5,
            NoteKind::Cc {
                controller: 1,
                value: 0.25,
            },
        ),
        (70, NoteKind::ChannelPressure { value: 0.75 }),
        (
            70,
            NoteKind::PolyPressure {
                key: 60,
                value: 0.5,
            },
        ),
        (
            71,
            NoteKind::NoteExpr {
                key: 60,
                kind: ExprKind::Tuning,
                value: 1.5,
            },
        ),
        (
            200,
            NoteKind::NoteExpr {
                key: 60,
                kind: ExprKind::Slide,
                value: 0.1,
            },
        ),
        (333, NoteKind::Off { key: 60 }),
    ]
}

/// A log fed by scheduled notes for every kind; returns the log's events.
fn through_log(block: usize, threads: u16) -> Vec<(u64, NoteKind)> {
    let c = cfg(0, 2);
    let mut g = Graph::new(&c);
    let (dev, log) = NoteLog::boxed(64);
    let log_id = g.add_device(dev);
    let (mut engine, mut handle) = engine_with(c, threads, None);
    handle.submit_plan(g.compile().unwrap()).unwrap();
    for (frame, kind) in all_kinds() {
        handle
            .send_note(sched(frame, log_id, NoteLog::NOTES_IN, kind))
            .unwrap();
    }
    render_blocks(&mut engine, &[], 2, 512, block);
    assert_eq!(handle.dropped_events(), 0);
    log.events()
}

#[test]
fn every_kind_arrives_at_its_exact_frame_and_in_order_for_any_block_and_workers() {
    let expected = all_kinds();
    for block in [1, 64, 333, 512] {
        for threads in [0, 2] {
            assert_eq!(through_log(block, threads), expected, "{block}/{threads}");
        }
    }
}

fn split_rig() -> (
    atomik_engine::Engine,
    atomik_engine::EngineHandle,
    DeviceInstanceId,
    [NoteEvents; 2],
) {
    let c = cfg(0, 2);
    let mut g = Graph::new(&c);
    let split = g.add_device((note_split::factory().create)());
    let mut logs = Vec::new();
    for (slot, transpose) in [(1usize, 0.0f32), (2, 12.0)] {
        let (dev, log) = NoteLog::boxed(64);
        let id = g.add_device(dev);
        g.connect(
            Endpoint {
                device: split,
                socket: SocketId(slot as u16),
            },
            Endpoint {
                device: id,
                socket: NoteLog::NOTES_IN,
            },
        )
        .unwrap();
        let d = g.descriptor(split).unwrap();
        for (key, v) in [
            (format!("s{slot}.on"), 1.0),
            (format!("s{slot}.transpose"), transpose),
        ] {
            let p = d.param_by_key(&key).unwrap().id;
            g.set_param(split, p, v).unwrap();
        }
        logs.push(log);
    }
    let (engine, mut handle) = engine_with(c, 0, None);
    handle.submit_plan(g.compile().unwrap()).unwrap();
    let [a, b]: [NoteEvents; 2] = logs.try_into().unwrap();
    (engine, handle, split, [a, b])
}

#[test]
fn the_split_forwards_channel_messages_to_every_slot_and_expressions_by_route() {
    let (mut engine, mut handle, split, [out1, out2]) = split_rig();
    let n = SocketId(0);
    let bend = NoteKind::PitchBend { value: 0.5 };
    let expr = |key| NoteKind::NoteExpr {
        key,
        kind: ExprKind::Pressure,
        value: 0.7,
    };
    for (frame, kind) in [
        (
            10,
            NoteKind::On {
                key: 60,
                velocity: 1.0,
            },
        ),
        (20, bend),
        (30, expr(60)),
        (40, NoteKind::Off { key: 60 }),
        (50, expr(60)),
    ] {
        handle.send_note(sched(frame, split, n, kind)).unwrap();
    }
    render_blocks(&mut engine, &[], 2, 128, 64);
    let on = |key| NoteKind::On { key, velocity: 1.0 };
    assert_eq!(
        out1.events(),
        vec![
            (10, on(60)),
            (20, bend),
            (30, expr(60)),
            (40, NoteKind::Off { key: 60 })
        ],
        "slot 1: the expression stops after the Off"
    );
    assert_eq!(
        out2.events(),
        vec![
            (10, on(72)),
            (20, bend),
            (30, expr(72)),
            (40, NoteKind::Off { key: 72 })
        ],
        "slot 2 transposes the key of the expression with the note"
    );
}
