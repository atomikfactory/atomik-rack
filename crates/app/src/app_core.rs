//! [`AppCore`]: the window-free heart of the app (no winit, no GPU).

use crate::clipboard::Clipboard;
use crate::record::{InputQuantize, RecordedNote, Take};
use crate::sink::EngineSink;
use crate::target::{NoteTarget, Target};
use atomik_core::{NoteKind, ParamId, SampleRate, Tempo, Ticks};
use atomik_device_sdk::{DeviceDescriptor, Registry, SocketId};
use atomik_engine::{
    DeviceInstanceId, Endpoint, EngineConfig, EngineError, Graph, GraphError,
    MAX_COMPENSATION_SECONDS, PdcReport, SongPosition, SongSnapshot, Topology, TransportCommand,
    TransportState, compile_song,
};
use atomik_model::console::{
    BUS_TYPE, CHANNEL_TYPE, MASTER_TYPE, SOLO_SILENCE_PARAM, VCA_DB_PARAM, VCA_MUTE_PARAM,
    VCA_SOLO_PARAM, VcaFeed, is_console_type,
};
use atomik_model::{
    ClipId, Command, DeviceKey, Document, ExpandedDevice, ExpandedRack, INTERFACE_KEY, ModelError,
    RackDevice, RackDoc, SocketRef, SongCommand, SyncOp, empty_doc, load_project, new_project_doc,
    register_stand_ins, save_patch,
};
use atomik_render::project::song_input_with_warps;
use atomik_ui::{
    ArrangementView, Canvas, ChannelMeters, ConsoleView, ContainerEditorView, PianoRollView,
    PluginParamsView, RackView, SampleEditorView, SeqEditorView, SliceEditorView, StepEditorView,
    Theme, TransportAction, TransportBar, UiAction, UiEvent,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// File errors.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// Reading or writing a file failed.
    #[error("{path}: {source}")]
    Io {
        /// The file.
        path: PathBuf,
        /// The cause.
        source: std::io::Error,
    },
    /// The file is not a valid project or patch.
    #[error(transparent)]
    Model(#[from] ModelError),
    /// `save` without a project file name (new, or opened from a patch): use save-as.
    #[error("the project has no file name yet; use Save As")]
    NoPath,
    /// The project bundle (audio files) could not be written.
    #[error("project audio: {0}")]
    Audio(#[from] atomik_audio_file::AudioFileError),
}

/// Velocity of auditioned notes (piano-roll keyboard and note edits).
pub const AUDITION_VELOCITY: f32 = 0.8;

pub use crate::settings::DEFAULT_METRONOME_GAIN;

/// The extension of project files.
pub const PROJECT_EXTENSION: &str = "atomik";

/// A part of the window that receives events (layout).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Pane {
    /// The rack view (top).
    Rack,
    /// The transport bar.
    Transport,
    /// The arrangement (below the transport bar).
    Arrangement,
    /// The piano roll (in place of the arrangement).
    PianoRoll,
    /// The console (in place of the arrangement, or maximised).
    Console,
    /// The sample editor (in place of the arrangement).
    SampleEditor,
    /// The step editor (in place of the arrangement).
    StepEditor,
    /// The slice editor (in place of the arrangement).
    SliceEditor,
    /// The sequencer grid editor (in place of the arrangement).
    SeqEditor,
    /// The container editor, splits and macros (in place of the arrangement).
    ContainerEditor,
    /// The plugin parameter pane (in place of the arrangement).
    PluginParams,
    /// The file browser, docked on the left of the window.
    FileBrowser,
}
/// The values an effect can publish at most (the EQ's 18 is the largest) with headroom.
pub const FX_METER_CAPACITY: usize = 32;

/// Metered outputs of a console channel or bus: main out left/right, then the gain-reduction CV.
pub const CHANNEL_METER_SOCKETS: [&str; 3] = ["out_l", "out_r", "gr_cv_out"];
/// Metered outputs of the master: main out left/right, then the bus compressor's GR CV.
pub const MASTER_METER_SOCKETS: [&str; 3] = ["main_l", "main_r", "gr_cv_out"];

/// One frame of a strip's meters from its three metered outputs.
///
/// - Output peak/RMS: the `out_l`/`out_r` (master: `main_l`/`main_r`) readings as they are
///   (linear amplitude; the peak is the largest sample since the previous read, the RMS is over
///   the last engine batch).
/// - Gain reduction: `gr_cv_out` carries `1 - g` (g = the dynamics' linear gain), so the view's
///   gain factor is `1 - peak(gr_cv_out)`, clamped to 0..=1. The CV's *peak* is used, so the
///   deepest reduction since the previous frame is shown even when it lasted less than a frame.
/// - A missing reading (not metered yet, or the routing is being swapped) reads as silence
///   (and no reduction).
pub fn channel_meters(
    out_l: Option<atomik_engine::MeterReading>,
    out_r: Option<atomik_engine::MeterReading>,
    gr_cv: Option<atomik_engine::MeterReading>,
) -> ChannelMeters {
    let l = out_l.unwrap_or_default();
    let r = out_r.unwrap_or_default();
    let reduction = gr_cv.map_or(0.0, |g| g.peak);
    let gr = if reduction.is_finite() {
        (1.0 - reduction).clamp(0.0, 1.0)
    } else {
        1.0
    };
    ChannelMeters {
        out_peak_l: l.peak,
        out_peak_r: r.peak,
        out_rms_l: l.rms,
        out_rms_r: r.rms,
        gr,
    }
}

/// The app state without a window: document, registry, rack view, the device key ↔ engine id
/// map, the engine topology and the engine sink.
///
/// # Engine sync rules
///
/// - Engine ids are allocated monotonically from 1 and never reused while an engine lives.
///   [`Self::rebuild_engine`] (open, new, audio restart) starts a fresh engine and
///   assigns 1, 2, 3… in rack order, exactly like `atomik-render` builds a patch, so devices are
///   prepared with the same seeds as the offline render of the saved file.
/// - The [`SyncOp`]s of one edit are processed in order: `DeviceAdded` inserts the device with
///   its stored params (a container: its whole inside) (the `ParamChanged`s that follow for it are folded into the insert),
///   `RoutingChanged` rebuilds the [`Topology`] from the document and submits its routing,
///   `DeviceRemoved` removes the instance (always after the routing that dropped it),
///   `ParamChanged` and `TempoChanged` go to the sink, `ViewChanged` needs no engine work.
/// - **Song.** `SongChanged` and `TempoChanged` mark the song stale; it is
///   rebuilt ([`atomik_render::project::song_input`]: effective tracks, flattened notes and automation, the tempo
///   map), compiled ([`compile_song`]) and submitted before the next device or routing op of
///   the same edit, else after the last op. So an added device is inserted and routed before
///   a song that uses it, and a removed device's track leaves the song before the routing
///   without it and before `remove_device`. A clone of the submitted snapshot is kept for
///   recording and position maths. The engine loop follows the song loop (`SetLoop` is sent
///   when the loop changes, and after every tempo change).
/// - **Samplers.** A sampler gets its sample map right after its insert (so after
///   every restart too, in the restart order) and on every `ZonesChanged`, one
///   [`EngineSink::set_device_samples`] call each; see `app_core_samples.rs` for how the map
///   is built (always RAM) and when assets arriving later resend it.
/// - **Containers.** The engine never sees a container: it gets [`RackDoc::expand`] (ids
///   1, 2, 3... in the expansion order on a restart, devices keyed by path key, a container
///   standing for its `~split` device and its inside). See `app_core_containers.rs` for the
///   container ops, macro values, split params and notes through the split.
/// - Engine errors never undo an edit; they are shown like a rejected command.
///
/// # Transport
///
/// [`TransportAction`]s map to [`TransportCommand`]s: Play, Stop, TogglePlay (by the app's
/// play state), ReturnToZero = `Locate(0)`, `Locate(t)`. ToggleLoop is the undoable song
/// edit `SetLoop` with the flag inverted (the engine follows through the song sync).
/// ToggleMetronome flips the click (`Metronome { enabled, gain }`). ToggleRecord starts a
/// [`Take`] on the armed tracks (and plays) or ends it; Stop and Locate end it too.
/// [`Self::frame`] reads the engine's transport state and shows it in the views.
pub struct AppCore<S: EngineSink> {
    registry: Registry,
    /// The in-process clipboard; New and Open keep it.
    clipboard: Clipboard,
    /// Tracks the creation chain armed: the next auto-route disarms them
    /// (and only them). Session state, forgotten when a document is replaced.
    auto_armed: Vec<DeviceKey>,
    /// The arrangement's selected track the last arming followed (see `follow_arrangement_track`).
    arranged_track: Option<DeviceKey>,
    /// A new arrangement track selection awaits its pointer release to hand the keys to the rack.
    arrangement_track_changed: bool,
    /// The console selection the last arming followed (see `follow_console_selection`).
    console_selection: Vec<DeviceKey>,
    /// The snapshot the mix was last stored, updated, recalled or compared with (see
    /// `app_core_snapshots.rs`); view state, forgotten when a document is replaced.
    snapshot_hint: Option<u32>,
    /// MIDI learn, the mapping runtime and the held mapped moves (see `app_core_midimap.rs`).
    midi: midimap::MidiControl,
    /// The current macro bank, the last touched params, the search index (see `app_core_macros.rs`).
    macros: macro_ops::MacroState,
    /// The control surface driving the console, if one is set up (see `app_core_surface.rs`).
    surface: Option<crate::surface::SurfaceHost>,
    /// A plain click selected exactly one strip: the shell moves the focus to the rack.
    rack_focus_request: bool,
    /// `follow_console_selection` saw a new console selection (cleared by the pane event).
    selection_changed: bool,
    document: Document,
    view: RackView,
    ids: BTreeMap<DeviceKey, DeviceInstanceId>,
    next_id: u32,
    topology: Topology,
    sink: S,
    file: Option<PathBuf>,
    saved: RackDoc,
    target: Arc<NoteTarget>,
    rejection: Option<String>,
    arrangement: ArrangementView,
    piano_roll: PianoRollView,
    sample_editor: SampleEditorView,
    step_editor: StepEditorView,
    slice_editor: SliceEditorView,
    seq_editor: SeqEditorView,
    container_editor: ContainerEditorView,
    /// The container editor is shown (the view has no close of its own).
    container_open: bool,
    /// The params the engine has for each split device (only the changed ones are re-sent).
    split_sent: BTreeMap<DeviceKey, BTreeMap<String, f32>>,
    /// Every (flat device, param) a macro drives and the value the engine has for it, so that a
    /// macro change sends only what moved and a removed mapping can release its param.
    macro_sent: BTreeMap<(DeviceKey, String), f32>,
    /// The flat devices whose published meter values feed the views (metered effects, players).
    meter_devices: Vec<DeviceKey>,
    /// The VCA feed last sent to each strip (a strip not listed has the default one), so only
    /// a change reaches the engine; forgotten when the engine is rebuilt.
    vca_sent: BTreeMap<DeviceKey, VcaFeed>,
    /// The spectrum and phase meter devices of the flat rack and their analysis state.
    analysers: analysers::Analysers,
    /// The pattern clipboard both sequencer views last agreed on.
    pattern_clip_seen: Option<atomik_model::PatternDoc>,
    transport_bar: TransportBar,
    piano_clip: Option<ClipId>,
    snapshot: Option<SongSnapshot>,
    sent_loop: Option<(i64, i64, bool)>,
    playing: bool,
    metronome: bool,
    metronome_gain: f32,
    quantize: InputQuantize,
    take: Option<Take>,
    /// Automation recording: the armed lanes and the running take's lane recorders.
    autorec: autorec::AutoRec,
    position: i64,
    recorded: Vec<RecordedNote>,
    console: ConsoleView,
    /// Metered endpoints per console strip (channels in console order, then the master).
    meter_points: Vec<(DeviceKey, [Endpoint; 3])>,
    /// The engine latency epoch the current routing was compiled with.
    latency_epoch: u64,
    /// Per-channel latency readouts of the current routing (frames).
    readouts: Vec<(DeviceKey, u32)>,
    /// The current routing hit the 10 s compensation limit.
    pdc_limit: bool,
    /// The Audio I/O panel's metered endpoints.
    io_meters: io_ops::IoMeters,
    /// The CV cables shown with an activity indicator on the back.
    cv_taps: io_ops::CvTaps,
    /// How many meters the CV indicators may fill the bank up to (the engine's
    /// [`atomik_engine::METER_CAPACITY`]; a field so a test can shrink it).
    meter_capacity: usize,
    /// Project bundle, imported assets, peaks and background workers.
    audio: AudioState,
    /// Host inputs the running engine was built with (a change of the document's count
    /// restarts the engine).
    engine_inputs: u16,
    /// A Play waiting for the streamer's read-ahead: UI frames waited so far.
    pending_play: Option<u32>,
    /// Effect meter feed: the last publish sequence applied per device.
    fx_meter_seq: BTreeMap<DeviceKey, u64>,
    /// Effect meter feed: the reused read buffer (never reallocated after construction).
    fx_meter_buf: Vec<f32>,
    /// The plugin parameter pane and whether it is shown.
    plugin_params: PluginParamsView,
    plugin_params_open: bool,
    file_browser: atomik_ui::FileBrowserView,
    browser: browser::BrowserState,
    /// Third-party plugins: the manager, the catalog, the instances. Declared after
    /// the sink so the engine's devices drop before the manager that owns their plugins.
    plugins: plugins::PluginState,
}

impl<S: EngineSink + std::fmt::Debug> std::fmt::Debug for AppCore<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppCore")
            .field("file", &self.file)
            .field("devices", &self.doc().devices.len())
            .field("ids", &self.ids)
            .field("sink", &self.sink)
            .finish_non_exhaustive()
    }
}

