//! Fake devices for engine and integration tests (fakes, not mocks).
//!
//! Every fake has a valid static descriptor with a `"testkit.<name>"` type id. Fakes that
//! record observations share them through `Arc`-held atomics, so tests can read them while the
//! device itself is owned by a render plan. Nothing a fake does in `process` allocates, locks or
//! blocks, except [`Allocator`], which exists to prove the allocation guard bites.

use atomik_core::{ExprKind, NoteKind, ParamDescriptor, ParamId, Taper, Unit};
use atomik_device_sdk::{
    Device, DeviceDescriptor, DeviceKind, Direction, PrepareInfo, ProcessContext, SocketDescriptor,
    SocketId, SocketKind,
};
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::ThreadId;

const fn socket(
    id: u16,
    key: &'static str,
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

const fn linear_param(
    id: u32,
    key: &'static str,
    min: f32,
    max: f32,
    default: f32,
) -> ParamDescriptor {
    ParamDescriptor {
        id: ParamId(id),
        key,
        name: key,
        min,
        max,
        default,
        taper: Taper::Linear,
        unit: Unit::None,
        discrete: false,
    }
}

// ---------------------------------------------------------------------------------------------
// ConstantSource
// ---------------------------------------------------------------------------------------------

/// Descriptor of [`ConstantSource`]: `audio_out` (Audio out), `cv_out` (Cv out), param `value`.
pub static CONSTANT_SOURCE: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "testkit.constant-source",
    name: "Constant Source",
    kind: DeviceKind::Utility,
    version: 1,
    params: &[linear_param(0, "value", -10.0, 10.0, 0.0)],
    sockets: &[
        socket(0, "audio_out", SocketKind::Audio, Direction::Output),
        socket(1, "cv_out", SocketKind::Cv, Direction::Output),
    ],
};

/// Writes `value` to every sample of `audio_out` and `cv_out`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConstantSource {
    /// The constant written each batch (param `value`, range −10..10).
    pub value: f32,
}

impl ConstantSource {
    /// Audio output socket.
    pub const AUDIO_OUT: SocketId = SocketId(0);
    /// Cv output socket.
    pub const CV_OUT: SocketId = SocketId(1);
    /// The `value` param.
    pub const VALUE: ParamId = ParamId(0);

    /// Boxed instance writing `value`.
    pub fn boxed(value: f32) -> Box<dyn Device> {
        Box::new(Self { value })
    }
}

impl Device for ConstantSource {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &CONSTANT_SOURCE
    }
    fn prepare(&mut self, _: &PrepareInfo) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, id: ParamId, plain: f32) {
        if id == Self::VALUE {
            self.value = plain;
        }
    }
    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        ctx.output(Self::AUDIO_OUT).fill(self.value);
        ctx.output(Self::CV_OUT).fill(self.value);
    }
}

// ---------------------------------------------------------------------------------------------
// Panicker
// ---------------------------------------------------------------------------------------------

/// Descriptor of [`Panicker`]: `audio_out` (Audio out).
pub static PANICKER: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "testkit.panicker",
    name: "Panicker",
    kind: DeviceKind::Utility,
    version: 1,
    params: &[],
    sockets: &[socket(0, "audio_out", SocketKind::Audio, Direction::Output)],
};

/// A device whose `process` always panics: for tests of fault containment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Panicker;

impl Panicker {
    /// Audio output socket.
    pub const AUDIO_OUT: SocketId = SocketId(0);

    /// Boxed instance.
    pub fn boxed() -> Box<dyn Device> {
        Box::new(Self)
    }
}

impl Device for Panicker {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &PANICKER
    }
    fn prepare(&mut self, _: &PrepareInfo) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _: ParamId, _: f32) {}
    fn process(&mut self, _: &mut ProcessContext<'_>) {
        panic!("device failure");
    }
}

// ---------------------------------------------------------------------------------------------
// Locker
// ---------------------------------------------------------------------------------------------

/// Descriptor of [`Locker`]: `audio_out` (Audio out).
pub static LOCKER: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "testkit.locker",
    name: "Locker",
    kind: DeviceKind::Utility,
    version: 1,
    params: &[],
    sockets: &[socket(0, "audio_out", SocketKind::Audio, Direction::Output)],
};

/// What a [`Locker`] breaks the audio-thread rules with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Misbehaviour {
    /// Takes a lock in `process`.
    Lock,
    /// Makes a blocking host call in `process`.
    Block,
    /// Nothing: the control.
    None,
}

/// A device that breaks an audio-thread rule in `process`: for tests of the rule guard.
#[derive(Debug)]
pub struct Locker {
    mode: Misbehaviour,
    lock: atomik_core::rt_guard::RtMutex<u32>,
}

impl Locker {
    /// Audio output socket.
    pub const AUDIO_OUT: SocketId = SocketId(0);

    /// Boxed instance.
    pub fn boxed(mode: Misbehaviour) -> Box<dyn Device> {
        Box::new(Self {
            mode,
            lock: atomik_core::rt_guard::RtMutex::new(0),
        })
    }
}

impl Device for Locker {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &LOCKER
    }
    fn prepare(&mut self, _: &PrepareInfo) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _: ParamId, _: f32) {}
    fn process(&mut self, _: &mut ProcessContext<'_>) {
        match self.mode {
            Misbehaviour::Lock => {
                if let Ok(mut n) = self.lock.lock() {
                    *n += 1;
                }
            }
            Misbehaviour::Block => atomik_core::rt_guard::forbid("a blocking host call"),
            Misbehaviour::None => {}
        }
    }
}

// ---------------------------------------------------------------------------------------------
// ImpulseOnNote
// ---------------------------------------------------------------------------------------------

/// Descriptor of [`ImpulseOnNote`]: `notes_in` (Notes in), `audio_out` (Audio out).
pub static IMPULSE_ON_NOTE: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "testkit.impulse-on-note",
    name: "Impulse On Note",
    kind: DeviceKind::Instrument,
    version: 1,
    params: &[],
    sockets: &[
        socket(0, "notes_in", SocketKind::Notes, Direction::Input),
        socket(1, "audio_out", SocketKind::Audio, Direction::Output),
    ],
};

/// Writes 1.0 to `audio_out` at the offset of every note-on it receives; 0 elsewhere.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ImpulseOnNote;

impl ImpulseOnNote {
    /// Notes input socket.
    pub const NOTES_IN: SocketId = SocketId(0);
    /// Audio output socket.
    pub const AUDIO_OUT: SocketId = SocketId(1);

    /// Boxed instance.
    pub fn boxed() -> Box<dyn Device> {
        Box::new(Self)
    }
}

