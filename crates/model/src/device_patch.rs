//! Device patches: applying one device's saved sound to a device that is already in the rack.
//!
//! A patch is a [`DeviceSnapshot`] of exactly one device (the `.atomikpatch` library item of a
//! single device). [`WorkflowCommand::ApplyPatch`](crate::WorkflowCommand::ApplyPatch) makes the
//! target device take everything that makes up its sound: every param (a param the patch does
//! not store goes back to its default), zones, step patterns, curves, drum patterns, a plugin's
//! state and a container's inside. The device keeps its key, name, column, colour, fold state,
//! cables, track and console channel. The whole change is one undo step, and a device that took
//! a patch is, as far as the engine goes, the same as one created from it: the same params,
//! zones, patterns and state, so both bounce identically.
//!
//! [`WorkflowCommand::ResetDevice`](crate::WorkflowCommand::ResetDevice) is the same with the
//! patch of a new device ([`DeviceSnapshot::defaults`]).
//!
//! Params driven by a macro are left as they are (the macro owns them), and a patch whose
//! device type differs from the target's changes nothing.

use crate::console::is_console_type;
use crate::console_cmd::Planner;
use crate::container::{SPLIT_TYPE, new_device, split_path};
use crate::history::Edit;
use crate::plugin::{MAX_PLUGIN_STATE_BYTES, MAX_PROJECT_PLUGIN_STATE_BYTES};
use crate::{DeviceKey, DeviceSnapshot, ModelError, RackDevice, RackDoc, rules, zones};
use atomik_device_sdk::Registry;
use std::collections::BTreeSet;

/// Most characters of a patch name.
pub const MAX_PATCH_NAME_CHARS: usize = 64;

/// Checks a patch name as the project loader and the commands do: trimmed, 1 to
/// [`MAX_PATCH_NAME_CHARS`] characters, no control characters.
pub(crate) fn check_patch_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.trim() != name
        || name.chars().count() > MAX_PATCH_NAME_CHARS
        || name.chars().any(char::is_control)
    {
        return Err(format!(
            "a patch name must be trimmed and 1 to {MAX_PATCH_NAME_CHARS} characters, without control characters"
        ));
    }
    Ok(())
}

impl DeviceSnapshot {
    /// The patch of a new device of `type_id`: default params, the default patterns of a
    /// sequencer, an empty inside for a container, no zones, no plugin state, no patch name.
    /// What [`WorkflowCommand::ResetDevice`](crate::WorkflowCommand::ResetDevice) applies.
    pub fn defaults(registry: &Registry, type_id: &str) -> Result<Self, ModelError> {
        let desc = rules::descriptor(registry, type_id)?;
        Ok(Self {
            devices: vec![new_device(
                registry,
                desc,
                DeviceKey("device".to_owned()),
                desc.name.to_owned(),
            )],
            cables: Vec::new(),
            tracks: Vec::new(),
            assets: Default::default(),
        })
    }

    /// The only device of a one-device snapshot (a device patch), or `None`.
    pub fn single_device(&self) -> Option<&RackDevice> {
        match self.devices.as_slice() {
            [only] if self.tracks.is_empty() => Some(only),
            _ => None,
        }
    }
}

/// Plans `ApplyPatch`: `key` takes the sound of the only device of `snapshot`.
pub(crate) fn plan_apply(
    doc: &RackDoc,
    registry: &Registry,
    key: &DeviceKey,
    snapshot: &DeviceSnapshot,
) -> Result<Edit, ModelError> {
    let Some(patch) = snapshot.single_device() else {
        return Err(ModelError::BadScope(
            "a device patch holds exactly one device".into(),
        ));
    };
    if let Some(h) = snapshot
        .required_assets()
        .into_iter()
        .find(|h| !doc.assets.contains_key(h))
    {
        return Err(ModelError::UnknownKey(format!(
            "asset {h} is not in this project's sample pool"
        )));
    }
    plan_into(doc, registry, key, patch)
}

/// Plans `ResetDevice`: `key` goes back to the sound of a new device of its type.
pub(crate) fn plan_reset(
    doc: &RackDoc,
    registry: &Registry,
    key: &DeviceKey,
) -> Result<Edit, ModelError> {
    let target = rules::resolve(doc, key)?;
    let defaults = DeviceSnapshot::defaults(registry, &target.type_id)?;
    plan_apply(doc, registry, key, &defaults)
}

/// Plans `SetPatchName`: the name the patch strip shows (empty clears it).
pub(crate) fn plan_set_name(
    doc: &RackDoc,
    key: &DeviceKey,
    name: &str,
) -> Result<Edit, ModelError> {
    let device = rules::resolve(doc, key)?;
    if !name.is_empty() {
        check_patch_name(name).map_err(ModelError::BadScope)?;
    }
    if device.patch_name == name {
        return Err(ModelError::BadScope(format!(
            "{key} already has that patch name"
        )));
    }
    Ok(Edit::PatchName {
        key: key.clone(),
        old: device.patch_name.clone(),
        new: name.to_owned(),
    })
}

fn bits(v: Option<f32>) -> Option<u32> {
    v.map(f32::to_bits)
}

