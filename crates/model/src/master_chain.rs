//! The master chain: the effects of the master section as one item the library keeps.
//!
//! A [`MasterChain`] holds the master's **insert effects** in order and the **FX send effects**
//! of each used send slot (the chain of each return in signal order), with everything the
//! devices carry (params, bypass, names, plugin documents and state, containers) and the
//! cables between them and to the master's insert loop and send and return sockets. It also
//! holds the master's own return params (`return{k}.level`, `.mute`, `.pan`, the three of each
//! slot), the part of the master section that belongs to the sends. The per-channel send levels
//! are not part of it: they belong to the channels.
//!
//! The item file is the library's own device item, an `atomik-project` text that holds the
//! master, the chain devices, their cables and the console's `master` and `returns`, with an
//! empty song, so any project reader opens it and the project format is not touched. The browser
//! lists it as a patch; [`is_master_chain_item`] tells it from an ordinary patch.
//!
//! [`WorkflowCommand::ApplyMasterChain`](crate::WorkflowCommand) **replaces** the chain of the
//! document's master: the current insert and send effects go with their cables, the item's
//! devices arrive with fresh keys right under the master (inserts first, then the sends in
//! slot order), wired as the item says, and the eight returns take the item's slots and
//! params, everything in one undo step. A slot the item does not use ends empty with default
//! return params, so what is on the master afterwards is exactly the item.

use crate::console::{FX_SLOTS, MASTER_TYPE};
use crate::console_cmd::{Planner, bad};
use crate::console_insert::{insert_chain, send_chain};
use crate::history::Edit;
use crate::project;
use crate::workflow::DeviceSnapshot;
use crate::{Cable, DeviceKey, ModelError, RackDoc, empty_doc, empty_song, rules};
use atomik_device_sdk::Registry;
use serde_json::Value;
use std::collections::BTreeMap;

/// The master's params that belong to a send slot, `return{k}.<this>`.
const RETURN_PARAMS: [&str; 3] = ["level", "mute", "pan"];

/// The effects of a master section (see the module documentation).
#[derive(Debug, Clone, PartialEq)]
pub struct MasterChain {
    /// The master's key in the item: the cables in [`Self::master_cables`] name it.
    pub master: DeviceKey,
    /// The chain effects: the insert chain in order, then the effects of each used send in slot
    /// order, with the cables between them and the pool entries they need.
    pub snapshot: DeviceSnapshot,
    /// The cables between the master and the chain effects (the insert loop and the send and
    /// return sockets).
    pub master_cables: Vec<Cable>,
    /// How many of the snapshot's first devices are insert effects.
    pub inserts: usize,
    /// The first effect of each send slot, `None` for a free slot.
    pub returns: [Option<DeviceKey>; FX_SLOTS],
    /// The master's params of the returns that were set (`return1.level`, ...). A param that is
    /// not here is at its default.
    pub return_params: BTreeMap<String, f32>,
}

impl MasterChain {
    /// The effects of the master insert chain, as the item keys them.
    pub fn insert_keys(&self) -> Vec<DeviceKey> {
        self.snapshot
            .devices
            .iter()
            .take(self.inserts)
            .map(|d| d.key.clone())
            .collect()
    }

    /// How many send slots the chain uses.
    pub fn send_count(&self) -> usize {
        self.returns.iter().flatten().count()
    }

    /// The type id of every effect, in item order (a summary for messages and tests).
    pub fn type_ids(&self) -> Vec<&str> {
        self.snapshot
            .devices
            .iter()
            .map(|d| d.type_id.as_str())
            .collect()
    }
}