/// The built-in device registry (the devices of `atomik-devices`).
pub fn default_registry() -> Registry {
    let mut registry = Registry::new();
    // The built-in set is fixed; its descriptors are validated by atomik-devices' tests.
    atomik_devices::register_all(&mut registry).expect("built-in devices register cleanly");
    registry
}

/// `registry` without the console devices (`atomik.channel`, `atomik.master`): documents
/// built with it get no automatic channels. The stress rack (a rack stress test, not a
/// console one) and the sync tests use it.
pub fn without_console(registry: &Registry) -> Registry {
    let mut out = Registry::new();
    for type_id in registry.type_ids().filter(|t| !is_console_type(t)) {
        if let Some(f) = registry.get(type_id) {
            // Already validated by `registry`, and the ids are unique there.
            let _ = out.register(*f);
        }
    }
    out
}

/// The engine configuration for a document (host inputs as the document's `input_channels`).
pub fn engine_config(doc: &RackDoc) -> EngineConfig {
    EngineConfig {
        // The model only accepts the standard rates and 20..=999 BPM (validated on load and
        // by `SetTempo`), so the fallbacks are unreachable for a document.
        sample_rate: SampleRate::new(f64::from(doc.sample_rate))
            .unwrap_or_else(|_| SampleRate::new(48_000.0).expect("48 kHz is valid")),
        tempo: Tempo::new(doc.tempo_bpm)
            .unwrap_or_else(|_| Tempo::new(120.0).expect("120 BPM is valid")),
        // The interface's `in_1..in_N` sockets: the rack's cables from them resolve. Until the
        // duplex input stream exists they carry silence (missing host channels read as zeros).
        input_channels: doc.input_channels,
        output_channels: doc.output_channels,
        seed: doc.seed,
    }
}

/// The first notes input of a descriptor.
pub fn notes_input(desc: &DeviceDescriptor) -> Option<SocketId> {
    atomik_render::project::notes_input(desc)
}

impl<S: EngineSink> AppCore<S> {
    /// A core with an empty rack ([`atomik_model::empty_doc`]); the engine is reset and gets
    /// one (empty) routing.
    pub fn new(sink: S) -> Self {
        Self::with_doc(sink, empty_doc(), None)
    }

    /// A core showing `doc` (e.g. a patch given on the command line), loaded from `file`.
    pub fn with_doc(sink: S, doc: RackDoc, file: Option<PathBuf>) -> Self {
        Self::with_registry(sink, default_registry(), doc, file)
    }

    /// A core with its own device registry (e.g. [`without_console`] for rack-only tests).
    pub fn with_registry(sink: S, registry: Registry, doc: RackDoc, file: Option<PathBuf>) -> Self {
        let theme = Theme::dark();
        let mut view = RackView::new(theme.clone());
        view.set_layout_provider(atomik_devices::panel_layout);
        let config = engine_config(&doc);
        let target = sink.note_target().unwrap_or_default();
        let hub = sink.control_hub().unwrap_or_default();
        let mut core = Self {
            registry,
            clipboard: Clipboard::new(),
            auto_armed: Vec::new(),
            arranged_track: None,
            arrangement_track_changed: false,
            console_selection: Vec::new(),
            snapshot_hint: None,
            midi: midimap::MidiControl::new(hub),
            macros: macro_ops::MacroState::default(),
            surface: None,
            rack_focus_request: false,
            selection_changed: false,
            saved: doc.clone(),
            document: Document::new(doc),
            view,
            ids: BTreeMap::new(),
            next_id: 1,
            topology: Topology::new(&config),
            sink,
            file,
            target,
            rejection: None,
            arrangement: ArrangementView::new(theme.clone()),
            piano_roll: PianoRollView::new(theme.clone()),
            sample_editor: SampleEditorView::new(theme.clone()),
            step_editor: StepEditorView::new(theme.clone()),
            slice_editor: SliceEditorView::new(theme.clone()),
            seq_editor: SeqEditorView::new(theme.clone()),
            container_editor: ContainerEditorView::new(theme.clone()),
            container_open: false,
            split_sent: BTreeMap::new(),
            macro_sent: BTreeMap::new(),
            meter_devices: Vec::new(),
            vca_sent: BTreeMap::new(),
            analysers: analysers::Analysers::default(),
            pattern_clip_seen: None,
            transport_bar: TransportBar::new(theme.clone()),
            console: ConsoleView::new(theme.clone()),
            meter_points: Vec::new(),
            latency_epoch: 0,
            readouts: Vec::new(),
            pdc_limit: false,
            io_meters: io_ops::IoMeters::default(),
            cv_taps: io_ops::CvTaps::default(),
            meter_capacity: atomik_engine::METER_CAPACITY,
            audio: AudioState::new(),
            engine_inputs: 0,
            pending_play: None,
            fx_meter_seq: BTreeMap::new(),
            fx_meter_buf: vec![0.0; FX_METER_CAPACITY],
            plugin_params: PluginParamsView::new(theme),
            file_browser: atomik_ui::FileBrowserView::new(Theme::dark()),
            browser: browser::BrowserState::default(),
            plugin_params_open: false,
            plugins: plugins::PluginState::new(),
            piano_clip: None,
            snapshot: None,
            sent_loop: None,
            playing: false,
            metronome: false,
            metronome_gain: DEFAULT_METRONOME_GAIN,
            quantize: InputQuantize::Off,
            take: None,
            autorec: autorec::AutoRec::new(),
            position: 0,
            recorded: Vec::new(),
        };
        core.audio_reset();
        core.rebuild_engine();
        core.audio_opened();
        core.midi_maps_changed();
        core
    }

    /// The device registry.
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// The document with its history.
    pub fn document(&self) -> &Document {
        &self.document
    }

    /// The current rack.
    pub fn doc(&self) -> &RackDoc {
        self.document.doc()
    }

    /// The rack view.
    pub fn view(&self) -> &RackView {
        &self.view
    }

    /// The rack view, mutably (zoom, flip, cable visibility, browser…). Call
    /// [`Self::after_view_change`] after selecting a device through it.
    pub fn view_mut(&mut self) -> &mut RackView {
        &mut self.view
    }

    /// The engine sink.
    pub fn sink(&self) -> &S {
        &self.sink
    }

    /// The engine sink, mutably.
    pub fn sink_mut(&mut self) -> &mut S {
        &mut self.sink
    }

    /// The engine routing model (mirrors the document's cables with engine ids).
    pub fn topology(&self) -> &Topology {
        &self.topology
    }

    /// The engine instance id of a device.
    pub fn device_id(&self, key: &DeviceKey) -> Option<DeviceInstanceId> {
        self.ids.get(key).copied()
    }

    /// The file the rack was loaded from or last saved to.
    pub fn file(&self) -> Option<&Path> {
        self.file.as_deref()
    }

    /// True when the rack differs from the last opened or saved state.
    pub fn is_dirty(&self) -> bool {
        self.doc() != &self.saved
    }

