//! Engine part: pattern data reaches devices through the control
//! queue (`EngineHandle::set_device_player_data`, `HotEdit::PlayerData`) or, before compile,
//! through `Graph::set_device_player_data`, in queue order and at batch boundaries. Replaced
//! data and the data of removed devices come back to the handle and are freed by
//! `collect_garbage`, never on the audio thread. Every render runs under `assert_no_alloc`.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceKind, Direction, PlayerData, PlayerDataRef, PrepareInfo,
    ProcessContext, SocketDescriptor, SocketId, SocketKind, StepSeqData,
};
use atomik_engine::{
    CONTROL_QUEUE_CAPACITY, DeviceInstanceId, Engine, EngineError, EngineHandle, Graph, HotEdit,
    Topology,
};
use common::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

static HOLDER: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "test.player-holder",
    name: "Player Holder",
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

/// Holds the data it was given; `seen[batch]` is the pattern count it holds at each batch
/// (0 = none), stored without allocating.
struct Holder {
    data: Option<PlayerDataRef>,
    seen: Arc<[AtomicU32]>,
    swaps: Arc<AtomicU32>,
}

impl Device for Holder {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &HOLDER
    }
    fn prepare(&mut self, _: &PrepareInfo) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _: atomik_core::ParamId, _: f32) {}
    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let count = self
            .data
            .as_deref()
            .map_or(0, |d| u32::from(d.notes().count));
        if let Some(slot) = self.seen.get((ctx.time.frame / 64) as usize) {
            slot.store(count, Ordering::Relaxed);
        }
    }
    fn swap_player_data(&mut self, data: Option<PlayerDataRef>) -> Option<PlayerDataRef> {
        self.swaps.fetch_add(1, Ordering::Relaxed);
        std::mem::replace(&mut self.data, data)
    }
}

struct Rig {
    engine: Engine,
    handle: EngineHandle,
    seen: Arc<[AtomicU32]>,
    swaps: Arc<AtomicU32>,
    topo: Topology,
}

const ID: DeviceInstanceId = DeviceInstanceId(1);

fn holder(seen: &Arc<[AtomicU32]>, swaps: &Arc<AtomicU32>) -> Box<dyn Device> {
    Box::new(Holder {
        data: None,
        seen: Arc::clone(seen),
        swaps: Arc::clone(swaps),
    })
}

fn rig() -> Rig {
    let c = cfg(0, 2);
    let (engine, mut handle) = engine_with(c, 0, None);
    let seen: Arc<[AtomicU32]> = (0..64).map(|_| AtomicU32::new(u32::MAX)).collect();
    let swaps = Arc::new(AtomicU32::new(0));
    handle
        .insert_device(ID, holder(&seen, &swaps), &[])
        .unwrap();
    let mut topo = Topology::new(&c);
    topo.add_node(ID, &HOLDER).unwrap();
    handle.submit_routing(topo.compile().unwrap()).unwrap();
    Rig {
        engine,
        handle,
        seen,
        swaps,
        topo,
    }
}

fn data(count: u8) -> PlayerDataRef {
    Arc::new(PlayerData::StepSeq(StepSeqData {
        count,
        ..StepSeqData::default()
    }))
}

fn batches(e: &mut Engine, n: usize) {
    for _ in 0..n {
        render_blocks(e, &[], 2, 64, 64);
    }
}

#[test]
fn insert_then_data_then_routing_applies_in_queue_order() {
    // Everything queued before the first batch: the device holds its data in batch 0.
    let c = cfg(0, 2);
    let (mut engine, mut handle) = engine_with(c, 0, None);
    let seen: Arc<[AtomicU32]> = (0..8).map(|_| AtomicU32::new(u32::MAX)).collect();
    let swaps = Arc::new(AtomicU32::new(0));
    handle
        .insert_device(ID, holder(&seen, &swaps), &[])
        .unwrap();
    handle.set_device_player_data(ID, Some(data(3))).unwrap();
    let mut topo = Topology::new(&c);
    topo.add_node(ID, &HOLDER).unwrap();
    handle.submit_routing(topo.compile().unwrap()).unwrap();
    batches(&mut engine, 2);
    assert_eq!(seen[0].load(Ordering::Relaxed), 3);
    assert_eq!(seen[1].load(Ordering::Relaxed), 3);
}

#[test]
fn a_timed_swap_lands_at_its_batch_boundary() {
    let mut r = rig();
    r.handle
        .schedule(
            64 * 5,
            HotEdit::PlayerData {
                device: ID,
                data: Some(data(2)),
            },
        )
        .unwrap();
    r.handle
        .schedule(
            64 * 9 + 1,
            HotEdit::PlayerData {
                device: ID,
                data: Some(data(4)),
            },
        )
        .unwrap();
    r.handle
        .schedule(
            64 * 14,
            HotEdit::PlayerData {
                device: ID,
                data: None,
            },
        )
        .unwrap();
    batches(&mut r.engine, 16);
    let seen: Vec<u32> = r.seen[..16]
        .iter()
        .map(|s| s.load(Ordering::Relaxed))
        .collect();
    let mut expect = vec![0u32; 5];
    expect.extend([2; 5]); // batches 5..=9
    expect.extend([4; 4]); // batches 10..=13
    expect.extend([0; 2]);
    // Frame 64 * 9 + 1 is mid-batch: it applies at batch 10, so batch 9 still sees the old data.
    assert_eq!(seen, expect, "{seen:?}");
    assert_eq!(r.swaps.load(Ordering::Relaxed), 3);
}

