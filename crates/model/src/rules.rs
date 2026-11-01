//! Validation rules shared by commands and patch loading: keys, sockets, cables (mirroring
//! the engine's graph rules), key and name generation.

use crate::container::{BOUNDARY_KEY, notes_input, scope_socket, split_path};
use crate::history::Edit;
use crate::song::MAX_AUDIO_FRAMES;
use crate::{
    AssetMeta, Cable, DeviceKey, INTERFACE_KEY, MAX_INPUT_CHANNELS, ModelError, RackDevice,
    RackDoc, SocketRef, is_valid_asset_hash,
};
use atomik_device_sdk::{DeviceDescriptor, Direction, Registry, SocketKind};
use std::collections::{BTreeMap, BTreeSet};

/// True if `key` matches `[a-z][a-z0-9_-]*`.
pub(crate) fn is_valid_key(key: &str) -> bool {
    let mut chars = key.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// Checks that `key` is valid, not reserved and unused in `doc`.
pub(crate) fn check_new_key(doc: &RackDoc, key: &str) -> Result<(), ModelError> {
    check_new_key_in(&doc.devices, key)
}

/// [`check_new_key`] inside one scope (the top level or a container): keys are unique per
/// scope; `interface` and `boundary` are reserved everywhere.
pub(crate) fn check_new_key_in(devices: &[RackDevice], key: &str) -> Result<(), ModelError> {
    if key == INTERFACE_KEY
        || key == BOUNDARY_KEY
        || !is_valid_key(key)
        || devices.iter().any(|d| d.key.0 == key)
    {
        return Err(ModelError::BadKey(key.to_owned()));
    }
    Ok(())
}

/// The descriptor of a device type.
pub(crate) fn descriptor(
    registry: &Registry,
    type_id: &str,
) -> Result<&'static DeviceDescriptor, ModelError> {
    registry
        .get(type_id)
        .map(|f| f.descriptor)
        .ok_or_else(|| ModelError::UnknownType(type_id.to_owned()))
}

/// What the master lock says; the first is the status text of a delete.
pub(crate) const MASTER_DELETE: &str = "The Master Section cannot be deleted";
pub(crate) const MASTER_COPY: &str = "The Master Section cannot be copied";
pub(crate) const MASTER_MOVE: &str = "The Master Section cannot be moved";
pub(crate) const MASTER_ABOVE: &str = "Nothing can be placed above the Master Section";

/// The lock error: `what` is `"master"`.
pub(crate) fn fixed_master(message: &str) -> ModelError {
    ModelError::Fixed {
        what: "master",
        message: message.to_owned(),
    }
}

/// True if `key` is the document's console master.
pub(crate) fn is_master(doc: &RackDoc, key: &DeviceKey) -> bool {
    doc.console.master.as_ref() == Some(key)
}

/// True while the console master is the first device of the rack (the position lock holds).
pub(crate) fn master_first(doc: &RackDoc) -> bool {
    doc.devices.first().is_some_and(|d| is_master(doc, &d.key))
}

/// A top-level rack index for a new device: `index` clamped to the rack, and to 1 or more while
/// the master is the first device (nothing goes above it).
pub(crate) fn top_index(doc: &RackDoc, index: usize) -> usize {
    let at = index.min(doc.devices.len());
    if master_first(doc) { at.max(1) } else { at }
}

/// Rejects removing the console master.
pub(crate) fn check_deletable(doc: &RackDoc, key: &DeviceKey) -> Result<(), ModelError> {
    if is_master(doc, key) {
        return Err(fixed_master(MASTER_DELETE));
    }
    Ok(())
}

/// Rejects a move that touches the master's place: while the master is first it cannot move
/// and nothing may be moved to index 0.
pub(crate) fn check_move(
    doc: &RackDoc,
    moving: &[DeviceKey],
    target_first: Option<&DeviceKey>,
) -> Result<(), ModelError> {
    if !master_first(doc) {
        return Ok(());
    }
    if moving.iter().any(|k| is_master(doc, k)) {
        return Err(fixed_master(MASTER_MOVE));
    }
    if target_first.is_some_and(|k| !is_master(doc, k)) {
        return Err(fixed_master(MASTER_ABOVE));
    }
    Ok(())
}

