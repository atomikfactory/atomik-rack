//! What the UI asks the app to do.

use atomik_model::{ClipId, Command, DeviceKey, RackSlot};
use std::path::PathBuf;

/// An action emitted by the views' `handle` methods ([`crate::RackView`],
/// [`crate::ArrangementView`], [`crate::PianoRollView`], [`crate::TransportBar`],
/// [`crate::ConsoleView`], [`crate::SampleEditorView`]). The UI never
/// mutates the document itself.
///
/// The enum is `#[non_exhaustive]`: hosts match the variants they handle and ignore the rest
/// with a wildcard arm, so later phases can add actions without breaking them.
// Most actions are commands, which carry whole clips: boxing them would add an allocation to
// nearly every action the views emit.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum UiAction {
    /// Apply this command to the [`atomik_model::Document`], then call the view's `sync`.
    /// If it fails, report the error with [`crate::RackView::notify_rejected`].
    Command(Command),
    /// A drag ended (pointer released): call [`atomik_model::Document::end_merge`].
    EndMerge,
    /// The Undo History panel asked to go to a row: the starting row is 0 and row `n` is the
    /// state right after the `n`-th undo step. The host undoes or redoes as many steps as it
    /// takes (see [`crate::HistoryView`]).
    HistoryGoto(usize),
    /// Arrangement clips were dragged out of the arrangement and dropped at `pos` (window logical
    /// pixels): on a launcher slot, perhaps.
    ClipsDragged {
        /// The clips.
        ids: Vec<ClipId>,
        /// Where they were dropped.
        pos: vello::kurbo::Point,
    },
    /// The Launcher view asked for something that is not a document edit: a launch, a stop, a
    /// switch, opening a clip (see [`crate::LauncherAction`]).
    Launcher(crate::launcher::LauncherAction),
    /// A step of a clip edge dragged with the stretch role: the host puts the document back to
    /// before the drag and stretches `ids` by `percent` (see
    /// [`atomik_model::WorkflowCommand::StretchClips`]) as a preview that adds no undo step.
    /// Any later command accepts the preview first.
    StretchStep {
        /// The clips being stretched.
        ids: Vec<ClipId>,
        /// The new tempo as a percentage of the old one.
        percent: f64,
        /// The start edge was grabbed.
        from_start: bool,
    },
    /// A step of a crossfade zone dragged: the host puts the document back to before the drag
    /// and moves the crossfade of `later` by `delta` ticks (see
    /// [`atomik_model::WorkflowCommand::MoveCrossfade`]) as a preview that adds no undo step.
    CrossfadeStep {
        /// The clip that fades in.
        later: ClipId,
        /// Ticks the zone moved by (not 0).
        delta: i64,
    },
    /// A step of a crossfade length handle dragged: the host puts the document back to before
    /// the drag and makes the crossfade of `later` `delta` ticks longer (shorter when negative,
    /// see [`atomik_model::WorkflowCommand::ResizeCrossfade`]) as a preview that adds no undo
    /// step. [`UiAction::CrossfadeEnd`] ends it.
    CrossfadeLengthStep {
        /// The clip that fades in.
        later: ClipId,
        /// Ticks added to the overlap (not 0).
        delta: i64,
    },
    /// A step of an edge of a clip dragged with the scale-selection role: the host puts the
    /// document back to before the drag and scales `ids` about the earliest start among them
    /// (see [`atomik_model::WorkflowCommand::ScaleTempo`]), or with `from_end` (the start edge
    /// was dragged) about the latest end, as a preview that adds no undo step.
    /// [`UiAction::StretchEnd`] ends it.
    ScaleStep {
        /// The clips being scaled.
        ids: Vec<ClipId>,
        /// The new tempo as a percentage of the old one.
        percent: f64,
        /// Scale about the latest end of the clips instead of the earliest start.
        from_end: bool,
    },
    /// The crossfade drag ended: `commit` keeps the last preview as one undo step, otherwise the
    /// document goes back to what it was before the drag.
    CrossfadeEnd {
        /// Keep the move.
        commit: bool,
    },
    /// The stretch drag ended: `commit` keeps the last preview as one undo step, otherwise the
    /// document goes back to what it was before the drag.
    StretchEnd {
        /// Keep the stretch.
        commit: bool,
    },
    /// Undo (Ctrl+Z / Cmd+Z).
    Undo,
    /// Redo (Ctrl+Shift+Z, Ctrl+Y / Cmd+Shift+Z).
    Redo,
    /// The selected device changed (the selection is that one device).
    Select(DeviceKey),
    /// EDIT on an insert row of the console: the rack shows the strip with its insert area
    /// unfolded and the effect selected.
    EditInsert(DeviceKey),
    /// "Empty Mix Channel" of the console's add-channel picker: a channel with no source, routed
    /// to the master. The app adds it (one undo step), selects it and says so in the status line.
    AddEmptyChannel,
    /// The rack selection holds several devices: `primary` is the device
    /// the editors and the live keyboard follow (the last clicked), `keys` the whole selection
    /// in rack order (path keys inside a container). View state, not an undo step.
    SelectMany {
        /// The primary device (also in `keys`).
        primary: DeviceKey,
        /// The selected devices in rack order.
        keys: Vec<DeviceKey>,
    },
    /// Ctrl-drag of console strips: copies of the channels `strips`, placed so that the first
    /// lands at console position `to` (see `AppCore::copy_strips_to`).
    CopyStrips {
        /// The channel strips dragged (the selection, when the dragged one is in it).
        strips: Vec<DeviceKey>,
        /// Console position the first copy lands at.
        to: usize,
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
    /// Open the controller lanes this track holds outside its clips in the piano roll (the CC
    /// badge of the track header; the same as Track > Edit Track Controllers for the track).
    OpenTrackControllers(DeviceKey),
    /// Close the piano roll and show the arrangement again.
    ClosePianoRoll,
    /// A transport request (buttons, shortcuts, ruler clicks).
    Transport(TransportAction),
    /// The arm button of an automation lane was clicked: arm the lane for automation recording,
    /// or disarm it. Session state of the app, not an undo step.
    ToggleLaneArm {
        /// The lane's track device.
        device: DeviceKey,
        /// The lane's param key.
        param: String,
    },
    /// The arm button of the tempo lane was clicked: the next take records tempo changes (or no
    /// longer does). Session state of the app, not an undo step.
    ToggleTempoArm,
    /// An arrow of the Spectrum EQ window: show the strip `delta` places along (wrapping). View
    /// state, not an undo step.
    SpectrumEqStep(i8),
    /// The Follow Selection box of the Spectrum EQ window was clicked: `true` follows the
    /// selected strip. View state, not an undo step.
    SpectrumEqFollow(bool),
    /// The arrow of a track header (Separated master keyboard mode) or the command: the track
    /// `Some(device)` now receives the master keyboard whatever is selected, `None` hands it
    /// back to the selection. View state, not an undo step.
    SetMasterKeyboard(Option<DeviceKey>),
    /// The Manual Rec switch of the arrangement corner: toggles whether selecting a track arms
    /// it. View state, not an undo step.
    ToggleManualRec,
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
    /// A note of the Piano Keys strip: note on (`on`, with `velocity` 1..=127) or off. The app
    /// plays it on the live note path to the armed track, as notes from a MIDI keyboard are,
    /// so a take records it. Every `on` is followed by a matching `off`.
    KeysNote {
        /// MIDI key 0..=127.
        key: u8,
        /// 7-bit velocity 1..=127 (ignored for note off).
        velocity: u8,
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
    /// MIDI files were dropped on the arrangement at the snapped song tick `tick` (never
    /// negative). With `lane` (the track's instrument and an index into its lanes, always a
    /// note lane) each file becomes one clip on that lane starting at `tick`; with `None`
    /// (dropped anywhere else in the track area) each file makes new tracks starting at `tick`.
    /// One undo step per file.
    ImportMidi {
        /// The dropped `.mid` / `.midi` files, in drop order.
        paths: Vec<PathBuf>,
        /// The note lane under the pointer, if it was on one.
        lane: Option<(DeviceKey, usize)>,
        /// Song tick of the first clip.
        tick: i64,
    },
    /// The TUNER button of an audio track panel: show (`on`) or hide that track's tuner. The app
    /// then reads the track's input at the display rate and feeds the panel with
    /// [`crate::RackView::set_analyser`]; the track is muted while it shows when its
    /// `tuner_mute` is on. View state, not an undo step.
    SetTuner {
        /// The audio track device.
        device: DeviceKey,
        /// The tuner is shown.
        on: bool,
    },
    /// The play button of a note player's panel: start the preview of the player (it and the
    /// players after it run on a clock of their own while the song is stopped, so the notes are
    /// heard through the track's instrument), or stop it when it runs. The app answers with
    /// [`crate::RackView::set_previewing`]. View state, not an undo step.
    PreviewPlayer(DeviceKey),
    /// The FIND button of a dynamic EQ: the app renders a few seconds of what reaches the
    /// device's input on a worker, finds the resonances in it and answers with
    /// [`crate::RackView::set_resonances`]. Changes nothing in the song.
    FindResonances(DeviceKey),
    /// The CUT button of a dynamic EQ: the resonances found last go into the free bells of the
    /// device as dynamic cuts, in one undo step.
    CutResonances(DeviceKey),
    /// The LEARN button of a Sidechain Tool: the app renders a few seconds of what reaches the
    /// device's key on a worker, learns a duck curve from the hits in it and puts the curve into
    /// the device in one undo step.
    LearnDuck(DeviceKey),
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
    /// The SAMPLE button of a sampler, drum machine, loop slicer or granular sampler (panel and
    /// editors): the first press starts recording from the sampling input pair, the next stops
    /// it and lands the sample in `device` as one undo step (a drum machine puts it on the pad of
    /// `at_key`, else on the next empty pad). Recording stops by itself after 30 s.
    SampleInto {
        /// The device the sample lands in.
        device: DeviceKey,
        /// The key (a drum pad's key) the sample goes to, if the button named one.
        at_key: Option<u8>,
    },
    /// A right-click on a SAMPLE button: the next sampling input (stereo pairs, then single
    /// channels). View state of the app, not an undo step.
    CycleSamplingInput,
    /// Apply an offline edit to the sample of a zone (the sample editor's edit buttons): the app
    /// makes a new asset on a worker and points the zone (or, with `all_uses`, every zone of
    /// the project that plays the same asset) at it in one undo step. Ranges are frames of the
    /// zone's asset.
    EditSample {
        /// The sampler holding the zone.
        device: DeviceKey,
        /// The zone's index.
        zone: usize,
        /// Every zone playing the same asset takes the edited copy, not only this one.
        all_uses: bool,
        /// The edit.
        edit: SampleOp,
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
    /// A library item was dropped on the rack, from the operating system or from the file
    /// browser's own drag: insert it at `slot`. `path` is a patch or container item file or
    /// a device URI ([`atomik_library::device_uri`]); the UI never reads it. With
    /// `container: None` the slot is a top-level rack slot (a column of the rack and the
    /// index among its devices; the column after the last one makes a new column); inside
    /// a container the drop names the container and `slot.column` is 0 with the index among
    /// its devices. One undo step; the app refuses what cannot be inserted.
    InsertLibrary {
        /// The item file or the device URI.
        path: PathBuf,
        /// The path key of the container the rack view was inside, `None` at the top level.
        container: Option<DeviceKey>,
        /// Where it goes.
        slot: RackSlot,
    },
    /// A device row or a one-device patch from the library was dropped on a device of the same
    /// kind (on the middle of its panel in the rack, or on a track header in the arrangement):
    /// the device is replaced in place, as the browser's replace does, routed like Replace
    /// Device; a patch of another type also loads its sound. One undo step; the app refuses
    /// what the model refuses and says why. `device` is the top-level key of the device.
    ReplaceWithLibrary {
        /// The device URI or the `.atomikpatch` file.
        path: PathBuf,
        /// The device that is replaced.
        device: DeviceKey,
    },
    /// A patch item was dropped on the patch strip of a device: the device takes the patch if
    /// the file holds one device of the device's type (the app says why when it does not). One
    /// undo step.
    ApplyLibraryPatch {
        /// The `.atomikpatch` file.
        path: PathBuf,
        /// The device (path key).
        device: DeviceKey,
    },
    /// A note clip item was dropped on a note lane of the arrangement: paste it on lane `lane`
    /// (an index into the track's lanes, always a note lane) of `track` at the snapped song
    /// tick `tick` (never negative). One undo step; a drop anywhere else emits nothing.
    InsertLibraryClip {
        /// The `.atomikclip` file.
        path: PathBuf,
        /// The track's instrument device.
        track: DeviceKey,
        /// The note lane (`Track::is_note_lane`).
        lane: usize,
        /// Song tick of the clip's start.
        tick: i64,
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
    /// Open this audio clip in the audio clip editor (double-click on an audio clip, Enter): call
    /// [`crate::AudioEditorView::open`] and show it in place of the arrangement. Opening another
    /// clip while the editor is open keeps its mode.
    OpenAudioEditor(ClipId),
    /// Copy takes of take group `group` (positions in the group's take list) to the takes
    /// clipboard: the take menu's Copy Take and Copy All Takes. Session state of the app, not
    /// an undo step.
    CopyTakes {
        /// The take group.
        group: u32,
        /// The takes to copy.
        takes: Vec<usize>,
    },
    /// Paste the takes clipboard into take group `group` as new takes (one undo step).
    PasteTakes {
        /// The take group.
        group: u32,
    },
    /// Import a file as a new take of take group `group`: the app asks for the file (an audio
    /// file for an audio group, a MIDI file when `notes`), then adds it as one undo step.
    ImportTake {
        /// The take group.
        group: u32,
        /// The group holds note takes (a MIDI file), not audio.
        notes: bool,
    },
    /// Close the audio clip editor and show the arrangement again.
    CloseAudioEditor,
    /// Make a Loop Slicer from the audio clip: a new device that slices the clip's span of its
    /// file at the clip's warp markers, as one undo step (the app owns the device keys).
    ClipToSlicer(ClipId),
    /// Find the transients of the audio clip open in the audio clip editor (Slice mode opened, a
    /// clip opened in it, or the sensitivity changed): the app detects them off the UI thread, in
    /// the span of the file the clip plays, and gives them back with
    /// [`crate::AudioEditorView::set_transients`]. `sensitivity` is in percent (0..=100).
    DetectTransients {
        /// The clip.
        clip: ClipId,
        /// Detector sensitivity in percent.
        sensitivity: u8,
    },
    /// Find the notes and pitch curves of the audio clip open in the audio clip editor (Vocal
    /// mode opened, a clip opened in it, or the sensitivity changed): the app analyses the span of
    /// the file the clip plays on a worker and gives the result back with
    /// [`crate::AudioEditorView::set_vocal_notes`], its progress with
    /// [`crate::AudioEditorView::set_vocal_progress`]. `sensitivity` is in percent (0..=100).
    AnalyseVocal {
        /// The clip.
        clip: ClipId,
        /// Note detector sensitivity in percent.
        sensitivity: u8,
    },
    /// Stop the analysis [`UiAction::AnalyseVocal`] started (Esc or CANCEL in the editor).
    CancelVocalAnalysis,
    /// The speaker tool is pressed: play the frames `start_frame..end_frame` of `asset` as they
    /// are in the file (at the file's own rate, no warp, fades or envelope), from the end down
    /// when `reverse`, at `gain_db`. Live only; a new press replaces the audition and
    /// [`UiAction::StopAudition`] (the release) ends it.
    AuditionSpan {
        /// The asset (its content hash).
        asset: String,
        /// First frame of the span.
        start_frame: u64,
        /// The frame after the last one.
        end_frame: u64,
        /// Play the span backwards.
        reverse: bool,
        /// Gain in dB.
        gain_db: f32,
    },
    /// The speaker tool is released: stop the audition of [`UiAction::AuditionSpan`].
    StopAudition,
    /// A press on the patch strip of a device panel: step to the previous or next patch of its
    /// type, browse the patches, or save the device as a patch (the app asks for the name). The
    /// press also selected the device.
    DevicePatch {
        /// The device (path key).
        device: DeviceKey,
        /// What was pressed.
        op: PatchOp,
    },
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
    /// GENERATE or RANDOMISE on a drum generator panel: the app writes pattern `index` of the
    /// generator from its role templates with the device's `style` and `density` params and a
    /// new seed (written to `random.seed`), as one undo step
    /// (`atomik_devices::drum_generator::generate_pattern`; `Command::SetDrumPattern`).
    /// `randomise` also draws the style, density, variation, accent and the pattern's swing, in the
    /// same undo step.
    GenerateDrums {
        /// The drum generator.
        device: DeviceKey,
        /// The edited pattern, 0-based.
        index: u8,
        /// RANDOMISE instead of GENERATE.
        randomise: bool,
    },
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
    /// A preset of the plugin was chosen in the parameter pane: the app loads it into the live
    /// plugin as one undo step.
    LoadPluginPreset {
        /// The plugin device (path key).
        device: DeviceKey,
        /// The preset id the app gave ([`crate::plugin_data::PresetItem::id`]).
        preset: String,
    },
    /// The `<` or `>` of the preset bar: load the preset `delta` places on (wrapping round).
    StepPluginPreset {
        /// The plugin device (path key).
        device: DeviceKey,
        /// -1 or 1.
        delta: i32,
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
    /// A right click on a control (a knob, a fader or a switch of a rack panel or a console strip)
    /// that has a menu of its own: Learn MIDI and Forget MIDI. `at` is the pointer position in the
    /// coordinates of the emitting view; the shell builds the menu.
    ControlMenu {
        /// The device that owns the control (a console strip is a device too).
        device: DeviceKey,
        /// The control's param key.
        param: String,
        /// Where the menu opens.
        at: vello::kurbo::Point,
    },
    /// A right click on a jack on the back of the rack. `at` is the pointer position in the
    /// coordinates of the emitting view; the shell builds the menu ([`crate::JackMenu`]).
    JackMenu {
        /// The jack (path key inside a container).
        socket: atomik_model::SocketRef,
        /// Where the menu opens.
        at: vello::kurbo::Point,
    },
    /// A press on a control (a knob, a fader or a switch of a rack panel or a console strip, a
    /// row of the parameter pane), whatever it goes on to do: the parameter is the one the user
    /// is touching. The shell uses it to make the arrangement follow the selection.
    ParamTouched {
        /// The device that owns the control (a console strip is a device too).
        device: DeviceKey,
        /// The control's param key.
        param: String,
    },
    /// A click on a control with the edit-automation role held (Alt, or Option): the same as
    /// the control menu's Edit Automation (`automation.edit_control`), for that control.
    EditAutomation {
        /// The device that owns the control.
        device: DeviceKey,
        /// The control's param key.
        param: String,
    },
    /// The Macros panel ([`crate::MacrosView`]) asks for something: move a macro, edit or remove an
    /// assignment, name a macro, make a bank, learn a controller for a macro, or close.
    Macros(crate::macros_panel::MacrosAction),
    /// The modulation matrix panel ([`crate::ModMatrixView`]) asks to close; its edits are
    /// ordinary [`UiAction::Command`]s.
    ModMatrix(crate::mod_matrix_panel::ModMatrixAction),
    /// The command palette ([`crate::CommandPaletteView`]) asks to run a command or to close.
    CommandPalette(crate::command_palette::PaletteAction),
    /// The manual ([`crate::ManualView`]) asks to close or to open something outside it.
    Manual(crate::manual::ManualAction),
    /// The parameter search ([`crate::ParamSearchView`]) asks for something: a new query, an action
    /// on a result, or to close.
    ParamSearch(crate::param_search_panel::ParamSearchAction),
    /// The MIDI Mappings panel ([`crate::MidiMapsView`]) asks for something: edit or delete a
    /// mapping, learn another control for it, export or import a preset, or close.
    MidiMaps(crate::midi_maps_panel::MidiMapsAction),
    /// The relink panel ([`crate::RelinkView`]) asks for something: a file for a missing audio
    /// asset, a folder to search, or to close.
    Relink(crate::relink_panel::RelinkAction),
    /// The audio export panel ([`crate::ExportView`]) asks for something: start the export with its
    /// options, or cancel (close the options, stop the render).
    Export(crate::export_panel::ExportAction),
    /// The film export panel ([`crate::FilmView`]) asks for something: start the export with its
    /// options, or cancel (close the options, stop the job).
    Film(crate::film_panel::FilmAction),
    /// The click-level slider of the transport bar moved: the metronome level, 0..=1.
    SetMetronomeGain(f32),
    /// The Q Rec switch or its grid changed: snap recorded note starts to a grid of this many
    /// ticks, or not at all (`None`).
    SetInputQuantize(Option<i64>),
    /// The quantize panel ([`crate::QuantizeView`]) asks for something: apply its settings, measure
    /// a groove from a clip, delete one, or close.
    Quantize(crate::quantize_panel::QuantizeAction),
    /// The blind A/B panel ([`crate::BlindView`]) asks for something: play X or Y, pick one,
    /// switch the level match, run again, or close.
    Blind(crate::blind_panel::BlindAction),
    /// The "Will the bass read?" panel ([`crate::BassReadView`]) asks for something: choose the
    /// strip, measure, stop, or close.
    BassRead(crate::bass_read_panel::BassReadAction),
    /// The Delivery panel ([`crate::DeliveryView`]) asks for something: a page, a target, a
    /// measurement, the ceiling, or a trim of the Peak Surgeon.
    Delivery(crate::delivery_panel::DeliveryAction),
    /// The Sound Analyst panel ([`crate::SoundAnalystView`]) asks for something: pick a file, stop
    /// the analysis, make the patch, match it, or close.
    SoundAnalyst(crate::sound_analyst_panel::SoundAnalystAction),
    /// The wavetable editor ([`crate::WavetableView`]) asks for something: store the edited
    /// table, cut its sample by cycle, play it on oscillator 1, or close.
    Wavetable(crate::wavetable_editor::WavetableAction),
    /// Open the wavetable editor on this Wavetable Synth (a click on a table display).
    EditWavetable(DeviceKey),
    /// The strip silence panel ([`crate::SilenceView`]) asks for something: strip the silence of
    /// the selected audio clips with its settings, or close.
    StripSilence(crate::silence_panel::SilenceAction),
    /// The reverb send wizard ([`crate::ReverbSendView`]) asks for something: add the send of the
    /// target track with its choices, or close.
    ReverbSend(crate::reverb_send_panel::ReverbSendAction),
    /// The Scale Tempo panel ([`crate::ScaleView`]) asks for something: preview the selected
    /// clips scaled by its percentage, make that their new tempo, or close.
    ScaleTempo(crate::scale_panel::ScaleAction),
    /// The song information panel ([`crate::SongInfoView`]) asks for something: make its texts the
    /// song information, or close.
    SongInfo(crate::song_info_panel::SongInfoAction),
    /// The restore list ([`crate::BackupView`]) asks for something: open a backup copy as a new
    /// song, or close.
    Backup(crate::backup_panel::BackupAction),
    /// The share panel ([`crate::ShareView`]) asks for something: ask for the file and share, work
    /// out the size again after a check box, or close.
    Share(crate::share_panel::ShareAction),
    /// Open the key and chord picker (see [`crate::HarmonyPickerView`]) on the song key, a key
    /// change or a chord: a double click on a lane item, or a click on the transport bar's Key.
    HarmonyPicker(crate::harmony_picker::HarmonyTarget),
    /// The key and chord picker asks for something: set the value, delete the item or close.
    Harmony(crate::harmony_picker::HarmonyPickerAction),
    /// The kick and bass check panel ([`crate::KickBassView`]) asks for something: pick a track,
    /// run or stop the check, apply a fix, or close.
    KickBass(crate::kick_bass_panel::KickBassAction),
    /// The references and decision zone panel ([`crate::ZoneView`]) asks for something: select a
    /// reference, measure the mix again, switch the listening, or close.
    Zone(crate::zone_panel::ZoneAction),
    /// The versions panel ([`crate::VersionsView`]) asks for something: keep a named version,
    /// restore one, protect one, or close.
    Versions(crate::versions_panel::VersionsAction),
    /// The plugin manager ([`crate::PluginManagerView`]) asks for something: mark a favourite,
    /// disable, tag, rescan, try again, show a file in its folder, or close.
    PluginManager(crate::plugin_manager::PluginManagerAction),
    /// The masking map panel ([`crate::MaskingView`]) asks for something: analyse the song, stop
    /// the analysis, locate a clash, de-mask a band, or close.
    Masking(crate::masking_panel::MaskingAction),
    /// The mix check panel ([`crate::MixCheckView`]) asks for something: run the check, stop it,
    /// locate a peak, apply a fix, open a tool, or close.
    MixCheck(crate::mixcheck_panel::MixCheckAction),
    /// The repetition panel ([`crate::RepetitionView`]) asks for something: search again, locate
    /// an idea, make a variation, or close.
    Repetition(crate::repetition_panel::RepetitionAction),
    /// The colour menu's Custom item: the app opens the colour picker
    /// ([`crate::ColorPickerView`]) for this target, from the colour it has now.
    PickColor(crate::color_picker::ColorTarget),
    /// The colour picker asks for something: give its colour to the target, or close.
    ColorPicker(crate::color_picker::ColorPickerAction),
    /// The theme editor ([`crate::ThemeEditorView`]) asks for something: preview, save, pick a
    /// colour for a token, or close.
    ThemeEditor(crate::theme_editor::ThemeEditorAction),
    /// The Audio to Notes panel ([`crate::NotesView`]) asks for something: extract the notes of
    /// the selected audio clip with its settings, or close.
    AudioToNotes(crate::notes_panel::NotesAction),
    /// The warp panel ([`crate::WarpView`]) asks for something: set its values on the selected
    /// audio clips, estimate a tempo, or close.
    Warp(crate::warp_panel::WarpAction),
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

/// A part of the patch strip of a device panel (see [`UiAction::DevicePatch`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchOp {
    /// The previous-patch arrow.
    Previous,
    /// The next-patch arrow.
    Next,
    /// The patch name: browse the patches of the device's type.
    Browse,
    /// The save button.
    Save,
    /// A right-click on the patch name: a pop-up of the patches of the same type in the folder.
    Siblings,
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
    /// A folder row in the arrangement.
    Folder,
    /// A clip in the arrangement.
    Clip,
    /// An empty stretch of a lane in the arrangement.
    Lane,
    /// The grid of the piano roll (notes are selected there).
    Notes,
    /// The root field of the piano roll's scale bar (a menu of the twelve notes).
    PianoScaleRoot,
    /// The scale field of the piano roll's scale bar (a menu of the scales).
    PianoScaleKind,
    /// The chord field of the piano roll's scale bar (a menu of the chord stamps).
    PianoChord,
    /// A point of the tempo lane.
    TempoPoint,
    /// The tempo lane away from its points.
    TempoLane,
    /// The ruler away from a meter change.
    RulerMeter,
    /// A meter change in the ruler.
    MeterChange,
    /// A marker of the marker strip or the marker list.
    Marker,
    /// A range marker of the strip or the list.
    MarkerRange,
    /// The marker strip away from its markers.
    MarkerStrip,
    /// The overflow button of the transport bar (shown when its controls do not all fit).
    TransportMore,
    /// A knob, fader or switch (the shell knows which from [`UiAction::ControlMenu`]).
    Control,
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
    /// Toggle the creation of automation lanes for the parameters moved during a take (the
    /// AUTO LANES switch next to Record).
    ToggleAutoLanes,
    /// Move the position by `bars` bar lines (negative: back): a held Rewind or Forward button
    /// sends this over and over, with larger steps the longer it is held.
    Bars(i64),
    /// Jump to the left (`false`) or the right (`true`) locator, the ends of the loop.
    GoLocator(bool),
    /// Dub: add a note lane to each armed note track and record the next pass on it (a take
    /// that is running continues on the new lane from this tick).
    Dub,
    /// Alt: Dub, and mute the lane the last pass went to.
    Alt,
    /// A tap of the tap-tempo button: the app times the taps and sets the tempo.
    TapTempo,
    /// Let the lanes held by a hand-moved value play again (the OVERRIDE lamp).
    ClearOverride,
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
    /// One section of the library: the built-in devices (rows the host builds with
    /// [`crate::device_rows`]) or the items saved in `<library folder>/<Section>` (listed
    /// recursively; a missing folder is an empty list).
    Library(atomik_library::LibrarySection),
    /// One section of the factory library that ships with the application: read-only items
    /// found below `<factory folder>/<Section>` (listed recursively; the host never saves into
    /// it or deletes from it). The devices section has no factory entry.
    Factory(atomik_library::LibrarySection),
    /// Every file in the named favourite list (see `atomik_library::LibraryDb::lists`).
    List(String),
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
    /// The level of the preview control changed, in whole dB (apply it to the next previews and
    /// persist it).
    SetPreviewLevel(f32),
    /// Start previewing this audio file (replaces any preview).
    Preview(PathBuf),
    /// Stop the preview.
    StopPreview,
    /// With a patch target ([`crate::FileBrowserView::set_patch_target`]): this patch row was
    /// selected (clicked, or stepped to with the arrow keys). Apply it to the target device as
    /// a preview with no undo step.
    Audition(PathBuf),
    /// With a patch target: Escape. Put the target device back as it was and end patch
    /// browsing.
    Cancel,
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
    /// Set the author of these files (text as typed; empty clears it).
    SetAuthor {
        /// Files.
        paths: Vec<PathBuf>,
        /// Author as typed (the app cleans it with `LibraryDb::set_author`).
        author: String,
    },
    /// Replace the categories of these files.
    SetCategories {
        /// Files.
        paths: Vec<PathBuf>,
        /// Category names separated by commas, as typed (the app validates each with
        /// `atomik_library::Tag::new`; empty clears them).
        categories: String,
    },
    /// Put these files into a named favourite list; the list is made when it does not exist.
    AddToList {
        /// Files.
        paths: Vec<PathBuf>,
        /// List name as typed.
        list: String,
    },
    /// Take these files out of a named favourite list.
    RemoveFromList {
        /// Files.
        paths: Vec<PathBuf>,
        /// List name.
        list: String,
    },
    /// Make an empty named favourite list.
    NewList(String),
    /// Rename a favourite list.
    RenameList {
        /// Current name.
        from: String,
        /// New name as typed.
        to: String,
    },
    /// Delete a favourite list (its files stay).
    DeleteList(String),
    /// Show this file in the system file manager (Show in Folder).
    Reveal(PathBuf),
    /// Bookmark a folder under Places.
    AddPlace(PathBuf),
    /// Remove a bookmark.
    RemovePlace(PathBuf),
    /// The pointer dragged these files past the drag threshold: the app takes the drag over
    /// and delivers it to the pane under the pointer as a file hover/drop. Library items and
    /// device URIs ride along (see [`atomik_library::device_uri`]).
    DragFiles(Vec<PathBuf>),
    /// Enter or a double-click on a library item row (a patch, container, clip or project
    /// file, or a device URI): load it where the app's rules say (the rack, the selected
    /// track, or as the open project).
    Load(PathBuf),
    /// "Open as Project" on a patch, container or project item: open the file as a project
    /// (the app asks about unsaved changes first).
    OpenProject(PathBuf),
    /// Rename a library item: `path` is the file, `to` the new name as typed (without the
    /// extension). Only offered for files inside the library folder
    /// ([`crate::FileBrowserView::set_library_folder`]); the app renames the file (and a
    /// sibling asset bundle), refuses a taken name and lists the section again.
    RenameItem {
        /// The item file.
        path: PathBuf,
        /// The new file stem as typed.
        to: String,
    },
    /// Delete a library item: `path` is the file of a patch, container or clip item inside the
    /// library folder, and the user confirmed the question that names it. The app refuses
    /// anything else, removes the file and the audio only it uses, and lists the section
    /// again.
    DeleteItem(PathBuf),
    /// These visible audio rows have no `AudioInfo` yet: probe them (each path is requested
    /// once until the rows are replaced).
    RequestInfo(Vec<PathBuf>),
    /// The panel width or tree/list split changed by a drag (persist them).
    LayoutChanged,
}

/// An offline edit of a sample (see [`UiAction::EditSample`]). Ranges are frames `[from, to)` of
/// the asset the zone plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleOp {
    /// Keep only the range.
    Crop {
        /// First frame kept.
        from: u64,
        /// End of the kept frames.
        to: u64,
    },
    /// Scale the sample so the peak of the range is at 0 dBFS.
    Normalise {
        /// First frame of the range.
        from: u64,
        /// End of the range.
        to: u64,
    },
    /// Play the range backwards (the whole sample when it is the whole asset).
    Reverse {
        /// First frame of the range.
        from: u64,
        /// End of the range.
        to: u64,
    },
    /// Fade the range in from silence.
    FadeIn {
        /// First frame of the range.
        from: u64,
        /// End of the range.
        to: u64,
    },
    /// Fade the range out to silence.
    FadeOut {
        /// First frame of the range.
        from: u64,
        /// End of the range.
        to: u64,
    },
}
