//! App commands and the menu model shared by the in-window menu bar and the native (macOS)
//! menus.
//!
//! Keys are not decided here: every [`AppCommand`] maps to a command id of the registry
//! ([`AppCommand::command_id`]) and the shortcut text of a menu item is derived from the active
//! keymap ([`menus`]); the key router lives in `atomik_ui::keymap` and the shell.

use crate::record::InputQuantize;
use crate::windows::WindowKind;
use atomik_model::{FadeCurve, WarpMode};
use atomik_ui::arrangement::{AudioMenu, HeightPreset, LaneMenu, OrgMenu};
use atomik_ui::keymap::{
    APP_ZOOM_ARGS, INPUT_CHANNEL_ARGS, Keymap, OUTPUT_CHANNEL_ARGS, RATE_ARGS, RECENT_ARGS,
};
use atomik_ui::{CableVisibility, Flip, ThemeChoice, TransportAction};
use std::borrow::Cow;

/// Something the user asked the app to do (menu item or shortcut).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppCommand {
    /// File ▸ New.
    New,
    /// File ▸ New at Sample Rate: a new project at this rate in Hz (44100, 48000 or 96000).
    NewAtRate(u32),
    /// File ▸ Convert Sample Rate: moves the open project to this rate (resamples its audio, one
    /// undo step).
    ConvertRate(u32),
    /// Audio ▸ Input Channels: the host inputs the project asks for (0 to 32).
    InputChannels(u16),
    /// Audio ▸ Output Channels: the host outputs the project asks for (1 to 32).
    OutputChannels(u16),
    /// File ▸ Open….
    Open,
    /// File ▸ Open Recent ▸ a song: the song at this place of the recent list (0 is the newest).
    OpenRecent(usize),
    /// File ▸ Open Recent ▸ Clear Menu: forget the recent songs.
    ClearRecent,
    /// File ▸ Close: end the song and start a new empty one (asks about unsaved changes).
    Close,
    /// File ▸ Save.
    Save,
    /// File ▸ Save As….
    SaveAs,
    /// File ▸ Import Audio…: pick audio files for new tracks or the selected
    /// audio track.
    ImportAudio,
    /// File ▸ Import MIDI…: a MIDI file as one clip on the selected note track, or as new
    /// tracks when none is selected.
    ImportMidi,
    /// File ▸ Import MIDI as Song…: a MIDI file as new tracks from the start, with its tempo map,
    /// meter and markers.
    ImportMidiSong,
    /// File ▸ Relink Missing Audio…: list the project's missing audio files and find them
    /// (a file each, or a folder search). Only enabled while some are missing.
    RelinkAudio,
    /// File ▸ Export Patch… (the rack as patch v2, without the song).
    ExportPatch,
    /// File ▸ Export MIDI Clip…: the selected note clip as a MIDI file.
    ExportMidiClip,
    /// File ▸ Export MIDI Track…: the selected note track as a MIDI file.
    ExportMidiTrack,
    /// File ▸ Export MIDI Song…: every note track as a type 1 MIDI file.
    ExportMidiSong,
    /// File ▸ Export Song as Audio File…: the song from the start to the End marker, plus its tail.
    ExportSongAudio,
    /// File ▸ Export Loop as Audio File…: exactly the range between the locators.
    ExportLoopAudio,
    /// File ▸ Stress Rack: 100 devices, 300 cables.
    StressRack,
    /// File ▸ Stress Song: 8 tracks × 64 bars with automation.
    StressSong,
    /// File ▸ Quit.
    Quit,
    /// Edit ▸ Undo.
    Undo,
    /// Edit ▸ Redo.
    Redo,
    /// Edit ▸ Cut (the focused pane's selection to the clipboard, removed).
    Cut,
    /// Edit ▸ Copy.
    Copy,
    /// Edit ▸ Paste.
    Paste,
    /// Edit ▸ Duplicate.
    Duplicate,
    /// Edit ▸ Delete.
    Delete,
    /// Edit ▸ Select All.
    SelectAll,
    /// Edit ▸ Combine: the selected devices of the rack go into a new container.
    Combine,
    /// Edit ▸ Uncombine: the selected container is dissolved, its devices come back to the rack.
    Uncombine,
    /// Edit ▸ Quantize: quantize the selected notes or clips with the quantize panel's last
    /// settings, without opening the panel.
    QuantizeApply,
    /// Edit ▸ Quantize Options…: the quantize panel (grid, strength, swing, groove, humanize and
    /// the groove library) for the selected notes or clips.
    QuantizeOptions,
    /// View ▸ Front/Back.
    ToggleFlip,
    /// View ▸ Cables ▸ Show / Hover / Hide.
    Cables(CableVisibility),
    /// View ▸ Zoom In.
    ZoomIn,
    /// View ▸ Zoom Out.
    ZoomOut,
    /// View ▸ Actual Size.
    ZoomReset,
    /// View ▸ Device Browser.
    ToggleBrowser,
    /// View ▸ File Browser (the docked audio file panel on the left).
    ToggleFileBrowser,
    /// View ▸ Piano Keys (the on-screen keyboard strip above the status bar).
    TogglePianoKeys,
    /// View ▸ Rack Navigator (the overview strip at the right of the rack).
    ToggleRackNavigator,
    /// View ▸ Arrangement (show or hide the song area below the transport bar).
    ToggleSong,
    /// View ▸ Piano Roll (open it on the selected or first note clip, or close it).
    TogglePianoRoll,
    /// View ▸ Sample Editor (open it on the selected or first sampler, or close it).
    ToggleSampleEditor,
    /// View ▸ Step Editor (open it on the selected or first drum machine, or
    /// close it).
    ToggleStepEditor,
    /// View ▸ Slice Editor (open it on the selected or first loop slicer, or
    /// close it).
    ToggleSliceEditor,
    /// Transport ▸ … (play, stop, record, loop, metronome, return to zero).
    Transport(TransportAction),
    /// Transport ▸ Input Quantize.
    Quantize(InputQuantize),
    /// Options ▸ Pre-count (Ctrl+P): the count-in of a recording on or off. It is a setting
    /// (`pre_count_bars`), so the shell applies and saves it.
    TogglePreCount,
    /// Options ▸ Sync ▸ MIDI Clock Out: send MIDI clock to the ports of the Sync settings (a
    /// setting, so the shell applies and saves it).
    ToggleClockOut,
    /// Options ▸ Sync ▸ Send Clock While Stopped: keep sending the clock while the transport is
    /// stopped.
    ToggleClockWhileStopped,
    /// Options ▸ Sync ▸ Follow MIDI Clock: the transport follows the clock of the Sync settings'
    /// input port.
    ToggleClockIn,
    /// Options ▸ Chase Notes: the selected track's note chase on or off.
    ToggleChase,
    /// View ▸ Show Console Devices in Rack (the channel and master devices in the rack).
    ToggleConsoleDevices,
    /// View > Show Console Links: draw the channel to master cables on the back of
    /// the rack (a channel otherwise shows "Output: Master").
    ToggleConsoleLinks,
    /// View > Tempo Lane: the tempo curve lane between the ruler and the track lanes in the
    /// arrangement (saved with the view state).
    ToggleTempoLane,
    /// View > Marker Strip: the strip of markers and ranges under the arrangement's ruler
    /// (saved with the view state).
    ToggleMarkerStrip,
    /// View > Inspector: the strip under the arrangement's track lanes with the selected clips'
    /// position, length and loop as editable fields (saved with the view state).
    ToggleInspector,
    /// View > Marker List: the list of markers and ranges docked at the right edge of the
    /// arrangement (saved with the view state).
    ToggleMarkerList,
    /// Transport > Add Marker: a marker at the playhead.
    AddMarker,
    /// Transport > Add Range Marker from Loop.
    AddRangeFromLoop,
    /// Transport > Set End Marker at Playhead: the song End moves to the playhead.
    SetEndMarker,
    /// Transport > Set Left Locator at Playhead: the loop starts at the playhead.
    SetLeftLocator,
    /// Transport > Set Right Locator at Playhead: the loop ends at the playhead.
    SetRightLocator,
    /// Transport > Previous Marker: the playhead goes to the marker before it.
    PreviousMarker,
    /// Transport > Next Marker: the playhead goes to the marker after it.
    NextMarker,
    /// Mixer ▸ Console (F5): swap the arrangement and the console in the lower pane.
    ToggleConsole,
    /// Mixer ▸ Maximise Console (Shift+F5): the console over the rack, or back.
    MaximiseConsole,
    /// Mixer ▸ Add Channel…: show the console and open its "add channel" picker.
    AddChannel,
    /// Mixer ▸ Delay Compensation (on/off, undoable).
    ToggleCompensation,
    /// Mixer ▸ Align Returns (on/off, undoable): FX returns, sidechain keys and split paths are
    /// delay-compensated like the summing inputs.
    ToggleReturnAlignment,
    /// Mixer > MIDI Learn (on/off): while on, changing a control arms it for the next hardware
    /// control that moves.
    MidiLearn,
    /// Mixer > MIDI Mappings...: the panel that lists the project's mappings.
    MidiMappings,
    /// Mixer > Export MIDI Mapping Preset...: the mappings as a preset file, by role.
    ExportMidiPreset,
    /// Mixer > Import MIDI Mapping Preset...: a preset file's mappings, added to the project.
    ImportMidiPreset,
    /// Mixer > Clear MIDI Mappings: removes every mapping (one undo step).
    ClearMidiMaps,
    /// Mixer > Macros...: the panel of the macro banks, their macros and assignments.
    MacroPanel,
    /// Mixer > New Macro Bank: a macro bank device at the end of the rack (one undo step).
    NewMacroBank,
    /// View > Find Parameter...: the search over every parameter of the project.
    FindParameter,
    /// View > Automation Follows Selection (on/off, kept in the workflow preferences): the
    /// arrangement shows the lane of the parameter being touched.
    FollowSelection,
    /// View > Show Automation Lane: the lane of the last parameter touched on the selected device.
    ShowParamLane,
    /// Mixer > Route to New Output Bus (Ctrl/Cmd+G): a new bus for the selected channels.
    RouteToNewBus,
    /// Mixer > Create VCA from Selection: a VCA group over the selected channels (the console's
    /// selection, the arrangement's tracks or the rack's devices, whichever has the focus).
    CreateVca,
    /// Mixer > Add to VCA: the selected strips join the VCA group `id` (`mixer.add_to_vca` with
    /// the id as its argument), or the last group when there is none.
    AddToVca(Option<u32>),
    /// Mixer > Remove from VCA: the selected strips leave their groups.
    RemoveFromVca,
    /// Mixer > Delete VCA: the group `id` (`mixer.delete_vca` with the id as its argument), or
    /// the groups the selected strips are in.
    DeleteVca(Option<u32>),
    /// Mixer > Store Snapshot: the mix as a new snapshot (named `A`, `B`, `1`, ...).
    StoreSnapshot,
    /// Mixer > Update Snapshot: overwrites a snapshot with the mix (`mixer.update_snapshot`, the
    /// id as its argument), the current one by default.
    UpdateSnapshot(Option<u32>),
    /// Mixer > Recall Snapshot: applies a snapshot (`mixer.recall_snapshot`, the id as its
    /// argument), the current one by default.
    RecallSnapshot(Option<u32>),
    /// Mixer > Recall Next Snapshot.
    RecallNextSnapshot,
    /// Mixer > Recall Previous Snapshot.
    RecallPreviousSnapshot,
    /// Mixer > Compare A/B: starts comparing the mix with a snapshot (`mixer.compare_snapshot`,
    /// the id as its argument; the current one by default) and flips between the two while it is on.
    CompareSnapshot(Option<u32>),
    /// Mixer > End Compare: keeps the mix that is live.
    EndCompare,
    /// Mixer > Delete Snapshot: a snapshot (`mixer.delete_snapshot`, the id as its argument), the
    /// current one by default.
    DeleteSnapshot(Option<u32>),
    /// Mixer > Toggle Polarity: flips the input polarity of the selected strips (all inverted
    /// switch back to normal, otherwise all are inverted).
    TogglePolarity,
    /// Mixer > Bypass All Inserts: bypasses the insert effects of the selected strips (all
    /// bypassed switch back in, otherwise all are bypassed); each effect keeps its latency.
    BypassAllInserts,
    /// Create ▸ Create Instrument (Ctrl+I): the device browser in create mode, instruments only.
    CreateInstrument,
    /// Create ▸ Create Effect (Ctrl+E): the device browser in create mode, effects only.
    CreateEffect,
    /// Create ▸ Create Player: the device browser in create mode, note players only; the
    /// player is cabled into the stack of the selected device.
    CreatePlayer,
    /// Create ▸ Create Audio Track (Ctrl+T).
    CreateAudioTrack,
    /// Create ▸ Create Mixer Channel (Ctrl+Shift+M): the console's add-channel picker.
    CreateMixerChannel,
    /// View ▸ Rack (F6): show or hide the rack (a layout flag in `workflow.json`).
    ToggleRack,
    /// View ▸ Detach Rack (Cmd+Shift+F6): the rack in a window of its own, or back in the main
    /// window.
    DetachRack,
    /// View ▸ Detach Mixer (Cmd+Shift+F5): the console in a window of its own, or back in the
    /// lower pane.
    DetachConsole,
    /// View ▸ Sequencer Only (F7): hide the rack and the console; again restores the layout.
    SequencerOnly,
    /// View ▸ Clip Editor (F8): open the editor of the selected clip, or close the open one.
    ToggleEditor,
    /// Track ▸ Group Tracks: the selected tracks (or the tracks of the selected clips) into a
    /// new folder.
    GroupTracks,
    /// Track ▸ Ungroup: dissolve the selected folder, or the folder of the selected track.
    UngroupTracks,
    /// Track ▸ Collapse Folder / Expand Folder.
    ToggleFolder,
    /// Track ▸ Rename Folder…: type a new name for the selected folder.
    RenameFolder,
    /// Track ▸ Move Track Up (a selected folder moves with its tracks).
    MoveTrackUp,
    /// Track ▸ Move Track Down.
    MoveTrackDown,
    /// Track ▸ Rename Track…: type the name of the selected track.
    RenameTrack,
    /// Track ▸ Track Comment…: type the selected track's comment.
    TrackComment,
    /// Track > Add Note Lane: one more note lane on the selected track.
    AddNoteLane,
    /// Track > Rename Note Lane...: type the name of the selected note lane.
    RenameNoteLane,
    /// Track > Delete Note Lane: the selected note lane (a track keeps one).
    DeleteNoteLane,
    /// Track > Mute Lane: mute or unmute the selected note lane.
    ToggleLaneMute,
    /// Track ▸ Fold Automation Lanes (`true`) and Unfold Automation Lanes (`false`), of the
    /// selected tracks or of every track.
    FoldLanes(bool),
    /// Track ▸ Track Height: Small, Medium, Large or Fit, for the selected tracks or every track.
    TrackHeight(HeightPreset),
    /// Track ▸ Freeze Track: render the selected tracks through their instrument and insert chain
    /// and play the render instead.
    FreezeTrack,
    /// Track ▸ Freeze Track up to Selected Device: the same, as far as the rack's selected device.
    FreezeTrackToDevice,
    /// Track ▸ Freeze with Tail: keep the decay of reverbs and delays past the end of the clips.
    FreezeTail,
    /// Track ▸ Cancel Freeze: stop the running render and drop the waiting ones.
    FreezeCancel,
    /// Track ▸ Unfreeze Track: the original track, instrument and effects again.
    UnfreezeTrack,
    /// Track ▸ Unfreeze Track and Keep Audio: also leave the render as a clip on a new track.
    UnfreezeTrackKeep,
    /// Track ▸ Bounce in Place: render the selected clips through the track's chain into a new
    /// audio track.
    BounceInPlace,
    /// Track ▸ Bounce in Place and Mute Originals.
    BounceInPlaceMute,
    /// Edit ▸ Reverse Audio: the selected audio clips play backwards (forwards again when they
    /// all do).
    AudioReverse,
    /// Edit ▸ Insert Bars at Locators: opens the loop range's length of empty time at the left
    /// locator, moving everything after it (one undo step).
    InsertBars,
    /// Edit ▸ Remove Bars at Locators: cuts the loop range out of the arrangement and closes the
    /// gap (one undo step).
    RemoveBars,
    /// Edit ▸ Reverse Clips (`true`: notes mirror as bars) and Reverse Note Starts (`false`): the
    /// selected note, automation and audio clips run backwards.
    ReverseClips(bool),
    /// Edit ▸ Scale Tempo: the selected clips play at a percentage of their tempo.
    ScaleTempo(ScaleStep),
    /// Edit ▸ Chop Clips To: the selected clips are cut into pieces of this length.
    ChopClips(ChopStep),
    /// Edit ▸ Normalise Audio: the clip gain that brings the peak of each selected audio clip to
    /// 0 dBFS.
    AudioNormalise,
    /// Edit ▸ Strip Silence…: the panel that cuts the quiet parts out of the selected audio clips.
    AudioStripSilence,
    /// Edit ▸ Quantize Audio: the transients of the selected audio clips move to the grid (warp
    /// markers), with the strength and grid of the quantize options.
    AudioQuantize,
    /// Audio ▸ Warp Audio: the selected audio clips follow the song tempo (off again when they
    /// all do).
    AudioWarp,
    /// Audio ▸ Warp Mode: the stretch engine of the selected audio clips.
    AudioWarpMode(WarpMode),
    /// Audio ▸ Warp to Song Tempo: warp the selected audio clips at the tempo found in their
    /// audio.
    AudioWarpTempo,
    /// Audio ▸ Warp Loop as Bars: the whole asset from the clip's start is this many bars; the
    /// clip becomes that long and warps to the song tempo.
    AudioWarpBars(u32),
    /// Audio ▸ Warp Settings…: the panel of warp, mode, source tempo and transposition.
    AudioWarpPanel,
    /// Audio ▸ Transpose: the pitch of the selected audio clips, without changing their length.
    AudioTranspose(TransposeStep),
    /// Audio ▸ Audio to Notes…: the panel that extracts the notes of the selected audio clip (a
    /// pitched voice or a drum loop) into a note clip.
    AudioToNotes,
    /// Audio ▸ Clip Gain: a step up or down in dB, or back to 0 dB.
    AudioGain(GainStep),
    /// Audio ▸ Fade Shape: the curve of the fades of the selected audio clips.
    AudioFadeShape(FadeCurve),
    /// Audio ▸ Crossfade: the shape of the crossfade where a selected audio clip starts over an
    /// earlier one (`None`: a hard cut).
    AudioCrossfade(Option<FadeCurve>),
    /// Audio ▸ Fades: a 5 ms fade in and out on the selected audio clips (`true`), or no fades.
    AudioFades(bool),
    /// Audio ▸ Clear Gain Envelope.
    AudioClearEnvelope,
    /// Audio ▸ Settings….
    AudioSettings,
    /// Audio ▸ Plugins…: the third-party plugin settings, scan and list.
    PluginSettings,
    /// Audio ▸ Rescan MIDI Ports: open the enabled MIDI inputs that are not open (a controller
    /// plugged in since).
    RescanMidi,
    /// Audio ▸ Restart Audio.
    RestartAudio,
    /// Help ▸ Export Diagnostics…: one report (settings, plugin state, counters, the last crash
    /// and the log) to attach to an issue.
    ExportDiagnostics,
    /// Help ▸ Keyboard Shortcuts (F1): the shortcut list, with search and remapping.
    KeyboardShortcuts,
    /// Help ▸ Documentation: the project page with the documentation, in the browser.
    Documentation,
    /// Help ▸ Report an Issue: the project's issue forms, in the browser.
    ReportIssue,
    /// Help ▸ About Atomik Rack…: the version, the licence and the notices.
    About,
    /// View ▸ Theme: dark, light, or the operating system's.
    SetTheme(ThemeChoice),
    /// View ▸ Application Zoom: the size of every window's content.
    AppZoom(ZoomChange),
    /// Window ▸ a window: bring it forward.
    FocusWindow(WindowKind),
}

