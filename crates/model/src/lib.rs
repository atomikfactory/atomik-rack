//! Atomik Rack document model.
//!
//! The rack document is the single source of truth for the UI. It is pure data: no UI, no
//! engine. Edits are [`Command`]s applied through [`Document`], which keeps undo/redo history
//! and reports what changed as [`SyncOp`]s, so the app can update the engine.
//!
//! Public signatures are a stable contract: changes need review.

mod audio_edit;
mod audio_notes;
mod audition;
pub mod automation_rec;
mod automation_view;
mod clip_io;
mod clip_transform;
mod console_bus;
mod console_cmd;
mod console_insert;
mod console_mix;
mod console_settings;
mod console_snapshot;
mod console_vca;
mod container;
mod container_cmd;
mod container_io;
mod container_move;
pub mod controller;
mod curves;
mod curves_cmd;
mod device_patch;
mod drums;
mod drums_cmd;
mod expand;
pub mod freeze;
pub mod groove;
mod groove_cmd;
mod history;
mod history_size;
mod labels;
mod lane_cmd;
mod library_io;
mod loop_convert;
mod macros;
mod macros_io;
mod midi_import;
mod midi_map;
mod midi_map_io;
mod midi_out;
pub mod note_tools;
pub mod organise;
mod organise_cmd;
mod param_search;
mod patch;
mod plugin;
mod plugin_cmd;
mod plugin_io;
mod project;
mod rack_columns;
mod rate_convert;
mod read_limit;
mod rec_source;
mod record_over;
mod relink;
mod replace_device;
mod rules;
pub mod slice_edit;
mod slices;
mod song_cmd;
mod song_info;
pub mod stems;
mod steps;
mod steps_cmd;
pub mod stress;
pub mod takes;
mod takes_cmd;
pub mod theory;
mod time_edit;
mod validate;
pub mod warp;
mod workflow;
mod workflow_cmd;
mod workflow_create;
mod zones;

pub use patch::{
    MAX_OUTPUT_CHANNELS, PATCH_FORMAT, PATCH_VERSION, SAMPLE_RATES, SAVE_LENGTH_BEATS,
};
pub use rec_source::{RecSource, rec_source, source_label, source_options, source_refusal};

pub mod console;
pub mod song;

pub use container::{
    BOUNDARY_KEY, CONTAINER_TYPE, ContainerCommand, ContainerDoc, FORBIDDEN_INSIDE, MACRO_COUNT,
    MAX_CONTAINER_DEPTH, MAX_CONTAINER_TARGETS, MAX_INNER_DEVICES, MAX_MACRO_NAME_CHARS,
    MAX_MACRO_TARGETS, MAX_PROJECT_INNER, MAX_SPLITS, MacroDoc, MacroTarget, NoteSplit, SPLIT_TYPE,
    boundary_sockets, scoped_cable, split_path,
};
pub use curves::{CURVE_VALUE_MAX, CurveDoc, CurvePatternDoc, CurveStepDoc};
pub use device_patch::MAX_PATCH_NAME_CHARS;
pub use drums::{DrumDoc, DrumLaneDoc, DrumPatternDoc, DrumStepDoc};
pub use expand::{
    BYPASS_PARAM, ExpandedDevice, ExpandedRack, ResolvedTarget, expand_macro_lane, macro_value,
};
pub use freeze::{
    FREEZE_SUFFIX, Frozen, TrackChain, clips_span, freeze_key, freeze_tap, frozen_devices,
    ticks_for_frames, track_chain,
};
pub use lane_cmd::ExtractMode;
pub use loop_convert::LoopConvert;
pub use macros::{
    MACRO_BANK_TYPE, MAX_MACRO_ASSIGNS, MacroAssign, MacroCommand, MacroName, MacroSet,
    assign_position, assign_value, expand_bank_lane, macro_index_of, macro_key,
};
pub use midi_import::{ImportedTrack, MidiImport};
pub use midi_map::{
    DEFAULT_SENSITIVITY, Effect, Learner, MAX_MIDI_MAPS, MAX_PORT_NAME_CHARS, MapMode, MapOptions,
    MapState, MidiMapCommand, MidiMapping, MidiPreset, MidiSource, MidiTarget, PAIR_WINDOW_SECONDS,
    PICKUP_WINDOW, PresetMap, PresetTarget, RelativeKind, SourceKind, TransportTarget, note_name,
};
pub use midi_map_io::{MIDI_PRESET_FORMAT, load_midi_preset, save_midi_preset};
pub use midi_out::{MAX_MIDI_OUT_PORT_CHARS, MIDI_OUT_TYPE, MidiOutDoc};
pub use param_search::{MAX_RESULTS, ParamEntry, ParamIndex, SearchTarget, unit_label};
pub use plugin::{
    MAX_LAYOUT_PARAMS, MAX_PINNED_PARAMS, MAX_PLUGIN_STATE_BYTES, MAX_PROJECT_PLUGIN_STATE_BYTES,
    PLUGIN_GESTURE_MERGE_BASE, PluginCommand, PluginDoc, register_stand_ins,
};
pub use project::{PROJECT_FORMAT, PROJECT_VERSION};
pub use rack_columns::{MAX_RACK_COLUMNS, RackSlot, ResolvedSlot};
pub use read_limit::{
    MAX_DOCUMENT_BYTES, MAX_DOCUMENT_ENV, document_size_limit, read_document_text,
    read_text_limited,
};
pub use record_over::{covering_note_clip, layered_clip, merge_laps};
pub use replace_device::{lost_lanes as replace_lost_lanes, replace_reason};
pub use slices::{
    MAX_SLICES, SLICE_KEY_BASE, default_loop_beats, grid_markers, markers_of, slices_from_markers,
    to_track_notes,
};
pub use song_info::{
    MAX_INFO_AUTHOR_CHARS, MAX_INFO_EMAIL_CHARS, MAX_INFO_NOTES_CHARS, MAX_INFO_TITLE_CHARS,
    MAX_INFO_URL_CHARS, SongInfo,
};
pub use stems::{StemKind, StemSource, safe_file_name, stem_file_names, stem_sources};
pub use steps::{
    MAX_NAME_CHARS, MAX_PATTERNS, MAX_STEPS, PatternDoc, SeqResolution, StepDoc, StepSeqDoc,
    player_data_for,
};
pub use workflow::{
    ClipEntry, ClipSnapshot, DeviceSnapshot, NoteSnapshot, PointSnapshot, neighbour_clip,
    neighbour_device, neighbour_track,
};
pub use workflow_cmd::{ClipMove, WorkflowCommand};
pub use workflow_create::{
    CreateOptions, CreateSource, CreateTarget, RouteNote, SEND_SLOTS_FULL, effect_route_note,
    first_free_send_slot, is_note_player,
};
pub use zones::{
    PAD_COUNT, PAD_KEY_BASE, ZoneDoc, auto_map, fill_empty_pads, pad_key, pad_of_key, pad_zones,
    parse_note_name, with_pad_files, zones_for,
};

use atomik_device_sdk::Registry;
pub use audio_edit::{
    StripParams, normalise_gain_db, peak, slice_envelope, split_envelope, strip_silence,
};
pub use audio_notes::{FoundNote, NoteOptions, notes_clip};
pub use audition::{AuditionSpan, audition_span};
pub use automation_rec::{ThinLevel, WriteMode};
pub use automation_view::{AutomationCurves, ShownValues};
pub use clip_io::{CLIP_FORMAT, CLIP_VERSION, ClipFile, load_clip_file, save_clip_file};
pub use console::{
    Console, ConsoleBus, ConsoleChannel, ConsoleCommand, FX_SLOTS, INSERT_SLOTS, InsertEffect,
    MAX_VCA_NAME_CHARS, MAX_VCAS, VCA_PALETTE, VcaFeed, VcaGroup, clamp_vca_db,
};
pub use console::{LinkKind, OutputDest, link_kind, output_dest};
pub use console_insert::{
    bypass_inserts_commands, insert_bypass_states, insert_chain, inserts_bypassed,
};
pub use console_settings::{ChannelSettingsClip, InsertClip};
pub use console_snapshot::{
    CompareState, MAX_SNAPSHOT_NAME_CHARS, MAX_SNAPSHOTS, MixerSnapshot, MixerState, SnapshotScope,
    SnapshotVca,
};
pub use controller::{ControllerKind, ControllerLane};
pub use groove::{Groove, GrooveSlot, QuantizeParams};
pub use history::InvariantError;
use history::{Edit, song_end_growth};
pub use labels::DEFAULT_LABEL;
use labels::Labels;
pub use library_io::{ItemKind, ItemSummary, load_device_item, save_device_item, summarize_item};
pub use note_tools::NoteTool;
pub use organise::{
    MAX_GROUP_DEPTH, MAX_GROUP_NAME, MAX_GROUPS, MAX_TRACK_COMMENT, Organisation, OutlineEntry,
    OutlineKind, TrackGroup, TrackPlace,
};
use rack_columns::Placement;
pub use slice_edit::SliceError;
pub use song::envelope_gain;
pub use song::{
    AudioClipContent, AudioRegion, AutomationPoint, Clip, ClipContent, ClipId, Curve,
    DEFAULT_TRACK_HEIGHT, FadeCurve, GainPoint, Lane, LaneKind, MAX_CLIP_NAME_CHARS,
    MAX_GAIN_POINTS, MAX_LANE_NAME_CHARS, MAX_MARKER_NAME, MAX_NOTE_LANES, MAX_TICK,
    MAX_TRACK_HEIGHT, MIN_TRACK_HEIGHT, Marker, MetaChange, MeterEvent, Note, NoteList,
    RangeMarker, Song, SongCommand, StepGrid, StepResolution, TempoEvent, Track, clip_name_ok,
    empty_song,
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;
pub use takes::{CompSegment, TakeGroup};
pub use validate::Violation;
pub use warp::{
    AudioQuantize, MAX_WARP_MARKERS, WARP_BPM_MAX, WARP_BPM_MIN, WARP_TRANSPOSE_MAX_CENTS, WarpJob,
    WarpMarker, WarpMode, WarpSettings, bpm_for_beats, estimate_bpm, quantize_audio,
    source_at_tick, tick_at_source, warp_job, warp_jobs, warped_at_song_tempo,
};

/// Most host input channels a document may ask for (the engine interface's limit).
pub const MAX_INPUT_CHANNELS: u16 = 32;

/// Longest display name of a pool asset, in characters.
pub const MAX_ASSET_NAME: usize = 200;

/// What the sample pool stores about an asset. Assets are immutable and identified by
/// their content hash (`atomik-audio-file`: 64 lowercase hex digits).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetMeta {
    /// Channel count (at least 1).
    pub channels: u16,
    /// Sample rate of the stored audio in Hz (at least 1).
    pub sample_rate: u32,
    /// Length in frames (1..=[`song::MAX_AUDIO_FRAMES`]).
    pub frames: u64,
    /// Display name: the imported file's name, or the name given to a recording.
    pub original_name: String,
}

/// True for 64 lowercase hex digits, the only form accepted as an asset id. A copy of
/// `atomik_audio_file::is_valid_hash`, so the model stays dependency-free (and I/O-free).
pub fn is_valid_asset_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Stable, unique device key within a document (e.g. `"synth"`, `"delay-2"`); matches
/// `[a-z][a-z0-9_-]*`. `"interface"` is reserved for the audio interface.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeviceKey(pub String);

/// The reserved key of the audio interface pseudo-device.
pub const INTERFACE_KEY: &str = "interface";

/// A socket on a device, by key.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SocketRef {
    /// Device key (`"interface"` for the audio interface).
    pub device: DeviceKey,
    /// Socket key from the device descriptor.
    pub socket: String,
}

/// A cable from an output socket to an input socket.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Cable {
    /// Output end.
    pub from: SocketRef,
    /// Input end (at most one cable per input).
    pub to: SocketRef,
}

