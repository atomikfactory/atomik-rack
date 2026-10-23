//! What the UI asks the app to do.

use atomik_model::{ClipId, Command, DeviceKey};
use std::path::PathBuf;

/// An action emitted by the views' `handle` methods ([`crate::RackView`],
/// [`crate::ArrangementView`], [`crate::PianoRollView`], [`crate::TransportBar`],
/// [`crate::ConsoleView`], [`crate::SampleEditorView`]). The UI never
/// mutates the document itself.
///
/// The enum is `#[non_exhaustive]`: hosts match the variants they handle and ignore the rest
/// with a wildcard arm, so later phases can add actions without breaking them.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum UiAction {
    /// Apply this command to the [`atomik_model::Document`], then call the view's `sync`.
    /// If it fails, report the error with [`crate::RackView::notify_rejected`].
    Command(Command),
    /// A drag ended (pointer released): call [`atomik_model::Document::end_merge`].
    EndMerge,
    /// Undo (Ctrl+Z / Cmd+Z).
    Undo,
    /// Redo (Ctrl+Shift+Z, Ctrl+Y / Cmd+Shift+Z).
    Redo,
    /// The selected device changed (the selection is that one device).
    Select(DeviceKey),
    /// The rack selection holds several devices: `primary` is the device
    /// the editors and the live keyboard follow (the last clicked), `keys` the whole selection
    /// in rack order (path keys inside a container). View state, not an undo step.
    SelectMany {
        /// The primary device (also in `keys`).
        primary: DeviceKey,
        /// The selected devices in rack order.
        keys: Vec<DeviceKey>,
    },
    /// Run a registry command, as if its key was pressed (a context-menu item): the
    /// shell resolves it against the pane the menu belongs to. `arg` is the command's
    /// argument, if it takes one.
    RunCommand {
        /// The command id (`edit.duplicate`).
        id: String,
        /// The command argument.
        arg: Option<String>,
    },
    /// The user right-clicked something that has a context menu. `at` is the pointer
    /// position in the coordinates of the event that caused it; the shell builds the menu
    /// from the registry for this `target` and the current selection.
    ContextMenu {
        /// What was clicked.
        target: ContextTarget,
        /// Where, in the emitting view's coordinates.
        at: vello::kurbo::Point,
    },
    /// The user asked for the device browser (double-click on empty rack space). The view
    /// has already shown it; the app may update menu state.
    OpenBrowser,
    /// Type a tempo for tempo event `i` (the context menu of the tempo lane): the app starts the
    /// transport bar's tempo entry for that event ([`crate::TransportBar::begin_tempo_edit`]).
    EditTempoPoint(usize),
    /// Open this note clip in the piano roll (double-click in the arrangement): call
    /// [`crate::PianoRollView::open`] and show the piano roll in place of the arrangement.
    OpenPianoRoll(ClipId),
    /// Close the piano roll and show the arrangement again.
    ClosePianoRoll,
    /// A transport request (buttons, shortcuts, ruler clicks).
    Transport(TransportAction),
    /// Preview a key on a device (piano-roll keyboard or a new/moved note): note on when
    /// `on`, note off otherwise. Every `on` is followed by a matching `off`.
    Audition {
        /// Device to play.
        device: DeviceKey,
        /// MIDI key 0..=127.
        key: u8,
        /// Note on (`true`) or off (`false`).
        on: bool,
    },
    /// Switch the lower pane between the arrangement and the console (F5, or the console's
    /// "ARRANGE" button). See [`crate::console_shortcut`].
    ToggleConsole,
    /// Maximise the console over the rack, or restore it (Shift+F5, or the console's "MAX"
    /// button). The app shows the console if it is hidden.
    MaximiseConsole,
    /// Import audio files: the user dropped `paths` on the arrangement.
    /// The UI never reads the files; the app decodes and registers each one (off the UI
    /// thread, reporting failures without touching the document), then adds clips.
    ///
    /// Placement rule, which the app implements:
    /// * [`ImportTarget::ExistingTrack`]: the clips go on that audio track, the first file
    ///   starting at `tick` and every further file right after the previous clip's end, in
    ///   the order of `paths`.
    /// * [`ImportTarget::NewTracks`]: one new audio track per file (`SongCommand::AddAudioTrack`
    ///   named after the file), each with its clip starting at `tick`.
    ///
    /// Each clip's length is the file's duration at the song tempo (the audio is not
    /// stretched). `tick` is the snapped tick under the pointer, never negative.
    ImportAudio {
        /// Dropped files, in drop order (opaque to the UI, any extension).
        paths: Vec<PathBuf>,
        /// Where the clips go.
        target: ImportTarget,
        /// Song tick of the first clip.
        tick: i64,
    },
    /// Show the file dialog of File > Import Audio...: the app picks files and continues as
    /// for [`UiAction::ImportAudio`] with the selected audio track (or new tracks) and the
    /// playhead as `tick`. The arrangement view itself has no menu and never emits this.
    ImportAudioDialog,
    /// Open the sample editor on this sampler (double-click on a sampler panel, or its Edit
    /// button): call [`crate::SampleEditorView::open`] and show it in place of the
    /// arrangement, like the piano roll.
    OpenSampleEditor(DeviceKey),
    /// Close the sample editor and show the arrangement again.
    CloseSampleEditor,
    /// The sample editor's Load button: the host shows the platform file dialog for audio
    /// files and then imports the picked files into `device` as [`UiAction::ImportSamples`]
    /// with `at_key: None` does.
    LoadSamplesDialog {
        /// The sampler being edited.
        device: DeviceKey,
    },
    /// Import sample files into a sampler: the user dropped `paths` on
    /// the sampler's rack panel (`at_key: None`) or on the sample editor (`at_key` is the key
    /// under the pointer when dropped on the key map, else `None`). The UI never reads the
    /// files; the app imports them (off the UI thread) and adds the zones in one undo step,
    /// mapped as [`atomik_model::auto_map`] does (`at_key` replaces C4 as the base key).
    ImportSamples {
        /// Dropped files, in drop order.
        paths: Vec<PathBuf>,
        /// The sampler.
        device: DeviceKey,
        /// The key under the pointer on the editor's key map.
        at_key: Option<u8>,
    },
    /// Open the step editor on this drum machine (the drum panel's Edit button, or a
    /// double-click on its pad grid): call [`crate::StepEditorView::open`] with `device` and
    /// the resolved clip and show it in place of the arrangement, like the piano roll.
    ///
    /// `clip` is `None` when the UI cannot know the clip (the rack never sees the selection
    /// or the playhead): the app resolves it with [`crate::StepEditorView::resolve_clip`]
    /// (the selected note clip on the device's track, else the note clip under the playhead,
    /// else `None`, which opens the editor's empty state with its "New pattern" button).
    OpenStepEditor {
        /// The drum machine.
        device: DeviceKey,
        /// The clip to edit, if already known.
        clip: Option<ClipId>,
    },
    /// Close the step editor and show the arrangement again.
    CloseStepEditor,
    /// The step editor's "New pattern" button (or the app's Edit with no clip): create a note
    /// clip of one bar at the playhead on `device`'s track (creating the track if needed) with
    /// a `16 x 1/16` [`atomik_model::StepGrid`], in one undo step, then open it with
    /// [`crate::StepEditorView::open`].
    CreateStepClip {
        /// The drum machine.
        device: DeviceKey,
    },
    /// Preview a key on a device at a given velocity (sample editor: the velocity comes from
    /// the click height in the key map, so velocity layers can be heard). Like
    /// [`UiAction::Audition`]: every `on` is followed by a matching `off`; `velocity` is the
    /// 7-bit value 1..=127 (send `velocity / 127` to the device).
    AuditionVelocity {
        /// Device to play.
        device: DeviceKey,
        /// MIDI key 0..=127.
        key: u8,
        /// 7-bit velocity 1..=127 (ignored for note off).
        velocity: u8,
        /// Note on (`true`) or off (`false`).
        on: bool,
    },
    /// Open the slice editor on this loop slicer (the slicer panel's Edit button, or a
    /// double-click on its overview strip): call [`crate::SliceEditorView::open`] and show it in
    /// place of the arrangement, like the sample editor.
    OpenSliceEditor(DeviceKey),
    /// Close the slice editor and show the arrangement again.
    CloseSliceEditor,
    /// Slice the loop of `device` (the Slice button of the slicer panel or of the slice editor).
    /// The UI has no samples, so the **app** computes the markers and applies one
    /// [`atomik_model::Command::SetZones`] built with [`atomik_model::slices_from_markers`]
    /// (`min_len` = 1 ms of frames) over the loop asset (the asset of the device's slices):
    /// [`SliceMode::Transients`] runs onset detection on the loop's samples (off the UI thread)
    /// at the given sensitivity, [`SliceMode::Grid`] uses [`atomik_model::grid_markers`] with the
    /// device's `loop.beats` and the division. A device without a loop is ignored.
    SliceLoop {
        /// The loop slicer.
        device: DeviceKey,
        /// How to find the markers.
        mode: SliceMode,
    },
    /// The "To track" button: the app builds the note clip that replays the slices
    /// ([`atomik_model::to_track_notes`] with the device's `loop.beats`) and adds or replaces it
    /// as one undo step.
    SlicesToTrack {
        /// The loop slicer.
        device: DeviceKey,
    },
    /// Open the sequencer grid editor on this step sequencer (the panel's Edit button, or a
    /// double-click on its pattern preview): call [`crate::SeqEditorView::open`] and show it in
    /// place of the arrangement, like the sample editor. The editor starts on the pattern the
    /// panel is editing: call [`crate::SeqEditorView::set_pattern_index`] with
    /// [`crate::RackView::edited_pattern`] after opening.
    OpenSeqEditor {
        /// The step sequencer.
        device: DeviceKey,
    },
    /// Close the sequencer grid editor and show the arrangement again.
    CloseSeqEditor,
    /// The edited pattern of a step sequencer changed (a pattern slot of the panel or of the
    /// grid editor was clicked, or a pattern was added or duplicated and became the edited one).
    /// It is view state, not part of the document: the app mirrors it into the other view
    /// ([`crate::SeqEditorView::set_pattern_index`], [`crate::RackView::set_edited_pattern`]).
    /// Nothing else has to happen; the playing pattern is the device's `pattern` param.
    SelectPattern {
        /// The step sequencer.
        device: DeviceKey,
        /// The pattern, 0-based.
        index: u8,
    },
    /// The rack view now shows the inside of a container: a double-click
    /// on a container panel, or its OPEN button. The view has already switched its scope (like
    /// [`UiAction::OpenBrowser`]); the app may update menus or the status bar. `device` is the
    /// container's **path key** (`layer`, `layer/fx`).
    EnterContainer {
        /// The container entered (path key).
        device: DeviceKey,
    },
    /// The rack view went up `levels` scopes (breadcrumb click, Back button, Esc with nothing
    /// selected, Backspace with nothing selected). The view has already switched its scope.
    ExitContainer {
        /// How many levels were left (at least 1).
        levels: usize,
    },
    /// Open the container editor on this container (the panel's EDIT button, or "Edit range" in
    /// a mapped control's menu): call [`crate::ContainerEditorView::open`] and show it in place
    /// of the arrangement, like the sample editor. `macros` is `Some(i)` to open on the MACROS
    /// tab with macro `i` (0-based) selected.
    OpenContainerEditor {
        /// The container (path key).
        device: DeviceKey,
        /// Open on the MACROS tab showing this macro.
        macros: Option<u8>,
    },
    /// Close the container editor and show the arrangement again.
    CloseContainerEditor,
    /// Open the plugin's own editor window (the panel's OPEN GUI button, shown only when the
    /// app told the view the plugin has an editor, see [`crate::RackView::set_plugin_status`]).
    /// `device` is the **path key**.
    OpenPluginGui {
        /// The plugin device (path key).
        device: DeviceKey,
    },
    /// Open the plugin parameter pane (the panel's PARAMS button): call
    /// [`crate::PluginParamsView::open`] and show it in place of the arrangement, like the
    /// sample editor. `device` is the **path key**.
    OpenPluginParams {
        /// The plugin device (path key).
        device: DeviceKey,
    },
    /// Close the plugin parameter pane and show the arrangement again.
    ClosePluginParams,
    /// RELOAD on a plugin panel: capture nothing, create the plugin instance again (also the
    /// way a stand-in finds its plugin after a rescan). `device` is the **path key**.
    ReloadPlugin {
        /// The plugin device (path key).
        device: DeviceKey,
    },
    /// The parameter pane's "automate" shortcut: the app adds an automation lane for the plugin
    /// param `p<param>` on the song track of the selected clip when there is one (the UI does
    /// not know the selection), as one undo step.
    AutomatePluginParam {
        /// The plugin device (path key).
        device: DeviceKey,
        /// The plugin's own param id.
        param: u32,
    },
    /// The Plugins section of the settings dialog asks for something.
    PluginSettings(crate::plugin_data::PluginSettingsAction),
    /// The project banner asks for something.
    PluginBanner(crate::plugin_data::PluginBannerAction),
    /// The parameter pane needs the display text of these params (plugin param id and the
    /// plain value to format): the visible rows that have no fresh text. Answer with
    /// [`crate::PluginParamsView::set_texts`] from `PluginManager::param_text`. Emitted by
    /// `handle` when scrolling, searching or an edit makes new rows or values visible (the
    /// app can also poll [`crate::PluginParamsView::take_text_requests`] every frame).
    PluginParamTexts {
        /// The plugin device (path key).
        device: DeviceKey,
        /// (plugin param id, plain value).
        params: Vec<(u32, f32)>,
    },
    /// A file browser request ([`crate::FileBrowserView`]); the app owns the disk, the
    /// library database and the preview player.
    Browse(BrowseAction),
}