/// What View ▸ Application Zoom does to the zoom of every window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoomChange {
    /// Sets the zoom to this percentage (one of [`crate::keymap_store::ZOOM_STEPS`]).
    To(u16),
    /// The next step up.
    In,
    /// The next step down.
    Out,
    /// Back to 100 %.
    Reset,
}

impl ZoomChange {
    /// The registry argument of `view.app_zoom`.
    pub fn arg(self) -> Option<&'static str> {
        match self {
            ZoomChange::To(p) => arg_of(APP_ZOOM_ARGS, p),
            ZoomChange::In => Some("in"),
            ZoomChange::Out => Some("out"),
            ZoomChange::Reset => Some("reset"),
        }
    }

    /// The change of a registry argument.
    pub fn from_arg(arg: &str) -> Option<Self> {
        match arg {
            "in" => Some(ZoomChange::In),
            "out" => Some(ZoomChange::Out),
            "reset" => Some(ZoomChange::Reset),
            _ => num_arg(APP_ZOOM_ARGS, arg).map(ZoomChange::To),
        }
    }

    /// The zoom this change leads to from `percent`.
    pub fn apply(self, percent: u16) -> u16 {
        let steps = &crate::keymap_store::ZOOM_STEPS;
        let at = steps.iter().position(|&s| s == percent);
        match (self, at) {
            (ZoomChange::To(p), _) => crate::keymap_store::nearest_zoom(u64::from(p)),
            (ZoomChange::Reset, _) => 100,
            (ZoomChange::In, Some(i)) => steps[(i + 1).min(steps.len() - 1)],
            (ZoomChange::Out, Some(i)) => steps[i.saturating_sub(1)],
            (ZoomChange::In | ZoomChange::Out, None) => {
                crate::keymap_store::nearest_zoom(u64::from(percent))
            }
        }
    }
}

/// The registry argument of a window.
fn window_arg(kind: WindowKind) -> &'static str {
    match kind {
        WindowKind::Main => "main",
        WindowKind::Rack => "rack",
        WindowKind::Console => "mixer",
    }
}

/// The entry of `args` that spells `n`.
fn arg_of<N: ToString>(args: &'static [&'static str], n: N) -> Option<&'static str> {
    let n = n.to_string();
    args.iter().copied().find(|a| *a == n)
}

/// The number an entry of `args` spells (`None` for a value `args` does not list).
fn num_arg<N: std::str::FromStr>(args: &[&str], a: &str) -> Option<N> {
    args.contains(&a).then(|| a.parse().ok())?
}

impl AppCommand {
    /// The registry command id and argument of this command, `None` only for
    /// `Transport(Locate(_))`, a pointer action that is not a command, and for the VCA and snapshot
    /// commands that name a group or a snapshot (`AddToVca(Some(_))`, `DeleteVca(Some(_))`,
    /// `RecallSnapshot(Some(_))` ...: their id is a runtime number, given as the argument of
    /// `mixer.add_to_vca`, `mixer.delete_vca`, `mixer.recall_snapshot` ...).
    pub fn command_id(&self) -> Option<(&'static str, Option<&'static str>)> {
        Some(match self {
            AppCommand::New => ("file.new", None),
            AppCommand::NewAtRate(r) => ("file.new_rate", Some(arg_of(RATE_ARGS, *r)?)),
            AppCommand::ConvertRate(r) => ("file.convert_rate", Some(arg_of(RATE_ARGS, *r)?)),
            AppCommand::InputChannels(n) => (
                "audio.input_channels",
                Some(arg_of(INPUT_CHANNEL_ARGS, *n)?),
            ),
            AppCommand::OutputChannels(n) => (
                "audio.output_channels",
                Some(arg_of(OUTPUT_CHANNEL_ARGS, *n)?),
            ),
            AppCommand::Open => ("file.open", None),
            AppCommand::OpenRecent(i) => ("file.open_recent", Some(RECENT_ARGS.get(*i)?)),
            AppCommand::ClearRecent => ("file.clear_recent", None),
            AppCommand::Close => ("file.close", None),
            AppCommand::Save => ("file.save", None),
            AppCommand::SaveAs => ("file.save_as", None),
            AppCommand::ImportAudio => ("file.import_audio", None),
            AppCommand::ImportMidi => ("file.import_midi", None),
            AppCommand::ImportMidiSong => ("file.import_midi_song", None),
            AppCommand::RelinkAudio => ("file.relink_audio", None),
            AppCommand::ExportPatch => ("file.export_patch", None),
            AppCommand::ExportMidiClip => ("file.export_midi_clip", None),
            AppCommand::ExportMidiTrack => ("file.export_midi_track", None),
            AppCommand::ExportMidiSong => ("file.export_midi_song", None),
            AppCommand::ExportSongAudio => ("file.export_song_audio", None),
            AppCommand::ExportLoopAudio => ("file.export_loop_audio", None),
            AppCommand::StressRack => ("file.stress_rack", None),
            AppCommand::StressSong => ("file.stress_song", None),
            AppCommand::Quit => ("file.quit", None),
            AppCommand::Undo => ("edit.undo", None),
            AppCommand::Redo => ("edit.redo", None),
            AppCommand::Cut => ("edit.cut", None),
            AppCommand::Copy => ("edit.copy", None),
            AppCommand::Paste => ("edit.paste", None),
            AppCommand::Duplicate => ("edit.duplicate", None),
            AppCommand::Delete => ("edit.delete", None),
            AppCommand::SelectAll => ("edit.select_all", None),
            AppCommand::Combine => ("rack.combine", None),
            AppCommand::Uncombine => ("rack.uncombine", None),
            AppCommand::QuantizeApply => ("editor.quantize", None),
            AppCommand::QuantizeOptions => ("edit.quantize_options", None),
            AppCommand::ToggleFlip => ("view.toggle_flip", None),
            AppCommand::Cables(CableVisibility::Show) => ("view.cables", Some("show")),
            AppCommand::Cables(CableVisibility::HoverOnly) => ("view.cables", Some("hover")),
            AppCommand::Cables(CableVisibility::SelectedOnly) => ("view.cables", Some("selected")),
            AppCommand::Cables(CableVisibility::HideAutoRouted) => ("view.cables", Some("auto")),
            AppCommand::Cables(CableVisibility::Hide) => ("view.cables", Some("hide")),
            AppCommand::ZoomIn => ("view.zoom_in", None),
            AppCommand::ZoomOut => ("view.zoom_out", None),
            AppCommand::ZoomReset => ("view.zoom_reset", None),
            AppCommand::ToggleBrowser => ("view.toggle_browser", None),
            AppCommand::ToggleFileBrowser => ("view.toggle_file_browser", None),
            AppCommand::TogglePianoKeys => ("view.toggle_piano_keys", None),
            AppCommand::ToggleSong => ("view.toggle_song", None),
            AppCommand::TogglePianoRoll => ("view.toggle_piano_roll", None),
            AppCommand::ToggleSampleEditor => ("view.toggle_sample_editor", None),
            AppCommand::ToggleStepEditor => ("view.toggle_step_editor", None),
            AppCommand::ToggleSliceEditor => ("view.toggle_slice_editor", None),
            AppCommand::Transport(TransportAction::Play) => ("transport.play", None),
            AppCommand::Transport(TransportAction::Stop) => ("transport.stop", None),
            AppCommand::Transport(TransportAction::TogglePlay) => ("transport.play_stop", None),
            AppCommand::Transport(TransportAction::ReturnToZero) => ("transport.go_start", None),
            AppCommand::Transport(TransportAction::ToggleLoop) => ("transport.loop", None),
            AppCommand::Transport(TransportAction::ToggleRecord) => ("transport.record", None),
            AppCommand::Transport(TransportAction::ToggleMetronome) => {
                ("transport.metronome", None)
            }
            AppCommand::Transport(TransportAction::ToggleAutoLanes) => {
                ("transport.auto_create_lanes", None)
            }
            AppCommand::Transport(TransportAction::ClearOverride) => {
                ("transport.clear_override", None)
            }
            AppCommand::Transport(
                TransportAction::Locate(_)
                | TransportAction::Bars(_)
                | TransportAction::GoLocator(_),
            ) => return None,
            AppCommand::Transport(TransportAction::TapTempo) => ("transport.tap_tempo", None),
            AppCommand::TogglePreCount => ("transport.pre_count", None),
            AppCommand::ToggleClockOut => ("sync.clock_out", None),
            AppCommand::ToggleClockWhileStopped => ("sync.clock_while_stopped", None),
            AppCommand::ToggleClockIn => ("sync.follow_clock", None),
            AppCommand::ToggleChase => ("sequencer.toggle_chase", None),
            AppCommand::Quantize(InputQuantize::OFF) => ("transport.input_quantize", Some("off")),
            AppCommand::Quantize(InputQuantize::EIGHTH) => {
                ("transport.input_quantize", Some("eighth"))
            }
            AppCommand::Quantize(InputQuantize::SIXTEENTH) => {
                ("transport.input_quantize", Some("sixteenth"))
            }
            // A grid the bar picked has no menu entry: it names the command without a value.
            AppCommand::Quantize(_) => ("transport.input_quantize", None),
            AppCommand::ToggleConsoleDevices => ("view.toggle_console_devices", None),
            AppCommand::ToggleConsoleLinks => ("view.toggle_console_links", None),
            AppCommand::ToggleTempoLane => ("view.toggle_tempo_lane", None),
            AppCommand::ToggleMarkerStrip => ("view.toggle_marker_strip", None),
            AppCommand::ToggleInspector => ("view.toggle_inspector", None),
            AppCommand::ToggleMarkerList => ("view.toggle_marker_list", None),
            AppCommand::AddMarker => ("transport.add_marker", None),
            AppCommand::AddRangeFromLoop => ("transport.add_range_from_loop", None),
            AppCommand::SetEndMarker => ("transport.set_end_marker", None),
            AppCommand::SetLeftLocator => ("transport.set_left_locator", None),
            AppCommand::SetRightLocator => ("transport.set_right_locator", None),
            AppCommand::PreviousMarker => ("transport.prev_marker", None),
            AppCommand::NextMarker => ("transport.next_marker", None),
            AppCommand::ToggleConsole => ("view.toggle_console", None),
            AppCommand::MaximiseConsole => ("view.maximise_console", None),
            AppCommand::AddChannel => ("mixer.add_channel", None),
            AppCommand::RouteToNewBus => ("mixer.route_to_new_bus", None),
            AppCommand::CreateVca => ("mixer.create_vca", None),
            AppCommand::AddToVca(None) => ("mixer.add_to_vca", None),
            AppCommand::DeleteVca(None) => ("mixer.delete_vca", None),
            AppCommand::AddToVca(Some(_)) | AppCommand::DeleteVca(Some(_)) => return None,
            AppCommand::RemoveFromVca => ("mixer.remove_from_vca", None),
            AppCommand::StoreSnapshot => ("mixer.store_snapshot", None),
            AppCommand::UpdateSnapshot(None) => ("mixer.update_snapshot", None),
            AppCommand::RecallSnapshot(None) => ("mixer.recall_snapshot", None),
            AppCommand::RecallNextSnapshot => ("mixer.recall_next_snapshot", None),
            AppCommand::RecallPreviousSnapshot => ("mixer.recall_previous_snapshot", None),
            AppCommand::CompareSnapshot(None) => ("mixer.compare_snapshot", None),
            AppCommand::EndCompare => ("mixer.end_compare", None),
            AppCommand::DeleteSnapshot(None) => ("mixer.delete_snapshot", None),
            AppCommand::UpdateSnapshot(Some(_))
            | AppCommand::RecallSnapshot(Some(_))
            | AppCommand::CompareSnapshot(Some(_))
            | AppCommand::DeleteSnapshot(Some(_)) => return None,
            AppCommand::TogglePolarity => ("mixer.toggle_polarity", None),
            AppCommand::BypassAllInserts => ("mixer.bypass_all_inserts", None),
            AppCommand::ToggleCompensation => ("mixer.toggle_compensation", None),
            AppCommand::ToggleReturnAlignment => ("mixer.toggle_return_alignment", None),
            AppCommand::MidiLearn => ("midi.learn", None),
            AppCommand::MidiMappings => ("midi.mappings", None),
            AppCommand::ExportMidiPreset => ("midi.export_preset", None),
            AppCommand::ImportMidiPreset => ("midi.import_preset", None),
            AppCommand::ClearMidiMaps => ("midi.clear_maps", None),
            AppCommand::MacroPanel => ("macro.panel", None),
            AppCommand::NewMacroBank => ("macro.new_bank", None),
            AppCommand::FindParameter => ("view.find_parameter", None),
            AppCommand::FollowSelection => ("view.follow_selection", None),
            AppCommand::ShowParamLane => ("view.show_param_lane", None),
            AppCommand::CreateInstrument => ("create.instrument", None),
            AppCommand::CreateEffect => ("create.effect", None),
            AppCommand::CreatePlayer => ("create.player", None),
            AppCommand::CreateAudioTrack => ("create.audio_track", None),
            AppCommand::CreateMixerChannel => ("create.mixer_channel", None),
            AppCommand::ToggleRack => ("view.toggle_rack", None),
            AppCommand::ToggleRackNavigator => ("view.toggle_rack_navigator", None),
            AppCommand::SequencerOnly => ("view.sequencer_only", None),
            AppCommand::DetachRack => ("view.detach_rack", None),
            AppCommand::DetachConsole => ("view.detach_console", None),
            AppCommand::ToggleEditor => ("view.toggle_editor", None),
            AppCommand::GroupTracks => ("sequencer.group_tracks", None),
            AppCommand::UngroupTracks => ("sequencer.ungroup_tracks", None),
            AppCommand::ToggleFolder => ("sequencer.toggle_folder", None),
            AppCommand::RenameFolder => ("sequencer.rename_folder", None),
            AppCommand::MoveTrackUp => ("sequencer.move_track_up", None),
            AppCommand::MoveTrackDown => ("sequencer.move_track_down", None),
            AppCommand::RenameTrack => ("sequencer.rename_track", None),
            AppCommand::TrackComment => ("sequencer.track_comment", None),
            AppCommand::AddNoteLane => ("sequencer.add_note_lane", None),
            AppCommand::RenameNoteLane => ("sequencer.rename_note_lane", None),
            AppCommand::DeleteNoteLane => ("sequencer.delete_note_lane", None),
            AppCommand::ToggleLaneMute => ("sequencer.toggle_lane_mute", None),
            AppCommand::FreezeTrack => ("sequencer.freeze_track", None),
            AppCommand::FreezeTrackToDevice => ("sequencer.freeze_track_to_device", None),
            AppCommand::FreezeTail => ("sequencer.freeze_tail", None),
            AppCommand::FreezeCancel => ("sequencer.freeze_cancel", None),
            AppCommand::UnfreezeTrack => ("sequencer.unfreeze_track", None),
            AppCommand::UnfreezeTrackKeep => ("sequencer.unfreeze_track_keep", None),
            AppCommand::BounceInPlace => ("sequencer.bounce_in_place", None),
            AppCommand::BounceInPlaceMute => ("sequencer.bounce_in_place_mute", None),
            AppCommand::FoldLanes(true) => ("sequencer.fold_lanes", Some("fold")),
            AppCommand::FoldLanes(false) => ("sequencer.fold_lanes", Some("unfold")),
            AppCommand::TrackHeight(p) => ("sequencer.track_height", Some(p.arg())),
            AppCommand::AudioReverse => ("sequencer.audio_reverse", None),
            AppCommand::InsertBars => ("sequencer.insert_bars", None),
            AppCommand::RemoveBars => ("sequencer.remove_bars", None),
            AppCommand::ReverseClips(true) => ("sequencer.reverse_clips", Some("bars")),
            AppCommand::ReverseClips(false) => ("sequencer.reverse_clips", Some("starts")),
            AppCommand::ScaleTempo(step) => ("sequencer.scale_tempo", Some(step.arg())),
            AppCommand::ChopClips(step) => ("sequencer.chop_clips", Some(step.arg())),
            AppCommand::AudioNormalise => ("sequencer.audio_normalise", None),
            AppCommand::AudioStripSilence => ("sequencer.audio_strip_silence", None),
            AppCommand::AudioQuantize => ("sequencer.audio_quantize", None),
            AppCommand::AudioWarp => ("sequencer.audio_warp", None),
            AppCommand::AudioWarpMode(mode) => ("sequencer.audio_warp_mode", Some(mode.id())),
            AppCommand::AudioWarpTempo => ("sequencer.audio_warp_tempo", None),
            AppCommand::AudioWarpBars(bars) => (
                "sequencer.audio_warp_bars",
                Some(match bars {
                    1 => "1",
                    2 => "2",
                    4 => "4",
                    8 => "8",
                    _ => "16",
                }),
            ),
            AppCommand::AudioWarpPanel => ("sequencer.audio_warp_panel", None),
            AppCommand::AudioToNotes => ("sequencer.audio_to_notes", None),
            AppCommand::AudioTranspose(step) => ("sequencer.audio_transpose", Some(step.arg())),
            AppCommand::AudioGain(step) => ("sequencer.audio_gain", Some(step.arg())),
            AppCommand::AudioFadeShape(curve) => ("sequencer.audio_fade_shape", Some(curve.id())),
            AppCommand::AudioCrossfade(curve) => (
                "sequencer.audio_crossfade",
                Some(curve.map_or("off", FadeCurve::id)),
            ),
            AppCommand::AudioFades(true) => ("sequencer.audio_fades", Some("declick")),
            AppCommand::AudioFades(false) => ("sequencer.audio_fades", Some("clear")),
            AppCommand::AudioClearEnvelope => ("sequencer.audio_envelope_clear", None),
            AppCommand::AudioSettings => ("audio.settings", None),
            AppCommand::PluginSettings => ("audio.plugins", None),
            AppCommand::RescanMidi => ("audio.rescan_midi", None),
            AppCommand::RestartAudio => ("audio.restart", None),
            AppCommand::ExportDiagnostics => ("help.export_diagnostics", None),
            AppCommand::KeyboardShortcuts => ("help.keyboard_shortcuts", None),
            AppCommand::Documentation => ("help.documentation", None),
            AppCommand::ReportIssue => ("help.report_issue", None),
            AppCommand::About => ("help.about", None),
            AppCommand::SetTheme(t) => ("view.theme", Some(t.id())),
            AppCommand::AppZoom(z) => ("view.app_zoom", Some(z.arg()?)),
            AppCommand::FocusWindow(k) => ("view.focus_window", Some(window_arg(*k))),
        })
    }