/// The `comp.key` switch edit that goes with a cable into the master's `key_in`: `Some` when `cable` ends at `key_in` of the console master and the master's
/// `comp.key` is off. Channels and every other socket give `None`; removing the cable never
/// switches the key off, and loading never calls this.
pub(crate) fn key_on_edit(doc: &RackDoc, registry: &Registry, cable: &Cable) -> Option<Edit> {
    if cable.to.socket != "key_in" || !is_master(doc, &cable.to.device) {
        return None;
    }
    let device = doc.device(&cable.to.device)?;
    let param = registry
        .get(&device.type_id)?
        .descriptor
        .param_by_key("comp.key")?;
    let old = device.params.get("comp.key").copied();
    if old.unwrap_or(param.default) >= 0.5 {
        return None;
    }
    Some(Edit::Param {
        key: device.key.clone(),
        param: "comp.key".to_owned(),
        old,
        new: Some(param.clamp(1.0)),
        default: param.default,
    })
}

/// Index of the device `key` in rack order.
pub(crate) fn index_of(doc: &RackDoc, key: &DeviceKey) -> Result<usize, ModelError> {
    doc.devices
        .iter()
        .position(|d| &d.key == key)
        .ok_or_else(|| ModelError::UnknownDevice(key.0.clone()))
}

/// The top-level device `key` (a path key is unknown here: the console and the song only know
/// top-level devices).
pub(crate) fn device<'d>(doc: &'d RackDoc, key: &DeviceKey) -> Result<&'d RackDevice, ModelError> {
    Ok(&doc.devices[index_of(doc, key)?])
}

/// The device at `key`, which may be a path key.
pub(crate) fn resolve<'d>(doc: &'d RackDoc, key: &DeviceKey) -> Result<&'d RackDevice, ModelError> {
    doc.resolve(key)
        .ok_or_else(|| ModelError::UnknownDevice(key.0.clone()))
}

/// Kind and direction of a socket. The interface has `in_1..in_M` (audio outputs, the host
/// inputs, M = `input_channels`) and `out_1..out_N` (audio inputs, N = `output_channels`), as
/// the engine builds it.
pub(crate) fn socket(
    doc: &RackDoc,
    registry: &Registry,
    at: &SocketRef,
) -> Result<(SocketKind, Direction), ModelError> {
    let unknown = || ModelError::UnknownKey(at.socket.clone());
    if at.device.0 == INTERFACE_KEY {
        let numbered = |prefix: &str, count: u16| {
            at.socket
                .strip_prefix(prefix)
                .and_then(|n| n.parse::<u16>().ok())
                .is_some_and(|n| (1..=count).contains(&n) && at.socket == format!("{prefix}{n}"))
        };
        return if numbered("out_", doc.output_channels) {
            Ok((SocketKind::Audio, Direction::Input))
        } else if numbered("in_", doc.input_channels) {
            Ok((SocketKind::Audio, Direction::Output))
        } else {
            Err(unknown())
        };
    }
    let dev = device(doc, &at.device)?;
    let s = descriptor(registry, &dev.type_id)?
        .socket_by_key(&at.socket)
        .ok_or_else(unknown)?;
    Ok((s.kind, s.direction))
}