impl Device for ImpulseOnNote {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &IMPULSE_ON_NOTE
    }
    fn prepare(&mut self, _: &PrepareInfo) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _: ParamId, _: f32) {}
    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        for i in 0..ctx.notes_in(Self::NOTES_IN).len() {
            let event = ctx.notes_in(Self::NOTES_IN)[i];
            if let NoteKind::On { .. } = event.kind {
                ctx.output(Self::AUDIO_OUT)[usize::from(event.offset)] = 1.0;
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Passthrough
// ---------------------------------------------------------------------------------------------

/// Descriptor of [`Passthrough`]: `audio_in` (Audio in), `audio_out` (Audio out), param `gain`.
pub static PASSTHROUGH: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "testkit.passthrough",
    name: "Passthrough",
    kind: DeviceKind::Effect,
    version: 1,
    params: &[linear_param(0, "gain", 0.0, 4.0, 1.0)],
    sockets: &[
        socket(0, "audio_in", SocketKind::Audio, Direction::Input),
        socket(1, "audio_out", SocketKind::Audio, Direction::Output),
    ],
};

/// `audio_out = audio_in * gain`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Passthrough {
    /// Linear gain (param `gain`, range 0..4).
    pub gain: f32,
}

impl Passthrough {
    /// Audio input socket.
    pub const AUDIO_IN: SocketId = SocketId(0);
    /// Audio output socket.
    pub const AUDIO_OUT: SocketId = SocketId(1);
    /// The `gain` param.
    pub const GAIN: ParamId = ParamId(0);

    /// Boxed instance with `gain`.
    pub fn boxed(gain: f32) -> Box<dyn Device> {
        Box::new(Self { gain })
    }
}

impl Device for Passthrough {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &PASSTHROUGH
    }
    fn prepare(&mut self, _: &PrepareInfo) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, id: ParamId, plain: f32) {
        if id == Self::GAIN {
            self.gain = plain;
        }
    }
    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let input = *ctx.input(Self::AUDIO_IN);
        for (o, i) in ctx.output(Self::AUDIO_OUT).iter_mut().zip(input) {
            *o = i * self.gain;
        }
    }
}

// ---------------------------------------------------------------------------------------------
// CallLog
// ---------------------------------------------------------------------------------------------

/// Descriptor of [`CallLog`]: `audio_in`, `audio_out` (copied through), `notes_in`, param `p`.
pub static CALL_LOG: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "testkit.call-log",
    name: "Call Log",
    kind: DeviceKind::Utility,
    version: 1,
    params: &[linear_param(0, "p", 0.0, 1.0, 0.0)],
    sockets: &[
        socket(0, "audio_in", SocketKind::Audio, Direction::Input),
        socket(1, "audio_out", SocketKind::Audio, Direction::Output),
        socket(2, "notes_in", SocketKind::Notes, Direction::Input),
    ],
};

/// Shared counters written by a [`CallLog`] device.
#[derive(Debug, Default)]
pub struct CallCounts {
    prepare: AtomicU64,
    reset: AtomicU64,
    set_param: AtomicU64,
    process: AtomicU64,
    last_param_bits: AtomicU32,
    last_seed: AtomicU64,
    last_frame: AtomicU64,
    notes_seen: AtomicU64,
}

impl CallCounts {
    /// Number of `prepare` calls.
    pub fn prepare(&self) -> u64 {
        self.prepare.load(Ordering::Relaxed)
    }
    /// Number of `reset` calls.
    pub fn reset(&self) -> u64 {
        self.reset.load(Ordering::Relaxed)
    }
    /// Number of `set_param` calls.
    pub fn set_param(&self) -> u64 {
        self.set_param.load(Ordering::Relaxed)
    }
    /// Number of `process` calls.
    pub fn process(&self) -> u64 {
        self.process.load(Ordering::Relaxed)
    }
    /// The last plain value passed to `set_param` (0.0 before any call).
    pub fn last_param(&self) -> f32 {
        f32::from_bits(self.last_param_bits.load(Ordering::Relaxed))
    }
    /// The seed passed to the last `prepare`.
    pub fn last_seed(&self) -> u64 {
        self.last_seed.load(Ordering::Relaxed)
    }
    /// `TimeInfo::frame` of the last `process` call.
    pub fn last_frame(&self) -> u64 {
        self.last_frame.load(Ordering::Relaxed)
    }
    /// Total number of note events received on `notes_in`.
    pub fn notes_seen(&self) -> u64 {
        self.notes_seen.load(Ordering::Relaxed)
    }
}

/// Counts calls to every `Device` method; copies `audio_in` to `audio_out`.
#[derive(Debug, Default)]
pub struct CallLog {
    counts: Arc<CallCounts>,
}

impl CallLog {
    /// Audio input socket.
    pub const AUDIO_IN: SocketId = SocketId(0);
    /// Audio output socket.
    pub const AUDIO_OUT: SocketId = SocketId(1);
    /// Notes input socket.
    pub const NOTES_IN: SocketId = SocketId(2);
    /// The `p` param.
    pub const P: ParamId = ParamId(0);

    /// A device and the counters it writes.
    pub fn new() -> (Self, Arc<CallCounts>) {
        let counts = Arc::new(CallCounts::default());
        (
            Self {
                counts: Arc::clone(&counts),
            },
            counts,
        )
    }

    /// Boxed device and its counters.
    pub fn boxed() -> (Box<dyn Device>, Arc<CallCounts>) {
        let (d, c) = Self::new();
        (Box::new(d), c)
    }
}

impl Device for CallLog {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &CALL_LOG
    }
    fn prepare(&mut self, info: &PrepareInfo) {
        self.counts.prepare.fetch_add(1, Ordering::Relaxed);
        self.counts.last_seed.store(info.seed, Ordering::Relaxed);
    }
    fn reset(&mut self) {
        self.counts.reset.fetch_add(1, Ordering::Relaxed);
    }
    fn set_param(&mut self, _: ParamId, plain: f32) {
        self.counts.set_param.fetch_add(1, Ordering::Relaxed);
        self.counts
            .last_param_bits
            .store(plain.to_bits(), Ordering::Relaxed);
    }
    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        self.counts.process.fetch_add(1, Ordering::Relaxed);
        self.counts
            .last_frame
            .store(ctx.time.frame, Ordering::Relaxed);
        let notes = ctx.notes_in(Self::NOTES_IN).len() as u64;
        self.counts.notes_seen.fetch_add(notes, Ordering::Relaxed);
        *ctx.output(Self::AUDIO_OUT) = *ctx.input(Self::AUDIO_IN);
    }
}

// ---------------------------------------------------------------------------------------------
// DropProbe
// ---------------------------------------------------------------------------------------------

/// Descriptor of [`DropProbe`]: `audio_out` (Audio out, silent).
pub static DROP_PROBE: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "testkit.drop-probe",
    name: "Drop Probe",
    kind: DeviceKind::Utility,
    version: 1,
    params: &[],
    sockets: &[socket(0, "audio_out", SocketKind::Audio, Direction::Output)],
};

