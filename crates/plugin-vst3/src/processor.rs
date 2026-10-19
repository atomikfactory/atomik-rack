//! `Vst3Processor`: the audio half of a VST3 plugin.
//! Everything it needs is allocated when the plugin is activated; `process` copies the host's
//! block into the plugin's buffers, turns the device's events into parameter changes and VST3
//! events, calls `IAudioProcessor::process` and copies the result back. The `unsafe` here is
//! the COM boundary and the raw channel pointers of `AudioBusBuffers`.

use crate::events::{EventListObj, zero_event};
use crate::host::Hub;
use crate::layout::{AudioPlan, Role};
use crate::params::ParamChanges;
use crate::values::{self, Steps};
use atomik_core::{BATCH, Batch, ExprKind};
use atomik_device_sdk::RunMode;
use atomik_plugin_host::{
    ParamPhase, PluginEvent, PluginEventKind, PluginIo, PluginMessage, PluginProcessor,
    ProcessStatus, SleepRule, TailLength,
};
use std::sync::Arc;
use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{ComPtr, ComWrapper};

/// Events per block the input list holds (a full note buffer and the Offs of an all-notes-off).
const EVENT_CAPACITY: usize = 4_096 + 512;
/// Capacity of the events a plugin may send out per block.
const OUT_EVENTS: usize = 512;
/// Params a plugin may change on its own per block.
const OUT_PARAMS: usize = 256;

/// Everything a processor is built from.
pub(crate) struct Setup {
    pub processor: ComPtr<IAudioProcessor>,
    pub component: ComPtr<IComponent>,
    pub plan: AudioPlan,
    pub steps: Steps,
    pub midi: Vec<Option<u32>>,
    pub expressions: u32,
    pub mode: RunMode,
    pub param_capacity: usize,
    pub hub: Arc<Hub>,
    pub edits: rtrb::Consumer<(u32, f64)>,
    /// Where every param value the rack sets is copied so the main thread can tell the
    /// controller (an editor shows the rack values).
    pub mirror: rtrb::Producer<(u32, f64)>,
}

/// One bus's place in the channel storage.
#[derive(Debug, Clone, Copy)]
struct Slot {
    first: usize,
    channels: usize,
    role: Role,
}

/// The audio half of a VST3 instance.
pub(crate) struct Vst3Processor {
    processor: ComPtr<IAudioProcessor>,
    _component: ComPtr<IComponent>,
    plan: AudioPlan,
    steps: Steps,
    midi: Vec<Option<u32>>,
    expressions: u32,
    mode: i32,
    hub: Arc<Hub>,
    edits: rtrb::Consumer<(u32, f64)>,
    mirror: rtrb::Producer<(u32, f64)>,
    in_slots: Vec<Slot>,
    out_slots: Vec<Slot>,
    in_data: Vec<Batch>,
    out_data: Vec<Batch>,
    in_ptrs: Vec<*mut f32>,
    out_ptrs: Vec<*mut f32>,
    in_buses: Vec<AudioBusBuffers>,
    out_buses: Vec<AudioBusBuffers>,
    changes_in: ComWrapper<ParamChanges>,
    changes_out: ComWrapper<ParamChanges>,
    events_in: ComWrapper<EventListObj>,
    events_out: ComWrapper<EventListObj>,
    context: ProcessContext,
    running: bool,
    tail: TailLength,
}

// SAFETY: the raw pointers point into heap buffers this struct owns, are rewritten before every
// `process` and only dereferenced by the plugin during that call on the owning thread; the COM
// wrappers of the lists are used only by that thread; the COM pointers are to objects the VST3
// specification lets the audio thread call.
unsafe impl Send for Vst3Processor {}

fn slots(plan: &[crate::layout::BusPlan]) -> (Vec<Slot>, usize) {
    let mut first = 0usize;
    let mut out = Vec::with_capacity(plan.len());
    for b in plan {
        let channels = if b.active() { b.channels as usize } else { 0 };
        out.push(Slot {
            first,
            channels,
            role: b.role,
        });
        first += channels;
    }
    (out, first)
}

fn bus_buffers(slots: &[Slot], ptrs: &mut [*mut f32]) -> Vec<AudioBusBuffers> {
    slots
        .iter()
        .map(|s| AudioBusBuffers {
            numChannels: s.channels as i32,
            silenceFlags: 0,
            __field0: AudioBusBuffers__type0 {
                channelBuffers32: if s.channels == 0 {
                    std::ptr::null_mut()
                } else {
                    ptrs[s.first..].as_mut_ptr()
                },
            },
        })
        .collect()
}

