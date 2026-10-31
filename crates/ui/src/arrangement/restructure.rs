//! The restructuring commands of the [`ArrangementView`]: insert and remove bars at the loop
//! locators, reverse, scale tempo and chop clips. Each sends one workflow command (one undo
//! step); the planning and the rules are in the model.

use super::{ArrangementView, workflow_cmd};
use crate::action::UiAction;
use crate::keymap::CommandOutcome;
use atomik_model::{
    ClipContent, ClipId, Command, LaneKind, SongCommand, ThinLevel, WorkflowCommand,
};

impl ArrangementView {
    /// The restructuring commands; `None` for an id that is not one of them.
    pub(super) fn restructure_command(
        &mut self,
        id: &str,
        arg: Option<&str>,
    ) -> Option<CommandOutcome> {
        match id {
            "sequencer.insert_bars" | "sequencer.remove_bars" => {
                let (start, end) = self.song.loop_range;
                let (at, length) = (start, end - start);
                let command = if id == "sequencer.insert_bars" {
                    WorkflowCommand::InsertTime { at, length }
                } else {
                    WorkflowCommand::RemoveTime { at, length }
                };
                Some(CommandOutcome::Handled(vec![workflow_cmd(command)]))
            }
            "sequencer.reverse_clips" => {
                let graphical = match arg {
                    None | Some("bars") => true,
                    Some("starts") => false,
                    Some(_) => return Some(CommandOutcome::Declined),
                };
                Some(self.with_selection("Select the clips to reverse", |ids| {
                    WorkflowCommand::ReverseClips { ids, graphical }
                }))
            }
            "sequencer.scale_tempo" => {
                let percent = match arg? {
                    "double" => 200.0,
                    "halve" => 50.0,
                    other => match other.parse::<f64>() {
                        Ok(p) => p,
                        Err(_) => return Some(CommandOutcome::Declined),
                    },
                };
                Some(self.with_selection("Select the clips to scale", |ids| {
                    WorkflowCommand::ScaleTempo { ids, percent }
                }))
            }
            "sequencer.chop_clips" => {
                let Some(first) = self.selected_in_order().first().map(|c| c.start) else {
                    return Some(CommandOutcome::NotApplicable(
                        "Select the clips to chop".into(),
                    ));
                };
                let table = self.song.meter_table();
                let map = table.map();
                let length = match arg? {
                    "bar" => map.bar_ticks_at(first),
                    "half_bar" => map.bar_ticks_at(first) / 2,
                    "beat" => map.beat_ticks_at(first),
                    "eighth" => map.beat_ticks_at(first) / 2,
                    "sixteenth" => map.beat_ticks_at(first) / 4,
                    _ => return Some(CommandOutcome::Declined),
                };
                Some(self.with_selection("Select the clips to chop", |ids| {
                    WorkflowCommand::ChopClips { ids, length }
                }))
            }
            "sequencer.cleanup_automation" => {
                let level = ThinLevel::from_id(arg?)?;
                let ids = self.cleanup_targets();
                if ids.is_empty() {
                    return Some(CommandOutcome::NotApplicable(
                        "Select automation clips or tracks to clean up".into(),
                    ));
                }
                Some(CommandOutcome::Handled(vec![workflow_cmd(
                    WorkflowCommand::CleanupAutomation { ids, level },
                )]))
            }
            "sequencer.note_tool" => {
                let Some(tool) = crate::note_tool_args::parse_note_tool(arg?, self.tool_seed)
                else {
                    return Some(CommandOutcome::Declined);
                };
                let clips: Vec<ClipId> = self.selected_in_order().iter().map(|c| c.id).collect();
                if clips.is_empty() {
                    return Some(CommandOutcome::NotApplicable(
                        "Select the note clips to edit".into(),
                    ));
                }
                self.tool_seed = self.tool_seed.wrapping_add(1);
                Some(CommandOutcome::Handled(vec![UiAction::Command(
                    Command::Song(SongCommand::ApplyNoteTool {
                        clips,
                        selection: None,
                        tool,
                    }),
                )]))
            }
            _ => None,
        }
    }

    /// The automation clips Cleanup Automation works on: the selected ones, else every one on the
    /// automation lanes of the selected tracks.
    fn cleanup_targets(&self) -> Vec<ClipId> {
        let selected: Vec<ClipId> = self
            .selected_in_order()
            .iter()
            .filter(|c| matches!(c.content, ClipContent::Automation(_)))
            .map(|c| c.id)
            .collect();
        if !selected.is_empty() {
            return selected;
        }
        let tracks = self.selected_tracks();
        let mut ids = Vec::new();
        for t in self
            .song
            .tracks
            .iter()
            .filter(|t| tracks.contains(&t.device))
        {
            for lane in t
                .lanes
                .iter()
                .filter(|l| matches!(l.kind, LaneKind::Automation(_)))
            {
                ids.extend(lane.clips.iter().map(|c| c.id));
            }
        }
        ids
    }

    /// `make` of the selected clips, or the message when none is selected.
    fn with_selection(
        &self,
        none: &str,
        make: impl FnOnce(Vec<ClipId>) -> WorkflowCommand,
    ) -> CommandOutcome {
        let ids: Vec<ClipId> = self.selected_in_order().iter().map(|c| c.id).collect();
        if ids.is_empty() {
            CommandOutcome::NotApplicable(none.into())
        } else {
            CommandOutcome::Handled(vec![workflow_cmd(make(ids))])
        }
    }
}