/// Validates a new cable against `doc` exactly as the engine graph does: both ends exist,
/// output → input, same kind, the input is free (a summing input takes any number of
/// cables, but not the same cable twice), and no cycle (the interface is a pure sink).
///
/// With containers in the document the cycle check (and the "same cable twice" check) runs on
/// the **expanded** graph, so a container is judged by what its inside does.
pub(crate) fn check_cable(
    doc: &RackDoc,
    registry: &Registry,
    cable: &Cable,
) -> Result<(), ModelError> {
    let from = socket(doc, registry, &cable.from)?;
    let to = socket(doc, registry, &cable.to)?;
    check_endpoints(from, to, cable, &doc.cables)?;
    if doc.has_containers() {
        let mut cables = doc.cables.clone();
        cables.push(cable.clone());
        return match expanded_problem(&doc.devices, &cables, registry) {
            Some(problem) => Err(ModelError::BadCable(format!(
                "{} -> {}: {problem}",
                cable.from, cable.to
            ))),
            None => Ok(()),
        };
    }
    if would_create_cycle(doc, registry, cable) {
        return Err(ModelError::BadCable(format!(
            "{} -> {} would create a cycle (feedback loops are not supported)",
            cable.from, cable.to
        )));
    }
    Ok(())
}

/// Whether `cables` can all be added, in any order, to the (so far cable-less) top-level rack of
/// `doc`: the rules of [`check_cable`] judged once over the whole set with indexes, instead of
/// once per cable against all the cables before it. The loader uses it to take a file of
/// thousands of cables in linear time.
///
/// A `true` means every cable passes [`check_cable`] whatever its position in the list (a
/// violation needs two cables, or a loop, so it shows in the whole set whenever it would show in
/// a prefix). A `false` only says "check them one by one": it also answers for anything this
/// pass does not model (a container, an unknown device or socket, a duplicate key).
pub(crate) fn cables_fit(doc: &RackDoc, registry: &Registry, cables: &[Cable]) -> bool {
    if doc.has_containers() || !doc.cables.is_empty() {
        return false;
    }
    // Stage nodes: device `i` owns `first[i] .. first[i] + stages[i]`.
    let mut index: BTreeMap<&str, usize> = BTreeMap::new();
    let mut descs: Vec<&DeviceDescriptor> = Vec::with_capacity(doc.devices.len());
    let mut first: Vec<usize> = Vec::with_capacity(doc.devices.len() + 1);
    let mut nodes = 0usize;
    for (i, d) in doc.devices.iter().enumerate() {
        let Ok(desc) = descriptor(registry, &d.type_id) else {
            return false;
        };
        if index.insert(d.key.0.as_str(), i).is_some() {
            return false;
        }
        descs.push(desc);
        first.push(nodes);
        nodes += usize::from(desc.stage_count());
    }
    first.push(nodes);
    let mut edges: Vec<Vec<usize>> = vec![Vec::new(); nodes];
    for span in first.windows(2) {
        // Stage `s` of a device feeds its stage `s + 1`.
        for (n, out) in edges[span[0]..span[1]].iter_mut().enumerate() {
            if span[0] + n + 1 < span[1] {
                out.push(span[0] + n + 1);
            }
        }
    }
    let mut plain_inputs: BTreeSet<(&str, &str)> = BTreeSet::new();
    let mut summed: BTreeSet<(&str, &str, &str, &str)> = BTreeSet::new();
    for cable in cables {
        let (Ok(from), Ok(to)) = (
            socket(doc, registry, &cable.from),
            socket(doc, registry, &cable.to),
        ) else {
            return false;
        };
        if check_endpoints(from, to, cable, &[]).is_err() {
            return false;
        }
        let to_key = (cable.to.device.0.as_str(), cable.to.socket.as_str());
        if to.1 == Direction::Input && !plain_inputs.insert(to_key) {
            return false;
        }
        if to.1 == Direction::SummingInput
            && !summed.insert((
                cable.from.device.0.as_str(),
                cable.from.socket.as_str(),
                to_key.0,
                to_key.1,
            ))
        {
            return false;
        }
        if cable.from.device.0 == INTERFACE_KEY || cable.to.device.0 == INTERFACE_KEY {
            continue; // the interface is a pure sink: no loop runs through it
        }
        let node = |at: &SocketRef| -> Option<usize> {
            let i = *index.get(at.device.0.as_str())?;
            let s = descs[i].socket_by_key(&at.socket)?;
            Some(first[i] + usize::from(descs[i].stage_of(s.id)))
        };
        let (Some(a), Some(b)) = (node(&cable.from), node(&cable.to)) else {
            return false;
        };
        edges[a].push(b);
    }
    // Kahn: every node leaves the queue unless a loop holds it back.
    let mut indegree = vec![0u32; nodes];
    for list in &edges {
        for &n in list {
            indegree[n] += 1;
        }
    }
    let mut ready: Vec<usize> = (0..nodes).filter(|&n| indegree[n] == 0).collect();
    let mut done = 0;
    while let Some(n) = ready.pop() {
        done += 1;
        for &next in &edges[n] {
            indegree[next] -= 1;
            if indegree[next] == 0 {
                ready.push(next);
            }
        }
    }
    done == nodes
}