impl Vst3Processor {
    pub(crate) fn new(s: Setup) -> Self {
        let (in_slots, in_total) = slots(&s.plan.inputs);
        let (out_slots, out_total) = slots(&s.plan.outputs);
        let mut in_data = vec![[0.0; BATCH]; in_total];
        let mut out_data = vec![[0.0; BATCH]; out_total];
        let mut in_ptrs: Vec<*mut f32> = in_data.iter_mut().map(|b| b.as_mut_ptr()).collect();
        let mut out_ptrs: Vec<*mut f32> = out_data.iter_mut().map(|b| b.as_mut_ptr()).collect();
        let in_buses = bus_buffers(&in_slots, &mut in_ptrs);
        let out_buses = bus_buffers(&out_slots, &mut out_ptrs);
        // SAFETY: a ProcessContext is plain data; all-zero is a valid value.
        let context: ProcessContext = unsafe { std::mem::zeroed() };
        Self {
            processor: s.processor,
            _component: s.component,
            plan: s.plan,
            steps: s.steps,
            midi: s.midi,
            expressions: s.expressions,
            mode: match s.mode {
                RunMode::Realtime => ProcessModes_::kRealtime as i32,
                RunMode::Offline => ProcessModes_::kOffline as i32,
            },
            hub: s.hub,
            edits: s.edits,
            mirror: s.mirror,
            in_slots,
            out_slots,
            in_data,
            out_data,
            in_ptrs,
            out_ptrs,
            in_buses,
            out_buses,
            changes_in: ParamChanges::new(s.param_capacity),
            changes_out: ParamChanges::new(OUT_PARAMS),
            events_in: EventListObj::new(EVENT_CAPACITY),
            events_out: EventListObj::new(OUT_EVENTS),
            context,
            running: false,
            tail: TailLength::Finite(0),
        }
    }

    fn fill_inputs(&mut self, io: &PluginIo<'_>) {
        for (slot, bus) in self.in_slots.iter().zip(self.in_buses.iter_mut()) {
            let (source, silent): (&[Batch], bool) = match slot.role {
                Role::Main => (io.main_in, io.main_in_silent),
                Role::Sidechain => (io.sidechain, io.sidechain_silent),
                Role::Unused => (&[], true),
            };
            for c in 0..slot.channels {
                self.in_data[slot.first + c] = source
                    .get(c)
                    .or_else(|| source.first())
                    .copied()
                    .unwrap_or([0.0; BATCH]);
            }
            // Every channel of a silent bus is flagged (VST3: one bit per channel).
            bus.silenceFlags = if silent && slot.channels > 0 {
                (1u64 << slot.channels.min(63)) - 1
            } else {
                0
            };
        }
        self.out_data.fill([0.0; BATCH]);
        let _ = (self.in_ptrs.len(), self.out_ptrs.len());
    }

    /// Adds a note event; false (and nothing added) when there is no event bus or no room.
    fn push_event(&self, offset: u16, build: impl FnOnce(&mut Event)) -> bool {
        if !self.plan.event_in {
            return false;
        }
        let mut e = zero_event();
        e.busIndex = 0;
        e.sampleOffset = i32::from(offset);
        build(&mut e);
        self.events_in.push(e)
    }

    fn mapped(&self, controller: usize) -> Option<u32> {
        self.midi.get(controller).copied().flatten()
    }