    /// The window title: `Atomik Rack — <file name>` (`Untitled` without a file), with ` *`
    /// when there are unsaved changes (rack or song).
    pub fn title(&self) -> String {
        let name = self.file.as_ref().and_then(|p| p.file_name()).map_or_else(
            || "Untitled".to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        let dirty = if self.is_dirty() { " *" } else { "" };
        format!("Atomik Rack \u{2014} {name}{dirty}")
    }

    /// The latest rejected command or engine error (also shown by the view until the next
    /// click).
    pub fn last_rejection(&self) -> Option<&str> {
        self.rejection.as_deref()
    }

    // ------------------------------------------------------------------ edits

    /// Applies a command and syncs the engine and the view. On rejection nothing changes and
    /// the view shows the error.
    pub fn apply_command(&mut self, command: Command) -> Result<(), ModelError> {
        // The creation chain tells the model which tracks an earlier auto-route armed.
        let command = self.prepare_creation(command);
        // A workflow command (paste, duplicate, join) selects what it created once it ran.
        let workflow = match &command {
            Command::Workflow(w) => {
                let w = w.clone();
                let pre = self.workflow_pre(&w);
                Some((w, pre))
            }
            _ => None,
        };
        // A device that goes away takes its latest plugin state into the undo history.
        self.plugins_flush_dirty();
        let merge = param_merge(&command);
        let hint = self.snapshot_hint_after(&command);
        match self.document.apply(command, &self.registry) {
            Ok(ops) => {
                if let Some(hint) = hint {
                    self.snapshot_hint = hint;
                    self.console.set_snapshot_hint(hint);
                }
                self.sync_ops(&ops);
                self.capture_automation(&ops, merge);
                if let Some((w, pre)) = workflow {
                    self.workflow_post(&w, pre);
                }
                Ok(())
            }
            Err(e) => {
                self.reject(e.to_string());
                Err(e)
            }
        }
    }

    /// Handles an action emitted by [`RackView::handle`].
    pub fn handle_action(&mut self, action: UiAction) {
        match action {
            UiAction::Command(c) => {
                // In MIDI Learn mode a change to a control arms it instead; otherwise a
                // rejection is already reported to the view.
                if !self.learn_swallows(&c) {
                    let _ = self.apply_command(c);
                }
            }
            UiAction::EndMerge => {
                self.document.end_merge();
                self.release_automation();
            }
            // The param being touched: the arrangement follows it when it is set to.
            UiAction::ParamTouched { device, param } => self.param_touched(&device, &param),
            UiAction::ToggleLaneArm { device, param } => {
                self.toggle_automation_arm(&device, &param);
            }
            // "Set Tempo..." on a tempo point: the transport bar's tempo entry takes that event.
            UiAction::EditTempoPoint(i) => self.transport_bar.begin_tempo_edit(Some(i)),
            UiAction::Undo => {
                self.undo();
            }
            UiAction::Redo => {
                self.redo();
            }
            // A device picked in the console (an insert slot) is selected in the rack too.
            UiAction::Select(key) => self.view.set_selected(Some(key)),
            UiAction::OpenBrowser => {}
            // A note clip of a drum machine opens in the step editor (View > Piano Roll
            // still opens it in the piano roll).
            UiAction::OpenPianoRoll(id) => match self.drum_device_of_clip(id) {
                Some(device) => self.open_step_editor(device, Some(id)),
                None => self.open_piano_roll(id),
            },
            UiAction::ClosePianoRoll => self.close_piano_roll(),
            UiAction::OpenSampleEditor(key) => self.open_sample_editor(key),
            UiAction::CloseSampleEditor => self.close_sample_editor(),
            UiAction::OpenStepEditor { device, clip } => self.open_step_editor(device, clip),
            UiAction::CloseStepEditor => self.close_step_editor(),
            UiAction::CreateStepClip { device } => self.create_step_clip(device),
            UiAction::OpenSeqEditor { device } => self.open_seq_editor(device),
            UiAction::OpenContainerEditor { device, macros } => {
                self.open_container_editor(device, macros)
            }
            UiAction::CloseContainerEditor => self.close_container_editor(),
            // The view switched its scope itself; the host needs nothing more.
            UiAction::EnterContainer { .. } | UiAction::ExitContainer { .. } => {}
            UiAction::CloseSeqEditor => self.close_seq_editor(),
            UiAction::SelectPattern { device, index } => {
                self.select_pattern(&device, usize::from(index))
            }
            UiAction::OpenSliceEditor(key) => self.open_slice_editor(key),
            UiAction::CloseSliceEditor => self.close_slice_editor(),
            UiAction::SliceLoop { device, mode } => self.slice_loop(device, mode),
            UiAction::SlicesToTrack { device } => self.slices_to_track(device),
            UiAction::ImportSamples {
                paths,
                device,
                at_key,
            } => self.import_samples(paths, device, at_key),
            UiAction::AuditionVelocity {
                device,
                key,
                velocity,
                on,
            } => self.audition_velocity(&device, key, velocity, on),
            UiAction::Transport(t) => {
                if !self.learn_swallows_transport(t) {
                    self.transport_action(t);
                }
            }
            UiAction::Browse(b) => self.browse(b),
            UiAction::Audition { device, key, on } => self.audition(&device, key, on),
            a @ (UiAction::OpenPluginGui { .. }
            | UiAction::OpenPluginParams { .. }
            | UiAction::ClosePluginParams
            | UiAction::ReloadPlugin { .. }
            | UiAction::AutomatePluginParam { .. }
            | UiAction::PluginParamTexts { .. }
            | UiAction::PluginSettings(_)
            | UiAction::PluginBanner(_)) => {
                self.plugin_action(&a);
            }
            UiAction::ImportAudio {
                paths,
                target,
                tick,
            } => self.import_audio(paths, target, tick),
            UiAction::InsertLibrary {
                path,
                container,
                slot,
            } => self.load_library_item(&path, LoadTarget::Rack { container, slot }),
            UiAction::InsertLibraryClip {
                path,
                track,
                lane,
                tick,
            } => self.load_library_item(
                &path,
                LoadTarget::Track {
                    device: track,
                    lane,
                    tick,
                },
            ),
            // `UiAction` is non-exhaustive: later phases may add actions.
            _ => {}
        }
        self.after_view_change();
    }

    /// Forwards a UI event to the rack view and handles the resulting actions, which are
    /// returned for the shell (selection, browser).
    pub fn handle_event(&mut self, event: &UiEvent) -> Vec<UiAction> {
        self.handle_pane_event(Pane::Rack, event)
    }

    /// Forwards a UI event to one pane's view (in that view's coordinates: the rack view's
    /// own, the sequencer views' window-logical ones) and handles the actions.
    pub fn handle_pane_event(&mut self, pane: Pane, event: &UiEvent) -> Vec<UiAction> {
        let doc = self.document.doc();
        let actions = match pane {
            Pane::Rack => self.view.handle(event, doc, &self.registry),
            Pane::Transport => self.transport_bar.handle(event, doc, &self.registry),
            Pane::Arrangement => self.arrangement.handle(event, doc, &self.registry),
            Pane::PianoRoll => self.piano_roll.handle(event, doc, &self.registry),
            Pane::Console => self.console.handle(event, doc, &self.registry),
            Pane::SampleEditor => self.sample_editor.handle(event, doc, &self.registry),
            Pane::StepEditor => self.step_editor.handle(event, doc, &self.registry),
            Pane::SliceEditor => self.slice_editor.handle(event, doc, &self.registry),
            Pane::SeqEditor => self.seq_editor.handle(event, doc, &self.registry),
            Pane::ContainerEditor => self.container_editor.handle(event, doc, &self.registry),
            Pane::PluginParams => self.plugin_params.handle(event, doc, &self.registry),
            Pane::FileBrowser => self.file_browser.handle(event),
        };
        for action in &actions {
            self.handle_action(action.clone());
        }
        if pane == Pane::Arrangement {
            self.follow_arrangement_track(event);
        }
        if pane == Pane::Console {
            self.follow_console_selection();
            // A plain click (no Shift/Ctrl) that left exactly one strip selected hands the keys
            // to the rack; multi-selection keeps the console (Ctrl+G, scroll keys).
            let plain = matches!(
                event,
                UiEvent::PointerUp { modifiers, .. } if !modifiers.shift && !modifiers.command()
            );
            self.rack_focus_request |=
                plain && self.selection_changed && self.console.selected_strips().len() == 1;
            self.selection_changed = false;
        }
        self.after_view_change();
        actions
    }

    /// Undoes the last step; false when there is nothing to undo.
    pub fn undo(&mut self) -> bool {
        self.plugins_flush_dirty();
        match self.document.undo(&self.registry) {
            Some(ops) => {
                self.sync_ops(&ops);
                self.refresh_missing();
                true
            }
            None => false,
        }
    }

    /// Redoes the last undone step; false when there is nothing to redo.
    pub fn redo(&mut self) -> bool {
        self.plugins_flush_dirty();
        match self.document.redo(&self.registry) {
            Some(ops) => {
                self.sync_ops(&ops);
                self.refresh_missing();
                true
            }
            None => false,
        }
    }

    /// Selects a device (or none) and retargets live notes.
    pub fn select(&mut self, key: Option<DeviceKey>) {
        self.view.set_selected(key);
        self.after_view_change();
    }

    /// Re-syncs the views and publishes the live-notes target. Call after changing a view
    /// directly.
    pub fn after_view_change(&mut self) {
        let doc = self.document.doc();
        self.view.sync(doc, &self.registry);
        self.arrangement.sync(doc, &self.registry);
        self.piano_roll.sync(doc, &self.registry);
        self.sample_editor.sync(doc, &self.registry);
        self.step_editor.sync(doc, &self.registry);
        self.slice_editor.sync(doc, &self.registry);
        self.seq_editor.sync(doc, &self.registry);
        self.container_editor.sync(doc, &self.registry);
        if self.plugin_params_open {
            self.plugin_params.sync(doc, &self.registry);
        }
        self.sample_editor
            .set_missing_assets(self.audio.missing.iter().map(|m| m.hash.clone()));
        self.slice_editor
            .set_missing_assets(self.audio.missing.iter().map(|m| m.hash.clone()));
        self.transport_bar.sync(doc, &self.registry);
        self.console.sync(doc, &self.registry);
        // The sampler was removed: the arrangement shows again.
        if self.sample_editor.device().is_some() && !self.sample_editor.is_present() {
            self.close_sample_editor();
        }
        if self.step_editor.device().is_some() && !self.step_editor.is_present() {
            self.close_step_editor();
        }
        if self.slice_editor.device().is_some() && !self.slice_editor.is_present() {
            self.close_slice_editor();
        }
        if self.seq_editor.device().is_some() && !self.seq_editor.is_present() {
            self.close_seq_editor();
        }
        if self.container_open && !self.container_editor.is_present() {
            self.close_container_editor();
        }
        if self.plugin_params_open && !self.plugin_params.is_present() {
            self.close_plugin_params();
        }
        self.share_pattern_clipboard();
        self.sync_midi_marks();
        self.follow_selected_device();
        self.target.set(self.notes_target());
    }

    // ------------------------------------------------------------------ files

    /// Replaces the rack with the starter project (Audio I/O, the locked
    /// master and the two `main` cables; a fresh engine and an empty undo history).
    pub fn new_document(&mut self) {
        self.replace(new_project_doc(), None);
    }

    /// Opens project or patch JSON as if read from `path`. On error nothing changes.
    pub fn open_str(&mut self, json: &str, path: Option<PathBuf>) -> Result<(), AppError> {
        let doc = load_project(json, &self.registry)?;
        // The plugins of the file that are not installed get stand-ins built from its layouts.
        register_stand_ins(&doc, &mut self.registry)?;
        self.replace(doc, path);
        self.plugins_after_open();
        Ok(())
    }

    /// Replaces the rack with `doc` (e.g. a generated stress rack), with no file name.
    pub fn open_doc(&mut self, doc: RackDoc) {
        // A generated document may use plugin types the registry does not know yet.
        let _ = register_stand_ins(&doc, &mut self.registry);
        self.replace(doc, None);
        self.plugins_after_open();
    }

    /// Writes the rack as a patch v2 file (no song) to `path`; the current file and the
    /// saved state do not change.
    ///
    /// A patch has no plugin objects, so a rack with plugin devices is refused with a message
    /// naming them (use Save As for a project).
    pub fn export_patch(&self, path: &Path) -> Result<(), AppError> {
        let plugins = self.doc().plugin_devices();
        if !plugins.is_empty() {
            let names: Vec<String> = plugins.iter().map(|(_, d)| d.name.clone()).collect();
            return Err(AppError::Model(ModelError::BadPlugin(format!(
                "a patch cannot hold third-party plugins ({}); save a project instead",
                names.join(", ")
            ))));
        }
        write_file(path, &save_patch(self.doc()))
    }

    fn replace(&mut self, doc: RackDoc, file: Option<PathBuf>) {
        // A new document does not keep the old one's plugin instances.
        self.plugins_discard_on_next_reset();
        self.plugin_params_open = false;
        self.saved = doc.clone();
        self.document = Document::new(doc);
        self.file = file;
        self.rejection = None;
        self.auto_armed.clear();
        self.arranged_track = None;
        self.console_selection.clear();
        self.snapshot_hint = None;
        self.console.set_snapshot_hint(None);
        self.midi_replaced();
        self.view.set_selected(None);
        self.close_piano_roll();
        self.close_sample_editor();
        self.close_step_editor();
        self.close_slice_editor();
        self.close_seq_editor();
        self.close_container_editor();
        // A new project shows the top level of the rack (the scope is view state).
        self.view.set_scope(&[]);
        self.arrangement.set_selected_clips(&[]);
        self.arrangement.set_selected_track(None);
        self.arrangement.set_selected_points(ClipId(0), &[]);
        self.position = 0;
        self.audio_reset();
        self.rebuild_engine();
        self.audio_opened();
    }

    // ------------------------------------------------------------------ engine

    /// Starts a fresh engine from the document: reset, then every device inserted with its
    /// stored params under ids 1, 2, 3… in rack order, then one routing, the song, the
    /// engine loop and the metronome. The transport starts stopped; a take in progress ends
    /// without a clip.
    pub fn rebuild_engine(&mut self) {
        if self.take.take().is_some() {
            self.sink.set_recording(&[]);
        }
        self.cancel_automation_take();
        self.cancel_audio_recording();
        self.playing = false;
        self.pending_play = None;
        let config = engine_config(self.doc());
        self.engine_inputs = config.input_channels;
        self.audio.monitor_sent.clear();
        self.plugins_before_reset();
        self.sink.reset(&config);
        self.plugins_after_reset();
        self.ids.clear();
        self.split_sent.clear();
        self.vca_sent.clear();
        self.fx_meter_seq.clear();
        self.analysers.restart();
        self.next_id = 1;
        self.sent_loop = None;
        // Containers are compiled away: the engine gets the expanded rack.
        let expanded = self.doc().expand(&self.registry);
        self.macro_sent = containers::macro_values(&expanded);
        self.meter_devices = plugins::metered_with_plugins(&expanded);
        self.analysers
            .set_devices(analysers::analyser_keys(&expanded));
        for device in &expanded.devices {
            // A frozen device keeps its place in the id order but is not built.
            if device.frozen {
                self.next_id += 1;
                continue;
            }
            self.insert_flat(device);
        }
        self.plugins_after_rebuild();
        self.submit_routing_with(&expanded);
        self.push_solo_silence();
        self.push_vca_feeds();
        self.push_monitor_live();
        self.submit_song(true);
        self.send_metronome();
        self.show_transport();
        self.sync_io_host();
        self.after_view_change();
    }

    /// Per-frame housekeeping: drops what the engine handed back, reads the transport state,
    /// converts recorded live notes and shows the position in the views.
    pub fn frame(&mut self) {
        self.sink.collect_garbage();
        self.plugins_frame();
        self.poll_background();
        // A device latency changed (e.g. a channel's `latency.adjust`): recompile with it.
        if self.sink.latency_epoch() != self.latency_epoch {
            self.submit_routing();
        }
        self.drive_pending_play();
        let state = self.sink.transport_state();
        if state.playing || state.engine_frame > 0 {
            self.position = state.tick;
        }
        self.feed_take(&state);
        self.show_transport();
        self.feed_device_meters();
        self.feed_device_scopes();
        self.sync_io_host();
        self.feed_io_meters();
        self.browser_frame();
    }

    /// Feeds the rack view the values every metered effect publishes
    /// (`Device::meter_values`: compressor GR, EQ analyser, reverb tail, distortion peaks,
    /// mastering loudness), once per frame from [`Self::frame`].
    ///
    /// It covers every device of a type with a visual that publishes values
    /// ([`atomik_ui::fx::FxKind::meter_count`] above 0), whether or not its panel is on
    /// screen (cheap: a lock-free read per effect; the view repaints only for visible
    /// panels). A device whose publish sequence did not move since the last feed is skipped,
    /// so a paused engine costs no view work. Allocation-free after the first feed of a
    /// device (one key clone), with one reused buffer.
    pub fn feed_device_meters(&mut self) {
        let mut buf = std::mem::take(&mut self.fx_meter_buf);
        // Effects with a display and the note players (the sequencer's cursor, the
        // arpeggiator's held and upcoming keys) publish values; the list holds the
        // flat devices, so the ones inside containers are fed by their path key.
        for key in &self.meter_devices {
            let Some(id) = self.ids.get(key) else {
                continue;
            };
            let Some((n, seq)) = self.sink.read_device_meter_values(*id, &mut buf) else {
                continue;
            };
            match self.fx_meter_seq.get_mut(key) {
                Some(last) if *last == seq => continue,
                Some(last) => *last = seq,
                None => {
                    self.fx_meter_seq.insert(key.clone(), seq);
                }
            }
            self.view.set_device_meter_values(key, &buf[..n]);
            if self.seq_editor.device() == Some(key) {
                self.seq_editor.set_meter_values(&buf[..n]);
            }
        }
        self.fx_meter_buf = buf;
    }

    fn sync_ops(&mut self, ops: &[SyncOp]) {
        // Devices (flat keys) inserted by this edit: they already got their stored params.
        let mut inserted: Vec<DeviceKey> = Vec::new();
        // The expansion of the document, computed once for the edit when an op needs it (the
        // document does not change while its ops are processed).
        let mut expanded: Option<ExpandedRack> = None;
        // (song stale, tempo changed): flushed before device and routing ops, and at the end.
        let mut stale = (false, false);
        let mut restart = false;
        for op in ops {
            if matches!(
                op,
                SyncOp::DeviceAdded(_) | SyncOp::DeviceRemoved(_) | SyncOp::RoutingChanged
            ) {
                self.flush_song(&mut stale);
            }
            match op {
                SyncOp::DeviceAdded(key) => {
                    let exp = expanded.get_or_insert_with(|| self.doc().expand(&self.registry));
                    // A container stands for its whole inside: its `~split` and every
                    // inner device, in the expansion's order.
                    let subtree = containers::subtree(self.document.doc(), exp, key);
                    for device in subtree {
                        self.fx_meter_seq.remove(&device.key);
                        self.audio.monitor_sent.remove(&device.key);
                        self.insert_flat(device);
                        inserted.push(device.key.clone());
                    }
                    self.fx_meter_seq.remove(key);
                    self.audio.monitor_sent.remove(key);
                    // A new strip starts from the default feed.
                    self.vca_sent.remove(key);
                    inserted.push(key.clone());
                }
                SyncOp::DeviceRemoved(key) => {
                    for flat in containers::flat_keys_of(&self.ids, key) {
                        self.fx_meter_seq.remove(&flat);
                        self.audio.monitor_sent.remove(&flat);
                        self.split_sent.remove(&flat);
                        self.plugins_device_removed(&flat);
                        if let Some(id) = self.ids.remove(&flat) {
                            let result = self.sink.remove_device(id);
                            self.engine_result(result);
                        }
                    }
                    self.fx_meter_seq.remove(key);
                    self.audio.monitor_sent.remove(key);
                    self.vca_sent.remove(key);
                }
                // A new input count needs a new engine: the restart below submits it.
                SyncOp::RoutingChanged if self.doc().input_channels != self.engine_inputs => {}
                SyncOp::RoutingChanged => {
                    let exp = expanded.get_or_insert_with(|| self.doc().expand(&self.registry));
                    self.submit_routing_with(exp);
                }
                SyncOp::ParamChanged { key, param, value } => {
                    if inserted.contains(key) || self.echo_suppressed() {
                        // A device inserted by this edit already got its stored params; a
                        // value the plugin's own window made is not sent back to it.
                    } else if self.doc().resolve(key).is_some_and(|d| {
                        d.inner.is_some() || d.type_id == atomik_model::MACRO_BANK_TYPE
                    }) {
                        // A container's or a bank's macro: one `set_param` per target.
                        let exp = expanded.get_or_insert_with(|| self.doc().expand(&self.registry));
                        self.send_macro_values(exp);
                    } else {
                        self.send_param(key, param, *value);
                    }
                }
                SyncOp::TempoChanged(bpm) => {
                    if let Ok(tempo) = Tempo::new(*bpm) {
                        let result = self.sink.set_tempo(tempo);
                        self.engine_result(result);
                    }
                    stale = (true, true);
                }
                SyncOp::ViewChanged => {}
                // Plugin pins, run mode, power or link: power and run mode reach the
                // live instance, a relink makes the engine rebuild.
                SyncOp::PluginChanged { .. } => self.plugins_sync_op(op),
                SyncOp::SongChanged => stale.0 = true,
                // The graph changes (devices leave, a player comes): a new engine.
                SyncOp::FrozenChanged => restart = true,
                // A sampler inserted by this edit already got its map after the insert.
                SyncOp::ZonesChanged(key) => {
                    if !inserted.contains(key) {
                        self.send_samples(key);
                    }
                }
                // A sequencer inserted by this edit already got its data after the insert.
                SyncOp::PatternsChanged { device } => {
                    if !inserted.contains(device) {
                        self.send_player_data(device);
                    }
                }
                // A mapping edit: every mapped target, and the params a removed mapping frees.
                SyncOp::MacrosChanged { .. } => {
                    let exp = expanded.get_or_insert_with(|| self.doc().expand(&self.registry));
                    self.send_macro_values(exp);
                    // Automation of a macro lane expands per target: the song follows the mapping.
                    stale.0 = true;
                }
                // The `~split` device's params come from the splits.
                SyncOp::SplitsChanged { container } => {
                    let exp = expanded.get_or_insert_with(|| self.doc().expand(&self.registry));
                    self.send_split_params(exp, container);
                }
                SyncOp::MidiMapsChanged => self.midi_maps_changed(),
                SyncOp::ConsoleChanged => {
                    if self.topology.compensation() != self.doc().console.compensation {
                        self.submit_routing();
                    }
                    self.push_solo_silence();
                    self.push_vca_feeds();
                }
            }
        }
        // A bank or a target that came or went changes what the macros drive: the targets of a
        // removed bank get their stored values back and those of a restored one the macro's.
        let membership = ops
            .iter()
            .any(|op| matches!(op, SyncOp::DeviceAdded(_) | SyncOp::DeviceRemoved(_)));
        if membership && (!self.macro_sent.is_empty() || !self.doc().macros.assigns.is_empty()) {
            let exp = expanded.get_or_insert_with(|| self.doc().expand(&self.registry));
            self.send_macro_values(exp);
        }
        if let Some(exp) = &expanded {
            self.macro_sent = containers::macro_values(exp);
            self.meter_devices = plugins::metered_with_plugins(exp);
            self.analysers.set_devices(analysers::analyser_keys(exp));
        }
        self.flush_song(&mut stale);
        if restart || self.doc().input_channels != self.engine_inputs {
            // The host stream opens the document's input count, or a track was frozen or
            // unfrozen: restart the engine.
            self.rebuild_engine();
            return;
        }
        self.push_monitor_live();
        if let Some(id) = self.piano_clip
            && self.doc().song.clip(id).is_none()
        {
            self.close_piano_roll();
        }
        self.after_view_change();
    }

    fn flush_song(&mut self, stale: &mut (bool, bool)) {
        if stale.0 {
            self.submit_song(stale.1);
        }
        *stale = (false, false);
    }

    /// Builds, compiles and submits the song; then syncs the engine loop (always when
    /// `tempo_changed`, else only when the song loop differs from what was sent).
    fn submit_song(&mut self, tempo_changed: bool) {
        // Warped clips play their rendered buffers: start what is missing, drop what is stale.
        self.sync_warps();
        let ids = &self.ids;
        let input = song_input_with_warps(
            self.document.doc(),
            &self.registry,
            |k| ids.get(k).copied(),
            self.audio.assets(),
            &self.audio.warps,
        );
        match compile_song(&input) {
            Ok(snapshot) => {
                self.snapshot = Some(snapshot.clone());
                let result = self.sink.submit_song(&input, snapshot);
                self.engine_result(result);
            }
            Err(e) => self.reject(format!("song: {e}")),
        }
        let song = &self.document.doc().song;
        let wanted = (song.loop_range.0, song.loop_range.1, song.loop_enabled);
        if tempo_changed || self.sent_loop != Some(wanted) {
            self.sent_loop = Some(wanted);
            self.send_transport(TransportCommand::SetLoop {
                start: Ticks(wanted.0),
                end: Ticks(wanted.1),
                enabled: wanted.2,
            });
        }
    }

    /// Creates the flat device `device` and hands it to the sink (which prepares it off the RT
    /// thread) with its stored params (macro values applied), under the next id. The container
    /// itself has no engine device: only the expansion's devices come here.
    fn insert_flat(&mut self, device: &ExpandedDevice) {
        // A plugin device is inserted as its stand-in (or, after an engine restart, as the live
        // device of its parked instance); the instance itself is created by a later frame.
        let plugin_device = match &device.plugin {
            Some(pd) => self.plugin_device_for_insert(device, pd),
            None => None,
        };
        let Some(factory) = self.registry.get(&device.type_id) else {
            let message = format!("unknown device type `{}`", device.type_id);
            self.reject(message);
            return;
        };
        let desc = plugin_device
            .as_ref()
            .map_or(factory.descriptor, |(_, d)| *d);
        let params: Vec<(ParamId, f32)> = device
            .params
            .iter()
            .filter_map(|(k, v)| desc.param_by_key(k).map(|p| (p.id, *v)))
            .collect();
        if device.type_id == atomik_model::SPLIT_TYPE {
            let sent = self.full_split_params(&device.params);
            self.split_sent.insert(device.key.clone(), sent);
        }
        let instance = match plugin_device {
            Some((device, _)) => device,
            None => (factory.create)(),
        };
        let id = DeviceInstanceId(self.next_id);
        self.next_id += 1;
        self.ids.insert(device.key.clone(), id);
        let result = self.sink.insert_device(id, instance, &params);
        self.engine_result(result);
        // Insert, then samples, then pattern data: the engine applies them in queue order.
        self.send_samples_after_insert(&device.key);
        self.send_player_data(&device.key);
    }

    fn send_param(&mut self, key: &DeviceKey, param: &str, value: f32) {
        let Some(id) = self.ids.get(key).copied() else {
            return;
        };
        let Some(pid) = containers::type_of(self.doc(), key)
            .and_then(|t| self.registry.get(&t))
            .and_then(|f| f.descriptor.param_by_key(param))
            .map(|p| p.id)
        else {
            return;
        };
        let result = self.sink.set_param(id, pid, value);
        self.engine_result(result);
    }

    /// Pushes [`RackDoc::solo_silence`] into every console strip's (channel's or bus's)
    /// `internal.solo_silence` param, through the engine only (never a model command: the
    /// param is not stored, saved or undoable).
    fn push_solo_silence(&mut self) {
        let silence = self.doc().solo_silence(&self.registry);
        for (key, silent) in silence {
            let Some(type_id) = self.doc().device(&key).map(|d| d.type_id.clone()) else {
                continue;
            };
            let Some(pid) = self
                .registry
                .get(&type_id)
                .and_then(|f| f.descriptor.param_by_key(SOLO_SILENCE_PARAM))
                .map(|p| p.id)
            else {
                continue;
            };
            if let Some(id) = self.ids.get(&key).copied() {
                let value = if silent { 1.0 } else { 0.0 };
                let result = self.sink.set_param(id, pid, value);
                self.engine_result(result);
            }
        }
    }

    /// Pushes [`RackDoc::vca_feeds`] into the strips' `internal.vca_db`, `internal.vca_mute` and
    /// `internal.vca_solo` params, through the engine only (like `internal.solo_silence`: never a
    /// model command, not stored, saved or undoable). Only what changed since the last push is
    /// sent, so a fader drag costs one param.
    fn push_vca_feeds(&mut self) {
        for (key, feed) in self.doc().vca_feeds() {
            let sent = self.vca_sent.get(&key).copied().unwrap_or(VcaFeed::NONE);
            if sent == feed {
                continue;
            }
            let Some(id) = self.ids.get(&key).copied() else {
                continue;
            };
            let Some(type_id) = self.doc().device(&key).map(|d| d.type_id.clone()) else {
                continue;
            };
            let changes = [
                (
                    VCA_DB_PARAM,
                    feed.db,
                    sent.db.to_bits() != feed.db.to_bits(),
                ),
                (
                    VCA_MUTE_PARAM,
                    f32::from(u8::from(feed.mute)),
                    sent.mute != feed.mute,
                ),
                (
                    VCA_SOLO_PARAM,
                    f32::from(u8::from(feed.solo)),
                    sent.solo != feed.solo,
                ),
            ];
            for (param, value, changed) in changes {
                if !changed {
                    continue;
                }
                let Some(pid) = self
                    .registry
                    .get(&type_id)
                    .and_then(|f| f.descriptor.param_by_key(param))
                    .map(|p| p.id)
                else {
                    continue;
                };
                let result = self.sink.set_param(id, pid, value);
                self.engine_result(result);
            }
            if feed.is_none() {
                self.vca_sent.remove(&key);
            } else {
                self.vca_sent.insert(key, feed);
            }
        }
    }

    /// Rebuilds the topology from the document (nodes in rack order, cables in canonical
    /// order) and submits its routing.
    ///
    /// The topology also gets the console's compensation switch, a meter on every
    /// console strip's outputs ([`CHANNEL_METER_SOCKETS`], [`MASTER_METER_SOCKETS`]) and the
    /// engine's current device latencies ([`EngineSink::fill_latencies`]); the compiled PDC
    /// report feeds the console view's latency readouts and warning.
    fn submit_routing(&mut self) {
        let expanded = self.doc().expand(&self.registry);
        self.submit_routing_with(&expanded);
    }

    /// [`Self::submit_routing`] for the expansion of the current document (the engine graph is
    /// the flat rack).
    fn submit_routing_with(&mut self, expanded: &ExpandedRack) {
        let epoch = self.sink.latency_epoch();
        let doc = self.document.doc();
        let config = engine_config(doc);
        let mut topology = Topology::new(&config);
        topology.set_compensation(doc.console.compensation);
        let mut problems = Vec::new();
        for device in &expanded.devices {
            let (Some(id), Some(f)) = (
                self.ids.get(&device.key),
                self.registry.get(&device.type_id),
            ) else {
                continue;
            };
            if let Err(e) = topology.add_node(*id, f.descriptor) {
                problems.push(format!("{}: {e}", device.key));
            }
        }
        for cable in &expanded.cables {
            let ends = (
                self.endpoint(&topology, &cable.from),
                self.endpoint(&topology, &cable.to),
            );
            let result = match ends {
                (Some(from), Some(to)) => topology.connect(from, to),
                _ => Err(GraphError::UnknownSocket),
            };
            if let Err(e) = result {
                problems.push(format!("{} -> {}: {e}", cable.from, cable.to));
            }
        }
        let console = &doc.console;
        let strips = console
            .channels
            .iter()
            .map(|c| (&c.channel, CHANNEL_TYPE, CHANNEL_METER_SOCKETS))
            .chain(
                console
                    .buses
                    .iter()
                    .map(|b| (&b.bus, BUS_TYPE, CHANNEL_METER_SOCKETS)),
            )
            .chain(
                console
                    .master
                    .iter()
                    .map(|m| (m, MASTER_TYPE, MASTER_METER_SOCKETS)),
            );
        let mut meter_points = Vec::new();
        for (key, type_id, sockets) in strips {
            let is_type = doc.device(key).is_some_and(|d| d.type_id == type_id);
            let ends = sockets.map(|s| {
                self.endpoint(
                    &topology,
                    &SocketRef {
                        device: key.clone(),
                        socket: s.to_string(),
                    },
                )
            });
            if let (true, [Some(l), Some(r), Some(g)]) = (is_type, ends) {
                for e in [l, r, g] {
                    // Outputs of a node of the topology: always accepted.
                    let _ = topology.add_meter(e);
                }
                meter_points.push((key.clone(), [l, r, g]));
            }
        }
        // The Audio I/O sockets' meters (read-only; they never touch a sample).
        let mut io_meters = self.plan_io_meters(&mut topology, expanded, &meter_points);
        let cv_taps = self.plan_cv_taps(&mut topology, expanded, &meter_points, &mut io_meters);
        self.sink.fill_latencies(&mut topology);
        let compiled = topology.compile();
        self.topology = topology;
        self.meter_points = meter_points;
        self.io_meters = io_meters;
        self.cv_taps = cv_taps;
        self.latency_epoch = epoch;
        for p in problems {
            self.reject(format!("engine routing: {p}"));
        }
        match compiled {
            Ok(plan) => {
                self.show_pdc(plan.pdc_report());
                let result = self.sink.submit_routing(plan);
                self.engine_result(result);
            }
            Err(e) => self.reject(format!("engine routing: {e}")),
        }
    }

    /// Latency readouts and the limit warning from a compiled routing. A channel's readout is
    /// its output latency (`out_l` in the report: the latency of everything up to and
    /// including the channel, inserts and `latency.adjust` included), so the largest readout
    /// is the path latency the other channels are delayed to; the readout does not change
    /// when compensation is switched off.
    fn show_pdc(&mut self, report: &PdcReport) {
        let latency_of = |e: Endpoint| {
            report
                .latencies
                .iter()
                .find(|(x, _)| *x == e)
                .map(|(_, l)| *l)
        };
        let console = &self.document.doc().console;
        let channels: Vec<&DeviceKey> = console
            .channels
            .iter()
            .map(|c| &c.channel)
            .chain(console.buses.iter().map(|b| &b.bus))
            .collect();
        self.readouts = self
            .meter_points
            .iter()
            .filter(|(k, _)| channels.contains(&k))
            .map(|(k, ends)| (k.clone(), latency_of(ends[0]).unwrap_or(0)))
            .collect();
        self.pdc_limit = report.limit_exceeded;
        self.console.set_latency_readouts(&self.readouts);
        self.console.set_pdc_warning(self.pdc_limit);
    }

    /// Per-channel latency readouts of the current routing, in console order (frames).
    pub fn latency_readouts(&self) -> &[(DeviceKey, u32)] {
        &self.readouts
    }

    /// True when the current routing had to switch compensation off because a delay would
    /// exceed [`MAX_COMPENSATION_SECONDS`].
    pub fn pdc_limit_exceeded(&self) -> bool {
        self.pdc_limit
    }

    /// The alert for the status bar while compensation is disabled by the limit.
    pub fn pdc_alert(&self) -> Option<String> {
        self.pdc_limit.then(|| {
            format!("delay compensation off: a delay would exceed {MAX_COMPENSATION_SECONDS:.0} s")
        })
    }

    /// This frame's meter values per console strip (channels in console order, then the
    /// master), read lock-free from the engine; see [`channel_meters`].
    pub fn console_meter_readings(&self) -> Vec<(DeviceKey, ChannelMeters)> {
        self.console_readings_with(&[])
    }

    /// The values the device `key` publishes for the UI (gain reduction, loudness, band
    /// energies...) into `out`, as `(count, publish sequence)`; `None` for an unknown device,
    /// one that publishes nothing, or while no engine runs. Poll once per frame; lock-free.
    pub fn device_meter_values(&self, key: &DeviceKey, out: &mut [f32]) -> Option<(usize, u64)> {
        let id = *self.ids.get(key)?;
        self.sink.read_device_meter_values(id, out)
    }

    /// Feeds the console view this frame's meters, then the monotonic time in seconds (call
    /// once per frame while the console shows).
    pub fn update_console_meters(&mut self, now_seconds: f64) {
        // The endpoints read earlier this frame (a read resets the peak); the cache stays for
        // the CV feed and is cleared by the next frame's read.
        let shared = std::mem::take(&mut self.io_meters.shared);
        let readings = self.console_readings_with(&shared);
        self.io_meters.shared = shared;
        self.console.set_meters(&readings);
        self.console.set_frame_time(now_seconds);
    }

    /// The console view.
    pub fn console(&self) -> &ConsoleView {
        &self.console
    }

    /// Whether the last console click asks for the rack to take the focus (a plain click that
    /// selected one strip); reading it clears it.
    pub fn take_rack_focus_request(&mut self) -> bool {
        std::mem::take(&mut self.rack_focus_request)
    }

    /// The console view, mutably (bounds, scale, focus).
    pub fn console_mut(&mut self) -> &mut ConsoleView {
        &mut self.console
    }

    fn endpoint(&self, topology: &Topology, socket: &SocketRef) -> Option<Endpoint> {
        let device = if socket.device.as_str() == INTERFACE_KEY {
            Graph::INTERFACE
        } else {
            *self.ids.get(&socket.device)?
        };
        let id = topology
            .descriptor(device)?
            .socket_by_key(&socket.socket)?
            .id;
        Some(Endpoint { device, socket: id })
    }

    fn engine_result(&mut self, result: Result<(), EngineError>) {
        if let Err(e) = result {
            self.reject(format!("engine: {e}"));
        }
    }

    /// Shows an error in the view (until the next click) and records it as the last
    /// rejection.
    pub fn report(&mut self, message: impl Into<String>) {
        self.reject(message.into());
    }

    fn reject(&mut self, message: String) {
        self.view.notify_rejected(message.clone());
        // The console shows it too: it may be maximised over the rack.
        self.console.notify_rejected(message.clone());
        self.rejection = Some(message);
    }

    // ------------------------------------------------------------------ output buses

    /// The console strips "Route to new output bus" acts on. With the console focused it is the
    /// console's selection (when it has one); otherwise the rack's selection, where a channel or
    /// bus device stands for itself and an instrument for its channel.
    pub fn strips_to_route(&self, console_focus: bool) -> Vec<DeviceKey> {
        if console_focus {
            let selected = self.console.selected_strips();
            if !selected.is_empty() {
                return selected;
            }
        }
        let console = &self.doc().console;
        let mut strips: Vec<DeviceKey> = Vec::new();
        for key in self.view.selection() {
            let strip =
                if console.channel_index(&key).is_some() || console.bus_index(&key).is_some() {
                    Some(key)
                } else {
                    console.channel_of(&key).map(|c| c.channel.clone())
                };
            if let Some(strip) = strip
                && !strips.contains(&strip)
            {
                strips.push(strip);
            }
        }
        strips
    }

    /// Ctrl/Cmd+G: creates a bus named `Bus n` and routes the selected channels (and buses) to
    /// it, in one undo step; the new bus is then selected in the console. False, with a message,
    /// when nothing is selected or the model refuses.
    pub fn route_selection_to_new_bus(&mut self, console_focus: bool) -> bool {
        let strips = self.strips_to_route(console_focus);
        if strips.is_empty() {
            self.reject("Select one or more channels to route to a new output bus".into());
            return false;
        }
        let bus = atomik_model::console::new_bus_key(self.doc());
        let name = self.doc().console.default_bus_name();
        let command =
            Command::Console(atomik_model::ConsoleCommand::RouteToNewBus { strips, name });
        if self.apply_command(command).is_err() {
            return false;
        }
        self.console.set_selected_strips(&[bus]);
        self.after_view_change();
        true
    }

    // ------------------------------------------------------------------ VCA groups

    /// The console strips the VCA commands act on, by what has the focus: the console's selection
    /// (when it has one), the arrangement's selected tracks (each as its console channel, a
    /// selected folder standing for its tracks), else the rack's selection as for
    /// [`Self::strips_to_route`].
    pub fn strips_for_vca(&self, pane: Pane) -> Vec<DeviceKey> {
        if pane == Pane::Arrangement {
            let console = &self.doc().console;
            let mut strips: Vec<DeviceKey> = Vec::new();
            for track in self.arrangement().selected_tracks() {
                if let Some(c) = console.channel_of(&track)
                    && !strips.contains(&c.channel)
                {
                    strips.push(c.channel.clone());
                }
            }
            if !strips.is_empty() {
                return strips;
            }
        }
        self.strips_to_route(pane == Pane::Console)
    }

    /// What the Mixer menu's VCA items can do with the selection `pane` has.
    pub fn vca_menu(&self, pane: Pane) -> crate::commands::VcaMenu {
        let strips = self.strips_for_vca(pane);
        let console = &self.doc().console;
        crate::commands::VcaMenu {
            can_create: !strips.is_empty(),
            can_add: !strips.is_empty() && !console.vcas.is_empty(),
            can_remove: strips.iter().any(|s| console.vca_of(s).is_some()),
            can_delete: !console.vcas.is_empty(),
        }
    }

    /// Mixer > Create VCA from Selection: a group named `VCA n` over the selected strips (they
    /// leave the groups they were in), in one undo step. False, with a message, when nothing is
    /// selected or the model refuses.
    pub fn create_vca_from_selection(&mut self, pane: Pane) -> bool {
        let members = self.strips_for_vca(pane);
        if members.is_empty() {
            self.reject("Select one or more channels or tracks to make a VCA from".into());
            return false;
        }
        let name = self.doc().console.default_vca_name();
        let command = Command::Console(atomik_model::ConsoleCommand::CreateVca {
            name,
            members: members.clone(),
        });
        if self.apply_command(command).is_err() {
            return false;
        }
        self.console.set_selected_strips(&members);
        self.after_view_change();
        true
    }

    /// Mixer > Add to VCA: the selected strips join group `vca`, or the last group when none is
    /// named, in one undo step. False, with a message, when there is nothing to add or no group.
    pub fn add_selection_to_vca(&mut self, pane: Pane, vca: Option<u32>) -> bool {
        let strips = self.strips_for_vca(pane);
        if strips.is_empty() {
            self.reject("Select one or more channels or tracks to add to a VCA".into());
            return false;
        }
        let Some(id) = vca.or_else(|| self.doc().console.vcas.last().map(|v| v.id)) else {
            self.reject("There is no VCA group yet: use Create VCA from Selection".into());
            return false;
        };
        let command = Command::Console(atomik_model::ConsoleCommand::AssignToVca {
            vca: Some(id),
            strips,
        });
        self.apply_command(command).is_ok()
    }

    /// Mixer > Remove from VCA: the selected strips leave their groups, in one undo step.
    pub fn remove_selection_from_vca(&mut self, pane: Pane) -> bool {
        let strips = self.strips_for_vca(pane);
        if strips.is_empty() {
            self.reject("Select one or more channels or tracks to take out of their VCAs".into());
            return false;
        }
        let command =
            Command::Console(atomik_model::ConsoleCommand::AssignToVca { vca: None, strips });
        self.apply_command(command).is_ok()
    }

    /// Mixer > Delete VCA: group `vca`, or else the groups the selected strips are in (each one
    /// undo step; the members keep their faders). False, with a message, when no group matches.
    pub fn delete_vca(&mut self, pane: Pane, vca: Option<u32>) -> bool {
        let ids: Vec<u32> = match vca {
            Some(id) => vec![id],
            None => {
                let console = &self.doc().console;
                let mut ids: Vec<u32> = Vec::new();
                for s in self.strips_for_vca(pane) {
                    if let Some(g) = console.vca_of(&s)
                        && !ids.contains(&g.id)
                    {
                        ids.push(g.id);
                    }
                }
                ids
            }
        };
        if ids.is_empty() {
            self.reject("Select a channel in a VCA group to delete it".into());
            return false;
        }
        ids.into_iter().all(|id| {
            self.apply_command(Command::Console(atomik_model::ConsoleCommand::DeleteVca {
                vca: id,
            }))
            .is_ok()
        })
    }

    // ------------------------------------------------------------------ channel strip upgrades

    /// What the Mixer menu's polarity and insert bypass items can do with the selection `pane`
    /// has.
    pub fn strip_menu(&self, pane: Pane) -> crate::commands::StripMenu {
        let strips = self.strips_for_vca(pane);
        let doc = self.doc();
        crate::commands::StripMenu {
            can_polarity: !strips.is_empty(),
            can_bypass: strips
                .iter()
                .any(|s| atomik_model::inserts_bypassed(doc, &self.registry, s).is_some()),
        }
    }

    /// Mixer > Toggle Polarity: the selected strips' `input.phase`, in one undo step: all
    /// inverted switch back to normal, otherwise every strip is inverted. False, with a message,
    /// when nothing is selected.
    pub fn toggle_polarity(&mut self, pane: Pane) -> bool {
        let strips = self.strips_for_vca(pane);
        if strips.is_empty() {
            self.reject("Select one or more channels or tracks to flip the polarity of".into());
            return false;
        }
        let inverted = |core: &Self, s: &DeviceKey| {
            core.doc()
                .device(s)
                .and_then(|d| d.params.get("input.phase"))
                .is_some_and(|v| *v >= 0.5)
        };
        let all = strips.iter().all(|s| inverted(self, s));
        let commands: Vec<Command> = strips
            .iter()
            .filter(|s| inverted(self, s) == all)
            .map(|s| Command::SetParam {
                key: s.clone(),
                param: "input.phase".into(),
                value: if all { 0.0 } else { 1.0 },
                merge: None,
            })
            .collect();
        self.apply_batch(commands)
    }

    /// Mixer > Bypass All Inserts: the insert effects of the selected strips, in one undo step:
    /// when every effect that can be bypassed already is, they switch back in, otherwise all are
    /// bypassed. Each effect keeps its delay compensation. False, with a message, when nothing
    /// is selected or the strips have nothing to bypass.
    pub fn bypass_all_inserts(&mut self, pane: Pane) -> bool {
        let strips = self.strips_for_vca(pane);
        if strips.is_empty() {
            self.reject("Select one or more channels or tracks to bypass the inserts of".into());
            return false;
        }
        let doc = self.doc();
        let states: Vec<bool> = strips
            .iter()
            .filter_map(|s| atomik_model::inserts_bypassed(doc, &self.registry, s))
            .collect();
        if states.is_empty() {
            self.reject("The selected strips have no insert effects to bypass".into());
            return false;
        }
        let bypass = !states.iter().all(|on| *on);
        let commands: Vec<Command> = strips
            .iter()
            .flat_map(|s| atomik_model::bypass_inserts_commands(doc, &self.registry, s, bypass))
            .collect();
        self.apply_batch(commands)
    }

    /// Applies `commands` as one undo step (a single command as itself); true when the step
    /// was made or there was nothing to change.
    fn apply_batch(&mut self, mut commands: Vec<Command>) -> bool {
        match commands.len() {
            0 => true,
            1 => self.apply_command(commands.remove(0)).is_ok(),
            _ => self.apply_command(Command::Batch(commands)).is_ok(),
        }
    }

    // ------------------------------------------------------------------ live notes

    /// The notes input of device `key` in the engine, if it has one.
    fn target_of(&self, key: &DeviceKey) -> Option<Target> {
        // A container's notes go through its `~split` device.
        let (flat, type_id) = containers::notes_device(self.doc(), key)?;
        let socket = notes_input(self.registry.get(&type_id)?.descriptor)?;
        Some((*self.ids.get(&flat)?, socket))
    }

    /// Where live MIDI notes go. With record-armed tracks: the selected device if it is
    /// armed, else the first armed track's device (an armed track receives the keyboard).
    /// Otherwise the selected device's first notes input if it has one, else the first device
    /// in rack order with a notes input.
    pub fn notes_target(&self) -> Option<Target> {
        let doc = self.doc();
        let selected = self.view.selected();
        let mut armed = doc.song.tracks.iter().filter(|t| t.armed);
        if let Some(first) = armed.next() {
            let pick = selected
                .filter(|k| doc.song.track(k).is_some_and(|t| t.armed))
                .unwrap_or(&first.device);
            if let Some(target) = self.target_of(pick) {
                return Some(target);
            }
        }
        selected
            .and_then(|k| self.target_of(k))
            .or_else(|| doc.devices.iter().find_map(|d| self.target_of(&d.key)))
    }

    /// The device live notes go to (see [`Self::notes_target`]).
    pub fn notes_target_device(&self) -> Option<&RackDevice> {
        let (id, _) = self.notes_target()?;
        let key = self.ids.iter().find(|(_, v)| **v == id)?.0;
        // A container's split stands for the container.
        let key = containers::container_of_split(key);
        self.doc().resolve(&key)
    }

    /// The shared live-notes target cell, read by the MIDI input thread.
    pub fn note_target_cell(&self) -> &Arc<NoteTarget> {
        &self.target
    }

    // ------------------------------------------------------------------ painting

    /// Paints the rack view (physical pixels; see the `atomik-ui` hosting contract).
    pub fn paint(&mut self, canvas: &mut dyn Canvas) {
        self.view.paint(canvas, self.document.doc(), &self.registry);
    }

    /// Paints one pane (physical pixels; the sequencer views draw inside their bounds).
    pub fn paint_pane(&mut self, pane: Pane, canvas: &mut dyn Canvas) {
        let doc = self.document.doc();
        match pane {
            Pane::Rack => self.view.paint(canvas, doc, &self.registry),
            Pane::Transport => self.transport_bar.paint(canvas, doc, &self.registry),
            Pane::Arrangement => self.arrangement.paint(canvas, doc, &self.registry),
            Pane::PianoRoll => self.piano_roll.paint(canvas, doc, &self.registry),
            Pane::Console => self.console.paint(canvas, doc, &self.registry),
            Pane::SampleEditor => self.sample_editor.paint(canvas, doc, &self.registry),
            Pane::StepEditor => self.step_editor.paint(canvas, doc, &self.registry),
            Pane::SliceEditor => self.slice_editor.paint(canvas, doc, &self.registry),
            Pane::SeqEditor => self.seq_editor.paint(canvas, doc, &self.registry),
            Pane::ContainerEditor => self.container_editor.paint(canvas, doc, &self.registry),
            Pane::PluginParams => self.plugin_params.paint(canvas, doc, &self.registry),
            Pane::FileBrowser => self.file_browser.paint(canvas),
        }
    }

    // ------------------------------------------------------------------ sequencer views

    /// The arrangement view.
    pub fn arrangement(&self) -> &ArrangementView {
        &self.arrangement
    }

    /// The arrangement view, mutably (bounds, scale, focus).
    pub fn arrangement_mut(&mut self) -> &mut ArrangementView {
        &mut self.arrangement
    }

    /// The piano roll.
    pub fn piano_roll(&self) -> &PianoRollView {
        &self.piano_roll
    }

    /// The piano roll, mutably (bounds, scale, focus).
    pub fn piano_roll_mut(&mut self) -> &mut PianoRollView {
        &mut self.piano_roll
    }

    /// The transport bar.
    pub fn transport_bar(&self) -> &TransportBar {
        &self.transport_bar
    }

    /// The transport bar, mutably (bounds, scale, focus).
    pub fn transport_bar_mut(&mut self) -> &mut TransportBar {
        &mut self.transport_bar
    }

    /// The clip open in the piano roll (shown in place of the arrangement).
    pub fn piano_roll_clip(&self) -> Option<ClipId> {
        self.piano_clip
    }

    /// Opens a note clip in the piano roll (ignored for other clips).
    pub fn open_piano_roll(&mut self, id: ClipId) {
        let is_notes = self
            .doc()
            .song
            .clip(id)
            .is_some_and(|(_, _, c)| c.content.is_notes());
        if is_notes {
            self.close_sample_editor();
            self.close_step_editor();
            self.close_slice_editor();
            self.close_seq_editor();
            self.close_container_editor();
            self.piano_roll.open(id);
            self.piano_clip = Some(id);
            self.piano_roll.sync(self.document.doc(), &self.registry);
        }
    }

    /// Opens the piano roll on the first selected note clip of the arrangement, else the
    /// first note clip of the selected device's track, else the song's first note clip.
    /// False when the song has no note clip.
    pub fn open_piano_roll_default(&mut self) -> bool {
        let song = &self.doc().song;
        let is_notes = |id: &ClipId| song.clip(*id).is_some_and(|(_, _, c)| c.content.is_notes());
        let first_in = |t: &atomik_model::Track| {
            t.lanes
                .iter()
                .filter(|l| l.kind.is_notes())
                .flat_map(|l| l.clips.iter())
                .map(|c| c.id)
                .next()
        };
        let id = self
            .arrangement
            .selected_clips()
            .into_iter()
            .find(is_notes)
            .or_else(|| {
                self.view
                    .selected()
                    .and_then(|k| song.track(k))
                    .and_then(first_in)
            })
            .or_else(|| song.tracks.iter().find_map(first_in));
        match id {
            Some(id) => {
                self.open_piano_roll(id);
                true
            }
            None => false,
        }
    }

    /// Closes the piano roll (the arrangement shows again).
    pub fn close_piano_roll(&mut self) {
        self.piano_roll.close();
        self.piano_clip = None;
    }

    // ------------------------------------------------------------------ transport

    /// The last submitted song snapshot (tick ↔ frame maths, recording conversion).
    pub fn song_snapshot(&self) -> Option<&SongSnapshot> {
        self.snapshot.as_ref()
    }

    /// Whether the app asked the engine to play.
    pub fn is_playing(&self) -> bool {
        self.playing
    }

    /// Whether a take is being recorded.
    pub fn is_recording(&self) -> bool {
        self.take.is_some()
    }

    /// Whether the metronome is on.
    pub fn metronome(&self) -> bool {
        self.metronome
    }

    /// The metronome level (linear gain).
    pub fn metronome_gain(&self) -> f32 {
        self.metronome_gain
    }

    /// Sets the metronome level (clamped to 0..=1) and sends it to the engine.
    pub fn set_metronome_gain(&mut self, gain: f32) {
        self.metronome_gain = if gain.is_finite() {
            gain.clamp(0.0, 1.0)
        } else {
            DEFAULT_METRONOME_GAIN
        };
        self.send_metronome();
    }

    /// The input quantize applied to recorded notes.
    pub fn input_quantize(&self) -> InputQuantize {
        self.quantize
    }

    /// Sets the input quantize for the next takes.
    pub fn set_input_quantize(&mut self, quantize: InputQuantize) {
        self.quantize = quantize;
    }

    /// The transport position shown (song ticks): the engine's when it runs, else the last
    /// locate.
    pub fn position(&self) -> i64 {
        self.position
    }

    /// Runs a transport request (see the type docs).
    pub fn transport_action(&mut self, action: TransportAction) {
        match action {
            TransportAction::Play => self.play(),
            TransportAction::Stop => self.stop(),
            TransportAction::TogglePlay if self.playing => self.stop(),
            TransportAction::TogglePlay => self.play(),
            TransportAction::ReturnToZero => self.locate(0),
            TransportAction::Locate(tick) => self.locate(tick.max(0)),
            TransportAction::ToggleLoop => {
                let song = &self.doc().song;
                let command = SongCommand::SetLoop {
                    range: song.loop_range,
                    enabled: !song.loop_enabled,
                };
                // A rejection is already reported.
                let _ = self.apply_command(Command::Song(command));
            }
            TransportAction::ToggleRecord if self.take.is_some() => {
                let state = self.sink.transport_state();
                self.finish_take(&state);
            }
            TransportAction::ToggleRecord => self.start_recording(),
            TransportAction::ToggleMetronome => {
                self.metronome = !self.metronome;
                self.send_metronome();
            }
        }
        self.show_transport();
    }

    fn send_transport(&mut self, command: TransportCommand) {
        let result = self.sink.transport(command);
        self.engine_result(result);
    }

    fn send_metronome(&mut self) {
        self.send_transport(TransportCommand::Metronome {
            enabled: self.metronome,
            gain: self.metronome_gain,
        });
    }

    fn play(&mut self) {
        // Streamed audio is prefetched only while the engine renders: wait for the read-ahead
        // (a few UI frames), so playback does not start with an underrun.
        if self.sink.stream_primed() {
            self.send_transport(TransportCommand::Play);
        } else {
            self.pending_play = Some(0);
        }
        self.playing = true;
    }

    fn stop(&mut self) {
        self.pending_play = None;
        let state = self.sink.transport_state();
        self.send_transport(TransportCommand::Stop);
        self.playing = false;
        self.finish_take(&state);
    }

    fn locate(&mut self, tick: i64) {
        // A take assumes an uninterrupted transport: a jump ends it.
        let state = self.sink.transport_state();
        self.finish_take(&state);
        self.send_transport(TransportCommand::Locate(Ticks(tick)));
        self.position = tick;
    }

    fn start_recording(&mut self) {
        if !self.doc().song.tracks.iter().any(|t| t.armed) && !self.has_armed_lanes() {
            self.reject(
                "recording needs an armed track or automation lane: arm one with its R button in the arrangement"
                    .to_string(),
            );
            return;
        }
        let audio = self.armed_audio_tracks();
        let song = &self.doc().song;
        let tracks: Vec<(DeviceInstanceId, DeviceKey, usize)> = song
            .tracks
            .iter()
            .filter(|t| t.armed && !audio.contains(&t.device))
            .filter_map(|t| {
                let flat = containers::flat_key(self.document.doc(), &t.device);
                let lane = self.arrangement.record_lane(&t.device);
                Some((*self.ids.get(&flat)?, t.device.clone(), lane))
            })
            .collect();
        let loop_range = song.loop_enabled.then_some(song.loop_range);
        let meter = song.meter_table();
        let audio_started = !audio.is_empty() && self.start_audio_recording(&audio);
        let automation_started = self.start_automation_take();
        if tracks.is_empty() && !audio_started && !automation_started {
            // Nothing can record: the reason was reported.
            return;
        }
        let take = Take::new(tracks, loop_range, meter);
        self.sink.set_recording(&take.devices());
        self.take = Some(take);
        if !self.playing {
            self.play();
        }
    }

    /// Converts newly captured notes with the latest transport state.
    fn feed_take(&mut self, state: &TransportState) {
        self.feed_audio_take(state);
        self.advance_automation(state);
        let Some(take) = &mut self.take else {
            return;
        };
        self.recorded.clear();
        self.sink.drain_recorded(&mut self.recorded);
        let sample_rate = f64::from(self.document.doc().sample_rate);
        if let Some(snapshot) = &self.snapshot {
            take.feed(&self.recorded, state, |s, frame| {
                snapshot.position_at(s, frame, sample_rate)
            });
        }
    }

    /// Ends the take at the transport position of `state` and writes its clips: per track one
    /// `AddClip`, or one `NewTakeGroup` when the loop made several passes (one undo step each).
    fn finish_take(&mut self, state: &TransportState) {
        if self.take.is_none() {
            return;
        }
        self.feed_take(state);
        let Some(take) = self.take.take() else {
            return;
        };
        self.sink.set_recording(&[]);
        self.finish_audio_take();
        self.finish_automation_take(state);
        let stop = SongPosition {
            song_frame: state.song_frame,
            tick: Ticks(state.tick),
            loop_count: state.loop_count,
        };
        // A track with several laps (the loop wrapped) gets one take group, the laps stacked as
        // takes with the last one playing; a single lap is a plain clip as before.
        let lanes = take.lanes().clone();
        let mut by_device: Vec<(DeviceKey, Vec<atomik_model::Clip>)> = Vec::new();
        for (device, clip) in take.finish(stop, self.quantize) {
            match by_device.iter_mut().find(|(d, _)| *d == device) {
                Some((_, clips)) => clips.push(clip),
                None => by_device.push((device, vec![clip])),
            }
        }
        for (device, mut clips) in by_device {
            // The lane picked when the take began, unless it is gone by now (or no longer a
            // note lane): then the first one.
            let lane = lanes
                .get(&device)
                .copied()
                .filter(|l| {
                    self.doc()
                        .song
                        .track(&device)
                        .is_some_and(|t| t.is_note_lane(*l))
                })
                .unwrap_or(0);
            let command = if clips.len() > 1 {
                SongCommand::NewTakeGroup {
                    device,
                    lane,
                    takes: clips,
                }
            } else {
                SongCommand::AddClip {
                    device,
                    lane,
                    clip: clips.remove(0),
                }
            };
            // A rejection is already reported.
            let _ = self.apply_command(Command::Song(command));
            self.document.end_merge();
        }
    }

    /// Shows the transport state in the views.
    fn show_transport(&mut self) {
        let recording = self.take.is_some();
        // The views follow the playhead only while the transport runs.
        self.arrangement.set_playing(self.playing);
        self.piano_roll.set_playing(self.playing);
        self.arrangement.set_playhead(self.position);
        self.arrangement.set_recording(recording);
        self.arrangement.set_armed_lanes(&self.autorec.armed);
        self.piano_roll.set_playhead(self.position);
        self.transport_bar.set_state(atomik_ui::TransportState {
            playing: self.playing,
            recording,
            metronome: self.metronome,
            position: self.position,
        });
    }

    /// Plays (`on`) or releases a key on a device through the live note path.
    pub fn audition(&mut self, device: &DeviceKey, key: u8, on: bool) {
        let kind = if on {
            NoteKind::On {
                key,
                velocity: AUDITION_VELOCITY,
            }
        } else {
            NoteKind::Off { key }
        };
        self.live_note_to(device, kind);
    }

    /// Sends one live note to `device` on the live note path.
    ///
    /// A note player (sequencer, arpeggiator) is not played itself: the note goes to what its
    /// notes output is cabled to ([`Self::audition_targets`]), so the grid editor's audition
    /// sounds the instrument the pattern drives.
    pub(super) fn live_note_to(&mut self, device: &DeviceKey, kind: NoteKind) {
        for target in self.audition_targets(device) {
            let result = self.sink.live_note(target, kind);
            self.engine_result(result);
        }
    }
}

/// The drag-merge id of a parameter command (the first one inside a batch): `Some` means a
/// control is being held.
fn param_merge(command: &Command) -> Option<u64> {
    match command {
        Command::SetParam { merge, .. } => *merge,
        Command::Batch(commands) => commands.iter().find_map(param_merge),
        _ => None,
    }
}

/// True if `path` has the project extension (`.atomik`, any case).
pub fn is_project_path(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(PROJECT_EXTENSION))
}

