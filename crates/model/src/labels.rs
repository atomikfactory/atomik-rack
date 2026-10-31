//! Names for the undo steps: `Command::label` gives the menu text of one command ("Move Device",
//! "Add Clip"), the [`crate::Document`] keeps the name of every step it records and
//! [`crate::Document::undo_label`] / [`crate::Document::redo_label`] hand the next one out. A
//! command without an entry here is called [`DEFAULT_LABEL`]. A step several commands share (a
//! drag that merges, a follow-up joined to the step before) keeps the name of the first.

use crate::{
    Command, ConsoleCommand, ContainerCommand, PluginCommand, SongCommand, WorkflowCommand,
};
use std::collections::VecDeque;

/// The name of a step whose command has no entry in the table.
pub const DEFAULT_LABEL: &str = "Edit";

/// The names of the undo, redo and trial steps, in the order of the steps they belong to.
#[derive(Debug, Clone, Default)]
pub(crate) struct Labels {
    pub(crate) undo: VecDeque<&'static str>,
    pub(crate) redo: Vec<&'static str>,
    pub(crate) trial: Vec<&'static str>,
    pub(crate) trial_redo: Vec<&'static str>,
}

impl Command {
    /// The undo-menu name of this command, `"Edit"` when it has none of its own. A batch is
    /// named after its first command.
    pub fn label(&self) -> &'static str {
        match self {
            Command::AddDevice { .. } | Command::AddDeviceAt { .. } => "Add Device",
            Command::AddRackColumn => "Add Rack Column",
            Command::RemoveRackColumn { .. } => "Remove Rack Column",
            Command::RemoveDevice { .. } => "Delete Device",
            Command::MoveDevice { .. } => "Move Device",
            Command::SetFolded { .. } => "Fold Device",
            Command::Rename { .. } => "Rename",
            Command::SetColor { .. } => "Set Color",
            Command::Connect(_) => "Connect Cable",
            Command::Disconnect { .. } => "Disconnect Cable",
            Command::SetParam { .. } => "Change Parameter",
            Command::SetTempo(_) => "Change Tempo",
            Command::Song(c) => song_label(c),
            Command::RelinkAsset { .. } => "Relink Sample",
            Command::RenameAsset { .. } => "Rename Sample",
            Command::SetInputChannels(_) | Command::SetOutputChannels(_) => "Set Channels",
            Command::ConvertSampleRate { .. } => "Convert Sample Rate",
            Command::Console(c) => console_label(c),
            Command::Container(c) => container_label(c),
            Command::Plugin(PluginCommand::Add { .. } | PluginCommand::AddAt { .. }) => {
                "Add Plugin"
            }
            Command::Plugin(PluginCommand::Relink { .. }) => "Relink Plugin",
            Command::Batch(commands) => commands.first().map_or(DEFAULT_LABEL, Command::label),
            Command::SetZones { .. } => "Edit Zones",
            Command::SetPattern { .. } | Command::AddPattern { .. } => "Edit Pattern",
            Command::SetDrumStep { .. } | Command::SetDrumPattern { .. } => "Edit Drum Pattern",
            Command::SetCurves { .. } => "Edit Curves",
            Command::Workflow(c) => workflow_label(c),
            _ => DEFAULT_LABEL,
        }
    }
}

fn song_label(c: &SongCommand) -> &'static str {
    match c {
        SongCommand::AddTrack { .. }
        | SongCommand::AddAudioTrack { .. }
        | SongCommand::AddRenderedTrack { .. } => "Add Track",
        SongCommand::RemoveTrack { .. } => "Delete Track",
        SongCommand::SetTrackFlags { .. } => "Change Track",
        SongCommand::MoveTrack { .. } => "Move Track",
        SongCommand::AddAutomationLane { .. } => "Add Automation Lane",
        SongCommand::RemoveAutomationLane { .. } => "Remove Automation Lane",
        SongCommand::AddClip { .. } | SongCommand::AddAudioClip { .. } => "Add Clip",
        SongCommand::RemoveClip { .. } => "Delete Clip",
        SongCommand::SetClipGeometry { .. } => "Move Clip",
        SongCommand::SetClipContent { .. } => "Edit Clip",
        SongCommand::SplitClip { .. } => "Split Clip",
        SongCommand::DuplicateClip { .. } => "Duplicate Clip",
        SongCommand::SetTempoMap(_) | SongCommand::DragTempoMap { .. } => "Edit Tempo Map",
        SongCommand::SetMarkers(_) | SongCommand::DragMarkers { .. } => "Edit Markers",
        SongCommand::SetLoop { .. } => "Set Loop",
        SongCommand::SetSongEnd(_) => "Set Song End",
        SongCommand::QuantizeNotes { .. } | SongCommand::ApplyQuantize { .. } => "Quantize",
        SongCommand::FreezeTrack { .. } => "Freeze Track",
        SongCommand::UnfreezeTrack { .. } => "Unfreeze Track",
        _ => DEFAULT_LABEL,
    }
}