    /// The command a registry id (and argument) stands for, if the app has one: the inverse
    /// of [`Self::command_id`]. Ids of commands the shell does not run yet give `None`.
    pub fn from_command(id: &str, arg: Option<&str>) -> Option<AppCommand> {
        Some(match (id, arg) {
            ("file.new", None) => AppCommand::New,
            ("file.new_rate", Some(a)) => AppCommand::NewAtRate(num_arg(RATE_ARGS, a)?),
            ("file.convert_rate", Some(a)) => AppCommand::ConvertRate(num_arg(RATE_ARGS, a)?),
            ("audio.input_channels", Some(a)) => {
                AppCommand::InputChannels(num_arg(INPUT_CHANNEL_ARGS, a)?)
            }
            ("audio.output_channels", Some(a)) => {
                AppCommand::OutputChannels(num_arg(OUTPUT_CHANNEL_ARGS, a)?)
            }
            ("file.open", None) => AppCommand::Open,
            ("file.open_recent", Some(n)) => {
                AppCommand::OpenRecent(RECENT_ARGS.iter().position(|a| *a == n)?)
            }
            ("file.clear_recent", None) => AppCommand::ClearRecent,
            ("file.close", None) => AppCommand::Close,
            ("file.save", None) => AppCommand::Save,
            ("file.save_as", None) => AppCommand::SaveAs,
            ("file.import_audio", None) => AppCommand::ImportAudio,
            ("file.import_midi", None) => AppCommand::ImportMidi,
            ("file.import_midi_song", None) => AppCommand::ImportMidiSong,
            ("file.relink_audio", None) => AppCommand::RelinkAudio,
            ("file.export_patch", None) => AppCommand::ExportPatch,
            ("file.export_midi_clip", None) => AppCommand::ExportMidiClip,
            ("file.export_midi_track", None) => AppCommand::ExportMidiTrack,
            ("file.export_midi_song", None) => AppCommand::ExportMidiSong,
            ("file.export_song_audio", None) => AppCommand::ExportSongAudio,
            ("file.export_loop_audio", None) => AppCommand::ExportLoopAudio,
            ("file.stress_rack", None) => AppCommand::StressRack,
            ("file.stress_song", None) => AppCommand::StressSong,
            ("file.quit", None) => AppCommand::Quit,
            ("edit.undo", None) => AppCommand::Undo,
            ("edit.redo", None) => AppCommand::Redo,
            ("edit.cut", None) => AppCommand::Cut,
            ("edit.copy", None) => AppCommand::Copy,
            ("edit.paste", None) => AppCommand::Paste,
            ("edit.duplicate", None) => AppCommand::Duplicate,
            ("edit.delete", None) => AppCommand::Delete,
            ("edit.select_all", None) => AppCommand::SelectAll,
            ("rack.combine", None) => AppCommand::Combine,
            ("rack.uncombine", None) => AppCommand::Uncombine,
            ("editor.quantize", None) => AppCommand::QuantizeApply,
            ("edit.quantize_options", None) => AppCommand::QuantizeOptions,
            ("view.toggle_flip", None) => AppCommand::ToggleFlip,
            ("view.cables", Some("show")) => AppCommand::Cables(CableVisibility::Show),
            ("view.cables", Some("hover")) => AppCommand::Cables(CableVisibility::HoverOnly),
            ("view.cables", Some("selected")) => AppCommand::Cables(CableVisibility::SelectedOnly),
            ("view.cables", Some("auto")) => AppCommand::Cables(CableVisibility::HideAutoRouted),
            ("view.cables", Some("hide")) => AppCommand::Cables(CableVisibility::Hide),
            ("view.zoom_in", None) => AppCommand::ZoomIn,
            ("view.zoom_out", None) => AppCommand::ZoomOut,
            ("view.zoom_reset", None) => AppCommand::ZoomReset,
            ("view.toggle_browser", None) => AppCommand::ToggleBrowser,
            ("view.toggle_file_browser", None) => AppCommand::ToggleFileBrowser,
            ("view.toggle_piano_keys", None) => AppCommand::TogglePianoKeys,
            ("view.toggle_song", None) => AppCommand::ToggleSong,
            ("view.toggle_piano_roll", None) => AppCommand::TogglePianoRoll,
            ("view.toggle_sample_editor", None) => AppCommand::ToggleSampleEditor,
            ("view.toggle_step_editor", None) => AppCommand::ToggleStepEditor,
            ("view.toggle_slice_editor", None) => AppCommand::ToggleSliceEditor,
            ("transport.tap_tempo", None) => AppCommand::Transport(TransportAction::TapTempo),
            ("transport.play", None) => AppCommand::Transport(TransportAction::Play),
            ("transport.stop", None) => AppCommand::Transport(TransportAction::Stop),
            ("transport.play_stop", None) => AppCommand::Transport(TransportAction::TogglePlay),
            ("transport.go_start", None) => AppCommand::Transport(TransportAction::ReturnToZero),
            ("transport.loop", None) => AppCommand::Transport(TransportAction::ToggleLoop),
            ("transport.record", None) => AppCommand::Transport(TransportAction::ToggleRecord),
            ("transport.metronome", None) => {
                AppCommand::Transport(TransportAction::ToggleMetronome)
            }
            ("transport.auto_create_lanes", None) => {
                AppCommand::Transport(TransportAction::ToggleAutoLanes)
            }
            ("transport.clear_override", None) => {
                AppCommand::Transport(TransportAction::ClearOverride)
            }
            ("transport.pre_count", None) => AppCommand::TogglePreCount,
            ("sync.clock_out", None) => AppCommand::ToggleClockOut,
            ("sync.clock_while_stopped", None) => AppCommand::ToggleClockWhileStopped,
            ("sync.follow_clock", None) => AppCommand::ToggleClockIn,
            ("sequencer.toggle_chase", None) => AppCommand::ToggleChase,
            ("transport.input_quantize", Some("off")) => AppCommand::Quantize(InputQuantize::OFF),
            ("transport.input_quantize", Some("eighth")) => {
                AppCommand::Quantize(InputQuantize::EIGHTH)
            }
            ("transport.input_quantize", Some("sixteenth")) => {
                AppCommand::Quantize(InputQuantize::SIXTEENTH)
            }
            ("view.toggle_console_devices", None) => AppCommand::ToggleConsoleDevices,
            ("view.toggle_console_links", None) => AppCommand::ToggleConsoleLinks,
            ("view.toggle_tempo_lane", None) => AppCommand::ToggleTempoLane,
            ("view.toggle_marker_strip", None) => AppCommand::ToggleMarkerStrip,
            ("view.toggle_inspector", None) => AppCommand::ToggleInspector,
            ("view.toggle_marker_list", None) => AppCommand::ToggleMarkerList,
            ("transport.add_marker", None) => AppCommand::AddMarker,
            ("transport.add_range_from_loop", None) => AppCommand::AddRangeFromLoop,
            ("transport.set_end_marker", None) => AppCommand::SetEndMarker,
            ("transport.set_left_locator", None) => AppCommand::SetLeftLocator,
            ("transport.set_right_locator", None) => AppCommand::SetRightLocator,
            ("transport.prev_marker", None) => AppCommand::PreviousMarker,
            ("transport.next_marker", None) => AppCommand::NextMarker,
            ("view.toggle_console", None) => AppCommand::ToggleConsole,
            ("view.maximise_console", None) => AppCommand::MaximiseConsole,
            ("mixer.add_channel", None) => AppCommand::AddChannel,
            ("mixer.route_to_new_bus", None) => AppCommand::RouteToNewBus,
            ("mixer.create_vca", None) => AppCommand::CreateVca,
            ("mixer.add_to_vca", None) => AppCommand::AddToVca(None),
            ("mixer.add_to_vca", Some(id)) => {
                return id.parse().ok().map(|id| AppCommand::AddToVca(Some(id)));
            }
            ("mixer.remove_from_vca", None) => AppCommand::RemoveFromVca,
            ("mixer.store_snapshot", None) => AppCommand::StoreSnapshot,
            ("mixer.update_snapshot", None) => AppCommand::UpdateSnapshot(None),
            ("mixer.recall_snapshot", None) => AppCommand::RecallSnapshot(None),
            ("mixer.recall_next_snapshot", None) => AppCommand::RecallNextSnapshot,
            ("mixer.recall_previous_snapshot", None) => AppCommand::RecallPreviousSnapshot,
            ("mixer.compare_snapshot", None) => AppCommand::CompareSnapshot(None),
            ("mixer.end_compare", None) => AppCommand::EndCompare,
            ("mixer.delete_snapshot", None) => AppCommand::DeleteSnapshot(None),
            ("mixer.update_snapshot", Some(id)) => {
                return id
                    .parse()
                    .ok()
                    .map(|id| AppCommand::UpdateSnapshot(Some(id)));
            }
            ("mixer.recall_snapshot", Some(id)) => {
                return id
                    .parse()
                    .ok()
                    .map(|id| AppCommand::RecallSnapshot(Some(id)));
            }
            ("mixer.compare_snapshot", Some(id)) => {
                return id
                    .parse()
                    .ok()
                    .map(|id| AppCommand::CompareSnapshot(Some(id)));
            }
            ("mixer.delete_snapshot", Some(id)) => {
                return id
                    .parse()
                    .ok()
                    .map(|id| AppCommand::DeleteSnapshot(Some(id)));
            }
            ("mixer.toggle_polarity", None) => AppCommand::TogglePolarity,
            ("mixer.bypass_all_inserts", None) => AppCommand::BypassAllInserts,
            ("mixer.delete_vca", None) => AppCommand::DeleteVca(None),
            ("mixer.delete_vca", Some(id)) => {
                return id.parse().ok().map(|id| AppCommand::DeleteVca(Some(id)));
            }
            ("mixer.toggle_compensation", None) => AppCommand::ToggleCompensation,
            ("mixer.toggle_return_alignment", None) => AppCommand::ToggleReturnAlignment,
            ("midi.learn", None) => AppCommand::MidiLearn,
            ("midi.mappings", None) => AppCommand::MidiMappings,
            ("midi.export_preset", None) => AppCommand::ExportMidiPreset,
            ("midi.import_preset", None) => AppCommand::ImportMidiPreset,
            ("midi.clear_maps", None) => AppCommand::ClearMidiMaps,
            ("macro.panel", None) => AppCommand::MacroPanel,
            ("macro.new_bank", None) => AppCommand::NewMacroBank,
            ("view.find_parameter", None) => AppCommand::FindParameter,
            ("view.follow_selection", None) => AppCommand::FollowSelection,
            ("view.show_param_lane", None) => AppCommand::ShowParamLane,
            ("create.instrument", None) => AppCommand::CreateInstrument,
            ("create.effect", None) => AppCommand::CreateEffect,
            ("create.player", None) => AppCommand::CreatePlayer,
            ("create.audio_track", None) => AppCommand::CreateAudioTrack,
            ("create.mixer_channel", None) => AppCommand::CreateMixerChannel,
            ("view.toggle_rack", None) => AppCommand::ToggleRack,
            ("view.toggle_rack_navigator", None) => AppCommand::ToggleRackNavigator,
            ("view.sequencer_only", None) => AppCommand::SequencerOnly,
            ("view.detach_rack", None) => AppCommand::DetachRack,
            ("view.detach_console", None) => AppCommand::DetachConsole,
            ("view.toggle_editor", None) => AppCommand::ToggleEditor,
            ("sequencer.group_tracks", None) => AppCommand::GroupTracks,
            ("sequencer.ungroup_tracks", None) => AppCommand::UngroupTracks,
            ("sequencer.toggle_folder", None) => AppCommand::ToggleFolder,
            ("sequencer.rename_folder", None) => AppCommand::RenameFolder,
            ("sequencer.move_track_up", None) => AppCommand::MoveTrackUp,
            ("sequencer.move_track_down", None) => AppCommand::MoveTrackDown,
            ("sequencer.rename_track", None) => AppCommand::RenameTrack,
            ("sequencer.track_comment", None) => AppCommand::TrackComment,
            ("sequencer.add_note_lane", None) => AppCommand::AddNoteLane,
            ("sequencer.rename_note_lane", None) => AppCommand::RenameNoteLane,
            ("sequencer.delete_note_lane", None) => AppCommand::DeleteNoteLane,
            ("sequencer.toggle_lane_mute", None) => AppCommand::ToggleLaneMute,
            ("sequencer.freeze_track", None) => AppCommand::FreezeTrack,
            ("sequencer.freeze_track_to_device", None) => AppCommand::FreezeTrackToDevice,
            ("sequencer.freeze_tail", None) => AppCommand::FreezeTail,
            ("sequencer.freeze_cancel", None) => AppCommand::FreezeCancel,
            ("sequencer.unfreeze_track", None) => AppCommand::UnfreezeTrack,
            ("sequencer.unfreeze_track_keep", None) => AppCommand::UnfreezeTrackKeep,
            ("sequencer.bounce_in_place", None) => AppCommand::BounceInPlace,
            ("sequencer.bounce_in_place_mute", None) => AppCommand::BounceInPlaceMute,
            ("sequencer.fold_lanes", Some("fold")) => AppCommand::FoldLanes(true),
            ("sequencer.fold_lanes", Some("unfold")) => AppCommand::FoldLanes(false),
            ("sequencer.track_height", Some(arg)) => {
                return HeightPreset::from_arg(arg).map(AppCommand::TrackHeight);
            }
            ("sequencer.audio_reverse", None) => AppCommand::AudioReverse,
            ("sequencer.insert_bars", None) => AppCommand::InsertBars,
            ("sequencer.remove_bars", None) => AppCommand::RemoveBars,
            ("sequencer.reverse_clips", Some("bars")) => AppCommand::ReverseClips(true),
            ("sequencer.reverse_clips", Some("starts")) => AppCommand::ReverseClips(false),
            ("sequencer.scale_tempo", Some(arg)) => {
                return ScaleStep::from_arg(arg).map(AppCommand::ScaleTempo);
            }
            ("sequencer.chop_clips", Some(arg)) => {
                return ChopStep::from_arg(arg).map(AppCommand::ChopClips);
            }
            ("sequencer.audio_normalise", None) => AppCommand::AudioNormalise,
            ("sequencer.audio_strip_silence", None) => AppCommand::AudioStripSilence,
            ("sequencer.audio_quantize", None) => AppCommand::AudioQuantize,
            ("sequencer.audio_warp", None) => AppCommand::AudioWarp,
            ("sequencer.audio_warp_mode", Some(arg)) => {
                return WarpMode::from_id(arg).map(AppCommand::AudioWarpMode);
            }
            ("sequencer.audio_warp_tempo", None) => AppCommand::AudioWarpTempo,
            ("sequencer.audio_warp_bars", Some(arg)) => {
                return arg
                    .parse()
                    .ok()
                    .filter(|b| [1, 2, 4, 8, 16].contains(b))
                    .map(AppCommand::AudioWarpBars);
            }
            ("sequencer.audio_warp_panel", None) => AppCommand::AudioWarpPanel,
            ("sequencer.audio_to_notes", None) => AppCommand::AudioToNotes,
            ("sequencer.audio_transpose", Some(arg)) => {
                return TransposeStep::from_arg(arg).map(AppCommand::AudioTranspose);
            }
            ("sequencer.audio_gain", Some(arg)) => {
                return GainStep::from_arg(arg).map(AppCommand::AudioGain);
            }
            ("sequencer.audio_fade_shape", Some(arg)) => {
                return FadeCurve::from_id(arg).map(AppCommand::AudioFadeShape);
            }
            ("sequencer.audio_crossfade", Some("off")) => AppCommand::AudioCrossfade(None),
            ("sequencer.audio_crossfade", Some(arg)) => {
                return FadeCurve::from_id(arg).map(|c| AppCommand::AudioCrossfade(Some(c)));
            }
            ("sequencer.audio_fades", Some("declick")) => AppCommand::AudioFades(true),
            ("sequencer.audio_fades", Some("clear")) => AppCommand::AudioFades(false),
            ("sequencer.audio_envelope_clear", None) => AppCommand::AudioClearEnvelope,
            ("audio.settings", None) => AppCommand::AudioSettings,
            ("audio.plugins", None) => AppCommand::PluginSettings,
            ("audio.rescan_midi", None) => AppCommand::RescanMidi,
            ("audio.restart", None) => AppCommand::RestartAudio,
            ("help.export_diagnostics", None) => AppCommand::ExportDiagnostics,
            ("help.keyboard_shortcuts", None) => AppCommand::KeyboardShortcuts,
            ("help.documentation", None) => AppCommand::Documentation,
            ("help.report_issue", None) => AppCommand::ReportIssue,
            ("help.about", None) => AppCommand::About,
            ("view.theme", Some(a)) => AppCommand::SetTheme(ThemeChoice::from_id(a)?),
            ("view.app_zoom", Some(a)) => AppCommand::AppZoom(ZoomChange::from_arg(a)?),
            ("view.focus_window", Some("main")) => AppCommand::FocusWindow(WindowKind::Main),
            ("view.focus_window", Some("rack")) => AppCommand::FocusWindow(WindowKind::Rack),
            ("view.focus_window", Some("mixer")) => AppCommand::FocusWindow(WindowKind::Console),
            _ => return None,
        })
    }

