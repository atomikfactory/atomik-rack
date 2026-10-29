//! Multicore rendering: a pool of worker threads that run the devices of
//! one dependency level alongside the audio thread.
//!
//! # Design (safe Rust, no locks, no allocation while rendering)
//!
//! * **Arena nodes.** Nodes live in the engine's device arena, not in the plan; a
//!   job carries the node's arena slot and the node goes back into that slot. A routing swap
//!   only happens at a batch boundary, when every node is home.
//! * **Ownership handoff, not shared mutation.** Each arena node is a `Box<Node>`. For a
//!   parallel level the audio thread *moves* the boxes of the worker share into per-worker
//!   `rtrb` SPSC job queues (a pointer move: no allocation), and the workers move them back
//!   through per-worker done queues. A node is therefore owned by exactly one thread at any
//!   time, and the compiler checks it: no `unsafe`, no `Mutex`, no `UnsafeCell`.
//! * **Gather / run / scatter.** Everything that touches the plan's shared slots runs on the
//!   audio thread: before a level, every node of the level *gathers* its inputs from the slots
//!   into its own scratch; then the nodes *run* (on any thread; `run` touches only the node);
//!   after every node is back, they *scatter* their outputs to the slots in plan order. Within
//!   a level no device reads another's output, and slot liveness is computed on the
//!   level-major order, so gather-all-then-scatter-all is equivalent to the sequential path.
//! * **Pinned devices.** A device whose `Device::pinned_to_audio_thread` answers true is never
//!   handed to a worker: it keeps its share position but runs on the audio thread (step 3 of a
//!   level), for plugins that assume a stable thread. The answer is read on the audio thread
//!   before each dispatch, so it may change while running.
//! * **Distribution.** Item `j` of a level with `n > 1` devices goes to participant
//!   `j % p`, `p = min(workers + 1, n)`; participant 0 is the audio thread. Only workers that
//!   received work are unparked, and nothing is dispatched for levels of one device. If a job
//!   queue is full the node simply runs on the audio thread.
//! * **Barrier.** The audio thread counts outstanding jobs and spins (`spin_loop`) until every
//!   node has come back through the done queues. The queues' release/acquire handoff makes
//!   the workers' writes to a node visible to the audio thread. Every barrier iteration that
//!   found nothing to collect is counted (xrun diagnostics: `Engine::last_render_barrier_spins`);
//!   the count is telemetry only and never influences rendering.
//! * **Parking.** An idle worker spins [`WORKER_SPIN_LIMIT`] times (or the engine's
//!   `EngineOptions::worker_linger_spins`), then parks. `unpark` keeps a token, so a wake-up
//!   that races the decision to park is never lost. A linger longer than the callback period
//!   keeps workers awake while audio runs, so no callback depends on an OS wake-up.
//! * **Determinism.** A device's output depends only on its own state and its gathered
//!   inputs, which are identical whatever thread runs it; the scatter order is the plan order.
//!   So output is bit-identical for any worker count.
//! * **Panics.** A device panic is caught where the device runs (`Node::run`, `catch_unwind`,
//!   free when nothing panics): the node is marked faulted and silent, still returned to the
//!   audio thread, and the audio thread records the engine's sticky fault. Nothing is re-raised.
//!   A worker thread that dies outside a job loses the nodes it owned; those devices are
//!   recorded as faulted and the worker is no longer used.

use crate::arena::{Arena, RoutingPlan};
use crate::console::Meters;
use crate::fault::FaultState;
use crate::plan::{Node, count_overflow};
use atomik_core::{Batch, TimeInfo};
use rtrb::{Consumer, Producer, PushError, RingBuffer};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::{JoinHandle, Thread};

/// Busy-wait iterations an idle worker performs before it parks. Bounded, so an idle
/// engine costs no CPU; long enough to bridge the gap between levels of one batch.
#[doc(hidden)]
pub const WORKER_SPIN_LIMIT: u64 = 20_000;

/// Jobs each worker queue holds; a bigger worker share falls back to the audio thread.
const WORKER_QUEUE_CAPACITY: usize = 256;

/// One device to run for one batch.
pub(crate) struct Job {
    index: usize,
    node: Box<Node>,
    time: TimeInfo,
    /// The device stage to run and whether it is the device's last.
    stage: u8,
    last: bool,
    panicked: bool,
}

struct Shared {
    shutdown: AtomicBool,
    /// Busy-wait iterations performed by all workers, flushed when a worker parks.
    idle_spins: AtomicU64,
    /// Times a worker parked (diagnostics).
    parks: AtomicU64,
    /// Idle spins before parking.
    linger: u64,
}

struct Link {
    jobs: Producer<Job>,
    done: Consumer<Job>,
    thread: Thread,
    handle: Option<JoinHandle<()>>,
    /// The thread exited outside a job: it is no longer given work (audio thread only).
    dead: bool,
    /// Jobs handed to this worker and not yet returned (audio thread only).
    pending: usize,
}