fn console_label(c: &ConsoleCommand) -> &'static str {
    match c {
        ConsoleCommand::AddChannel { .. } | ConsoleCommand::AddChannelFor { .. } => "Add Channel",
        ConsoleCommand::RemoveChannel { .. } => "Delete Channel",
        ConsoleCommand::MoveChannel { .. } => "Move Channel",
        ConsoleCommand::SetReturn { .. } => "Set Send Effect",
        ConsoleCommand::SetInsert { .. } | ConsoleCommand::MoveInsert { .. } => "Edit Inserts",
        ConsoleCommand::CreateBus { .. } => "Create Bus",
        ConsoleCommand::DeleteBus { .. } => "Delete Bus",
        ConsoleCommand::SetOutput { .. } => "Set Output",
        _ => DEFAULT_LABEL,
    }
}

fn container_label(c: &ContainerCommand) -> &'static str {
    match c {
        ContainerCommand::Combine { .. } => "Combine",
        ContainerCommand::Uncombine { .. } => "Uncombine",
        ContainerCommand::MoveIntoContainer { .. } => "Move Into Container",
        ContainerCommand::MoveOutOfContainer { .. } => "Move Out of Container",
        ContainerCommand::DuplicateContainer { .. } => "Duplicate Container",
        _ => DEFAULT_LABEL,
    }
}

fn workflow_label(c: &WorkflowCommand) -> &'static str {
    match c {
        WorkflowCommand::RemoveDevices { .. } | WorkflowCommand::DeleteDevices { .. } => {
            "Delete Devices"
        }
        WorkflowCommand::DuplicateDevices { .. } => "Duplicate Devices",
        WorkflowCommand::PasteDevices { .. } | WorkflowCommand::PasteDevicesAt { .. } => {
            "Paste Devices"
        }
        WorkflowCommand::MoveDevices { .. } | WorkflowCommand::MoveDevicesToSlot { .. } => {
            "Move Device"
        }
        WorkflowCommand::CreateInstrument { .. } | WorkflowCommand::CreateInstrumentAt { .. } => {
            "Create Instrument"
        }
        WorkflowCommand::CreateEffect { .. } | WorkflowCommand::CreateEffectAt { .. } => {
            "Create Effect"
        }
        WorkflowCommand::CreatePlayer { .. } => "Create Player",
        WorkflowCommand::ReplaceDevice { .. } => "Replace Device",
        WorkflowCommand::SetFoldedMany { .. } => "Fold Devices",
        WorkflowCommand::AutoRoute { .. } => "Auto-route",
        WorkflowCommand::DisconnectDevice { .. } | WorkflowCommand::DisconnectDevices { .. } => {
            "Disconnect"
        }
        WorkflowCommand::DuplicateTrack { .. } => "Duplicate Track",
        WorkflowCommand::PasteClips { .. } => "Paste Clips",
        WorkflowCommand::DuplicateClips { .. } => "Duplicate Clips",
        WorkflowCommand::MoveClips { copy: true, .. } => "Copy Clips",
        WorkflowCommand::MoveClips { .. } => "Move Clips",
        WorkflowCommand::RemoveClips { .. } => "Delete Clips",
        WorkflowCommand::SplitClips { .. } => "Split Clips",
        WorkflowCommand::JoinClips { .. } => "Join Clips",
        WorkflowCommand::TrimClips { .. } => "Trim Clips",
        WorkflowCommand::ReverseClips { .. } => "Reverse Clips",
        WorkflowCommand::ScaleTempo { .. } => "Scale Tempo",
        WorkflowCommand::ChopClips { .. } => "Chop Clips",
        WorkflowCommand::InsertTime { .. } => "Insert Bars",
        WorkflowCommand::RemoveTime { .. } => "Remove Bars",
        WorkflowCommand::PasteNotes { .. } | WorkflowCommand::DuplicateNotes { .. } => "Edit Notes",
        WorkflowCommand::ApplyPatch { .. } => "Load Patch",
        WorkflowCommand::ResetDevice { .. } => "Reset Device",
        _ => DEFAULT_LABEL,
    }
}