/// Where a [`DropProbe`] records the thread that dropped it.
#[derive(Debug, Clone, Default)]
pub struct DropRecord(Arc<Mutex<Option<ThreadId>>>);

impl DropRecord {
    /// The id of the thread that dropped the probe, if it has been dropped.
    pub fn thread(&self) -> Option<ThreadId> {
        *self.0.lock().expect("drop record poisoned")
    }
}

/// Silent device whose `Drop` stores `std::thread::current().id()` in a shared [`DropRecord`].
/// Dropping is never on the render path, so the mutex in `Drop` does not break HI-1.
#[derive(Debug)]
pub struct DropProbe {
    record: DropRecord,
}

impl DropProbe {
    /// A probe and its record.
    pub fn new() -> (Self, DropRecord) {
        let record = DropRecord::default();
        (
            Self {
                record: record.clone(),
            },
            record,
        )
    }

    /// Boxed probe and its record.
    pub fn boxed() -> (Box<dyn Device>, DropRecord) {
        let (d, r) = Self::new();
        (Box::new(d), r)
    }
}

impl Drop for DropProbe {
    fn drop(&mut self) {
        if let Ok(mut slot) = self.record.0.lock() {
            *slot = Some(std::thread::current().id());
        }
    }
}

impl Device for DropProbe {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &DROP_PROBE
    }
    fn prepare(&mut self, _: &PrepareInfo) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _: ParamId, _: f32) {}
    fn process(&mut self, _: &mut ProcessContext<'_>) {}
}

// ---------------------------------------------------------------------------------------------
// TempoProbe
// ---------------------------------------------------------------------------------------------

/// Descriptor of [`TempoProbe`]: `audio_out` (Audio out). Not listed in [`descriptors`] (its
/// array length is part of that function's signature).
pub static TEMPO_PROBE: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "testkit.tempo-probe",
    name: "Tempo Probe",
    kind: DeviceKind::Utility,
    version: 1,
    params: &[],
    sockets: &[socket(0, "audio_out", SocketKind::Audio, Direction::Output)],
};

/// Writes `TimeInfo::tempo` (in BPM, as `f32`) to every sample of `audio_out`, so tests can see
/// exactly at which batch a tempo change lands.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TempoProbe;

impl TempoProbe {
    /// Audio output socket.
    pub const AUDIO_OUT: SocketId = SocketId(0);

    /// Boxed instance.
    pub fn boxed() -> Box<dyn Device> {
        Box::new(Self)
    }
}

impl Device for TempoProbe {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &TEMPO_PROBE
    }
    fn prepare(&mut self, _: &PrepareInfo) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _: ParamId, _: f32) {}
    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let bpm = ctx.time.tempo.bpm() as f32;
        ctx.output(Self::AUDIO_OUT).fill(bpm);
    }
}

// ---------------------------------------------------------------------------------------------
// Allocator
// ---------------------------------------------------------------------------------------------

/// Descriptor of [`Allocator`]: `audio_out` (Audio out, silent).
pub static ALLOCATOR: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "testkit.allocator",
    name: "Allocator",
    kind: DeviceKind::Utility,
    version: 1,
    params: &[],
    sockets: &[socket(0, "audio_out", SocketKind::Audio, Direction::Output)],
};

/// Deliberately breaks HI-1: allocates in `process`. Only used to prove the guard bites.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Allocator;

impl Allocator {
    /// Boxed instance.
    pub fn boxed() -> Box<dyn Device> {
        Box::new(Self)
    }
}

impl Device for Allocator {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &ALLOCATOR
    }
    fn prepare(&mut self, _: &PrepareInfo) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _: ParamId, _: f32) {}
    fn process(&mut self, _: &mut ProcessContext<'_>) {
        let v: Vec<f32> = Vec::with_capacity(1);
        std::hint::black_box(&v);
    }
}

// ---------------------------------------------------------------------------------------------
// RecordingSink
// ---------------------------------------------------------------------------------------------

/// Descriptor of [`RecordingSink`]: `audio_in` (Audio in).
pub static RECORDING_SINK: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "testkit.recording-sink",
    name: "Recording Sink",
    kind: DeviceKind::Utility,
    version: 1,
    params: &[],
    sockets: &[socket(0, "audio_in", SocketKind::Audio, Direction::Input)],
};

#[derive(Debug)]
struct RecordingBuf {
    samples: Box<[AtomicU32]>,
    len: AtomicUsize,
}

/// Test-side view of what a [`RecordingSink`] captured.
#[derive(Debug, Clone)]
pub struct Recording(Arc<RecordingBuf>);

impl Recording {
    /// The captured samples, in order (at most the sink's capacity).
    pub fn samples(&self) -> Vec<f32> {
        let len = self.0.len.load(Ordering::Acquire);
        self.0.samples[..len]
            .iter()
            .map(|s| f32::from_bits(s.load(Ordering::Relaxed)))
            .collect()
    }
}

/// Copies `audio_in` into a fixed-capacity, lock-free buffer; stops when full.
///
/// The buffer is allocated in [`RecordingSink::new`] (non-RT) because the test-side
/// [`Recording`] handle must exist before the device is moved into a plan; `prepare` only
/// rewinds it. `process` never allocates.
#[derive(Debug)]
pub struct RecordingSink {
    buf: Arc<RecordingBuf>,
}

impl RecordingSink {
    /// Audio input socket.
    pub const AUDIO_IN: SocketId = SocketId(0);

    /// A sink able to capture `capacity` frames, and its recording.
    pub fn new(capacity: usize) -> (Self, Recording) {
        let buf = Arc::new(RecordingBuf {
            samples: (0..capacity).map(|_| AtomicU32::new(0)).collect(),
            len: AtomicUsize::new(0),
        });
        (
            Self {
                buf: Arc::clone(&buf),
            },
            Recording(buf),
        )
    }

    /// Boxed sink and its recording.
    pub fn boxed(capacity: usize) -> (Box<dyn Device>, Recording) {
        let (d, r) = Self::new(capacity);
        (Box::new(d), r)
    }
}

impl Device for RecordingSink {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &RECORDING_SINK
    }
    fn prepare(&mut self, _: &PrepareInfo) {
        self.buf.len.store(0, Ordering::Release);
    }
    fn reset(&mut self) {}
    fn set_param(&mut self, _: ParamId, _: f32) {}
    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let len = self.buf.len.load(Ordering::Relaxed);
        let free = self.buf.samples.len() - len;
        let input = ctx.input(Self::AUDIO_IN);
        let n = free.min(input.len());
        for (dst, src) in self.buf.samples[len..len + n].iter().zip(input) {
            dst.store(src.to_bits(), Ordering::Relaxed);
        }
        self.buf.len.store(len + n, Ordering::Release);
    }
}