/// Why a worker pool could not be created.
#[derive(Debug, thiserror::Error)]
pub enum PoolError {
    /// The operating system refused to start a render worker thread.
    #[error("cannot start a render worker thread: {0}")]
    Spawn(#[source] std::io::Error),
}

/// The worker threads of one engine. Created and dropped on the non-RT side.
pub(crate) struct Pool {
    links: Vec<Link>,
    shared: Arc<Shared>,
}

impl Pool {
    /// Spawns `threads` workers; each calls `on_thread_start` before anything else. If the OS
    /// refuses a thread the pool degrades to the workers it got (see [`Pool::len`]); it is an
    /// error only when it got none.
    pub(crate) fn new(
        threads: u16,
        on_thread_start: Option<fn()>,
        linger: u64,
    ) -> Result<Self, PoolError> {
        let shared = Arc::new(Shared {
            shutdown: AtomicBool::new(false),
            idle_spins: AtomicU64::new(0),
            parks: AtomicU64::new(0),
            linger,
        });
        let mut links = Vec::with_capacity(usize::from(threads));
        let mut first_error = None;
        for k in 0..threads {
            let (jobs, jobs_rx) = RingBuffer::new(WORKER_QUEUE_CAPACITY);
            let (done_tx, done) = RingBuffer::new(WORKER_QUEUE_CAPACITY);
            let worker_shared = Arc::clone(&shared);
            let spawned = std::thread::Builder::new()
                .name(format!("atomik-render-{}", k + 1))
                .spawn(move || worker(jobs_rx, done_tx, &worker_shared, on_thread_start));
            match spawned {
                Ok(handle) => links.push(Link {
                    jobs,
                    done,
                    thread: handle.thread().clone(),
                    handle: Some(handle),
                    dead: false,
                    pending: 0,
                }),
                Err(e) => {
                    first_error = Some(e);
                    break;
                }
            }
        }
        if links.is_empty()
            && let Some(source) = first_error
        {
            return Err(PoolError::Spawn(source));
        }
        Ok(Self { links, shared })
    }

    pub(crate) fn len(&self) -> usize {
        self.links.len()
    }

    pub(crate) fn idle_spins(&self) -> u64 {
        self.shared.idle_spins.load(Ordering::Relaxed)
    }

    pub(crate) fn parks(&self) -> u64 {
        self.shared.parks.load(Ordering::Relaxed)
    }

    pub(crate) fn linger(&self) -> u64 {
        self.shared.linger
    }
}

impl Drop for Pool {
    /// Stops and joins every worker (non-RT, by contract the engine is dropped off the audio
    /// thread). Workers hold no node between batches, so nothing is lost.
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Ordering::Release);
        for link in &self.links {
            link.thread.unpark();
        }
        for link in &mut self.links {
            if let Some(handle) = link.handle.take() {
                let _ = handle.join();
            }
        }
    }
}

/// Worker main loop: run jobs; when idle spin a bounded number of times, then park.
fn worker(
    mut jobs: Consumer<Job>,
    mut done: Producer<Job>,
    shared: &Shared,
    on_thread_start: Option<fn()>,
) {
    if let Some(start) = on_thread_start {
        start();
    }
    let mut spins = 0u64;
    let mut unflushed = 0u64;
    loop {
        if let Ok(mut job) = jobs.pop() {
            let (node, time, stage, last) = (&mut job.node, job.time, job.stage, job.last);
            job.panicked = node.run(time, stage, last);
            // At most WORKER_QUEUE_CAPACITY jobs are in flight per worker, so this fits.
            while let Err(PushError::Full(back)) = done.push(job) {
                job = back;
                std::hint::spin_loop();
            }
            spins = 0;
            continue;
        }
        if shared.shutdown.load(Ordering::Acquire) {
            break;
        }
        if spins < shared.linger {
            spins += 1;
            unflushed += 1;
            std::hint::spin_loop();
            continue;
        }
        shared.idle_spins.fetch_add(unflushed, Ordering::Relaxed);
        shared.parks.fetch_add(1, Ordering::Relaxed);
        unflushed = 0;
        std::thread::park();
        spins = 0;
    }
    shared.idle_spins.fetch_add(unflushed, Ordering::Relaxed);
}

impl RoutingPlan {
    /// Like `process`, with the devices of each level spread over the audio thread and the
    /// pool's workers. Never allocates, frees, locks or blocks (the audio thread only spins
    /// while waiting for the level's last device). Returns the barrier spins: busy-wait
    /// iterations in which the audio thread found no finished node to collect.
    ///
    /// A device that panics is caught and recorded in `fault`; nothing is re-raised.
    #[allow(clippy::too_many_arguments)] // Justification: one call site; all are borrowed state.
    pub(crate) fn process_parallel(
        &mut self,
        time: TimeInfo,
        host_in: &[Batch],
        host_out: &mut [Batch],
        dropped: &AtomicU64,
        pool: &mut Pool,
        arena: &mut Arena,
        meters: &Meters,
        fault: &FaultState,
    ) -> u64 {
        self.read_host_inputs(host_in, meters);
        let mut spins = 0u64;
        for level in 0..self.level_starts.len().saturating_sub(1) {
            let (a, b) = (self.level_starts[level], self.level_starts[level + 1]);
            if b - a <= 1 || pool.links.is_empty() {
                self.run_sequential(a, b, time, dropped, arena, meters, fault);
            } else {
                let level_spins = self.run_level(a, b, time, dropped, pool, arena, meters, fault);
                spins = spins.saturating_add(level_spins);
            }
        }
        self.write_host_outputs(host_out);
        spins
    }

