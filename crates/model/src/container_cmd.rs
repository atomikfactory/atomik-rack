//! Planning of the container commands and of the path-addressed structure commands (Phase
//! containers): `AddInner`, `SetSplits`, `SetMacro`, `RemoveMacroTarget`,
//! `RenameMacro`, `DuplicateContainer`, and `RemoveDevice`, `MoveDevice`, `Connect` and
//! `Disconnect` on inner devices.
//!
//! Every command is planned on the current document and becomes one reversible [`Edit`]; the
//! edit is then performed on a scratch copy and the whole project is checked
//! ([`RackDoc::check_containers`]), so a rejected command changes nothing and every limit and
//! rule of has one home.

use crate::container::{
    ContainerCommand, ContainerDoc, FORBIDDEN_INSIDE, MACRO_COUNT, MAX_CONTAINER_DEPTH,
    MAX_INNER_DEVICES, MAX_PROJECT_INNER, MAX_SPLITS, MacroDoc, NoteSplit, SPLIT_TYPE, new_device,
    notes_input, scope_socket, split_path,
};
use crate::container_move;
use crate::history::Edit;
use crate::{Cable, DeviceKey, ModelError, RackDevice, RackDoc, SocketRef, rules};
use atomik_device_sdk::Registry;

fn bad_scope(message: impl Into<String>) -> ModelError {
    ModelError::BadScope(message.into())
}

fn bad_macro(message: impl Into<String>) -> ModelError {
    ModelError::BadMacro(message.into())
}

/// The inside of the container at path key `key`.
fn container_of<'d>(doc: &'d RackDoc, key: &DeviceKey) -> Result<&'d ContainerDoc, ModelError> {
    let device = rules::resolve(doc, key)?;
    device
        .inner
        .as_deref()
        .ok_or_else(|| bad_scope(format!("{key} is not a container")))
}

/// Performs `edit` on a scratch copy and checks the whole project; the edit when it passes.
fn validated(doc: &RackDoc, registry: &Registry, edit: Edit) -> Result<Edit, ModelError> {
    let mut scratch = doc.clone();
    edit.perform(&mut scratch)?;
    scratch.check_containers(registry)?;
    Ok(edit)
}

fn compound(mut edits: Vec<Edit>) -> Edit {
    if edits.len() == 1 {
        edits.remove(0)
    } else {
        Edit::Compound(edits)
    }
}

