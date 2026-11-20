//! Which commands may take the live app past a plugin whose state it has not captured yet.
//!
//! A plugin's state reaches the document by a background capture (see `app_core_plugins.rs`), so
//! it can be a little behind the plugin. That only matters when the stored state is about to be
//! used: a command that removes or replaces a device that holds a plugin, or takes it out of the
//! rack (a frozen track's plugin is not built), would put the stale state in its undo step. Those
//! commands capture first. Every command and sub-command enum is matched here without a wildcard,
//! so a new variant does not compile until it is classified.

use atomik_model::{Command, ConsoleCommand, SongCommand, WorkflowCommand};

/// True when `command` leaves every device that holds a plugin in place, so the live app need not
/// wait for the plugin's state before it. A `Batch` keeps plugins when every child does.
pub(super) fn keeps_plugins(command: &Command) -> bool {
    match command {
        Command::Batch(children) => children.iter().all(keeps_plugins),
        Command::Song(song) => song_keeps_plugins(song),
        Command::Console(console) => console_keeps_plugins(console),
        Command::Workflow(workflow) => workflow_keeps_plugins(workflow),
        // These leave every plugin device where it is.
        Command::AddDevice { .. }
        | Command::AddDeviceAt { .. }
        | Command::AddRackColumn
        | Command::RemoveRackColumn { .. }
        | Command::MoveDevice { .. }
        | Command::SetFolded { .. }
        | Command::Rename { .. }
        | Command::SetColor { .. }
        | Command::Connect { .. }
        | Command::Disconnect { .. }
        | Command::SetParam { .. }
        | Command::SetTempo { .. }
        | Command::RegisterAsset { .. }
        | Command::RelinkAsset { .. }
        | Command::RenameAsset { .. }
        | Command::SetInputChannels { .. }
        | Command::SetOutputChannels { .. }
        | Command::SetSongInfo { .. }
        | Command::SetMasterTune { .. }
        | Command::SetRackSort { .. }
        | Command::SetReferences { .. }
        | Command::SetDelivery { .. }
        | Command::SetCameraOverride { .. }
        | Command::SetInsertsFolded { .. }
        | Command::SetSpectrumCurves { .. }
        | Command::Video { .. }
        | Command::ConvertSampleRate { .. }
        | Command::MidiMap { .. }
        | Command::Macro { .. }
        | Command::SetZones { .. }
        | Command::SetPattern { .. }
        | Command::SetCurves { .. }
        | Command::SetDrumStep { .. }
        | Command::SetDrumStepFx { .. }
        | Command::SetDrumLane { .. }
        | Command::SetDrumPattern { .. }
        | Command::AddDrumPattern { .. }
        | Command::DuplicateDrumPattern { .. }
        | Command::RemoveDrumPattern { .. }
        | Command::AddPattern { .. }
        | Command::RemovePattern { .. }
        | Command::DuplicatePattern { .. } => true,
        // These remove or replace devices, or move them to another key (RemoveDevice, a
        // container, a plugin command): the stored state must be current first.
        Command::RemoveDevice { .. } | Command::Container { .. } | Command::Plugin { .. } => false,
    }
}