// ---------------------------------------------------------------------------------------------
// NoteThru
// ---------------------------------------------------------------------------------------------

/// Descriptor of [`NoteThru`]: `notes_in` (Notes in), `notes_out` (Notes out).
pub static NOTE_THRU: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "testkit.note-thru",
    name: "Note Thru",
    kind: DeviceKind::Utility,
    version: 1,
    params: &[],
    sockets: &[
        socket(0, "notes_in", SocketKind::Notes, Direction::Input),
        socket(1, "notes_out", SocketKind::Notes, Direction::Output),
    ],
};

/// Copies every event from `notes_in` to `notes_out` (for testing Notes cables).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NoteThru;

impl NoteThru {
    /// Notes input socket.
    pub const NOTES_IN: SocketId = SocketId(0);
    /// Notes output socket.
    pub const NOTES_OUT: SocketId = SocketId(1);

    /// Boxed instance.
    pub fn boxed() -> Box<dyn Device> {
        Box::new(Self)
    }
}

impl Device for NoteThru {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &NOTE_THRU
    }
    fn prepare(&mut self, _: &PrepareInfo) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _: ParamId, _: f32) {}
    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        for i in 0..ctx.notes_in(Self::NOTES_IN).len() {
            let event = ctx.notes_in(Self::NOTES_IN)[i];
            // Same capacity as the input, so this cannot overflow; ignore if it ever does.
            let _ = ctx.notes_out(Self::NOTES_OUT).push(event);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// NoteLog
// ---------------------------------------------------------------------------------------------

/// Descriptor of [`NoteLog`]: `notes_in` (Notes in), `audio_out` (Audio out, silent).
pub static NOTE_LOG: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "testkit.note-log",
    name: "Note Log",
    kind: DeviceKind::Utility,
    version: 1,
    params: &[],
    sockets: &[
        socket(0, "notes_in", SocketKind::Notes, Direction::Input),
        socket(1, "audio_out", SocketKind::Audio, Direction::Output),
    ],
};

#[derive(Debug)]
struct NoteLogBuf {
    /// `(timeline frame, encoded kind)` pairs.
    events: Box<[(AtomicU64, AtomicU64)]>,
    len: AtomicUsize,
}

fn encode_kind(kind: NoteKind) -> u64 {
    match kind {
        NoteKind::On { key, velocity } => {
            1 << 40 | u64::from(key) << 32 | u64::from(velocity.to_bits())
        }
        NoteKind::Off { key } => 2 << 40 | u64::from(key) << 32,
        NoteKind::AllOff => 3 << 40,
        NoteKind::PitchBend { value } => 4 << 40 | u64::from(value.to_bits()),
        NoteKind::Cc { controller, value } => {
            5 << 40 | u64::from(controller) << 32 | u64::from(value.to_bits())
        }
        NoteKind::ChannelPressure { value } => 6 << 40 | u64::from(value.to_bits()),
        NoteKind::PolyPressure { key, value } => {
            7 << 40 | u64::from(key) << 32 | u64::from(value.to_bits())
        }
        NoteKind::NoteExpr { key, kind, value } => {
            let k = match kind {
                ExprKind::Tuning => 0,
                ExprKind::Pressure => 1,
                ExprKind::Slide => 2,
                ExprKind::Volume => 3,
                ExprKind::Pan => 4,
            };
            (8 + k) << 40 | u64::from(key) << 32 | u64::from(value.to_bits())
        }
    }
}

fn decode_kind(bits: u64) -> NoteKind {
    let key = (bits >> 32) as u8;
    match bits >> 40 {
        1 => NoteKind::On {
            key,
            velocity: f32::from_bits(bits as u32),
        },
        2 => NoteKind::Off { key },
        4 => NoteKind::PitchBend {
            value: f32::from_bits(bits as u32),
        },
        5 => NoteKind::Cc {
            controller: key,
            value: f32::from_bits(bits as u32),
        },
        6 => NoteKind::ChannelPressure {
            value: f32::from_bits(bits as u32),
        },
        7 => NoteKind::PolyPressure {
            key,
            value: f32::from_bits(bits as u32),
        },
        t @ 8..=12 => NoteKind::NoteExpr {
            key,
            kind: match t - 8 {
                0 => ExprKind::Tuning,
                1 => ExprKind::Pressure,
                2 => ExprKind::Slide,
                3 => ExprKind::Volume,
                _ => ExprKind::Pan,
            },
            value: f32::from_bits(bits as u32),
        },
        _ => NoteKind::AllOff,
    }
}

/// Test-side view of what a [`NoteLog`] received.
#[derive(Debug, Clone)]
pub struct NoteEvents(Arc<NoteLogBuf>);

impl NoteEvents {
    /// Every received event as `(timeline frame, kind)`, in delivery order (at most the
    /// log's capacity).
    pub fn events(&self) -> Vec<(u64, NoteKind)> {
        let len = self.0.len.load(Ordering::Acquire);
        self.0.events[..len]
            .iter()
            .map(|(f, k)| {
                (
                    f.load(Ordering::Relaxed),
                    decode_kind(k.load(Ordering::Relaxed)),
                )
            })
            .collect()
    }
}

/// Records every event arriving on `notes_in` with its timeline frame (`time.frame + offset`)
/// into a fixed-capacity, lock-free log; stops when full. `audio_out` stays silent.
#[derive(Debug)]
pub struct NoteLog {
    buf: Arc<NoteLogBuf>,
}

impl NoteLog {
    /// Notes input socket.
    pub const NOTES_IN: SocketId = SocketId(0);
    /// Audio output socket (always silent).
    pub const AUDIO_OUT: SocketId = SocketId(1);

    /// A log able to hold `capacity` events, and its test-side view.
    pub fn new(capacity: usize) -> (Self, NoteEvents) {
        let buf = Arc::new(NoteLogBuf {
            events: (0..capacity)
                .map(|_| (AtomicU64::new(0), AtomicU64::new(0)))
                .collect(),
            len: AtomicUsize::new(0),
        });
        (
            Self {
                buf: Arc::clone(&buf),
            },
            NoteEvents(buf),
        )
    }

    /// Boxed log and its view.
    pub fn boxed(capacity: usize) -> (Box<dyn Device>, NoteEvents) {
        let (d, v) = Self::new(capacity);
        (Box::new(d), v)
    }
}

impl Device for NoteLog {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &NOTE_LOG
    }
    fn prepare(&mut self, _: &PrepareInfo) {
        self.buf.len.store(0, Ordering::Release);
    }
    fn reset(&mut self) {}
    fn set_param(&mut self, _: ParamId, _: f32) {}
    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let mut len = self.buf.len.load(Ordering::Relaxed);
        for event in ctx.notes_in(Self::NOTES_IN) {
            let Some((frame, kind)) = self.buf.events.get(len) else {
                break;
            };
            frame.store(ctx.time.frame + u64::from(event.offset), Ordering::Relaxed);
            kind.store(encode_kind(event.kind), Ordering::Relaxed);
            len += 1;
        }
        self.buf.len.store(len, Ordering::Release);
    }
}