/// What a context menu was opened on (see [`UiAction::ContextMenu`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextTarget {
    /// A device plate in the rack.
    Device,
    /// The empty rack background.
    Rack,
    /// A track header in the arrangement.
    TrackHeader,
    /// A clip in the arrangement.
    Clip,
    /// An empty stretch of a lane in the arrangement.
    Lane,
    /// The grid of the piano roll (notes are selected there).
    Notes,
    /// A point of the tempo lane.
    TempoPoint,
    /// The tempo lane away from its points.
    TempoLane,
    /// The ruler away from a meter change.
    RulerMeter,
    /// A meter change in the ruler.
    MeterChange,
}

/// How [`UiAction::SliceLoop`] finds the slice markers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SliceMode {
    /// Transient (onset) detection; `sensitivity` is 0..=100 percent (higher finds more).
    Transients {
        /// Sensitivity in percent, 0..=100.
        sensitivity: u8,
    },
    /// An even grid; `division` is the number of slices per beat (1 = 1/4, 2 = 1/8, 4 = 1/16,
    /// 8 = 1/32).
    Grid {
        /// Slices per beat.
        division: u32,
    },
}

/// Where dropped audio files go (see [`UiAction::ImportAudio`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportTarget {
    /// Onto this audio track (the file was dropped on one of its lanes or its header).
    ExistingTrack(DeviceKey),
    /// Onto new audio tracks, one per file (dropped elsewhere in the track area).
    NewTracks,
}

