//! Devices and a rig for the idle-skip tests: a `Probe` that counts its calls and answers
//! `quiescent()` as told, a source whose output the test scripts batch by batch, and a
//! multi-stage device. Costs come from a fake clock the probes advance (as in `costed.rs`), so
//! the load rule is decided by what the test says, never by how fast the machine is.
use super::*;
use atomik_core::{BATCH, NoteKind, ParamDescriptor, ParamId, Taper, Tempo, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceKind, Direction, PrepareInfo, ProcessContext, SocketDescriptor,
    SocketId, SocketKind,
};
use atomik_engine::{GateMode, ScheduledNote, TransportCommand};
use atomik_testkit::no_alloc::forbid_on_this_thread;
use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed};

thread_local! {
    /// The fake clock of the thread: it moves only when a probe or a source runs.
    static NOW: Cell<u64> = const { Cell::new(0) };
    /// This thread is a render worker.
    static WORKER: Cell<bool> = const { Cell::new(false) };
}

fn start_worker() {
    forbid_on_this_thread();
    WORKER.with(|w| w.set(true));
}

fn fake_clock() -> u64 {
    NOW.with(Cell::get)
}

pub const AUDIO_IN: SocketId = SocketId(0);
pub const CV_IN: SocketId = SocketId(1);
pub const NOTES_IN: SocketId = SocketId(2);
pub const AUDIO_OUT: SocketId = SocketId(3);

const fn sock(
    key: &'static str,
    id: u16,
    kind: SocketKind,
    direction: Direction,
) -> SocketDescriptor {
    SocketDescriptor {
        id: SocketId(id),
        key,
        name: key,
        kind,
        direction,
    }
}

static GAIN: ParamDescriptor = ParamDescriptor {
    id: ParamId(0),
    key: "gain",
    name: "Gain",
    min: 0.0,
    max: 2.0,
    default: 1.0,
    taper: Taper::Linear,
    unit: Unit::None,
    discrete: false,
};

pub static PROBE: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "test.idle-probe",
    name: "Probe",
    kind: DeviceKind::Utility,
    version: 1,
    params: std::slice::from_ref(&GAIN),
    sockets: &[
        sock("audio_in", 0, SocketKind::Audio, Direction::Input),
        sock("cv_in", 1, SocketKind::Cv, Direction::Input),
        sock("notes_in", 2, SocketKind::Notes, Direction::Input),
        sock("audio_out", 3, SocketKind::Audio, Direction::Output),
    ],
};

/// What the tests read from a device, shared with the engine thread.
#[derive(Default)]
pub struct Counts {
    pub process: AtomicU64,
    pub advance: AtomicU64,
    pub advanced_frames: AtomicU64,
    /// `process` calls made on a render worker.
    pub on_worker: AtomicU64,
}

/// What the tests tell a device while it runs.
pub struct Settings {
    /// The answer of `quiescent()`.
    pub answer: AtomicBool,
    /// Fake nanoseconds one `process` takes.
    pub cost: AtomicU64,
    /// `idle_advance` panics.
    pub panic_idle: AtomicBool,
    /// `process` panics when an input is not silent.
    pub panic_sound: AtomicBool,
}

impl Settings {
    fn new(cost: u64) -> Arc<Self> {
        Arc::new(Self {
            answer: AtomicBool::new(true),
            cost: AtomicU64::new(cost),
            panic_idle: AtomicBool::new(false),
            panic_sound: AtomicBool::new(false),
        })
    }
}

/// Multiplies its audio input by a phase it advances per sample, so a skipped stretch that
/// does not advance the phase exactly as `process` would is heard when sound returns. Silent in,
/// silent out: it may say `quiescent()`.
struct Probe {
    counts: Arc<Counts>,
    set: Arc<Settings>,
    phase: f32,
    inc: f32,
    gain: f32,
    /// `idle_advance` does what `process` does to the phase (false: it does nothing).
    right: bool,
    /// Publishes meter values and a variable latency (both are read after every batch).
    meters: bool,
}

impl Probe {
    fn step(&mut self) {
        self.phase += self.inc;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
    }
}

