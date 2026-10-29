//! Test support: an [`EngineSink`] over a real, offline engine that the test renders by hand
//! with scripted input (no audio device). Used by the audio recording and streaming tests, which
//! need the real record tap, transport state and streamer.

use crate::record::RecordedNote;
use crate::sink::EngineSink;
use crate::target::Target;
use atomik_core::{NoteKind, ParamId, Tempo};
use atomik_device_sdk::{Device, PlayerDataRef, SampleMapRef};
use atomik_engine::{
    DeviceInstanceId, Endpoint, Engine, EngineConfig, EngineError, EngineHandle, MeterReading,
    Meters, RecordTap, RoutingPlan, SongInput, SongSnapshot, TapChannel, TapControls, Topology,
    TransportCommand, TransportState, new_engine,
};
use std::sync::Arc;

/// Scripted host input: channel and host frame to sample.
pub type InputScript = Box<dyn Fn(usize, u64) -> f32>;

/// A real engine rendered by [`OfflineSink::render`].
pub struct OfflineSink {
    engine: Option<Engine>,
    handle: Option<EngineHandle>,
    meters: Option<Arc<Meters>>,
    /// Host inputs of the engine (from the last reset).
    pub inputs: u16,
    outputs: u16,
    /// Scripted input: `(channel, host frame) -> sample`.
    pub input: InputScript,
    /// What [`EngineSink::record_offset_frames`] reports.
    pub compensation: i64,
    /// Every reset's input channel count.
    pub resets: Vec<u16>,
    /// Overrides the tap capacity the recorder asks for (frames), to force overflows.
    pub tap_capacity: Option<usize>,
    /// Overrides [`EngineSink::stream_threshold_bytes`].
    pub threshold: Option<u64>,
    /// Host frames rendered since the last reset.
    pub frames: u64,
    /// The last rendered output, per channel.
    pub last_output: Vec<Vec<f32>>,
    /// `set_param` calls since construction (the plugin tests check a value is not echoed back).
    pub param_calls: usize,
}

impl Default for OfflineSink {
    fn default() -> Self {
        Self {
            engine: None,
            handle: None,
            meters: None,
            inputs: 0,
            outputs: 2,
            input: Box::new(|_, _| 0.0),
            compensation: 64,
            resets: Vec::new(),
            tap_capacity: None,
            threshold: None,
            frames: 0,
            last_output: Vec::new(),
            param_calls: 0,
        }
    }
}

impl std::fmt::Debug for OfflineSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OfflineSink")
            .field("frames", &self.frames)
            .finish_non_exhaustive()
    }
}

impl OfflineSink {
    /// A sink with no engine until the first reset.
    pub fn new() -> Self {
        Self::default()
    }

    /// Renders `frames` host frames in blocks of `block` with the scripted input and returns the
    /// output (per channel).
    pub fn render_blocks(&mut self, frames: usize, block: usize) -> Vec<Vec<f32>> {
        let outputs = usize::from(self.outputs);
        let mut all = vec![Vec::with_capacity(frames); outputs];
        let Some(engine) = self.engine.as_mut() else {
            return all;
        };
        let mut done = 0;
        while done < frames {
            let n = block.min(frames - done);
            let ins: Vec<Vec<f32>> = (0..usize::from(self.inputs))
                .map(|c| {
                    (0..n)
                        .map(|i| (self.input)(c, self.frames + i as u64))
                        .collect()
                })
                .collect();
            let in_refs: Vec<&[f32]> = ins.iter().map(Vec::as_slice).collect();
            let mut outs = vec![vec![0.0f32; n]; outputs];
            let mut out_refs: Vec<&mut [f32]> = outs.iter_mut().map(Vec::as_mut_slice).collect();
            engine.render(&in_refs, &mut out_refs);
            for (a, o) in all.iter_mut().zip(&outs) {
                a.extend_from_slice(o);
            }
            self.frames += n as u64;
            done += n;
        }
        self.last_output = all.clone();
        all
    }

    /// Renders 64-frame blocks.
    pub fn render(&mut self, frames: usize) -> Vec<Vec<f32>> {
        self.render_blocks(frames, 64)
    }
}

macro_rules! handle {
    ($self:ident) => {
        match $self.handle.as_mut() {
            Some(h) => h,
            None => return Ok(()),
        }
    };
}

impl EngineSink for OfflineSink {
    fn reset(&mut self, config: &EngineConfig) {
        self.resets.push(config.input_channels);
        self.inputs = config.input_channels;
        self.outputs = config.output_channels;
        self.frames = 0;
        self.engine = None;
        if let Some(mut h) = self.handle.take() {
            h.collect_garbage();
        }
        let (engine, handle) = new_engine(*config);
        self.meters = Some(handle.meters());
        self.engine = Some(engine);
        self.handle = Some(handle);
    }

    fn insert_device(
        &mut self,
        id: DeviceInstanceId,
        device: Box<dyn Device>,
        params: &[(ParamId, f32)],
    ) -> Result<(), EngineError> {
        handle!(self).insert_device(id, device, params)
    }

