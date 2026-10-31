//! Copy and paste of channel settings, reset and clear inserts, on the strips a pane selects.
//!
//! The copied settings stay in the core (not on the system clipboard: they mean nothing outside
//! the app) until the next copy; they survive New and Open within one run, like the clipboard of
//! devices and clips.

use super::*;
use crate::commands::SettingsSection;
use atomik_model::{ChannelSettingsClip, ConsoleCommand};

impl<S: EngineSink> AppCore<S> {
    /// Whether channel settings have been copied.
    pub fn has_copied_settings(&self) -> bool {
        self.settings_clip.is_some()
    }

    /// The copied channel settings, for tests.
    pub fn copied_settings(&self) -> Option<&ChannelSettingsClip> {
        self.settings_clip.as_ref()
    }

    /// Mixer > Copy Channel Settings: the settings of `sections` of the first selected strip.
    /// False, with a message, when nothing is selected.
    pub fn copy_strip_settings(&mut self, pane: Pane, sections: SettingsSection) -> bool {
        let Some(strip) = self.strips_for_vca(pane).into_iter().next() else {
            self.reject("Select a channel to copy the settings of".into());
            return false;
        };
        match self
            .doc()
            .copy_channel_settings(&self.registry, &strip, sections.scope())
        {
            Ok(clip) => {
                self.settings_clip = Some(clip);
                true
            }
            Err(e) => {
                self.reject(e.to_string());
                false
            }
        }
    }

    /// Mixer > Paste Channel Settings: the copied settings onto every selected strip, in one
    /// undo step. False, with a message, when nothing was copied or selected, or the model
    /// refuses.
    pub fn paste_strip_settings(&mut self, pane: Pane) -> bool {
        let Some(clip) = self.settings_clip.clone() else {
            self.reject("Copy the settings of a channel first".into());
            return false;
        };
        let strips = self.strips_for_vca(pane);
        if strips.is_empty() {
            self.reject("Select one or more channels to paste the settings onto".into());
            return false;
        }
        self.apply_command(Command::Console(ConsoleCommand::PasteSettings {
            strips,
            clip,
        }))
        .is_ok()
    }

    /// Mixer > Reset Channel: every selected strip back to a fresh one, its routing kept, in one
    /// undo step. False, with a message, when nothing is selected.
    pub fn reset_strips(&mut self, pane: Pane) -> bool {
        let strips = self.strips_for_vca(pane);
        if strips.is_empty() {
            self.reject("Select one or more channels to reset".into());
            return false;
        }
        let commands = strips
            .into_iter()
            .map(|strip| {
                Command::Console(ConsoleCommand::ResetStrip {
                    strip,
                    keep_routing: true,
                })
            })
            .collect();
        self.apply_commands_as_one(commands)
    }

    /// Mixer > Clear Inserts: the insert chains of the selected strips emptied, in one undo
    /// step. False, with a message, when nothing is selected.
    pub fn clear_inserts(&mut self, pane: Pane) -> bool {
        let strips = self.strips_for_vca(pane);
        if strips.is_empty() {
            self.reject("Select one or more channels to clear the inserts of".into());
            return false;
        }
        let commands = strips
            .into_iter()
            .map(|strip| Command::Console(ConsoleCommand::ClearInserts { strip }))
            .collect();
        self.apply_commands_as_one(commands)
    }

    /// Applies `commands` one after the other as a single undo step (the first is a step, the
    /// rest are folded into it). A refusal stops the run; what was applied stays one step.
    fn apply_commands_as_one(&mut self, commands: Vec<Command>) -> bool {
        let depth = self.document.undo_depth();
        for command in commands {
            // Folded only once a step of this run exists to fold into.
            self.join_next = self.document.undo_depth() > depth;
            let result = self.apply_command(command);
            self.join_next = false;
            if result.is_err() {
                return false;
            }
        }
        true
    }
}