/// One device in the rack.
#[derive(Debug, Clone, PartialEq)]
pub struct RackDevice {
    /// Unique key.
    pub key: DeviceKey,
    /// Device type id (e.g. `"atomik.poly-synth"`).
    pub type_id: String,
    /// User-visible name (defaults to the descriptor name, made unique).
    pub name: String,
    /// Param values by key (plain values, clamped). Missing keys mean the descriptor default.
    pub params: BTreeMap<String, f32>,
    /// Folded (collapsed) in the rack view.
    pub folded: bool,
    /// Optional user accent colour (sRGB).
    pub color: Option<[u8; 3]>,
    /// Sampler zones; empty for every device that does not take samples.
    pub zones: Vec<ZoneDoc>,
    /// Step sequencer patterns; `None` for every device that takes no
    /// pattern data. A new sequencer starts with [`StepSeqDoc::default`].
    pub steps: Option<StepSeqDoc>,
    /// CV sequencer curves; `None` for every device whose type takes no curves
    /// (`Registry::takes_curves`) and, on one that does, for flat zero curves (a new CV
    /// sequencer starts with none). Pattern `i` of the curves goes with pattern `i` of
    /// [`RackDevice::steps`].
    pub curves: Option<CurveDoc>,
    /// Drum generator patterns; `Some` exactly on devices whose type takes drum data
    /// (`Registry::takes_drums`), `None` on every other device. A new generator starts with
    /// [`DrumDoc::default`]; such a device has no [`RackDevice::steps`].
    pub drums: Option<DrumDoc>,
    /// A container's inside: `Some` exactly on devices whose type is a
    /// `DeviceKind::Container`, `None` on every other device. See [`ContainerDoc`].
    pub inner: Option<Box<ContainerDoc>>,
    /// A plugin device's document: `Some` exactly on devices whose type id
    /// is a plugin type (`clap.<id>`, `vst3.<hex>`), `None` on every other device. See
    /// [`PluginDoc`].
    pub plugin: Option<Box<PluginDoc>>,
    /// The rack column the device is shown in (top level only; always 0 for a device inside a
    /// container). See [`RackDoc::columns`] for the ordering invariant.
    pub column: u16,
    /// The name of the patch the device last took its sound from (the patch strip shows it);
    /// empty when it has none. Display only: it plays nothing, and it is stored only when
    /// non-empty (project format 36).
    pub patch_name: String,
    /// A MIDI Out device's destination port: `Some` once the user chose one, `None` while the
    /// device uses the fallback port of the app settings; always `None` on every other device
    /// type. Stored only when `Some` (project format 37). See [`MidiOutDoc`].
    pub midi_out: Option<MidiOutDoc>,
}

/// The rack document.
#[derive(Debug, Clone, PartialEq)]
pub struct RackDoc {
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Tempo in BPM.
    pub tempo_bpm: f64,
    /// Engine seed.
    pub seed: u64,
    /// Audio interface output channels.
    pub output_channels: u16,
    /// Audio interface input channels: the host inputs `interface:in_1..in_N`. 2
    /// for new documents, 0 for files that predate inputs.
    pub input_channels: u16,
    /// The sample pool: audio assets by id. Holds the assets the song
    /// references and those kept for undo; see [`RackDoc::prune_unreferenced_assets`].
    pub assets: BTreeMap<String, AssetMeta>,
    /// Devices in rack order (top to bottom, column 0 first). **Invariant**, kept by every
    /// command and loader and checked by [`RackDoc::check_columns`]: the list is stable-sorted
    /// by [`RackDevice::column`], so each column is a contiguous run of this list. Signal and
    /// graph order never depend on columns.
    pub devices: Vec<RackDevice>,
    /// Number of rack columns shown side by side (at least 1, at most [`MAX_RACK_COLUMNS`]);
    /// empty columns count. Every device's column is below it.
    pub columns: u16,
    /// Cables, in canonical order (sorted).
    pub cables: Vec<Cable>,
    /// The song (tracks, clips, tempo map, loop) . Invariant, kept by every command
    /// and loader: `song.tempo[0].bpm == tempo_bpm`. Shared between copies of the document:
    /// read it through `Deref`, write it through [`RackDoc::song_mut`] (the first write of a
    /// shared song copies its structure, never its notes).
    pub song: Arc<Song>,
    /// The console (main mixer) section.
    pub console: Console,
    /// The MIDI mappings (hardware controls bound to params, the tempo and the transport).
    /// Project data with no part in the sound: see [`crate::midi_map`].
    pub midi_maps: Vec<MidiMapping>,
    /// The macro assignments and names (see [`crate::macros`]). Project data: a driven param is
    /// set to the macro's value wherever the rack is expanded.
    pub macros: MacroSet,
    /// What the user wrote about the song: title, author, notes, address and e-mail (empty for a
    /// new project). Project data with no part in the sound: see [`crate::song_info`].
    pub info: SongInfo,
}