// ---------------------------------------------------------------------------------------------
// ParamLog
// ---------------------------------------------------------------------------------------------

/// Descriptor of [`ParamLog`]: `notes_in` (Notes in), `audio_out` (Audio out, silent), params
/// `a` (0..1) and `b` (-10..10). Not listed in [`descriptors`].
pub static PARAM_LOG: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "testkit.param-log",
    name: "Param Log",
    kind: DeviceKind::Utility,
    version: 1,
    params: &[
        linear_param(0, "a", 0.0, 1.0, 0.0),
        linear_param(1, "b", -10.0, 10.0, 0.0),
    ],
    sockets: &[
        socket(0, "notes_in", SocketKind::Notes, Direction::Input),
        socket(1, "audio_out", SocketKind::Audio, Direction::Output),
    ],
};

/// One [`ParamLog`] observation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ParamLogEntry {
    /// `set_param(param, plain)`.
    Set(ParamId, f32),
    /// `process` with `TimeInfo::frame`.
    Process(u64),
}

/// Test-side view of a [`ParamLog`].
#[derive(Debug, Clone)]
pub struct ParamEvents(Arc<NoteLogBuf>);

impl ParamEvents {
    /// Every observation in call order (at most the log's capacity).
    pub fn entries(&self) -> Vec<ParamLogEntry> {
        let len = self.0.len.load(Ordering::Acquire);
        self.0.events[..len]
            .iter()
            .map(|(a, b)| {
                let (a, b) = (a.load(Ordering::Relaxed), b.load(Ordering::Relaxed));
                if a == u64::MAX {
                    ParamLogEntry::Process(b)
                } else {
                    ParamLogEntry::Set(ParamId(a as u32), f32::from_bits(b as u32))
                }
            })
            .collect()
    }

    /// For each `process` call preceded (since the previous one) by `set_param(param, ..)`,
    /// `(frame, last value set)`.
    pub fn applied(&self, param: ParamId) -> Vec<(u64, f32)> {
        let mut out = Vec::new();
        let mut pending = None;
        for entry in self.entries() {
            match entry {
                ParamLogEntry::Set(p, v) if p == param => pending = Some(v),
                ParamLogEntry::Set(..) => {}
                ParamLogEntry::Process(frame) => {
                    if let Some(v) = pending.take() {
                        out.push((frame, v));
                    }
                }
            }
        }
        out
    }
}

/// Records every `set_param` call and every `process` frame into a fixed-capacity, lock-free
/// log (what the sequencer's automation applied, batch by batch).
#[derive(Debug)]
pub struct ParamLog {
    buf: Arc<NoteLogBuf>,
}

impl ParamLog {
    /// Notes input socket.
    pub const NOTES_IN: SocketId = SocketId(0);
    /// Audio output socket (silent).
    pub const AUDIO_OUT: SocketId = SocketId(1);
    /// Param `a` (0..1).
    pub const A: ParamId = ParamId(0);
    /// Param `b` (-10..10).
    pub const B: ParamId = ParamId(1);

    /// A log able to hold `capacity` entries, and its view.
    pub fn new(capacity: usize) -> (Self, ParamEvents) {
        let buf = Arc::new(NoteLogBuf {
            events: (0..capacity)
                .map(|_| (AtomicU64::new(0), AtomicU64::new(0)))
                .collect(),
            len: AtomicUsize::new(0),
        });
        (
            Self {
                buf: Arc::clone(&buf),
            },
            ParamEvents(buf),
        )
    }

    fn log(&self, a: u64, b: u64) {
        let len = self.buf.len.load(Ordering::Relaxed);
        if let Some((x, y)) = self.buf.events.get(len) {
            x.store(a, Ordering::Relaxed);
            y.store(b, Ordering::Relaxed);
            self.buf.len.store(len + 1, Ordering::Release);
        }
    }

    /// Boxed log and its view.
    pub fn boxed(capacity: usize) -> (Box<dyn Device>, ParamEvents) {
        let (d, v) = Self::new(capacity);
        (Box::new(d), v)
    }
}

impl Device for ParamLog {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &PARAM_LOG
    }
    fn prepare(&mut self, _: &PrepareInfo) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, param: ParamId, plain: f32) {
        self.log(u64::from(param.0), u64::from(plain.to_bits()));
    }
    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        self.log(u64::MAX, ctx.time.frame);
    }
}

// ---------------------------------------------------------------------------------------------
// SummingBus
// ---------------------------------------------------------------------------------------------

/// Descriptor of [`SummingBus`]: `sum_in` (Audio **summing** in), `audio_out`, `cv_sum_in`
/// (Cv summing in), `cv_out`, `notes_sum_in` (Notes summing in), `notes_out`, and a plain
/// `audio_in` that is added to `audio_out` (for split-path tests).
pub static SUMMING_BUS: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "testkit.summing-bus",
    name: "Summing Bus",
    kind: DeviceKind::Mixer,
    version: 1,
    params: &[],
    sockets: &[
        socket(0, "sum_in", SocketKind::Audio, Direction::SummingInput),
        socket(1, "audio_out", SocketKind::Audio, Direction::Output),
        socket(2, "cv_sum_in", SocketKind::Cv, Direction::SummingInput),
        socket(3, "cv_out", SocketKind::Cv, Direction::Output),
        socket(
            4,
            "notes_sum_in",
            SocketKind::Notes,
            Direction::SummingInput,
        ),
        socket(5, "notes_out", SocketKind::Notes, Direction::Output),
        socket(6, "audio_in", SocketKind::Audio, Direction::Input),
    ],
};

/// A console-bus stand-in: `audio_out = sum_in + audio_in`, `cv_out = cv_sum_in`, and every
/// event of `notes_sum_in` copied to `notes_out`. The engine does the summing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SummingBus;

impl SummingBus {
    /// Audio summing input.
    pub const SUM_IN: SocketId = SocketId(0);
    /// Audio output.
    pub const AUDIO_OUT: SocketId = SocketId(1);
    /// Cv summing input.
    pub const CV_SUM_IN: SocketId = SocketId(2);
    /// Cv output.
    pub const CV_OUT: SocketId = SocketId(3);
    /// Notes summing input.
    pub const NOTES_SUM_IN: SocketId = SocketId(4);
    /// Notes output.
    pub const NOTES_OUT: SocketId = SocketId(5);
    /// Plain Audio input, added to `audio_out`.
    pub const AUDIO_IN: SocketId = SocketId(6);

    /// Boxed instance.
    pub fn boxed() -> Box<dyn Device> {
        Box::new(Self)
    }
}