impl Device for Probe {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &PROBE
    }
    fn prepare(&mut self, _: &PrepareInfo) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _: ParamId, plain: f32) {
        self.gain = plain;
    }
    fn has_meter_values(&self) -> bool {
        self.meters
    }
    fn meter_values(&self, out: &mut [f32]) -> usize {
        out[0] = self.phase;
        1
    }
    fn variable_latency(&self) -> bool {
        self.meters
    }
    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        self.counts.process.fetch_add(1, Relaxed);
        if WORKER.with(Cell::get) {
            self.counts.on_worker.fetch_add(1, Relaxed);
        }
        let cost = self.set.cost.load(Relaxed);
        NOW.with(|n| n.set(n.get() + cost));
        let audio = *ctx.input(AUDIO_IN);
        let cv = *ctx.input(CV_IN);
        let hit = !ctx.notes_in(NOTES_IN).is_empty();
        if self.set.panic_sound.load(Relaxed) && audio.iter().any(|x| x.to_bits() != 0) {
            panic!("probe: sound in");
        }
        let mut out = [0.0f32; BATCH];
        for i in 0..BATCH {
            self.step();
            out[i] = if audio[i].to_bits() == 0 && cv[i].to_bits() == 0 {
                0.0
            } else {
                audio[i] * self.gain * (self.phase * 2.0 - 1.0) + cv[i]
            };
        }
        if hit {
            out[0] += 0.5;
        }
        *ctx.output(AUDIO_OUT) = out;
    }
    fn quiescent(&self) -> bool {
        self.set.answer.load(Relaxed)
    }
    fn idle_advance(&mut self, frames: usize) {
        self.counts.advance.fetch_add(1, Relaxed);
        self.counts
            .advanced_frames
            .fetch_add(frames as u64, Relaxed);
        if self.set.panic_idle.load(Relaxed) {
            panic!("probe: idle_advance");
        }
        if self.right {
            for _ in 0..frames {
                self.step();
            }
        }
    }
}

/// What a source writes.
pub const SILENCE: u32 = 0;
pub const NOISE: u32 = 1;
pub const NEGATIVE_ZERO: u32 = 2;
pub const CV_ONLY: u32 = 3;

pub static SRC: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "test.idle-source",
    name: "Source",
    kind: DeviceKind::Utility,
    version: 1,
    params: &[],
    sockets: &[
        sock("audio_out", 0, SocketKind::Audio, Direction::Output),
        sock("cv_out", 1, SocketKind::Cv, Direction::Output),
    ],
};

/// A source the test scripts: silence, noise, `-0.0`, or a constant CV with silent audio. It
/// never says `quiescent()`.
struct Src {
    mode: Arc<AtomicU32>,
    cost: Arc<Settings>,
    state: u32,
}

impl Device for Src {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &SRC
    }
    fn prepare(&mut self, _: &PrepareInfo) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _: ParamId, _: f32) {}
    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let cost = self.cost.cost.load(Relaxed);
        NOW.with(|n| n.set(n.get() + cost));
        let mode = self.mode.load(Relaxed);
        let mut audio = [0.0f32; BATCH];
        let mut cv = [0.0f32; BATCH];
        match mode {
            NOISE => audio.iter_mut().for_each(|x| {
                self.state = self
                    .state
                    .wrapping_mul(1_664_525)
                    .wrapping_add(1_013_904_223);
                *x = (self.state >> 8) as f32 / (1u32 << 24) as f32 - 0.5;
            }),
            NEGATIVE_ZERO => audio.fill(-0.0),
            CV_ONLY => cv.fill(0.25),
            _ => {}
        }
        *ctx.output(SocketId(0)) = audio;
        *ctx.output(SocketId(1)) = cv;
    }
}

static STAGED: DeviceDescriptor = DeviceDescriptor {
    stages: &[0, 0, 1, 1],
    type_id: "test.idle-staged",
    name: "Staged",
    kind: DeviceKind::Utility,
    version: 1,
    params: &[],
    sockets: &[
        sock("in", 0, SocketKind::Audio, Direction::Input),
        sock("send", 1, SocketKind::Audio, Direction::Output),
        sock("ret", 2, SocketKind::Audio, Direction::Input),
        sock("out", 3, SocketKind::Audio, Direction::Output),
    ],
};

/// A two-stage device that says `quiescent()` all the time: the engine must still call it.
struct Staged {
    counts: Arc<Counts>,
}

impl Device for Staged {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &STAGED
    }
    fn prepare(&mut self, _: &PrepareInfo) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _: ParamId, _: f32) {}
    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        self.process_stage(0, ctx);
    }
    fn process_stage(&mut self, stage: u8, ctx: &mut ProcessContext<'_>) {
        self.counts.process.fetch_add(1, Relaxed);
        match stage {
            0 => *ctx.output(SocketId(1)) = *ctx.input(SocketId(0)),
            _ => *ctx.output(SocketId(3)) = *ctx.input(SocketId(2)),
        }
    }
    fn quiescent(&self) -> bool {
        true
    }
    fn idle_advance(&mut self, _: usize) {
        self.counts.advance.fetch_add(1, Relaxed);
    }
}