/// Plans one container command.
pub(crate) fn plan(
    doc: &RackDoc,
    registry: &Registry,
    command: ContainerCommand,
) -> Result<Edit, ModelError> {
    match command {
        ContainerCommand::AddInner {
            container,
            index,
            type_id,
            key,
            auto_split,
        } => add_inner(doc, registry, container, index, &type_id, key, auto_split),
        ContainerCommand::SetSplits {
            container, splits, ..
        } => {
            let c = container_of(doc, &container)?;
            let edit = Edit::Splits {
                old: c.splits.clone(),
                new: splits,
                container,
            };
            validated(doc, registry, edit)
        }
        ContainerCommand::SetMacro {
            container,
            index,
            target,
            ..
        } => {
            let c = container_of(doc, &container)?;
            let i = macro_index(index)?;
            // A param a plugin no longer has stays mapped but cannot be mapped anew.
            if let Some(dev) = crate::container::find_relative(&c.devices, &target.device.0)
                && let Ok(desc) = rules::descriptor(registry, &dev.type_id)
                && desc.param_by_key(&target.param).is_none()
            {
                return Err(bad_macro(format!(
                    "{} has no param {:?}",
                    desc.type_id, target.param
                )));
            }
            if let Some(k) = (0..MACRO_COUNT).find(|&k| {
                k != i
                    && c.macros[k]
                        .targets
                        .iter()
                        .any(|t| t.device == target.device && t.param == target.param)
            }) {
                return Err(bad_macro(format!(
                    "{}.{} is already driven by macro {}",
                    target.device,
                    target.param,
                    k + 1
                )));
            }
            let old = c.macros[i].clone();
            let mut new = old.clone();
            match new
                .targets
                .iter_mut()
                .find(|t| t.device == target.device && t.param == target.param)
            {
                Some(t) => {
                    t.min = target.min;
                    t.max = target.max;
                }
                None => new.targets.push(target),
            }
            let edit = Edit::Macro {
                container,
                index,
                old,
                new,
            };
            validated(doc, registry, edit)
        }
        ContainerCommand::RemoveMacroTarget {
            container,
            index,
            device,
            param,
        } => {
            let c = container_of(doc, &container)?;
            let i = macro_index(index)?;
            let old = c.macros[i].clone();
            let mut new = old.clone();
            let before = new.targets.len();
            new.targets
                .retain(|t| !(t.device == device && t.param == param));
            if new.targets.len() == before {
                return Err(bad_macro(format!(
                    "macro {} does not drive {device}.{param}",
                    i + 1
                )));
            }
            Ok(Edit::Macro {
                container,
                index,
                old,
                new,
            })
        }
        ContainerCommand::RenameMacro {
            container,
            index,
            name,
        } => {
            let c = container_of(doc, &container)?;
            let i = macro_index(index)?;
            let old = c.macros[i].clone();
            let new = MacroDoc {
                name,
                targets: old.targets.clone(),
            };
            let edit = Edit::Macro {
                container,
                index,
                old,
                new,
            };
            validated(doc, registry, edit)
        }
        ContainerCommand::DuplicateContainer { container } => duplicate(doc, registry, &container),
        ContainerCommand::Combine { keys, name } => {
            container_move::combine(doc, registry, &keys, &name).map(|planned| planned.edit)
        }
        ContainerCommand::Uncombine { container } => {
            container_move::uncombine(doc, registry, &container).map(|planned| planned.edit)
        }
        ContainerCommand::MoveIntoContainer {
            key,
            container,
            index,
        } => container_move::move_into(doc, registry, &key, &container, index)
            .map(|planned| planned.edit),
        ContainerCommand::MoveOutOfContainer { key, index } => {
            container_move::move_out(doc, registry, &key, index).map(|planned| planned.edit)
        }
    }
}

fn macro_index(index: u8) -> Result<usize, ModelError> {
    if usize::from(index) < MACRO_COUNT {
        Ok(usize::from(index))
    } else {
        Err(bad_macro(format!(
            "macro index {index} is outside 0..{MACRO_COUNT}"
        )))
    }
}

fn add_inner(
    doc: &RackDoc,
    registry: &Registry,
    container: DeviceKey,
    index: usize,
    type_id: &str,
    key: Option<DeviceKey>,
    auto_split: bool,
) -> Result<Edit, ModelError> {
    let desc = rules::descriptor(registry, type_id)?;
    crate::plugin_cmd::refuse_generic_add(type_id)?;
    add_inner_device(
        doc,
        registry,
        container,
        index,
        desc,
        key,
        None,
        auto_split,
        |key, name| new_device(registry, desc, key, name),
    )
}