impl Device for SummingBus {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &SUMMING_BUS
    }
    fn prepare(&mut self, _: &PrepareInfo) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _: ParamId, _: f32) {}
    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let sum = *ctx.input(Self::SUM_IN);
        let plain = *ctx.input(Self::AUDIO_IN);
        for ((o, s), p) in ctx.output(Self::AUDIO_OUT).iter_mut().zip(sum).zip(plain) {
            *o = s + p;
        }
        let cv = *ctx.input(Self::CV_SUM_IN);
        ctx.output(Self::CV_OUT).copy_from_slice(&cv);
        for i in 0..ctx.notes_in(Self::NOTES_SUM_IN).len() {
            let event = ctx.notes_in(Self::NOTES_SUM_IN)[i];
            // Same capacity as the input, so this cannot overflow; ignore if it ever does.
            let _ = ctx.notes_out(Self::NOTES_OUT).push(event);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// LatencyPassthrough
// ---------------------------------------------------------------------------------------------

/// Largest value of the [`LatencyPassthrough`] `latency` param, in frames.
pub const LATENCY_PARAM_MAX: u32 = 8192;

/// Descriptor of [`LatencyPassthrough`]: `audio_in`, `audio_out`, param `latency` (frames,
/// 0..[`LATENCY_PARAM_MAX`], discrete).
pub static LATENCY_PASSTHROUGH: DeviceDescriptor = DeviceDescriptor {
    stages: &[],
    type_id: "testkit.latency-passthrough",
    name: "Latency Passthrough",
    kind: DeviceKind::Effect,
    version: 1,
    params: &[ParamDescriptor {
        discrete: true,
        ..linear_param(0, "latency", 0.0, LATENCY_PARAM_MAX as f32, 0.0)
    }],
    sockets: &[
        socket(0, "audio_in", SocketKind::Audio, Direction::Input),
        socket(1, "audio_out", SocketKind::Audio, Direction::Output),
    ],
};

/// A pure delay of `N` frames that reports `latency_frames() = N` (PDC tests).
///
/// `N` is set at construction (any size) or through the `latency` param (clamped to
/// [`LATENCY_PARAM_MAX`]). The ring buffer is allocated in `prepare`, large enough for both.
#[derive(Debug, Clone, PartialEq)]
pub struct LatencyPassthrough {
    delay: u32,
    ring: Vec<f32>,
    write: usize,
}

impl LatencyPassthrough {
    /// Audio input socket.
    pub const AUDIO_IN: SocketId = SocketId(0);
    /// Audio output socket.
    pub const AUDIO_OUT: SocketId = SocketId(1);
    /// The `latency` param (frames).
    pub const LATENCY: ParamId = ParamId(0);

    /// A delay of `frames` (not yet prepared).
    pub fn new(frames: u32) -> Self {
        Self {
            delay: frames,
            ring: Vec::new(),
            write: 0,
        }
    }

    /// Boxed delay of `frames`.
    pub fn boxed(frames: u32) -> Box<dyn Device> {
        Box::new(Self::new(frames))
    }
}

impl Device for LatencyPassthrough {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &LATENCY_PASSTHROUGH
    }
    fn prepare(&mut self, _: &PrepareInfo) {
        let capacity = self.delay.max(LATENCY_PARAM_MAX) as usize + 1;
        self.ring = vec![0.0; capacity];
        self.write = 0;
    }
    fn reset(&mut self) {
        self.ring.fill(0.0);
        self.write = 0;
    }
    fn set_param(&mut self, id: ParamId, plain: f32) {
        if id == Self::LATENCY {
            self.delay = plain.round().clamp(0.0, LATENCY_PARAM_MAX as f32) as u32;
        }
    }
    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let input = *ctx.input(Self::AUDIO_IN);
        let capacity = self.ring.len();
        if capacity == 0 {
            return; // not prepared
        }
        let delay = (self.delay as usize).min(capacity - 1);
        for (o, i) in ctx.output(Self::AUDIO_OUT).iter_mut().zip(input) {
            self.ring[self.write] = i;
            *o = self.ring[(self.write + capacity - delay) % capacity];
            self.write = (self.write + 1) % capacity;
        }
    }
    fn latency_frames(&self) -> u32 {
        self.delay
    }
    fn variable_latency(&self) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------------------------
// InsertLoop (§9: a two-stage device)
// ---------------------------------------------------------------------------------------------

/// Descriptor of [`InsertLoop`]: stage 0 `in` (Audio in) -> `send` (Audio out); stage 1 `ret`
/// (Audio in) -> `out` (Audio out). Param `dry` (0..1).
pub static INSERT_LOOP: DeviceDescriptor = DeviceDescriptor {
    type_id: "testkit.insert-loop",
    name: "Insert Loop",
    kind: DeviceKind::Mixer,
    version: 1,
    params: &[linear_param(0, "dry", 0.0, 1.0, 0.5)],
    sockets: &[
        socket(0, "in", SocketKind::Audio, Direction::Input),
        socket(1, "send", SocketKind::Audio, Direction::Output),
        socket(2, "ret", SocketKind::Audio, Direction::Input),
        socket(3, "out", SocketKind::Audio, Direction::Output),
    ],
    stages: &[0, 0, 1, 1],
};

/// A channel-insert stand-in with two processing stages: stage 0 writes `send = in`; stage 1
/// writes `out = ret + dry * in` (it reads its stage-0 input again, which stage ≤ 1 may).
/// Reports `latency_frames()` = the value given to [`InsertLoop::with_latency`] without
/// delaying anything (for PDC report tests). Stage calls are counted per stage.
#[derive(Debug, Clone)]
pub struct InsertLoop {
    dry: f32,
    latency: u32,
    calls: Arc<[AtomicU64; 2]>,
}

impl InsertLoop {
    /// Stage-0 audio input.
    pub const IN: SocketId = SocketId(0);
    /// Stage-0 audio output (to the insert effect).
    pub const SEND: SocketId = SocketId(1);
    /// Stage-1 audio input (from the insert effect).
    pub const RET: SocketId = SocketId(2);
    /// Stage-1 audio output.
    pub const OUT: SocketId = SocketId(3);
    /// The `dry` param.
    pub const DRY: ParamId = ParamId(0);

    /// Boxed instance with `dry` = 0.5 and no reported latency.
    pub fn boxed() -> Box<dyn Device> {
        Self::with_latency(0).0
    }

    /// Boxed instance reporting `latency` frames, plus its per-stage call counters.
    pub fn with_latency(latency: u32) -> (Box<dyn Device>, Arc<[AtomicU64; 2]>) {
        let calls = Arc::new([AtomicU64::new(0), AtomicU64::new(0)]);
        let device = Box::new(Self {
            dry: 0.5,
            latency,
            calls: Arc::clone(&calls),
        });
        (device, calls)
    }
}

