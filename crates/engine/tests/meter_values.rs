//! Devices publish display values (`Device::meter_values`) after every batch;
//! the handle (or the shared `Meters`) reads them lock-free, with a publish sequence. Own
//! binary: the worker violation counter is process-global.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::{BATCH, ParamId};
use atomik_device_sdk::{Device, DeviceDescriptor, MAX_METER_VALUES, PrepareInfo, ProcessContext};
use atomik_engine::{
    DeviceInstanceId, EngineHandle, EngineOptions, Topology, new_engine_with_arena,
};
use atomik_testkit::fakes::{CONSTANT_SOURCE, ConstantSource};
use atomik_testkit::no_alloc::{forbid_on_this_thread, worker_violations};
use common::*;

/// A source whose meter values after batch `n` (1-based) are `n * 100 + i` for `i < count`.
struct Meterer {
    count: usize,
    batches: u32,
    opt_in: bool,
}

impl Meterer {
    fn boxed(count: usize) -> Box<dyn Device> {
        Box::new(Self {
            count,
            batches: 0,
            opt_in: true,
        })
    }
}

impl Device for Meterer {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &CONSTANT_SOURCE
    }
    fn prepare(&mut self, _: &PrepareInfo) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _: ParamId, _: f32) {}
    fn process(&mut self, _ctx: &mut ProcessContext<'_>) {
        self.batches += 1;
    }
    fn has_meter_values(&self) -> bool {
        self.opt_in
    }
    fn meter_values(&self, out: &mut [f32]) -> usize {
        let n = self.count.min(out.len());
        for (i, o) in out[..n].iter_mut().enumerate() {
            *o = (self.batches * 100) as f32 + i as f32;
        }
        n
    }
}

fn id(n: u32) -> DeviceInstanceId {
    DeviceInstanceId(n)
}

/// Routes the first of `ids` (constant-source type) to the interface; the others are routed
/// too but feed nothing, so every id is in the routing.
fn route(handle: &mut EngineHandle, ids: &[u32]) {
    let c = cfg(0, 2);
    let mut t = Topology::new(&c);
    for &n in ids {
        t.add_node(id(n), &CONSTANT_SOURCE).unwrap();
    }
    if let Some(&n) = ids.first() {
        let s = t
            .descriptor(atomik_engine::Graph::INTERFACE)
            .unwrap()
            .socket_by_key("out_1")
            .unwrap();
        t.connect(
            ep(id(n), ConstantSource::AUDIO_OUT),
            ep(atomik_engine::Graph::INTERFACE, s.id),
        )
        .unwrap();
    }
    handle.submit_routing(t.compile().unwrap()).unwrap();
}

fn read(handle: &EngineHandle, n: u32) -> Option<(Vec<f32>, u64)> {
    let mut out = [f32::NAN; MAX_METER_VALUES + 4];
    let (count, seq) = handle.device_meter_values(id(n), &mut out)?;
    Some((out[..count].to_vec(), seq))
}

fn batches(engine: &mut atomik_engine::Engine, n: usize, block: usize) {
    render_blocks(engine, &[], 2, n * BATCH, block);
}

#[test]
fn values_are_published_after_every_batch_with_a_sequence() {
    let c = cfg(0, 2);
    let (mut engine, mut handle) = new_engine_with_arena(c, EngineOptions::default(), 8);
    handle.insert_device(id(1), Meterer::boxed(3), &[]).unwrap();
    route(&mut handle, &[1]);
    assert_eq!(read(&handle, 1), None, "nothing before the first batch");
    batches(&mut engine, 1, BATCH);
    assert_eq!(read(&handle, 1), Some((vec![100.0, 101.0, 102.0], 1)));
    batches(&mut engine, 4, BATCH);
    assert_eq!(read(&handle, 1), Some((vec![500.0, 501.0, 502.0], 5)));
    // A short buffer truncates; the shared Meters gives the same answer.
    let mut two = [0.0f32; 2];
    assert_eq!(handle.device_meter_values(id(1), &mut two), Some((2, 5)));
    assert_eq!(two, [500.0, 501.0]);
    assert_eq!(handle.meters().device_values(id(1), &mut two), Some((2, 5)));
}

#[test]
fn count_is_capped_at_max_meter_values() {
    let c = cfg(0, 2);
    let (mut engine, mut handle) = new_engine_with_arena(c, EngineOptions::default(), 8);
    handle
        .insert_device(id(1), Meterer::boxed(MAX_METER_VALUES + 50), &[])
        .unwrap();
    route(&mut handle, &[1]);
    batches(&mut engine, 1, BATCH);
    let (values, _) = read(&handle, 1).unwrap();
    assert_eq!(values.len(), MAX_METER_VALUES);
    assert_eq!(values[MAX_METER_VALUES - 1], 100.0 + 31.0);
}

