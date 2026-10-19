//! `ClapProcessor`: the audio half of a CLAP plugin. Everything it
//! needs is allocated at activation; `process` copies the host's block into the plugin's
//! buffers, turns [`PluginEvent`]s into CLAP events, calls the plugin and copies the result
//! back. The `unsafe` here builds the raw audio buffer descriptors (so the `constant_mask` of a
//! silent input is exact for every channel) and nothing else.

use crate::hub::{ClapHost, Hub};
use crate::layout::{AudioPlan, InRole, NotePlan};
use atomik_core::{BATCH, Batch, ExprKind};
use atomik_plugin_host::{
    ParamPhase, PluginEvent, PluginEventKind, PluginIo, PluginMessage, PluginProcessor,
    PluginTransport, ProcessStatus, SleepRule, TailLength,
};
use clack_extensions::tail::{PluginTail, TailLength as ClapTail};
use clack_host::events::event_types::{
    MidiEvent, NoteExpressionEvent, NoteExpressionType, NoteOffEvent, NoteOnEvent, ParamValueEvent,
    TransportEvent, TransportFlags,
};
use clack_host::events::io::{OutputEventBuffer, TryPushError};
use clack_host::events::spaces::CoreEventSpace;
use clack_host::events::{EventFlags, EventHeader, Match};
use clack_host::prelude::*;
use clack_host::process::PluginAudioProcessor;
use clack_host::utils::{BeatTime, SecondsTime};
use clap_sys::audio_buffer::clap_audio_buffer;
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// Capacity of the events a plugin may send out per block.
const OUT_CAPACITY: usize = 512;
/// Capacity of the events sent to the plugin per block (a full note buffer, the Offs of an
/// all-notes-off and a param change for every param).
const IN_CAPACITY: usize = 4_096 + 2_048 + 512;

/// Collects the events a plugin sends out into preallocated lists; refuses (and counts) what
/// does not fit.
struct OutSink {
    notes: Vec<PluginEvent>,
    edits: Vec<(u32, f64, ParamPhase)>,
    dropped: u32,
}

impl OutputEventBuffer for OutSink {
    fn try_push(&mut self, event: &UnknownEvent) -> Result<(), TryPushError> {
        let offset = event.header().time().min(BATCH as u32 - 1) as u16;
        let ok = match event.as_core_event() {
            Some(CoreEventSpace::NoteOn(e)) => push_note(
                &mut self.notes,
                offset,
                PluginEventKind::NoteOn {
                    key: key_of(e.key()),
                    velocity: e.velocity() as f32,
                    note_id: e.note_id().into_specific().map_or(-1, |id| id as i32),
                },
            ),
            Some(CoreEventSpace::NoteOff(e)) => push_note(
                &mut self.notes,
                offset,
                PluginEventKind::NoteOff {
                    key: key_of(e.key()),
                    note_id: e.note_id().into_specific().map_or(-1, |id| id as i32),
                },
            ),
            Some(CoreEventSpace::ParamValue(e)) => match e.param_id() {
                Some(id) if self.edits.len() < self.edits.capacity() => {
                    self.edits.push((id.get(), e.value(), ParamPhase::Change));
                    true
                }
                _ => false,
            },
            Some(CoreEventSpace::ParamGestureBegin(e)) => {
                push_gesture(self, e.param_id(), ParamPhase::Begin)
            }
            Some(CoreEventSpace::ParamGestureEnd(e)) => {
                push_gesture(self, e.param_id(), ParamPhase::End)
            }
            // Everything else a plugin may send out (expressions, MIDI, transport requests) is
            // accepted and ignored: the rack has nowhere to put it.
            _ => true,
        };
        if ok {
            Ok(())
        } else {
            self.dropped = self.dropped.saturating_add(1);
            Err(TryPushError::new())
        }
    }
}

fn key_of(key: Match<u16>) -> u8 {
    key.into_specific().map_or(0, |k| k.min(127) as u8)
}

fn push_note(list: &mut Vec<PluginEvent>, offset: u16, kind: PluginEventKind) -> bool {
    if list.len() >= list.capacity() {
        return false;
    }
    list.push(PluginEvent { offset, kind });
    true
}

fn push_midi(events: &mut EventBuffer, allowed: bool, time: u32, data: [u8; 3]) -> bool {
    if allowed {
        events.push(&MidiEvent::new(time, 0, data));
    }
    allowed
}