/// An edit. Every command is undoable.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Insert a new device of `type_id` at rack `index` (clamped). `key` is generated when `None`.
    ///
    /// When the registry has the console devices, an instrument with an audio output
    /// also gets its console channel (and the master, if missing), auto-wired as
    /// [`ConsoleCommand::AddChannel`] does, in the same undo step.
    AddDevice {
        /// Rack position.
        index: usize,
        /// Device type id.
        type_id: String,
        /// Explicit key, or `None` to generate one.
        key: Option<DeviceKey>,
    },
    /// Like [`Command::AddDevice`], at a rack column slot (see [`RackSlot`]): the device
    /// lands at `slot.index` of column `slot.column` (an empty or too-high index goes to the
    /// end of the column; column 0 never goes above the Master Section). `slot.column ==
    /// columns` appends a new column in the same undo step. Reports `DeviceAdded`,
    /// `RoutingChanged` (and `ViewChanged` when a column is added).
    AddDeviceAt {
        /// Where the device goes.
        slot: RackSlot,
        /// Device type id.
        type_id: String,
        /// Explicit key, or `None` to generate one.
        key: Option<DeviceKey>,
    },
    /// Append an empty rack column (one undo step; refused at [`MAX_RACK_COLUMNS`]).
    /// Reports [`SyncOp::ViewChanged`].
    AddRackColumn,
    /// Remove rack column `column` (one undo step). A non-empty column's devices keep their
    /// place in the rack order and join the end of the column to the left (the start of the
    /// next column when column 0 goes); higher columns shift down by one. The last column
    /// cannot be removed. Reports [`SyncOp::ViewChanged`].
    RemoveRackColumn {
        /// The column to remove.
        column: u16,
    },
    /// Remove a device, every cable attached to it and its song track (one undo step).
    ///
    /// Console bookkeeping is part of the step: a channel leaves the console; an
    /// instrument's channel is removed with it (a non-instrument source leaves its channel
    /// source-less); removing the master clears the master and every return; a return
    /// effect leaves its slot.
    RemoveDevice {
        /// Device to remove.
        key: DeviceKey,
    },
    /// Move a device to rack index `to` (clamped). It takes the column of the device before its
    /// new position (column 0 at the top).
    MoveDevice {
        /// Device to move.
        key: DeviceKey,
        /// New rack position.
        to: usize,
    },
    /// Fold or unfold a device.
    SetFolded {
        /// Device.
        key: DeviceKey,
        /// New state.
        folded: bool,
    },
    /// Rename a device (names need not be unique). The name is trimmed; a blank one restores
    /// the default name of the device's type, made unique among its siblings.
    Rename {
        /// Device.
        key: DeviceKey,
        /// New name.
        name: String,
    },
    /// Set or clear a device's accent colour.
    SetColor {
        /// Device.
        key: DeviceKey,
        /// Colour.
        color: Option<[u8; 3]>,
    },
    /// Connect a cable (validated against descriptors: direction, kind, one cable per input,
    /// no cycles). A summing input (`Direction::SummingInput`) takes any number of cables,
    /// but not the same cable twice. For the cycle check the master is two nodes: its
    /// `fx_send*` outputs (fed by its `send_bus*` inputs only) and the rest.
    Connect(Cable),
    /// Remove the cable plugged into input `to` (every cable, in one step, for a summing
    /// input).
    Disconnect {
        /// Input end.
        to: SocketRef,
    },
    /// Set a param (clamped by the descriptor). Consecutive `SetParam`s with the same
    /// `merge` id on the same device and param merge into one undo step (knob drags).
    SetParam {
        /// Device.
        key: DeviceKey,
        /// Param key.
        param: String,
        /// Plain value.
        value: f32,
        /// Drag-merge group, or `None` for a standalone step.
        merge: Option<u64>,
    },
    /// Set the tempo (20..=999 BPM).
    SetTempo(f64),
    /// A song edit.
    Song(SongCommand),
    /// Add an audio asset to the sample pool. Idempotent: an asset already in the
    /// pool is kept as it is (the first name wins) when `meta` has the same channels, rate
    /// and length, and rejected when it differs. `hash` must be 64 lowercase hex digits.
    ///
    /// This is the one command that is not an undo step: the asset file already exists in the
    /// bundle, the pool only grows (until [`RackDoc::prune_unreferenced_assets`] on save), so
    /// undoing the edits that use an asset never loses it. It returns no [`SyncOp`], leaves
    /// the redo history alone and does not end a drag-merge group.
    RegisterAsset {
        /// Asset id.
        hash: String,
        /// Format of the stored audio.
        meta: AssetMeta,
    },
    /// Swap every reference to the pool asset `from` for the pool asset `to` (one undo step):
    /// the audio clips and the zones of samplers, slicers and drum machines, inside containers
    /// too. This is how a missing audio file is relinked: the app imports the replacement
    /// file, registers it ([`Command::RegisterAsset`]) and issues this. Both ids must be in the
    /// pool, differ, and `from` must be used by a clip or a zone. Zone ranges and clip source
    /// offsets are kept, so a replacement shorter than they need is rejected
    /// ([`ModelError::BadZones`] or [`ModelError::BadSong`]). The old pool entry stays (undo and
    /// redo need it; a save drops it once the history no longer references it). Reports
    /// [`SyncOp::SongChanged`] once if clips changed and [`SyncOp::ZonesChanged`] per device.
    /// Not accepted by [`Command::Batch`].
    RelinkAsset {
        /// The asset to replace (usually one whose file is missing).
        from: String,
        /// The asset that takes its place.
        to: String,
    },
    /// Renames a pool asset (one undo step): the name the file browser, the clip editor and the
    /// clip labels show. The name is trimmed and must be 1..=[`MAX_ASSET_NAME`] characters
    /// without control characters; a name equal to the current one is rejected. Reports
    /// [`SyncOp::ViewChanged`]. Not accepted by [`Command::Batch`].
    RenameAsset {
        /// The asset id.
        asset: String,
        /// The new display name.
        name: String,
    },
    /// Set the interface's input channel count (0..=[`MAX_INPUT_CHANNELS`]).
    /// Cables from inputs above the new count are removed in the same step. Reports
    /// [`SyncOp::RoutingChanged`] (the app rebuilds the interface).
    SetInputChannels(u16),
    /// Set the interface's output channel count (1..=[`MAX_OUTPUT_CHANNELS`]). Cables into
    /// outputs above the new count are removed in the same step. Reports
    /// [`SyncOp::RoutingChanged`] (the app rebuilds the interface).
    SetOutputChannels(u16),
    /// Sets the song information (title, author, notes, address, e-mail), one undo step. Every
    /// text must be within its limit and hold no control character (see [`SongInfo::check`]); an
    /// info equal to the current one is rejected. Reports [`SyncOp::ViewChanged`]. Not accepted
    /// by [`Command::Batch`].
    SetSongInfo(SongInfo),
    /// Moves the project to another sample rate (one of [`SAMPLE_RATES`], not the current one)
    /// in one undo step. The app has already resampled the assets the song uses and registered
    /// the copies ([`Command::RegisterAsset`]); `assets` pairs each used asset id with its copy,
    /// which must have the new rate and the same channel count. Every clip, take, frozen render
    /// and zone is pointed at the copy and its frame positions (source offsets, fades, gain
    /// envelope points, warp marker sources, zone ranges) are scaled by the rate ratio; ticks do
    /// not change. Reports [`SyncOp::RoutingChanged`] and [`SyncOp::SongChanged`] (the app
    /// rebuilds the engine at the new rate), then [`SyncOp::ZonesChanged`] per device with
    /// zones. Not accepted by [`Command::Batch`].
    ConvertSampleRate {
        /// The new rate in Hz.
        rate: u32,
        /// `(used asset, its copy at the new rate)`.
        assets: Vec<(String, String)>,
    },
    /// A console edit.
    Console(ConsoleCommand),
    /// A MIDI mapping edit (see [`MidiMapCommand`]); reports [`SyncOp::MidiMapsChanged`].
    MidiMap(MidiMapCommand),
    /// A macro edit (see [`MacroCommand`]); reports [`SyncOp::MacrosChanged`] for each bank it
    /// touches. Not accepted by [`Command::Batch`].
    Macro(MacroCommand),
    /// Several commands as one undo step, validated in order (each against the
    /// document as the earlier ones left it) and rejected as a whole if any fails, if the
    /// batch is empty or if it holds anything else. **Accepted:** `Connect`, `Disconnect`
    /// (cables; used by the audio-track input selector) and, later,
    /// `Command::Song(SongCommand::SetClipGrid { .. })` and
    /// `Command::Song(SongCommand::SetClipGeometry { .. })` (the step editor sets a grid and
    /// the clip length as one step) and, later,
    /// `Command::Song(SongCommand::AddTrack { .. })` and `Command::Song(SongCommand::AddClip { .. })`
    /// (a new pattern on a device without a track is one step) and, later,
    /// `Command::Song(SongCommand::SetTrackFlags { .. })` (arming one track and disarming
    /// another is one step) and, later,
    /// `Command::Song(SongCommand::SetTrackColor { .. })` and
    /// `Command::Song(SongCommand::SetLaneFolded { .. })` (colouring or folding several at once is
    /// one step) and, later,
    /// `Command::SetParam { .. }` and `Command::SetZones { .. }` (loading a loop into a slicer
    /// sets `loop.beats` and the slices as one step). The batch's last allocated
    /// clip id is [`Document::last_clip_id`] afterwards. A `merge` id inside a batched command
    /// is ignored: a batch never joins a drag-merge group. Reports each kind of change once,
    /// in the order of the compound rules of [`SyncOp`]: [`SyncOp::RoutingChanged`] for cables,
    /// [`SyncOp::SongChanged`] for clip edits, [`SyncOp::ParamChanged`] for each param set and
    /// one [`SyncOp::ZonesChanged`] per device whose zones are set.
    Batch(Vec<Command>),
    /// A container edit: inner devices, note splits, macros, duplicate. Each
    /// is one undo step; `SetSplits`, `SetMacro`, `RemoveMacroTarget` and `RenameMacro` are also
    /// accepted by [`Command::Batch`]. Inner devices are addressed by path keys in the existing
    /// commands too (see [`RackDoc::resolve`]).
    Container(ContainerCommand),
    /// A plugin edit: add a plugin device, pins, run mode, power, relink
    /// (each one undo step; `SetPinned`, `SetPower` and `Relink` are also accepted by
    /// [`Command::Batch`]), and the data syncs `SyncState` and `SyncLayout`, which are **not**
    /// undo steps (like `RegisterAsset`: no sync op, redo kept, drag group kept). A plugin's own
    /// params are ordinary [`Command::SetParam`]s on the keys `p<id>`; a gesture in the plugin
    /// window is one undo step by giving its `SetParam`s one merge id from
    /// [`PLUGIN_GESTURE_MERGE_BASE`].
    Plugin(PluginCommand),
    /// Replace a sampler's whole zone list (one undo step). The device must take
    /// samples (`Registry::takes_samples`); every zone's asset must be in the pool and every
    /// range valid as the SDK's `SampleMap::new` demands, with frames within the asset (see
    /// [`ZoneDoc`]). A rejected list changes nothing. Consecutive `SetZones` with the same
    /// `merge` id on the same device merge into one undo step (zone drags). Reports
    /// [`SyncOp::ZonesChanged`].
    SetZones {
        /// The sampler.
        device: DeviceKey,
        /// The new zone list.
        zones: Vec<ZoneDoc>,
        /// Drag-merge group, or `None` for a standalone step.
        merge: Option<u64>,
    },
    /// Replace pattern `index` (0-based) of a step sequencer (one undo step). The
    /// device must take pattern data (`Registry::takes_player_data`) and the pattern must be
    /// valid (see [`PatternDoc::check`]); a rejected pattern changes nothing. Consecutive
    /// `SetPattern`s with the same `merge` id on the same device and pattern merge into one
    /// undo step (drags). Merge ids of the sequencer editor start at 6 * 2^40. Reports
    /// [`SyncOp::PatternsChanged`].
    SetPattern {
        /// The sequencer.
        device: DeviceKey,
        /// Pattern index, below the pattern count.
        index: u8,
        /// The new pattern.
        pattern: PatternDoc,
        /// Drag-merge group, or `None` for a standalone step.
        merge: Option<u64>,
    },
    /// Replace the curve rows of pattern `index` (0-based, below the device's pattern count) of
    /// a CV sequencer (one undo step); the curves of lower patterns the document does not hold
    /// yet are filled in flat. The device must take curves (`Registry::takes_curves`) and the
    /// rows must be valid (see [`CurvePatternDoc::check`]); a rejected command changes nothing.
    /// Consecutive `SetCurves` with the same `merge` id on the same device and pattern merge
    /// into one undo step (drags; the sequencer editor uses the ids it uses for
    /// `SetPattern`). Reports [`SyncOp::PatternsChanged`].
    SetCurves {
        /// The CV sequencer.
        device: DeviceKey,
        /// Pattern index, below the pattern count.
        index: u8,
        /// The new rows.
        curves: CurvePatternDoc,
        /// Drag-merge group, or `None` for a standalone step.
        merge: Option<u64>,
    },
    /// Set one step of one lane of drum pattern `index` of a drum generator (one undo step): a
    /// `velocity` of 0 clears the hit. The device must take drum data (`Registry::takes_drums`);
    /// a rejected command changes nothing. Consecutive `SetDrumStep` and `SetDrumPattern` with the
    /// same `merge` id on the same device and pattern merge into one undo step (velocity drags).
    /// Reports [`SyncOp::PatternsChanged`].
    SetDrumStep {
        /// The drum generator.
        device: DeviceKey,
        /// Pattern index, below the pattern count.
        index: u8,
        /// Lane, 0..16.
        lane: u8,
        /// Step, 0..64.
        step: u8,
        /// Velocity, 0..=127 (0 = no hit).
        velocity: u8,
        /// Probability in percent, 0..=100.
        prob: u8,
        /// Drag-merge group, or `None` for a standalone step.
        merge: Option<u64>,
    },
    /// Replace the key, role and mute of one lane of drum pattern `index` (one undo step).
    /// Reports [`SyncOp::PatternsChanged`].
    SetDrumLane {
        /// The drum generator.
        device: DeviceKey,
        /// Pattern index, below the pattern count.
        index: u8,
        /// Lane, 0..16.
        lane: u8,
        /// The new key, role and mute.
        setting: DrumLaneDoc,
    },
    /// Replace drum pattern `index` as a whole (paste, clear, rename; one undo step). The pattern
    /// must be valid (see [`DrumPatternDoc::check`]). Reports [`SyncOp::PatternsChanged`].
    SetDrumPattern {
        /// The drum generator.
        device: DeviceKey,
        /// Pattern index, below the pattern count.
        index: u8,
        /// The new pattern.
        pattern: DrumPatternDoc,
        /// Drag-merge group, or `None` for a standalone step.
        merge: Option<u64>,
    },
    /// Append an empty 16-step 1/16 pattern (`Pattern N`) to a drum generator (one undo step);
    /// refused at [`MAX_PATTERNS`]. Reports [`SyncOp::PatternsChanged`].
    AddDrumPattern {
        /// The drum generator.
        device: DeviceKey,
    },
    /// Append a copy of drum pattern `index` (one undo step); refused at [`MAX_PATTERNS`].
    /// Reports [`SyncOp::PatternsChanged`].
    DuplicateDrumPattern {
        /// The drum generator.
        device: DeviceKey,
        /// The pattern to copy.
        index: u8,
    },
    /// Remove drum pattern `index` (one undo step); the last pattern cannot be removed, and the
    /// `pattern` param is lowered when it would point past the end. Reports
    /// [`SyncOp::PatternsChanged`].
    RemoveDrumPattern {
        /// The drum generator.
        device: DeviceKey,
        /// The pattern to remove.
        index: u8,
    },
    /// Append an empty 16-step 1/16 pattern (`Pattern N`) to a sequencer (one
    /// undo step); refused at [`MAX_PATTERNS`]. Reports [`SyncOp::PatternsChanged`].
    AddPattern {
        /// The sequencer.
        device: DeviceKey,
    },
    /// Remove pattern `index` (one undo step); the later patterns move up. The
    /// last pattern cannot be removed. When the device's `pattern` param would point past the
    /// new count it is lowered to the last pattern in the same step (a `ParamChanged` before
    /// the `PatternsChanged`). Reports [`SyncOp::PatternsChanged`].
    RemovePattern {
        /// The sequencer.
        device: DeviceKey,
        /// Pattern index.
        index: u8,
    },
    /// Append a copy of pattern `index` (named `<name> copy`, cut to
    /// [`MAX_NAME_CHARS`]) at the end, so the numbers of the existing patterns stay as they
    /// are; refused at [`MAX_PATTERNS`] (one undo step). Reports [`SyncOp::PatternsChanged`].
    DuplicatePattern {
        /// The sequencer.
        device: DeviceKey,
        /// Pattern to copy.
        index: u8,
    },
    /// A workflow edit: multi-device remove, duplicate, paste, move and
    /// disconnect, duplicate track, and the clip, note and point pastes and duplicates. Each is
    /// one undo step, planned on a scratch copy; a rejected command changes nothing. Not
    /// accepted by [`Command::Batch`].
    Workflow(WorkflowCommand),
}