#[test]
fn default_devices_publish_nothing() {
    let c = cfg(0, 2);
    let (mut engine, mut handle) = new_engine_with_arena(c, EngineOptions::default(), 8);
    handle
        .insert_device(id(1), ConstantSource::boxed(0.5), &[])
        .unwrap();
    let silent = Meterer {
        count: 4,
        batches: 0,
        opt_in: false,
    };
    handle.insert_device(id(2), Box::new(silent), &[]).unwrap();
    route(&mut handle, &[1, 2]);
    batches(&mut engine, 3, BATCH);
    assert_eq!(read(&handle, 1), None);
    assert_eq!(read(&handle, 2), None, "no opt-in, no calls, no values");
}

#[test]
fn removal_clears_and_a_reused_slot_never_shows_old_values() {
    let c = cfg(0, 2);
    let (mut engine, mut handle) = new_engine_with_arena(c, EngineOptions::default(), 1);
    handle.insert_device(id(1), Meterer::boxed(3), &[]).unwrap();
    route(&mut handle, &[1]);
    batches(&mut engine, 2, BATCH);
    let (_, seq_before) = read(&handle, 1).unwrap();
    // Unroute, remove: the values are gone.
    route(&mut handle, &[]);
    handle.remove_device(id(1)).unwrap();
    batches(&mut engine, 1, BATCH);
    assert_eq!(read(&handle, 1), None);
    handle.collect_garbage();
    // A plain device takes the only arena slot: nothing to show.
    handle
        .insert_device(id(2), ConstantSource::boxed(0.0), &[])
        .unwrap();
    batches(&mut engine, 1, BATCH);
    assert_eq!(read(&handle, 1), None);
    assert_eq!(read(&handle, 2), None);
    route(&mut handle, &[]);
    handle.remove_device(id(2)).unwrap();
    batches(&mut engine, 1, BATCH);
    handle.collect_garbage();
    // A metering device reuses the slot: only its own values.
    handle.insert_device(id(3), Meterer::boxed(1), &[]).unwrap();
    route(&mut handle, &[3]);
    batches(&mut engine, 1, BATCH);
    assert_eq!(read(&handle, 1), None);
    assert_eq!(read(&handle, 2), None);
    let (values, seq) = read(&handle, 3).unwrap();
    assert_eq!(values, vec![100.0], "its own count, not the old one's");
    assert!(seq > seq_before, "the sequence never goes backwards");
}

#[test]
fn values_are_identical_across_host_block_sizes_and_worker_counts() {
    let run = |threads: u16, block: usize| {
        let c = cfg(0, 2);
        let (mut engine, mut handle) = engine_with(c, threads, None);
        handle.insert_device(id(1), Meterer::boxed(5), &[]).unwrap();
        handle.insert_device(id(2), Meterer::boxed(2), &[]).unwrap();
        route(&mut handle, &[1, 2]);
        batches(&mut engine, 8, block);
        (read(&handle, 1).unwrap(), read(&handle, 2).unwrap())
    };
    let want = run(0, BATCH);
    assert_eq!(want.0.1, 8);
    for threads in [0u16, 2] {
        for block in [1, BATCH, 8 * BATCH] {
            assert_eq!(run(threads, block), want, "threads {threads} block {block}");
        }
    }
}

#[test]
fn publishing_allocates_on_no_thread() {
    let c = cfg(0, 2);
    for threads in [0u16, 2] {
        let baseline = worker_violations();
        let (mut engine, mut handle) = engine_with(c, threads, Some(forbid_on_this_thread));
        handle.insert_device(id(1), Meterer::boxed(8), &[]).unwrap();
        handle
            .insert_device(id(2), Meterer::boxed(32), &[])
            .unwrap();
        route(&mut handle, &[1, 2]);
        // `render_blocks` renders under `assert_no_alloc`.
        render_blocks(&mut engine, &[], 2, 40 * BATCH, 100);
        route(&mut handle, &[1]);
        handle.remove_device(id(2)).unwrap();
        render_blocks(&mut engine, &[], 2, 4 * BATCH, 100);
        assert_eq!(read(&handle, 2), None);
        assert!(read(&handle, 1).is_some());
        assert_eq!(worker_violations(), baseline, "threads {threads}");
        handle.collect_garbage();
    }
}