/// Which devices make up the plan.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// `width` sources (level 0), each feeding a probe (level 1) that feeds output `k`.
    Sources,
    /// `width` probes with no input (level 0, like instruments that take notes), each feeding a
    /// probe (level 1) that feeds output `k`.
    Chain,
}

/// How the engine decides on its workers.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// Offline: every level of two or more devices is handed over.
    Offline,
    /// Live: the load rule.
    Live,
}

pub struct Options {
    pub layout: Layout,
    pub width: usize,
    pub threads: u16,
    pub rule: Rule,
    /// Fake nanoseconds each device takes per `process`.
    pub cost: u64,
    /// `idle_advance` of every probe does what `process` does (false: nothing).
    pub right: bool,
    pub meters: bool,
    pub skip: bool,
    pub song: bool,
}

impl Options {
    pub fn new(layout: Layout, width: usize) -> Self {
        Self {
            layout,
            width,
            threads: 0,
            rule: Rule::Offline,
            cost: 0,
            right: true,
            meters: false,
            skip: true,
            song: false,
        }
    }
}

pub struct ProbeRef {
    pub id: DeviceInstanceId,
    pub counts: Arc<Counts>,
    pub set: Arc<Settings>,
}

pub struct World {
    pub engine: Engine,
    pub handle: EngineHandle,
    /// The sources' modes (empty for `Layout::Chain`).
    pub srcs: Vec<Arc<AtomicU32>>,
    /// Every probe: for `Sources` the probe of each source, for `Chain` the level-0 probes then
    /// the level-1 probes.
    pub probes: Vec<ProbeRef>,
    /// Batches rendered so far.
    pub batches: u64,
    outputs: usize,
}

pub fn world(o: &Options) -> World {
    let outputs = o.width.max(1);
    let c = cfg(0, outputs as u16);
    let mut g = Graph::new(&c);
    let mut srcs = Vec::new();
    let mut probes = Vec::new();
    let probe = |g: &mut Graph, k: usize, probes: &mut Vec<ProbeRef>| {
        let counts = Arc::new(Counts::default());
        let set = Settings::new(o.cost);
        let id = g.add_device(Box::new(Probe {
            counts: Arc::clone(&counts),
            set: Arc::clone(&set),
            phase: 0.0,
            inc: 0.001 * (k % 7 + 1) as f32 + 0.000_3,
            gain: 1.0,
            right: o.right,
            meters: o.meters,
        }));
        probes.push(ProbeRef { id, counts, set });
        id
    };
    let mut firsts = Vec::new();
    for k in 0..o.width {
        match o.layout {
            Layout::Sources => {
                let mode = Arc::new(AtomicU32::new(SILENCE));
                let src = g.add_device(Box::new(Src {
                    mode: Arc::clone(&mode),
                    cost: Settings::new(o.cost),
                    state: 0x9E37_79B9 ^ k as u32,
                }));
                srcs.push(mode);
                firsts.push((src, SocketId(0), Some(SocketId(1))));
            }
            Layout::Chain => {
                let a = probe(&mut g, k, &mut probes);
                firsts.push((a, AUDIO_OUT, None));
            }
        }
    }
    for (k, (first, audio, cv)) in firsts.into_iter().enumerate() {
        let p = probe(&mut g, k + o.width, &mut probes);
        g.connect(ep(first, audio), ep(p, AUDIO_IN)).unwrap();
        if let Some(cv) = cv {
            g.connect(ep(first, cv), ep(p, CV_IN)).unwrap();
        }
        let out = iface(&g, &format!("out_{}", k + 1));
        g.connect(ep(p, AUDIO_OUT), out).unwrap();
    }
    if o.layout == Layout::Sources {
        // The probes of a source rig were pushed in order, one per source.
        debug_assert_eq!(probes.len(), o.width);
    }
    let (mut engine, mut handle) = new_engine_with_options(
        c,
        EngineOptions {
            workers: WorkerConfig {
                threads: o.threads,
                on_thread_start: Some(start_worker),
            },
            realtime: o.rule == Rule::Live,
            ..EngineOptions::default()
        },
    );
    engine.set_level_cost_clock(fake_clock);
    if o.rule == Rule::Offline {
        engine.set_gate_mode(GateMode::ForceDispatch);
    }
    engine.set_idle_skip(o.skip);
    handle.submit_plan(g.compile().unwrap()).unwrap();
    if o.song {
        handle
            .submit_song(song::song(Vec::new(), &[(0, 120.0)]))
            .unwrap();
    }
    World {
        engine,
        handle,
        srcs,
        probes,
        batches: 0,
        outputs,
    }
}

