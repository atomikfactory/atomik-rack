//! `WorkflowCommand::ReplaceDevice`: a device of the rack becomes another one of the same kind
//! and keeps what the project hung on it. The new device takes the old one's key and place, so
//! its song track (clips, lanes, armed state), its console channel with the sends, the cables
//! that fit the new device's sockets, the note entry of every track, the insert-chain or
//! send/return place and the column all stay as they were. What cannot stay is left behind:
//! the params and the sample, pattern and plugin data of the old device, the cables on sockets
//! the new one lacks, and the automation lanes of params it lacks ([`lost_lanes`] lists those).
//! A default-named device and a default-named channel take the new device's name.

use crate::console::is_console_type;
use crate::console_cmd::{Planner, default_channel_name};
use crate::history::Edit;
use crate::song::LaneKind;
use crate::song_cmd::has_notes_input;
use crate::workflow_create::{CreateSource, is_note_player};
use crate::{Cable, DeviceKey, ModelError, RackDoc, SocketRef, console_cmd, rules};
use atomik_device_sdk::{DeviceDescriptor, DeviceKind, Registry};

fn refuse(message: impl Into<String>) -> ModelError {
    ModelError::BadScope(message.into())
}

/// The kind a replacement must share: instrument, effect or note player.
fn family(desc: &DeviceDescriptor) -> Option<&'static str> {
    if is_console_type(desc.type_id) {
        return None;
    }
    if is_note_player(desc) {
        return Some("player");
    }
    match desc.kind {
        DeviceKind::Instrument => Some("instrument"),
        DeviceKind::Effect => Some("effect"),
        _ => None,
    }
}

/// The descriptor the new device has; the source must be known to the registry.
fn source_descriptor(
    registry: &Registry,
    source: &CreateSource,
) -> Result<&'static DeviceDescriptor, ModelError> {
    match source {
        CreateSource::Type(type_id) => rules::descriptor(registry, type_id),
        CreateSource::Plugin { plugin, .. } => {
            let type_id = plugin.type_id();
            registry
                .descriptor(&type_id)
                .filter(|_| registry.is_plugin(&type_id))
                .ok_or(ModelError::UnknownType(type_id))
        }
    }
}

/// Checks that `target` may become a device of `source` and returns the two descriptors.
fn check(
    doc: &RackDoc,
    registry: &Registry,
    target: &DeviceKey,
    source: &CreateSource,
) -> Result<(&'static DeviceDescriptor, &'static DeviceDescriptor), ModelError> {
    let device = rules::resolve(doc, target)?;
    let name = device.name.clone();
    if target.0.contains('/') {
        return Err(refuse(format!(
            "{name} is inside a container; only top-level devices can be replaced"
        )));
    }
    let old = rules::descriptor(registry, &device.type_id)?;
    let new = source_descriptor(registry, source)?;
    let (Some(from), Some(to)) = (family(old), family(new)) else {
        return Err(refuse(format!(
            "{name} cannot be replaced: only instruments, effects and note players can"
        )));
    };
    if from != to {
        return Err(refuse(format!("{name} is an {from}; {} is not", new.name)));
    }
    if crate::freeze::frozen_devices(doc).contains(target) {
        return Err(ModelError::Frozen(format!(
            "{name} belongs to a frozen track; unfreeze it first"
        )));
    }
    if doc.song.track(target).is_some() && has_notes_input(old) != has_notes_input(new) {
        return Err(refuse(format!(
            "{} does not take notes like {name} does",
            new.name
        )));
    }
    Ok((old, new))
}

/// The automation lanes (their param keys) the replacement drops: those of `target`'s track for
/// params the new device does not have. Empty when the replacement is refused.
pub fn lost_lanes(
    doc: &RackDoc,
    registry: &Registry,
    target: &DeviceKey,
    source: &CreateSource,
) -> Vec<String> {
    let Ok((_, new)) = check(doc, registry, target, source) else {
        return Vec::new();
    };
    doc.song.track(target).map_or_else(Vec::new, |t| {
        t.lanes
            .iter()
            .filter_map(|l| match &l.kind {
                LaneKind::Automation(p) if new.param_by_key(p).is_none() => Some(p.clone()),
                _ => None,
            })
            .collect()
    })
}

/// The first plain insert inside `edit` (a creation plan may carry a console channel too).
fn first_insert(edit: Edit) -> Option<Edit> {
    match edit {
        insert @ Edit::Insert { .. } => Some(insert),
        Edit::Compound(edits) => edits.into_iter().find_map(first_insert),
        _ => None,
    }
}

/// Whether `name` is what a device of `desc` gets by default: the type's name, numbered or not.
fn is_default_name(name: &str, desc: &DeviceDescriptor) -> bool {
    match name.strip_prefix(desc.name) {
        Some("") => true,
        Some(rest) => rest
            .strip_prefix(' ')
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())),
        None => false,
    }
}