    fn build_events(&mut self, io: &mut PluginIo<'_>) {
        self.changes_in.clear();
        self.events_in.clear();
        self.changes_out.clear();
        self.events_out.clear();
        let mut dropped = 0u32;
        // Edits made in the plugin's own editor reach the processor first, at offset 0.
        while let Ok((id, n)) = self.edits.pop() {
            if !self.changes_in.add_point(id, 0, n) {
                dropped += 1;
            }
        }
        let unit = |v: f32| f64::from(v.clamp(0.0, 1.0));
        for PluginEvent { offset, kind } in io.events_in.iter().copied() {
            let at = i32::from(offset);
            let ok = match kind {
                PluginEventKind::NoteOn {
                    key,
                    velocity,
                    note_id,
                } => self.push_event(offset, |e| {
                    e.r#type = Event_::EventTypes_::kNoteOnEvent as u16;
                    e.__field0.noteOn = NoteOnEvent {
                        channel: 0,
                        pitch: i16::from(key),
                        tuning: 0.0,
                        velocity,
                        length: 0,
                        noteId: note_id,
                    };
                }),
                PluginEventKind::NoteOff { key, note_id } => self.push_event(offset, |e| {
                    e.r#type = Event_::EventTypes_::kNoteOffEvent as u16;
                    e.__field0.noteOff = NoteOffEvent {
                        channel: 0,
                        pitch: i16::from(key),
                        velocity: 0.0,
                        noteId: note_id,
                        tuning: 0.0,
                    };
                }),
                PluginEventKind::PitchBend { value } => match self.mapped(129) {
                    Some(id) => self.changes_in.add_point(
                        id,
                        at,
                        (f64::from(value.clamp(-1.0, 1.0)) + 1.0) * 0.5,
                    ),
                    None => false,
                },
                PluginEventKind::Cc { controller, value } => {
                    match self.mapped(usize::from(controller)) {
                        Some(id) => self.changes_in.add_point(id, at, unit(value)),
                        // CC 123 follows the Offs the device already sent; an unmapped one is no loss.
                        None => controller == 123,
                    }
                }
                PluginEventKind::ChannelPressure { value } => match self.mapped(128) {
                    Some(id) => self.changes_in.add_point(id, at, unit(value)),
                    None => false,
                },
                PluginEventKind::PolyPressure {
                    key,
                    note_id,
                    value,
                } => self.push_event(offset, |e| {
                    e.r#type = Event_::EventTypes_::kPolyPressureEvent as u16;
                    e.__field0.polyPressure = PolyPressureEvent {
                        channel: 0,
                        pitch: i16::from(key),
                        pressure: value.clamp(0.0, 1.0),
                        noteId: note_id,
                    };
                }),
                PluginEventKind::NoteExpr {
                    key,
                    note_id,
                    kind,
                    value,
                } => match kind {
                    // VST3 has no pressure note expression: per-note pressure is its own event.
                    ExprKind::Pressure => self.push_event(offset, |e| {
                        e.r#type = Event_::EventTypes_::kPolyPressureEvent as u16;
                        e.__field0.polyPressure = PolyPressureEvent {
                            channel: 0,
                            pitch: i16::from(key),
                            pressure: value.clamp(0.0, 1.0),
                            noteId: note_id,
                        };
                    }),
                    _ => {
                        let (type_id, v) = match kind {
                            ExprKind::Volume => (0u32, f64::from(value)),
                            ExprKind::Pan => (1, (f64::from(value) + 1.0) * 0.5),
                            ExprKind::Tuning => (2, 0.5 + f64::from(value) / 240.0),
                            _ => (5, f64::from(value)),
                        };
                        self.expressions & (1 << type_id) != 0
                            && self.push_event(offset, |e| {
                                e.r#type = Event_::EventTypes_::kNoteExpressionValueEvent as u16;
                                e.__field0.noteExpressionValue = NoteExpressionValueEvent {
                                    typeId: type_id,
                                    noteId: note_id,
                                    value: v.clamp(0.0, 1.0),
                                };
                            })
                    }
                },
                PluginEventKind::Param { id, value } => {
                    let n = values::normalized(&self.steps, id, value);
                    // A full ring only means the controller shows an older value for a while.
                    let _ = self.mirror.push((id, n));
                    self.changes_in.add_point(id, at, n)
                }
            };
            if !ok {
                dropped += 1;
            }
        }
        io.dropped = io.dropped.saturating_add(dropped);
    }

    fn fill_context(&mut self, io: &PluginIo<'_>) {
        let c = values::context_values(&io.transport);
        self.context.state = c.state;
        self.context.sampleRate = c.sample_rate;
        self.context.projectTimeSamples = c.project_time_samples;
        self.context.systemTime = 0;
        self.context.continousTimeSamples = c.continuous_time_samples;
        self.context.projectTimeMusic = c.project_time_music;
        self.context.barPositionMusic = c.bar_position_music;
        self.context.cycleStartMusic = c.cycle_start_music;
        self.context.cycleEndMusic = c.cycle_end_music;
        self.context.tempo = c.tempo;
        self.context.timeSigNumerator = c.time_sig_numerator;
        self.context.timeSigDenominator = c.time_sig_denominator;
    }