/// Song edits touch the song only; freezing takes a track's plugin out of the rack.
fn song_keeps_plugins(command: &SongCommand) -> bool {
    match command {
        SongCommand::AddTrack { .. }
        | SongCommand::RemoveTrack { .. }
        | SongCommand::SetTrackFlags { .. }
        | SongCommand::SetTrackChase { .. }
        | SongCommand::SetDirectRecord { .. }
        | SongCommand::SetTrackGroove { .. }
        | SongCommand::SetClipGroove { .. }
        | SongCommand::AddNoteLane { .. }
        | SongCommand::RemoveNoteLane { .. }
        | SongCommand::RenameLane { .. }
        | SongCommand::MoveNoteLane { .. }
        | SongCommand::MergeNoteLanes { .. }
        | SongCommand::SetLaneMuted { .. }
        | SongCommand::SetLaneSolo { .. }
        | SongCommand::SetLaneColor { .. }
        | SongCommand::RemoveAutomationLane { .. }
        | SongCommand::SetLaneStatic { .. }
        | SongCommand::SetLaneWriteMode { .. }
        | SongCommand::GroupTracks { .. }
        | SongCommand::Ungroup { .. }
        | SongCommand::RenameGroup { .. }
        | SongCommand::SetGroupCollapsed { .. }
        | SongCommand::SetGroupColor { .. }
        | SongCommand::MoveTrack { .. }
        | SongCommand::MoveTracks { .. }
        | SongCommand::MoveGroup { .. }
        | SongCommand::SetGroupFlags { .. }
        | SongCommand::MoveGroupTo { .. }
        | SongCommand::SetTrackColor { .. }
        | SongCommand::SetTrackComment { .. }
        | SongCommand::SetLaneFolded { .. }
        | SongCommand::SetTakesOpen { .. }
        | SongCommand::AddAutomationLane { .. }
        | SongCommand::AddClip { .. }
        | SongCommand::RemoveClip { .. }
        | SongCommand::SetClipGeometry { .. }
        | SongCommand::SetClipContent { .. }
        | SongCommand::SplitClip { .. }
        | SongCommand::DuplicateClip { .. }
        | SongCommand::SetTempoMap { .. }
        | SongCommand::DragTempoMap { .. }
        | SongCommand::SetMeter { .. }
        | SongCommand::SetMeterMap { .. }
        | SongCommand::DragMeterMap { .. }
        | SongCommand::SetHarmony { .. }
        | SongCommand::DragHarmony { .. }
        | SongCommand::SetSheets { .. }
        | SongCommand::SetMarkers { .. }
        | SongCommand::DragMarkers { .. }
        | SongCommand::SetRanges { .. }
        | SongCommand::DragRanges { .. }
        | SongCommand::SetTrackHeights { .. }
        | SongCommand::DragTrackHeights { .. }
        | SongCommand::SetLoop { .. }
        | SongCommand::SetSongEnd { .. }
        | SongCommand::Launcher { .. }
        | SongCommand::SetClipGrid { .. }
        | SongCommand::SetClipControllers { .. }
        | SongCommand::SetTrackControllers { .. }
        | SongCommand::SetClipMidi { .. }
        | SongCommand::SetClipParamLanes { .. }
        | SongCommand::SetClipScale { .. }
        | SongCommand::SetClipFollowKey { .. }
        | SongCommand::SetClipMeta { .. }
        | SongCommand::AddAudioTrack { .. }
        | SongCommand::AddAudioClip { .. }
        | SongCommand::NewTakeGroup { .. }
        | SongCommand::GroupAsTakes { .. }
        | SongCommand::AddTake { .. }
        | SongCommand::PickTake { .. }
        | SongCommand::SetCompSegment { .. }
        | SongCommand::SetLinkedComp { .. }
        | SongCommand::LinkTakeGroups { .. }
        | SongCommand::UnlinkTakeGroup { .. }
        | SongCommand::DeleteTake { .. }
        | SongCommand::DuplicateTake { .. }
        | SongCommand::MoveTake { .. }
        | SongCommand::DeleteUnusedTakes { .. }
        | SongCommand::SetCompXfade { .. }
        | SongCommand::SetCompSilence { .. }
        | SongCommand::SetTake { .. }
        | SongCommand::SetTakeLoop { .. }
        | SongCommand::FlattenTakes { .. }
        | SongCommand::SetCompRange { .. }
        | SongCommand::FlattenComp { .. }
        | SongCommand::ApplyQuantize { .. }
        | SongCommand::QuantizeNotes { .. }
        | SongCommand::ApplyNoteTool { .. }
        | SongCommand::SetGroove { .. }
        | SongCommand::RenameGroove { .. }
        | SongCommand::DeleteGroove { .. }
        | SongCommand::ExtractGroove { .. }
        | SongCommand::SetAudioClip { .. }
        | SongCommand::SetWarpMarkers { .. }
        | SongCommand::SetPitchEdits { .. }
        | SongCommand::StripSilence { .. }
        | SongCommand::AddRenderedTrack { .. }
        | SongCommand::AddStemTracks { .. } => true,
        SongCommand::FreezeTrack { .. }
        | SongCommand::RefreezeTrack { .. }
        | SongCommand::UnfreezeTrack { .. } => false,
    }
}

