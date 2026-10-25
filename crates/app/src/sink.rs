//! The engine boundary of [`crate::AppCore`]: [`EngineSink`], implemented by the live session
//! (an [`atomik_engine::EngineHandle`] plus audio and MIDI) and by [`RecordingSink`] in tests.

use crate::record::RecordedNote;
use crate::target::{NoteTarget, Target};
use atomik_core::{NoteKind, ParamId, Tempo};
use atomik_device_sdk::{Device, PlayerDataRef, SampleMapRef};
use atomik_engine::{
    DeviceInstanceId, Endpoint, EngineConfig, EngineError, MeterReading, PreviewStream, RecordTap,
    RoutingPlan, SongInput, SongSnapshot, Topology, TransportCommand, TransportState,
};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Where [`crate::AppCore`] sends engine operations.
///
/// Every method runs on the UI thread and must not block on the audio thread.
pub trait EngineSink {
    /// Replaces the engine with a fresh, empty one built from `config` (open, new, audio
    /// restart). The core then inserts every device with ids 1, 2, 3… and submits one routing.
    fn reset(&mut self, config: &EngineConfig);
    /// [`atomik_engine::EngineHandle::insert_device`].
    fn insert_device(
        &mut self,
        id: DeviceInstanceId,
        device: Box<dyn Device>,
        params: &[(ParamId, f32)],
    ) -> Result<(), EngineError>;
    /// [`atomik_engine::EngineHandle::remove_device`].
    fn remove_device(&mut self, id: DeviceInstanceId) -> Result<(), EngineError>;
    /// [`atomik_engine::EngineHandle::submit_routing`].
    fn submit_routing(&mut self, plan: RoutingPlan) -> Result<(), EngineError>;
    /// [`atomik_engine::EngineHandle::set_device_samples`]: installs a sampler's
    /// sample map (`None` clears it) in queue order after the device's insert.
    fn set_device_samples(
        &mut self,
        id: DeviceInstanceId,
        map: Option<SampleMapRef>,
    ) -> Result<(), EngineError>;
    /// [`atomik_engine::EngineHandle::set_device_player_data`]: installs a step
    /// sequencer's pattern data (`None` clears it) in queue order after the device's insert.
    fn set_device_player_data(
        &mut self,
        id: DeviceInstanceId,
        data: Option<PlayerDataRef>,
    ) -> Result<(), EngineError>;
    /// [`atomik_engine::EngineHandle::set_param`].
    fn set_param(
        &mut self,
        id: DeviceInstanceId,
        param: ParamId,
        plain: f32,
    ) -> Result<(), EngineError>;
    /// [`atomik_engine::EngineHandle::set_tempo`].
    fn set_tempo(&mut self, tempo: Tempo) -> Result<(), EngineError>;
    /// [`atomik_engine::EngineHandle::collect_garbage`]; called once per UI frame.
    fn collect_garbage(&mut self) -> usize;
    /// [`atomik_engine::EngineHandle::submit_song`]. `input` is what `song` was
    /// compiled from (the fake records it; the live session ignores it).
    fn submit_song(&mut self, input: &SongInput, song: SongSnapshot) -> Result<(), EngineError>;
    /// [`atomik_engine::EngineHandle::transport`].
    fn transport(&mut self, command: TransportCommand) -> Result<(), EngineError>;
    /// [`atomik_engine::EngineHandle::transport_state`]: the latest published state (the
    /// default state when no engine runs).
    fn transport_state(&self) -> TransportState;
    /// Sends one live note to `target` now, on the live note path (audition).
    fn live_note(&mut self, target: Target, kind: NoteKind) -> Result<(), EngineError>;
    /// Arms (non-empty) or disarms (empty) MIDI recording: live notes routed to one of
    /// `devices` are captured with their engine frame.
    fn set_recording(&mut self, devices: &[DeviceInstanceId]);
    /// Moves the captured live notes into `out` (oldest first).
    fn drain_recorded(&mut self, out: &mut Vec<RecordedNote>);
    /// [`atomik_engine::EngineHandle::hold_automation`]: the song player leaves these
    /// `(device, param)` lanes alone (empty releases them). A take that writes automation
    /// holds the lanes it writes. The default does nothing (no engine).
    fn hold_automation(&mut self, _lanes: &[(DeviceInstanceId, ParamId)]) {}
    /// The live-notes target cell the sink's MIDI input reads, if it has one; the core
    /// publishes the target there. `None` (the default) makes the core create its own.
    fn note_target(&self) -> Option<Arc<NoteTarget>> {
        None
    }
    /// The control hub the sink's MIDI input fills (the mappings read it); `None` (the default)
    /// makes the core create its own, which only injected messages reach.
    fn control_hub(&self) -> Option<Arc<crate::midi_control::ControlHub>> {
        None
    }
    /// [`atomik_engine::EngineHandle::latency_epoch`]: when it moves, the core
    /// refills the latencies and resubmits the routing. 0 when no engine runs.
    fn latency_epoch(&self) -> u64 {
        0
    }
    /// [`atomik_engine::EngineHandle::fill_latencies`]: copies the engine's
    /// `Device::latency_frames()` into `topology` before a compile.
    fn fill_latencies(&self, _topology: &mut Topology) {}
    /// [`atomik_engine::Meters::read`]: reads (and resets the peak of) a metered
    /// output; `None` when it is not metered or no engine runs. Lock-free.
    fn read_meter(&self, _endpoint: Endpoint) -> Option<MeterReading> {
        None
    }
    /// [`atomik_engine::Meters::device_values`]: the values a device published with
    /// `Device::meter_values` into `out`, as `(count, publish sequence)`; `None` when the
    /// device publishes nothing or no engine runs. Lock-free.
    fn read_device_meter_values(
        &self,
        _id: DeviceInstanceId,
        _out: &mut [f32],
    ) -> Option<(usize, u64)> {
        None
    }
    /// [`atomik_engine::Meters::scope_tap`]: copies the newest frames of the scope tap of a
    /// spectrum or phase meter device into `left` and `right` (oldest first, at most their
    /// length) and returns `(frames copied, the tap's write position)`; `None` when the device
    /// has no tap or no engine runs. Lock-free.
    fn read_device_scope(
        &self,
        _id: DeviceInstanceId,
        _left: &mut [f32],
        _right: &mut [f32],
    ) -> Option<(usize, u64)> {
        None
    }
    /// [`atomik_engine::EngineHandle::record_tap`]: starts copying the host input
    /// `channels` into a ring of `capacity_frames` for the audio recorder. `None` when no
    /// engine runs (the default).
    fn start_audio_record(
        &mut self,
        _channels: &[usize],
        _capacity_frames: usize,
    ) -> Option<RecordTap> {
        None
    }
    /// [`atomik_engine::EngineHandle::stop_record_tap`]: stops the tap (frames in the ring
    /// stay drainable).
    fn stop_audio_record(&mut self) {}
    /// The compensation in frames that moves a recording onto the timeline
    /// ([`crate::audio_record::record_offset_frames`]: the measured input and output latencies,
    /// the engine FIFO and the user offset).
    fn record_offset_frames(&self) -> i64 {
        crate::audio_record::record_offset_frames(0, 0, 0)
    }
    /// Streamed audio the engine could not deliver in time so far
    /// ([`atomik_engine::EngineHandle::stream_underruns`]).
    fn stream_underruns(&self) -> u64 {
        0
    }
    /// [`atomik_engine::EngineHandle::stream_primed`]: every streamed track has its read-ahead
    /// (true when nothing streams or no engine runs).
    fn stream_primed(&self) -> bool {
        true
    }
    /// The streaming threshold override in bytes (the hidden setting), if any.
    fn stream_threshold_bytes(&self) -> Option<u64> {
        None
    }
    /// The channel counts of the host audio device, `(inputs, outputs)`, while a stream runs
    /// (the Audio I/O lamps turn red for a cabled socket the device does
    /// not have). `None` for the offline sink and while audio is off: every channel counts as
    /// available.
    fn host_channels(&self) -> Option<(u16, u16)> {
        None
    }
    /// Whether an engine exists to take devices (false while audio is off). Plugin instances are
    /// only created while it does, since a device handed to no engine is dropped at once.
    fn has_engine(&self) -> bool {
        true
    }
    /// Why no engine runs, when the last start failed (shown on plugin panels that wait).
    fn engine_error(&self) -> Option<String> {
        None
    }
    /// [`atomik_engine::EngineHandle::preview_start`]: plays a file browser preview (live only;
    /// the engine fades a previous one out). Without an engine the stream is dropped, which its
    /// feed sees as abandoned.
    fn preview_start(&mut self, _stream: PreviewStream, _gain: f32) -> Result<(), EngineError> {
        Ok(())
    }
    /// [`atomik_engine::EngineHandle::preview_stop`].
    fn preview_stop(&mut self) -> Result<(), EngineError> {
        Ok(())
    }
    /// [`atomik_engine::EngineHandle::preview_active`]: false when no engine runs.
    fn preview_active(&self) -> bool {
        false
    }
}