impl RackDoc {
    /// Whether `cable` could be added to the top-level rack: the same rules `Command::Connect`
    /// applies (kinds, directions, no cycle), judged without changing anything. With
    /// `replacing` the rules are judged as if that cable were removed first (the cable an
    /// occupied plain input gives up).
    pub fn cable_fits(
        &self,
        registry: &Registry,
        cable: &Cable,
        replacing: Option<&Cable>,
    ) -> bool {
        match replacing.filter(|r| self.cables.contains(r)) {
            None => check_cable(self, registry, cable).is_ok(),
            Some(r) => {
                let mut scratch = self.clone();
                scratch.cables.retain(|c| c != r);
                check_cable(&scratch, registry, cable).is_ok()
            }
        }
    }
}

/// The rules of one cable that do not depend on the rest of the graph: output → input, one
/// kind, a plain input takes one cable, a summing input not the same cable twice. `existing`
/// are the cables of the same scope.
pub(crate) fn check_endpoints(
    (from_kind, from_dir): (SocketKind, Direction),
    (to_kind, to_dir): (SocketKind, Direction),
    cable: &Cable,
    existing: &[Cable],
) -> Result<(), ModelError> {
    let bad = |m: String| Err(ModelError::BadCable(m));
    if from_dir != Direction::Output {
        return bad(format!(
            "{} is an input; a cable starts at an output",
            cable.from
        ));
    }
    if !to_dir.is_input() {
        return bad(format!(
            "{} is an output; a cable ends at an input",
            cable.to
        ));
    }
    if from_kind != to_kind {
        return bad(format!(
            "cannot connect {} output {} to {} input {}",
            kind_name(from_kind),
            cable.from,
            kind_name(to_kind),
            cable.to
        ));
    }
    if to_dir == Direction::Input && existing.iter().any(|c| c.to == cable.to) {
        return bad(format!("{} is already connected", cable.to));
    }
    if to_dir == Direction::SummingInput && existing.contains(cable) {
        return bad(format!(
            "{} -> {} is already connected",
            cable.from, cable.to
        ));
    }
    Ok(())
}

/// The container that holds both ends of `cable` (`None` for the top level), or `BadScope`:
/// both ends must live in the same scope, an end may be `<container path>/boundary`.
pub(crate) fn cable_scope(cable: &Cable) -> Result<Option<DeviceKey>, ModelError> {
    let (from_parent, _) = split_path(&cable.from.device);
    let (to_parent, _) = split_path(&cable.to.device);
    if from_parent != to_parent {
        return Err(ModelError::BadScope(format!(
            "{} and {} are not in the same container",
            cable.from, cable.to
        )));
    }
    Ok(from_parent)
}

/// `cable` (path keys) as the local cable of its scope.
pub(crate) fn local_cable(cable: &Cable) -> Cable {
    let local = |s: &SocketRef| SocketRef {
        device: split_path(&s.device).1,
        socket: s.socket.clone(),
    };
    Cable {
        from: local(&cable.from),
        to: local(&cable.to),
    }
}