/// Copies the master chain of `doc`: its insert effects and the effects of every used send.
///
/// # Errors
/// The document has no master.
pub fn capture_master_chain(doc: &RackDoc, registry: &Registry) -> Result<MasterChain, ModelError> {
    let master = doc
        .console
        .master
        .clone()
        .filter(|m| doc.device(m).is_some())
        .ok_or_else(|| bad("there is no master section"))?;
    let mut keys = insert_chain(doc, registry, &master);
    let inserts = keys.len();
    let mut returns: [Option<DeviceKey>; FX_SLOTS] = Default::default();
    for (slot, first) in returns.iter_mut().enumerate() {
        let chain = send_chain(doc, registry, slot);
        *first = chain.first().cloned();
        for k in chain {
            if !keys.contains(&k) {
                keys.push(k);
            }
        }
    }
    let mut snapshot = if keys.is_empty() {
        DeviceSnapshot {
            devices: Vec::new(),
            cables: Vec::new(),
            tracks: Vec::new(),
            assets: BTreeMap::new(),
        }
    } else {
        DeviceSnapshot::capture(doc, &keys, false)?
    };
    for d in &mut snapshot.devices {
        d.column = 0;
    }
    // A slot whose first effect did not come along (the capture leaves console devices out) is
    // free in the copy.
    for first in &mut returns {
        if first
            .as_ref()
            .is_some_and(|k| !snapshot.devices.iter().any(|d| d.key == *k))
        {
            *first = None;
        }
    }
    let on_chain = |k: &DeviceKey| snapshot.devices.iter().any(|d| d.key == *k);
    let mut master_cables: Vec<Cable> = doc
        .cables
        .iter()
        .filter(|c| {
            (c.from.device == master && on_chain(&c.to.device))
                || (c.to.device == master && on_chain(&c.from.device))
        })
        .cloned()
        .collect();
    master_cables.sort();
    let device = rules::device(doc, &master)?;
    let return_params = (1..=FX_SLOTS)
        .flat_map(|k| RETURN_PARAMS.map(|p| format!("return{k}.{p}")))
        .filter_map(|key| device.params.get(&key).map(|v| (key, *v)))
        .collect();
    Ok(MasterChain {
        master,
        snapshot,
        master_cables,
        inserts,
        returns,
        return_params,
    })
}

/// The text of the library item that holds `chain`; `doc` is the document it came from (the
/// item keeps its sample rate and tempo). The text is canonical, so the same chain always gives
/// the same bytes.
///
/// # Errors
/// `doc` has no master device to take the return params from.
pub fn save_master_chain_item(chain: &MasterChain, doc: &RackDoc) -> Result<String, ModelError> {
    let master = doc
        .device(&chain.master)
        .ok_or_else(|| bad("there is no master section"))?
        .clone();
    let mut item = empty_doc();
    item.sample_rate = doc.sample_rate;
    item.tempo_bpm = doc.tempo_bpm;
    item.song = std::sync::Arc::new(empty_song(doc.tempo_bpm));
    // The item's master is a clean one: only the return params of the chain are kept.
    let mut master = master;
    master.column = 0;
    master.params = chain.return_params.clone();
    item.devices.push(master);
    item.devices.extend(chain.snapshot.devices.iter().cloned());
    item.cables = chain.snapshot.cables.clone();
    item.cables.extend(chain.master_cables.iter().cloned());
    item.cables.sort();
    item.assets = chain.snapshot.assets.clone();
    item.console.master = Some(chain.master.clone());
    item.console.returns = chain.returns.clone();
    Ok(project::save(&item))
}

/// Whether `text` is a master chain item: a device item whose console names a master. A cheap
/// look at the JSON, no registry and no validation; [`load_master_chain_item`] does the rest.
pub fn is_master_chain_item(text: &str) -> bool {
    serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|v| {
            let console = v.get("console")?;
            let master = console.get("master")?.as_str()?.to_owned();
            let has_master = v
                .get("devices")?
                .as_array()?
                .iter()
                .any(|d| d.get("id").and_then(Value::as_str) == Some(master.as_str()));
            Some(has_master)
        })
        .unwrap_or(false)
}

/// Reads a master chain item text. A plugin the registry does not know is registered as a
/// stand-in first (as opening a project does), so its effect is on the chain like the others.
///
/// # Errors
/// The text is not a readable project, or holds no master section
/// ([`ModelError::Load`]).
pub fn load_master_chain_item(
    text: &str,
    registry: &mut Registry,
) -> Result<MasterChain, ModelError> {
    let doc = project::load(text, registry)?;
    let not_chain = || ModelError::Load {
        path: "$.console.master".into(),
        message: "this item holds no master section".into(),
    };
    let master = doc.console.master.clone().ok_or_else(not_chain)?;
    if doc.device(&master).is_none_or(|d| d.type_id != MASTER_TYPE) {
        return Err(not_chain());
    }
    crate::register_stand_ins(&doc, registry)?;
    capture_master_chain(&doc, registry)
}

