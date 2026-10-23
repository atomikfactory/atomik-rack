//! `PluginDevice`: the rack device of every plugin, for every format.
//!
//! The device is the engine-side half of a plugin instance. Each batch it
//!
//! 1. services the restart hand-shake (hands the processor back, or takes a new one),
//! 2. gathers the cabled inputs into preallocated buffers (stereo, or the mono mapping of
//!    the sockets) and builds the event list: coalesced param changes first, then the batch's notes
//!    as events with host note ids,
//! 3. applies the skipping rules (sleep, zero tail, power off),
//! 4. calls the processor with exactly 64 frames inside a panic boundary and a flush-to-zero
//!    guard, and fails the device on an error status, a panic or non-finite output (a sandboxed
//!    plugin may also answer `Late`: no wet signal this batch, counted; or `Lost`: its process
//!    is gone and the device is CRASHED for good),
//! 5. writes the outputs through the 5 ms bypass crossfade and forwards a note effect's notes.
//!
//! Nothing in `process` allocates, locks or blocks; everything it needs is sized in `new`.
//! What the plugin itself does inside `process` is its own business.

use crate::backend::{
    EventList, PluginEventKind, PluginIo, PluginProcessor, PluginTransport, ProcessStatus,
    SleepRule, TailLength,
};
use crate::fpenv::FtzGuard;
use crate::layout::{PluginSockets, layout_matches, param_ids};
use crate::manager::LOST;
use crate::slot::{
    DeviceEnd, DeviceState, HostMessenger, PluginMessage, PluginSlot, ReapItem, ReaperQueue,
};
use crate::threads::AudioThreadGuard;
use atomik_core::{
    BATCH, Batch, ExprKind, MAX_EVENTS_PER_BATCH, NoteEvent, NoteKind, PPQ, ParamId,
};
use atomik_device_sdk::{
    Channels, Device, DeviceDescriptor, DeviceKind, PluginLayout, PrepareInfo, ProcessContext,
    RunMode, SocketId,
};
use rtrb::{Consumer, Producer, PushError};
use std::panic::{AssertUnwindSafe, catch_unwind};

/// Number of meter values a plugin device publishes: input peak, output peak, state, latency
/// frames, dropped events, late batches (sandbox: batches whose result did not arrive in time).
pub const METER_VALUES: usize = 6;

/// Events the device can hold per batch: the notes of a full note buffer, the extra Offs of an
/// all-notes-off, and the param changes.
const EVENT_CAPACITY: usize = 4 * MAX_EVENTS_PER_BATCH + 256;

/// Length of the bypass crossfade.
const BYPASS_FADE_SECONDS: f64 = 0.005;

/// "No note open on this key."
const CLOSED: i32 = -1;

/// How long an instrument must have been silent, with no note open and nothing pending, before
/// a zero-tail plugin is put to sleep (seconds). A plugin that reports a zero tail while a
/// voice still sounds or releases (a commercial wavetable synth) must not be cut: its own output is the evidence.
const IDLE_SECONDS: f64 = 1.0;

/// An output block whose peak is at or below this is silent for the idle rule (-120 dB).
const IDLE_PEAK: f32 = 1.0e-6;

type ProcBox = Box<dyn PluginProcessor>;

const SILENCE: Batch = [0.0; BATCH];

/// The engine-side device of one plugin instance.
pub struct PluginDevice {
    descriptor: &'static DeviceDescriptor,
    sockets: PluginSockets,
    in_channels: usize,
    out_channels: usize,
    has_sidechain: bool,

    processor: Option<ProcBox>,
    started: bool,
    failed: bool,
    sleeping: bool,
    /// Consecutive processed blocks of an instrument with silent output and no event (the idle
    /// rule of [`SleepRule::ZeroTail`]); any event or sound resets it.
    quiet_batches: u32,
    /// Quiet blocks that make an instrument idle (`IDLE_SECONDS`).
    idle_batches: u32,
    state: DeviceState,
    slot: PluginSlot,
    messenger: HostMessenger,
    to_manager: Producer<ProcBox>,
    from_manager: Consumer<ProcBox>,
    reaper: Option<ReaperQueue>,

