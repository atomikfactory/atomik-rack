//! Planning of the plugin commands: `Add`, `SetPinned`, `SetRunMode`,
//! `SetPower`, `Relink` (each one reversible [`Edit`], one undo step), and the two data syncs
//! that are not undo steps (`SyncState`, `SyncLayout`).

use crate::history::Edit;
use crate::plugin::{
    MAX_PINNED_PARAMS, MAX_PLUGIN_STATE_BYTES, MAX_PROJECT_PLUGIN_STATE_BYTES, PluginCommand,
    PluginDoc, check_matches_registered, key_base_of_name, plugin_device, validate_layout,
};
use crate::{DeviceKey, ModelError, RackDoc, rules};
use atomik_device_sdk::{DeviceKind, Registry};
use atomik_device_sdk::{PluginFormat, PluginLayout, PluginRef, PluginStateBlob};
use std::collections::BTreeSet;
use std::sync::Arc;

/// The part of a plugin device that a relink replaces (and that undo puts back).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RelinkData {
    pub(crate) type_id: String,
    pub(crate) format: PluginFormat,
    pub(crate) id: String,
    pub(crate) path_hint: Option<String>,
    pub(crate) layout: PluginLayout,
    pub(crate) pinned: Vec<u32>,
}

fn bad(message: impl Into<String>) -> ModelError {
    ModelError::BadPlugin(message.into())
}

fn layout_error(type_id: &str, f: crate::plugin::LayoutFault) -> ModelError {
    match f.row {
        Some(row) => bad(format!("{type_id} layout param row {row}: {}", f.message)),
        None => bad(format!("{type_id} layout: {}", f.message)),
    }
}

/// The error for a plugin type id of a generic command (`AddDevice`, `AddInner`, an insert
/// slot): plugins carry a layout and are added with [`PluginCommand::Add`].
pub(crate) fn refuse_generic_add(type_id: &str) -> Result<(), ModelError> {
    if atomik_device_sdk::is_plugin_type_id(type_id) {
        return Err(bad(format!(
            "{type_id} is a plugin: add it with PluginCommand::Add"
        )));
    }
    Ok(())
}