/// One call recorded by [`RecordingSink`].
#[derive(Debug, Clone, PartialEq)]
pub enum SinkCall {
    /// [`EngineSink::reset`].
    Reset {
        /// Sample rate in Hz.
        sample_rate: f64,
        /// Tempo in BPM.
        tempo_bpm: f64,
        /// Output channels.
        output_channels: u16,
        /// Patch seed.
        seed: u64,
    },
    /// [`EngineSink::insert_device`].
    Insert {
        /// Instance id.
        id: DeviceInstanceId,
        /// Device type id (from the instance's descriptor).
        type_id: &'static str,
        /// Params applied after prepare.
        params: Vec<(ParamId, f32)>,
    },
    /// [`EngineSink::remove_device`].
    Remove(DeviceInstanceId),
    /// [`EngineSink::submit_routing`]: the plan's processing order.
    Routing(Vec<DeviceInstanceId>),
    /// [`EngineSink::hold_automation`].
    Hold(Vec<(DeviceInstanceId, ParamId)>),
    /// [`EngineSink::set_param`].
    Param {
        /// Device.
        id: DeviceInstanceId,
        /// Param.
        param: ParamId,
        /// Plain value.
        plain: f32,
    },
    /// [`EngineSink::set_device_samples`]: the map that was sent (`None` clears it).
    Samples {
        /// Device.
        id: DeviceInstanceId,
        /// The map.
        map: Option<SampleMapRef>,
    },
    /// [`EngineSink::set_device_player_data`]: the data that was sent (`None` clears it).
    PlayerData {
        /// Device.
        id: DeviceInstanceId,
        /// The pattern data.
        data: Option<PlayerDataRef>,
    },
    /// [`EngineSink::set_tempo`] in BPM.
    Tempo(f64),
    /// [`EngineSink::submit_song`]: the input the snapshot was compiled from.
    Song(SongInput),
    /// [`EngineSink::transport`].
    Transport(TransportCommand),
    /// [`EngineSink::live_note`].
    LiveNote {
        /// Target device and socket.
        target: Target,
        /// The event.
        kind: NoteKind,
    },
    /// [`EngineSink::set_recording`].
    Recording(Vec<DeviceInstanceId>),
    /// [`EngineSink::preview_start`] (the stream itself is held in
    /// [`RecordingSink::previews`]).
    PreviewStart {
        /// Linear gain.
        gain: f32,
    },
    /// [`EngineSink::preview_stop`].
    PreviewStop,
}