    // Params: index 0 is bypass, 1.. the plugin params in descriptor order.
    ids: Vec<u32>,
    values: Vec<f64>,
    ever_set: Vec<bool>,
    dirty: Vec<bool>,
    pending: Vec<u32>,
    bypass: bool,
    bypass_mix: f32,
    fade_step: f32,

    sample_rate: f64,
    activation_rate: f64,
    restart_posted: bool,
    run_mode: RunMode,

    note_ids: [i32; 128],
    next_note_id: i32,

    in_main: [Batch; 2],
    sidechain: [Batch; 2],
    wet: [Batch; 2],
    events_in: EventList,
    events_out: EventList,

    in_peak: f32,
    out_peak: f32,
    dropped: u64,
    /// Batches whose result did not arrive in time (sandbox).
    late: u64,
}

fn idle_batches_for(sample_rate: f64) -> u32 {
    (sample_rate * IDLE_SECONDS / BATCH as f64).ceil().max(1.0) as u32
}

fn fade_step_for(sample_rate: f64) -> f32 {
    let frames = (sample_rate * BYPASS_FADE_SECONDS).round().max(1.0);
    (1.0 / frames) as f32
}

fn channel_count(c: Option<Channels>) -> usize {
    match c {
        None => 0,
        Some(Channels::Mono) => 1,
        Some(Channels::Stereo) => 2,
    }
}

fn finite_or(v: f32, fallback: f32) -> f32 {
    if v.is_finite() { v } else { fallback }
}

fn peak(a: &Batch, b: &Batch) -> f32 {
    a.iter().chain(b).fold(0.0f32, |m, s| m.max(s.abs()))
}

/// Whether any sample is not (positive or negative) zero. NaN counts as a signal.
fn has_signal(b: &Batch) -> bool {
    b.iter().any(|s| s.to_bits() & 0x7FFF_FFFF != 0)
}

fn mix(wet: f32, dry: f32, m: f32) -> f32 {
    if m <= 0.0 {
        wet
    } else if m >= 1.0 {
        dry
    } else {
        wet * (1.0 - m) + dry * m
    }
}

fn connected(ctx: &ProcessContext<'_>, socket: Option<SocketId>) -> bool {
    socket.is_some_and(|s| ctx.is_connected(s))
}

fn input_copy(ctx: &ProcessContext<'_>, socket: Option<SocketId>) -> Batch {
    socket.map_or(SILENCE, |s| *ctx.input(s))
}

impl PluginDevice {
    /// Builds the device of a plugin instance.
    ///
    /// `descriptor` is the interned descriptor of the plugin's layout, `processor` the activated
    /// audio half, `end` the engine side of the instance's channel and `activation_rate` the
    /// sample rate the processor was activated at (a different `prepare` rate asks the manager
    /// for a restart). Non-RT.
    pub fn new(
        descriptor: &'static DeviceDescriptor,
        layout: &PluginLayout,
        processor: Box<dyn PluginProcessor>,
        end: DeviceEnd,
        activation_rate: f64,
    ) -> Self {
        debug_assert!(
            layout_matches(layout, descriptor),
            "the descriptor was not built from this layout"
        );
        let ids = param_ids(descriptor);
        let n = ids.len() + 1;
        let DeviceEnd {
            slot,
            to_manager,
            from_manager,
        } = end;
        Self {
            descriptor,
            sockets: PluginSockets::of(descriptor),
            in_channels: channel_count(layout.main_in),
            out_channels: channel_count(layout.main_out),
            has_sidechain: layout.sidechain,
            processor: Some(processor),
            started: false,
            failed: false,
            sleeping: false,
            quiet_batches: 0,
            idle_batches: idle_batches_for(activation_rate),
            state: DeviceState::Running,
            messenger: slot.messenger(),
            slot,
            to_manager,
            from_manager,
            reaper: None,
            ids,
            values: vec![0.0; n],
            ever_set: vec![false; n],
            dirty: vec![false; n],
            pending: Vec::with_capacity(n),
            bypass: false,
            bypass_mix: 0.0,
            fade_step: fade_step_for(activation_rate),
            sample_rate: activation_rate,
            activation_rate,
            restart_posted: false,
            run_mode: RunMode::Realtime,
            note_ids: [CLOSED; 128],
            next_note_id: 0,
            in_main: [SILENCE; 2],
            sidechain: [SILENCE; 2],
            wet: [SILENCE; 2],
            events_in: EventList::with_capacity(EVENT_CAPACITY + n),
            events_out: EventList::with_capacity(MAX_EVENTS_PER_BATCH),
            in_peak: 0.0,
            out_peak: 0.0,
            dropped: 0,
            late: 0,
        }
    }

