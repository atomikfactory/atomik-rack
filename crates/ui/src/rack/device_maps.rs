//! Per-device view state that outlives a rebuild: the selected drum pad, the loop slicer
//! sensitivity, the edited pattern, the drum generator page and cell mode, and the ids of the
//! cached panel lists. A device that leaves the document takes its entries with it, so a long
//! session that adds and removes devices does not grow these maps.

use super::RackView;
use atomik_model::{BOUNDARY_KEY, DeviceKey, INTERFACE_KEY, RackDevice, RackDoc};
use std::collections::BTreeSet;

/// Every key `devices` and the containers inside them go by: the local key (what the maps of a
/// scope use) and the path key from the top level (what the document and the cache ids use).
fn collect(devices: &[RackDevice], parent: Option<&str>, out: &mut BTreeSet<DeviceKey>) {
    for d in devices {
        out.insert(d.key.clone());
        let path = match parent {
            Some(p) => format!("{p}/{}", d.key.0),
            None => d.key.0.clone(),
        };
        out.insert(DeviceKey(path.clone()));
        if let Some(inner) = d.inner.as_deref() {
            collect(&inner.devices, Some(&path), out);
        }
    }
}

/// The keys of every device in `doc`, at every depth (see [`collect`]).
pub fn known_keys(doc: &RackDoc) -> BTreeSet<DeviceKey> {
    let mut out = BTreeSet::new();
    collect(&doc.devices, None, &mut out);
    out
}

impl RackView {
    /// Forgets the view state of devices that no longer exist in `doc`.
    pub(super) fn prune_device_maps(&mut self, doc: &RackDoc) {
        let known = known_keys(doc);
        self.pads.retain(|k, _| known.contains(k));
        self.sens.retain(|k, _| known.contains(k));
        self.edits.retain(|k, _| known.contains(k));
        self.pages.retain(|k, _| known.contains(k));
        self.fx_modes.retain(|k, _| known.contains(k));
        // The interface and boundary panels are not devices of the document: keep their ids.
        self.cache_ids.retain(|k, _| {
            let last = k.0.rsplit('/').next().unwrap_or("");
            known.contains(k) || last == INTERFACE_KEY || last.starts_with(BOUNDARY_KEY)
        });
    }

    /// How many per-device entries the view keeps (for tests of the pruning).
    pub fn device_state_len(&self) -> usize {
        self.pads.len()
            + self.sens.len()
            + self.edits.len()
            + self.pages.len()
            + self.fx_modes.len()
            + self.cache_ids.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Theme;
    use atomik_model::DeviceKey;

    fn key(k: &str) -> DeviceKey {
        DeviceKey(k.to_string())
    }

    fn doc_of(devices: Vec<RackDevice>) -> RackDoc {
        RackDoc {
            sample_rate: 48_000,
            tempo_bpm: 120.0,
            seed: 0,
            output_channels: 2,
            input_channels: 0,
            assets: Default::default(),
            devices,
            columns: 1,
            cables: Vec::new(),
            song: atomik_model::empty_song(120.0).into(),
            console: atomik_model::Console::default(),
            midi_maps: Vec::new(),
            macros: Default::default(),
            info: Default::default(),
            tune_cents: 0.0,
            sort_mode: atomik_model::RackSort::Free,
            references: Vec::new(),
        }
    }

    fn device(k: &str, inner: Option<Vec<RackDevice>>) -> RackDevice {
        let mut d = RackDevice {
            key: key(k),
            type_id: "atomik.stereo-delay".to_string(),
            name: k.to_string(),
            params: Default::default(),
            folded: false,
            color: None,
            zones: Vec::new(),
            steps: None,
            curves: None,
            drums: None,
            inner: None,
            plugin: None,
            column: 0,
            patch_name: String::new(),
            midi_out: None,
            mods: Vec::new(),
        };
        if let Some(devices) = inner {
            d.inner = Some(Box::new(atomik_model::ContainerDoc {
                devices,
                ..Default::default()
            }));
        }
        d
    }

    #[test]
    fn known_keys_cover_every_depth_by_local_and_path_key() {
        let doc = doc_of(vec![
            device("a", None),
            device("group", Some(vec![device("b", None)])),
        ]);
        let known = known_keys(&doc);
        for k in ["a", "group", "b", "group/b"] {
            assert!(known.contains(&key(k)), "{k}");
        }
        assert!(!known.contains(&key("gone")));
    }

    #[test]
    fn a_removed_device_takes_its_view_state_with_it() {
        let mut doc = doc_of(vec![device("a", None), device("b", None)]);
        let mut v = RackView::new(Theme::dark());
        for k in ["a", "b"] {
            v.pads.insert(key(k), 3);
            v.sens.insert(key(k), 40);
            v.edits.insert(key(k), 1);
            v.pages.insert(key(k), 1);
            v.fx_modes.insert(key(k), 1);
            v.cache_ids.insert(key(k), 9);
        }
        v.cache_ids.insert(key(INTERFACE_KEY), 1);
        v.prune_device_maps(&doc);
        assert_eq!(v.device_state_len(), 2 * 6 + 1);
        doc.devices.pop();
        v.prune_device_maps(&doc);
        assert_eq!(
            v.device_state_len(),
            6 + 1,
            "only `a` and the interface remain"
        );
        assert!(!v.pads.contains_key(&key("b")));
    }
}
