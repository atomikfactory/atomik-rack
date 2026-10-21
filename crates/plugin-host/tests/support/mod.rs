//! Shared helpers for the plugin host tests: processors over the test-plugin DSP, a probe
//! processor that records what the host hands a plugin, and builders for descriptors and
//! devices.
#![allow(dead_code)] // Justification: each test binary uses a different subset of the helpers.

use atomik_core::BATCH;
use atomik_device_sdk::{
    Channels, DeviceDescriptor, PluginFormat, PluginKindHint, PluginLayout, PluginParamInfo,
    PluginRef, descriptor_for,
};
use atomik_plugin_host::{
    DeviceEnd, ManagerEnd, PluginDevice, PluginEvent, PluginEventKind, PluginIo, PluginProcessor,
    PluginTransport, ProcessStatus, ReaperQueue, SleepRule, TailLength, plugin_channel,
};
use atomik_test_plugin_core as core;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

pub const SR: f64 = 48_000.0;

// ----- layouts and descriptors --------------------------------------------------------------

pub fn param(
    id: u32,
    name: &str,
    min: f64,
    max: f64,
    default: f64,
    stepped: bool,
) -> PluginParamInfo {
    PluginParamInfo {
        id,
        name: name.to_string(),
        module: String::new(),
        min,
        max,
        default,
        stepped,
        steps: 0,
    }
}

/// The layout of the test effect: stereo in and out, a sidechain, params gain, mute, latency
/// and duck.
pub fn fx_layout() -> PluginLayout {
    PluginLayout {
        name: "Test Gain".to_string(),
        vendor: "Atomik".to_string(),
        version: "1".to_string(),
        kind_hint: PluginKindHint::Effect,
        main_in: Some(Channels::Stereo),
        main_out: Some(Channels::Stereo),
        sidechain: true,
        notes_in: false,
        notes_out: false,
        params: vec![
            param(core::GainFx::GAIN, "Gain", -60.0, 12.0, 0.0, false),
            param(core::GainFx::MUTE, "Mute", 0.0, 1.0, 0.0, true),
            param(core::GainFx::LATENCY, "Latency", 0.0, 512.0, 0.0, true),
            param(core::GainFx::DUCK, "Duck", 0.0, 1.0, 1.0, false),
        ],
    }
}

pub fn synth_layout() -> PluginLayout {
    PluginLayout {
        name: "Test Synth".to_string(),
        vendor: "Atomik".to_string(),
        version: "1".to_string(),
        kind_hint: PluginKindHint::Instrument,
        main_in: None,
        main_out: Some(Channels::Stereo),
        sidechain: false,
        notes_in: true,
        notes_out: false,
        params: vec![
            param(core::TestSynth::WAVE, "Wave", 0.0, 1.0, 0.0, true),
            param(core::TestSynth::LEVEL, "Level", 0.0, 1.0, 0.5, false),
        ],
    }
}

pub fn shift_layout() -> PluginLayout {
    PluginLayout {
        name: "Test Shift".to_string(),
        vendor: "Atomik".to_string(),
        version: "1".to_string(),
        kind_hint: PluginKindHint::NoteEffect,
        main_in: None,
        main_out: None,
        sidechain: false,
        notes_in: true,
        notes_out: true,
        params: vec![param(
            core::NoteShift::SEMITONES,
            "Semitones",
            -24.0,
            24.0,
            0.0,
            true,
        )],
    }
}

pub fn descriptor(id: &str, layout: &PluginLayout) -> &'static DeviceDescriptor {
    descriptor_for(
        &PluginRef {
            format: PluginFormat::Clap,
            id: id.to_string(),
        },
        layout,
    )
    .unwrap()
}

// ----- a device over a processor ------------------------------------------------------------

pub struct Rig {
    pub device: PluginDevice,
    pub manager: ManagerEnd,
    pub reaper: ReaperQueue,
    pub descriptor: &'static DeviceDescriptor,
}

pub fn rig(id: &str, layout: &PluginLayout, processor: Box<dyn PluginProcessor>) -> Rig {
    let descriptor = descriptor(id, layout);
    let (device_end, manager): (DeviceEnd, ManagerEnd) =
        plugin_channel(atomik_plugin_host::InstanceId(1));
    let reaper = ReaperQueue::new();
    let device = PluginDevice::new(descriptor, layout, processor, device_end, SR)
        .with_reaper(reaper.clone());
    Rig {
        device,
        manager,
        reaper,
        descriptor,
    }
}

/// What stays with the test when the device moves into a harness or an engine.
pub struct Rest {
    pub manager: ManagerEnd,
    pub reaper: ReaperQueue,
    pub descriptor: &'static DeviceDescriptor,
}