/// What changed, for keeping the engine in sync.
///
/// Process the ops of one apply/undo/redo **in order**; read the current routing and device
/// set from [`Document::doc`] when handling [`SyncOp::RoutingChanged`]. The sequences are:
///
/// | Change | Ops |
/// |---|---|
/// | device added (`AddDevice`, undo of `RemoveDevice`) | `DeviceAdded`, `ParamChanged` for each stored param (none for a new device), `RoutingChanged` |
/// | device removed (`RemoveDevice`, undo of `AddDevice`) | `RoutingChanged`, `DeviceRemoved` |
/// | cable connected or disconnected | `RoutingChanged` |
/// | param set (also undo/redo; on undo to "not stored" the value is the descriptor default) | `ParamChanged` |
/// | tempo (`SetTempo`; also sets `song.tempo[0].bpm`) | `TempoChanged` |
/// | move, fold, rename, colour | `ViewChanged` |
/// | song edit (`Command::Song`, including `SetClipGrid`) | `SongChanged` |
/// | `SetInputChannels`, `SetOutputChannels` | `RoutingChanged` |
/// | `SetSongInfo` (also undo/redo) | `ViewChanged` |
/// | `ConvertSampleRate` | `RoutingChanged`, `SongChanged`, then `ZonesChanged` per device with zones |
/// | `Batch` | `RoutingChanged` (once, if it holds cables), `ParamChanged` per `SetParam`, `ZonesChanged` (once per device, if it holds `SetZones`), `SongChanged` (once, if it holds track or clip edits, audio clip edits included) |
/// | `RegisterAsset` | none (not an undo step) |
/// | `RelinkAsset` | `ZonesChanged` per device with a swapped zone, then `SongChanged` if clips were swapped |
/// | `RenameAsset` | `ViewChanged` |
/// | tempo map (`SongCommand::SetTempoMap` and `DragTempoMap`, also set `tempo_bpm`) | `TempoChanged(first bpm)`, `SongChanged` |
/// | meter (`SetMeter`, `SetMeterMap`, `DragMeterMap`) | `SongChanged` |
/// | markers and ranges (`SetMarkers`, `DragMarkers`, `SetRanges`, `DragRanges`; also undo/redo) | `ViewChanged` |
/// | quantize (`ApplyQuantize`, `QuantizeNotes`) | `SongChanged` |
/// | note tools (`ApplyNoteTool`) | `SongChanged` |
/// | the groove library (`SetGroove`, `DeleteGroove`, `ExtractGroove`; also undo/redo) | `ViewChanged` |
/// | track heights (`SetTrackHeights`, `DragTrackHeights`; also undo/redo) | `ViewChanged` |
/// | track organisation (`GroupTracks`, `Ungroup`, `RenameGroup`, `SetGroupCollapsed`, `SetGroupColor`, `MoveTrack`, `MoveTracks`, `MoveGroup`, `SetTrackColor`, `SetTrackComment`, `SetLaneFolded`, `SetTakesOpen`; also undo/redo) | `ViewChanged`, or `SongChanged` when tracks changed place |
/// | `RemoveDevice` of a device with a track | `SongChanged`, `RoutingChanged`, `DeviceRemoved` |
/// | console structure, compensation, a channel's `solo` | `ConsoleChanged` (after the `ParamChanged` for `solo`) |
/// | `SetZones` (also undo/redo) | `ZonesChanged` |
/// | inner device added (`AddInner`) | `DeviceAdded(path)`, `RoutingChanged` (`AddInner` with `auto_split` also `SplitsChanged`) |
/// | container removed, added back (undo), duplicated, or added with `AddDevice` | `DeviceRemoved(container)` or `DeviceAdded(container)` for the **whole inside** (the app expands it; no per-inner ops) |
/// | `SetSplits` | `SplitsChanged`, plus `RoutingChanged` only when the list of targets changed |
/// | `SetMacro`, `RemoveMacroTarget`, `RenameMacro` (also undo/redo, and the clean-up when a mapped device is removed) | `MacrosChanged` |
/// | plugin added (`PluginCommand::Add`, also inside a container; `PluginCommand::AddAt` also reports `ViewChanged` when it adds a column) | as for `AddDevice` / `AddInner` (`DeviceAdded`, `RoutingChanged`, `ConsoleChanged` for an instrument's channel); the app creates the real plugin from [`RackDevice::plugin`] |
/// | `SetPinned`, `SetRunMode`, `SetPower`, `Relink` (also undo/redo, also in a `Batch`, once per device) | `PluginChanged` |
/// | `SyncState`, `SyncLayout` | none (not undo steps) |
/// | `ApplyPatch`, `ResetDevice` (also undo/redo) | `ParamChanged` per changed param, `ZonesChanged`, `PatternsChanged`, `PluginStateChanged`, `ViewChanged` (the patch name); a container's inside: `DeviceRemoved` and `DeviceAdded` for the container |
/// | `SetPatchName` (also undo/redo) | `ViewChanged` |
/// | `SetMidiOutPort` (also undo/redo) | `ViewChanged` (the app re-routes the device's output) |
/// | `Workflow` (paste, duplicate, remove, move, disconnect devices; clip, note and point pastes) | the compound order below: `DeviceAdded` with stored params, `RoutingChanged`, `ViewChanged`, `ConsoleChanged`, `SongChanged`, then the removals; a move reports `ViewChanged` only |
///
/// A step with several parts (e.g. `AddDevice` of an instrument with its channel and
/// master, `RemoveDevice` with its channel, `AddChannel`) reports each kind once, in this
/// order: `DeviceAdded`s (each with its `ParamChanged`s), `RoutingChanged` (when nothing is
/// removed), other `ParamChanged`/`TempoChanged`, `ViewChanged`, `ConsoleChanged`,
/// `SongChanged`, then (when something is removed) `RoutingChanged` and the
/// `DeviceRemoved`s.
/// | undo of that removal | `DeviceAdded`, `ParamChanged`…, `RoutingChanged`, `SongChanged` |
///
/// The song never references a device the engine does not have: `SongChanged` comes before
/// `DeviceRemoved` and after `DeviceAdded`. A `TempoChanged(bpm)` alone also means the song's
/// first tempo event became `bpm`.
#[derive(Debug, Clone, PartialEq)]
pub enum SyncOp {
    /// A device was added; the app creates and prepares an instance.
    DeviceAdded(DeviceKey),
    /// A device was removed; the app removes its instance after the routing update.
    DeviceRemoved(DeviceKey),
    /// Cables (or device set) changed; the app submits new routing.
    RoutingChanged,
    /// A param changed; the app sends it through the engine handle.
    ParamChanged {
        /// Device.
        key: DeviceKey,
        /// Param key.
        param: String,
        /// New plain value (clamped).
        value: f32,
    },
    /// Tempo changed.
    TempoChanged(f64),
    /// Only presentation changed (fold, name, colour, order); no engine work.
    ViewChanged,
    /// The song changed (tracks, clips, tempo map, meter, loop); the app recompiles and
    /// submits the engine song snapshot.
    SongChanged,
    /// A track was frozen or unfrozen (see [`crate::freeze`]): the engine graph changes (the
    /// frozen devices leave it and a player takes their place), so the app restarts its
    /// engine. [`SyncOp::SongChanged`] comes with it.
    FrozenChanged,
    /// The console changed: channels, their order, the master, returns or
    /// compensation, or a console channel's `solo` param. The app re-reads
    /// [`RackDoc::console`], refreshes the console view, pushes
    /// [`RackDoc::solo_silence`] to the channels' `internal.solo_silence` params and the
    /// compensation switch to the engine.
    ConsoleChanged,
    /// The MIDI mappings changed (any mapping command, its undo and redo, a project load). The
    /// app rebuilds its mapping table; nothing reaches the engine.
    MidiMapsChanged,
    /// A sampler's zone list changed (`SetZones`, its undo and redo; not reported
    /// when a whole device comes back or goes away, the app builds its map on insertion). The
    /// app rebuilds the device's sample map from [`RackDevice::zones`].
    ZonesChanged(DeviceKey),
    /// A step or CV sequencer's patterns or curves changed (the pattern commands and
    /// `SetCurves`, their undo and redo, also inside a `Batch`; not reported when a whole device comes back or goes
    /// away, the app builds its data on insertion). The app rebuilds the device's pattern data
    /// with [`player_data_for`] and sends it to the engine.
    PatternsChanged {
        /// The sequencer.
        device: DeviceKey,
    },
    /// A container's macro mapping or name changed (`SetMacro`,
    /// `RemoveMacroTarget`, `RenameMacro`, the clean-up when a mapped device is removed, their
    /// undo and redo). The app re-sends every mapped target and the container's macro values
    /// ([`ExpandedRack::macro_updates`]).
    MacrosChanged {
        /// The container (path key).
        container: DeviceKey,
    },
    /// A plugin device's pins, run mode, power or link changed (`SetPinned`,
    /// `SetRunMode`, `SetPower`, `Relink`, their undo and redo). The app re-reads
    /// [`RackDevice::plugin`] and updates the plugin manager (pins are view-only; run mode and
    /// power reach the live device; after a `Relink` the device's type id may have changed, so
    /// the app swaps the instance for the new plugin or its stand-in).
    PluginChanged {
        /// The plugin device (path key).
        device: DeviceKey,
    },
    /// A plugin device's state blob changed (a patch applied to it, its undo and redo; not
    /// the captures the app stores itself). The app restores [`RackDevice::plugin`]'s state into
    /// the live instance.
    PluginStateChanged {
        /// The plugin device (path key).
        device: DeviceKey,
    },
    /// A container's split list changed (`SetSplits`, the automatic split of
    /// `AddInner`, the clean-up when a split target is removed, their undo and redo). The app
    /// re-sends the params of the container's `~split` device. A change of the *targets* also
    /// reports [`SyncOp::RoutingChanged`].
    SplitsChanged {
        /// The container (path key).
        container: DeviceKey,
    },
}

/// Model errors.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ModelError {
    /// Unknown device key.
    #[error("unknown device `{0}`")]
    UnknownDevice(String),
    /// Unknown device type.
    #[error("unknown device type `{0}`")]
    UnknownType(String),
    /// Unknown socket or param key.
    #[error("unknown key `{0}`")]
    UnknownKey(String),
    /// Key already used or invalid.
    #[error("invalid or duplicate device key `{0}`")]
    BadKey(String),
    /// Cable rejected (direction, kind, input already connected, cycle).
    #[error("cable rejected: {0}")]
    BadCable(String),
    /// Value out of range (e.g. tempo).
    #[error("value out of range: {0}")]
    OutOfRange(String),
    /// Song edit rejected (missing track, wrong lane kind, invalid clip data, bad asset id...).
    #[error("song: {0}")]
    BadSong(String),
    /// Console edit rejected (not a channel, no master, source already has a channel…).
    #[error("console: {0}")]
    BadConsole(String),
    /// MIDI mapping edit rejected (bad source or options, a target that does not exist, no such
    /// mapping, too many mappings).
    #[error("MIDI mapping: {0}")]
    BadMidiMap(String),
    /// Zone edit rejected (device takes no samples, asset not in the pool, bad range...).
    #[error("zones: {0}")]
    BadZones(String),
    /// Pattern edit rejected (device takes no pattern data, bad index or pattern, too many or
    /// too few patterns).
    #[error("patterns: {0}")]
    BadPatterns(String),
    /// Rack column edit rejected (no such column, the last column, too many columns).
    #[error("rack column: {0}")]
    BadColumn(String),
    /// Containers nest too deep.
    #[error("{0}")]
    ContainerTooDeep(String),
    /// A container or the project holds too many inner devices.
    #[error("{0}")]
    ContainerFull(String),
    /// A device type that cannot live inside a container (console devices, audio tracks, the
    /// internal note split).
    #[error("{0}")]
    NotAllowedInContainer(String),
    /// Note split rejected (bad target, range or count).
    #[error("split: {0}")]
    BadSplit(String),
    /// Macro edit rejected (bad index, target, name or count).
    #[error("macro: {0}")]
    BadMacro(String),
    /// A param that a macro drives cannot be set by hand.
    #[error("{0}")]
    MacroDriven(String),
    /// The command names something in the wrong scope (not a container, a cable between two
    /// containers).
    #[error("{0}")]
    BadScope(String),
    /// Plugin edit rejected (not a plugin device, unusable plugin layout, state too large, bad
    /// pins, an incompatible relink...).
    #[error("plugin: {0}")]
    BadPlugin(String),
    /// A frozen track, or a device it freezes, was asked to change (see [`crate::freeze`]): the
    /// message names the track and says to unfreeze it.
    #[error("{0}")]
    Frozen(String),
    /// A batch was rejected: it is empty, or holds a command that cannot be batched.
    #[error("batch: {0}")]
    BadBatch(String),
    /// A command names an asset that is not in the project's pool (the hash).
    #[error("asset {0} is not in the pool")]
    MissingAsset(String),
    /// A project, patch or clip file could not be read: `path` is the JSON path of what is wrong
    /// (`$` for the whole file) and `message` says what.
    #[error("file: {path}: {message}")]
    Load {
        /// The JSON path of the fault (`song.tracks[0].lanes[1]`), `$` for the file as a whole.
        path: String,
        /// What is wrong there.
        message: String,
    },
    /// An edit did not fit the document it was applied to (an internal fault: the planner and
    /// the history disagree). The document and its history are unchanged.
    #[error(transparent)]
    Invariant(#[from] InvariantError),
    /// The document breaks one of its own rules (see [`RackDoc::validate`]), or an internal error
    /// stopped the command: the text says which.
    #[error("{0}")]
    Invalid(String),
    /// A fixed part of the rack was asked to go away, move or be copied: `what`
    /// names it (`"master"`, the console master) and `message` says what is refused. The lock is
    /// command-level only: loaders never raise it.
    #[error("{message}")]
    Fixed {
        /// The fixed thing (`"master"`).
        what: &'static str,
        /// The text for the user, e.g. "The Master Section cannot be deleted".
        message: String,
    },
}