    /// Makes a dropped device leave its processor with the manager's reaper instead of
    /// destroying it on the dropping thread.
    #[must_use]
    pub fn with_reaper(mut self, reaper: ReaperQueue) -> Self {
        self.reaper = Some(reaper);
        self
    }

    /// The run mode set by [`Device::set_run_mode`].
    pub fn run_mode(&self) -> RunMode {
        self.run_mode
    }

    fn mark_dirty(&mut self, index: usize) {
        if !self.dirty[index] {
            self.dirty[index] = true;
            self.pending.push(index as u32);
        }
    }

    fn fail(&mut self, reason: &'static str) {
        self.failed = true;
        self.state = DeviceState::Failed;
        self.messenger.post(PluginMessage::Failed { reason });
    }

    /// The plugin's process is gone: CRASHED for good (a new instance replaces it).
    fn fail_lost(&mut self) {
        self.failed = true;
        self.state = DeviceState::Crashed;
        self.messenger.post(PluginMessage::Failed { reason: LOST });
    }

    fn close_all_notes(&mut self) {
        self.note_ids = [CLOSED; 128];
    }

    // ----- the restart hand-shake ------------------------------------------------------------

    fn service_handshake(&mut self) {
        if self.processor.is_some() {
            if !self.slot.stop_requested() {
                return;
            }
            let Some(mut processor) = self.processor.take() else {
                return;
            };
            if self.started {
                processor.stop_processing();
                self.started = false;
            }
            match self.to_manager.push(processor) {
                Ok(()) => {
                    self.failed = false;
                    self.sleeping = false;
                    self.state = DeviceState::Restarting;
                    self.close_all_notes();
                }
                // The ring cannot be full in the protocol; if it is, try again next batch.
                Err(PushError::Full(p)) => self.processor = Some(p),
            }
        } else if let Ok(processor) = self.from_manager.pop() {
            self.processor = Some(processor);
            self.started = false;
            self.failed = false;
            self.sleeping = false;
            self.quiet_batches = 0;
            self.state = DeviceState::Running;
            self.close_all_notes();
            // A restarted plugin has restored its state: every param the rack ever set goes
            // out again, so the rack-side values win.
            for i in 1..self.values.len() {
                if self.ever_set[i] {
                    self.mark_dirty(i);
                }
            }
        }
    }

    // ----- events -------------------------------------------------------------------------------

    fn build_events(&mut self, ctx: &ProcessContext<'_>, hold_new_notes: bool) {
        self.events_in.clear();
        for k in 0..self.pending.len() {
            let i = self.pending[k] as usize;
            self.dirty[i] = false;
            let kind = PluginEventKind::Param {
                id: self.ids[i - 1],
                value: self.values[i],
            };
            self.events_in.push(0, kind);
        }
        self.pending.clear();
        if let Some(socket) = self.sockets.notes_in {
            for event in ctx.notes_in(socket) {
                self.translate(*event, hold_new_notes);
            }
        }
        self.dropped += u64::from(self.events_in.take_dropped());
    }

    /// The events of a powered-off device: the ends of open notes only (see `run_plugin`).
    fn build_stopped_events(&mut self, ctx: &ProcessContext<'_>) {
        self.events_in.clear();
        if let Some(socket) = self.sockets.notes_in {
            for event in ctx.notes_in(socket) {
                match event.kind {
                    NoteKind::Off { .. } => self.translate(*event, true),
                    NoteKind::AllOff if self.note_ids.iter().any(|id| *id != CLOSED) => {
                        self.translate(*event, true);
                    }
                    _ => {}
                }
            }
        }
        self.dropped += u64::from(self.events_in.take_dropped());
    }