/// Plans one undoable plugin command. The two data syncs are not planned (see [`sync_state`]).
pub(crate) fn plan(
    doc: &RackDoc,
    registry: &Registry,
    command: PluginCommand,
) -> Result<Edit, ModelError> {
    match command {
        PluginCommand::Add {
            index,
            container,
            plugin,
            layout,
            path_hint,
            key,
            auto_split,
        } => add(
            doc, registry, index, container, &plugin, layout, path_hint, key, auto_split,
        ),
        PluginCommand::SetPinned { device, pinned } => set_pinned(doc, device, pinned),
        PluginCommand::SetRunMode {
            device,
            run_on_audio_thread,
        } => {
            let (_, p) = plugin_device(doc, &device)?;
            Ok(Edit::PluginRun {
                old: p.run_on_audio_thread,
                new: run_on_audio_thread,
                key: device,
            })
        }
        PluginCommand::SetPower { device, powered } => {
            let (_, p) = plugin_device(doc, &device)?;
            Ok(Edit::PluginPower {
                old: p.powered,
                new: powered,
                key: device,
            })
        }
        PluginCommand::Relink {
            device,
            plugin,
            layout,
            path_hint,
        } => relink(doc, registry, device, &plugin, layout, path_hint),
        PluginCommand::SyncState { .. } | PluginCommand::SyncLayout { .. } => {
            unreachable!("the data syncs are handled before planning")
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn add(
    doc: &RackDoc,
    registry: &Registry,
    index: usize,
    container: Option<DeviceKey>,
    plugin: &PluginRef,
    layout: PluginLayout,
    path_hint: Option<String>,
    key: Option<DeviceKey>,
    auto_split: bool,
) -> Result<Edit, ModelError> {
    let type_id = plugin.type_id();
    let desc = registry
        .descriptor(&type_id)
        .filter(|_| registry.is_plugin(&type_id))
        .ok_or_else(|| ModelError::UnknownType(type_id.clone()))?;
    validate_layout(plugin, &layout).map_err(|f| layout_error(&type_id, f))?;
    check_matches_registered(desc, &layout)?;
    let base = key_base_of_name(&layout.name);
    let pdoc = Box::new(PluginDoc::new(plugin, layout, path_hint));
    let make = move |key: DeviceKey, name: String| {
        let mut device = crate::container::new_device(registry, desc, key, name);
        device.plugin = Some(pdoc);
        device
    };
    match container {
        None => {
            let key = match key {
                Some(k) => {
                    rules::check_new_key(doc, &k.0)?;
                    k
                }
                None => rules::generate_key_with_base_in(&doc.devices, &base),
            };
            let device = make(key, rules::unique_name(doc, desc.name));
            Ok(crate::console_cmd::insert_with_channel(
                doc,
                registry,
                index,
                device,
                desc.kind == DeviceKind::Instrument,
            ))
        }
        Some(container) => crate::container_cmd::add_inner_device(
            doc,
            registry,
            container,
            index,
            desc,
            key,
            Some(&base),
            auto_split,
            make,
        ),
    }
}

fn set_pinned(doc: &RackDoc, device: DeviceKey, pinned: Vec<u32>) -> Result<Edit, ModelError> {
    let (_, p) = plugin_device(doc, &device)?;
    if pinned.len() > MAX_PINNED_PARAMS {
        return Err(bad(format!(
            "at most {MAX_PINNED_PARAMS} params can be pinned, found {}",
            pinned.len()
        )));
    }
    let mut seen = BTreeSet::new();
    for id in &pinned {
        if !seen.insert(*id) {
            return Err(bad(format!("param {id} is pinned twice")));
        }
        if !p.layout.params.iter().any(|row| row.id == *id) {
            return Err(bad(format!("param {id} is not in the layout of {device}")));
        }
    }
    Ok(Edit::PluginPins {
        old: p.pinned.clone(),
        new: pinned,
        key: device,
    })
}

// ----------------------------------------------------------------------------- relink

fn relink(
    doc: &RackDoc,
    registry: &Registry,
    device: DeviceKey,
    plugin: &PluginRef,
    layout: PluginLayout,
    path_hint: Option<String>,
) -> Result<Edit, ModelError> {
    let (dev, old) = plugin_device(doc, &device)?;
    let new_type = plugin.type_id();
    let new_desc = registry
        .descriptor(&new_type)
        .filter(|_| registry.is_plugin(&new_type))
        .ok_or_else(|| ModelError::UnknownType(new_type.clone()))?;
    validate_layout(plugin, &layout).map_err(|f| layout_error(&new_type, f))?;
    check_matches_registered(new_desc, &layout)?;
    let old_desc = rules::descriptor(registry, &dev.type_id)?;
    if old_desc.kind != new_desc.kind {
        return Err(bad(format!(
            "cannot relink {device} ({}, {:?}) to {new_type} ({:?}): the kinds differ",
            dev.type_id, old_desc.kind, new_desc.kind
        )));
    }
    let local = crate::split_path(&device).1;
    let scope_cables = match crate::split_path(&device).0 {
        None => Some(doc.cables.as_slice()),
        Some(c) => doc.inner(&c).map(|i| i.cables.as_slice()),
    }
    .unwrap_or(&[]);
    for c in scope_cables {
        for end in [&c.from, &c.to] {
            if end.device != local {
                continue;
            }
            let ok = new_desc.socket_by_key(&end.socket).is_some_and(|s| {
                old_desc
                    .socket_by_key(&end.socket)
                    .is_some_and(|o| o.kind == s.kind && o.direction == s.direction)
            });
            if !ok {
                return Err(bad(format!(
                    "{device}:{} is cabled and {new_type} has no such socket",
                    end.socket
                )));
            }
        }
    }
    // The song track and its automation lanes (top-level devices only).
    if let Some(track) = doc.song.track(&device) {
        if crate::container::notes_input(new_desc).is_none() {
            return Err(bad(format!(
                "{device} has a song track and {new_type} has no Notes input"
            )));
        }
        for lane in &track.lanes {
            if let crate::LaneKind::Automation(param) = &lane.kind
                && new_desc.param_by_key(param).is_none()
            {
                return Err(bad(format!(
                    "{device} {param} is automated and {new_type} has no such param"
                )));
            }
        }
    }
    // Macro targets that name the device, in any container above it.
    for (container, index, param) in macro_targets_of(doc, &device) {
        if new_desc.param_by_key(&param).is_none() {
            return Err(bad(format!(
                "{device} {param} is driven by macro {} of {container} and {new_type} has no such param",
                index + 1
            )));
        }
    }
    let pinned = old
        .pinned
        .iter()
        .copied()
        .filter(|id| layout.params.iter().any(|row| row.id == *id))
        .collect();
    Ok(Edit::PluginRelink {
        old: Box::new(RelinkData {
            type_id: dev.type_id.clone(),
            format: old.format,
            id: old.id.clone(),
            path_hint: old.path_hint.clone(),
            layout: old.layout.clone(),
            pinned: old.pinned.clone(),
        }),
        new: Box::new(RelinkData {
            type_id: new_type,
            format: plugin.format,
            id: plugin.id.clone(),
            path_hint,
            layout,
            pinned,
        }),
        key: device,
    })
}

/// `(container path, macro index, param)` of every macro target that names `device` (a path
/// key), in the containers above it.
fn macro_targets_of(doc: &RackDoc, device: &DeviceKey) -> Vec<(DeviceKey, u8, String)> {
    let segments: Vec<&str> = device.0.split('/').collect();
    let mut out = Vec::new();
    for depth in 1..segments.len() {
        let container = DeviceKey(segments[..depth].join("/"));
        let relative = segments[depth..].join("/");
        let Some(inner) = doc.inner(&container) else {
            continue;
        };
        for (i, m) in inner.macros.iter().enumerate() {
            for t in &m.targets {
                if t.device.0 == relative {
                    out.push((container.clone(), i as u8, t.param.clone()));
                }
            }
        }
    }
    out
}

// ----------------------------------------------------------------------------- data syncs

/// `PluginCommand::SyncState`: stores the blob (not an undo step).
pub(crate) fn sync_state(
    doc: &mut RackDoc,
    device: &DeviceKey,
    state: PluginStateBlob,
) -> Result<(), ModelError> {
    plugin_device(doc, device)?;
    let len = state.0.len() as u64;
    if len > MAX_PLUGIN_STATE_BYTES {
        return Err(bad(format!(
            "the state of {device} is {len} bytes; the limit is 16 MiB per plugin"
        )));
    }
    let others = doc.plugin_state_bytes(Some(device));
    if others + len > MAX_PROJECT_PLUGIN_STATE_BYTES {
        return Err(bad(format!(
            "the plugin states of the project would total {} bytes; the limit is 128 MiB",
            others + len
        )));
    }
    let p = doc
        .resolve_mut(device)
        .and_then(|d| d.plugin.as_mut())
        .expect("checked above");
    p.state = Some(Arc::new(state));
    Ok(())
}

/// `PluginCommand::SyncLayout`: replaces the layout snapshot (not an undo step).
pub(crate) fn sync_layout(
    doc: &mut RackDoc,
    device: &DeviceKey,
    layout: PluginLayout,
) -> Result<(), ModelError> {
    let (_, p) = plugin_device(doc, device)?;
    validate_layout(&p.plugin_ref(), &layout).map_err(|f| layout_error(&p.type_id(), f))?;
    let p = doc
        .resolve_mut(device)
        .and_then(|d| d.plugin.as_mut())
        .expect("checked above");
    p.layout = layout;
    Ok(())
}