/// Console edits add, move and cable strips and inserts; removing a strip removes its console
/// device only, and an insert taken out of a chain stays in the rack.
fn console_keeps_plugins(command: &ConsoleCommand) -> bool {
    match command {
        ConsoleCommand::AddChannel { .. }
        | ConsoleCommand::AddChannelFor { .. }
        | ConsoleCommand::AddMixChannel { .. }
        | ConsoleCommand::CreateParallel { .. }
        | ConsoleCommand::SetSendFollow { .. }
        | ConsoleCommand::RemoveChannel { .. }
        | ConsoleCommand::MoveChannel { .. }
        | ConsoleCommand::SetReturn { .. }
        | ConsoleCommand::SetInsert { .. }
        | ConsoleCommand::AddInsert { .. }
        | ConsoleCommand::ExtractInsert { .. }
        | ConsoleCommand::MoveInsert { .. }
        | ConsoleCommand::PasteSettings { .. }
        | ConsoleCommand::ResetStrip { .. }
        | ConsoleCommand::ClearInserts { .. }
        | ConsoleCommand::SetCompensation { .. }
        | ConsoleCommand::SetAlignReturns { .. }
        | ConsoleCommand::ClearSolos
        | ConsoleCommand::ClearMutes
        | ConsoleCommand::CreateBus { .. }
        | ConsoleCommand::RenameBus { .. }
        | ConsoleCommand::DeleteBus { .. }
        | ConsoleCommand::ReorderBus { .. }
        | ConsoleCommand::SetOutput { .. }
        | ConsoleCommand::RouteToNewBus { .. }
        | ConsoleCommand::CreateVca { .. }
        | ConsoleCommand::CreateVcaForFolder { .. }
        | ConsoleCommand::SetVcaFollow { .. }
        | ConsoleCommand::RouteFolderToBus { .. }
        | ConsoleCommand::RouteFolderToNewBus { .. }
        | ConsoleCommand::UnrouteFolder { .. }
        | ConsoleCommand::RouteVcaToBus { .. }
        | ConsoleCommand::RouteVcaToNewBus { .. }
        | ConsoleCommand::UnrouteVca { .. }
        | ConsoleCommand::RenameVca { .. }
        | ConsoleCommand::SetVcaColor { .. }
        | ConsoleCommand::SetVcaLevel { .. }
        | ConsoleCommand::SetVcaMute { .. }
        | ConsoleCommand::SetVcaSolo { .. }
        | ConsoleCommand::AssignToVca { .. }
        | ConsoleCommand::DeleteVca { .. }
        | ConsoleCommand::StoreSnapshot { .. }
        | ConsoleCommand::UpdateSnapshot { .. }
        | ConsoleCommand::RecallSnapshot { .. }
        | ConsoleCommand::RenameSnapshot { .. }
        | ConsoleCommand::SetSnapshotEffects { .. }
        | ConsoleCommand::SetSnapshotNote { .. }
        | ConsoleCommand::SetSnapshotColor { .. }
        | ConsoleCommand::MorphSnapshots { .. }
        | ConsoleCommand::MorphFromMix { .. }
        | ConsoleCommand::SetSnapshotMarker { .. }
        | ConsoleCommand::CompareStrip { .. }
        | ConsoleCommand::DeleteSnapshot { .. }
        | ConsoleCommand::MoveSnapshot { .. }
        | ConsoleCommand::Compare { .. }
        | ConsoleCommand::EndCompare => true,
    }
}