    fn translate(&mut self, event: NoteEvent, hold_new_notes: bool) {
        let offset = event.offset;
        match event.kind {
            NoteKind::On { key, velocity } => {
                if hold_new_notes {
                    return;
                }
                let key = key & 0x7F;
                let old = self.note_ids[usize::from(key)];
                if old != CLOSED {
                    self.events_in
                        .push(offset, PluginEventKind::NoteOff { key, note_id: old });
                }
                let id = self.next_note_id;
                self.next_note_id = (id + 1) & 0x7FFF_FFFF;
                self.note_ids[usize::from(key)] = id;
                let velocity = if velocity.is_nan() {
                    1.0
                } else {
                    velocity.clamp(1.0 / 127.0, 1.0)
                };
                self.events_in.push(
                    offset,
                    PluginEventKind::NoteOn {
                        key,
                        velocity,
                        note_id: id,
                    },
                );
            }
            NoteKind::Off { key } => {
                let key = key & 0x7F;
                let id = self.note_ids[usize::from(key)];
                if id != CLOSED {
                    self.note_ids[usize::from(key)] = CLOSED;
                    self.events_in
                        .push(offset, PluginEventKind::NoteOff { key, note_id: id });
                }
            }
            NoteKind::AllOff => {
                for key in 0..128u8 {
                    let id = self.note_ids[usize::from(key)];
                    if id != CLOSED {
                        self.note_ids[usize::from(key)] = CLOSED;
                        self.events_in
                            .push(offset, PluginEventKind::NoteOff { key, note_id: id });
                    }
                }
                self.events_in.push(
                    offset,
                    PluginEventKind::Cc {
                        controller: 123,
                        value: 0.0,
                    },
                );
            }
            NoteKind::PitchBend { value } => {
                self.events_in.push(
                    offset,
                    PluginEventKind::PitchBend {
                        value: finite_or(value, 0.0).clamp(-1.0, 1.0),
                    },
                );
            }
            NoteKind::Cc { controller, value } => {
                self.events_in.push(
                    offset,
                    PluginEventKind::Cc {
                        controller: controller & 0x7F,
                        value: finite_or(value, 0.0).clamp(0.0, 1.0),
                    },
                );
            }
            NoteKind::ChannelPressure { value } => {
                self.events_in.push(
                    offset,
                    PluginEventKind::ChannelPressure {
                        value: finite_or(value, 0.0).clamp(0.0, 1.0),
                    },
                );
            }
            NoteKind::PolyPressure { key, value } => {
                let key = key & 0x7F;
                let id = self.note_ids[usize::from(key)];
                if id != CLOSED {
                    self.events_in.push(
                        offset,
                        PluginEventKind::PolyPressure {
                            key,
                            note_id: id,
                            value: finite_or(value, 0.0).clamp(0.0, 1.0),
                        },
                    );
                }
            }
            NoteKind::NoteExpr { key, kind, value } => {
                let key = key & 0x7F;
                let id = self.note_ids[usize::from(key)];
                if id != CLOSED {
                    let v = finite_or(value, 0.0);
                    let value = match kind {
                        ExprKind::Tuning => v.clamp(-120.0, 120.0),
                        ExprKind::Pan => v.clamp(-1.0, 1.0),
                        ExprKind::Pressure | ExprKind::Slide | ExprKind::Volume => {
                            v.clamp(0.0, 1.0)
                        }
                    };
                    self.events_in.push(
                        offset,
                        PluginEventKind::NoteExpr {
                            key,
                            note_id: id,
                            kind,
                            value,
                        },
                    );
                }
            }
        }
    }

    fn forward_notes(&mut self, ctx: &mut ProcessContext<'_>) {
        let Some(socket) = self.sockets.notes_out else {
            return;
        };
        let mut last = 0u16;
        let mut dropped = 0u64;
        for e in self.events_out.as_slice() {
            let kind = match e.kind {
                PluginEventKind::NoteOn { key, velocity, .. } => NoteKind::On {
                    key: key & 0x7F,
                    velocity: finite_or(velocity, 1.0).clamp(1.0 / 127.0, 1.0),
                },
                PluginEventKind::NoteOff { key, .. } => NoteKind::Off { key: key & 0x7F },
                // Expressions, messages and params going out are dropped.
                _ => continue,
            };
            let offset = e.offset.max(last);
            last = offset;
            if ctx
                .notes_out(socket)
                .push(NoteEvent { offset, kind })
                .is_err()
            {
                dropped += 1;
            }
        }
        self.dropped += dropped;
    }

    // ----- inputs and transport ---------------------------------------------------------------