fn push_gesture(sink: &mut OutSink, id: Option<ClapId>, phase: ParamPhase) -> bool {
    match id {
        Some(id) if sink.edits.len() < sink.edits.capacity() => {
            // A gesture marker carries no value.
            sink.edits.push((id.get(), f64::NAN, phase));
            true
        }
        _ => false,
    }
}

/// The audio half of a CLAP instance.
pub(crate) struct ClapProcessor {
    processor: PluginAudioProcessor<ClapHost>,
    hub: Arc<Hub>,
    audio: AudioPlan,
    notes: NotePlan,
    tail_ext: Option<PluginTail>,
    tail: TailLength,
    // Channel storage and the raw descriptors that point into it.
    in_data: Vec<Batch>,
    out_data: Vec<Batch>,
    in_ptrs: Vec<*mut f32>,
    out_ptrs: Vec<*mut f32>,
    in_bufs: Vec<clap_audio_buffer>,
    out_bufs: Vec<clap_audio_buffer>,
    in_events: EventBuffer,
    out_sink: OutSink,
}

// SAFETY: the raw pointers point into the heap buffers this struct owns (`in_data`,
// `out_data`, `in_ptrs`, `out_ptrs`), are rewritten before every `process` call and are only
// dereferenced by the plugin during that call on the thread that owns the processor. Every
// other field is `Send`.
unsafe impl Send for ClapProcessor {}

fn buffers(
    plan: &[crate::layout::PortPlan],
    data: &mut Vec<Batch>,
    ptrs: &mut Vec<*mut f32>,
) -> Vec<clap_audio_buffer> {
    let total: usize = plan.iter().map(|p| p.channels as usize).sum();
    data.clear();
    data.resize(total, [0.0; BATCH]);
    ptrs.clear();
    ptrs.extend(data.iter_mut().map(|b| b.as_mut_ptr()));
    let mut out = Vec::with_capacity(plan.len());
    let mut first = 0usize;
    for p in plan {
        out.push(clap_audio_buffer {
            data32: if p.channels == 0 {
                std::ptr::null_mut()
            } else {
                // The pointer table is stable (never resized after this) and `first` is in range.
                ptrs[first..].as_mut_ptr()
            },
            data64: std::ptr::null_mut(),
            channel_count: p.channels,
            latency: 0,
            constant_mask: 0,
        });
        first += p.channels as usize;
    }
    out
}

impl ClapProcessor {
    pub fn new(
        processor: PluginAudioProcessor<ClapHost>,
        hub: Arc<Hub>,
        audio: AudioPlan,
        notes: NotePlan,
        tail_ext: Option<PluginTail>,
    ) -> Self {
        let mut in_data = Vec::new();
        let mut out_data = Vec::new();
        let mut in_ptrs = Vec::new();
        let mut out_ptrs = Vec::new();
        let in_bufs = buffers(&audio.inputs, &mut in_data, &mut in_ptrs);
        let out_bufs = buffers(&audio.outputs, &mut out_data, &mut out_ptrs);
        Self {
            processor,
            hub,
            audio,
            notes,
            tail_ext,
            tail: TailLength::Finite(0),
            in_data,
            out_data,
            in_ptrs,
            out_ptrs,
            in_bufs,
            out_bufs,
            in_events: EventBuffer::with_capacity(IN_CAPACITY),
            out_sink: OutSink {
                notes: Vec::with_capacity(OUT_CAPACITY),
                edits: Vec::with_capacity(OUT_CAPACITY),
                dropped: 0,
            },
        }
    }

    fn fill_inputs(&mut self, io: &PluginIo<'_>) {
        let mut first = 0usize;
        for (port, buf) in self.audio.inputs.iter().zip(self.in_bufs.iter_mut()) {
            let n = port.channels as usize;
            let (source, silent): (&[Batch], bool) = match port.role {
                InRole::Main => (io.main_in, io.main_in_silent),
                InRole::Sidechain => (io.sidechain, io.sidechain_silent),
                InRole::Unused => (&[], true),
            };
            for c in 0..n {
                self.in_data[first + c] = source
                    .get(c)
                    .or_else(|| source.first())
                    .copied()
                    .unwrap_or([0.0; BATCH]);
            }
            // Every channel of a silent port is constant (CLAP: bit per channel).
            buf.constant_mask = if silent { (1u64 << n.min(63)) - 1 } else { 0 };
            first += n;
        }
        self.out_data.fill([0.0; BATCH]);
    }