    fn collect_outputs(&mut self, io: &mut PluginIo<'_>) {
        // The main output: a mono plugin's channel is copied by the device to both outputs.
        if let Some(slot) = self.out_slots.iter().find(|s| s.role == Role::Main) {
            for (c, out) in io.main_out.iter_mut().enumerate() {
                if c < slot.channels {
                    *out = self.out_data[slot.first + c];
                }
            }
        }
        // Notes out.
        for i in 0..self.events_out.len() {
            let Some(e) = self.events_out.get(i) else {
                continue;
            };
            let offset = e.sampleOffset.clamp(0, BATCH as i32 - 1) as u16;
            let t = u32::from(e.r#type);
            if t == Event_::EventTypes_::kNoteOnEvent as u32 {
                // SAFETY: the type tag says which union member the plugin filled.
                let n = unsafe { e.__field0.noteOn };
                io.events_out.push(
                    offset,
                    PluginEventKind::NoteOn {
                        key: n.pitch.clamp(0, 127) as u8,
                        velocity: n.velocity,
                        note_id: n.noteId,
                    },
                );
            } else if t == Event_::EventTypes_::kNoteOffEvent as u32 {
                // SAFETY: as above.
                let n = unsafe { e.__field0.noteOff };
                io.events_out.push(
                    offset,
                    PluginEventKind::NoteOff {
                        key: n.pitch.clamp(0, 127) as u8,
                        note_id: n.noteId,
                    },
                );
            }
        }
        // Params the plugin changed on its own go to the app like editor edits.
        for i in 0..self.changes_out.used() {
            if let Some((id, _, v)) = self.changes_out.last_of(i) {
                self.hub.messenger.post(PluginMessage::ParamEdit {
                    id,
                    value: values::host_value(&self.steps, id, v),
                    phase: ParamPhase::Change,
                });
            }
        }
    }
}

impl PluginProcessor for Vst3Processor {
    fn start_processing(&mut self) -> bool {
        // SAFETY: a live processor; the audio thread calls `setProcessing`.
        let r = unsafe { self.processor.setProcessing(1) };
        let started = r == kResultOk || r == kNotImplemented;
        self.running = started;
        started
    }

    fn stop_processing(&mut self) {
        if self.running {
            // SAFETY: a live processor.
            unsafe { self.processor.setProcessing(0) };
            self.running = false;
        }
    }

    fn reset(&mut self) {
        if self.running {
            // SAFETY: a live processor; off then on is how VST3 clears the audio state.
            unsafe {
                self.processor.setProcessing(0);
                self.processor.setProcessing(1);
            }
        }
    }

    fn process(&mut self, io: &mut PluginIo<'_>) -> ProcessStatus {
        self.fill_inputs(io);
        self.build_events(io);
        self.fill_context(io);

        let changes_in = self
            .changes_in
            .as_com_ref::<IParameterChanges>()
            .map_or(std::ptr::null_mut(), |r| r.as_ptr());
        let changes_out = self
            .changes_out
            .as_com_ref::<IParameterChanges>()
            .map_or(std::ptr::null_mut(), |r| r.as_ptr());
        let events_in = self
            .events_in
            .as_com_ref::<IEventList>()
            .map_or(std::ptr::null_mut(), |r| r.as_ptr());
        let events_out = self
            .events_out
            .as_com_ref::<IEventList>()
            .map_or(std::ptr::null_mut(), |r| r.as_ptr());
        let mut data = ProcessData {
            processMode: self.mode,
            symbolicSampleSize: SymbolicSampleSizes_::kSample32 as i32,
            numSamples: BATCH as i32,
            numInputs: self.in_buses.len() as i32,
            numOutputs: self.out_buses.len() as i32,
            inputs: self.in_buses.as_mut_ptr(),
            outputs: self.out_buses.as_mut_ptr(),
            inputParameterChanges: changes_in,
            outputParameterChanges: changes_out,
            inputEvents: events_in,
            outputEvents: events_out,
            processContext: &mut self.context,
        };
        // SAFETY: `data` describes buffers and lists this struct owns: every channel pointer of
        // `in_buses` and `out_buses` addresses a live `[f32; BATCH]` of `in_data` or `out_data`,
        // the frame count is BATCH, and `&mut self` makes the output buffers exclusive.
        let result = unsafe { self.processor.process(&mut data) };

        self.collect_outputs(io);
        // SAFETY: a live processor.
        self.tail = match unsafe { self.processor.getTailSamples() } {
            n if n == kInfiniteTail => TailLength::Infinite,
            n => TailLength::Finite(n),
        };
        if result == kResultOk {
            ProcessStatus::Continue
        } else {
            ProcessStatus::Error
        }
    }

    fn sleep_rule(&self) -> SleepRule {
        SleepRule::ZeroTail
    }

    fn tail_samples(&self) -> TailLength {
        self.tail
    }
}

impl Drop for Vst3Processor {
    fn drop(&mut self) {
        self.stop_processing();
    }
}