/// Validates the cable `local` (local keys) inside the container at `container` (a path key):
/// the cable rules over that scope's devices and the boundary, a Notes input fed by a split
/// takes no cable, and the expanded graph stays acyclic with no cable twice.
pub(crate) fn check_inner_cable(
    doc: &RackDoc,
    registry: &Registry,
    container: &DeviceKey,
    local: &Cable,
) -> Result<(), ModelError> {
    let holder = resolve(doc, container)?;
    let Some(inner) = &holder.inner else {
        return Err(ModelError::BadScope(format!(
            "{container} is not a container"
        )));
    };
    let from = scope_socket(&inner.devices, registry, &local.from)?;
    let to = scope_socket(&inner.devices, registry, &local.to)?;
    check_endpoints(from, to, local, &inner.cables)?;
    if let Some(split) = inner.splits.iter().find(|s| s.target == local.to.device) {
        let notes = inner
            .devices
            .iter()
            .find(|d| d.key == split.target)
            .and_then(|d| registry.get(&d.type_id))
            .and_then(|f| notes_input(f.descriptor));
        if notes == Some(local.to.socket.as_str()) {
            return Err(ModelError::BadCable(format!(
                "{} is fed by a note split; remove the split first",
                join_socket(container, &local.to)
            )));
        }
    }
    let mut devices = doc.devices.clone();
    if let Some(scratch) = crate::container::find_relative_mut(&mut devices, &container.0)
        && let Some(i) = scratch.inner.as_mut()
    {
        i.cables.push(local.clone());
    }
    match expanded_problem(&devices, &doc.cables, registry) {
        Some(problem) => Err(ModelError::BadCable(format!(
            "{} -> {}: {problem}",
            join_socket(container, &local.from),
            join_socket(container, &local.to)
        ))),
        None => Ok(()),
    }
}

fn join_socket(container: &DeviceKey, local: &SocketRef) -> SocketRef {
    SocketRef {
        device: crate::container::join(Some(container), &local.device),
        socket: local.socket.clone(),
    }
}

/// What is wrong with the flattened graph of `devices` and `cables`, if anything: a loop (as
/// the engine judges it, per (device, stage) node), a loop of pure pass-throughs, or the same
/// flat cable twice (two containers passing one signal into one summing input).
pub(crate) fn expanded_problem(
    devices: &[RackDevice],
    cables: &[Cable],
    registry: &Registry,
) -> Option<String> {
    const CYCLE: &str = "would create a cycle (feedback loops are not supported)";
    let (flat, looped) = crate::expand::expand_with_loops(devices, cables, registry);
    if looped {
        return Some(format!(
            "{CYCLE} through containers that only pass the signal on"
        ));
    }
    if let Some(w) = flat.cables.windows(2).find(|w| w[0] == w[1]) {
        return Some(format!(
            "{} -> {} would be connected twice through containers",
            w[0].from, w[0].to
        ));
    }
    let types: BTreeMap<&DeviceKey, &str> = flat
        .devices
        .iter()
        .map(|d| (&d.key, d.type_id.as_str()))
        .collect();
    let stage_of = |at: &SocketRef| -> u8 {
        types
            .get(&at.device)
            .and_then(|t| registry.get(t))
            .and_then(|f| {
                let desc = f.descriptor;
                desc.socket_by_key(&at.socket).map(|s| desc.stage_of(s.id))
            })
            .unwrap_or(0)
    };
    let stages = |device: &DeviceKey| -> u8 {
        types
            .get(device)
            .and_then(|t| registry.get(t))
            .map_or(1, |f| f.descriptor.stage_count())
    };
    // Kahn over (device, stage) nodes.
    let mut edges: BTreeMap<(&DeviceKey, u8), Vec<(&DeviceKey, u8)>> = BTreeMap::new();
    let mut indegree: BTreeMap<(&DeviceKey, u8), usize> = BTreeMap::new();
    for d in &flat.devices {
        for s in 0..stages(&d.key) {
            indegree.entry((&d.key, s)).or_insert(0);
            if s + 1 < stages(&d.key) {
                edges.entry((&d.key, s)).or_default().push((&d.key, s + 1));
                *indegree.entry((&d.key, s + 1)).or_insert(0) += 1;
            }
        }
    }
    for c in &flat.cables {
        if c.from.device.0 == INTERFACE_KEY || c.to.device.0 == INTERFACE_KEY {
            continue;
        }
        let from = (&c.from.device, stage_of(&c.from));
        let to = (&c.to.device, stage_of(&c.to));
        if !indegree.contains_key(&from) || !indegree.contains_key(&to) {
            continue;
        }
        edges.entry(from).or_default().push(to);
        *indegree.entry(to).or_insert(0) += 1;
    }
    let mut ready: Vec<(&DeviceKey, u8)> = indegree
        .iter()
        .filter(|(_, n)| **n == 0)
        .map(|(k, _)| *k)
        .collect();
    let mut done = 0;
    while let Some(n) = ready.pop() {
        done += 1;
        for next in edges.get(&n).into_iter().flatten() {
            let d = indegree.get_mut(next).expect("node known");
            *d -= 1;
            if *d == 0 {
                ready.push(*next);
            }
        }
    }
    (done != indegree.len()).then(|| CYCLE.to_owned())
}

