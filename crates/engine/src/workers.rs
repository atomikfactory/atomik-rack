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
//! * **When the workers are used.** A level of one device is never dispatched. For a live
//!   engine (`EngineOptions::realtime`) the workers are used only while the audio thread needs
//!   help (`crate::load_gate`): the plan is *busy* from 30 % of the batch budget at once and
//!   *calm* after a second under 20 %; it is busy while a cost is unknown, for a second after
//!   the transport starts and, once it has needed help while playing, for as long as it plays.
//!   A calm plan runs every level inline and wakes no worker. The node costs are timed on every
//!   batch while the plan is calm and, while it is busy, on every batch where they ran plus one
//!   inline batch in 64, taken only if the batch before it, timed where it ran, was under half
//!   the budget (a run on a worker costs more than the same run at home). The clock is the
//!   per-device timing's when it is on, else the engine's own. An offline engine has no
//!   deadline and hands every level of two or more devices over. The decision never reaches
//!   the signal.
//! * **Idle devices.** A node that will be skipped (`Node::skippable`: a quiescent device with silent inputs, see
//!   `crate::plan`) stays on the audio thread, where it costs one `idle_advance`. The dispatch decision of a level
//!   is taken before its gather (the inputs of the level are not known before), so `run_level`
//!   gathers first and then hands over only the nodes that really run, at the positions it would have given them
//!   without the skip, and raises the callback only if there is one. A level in which nothing runs wakes no worker,
//!   and `dispatchable` follows what was really handed over; the batch in which idle nodes wake (a note, Play, sound
//!   arriving) hands over exactly the nodes it would hand over without the skip. The load figure is not changed by
//!   the skip: a skipped node keeps the cost of its last real run, so the gate stands where it would have stood
//!   (counting skipped nodes as free would decide the waking batch as calm and run it late). When a node that was
//!   skipped until now has a note, a playback batch or a transport change, the callback is raised at the start of the
//!   batch (`idle_node_wakes`); a node woken by sound from upstream wakes the workers at its own level.
//! * **Distribution.** Item `j` of a dispatched level of `n > 1` devices goes to participant
//!   `j % p`, `p = min(workers + 1, n)`; participant 0 is the audio thread. Only workers that
//!   received work are unparked. If a job queue is full the node simply runs on the audio
//!   thread.
//! * **Barrier.** The audio thread counts outstanding jobs and waits (`spin_loop` for a bounded
//!   number of empty iterations, then `yield_now`) until every
//!   node has come back through the done queues. The queues' release/acquire handoff makes
//!   the workers' writes to a node visible to the audio thread. Every barrier iteration that
//!   found nothing to collect is counted (xrun diagnostics: `Engine::last_render_barrier_spins`);
//!   the count is telemetry only and never influences rendering.
//! * **Parking.** An idle worker spins [`WORKER_SPIN_LIMIT`] times (or the engine's
//!   `EngineOptions::worker_linger_spins`), then parks. `unpark` keeps a token, so a wake-up
//!   that races the decision to park is never lost. A linger longer than the callback period
//!   keeps workers awake while a callback runs, so no level depends on an OS wake-up.
//! * **Callback flag.** A worker spins only while a render call that dispatches a level is in
//!   progress: the audio thread raises an atomic flag at the start of such a call
//!   ([`Pool::begin_callback`]; only when the plan is busy and a level has a device a worker
//!   can take), unparks every live worker so the wake-up overlaps the call's serial work, and
//!   lowers the flag at its end ([`Pool::end_callback`]). A level handed over in the middle of
//!   a call (the plan turned busy since the call began) raises it itself
//!   ([`Pool::ensure_callback`]); a calm plan leaves every worker parked. A worker that finds
//!   the flag down parks at once, so a long linger costs nothing between callbacks or while
//!   the transport is stopped. Both calls are atomics and `unpark`: no allocation, no lock.
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
use crate::load_gate::{GateMode, LoadGate, PlanLoad};
use crate::plan::{Node, count_overflow};
use crate::timing::{ClockFn, CostProbe};
use atomik_core::{Batch, TimeInfo};
use rtrb::{Consumer, Producer, PushError, RingBuffer};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::{JoinHandle, Thread};