/// A preview stream held by [`RecordingSink`] so its feed is not abandoned while a test runs.
pub struct HeldStream(pub PreviewStream);

impl std::fmt::Debug for HeldStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HeldStream")
    }
}

/// A fake engine that records every call (garbage collection is counted, not recorded). It can
/// be told to fail the next call.
#[derive(Debug, Default)]
pub struct RecordingSink {
    /// Calls in order.
    pub calls: Vec<SinkCall>,
    /// `collect_garbage` calls.
    pub garbage_collections: usize,
    /// When set, the next fallible call returns this error (and records nothing).
    pub fail_next: Option<EngineError>,
    /// What [`EngineSink::transport_state`] returns (tests script the engine timeline).
    pub state: TransportState,
    /// What [`EngineSink::drain_recorded`] hands out next (tests script live input).
    pub recorded: Vec<RecordedNote>,
    /// What [`EngineSink::fill_latencies`] reports per device (tests script latencies).
    pub latencies: BTreeMap<DeviceInstanceId, u32>,
    /// What [`EngineSink::latency_epoch`] returns.
    pub epoch: u64,
    /// What [`EngineSink::read_meter`] returns per endpoint (not reset on read).
    pub meters: BTreeMap<Endpoint, MeterReading>,
    /// When set, a read spends the endpoint's reading (a second read of it finds nothing until
    /// the test inserts one again), like the real meter bank, whose reads reset the peak.
    pub spend_meters_on_read: bool,
    /// The endpoints already spent by [`Self::spend_meters_on_read`].
    pub spent_meters: std::sync::Mutex<std::collections::BTreeSet<Endpoint>>,
    /// What [`EngineSink::read_device_meter_values`] returns per device: the values and the
    /// publish sequence.
    pub device_values: BTreeMap<DeviceInstanceId, (Vec<f32>, u64)>,
    /// What [`EngineSink::read_device_scope`] returns per device: the frames (left, right) and
    /// the tap's write position.
    pub device_scopes: BTreeMap<DeviceInstanceId, (Vec<f32>, Vec<f32>, u64)>,
    /// The PDC report of the last submitted routing.
    pub pdc: Option<atomik_engine::PdcReport>,
    /// What [`EngineSink::stream_threshold_bytes`] returns.
    pub stream_threshold: Option<u64>,
    /// When set, [`EngineSink::stream_primed`] reports false (the streamer has no read-ahead).
    pub unprimed: bool,
    /// When set, [`EngineSink::has_engine`] reports false (audio off).
    pub no_engine: bool,
    /// What [`EngineSink::host_channels`] returns.
    pub host_channels: Option<(u16, u16)>,
    /// The preview stream the fake keeps alive (at most one: a new start replaces it, a stop
    /// drops it, as the engine retires streams). Nothing reads it; its feed sees it as playing.
    pub previews: Vec<HeldStream>,
}