pub fn harness(rig: Rig) -> (atomik_testkit::harness::DeviceHarness, Rest) {
    let Rig {
        device,
        manager,
        reaper,
        descriptor,
    } = rig;
    (
        atomik_testkit::harness::DeviceHarness::new(Box::new(device), SR),
        Rest {
            manager,
            reaper,
            descriptor,
        },
    )
}

// ----- processors over the test DSP ---------------------------------------------------------

fn to_core(e: &PluginEvent) -> core::TimedEvent {
    use core::Event;
    let event = match e.kind {
        PluginEventKind::NoteOn {
            key,
            velocity,
            note_id,
        } => Event::NoteOn {
            key,
            velocity,
            id: note_id,
        },
        PluginEventKind::NoteOff { key, note_id } => Event::NoteOff { key, id: note_id },
        PluginEventKind::PitchBend { value } => Event::PitchBend(value),
        PluginEventKind::Cc { controller, value } => Event::Cc { controller, value },
        PluginEventKind::ChannelPressure { value } => Event::ChannelPressure(value),
        PluginEventKind::PolyPressure {
            key,
            note_id,
            value,
        } => Event::PolyPressure {
            key,
            id: note_id,
            value,
        },
        PluginEventKind::NoteExpr {
            key,
            note_id,
            kind,
            value,
        } => Event::Expr {
            key,
            id: note_id,
            kind: match kind {
                atomik_core::ExprKind::Tuning => core::ExprKind::Tuning,
                atomik_core::ExprKind::Pressure => core::ExprKind::Pressure,
                atomik_core::ExprKind::Slide => core::ExprKind::Slide,
                atomik_core::ExprKind::Volume => core::ExprKind::Volume,
                atomik_core::ExprKind::Pan => core::ExprKind::Pan,
            },
            value,
        },
        PluginEventKind::Param { id, value } => Event::Param { id, value },
    };
    core::TimedEvent {
        offset: e.offset,
        event,
    }
}

/// The test effect as a processor (reference DSP behind the host's `PluginIo`).
pub struct CoreEffect {
    pub fx: core::GainFx,
}

impl CoreEffect {
    pub fn new() -> Self {
        Self {
            fx: core::GainFx::new(SR),
        }
    }
}

impl PluginProcessor for CoreEffect {
    fn start_processing(&mut self) -> bool {
        true
    }
    fn stop_processing(&mut self) {}
    fn reset(&mut self) {
        self.fx.reset();
    }
    fn process(&mut self, io: &mut PluginIo<'_>) -> ProcessStatus {
        for e in io.events_in {
            if let PluginEventKind::Param { id, value } = e.kind {
                self.fx.set_param(id, value);
            }
        }
        let input = [io.main_in[0], io.main_in[1]];
        let sc = (!io.sidechain_silent && io.sidechain.len() == 2)
            .then(|| [io.sidechain[0], io.sidechain[1]]);
        let mut out = [[0.0; BATCH]; 2];
        self.fx.process(&input, sc.as_ref(), &mut out);
        io.main_out[0] = out[0];
        io.main_out[1] = out[1];
        ProcessStatus::Continue
    }
    fn sleep_rule(&self) -> SleepRule {
        SleepRule::PluginDecides
    }
    fn tail_samples(&self) -> TailLength {
        TailLength::Finite(self.fx.tail_frames())
    }
}

/// The test synth as a processor.
pub struct CoreSynth {
    pub synth: core::TestSynth,
    pub buf: Vec<core::TimedEvent>,
}

impl CoreSynth {
    pub fn new() -> Self {
        Self {
            synth: core::TestSynth::new(SR),
            buf: Vec::with_capacity(8_192),
        }
    }
}

impl PluginProcessor for CoreSynth {
    fn start_processing(&mut self) -> bool {
        true
    }
    fn stop_processing(&mut self) {}
    fn reset(&mut self) {
        self.synth.reset();
    }
    fn process(&mut self, io: &mut PluginIo<'_>) -> ProcessStatus {
        self.buf.clear();
        for e in io.events_in {
            if let PluginEventKind::Param { id, value } = e.kind {
                self.synth.set_param(id, value);
            } else if self.buf.len() < self.buf.capacity() {
                self.buf.push(to_core(e));
            }
        }
        let mut out = [[0.0; BATCH]; 2];
        self.synth.process(&self.buf, &mut out);
        io.main_out[0] = out[0];
        io.main_out[1] = out[1];
        if self.synth.is_idle() {
            ProcessStatus::Sleep
        } else {
            ProcessStatus::Continue
        }
    }
}

/// The test note effect as a processor.
pub struct CoreShift {
    pub shift: core::NoteShift,
    pub inbuf: Vec<core::TimedEvent>,
    pub outbuf: Vec<core::TimedEvent>,
}

impl CoreShift {
    pub fn new() -> Self {
        Self {
            shift: core::NoteShift::new(),
            inbuf: Vec::with_capacity(8_192),
            outbuf: Vec::with_capacity(8_192),
        }
    }
}