    fn gather_inputs(&mut self, ctx: &ProcessContext<'_>, raw: &[Batch; 2]) {
        match self.in_channels {
            0 => {}
            2 => self.in_main = *raw,
            _ => {
                let (left, right) = (
                    connected(ctx, self.sockets.in_l),
                    connected(ctx, self.sockets.in_r),
                );
                if left && right {
                    for ((m, a), b) in self.in_main[0].iter_mut().zip(&raw[0]).zip(&raw[1]) {
                        *m = (a + b) * 0.5;
                    }
                } else if right {
                    self.in_main[0] = raw[1];
                } else {
                    self.in_main[0] = raw[0];
                }
            }
        }
        if self.has_sidechain {
            self.sidechain = [
                input_copy(ctx, self.sockets.sc_l),
                input_copy(ctx, self.sockets.sc_r),
            ];
        }
    }

    fn inputs_have_signal(&self) -> bool {
        self.in_main[..self.in_channels].iter().any(has_signal)
            || (self.has_sidechain && self.sidechain.iter().any(has_signal))
    }

    fn transport(&self, ctx: &ProcessContext<'_>) -> PluginTransport {
        let t = ctx.time;
        let beats = t.tick / PPQ as f64;
        let bpm = t.tempo.bpm();
        let (loop_active, loop_start, loop_end) = match t.loop_ticks {
            Some((a, b)) => (true, a / PPQ as f64, b / PPQ as f64),
            None => (false, 0.0, 0.0),
        };
        PluginTransport {
            playing: t.playing,
            tempo_bpm: bpm,
            song_pos_beats: beats,
            song_pos_seconds: beats * 60.0 / bpm,
            steady_frames: t.frame,
            time_signature: t.time_signature,
            loop_active,
            loop_start_beats: loop_start,
            loop_end_beats: loop_end,
            sample_rate: self.sample_rate,
        }
    }

    // ----- one batch --------------------------------------------------------------------------

