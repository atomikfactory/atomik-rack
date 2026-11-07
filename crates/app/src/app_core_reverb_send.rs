//! The reverb send wizard in the app core: **Track > Add Reverb Send...** gives the selected track
//! a send to a new FX return with an optional de-esser, an EQ, and a Space Reverb that is keyed by
//! the track ([`atomik_model::WorkflowCommand::AddReverbSend`] has the rules), in one undo step.
//! The new reverb is selected afterwards and the status line says what was built.
//!
//! The target is the track whose device is selected in the rack (a track with a console channel),
//! else the selected track of the arrangement. The wizard panel keeps its choices between
//! openings; [`AppCore::reverb_send_info`] tells it the track and the decay the track suggests.

use super::*;
use atomik_model::reverb_send::{ReverbSendOptions, reverb_of_return, suggested_decay};
use atomik_model::{WorkflowCommand, first_free_send_slot};

impl<S: EngineSink> AppCore<S> {
    /// The device of the track the wizard would work on: a top-level track with a console channel
    /// that is selected in the rack, else the selected track of the arrangement.
    fn reverb_send_target(&self) -> Option<DeviceKey> {
        let doc = self.doc();
        let fits = |k: &DeviceKey| {
            !k.0.contains('/') && doc.song.track(k).is_some() && doc.console.channel_of(k).is_some()
        };
        if self.view.scope_depth() == 0
            && let Some(k) = self.view.selection().into_iter().find(fits)
        {
            return Some(k);
        }
        self.arrangement
            .selected_track()
            .filter(|k| fits(k))
            .cloned()
    }

    /// Whether the wizard can work: a track is selected and a send/return slot is free.
    pub fn reverb_send_menu(&self) -> bool {
        self.reverb_send_target().is_some() && first_free_send_slot(self.doc()).is_some()
    }

    /// The name of the track the wizard works on and the decay it suggests for it (a `vocal` track
    /// has a lower cap), for the panel. `None` when no track is selected.
    pub fn reverb_send_info(&self, vocal: bool) -> Option<(String, f32)> {
        let device = self.reverb_send_target()?;
        let doc = self.doc();
        let name = doc
            .device(&device)
            .map_or_else(|| device.0.clone(), |d| d.name.clone());
        let decay = suggested_decay(&doc.song, doc.song.track(&device)?, vocal);
        Some((name, decay))
    }

    /// Adds the reverb send of the selected track with `options` (see the module docs). False,
    /// with the reason in the status line, when no track is selected or the model refuses.
    pub fn add_reverb_send(&mut self, options: ReverbSendOptions) -> bool {
        let Some(track) = self.reverb_send_target() else {
            self.reject("Select a track to give it a reverb send".into());
            return false;
        };
        let name = self
            .doc()
            .device(&track)
            .map_or_else(|| track.0.clone(), |d| d.name.clone());
        let Some(slot) = first_free_send_slot(self.doc()) else {
            self.reject(atomik_model::SEND_SLOTS_FULL.into());
            return false;
        };
        let (vocal, duck, pre) = (options.vocal, options.duck, options.pre_fader);
        let decay = options.decay.unwrap_or_else(|| {
            let doc = self.doc();
            doc.song
                .track(&track)
                .map_or(atomik_model::reverb_send::DEFAULT_DECAY, |t| {
                    suggested_decay(&doc.song, t, vocal)
                })
        });
        let command = Command::Workflow(WorkflowCommand::AddReverbSend { track, options });
        if self.apply_command(command).is_err() {
            return false;
        }
        if let Some(reverb) = reverb_of_return(self.doc(), slot) {
            self.view
                .set_selection(std::slice::from_ref(&reverb), Some(&reverb));
            self.after_view_change();
        }
        self.reject(format!(
            "Reverb send: {name} sends {} to return {}, {}decay {decay:.1} s{}",
            if pre { "pre-fader" } else { "post-fader" },
            slot + 1,
            if vocal { "de-esser, EQ, " } else { "EQ, " },
            if duck { ", ducked by the track" } else { "" },
        ));
        true
    }
}