fn write_file(path: &Path, text: &str) -> Result<(), AppError> {
    std::fs::write(path, text).map_err(|source| AppError::Io {
        path: path.to_path_buf(),
        source,
    })
}

impl<S: EngineSink> Drop for AppCore<S> {
    fn drop(&mut self) {
        self.audio.remove_scratch();
    }
}

#[path = "app_core_audio.rs"]
mod audio;
use audio::AudioState;

#[path = "app_core_rec_audio.rs"]
mod rec;

#[path = "app_core_autorec.rs"]
mod autorec;

#[path = "app_core_samples.rs"]
mod samples;

#[path = "app_core_drums.rs"]
mod drums;

#[path = "app_core_slicer.rs"]
mod slicer;

#[path = "app_core_players.rs"]
mod players;

#[path = "app_core_analysers.rs"]
mod analysers;
pub use analysers::{MeterReadout, PHASE_VALUE_NAMES};

#[path = "app_core_containers.rs"]
mod containers;

#[path = "app_core_plugins.rs"]
mod plugins;

#[path = "app_core_workflow.rs"]
mod workflow;

#[path = "app_core_clipboard.rs"]
mod clipboard_ops;

#[path = "app_core_snapshots.rs"]
mod snapshots;

#[path = "app_core_midimap.rs"]
mod midimap;