    fn remove_device(&mut self, id: DeviceInstanceId) -> Result<(), EngineError> {
        handle!(self).remove_device(id)
    }

    fn submit_routing(&mut self, plan: RoutingPlan) -> Result<(), EngineError> {
        handle!(self).submit_routing(plan)
    }

    fn set_device_samples(
        &mut self,
        id: DeviceInstanceId,
        map: Option<SampleMapRef>,
    ) -> Result<(), EngineError> {
        handle!(self).set_device_samples(id, map)
    }

    fn set_device_player_data(
        &mut self,
        id: DeviceInstanceId,
        data: Option<PlayerDataRef>,
    ) -> Result<(), EngineError> {
        handle!(self).set_device_player_data(id, data)
    }

    fn set_param(
        &mut self,
        id: DeviceInstanceId,
        param: ParamId,
        plain: f32,
    ) -> Result<(), EngineError> {
        self.param_calls += 1;
        handle!(self).set_param(id, param, plain)
    }

    fn set_tempo(&mut self, tempo: Tempo) -> Result<(), EngineError> {
        handle!(self).set_tempo(tempo)
    }

    fn collect_garbage(&mut self) -> usize {
        self.handle
            .as_mut()
            .map_or(0, EngineHandle::collect_garbage)
    }

    fn submit_song(&mut self, _input: &SongInput, song: SongSnapshot) -> Result<(), EngineError> {
        handle!(self).submit_song(song)
    }

    fn transport(&mut self, command: TransportCommand) -> Result<(), EngineError> {
        handle!(self).transport(command)
    }

    fn transport_state(&self) -> TransportState {
        self.handle
            .as_ref()
            .map_or_else(TransportState::default, EngineHandle::transport_state)
    }

    fn live_note(&mut self, _target: Target, _kind: NoteKind) -> Result<(), EngineError> {
        Ok(())
    }

    fn play_note(&mut self, _target: Target, _kind: NoteKind) -> Result<(), EngineError> {
        Ok(())
    }

    fn set_recording(&mut self, _devices: &[DeviceInstanceId]) {}

    fn drain_recorded(&mut self, _out: &mut Vec<RecordedNote>) {}

    fn hold_automation(&mut self, lanes: &[(DeviceInstanceId, ParamId)]) {
        if let Some(h) = &self.handle {
            let _ = h.hold_automation(lanes);
        }
    }

    fn latency_epoch(&self) -> u64 {
        self.handle.as_ref().map_or(0, EngineHandle::latency_epoch)
    }

    fn fill_latencies(&self, topology: &mut Topology) {
        if let Some(h) = &self.handle {
            h.fill_latencies(topology);
        }
    }

    fn read_meter(&self, endpoint: Endpoint) -> Option<MeterReading> {
        self.meters.as_ref()?.read(endpoint)
    }

    fn read_device_meter_values(
        &self,
        id: DeviceInstanceId,
        out: &mut [f32],
    ) -> Option<(usize, u64)> {
        self.meters.as_ref()?.device_values(id, out)
    }

    fn read_device_scope(
        &self,
        id: DeviceInstanceId,
        left: &mut [f32],
        right: &mut [f32],
    ) -> Option<(usize, u64)> {
        let tap = self.meters.as_ref()?.scope_tap(id)?;
        Some(tap.read_latest(left, right))
    }

    fn start_audio_record(
        &mut self,
        channels: &[TapChannel],
        capacity_frames: usize,
        controls: Arc<TapControls>,
    ) -> Option<RecordTap> {
        self.handle
            .as_mut()?
            .record_tap_gained(
                channels,
                self.tap_capacity.unwrap_or(capacity_frames),
                controls,
            )
            .ok()
    }

    fn stop_audio_record(&mut self) {
        if let Some(h) = &mut self.handle {
            let _ = h.stop_record_tap();
        }
    }

    fn record_offset_frames(&self) -> i64 {
        self.compensation
    }

    fn stream_underruns(&self) -> u64 {
        self.handle
            .as_ref()
            .map_or(0, EngineHandle::stream_underruns)
    }

    fn stream_primed(&self) -> bool {
        self.handle.as_ref().is_none_or(EngineHandle::stream_primed)
    }

    fn stream_threshold_bytes(&self) -> Option<u64> {
        self.threshold
    }

    // This sink is the hand-rendered test engine, so it takes previews like the live session
    // does (the browser tests listen to them). Real offline renders (bounce) never get one:
    // they do not go through `EngineSink`.
    fn preview_start(
        &mut self,
        stream: atomik_engine::PreviewStream,
        gain: f32,
    ) -> Result<(), EngineError> {
        handle!(self).preview_start(stream, gain)
    }

    fn preview_stop(&mut self) -> Result<(), EngineError> {
        handle!(self).preview_stop()
    }

    fn preview_active(&self) -> bool {
        self.handle
            .as_ref()
            .is_some_and(EngineHandle::preview_active)
    }
}
