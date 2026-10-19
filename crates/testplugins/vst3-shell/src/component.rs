//! The processor half of a test plugin: `IComponent`, `IAudioProcessor`,
//! `IProcessContextRequirements` and the connection point, over a [`Dsp`].

use crate::api::{Arrangement, Dsp, Info, Io, ParamSpec, PluginDef, Ports, Status};
use crate::hooks::{self, Shared, bump, counter, probe};
use crate::util::{copy_wstr, cstr_text, plain_of, read_all, write_all};
use atomik_test_plugin_core::{BLOCK, Event, ExprKind, TimedEvent};
use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use vst3::Steinberg::Vst::Event as VstEvent;
use vst3::{ComPtr, ComRef, Steinberg::Vst::*, Steinberg::*};

/// The hidden params the controller maps MIDI controllers to (`IMidiMapping`).
pub const MIDI_BEND: u32 = 0x7F00_0001;
/// CC 1.
pub const MIDI_MOD: u32 = 0x7F00_0002;
/// Channel pressure.
pub const MIDI_PRESSURE: u32 = 0x7F00_0003;
/// CC 123.
pub const MIDI_ALL_OFF: u32 = 0x7F00_0004;

/// The hidden param of a MIDI controller number, if the plugin maps it.
pub(crate) fn midi_param(controller: i16) -> Option<u32> {
    match i32::from(controller) {
        129 => Some(MIDI_BEND),
        1 => Some(MIDI_MOD),
        128 => Some(MIDI_PRESSURE),
        123 => Some(MIDI_ALL_OFF),
        _ => None,
    }
}

const EVENT_CAPACITY: usize = 8_192;

struct Proc {
    dsp: Box<dyn Dsp>,
    pending: Vec<(u16, u32, TimedEvent)>,
    events: Vec<TimedEvent>,
    out_events: Vec<TimedEvent>,
    params: Vec<ParamSpec>,
    grown: bool,
}