/// A transport request. The app owns the transport state and the engine calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportAction {
    /// Start playback from the current position.
    Play,
    /// Stop playback (and recording).
    Stop,
    /// Play if stopped, stop if playing (Space).
    TogglePlay,
    /// Move the position to tick 0 (Enter).
    ReturnToZero,
    /// Move the position to this song tick.
    Locate(i64),
    /// Toggle the song loop (L): apply `SongCommand::SetLoop` with the current range and the
    /// flag inverted.
    ToggleLoop,
    /// Toggle recording (R).
    ToggleRecord,
    /// Toggle the metronome click.
    ToggleMetronome,
}

/// Something the file browser shows in its list: a folder, or a library collection.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Place {
    /// The contents of a folder (with sub-folders, or recursively when searching below it).
    Folder(PathBuf),
    /// Every favourite file.
    Favorites,
    /// Every file with this tag.
    Tag(String),
}

/// A request from [`crate::FileBrowserView`] (see [`UiAction::Browse`]).
#[derive(Debug, Clone, PartialEq)]
pub enum BrowseAction {
    /// Show this place in the list: list it on a worker, then call `set_location` and `set_rows`.
    Show(Place),
    /// Go to the parent of the current folder.
    Parent,
    /// List the current place again.
    Refresh,
    /// The sub-folders of this folder are needed for the tree (it was expanded): list them
    /// on a worker and answer with `set_children`.
    ListChildren(PathBuf),
    /// Search below the current folder (true) or in it only (false). The query itself stays
    /// in the view; when true the app streams matching candidates with `append_rows`.
    SetRecursive(bool),
    /// Auto-preview on selection changed (persist it).
    SetAutoPreview(bool),
    /// Start previewing this audio file (replaces any preview).
    Preview(PathBuf),
    /// Stop the preview.
    StopPreview,
    /// Toggle favourite on these files together (see `LibraryDb::toggle_favorite`).
    ToggleFavorite(Vec<PathBuf>),
    /// Tag these files.
    AddTag {
        /// Files.
        paths: Vec<PathBuf>,
        /// Tag name as typed (the app validates it with `atomik_library::Tag::new`).
        tag: String,
    },
    /// Untag these files.
    RemoveTag {
        /// Files.
        paths: Vec<PathBuf>,
        /// Tag name.
        tag: String,
    },
    /// Rename a tag everywhere.
    RenameTag {
        /// Current name.
        from: String,
        /// New name as typed.
        to: String,
    },
    /// Delete a tag everywhere.
    DeleteTag(String),
    /// Bookmark a folder under Places.
    AddPlace(PathBuf),
    /// Remove a bookmark.
    RemovePlace(PathBuf),
    /// The pointer dragged these files past the drag threshold: the app takes the drag over
    /// and delivers it to the pane under the pointer as a file hover/drop.
    DragFiles(Vec<PathBuf>),
    /// These visible audio rows have no `AudioInfo` yet: probe them (each path is requested
    /// once until the rows are replaced).
    RequestInfo(Vec<PathBuf>),
    /// The panel width or tree/list split changed by a drag (persist them).
    LayoutChanged,
}