    /// The shortcut text of this command in `keymap`, if it has a command id and a binding.
    pub fn shortcut_text(&self, keymap: &Keymap) -> Option<String> {
        let (id, arg) = self.command_id()?;
        keymap.shortcut_text(id, arg)
    }
}

/// The text of a menu shortcut as a native macOS accelerator (`Cmd+Shift+S` becomes
/// `CmdOrCtrl+Shift+S`, `Opt` becomes `Alt`); only chords that start with the primary
/// modifier get one (Tab, F-keys and the like stay with the window).
pub fn native_accelerator_text(shortcut: &str) -> Option<String> {
    let rest = shortcut.strip_prefix("Cmd+")?;
    Some(format!("CmdOrCtrl+{}", rest.replace("Opt+", "Alt+")))
}

/// The tempo Edit ▸ Scale Tempo gives the selected clips, as a share of what it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScaleStep {
    /// 200 %: twice as fast.
    Double,
    /// 50 %: half as fast.
    Halve,
    /// 150 %.
    ThreeHalves,
    /// 125 %.
    FiveQuarters,
    /// 75 %.
    ThreeQuarters,
}

impl ScaleStep {
    /// Every step, in menu order.
    pub const ALL: [ScaleStep; 5] = [
        ScaleStep::Double,
        ScaleStep::Halve,
        ScaleStep::ThreeHalves,
        ScaleStep::FiveQuarters,
        ScaleStep::ThreeQuarters,
    ];

    /// The registry argument of `sequencer.scale_tempo`.
    pub fn arg(self) -> &'static str {
        match self {
            ScaleStep::Double => "double",
            ScaleStep::Halve => "halve",
            ScaleStep::ThreeHalves => "150",
            ScaleStep::FiveQuarters => "125",
            ScaleStep::ThreeQuarters => "75",
        }
    }

    /// The step of a registry argument.
    pub fn from_arg(arg: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.arg() == arg)
    }

    /// The menu text.
    pub fn label(self) -> &'static str {
        match self {
            ScaleStep::Double => "Double (200 %)",
            ScaleStep::Halve => "Halve (50 %)",
            ScaleStep::ThreeHalves => "150 %",
            ScaleStep::FiveQuarters => "125 %",
            ScaleStep::ThreeQuarters => "75 %",
        }
    }
}

/// The length of the pieces Edit ▸ Chop Clips To cuts the selected clips into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChopStep {
    /// A bar.
    Bar,
    /// Half a bar.
    HalfBar,
    /// A beat.
    Beat,
    /// An eighth of a whole note (half a beat in x/4).
    Eighth,
    /// A sixteenth of a whole note.
    Sixteenth,
}

impl ChopStep {
    /// Every step, in menu order.
    pub const ALL: [ChopStep; 5] = [
        ChopStep::Bar,
        ChopStep::HalfBar,
        ChopStep::Beat,
        ChopStep::Eighth,
        ChopStep::Sixteenth,
    ];

    /// The registry argument of `sequencer.chop_clips`.
    pub fn arg(self) -> &'static str {
        match self {
            ChopStep::Bar => "bar",
            ChopStep::HalfBar => "half_bar",
            ChopStep::Beat => "beat",
            ChopStep::Eighth => "eighth",
            ChopStep::Sixteenth => "sixteenth",
        }
    }

    /// The step of a registry argument.
    pub fn from_arg(arg: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.arg() == arg)
    }

    /// The menu text.
    pub fn label(self) -> &'static str {
        match self {
            ChopStep::Bar => "Bar",
            ChopStep::HalfBar => "Half Bar",
            ChopStep::Beat => "Beat",
            ChopStep::Eighth => "1/8 Note",
            ChopStep::Sixteenth => "1/16 Note",
        }
    }
}

/// What Audio ▸ Clip Gain does to the gain of the selected audio clips.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GainStep {
    /// 1 dB louder.
    Up,
    /// 1 dB quieter.
    Down,
    /// Back to 0 dB.
    Reset,
}

impl GainStep {
    /// The registry argument of `sequencer.audio_gain`.
    pub fn arg(self) -> &'static str {
        match self {
            GainStep::Up => "up",
            GainStep::Down => "down",
            GainStep::Reset => "reset",
        }
    }

    /// The step of a registry argument.
    pub fn from_arg(arg: &str) -> Option<Self> {
        match arg {
            "up" => Some(GainStep::Up),
            "down" => Some(GainStep::Down),
            "reset" => Some(GainStep::Reset),
            _ => None,
        }
    }
}

/// What Audio ▸ Transpose does to the pitch of the selected audio clips.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransposeStep {
    /// A semitone up.
    Up,
    /// A semitone down.
    Down,
    /// An octave up.
    OctaveUp,
    /// An octave down.
    OctaveDown,
    /// Back to the original pitch.
    Reset,
}

impl TransposeStep {
    /// The registry argument of `sequencer.audio_transpose`.
    pub fn arg(self) -> &'static str {
        match self {
            TransposeStep::Up => "up",
            TransposeStep::Down => "down",
            TransposeStep::OctaveUp => "octave_up",
            TransposeStep::OctaveDown => "octave_down",
            TransposeStep::Reset => "reset",
        }
    }

    /// The step of a registry argument.
    pub fn from_arg(arg: &str) -> Option<Self> {
        [
            TransposeStep::Up,
            TransposeStep::Down,
            TransposeStep::OctaveUp,
            TransposeStep::OctaveDown,
            TransposeStep::Reset,
        ]
        .into_iter()
        .find(|s| s.arg() == arg)
    }
}

/// What the menus show as checked or enabled.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MenuState {
    /// Visible side.
    pub flip: Flip,
    /// Cable visibility.
    pub cables: CableVisibility,
    /// Browser shown.
    pub browser: bool,
    /// File browser panel shown.
    pub file_browser: bool,
    /// Piano Keys strip shown.
    pub piano_keys: bool,
    /// Undo possible.
    pub can_undo: bool,
    /// Redo possible.
    pub can_redo: bool,
    /// The name of the step Undo runs (`"Move Device"`), when there is one.
    pub undo_label: Option<&'static str>,
    /// The name of the step Redo runs.
    pub redo_label: Option<&'static str>,
    /// The focused pane has a selection Cut, Copy, Duplicate and Delete can work on.
    pub can_edit: bool,
    /// The clipboard holds something the focused pane can paste.
    pub can_paste: bool,
    /// The rack's selection can be combined into a container.
    pub can_combine: bool,
    /// The rack's selection holds a container that can be uncombined.
    pub can_uncombine: bool,
    /// The arrangement area is shown.
    pub song_visible: bool,
    /// The piano roll is open.
    pub piano_roll: bool,
    /// The sample editor is open.
    pub sample_editor: bool,
    /// The step editor is open.
    pub step_editor: bool,
    /// The slice editor is open.
    pub slice_editor: bool,
    /// Playing.
    pub playing: bool,
    /// Recording.
    pub recording: bool,
    /// The song loop is on.
    pub loop_enabled: bool,
    /// The metronome is on.
    pub metronome: bool,
    /// A take also creates the lanes of the parameters moved during it.
    pub auto_lanes: bool,
    /// The pre-count (the count-in of a recording) is on.
    pub pre_count: bool,
    /// MIDI clock is being sent.
    pub clock_out: bool,
    /// The clock is sent while the transport is stopped too.
    pub clock_while_stopped: bool,
    /// The transport follows a MIDI clock.
    pub follow_clock: bool,
    /// The project's sample rate in Hz.
    pub sample_rate: u32,
    /// The host inputs the project asks for.
    pub input_channels: u16,
    /// The host outputs the project asks for.
    pub output_channels: u16,
    /// Input quantize for recording.
    pub quantize: InputQuantize,
    /// The console shows in the lower pane.
    pub console: bool,
    /// The console is maximised over the rack.
    pub console_maximised: bool,
    /// Delay compensation is on (document switch).
    pub compensation: bool,
    /// Return alignment is on (document switch).
    pub align_returns: bool,
    /// Console devices are shown in the rack.
    pub console_devices: bool,
    /// The console links are drawn as cables.
    pub console_links: bool,
    /// The arrangement shows the tempo lane.
    pub tempo_lane: bool,
    /// The arrangement shows the marker strip.
    pub marker_strip: bool,
    /// The arrangement shows the inspector strip.
    pub inspector: bool,
    /// The arrangement shows the marker list.
    pub marker_list: bool,
    /// The rack navigator is on.
    pub rack_navigator: bool,
    /// The rack is shown.
    pub rack: bool,
    /// The rack is in a window of its own.
    pub rack_detached: bool,
    /// The console is in a window of its own.
    pub console_detached: bool,
    /// The sequencer-only layout (F7) is active.
    pub sequencer_only: bool,
    /// An editor (piano roll, sample, step, slice, ...) is open.
    pub editor: bool,
    /// Audio files of the project are missing (File > Relink Missing Audio is available).
    pub missing_audio: bool,
    /// A note clip is selected or open (File > Export MIDI Clip is available).
    pub midi_clip: bool,
    /// A note track is selected (File > Export MIDI Track is available).
    pub midi_track: bool,
    /// The song has a note track (File > Export MIDI Song is available).
    pub midi_song: bool,
    /// The song can be exported as audio: it has a length and no export runs (File > Export Song
    /// as Audio File is available).
    pub export_song: bool,
    /// The loop can be exported as audio: its locators are apart and no export runs (File >
    /// Export Loop as Audio File is available).
    pub export_loop: bool,
    /// What the selected tracks and folder allow (the Track menu).
    pub org: OrgMenu,
    /// What the selected note lane allows (the lane items of the Track menu).
    pub lane: LaneMenu,
    /// What the selected audio clips allow (the audio items of the Edit and Audio menus).
    pub audio: AudioMenu,
    /// What the selected strips allow for VCA groups (the Mixer menu).
    pub vca: VcaMenu,
    /// What the selected strips allow for polarity and insert bypass (the Mixer menu).
    pub strip: StripMenu,
    /// What the mixer snapshots allow (the Mixer menu).
    pub snapshots: SnapshotMenu,
    /// What MIDI Learn and the mappings allow (the Mixer menu).
    pub midi: MidiMenu,
    /// What the selected tracks and clips allow for freezing and bouncing (the Track menu).
    pub freeze: FreezeMenu,
    /// Automation follows the selection (the View menu item is checked).
    pub follow_selection: bool,
    /// The colour theme in use (the View ▸ Theme item that is checked).
    pub theme: ThemeChoice,
    /// The application zoom in percent (the View ▸ Application Zoom item that is checked).
    pub app_zoom: u16,
}

/// What the Mixer menu needs to know for MIDI Learn and the mappings.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MidiMenu {
    /// MIDI Learn mode is on (the item is checked).
    pub learning: bool,
    /// The project has mappings: export and clear work.
    pub has_maps: bool,
}

/// What the Mixer menu needs to know to store, recall, compare and delete mixer snapshots.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SnapshotMenu {
    /// Fewer than the most snapshots: Store Snapshot works.
    pub can_store: bool,
    /// A snapshot is current and the mix differs from it: Update Snapshot works.
    pub can_update: bool,
    /// A snapshot is current: Recall Snapshot works.
    pub can_recall: bool,
    /// There are snapshots: Recall Next and Previous work.
    pub can_cycle: bool,
    /// There are snapshots: Compare A/B works.
    pub can_compare: bool,
    /// A compare is on: End Compare works.
    pub comparing: bool,
    /// A snapshot is current: Delete Snapshot works.
    pub can_delete: bool,
}

/// What the Mixer menu needs to know to flip the polarity and bypass the inserts of strips.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StripMenu {
    /// Some channel or bus is selected: Toggle Polarity works.
    pub can_polarity: bool,
    /// A selected strip has an insert effect that can be bypassed: Bypass All Inserts works.
    pub can_bypass: bool,
}

/// What the Mixer menu needs to know to make, fill, empty and delete VCA groups.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VcaMenu {
    /// Some channel or bus is selected: Create VCA from Selection works.
    pub can_create: bool,
    /// Some strip is selected and a group exists: Add to VCA works.
    pub can_add: bool,
    /// A selected strip is in a group: Remove from VCA works.
    pub can_remove: bool,
    /// A group exists: Delete VCA works (on the selected strips' groups, or the last one).
    pub can_delete: bool,
}

/// What the Track menu and the context menus need to know to freeze, unfreeze and bounce.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FreezeMenu {
    /// A selected track can be frozen: it has clips and a channel, is not frozen, and no render
    /// runs.
    pub can_freeze: bool,
    /// A selected track is frozen.
    pub can_unfreeze: bool,
    /// A selected clip can be bounced (a note or audio clip of an unfrozen track), and no render
    /// runs.
    pub can_bounce: bool,
    /// A render runs or waits (Cancel Freeze works).
    pub busy: bool,
    /// Freezes keep the tail of the chain (the item is checked).
    pub tail: bool,
}

/// One menu item.
#[derive(Debug, Clone, PartialEq)]
pub struct MenuItem {
    /// Label (`…` marks items that open a dialog). Most are fixed texts; the recent songs are
    /// named after their files.
    pub label: Cow<'static, str>,
    /// What it does.
    pub command: AppCommand,
    /// Shortcut text derived from the keymap and the OS, e.g. `Ctrl+S` (`Cmd+S` on macOS).
    pub shortcut: Option<String>,
    /// `Some(checked)` for check/radio items.
    pub checked: Option<bool>,
    /// Whether it can be chosen.
    pub enabled: bool,
}

/// A menu entry.
#[derive(Debug, Clone, PartialEq)]
pub enum MenuEntry {
    /// An item.
    Item(MenuItem),
    /// A separator line.
    Separator,
    /// An entry that opens a list of its own (File ▸ Open Recent).
    Submenu(Submenu),
}

/// A submenu: a label and the entries it opens.
#[derive(Debug, Clone, PartialEq)]
pub struct Submenu {
    /// Text of the entry in the menu.
    pub label: &'static str,
    /// The entries, top to bottom.
    pub entries: Vec<MenuEntry>,
}