    /// Runs the plugin (or skips it by the skipping rules). Returns true when `self.wet` holds
    /// this batch's wet output (silence while skipped), false when there is none: the device
    /// failed, or the result was late.
    fn run_plugin(
        &mut self,
        ctx: &mut ProcessContext<'_>,
        raw: &[Batch; 2],
        bypassed: bool,
        powered: bool,
    ) -> bool {
        self.gather_inputs(ctx, raw);
        self.wet = [SILENCE; 2];
        let stopped = !powered && self.bypass_mix >= 1.0;
        if stopped {
            // Powered off and fully bypassed: only the end of a note is delivered, so a held
            // note cannot hang inside the plugin; params stay pending, new notes
            // are held back, and without such an event the plugin is not called at all.
            self.state = DeviceState::Stopped;
            self.build_stopped_events(ctx);
            if self.events_in.is_empty() {
                return true;
            }
        } else {
            let hold_new_notes = bypassed && self.bypass_mix >= 1.0;
            self.build_events(ctx, hold_new_notes);
        }
        let (rule, zero_tail, requested) = match self.processor.as_ref() {
            Some(p) => (
                p.sleep_rule(),
                p.tail_samples() == TailLength::Finite(0),
                p.wake_requested(),
            ),
            None => return false,
        };
        let watch_input = self.sleeping || rule == SleepRule::ZeroTail;
        let wake =
            !self.events_in.is_empty() || requested || (watch_input && self.inputs_have_signal());
        // A device with a note open is never idle (a note effect holds the state of its open
        // notes, an instrument may report a zero tail and still sound a held note). An
        // instrument (a plugin of kind Instrument, which may still have an audio input) is also
        // not idle until its own output has been silent for a while: its release sounds after
        // the note ended. Only batch content and the plugin's deterministic output decide, so
        // the live path and the bounce agree.
        let instrument =
            self.descriptor.kind == DeviceKind::Instrument && self.sockets.notes_in.is_some();
        let busy = self.note_ids.iter().any(|id| *id != CLOSED)
            || (instrument && self.quiet_batches < self.idle_batches);
        let skip = match rule {
            SleepRule::PluginDecides => self.sleeping && !wake,
            SleepRule::ZeroTail => zero_tail && !wake && !busy,
        };
        if skip {
            self.state = DeviceState::Sleeping;
            if instrument
                && rule == SleepRule::ZeroTail
                && self.started
                && let Some(p) = self.processor.as_mut()
            {
                // Really asleep: the plugin is told (VST3 `setProcessing(false)`) and is started
                // again before the block that wakes it. Only instruments, which are idle by
                // their own silence; an effect or a note effect that merely has nothing to do
                // keeps its state and is just not called.
                p.stop_processing();
                self.started = false;
            }
            return true;
        }
        if wake {
            self.sleeping = false;
        }

        let Some(mut processor) = self.processor.take() else {
            return false;
        };
        if !self.started {
            if !processor.start_processing() {
                self.processor = Some(processor);
                self.fail("the plugin could not start processing");
                return false;
            }
            self.started = true;
        }
        let transport = self.transport(ctx);
        self.events_out.clear();
        let main_connected = connected(ctx, self.sockets.in_l) || connected(ctx, self.sockets.in_r);
        let sidechain_connected =
            connected(ctx, self.sockets.sc_l) || connected(ctx, self.sockets.sc_r);
        let (result, reported_drops) = {
            let Self {
                in_main,
                sidechain,
                wet,
                events_in,
                events_out,
                in_channels,
                out_channels,
                has_sidechain,
                ..
            } = self;
            let mut io = PluginIo {
                frames: BATCH,
                main_in: &in_main[..*in_channels],
                main_in_silent: !(*in_channels > 0 && main_connected),
                sidechain: if *has_sidechain { &sidechain[..] } else { &[] },
                sidechain_silent: !sidechain_connected,
                main_out: &mut wet[..*out_channels],
                events_in: events_in.as_slice(),
                events_out,
                transport,
                dropped: 0,
            };
            let result = catch_unwind(AssertUnwindSafe(|| {
                let _flush = FtzGuard::enter();
                processor.process(&mut io)
            }));
            (result, io.dropped)
        };
        self.processor = Some(processor);
        self.dropped += u64::from(reported_drops) + u64::from(self.events_out.take_dropped());

        match result {
            Err(_) => {
                self.fail("the plugin panicked");
                false
            }
            Ok(ProcessStatus::Error) => {
                self.fail("the plugin returned an error status");
                false
            }
            Ok(ProcessStatus::Late) => {
                // Not a failure: this batch has no wet signal, the state stays as it was.
                self.late += 1;
                false
            }
            Ok(ProcessStatus::Lost) => {
                self.fail_lost();
                false
            }
            Ok(status) => {
                if self.wet[..self.out_channels]
                    .iter()
                    .any(|b| !b.iter().all(|s| s.is_finite()))
                {
                    self.fail("the plugin produced non-finite output");
                    return false;
                }
                if status == ProcessStatus::Sleep && rule == SleepRule::PluginDecides {
                    self.sleeping = true;
                }
                if instrument {
                    let silent = self.wet[..self.out_channels]
                        .iter()
                        .all(|b| b.iter().all(|s| s.abs() <= IDLE_PEAK));
                    self.quiet_batches = if silent && self.events_in.is_empty() {
                        self.quiet_batches.saturating_add(1)
                    } else {
                        0
                    };
                }
                self.state = if stopped {
                    DeviceState::Stopped
                } else if self.sleeping {
                    DeviceState::Sleeping
                } else {
                    DeviceState::Running
                };
                self.forward_notes(ctx);
                true
            }
        }
    }

    /// Writes the main outputs: the wet signal through the bypass crossfade, or (when there is
    /// no valid wet signal) the dry input for effects and silence for the rest.
    fn write_outputs(
        &mut self,
        ctx: &mut ProcessContext<'_>,
        dry: &[Batch; 2],
        wet_valid: bool,
        bypassed: bool,
    ) {
        let target = if bypassed { 1.0 } else { 0.0 };
        let (Some(out_l), Some(out_r)) = (self.sockets.out_l, self.sockets.out_r) else {
            self.bypass_mix = target;
            return;
        };
        if !wet_valid {
            self.bypass_mix = target;
            *ctx.output(out_l) = dry[0];
            *ctx.output(out_r) = dry[1];
            self.out_peak = peak(&dry[0], &dry[1]);
            return;
        }
        let wet_left = self.wet[0];
        let wet_right = if self.out_channels >= 2 {
            self.wet[1]
        } else {
            self.wet[0]
        };
        let mut left = SILENCE;
        let mut right = SILENCE;
        for i in 0..BATCH {
            if self.bypass_mix < target {
                self.bypass_mix = (self.bypass_mix + self.fade_step).min(target);
            } else if self.bypass_mix > target {
                self.bypass_mix = (self.bypass_mix - self.fade_step).max(target);
            }
            let m = self.bypass_mix;
            left[i] = mix(wet_left[i], dry[0][i], m);
            right[i] = mix(wet_right[i], dry[1][i], m);
        }
        self.out_peak = peak(&left, &right);
        *ctx.output(out_l) = left;
        *ctx.output(out_r) = right;
    }
}

