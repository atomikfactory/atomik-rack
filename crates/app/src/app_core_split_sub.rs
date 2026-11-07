//! Split Sub in the app core: **Edit > Split Sub** (and the rack's device menu) puts the selected
//! Mono Bass into a container with a mono sine sub layer underneath it and a low cut on the bass
//! itself ([`atomik_model::WorkflowCommand::SplitSub`] has the rules), in one undo step. The
//! container is selected afterwards and the status line says what was built and what had to be
//! left behind (the automation lanes of the track).
//!
//! The target is the Mono Bass that is selected in the rack, else the one whose track is selected
//! in the arrangement. The split frequency is [`DEFAULT_SPLIT_HZ`]; the container's EQ and the sub
//! layer's low-pass are ordinary devices to adjust afterwards.

use super::*;
use atomik_model::{DEFAULT_SPLIT_HZ, SPLIT_SUB_TYPE, WorkflowCommand};

impl<S: EngineSink> AppCore<S> {
    /// The Mono Bass of the rack that Split Sub would work on: the selected one, else the one
    /// whose track is selected.
    fn split_sub_target(&self) -> Option<DeviceKey> {
        let doc = self.doc();
        let is_bass = |k: &DeviceKey| {
            !k.0.contains('/')
                && doc.device(k).is_some_and(|d| d.type_id == SPLIT_SUB_TYPE)
                && doc.song.track(k).is_some()
        };
        if self.view.scope_depth() == 0
            && let Some(k) = self.view.selection().into_iter().find(is_bass)
        {
            return Some(k);
        }
        self.arrangement
            .selected_track()
            .filter(|k| is_bass(k))
            .cloned()
    }

    /// Whether `device` is a top-level Mono Bass with a track.
    pub fn can_split_sub(&self, device: &DeviceKey) -> bool {
        let doc = self.doc();
        !device.0.contains('/')
            && doc
                .device(device)
                .is_some_and(|d| d.type_id == SPLIT_SUB_TYPE)
            && doc.song.track(device).is_some()
    }

    /// Whether Split Sub can work on the selection (a Mono Bass with a track is selected).
    pub fn split_sub_menu(&self) -> bool {
        self.split_sub_target().is_some()
    }

    /// Splits the selected Mono Bass (see the module docs). False, with the reason in the status
    /// line, when no Mono Bass is selected or the model refuses.
    pub fn split_sub(&mut self) -> bool {
        let Some(device) = self.split_sub_target() else {
            self.reject("Select a Mono Bass track to split its sub".into());
            return false;
        };
        let name = self
            .doc()
            .device(&device)
            .map_or_else(|| device.0.clone(), |d| d.name.clone());
        let notes = self
            .doc()
            .split_sub_notes(&self.registry, &device, DEFAULT_SPLIT_HZ)
            .unwrap_or_default();
        // The container takes the track of the bass, so it is the track that is new afterwards.
        let before: Vec<DeviceKey> = self
            .doc()
            .song
            .tracks
            .iter()
            .map(|t| t.device.clone())
            .collect();
        let command = Command::Workflow(WorkflowCommand::SplitSub {
            device,
            freq: DEFAULT_SPLIT_HZ,
        });
        if self.apply_command(command).is_err() {
            return false;
        }
        let created: Vec<DeviceKey> = self
            .doc()
            .song
            .tracks
            .iter()
            .map(|t| t.device.clone())
            .filter(|k| !before.contains(k))
            .collect();
        self.view.set_selection(&created, created.last());
        self.after_view_change();
        let mut message = format!(
            "Split Sub: {name} has a mono sine sub below {DEFAULT_SPLIT_HZ:.0} Hz and its bass is cut under it"
        );
        for note in notes {
            message.push_str(". ");
            message.push_str(&note);
        }
        self.reject(message);
        true
    }
}