/// A node of the cycle check: one processing stage of a device.
type Node<'d> = (&'d DeviceKey, u8);

/// The stage of socket `at` (0 for single-stage devices and anything unknown).
fn stage(doc: &RackDoc, registry: &Registry, at: &SocketRef) -> u8 {
    doc.device(&at.device)
        .and_then(|d| registry.get(&d.type_id))
        .and_then(|f| {
            let desc = f.descriptor;
            desc.socket_by_key(&at.socket).map(|s| desc.stage_of(s.id))
        })
        .unwrap_or(0)
}

/// The number of stages of `device`.
fn stage_count(doc: &RackDoc, registry: &Registry, device: &DeviceKey) -> u8 {
    doc.device(device)
        .and_then(|d| registry.get(&d.type_id))
        .map_or(1, |f| f.descriptor.stage_count())
}

/// True if `cable` closes a loop, checked per (device, stage) node as the engine does:
/// a cable runs from (source device, stage of its source socket) to (destination device,
/// stage of its destination socket), and stage `s` of a device feeds its stage `s + 1`. So a
/// channel insert loop (`insert_send` → effect → `insert_return`) and an FX return loop
/// (master `fx_send` → effect → `return`) are not cycles. Cables touching the interface
/// never close one.
fn would_create_cycle(doc: &RackDoc, registry: &Registry, cable: &Cable) -> bool {
    if cable.from.device.0 == INTERFACE_KEY || cable.to.device.0 == INTERFACE_KEY {
        return false;
    }
    let src: Node<'_> = (&cable.from.device, stage(doc, registry, &cable.from));
    let dst: Node<'_> = (&cable.to.device, stage(doc, registry, &cable.to));
    let mut stack = vec![dst];
    let mut visited = BTreeSet::new();
    while let Some(n) = stack.pop() {
        if n == src {
            return true;
        }
        if !visited.insert(n) {
            continue;
        }
        let (device, s) = n;
        if s + 1 < stage_count(doc, registry, device) {
            stack.push((device, s + 1));
        }
        stack.extend(
            doc.cables
                .iter()
                .filter(|c| {
                    &c.from.device == device
                        && c.to.device.0 != INTERFACE_KEY
                        && stage(doc, registry, &c.from) == s
                })
                .map(|c| (&c.to.device, stage(doc, registry, &c.to))),
        );
    }
    false
}

fn kind_name(kind: SocketKind) -> &'static str {
    match kind {
        SocketKind::Audio => "an audio",
        SocketKind::Cv => "a CV",
        SocketKind::Notes => "a notes",
    }
}