impl World {
    /// Renders one batch (guarded against allocation) and returns its output channels.
    pub fn batch(&mut self) -> Vec<Vec<f32>> {
        self.batches += 1;
        render_block(&mut self.engine, &[], self.outputs, BATCH)
    }

    /// Renders one batch without the allocation guard (a device that panics allocates).
    pub fn raw_batch(&mut self) {
        self.batches += 1;
        let mut outs = vec![vec![0.0f32; BATCH]; self.outputs];
        let mut refs: Vec<&mut [f32]> = outs.iter_mut().map(Vec::as_mut_slice).collect();
        self.engine.render(&[], &mut refs);
    }

    pub fn run(&mut self, n: usize) {
        for _ in 0..n {
            self.batch();
        }
    }

    pub fn skipped(&self, probe: usize) -> u64 {
        self.engine.idle_skipped(self.probes[probe].id).unwrap()
    }

    pub fn processes(&self, probe: usize) -> u64 {
        self.probes[probe].counts.process.load(Relaxed)
    }

    pub fn advances(&self, probe: usize) -> u64 {
        self.probes[probe].counts.advance.load(Relaxed)
    }

    pub fn on_worker(&self, probe: usize) -> u64 {
        self.probes[probe].counts.on_worker.load(Relaxed)
    }

    pub fn set_source(&self, k: usize, mode: u32) {
        self.srcs[k].store(mode, Relaxed);
    }

    /// Sends a note to `probe`, landing at the start of the next batch.
    pub fn note(&mut self, probe: usize) {
        self.handle
            .send_note(ScheduledNote {
                frame: self.batches * BATCH as u64,
                device: self.probes[probe].id,
                socket: NOTES_IN,
                kind: NoteKind::On {
                    key: 60,
                    velocity: 1.0,
                },
            })
            .unwrap();
    }

    pub fn param(&mut self, probe: usize, plain: f32) {
        self.handle
            .set_param(self.probes[probe].id, ParamId(0), plain)
            .unwrap();
    }

    pub fn tempo(&mut self, bpm: f64) {
        self.handle.set_tempo(Tempo::new(bpm).unwrap()).unwrap();
    }

    pub fn transport(&mut self, command: TransportCommand) {
        self.handle.transport(command).unwrap();
    }
}

/// A plan of one two-stage device fed by a silent source on both of its inputs.
pub fn staged_world(skip: bool) -> (World, Arc<Counts>) {
    let c = cfg(0, 1);
    let mut g = Graph::new(&c);
    let mode = Arc::new(AtomicU32::new(SILENCE));
    let src = g.add_device(Box::new(Src {
        mode: Arc::clone(&mode),
        cost: Settings::new(0),
        state: 1,
    }));
    let counts = Arc::new(Counts::default());
    let dev = g.add_device(Box::new(Staged {
        counts: Arc::clone(&counts),
    }));
    g.connect(ep(src, SocketId(0)), ep(dev, SocketId(0)))
        .unwrap();
    g.connect(ep(src, SocketId(0)), ep(dev, SocketId(2)))
        .unwrap();
    let out = iface(&g, "out_1");
    g.connect(ep(dev, SocketId(3)), out).unwrap();
    let (mut engine, mut handle) = new_engine_with_options(c, EngineOptions::default());
    engine.set_idle_skip(skip);
    handle.submit_plan(g.compile().unwrap()).unwrap();
    (
        World {
            engine,
            handle,
            srcs: vec![mode],
            probes: Vec::new(),
            batches: 0,
            outputs: 1,
        },
        counts,
    )
}

/// The bit patterns of every output sample of a batch, appended to `into`.
pub fn push_bits(into: &mut Vec<u32>, batch: &[Vec<f32>]) {
    for channel in batch {
        into.extend(channel.iter().map(|s| s.to_bits()));
    }
}

/// A probe on its own, for tests that build their routing by hand.
pub fn new_probe(counts: &Arc<Counts>, set: &Arc<Settings>) -> Box<dyn Device> {
    Box::new(Probe {
        counts: Arc::clone(counts),
        set: Arc::clone(set),
        phase: 0.0,
        inc: 0.002,
        gain: 1.0,
        right: true,
        meters: false,
    })
}

/// A silent source on its own.
pub fn new_source(mode: &Arc<AtomicU32>) -> Box<dyn Device> {
    Box::new(Src {
        mode: Arc::clone(mode),
        cost: Settings::new(0),
        state: 1,
    })
}

/// A fresh settings block for [`new_probe`] with no cost.
pub fn settings() -> Arc<Settings> {
    Settings::new(0)
}