impl Device for PluginDevice {
    fn descriptor(&self) -> &'static DeviceDescriptor {
        self.descriptor
    }

    fn prepare(&mut self, info: &PrepareInfo) {
        self.sample_rate = info.sample_rate.hz();
        self.fade_step = fade_step_for(self.sample_rate);
        self.idle_batches = idle_batches_for(self.sample_rate);
        if (self.sample_rate - self.activation_rate).abs() > 0.5 && !self.restart_posted {
            self.restart_posted = true;
            self.messenger.post(PluginMessage::RequestRestart);
        }
    }

    fn reset(&mut self) {
        self.close_all_notes();
        self.sleeping = false;
        self.quiet_batches = 0;
        self.bypass_mix = if self.bypass || !self.slot.powered() {
            1.0
        } else {
            0.0
        };
        if self.started
            && let Some(p) = self.processor.as_mut()
        {
            p.reset();
        }
    }

    fn set_param(&mut self, id: ParamId, plain: f32) {
        if !plain.is_finite() {
            return;
        }
        let i = id.0 as usize;
        if i == 0 {
            self.bypass = plain >= 0.5;
        } else if i < self.values.len() {
            self.values[i] = f64::from(plain);
            self.ever_set[i] = true;
            self.mark_dirty(i);
        }
    }

    fn process(&mut self, ctx: &mut ProcessContext<'_>) {
        let _audio = AudioThreadGuard::enter();
        self.service_handshake();
        let powered = self.slot.powered();
        let bypassed = self.bypass || !powered;
        let raw = [
            input_copy(ctx, self.sockets.in_l),
            input_copy(ctx, self.sockets.in_r),
        ];
        self.in_peak = peak(&raw[0], &raw[1]);
        let dry = if self.descriptor.kind == DeviceKind::Effect {
            raw
        } else {
            [SILENCE; 2]
        };
        let wet_valid = if self.processor.is_none() {
            self.state = DeviceState::Restarting;
            false
        } else if self.failed {
            false
        } else {
            self.run_plugin(ctx, &raw, bypassed, powered)
        };
        self.write_outputs(ctx, &dry, wet_valid, bypassed);
        self.slot.publish_state(self.state);
    }

    fn has_meter_values(&self) -> bool {
        true
    }

    fn meter_values(&self, out: &mut [f32]) -> usize {
        let values = [
            self.in_peak,
            self.out_peak,
            self.state.as_f32(),
            self.slot.latency() as f32,
            self.dropped as f32,
            self.late as f32,
        ];
        let n = out.len().min(values.len());
        out[..n].copy_from_slice(&values[..n]);
        n
    }

    fn latency_frames(&self) -> u32 {
        self.slot.latency()
    }

    fn pinned_to_audio_thread(&self) -> bool {
        self.slot.pinned()
    }

    fn set_run_mode(&mut self, mode: RunMode) {
        self.run_mode = mode;
    }
}

impl Drop for PluginDevice {
    /// The plugin must be destroyed on the main thread, not on whatever thread drops a retired
    /// device: the processor goes to the reaper queue. Without a reaper (tests) it is
    /// dropped here.
    fn drop(&mut self) {
        let Some(reaper) = self.reaper.as_ref() else {
            return;
        };
        // Always post: the manager must learn that the device is gone even when the processor
        // is with it (a restart in flight).
        reaper.post(ReapItem {
            slot: self.slot.clone(),
            processor: self.processor.take(),
        });
        // A processor the manager sent that the device never took also belongs to the reaper.
        while let Ok(processor) = self.from_manager.pop() {
            reaper.post(ReapItem {
                slot: self.slot.clone(),
                processor: Some(processor),
            });
        }
    }
}