/// A document with undo/redo history.
///
/// Every successful [`Document::apply`] records one undo step (or extends the open drag-merge
/// step) and clears the redo history. A rejected command changes nothing.
#[derive(Debug, Clone)]
pub struct Document {
    doc: RackDoc,
    undo: VecDeque<Edit>,
    /// The estimated bytes the undo steps hold (see [`Edit::approx_bytes`]).
    undo_bytes: usize,
    /// The most undo steps and the most estimated bytes the history keeps; see
    /// [`Document::set_undo_limits`].
    undo_limits: (usize, usize),
    /// Set while a trial step is applied: it is not kept yet, so it must not push an older
    /// step out of the history.
    hold_trim: bool,
    redo: Vec<Edit>,
    /// The open drag-merge group of the last applied command.
    merge: Option<MergeKey>,
    /// The clip id allocated by the last `AddClip`, `DuplicateClip` or `SplitClip`.
    last_clip: Option<ClipId>,
    /// Changes whenever [`Document::doc`] may have changed (see [`Document::revision`]).
    revision: u64,
    /// The steps of an open trial, oldest first (see [`Document::apply_trial`]).
    trial: Vec<Edit>,
    /// The redo history an open trial set aside.
    trial_redo: Vec<Edit>,
    /// The names of the steps above, in the same order (see [`Document::undo_label`]).
    labels: Labels,
}

/// The most undo steps a [`Document`] keeps: with more, the oldest are forgotten.
pub const MAX_UNDO_STEPS: usize = 1000;

/// The most memory, estimated in bytes, the undo steps of a [`Document`] may hold (the clips,
/// devices and lists they keep); the oldest steps are forgotten beyond it, but the latest step
/// is always kept. 512 MiB.
pub const MAX_UNDO_BYTES: usize = 512 << 20;

/// The merge key of a batch of `SetParam`s that share one merge id (several params moved by one
/// gesture); `None` for any other batch.
fn gang_key(commands: &[Command]) -> Option<MergeKey> {
    let mut id = None;
    let mut targets = Vec::with_capacity(commands.len());
    for command in commands {
        let Command::SetParam {
            key,
            param,
            merge: Some(m),
            ..
        } = command
        else {
            return None;
        };
        if *id.get_or_insert(*m) != *m {
            return None;
        }
        targets.push((key.clone(), param.clone()));
    }
    Some(MergeKey::Gang(id?, targets))
}