/// Plans the insertion of a device of type `desc` inside `container` (a path key): the
/// container's rules (allowed type, depth, the 32 and 256 device limits, a free local key), an
/// optional full-range split, then the check of the whole project. `key_base` is the base of a
/// generated key (the type's own by default); `make` builds the device from its key and name.
#[allow(clippy::too_many_arguments)]
pub(crate) fn add_inner_device(
    doc: &RackDoc,
    registry: &Registry,
    container: DeviceKey,
    index: usize,
    desc: &'static atomik_device_sdk::DeviceDescriptor,
    key: Option<DeviceKey>,
    key_base: Option<&str>,
    auto_split: bool,
    make: impl FnOnce(DeviceKey, String) -> RackDevice,
) -> Result<Edit, ModelError> {
    let type_id = desc.type_id;
    let c = container_of(doc, &container)?;
    if FORBIDDEN_INSIDE.contains(&type_id) || type_id == SPLIT_TYPE {
        return Err(ModelError::NotAllowedInContainer(format!(
            "{} cannot be placed inside a container",
            desc.name
        )));
    }
    if desc.kind == atomik_device_sdk::DeviceKind::Container
        && container.0.split('/').count() + 1 > MAX_CONTAINER_DEPTH
    {
        return Err(ModelError::ContainerTooDeep(format!(
            "containers nest at most {MAX_CONTAINER_DEPTH} levels deep"
        )));
    }
    if c.devices.len() >= MAX_INNER_DEVICES {
        return Err(ModelError::ContainerFull(format!(
            "a container holds at most {MAX_INNER_DEVICES} devices"
        )));
    }
    if doc.inner_device_count() >= MAX_PROJECT_INNER {
        return Err(ModelError::ContainerFull(format!(
            "a project holds at most {MAX_PROJECT_INNER} devices inside containers"
        )));
    }
    let key = match key {
        Some(k) => {
            rules::check_new_key_in(&c.devices, &k.0)?;
            k
        }
        None => match key_base {
            Some(base) => rules::generate_key_with_base_in(&c.devices, base),
            None => rules::generate_key_in(&c.devices, type_id),
        },
    };
    let name = rules::unique_name_in(&c.devices, desc.name);
    let device = make(key.clone(), name);
    let mut edits = vec![Edit::InnerInsert {
        container: container.clone(),
        index: index.min(c.devices.len()),
        device,
        cables: Vec::new(),
    }];
    if auto_split && notes_input(desc).is_some() && c.splits.len() < MAX_SPLITS {
        let mut new = c.splits.clone();
        new.push(NoteSplit::full(key));
        edits.push(Edit::Splits {
            container,
            old: c.splits.clone(),
            new,
        });
    }
    validated(doc, registry, compound(edits))
}

fn duplicate(
    doc: &RackDoc,
    registry: &Registry,
    container: &DeviceKey,
) -> Result<Edit, ModelError> {
    let original = rules::resolve(doc, container)?;
    if original.inner.is_none() {
        return Err(bad_scope(format!("{container} is not a container")));
    }
    let (parent, local) = split_path(container);
    let scope = doc
        .scope_devices(parent.as_ref())
        .ok_or_else(|| ModelError::UnknownDevice(container.0.clone()))?;
    let position = scope
        .iter()
        .position(|d| d.key == local)
        .ok_or_else(|| ModelError::UnknownDevice(container.0.clone()))?;
    let mut copy = original.clone();
    copy.key = rules::generate_key_in(scope, &original.type_id);
    copy.name = format!("{} copy", original.name);
    let edit = match parent {
        None => Edit::Insert {
            index: position + 1,
            device: copy,
            cables: Vec::new(),
        },
        Some(container) => Edit::InnerInsert {
            container,
            index: position + 1,
            device: copy,
            cables: Vec::new(),
        },
    };
    validated(doc, registry, edit)
}

// ------------------------------------------------------------------ path-addressed commands