impl PluginProcessor for CoreShift {
    fn start_processing(&mut self) -> bool {
        true
    }
    fn stop_processing(&mut self) {}
    fn reset(&mut self) {
        self.shift.reset();
    }
    fn process(&mut self, io: &mut PluginIo<'_>) -> ProcessStatus {
        self.inbuf.clear();
        self.outbuf.clear();
        for e in io.events_in {
            if let PluginEventKind::Param { id, value } = e.kind {
                self.shift.set_param(id, value);
            } else if self.inbuf.len() < self.inbuf.capacity() {
                self.inbuf.push(to_core(e));
            }
        }
        self.shift.process(&self.inbuf, &mut self.outbuf);
        for e in &self.outbuf {
            let kind = match e.event {
                core::Event::NoteOn { key, velocity, id } => PluginEventKind::NoteOn {
                    key,
                    velocity,
                    note_id: id,
                },
                core::Event::NoteOff { key, id } => PluginEventKind::NoteOff { key, note_id: id },
                _ => continue,
            };
            io.events_out.push(e.offset, kind);
        }
        ProcessStatus::Continue
    }
}

// ----- the probe ----------------------------------------------------------------------------

/// What a [`Probe`] records. Every vector is preallocated so recording inside `process` never
/// allocates (the tests run under the allocation guard).
pub struct ProbeLog {
    pub calls: usize,
    pub starts: usize,
    pub stops: usize,
    pub resets: usize,
    pub events: Vec<PluginEvent>,
    /// `(start, len)` into `events`, one per call.
    pub ranges: Vec<(usize, usize)>,
    pub transports: Vec<PluginTransport>,
    /// `(main_in_silent, sidechain_silent)` per call.
    pub silent: Vec<(bool, bool)>,
    /// Channel counts `(main_in, sidechain, main_out)` per call.
    pub shapes: Vec<(usize, usize, usize)>,
    /// A copy of `main_in[0]` and `main_in[1]` of the last call.
    pub last_in: [[f32; BATCH]; 2],
    pub last_sc: [[f32; BATCH]; 2],
    pub audio_thread_flags: Vec<bool>,
    pub main_thread_flags: Vec<bool>,
    pub flush_flags: Vec<bool>,
    pub thread_ids: Vec<std::thread::ThreadId>,
    pub frames: Vec<usize>,
}

impl ProbeLog {
    fn new() -> Self {
        Self {
            calls: 0,
            starts: 0,
            stops: 0,
            resets: 0,
            events: Vec::with_capacity(200_000),
            ranges: Vec::with_capacity(20_000),
            transports: Vec::with_capacity(20_000),
            silent: Vec::with_capacity(20_000),
            shapes: Vec::with_capacity(20_000),
            last_in: [[0.0; BATCH]; 2],
            last_sc: [[0.0; BATCH]; 2],
            audio_thread_flags: Vec::with_capacity(20_000),
            main_thread_flags: Vec::with_capacity(20_000),
            flush_flags: Vec::with_capacity(20_000),
            thread_ids: Vec::with_capacity(20_000),
            frames: Vec::with_capacity(20_000),
        }
    }

    pub fn events_of_call(&self, call: usize) -> &[PluginEvent] {
        let (s, l) = self.ranges[call];
        &self.events[s..s + l]
    }
}

/// How the probe fills its output.
#[derive(Clone, Copy)]
pub enum ProbeOut {
    Silent,
    /// `out = in * gain` per channel (mono in is copied to every out channel).
    Gain(f32),
    Constant(f32),
    Nan,
}

pub struct ProbeShared {
    pub log: Mutex<ProbeLog>,
    status: AtomicU8,
    out_mode: AtomicU8,
    out_value: Mutex<f32>,
    /// Panic in the call with this index (`usize::MAX` = never).
    panic_at: AtomicUsize,
    rule: AtomicU8,
    tail: AtomicU8,
    /// Events the probe emits on `events_out` in every call.
    pub emit: Mutex<Vec<PluginEvent>>,
}

pub struct Probe {
    shared: Arc<ProbeShared>,
}

impl Probe {
    pub fn new() -> (Self, Arc<ProbeShared>) {
        let shared = Arc::new(ProbeShared {
            log: Mutex::new(ProbeLog::new()),
            status: AtomicU8::new(0),
            out_mode: AtomicU8::new(1),
            out_value: Mutex::new(1.0),
            panic_at: AtomicUsize::new(usize::MAX),
            rule: AtomicU8::new(0),
            tail: AtomicU8::new(0),
            emit: Mutex::new(Vec::with_capacity(64)),
        });
        // Apple's `std` mutex boxes its pthread mutex on the first lock; take each lock once here so
        // that allocation is not charged to the no-alloc render section.
        drop(shared.log.lock());
        drop(shared.out_value.lock());
        drop(shared.emit.lock());
        (
            Self {
                shared: Arc::clone(&shared),
            },
            shared,
        )
    }
}

