//! App commands and the menu model shared by the in-window menu bar and the native (macOS)
//! menus.
//!
//! Keys are not decided here: every [`AppCommand`] maps to a command id of the registry
//! ([`AppCommand::command_id`]) and the shortcut text of a menu item is derived from the active
//! keymap ([`menus`]); the key router lives in `atomik_ui::keymap` and the shell.

use crate::record::InputQuantize;
use atomik_model::{FadeCurve, WarpMode};
use atomik_ui::arrangement::{AudioMenu, HeightPreset, OrgMenu};
use atomik_ui::keymap::Keymap;
use atomik_ui::{CableVisibility, Flip, TransportAction};

/// Something the user asked the app to do (menu item or shortcut).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppCommand {
    /// File ▸ New.
    New,
    /// File ▸ Open….
    Open,
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
    /// View > Marker List: the list of markers and ranges docked at the right edge of the
    /// arrangement (saved with the view state).
    ToggleMarkerList,
    /// Transport > Add Marker: a marker at the playhead.
    AddMarker,
    /// Transport > Add Range Marker from Loop.
    AddRangeFromLoop,
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
    /// Mixer > Route to New Output Bus (Ctrl/Cmd+G): a new bus for the selected channels.
    RouteToNewBus,
    /// Create ▸ Create Instrument (Ctrl+I): the device browser in create mode, instruments only.
    CreateInstrument,
    /// Create ▸ Create Effect (Ctrl+E): the device browser in create mode, effects only.
    CreateEffect,
    /// Create ▸ Create Audio Track (Ctrl+T).
    CreateAudioTrack,
    /// Create ▸ Create Mixer Channel (Ctrl+Shift+M): the console's add-channel picker.
    CreateMixerChannel,
    /// View ▸ Rack (F6): show or hide the rack (a layout flag in `workflow.json`).
    ToggleRack,
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
    /// Track ▸ Track Comment…: type the selected track's comment.
    TrackComment,
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
    /// Audio ▸ Restart Audio.
    RestartAudio,
}

impl AppCommand {
    /// The registry command id and argument of this command, `None` only for
    /// `Transport(Locate(_))`, a pointer action that is not a command.
    pub fn command_id(&self) -> Option<(&'static str, Option<&'static str>)> {
        Some(match self {
            AppCommand::New => ("file.new", None),
            AppCommand::Open => ("file.open", None),
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
            AppCommand::QuantizeApply => ("editor.quantize", None),
            AppCommand::QuantizeOptions => ("edit.quantize_options", None),
            AppCommand::ToggleFlip => ("view.toggle_flip", None),
            AppCommand::Cables(CableVisibility::Show) => ("view.cables", Some("show")),
            AppCommand::Cables(CableVisibility::HoverOnly) => ("view.cables", Some("hover")),
            AppCommand::Cables(CableVisibility::Hide) => ("view.cables", Some("hide")),
            AppCommand::ZoomIn => ("view.zoom_in", None),
            AppCommand::ZoomOut => ("view.zoom_out", None),
            AppCommand::ZoomReset => ("view.zoom_reset", None),
            AppCommand::ToggleBrowser => ("view.toggle_browser", None),
            AppCommand::ToggleFileBrowser => ("view.toggle_file_browser", None),
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
            AppCommand::Transport(TransportAction::Locate(_)) => return None,
            AppCommand::Quantize(InputQuantize::Off) => ("transport.input_quantize", Some("off")),
            AppCommand::Quantize(InputQuantize::Eighth) => {
                ("transport.input_quantize", Some("eighth"))
            }
            AppCommand::Quantize(InputQuantize::Sixteenth) => {
                ("transport.input_quantize", Some("sixteenth"))
            }
            AppCommand::ToggleConsoleDevices => ("view.toggle_console_devices", None),
            AppCommand::ToggleConsoleLinks => ("view.toggle_console_links", None),
            AppCommand::ToggleTempoLane => ("view.toggle_tempo_lane", None),
            AppCommand::ToggleMarkerStrip => ("view.toggle_marker_strip", None),
            AppCommand::ToggleMarkerList => ("view.toggle_marker_list", None),
            AppCommand::AddMarker => ("transport.add_marker", None),
            AppCommand::AddRangeFromLoop => ("transport.add_range_from_loop", None),
            AppCommand::PreviousMarker => ("transport.prev_marker", None),
            AppCommand::NextMarker => ("transport.next_marker", None),
            AppCommand::ToggleConsole => ("view.toggle_console", None),
            AppCommand::MaximiseConsole => ("view.maximise_console", None),
            AppCommand::AddChannel => ("mixer.add_channel", None),
            AppCommand::RouteToNewBus => ("mixer.route_to_new_bus", None),
            AppCommand::ToggleCompensation => ("mixer.toggle_compensation", None),
            AppCommand::CreateInstrument => ("create.instrument", None),
            AppCommand::CreateEffect => ("create.effect", None),
            AppCommand::CreateAudioTrack => ("create.audio_track", None),
            AppCommand::CreateMixerChannel => ("create.mixer_channel", None),
            AppCommand::ToggleRack => ("view.toggle_rack", None),
            AppCommand::ToggleRackNavigator => ("view.toggle_rack_navigator", None),
            AppCommand::SequencerOnly => ("view.sequencer_only", None),
            AppCommand::ToggleEditor => ("view.toggle_editor", None),
            AppCommand::GroupTracks => ("sequencer.group_tracks", None),
            AppCommand::UngroupTracks => ("sequencer.ungroup_tracks", None),
            AppCommand::ToggleFolder => ("sequencer.toggle_folder", None),
            AppCommand::RenameFolder => ("sequencer.rename_folder", None),
            AppCommand::MoveTrackUp => ("sequencer.move_track_up", None),
            AppCommand::MoveTrackDown => ("sequencer.move_track_down", None),
            AppCommand::TrackComment => ("sequencer.track_comment", None),
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
            AppCommand::RestartAudio => ("audio.restart", None),
        })
    }