/// The processor core. Every method is called by the COM wrapper classes.
pub(crate) struct ComponentCore {
    pub info: &'static Info,
    pub shared: Arc<Shared>,
    proc: Mutex<Proc>,
    ports: Ports,
    host: Mutex<Option<ComPtr<IHostApplication>>>,
    peer: Mutex<Option<ComPtr<IConnectionPoint>>>,
    rate: AtomicU64,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl ComponentCore {
    pub fn new(def: &'static PluginDef, shared: Arc<Shared>) -> Self {
        let dsp = (def.create)();
        let ports = dsp.ports();
        let params = dsp.params();
        bump(counter::CREATED);
        Self {
            info: def.info,
            shared,
            proc: Mutex::new(Proc {
                dsp,
                pending: Vec::with_capacity(EVENT_CAPACITY),
                events: Vec::with_capacity(EVENT_CAPACITY),
                out_events: Vec::with_capacity(EVENT_CAPACITY),
                params,
                grown: false,
            }),
            ports,
            host: Mutex::new(None),
            peer: Mutex::new(None),
            rate: AtomicU64::new(48_000f64.to_bits()),
        }
    }

    // ----- IPluginBase -----------------------------------------------------------------------

    pub unsafe fn initialize(&self, context: *mut FUnknown) -> tresult {
        // SAFETY: the host passes null or a valid FUnknown.
        if let Some(ctx) = unsafe { ComRef::from_raw(context) } {
            *lock(&self.host) = ctx.cast::<IHostApplication>();
        }
        if lock(&self.proc).dsp.init() {
            kResultOk
        } else {
            kResultFalse
        }
    }

    pub fn terminate(&self) -> tresult {
        *lock(&self.host) = None;
        *lock(&self.peer) = None;
        kResultOk
    }

    // ----- buses -------------------------------------------------------------------------------

    pub fn bus_count(&self, media: MediaType, dir: BusDirection) -> i32 {
        let audio = media == MediaTypes_::kAudio as MediaType;
        let input = dir == BusDirections_::kInput as BusDirection;
        match (audio, input) {
            (true, true) => {
                i32::from(self.ports.audio_in.is_some()) + i32::from(self.ports.sidechain)
            }
            (true, false) => i32::from(self.ports.audio_out.is_some()),
            (false, true) => i32::from(self.ports.notes_in),
            (false, false) => i32::from(self.ports.notes_out),
        }
    }

    pub unsafe fn bus_info(
        &self,
        media: MediaType,
        dir: BusDirection,
        index: i32,
        bus: *mut BusInfo,
    ) -> tresult {
        if index < 0 || index >= self.bus_count(media, dir) {
            return kInvalidArgument;
        }
        // SAFETY: the host passes a valid BusInfo.
        let bus = unsafe { &mut *bus };
        bus.mediaType = media;
        bus.direction = dir;
        let audio = media == MediaTypes_::kAudio as MediaType;
        let input = dir == BusDirections_::kInput as BusDirection;
        if audio {
            let (arr, aux) = if input {
                if index == 0 && self.ports.audio_in.is_some() {
                    (self.ports.audio_in, false)
                } else {
                    (Some(Arrangement::Stereo), true)
                }
            } else {
                (self.ports.audio_out, false)
            };
            bus.channelCount = channels(arr.unwrap_or(Arrangement::Stereo)) as i32;
            copy_wstr(
                match (input, aux) {
                    (true, false) => "Input",
                    (true, true) => "Sidechain",
                    _ => "Output",
                },
                &mut bus.name,
            );
            bus.busType = if aux {
                BusTypes_::kAux as BusType
            } else {
                BusTypes_::kMain as BusType
            };
            bus.flags = if aux {
                0
            } else {
                BusInfo_::BusFlags_::kDefaultActive as u32
            };
        } else {
            bus.channelCount = 16;
            copy_wstr(if input { "Notes In" } else { "Notes Out" }, &mut bus.name);
            bus.busType = BusTypes_::kMain as BusType;
            bus.flags = BusInfo_::BusFlags_::kDefaultActive as u32;
        }
        kResultOk
    }

    pub fn activate_bus(
        &self,
        media: MediaType,
        dir: BusDirection,
        index: i32,
        state: bool,
    ) -> tresult {
        if index < 0 || index >= self.bus_count(media, dir) {
            return kInvalidArgument;
        }
        let audio = media == MediaTypes_::kAudio as MediaType;
        let input = dir == BusDirections_::kInput as BusDirection;
        let bit = match (audio, input, index) {
            (true, true, 0) => 1,
            (true, true, _) => 2,
            (true, false, _) => 4,
            (false, true, _) => 8,
            (false, false, _) => 16,
        };
        if state {
            self.shared.or(probe::BUS_ACTIVE, bit);
        } else {
            self.shared.and_not(probe::BUS_ACTIVE, bit);
        }
        kResultOk
    }

    pub unsafe fn set_bus_arrangements(
        &self,
        inputs: *mut SpeakerArrangement,
        num_ins: i32,
        outputs: *mut SpeakerArrangement,
        num_outs: i32,
    ) -> tresult {
        self.shared.add(probe::ARRANGEMENT_CALLS, 1);
        let want_in = self.bus_count(
            MediaTypes_::kAudio as MediaType,
            BusDirections_::kInput as BusDirection,
        );
        let want_out = self.bus_count(
            MediaTypes_::kAudio as MediaType,
            BusDirections_::kOutput as BusDirection,
        );
        if num_ins != want_in || num_outs != want_out {
            return kResultFalse;
        }
        let mut ok = true;
        for i in 0..num_ins {
            // SAFETY: the host passes `num_ins` arrangements.
            let a = unsafe { *inputs.offset(i as isize) };
            let want = if i == 0 && self.ports.audio_in.is_some() {
                arrangement_bits(self.ports.audio_in.unwrap_or(Arrangement::Stereo))
            } else {
                SpeakerArr::kStereo
            };
            ok &= a == want;
        }
        for i in 0..num_outs {
            // SAFETY: the host passes `num_outs` arrangements.
            let a = unsafe { *outputs.offset(i as isize) };
            ok &= a == arrangement_bits(self.ports.audio_out.unwrap_or(Arrangement::Stereo));
        }
        if ok { kResultTrue } else { kResultFalse }
    }

    pub unsafe fn bus_arrangement(
        &self,
        dir: BusDirection,
        index: i32,
        arr: *mut SpeakerArrangement,
    ) -> tresult {
        let media = MediaTypes_::kAudio as MediaType;
        if index < 0 || index >= self.bus_count(media, dir) {
            return kInvalidArgument;
        }
        let input = dir == BusDirections_::kInput as BusDirection;
        let a = if input {
            if index == 0 && self.ports.audio_in.is_some() {
                arrangement_bits(self.ports.audio_in.unwrap_or(Arrangement::Stereo))
            } else {
                SpeakerArr::kStereo
            }
        } else {
            arrangement_bits(self.ports.audio_out.unwrap_or(Arrangement::Stereo))
        };
        // SAFETY: the host passes a valid out pointer.
        unsafe { *arr = a };
        kResultOk
    }

    // ----- activation -------------------------------------------------------------------------

    pub fn set_active(&self, state: bool) -> tresult {
        if state {
            bump(counter::ACTIVATED);
            self.shared.add(probe::ACTIVATIONS, 1);
            // A split plugin tells its controller who it is once both halves are connected: the
            // controller then shares probes and answers.
            if self.info.controller_cid.is_some() {
                let peer = lock(&self.peer).clone();
                if let Some(peer) = peer {
                    self.send(&peer, "atomik.link", self.shared.number as i64);
                }
            }
        } else {
            bump(counter::DEACTIVATED);
            self.shared.add(probe::DEACTIVATIONS, 1);
            // Bus activation is per activation cycle in this plugin's books.
            self.shared.set(probe::BUS_ACTIVE, 0);
        }
        kResultOk
    }

    pub unsafe fn setup_processing(&self, setup: *mut ProcessSetup) -> tresult {
        // SAFETY: the host passes a valid ProcessSetup.
        let s = unsafe { &*setup };
        self.rate.store(s.sampleRate.to_bits(), Ordering::Relaxed);
        self.shared.set(probe::RENDER_MODE, s.processMode as u64);
        self.shared
            .set(probe::MAX_FRAMES, s.maxSamplesPerBlock as u64);
        self.shared.set(probe::SAMPLE_RATE, s.sampleRate.to_bits());
        let mut p = lock(&self.proc);
        if self.shared.grown() && !p.grown {
            p.grown = true;
            p.dsp.grow();
            p.params = p.dsp.params();
        }
        if p.dsp.activate(s.sampleRate) {
            kResultOk
        } else {
            kResultFalse
        }
    }

    pub fn set_processing(&self, state: bool) -> tresult {
        if state {
            self.shared.add(probe::PROCESSING_STARTS, 1);
            if !lock(&self.proc).dsp.start_processing() {
                return kResultFalse;
            }
        } else {
            self.shared.add(probe::PROCESSING_STOPS, 1);
            lock(&self.proc).dsp.stop_processing();
        }
        kResultOk
    }

    pub fn latency(&self) -> u32 {
        lock(&self.proc).dsp.latency()
    }

    pub fn tail(&self) -> u32 {
        lock(&self.proc).dsp.tail()
    }

    // ----- state -------------------------------------------------------------------------------

    pub unsafe fn get_state(&self, stream: *mut IBStream) -> tresult {
        self.shared.add(probe::STATE_SAVES, 1);
        let bytes = lock(&self.proc).dsp.save();
        // SAFETY: the host passes a valid stream.
        if unsafe { write_all(stream, &bytes) } {
            kResultOk
        } else {
            kResultFalse
        }
    }

    pub unsafe fn set_state(&self, stream: *mut IBStream) -> tresult {
        self.shared.add(probe::STATE_LOADS, 1);
        // SAFETY: the host passes a valid stream.
        let Some(bytes) = (unsafe { read_all(stream) }) else {
            return kInvalidArgument;
        };
        if lock(&self.proc).dsp.load(&bytes) {
            kResultOk
        } else {
            kResultFalse
        }
    }

    // ----- connection --------------------------------------------------------------------------

    pub unsafe fn connect(&self, other: *mut IConnectionPoint) -> tresult {
        self.shared.add(probe::CONNECTS, 1);
        // SAFETY: the host passes a valid connection point.
        let Some(peer) = (unsafe { ComRef::from_raw(other) }) else {
            return kInvalidArgument;
        };
        *lock(&self.peer) = Some(peer.to_com_ptr());
        kResultOk
    }

    pub unsafe fn disconnect(&self, _other: *mut IConnectionPoint) -> tresult {
        *lock(&self.peer) = None;
        kResultOk
    }

    pub unsafe fn notify(&self, message: *mut IMessage) -> tresult {
        // SAFETY: the host passes a valid message.
        if unsafe { ComRef::from_raw(message) }.is_some() {
            self.shared.add(probe::MESSAGES, 1);
        }
        kResultOk
    }

    /// Sends a message with one integer attribute `id` through `peer`.
    pub(crate) fn send(&self, peer: &ComPtr<IConnectionPoint>, name: &str, id: i64) {
        let host = lock(&self.host).clone();
        send_message(host.as_ref(), peer, name, id);
    }

    // ----- process -----------------------------------------------------------------------------

    pub unsafe fn process(&self, data: *mut ProcessData) -> tresult {
        // SAFETY: the host passes a valid ProcessData for the call.
        let d = unsafe { &*data };
        let mut guard = lock(&self.proc);
        let p = &mut *guard;
        p.pending.clear();
        p.events.clear();
        p.out_events.clear();
        let mut seq = 0u32;
        let mut seen = 0u64;
        let mut points_seen = 0u64;
        let mut max_offset = 0u64;
        // Parameter changes first: they sort before notes at an equal offset.
        // SAFETY: the host passes null or a valid list for the call.
        if let Some(changes) = unsafe { ComRef::from_raw(d.inputParameterChanges) } {
            // SAFETY: calling methods of the host's list.
            let count = unsafe { changes.getParameterCount() };
            for i in 0..count {
                // SAFETY: as above.
                let Some(queue) = (unsafe { ComRef::from_raw(changes.getParameterData(i)) }) else {
                    continue;
                };
                // SAFETY: as above.
                let (id, points) = unsafe { (queue.getParameterId(), queue.getPointCount()) };
                for k in 0..points {
                    let (mut off, mut value) = (0i32, 0f64);
                    // SAFETY: valid out pointers.
                    if unsafe { queue.getPoint(k, &mut off, &mut value) } != kResultOk {
                        continue;
                    }
                    seen += 1;
                    points_seen += 1;
                    max_offset = max_offset.max(off.max(0) as u64);
                    let event = match id {
                        MIDI_BEND => Some(Event::PitchBend((value * 2.0 - 1.0) as f32)),
                        MIDI_MOD => Some(Event::Cc {
                            controller: 1,
                            value: value as f32,
                        }),
                        MIDI_PRESSURE => Some(Event::ChannelPressure(value as f32)),
                        MIDI_ALL_OFF => Some(Event::Cc {
                            controller: 123,
                            value: value as f32,
                        }),
                        _ => p.params.iter().find(|s| s.id == id).map(|s| Event::Param {
                            id,
                            value: plain_of(s, value),
                        }),
                    };
                    if let Some(event) = event
                        && p.pending.len() < p.pending.capacity()
                    {
                        p.pending.push((
                            off.clamp(0, BLOCK as i32 - 1) as u16,
                            seq,
                            TimedEvent {
                                offset: off.clamp(0, BLOCK as i32 - 1) as u16,
                                event,
                            },
                        ));
                        seq += 1;
                    }
                }
            }
        }
        // SAFETY: the host passes null or a valid list for the call.
        if let Some(list) = unsafe { ComRef::from_raw(d.inputEvents) } {
            // SAFETY: calling methods of the host's list.
            let count = unsafe { list.getEventCount() };
            for i in 0..count {
                // SAFETY: `Event` is plain data; an all-zero value is valid.
                let mut e: VstEvent = unsafe { std::mem::zeroed() };
                // SAFETY: a valid out pointer.
                if unsafe { list.getEvent(i, &mut e) } != kResultOk {
                    continue;
                }
                seen += 1;
                let offset = e.sampleOffset.clamp(0, BLOCK as i32 - 1) as u16;
                let event = match u32::from(e.r#type) {
                    t if t == Event_::EventTypes_::kNoteOnEvent as u32 => {
                        // SAFETY: the type tag says which union member is live.
                        let n = unsafe { e.__field0.noteOn };
                        self.shared.set(probe::LAST_NOTE_ID, n.noteId as u32 as u64);
                        Some(Event::NoteOn {
                            key: n.pitch.clamp(0, 127) as u8,
                            velocity: n.velocity,
                            id: n.noteId,
                        })
                    }
                    t if t == Event_::EventTypes_::kNoteOffEvent as u32 => {
                        // SAFETY: as above.
                        let n = unsafe { e.__field0.noteOff };
                        Some(Event::NoteOff {
                            key: n.pitch.clamp(0, 127) as u8,
                            id: n.noteId,
                        })
                    }
                    t if t == Event_::EventTypes_::kPolyPressureEvent as u32 => {
                        // SAFETY: as above.
                        let n = unsafe { e.__field0.polyPressure };
                        Some(Event::PolyPressure {
                            key: n.pitch.clamp(0, 127) as u8,
                            id: n.noteId,
                            value: n.pressure,
                        })
                    }
                    t if t == Event_::EventTypes_::kNoteExpressionValueEvent as u32 => {
                        // SAFETY: as above.
                        let n = unsafe { e.__field0.noteExpressionValue };
                        let v = n.value;
                        let (kind, value) = match n.typeId {
                            0 => (ExprKind::Volume, v),
                            1 => (ExprKind::Pan, v * 2.0 - 1.0),
                            2 => (ExprKind::Tuning, (v - 0.5) * 240.0),
                            5 => (ExprKind::Slide, v),
                            _ => continue,
                        };
                        Some(Event::Expr {
                            key: 0,
                            id: n.noteId,
                            kind,
                            value: value as f32,
                        })
                    }
                    _ => None,
                };
                if let Some(event) = event
                    && p.pending.len() < p.pending.capacity()
                {
                    p.pending.push((offset, seq, TimedEvent { offset, event }));
                    seq += 1;
                }
            }
        }
        p.pending.sort_unstable_by_key(|(o, s, _)| (*o, *s));
        for (_, _, e) in &p.pending {
            p.events.push(*e);
        }
        self.shared.set(probe::EVENTS_IN, seen);
        self.shared.add(probe::PARAM_POINTS, points_seen);
        if points_seen > 0 {
            self.shared.set(probe::LAST_POINT_OFFSET, max_offset);
        }

        self.shared.add(probe::PROCESS_CALLS, 1);
        self.shared.set(probe::FRAMES, d.numSamples as u64);
        if !d.processContext.is_null() {
            // SAFETY: non-null and valid for the call.
            let c = unsafe { &*d.processContext };
            self.shared.set(probe::TRANSPORT_FLAGS, u64::from(c.state));
            self.shared.set(probe::TEMPO, c.tempo.to_bits());
            self.shared.set(probe::BEATS, c.projectTimeMusic.to_bits());
            self.shared.set(probe::SECONDS, c.projectTimeSamples as u64);
            self.shared
                .set(probe::STEADY, c.continousTimeSamples as u64);
            self.shared.set(probe::TSIG_NUM, c.timeSigNumerator as u64);
            self.shared
                .set(probe::TSIG_DEN, c.timeSigDenominator as u64);
            self.shared
                .set(probe::LOOP_START, c.cycleStartMusic.to_bits());
            self.shared.set(probe::LOOP_END, c.cycleEndMusic.to_bits());
        }
        if d.numSamples as usize != BLOCK {
            return kResultOk;
        }

        // Audio in.
        let mut input = [[0.0f32; BLOCK]; 2];
        let mut input_constant = true;
        let mut sidechain = None;
        let mut sidechain_constant = true;
        if self.ports.audio_in.is_some() {
            // SAFETY: the host passes `numInputs` bus descriptors.
            if let Some((blocks, silence, n)) = unsafe { read_bus(d.inputs, d.numInputs, 0) } {
                input = blocks;
                input_constant = silence == (1u64 << n) - 1 || n == 0;
                self.shared.set(probe::MASK_MAIN, silence);
            }
        }
        if self.ports.sidechain {
            // SAFETY: as above.
            if let Some((blocks, silence, _)) = unsafe { read_bus(d.inputs, d.numInputs, 1) } {
                sidechain = Some(blocks);
                sidechain_constant = silence == 0b11;
                self.shared.set(probe::MASK_SIDECHAIN, silence);
            }
        }

        let mut io = Io {
            events: &p.events,
            out_events: &mut p.out_events,
            input,
            input_constant,
            sidechain,
            sidechain_constant,
            output: [[0.0; BLOCK]; 2],
        };
        let latency_before = p.dsp.latency();
        let status = p.dsp.process(&mut io);
        let output = io.output;
        if p.dsp.latency() != latency_before {
            // A plugin whose latency changed (a param edit, a state) tells the host, which
            // re-reads `getLatencySamples` when it restarts the plugin (the CLAP test shell asks
            // for a restart the same way). Real plugins announce from their controller or from
            // a message; this test shell announces from the call that changed the latency.
            self.shared.announce_latency();
        }

        // Audio out.
        if d.numOutputs > 0 && !d.outputs.is_null() {
            // SAFETY: the host passes `numOutputs` bus descriptors.
            let bus = unsafe { &*d.outputs };
            // SAFETY: the host passes `numChannels` channel pointers when the bus is active.
            if bus.numChannels > 0 && !unsafe { bus.__field0.channelBuffers32 }.is_null() {
                for (c, block) in output.iter().enumerate().take(bus.numChannels as usize) {
                    // SAFETY: channel `c` points at `numSamples` (= BLOCK) floats.
                    let ptr = unsafe { *bus.__field0.channelBuffers32.add(c) };
                    if !ptr.is_null() {
                        // SAFETY: as above.
                        unsafe { std::ptr::copy_nonoverlapping(block.as_ptr(), ptr, BLOCK) };
                    }
                }
            }
        }
        // Note output.
        // SAFETY: the host passes null or a valid list.
        if let Some(list) = unsafe { ComRef::from_raw(d.outputEvents) } {
            for e in p.out_events.iter() {
                // SAFETY: plain data.
                let mut out: VstEvent = unsafe { std::mem::zeroed() };
                out.sampleOffset = i32::from(e.offset);
                match e.event {
                    Event::NoteOn { key, velocity, id } => {
                        out.r#type = Event_::EventTypes_::kNoteOnEvent as u16;
                        out.__field0.noteOn = NoteOnEvent {
                            channel: 0,
                            pitch: i16::from(key),
                            tuning: 0.0,
                            velocity,
                            length: 0,
                            noteId: id,
                        };
                    }
                    Event::NoteOff { key, id } => {
                        out.r#type = Event_::EventTypes_::kNoteOffEvent as u16;
                        out.__field0.noteOff = NoteOffEvent {
                            channel: 0,
                            pitch: i16::from(key),
                            velocity: 0.0,
                            noteId: id,
                            tuning: 0.0,
                        };
                    }
                    _ => continue,
                }
                // SAFETY: a valid event pointer for the call.
                unsafe { list.addEvent(&mut out) };
            }
        }
        if status == Status::Error {
            kResultFalse
        } else {
            kResultOk
        }
    }
}

impl Drop for ComponentCore {
    fn drop(&mut self) {
        bump(counter::DESTROYED);
        hooks::unregister(self.shared.number);
    }
}

fn channels(a: Arrangement) -> u32 {
    match a {
        Arrangement::Mono => 1,
        Arrangement::Stereo => 2,
        Arrangement::Surround => 6,
    }
}

fn arrangement_bits(a: Arrangement) -> SpeakerArrangement {
    match a {
        Arrangement::Mono => SpeakerArr::kMono,
        Arrangement::Stereo => SpeakerArr::kStereo,
        Arrangement::Surround => SpeakerArr::k51,
    }
}

/// Reads bus `index` of `buses` into two blocks (a mono bus is copied to both); returns the blocks,
/// the silence flags and the channel count.
///
/// # Safety
/// `buses` must point at `count` valid bus descriptors.
unsafe fn read_bus(
    buses: *mut AudioBusBuffers,
    count: i32,
    index: i32,
) -> Option<([[f32; BLOCK]; 2], u64, usize)> {
    if buses.is_null() || index >= count {
        return None;
    }
    // SAFETY: `index < count`.
    let bus = unsafe { &*buses.offset(index as isize) };
    let n = bus.numChannels.max(0) as usize;
    // SAFETY: reading the union member the host fills.
    let channel_ptrs = unsafe { bus.__field0.channelBuffers32 };
    let mut out = [[0.0f32; BLOCK]; 2];
    if n == 0 || channel_ptrs.is_null() {
        return Some((out, 0b11, 0));
    }
    for (c, block) in out.iter_mut().enumerate().take(n) {
        // SAFETY: channel `c` points at BLOCK floats.
        let ptr = unsafe { *channel_ptrs.add(c) };
        if !ptr.is_null() {
            // SAFETY: as above.
            unsafe { std::ptr::copy_nonoverlapping(ptr, block.as_mut_ptr(), BLOCK) };
        }
    }
    if n == 1 {
        out[1] = out[0];
    }
    Some((out, bus.silenceFlags, n))
}

/// Builds a host message named `name` with the integer attribute `id` and sends it to `peer`.
pub(crate) fn send_message(
    host: Option<&ComPtr<IHostApplication>>,
    peer: &ComPtr<IConnectionPoint>,
    name: &str,
    id: i64,
) {
    let Some(host) = host else { return };
    let mut cid = IMessage_iid;
    let mut iid = IMessage_iid;
    let mut obj: *mut c_void = std::ptr::null_mut();
    // SAFETY: valid ids and out pointer; the host returns an owned IMessage or fails.
    let r = unsafe { host.createInstance(&mut cid, &mut iid, &mut obj) };
    if r != kResultOk {
        return;
    }
    // SAFETY: the host handed us one reference to an IMessage.
    let Some(message) = (unsafe { ComPtr::from_raw(obj.cast::<IMessage>()) }) else {
        return;
    };
    let text = std::ffi::CString::new(name).unwrap_or_default();
    // SAFETY: the message is live; the id string outlives the calls.
    unsafe {
        message.setMessageID(text.as_ptr());
        if let Some(attrs) = ComRef::from_raw(message.getAttributes()) {
            attrs.setInt(c"id".as_ptr(), id);
        }
        peer.notify(message.as_ptr());
    }
}

/// Reads the integer attribute `id` of a message and its name.
///
/// # Safety
/// `message` must be a valid message.
pub(crate) unsafe fn read_message(message: ComRef<'_, IMessage>) -> (String, Option<i64>) {
    // SAFETY: a valid message.
    let name = unsafe { cstr_text(message.getMessageID()) };
    let mut v = 0i64;
    // SAFETY: a valid message and out pointer.
    let ok = unsafe {
        ComRef::from_raw(message.getAttributes())
            .is_some_and(|a| a.getInt(c"id".as_ptr(), &mut v) == kResultOk)
    };
    (name, ok.then_some(v))
}