/// The source of document revisions: process wide, so two documents (a replaced one and the
/// one that took its place) never share a revision.
static REVISIONS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn next_revision() -> u64 {
    REVISIONS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// What a drag-merge group is keyed on: consecutive commands with an equal key merge.
#[derive(Debug, Clone, PartialEq)]
enum MergeKey {
    /// `SetParam` on one device param.
    Param(u64, DeviceKey, String),
    /// A `Batch` of `SetParam`s that all carry one merge id (a ganged drag): the same params
    /// again merge, in the order of the batch.
    Gang(u64, Vec<(DeviceKey, String)>),
    /// The clip commands of one gesture (`SetClipGeometry`, `SetClipContent`, ..., and the
    /// final `MoveClips` of a drag): a drag of several clips is one step.
    Clip(u64),
    /// `DragTempoMap`.
    TempoMap(u64),
    /// `DragMeterMap`.
    MeterMap(u64),
    /// `DragMarkers`.
    Markers(u64),
    /// `DragRanges`.
    Ranges(u64),
    /// `DragTrackHeights`.
    TrackHeights(u64),
    /// `SetVcaLevel` on one VCA group.
    Vca(u64, u32),
    /// `SetZones` on one device.
    Zones(u64, DeviceKey),
    /// `SetPattern` on one pattern of one device.
    Pattern(u64, DeviceKey, u8),
    /// `SetCurves` on one pattern of one device.
    Curves(u64, DeviceKey, u8),
    /// `SetDrumStep` or `SetDrumPattern` on one pattern of one device.
    Drums(u64, DeviceKey, u8),
    /// `SetSplits` on one container.
    Splits(u64, DeviceKey),
    /// `SetMacro` on one target (container, macro, device, param).
    Macro(u64, DeviceKey, u8, DeviceKey, String),
}

impl Document {
    /// Wraps a document with empty history.
    pub fn new(doc: RackDoc) -> Self {
        debug_assert!(
            doc.validate().is_ok(),
            "a document must be valid: {}",
            doc.validate()
                .err()
                .map_or_else(String::new, |v| validate::describe(&v))
        );
        Self {
            doc,
            undo: VecDeque::new(),
            undo_bytes: 0,
            undo_limits: (MAX_UNDO_STEPS, MAX_UNDO_BYTES),
            hold_trim: false,
            redo: Vec::new(),
            merge: None,
            last_clip: None,
            revision: next_revision(),
            trial: Vec::new(),
            trial_redo: Vec::new(),
            labels: Labels::default(),
        }
    }

    /// Changes how much undo history the document keeps: at most `steps` steps (at least 1) and
    /// an estimated `bytes` of memory (the latest step stays whatever its size). Older steps are
    /// forgotten at once; the assets only they referenced leave the pool at the next
    /// [`Document::prune_unreferenced_assets`]. The defaults are [`MAX_UNDO_STEPS`] and
    /// [`MAX_UNDO_BYTES`].
    pub fn set_undo_limits(&mut self, steps: usize, bytes: usize) {
        self.undo_limits = (steps.max(1), bytes);
        self.trim_undo();
    }

    /// Records `edit`, named `label`, as the latest undo step and forgets the oldest steps
    /// beyond the limits.
    fn push_undo(&mut self, edit: Edit, label: &'static str) {
        self.undo_bytes += edit.approx_bytes();
        self.undo.push_back(edit);
        self.labels.undo.push_back(label);
        self.trim_undo();
    }

    /// Takes the latest undo step, with its name, off the history.
    fn pop_undo(&mut self) -> Option<(Edit, &'static str)> {
        let edit = self.undo.pop_back()?;
        let label = self.labels.undo.pop_back().unwrap_or(DEFAULT_LABEL);
        self.undo_bytes = self.undo_bytes.saturating_sub(edit.approx_bytes());
        Some((edit, label))
    }

    /// The name of the step [`Document::undo`] would undo (`"Move Device"`, `"Add Clip"`; a
    /// step of several drag-merged commands has the name of the first), `None` without one.
    pub fn undo_label(&self) -> Option<&'static str> {
        self.labels.undo.back().copied()
    }

    /// The name of the step [`Document::redo`] would redo, `None` without one.
    pub fn redo_label(&self) -> Option<&'static str> {
        self.labels.redo.last().copied()
    }

    /// Accounts for the latest undo step having changed from `before` estimated bytes (a merged
    /// drag extends it) to what it is now.
    fn resize_last_undo(&mut self, before: usize) {
        let now = self.undo.back().map_or(0, Edit::approx_bytes);
        self.undo_bytes = (self.undo_bytes + now).saturating_sub(before);
        self.trim_undo();
    }

    fn trim_undo(&mut self) {
        if self.hold_trim {
            return;
        }
        let (steps, bytes) = self.undo_limits;
        while self.undo.len() > 1 && (self.undo.len() > steps || self.undo_bytes > bytes) {
            if let Some(old) = self.undo.pop_front() {
                self.labels.undo.pop_front();
                self.undo_bytes = self.undo_bytes.saturating_sub(old.approx_bytes());
            }
        }
    }

    /// The current document.
    pub fn doc(&self) -> &RackDoc {
        &self.doc
    }

    /// A number that differs after every change of [`Document::doc`]: `apply` (the silent data
    /// syncs and asset registrations included), `undo`, `redo` and an asset prune that removed
    /// something take a new one, a rejected command keeps it, and a new `Document` starts with
    /// one no other document has had. Equal revisions mean equal documents, so a view that has
    /// synced at revision `r` has nothing to do while the revision is still `r`; the converse
    /// does not hold (a change and its undo leave two different numbers on equal content).
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Applies a command (validated with `registry`) and records it for undo.
    ///
    /// The returned [`SyncOp`]s must be processed in order (see [`SyncOp`]).
    pub fn apply(
        &mut self,
        command: Command,
        registry: &Registry,
    ) -> Result<Vec<SyncOp>, ModelError> {
        let ops = self.apply_inner(command, registry)?;
        // Debug builds check the whole document after every command: a planner that breaks an
        // invariant is caught at the command that broke it.
        debug_assert!(
            self.doc.validate().is_ok(),
            "a command left the document invalid: {}",
            self.doc
                .validate()
                .err()
                .map_or_else(String::new, |v| validate::describe(&v))
        );
        self.revision = next_revision();
        Ok(ops)
    }

    fn apply_inner(
        &mut self,
        command: Command,
        registry: &Registry,
    ) -> Result<Vec<SyncOp>, ModelError> {
        if let Command::RegisterAsset { hash, meta } = command {
            // Not an undo step; leaves history, redo and the merge group alone.
            return rules::register_asset(&mut self.doc, hash, meta).map(|()| Vec::new());
        }
        // The plugin data syncs are not undo steps either.
        let command = match command {
            Command::Plugin(PluginCommand::SyncState { device, state }) => {
                return plugin_cmd::sync_state(&mut self.doc, &device, state).map(|()| Vec::new());
            }
            Command::Plugin(PluginCommand::SyncLayout { device, layout }) => {
                return plugin_cmd::sync_layout(&mut self.doc, &device, layout)
                    .map(|()| Vec::new());
            }
            other => other,
        };
        let merge = match &command {
            Command::SetParam {
                key,
                param,
                merge: Some(id),
                ..
            } => Some(MergeKey::Param(*id, key.clone(), param.clone())),
            Command::Batch(commands) => gang_key(commands),
            Command::Song(
                SongCommand::SetClipGeometry { merge: Some(m), .. }
                | SongCommand::SetClipContent { merge: Some(m), .. }
                | SongCommand::SetClipControllers { merge: Some(m), .. }
                | SongCommand::SetAudioClip { merge: Some(m), .. }
                | SongCommand::SetWarpMarkers { merge: Some(m), .. }
                | SongCommand::SetClipGrid { merge: Some(m), .. },
            ) => Some(MergeKey::Clip(*m)),
            Command::Workflow(
                WorkflowCommand::MoveClips { merge: Some(m), .. }
                | WorkflowCommand::TrimClips { merge: Some(m), .. },
            ) => Some(MergeKey::Clip(*m)),
            Command::Song(SongCommand::DragTempoMap { merge, .. }) => {
                Some(MergeKey::TempoMap(*merge))
            }
            Command::Song(SongCommand::DragMeterMap { merge, .. }) => {
                Some(MergeKey::MeterMap(*merge))
            }
            Command::Song(SongCommand::DragMarkers { merge, .. }) => {
                Some(MergeKey::Markers(*merge))
            }
            Command::Song(SongCommand::DragRanges { merge, .. }) => Some(MergeKey::Ranges(*merge)),
            Command::Song(SongCommand::DragTrackHeights { merge, .. }) => {
                Some(MergeKey::TrackHeights(*merge))
            }
            Command::Console(ConsoleCommand::SetVcaLevel {
                vca,
                merge: Some(m),
                ..
            }) => Some(MergeKey::Vca(*m, *vca)),
            Command::SetZones {
                device,
                merge: Some(m),
                ..
            } => Some(MergeKey::Zones(*m, device.clone())),
            Command::SetPattern {
                device,
                index,
                merge: Some(m),
                ..
            } => Some(MergeKey::Pattern(*m, device.clone(), *index)),
            Command::SetCurves {
                device,
                index,
                merge: Some(m),
                ..
            } => Some(MergeKey::Curves(*m, device.clone(), *index)),
            Command::SetDrumStep {
                device,
                index,
                merge: Some(m),
                ..
            }
            | Command::SetDrumPattern {
                device,
                index,
                merge: Some(m),
                ..
            } => Some(MergeKey::Drums(*m, device.clone(), *index)),
            Command::Container(ContainerCommand::SetSplits {
                container,
                merge: Some(m),
                ..
            }) => Some(MergeKey::Splits(*m, container.clone())),
            Command::Container(ContainerCommand::SetMacro {
                container,
                index,
                target,
                merge: Some(m),
            }) => Some(MergeKey::Macro(
                *m,
                container.clone(),
                *index,
                target.device.clone(),
                target.param.clone(),
            )),
            _ => None,
        };
        let label = command.label();
        let (edit, new_clip) = Self::plan(&self.doc, command, registry)?;
        let mut ops = edit.perform(&mut self.doc)?;
        let edit = match song_end_growth(&self.doc, &edit) {
            Some(grow) => {
                match grow.perform(&mut self.doc) {
                    Ok(more) => ops.extend(more),
                    Err(fault) => {
                        // All or nothing: the command goes back out, history untouched.
                        let mut back = edit;
                        back.invert();
                        back.perform(&mut self.doc)?;
                        return Err(fault.into());
                    }
                }
                Edit::Compound(vec![edit, grow])
            }
            None => edit,
        };
        self.redo.clear();
        self.labels.redo.clear();
        let merged = merge.is_some() && merge == self.merge;
        let before = if merged {
            self.undo.back().map_or(0, Edit::approx_bytes)
        } else {
            0
        };
        if merged
            && matches!(merge, Some(MergeKey::Clip(_)))
            && let Some(last) = self.undo.back_mut()
        {
            last.fold_clip_edit(edit);
            self.resize_last_undo(before);
            self.merge = merge;
            if new_clip.is_some() {
                self.last_clip = new_clip;
            }
            return Ok(ops);
        }
        if merged
            && matches!(merge, Some(MergeKey::Gang(..)))
            && let Edit::Compound(latest) = &edit
            && let Some(Edit::Compound(parts)) = self.undo.back_mut()
            && Edit::merge_gang(parts, latest)
        {
            self.resize_last_undo(before);
            self.merge = merge;
            return Ok(ops);
        }
        let mut pushed = false;
        match (self.undo.back_mut(), &edit) {
            (Some(Edit::Param { new, .. }), Edit::Param { new: latest, .. }) if merged => {
                *new = *latest;
            }
            (Some(Edit::Zones { new, .. }), Edit::Zones { new: latest, .. }) if merged => {
                new.clone_from(latest);
            }
            (Some(Edit::Steps { new, .. }), Edit::Steps { new: latest, .. }) if merged => {
                new.clone_from(latest);
            }
            (Some(Edit::Curves { new, .. }), Edit::Curves { new: latest, .. }) if merged => {
                new.clone_from(latest);
            }
            (Some(Edit::Drums { new, .. }), Edit::Drums { new: latest, .. }) if merged => {
                new.clone_from(latest);
            }
            (Some(Edit::ConsoleVcas { new, .. }), Edit::ConsoleVcas { new: latest, .. })
                if merged =>
            {
                new.clone_from(latest);
            }
            (Some(Edit::Splits { new, .. }), Edit::Splits { new: latest, .. }) if merged => {
                new.clone_from(latest);
            }
            (Some(Edit::Macro { new, .. }), Edit::Macro { new: latest, .. }) if merged => {
                new.clone_from(latest);
            }
            (Some(Edit::TempoMap { new, .. }), Edit::TempoMap { new: latest, .. }) if merged => {
                new.clone_from(latest);
            }
            (Some(Edit::MeterMap { new, .. }), Edit::MeterMap { new: latest, .. }) if merged => {
                new.clone_from(latest);
            }
            (Some(Edit::Markers { new, .. }), Edit::Markers { new: latest, .. }) if merged => {
                new.clone_from(latest);
            }
            (Some(Edit::Ranges { new, .. }), Edit::Ranges { new: latest, .. }) if merged => {
                new.clone_from(latest);
            }
            (Some(Edit::TrackHeights { new, .. }), Edit::TrackHeights { new: latest, .. })
                if merged =>
            {
                new.clone_from(latest);
            }
            _ => {
                pushed = true;
                self.push_undo(edit, label);
            }
        }
        if merged && !pushed {
            self.resize_last_undo(before);
        }
        self.merge = merge;
        if new_clip.is_some() {
            self.last_clip = new_clip;
        }
        Ok(ops)
    }

    /// Applies `command` and folds it into the undo step before it, so one undo reverts both: a
    /// follow-up that completes the edit just made (the clip a note edit had to grow, say). The
    /// command is planned and validated like [`Document::apply`]; with no earlier step, or when
    /// the command changes nothing the history records, it is an ordinary step.
    pub fn apply_joined(
        &mut self,
        command: Command,
        registry: &Registry,
    ) -> Result<Vec<SyncOp>, ModelError> {
        let depth = self.undo.len();
        self.merge = None;
        let ops = self.apply(command, registry)?;
        if depth >= 1
            && self.undo.len() == depth + 1
            && let Some((latest, _)) = self.pop_undo()
            && let Some((previous, label)) = self.pop_undo()
        {
            self.push_undo(Edit::Compound(vec![previous, latest]), label);
        }
        self.merge = None;
        Ok(ops)
    }

    /// Applies `command` as one more step of a trial: the document changes and the sync ops
    /// come back, but no undo step is recorded and the redo history is kept aside. A trial is
    /// a run of previews (stepping through the patches of a device, say) that the user then
    /// accepts with [`Document::commit_trial`], one undo step for the whole run, or drops with
    /// [`Document::cancel_trial`], which puts the document back. Nothing else may be applied
    /// while a trial is open (the caller ends it first); undo and redo refuse to run.
    ///
    /// # Errors
    /// As [`Document::apply`]; a rejected command leaves the trial as it was.
    pub fn apply_trial(
        &mut self,
        command: Command,
        registry: &Registry,
    ) -> Result<Vec<SyncOp>, ModelError> {
        let redo = std::mem::take(&mut self.redo);
        let redo_labels = std::mem::take(&mut self.labels.redo);
        let depth = self.undo.len();
        self.hold_trim = true;
        let applied = self.apply(command, registry);
        self.hold_trim = false;
        let ops = match applied {
            Ok(ops) => ops,
            Err(e) => {
                self.redo = redo;
                self.labels.redo = redo_labels;
                return Err(e);
            }
        };
        if self.undo.len() > depth
            && let Some((step, label)) = self.pop_undo()
        {
            if self.trial.is_empty() {
                self.trial_redo = redo;
                self.labels.trial_redo = redo_labels;
            }
            self.trial.push(step);
            self.labels.trial.push(label);
        } else if self.trial.is_empty() {
            // The command changed nothing the history records: the redo history stays.
            self.redo = redo;
            self.labels.redo = redo_labels;
        }
        self.merge = None;
        Ok(ops)
    }

    /// Whether a trial is open ([`Document::apply_trial`] ran and was not ended).
    pub fn in_trial(&self) -> bool {
        !self.trial.is_empty()
    }

    /// Accepts the open trial: its steps become one undo step (none when it changed nothing).
    pub fn commit_trial(&mut self) {
        self.merge = None;
        self.trial_redo.clear();
        self.labels.trial_redo.clear();
        let labels = std::mem::take(&mut self.labels.trial);
        let label = labels.first().copied().unwrap_or(DEFAULT_LABEL);
        match self.trial.len() {
            0 => {}
            1 => {
                let steps = std::mem::take(&mut self.trial);
                steps
                    .into_iter()
                    .for_each(|step| self.push_undo(step, label));
            }
            _ => {
                let steps = std::mem::take(&mut self.trial);
                self.push_undo(Edit::Compound(steps), label);
            }
        }
    }

    /// Drops the open trial: the document is as it was before it, and so is the redo history.
    /// The sync ops to apply come back (empty without a trial).
    ///
    /// # Errors
    /// [`ModelError::Invariant`] when a step does not fit the document (the trial stays open
    /// with the steps that were not undone).
    pub fn cancel_trial(&mut self) -> Result<Vec<SyncOp>, ModelError> {
        self.merge = None;
        let mut ops = Vec::new();
        while let Some(mut step) = self.trial.pop() {
            step.invert();
            let done = step.perform(&mut self.doc);
            step.invert();
            match done {
                Ok(more) => {
                    ops.extend(more);
                    self.labels.trial.pop();
                }
                Err(fault) => {
                    self.trial.push(step);
                    return Err(fault.into());
                }
            }
        }
        self.redo = std::mem::take(&mut self.trial_redo);
        self.labels.redo = std::mem::take(&mut self.labels.trial_redo);
        self.revision = next_revision();
        Ok(ops)
    }

    /// Undoes the last step; `None` when there is nothing to undo (or when the step cannot be
    /// undone: it stays where it is and the document is unchanged; [`Document::try_undo`] says
    /// why).
    pub fn undo(&mut self, registry: &Registry) -> Option<Vec<SyncOp>> {
        self.try_undo(registry).unwrap_or(None)
    }

    /// Redoes the last undone step; `None` when there is nothing to redo (or when the step cannot
    /// be redone; see [`Document::undo`]).
    pub fn redo(&mut self, registry: &Registry) -> Option<Vec<SyncOp>> {
        self.try_redo(registry).unwrap_or(None)
    }

    /// Undoes the last step: `Ok(None)` when there is nothing to undo.
    ///
    /// # Errors
    /// [`ModelError::Invariant`] when the step does not fit the document. Nothing changes: the
    /// document is as it was and the step stays on the undo stack.
    pub fn try_undo(&mut self, _registry: &Registry) -> Result<Option<Vec<SyncOp>>, ModelError> {
        self.merge = None;
        if self.in_trial() {
            return Ok(None);
        }
        let Some((mut edit, label)) = self.pop_undo() else {
            return Ok(None);
        };
        edit.invert();
        let done = edit.perform(&mut self.doc);
        edit.invert();
        match done {
            Ok(ops) => {
                self.redo.push(edit);
                self.labels.redo.push(label);
                self.revision = next_revision();
                Ok(Some(ops))
            }
            Err(fault) => {
                self.push_undo(edit, label);
                Err(fault.into())
            }
        }
    }

    /// Redoes the last undone step: `Ok(None)` when there is nothing to redo.
    ///
    /// # Errors
    /// As [`Document::try_undo`]; the step stays on the redo stack.
    pub fn try_redo(&mut self, _registry: &Registry) -> Result<Option<Vec<SyncOp>>, ModelError> {
        self.merge = None;
        if self.in_trial() {
            return Ok(None);
        }
        let Some(edit) = self.redo.pop() else {
            return Ok(None);
        };
        let label = self.labels.redo.pop().unwrap_or(DEFAULT_LABEL);
        match edit.perform(&mut self.doc) {
            Ok(ops) => {
                self.push_undo(edit, label);
                self.revision = next_revision();
                Ok(Some(ops))
            }
            Err(fault) => {
                self.redo.push(edit);
                self.labels.redo.push(label);
                Err(fault.into())
            }
        }
    }

    /// The id allocated by the last successful `AddClip`, `DuplicateClip` or `SplitClip`.
    ///
    /// Other commands, undo and redo leave it unchanged. The UI reads it right after applying
    /// one of those commands (e.g. to select the new clip).
    pub fn last_clip_id(&self) -> Option<ClipId> {
        self.last_clip
    }

    /// Every asset id the document needs: those its song and sampler zones reference, plus those referenced by
    /// clips and zones in the undo and redo history (undo or redo must find them in the pool). The app
    /// keeps exactly these files in the bundle's `Audio/` folder on save.
    pub fn referenced_assets(&self) -> BTreeSet<String> {
        let mut out = self.doc.referenced_assets();
        for edit in self.undo.iter().chain(&self.redo).chain(&self.trial) {
            edit.collect_assets(&mut out);
        }
        out
    }

    /// Drops every pool asset that neither the song nor the undo/redo history references
    /// (see [`Document::referenced_assets`]); undo and redo stay valid. Call it on save. It is
    /// not an undoable command: it removes nothing a step could bring back.
    pub fn prune_unreferenced_assets(&mut self) {
        let keep = self.referenced_assets();
        let before = self.doc.assets.len();
        self.doc.assets.retain(|h, _| keep.contains(h));
        if self.doc.assets.len() != before {
            self.revision = next_revision();
        }
    }

    /// How many undo steps there are (a count for tests; merged commands share a step).
    #[doc(hidden)]
    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }

    /// Whether undo is possible.
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// Whether redo is possible.
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Ends any open drag-merge group (call on pointer release).
    pub fn end_merge(&mut self) {
        self.merge = None;
    }

    /// Validates `command` against the current document and turns it into a reversible edit.
    fn plan(
        doc: &RackDoc,
        command: Command,
        registry: &Registry,
    ) -> Result<(Edit, Option<ClipId>), ModelError> {
        freeze::guard(doc, &command)?;
        let edit = match command {
            Command::Batch(commands) => {
                if commands.is_empty() {
                    return Err(ModelError::BadBatch("an empty batch".into()));
                }
                let mut scratch = doc.clone();
                let mut edits = Vec::with_capacity(commands.len());
                let mut new_clip = None;
                for command in commands {
                    if !matches!(
                        command,
                        Command::Connect(_)
                            | Command::Disconnect { .. }
                            | Command::SetParam { .. }
                            | Command::SetZones { .. }
                            | Command::SetPattern { .. }
                            | Command::SetCurves { .. }
                            | Command::SetDrumStep { .. }
                            | Command::SetDrumLane { .. }
                            | Command::SetDrumPattern { .. }
                            | Command::AddDrumPattern { .. }
                            | Command::RemoveDrumPattern { .. }
                            | Command::DuplicateDrumPattern { .. }
                            | Command::AddPattern { .. }
                            | Command::RemovePattern { .. }
                            | Command::DuplicatePattern { .. }
                            | Command::Plugin(
                                PluginCommand::SetPinned { .. }
                                    | PluginCommand::SetPower { .. }
                                    | PluginCommand::Relink { .. }
                            )
                            | Command::Container(
                                ContainerCommand::SetSplits { .. }
                                    | ContainerCommand::SetMacro { .. }
                                    | ContainerCommand::RemoveMacroTarget { .. }
                                    | ContainerCommand::RenameMacro { .. }
                            )
                            | Command::SetColor { .. }
                            | Command::Song(
                                SongCommand::AddTrack { .. }
                                    | SongCommand::AddClip { .. }
                                    | SongCommand::SetClipGrid { .. }
                                    | SongCommand::SetClipMeta { .. }
                                    | SongCommand::SetClipControllers { .. }
                                    | SongCommand::SetTrackFlags { .. }
                                    | SongCommand::SetClipGeometry { .. }
                                    | SongCommand::SetAudioClip { .. }
                                    | SongCommand::SetWarpMarkers { .. }
                                    | SongCommand::SplitClip { .. }
                                    | SongCommand::StripSilence { .. }
                                    | SongCommand::SetTrackColor { .. }
                                    | SongCommand::SetGroupCollapsed { .. }
                                    | SongCommand::SetLaneFolded { .. }
                                    | SongCommand::AddAutomationLane { .. }
                                    | SongCommand::RemoveAutomationLane { .. }
                                    | SongCommand::SetLaneMuted { .. }
                                    | SongCommand::SetLaneStatic { .. }
                                    | SongCommand::SetLaneWriteMode { .. }
                            )
                    ) {
                        return Err(ModelError::BadBatch(format!(
                            "only cable commands, SetParam, SetZones, the pattern commands, SetSplits, SetMacro, RemoveMacroTarget, RenameMacro, SetPinned, SetPower, Relink, AddTrack, AddClip, SetClipGrid, SetClipMeta, SetClipControllers, SetClipGeometry, SetAudioClip, SetWarpMarkers, SplitClip, StripSilence, SetTrackFlags, SetTrackColor, SetColor, SetGroupCollapsed, SetLaneFolded, AddAutomationLane, RemoveAutomationLane, SetLaneMuted, SetLaneStatic and SetLaneWriteMode can be batched, not {command:?}"
                        )));
                    }
                    let (edit, clip) = Self::plan(&scratch, command, registry)?;
                    edit.perform(&mut scratch)?;
                    edits.push(edit);
                    new_clip = clip.or(new_clip);
                }
                return Ok((Edit::Compound(edits), new_clip));
            }
            Command::AddDevice {
                index,
                type_id,
                key,
            } => {
                let desc = rules::descriptor(registry, &type_id)?;
                plugin_cmd::refuse_generic_add(&type_id)?;
                if type_id == console::BUS_TYPE {
                    return Err(console_cmd::bad(
                        "an output bus is created with ConsoleCommand::CreateBus",
                    ));
                }
                if type_id == SPLIT_TYPE {
                    return Err(ModelError::NotAllowedInContainer(format!(
                        "{} is internal; the container expansion creates it",
                        desc.name
                    )));
                }
                let key = match key {
                    Some(k) => {
                        rules::check_new_key(doc, &k.0)?;
                        k
                    }
                    None => rules::generate_key(doc, &type_id),
                };
                let instrument = desc.kind == atomik_device_sdk::DeviceKind::Instrument;
                let device = container::new_device(
                    registry,
                    desc,
                    key.clone(),
                    rules::unique_name(doc, desc.name),
                );
                // an instrument gets its console channel in the same undo step, when
                // the registry has the console devices. An instrument without audio
                // outputs gets none.
                console_cmd::insert_with_channel(doc, registry, index, device, instrument)
            }
            Command::RemoveDevice { key } => {
                // The device's track goes with it, first: the song stops playing the device
                // before the engine removes it (and comes back after it on undo). Console
                // bookkeeping (its channel, master, return slot) is part of the same step.
                if key.0.contains('/') {
                    container_cmd::remove_inner(doc, &key)?
                } else {
                    rules::index_of(doc, &key)?;
                    let mut p = console_cmd::Planner::new(doc, registry);
                    console_cmd::remove_device(&mut p, &key)?;
                    p.finish()
                }
            }
            Command::MoveDevice { key, to } => {
                if key.0.contains('/') {
                    container_cmd::move_inner(doc, &key, to)?
                } else {
                    rules::index_of(doc, &key)?;
                    let to = to.min(doc.devices.len() - 1);
                    // The master stays where it is, and nothing goes above it.
                    rack_columns::plan_move(doc, std::slice::from_ref(&key), Placement::Flat(to))?
                }
            }
            Command::AddDeviceAt { slot, type_id, key } => {
                let at = doc.resolve_slot(slot)?;
                let (edit, _) = Self::plan(
                    doc,
                    Command::AddDevice {
                        index: at.index,
                        type_id,
                        key,
                    },
                    registry,
                )?;
                rack_columns::place_new_device(doc, at, edit)
            }
            Command::Plugin(PluginCommand::AddAt {
                slot,
                plugin,
                layout,
                path_hint,
                key,
            }) => {
                let at = doc.resolve_slot(slot)?;
                let edit = plugin_cmd::plan(
                    doc,
                    registry,
                    PluginCommand::Add {
                        index: at.index,
                        container: None,
                        plugin,
                        layout,
                        path_hint,
                        key,
                        auto_split: false,
                    },
                )?;
                rack_columns::place_new_device(doc, at, edit)
            }
            Command::AddRackColumn => rack_columns::plan_add_column(doc)?,
            Command::RemoveRackColumn { column } => rack_columns::plan_remove_column(doc, column)?,
            Command::SetFolded { key, folded } => Edit::Folded {
                old: rules::resolve(doc, &key)?.folded,
                new: folded,
                key,
            },
            Command::Rename { key, name } if doc.console.bus_index(&key).is_some() => {
                // A bus carries its name in the console too.
                let mut p = console_cmd::Planner::new(doc, registry);
                console_bus::rename_bus(&mut p, &key, &name)?;
                p.finish()
            }
            Command::Rename { key, name } => {
                let device = rules::resolve(doc, &key)?;
                let name = name.trim();
                let new = if name.is_empty() {
                    rules::default_name(doc, registry, &key, device)?
                } else {
                    name.to_owned()
                };
                if doc.console.parallels_of(&key).next().is_some() && new != device.name {
                    // The parallel channels of a strip carry its name: they follow.
                    let old = device.name.clone();
                    let mut p = console_cmd::Planner::new(doc, registry);
                    p.push(Edit::Name {
                        old: old.clone(),
                        new: new.clone(),
                        key: key.clone(),
                    });
                    console_mix::follow_names(&mut p, &key, &old, &new)?;
                    p.finish()
                } else {
                    Edit::Name {
                        old: device.name.clone(),
                        new,
                        key,
                    }
                }
            }
            Command::SetColor { key, color } => Edit::Color {
                old: rules::resolve(doc, &key)?.color,
                new: color,
                key,
            },
            Command::Connect(cable) => workflow_create::with_entries(
                doc,
                registry,
                container_cmd::connect(doc, registry, &cable)?,
            ),
            Command::Disconnect { to } if to.device.0.contains('/') => {
                container_cmd::disconnect(doc, registry, &to)?
            }
            Command::Disconnect { to } => {
                let (_, direction) = rules::socket(doc, registry, &to)?;
                if !direction.is_input() {
                    return Err(ModelError::BadCable(format!("{to} is not an input")));
                }
                // A summing input loses every cable plugged into it, in one step.
                let mut cables: Vec<Edit> = doc
                    .cables
                    .iter()
                    .filter(|c| c.to == to)
                    .cloned()
                    .map(Edit::Disconnect)
                    .collect();
                let cut = match cables.len() {
                    0 => return Err(ModelError::BadCable(format!("{to} is not connected"))),
                    1 => cables.remove(0),
                    _ => Edit::Compound(cables),
                };
                workflow_create::with_entries(doc, registry, cut)
            }
            Command::SetParam {
                key, param, value, ..
            } => {
                let device = rules::resolve(doc, &key)?;
                if console::is_internal_param(&param) {
                    // Set by the app from derived state; never stored.
                    return Err(ModelError::UnknownKey(param));
                }
                let desc = rules::descriptor(registry, &device.type_id)?;
                let p = desc
                    .param_by_key(&param)
                    .ok_or_else(|| ModelError::UnknownKey(param.clone()))?;
                if value.is_nan() {
                    return Err(ModelError::OutOfRange(format!("{key} {param:?} is NaN")));
                }
                if let Some(a) = doc.bank_driver(&key, &param) {
                    return Err(ModelError::MacroDriven(format!(
                        "{key} {param} is driven by {} of {}; change the macro or remove the assignment",
                        doc.macro_name(&a.bank, a.index),
                        a.bank
                    )));
                }
                if let Some((container, index)) = doc.macro_driver(&key, &param) {
                    return Err(ModelError::MacroDriven(format!(
                        "{key} {param} is driven by macro {} of {container}; unmap it first",
                        index + 1
                    )));
                }
                Edit::Param {
                    old: device.params.get(&param).copied(),
                    new: Some(p.clamp(value)),
                    default: p.default,
                    key,
                    param,
                }
            }
            Command::SetZones { device, zones, .. } => {
                let d = rules::resolve(doc, &device)?;
                if !registry.takes_samples(&d.type_id) {
                    return Err(ModelError::BadZones(format!(
                        "{device} ({}) does not take samples",
                        d.type_id
                    )));
                }
                zones::check_zones(&doc.assets, &zones)
                    .map_err(|(i, m)| ModelError::BadZones(format!("zone {i}: {m}")))?;
                Edit::Zones {
                    old: d.zones.clone(),
                    new: zones,
                    key: device,
                }
            }
            command @ (Command::SetPattern { .. }
            | Command::SetCurves { .. }
            | Command::AddPattern { .. }
            | Command::RemovePattern { .. }
            | Command::DuplicatePattern { .. }) => steps_cmd::plan(doc, registry, command)?,
            command @ (Command::SetDrumStep { .. }
            | Command::SetDrumLane { .. }
            | Command::SetDrumPattern { .. }
            | Command::AddDrumPattern { .. }
            | Command::RemoveDrumPattern { .. }
            | Command::DuplicateDrumPattern { .. }) => drums_cmd::plan(doc, registry, command)?,
            Command::Container(c) => container_cmd::plan(doc, registry, c)?,
            Command::Plugin(c) => plugin_cmd::plan(doc, registry, c)?,
            Command::RegisterAsset { .. } => unreachable!("handled before planning"),
            Command::RelinkAsset { from, to } => relink::plan(doc, &from, &to)?,
            Command::RenameAsset { asset, name } => rules::plan_rename_asset(doc, &asset, &name)?,
            Command::SetInputChannels(n) => rules::plan_input_channels(doc, n)?,
            Command::SetOutputChannels(n) => rules::plan_output_channels(doc, n)?,
            Command::SetSongInfo(info) => song_info::plan_set(doc, &info)?,
            Command::ConvertSampleRate { rate, assets } => rate_convert::plan(doc, rate, &assets)?,
            Command::SetTempo(bpm) => {
                if !(TEMPO_MIN..=TEMPO_MAX).contains(&bpm) {
                    return Err(ModelError::OutOfRange(format!(
                        "{bpm} BPM is outside {TEMPO_MIN}..={TEMPO_MAX}"
                    )));
                }
                Edit::Tempo {
                    old: doc.tempo_bpm,
                    new: bpm,
                }
            }
            Command::Console(console_command) => console_cmd::plan(doc, registry, console_command)?,
            Command::MidiMap(c) => midi_map::plan(doc, registry, c)?,
            Command::Macro(c) => macros::plan(doc, registry, c)?,
            Command::Song(song_command) => return song_cmd::plan(doc, registry, song_command),
            Command::Workflow(c) => return workflow_cmd::plan(doc, registry, c),
        };
        Ok((edit, None))
    }
}