#[test]
fn replaced_data_is_freed_by_collect_garbage_not_the_render_thread() {
    let mut r = rig();
    let first = data(2);
    let weak = Arc::downgrade(&first);
    r.handle.set_device_player_data(ID, Some(first)).unwrap();
    batches(&mut r.engine, 2);
    r.handle.set_device_player_data(ID, Some(data(5))).unwrap();
    batches(&mut r.engine, 2);
    assert!(weak.upgrade().is_some(), "not freed by the render thread");
    assert!(r.handle.collect_garbage() >= 1);
    assert!(weak.upgrade().is_none(), "freed on the handle side");
    // Clearing hands the current data back the same way.
    r.handle.set_device_player_data(ID, None).unwrap();
    batches(&mut r.engine, 2);
    assert!(r.handle.collect_garbage() >= 1);
    assert_eq!(r.seen[5].load(Ordering::Relaxed), 0);
}

#[test]
fn a_device_that_takes_no_data_hands_it_straight_back() {
    let c = cfg(0, 2);
    let (mut engine, mut handle) = engine_with(c, 0, None);
    // A device without `swap_player_data` (the default returns its argument).
    let (dev, _log) = atomik_testkit::fakes::NoteLog::boxed(8);
    let id = DeviceInstanceId(1);
    handle.insert_device(id, dev, &[]).unwrap();
    let d = data(2);
    let weak = Arc::downgrade(&d);
    handle.set_device_player_data(id, Some(d)).unwrap();
    batches(&mut engine, 2);
    assert!(weak.upgrade().is_some(), "not freed by the render thread");
    assert!(handle.collect_garbage() >= 1);
    assert!(weak.upgrade().is_none());
}

#[test]
fn a_removed_devices_data_is_dropped_on_the_handle_side() {
    let mut r = rig();
    let d = data(2);
    let weak = Arc::downgrade(&d);
    r.handle.set_device_player_data(ID, Some(d)).unwrap();
    batches(&mut r.engine, 2);
    r.topo.remove_node(ID).unwrap();
    r.handle.submit_routing(r.topo.compile().unwrap()).unwrap();
    r.handle.remove_device(ID).unwrap();
    batches(&mut r.engine, 2);
    assert!(weak.upgrade().is_some(), "not freed by the render thread");
    r.handle.collect_garbage();
    assert!(weak.upgrade().is_none());
}

#[test]
fn unknown_devices_and_a_full_queue_are_rejected_on_the_handle_side() {
    let mut r = rig();
    let d = data(1);
    let weak = Arc::downgrade(&d);
    assert_eq!(
        r.handle
            .set_device_player_data(DeviceInstanceId(9), Some(d)),
        Err(EngineError::UnknownDevice)
    );
    assert!(weak.upgrade().is_none(), "dropped on the caller's side");
    batches(&mut r.engine, 2);
    r.handle.collect_garbage();
    // Fill the control queue.
    let mut sent = 0;
    let (error, weak) = loop {
        let d = data(1);
        let weak = Arc::downgrade(&d);
        match r.handle.set_device_player_data(ID, Some(d)) {
            Ok(()) => sent += 1,
            Err(e) => break (e, weak),
        }
    };
    assert_eq!(error, EngineError::QueueFull);
    assert!(weak.upgrade().is_none());
    assert!(sent + 1 >= CONTROL_QUEUE_CAPACITY, "{sent}");
}

#[test]
fn the_graph_path_installs_data_before_compile() {
    let c = cfg(0, 2);
    let seen: Arc<[AtomicU32]> = (0..8).map(|_| AtomicU32::new(u32::MAX)).collect();
    let swaps = Arc::new(AtomicU32::new(0));
    let mut g = Graph::new(&c);
    let id = g.add_device(holder(&seen, &swaps));
    let d = data(6);
    let weak = Arc::downgrade(&d);
    g.set_device_player_data(id, Some(d)).unwrap();
    assert_eq!(
        g.set_device_player_data(DeviceInstanceId(77), None),
        Err(atomik_engine::GraphError::UnknownDevice)
    );
    let out = atomik_engine::render_offline(g.compile().unwrap(), c, 256, 64, &[], &[]);
    assert_eq!(out.len(), 2);
    assert!(
        seen[..4].iter().all(|s| s.load(Ordering::Relaxed) == 6),
        "the data is in place from the first batch"
    );
    assert_eq!(swaps.load(Ordering::Relaxed), 1);
    assert!(
        weak.upgrade().is_none(),
        "plan and device are gone: data freed with them"
    );
}
