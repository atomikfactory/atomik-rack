//! The restructuring commands of the [`ArrangementView`]: insert and remove bars at the loop
//! locators, reverse, scale tempo and chop clips. Each sends one workflow command (one undo
//! step); the planning and the rules are in the model.

use super::{ArrangementView, workflow_cmd};
use crate::keymap::CommandOutcome;
use atomik_model::{ClipId, WorkflowCommand};

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
            _ => None,
        }
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