/// Plans `RemoveDevice` of an inner device (a path key): its cables, the splits to it and the
/// macro targets that name it (in this container and in every container around it) go in the
/// same step.
pub(crate) fn remove_inner(doc: &RackDoc, key: &DeviceKey) -> Result<Edit, ModelError> {
    let (parent, local) = split_path(key);
    let container = parent.ok_or_else(|| ModelError::UnknownDevice(key.0.clone()))?;
    let c = container_of(doc, &container)?;
    let index = c
        .devices
        .iter()
        .position(|d| d.key == local)
        .ok_or_else(|| ModelError::UnknownDevice(key.0.clone()))?;
    let mut edits = Vec::new();
    if c.splits.iter().any(|s| s.target == local) {
        edits.push(Edit::Splits {
            container: container.clone(),
            old: c.splits.clone(),
            new: c
                .splits
                .iter()
                .filter(|s| s.target != local)
                .cloned()
                .collect(),
        });
    }
    // The macros of this container and of every container around it that name the device or
    // anything inside it.
    let segments: Vec<&str> = container.0.split('/').collect();
    for depth in (1..=segments.len()).rev() {
        let holder = DeviceKey(segments[..depth].join("/"));
        let mut relative = segments[depth..].join("/");
        if !relative.is_empty() {
            relative.push('/');
        }
        relative.push_str(&local.0);
        let inside = format!("{relative}/");
        let Some(inner) = doc.inner(&holder) else {
            continue;
        };
        for (i, m) in inner.macros.iter().enumerate() {
            let kept: Vec<_> = m
                .targets
                .iter()
                .filter(|t| t.device.0 != relative && !t.device.0.starts_with(&inside))
                .cloned()
                .collect();
            if kept.len() != m.targets.len() {
                edits.push(Edit::Macro {
                    container: holder.clone(),
                    index: i as u8,
                    old: m.clone(),
                    new: MacroDoc {
                        name: m.name.clone(),
                        targets: kept,
                    },
                });
            }
        }
    }
    edits.push(Edit::InnerRemove {
        cables: c
            .cables
            .iter()
            .filter(|x| x.from.device == local || x.to.device == local)
            .cloned()
            .collect(),
        device: c.devices[index].clone(),
        container,
        index,
    });
    Ok(compound(edits))
}

/// Plans `MoveDevice` of an inner device (a path key) within its container.
pub(crate) fn move_inner(doc: &RackDoc, key: &DeviceKey, to: usize) -> Result<Edit, ModelError> {
    let (parent, local) = split_path(key);
    let container = parent.ok_or_else(|| ModelError::UnknownDevice(key.0.clone()))?;
    let c = container_of(doc, &container)?;
    let from = c
        .devices
        .iter()
        .position(|d| d.key == local)
        .ok_or_else(|| ModelError::UnknownDevice(key.0.clone()))?;
    Ok(Edit::InnerMove {
        container,
        from,
        to: to.min(c.devices.len() - 1),
    })
}

/// Plans `Connect` of a cable whose ends are path keys (an end may be
/// `<container path>/boundary`); both ends must live in the same container.
pub(crate) fn connect(
    doc: &RackDoc,
    registry: &Registry,
    cable: &Cable,
) -> Result<Edit, ModelError> {
    match rules::cable_scope(cable)? {
        None => {
            rules::check_cable(doc, registry, cable)?;
            // A cable into the master's `key_in` switches the compressor key on, same step.
            Ok(match rules::key_on_edit(doc, registry, cable) {
                Some(key_on) => Edit::Compound(vec![Edit::Connect(cable.clone()), key_on]),
                None => Edit::Connect(cable.clone()),
            })
        }
        Some(container) => {
            let local = rules::local_cable(cable);
            rules::check_inner_cable(doc, registry, &container, &local)?;
            Ok(Edit::InnerConnect {
                container,
                cable: local,
            })
        }
    }
}

/// Plans `Disconnect` of the cables plugged into the input `to` (a path key).
pub(crate) fn disconnect(
    doc: &RackDoc,
    registry: &Registry,
    to: &SocketRef,
) -> Result<Edit, ModelError> {
    let (parent, local_device) = split_path(&to.device);
    let Some(container) = parent else {
        unreachable!("top-level disconnects are planned by the caller");
    };
    let c = container_of(doc, &container)?;
    let local = SocketRef {
        device: local_device,
        socket: to.socket.clone(),
    };
    let (_, direction) = scope_socket(&c.devices, registry, &local)?;
    if !direction.is_input() {
        return Err(ModelError::BadCable(format!("{to} is not an input")));
    }
    let mut edits: Vec<Edit> = c
        .cables
        .iter()
        .filter(|x| x.to == local)
        .cloned()
        .map(|cable| Edit::InnerDisconnect {
            container: container.clone(),
            cable,
        })
        .collect();
    match edits.len() {
        0 => Err(ModelError::BadCable(format!("{to} is not connected"))),
        1 => Ok(edits.remove(0)),
        _ => Ok(Edit::Compound(edits)),
    }
}