    /// Runs one level of `n = b - a > 1` devices in parallel. Nodes move from their arena
    /// slots to the workers and back (the job carries the slot index). Returns the barrier
    /// spins of this level; device panics are recorded in `fault`. Gathering (summing and PDC
    /// delay lines) and scattering (meters) stay on the audio thread.
    #[allow(clippy::too_many_arguments)] // Justification: one call site; all are borrowed state.
    fn run_level(
        &mut self,
        a: usize,
        b: usize,
        time: TimeInfo,
        dropped: &AtomicU64,
        pool: &mut Pool,
        arena: &mut Arena,
        meters: &Meters,
        fault: &FaultState,
    ) -> u64 {
        let Self {
            devices,
            steps,
            slots,
            note_slots,
            spare_notes,
            console,
            ..
        } = self;
        let delays = &mut console.delays;
        // A level holds at most one stage of any device (stage s -> s+1 is an edge), so each
        // arena node moves at most once per level.
        let level = &steps[a..b];
        let slot_of = |step: &crate::plan::Step| devices[step.device].slot;
        // 1. Gather every input of the level before anything of the level runs.
        for step in level {
            if let Some(node) = arena.node_mut(slot_of(step)) {
                let r = &step.routes;
                let overflow = node.gather(r, step.stage, slots, note_slots, spare_notes, delays);
                count_overflow(dropped, overflow);
            }
        }
        // 2. Hand the worker shares over (moving boxes), waking only workers that got work.
        let n = b - a;
        let participants = (pool.links.len() + 1).min(n);
        let mut outstanding = 0usize;
        for (w, link) in pool.links.iter_mut().enumerate().take(participants - 1) {
            if link.dead {
                continue; // its share stays home and runs on the audio thread
            }
            let mut sent = false;
            for step in level.iter().skip(w + 1).step_by(participants) {
                let index = slot_of(step);
                let Some(slot) = arena.nodes.get_mut(index) else {
                    continue;
                };
                // A pinned device stays home and runs in step 3, on the
                // audio thread. Read each batch, so the answer may change at run time.
                if slot
                    .as_ref()
                    .is_none_or(|node| node.device.pinned_to_audio_thread())
                {
                    continue;
                }
                let Some(node) = slot.take() else {
                    continue;
                };
                let job = Job {
                    index,
                    node,
                    time,
                    stage: step.stage,
                    last: step.last,
                    panicked: false,
                };
                match link.jobs.push(job) {
                    Ok(()) => {
                        outstanding += 1;
                        link.pending += 1;
                        sent = true;
                    }
                    // Queue full: the node stays home and runs on the audio thread.
                    Err(PushError::Full(job)) => arena.nodes[job.index] = Some(job.node),
                }
            }
            if sent {
                link.thread.unpark();
            }
        }
        // 3. The audio thread runs whatever stayed home (its share and any overflow).
        for step in level {
            if let Some(node) = arena.node_mut(slot_of(step))
                && node.run(time, step.stage, step.last)
            {
                fault.record(devices[step.device].id);
            }
        }
        // 4. Barrier: wait until every handed-over node is back.
        let mut spins = 0u64;
        while outstanding > 0 {
            let mut progressed = false;
            for link in pool.links.iter_mut().filter(|l| l.pending > 0) {
                while let Ok(job) = link.done.pop() {
                    if job.panicked
                        && let Some(device) = devices.iter().find(|d| d.slot == job.index)
                    {
                        fault.record(device.id);
                    }
                    arena.nodes[job.index] = Some(job.node);
                    outstanding -= 1;
                    link.pending -= 1;
                    progressed = true;
                }
                if link.pending > 0 && link.handle.as_ref().is_some_and(JoinHandle::is_finished) {
                    // The worker died outside a job (its `on_thread_start` panicked): the
                    // nodes it owns can never come back. Their devices are faulted, the
                    // barrier stops waiting for them and the worker gets no more work.
                    link.dead = true;
                    outstanding -= link.pending;
                    link.pending = 0;
                    for step in level {
                        let dev = &devices[step.device];
                        if arena.nodes.get(dev.slot).is_some_and(Option::is_none) {
                            fault.record(dev.id);
                        }
                    }
                    progressed = true;
                }
            }
            if !progressed {
                spins += 1;
                std::hint::spin_loop();
            }
        }
        // 5. Scatter in plan order.
        for step in level {
            if let Some(node) = arena.node_mut(slot_of(step)) {
                node.scatter(&step.routes, slots, note_slots, meters);
            }
        }
        spins
    }
}