/// Workflows that delete, replace or reset devices need the stored state; the others only add
/// devices or edit the song, cables and view.
fn workflow_keeps_plugins(command: &WorkflowCommand) -> bool {
    match command {
        WorkflowCommand::DuplicateDevices { .. }
        | WorkflowCommand::PasteDevices { .. }
        | WorkflowCommand::MoveDevices { .. }
        | WorkflowCommand::MoveDevicesToSlot { .. }
        | WorkflowCommand::PasteDevicesAt { .. }
        | WorkflowCommand::CreateInstrumentAt { .. }
        | WorkflowCommand::CreateEffectAt { .. }
        | WorkflowCommand::AddReverbSend { .. }
        | WorkflowCommand::DuckFrom { .. }
        | WorkflowCommand::GentlePump { .. }
        | WorkflowCommand::AddRumble { .. }
        | WorkflowCommand::AddTransition { .. }
        | WorkflowCommand::DuplicateTrack { .. }
        | WorkflowCommand::DuplicateTracks { .. }
        | WorkflowCommand::SetFoldedMany { .. }
        | WorkflowCommand::DisconnectDevice { .. }
        | WorkflowCommand::PasteClips { .. }
        | WorkflowCommand::PasteTakes { .. }
        | WorkflowCommand::DuplicateClips { .. }
        | WorkflowCommand::MoveClips { .. }
        | WorkflowCommand::RepeatClips { .. }
        | WorkflowCommand::RemoveClips { .. }
        | WorkflowCommand::SetClipsMuted { .. }
        | WorkflowCommand::ExtractNotes { .. }
        | WorkflowCommand::ExplodeNotes { .. }
        | WorkflowCommand::ConvertLoop { .. }
        | WorkflowCommand::RenderLoop { .. }
        | WorkflowCommand::RenderJoin { .. }
        | WorkflowCommand::DuplicateLoopInstance { .. }
        | WorkflowCommand::AutomationIntoClip { .. }
        | WorkflowCommand::JoinClips { .. }
        | WorkflowCommand::TrimClips { .. }
        | WorkflowCommand::ExtendClips { .. }
        | WorkflowCommand::DeleteOutsideEvents { .. }
        | WorkflowCommand::SplitClips { .. }
        | WorkflowCommand::CutOut { .. }
        | WorkflowCommand::ReverseClips { .. }
        | WorkflowCommand::ScaleTempo { .. }
        | WorkflowCommand::MoveCrossfade { .. }
        | WorkflowCommand::ResizeCrossfade { .. }
        | WorkflowCommand::StretchClips { .. }
        | WorkflowCommand::ChopClips { .. }
        | WorkflowCommand::CleanupAutomation { .. }
        | WorkflowCommand::InsertTime { .. }
        | WorkflowCommand::RemoveTime { .. }
        | WorkflowCommand::LayerRecording { .. }
        | WorkflowCommand::EngulfRecording { .. }
        | WorkflowCommand::PasteNotes { .. }
        | WorkflowCommand::DuplicateNotes { .. }
        | WorkflowCommand::PasteAutomationPoints { .. }
        | WorkflowCommand::DuplicatePoints { .. }
        | WorkflowCommand::CreateInstrument { .. }
        | WorkflowCommand::CreateEffect { .. }
        | WorkflowCommand::CreatePlayer { .. }
        | WorkflowCommand::AutoRoute { .. }
        | WorkflowCommand::AssignSend { .. }
        | WorkflowCommand::DisconnectDevices { .. }
        | WorkflowCommand::SetPatchName { .. }
        | WorkflowCommand::SetMidiOutPort { .. }
        | WorkflowCommand::SetHeadphoneData { .. }
        | WorkflowCommand::SetModSlot { .. }
        | WorkflowCommand::ImportMidi { .. } => true,
        WorkflowCommand::RemoveDevices { .. }
        | WorkflowCommand::ReplaceDevice { .. }
        | WorkflowCommand::SplitSub { .. }
        | WorkflowCommand::RemoveDucking { .. }
        | WorkflowCommand::ApplyMasterChain { .. }
        | WorkflowCommand::DeleteDevices { .. }
        | WorkflowCommand::ApplyPatch { .. }
        | WorkflowCommand::ResetDevice { .. } => false,
    }
}