impl Submenu {
    /// The items (separators skipped).
    pub fn items(&self) -> impl Iterator<Item = &MenuItem> {
        self.entries.iter().flat_map(MenuEntry::items)
    }
}

impl MenuEntry {
    /// The items this entry is or holds: none for a separator, the entry itself for an item,
    /// the items of its entries for a submenu.
    pub fn items(&self) -> Box<dyn Iterator<Item = &MenuItem> + '_> {
        match self {
            MenuEntry::Item(i) => Box::new(std::iter::once(i)),
            MenuEntry::Separator => Box::new(std::iter::empty()),
            MenuEntry::Submenu(s) => Box::new(s.items()),
        }
    }
}

/// A top-level menu.
#[derive(Debug, Clone, PartialEq)]
pub struct Menu {
    /// Title in the menu bar.
    pub title: &'static str,
    /// Entries, top to bottom.
    pub entries: Vec<MenuEntry>,
}

impl Menu {
    /// The items (separators skipped, the items of submenus included).
    pub fn items(&self) -> impl Iterator<Item = &MenuItem> {
        self.entries.iter().flat_map(MenuEntry::items)
    }
}

/// The menu bar: File, Edit, View, Transport, Mixer, Audio. Shortcut texts come from `keymap`
/// (its OS decides `Ctrl` or `Cmd`); no key is written in this function.
pub fn menus(state: &MenuState, keymap: &Keymap) -> Vec<Menu> {
    menus_with_recent(state, keymap, &[])
}