impl RecordingSink {
    /// An empty recorder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes the recorded calls, leaving the list empty.
    pub fn take(&mut self) -> Vec<SinkCall> {
        std::mem::take(&mut self.calls)
    }

    fn record(&mut self, call: SinkCall) -> Result<(), EngineError> {
        if let Some(e) = self.fail_next.take() {
            return Err(e);
        }
        self.calls.push(call);
        Ok(())
    }
}

impl EngineSink for RecordingSink {
    fn reset(&mut self, config: &EngineConfig) {
        self.calls.push(SinkCall::Reset {
            sample_rate: config.sample_rate.hz(),
            tempo_bpm: config.tempo.bpm(),
            output_channels: config.output_channels,
            seed: config.seed,
        });
    }

    fn insert_device(
        &mut self,
        id: DeviceInstanceId,
        device: Box<dyn Device>,
        params: &[(ParamId, f32)],
    ) -> Result<(), EngineError> {
        self.record(SinkCall::Insert {
            id,
            type_id: device.descriptor().type_id,
            params: params.to_vec(),
        })
    }

    fn remove_device(&mut self, id: DeviceInstanceId) -> Result<(), EngineError> {
        self.record(SinkCall::Remove(id))
    }

    fn submit_routing(&mut self, plan: RoutingPlan) -> Result<(), EngineError> {
        let pdc = plan.pdc_report().clone();
        self.record(SinkCall::Routing(plan.order().to_vec()))?;
        self.pdc = Some(pdc);
        Ok(())
    }

    fn set_device_samples(
        &mut self,
        id: DeviceInstanceId,
        map: Option<SampleMapRef>,
    ) -> Result<(), EngineError> {
        self.record(SinkCall::Samples { id, map })
    }

    fn set_device_player_data(
        &mut self,
        id: DeviceInstanceId,
        data: Option<PlayerDataRef>,
    ) -> Result<(), EngineError> {
        self.record(SinkCall::PlayerData { id, data })
    }

    fn set_param(
        &mut self,
        id: DeviceInstanceId,
        param: ParamId,
        plain: f32,
    ) -> Result<(), EngineError> {
        self.record(SinkCall::Param { id, param, plain })
    }

    fn set_tempo(&mut self, tempo: Tempo) -> Result<(), EngineError> {
        self.record(SinkCall::Tempo(tempo.bpm()))
    }

    fn collect_garbage(&mut self) -> usize {
        self.garbage_collections += 1;
        0
    }

    fn submit_song(&mut self, input: &SongInput, _song: SongSnapshot) -> Result<(), EngineError> {
        self.record(SinkCall::Song(input.clone()))
    }

    fn transport(&mut self, command: TransportCommand) -> Result<(), EngineError> {
        self.record(SinkCall::Transport(command))
    }

