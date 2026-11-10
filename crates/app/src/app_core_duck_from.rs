//! Quick sidechain in the app core: **Duck from** a track, **Gentle Pump** and **Remove Ducking**
//! ([`atomik_model::duck_from`] has the rules), each one undo step with a toast that names the
//! knob to turn.
//!
//! The target is the track whose device is selected in the rack, else the selected track of the
//! arrangement; a menu opened on a track or a device names its own target, so the rows it shows
//! and the command it runs always agree. Ducking from a track the target already ducks from
//! changes nothing: the device is selected and the toast says so.

use super::*;
use crate::commands::DuckGate;
use atomik_model::WorkflowCommand;
use atomik_model::duck_from::{DuckSource, MENU_ROWS, duck_sources, duckers_of, ducking_from};

/// What the quick sidechain menus list for one track.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DuckMenu {
    /// The tracks it can duck from, in song order, the ones it already ducks from marked.
    pub sources: Vec<DuckSource>,
    /// The track has a ducker or a gentle pump.
    pub has_ducking: bool,
    /// The track can take a gentle pump (it has none yet).
    pub can_pump: bool,
}

impl DuckMenu {
    /// The most rows a menu lists before it offers More (see [`MENU_ROWS`]).
    pub const ROWS: usize = MENU_ROWS;
}

impl<S: EngineSink> AppCore<S> {
    /// Whether `k` is a top-level track with a console channel.
    fn duck_fits(&self, k: &DeviceKey) -> bool {
        let doc = self.doc();
        !k.0.contains('/') && doc.song.track(k).is_some() && doc.console.channel_of(k).is_some()
    }

    /// The track a quick sidechain command works on when no menu named one: the device selected
    /// in the rack, else the selected track of the arrangement.
    pub fn duck_target(&self) -> Option<DeviceKey> {
        self.duck_rack_target()
            .or_else(|| self.duck_arranged_target())
    }

    /// The primary device of the rack when it is a track (the target of a rack device menu).
    pub fn duck_rack_target(&self) -> Option<DeviceKey> {
        if self.view.scope_depth() != 0 {
            return None;
        }
        self.view.primary().filter(|k| self.duck_fits(k))
    }

    /// The selected track of the arrangement when it can duck (the target of a track menu).
    pub fn duck_arranged_target(&self) -> Option<DeviceKey> {
        self.arrangement
            .selected_track()
            .filter(|k| self.duck_fits(k))
            .cloned()
    }

    /// The name of `key` for a message.
    fn duck_name(&self, key: &DeviceKey) -> String {
        self.doc()
            .device(key)
            .map_or_else(|| key.0.clone(), |d| d.name.clone())
    }

    /// The menu rows of the track `target`.
    pub fn duck_menu(&self, target: &DeviceKey) -> DuckMenu {
        if !self.duck_fits(target) {
            return DuckMenu::default();
        }
        let doc = self.doc();
        let registry = self.registry();
        let ducking = duckers_of(doc, registry, target);
        DuckMenu {
            sources: duck_sources(doc, registry, target),
            has_ducking: !ducking.is_empty(),
            can_pump: !ducking.iter().any(|d| d.pump),
        }
    }

    /// Which items of the Track menu work on the track a command would take.
    pub fn duck_gate(&self) -> DuckGate {
        let Some(target) = self.duck_target() else {
            return DuckGate::default();
        };
        let menu = self.duck_menu(&target);
        DuckGate {
            can_duck: !menu.sources.is_empty(),
            can_pump: menu.can_pump,
            has_ducking: menu.has_ducking,
        }
    }

    /// Selects `device` in the rack, so its panel shows the knob the toast names.
    fn duck_select(&mut self, device: &DeviceKey) {
        self.view
            .set_selection(std::slice::from_ref(device), Some(device));
        self.after_view_change();
    }

    /// Makes `target` (the track a command takes when `None`) duck from the track `source`, with
    /// the key before the source's fader when `pre_fader`. False, with the reason in the status
    /// line, when there is no target or the model refuses.
    pub fn duck_from(
        &mut self,
        target: Option<DeviceKey>,
        source: &DeviceKey,
        pre_fader: bool,
    ) -> bool {
        let Some(target) = target.or_else(|| self.duck_target()) else {
            self.reject("Select a track to make it duck".into());
            return false;
        };
        let (target_name, source_name) = (self.duck_name(&target), self.duck_name(source));
        if let Some(existing) = ducking_from(self.doc(), self.registry(), &target, source) {
            self.duck_select(&existing.device);
            self.reject(format!(
                "Already ducking from {source_name}. Adjust: Threshold on the Compressor."
            ));
            return true;
        }
        let command = Command::Workflow(WorkflowCommand::DuckFrom {
            target: target.clone(),
            source: source.clone(),
            pre_fader,
        });
        if self.apply_command(command).is_err() {
            return false;
        }
        if let Some(ducker) = ducking_from(self.doc(), self.registry(), &target, source) {
            self.duck_select(&ducker.device);
        }
        self.reject(format!(
            "{target_name} ducks under {source_name}. Adjust: Threshold on the Compressor."
        ));
        true
    }

    /// Puts a gentle pump on `target` (the track a command takes when `None`).
    pub fn gentle_pump(&mut self, target: Option<DeviceKey>) -> bool {
        let Some(target) = target.or_else(|| self.duck_target()) else {
            self.reject("Select a track to make it pump".into());
            return false;
        };
        let name = self.duck_name(&target);
        let command = Command::Workflow(WorkflowCommand::GentlePump {
            target: target.clone(),
        });
        if self.apply_command(command).is_err() {
            return false;
        }
        if let Some(pump) = atomik_model::duck_from::pump_of(self.doc(), self.registry(), &target) {
            self.duck_select(&pump.device);
        }
        self.reject(format!(
            "{name} pumps gently on every beat. Adjust: Depth on the Sidechain Tool."
        ));
        true
    }

    /// Takes the ducking off `target` (the track a command takes when `None`).
    pub fn remove_ducking(&mut self, target: Option<DeviceKey>) -> bool {
        let Some(target) = target.or_else(|| self.duck_target()) else {
            self.reject("Select a track to remove its ducking".into());
            return false;
        };
        let name = self.duck_name(&target);
        let command = Command::Workflow(WorkflowCommand::RemoveDucking { target });
        if self.apply_command(command).is_err() {
            return false;
        }
        self.reject(format!("Ducking removed from {name}."));
        true
    }
}