/// [`menus`], with the File ▸ Open Recent submenu listing `recent` (the label of each song,
/// newest first; at most [`RECENT_ARGS`]'s length are shown).
pub fn menus_with_recent(state: &MenuState, keymap: &Keymap, recent: &[String]) -> Vec<Menu> {
    let make = |label: &'static str, command: AppCommand, checked, enabled| {
        MenuEntry::Item(MenuItem {
            label: Cow::Borrowed(label),
            command,
            shortcut: command.shortcut_text(keymap),
            checked,
            enabled,
        })
    };
    // Undo and Redo name the step they run ("Undo Move Device").
    let step = |verb: &'static str, label: Option<&'static str>, command: AppCommand, enabled| {
        let text = match label {
            Some(l) if enabled => Cow::Owned(format!("{verb} {l}")),
            _ => Cow::Borrowed(verb),
        };
        MenuEntry::Item(MenuItem {
            label: text,
            command,
            shortcut: command.shortcut_text(keymap),
            checked: None,
            enabled,
        })
    };
    let item = |label, command| make(label, command, None, true);
    let check = |label, command, checked| make(label, command, Some(checked), true);
    let enabled = |label, command, enabled| make(label, command, None, enabled);
    let cables = |label, v: CableVisibility| check(label, AppCommand::Cables(v), state.cables == v);
    let quantize =
        |label, q: InputQuantize| check(label, AppCommand::Quantize(q), state.quantize == q);
    let transport = AppCommand::Transport;
    let open_recent = MenuEntry::Submenu(Submenu {
        label: "Open Recent",
        entries: recent
            .iter()
            .take(RECENT_ARGS.len())
            .enumerate()
            .map(|(i, name)| {
                MenuEntry::Item(MenuItem {
                    label: Cow::Owned(name.clone()),
                    command: AppCommand::OpenRecent(i),
                    shortcut: None,
                    checked: None,
                    enabled: true,
                })
            })
            .chain([
                MenuEntry::Separator,
                enabled("Clear Menu", AppCommand::ClearRecent, !recent.is_empty()),
            ])
            .collect(),
    });
    vec![
        Menu {
            title: "File",
            entries: vec![
                item("New", AppCommand::New),
                item("Open\u{2026}", AppCommand::Open),
                open_recent,
                item("Close", AppCommand::Close),
                item("Save", AppCommand::Save),
                item("Save As\u{2026}", AppCommand::SaveAs),
                MenuEntry::Submenu(Submenu {
                    label: "New at Sample Rate",
                    entries: vec![
                        item("44.1 kHz", AppCommand::NewAtRate(44_100)),
                        item("48 kHz", AppCommand::NewAtRate(48_000)),
                        item("96 kHz", AppCommand::NewAtRate(96_000)),
                    ],
                }),
                MenuEntry::Submenu(Submenu {
                    label: "Convert Sample Rate",
                    entries: vec![
                        check(
                            "44.1 kHz",
                            AppCommand::ConvertRate(44_100),
                            state.sample_rate == 44_100,
                        ),
                        check(
                            "48 kHz",
                            AppCommand::ConvertRate(48_000),
                            state.sample_rate == 48_000,
                        ),
                        check(
                            "96 kHz",
                            AppCommand::ConvertRate(96_000),
                            state.sample_rate == 96_000,
                        ),
                    ],
                }),
                item("Import Audio\u{2026}", AppCommand::ImportAudio),
                item("Import MIDI\u{2026}", AppCommand::ImportMidi),
                item("Import MIDI as Song\u{2026}", AppCommand::ImportMidiSong),
                enabled(
                    "Relink Missing Audio\u{2026}",
                    AppCommand::RelinkAudio,
                    state.missing_audio,
                ),
                item("Export Patch\u{2026}", AppCommand::ExportPatch),
                enabled(
                    "Export MIDI Clip\u{2026}",
                    AppCommand::ExportMidiClip,
                    state.midi_clip,
                ),
                enabled(
                    "Export MIDI Track\u{2026}",
                    AppCommand::ExportMidiTrack,
                    state.midi_track,
                ),
                enabled(
                    "Export MIDI Song\u{2026}",
                    AppCommand::ExportMidiSong,
                    state.midi_song,
                ),
                enabled(
                    "Export Song as Audio File\u{2026}",
                    AppCommand::ExportSongAudio,
                    state.export_song,
                ),
                enabled(
                    "Export Loop as Audio File\u{2026}",
                    AppCommand::ExportLoopAudio,
                    state.export_loop,
                ),
                MenuEntry::Separator,
                item("Stress Rack", AppCommand::StressRack),
                item("Stress Song", AppCommand::StressSong),
                MenuEntry::Separator,
                item("Quit", AppCommand::Quit),
            ],
        },
        Menu {
            title: "Edit",
            entries: vec![
                step("Undo", state.undo_label, AppCommand::Undo, state.can_undo),
                step("Redo", state.redo_label, AppCommand::Redo, state.can_redo),
                MenuEntry::Separator,
                enabled("Cut", AppCommand::Cut, state.can_edit),
                enabled("Copy", AppCommand::Copy, state.can_edit),
                enabled("Paste", AppCommand::Paste, state.can_paste),
                enabled("Duplicate", AppCommand::Duplicate, state.can_edit),
                enabled("Delete", AppCommand::Delete, state.can_edit),
                MenuEntry::Separator,
                item("Select All", AppCommand::SelectAll),
                MenuEntry::Separator,
                enabled("Combine", AppCommand::Combine, state.can_combine),
                enabled("Uncombine", AppCommand::Uncombine, state.can_uncombine),
                MenuEntry::Separator,
                enabled("Quantize", AppCommand::QuantizeApply, state.can_edit),
                item("Quantize Options\u{2026}", AppCommand::QuantizeOptions),
                MenuEntry::Separator,
                enabled(
                    "Reverse Audio",
                    AppCommand::AudioReverse,
                    state.audio.can_edit,
                ),
                enabled(
                    "Normalise Audio",
                    AppCommand::AudioNormalise,
                    state.audio.can_edit,
                ),
                enabled(
                    "Strip Silence\u{2026}",
                    AppCommand::AudioStripSilence,
                    state.audio.can_edit,
                ),
                enabled(
                    "Quantize Audio",
                    AppCommand::AudioQuantize,
                    state.audio.can_warp,
                ),
                MenuEntry::Separator,
                item("Insert Bars at Locators", AppCommand::InsertBars),
                item("Remove Bars at Locators", AppCommand::RemoveBars),
                MenuEntry::Separator,
                item("Reverse Clips", AppCommand::ReverseClips(true)),
                item("Reverse Note Starts", AppCommand::ReverseClips(false)),
                MenuEntry::Submenu(Submenu {
                    label: "Scale Tempo",
                    entries: ScaleStep::ALL
                        .into_iter()
                        .map(|s| item(s.label(), AppCommand::ScaleTempo(s)))
                        .collect(),
                }),
                MenuEntry::Submenu(Submenu {
                    label: "Chop Clips To",
                    entries: ChopStep::ALL
                        .into_iter()
                        .map(|s| item(s.label(), AppCommand::ChopClips(s)))
                        .collect(),
                }),
            ],
        },
        Menu {
            title: "View",
            entries: vec![
                check(
                    "Back of Rack",
                    AppCommand::ToggleFlip,
                    state.flip == Flip::Back,
                ),
                MenuEntry::Separator,
                cables("Cables: Show", CableVisibility::Show),
                cables("Cables: Hover", CableVisibility::HoverOnly),
                cables("Cables: Selected", CableVisibility::SelectedOnly),
                cables("Cables: Hide Auto-routed", CableVisibility::HideAutoRouted),
                cables("Cables: Hide", CableVisibility::Hide),
                MenuEntry::Separator,
                item("Zoom In", AppCommand::ZoomIn),
                item("Zoom Out", AppCommand::ZoomOut),
                item("Actual Size", AppCommand::ZoomReset),
                MenuEntry::Submenu(Submenu {
                    label: "Application Zoom",
                    entries: crate::keymap_store::ZOOM_STEPS
                        .into_iter()
                        .map(|p| {
                            make(
                                match p {
                                    75 => "75 %",
                                    100 => "100 %",
                                    125 => "125 %",
                                    150 => "150 %",
                                    175 => "175 %",
                                    _ => "200 %",
                                },
                                AppCommand::AppZoom(ZoomChange::To(p)),
                                Some(state.app_zoom == p),
                                true,
                            )
                        })
                        .collect(),
                }),
                MenuEntry::Submenu(Submenu {
                    label: "Theme",
                    entries: ThemeChoice::ALL
                        .into_iter()
                        .map(|t| {
                            make(
                                t.label(),
                                AppCommand::SetTheme(t),
                                Some(state.theme == t),
                                true,
                            )
                        })
                        .collect(),
                }),
                MenuEntry::Separator,
                check("Device Browser", AppCommand::ToggleBrowser, state.browser),
                check(
                    "File Browser",
                    AppCommand::ToggleFileBrowser,
                    state.file_browser,
                ),
                check("Piano Keys", AppCommand::TogglePianoKeys, state.piano_keys),
                check("Rack", AppCommand::ToggleRack, state.rack),
                check("Detach Rack", AppCommand::DetachRack, state.rack_detached),
                check(
                    "Detach Mixer",
                    AppCommand::DetachConsole,
                    state.console_detached,
                ),
                check(
                    "Rack Navigator",
                    AppCommand::ToggleRackNavigator,
                    state.rack_navigator,
                ),
                check(
                    "Sequencer Only",
                    AppCommand::SequencerOnly,
                    state.sequencer_only,
                ),
                check("Clip Editor", AppCommand::ToggleEditor, state.editor),
                MenuEntry::Separator,
                check("Arrangement", AppCommand::ToggleSong, state.song_visible),
                check("Piano Roll", AppCommand::TogglePianoRoll, state.piano_roll),
                check(
                    "Sample Editor",
                    AppCommand::ToggleSampleEditor,
                    state.sample_editor,
                ),
                check(
                    "Step Editor",
                    AppCommand::ToggleStepEditor,
                    state.step_editor,
                ),
                check(
                    "Slice Editor",
                    AppCommand::ToggleSliceEditor,
                    state.slice_editor,
                ),
                MenuEntry::Separator,
                check(
                    "Show Console Devices in Rack",
                    AppCommand::ToggleConsoleDevices,
                    state.console_devices,
                ),
                check(
                    "Show Console Links",
                    AppCommand::ToggleConsoleLinks,
                    state.console_links,
                ),
                MenuEntry::Separator,
                check("Tempo Lane", AppCommand::ToggleTempoLane, state.tempo_lane),
                check(
                    "Marker Strip",
                    AppCommand::ToggleMarkerStrip,
                    state.marker_strip,
                ),
                check(
                    "Marker List",
                    AppCommand::ToggleMarkerList,
                    state.marker_list,
                ),
                check("Inspector", AppCommand::ToggleInspector, state.inspector),
                MenuEntry::Separator,
                item("Find Parameter\u{2026}", AppCommand::FindParameter),
                check(
                    "Automation Follows Selection",
                    AppCommand::FollowSelection,
                    state.follow_selection,
                ),
                item("Show Automation Lane", AppCommand::ShowParamLane),
            ],
        },
        Menu {
            title: "Transport",
            entries: vec![
                check(
                    "Play / Stop",
                    transport(TransportAction::TogglePlay),
                    state.playing,
                ),
                check(
                    "Record",
                    transport(TransportAction::ToggleRecord),
                    state.recording,
                ),
                item("Return to Zero", transport(TransportAction::ReturnToZero)),
                MenuEntry::Separator,
                check(
                    "Loop",
                    transport(TransportAction::ToggleLoop),
                    state.loop_enabled,
                ),
                check(
                    "Metronome",
                    transport(TransportAction::ToggleMetronome),
                    state.metronome,
                ),
                check(
                    "Auto-create Lanes",
                    transport(TransportAction::ToggleAutoLanes),
                    state.auto_lanes,
                ),
                MenuEntry::Separator,
                item("Add Marker", AppCommand::AddMarker),
                item("Add Range Marker from Loop", AppCommand::AddRangeFromLoop),
                item("Set End Marker at Playhead", AppCommand::SetEndMarker),
                item("Set Left Locator at Playhead", AppCommand::SetLeftLocator),
                item("Set Right Locator at Playhead", AppCommand::SetRightLocator),
                item("Previous Marker", AppCommand::PreviousMarker),
                item("Next Marker", AppCommand::NextMarker),
                MenuEntry::Separator,
                quantize("Input Quantize: Off", InputQuantize::OFF),
                quantize("Input Quantize: 1/8", InputQuantize::EIGHTH),
                quantize("Input Quantize: 1/16", InputQuantize::SIXTEENTH),
            ],
        },
        Menu {
            title: "Mixer",
            entries: vec![
                check("Console", AppCommand::ToggleConsole, state.console),
                check(
                    "Maximise Console",
                    AppCommand::MaximiseConsole,
                    state.console_maximised,
                ),
                MenuEntry::Separator,
                item("Add Channel\u{2026}", AppCommand::AddChannel),
                item("Route to New Output Bus", AppCommand::RouteToNewBus),
                MenuEntry::Separator,
                enabled(
                    "Create VCA from Selection",
                    AppCommand::CreateVca,
                    state.vca.can_create,
                ),
                enabled("Add to VCA", AppCommand::AddToVca(None), state.vca.can_add),
                enabled(
                    "Remove from VCA",
                    AppCommand::RemoveFromVca,
                    state.vca.can_remove,
                ),
                enabled(
                    "Delete VCA",
                    AppCommand::DeleteVca(None),
                    state.vca.can_delete,
                ),
                MenuEntry::Separator,
                enabled(
                    "Toggle Polarity",
                    AppCommand::TogglePolarity,
                    state.strip.can_polarity,
                ),
                enabled(
                    "Bypass All Inserts",
                    AppCommand::BypassAllInserts,
                    state.strip.can_bypass,
                ),
                MenuEntry::Separator,
                enabled(
                    "Store Snapshot",
                    AppCommand::StoreSnapshot,
                    state.snapshots.can_store,
                ),
                enabled(
                    "Update Snapshot",
                    AppCommand::UpdateSnapshot(None),
                    state.snapshots.can_update,
                ),
                enabled(
                    "Recall Snapshot",
                    AppCommand::RecallSnapshot(None),
                    state.snapshots.can_recall,
                ),
                enabled(
                    "Recall Next Snapshot",
                    AppCommand::RecallNextSnapshot,
                    state.snapshots.can_cycle,
                ),
                enabled(
                    "Recall Previous Snapshot",
                    AppCommand::RecallPreviousSnapshot,
                    state.snapshots.can_cycle,
                ),
                enabled(
                    "Compare A/B",
                    AppCommand::CompareSnapshot(None),
                    state.snapshots.can_compare,
                ),
                enabled(
                    "End Compare",
                    AppCommand::EndCompare,
                    state.snapshots.comparing,
                ),
                enabled(
                    "Delete Snapshot",
                    AppCommand::DeleteSnapshot(None),
                    state.snapshots.can_delete,
                ),
                MenuEntry::Separator,
                check(
                    "Delay Compensation",
                    AppCommand::ToggleCompensation,
                    state.compensation,
                ),
                check(
                    "Align Returns",
                    AppCommand::ToggleReturnAlignment,
                    state.align_returns,
                ),
                MenuEntry::Separator,
                check("MIDI Learn", AppCommand::MidiLearn, state.midi.learning),
                enabled("MIDI Mappings\u{2026}", AppCommand::MidiMappings, true),
                enabled(
                    "Export MIDI Mapping Preset\u{2026}",
                    AppCommand::ExportMidiPreset,
                    state.midi.has_maps,
                ),
                enabled(
                    "Import MIDI Mapping Preset\u{2026}",
                    AppCommand::ImportMidiPreset,
                    true,
                ),
                enabled(
                    "Clear MIDI Mappings",
                    AppCommand::ClearMidiMaps,
                    state.midi.has_maps,
                ),
                MenuEntry::Separator,
                enabled("Macros\u{2026}", AppCommand::MacroPanel, true),
                enabled("New Macro Bank", AppCommand::NewMacroBank, true),
            ],
        },
        Menu {
            title: "Create",
            entries: vec![
                item("Create Instrument\u{2026}", AppCommand::CreateInstrument),
                item("Create Effect\u{2026}", AppCommand::CreateEffect),
                item("Create Player\u{2026}", AppCommand::CreatePlayer),
                item("Create Audio Track", AppCommand::CreateAudioTrack),
                item(
                    "Create Mixer Channel\u{2026}",
                    AppCommand::CreateMixerChannel,
                ),
            ],
        },
        Menu {
            title: "Track",
            entries: vec![
                enabled("Group Tracks", AppCommand::GroupTracks, state.org.can_group),
                enabled("Ungroup", AppCommand::UngroupTracks, state.org.in_folder),
                enabled(
                    if state.org.collapsed {
                        "Expand Folder"
                    } else {
                        "Collapse Folder"
                    },
                    AppCommand::ToggleFolder,
                    state.org.in_folder,
                ),
                enabled(
                    "Rename Folder\u{2026}",
                    AppCommand::RenameFolder,
                    state.org.folder_selected,
                ),
                MenuEntry::Separator,
                enabled(
                    "Move Track Up",
                    AppCommand::MoveTrackUp,
                    state.org.can_move_up,
                ),
                enabled(
                    "Move Track Down",
                    AppCommand::MoveTrackDown,
                    state.org.can_move_down,
                ),
                enabled(
                    "Rename Track\u{2026}",
                    AppCommand::RenameTrack,
                    state.org.track_selected,
                ),
                enabled(
                    "Track Comment\u{2026}",
                    AppCommand::TrackComment,
                    state.org.track_selected,
                ),
                MenuEntry::Separator,
                enabled("Add Note Lane", AppCommand::AddNoteLane, state.lane.can_add),
                enabled(
                    "Rename Note Lane\u{2026}",
                    AppCommand::RenameNoteLane,
                    state.lane.can_rename,
                ),
                enabled(
                    "Delete Note Lane",
                    AppCommand::DeleteNoteLane,
                    state.lane.can_delete,
                ),
                make(
                    "Mute Lane",
                    AppCommand::ToggleLaneMute,
                    Some(state.lane.muted),
                    state.lane.can_rename,
                ),
                MenuEntry::Separator,
                enabled(
                    "Freeze Track",
                    AppCommand::FreezeTrack,
                    state.freeze.can_freeze,
                ),
                enabled(
                    "Freeze Track up to Selected Device",
                    AppCommand::FreezeTrackToDevice,
                    state.freeze.can_freeze,
                ),
                check(
                    "Freeze with Tail",
                    AppCommand::FreezeTail,
                    state.freeze.tail,
                ),
                enabled(
                    "Unfreeze Track",
                    AppCommand::UnfreezeTrack,
                    state.freeze.can_unfreeze,
                ),
                enabled(
                    "Unfreeze Track and Keep Audio",
                    AppCommand::UnfreezeTrackKeep,
                    state.freeze.can_unfreeze,
                ),
                enabled("Cancel Freeze", AppCommand::FreezeCancel, state.freeze.busy),
                MenuEntry::Separator,
                enabled(
                    "Bounce in Place",
                    AppCommand::BounceInPlace,
                    state.freeze.can_bounce,
                ),
                enabled(
                    "Bounce in Place and Mute Originals",
                    AppCommand::BounceInPlaceMute,
                    state.freeze.can_bounce,
                ),
                MenuEntry::Separator,
                item("Fold Automation Lanes", AppCommand::FoldLanes(true)),
                item("Unfold Automation Lanes", AppCommand::FoldLanes(false)),
                MenuEntry::Separator,
                item(
                    "Track Height: Small",
                    AppCommand::TrackHeight(HeightPreset::Small),
                ),
                item(
                    "Track Height: Medium",
                    AppCommand::TrackHeight(HeightPreset::Medium),
                ),
                item(
                    "Track Height: Large",
                    AppCommand::TrackHeight(HeightPreset::Large),
                ),
                item(
                    "Track Height: Fit",
                    AppCommand::TrackHeight(HeightPreset::Fit),
                ),
            ],
        },
        Menu {
            title: "Audio",
            entries: vec![
                item("Settings\u{2026}", AppCommand::AudioSettings),
                item("Plugins\u{2026}", AppCommand::PluginSettings),
                item("Restart Audio", AppCommand::RestartAudio),
                item("Rescan MIDI Ports", AppCommand::RescanMidi),
                MenuEntry::Submenu(Submenu {
                    label: "Input Channels",
                    entries: [
                        ("0 (off)", 0u16),
                        ("1", 1),
                        ("2", 2),
                        ("4", 4),
                        ("8", 8),
                        ("16", 16),
                        ("32", 32),
                    ]
                    .into_iter()
                    .map(|(label, n)| {
                        check(
                            label,
                            AppCommand::InputChannels(n),
                            state.input_channels == n,
                        )
                    })
                    .collect(),
                }),
                MenuEntry::Submenu(Submenu {
                    label: "Output Channels",
                    entries: [
                        ("1", 1u16),
                        ("2", 2),
                        ("4", 4),
                        ("8", 8),
                        ("16", 16),
                        ("32", 32),
                    ]
                    .into_iter()
                    .map(|(label, n)| {
                        check(
                            label,
                            AppCommand::OutputChannels(n),
                            state.output_channels == n,
                        )
                    })
                    .collect(),
                }),
                MenuEntry::Separator,
                enabled(
                    "Clip Gain: Up 1 dB",
                    AppCommand::AudioGain(GainStep::Up),
                    state.audio.can_edit,
                ),
                enabled(
                    "Clip Gain: Down 1 dB",
                    AppCommand::AudioGain(GainStep::Down),
                    state.audio.can_edit,
                ),
                enabled(
                    "Clip Gain: Reset",
                    AppCommand::AudioGain(GainStep::Reset),
                    state.audio.can_edit,
                ),
                MenuEntry::Separator,
                enabled(
                    "Fades: 5 ms Declick",
                    AppCommand::AudioFades(true),
                    state.audio.can_edit,
                ),
                enabled(
                    "Fades: None",
                    AppCommand::AudioFades(false),
                    state.audio.can_edit,
                ),
                enabled(
                    "Clear Gain Envelope",
                    AppCommand::AudioClearEnvelope,
                    state.audio.envelope,
                ),
                MenuEntry::Separator,
                enabled(
                    "Fade Shape: Linear",
                    AppCommand::AudioFadeShape(FadeCurve::Linear),
                    state.audio.can_edit,
                ),
                enabled(
                    "Fade Shape: Equal Power",
                    AppCommand::AudioFadeShape(FadeCurve::EqualPower),
                    state.audio.can_edit,
                ),
                enabled(
                    "Fade Shape: Exponential",
                    AppCommand::AudioFadeShape(FadeCurve::Exponential),
                    state.audio.can_edit,
                ),
                enabled(
                    "Fade Shape: S-curve",
                    AppCommand::AudioFadeShape(FadeCurve::SCurve),
                    state.audio.can_edit,
                ),
                MenuEntry::Separator,
                enabled(
                    "Crossfade: Off",
                    AppCommand::AudioCrossfade(None),
                    state.audio.can_edit,
                ),
                enabled(
                    "Crossfade: Linear",
                    AppCommand::AudioCrossfade(Some(FadeCurve::Linear)),
                    state.audio.can_edit,
                ),
                enabled(
                    "Crossfade: Equal Power",
                    AppCommand::AudioCrossfade(Some(FadeCurve::EqualPower)),
                    state.audio.can_edit,
                ),
                enabled(
                    "Crossfade: Exponential",
                    AppCommand::AudioCrossfade(Some(FadeCurve::Exponential)),
                    state.audio.can_edit,
                ),
                enabled(
                    "Crossfade: S-curve",
                    AppCommand::AudioCrossfade(Some(FadeCurve::SCurve)),
                    state.audio.can_edit,
                ),
                MenuEntry::Separator,
                enabled("Warp Audio", AppCommand::AudioWarp, state.audio.can_warp),
                enabled(
                    "Warp to Song Tempo",
                    AppCommand::AudioWarpTempo,
                    state.audio.can_warp,
                ),
                enabled(
                    "Warp Settings\u{2026}",
                    AppCommand::AudioWarpPanel,
                    state.audio.can_edit,
                ),
                enabled(
                    "Warp Mode: Beats",
                    AppCommand::AudioWarpMode(WarpMode::Beats),
                    state.audio.can_edit,
                ),
                enabled(
                    "Warp Mode: Tonal",
                    AppCommand::AudioWarpMode(WarpMode::Tonal),
                    state.audio.can_edit,
                ),
                enabled(
                    "Warp Mode: Repitch",
                    AppCommand::AudioWarpMode(WarpMode::Repitch),
                    state.audio.can_edit,
                ),
                enabled(
                    "Warp Loop as 1 Bar",
                    AppCommand::AudioWarpBars(1),
                    state.audio.can_warp,
                ),
                enabled(
                    "Warp Loop as 2 Bars",
                    AppCommand::AudioWarpBars(2),
                    state.audio.can_warp,
                ),
                enabled(
                    "Warp Loop as 4 Bars",
                    AppCommand::AudioWarpBars(4),
                    state.audio.can_warp,
                ),
                enabled(
                    "Warp Loop as 8 Bars",
                    AppCommand::AudioWarpBars(8),
                    state.audio.can_warp,
                ),
                MenuEntry::Separator,
                enabled(
                    "Transpose: Up a Semitone",
                    AppCommand::AudioTranspose(TransposeStep::Up),
                    state.audio.can_warp,
                ),
                enabled(
                    "Transpose: Down a Semitone",
                    AppCommand::AudioTranspose(TransposeStep::Down),
                    state.audio.can_warp,
                ),
                enabled(
                    "Transpose: Up an Octave",
                    AppCommand::AudioTranspose(TransposeStep::OctaveUp),
                    state.audio.can_warp,
                ),
                enabled(
                    "Transpose: Down an Octave",
                    AppCommand::AudioTranspose(TransposeStep::OctaveDown),
                    state.audio.can_warp,
                ),
                enabled(
                    "Transpose: Reset",
                    AppCommand::AudioTranspose(TransposeStep::Reset),
                    state.audio.can_warp,
                ),
                MenuEntry::Separator,
                enabled(
                    "Audio to Notes\u{2026}",
                    AppCommand::AudioToNotes,
                    state.audio.can_edit,
                ),
            ],
        },
        // The playback and recording settings, gathered: each entry is the command it already
        // has in its own menu, which keeps working.
        Menu {
            title: "Options",
            entries: vec![
                check(
                    "Click",
                    transport(TransportAction::ToggleMetronome),
                    state.metronome,
                ),
                check("Pre-count", AppCommand::TogglePreCount, state.pre_count),
                MenuEntry::Separator,
                quantize("Input Quantize: Off", InputQuantize::OFF),
                quantize("Input Quantize: 1/8", InputQuantize::EIGHTH),
                quantize("Input Quantize: 1/16", InputQuantize::SIXTEENTH),
                MenuEntry::Separator,
                item("Chase Notes", AppCommand::ToggleChase),
                check(
                    "Delay Compensation",
                    AppCommand::ToggleCompensation,
                    state.compensation,
                ),
                check(
                    "Align Returns",
                    AppCommand::ToggleReturnAlignment,
                    state.align_returns,
                ),
                MenuEntry::Separator,
                MenuEntry::Submenu(Submenu {
                    label: "Sync",
                    entries: vec![
                        check(
                            "MIDI Clock Out",
                            AppCommand::ToggleClockOut,
                            state.clock_out,
                        ),
                        check(
                            "Send Clock While Stopped",
                            AppCommand::ToggleClockWhileStopped,
                            state.clock_while_stopped,
                        ),
                        check(
                            "Follow MIDI Clock",
                            AppCommand::ToggleClockIn,
                            state.follow_clock,
                        ),
                    ],
                }),
                MenuEntry::Separator,
                item("Settings\u{2026}", AppCommand::AudioSettings),
            ],
        },
        Menu {
            title: "Window",
            entries: vec![
                enabled(
                    "Main Window",
                    AppCommand::FocusWindow(WindowKind::Main),
                    true,
                ),
                enabled(
                    "Rack Window",
                    AppCommand::FocusWindow(WindowKind::Rack),
                    state.rack_detached,
                ),
                enabled(
                    "Mixer Window",
                    AppCommand::FocusWindow(WindowKind::Console),
                    state.console_detached,
                ),
            ],
        },
        Menu {
            title: "Help",
            entries: vec![
                item("Keyboard Shortcuts", AppCommand::KeyboardShortcuts),
                item("Documentation", AppCommand::Documentation),
                item("Report an Issue", AppCommand::ReportIssue),
                MenuEntry::Separator,
                item("Export Diagnostics\u{2026}", AppCommand::ExportDiagnostics),
                MenuEntry::Separator,
                item("About Atomik Rack", AppCommand::About),
            ],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_ui::keymap::{Os, Profile, Registry};

    fn legacy(os: Os) -> Keymap {
        Keymap::bundled(Profile::Legacy, os)
    }

    fn state() -> MenuState {
        MenuState {
            flip: Flip::Front,
            cables: CableVisibility::Show,
            browser: false,
            file_browser: false,
            piano_keys: false,
            can_undo: true,
            can_redo: false,
            undo_label: None,
            redo_label: None,
            can_edit: false,
            can_paste: false,
            can_combine: false,
            can_uncombine: false,
            song_visible: true,
            piano_roll: false,
            sample_editor: false,
            step_editor: false,
            slice_editor: false,
            playing: false,
            recording: false,
            loop_enabled: false,
            metronome: false,
            auto_lanes: false,
            pre_count: false,
            clock_out: false,
            clock_while_stopped: false,
            follow_clock: false,
            sample_rate: 48_000,
            input_channels: 2,
            output_channels: 2,
            quantize: InputQuantize::OFF,
            console: false,
            console_maximised: false,
            compensation: true,
            align_returns: true,
            console_devices: false,
            console_links: false,
            tempo_lane: false,
            marker_strip: false,
            inspector: false,
            marker_list: false,
            rack_navigator: true,
            rack: true,
            rack_detached: false,
            console_detached: false,
            sequencer_only: false,
            editor: false,
            missing_audio: false,
            midi_clip: false,
            midi_track: false,
            midi_song: false,
            export_song: false,
            export_loop: false,
            org: OrgMenu::default(),
            lane: LaneMenu::default(),
            audio: AudioMenu::default(),
            vca: VcaMenu::default(),
            strip: StripMenu::default(),
            snapshots: SnapshotMenu::default(),
            midi: MidiMenu::default(),
            freeze: FreezeMenu::default(),
            follow_selection: false,
            theme: ThemeChoice::Dark,
            app_zoom: 100,
        }
    }

    /// Every app command (a variant of each kind), for the bijection tests.
    fn all_commands() -> Vec<AppCommand> {
        let mut v = vec![
            AppCommand::New,
            AppCommand::Open,
            AppCommand::OpenRecent(0),
            AppCommand::OpenRecent(7),
            AppCommand::ClearRecent,
            AppCommand::Close,
            AppCommand::Save,
            AppCommand::SaveAs,
            AppCommand::ImportAudio,
            AppCommand::ImportMidi,
            AppCommand::ImportMidiSong,
            AppCommand::RelinkAudio,
            AppCommand::ExportPatch,
            AppCommand::ExportMidiClip,
            AppCommand::ExportMidiTrack,
            AppCommand::ExportMidiSong,
            AppCommand::ExportSongAudio,
            AppCommand::ExportLoopAudio,
            AppCommand::StressRack,
            AppCommand::StressSong,
            AppCommand::Quit,
            AppCommand::Undo,
            AppCommand::Redo,
            AppCommand::Cut,
            AppCommand::Copy,
            AppCommand::Paste,
            AppCommand::Duplicate,
            AppCommand::Delete,
            AppCommand::SelectAll,
            AppCommand::Combine,
            AppCommand::Uncombine,
            AppCommand::QuantizeApply,
            AppCommand::QuantizeOptions,
            AppCommand::ToggleFlip,
            AppCommand::ZoomIn,
            AppCommand::ZoomOut,
            AppCommand::ZoomReset,
            AppCommand::ToggleBrowser,
            AppCommand::ToggleFileBrowser,
            AppCommand::TogglePianoKeys,
            AppCommand::ToggleSong,
            AppCommand::TogglePianoRoll,
            AppCommand::ToggleSampleEditor,
            AppCommand::ToggleStepEditor,
            AppCommand::ToggleSliceEditor,
            AppCommand::ToggleConsoleDevices,
            AppCommand::ToggleConsoleLinks,
            AppCommand::ToggleTempoLane,
            AppCommand::ToggleMarkerStrip,
            AppCommand::ToggleInspector,
            AppCommand::ToggleMarkerList,
            AppCommand::FindParameter,
            AppCommand::FollowSelection,
            AppCommand::ShowParamLane,
            AppCommand::AddMarker,
            AppCommand::AddRangeFromLoop,
            AppCommand::SetEndMarker,
            AppCommand::SetLeftLocator,
            AppCommand::SetRightLocator,
            AppCommand::PreviousMarker,
            AppCommand::NextMarker,
            AppCommand::ToggleConsole,
            AppCommand::MaximiseConsole,
            AppCommand::AddChannel,
            AppCommand::RouteToNewBus,
            AppCommand::CreateVca,
            AppCommand::AddToVca(None),
            AppCommand::RemoveFromVca,
            AppCommand::DeleteVca(None),
            AppCommand::TogglePolarity,
            AppCommand::BypassAllInserts,
            AppCommand::StoreSnapshot,
            AppCommand::UpdateSnapshot(None),
            AppCommand::RecallSnapshot(None),
            AppCommand::RecallNextSnapshot,
            AppCommand::RecallPreviousSnapshot,
            AppCommand::CompareSnapshot(None),
            AppCommand::EndCompare,
            AppCommand::DeleteSnapshot(None),
            AppCommand::ToggleCompensation,
            AppCommand::ToggleReturnAlignment,
            AppCommand::MidiLearn,
            AppCommand::MidiMappings,
            AppCommand::ExportMidiPreset,
            AppCommand::ImportMidiPreset,
            AppCommand::ClearMidiMaps,
            AppCommand::MacroPanel,
            AppCommand::NewMacroBank,
            AppCommand::CreateInstrument,
            AppCommand::CreateEffect,
            AppCommand::CreatePlayer,
            AppCommand::CreateAudioTrack,
            AppCommand::CreateMixerChannel,
            AppCommand::ToggleRack,
            AppCommand::ToggleRackNavigator,
            AppCommand::SequencerOnly,
            AppCommand::DetachRack,
            AppCommand::DetachConsole,
            AppCommand::ToggleEditor,
            AppCommand::GroupTracks,
            AppCommand::UngroupTracks,
            AppCommand::ToggleFolder,
            AppCommand::RenameFolder,
            AppCommand::MoveTrackUp,
            AppCommand::MoveTrackDown,
            AppCommand::RenameTrack,
            AppCommand::TrackComment,
            AppCommand::AddNoteLane,
            AppCommand::RenameNoteLane,
            AppCommand::DeleteNoteLane,
            AppCommand::ToggleLaneMute,
            AppCommand::FreezeTrack,
            AppCommand::FreezeTrackToDevice,
            AppCommand::FreezeTail,
            AppCommand::FreezeCancel,
            AppCommand::UnfreezeTrack,
            AppCommand::UnfreezeTrackKeep,
            AppCommand::BounceInPlace,
            AppCommand::BounceInPlaceMute,
            AppCommand::FoldLanes(true),
            AppCommand::FoldLanes(false),
            AppCommand::AudioReverse,
            AppCommand::InsertBars,
            AppCommand::RemoveBars,
            AppCommand::ReverseClips(true),
            AppCommand::ReverseClips(false),
            AppCommand::AudioNormalise,
            AppCommand::AudioStripSilence,
            AppCommand::AudioQuantize,
            AppCommand::AudioWarp,
            AppCommand::AudioWarpTempo,
            AppCommand::AudioWarpPanel,
            AppCommand::AudioToNotes,
            AppCommand::AudioFades(true),
            AppCommand::AudioFades(false),
            AppCommand::AudioClearEnvelope,
            AppCommand::AudioSettings,
            AppCommand::PluginSettings,
            AppCommand::RescanMidi,
            AppCommand::RestartAudio,
            AppCommand::ExportDiagnostics,
            AppCommand::KeyboardShortcuts,
            AppCommand::Documentation,
            AppCommand::ReportIssue,
            AppCommand::About,
            AppCommand::AppZoom(ZoomChange::In),
            AppCommand::AppZoom(ZoomChange::Out),
            AppCommand::AppZoom(ZoomChange::Reset),
            AppCommand::FocusWindow(WindowKind::Main),
            AppCommand::FocusWindow(WindowKind::Rack),
            AppCommand::FocusWindow(WindowKind::Console),
        ];
        for t in ThemeChoice::ALL {
            v.push(AppCommand::SetTheme(t));
        }
        for p in crate::keymap_store::ZOOM_STEPS {
            v.push(AppCommand::AppZoom(ZoomChange::To(p)));
        }
        for rate in [44_100, 48_000, 96_000] {
            v.push(AppCommand::NewAtRate(rate));
            v.push(AppCommand::ConvertRate(rate));
        }
        for n in [0, 1, 2, 4, 8, 16, 32] {
            v.push(AppCommand::InputChannels(n));
        }
        for n in [1, 2, 4, 8, 16, 32] {
            v.push(AppCommand::OutputChannels(n));
        }
        for step in [GainStep::Up, GainStep::Down, GainStep::Reset] {
            v.push(AppCommand::AudioGain(step));
        }
        for mode in WarpMode::ALL {
            v.push(AppCommand::AudioWarpMode(mode));
        }
        for bars in [1, 2, 4, 8, 16] {
            v.push(AppCommand::AudioWarpBars(bars));
        }
        for step in ScaleStep::ALL {
            v.push(AppCommand::ScaleTempo(step));
        }
        for step in ChopStep::ALL {
            v.push(AppCommand::ChopClips(step));
        }
        for step in [
            TransposeStep::Up,
            TransposeStep::Down,
            TransposeStep::OctaveUp,
            TransposeStep::OctaveDown,
            TransposeStep::Reset,
        ] {
            v.push(AppCommand::AudioTranspose(step));
        }
        for curve in FadeCurve::ALL {
            v.push(AppCommand::AudioFadeShape(curve));
            v.push(AppCommand::AudioCrossfade(Some(curve)));
        }
        v.push(AppCommand::AudioCrossfade(None));
        for p in [
            HeightPreset::Small,
            HeightPreset::Medium,
            HeightPreset::Large,
            HeightPreset::Fit,
        ] {
            v.push(AppCommand::TrackHeight(p));
        }
        for c in [
            CableVisibility::Show,
            CableVisibility::HoverOnly,
            CableVisibility::SelectedOnly,
            CableVisibility::HideAutoRouted,
            CableVisibility::Hide,
        ] {
            v.push(AppCommand::Cables(c));
        }
        for t in [
            TransportAction::Play,
            TransportAction::Stop,
            TransportAction::TogglePlay,
            TransportAction::ReturnToZero,
            TransportAction::TapTempo,
            TransportAction::ToggleLoop,
            TransportAction::ToggleRecord,
            TransportAction::ToggleMetronome,
            TransportAction::ToggleAutoLanes,
            TransportAction::ClearOverride,
        ] {
            v.push(AppCommand::Transport(t));
        }
        v.push(AppCommand::TogglePreCount);
        v.push(AppCommand::ToggleClockOut);
        v.push(AppCommand::ToggleClockWhileStopped);
        v.push(AppCommand::ToggleClockIn);
        v.push(AppCommand::ToggleChase);
        for q in [
            InputQuantize::OFF,
            InputQuantize::EIGHTH,
            InputQuantize::SIXTEENTH,
        ] {
            v.push(AppCommand::Quantize(q));
        }
        v
    }

    #[test]
    fn command_ids_are_a_bijection_onto_registry_commands() {
        let registry = Registry::bundled();
        let mut seen = Vec::new();
        for c in all_commands() {
            let (id, arg) = c.command_id().expect("every command has an id");
            assert!(registry.validate(id, arg), "{c:?} -> {id} {arg:?}");
            assert_eq!(AppCommand::from_command(id, arg), Some(c), "{id}");
            assert!(!seen.contains(&(id, arg)), "{id} {arg:?} is used twice");
            seen.push((id, arg));
        }
        assert_eq!(
            AppCommand::Transport(TransportAction::Locate(4)).command_id(),
            None,
            "a locate is a pointer action, not a command"
        );
        assert_eq!(AppCommand::from_command("create.nothing", None), None);
        assert_eq!(AppCommand::from_command("view.cables", None), None);
        assert_eq!(
            AppCommand::from_command("view.cables", Some("sideways")),
            None
        );
    }

    #[test]
    fn every_menu_item_maps_to_a_menu_command_of_the_registry() {
        let registry = Registry::bundled();
        let keymap = legacy(Os::Windows);
        for menu in menus(&state(), &keymap) {
            for item in menu.items() {
                let (id, _) = item.command.command_id().expect("menu items have ids");
                let def = registry.get(id).expect("registered");
                assert!(def.menu, "{id} is in a menu so the registry says menu");
                assert!(!def.label.is_empty());
            }
        }
    }

    #[test]
    fn menu_commands_in_the_registry_all_have_a_menu_item() {
        let keymap = legacy(Os::Windows);
        let recent: Vec<String> = (0..8).map(|i| format!("song {i}")).collect();
        let model = menus_with_recent(&state(), &keymap, &recent);
        let in_menu: Vec<AppCommand> = model
            .iter()
            .flat_map(Menu::items)
            .map(|i| i.command)
            .collect();
        for def in Registry::bundled().commands().iter().filter(|d| d.menu) {
            let any = all_commands()
                .into_iter()
                .filter(|c| c.command_id().is_some_and(|(id, _)| id == def.id))
                .any(|c| in_menu.contains(&c));
            assert!(any, "{} is marked menu but has no item", def.id);
        }
    }

    #[test]
    fn registry_detach_commands_have_menu_items() {
        let keymap = Keymap::bundled(Profile::Standard, Os::Windows);
        let items: Vec<MenuItem> = menus(&state(), &keymap)
            .iter()
            .flat_map(|m| m.items().cloned().collect::<Vec<_>>())
            .collect();
        let registry = Registry::bundled();
        for (id, label, command) in [
            ("view.detach_rack", "Detach Rack", AppCommand::DetachRack),
            (
                "view.detach_console",
                "Detach Mixer",
                AppCommand::DetachConsole,
            ),
        ] {
            assert!(registry.get(id).is_some_and(|d| d.menu), "{id}");
            assert_eq!(command.command_id(), Some((id, None)));
            assert_eq!(AppCommand::from_command(id, None), Some(command));
            let item = items.iter().find(|i| i.command == command).unwrap();
            assert_eq!(item.label, label);
            assert_eq!(
                item.checked,
                Some(false),
                "a check item: native menus cannot relabel"
            );
        }
    }

    /// The `shortcut()` function this file had before the configurable keymap, kept here as the oracle of
    /// the legacy profile: Ctrl or Cmd was the command modifier.
    fn old_shortcut(key: &atomik_ui::Key, modifiers: atomik_ui::Modifiers) -> Option<AppCommand> {
        use atomik_ui::Key;
        if modifiers.alt {
            return None;
        }
        if *key == Key::F(5) && !(modifiers.ctrl || modifiers.meta) {
            return Some(if modifiers.shift {
                AppCommand::MaximiseConsole
            } else {
                AppCommand::ToggleConsole
            });
        }
        let command = modifiers.ctrl || modifiers.meta;
        if *key == Key::Space && command && !modifiers.shift {
            return Some(AppCommand::Transport(TransportAction::ToggleRecord));
        }
        if !command {
            return (*key == Key::Tab && !modifiers.shift).then_some(AppCommand::ToggleFlip);
        }
        let Key::Char(c) = key else {
            return None;
        };
        let shift = modifiers.shift;
        Some(match (c.to_ascii_lowercase(), shift) {
            ('n', false) => AppCommand::New,
            ('o', false) => AppCommand::Open,
            ('s', false) => AppCommand::Save,
            ('s', true) => AppCommand::SaveAs,
            ('q', false) => AppCommand::Quit,
            ('i', false) => AppCommand::ImportAudio,
            ('z', false) => AppCommand::Undo,
            ('z', true) | ('y', _) => AppCommand::Redo,
            ('=' | '+', _) => AppCommand::ZoomIn,
            ('-', false) => AppCommand::ZoomOut,
            ('0', false) => AppCommand::ZoomReset,
            ('b', false) => AppCommand::ToggleBrowser,
            (',', false) => AppCommand::AudioSettings,
            _ => return None,
        })
    }

    /// The legacy profile in the `Global` context, through the router's own resolution.
    fn legacy_shortcut(
        keymap: &Keymap,
        key: &atomik_ui::Key,
        modifiers: atomik_ui::Modifiers,
    ) -> Option<AppCommand> {
        use atomik_ui::keymap::{ContextStack, Resolution};
        match keymap.resolve_event(key, modifiers, &ContextStack::global()) {
            // The file browser, the piano keys and the shortcut list (F1) are new: they are not
            // part of the old function being compared.
            Resolution::Bound(list) => list
                .iter()
                .find_map(|r| AppCommand::from_command(&r.command, r.arg.as_deref()))
                .filter(|c| {
                    !matches!(
                        c,
                        AppCommand::ToggleFileBrowser | AppCommand::KeyboardShortcuts
                    )
                }),
            _ => None,
        }
    }

    #[test]
    fn the_legacy_profile_reproduces_the_old_shortcut_function_exactly() {
        use atomik_ui::{Key, Modifiers};
        let mut keys: Vec<Key> =
            "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789=+-_,.;/[]"
                .chars()
                .map(Key::Char)
                .collect();
        keys.extend([
            Key::Space,
            Key::Enter,
            Key::Tab,
            Key::Escape,
            Key::Backspace,
            Key::Delete,
            Key::Up,
            Key::Down,
            Key::Left,
            Key::Right,
            Key::Home,
            Key::End,
            Key::PageUp,
            Key::PageDown,
            Key::F(1),
            Key::F(4),
            Key::F(5),
            Key::F(6),
            Key::F(12),
            Key::Other,
        ]);
        // On Windows and Linux the command modifier is Control; on macOS it is Command. (The
        // old function accepted either everywhere; the new keyboard matching is strict
        // (d), so each OS is compared with its own primary modifier.)
        for os in Os::ALL {
            let keymap = legacy(os);
            for shift in [false, true] {
                for alt in [false, true] {
                    for primary in [false, true] {
                        let mods = Modifiers {
                            shift,
                            alt,
                            ctrl: primary && os != Os::Mac,
                            meta: primary && os == Os::Mac,
                        };
                        for key in &keys {
                            assert_eq!(
                                legacy_shortcut(&keymap, key, mods),
                                old_shortcut(key, mods),
                                "{os:?} {key:?} {mods:?}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn menu_shortcut_text_is_derived_from_the_keymap_and_the_os() {
        let find = |model: &[Menu], label: &str| {
            model
                .iter()
                .flat_map(Menu::items)
                .find(|i| i.label == label)
                .unwrap()
                .shortcut
                .clone()
        };
        let win = menus(&state(), &legacy(Os::Windows));
        let mac = menus(&state(), &legacy(Os::Mac));
        assert_eq!(
            find(&win, "Save As\u{2026}").as_deref(),
            Some("Ctrl+Shift+S")
        );
        assert_eq!(
            find(&mac, "Save As\u{2026}").as_deref(),
            Some("Cmd+Shift+S")
        );
        assert_eq!(
            find(&win, "Import Audio\u{2026}").as_deref(),
            Some("Ctrl+I")
        );
        assert_eq!(find(&win, "Record").as_deref(), Some("Ctrl+Space"));
        assert_eq!(find(&win, "Redo").as_deref(), Some("Ctrl+Y"));
        assert_eq!(find(&win, "Zoom In").as_deref(), Some("Ctrl+="));
        assert_eq!(find(&win, "Return to Zero").as_deref(), Some("Enter"));
        assert_eq!(find(&win, "Back of Rack").as_deref(), Some("Tab"));
        assert_eq!(find(&win, "Maximise Console").as_deref(), Some("Shift+F5"));
        assert_eq!(find(&win, "Cables: Hide"), None);
        // The standard profile shows its own keys, so a remap changes the menu.
        let reference = Keymap::bundled(Profile::Standard, Os::Mac);
        let model = menus(&state(), &reference);
        assert_eq!(
            find(&model, "Import Audio\u{2026}").as_deref(),
            Some("Cmd+Opt+I")
        );
        assert_eq!(find(&model, "Record").as_deref(), Some("Cmd+Enter"));
        assert_eq!(
            find(&model, "Create Instrument\u{2026}").as_deref(),
            Some("Cmd+I")
        );
        assert_eq!(find(&model, "Rack").as_deref(), Some("F6"));
        assert_eq!(find(&model, "Device Browser").as_deref(), Some("F3"));
    }

    #[test]
    fn no_shortcut_text_is_a_string_literal_in_this_file() {
        let source = include_str!("commands.rs");
        let production = source.split("#[cfg(test)]").next().unwrap();
        for needle in [
            "\"Ctrl+",
            "\"Cmd+",
            "\"Shift+F",
            "Some(\"F5",
            "Some(\"Tab\"",
        ] {
            assert!(
                !production.contains(needle) || needle == "\"Cmd+",
                "{needle} found: shortcuts come from the keymap"
            );
        }
        assert_eq!(
            production.matches("\"Cmd+\"").count(),
            1,
            "the only literal is the prefix that native_accelerator_text strips"
        );
    }

    #[test]
    fn native_accelerators_follow_the_mac_shortcut_text() {
        assert_eq!(
            native_accelerator_text("Cmd+Shift+S").as_deref(),
            Some("CmdOrCtrl+Shift+S")
        );
        assert_eq!(
            native_accelerator_text("Cmd+Opt+I").as_deref(),
            Some("CmdOrCtrl+Alt+I")
        );
        assert_eq!(native_accelerator_text("Tab"), None);
        assert_eq!(native_accelerator_text("F5"), None);
        assert_eq!(native_accelerator_text("Ctrl+Space"), None);
    }

    #[test]
    fn the_menu_bar_has_its_menus_with_their_items() {
        let m = menus(&state(), &legacy(Os::Windows));
        let titles: Vec<_> = m.iter().map(|m| m.title).collect();
        assert_eq!(
            titles,
            [
                "File",
                "Edit",
                "View",
                "Transport",
                "Mixer",
                "Create",
                "Track",
                "Audio",
                "Options",
                "Window",
                "Help"
            ]
        );
        let labels = |i: usize| m[i].items().map(|it| it.label.as_ref()).collect::<Vec<_>>();
        assert_eq!(
            labels(0),
            [
                "New",
                "Open\u{2026}",
                "Clear Menu",
                "Close",
                "Save",
                "Save As\u{2026}",
                // New at Sample Rate, then Convert Sample Rate: three rates each.
                "44.1 kHz",
                "48 kHz",
                "96 kHz",
                "44.1 kHz",
                "48 kHz",
                "96 kHz",
                "Import Audio\u{2026}",
                "Import MIDI\u{2026}",
                "Import MIDI as Song\u{2026}",
                "Relink Missing Audio\u{2026}",
                "Export Patch\u{2026}",
                "Export MIDI Clip\u{2026}",
                "Export MIDI Track\u{2026}",
                "Export MIDI Song\u{2026}",
                "Export Song as Audio File\u{2026}",
                "Export Loop as Audio File\u{2026}",
                "Stress Rack",
                "Stress Song",
                "Quit"
            ]
        );
        assert_eq!(
            labels(1),
            [
                "Undo",
                "Redo",
                "Cut",
                "Copy",
                "Paste",
                "Duplicate",
                "Delete",
                "Select All",
                "Combine",
                "Uncombine",
                "Quantize",
                "Quantize Options\u{2026}",
                "Reverse Audio",
                "Normalise Audio",
                "Strip Silence\u{2026}",
                "Quantize Audio",
                "Insert Bars at Locators",
                "Remove Bars at Locators",
                "Reverse Clips",
                "Reverse Note Starts",
                "Double (200 %)",
                "Halve (50 %)",
                "150 %",
                "125 %",
                "75 %",
                "Bar",
                "Half Bar",
                "Beat",
                "1/8 Note",
                "1/16 Note"
            ]
        );
        assert_eq!(
            labels(2),
            [
                "Back of Rack",
                "Cables: Show",
                "Cables: Hover",
                "Cables: Selected",
                "Cables: Hide Auto-routed",
                "Cables: Hide",
                "Zoom In",
                "Zoom Out",
                "Actual Size",
                // Application Zoom, then Theme.
                "75 %",
                "100 %",
                "125 %",
                "150 %",
                "175 %",
                "200 %",
                "Dark",
                "Light",
                "Same as System",
                "Device Browser",
                "File Browser",
                "Piano Keys",
                "Rack",
                "Detach Rack",
                "Detach Mixer",
                "Rack Navigator",
                "Sequencer Only",
                "Clip Editor",
                "Arrangement",
                "Piano Roll",
                "Sample Editor",
                "Step Editor",
                "Slice Editor",
                "Show Console Devices in Rack",
                "Show Console Links",
                "Tempo Lane",
                "Marker Strip",
                "Marker List",
                "Inspector",
                "Find Parameter\u{2026}",
                "Automation Follows Selection",
                "Show Automation Lane",
            ]
        );
        assert_eq!(
            labels(4),
            [
                "Console",
                "Maximise Console",
                "Add Channel\u{2026}",
                "Route to New Output Bus",
                "Create VCA from Selection",
                "Add to VCA",
                "Remove from VCA",
                "Delete VCA",
                "Toggle Polarity",
                "Bypass All Inserts",
                "Store Snapshot",
                "Update Snapshot",
                "Recall Snapshot",
                "Recall Next Snapshot",
                "Recall Previous Snapshot",
                "Compare A/B",
                "End Compare",
                "Delete Snapshot",
                "Delay Compensation",
                "Align Returns",
                "MIDI Learn",
                "MIDI Mappings\u{2026}",
                "Export MIDI Mapping Preset\u{2026}",
                "Import MIDI Mapping Preset\u{2026}",
                "Clear MIDI Mappings",
                "Macros\u{2026}",
                "New Macro Bank"
            ]
        );
        assert_eq!(
            labels(5),
            [
                "Create Instrument\u{2026}",
                "Create Effect\u{2026}",
                "Create Player\u{2026}",
                "Create Audio Track",
                "Create Mixer Channel\u{2026}"
            ]
        );
        assert_eq!(
            labels(6),
            [
                "Group Tracks",
                "Ungroup",
                "Collapse Folder",
                "Rename Folder\u{2026}",
                "Move Track Up",
                "Move Track Down",
                "Rename Track\u{2026}",
                "Track Comment\u{2026}",
                "Add Note Lane",
                "Rename Note Lane\u{2026}",
                "Delete Note Lane",
                "Mute Lane",
                "Freeze Track",
                "Freeze Track up to Selected Device",
                "Freeze with Tail",
                "Unfreeze Track",
                "Unfreeze Track and Keep Audio",
                "Cancel Freeze",
                "Bounce in Place",
                "Bounce in Place and Mute Originals",
                "Fold Automation Lanes",
                "Unfold Automation Lanes",
                "Track Height: Small",
                "Track Height: Medium",
                "Track Height: Large",
                "Track Height: Fit"
            ]
        );
        assert_eq!(
            labels(7),
            [
                "Settings\u{2026}",
                "Plugins\u{2026}",
                "Restart Audio",
                "Rescan MIDI Ports",
                // Input Channels (0 is off), then Output Channels.
                "0 (off)",
                "1",
                "2",
                "4",
                "8",
                "16",
                "32",
                "1",
                "2",
                "4",
                "8",
                "16",
                "32",
                "Clip Gain: Up 1 dB",
                "Clip Gain: Down 1 dB",
                "Clip Gain: Reset",
                "Fades: 5 ms Declick",
                "Fades: None",
                "Clear Gain Envelope",
                "Fade Shape: Linear",
                "Fade Shape: Equal Power",
                "Fade Shape: Exponential",
                "Fade Shape: S-curve",
                "Crossfade: Off",
                "Crossfade: Linear",
                "Crossfade: Equal Power",
                "Crossfade: Exponential",
                "Crossfade: S-curve",
                "Warp Audio",
                "Warp to Song Tempo",
                "Warp Settings\u{2026}",
                "Warp Mode: Beats",
                "Warp Mode: Tonal",
                "Warp Mode: Repitch",
                "Warp Loop as 1 Bar",
                "Warp Loop as 2 Bars",
                "Warp Loop as 4 Bars",
                "Warp Loop as 8 Bars",
                "Transpose: Up a Semitone",
                "Transpose: Down a Semitone",
                "Transpose: Up an Octave",
                "Transpose: Down an Octave",
                "Transpose: Reset",
                "Audio to Notes\u{2026}"
            ]
        );
        assert_eq!(
            labels(8),
            [
                "Click",
                "Pre-count",
                "Input Quantize: Off",
                "Input Quantize: 1/8",
                "Input Quantize: 1/16",
                "Chase Notes",
                "Delay Compensation",
                "Align Returns",
                "MIDI Clock Out",
                "Send Clock While Stopped",
                "Follow MIDI Clock",
                "Settings\u{2026}"
            ]
        );
        assert_eq!(
            labels(9),
            ["Main Window", "Rack Window", "Mixer Window"],
            "the Window menu lists the windows"
        );
        assert_eq!(
            labels(10),
            [
                "Keyboard Shortcuts",
                "Documentation",
                "Report an Issue",
                "Export Diagnostics\u{2026}",
                "About Atomik Rack"
            ]
        );
    }

    #[test]
    fn the_window_menu_enables_the_detached_windows_only() {
        let mut on = state();
        let enabled = |s: &MenuState| {
            let m = menus(s, &legacy(Os::Windows));
            let window = m.iter().find(|m| m.title == "Window").unwrap();
            window.items().map(|i| i.enabled).collect::<Vec<_>>()
        };
        assert_eq!(enabled(&on), [true, false, false]);
        on.rack_detached = true;
        on.console_detached = true;
        assert_eq!(enabled(&on), [true, true, true]);
    }

    #[test]
    fn the_edit_menu_names_the_steps_undo_and_redo_run() {
        let mut s = state();
        s.can_undo = true;
        s.can_redo = true;
        s.undo_label = Some("Move Device");
        s.redo_label = Some("Add Clip");
        let m = menus(&s, &legacy(Os::Windows));
        let edit = m.iter().find(|m| m.title == "Edit").unwrap();
        let labels: Vec<&str> = edit.items().take(2).map(|i| i.label.as_ref()).collect();
        assert_eq!(labels, ["Undo Move Device", "Redo Add Clip"]);
        // Disabled, or without a name, the plain verbs stay.
        s.can_undo = false;
        s.redo_label = None;
        let m = menus(&s, &legacy(Os::Windows));
        let edit = m.iter().find(|m| m.title == "Edit").unwrap();
        let labels: Vec<&str> = edit.items().take(2).map(|i| i.label.as_ref()).collect();
        assert_eq!(labels, ["Undo", "Redo"]);
        assert_eq!(edit.items().next().unwrap().command, AppCommand::Undo);
    }

    #[test]
    fn the_view_menu_checks_the_theme_and_the_zoom_in_use() {
        let mut s = state();
        s.theme = ThemeChoice::Light;
        s.app_zoom = 150;
        let m = menus(&s, &legacy(Os::Windows));
        let view = m.iter().find(|m| m.title == "View").unwrap();
        let checked = |label: &str| {
            view.items()
                .find(|i| i.label.as_ref() == label)
                .unwrap_or_else(|| panic!("no {label}"))
                .checked
        };
        assert_eq!(checked("Light"), Some(true));
        assert_eq!(checked("Dark"), Some(false));
        assert_eq!(checked("150 %"), Some(true));
        assert_eq!(checked("100 %"), Some(false));
    }

    #[test]
    fn the_new_commands_have_ids_and_a_standard_key_for_the_shortcut_list() {
        assert_eq!(
            AppCommand::SetTheme(ThemeChoice::System).command_id(),
            Some(("view.theme", Some("system")))
        );
        assert_eq!(
            AppCommand::from_command("view.app_zoom", Some("in")),
            Some(AppCommand::AppZoom(ZoomChange::In))
        );
        assert_eq!(
            AppCommand::from_command("view.app_zoom", Some("175")),
            Some(AppCommand::AppZoom(ZoomChange::To(175)))
        );
        assert_eq!(AppCommand::from_command("view.app_zoom", Some("90")), None);
        assert_eq!(
            AppCommand::from_command("view.focus_window", Some("mixer")),
            Some(AppCommand::FocusWindow(WindowKind::Console))
        );
        // F1 is the shortcut list in both profiles, as every OS shows it.
        for profile in [Profile::Standard, Profile::Legacy] {
            for os in Os::ALL {
                let keymap = Keymap::bundled(profile, os);
                assert_eq!(
                    AppCommand::KeyboardShortcuts
                        .shortcut_text(&keymap)
                        .as_deref(),
                    Some("F1"),
                    "{profile:?} {os:?}"
                );
            }
        }
    }

    #[test]
    fn the_zoom_steps_walk_the_list_and_stop_at_its_ends() {
        let step = |c: ZoomChange, from: u16| c.apply(from);
        assert_eq!(step(ZoomChange::In, 100), 125);
        assert_eq!(step(ZoomChange::In, 200), 200);
        assert_eq!(step(ZoomChange::Out, 100), 75);
        assert_eq!(step(ZoomChange::Out, 75), 75);
        assert_eq!(step(ZoomChange::Reset, 175), 100);
        assert_eq!(step(ZoomChange::To(150), 75), 150);
        assert_eq!(step(ZoomChange::To(130), 75), 125, "the nearest step");
    }

    #[test]
    fn the_options_menu_reuses_the_commands_of_their_own_menus() {
        let m = menus(&state(), &legacy(Os::Windows));
        let options = &m[8];
        assert_eq!(options.title, "Options");
        let command = |label: &str| {
            options
                .items()
                .find(|i| i.label.as_ref() == label)
                .unwrap_or_else(|| panic!("no {label}"))
                .command
        };
        assert_eq!(
            command("Click"),
            AppCommand::Transport(TransportAction::ToggleMetronome)
        );
        assert_eq!(command("Pre-count"), AppCommand::TogglePreCount);
        assert_eq!(command("Chase Notes"), AppCommand::ToggleChase);
        assert_eq!(
            command("Delay Compensation"),
            AppCommand::ToggleCompensation
        );
        assert_eq!(command("Settings\u{2026}"), AppCommand::AudioSettings);
        // Pre-count has a key in the standard profile and shows it.
        let standard = Keymap::bundled(Profile::Standard, Os::Windows);
        let m = menus(&state(), &standard);
        let pre = m[8]
            .items()
            .find(|i| i.label.as_ref() == "Pre-count")
            .unwrap();
        assert_eq!(pre.shortcut.as_deref(), Some("Ctrl+P"));
        let on = MenuState {
            pre_count: true,
            clock_out: false,
            clock_while_stopped: false,
            follow_clock: false,
            sample_rate: 48_000,
            input_channels: 2,
            output_channels: 2,
            ..state()
        };
        let m = menus(&on, &standard);
        let pre = m[8]
            .items()
            .find(|i| i.label.as_ref() == "Pre-count")
            .unwrap();
        assert_eq!(pre.checked, Some(true));
    }

    #[test]
    fn checks_and_enabled_states_follow_the_state() {
        let keymap = legacy(Os::Windows);
        let m = menus(
            &MenuState {
                flip: Flip::Back,
                cables: CableVisibility::HoverOnly,
                browser: true,
                file_browser: false,
                piano_keys: false,
                can_undo: false,
                can_redo: true,
                undo_label: None,
                redo_label: None,
                can_edit: true,
                can_paste: false,
                can_combine: false,
                can_uncombine: false,
                song_visible: false,
                piano_roll: true,
                sample_editor: false,
                step_editor: false,
                slice_editor: false,
                playing: true,
                recording: false,
                loop_enabled: true,
                metronome: false,
                auto_lanes: false,
                pre_count: false,
                clock_out: false,
                clock_while_stopped: false,
                follow_clock: false,
                sample_rate: 48_000,
                input_channels: 2,
                output_channels: 2,
                quantize: InputQuantize::EIGHTH,
                console: true,
                console_maximised: false,
                compensation: false,
                align_returns: false,
                console_devices: true,
                console_links: false,
                tempo_lane: false,
                marker_strip: false,
                inspector: false,
                marker_list: false,
                rack_navigator: true,
                rack: true,
                rack_detached: false,
                console_detached: false,
                sequencer_only: false,
                editor: false,
                missing_audio: true,
                midi_clip: true,
                midi_track: false,
                midi_song: true,
                export_song: true,
                export_loop: true,
                org: OrgMenu::default(),
                lane: LaneMenu::default(),
                audio: AudioMenu::default(),
                vca: VcaMenu::default(),
                strip: StripMenu::default(),
                snapshots: SnapshotMenu::default(),
                midi: MidiMenu::default(),
                freeze: FreezeMenu {
                    tail: true,
                    ..FreezeMenu::default()
                },
                follow_selection: false,
                theme: ThemeChoice::Dark,
                app_zoom: 100,
            },
            &keymap,
        );
        let find = |label: &str| {
            m.iter()
                .flat_map(|m| m.items())
                .find(|i| i.label == label)
                .unwrap()
                .clone()
        };
        assert_eq!(find("Back of Rack").checked, Some(true));
        assert_eq!(find("Cables: Show").checked, Some(false));
        assert_eq!(find("Cables: Hover").checked, Some(true));
        assert_eq!(find("Cables: Hide").checked, Some(false));
        assert_eq!(find("Device Browser").checked, Some(true));
        assert_eq!(find("Arrangement").checked, Some(false));
        assert_eq!(find("Piano Roll").checked, Some(true));
        assert_eq!(find("Sample Editor").checked, Some(false));
        assert_eq!(find("Play / Stop").checked, Some(true));
        assert_eq!(find("Loop").checked, Some(true));
        assert_eq!(find("Input Quantize: 1/8").checked, Some(true));
        assert_eq!(find("Input Quantize: Off").checked, Some(false));
        assert!(!find("Undo").enabled);
        assert!(find("Redo").enabled);
        assert_eq!(find("Save").checked, None);
        assert_eq!(find("Console").checked, Some(true));
        assert_eq!(find("Maximise Console").checked, Some(false));
        assert_eq!(find("Delay Compensation").checked, Some(false));
        assert_eq!(find("Show Console Devices in Rack").checked, Some(true));
        let front = menus(&state(), &keymap);
        let back = front[2].items().next().unwrap();
        assert_eq!(back.checked, Some(false));
        assert_eq!(back.shortcut.as_deref(), Some("Tab"));
    }

    #[test]
    fn a_menu_item_with_an_argument_shows_only_its_own_binding() {
        use atomik_ui::keymap::UserKeymap;
        let (user, _) = UserKeymap::from_json(
            r#"{"bindings":[{"command":"view.cables","arg":"hide","context":"global","chords":["Cmd+Alt+H"]}]}"#,
        )
        .unwrap();
        let built = user.build(Profile::Legacy, Os::Windows);
        assert_eq!(
            AppCommand::Cables(CableVisibility::Hide)
                .shortcut_text(&built.keymap)
                .as_deref(),
            Some("Ctrl+Alt+H")
        );
        assert_eq!(
            AppCommand::Cables(CableVisibility::Show).shortcut_text(&built.keymap),
            None
        );
    }
}