    fn transport_state(&self) -> TransportState {
        self.state
    }

    fn live_note(&mut self, target: Target, kind: NoteKind) -> Result<(), EngineError> {
        self.record(SinkCall::LiveNote { target, kind })
    }

    fn set_recording(&mut self, devices: &[DeviceInstanceId]) {
        self.calls.push(SinkCall::Recording(devices.to_vec()));
    }

    fn drain_recorded(&mut self, out: &mut Vec<RecordedNote>) {
        out.append(&mut self.recorded);
    }

    fn hold_automation(&mut self, lanes: &[(DeviceInstanceId, ParamId)]) {
        self.calls.push(SinkCall::Hold(lanes.to_vec()));
    }

    fn latency_epoch(&self) -> u64 {
        self.epoch
    }

    fn fill_latencies(&self, topology: &mut Topology) {
        for (id, frames) in &self.latencies {
            let _ = topology.set_latency(*id, *frames);
        }
    }

    fn read_meter(&self, endpoint: Endpoint) -> Option<MeterReading> {
        let reading = self.meters.get(&endpoint).copied()?;
        if self.spend_meters_on_read
            && let Ok(mut spent) = self.spent_meters.lock()
            && !spent.insert(endpoint)
        {
            return None;
        }
        Some(reading)
    }

    fn read_device_meter_values(
        &self,
        id: DeviceInstanceId,
        out: &mut [f32],
    ) -> Option<(usize, u64)> {
        let (values, seq) = self.device_values.get(&id)?;
        let n = values.len().min(out.len());
        out[..n].copy_from_slice(&values[..n]);
        Some((n, *seq))
    }

    fn read_device_scope(
        &self,
        id: DeviceInstanceId,
        left: &mut [f32],
        right: &mut [f32],
    ) -> Option<(usize, u64)> {
        let (l, r, head) = self.device_scopes.get(&id)?;
        let n = l.len().min(r.len()).min(left.len()).min(right.len());
        left[..n].copy_from_slice(&l[l.len() - n..]);
        right[..n].copy_from_slice(&r[r.len() - n..]);
        Some((n, *head))
    }

    fn stream_primed(&self) -> bool {
        !self.unprimed
    }

    fn stream_threshold_bytes(&self) -> Option<u64> {
        self.stream_threshold
    }

    fn host_channels(&self) -> Option<(u16, u16)> {
        self.host_channels
    }

    fn has_engine(&self) -> bool {
        !self.no_engine
    }

    fn preview_start(&mut self, stream: PreviewStream, gain: f32) -> Result<(), EngineError> {
        self.record(SinkCall::PreviewStart { gain })?;
        self.previews.clear();
        self.previews.push(HeldStream(stream));
        Ok(())
    }

    fn preview_stop(&mut self) -> Result<(), EngineError> {
        self.record(SinkCall::PreviewStop)?;
        self.previews.clear();
        Ok(())
    }

    fn preview_active(&self) -> bool {
        !self.previews.is_empty()
    }
}

/// Test support: after the last engine reset the engine got the new devices first and then a
/// routing that names every one of them. A routing sent before the devices leaves a new track
/// unconnected, so it makes no sound.
#[cfg(test)]
pub(crate) fn assert_new_devices_routed(calls: &[SinkCall]) {
    let last_reset = calls
        .iter()
        .rposition(|c| matches!(c, SinkCall::Reset { .. }))
        .map_or(0, |i| i + 1);
    let calls = &calls[last_reset..];
    let new: Vec<_> = calls
        .iter()
        .filter_map(|c| match c {
            SinkCall::Insert { id, .. } => Some(*id),
            _ => None,
        })
        .collect();
    assert!(
        !new.is_empty(),
        "the track and its channel are inserted: {calls:?}"
    );
    let routing = calls
        .iter()
        .rposition(|c| matches!(c, SinkCall::Routing(_)))
        .expect("a routing is sent");
    let last_insert = calls
        .iter()
        .rposition(|c| matches!(c, SinkCall::Insert { .. }))
        .unwrap();
    assert!(
        last_insert < routing,
        "routing before the devices: {calls:?}"
    );
    let SinkCall::Routing(ids) = &calls[routing] else {
        unreachable!()
    };
    for id in new {
        assert!(ids.contains(&id), "{id:?} is not routed: {ids:?}");
    }
}