#[path = "app_core_macros.rs"]
mod macro_ops;
pub use macro_ops::{Revealed, SEARCH_ROWS};

#[path = "app_core_surface.rs"]
mod surface_ops;
pub use midimap::{GESTURE_TIMEOUT, describe_message};

#[path = "app_core_audio_edit.rs"]
mod audio_edit_ops;
pub use audio_edit_ops::NORMALISE_TARGET_DB;

#[path = "app_core_warp.rs"]
mod warp;
pub use warp::WARP_CACHE_BYTES;

#[path = "app_core_notes.rs"]
mod notes;

#[path = "app_core_freeze.rs"]
mod freeze;
pub use freeze::{FREEZE_TAIL_SECONDS, FreezeThrough};

#[path = "app_core_quantize.rs"]
mod quantize_ops;
pub use quantize_ops::QuantizeTarget;

#[path = "app_core_midi.rs"]
mod midi_ops;
pub use midi_ops::{IMPORT_INSTRUMENT, MidiExport};

#[path = "app_core_create.rs"]
mod create_ops;

#[path = "app_core_io.rs"]
mod io_ops;

#[path = "app_core_browser.rs"]
mod browser;

#[path = "app_core_library.rs"]
mod library;
pub use library::LoadTarget;

pub use plugins::{
    LOG_CAPACITY, PluginConfig, PluginPaths, PluginWindowInfo, PluginWindowRequest, RECOVERY_KEEP,
    RECOVERY_PERIOD_SECS, STATE_QUIET_SECS,
};

