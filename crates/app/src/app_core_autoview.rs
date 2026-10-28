//! Controls that follow automation: the value a control shows for a param a lane plays.
//!
//! The document holds one value per param; the song player drives the param to the lane's value
//! at every batch without touching the document. The views therefore show an **overlay**: the
//! lane's value at the transport position ([`atomik_model::RackDoc::automation_values_at`], the
//! player's own rule, pinned bit for bit by a render test) for every param a playing lane
//! drives, the document's value for the rest. Playback never changes the document: no undo
//! entry, not dirty.
//!
//! * **Where it is computed.** On the UI thread, from the document and the transport position
//!   the engine published (the same `position` the playhead shows). The engine publishes
//!   nothing more and the audio thread is not involved, so there is nothing to keep realtime
//!   safe and no tempo map, clip loop or offset to replicate: the lanes are flattened by the
//!   code the engine's own input comes from. A stopped transport shows the value at the
//!   playhead, so a locate moves the controls too.
//! * **What the user holds.** A lane a take is writing (Overwrite from the start, Latch from
//!   the first touch, Touch while the control is held, see `app_core_autorec`) is left out, so
//!   the control shows the value being moved; on release in Touch it follows the lane again. With
//!   the transport stopped a param the user edited keeps their value until the playhead moves or
//!   the song plays (the audio holds that value too: the player applies lanes only while
//!   playing).
//! * **Cost.** One flatten per lane per frame while some lane exists; the views re-record a
//!   panel only when a value it shows changed (`set_shown_values` reports whether the set
//!   changed, and an unchanged set costs a comparison).

use super::autorec::LaneId;
use super::*;
use std::collections::BTreeSet;

/// What the app keeps to decide what the controls show.
#[derive(Debug, Default)]
pub(super) struct AutoView {
    /// Params the user edited since the playhead last moved or the song last played: they show
    /// the user's value, not the lane's.
    touched: BTreeSet<LaneId>,
    /// `(playing, tick)` of the last look: a change forgets the touched params.
    seen: Option<(bool, i64)>,
}

impl<S: EngineSink> AppCore<S> {
    /// A param was changed in the document by an edit (a drag, a text entry, an undo, a plugin
    /// window): with the song stopped it keeps its value on show until the playhead moves.
    pub(super) fn note_param_edit(&mut self, device: &DeviceKey, param: &str) {
        if !self.playing {
            self.autoview
                .touched
                .insert((device.clone(), param.to_owned()));
        }
    }

    /// The values the controls show instead of the stored ones now: what each playing lane gives
    /// at the transport position, without the lanes a take is writing and the params the user
    /// edited with the song stopped.
    pub fn automation_shown(&mut self) -> atomik_model::ShownValues {
        let playing = self.playing || self.sink.transport_state().playing;
        let seen = (playing, self.position);
        if self.autoview.seen != Some(seen) {
            self.autoview.seen = Some(seen);
            self.autoview.touched.clear();
        }
        let mut shown = self.doc().automation_values_at(self.position as f64);
        if shown.is_empty() {
            return shown;
        }
        for lane in &self.autoview.touched {
            shown.remove(lane);
        }
        for lane in self.writing_automation_lanes() {
            shown.remove(&lane);
        }
        shown
    }

    /// Hands the views the values to show. Returns whether any view's set changed (the caller
    /// then syncs the views).
    pub(super) fn update_shown_values(&mut self) -> bool {
        let shown = self.automation_shown();
        let a = self.view.set_shown_values(&shown);
        let b = self.console.set_shown_values(&shown);
        let c = self.plugin_params.set_shown_values(&shown);
        a || b || c
    }

    /// Per frame: moves the controls with the song. Only the views whose values changed are
    /// synced, and each re-records only the panels that show a changed value.
    pub(super) fn feed_automation_values(&mut self) {
        if !self.update_shown_values() {
            return;
        }
        let doc = self.document.doc();
        self.view.sync(doc, &self.registry);
        self.console.sync(doc, &self.registry);
        if self.plugin_params_open {
            self.plugin_params.sync(doc, &self.registry);
        }
    }
}