/// Plans `ApplyMasterChain`: the master's insert and send effects are replaced by `chain`.
pub(crate) fn plan_apply(
    doc: &RackDoc,
    registry: &Registry,
    chain: &MasterChain,
) -> Result<Edit, ModelError> {
    let master = doc
        .console
        .master
        .clone()
        .filter(|m| doc.device(m).is_some())
        .ok_or_else(|| bad("there is no master section to put the chain on"))?;
    let mut p = Planner::new(doc, registry);
    // The effects that are there now go, each with its cables and its send slot.
    let mut old = insert_chain(doc, registry, &master);
    for slot in 0..FX_SLOTS {
        for k in send_chain(doc, registry, slot) {
            if !old.contains(&k) {
                old.push(k);
            }
        }
    }
    for k in &old {
        crate::console_cmd::remove_device(&mut p, k)?;
    }
    // The returns of an effect that was not on its chain (hand cabled) still stand: free them.
    for slot in 0..FX_SLOTS {
        if let Some(effect) = p.doc().console.returns[slot].clone() {
            p.push(Edit::ConsoleReturn {
                slot,
                old: Some(effect),
                new: None,
            });
        }
    }
    // The item's effects arrive right under the master, in its column.
    let mut map: BTreeMap<DeviceKey, DeviceKey> = BTreeMap::new();
    map.insert(chain.master.clone(), master.clone());
    if !chain.snapshot.devices.is_empty() {
        let at = rules::index_of(p.doc(), &master)? + 1;
        let column = p.doc().device(&master).map(|d| d.column);
        let new = crate::workflow_cmd::paste_devices(&mut p, &chain.snapshot, None, at, column)?;
        for (d, key) in chain.snapshot.devices.iter().zip(new) {
            map.insert(d.key.clone(), key);
        }
    }
    for c in &chain.master_cables {
        let (Some(from), Some(to)) = (map.get(&c.from.device), map.get(&c.to.device)) else {
            return Err(bad("the master chain has a cable to a device it lacks"));
        };
        p.connect(Cable {
            from: crate::SocketRef {
                device: from.clone(),
                socket: c.from.socket.clone(),
            },
            to: crate::SocketRef {
                device: to.clone(),
                socket: c.to.socket.clone(),
            },
        })?;
    }
    for (slot, first) in chain.returns.iter().enumerate() {
        let Some(first) = first.as_ref().and_then(|k| map.get(k)) else {
            continue;
        };
        p.push(Edit::ConsoleReturn {
            slot,
            old: None,
            new: Some(first.clone()),
        });
    }
    set_return_params(&mut p, &master, &chain.return_params)?;
    Ok(p.finish())
}

/// Sets the master's `return{k}.{level,mute,pan}` params to the chain's values, a missing one
/// to its default.
fn set_return_params(
    p: &mut Planner<'_>,
    master: &DeviceKey,
    values: &BTreeMap<String, f32>,
) -> Result<(), ModelError> {
    let desc = rules::descriptor(p.registry, p.type_of(master)?)?;
    for k in 1..=FX_SLOTS {
        for name in RETURN_PARAMS {
            let key = format!("return{k}.{name}");
            let Some(param) = desc.param_by_key(&key) else {
                continue;
            };
            let target = param.clamp(values.get(&key).copied().unwrap_or(param.default));
            let old = p
                .doc()
                .device(master)
                .and_then(|d| d.params.get(&key).copied());
            if param.clamp(old.unwrap_or(param.default)).to_bits() == target.to_bits() {
                continue;
            }
            p.push(Edit::Param {
                key: master.clone(),
                param: key,
                old,
                new: (target.to_bits() != param.default.to_bits()).then_some(target),
                default: param.default,
            });
        }
    }
    Ok(())
}
