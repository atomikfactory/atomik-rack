//! The EQ analyser switch in the app core.
//!
//! The 16-band analyser of the EQ (and the two of the dynamic EQ) costs more than the bands
//! themselves, and only a panel that shows the display needs it. Once per frame the core asks the
//! rack view which EQ and dynamic EQ panels are on screen ([`atomik_ui::RackView::visible_panels`])
//! and keeps `internal.analyser_live` of exactly those devices at 1, every other one at 0 (the
//! engine's default). The value is sent only when it
//! changed, again after an engine restart, and again for a device that was added anew (a new
//! engine id). It is UI state: never stored, not undoable, and the audio output does not depend
//! on it.

use super::*;
use atomik_ui::fx::FxKind;
use std::collections::BTreeMap;

/// The `internal.analyser_live` param of the EQ.
const ANALYSER_LIVE_PARAM: &str = "internal.analyser_live";

/// The effects with an analyser switch, in the order of [`EqLive::sent`].
const ANALYSER_KINDS: [FxKind; 2] = [FxKind::Eq, FxKind::DynamicEq];

/// What the engine was told per EQ and dynamic EQ: its engine id and whether the analyser runs.
#[derive(Debug, Default)]
pub(super) struct EqLive {
    sent: [BTreeMap<DeviceKey, (DeviceInstanceId, bool)>; ANALYSER_KINDS.len()],
}

impl EqLive {
    /// A new engine knows nothing of what was sent.
    pub(super) fn restart(&mut self) {
        for sent in &mut self.sent {
            sent.clear();
        }
    }
}

impl<S: EngineSink> AppCore<S> {
    /// Switches the analyser on for the EQ panels in view and off for the others (module docs);
    /// once per frame from [`Self::frame`]. Allocation-light: one key list per frame, and only
    /// while an EQ exists.
    pub(super) fn feed_eq_analysers(&mut self) {
        for (slot, kind) in ANALYSER_KINDS.into_iter().enumerate() {
            self.feed_analyser_switch(slot, kind);
        }
    }

    fn feed_analyser_switch(&mut self, slot: usize, kind: FxKind) {
        let shown = self.view.visible_panels(kind);
        if shown.is_empty() && self.eq_live.sent[slot].is_empty() {
            return;
        }
        let Some(pid) = self
            .registry
            .get(kind.type_id())
            .and_then(|f| f.descriptor.param_by_key(ANALYSER_LIVE_PARAM))
            .map(|p| p.id)
        else {
            return;
        };
        // Off for what left the view (or the rack); a device that is gone takes its entry.
        let mut off: Vec<(DeviceKey, DeviceInstanceId)> = Vec::new();
        self.eq_live.sent[slot].retain(|key, (id, on)| {
            if self.ids.get(key) != Some(id) {
                return false;
            }
            if *on && !shown.contains(key) {
                off.push((key.clone(), *id));
            }
            true
        });
        for (key, id) in off {
            let result = self.sink.set_param(id, pid, 0.0);
            self.engine_result(result);
            self.eq_live.sent[slot].insert(key, (id, false));
        }
        for key in shown {
            let Some(id) = self.ids.get(&key).copied() else {
                continue;
            };
            if self.eq_live.sent[slot].get(&key) == Some(&(id, true)) {
                continue;
            }
            let result = self.sink.set_param(id, pid, 1.0);
            self.engine_result(result);
            self.eq_live.sent[slot].insert(key, (id, true));
        }
    }
}