/// Lowest accepted tempo in BPM.
pub const TEMPO_MIN: f64 = 20.0;
/// Highest accepted tempo in BPM.
pub const TEMPO_MAX: f64 = 999.0;

impl DeviceKey {
    /// The key as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for DeviceKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::fmt::Display for SocketRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.device.0, self.socket)
    }
}

impl std::fmt::Display for Cable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} -> {}", self.from, self.to)
    }
}

impl RackDoc {
    /// The song, for writing: the first write of a song shared with another copy of the
    /// document copies its structure (tracks, lanes and clips), never the notes of a clip.
    pub fn song_mut(&mut self) -> &mut Song {
        Arc::make_mut(&mut self.song)
    }

    /// The device `key`, if present.
    pub fn device(&self, key: &DeviceKey) -> Option<&RackDevice> {
        self.devices.iter().find(|d| &d.key == key)
    }

    /// The device the notes of `track` enter: its `notes_to` (the top of its player stack) when
    /// that is a top-level device of the document, else the track's own device. A `notes_to`
    /// that names a device that is gone (an older code path, a hand-edited file) is not an
    /// error: the track plays its own device.
    pub fn notes_entry<'a>(&'a self, track: &'a Track) -> &'a DeviceKey {
        track
            .notes_to
            .as_ref()
            .filter(|k| **k != track.device && self.device(k).is_some())
            .unwrap_or(&track.device)
    }