fn plan_into(
    doc: &RackDoc,
    registry: &Registry,
    key: &DeviceKey,
    patch: &RackDevice,
) -> Result<Edit, ModelError> {
    let target = rules::resolve(doc, key)?;
    if is_console_type(&target.type_id) || target.type_id == SPLIT_TYPE {
        return Err(ModelError::BadConsole(format!(
            "{} is a console device; it has no patches",
            target.name
        )));
    }
    if target.type_id != patch.type_id {
        return Err(ModelError::BadScope(format!(
            "the patch is for {}, not for {} ({})",
            patch.type_id, target.name, target.type_id
        )));
    }
    let desc = rules::descriptor(registry, &target.type_id)?;
    let mut p = Planner::new(doc, registry);

    // Params: the patch's stored values, every other param back at its default.
    let names: BTreeSet<&String> = target.params.keys().chain(patch.params.keys()).collect();
    for name in names {
        let Some(param) = desc.param_by_key(name) else {
            continue;
        };
        if crate::console::is_internal_param(name)
            || doc.bank_driver(key, name).is_some()
            || doc.macro_driver(key, name).is_some()
        {
            continue;
        }
        let old = target.params.get(name).copied();
        let new = patch.params.get(name).map(|v| param.clamp(*v));
        if bits(old) != bits(new) {
            p.push(Edit::Param {
                key: key.clone(),
                param: name.clone(),
                old,
                new,
                default: param.default,
            });
        }
    }

    // Zones.
    if target.zones != patch.zones {
        if !registry.takes_samples(&target.type_id) {
            return Err(ModelError::BadZones(format!(
                "{} does not take samples",
                target.name
            )));
        }
        zones::check_zones(&doc.assets, &patch.zones)
            .map_err(|(i, m)| ModelError::BadZones(format!("zone {i}: {m}")))?;
        p.push(Edit::Zones {
            key: key.clone(),
            old: target.zones.clone(),
            new: patch.zones.clone(),
        });
    }

    // Step patterns, curves and drum patterns.
    if target.steps != patch.steps {
        p.push(Edit::Steps {
            key: key.clone(),
            old: target.steps.clone(),
            new: patch.steps.clone(),
        });
    }
    if target.curves != patch.curves {
        p.push(Edit::Curves {
            key: key.clone(),
            old: target.curves.clone(),
            new: patch.curves.clone(),
        });
    }
    if target.drums != patch.drums {
        p.push(Edit::Drums {
            key: key.clone(),
            old: target.drums.clone(),
            new: patch.drums.clone(),
        });
    }

    // A plugin's state.
    if let Some(plugin) = &target.plugin {
        let new = patch.plugin.as_ref().and_then(|d| d.state.clone());
        if plugin.state != new {
            let len = new.as_ref().map_or(0, |s| s.0.len() as u64);
            if len > MAX_PLUGIN_STATE_BYTES {
                return Err(ModelError::BadPlugin(format!(
                    "the state of {key} is {len} bytes; the limit is 16 MiB per plugin"
                )));
            }
            let total = doc.plugin_state_bytes(Some(key)) + len;
            if total > MAX_PROJECT_PLUGIN_STATE_BYTES {
                return Err(ModelError::BadPlugin(format!(
                    "the plugin states of the project would total {total} bytes; the limit is 128 MiB"
                )));
            }
            p.push(Edit::PluginState {
                key: key.clone(),
                old: plugin.state.clone(),
                new,
            });
        }
    }

    // The name.
    if target.patch_name != patch.patch_name {
        p.push(Edit::PatchName {
            key: key.clone(),
            old: target.patch_name.clone(),
            new: patch.patch_name.clone(),
        });
    }

    // A container's inside is replaced by taking the device out and putting the new one back
    // in its place: the engine rebuilds the whole inside either way.
    if target.inner != patch.inner {
        replace_inside(&mut p, key, patch)?;
    }

    let edit = p.finish();
    if matches!(&edit, Edit::Compound(edits) if edits.is_empty()) {
        return Err(ModelError::BadScope(format!(
            "{key} already has that sound"
        )));
    }
    Ok(edit)
}

/// Swaps the inside of the container `key` for the inside of `patch`.
fn replace_inside(
    p: &mut Planner<'_>,
    key: &DeviceKey,
    patch: &RackDevice,
) -> Result<(), ModelError> {
    let (parent, local) = split_path(key);
    let old = rules::resolve(p.doc(), key)?.clone();
    let mut new = old.clone();
    new.inner.clone_from(&patch.inner);
    let (index, cables) = match &parent {
        None => (
            rules::index_of(p.doc(), &local)?,
            p.doc()
                .cables
                .iter()
                .filter(|c| c.from.device == local || c.to.device == local)
                .cloned()
                .collect::<Vec<_>>(),
        ),
        Some(c) => {
            let inner = p
                .doc()
                .inner(c)
                .ok_or_else(|| ModelError::UnknownDevice(c.0.clone()))?;
            (
                inner
                    .devices
                    .iter()
                    .position(|d| d.key == local)
                    .ok_or_else(|| ModelError::UnknownDevice(key.0.clone()))?,
                inner
                    .cables
                    .iter()
                    .filter(|x| x.from.device == local || x.to.device == local)
                    .cloned()
                    .collect(),
            )
        }
    };
    match parent {
        None => {
            p.push(Edit::Remove {
                index,
                device: old,
                cables: cables.clone(),
            });
            p.push(Edit::Insert {
                index,
                device: new,
                cables,
            });
        }
        Some(container) => {
            p.push(Edit::InnerRemove {
                container: container.clone(),
                index,
                device: old,
                cables: cables.clone(),
            });
            p.push(Edit::InnerInsert {
                container,
                index,
                device: new,
                cables,
            });
        }
    }
    p.doc().check_containers(p.registry)
}
