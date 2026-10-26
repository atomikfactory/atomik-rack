//! Atomik Rack device SDK: static device descriptors, the [`Device`] trait, the per-batch
//! [`ProcessContext`] and the device [`Registry`].
//!
//! This crate is a stable contract: public changes need review.

mod context;
mod descriptor;
mod panel;
mod player;
mod plugin;
mod registry;
mod samples;
mod scope;

pub use context::ProcessContext;
pub use descriptor::{
    DescriptorError, DeviceDescriptor, DeviceKind, Direction, SocketDescriptor, SocketId,
    SocketKind,
};
pub use panel::{ControlKind, PanelControl, PanelLayout, PanelSocket, RACK_UNIT, RACK_WIDTH};
pub use player::{
    CURVE_ROWS, CurveStep, CvSeqData, MAX_PATTERNS, MAX_STEPS, PatternData, PlayerData,
    PlayerDataRef, SeqResolution, StepData, StepSeqData,
};
pub use plugin::{
    CLAP_PREFIX, Channels, MAX_INTERNED_DESCRIPTORS, MAX_PLUGIN_PARAMS, PluginFormat,
    PluginKindHint, PluginLayout, PluginParamInfo, PluginRef, PluginStandIn, PluginStateBlob,
    RunMode, VST3_PREFIX, descriptor_for, is_plugin_type_id,
};
pub use registry::{DeviceFactory, Registry, RegistryError};
pub use samples::{
    LoopMode, SampleBuffer, SampleMap, SampleMapError, SampleMapRef, TriggerMode, ZONE_GAIN_RANGE,
    ZONE_TRANSPOSE_RANGE, ZONE_TUNE_RANGE, Zone, velocity_to_7bit,
};
pub use scope::{SCOPE_FRAMES, ScopeTap};

use atomik_core::{ParamId, SampleRate};
use std::sync::Arc;

/// Maximum number of values a device can publish through [`Device::meter_values`].
pub const MAX_METER_VALUES: usize = 32;

/// Information passed to [`Device::prepare`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PrepareInfo {
    /// Sample rate the device will run at.
    pub sample_rate: SampleRate,
    /// Deterministic seed for any pseudo-random source in the device.
    pub seed: u64,
}

/// A rack device. Implementations must be real-time safe in every method except [`Device::prepare`].
pub trait Device: Send + 'static {
    /// The static description of this device type.
    fn descriptor(&self) -> &'static DeviceDescriptor;

    /// Non-RT. The only place a device may allocate. Called before the first `process`
    /// and whenever the sample rate changes. Must leave the device in a reset state.
    fn prepare(&mut self, info: &PrepareInfo);

    /// RT-safe. Clears all audio state (voices, delay lines, smoothers) without allocating.
    fn reset(&mut self);

    /// RT-safe. Sets a parameter to an already-clamped plain value.
    fn set_param(&mut self, id: ParamId, plain: f32);

    /// RT-safe. Processes exactly one batch. Must not allocate, lock or block.
    /// Output batches arrive zeroed.
    fn process(&mut self, ctx: &mut ProcessContext<'_>);

    /// RT-safe. Processes one stage of a multi-stage device. Stage `s` may read
    /// inputs of stage <= `s` and writes outputs of stage `s` (which arrive zeroed). The
    /// default runs [`Device::process`] for stage 0 and does nothing for other stages.
    fn process_stage(&mut self, stage: u8, ctx: &mut ProcessContext<'_>) {
        if stage == 0 {
            self.process(ctx);
        }
    }

    /// RT-safe. Installs a new sample map (or none) and returns a map the caller must drop off
    /// the audio thread: the previous map, a map still draining voices, or
    /// `map` itself when the device takes no samples (the default). Must not allocate.
    fn swap_samples(&mut self, map: Option<SampleMapRef>) -> Option<SampleMapRef> {
        map
    }

    /// RT-safe. Installs new pattern data (or none) and returns data the caller must drop off
    /// the audio thread: the previous data, or `data` itself when the
    /// device takes none (the default). Applies at a batch boundary (the engine calls it
    /// between batches) and must not cut a sounding note. Must not allocate.
    fn swap_player_data(&mut self, data: Option<PlayerDataRef>) -> Option<PlayerDataRef> {
        data
    }

    /// Opt-in to [`Device::meter_values`]. The engine reads it once, when the device is
    /// inserted (after `prepare` and the initial params), and only calls `meter_values` for
    /// devices that answered `true`; every other device costs one bool test per batch and
    /// publishes nothing. Non-RT.
    fn has_meter_values(&self) -> bool {
        false
    }

    /// RT-safe. Writes up to `out.len()` (at most [`MAX_METER_VALUES`]) display values (gain
    /// reduction, loudness, band energies...) into `out` and returns how many it wrote; the
    /// default writes none. Called by the engine on the audio thread once per batch, after the
    /// device's last stage. Values are the latest ones, not accumulated: a device that wants
    /// peak-hold does it itself. Only called when [`Device::has_meter_values`] is true. Must not
    /// allocate, lock or block.
    fn meter_values(&self, _out: &mut [f32]) -> usize {
        0
    }

    /// Opt-in to a [`ScopeTap`]: the ring of recent stereo frames the UI analyses (spectrum,
    /// goniometer). Read once by the engine, when the device is inserted or its plan is
    /// submitted, and handed to the UI through the meter bank; the device keeps its own clone
    /// and pushes every batch it processes. Non-RT; the default is no tap.
    fn scope_tap(&self) -> Option<Arc<ScopeTap>> {
        None
    }

    /// Processing latency in frames introduced by this device.
    fn latency_frames(&self) -> u32 {
        0
    }

    /// True when the engine must run this device on the audio callback thread and never hand it
    /// to a render worker (plugins that assume a stable thread). Read on the
    /// audio thread each time the device is about to be dispatched, so it may change at run
    /// time; implementations answer from a plain field or an atomic and must not block.
    fn pinned_to_audio_thread(&self) -> bool {
        false
    }

    /// Non-RT. Tells the device whether it runs live or in an offline bounce. Called before the device is inserted; the default ignores it.
    fn set_run_mode(&mut self, _mode: RunMode) {}
}
