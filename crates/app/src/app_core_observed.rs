//! The observed flag of the mastering device and the phase meter in the app core.
//!
//! Both devices measure loudness and true peak (the phase meter also the stereo image) only for
//! their panel. They own a [`atomik_device_sdk::ScopeTap`] that carries the flag
//! ([`atomik_device_sdk::ScopeTap::set_observed`]): on by default, so an offline render, a test and
//! the QA server never see it change. The windowed app ([`AppCore::set_analyse_only_shown`]) says
//! no for a device whose panel is not on screen ([`atomik_ui::RackView::is_panel_visible`]: not
//! folded, not scrolled out, not on the back face, not in a closed container, and the rack's window
//! neither minimized nor covered) and yes again as soon as it is, once per frame before the panel
//! paints. The flag is sent when it changes and again for a device with a new engine id; it is
//! not a parameter, so it is neither saved nor automated nor an undo step. What a device does with
//! it, and what the panel shows when it comes back, is in the device's docs.

use super::*;
use std::collections::BTreeMap;

/// The device types that carry the flag.
const OBSERVED_TYPES: [&str; 2] = ["atomik.mastering", "atomik.phase-meter"];

/// The devices of the rack that carry the flag and what the engine was told.
#[derive(Default)]
pub(super) struct Observed {
    devices: Vec<DeviceKey>,
    /// The flag as the engine has it: the device's engine id and the value (absent: the default,
    /// yes).
    sent: BTreeMap<DeviceKey, (DeviceInstanceId, bool)>,
}

impl Observed {
    /// Sets the devices of the new expansion and forgets what was sent to the ones that left.
    pub(super) fn set_devices(&mut self, expanded: &ExpandedRack) {
        self.devices = expanded
            .devices
            .iter()
            .filter(|d| !d.frozen && OBSERVED_TYPES.contains(&d.type_id.as_str()))
            .map(|d| d.key.clone())
            .collect();
        let devices = &self.devices;
        self.sent.retain(|k, _| devices.contains(k));
    }

    /// A new engine: its devices start observed.
    pub(super) fn restart(&mut self) {
        self.sent.clear();
    }

    /// The devices that carry the flag (flat keys).
    pub(super) fn devices(&self) -> &[DeviceKey] {
        &self.devices
    }
}

impl<S: EngineSink> AppCore<S> {
    /// Tells the devices that carry the flag whether a panel shows them (module docs); once per
    /// frame from [`Self::frame`]. Nothing is sent unless the app asked to analyse only what is
    /// shown. Allocation-free while nothing changes.
    pub(super) fn feed_observed(&mut self) {
        if !self.analyse_only_shown() {
            return;
        }
        let mut state = std::mem::take(&mut self.observed);
        for key in state.devices.iter() {
            let Some(id) = self.ids.get(key).copied() else {
                continue;
            };
            let want = self.view.is_panel_visible(key);
            let sent = state
                .sent
                .get(key)
                .filter(|(sent_id, _)| *sent_id == id)
                .is_none_or(|(_, v)| *v);
            if sent == want {
                continue;
            }
            if self.sink.set_device_observed(id, want) {
                state.sent.insert(key.clone(), (id, want));
            }
        }
        self.observed = state;
    }

    /// Whether the device at `key` was told that nothing shows it (and not told otherwise since).
    pub fn device_observed(&self, key: &DeviceKey) -> bool {
        let Some(id) = self.ids.get(key) else {
            return true;
        };
        self.observed
            .sent
            .get(key)
            .filter(|(sent_id, _)| sent_id == id)
            .is_none_or(|(_, v)| *v)
    }

    /// The devices that carry the observed flag (flat keys).
    pub fn observed_devices(&self) -> &[DeviceKey] {
        self.observed.devices()
    }
}
