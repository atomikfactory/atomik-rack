//! Atomik Rack audio engine: the editable [`Graph`], its compilation into an immutable
//! [`RenderPlan`], the real-time [`Engine`] with its non-RT [`EngineHandle`], and the
//! [`render_offline`] helper.
//!
//!
mod arena;
mod audio;
mod console;
mod controllers;
mod engine;
mod fault;
mod graph;
mod interface;
mod live;
mod offline;
mod plan;
mod player;
mod preview;
mod record;
mod seq;
mod stream;
mod sync;
mod workers;

pub use arena::{RoutingPlan, Topology};
pub use audio::{
    AssetId, SongAsset, SongAudioRegion, SongAudioTrack, SongFadeCurve, StreamSource, sin_quarter,
};
pub use console::{
    MAX_COMPENSATION_SECONDS, MAX_METER_VALUES, METER_CAPACITY, MeterReading, Meters, PdcReport,
};
pub use controllers::{RAMP_STEP, SongController, SongControllerKind};
pub use engine::{
    CONTROL_QUEUE_CAPACITY, DEFAULT_ARENA_CAPACITY, Engine, EngineError, EngineHandle, HotEdit,
    NOTE_QUEUE_CAPACITY, PARAM_QUEUE_CAPACITY, PLAN_QUEUE_CAPACITY, RETIRE_QUEUE_CAPACITY,
    SONG_EVENT_CAPACITY, ScheduledNote, TimedEdit, new_engine, new_engine_with_arena,
    new_engine_with_options,
};
pub use graph::{Cable, DeviceInstanceId, Endpoint, EngineConfig, Graph, GraphError};
pub use interface::{INTERFACE_TYPE_ID, MAX_INTERFACE_CHANNELS, interface_descriptor};
pub use live::{EngineOptions, FrameClock, LiveNoteSender, WorkerConfig};
pub use offline::{
    OfflineError, OfflineJob, render_offline, render_offline_scripted, render_offline_song,
    render_offline_with, try_render_offline, try_render_offline_scripted, try_render_offline_song,
    try_render_offline_with,
};
pub use plan::RenderPlan;
pub use player::{HOLD_CAPACITY, lane_value};
pub use preview::{
    PREVIEW_RING_FRAMES, PreviewFeed, PreviewMonitor, PreviewStream, preview_channel,
};
pub use record::{LIMIT_RELEASE_FRAMES, RecordTap, TapChannel, TapControls};
pub use seq::{
    PreRoll, SongAutomation, SongCurve, SongError, SongInput, SongNote, SongPoint, SongPosition,
    SongSnapshot, SongTempo, SongTrack, TransportCommand, TransportState, compile_song,
};
pub use sync::{
    CLOCK, CONTINUE, ExternalClock, ExternalState, SONG_POSITION, START, STOP, SyncConfig, SyncOut,
    TICKS_PER_CLOCK, TICKS_PER_POSITION, clocks_in, song_position_bytes, song_position_ticks,
};
#[doc(hidden)]
pub use workers::WORKER_SPIN_LIMIT;

#[cfg(test)]
atomik_testkit::install_alloc_guard!();
