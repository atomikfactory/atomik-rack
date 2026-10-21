//! A device that answers `pinned_to_audio_thread()` with true runs on
//! the thread that calls `Engine::render`, whatever the worker count; an unpinned one is still
//! spread over the workers, and flipping the answer at run time takes effect at the next batch.

atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::ParamId;
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceKind, Direction, PrepareInfo, ProcessContext, SocketDescriptor,
    SocketId, SocketKind,
};
use atomik_engine::{DeviceInstanceId, Engine, EngineHandle, Topology};
use common::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::ThreadId;

static PROBE: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "test.thread-probe",
    name: "Thread Probe",
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

type Log = Arc<Mutex<Vec<ThreadId>>>;

struct Probe {
    pinned: Arc<AtomicBool>,
    log: Log,
}

impl Device for Probe {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &PROBE
    }
    fn prepare(&mut self, _: &PrepareInfo) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _: ParamId, _: f32) {}
    fn process(&mut self, _: &mut ProcessContext<'_>) {
        // Preallocated: pushing inside the capacity does not allocate.
        self.log.lock().unwrap().push(std::thread::current().id());
    }
    fn pinned_to_audio_thread(&self) -> bool {
        self.pinned.load(Ordering::Relaxed)
    }
}

struct Rig {
    engine: Engine,
    handle: EngineHandle,
    flags: Vec<Arc<AtomicBool>>,
    logs: Vec<Log>,
}

/// `n` independent probes (one dependency level) on an engine with `threads` workers; device
/// `i` starts pinned when `pinned(i)`.
fn rig(n: usize, threads: u16, pinned: impl Fn(usize) -> bool) -> Rig {
    let c = cfg(0, 2);
    let (engine, mut handle) = engine_with(c, threads, None);
    let mut topo = Topology::new(&c);
    let (mut flags, mut logs) = (Vec::new(), Vec::new());
    for i in 0..n {
        let flag = Arc::new(AtomicBool::new(pinned(i)));
        let log: Log = Arc::new(Mutex::new(Vec::with_capacity(4_000)));
        // Apple's `std` mutex boxes its pthread mutex on the first lock: take it once outside the
        // no-alloc render section.
        drop(log.lock());
        let id = DeviceInstanceId(i as u32 + 1);
        handle
            .insert_device(
                id,
                Box::new(Probe {
                    pinned: Arc::clone(&flag),
                    log: Arc::clone(&log),
                }),
                &[],
            )
            .unwrap();
        topo.add_node(id, &PROBE).unwrap();
        flags.push(flag);
        logs.push(log);
    }
    handle.submit_routing(topo.compile().unwrap()).unwrap();
    Rig {
        engine,
        handle,
        flags,
        logs,
    }
}

fn batches(e: &mut Engine, n: usize) {
    for _ in 0..n {
        render_blocks(e, &[], 2, 64, 64);
    }
}

fn threads_of(log: &Log) -> Vec<ThreadId> {
    log.lock().unwrap().clone()
}

#[test]
fn pinned_devices_never_leave_the_calling_thread_for_any_worker_count() {
    for workers in [0u16, 1, 2, 4] {
        let mut r = rig(8, workers, |i| i % 2 == 0);
        batches(&mut r.engine, 200);
        let me = std::thread::current().id();
        for (i, log) in r.logs.iter().enumerate() {
            let t = threads_of(log);
            assert_eq!(t.len(), 200, "workers {workers} device {i} ran every batch");
            if i % 2 == 0 {
                assert!(
                    t.iter().all(|id| *id == me),
                    "workers {workers}: pinned device {i} left the audio thread"
                );
            }
        }
        if workers > 0 {
            // The test bites: unpinned devices do use the workers.
            let off_thread = r
                .logs
                .iter()
                .enumerate()
                .filter(|(i, _)| i % 2 == 1)
                .any(|(_, log)| threads_of(log).iter().any(|id| *id != me));
            assert!(
                off_thread,
                "workers {workers}: no unpinned device ran on a worker"
            );
        }
        drop(r.handle);
    }
}

#[test]
fn flipping_the_pin_at_run_time_applies_from_the_next_batch() {
    let mut r = rig(6, 2, |_| false);
    let me = std::thread::current().id();
    batches(&mut r.engine, 50);
    let ran_off = |r: &Rig| r.logs.iter().flat_map(threads_of).any(|id| id != me);
    assert!(ran_off(&r), "unpinned devices use the workers");
    for log in &r.logs {
        log.lock().unwrap().clear();
    }
    for f in &r.flags {
        f.store(true, Ordering::Relaxed);
    }
    batches(&mut r.engine, 50);
    assert!(!ran_off(&r), "every device is pinned now");
    for f in &r.flags {
        f.store(false, Ordering::Relaxed);
    }
    for log in &r.logs {
        log.lock().unwrap().clear();
    }
    batches(&mut r.engine, 50);
    assert!(ran_off(&r), "unpinning gives the workers their share back");
}