impl Device for InsertLoop {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        &INSERT_LOOP
    }
    fn prepare(&mut self, _: &PrepareInfo) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, id: ParamId, plain: f32) {
        if id == Self::DRY {
            self.dry = plain;
        }
    }
    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        self.process_stage(0, ctx);
    }
    fn process_stage(&mut self, stage: u8, ctx: &mut ProcessContext<'_>) {
        if let Some(c) = self.calls.get(usize::from(stage)) {
            c.fetch_add(1, Ordering::Relaxed);
        }
        let input = *ctx.input(Self::IN);
        if stage == 0 {
            ctx.output(Self::SEND).copy_from_slice(&input);
        } else {
            let ret = *ctx.input(Self::RET);
            let dry = self.dry;
            for ((o, r), i) in ctx.output(Self::OUT).iter_mut().zip(ret).zip(input) {
                *o = r + dry * i;
            }
        }
    }
    fn latency_frames(&self) -> u32 {
        self.latency
    }
    fn variable_latency(&self) -> bool {
        true
    }
}

/// Every fake descriptor, for registry-style lookups in tests.
pub fn descriptors() -> [&'static DeviceDescriptor; 8] {
    [
        &CONSTANT_SOURCE,
        &IMPULSE_ON_NOTE,
        &PASSTHROUGH,
        &CALL_LOG,
        &DROP_PROBE,
        &ALLOCATOR,
        &RECORDING_SINK,
        &NOTE_THRU,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::DeviceHarness;
    use atomik_core::{BATCH, NoteEvent};

    const SR: f64 = 48_000.0;

    fn on(offset: u16) -> NoteEvent {
        NoteEvent {
            offset,
            kind: NoteKind::On {
                key: 60,
                velocity: 0.5,
            },
        }
    }

    #[test]
    fn descriptors_all_validate_with_testkit_type_ids() {
        for d in descriptors() {
            assert_eq!(d.validate(), Ok(()), "{}", d.type_id);
            assert!(d.type_id.starts_with("testkit."), "{}", d.type_id);
        }
    }

    #[test]
    fn tempo_probe_writes_the_batch_tempo() {
        assert_eq!(TEMPO_PROBE.validate(), Ok(()));
        for bpm in [120.0, 97.5] {
            let mut h = DeviceHarness::with_options(TempoProbe::boxed(), SR, bpm, 0);
            h.process();
            assert!(
                h.output("audio_out")
                    .iter()
                    .all(|s| s.to_bits() == (bpm as f32).to_bits())
            );
        }
    }

    #[test]
    fn constant_source_process_writes_value_to_both_outputs() {
        let mut h = DeviceHarness::new(ConstantSource::boxed(0.25), SR);
        h.process();
        assert!(
            h.output("audio_out")
                .iter()
                .all(|s| (s - 0.25).abs() < 1e-7)
        );
        assert!(h.output("cv_out").iter().all(|s| (s - 0.25).abs() < 1e-7));
        h.set_param("value", -0.5);
        h.process();
        assert!(h.output("audio_out").iter().all(|s| (s + 0.5).abs() < 1e-7));
    }

    #[test]
    fn impulse_on_note_process_writes_one_at_note_on_offsets() {
        let mut h = DeviceHarness::new(ImpulseOnNote::boxed(), SR);
        h.push_note("notes_in", on(3));
        h.push_note(
            "notes_in",
            NoteEvent {
                offset: 5,
                kind: NoteKind::Off { key: 60 },
            },
        );
        h.push_note("notes_in", on(40));
        h.process();
        let out = h.output("audio_out");
        for (i, s) in out.iter().enumerate() {
            let want = if i == 3 || i == 40 { 1.0 } else { 0.0 };
            assert!((s - want).abs() < 1e-7, "sample {i} = {s}");
        }
        h.process();
        assert!(h.output("audio_out").iter().all(|s| s.abs() < 1e-7));
    }

    #[test]
    fn passthrough_process_scales_input_by_gain() {
        let mut h = DeviceHarness::new(Passthrough::boxed(2.0), SR);
        for (i, s) in h.input_mut("audio_in").iter_mut().enumerate() {
            *s = i as f32 / BATCH as f32;
        }
        h.process();
        for (i, s) in h.output("audio_out").iter().enumerate() {
            assert!((s - 2.0 * i as f32 / BATCH as f32).abs() < 1e-6);
        }
    }

    #[test]
    fn call_log_counts_every_call() {
        let (dev, counts) = CallLog::boxed();
        let mut h = DeviceHarness::with_options(dev, SR, 120.0, 42);
        h.set_param("p", 0.75);
        h.input_mut("audio_in")[7] = 0.5;
        h.push_note("notes_in", on(0));
        h.process();
        h.process();
        h.device_mut().reset();
        assert_eq!(counts.prepare(), 1);
        assert_eq!(counts.last_seed(), 42);
        assert_eq!(counts.set_param(), 1);
        assert!((counts.last_param() - 0.75).abs() < 1e-7);
        assert_eq!(counts.process(), 2);
        assert_eq!(counts.last_frame(), BATCH as u64);
        assert_eq!(counts.notes_seen(), 1);
        assert_eq!(counts.reset(), 1);
        assert!((h.output("audio_out")[7] - 0.5).abs() < 1e-7);
    }

    #[test]
    fn drop_probe_records_dropping_thread() {
        let (probe, record) = DropProbe::new();
        assert_eq!(record.thread(), None);
        let id = std::thread::spawn(move || {
            drop(probe);
            std::thread::current().id()
        })
        .join()
        .unwrap();
        assert_eq!(record.thread(), Some(id));
        assert_ne!(record.thread(), Some(std::thread::current().id()));
    }

    #[test]
    #[should_panic(expected = "HI-1 violated")]
    fn allocator_process_is_caught_by_guard() {
        let mut h = DeviceHarness::new(Allocator::boxed(), SR);
        h.process();
    }

    #[test]
    fn recording_sink_captures_input_up_to_capacity() {
        let (dev, rec) = RecordingSink::boxed(100);
        let mut h = DeviceHarness::new(dev, SR);
        for (i, s) in h.input_mut("audio_in").iter_mut().enumerate() {
            *s = i as f32;
        }
        h.process();
        h.process();
        h.process();
        let got = rec.samples();
        assert_eq!(got.len(), 100);
        for (n, s) in got.iter().enumerate() {
            assert!((s - (n % BATCH) as f32).abs() < 1e-7);
        }
    }

    #[test]
    fn note_log_records_timeline_frame_and_kind_in_order() {
        assert_eq!(NOTE_LOG.validate(), Ok(()));
        let (dev, log) = NoteLog::boxed(3);
        let mut h = DeviceHarness::new(dev, SR);
        h.push_note("notes_in", on(2));
        h.push_note(
            "notes_in",
            NoteEvent {
                offset: 2,
                kind: NoteKind::Off { key: 61 },
            },
        );
        h.process();
        h.push_note(
            "notes_in",
            NoteEvent {
                offset: 0,
                kind: NoteKind::AllOff,
            },
        );
        h.push_note("notes_in", on(5));
        h.process();
        assert_eq!(
            log.events(),
            vec![
                (2, on(2).kind),
                (2, NoteKind::Off { key: 61 }),
                (BATCH as u64, NoteKind::AllOff),
            ],
            "capacity 3: the fourth event is not recorded"
        );
    }

    #[test]
    fn note_thru_process_copies_notes_in_to_out() {
        let mut h = DeviceHarness::new(NoteThru::boxed(), SR);
        h.push_note("notes_in", on(2));
        h.push_note("notes_in", on(9));
        h.process();
        assert_eq!(h.notes_out("notes_out"), &[on(2), on(9)]);
        h.process();
        assert!(h.notes_out("notes_out").is_empty());
    }

    #[test]
    fn param_log_records_set_param_calls_per_batch() {
        assert_eq!(PARAM_LOG.validate(), Ok(()));
        let (dev, log) = ParamLog::boxed(8);
        let mut h = DeviceHarness::new(dev, SR);
        h.set_param("a", 0.25);
        h.set_param("b", -3.0);
        h.process();
        h.process();
        h.set_param("a", 0.5);
        h.set_param("a", 0.75);
        h.process();
        assert_eq!(
            log.entries(),
            vec![
                ParamLogEntry::Set(ParamLog::A, 0.25),
                ParamLogEntry::Set(ParamLog::B, -3.0),
                ParamLogEntry::Process(0),
                ParamLogEntry::Process(BATCH as u64),
                ParamLogEntry::Set(ParamLog::A, 0.5),
                ParamLogEntry::Set(ParamLog::A, 0.75),
                ParamLogEntry::Process(2 * BATCH as u64),
            ]
        );
        assert_eq!(
            log.applied(ParamLog::A),
            vec![(0, 0.25), (2 * BATCH as u64, 0.75)],
            "the last value set before each process call"
        );
        assert_eq!(log.applied(ParamLog::B), vec![(0, -3.0)]);
        // Capacity 8: one more entry fits, then logging stops.
        h.process();
        h.process();
        assert_eq!(log.entries().len(), 8);
    }

    #[test]
    fn insert_loop_runs_two_stages() {
        assert_eq!(INSERT_LOOP.validate(), Ok(()));
        assert_eq!(INSERT_LOOP.stage_count(), 2);
        let (mut dev, calls) = InsertLoop::with_latency(9);
        assert_eq!(dev.latency_frames(), 9);
        dev.set_param(InsertLoop::DRY, 0.25);
        let mut signals = vec![[0.0f32; BATCH]; 4];
        signals[0][2] = 1.0;
        signals[2][2] = 3.0;
        let mut notes: Vec<_> = (0..4)
            .map(|_| atomik_core::NoteBuffer::with_capacity(0))
            .collect();
        let connected = [true; 4];
        let time = atomik_core::TimeInfo::at(
            0,
            atomik_core::SampleRate::new(SR).unwrap(),
            atomik_core::Tempo::new(120.0).unwrap(),
            false,
        );
        {
            let mut ctx = ProcessContext::new(time, &mut signals, &mut notes, &connected);
            dev.process(&mut ctx);
            dev.process_stage(1, &mut ctx);
            dev.process_stage(7, &mut ctx);
        }
        assert_eq!(signals[1][2], 1.0, "send = in");
        assert_eq!(signals[3][2], 3.25, "out = ret + dry * in");
        assert_eq!(calls[0].load(Ordering::Relaxed), 1);
        assert_eq!(
            calls[1].load(Ordering::Relaxed),
            1,
            "stage 7 is not counted"
        );
        dev.reset();
        let mut h = DeviceHarness::new(InsertLoop::boxed(), SR);
        h.process();
    }

    #[test]
    fn summing_bus_adds_the_plain_input_and_copies_cv_and_notes() {
        assert_eq!(SUMMING_BUS.validate(), Ok(()));
        let mut h = DeviceHarness::new(SummingBus::boxed(), SR);
        h.input_mut("sum_in")[3] = 0.5;
        h.input_mut("audio_in")[3] = 0.25;
        h.input_mut("cv_sum_in")[1] = -0.5;
        h.push_note("notes_sum_in", on(4));
        h.process();
        assert_eq!(h.output("audio_out")[3], 0.75);
        assert_eq!(h.output("cv_out")[1], -0.5);
        assert_eq!(h.notes_out("notes_out"), &[on(4)]);
        h.process();
        assert!(
            h.notes_out("notes_out").is_empty(),
            "summing note inputs are consumed after each batch"
        );
    }

    #[test]
    fn latency_passthrough_delays_by_its_reported_latency() {
        assert_eq!(LATENCY_PASSTHROUGH.validate(), Ok(()));
        for frames in [0u32, 1, 63, 64, 100] {
            let mut h = DeviceHarness::new(LatencyPassthrough::boxed(frames), SR);
            assert_eq!(h.device_mut().latency_frames(), frames);
            let mut out = Vec::new();
            for b in 0..4 {
                h.input_mut("audio_in")[0] = if b == 0 { 1.0 } else { 0.0 };
                h.process();
                out.extend_from_slice(h.output("audio_out"));
            }
            let at: Vec<usize> = (0..out.len()).filter(|&n| out[n] != 0.0).collect();
            assert_eq!(at, vec![frames as usize], "{frames}");
        }
        let mut h = DeviceHarness::new(LatencyPassthrough::boxed(10), SR);
        h.set_param("latency", 3.4);
        assert_eq!(h.device_mut().latency_frames(), 3);
        h.set_param("latency", 1e9);
        assert_eq!(h.device_mut().latency_frames(), LATENCY_PARAM_MAX);
        h.device_mut().reset();
        let mut unprepared = LatencyPassthrough::new(5);
        assert_eq!(unprepared.latency_frames(), 5);
        unprepared.reset();
        let mut ctx_signals = vec![[1.0f32; BATCH]; 2];
        let mut notes = vec![
            atomik_core::NoteBuffer::with_capacity(0),
            atomik_core::NoteBuffer::with_capacity(0),
        ];
        let connected = [true, true];
        let time = atomik_core::TimeInfo::at(
            0,
            atomik_core::SampleRate::new(SR).unwrap(),
            atomik_core::Tempo::new(120.0).unwrap(),
            false,
        );
        let mut ctx = ProcessContext::new(time, &mut ctx_signals, &mut notes, &connected);
        unprepared.process(&mut ctx); // unprepared: a no-op, never panics
    }
}