/// Busy-wait iterations an idle worker performs before it parks. Bounded, so an idle
/// engine costs no CPU; long enough to bridge the gap between levels of one batch.
#[doc(hidden)]
pub const WORKER_SPIN_LIMIT: u64 = 20_000;

/// Barrier iterations without progress the audio thread busy-waits before it starts yielding
/// its core (a worker that was preempted, or shares the core, then gets to run).
#[doc(hidden)]
pub const BARRIER_SPIN_LIMIT: u64 = 2_000;

/// How the barrier waits for the next collected node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pause {
    Spin,
    Yield,
}

/// The wait policy of the level barrier: spin while the node is probably about to arrive, then
/// yield on every iteration. Pure (no clock), so the policy is testable.
pub(crate) fn barrier_pause(since_progress: u64) -> Pause {
    if since_progress <= BARRIER_SPIN_LIMIT {
        Pause::Spin
    } else {
        Pause::Yield
    }
}

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
    probe: Option<CostProbe>,
    /// The engine's idle-skip switch of this batch.
    idle_skip: bool,
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
    /// A render call that uses the pool is in progress: idle workers may spin (up to
    /// `linger`); otherwise they park.
    callback_active: AtomicBool,
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
    /// Keep `callback_active` raised (see [`Pool::set_always_active`]; audio side only).
    always_active: bool,
    /// A render call that uses the pool is in progress (audio side only).
    in_callback: bool,
    /// How the engine decides when to use the workers (see [`GateMode`]; audio side only).
    pub(crate) gate_mode: GateMode,
    /// The engine serves a live host with a deadline (`EngineOptions::realtime`).
    realtime: bool,
    /// The load rule's state (audio side only).
    pub(crate) load: LoadGate,
    /// The clock that times nodes for the load rule when the per-device timing is off.
    pub(crate) cost_clock: ClockFn,
}