/// Plans `Command::SetInputChannels(n)`: the cables from inputs above `n` go in the same step.
pub(crate) fn plan_input_channels(doc: &RackDoc, n: u16) -> Result<Edit, ModelError> {
    if n > MAX_INPUT_CHANNELS {
        return Err(ModelError::OutOfRange(format!(
            "{n} input channels is outside 0..={MAX_INPUT_CHANNELS}"
        )));
    }
    let mut edits: Vec<Edit> = doc
        .cables
        .iter()
        .filter(|c| {
            c.from.device.0 == INTERFACE_KEY
                && c.from
                    .socket
                    .strip_prefix("in_")
                    .and_then(|k| k.parse::<u16>().ok())
                    .is_some_and(|k| k > n)
        })
        .cloned()
        .map(Edit::Disconnect)
        .collect();
    edits.push(Edit::InputChannels {
        old: doc.input_channels,
        new: n,
    });
    Ok(Edit::Compound(edits))
}

/// Plans `Command::SetOutputChannels(n)`: the cables into outputs above `n` go in the same step.
pub(crate) fn plan_output_channels(doc: &RackDoc, n: u16) -> Result<Edit, ModelError> {
    if !(1..=crate::MAX_OUTPUT_CHANNELS).contains(&n) {
        return Err(ModelError::OutOfRange(format!(
            "{n} output channels is outside 1..={}",
            crate::MAX_OUTPUT_CHANNELS
        )));
    }
    let mut edits: Vec<Edit> = doc
        .cables
        .iter()
        .filter(|c| {
            c.to.device.0 == INTERFACE_KEY
                && c.to
                    .socket
                    .strip_prefix("out_")
                    .and_then(|k| k.parse::<u16>().ok())
                    .is_some_and(|k| k > n)
        })
        .cloned()
        .map(Edit::Disconnect)
        .collect();
    edits.push(Edit::OutputChannels {
        old: doc.output_channels,
        new: n,
    });
    Ok(Edit::Compound(edits))
}

/// Checks an asset id and its format.
pub(crate) fn check_asset(hash: &str, meta: &AssetMeta) -> Result<(), ModelError> {
    if !is_valid_asset_hash(hash) {
        return Err(ModelError::BadSong(format!(
            "asset id {hash:?} must be 64 lowercase hex digits"
        )));
    }
    if meta.channels == 0 || meta.sample_rate == 0 {
        return Err(ModelError::OutOfRange(format!(
            "asset {hash} has {} channels at {} Hz",
            meta.channels, meta.sample_rate
        )));
    }
    if !(1..=MAX_AUDIO_FRAMES).contains(&meta.frames) {
        return Err(ModelError::OutOfRange(format!(
            "asset {hash} has {} frames; must be 1..={MAX_AUDIO_FRAMES}",
            meta.frames
        )));
    }
    Ok(())
}

/// Plans `Command::RenameAsset`: the name is trimmed and must be 1..=`MAX_ASSET_NAME` characters
/// without control characters; the asset must be in the pool and the name must differ.
pub(crate) fn plan_rename_asset(
    doc: &RackDoc,
    asset: &str,
    name: &str,
) -> Result<Edit, ModelError> {
    let meta = doc
        .assets
        .get(asset)
        .ok_or_else(|| ModelError::MissingAsset(asset.to_owned()))?;
    let name = name.trim();
    if name.is_empty()
        || name.chars().count() > crate::MAX_ASSET_NAME
        || name.chars().any(char::is_control)
    {
        return Err(ModelError::OutOfRange(format!(
            "an asset name must be 1 to {} characters without control characters",
            crate::MAX_ASSET_NAME
        )));
    }
    if name == meta.original_name {
        return Err(ModelError::BadSong(
            "the asset already has that name".into(),
        ));
    }
    Ok(Edit::AssetName {
        asset: asset.to_owned(),
        old: meta.original_name.clone(),
        new: name.to_owned(),
    })
}

/// `Command::RegisterAsset`: adds the asset, or checks it against the one already there.
pub(crate) fn register_asset(
    doc: &mut RackDoc,
    hash: String,
    meta: AssetMeta,
) -> Result<(), ModelError> {
    check_asset(&hash, &meta)?;
    match doc.assets.get(&hash) {
        Some(have)
            if (have.channels, have.sample_rate, have.frames)
                == (meta.channels, meta.sample_rate, meta.frames) => {}
        Some(have) => {
            return Err(ModelError::BadSong(format!(
                "asset {hash} is already in the pool as {} ch, {} Hz, {} frames",
                have.channels, have.sample_rate, have.frames
            )));
        }
        None => {
            doc.assets.insert(hash, meta);
        }
    }
    Ok(())
}