#[cfg(test)]
#[path = "app_core_rec_audio_tests.rs"]
mod rec_audio_tests;

#[cfg(test)]
#[path = "app_core_freeze_tests.rs"]
mod freeze_tests;

#[cfg(test)]
#[path = "app_core_plugins_tests.rs"]
mod plugins_tests;

#[cfg(test)]
#[path = "app_core_plugins_gui_tests.rs"]
mod plugins_gui_tests;

#[cfg(test)]
#[path = "app_core_plugins_qa_tests.rs"]
mod plugins_qa_tests;

#[cfg(test)]
#[path = "app_core_plugins_vst3_tests.rs"]
mod plugins_vst3_tests;

#[cfg(test)]
#[path = "app_core_plugins_real_tests.rs"]
mod plugins_real_tests;

#[cfg(test)]
#[path = "app_core_plugins_latency_tests.rs"]
mod plugins_latency_tests;

#[cfg(test)]
#[path = "app_core_plugins_sandbox_tests.rs"]
mod plugins_sandbox_tests;

#[cfg(test)]
#[path = "app_core_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "app_core_autorec_tests.rs"]
mod autorec_tests;

#[cfg(test)]
#[path = "app_core_song_tests.rs"]
mod song_tests;

#[cfg(test)]
#[path = "app_core_console_tests.rs"]
mod console_tests;