    fn build_events(&mut self, io: &mut PluginIo<'_>) {
        self.in_events.clear();
        let clap_notes = self.notes.clap_dialect();
        let midi = self.notes.midi();
        let mut dropped = 0u32;
        for e in io.events_in {
            let t = u32::from(e.offset);
            let unit = |v: f32| (v.clamp(0.0, 1.0) * 127.0).round() as u8;
            let ok = match e.kind {
                PluginEventKind::NoteOn {
                    key,
                    velocity,
                    note_id,
                } => {
                    if clap_notes {
                        self.in_events.push(&NoteOnEvent::new(
                            t,
                            Pckn::new(0u16, 0u16, u16::from(key), note_id as u32),
                            f64::from(velocity),
                        ));
                        true
                    } else {
                        let v = unit(velocity).max(1);
                        push_midi(&mut self.in_events, midi, t, [0x90, key & 0x7F, v])
                    }
                }
                PluginEventKind::NoteOff { key, note_id } => {
                    if clap_notes {
                        self.in_events.push(&NoteOffEvent::new(
                            t,
                            Pckn::new(0u16, 0u16, u16::from(key), note_id as u32),
                            0.0,
                        ));
                        true
                    } else {
                        push_midi(&mut self.in_events, midi, t, [0x80, key & 0x7F, 0])
                    }
                }
                PluginEventKind::PitchBend { value } => {
                    let v14 = ((value.clamp(-1.0, 1.0) + 1.0) * 8192.0)
                        .round()
                        .clamp(0.0, 16_383.0) as u16;
                    push_midi(
                        &mut self.in_events,
                        midi,
                        t,
                        [0xE0, (v14 & 0x7F) as u8, (v14 >> 7) as u8],
                    )
                }
                PluginEventKind::Cc { controller, value } => push_midi(
                    &mut self.in_events,
                    midi,
                    t,
                    [0xB0, controller & 0x7F, unit(value)],
                ),
                PluginEventKind::ChannelPressure { value } => {
                    push_midi(&mut self.in_events, midi, t, [0xD0, unit(value), 0])
                }
                PluginEventKind::PolyPressure { key, value, .. } => push_midi(
                    &mut self.in_events,
                    midi,
                    t,
                    [0xA0, key & 0x7F, unit(value)],
                ),
                PluginEventKind::NoteExpr {
                    key,
                    note_id,
                    kind,
                    value,
                } => {
                    if clap_notes {
                        let (kind, value) = match kind {
                            ExprKind::Tuning => (NoteExpressionType::Tuning, f64::from(value)),
                            ExprKind::Pressure => (NoteExpressionType::Pressure, f64::from(value)),
                            ExprKind::Slide => (NoteExpressionType::Brightness, f64::from(value)),
                            ExprKind::Volume => (NoteExpressionType::Volume, f64::from(value)),
                            ExprKind::Pan => {
                                (NoteExpressionType::Pan, f64::from((value + 1.0) * 0.5))
                            }
                        };
                        self.in_events.push(&NoteExpressionEvent::new(
                            t,
                            Pckn::new(0u16, 0u16, u16::from(key), note_id as u32),
                            kind,
                            value,
                        ));
                        true
                    } else {
                        false
                    }
                }
                PluginEventKind::Param { id, value } => match ClapId::from_raw(id) {
                    Some(id) => {
                        self.in_events
                            .push(&ParamValueEvent::new(t, id, Pckn::match_all(), value));
                        true
                    }
                    None => false,
                },
            };
            if !ok {
                dropped += 1;
            }
        }
        io.dropped = io.dropped.saturating_add(dropped);
    }
}

fn transport_event(t: &PluginTransport) -> TransportEvent {
    let per_bar = f64::from(t.time_signature.0) * 4.0 / f64::from(t.time_signature.1.max(1));
    let bar = if per_bar > 0.0 {
        (t.song_pos_beats / per_bar).floor()
    } else {
        0.0
    };
    let secs = |beats: f64| beats * 60.0 / t.tempo_bpm;
    let mut flags = TransportFlags::HAS_TEMPO
        | TransportFlags::HAS_BEATS_TIMELINE
        | TransportFlags::HAS_SECONDS_TIMELINE
        | TransportFlags::HAS_TIME_SIGNATURE;
    if t.playing {
        flags |= TransportFlags::IS_PLAYING;
    }
    if t.loop_active {
        flags |= TransportFlags::IS_LOOP_ACTIVE;
    }
    TransportEvent {
        header: EventHeader::new_core(0, EventFlags::empty()),
        flags,
        song_pos_beats: BeatTime::from_float(t.song_pos_beats),
        song_pos_seconds: SecondsTime::from_float(t.song_pos_seconds),
        tempo: t.tempo_bpm,
        tempo_inc: 0.0,
        loop_start_beats: BeatTime::from_float(t.loop_start_beats),
        loop_end_beats: BeatTime::from_float(t.loop_end_beats),
        loop_start_seconds: SecondsTime::from_float(secs(t.loop_start_beats)),
        loop_end_seconds: SecondsTime::from_float(secs(t.loop_end_beats)),
        bar_start: BeatTime::from_float(bar * per_bar),
        bar_number: bar as i32,
        time_signature_numerator: u16::from(t.time_signature.0),
        time_signature_denominator: u16::from(t.time_signature.1),
    }
}

