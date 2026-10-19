//! The flat engine graph of a project: the [`atomik_model::ExpandedRack`]
//! as a [`Patch`] the shared builder understands.
//!
//! Containers vanish in [`atomik_model::RackDoc::expand`]; what is left is a list of devices
//! keyed by flat **path keys** (`layer/synth`, `layer/~split`) and a list of cables between them.
//! A patch device id must match `[a-z][a-z0-9_-]*`, which a path key does not, so inner devices
//! get a generated id (`in<N>-<sanitised key>`, unique among all ids) and top-level devices keep
//! their key. The engine id of a device is its position in the list (1-based), never its patch id,
//! so the generated ids carry no sound.

use crate::patch::{CableSpec, DeviceSpec};
use crate::patch::{FORMAT_TAG, FORMAT_VERSION_V2, Patch};
use atomik_model::{DeviceKey, ExpandedRack, INTERFACE_KEY, RackDoc, SAVE_LENGTH_BEATS};
use std::collections::{BTreeMap, BTreeSet};

/// Flat key to patch id for every expanded device (`interface` is not listed).
pub type PatchIds = BTreeMap<DeviceKey, String>;

/// The patch of an expanded rack and the patch id of each flat device. The header comes from
/// `doc` as [`atomik_model::save_patch`] writes it, so a container-free document builds exactly
/// the patch `save_patch` + [`Patch::from_json`] gave before containers.
pub fn expanded_patch(doc: &RackDoc, expanded: &ExpandedRack) -> (Patch, PatchIds) {
    let mut taken: BTreeSet<String> = expanded
        .devices
        .iter()
        .filter(|d| is_plain(&d.key.0))
        .map(|d| d.key.0.clone())
        .collect();
    let mut ids = PatchIds::new();
    for (n, d) in expanded.devices.iter().enumerate() {
        let id = if is_plain(&d.key.0) {
            d.key.0.clone()
        } else {
            let base: String = d
                .key
                .0
                .chars()
                .map(|c| {
                    if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_' {
                        c
                    } else {
                        '-'
                    }
                })
                .collect();
            let mut id = format!("in{}-{base}", n + 1);
            while taken.contains(&id) {
                id.push('x');
            }
            taken.insert(id.clone());
            id
        };
        ids.insert(d.key.clone(), id);
    }
    let end = |r: &atomik_model::SocketRef| {
        let device = if r.device.as_str() == INTERFACE_KEY {
            INTERFACE_KEY.to_string()
        } else {
            ids.get(&r.device)
                .cloned()
                .unwrap_or_else(|| r.device.0.clone())
        };
        format!("{device}:{}", r.socket)
    };
    let patch = Patch {
        format: FORMAT_TAG.to_string(),
        version: FORMAT_VERSION_V2,
        sample_rate: doc.sample_rate,
        tempo_bpm: doc.tempo_bpm,
        seed: doc.seed,
        length_beats: SAVE_LENGTH_BEATS,
        output_channels: doc.output_channels,
        devices: expanded
            .devices
            .iter()
            .map(|d| DeviceSpec {
                id: ids[&d.key].clone(),
                device_type: d.type_id.clone(),
                params: d
                    .params
                    .iter()
                    .map(|(k, v)| (k.clone(), f64::from(*v)))
                    .collect(),
            })
            .collect(),
        cables: expanded
            .cables
            .iter()
            .map(|c| CableSpec {
                from: end(&c.from),
                to: end(&c.to),
            })
            .collect(),
        notes: Vec::new(),
    };
    (patch, ids)
}

/// True for a top-level key (no path separator, not a synthetic one).
fn is_plain(key: &str) -> bool {
    !key.contains('/') && !key.contains('~')
}