    /// The effective plain value of a param: the stored value, or the descriptor default when
    /// none is stored. `None` if the device, its type or the param is unknown. `key` may be a
    /// path key.
    pub fn param_value(&self, key: &DeviceKey, param: &str, registry: &Registry) -> Option<f32> {
        let device = self.resolve(key)?;
        let desc = registry
            .get(&device.type_id)?
            .descriptor
            .param_by_key(param)?;
        Some(device.params.get(param).copied().unwrap_or(desc.default))
    }

    /// The derived `internal.solo_silence` state of every console strip: the channels in
    /// console order, then the buses (additive solo-in-place): when any strip is soloed
    /// (`solo` >= 0.5, or its VCA group is soloed), every strip that is not audible is silenced. A soloed strip is audible,
    /// so is every bus on its path to the master or an Audio I/O pair (transitively), and so is
    /// every strip that feeds a soloed bus (transitively). Returns are not strips, so they are
    /// solo-safe. Mute plays no part. The app pushes these values to the engine.
    pub fn solo_silence(&self, registry: &Registry) -> Vec<(DeviceKey, bool)> {
        console_bus::solo_silence(self, registry)
    }

    /// True if `device` has a track that is an audio track and the device is an
    /// `atomik.audio-track`.
    pub fn is_audio_track(&self, device: &DeviceKey, registry: &Registry) -> bool {
        self.device(device).is_some_and(|d| {
            d.type_id == song::AUDIO_TRACK_TYPE
                && registry.get(&d.type_id).is_some()
                && self.song.track(device).is_some_and(Track::is_audio)
        })
    }

    /// The asset ids the song's audio clips and the devices' zones reference (clips on every
    /// audio lane, muted or not). The app uses it to find orphaned files in the bundle; see also
    /// [`Document::referenced_assets`], which includes the undo history.
    pub fn referenced_assets(&self) -> BTreeSet<String> {
        self.song
            .tracks
            .iter()
            .flat_map(|t| &t.lanes)
            .flat_map(|l| {
                l.clips
                    .iter()
                    .chain(l.take_groups.iter().flat_map(|g| &g.takes))
            })
            .filter_map(|c| match &c.content {
                ClipContent::Audio(a) => Some(a.asset.clone()),
                _ => None,
            })
            // The render of a frozen track.
            .chain(
                self.song
                    .tracks
                    .iter()
                    .filter_map(|t| t.frozen.as_ref().map(|f| f.audio.asset.clone())),
            )
            .collect::<BTreeSet<String>>()
            .into_iter()
            .chain({
                let mut zones = BTreeSet::new();
                for d in &self.devices {
                    container::collect_device_assets(d, &mut zones);
                }
                zones
            })
            .collect()
    }

    /// Removes the pool assets the song does not reference. Not an undoable command, and not
    /// aware of undo history: prune a clone that is about to be saved, or use
    /// [`Document::prune_unreferenced_assets`] on a live document.
    pub fn prune_unreferenced_assets(&mut self) {
        let keep = self.referenced_assets();
        self.assets.retain(|h, _| keep.contains(h));
    }

    /// The cable plugged into input `to`, if any.
    pub fn cable_into(&self, to: &SocketRef) -> Option<&Cable> {
        self.cables.iter().find(|c| &c.to == to)
    }
}

/// An empty rack at 48 kHz, 120 BPM, stereo out, seed 0.
pub fn empty_doc() -> RackDoc {
    RackDoc {
        sample_rate: 48_000,
        tempo_bpm: 120.0,
        seed: 0,
        output_channels: 2,
        input_channels: 2,
        assets: BTreeMap::new(),
        devices: Vec::new(),
        columns: 1,
        cables: Vec::new(),
        song: Arc::new(empty_song(120.0)),
        console: Console::default(),
        midi_maps: Vec::new(),
        macros: MacroSet::default(),
        info: SongInfo::default(),
    }
}

/// The starter document of File, New: [`empty_doc`] plus the console master,
/// one `atomik.master` device keyed `master` (named "Master") placed first, so it sits directly
/// under Audio I/O, with the cables `master:main_l -> interface:out_1` and
/// `master:main_r -> interface:out_2` (the second skipped below two outputs). No channels, no
/// returns, compensation, return alignment and send follow on, one transport track as in `empty_doc`. It is built by the same code
/// that creates the master with the first channel, is deterministic and saves byte-identically.
/// It also turns on note tails ([`Song::tails`]). [`empty_doc`] is unchanged; loaders and Open
/// never call this.
pub fn new_project_doc() -> RackDoc {
    let mut doc = empty_doc();
    console_cmd::install_starter_master(&mut doc);
    // A new project lets notes ring past the end of their clip; a loaded one keeps its own flag.
    doc.song_mut().tails = true;
    // ...and aligns FX returns and sidechain keys with the dry path; a loaded one keeps its own.
    doc.console.align_returns = true;
    // ...and lets the sends of a strip that plays into a bus follow the bus; a loaded one keeps its own.
    doc.console.send_follow = true;
    doc
}

/// Loads a patch file (format v1 or v2) into a document, validating against `registry`.
///
/// v1 files carry no layout: names default to the descriptor name made unique in rack order
/// (`"Poly Synth"`, `"Poly Synth 2"`), devices are unfolded and uncoloured. Params are
/// clamped through their descriptors; `notes` are accepted and ignored; the song is
/// [`empty_song`] at the file's tempo. Cables are validated
/// with the engine's rules and sorted. Errors are [`ModelError::Load`] with a message
/// `"<json path>: <what is wrong>"`, e.g. `devices[1].params.mixx: … has no param "mixx"`.
pub fn load_patch(json: &str, registry: &Registry) -> Result<RackDoc, ModelError> {
    patch::load(json, registry)
}

/// [`load_project`] and the load warnings: everything the file had that the document does not.
/// Each warning is one line, led by the JSON path it concerns:
///
/// - a device keyed `boundary` in a file older than version 8 was renamed (the key is reserved
///   for a container's inside edge since then; the device becomes `boundary-1`, cables and
///   references follow);
/// - a version 1 instrument that cannot get a console channel was left without one;
/// - the height of a device without a track, and a comp clip id that is not on its lane, were
///   dropped;
/// - a file from a newer build had fields its `"ignorable"` list allows an older reader to skip,
///   and they were skipped.
pub fn load_project_with_warnings(
    json: &str,
    registry: &Registry,
) -> Result<(RackDoc, Vec<String>), ModelError> {
    project::load_with_warnings(json, registry)
}

/// Saves a document as a patch file, format v2. Patches have no inputs, audio tracks or
/// pool: cables from `interface:in_k` do not load back.
///
/// Writes every v1 field (`length_beats` is [`SAVE_LENGTH_BEATS`], no `notes`) plus each
/// device's `name`, `folded` and, when set, `color` (`"#rrggbb"`), with devices in rack order
/// and cables in canonical order. The output is deterministic and [`load_patch`] reads it
/// back to an equal document.
pub fn save_patch(doc: &RackDoc) -> String {
    patch::save(doc)
}

/// Loads an `.atomik` project file, or a patch v1/v2 file as a song-less
/// project. Errors are [`ModelError::Load`] with a message `"<json path>: <what is wrong>"`,
/// e.g. `song.tracks[0].lanes[0].clips[0].notes[1]: notes must be sorted…`. The schema is in
/// [`save_project`] docs.
pub fn load_project(json: &str, registry: &Registry) -> Result<RackDoc, ModelError> {
    project::load(json, registry)
}

/// Saves a document as an `.atomik` project file, format [`PROJECT_FORMAT`] version
/// [`PROJECT_VERSION`]: the patch v2 rack fields (plus each device's `"zones"` when it has any,
/// `"steps"`, `"curves"` and `"drums"` when present, a container's `"inner"`, a plugin device's
/// `"plugin"` and the `"patch"` name) (without `length_beats`),
/// `"input_channels"`, `"console"` (channels with their sources, master, 8 returns,
/// compensation), `"assets"` (the sample pool) and `"song"` with every song field, clip ids
/// and `next_clip_id`. The output is deterministic and canonical,
/// and [`load_project`] reads it back bit-exactly.
pub fn save_project(doc: &RackDoc) -> String {
    project::save(doc)
}

/// [`save_project`] after [`RackDoc::validate`]: a document that breaks its own rules (a NaN in
/// a stored value, a clip past the last tick) is an error here, before anything is written,
/// instead of a file that cannot be read back.
///
/// # Errors
/// [`ModelError::Invalid`] listing the violations.
pub fn save_project_checked(doc: &RackDoc) -> Result<String, ModelError> {
    doc.validate().map_err(|v| {
        ModelError::Invalid(format!(
            "the document is not valid: {}",
            validate::describe(&v)
        ))
    })?;
    Ok(project::save(doc))
}