impl ProbeShared {
    pub fn set_status(&self, s: ProcessStatus) {
        self.status.store(
            match s {
                ProcessStatus::Continue => 0,
                ProcessStatus::ContinueIfNotQuiet => 1,
                ProcessStatus::Tail => 2,
                ProcessStatus::Sleep => 3,
                ProcessStatus::Error => 4,
            },
            Ordering::Relaxed,
        );
    }
    pub fn set_out(&self, o: ProbeOut) {
        let (mode, v) = match o {
            ProbeOut::Silent => (0, 0.0),
            ProbeOut::Gain(g) => (1, g),
            ProbeOut::Constant(c) => (2, c),
            ProbeOut::Nan => (3, 0.0),
        };
        *self.out_value.lock().unwrap() = v;
        self.out_mode.store(mode, Ordering::Relaxed);
    }
    pub fn panic_at(&self, call: usize) {
        self.panic_at.store(call, Ordering::Relaxed);
    }
    pub fn set_rule(&self, rule: SleepRule, tail: TailLength) {
        self.rule.store(
            match rule {
                SleepRule::PluginDecides => 0,
                SleepRule::ZeroTail => 1,
            },
            Ordering::Relaxed,
        );
        self.tail.store(
            match tail {
                TailLength::Finite(0) => 0,
                TailLength::Finite(_) => 1,
                TailLength::Infinite => 2,
            },
            Ordering::Relaxed,
        );
    }
    pub fn calls(&self) -> usize {
        self.log.lock().unwrap().calls
    }
}

impl PluginProcessor for Probe {
    fn start_processing(&mut self) -> bool {
        self.shared.log.lock().unwrap().starts += 1;
        true
    }
    fn stop_processing(&mut self) {
        self.shared.log.lock().unwrap().stops += 1;
    }
    fn reset(&mut self) {
        self.shared.log.lock().unwrap().resets += 1;
    }
    fn process(&mut self, io: &mut PluginIo<'_>) -> ProcessStatus {
        let call = {
            let mut log = self.shared.log.lock().unwrap();
            let call = log.calls;
            log.calls += 1;
            let start = log.events.len();
            for e in io.events_in {
                if log.events.len() < log.events.capacity() {
                    log.events.push(*e);
                }
            }
            let len = log.events.len() - start;
            log.ranges.push((start, len));
            log.transports.push(io.transport);
            log.silent.push((io.main_in_silent, io.sidechain_silent));
            log.shapes
                .push((io.main_in.len(), io.sidechain.len(), io.main_out.len()));
            log.frames.push(io.frames);
            for (i, b) in io.main_in.iter().take(2).enumerate() {
                log.last_in[i] = *b;
            }
            for (i, b) in io.sidechain.iter().take(2).enumerate() {
                log.last_sc[i] = *b;
            }
            log.audio_thread_flags
                .push(atomik_plugin_host::is_audio_thread());
            log.main_thread_flags
                .push(atomik_plugin_host::is_main_thread());
            log.flush_flags
                .push(atomik_plugin_host::denormals_flushed());
            log.thread_ids.push(std::thread::current().id());
            call
        };
        if call == self.shared.panic_at.load(Ordering::Relaxed) {
            panic!("probe panics on purpose");
        }
        let mode = self.shared.out_mode.load(Ordering::Relaxed);
        let v = *self.shared.out_value.lock().unwrap();
        let channels = io.main_out.len();
        for ch in 0..channels {
            let src = io.main_in.get(ch).or_else(|| io.main_in.first());
            for i in 0..BATCH {
                io.main_out[ch][i] = match mode {
                    0 => 0.0,
                    1 => src.map_or(0.0, |s| s[i]) * v,
                    2 => v,
                    _ => f32::NAN,
                };
            }
        }
        for e in self.shared.emit.lock().unwrap().iter() {
            io.events_out.push(e.offset, e.kind);
        }
        match self.shared.status.load(Ordering::Relaxed) {
            1 => ProcessStatus::ContinueIfNotQuiet,
            2 => ProcessStatus::Tail,
            3 => ProcessStatus::Sleep,
            4 => ProcessStatus::Error,
            _ => ProcessStatus::Continue,
        }
    }
    fn sleep_rule(&self) -> SleepRule {
        match self.shared.rule.load(Ordering::Relaxed) {
            1 => SleepRule::ZeroTail,
            _ => SleepRule::PluginDecides,
        }
    }
    fn tail_samples(&self) -> TailLength {
        match self.shared.tail.load(Ordering::Relaxed) {
            0 => TailLength::Finite(0),
            1 => TailLength::Finite(1_000),
            _ => TailLength::Infinite,
        }
    }
}