    /// The command a registry id (and argument) stands for, if the app has one: the inverse
    /// of [`Self::command_id`]. Ids of commands the shell does not run yet give `None`.
    pub fn from_command(id: &str, arg: Option<&str>) -> Option<AppCommand> {
        Some(match (id, arg) {
            ("file.new", None) => AppCommand::New,
            ("file.open", None) => AppCommand::Open,
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
            ("editor.quantize", None) => AppCommand::QuantizeApply,
            ("edit.quantize_options", None) => AppCommand::QuantizeOptions,
            ("view.toggle_flip", None) => AppCommand::ToggleFlip,
            ("view.cables", Some("show")) => AppCommand::Cables(CableVisibility::Show),
            ("view.cables", Some("hover")) => AppCommand::Cables(CableVisibility::HoverOnly),
            ("view.cables", Some("hide")) => AppCommand::Cables(CableVisibility::Hide),
            ("view.zoom_in", None) => AppCommand::ZoomIn,
            ("view.zoom_out", None) => AppCommand::ZoomOut,
            ("view.zoom_reset", None) => AppCommand::ZoomReset,
            ("view.toggle_browser", None) => AppCommand::ToggleBrowser,
            ("view.toggle_file_browser", None) => AppCommand::ToggleFileBrowser,
            ("view.toggle_song", None) => AppCommand::ToggleSong,
            ("view.toggle_piano_roll", None) => AppCommand::TogglePianoRoll,
            ("view.toggle_sample_editor", None) => AppCommand::ToggleSampleEditor,
            ("view.toggle_step_editor", None) => AppCommand::ToggleStepEditor,
            ("view.toggle_slice_editor", None) => AppCommand::ToggleSliceEditor,
            ("transport.play", None) => AppCommand::Transport(TransportAction::Play),
            ("transport.stop", None) => AppCommand::Transport(TransportAction::Stop),
            ("transport.play_stop", None) => AppCommand::Transport(TransportAction::TogglePlay),
            ("transport.go_start", None) => AppCommand::Transport(TransportAction::ReturnToZero),
            ("transport.loop", None) => AppCommand::Transport(TransportAction::ToggleLoop),
            ("transport.record", None) => AppCommand::Transport(TransportAction::ToggleRecord),
            ("transport.metronome", None) => {
                AppCommand::Transport(TransportAction::ToggleMetronome)
            }
            ("transport.input_quantize", Some("off")) => AppCommand::Quantize(InputQuantize::Off),
            ("transport.input_quantize", Some("eighth")) => {
                AppCommand::Quantize(InputQuantize::Eighth)
            }
            ("transport.input_quantize", Some("sixteenth")) => {
                AppCommand::Quantize(InputQuantize::Sixteenth)
            }
            ("view.toggle_console_devices", None) => AppCommand::ToggleConsoleDevices,
            ("view.toggle_console_links", None) => AppCommand::ToggleConsoleLinks,
            ("view.toggle_tempo_lane", None) => AppCommand::ToggleTempoLane,
            ("view.toggle_marker_strip", None) => AppCommand::ToggleMarkerStrip,
            ("view.toggle_marker_list", None) => AppCommand::ToggleMarkerList,
            ("transport.add_marker", None) => AppCommand::AddMarker,
            ("transport.add_range_from_loop", None) => AppCommand::AddRangeFromLoop,
            ("transport.prev_marker", None) => AppCommand::PreviousMarker,
            ("transport.next_marker", None) => AppCommand::NextMarker,
            ("view.toggle_console", None) => AppCommand::ToggleConsole,
            ("view.maximise_console", None) => AppCommand::MaximiseConsole,
            ("mixer.add_channel", None) => AppCommand::AddChannel,
            ("mixer.route_to_new_bus", None) => AppCommand::RouteToNewBus,
            ("mixer.toggle_compensation", None) => AppCommand::ToggleCompensation,
            ("create.instrument", None) => AppCommand::CreateInstrument,
            ("create.effect", None) => AppCommand::CreateEffect,
            ("create.audio_track", None) => AppCommand::CreateAudioTrack,
            ("create.mixer_channel", None) => AppCommand::CreateMixerChannel,
            ("view.toggle_rack", None) => AppCommand::ToggleRack,
            ("view.toggle_rack_navigator", None) => AppCommand::ToggleRackNavigator,
            ("view.sequencer_only", None) => AppCommand::SequencerOnly,
            ("view.toggle_editor", None) => AppCommand::ToggleEditor,
            ("sequencer.group_tracks", None) => AppCommand::GroupTracks,
            ("sequencer.ungroup_tracks", None) => AppCommand::UngroupTracks,
            ("sequencer.toggle_folder", None) => AppCommand::ToggleFolder,
            ("sequencer.rename_folder", None) => AppCommand::RenameFolder,
            ("sequencer.move_track_up", None) => AppCommand::MoveTrackUp,
            ("sequencer.move_track_down", None) => AppCommand::MoveTrackDown,
            ("sequencer.track_comment", None) => AppCommand::TrackComment,
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
            ("audio.restart", None) => AppCommand::RestartAudio,
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
    /// Undo possible.
    pub can_undo: bool,
    /// Redo possible.
    pub can_redo: bool,
    /// The focused pane has a selection Cut, Copy, Duplicate and Delete can work on.
    pub can_edit: bool,
    /// The clipboard holds something the focused pane can paste.
    pub can_paste: bool,
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
    /// Input quantize for recording.
    pub quantize: InputQuantize,
    /// The console shows in the lower pane.
    pub console: bool,
    /// The console is maximised over the rack.
    pub console_maximised: bool,
    /// Delay compensation is on (document switch).
    pub compensation: bool,
    /// Console devices are shown in the rack.
    pub console_devices: bool,
    /// The console links are drawn as cables.
    pub console_links: bool,
    /// The arrangement shows the tempo lane.
    pub tempo_lane: bool,
    /// The arrangement shows the marker strip.
    pub marker_strip: bool,
    /// The arrangement shows the marker list.
    pub marker_list: bool,
    /// The rack navigator is on.
    pub rack_navigator: bool,
    /// The rack is shown.
    pub rack: bool,
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
    /// What the selected tracks and folder allow (the Track menu).
    pub org: OrgMenu,
    /// What the selected audio clips allow (the audio items of the Edit and Audio menus).
    pub audio: AudioMenu,
    /// What the selected tracks and clips allow for freezing and bouncing (the Track menu).
    pub freeze: FreezeMenu,
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
    /// Label (`…` marks items that open a dialog).
    pub label: &'static str,
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
    /// The items (separators skipped).
    pub fn items(&self) -> impl Iterator<Item = &MenuItem> {
        self.entries.iter().filter_map(|e| match e {
            MenuEntry::Item(i) => Some(i),
            MenuEntry::Separator => None,
        })
    }
}

/// The menu bar: File, Edit, View, Transport, Mixer, Audio. Shortcut texts come from `keymap`
/// (its OS decides `Ctrl` or `Cmd`); no key is written in this function.
pub fn menus(state: &MenuState, keymap: &Keymap) -> Vec<Menu> {
    let make = |label, command: AppCommand, checked, enabled| {
        MenuEntry::Item(MenuItem {
            label,
            command,
            shortcut: command.shortcut_text(keymap),
            checked,
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
    vec![
        Menu {
            title: "File",
            entries: vec![
                item("New", AppCommand::New),
                item("Open\u{2026}", AppCommand::Open),
                item("Save", AppCommand::Save),
                item("Save As\u{2026}", AppCommand::SaveAs),
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
                enabled("Undo", AppCommand::Undo, state.can_undo),
                enabled("Redo", AppCommand::Redo, state.can_redo),
                MenuEntry::Separator,
                enabled("Cut", AppCommand::Cut, state.can_edit),
                enabled("Copy", AppCommand::Copy, state.can_edit),
                enabled("Paste", AppCommand::Paste, state.can_paste),
                enabled("Duplicate", AppCommand::Duplicate, state.can_edit),
                enabled("Delete", AppCommand::Delete, state.can_edit),
                MenuEntry::Separator,
                item("Select All", AppCommand::SelectAll),
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
                cables("Cables: Hide", CableVisibility::Hide),
                MenuEntry::Separator,
                item("Zoom In", AppCommand::ZoomIn),
                item("Zoom Out", AppCommand::ZoomOut),
                item("Actual Size", AppCommand::ZoomReset),
                MenuEntry::Separator,
                check("Device Browser", AppCommand::ToggleBrowser, state.browser),
                check(
                    "File Browser",
                    AppCommand::ToggleFileBrowser,
                    state.file_browser,
                ),
                check("Rack", AppCommand::ToggleRack, state.rack),
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
                MenuEntry::Separator,
                item("Add Marker", AppCommand::AddMarker),
                item("Add Range Marker from Loop", AppCommand::AddRangeFromLoop),
                item("Previous Marker", AppCommand::PreviousMarker),
                item("Next Marker", AppCommand::NextMarker),
                MenuEntry::Separator,
                quantize("Input Quantize: Off", InputQuantize::Off),
                quantize("Input Quantize: 1/8", InputQuantize::Eighth),
                quantize("Input Quantize: 1/16", InputQuantize::Sixteenth),
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
                check(
                    "Delay Compensation",
                    AppCommand::ToggleCompensation,
                    state.compensation,
                ),
            ],
        },
        Menu {
            title: "Create",
            entries: vec![
                item("Create Instrument\u{2026}", AppCommand::CreateInstrument),
                item("Create Effect\u{2026}", AppCommand::CreateEffect),
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
                    "Track Comment\u{2026}",
                    AppCommand::TrackComment,
                    state.org.track_selected,
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
            can_undo: true,
            can_redo: false,
            can_edit: false,
            can_paste: false,
            song_visible: true,
            piano_roll: false,
            sample_editor: false,
            step_editor: false,
            slice_editor: false,
            playing: false,
            recording: false,
            loop_enabled: false,
            metronome: false,
            quantize: InputQuantize::Off,
            console: false,
            console_maximised: false,
            compensation: true,
            console_devices: false,
            console_links: false,
            tempo_lane: false,
            marker_strip: false,
            marker_list: false,
            rack_navigator: true,
            rack: true,
            sequencer_only: false,
            editor: false,
            missing_audio: false,
            midi_clip: false,
            midi_track: false,
            midi_song: false,
            org: OrgMenu::default(),
            audio: AudioMenu::default(),
            freeze: FreezeMenu::default(),
        }
    }

    /// Every app command (a variant of each kind), for the bijection tests.
    fn all_commands() -> Vec<AppCommand> {
        let mut v = vec![
            AppCommand::New,
            AppCommand::Open,
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
            AppCommand::QuantizeApply,
            AppCommand::QuantizeOptions,
            AppCommand::ToggleFlip,
            AppCommand::ZoomIn,
            AppCommand::ZoomOut,
            AppCommand::ZoomReset,
            AppCommand::ToggleBrowser,
            AppCommand::ToggleFileBrowser,
            AppCommand::ToggleSong,
            AppCommand::TogglePianoRoll,
            AppCommand::ToggleSampleEditor,
            AppCommand::ToggleStepEditor,
            AppCommand::ToggleSliceEditor,
            AppCommand::ToggleConsoleDevices,
            AppCommand::ToggleConsoleLinks,
            AppCommand::ToggleTempoLane,
            AppCommand::ToggleMarkerStrip,
            AppCommand::ToggleMarkerList,
            AppCommand::AddMarker,
            AppCommand::AddRangeFromLoop,
            AppCommand::PreviousMarker,
            AppCommand::NextMarker,
            AppCommand::ToggleConsole,
            AppCommand::MaximiseConsole,
            AppCommand::AddChannel,
            AppCommand::RouteToNewBus,
            AppCommand::ToggleCompensation,
            AppCommand::CreateInstrument,
            AppCommand::CreateEffect,
            AppCommand::CreateAudioTrack,
            AppCommand::CreateMixerChannel,
            AppCommand::ToggleRack,
            AppCommand::ToggleRackNavigator,
            AppCommand::SequencerOnly,
            AppCommand::ToggleEditor,
            AppCommand::GroupTracks,
            AppCommand::UngroupTracks,
            AppCommand::ToggleFolder,
            AppCommand::RenameFolder,
            AppCommand::MoveTrackUp,
            AppCommand::MoveTrackDown,
            AppCommand::TrackComment,
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
            AppCommand::RestartAudio,
        ];
        for step in [GainStep::Up, GainStep::Down, GainStep::Reset] {
            v.push(AppCommand::AudioGain(step));
        }
        for mode in WarpMode::ALL {
            v.push(AppCommand::AudioWarpMode(mode));
        }
        for bars in [1, 2, 4, 8, 16] {
            v.push(AppCommand::AudioWarpBars(bars));
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
            CableVisibility::Hide,
        ] {
            v.push(AppCommand::Cables(c));
        }
        for t in [
            TransportAction::Play,
            TransportAction::Stop,
            TransportAction::TogglePlay,
            TransportAction::ReturnToZero,
            TransportAction::ToggleLoop,
            TransportAction::ToggleRecord,
            TransportAction::ToggleMetronome,
        ] {
            v.push(AppCommand::Transport(t));
        }
        for q in [
            InputQuantize::Off,
            InputQuantize::Eighth,
            InputQuantize::Sixteenth,
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
        let model = menus(&state(), &keymap);
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
            // F4 (the file browser) is new: it is not part of the old function being compared.
            Resolution::Bound(list) => list
                .iter()
                .find_map(|r| AppCommand::from_command(&r.command, r.arg.as_deref()))
                .filter(|c| *c != AppCommand::ToggleFileBrowser),
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
    fn the_menu_bar_has_the_five_menus_with_their_items() {
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
                "Audio"
            ]
        );
        let labels = |i: usize| m[i].items().map(|it| it.label).collect::<Vec<_>>();
        assert_eq!(
            labels(0),
            [
                "New",
                "Open\u{2026}",
                "Save",
                "Save As\u{2026}",
                "Import Audio\u{2026}",
                "Import MIDI\u{2026}",
                "Import MIDI as Song\u{2026}",
                "Relink Missing Audio\u{2026}",
                "Export Patch\u{2026}",
                "Export MIDI Clip\u{2026}",
                "Export MIDI Track\u{2026}",
                "Export MIDI Song\u{2026}",
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
                "Quantize",
                "Quantize Options\u{2026}",
                "Reverse Audio",
                "Normalise Audio",
                "Strip Silence\u{2026}",
                "Quantize Audio"
            ]
        );
        assert_eq!(
            labels(2),
            [
                "Back of Rack",
                "Cables: Show",
                "Cables: Hover",
                "Cables: Hide",
                "Zoom In",
                "Zoom Out",
                "Actual Size",
                "Device Browser",
                "File Browser",
                "Rack",
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
            ]
        );
        assert_eq!(
            labels(4),
            [
                "Console",
                "Maximise Console",
                "Add Channel\u{2026}",
                "Route to New Output Bus",
                "Delay Compensation"
            ]
        );
        assert_eq!(
            labels(5),
            [
                "Create Instrument\u{2026}",
                "Create Effect\u{2026}",
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
                "Track Comment\u{2026}",
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
                can_undo: false,
                can_redo: true,
                can_edit: true,
                can_paste: false,
                song_visible: false,
                piano_roll: true,
                sample_editor: false,
                step_editor: false,
                slice_editor: false,
                playing: true,
                recording: false,
                loop_enabled: true,
                metronome: false,
                quantize: InputQuantize::Eighth,
                console: true,
                console_maximised: false,
                compensation: false,
                console_devices: true,
                console_links: false,
                tempo_lane: false,
                marker_strip: false,
                marker_list: false,
                rack_navigator: true,
                rack: true,
                sequencer_only: false,
                editor: false,
                missing_audio: true,
                midi_clip: true,
                midi_track: false,
                midi_song: true,
                org: OrgMenu::default(),
                audio: AudioMenu::default(),
                freeze: FreezeMenu {
                    tail: true,
                    ..FreezeMenu::default()
                },
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