#[cfg(test)]
#[path = "app_core_snapshots_tests.rs"]
mod snapshots_tests;

#[cfg(test)]
#[path = "app_core_samples_tests.rs"]
mod samples_tests;

#[cfg(test)]
#[path = "app_core_drums_tests.rs"]
mod drums_tests;

#[cfg(test)]
#[path = "app_core_slicer_tests.rs"]
mod slicer_tests;

#[cfg(test)]
#[path = "app_core_fx_tests.rs"]
mod fx_tests;

#[cfg(test)]
#[path = "app_core_analyser_tests.rs"]
mod analyser_tests;

#[cfg(test)]
#[path = "app_core_players_tests.rs"]
mod players_tests;

#[cfg(test)]
#[path = "app_core_character_tests.rs"]
mod character_tests;

#[cfg(test)]
#[path = "app_core_containers_tests.rs"]
mod containers_tests;

#[cfg(test)]
#[path = "app_core_macros_tests.rs"]
mod macros_tests;

#[cfg(test)]
#[path = "app_core_clipboard_tests.rs"]
mod clipboard_tests;

#[cfg(test)]
#[path = "app_core_console_select_tests.rs"]
mod console_select_tests;
#[cfg(test)]
#[path = "app_core_create_tests.rs"]
mod create_tests;

#[cfg(test)]
#[path = "app_core_starter_tests.rs"]
mod starter_tests;

#[cfg(test)]
#[path = "app_core_browser_tests.rs"]
mod browser_tests;

#[cfg(test)]
#[path = "app_core_library_tests.rs"]
mod library_tests;

#[cfg(test)]
#[path = "app_core_tempo_tests.rs"]
mod tempo_tests;

#[cfg(test)]
#[path = "app_core_marker_tests.rs"]
mod marker_tests;

#[cfg(test)]
#[path = "app_core_audio_edit_tests.rs"]
mod audio_edit_tests;

#[cfg(test)]
#[path = "app_core_warp_tests.rs"]
mod warp_tests;