/// The cable `c` with the ends on `key` kept when the new descriptor has the socket on the same
/// side and of the same kind; `None` when it does not fit.
fn carried(
    c: &Cable,
    key: &DeviceKey,
    old: &DeviceDescriptor,
    new: &DeviceDescriptor,
) -> Option<Cable> {
    let fits = |end: &SocketRef| -> bool {
        if &end.device != key {
            return true;
        }
        match (
            old.socket_by_key(&end.socket),
            new.socket_by_key(&end.socket),
        ) {
            (Some(a), Some(b)) => a.kind == b.kind && a.direction == b.direction,
            _ => false,
        }
    };
    (fits(&c.from) && fits(&c.to)).then(|| c.clone())
}

/// Plans the replacement of `target` by a device of `source` (see the module documentation).
pub(crate) fn plan(
    doc: &RackDoc,
    registry: &Registry,
    target: &DeviceKey,
    source: &CreateSource,
) -> Result<Edit, ModelError> {
    let (old_desc, new_desc) = check(doc, registry, target, source)?;
    let index = rules::index_of(doc, target)?;
    let old = doc.devices[index].clone();
    let cables: Vec<Cable> = doc
        .cables
        .iter()
        .filter(|c| &c.from.device == target || &c.to.device == target)
        .cloned()
        .collect();
    let mut p = Planner::new(doc, registry);
    p.push(Edit::Remove {
        index,
        device: old.clone(),
        cables: cables.clone(),
    });
    // The creation plan of an instrument also makes a channel; only its device is wanted.
    let command = match source {
        CreateSource::Type(type_id) => crate::Command::AddDevice {
            index,
            type_id: type_id.clone(),
            key: Some(target.clone()),
        },
        CreateSource::Plugin {
            plugin,
            layout,
            path_hint,
        } => crate::Command::Plugin(crate::PluginCommand::Add {
            index,
            container: None,
            plugin: plugin.clone(),
            layout: layout.clone(),
            path_hint: path_hint.clone(),
            key: Some(target.clone()),
            auto_split: false,
        }),
    };
    let (planned, _) = crate::Document::plan(p.doc(), command, registry)?;
    let Some(Edit::Insert {
        index: at,
        mut device,
        ..
    }) = first_insert(planned)
    else {
        return Err(refuse("the replacement could not be created"));
    };
    device.column = old.column;
    device.folded = old.folded;
    device.color = old.color;
    let siblings: Vec<_> = doc
        .devices
        .iter()
        .filter(|d| &d.key != target)
        .cloned()
        .collect();
    device.name = if is_default_name(&old.name, old_desc) {
        rules::unique_name_in(&siblings, new_desc.name)
    } else {
        old.name.clone()
    };
    let new_name = device.name.clone();
    p.push(Edit::Insert {
        index: at,
        device,
        cables: Vec::new(),
    });
    for c in cables
        .iter()
        .filter_map(|c| carried(c, target, old_desc, new_desc))
    {
        // A cable the rules refuse on the new device is left out.
        let _ = p.connect(c);
    }
    // The track keeps its clips; the lanes of params the new device lacks go.
    if let Some(t) = p.doc().song.track(target) {
        let kept: Vec<_> = t
            .lanes
            .iter()
            .filter(|l| match &l.kind {
                LaneKind::Automation(param) => new_desc.param_by_key(param).is_some(),
                _ => true,
            })
            .cloned()
            .collect();
        if kept.len() != t.lanes.len() {
            let index = p
                .doc()
                .song
                .tracks
                .iter()
                .position(|x| &x.device == target)
                .expect("the track exists");
            let old_track = t.clone();
            // A track needs a lane: one whose lanes all went (an automation track of params
            // the new device lacks) goes with them.
            let new_track = (!kept.is_empty()).then(|| {
                let mut track = old_track.clone();
                track.lanes = kept;
                track
            });
            p.push(Edit::Track {
                index,
                old: Some(old_track),
                new: new_track,
            });
        }
    }
    // A channel named after the old device is named after the new one.
    if let Some(channel) = doc.console.channel_of(target).map(|c| c.channel.clone()) {
        let current = doc.device(&channel).map(|d| d.name.clone());
        let old_out = console_cmd::source_outputs(doc, registry, target).ok();
        let new_out = console_cmd::source_outputs(p.doc(), registry, target).ok();
        if let (Some(current), Some(old_out), Some(new_out)) = (current, old_out, new_out)
            && current == default_channel_name(&old.name, &old_out.0)
        {
            let renamed = default_channel_name(&new_name, &new_out.0);
            if renamed != current {
                p.push(Edit::Name {
                    key: channel,
                    old: current,
                    new: renamed,
                });
            }
        }
    }
    Ok(p.finish())
}

/// Why `target` cannot be replaced by a device of `source` on `doc`, or `None` when it can (the
/// plan is tried on a copy of the document). A drop target offers itself only on `None`.
pub fn replace_reason(
    doc: &RackDoc,
    registry: &Registry,
    target: &DeviceKey,
    source: &CreateSource,
) -> Option<String> {
    let edit = match plan(doc, registry, target, source) {
        Ok(edit) => edit,
        Err(e) => return Some(e.to_string()),
    };
    let mut scratch = doc.clone();
    edit.perform(&mut scratch).err().map(|e| e.to_string())
}