impl PluginProcessor for ClapProcessor {
    fn start_processing(&mut self) -> bool {
        self.processor.ensure_processing_started().is_ok()
    }

    fn stop_processing(&mut self) {
        self.processor.ensure_processing_stopped();
    }

    fn reset(&mut self) {
        self.processor.reset();
    }

    fn process(&mut self, io: &mut PluginIo<'_>) -> ProcessStatus {
        self.fill_inputs(io);
        self.build_events(io);
        self.out_sink.notes.clear();
        self.out_sink.edits.clear();
        let transport = transport_event(&io.transport);
        let steady = io.transport.steady_frames;

        let result = {
            let in_events = InputEvents::from_buffer(&self.in_events);
            let mut out_events = OutputEvents::from_buffer(&mut self.out_sink);
            // SAFETY: `in_bufs` describes `in_data` through `in_ptrs`: every channel pointer
            // addresses a live `[f32; BATCH]` of this struct, and the frame count is BATCH.
            let inputs =
                unsafe { InputAudioBuffers::from_raw_buffers(&self.in_bufs, BATCH as u32) };
            // SAFETY: as above for `out_bufs`, `out_data` and `out_ptrs`; `&mut self` makes the
            // output buffers exclusive for the call.
            let mut outputs =
                unsafe { OutputAudioBuffers::from_raw_buffers(&mut self.out_bufs, BATCH as u32) };
            match self.processor.as_started_mut() {
                Ok(started) => started.process(
                    &inputs,
                    &mut outputs,
                    &in_events,
                    &mut out_events,
                    Some(steady),
                    Some(&transport),
                ),
                Err(e) => Err(e),
            }
        };

        // What the plugin sent out: notes to the device, GUI edits to the app.
        for e in &self.out_sink.notes {
            io.events_out.push(e.offset, e.kind);
        }
        for &(id, value, phase) in &self.out_sink.edits {
            self.hub
                .messenger
                .post(PluginMessage::ParamEdit { id, value, phase });
        }
        io.dropped = io
            .dropped
            .saturating_add(std::mem::take(&mut self.out_sink.dropped));

        // The main output: a mono plugin's channel is copied by the device to both outputs.
        if let Some(port) = self
            .audio
            .outputs
            .iter()
            .position(|p| p.role == InRole::Main)
        {
            let first: usize = self.audio.outputs[..port]
                .iter()
                .map(|p| p.channels as usize)
                .sum();
            for (c, out) in io.main_out.iter_mut().enumerate() {
                let n = self.audio.outputs[port].channels as usize;
                if c < n {
                    *out = self.out_data[first + c];
                }
            }
        }

        if let Some(ext) = self.tail_ext {
            let handle = self.processor.plugin_handle();
            self.tail = match ext.get(&handle) {
                ClapTail::Finite(n) => TailLength::Finite(n),
                ClapTail::Infinite => TailLength::Infinite,
            };
        }
        let _ = self.in_ptrs.len() + self.out_ptrs.len();
        match result {
            Ok(clack_host::process::ProcessStatus::Continue) => ProcessStatus::Continue,
            Ok(clack_host::process::ProcessStatus::ContinueIfNotQuiet) => {
                ProcessStatus::ContinueIfNotQuiet
            }
            Ok(clack_host::process::ProcessStatus::Tail) => ProcessStatus::Tail,
            Ok(clack_host::process::ProcessStatus::Sleep) => ProcessStatus::Sleep,
            Err(_) => ProcessStatus::Error,
        }
    }

    fn sleep_rule(&self) -> SleepRule {
        SleepRule::PluginDecides
    }

    fn tail_samples(&self) -> TailLength {
        self.tail
    }

    fn wake_requested(&self) -> bool {
        self.hub.wake.swap(false, Ordering::AcqRel)
    }
}