/// The key base for a type: the part after the last `.`, lowercased, with every character
/// outside `[a-z0-9_-]` replaced by `-`, prefixed with `d` if it does not start with a letter.
fn key_base(type_id: &str) -> String {
    let tail = type_id.rsplit('.').next().unwrap_or(type_id);
    let mut base: String = tail
        .chars()
        .map(|c| {
            let c = c.to_ascii_lowercase();
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if !base.starts_with(|c: char| c.is_ascii_lowercase()) {
        base.insert(0, 'd');
    }
    if base == INTERFACE_KEY || base == BOUNDARY_KEY {
        base.push_str("-device");
    }
    base
}

/// The first of `base`, `base-2`, `base-3`, … that is not used in `doc`.
pub(crate) fn generate_key(doc: &RackDoc, type_id: &str) -> DeviceKey {
    generate_key_in(&doc.devices, type_id)
}

/// [`generate_key`] among the devices of one scope.
pub(crate) fn generate_key_in(devices: &[RackDevice], type_id: &str) -> DeviceKey {
    generate_key_with_base_in(devices, &key_base(type_id))
}

/// The first of `base`, `base-2`, `base-3`, … that is not used among `devices`; `base` must be
/// a valid key (a plugin device is keyed after the plugin's name).
pub(crate) fn generate_key_with_base_in(devices: &[RackDevice], base: &str) -> DeviceKey {
    // One pass over the devices, then one lookup per candidate: with many devices of a type the
    // candidates run into the hundreds.
    let taken: BTreeSet<&str> = devices.iter().map(|d| d.key.0.as_str()).collect();
    if !taken.contains(base) {
        return DeviceKey(base.to_owned());
    }
    (2u64..)
        .map(|n| format!("{base}-{n}"))
        .find(|k| !taken.contains(k.as_str()))
        .map(DeviceKey)
        .expect("a free key exists")
}

/// The first of `name`, `name 2`, `name 3`, … that no device in `doc` is called.
pub(crate) fn unique_name(doc: &RackDoc, name: &str) -> String {
    unique_name_in(&doc.devices, name)
}

/// The name a device of `key` gets when its name is cleared: its type's name, made unique
/// among the other devices of its scope (the top level, or the inside of its container).
pub(crate) fn default_name(
    doc: &RackDoc,
    registry: &Registry,
    key: &DeviceKey,
    device: &RackDevice,
) -> Result<String, ModelError> {
    let desc = descriptor(registry, &device.type_id)?;
    let (container, local) = crate::container::split_path(key);
    let siblings: Vec<RackDevice> = doc
        .scope_devices(container.as_ref())
        .unwrap_or_default()
        .iter()
        .filter(|d| d.key != local)
        .cloned()
        .collect();
    Ok(unique_name_in(&siblings, desc.name))
}

/// [`unique_name`] among the devices of one scope.
pub(crate) fn unique_name_in(devices: &[RackDevice], name: &str) -> String {
    let taken: BTreeSet<&str> = devices.iter().map(|d| d.name.as_str()).collect();
    if !taken.contains(name) {
        return name.to_owned();
    }
    (2u64..)
        .map(|n| format!("{name} {n}"))
        .find(|n| !taken.contains(n.as_str()))
        .expect("a free name exists")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_base_sanitizes_type_ids() {
        assert_eq!(key_base("atomik.poly-synth"), "poly-synth");
        assert_eq!(key_base("acme.Big Verb"), "big-verb");
        assert_eq!(key_base("x.9lives"), "d9lives");
        assert_eq!(key_base("plain"), "plain");
        assert_eq!(key_base("x.interface"), "interface-device");
        assert!(is_valid_key(&key_base("weird.!!")));
    }
}