impl Pool {
    /// Spawns `threads` workers; each calls `on_thread_start` before anything else. If the OS
    /// refuses a thread the pool degrades to the workers it got (see [`Pool::len`]); it is an
    /// error only when it got none.
    pub(crate) fn new(
        threads: u16,
        on_thread_start: Option<fn()>,
        linger: u64,
        realtime: bool,
    ) -> Result<Self, PoolError> {
        // Read the origin of the gate's clock once here, off the audio thread.
        let _ = crate::timing::own_clock();
        let shared = Arc::new(Shared {
            shutdown: AtomicBool::new(false),
            idle_spins: AtomicU64::new(0),
            parks: AtomicU64::new(0),
            linger,
            callback_active: AtomicBool::new(false),
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
        Ok(Self {
            links,
            shared,
            always_active: false,
            in_callback: false,
            gate_mode: crate::load_gate::default_gate_mode(),
            realtime,
            load: LoadGate::new(),
            cost_clock: crate::timing::own_clock,
        })
    }

    /// Marks the start of a render call that will use the pool: idle workers may spin from
    /// now on, and every live one is woken so it is running when the first level needs it.
    /// Atomics and `unpark` only (audio thread).
    pub(crate) fn begin_callback(&mut self) {
        self.in_callback = true;
        self.shared.callback_active.store(true, Ordering::Release);
        for link in self.links.iter().filter(|l| !l.dead) {
            link.thread.unpark();
        }
    }

    /// Marks the end of the render call: a worker that runs out of jobs parks instead of
    /// spinning. Stays raised while [`Pool::set_always_active`] is on.
    pub(crate) fn end_callback(&mut self) {
        self.in_callback = false;
        if !self.always_active {
            self.shared.callback_active.store(false, Ordering::Release);
        }
    }

    /// Starts the callback in the middle of a render call, when the first level of it that
    /// is worth a hand-over comes up (a level turned to dispatch since the call began). Does
    /// nothing while the call is already marked. Audio thread.
    fn ensure_callback(&mut self) {
        if !self.in_callback {
            self.begin_callback();
        }
    }

    /// Test and benchmark switch: keep the callback flag raised for good, so idle workers spin
    /// for their whole linger between callbacks (the behaviour before the flag existed).
    pub(crate) fn set_always_active(&mut self, on: bool) {
        self.always_active = on;
        self.shared.callback_active.store(on, Ordering::Release);
    }

    /// The rule in force: the mode, with `Auto` resolved for the host.
    pub(crate) fn mode(&self) -> GateMode {
        self.gate_mode.resolve(self.realtime)
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

    pub(crate) fn counters(&self) -> WorkerCounters {
        WorkerCounters {
            shared: Arc::clone(&self.shared),
            threads: self.links.len(),
        }
    }
}

/// The counters of a worker pool, readable from another thread (the control side).
#[derive(Clone)]
pub(crate) struct WorkerCounters {
    shared: Arc<Shared>,
    threads: usize,
}

impl WorkerCounters {
    pub(crate) fn stats(&self) -> WorkerStats {
        WorkerStats {
            threads: self.threads,
            linger: self.shared.linger,
            idle_spins: self.shared.idle_spins.load(Ordering::Relaxed),
            parks: self.shared.parks.load(Ordering::Relaxed),
        }
    }
}

/// What the render workers have done: how many there are, the idle spins each performs before
/// parking, and (summed over them, counted when a worker parks) the idle spins and the parks so
/// far.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WorkerStats {
    /// Worker threads running.
    pub threads: usize,
    /// Idle spins a worker performs before it parks.
    pub linger: u64,
    /// Busy-wait iterations the workers performed while idle.
    pub idle_spins: u64,
    /// Times a worker parked.
    pub parks: u64,
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

/// Worker main loop: run jobs; when idle spin a bounded number of times while a callback is
/// active, then park (at once when none is).
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
            job.panicked = node.run(time, stage, last, job.probe, job.idle_skip);
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
        if spins < shared.linger && shared.callback_active.load(Ordering::Acquire) {
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
    /// Whether any level holds two or more devices (what [`GateMode::ForceDispatch`] hands to
    /// the pool).
    pub(crate) fn has_parallel_level(&self) -> bool {
        self.level_starts.windows(2).any(|w| w[1] - w[0] > 1)
    }

    /// Whether a render call would hand work to the pool: the rule forces it, or the plan is
    /// busy and some level has a device a worker could take. A calm plan leaves the workers
    /// parked.
    pub(crate) fn wants_workers(&self, pool: &Pool) -> bool {
        match pool.mode() {
            GateMode::ForceInline => false,
            GateMode::ForceDispatch => self.has_parallel_level(),
            _ => pool.load.busy() && self.dispatchable && self.has_parallel_level(),
        }
    }

    /// What the plan costs in one batch: every device's cost, counted at its last stage.
    fn plan_load(&self, arena: &Arena) -> PlanLoad {
        let mut load = PlanLoad::default();
        for step in self.steps.iter().filter(|step| step.last) {
            if let Some(Some(node)) = arena.nodes.get(self.devices[step.device].slot) {
                load.add(node.cost(), node.seen());
            }
        }
        load
    }

    /// Whether a device that was skipped until now has an event this batch (a note, a playback
    /// batch, a transport change): the plan is about to need the workers. Costs one bool test
    /// per node; read only while the workers are not awake.
    fn idle_node_wakes(&self, arena: &Arena, time: &TimeInfo) -> bool {
        self.steps.iter().filter(|step| step.last).any(|step| {
            matches!(
                arena.nodes.get(self.devices[step.device].slot),
                Some(Some(node)) if node.wakes_by_event(time)
            )
        })
    }

    /// How many devices of steps `a..b` a worker could take: those `run_level` would hand over,
    /// that is not at the audio thread's own positions and not pinned to it.
    fn sendable(&self, a: usize, b: usize, pool: &Pool, arena: &Arena) -> usize {
        let participants = (pool.links.len() + 1).min(b - a);
        self.steps[a..b]
            .iter()
            .enumerate()
            .filter(|(j, step)| {
                j % participants != 0
                    && matches!(
                        arena.nodes.get(self.devices[step.device].slot),
                        Some(Some(node)) if !node.device.pinned_to_audio_thread()
                    )
            })
            .count()
    }

    /// Like `process`, with the devices of each level spread over the audio thread and the
    /// pool's workers when the rule says so (see [`crate::load_gate`]); otherwise every level
    /// runs inline. Never allocates, frees, locks or blocks (the audio thread only spins
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
        let mode = pool.mode();
        // The load rule: decided from the costs of the batches before this one, which this
        // batch may time again: every batch while the plan is calm (so a rise is seen at once)
        // and one inline batch in 64 while it is busy.
        let mut busy = true;
        let mut probing = false;
        let mut probe = None;
        if mode == GateMode::Load {
            let budget = crate::timing::batch_budget_ns(time.sample_rate.hz());
            busy = pool
                .load
                .update(self.plan_load(arena), budget, time.playing);
            probing = busy && pool.load.probe_due();
            probe = Some(CostProbe {
                clock: pool.cost_clock,
                due: !busy || probing,
                lower: busy && !probing,
            });
        }
        // Idle devices keep the workers parked (`run_level` wakes them only for a node that
        // really runs). When one wakes by an event the plan knows now, the workers are woken at
        // the start of the batch, as a plan that was never idle has them awake from the start
        // of the render call, so the wake-up still overlaps the batch's serial work.
        if mode == GateMode::Load
            && busy
            && !probing
            && !pool.in_callback
            && arena.idle_skip
            && self.has_parallel_level()
            && self.idle_node_wakes(arena, &time)
        {
            pool.ensure_callback();
        }
        let mut spins = 0u64;
        let mut any = false;
        for level in 0..self.level_starts.len().saturating_sub(1) {
            let (a, b) = (self.level_starts[level], self.level_starts[level + 1]);
            let dispatch = b - a > 1
                && !pool.links.is_empty()
                && match mode {
                    GateMode::ForceInline => false,
                    GateMode::ForceDispatch => true,
                    _ => busy && !probing && self.sendable(a, b, pool, arena) > 0,
                };
            if dispatch {
                // The level gathers first; only a node that really runs is handed over, and
                // the workers are woken only if one is (see `run_level`).
                let (level_spins, handed) =
                    self.run_level(a, b, time, dropped, pool, arena, meters, fault, probe);
                any |= handed;
                spins = spins.saturating_add(level_spins);
            } else {
                self.run_sequential(a, b, time, dropped, arena, meters, fault, probe);
            }
        }
        if mode == GateMode::Load && busy && !probing {
            self.dispatchable = any;
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
        probe: Option<CostProbe>,
    ) -> (u64, bool) {
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
        let idle_skip = arena.idle_skip;
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
        // A node that will be skipped (a quiescent device with silent inputs: the same pure
        // question `Node::run` asks) stays home and costs nothing, so it is not counted as work
        // for the workers. They are woken only when a node at a worker's position really runs:
        // a level of idle devices wakes nobody, and a level in which they wake (a note, Play,
        // sound arriving) hands over exactly the nodes it would hand over without the skip.
        let handing = level.iter().enumerate().any(|(j, step)| {
            j % participants != 0
                && matches!(
                    arena.nodes.get(slot_of(step)),
                    Some(Some(node)) if !(idle_skip && node.skippable(&time))
                )
        });
        if handing {
            pool.ensure_callback();
        }
        let mut handed = false;
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
                if slot.as_ref().is_none_or(|node| {
                    node.device.pinned_to_audio_thread() || (idle_skip && node.skippable(&time))
                }) {
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
                    probe,
                    idle_skip,
                    panicked: false,
                };
                match link.jobs.push(job) {
                    Ok(()) => {
                        outstanding += 1;
                        link.pending += 1;
                        sent = true;
                        handed = true;
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
                && node.run(time, step.stage, step.last, probe, idle_skip)
            {
                fault.record(devices[step.device].id);
            }
        }
        // 4. Barrier: wait until every handed-over node is back.
        let mut spins = 0u64;
        let mut waited = 0u64;
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
            if progressed {
                waited = 0;
            } else {
                spins += 1;
                waited += 1;
                match barrier_pause(waited) {
                    Pause::Spin => std::hint::spin_loop(),
                    Pause::Yield => std::thread::yield_now(),
                }
            }
        }
        // 5. Scatter in plan order.
        for step in level {
            if let Some(node) = arena.node_mut(slot_of(step)) {
                node.scatter(&step.routes, slots, note_slots, meters);
            }
        }
        (spins, handed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_barrier_spins_a_bounded_number_of_times_then_yields() {
        assert_eq!(barrier_pause(1), Pause::Spin);
        assert_eq!(barrier_pause(BARRIER_SPIN_LIMIT), Pause::Spin);
        assert_eq!(barrier_pause(BARRIER_SPIN_LIMIT + 1), Pause::Yield);
        assert_eq!(barrier_pause(u64::MAX), Pause::Yield);
    }
}
