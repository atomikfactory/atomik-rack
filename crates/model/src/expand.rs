//! Expansion: compiling containers away.
//!
//! [`RackDoc::expand`] is a pure, deterministic function from the rack tree to the flat device
//! and cable lists the engine understands, so a container costs the engine nothing:
//!
//! * the container itself is **not** in the flat list; every inner device becomes a flat device
//!   keyed by its **path** (`layer/synth`) with its type, params (macro values applied), zones
//!   and steps;
//! * per container one synthetic `atomik.note-split` device keyed `<container path>/~split`,
//!   its params set from the splits (slot `k`: `sK.on`, `sK.key_lo`, `sK.key_hi`, `sK.vel_lo`,
//!   `sK.vel_hi`, `sK.transpose`);
//! * cables are **spliced**: a cable into `container:in_l` and an inner cable
//!   `boundary:in_l -> X:s` become `source -> X:s` (one per inner target); `Y:o ->
//!   boundary:out_l` and `container:out_l -> Z:s` become `Y:o -> Z:s`; an inside
//!   `boundary:in_l -> boundary:out_l` passes through; a boundary end with nothing on the other
//!   side makes no cable (a silent edge, not an error); a Notes cable into `container:notes_in`
//!   goes to `<container>/~split:notes_in`, and `~split:out_k` goes to the target of split `k`;
//! * the order is rack order; right after a container's slot its `~split`, then its inner devices
//!   in document order, recursively. The engine id of a flat device is its 1-based position.
//!
//! [`ExpandedRack::alias`] maps every container key to its `~split` device (notes for song
//! tracks and live audition). [`ExpandedRack::macro_targets`] lists the macro targets with the
//! value each one is set to; the app turns a macro value into one `set_param` per target
//! ([`ExpandedRack::macro_updates`]) and the bounce builder expands macro automation with
//! [`expand_macro_lane`]. [`macro_value`] is the one mapping function: `+ - *` only, no libm.
//!
//! # Engine-facing sites to convert
//!
//! Every place that walks `RackDoc::devices` or `RackDoc::cables` to build or feed the engine
//! must walk the expansion instead.

use crate::container::{
    BOUNDARY_KEY, ContainerDoc, MACRO_COUNT, NoteSplit, SPLIT_TYPE, find_relative, notes_input,
};
use crate::{
    AutomationPoint, Cable, DeviceKey, INTERFACE_KEY, PluginDoc, RackDevice, RackDoc, SocketRef,
    StepSeqDoc, ZoneDoc,
};
use atomik_core::ParamDescriptor;
use atomik_device_sdk::Registry;
use std::collections::BTreeMap;

/// One flat device (a device of the engine graph).
#[derive(Debug, Clone, PartialEq)]
pub struct ExpandedDevice {
    /// The path key (`synth`, `layer/synth`, `layer/~split`): the key the app uses everywhere.
    pub key: DeviceKey,
    /// Device type id.
    pub type_id: String,
    /// Params to set after the device is created (stored values, macro values applied).
    pub params: BTreeMap<String, f32>,
    /// Sampler zones (empty for every device that does not take samples).
    pub zones: Vec<ZoneDoc>,
    /// Step sequencer patterns.
    pub steps: Option<StepSeqDoc>,
    /// A plugin device's document: the app and the bounce build the real
    /// plugin (or its stand-in) from it. `None` for every other device.
    pub plugin: Option<Box<PluginDoc>>,
    /// The device belongs to a frozen track (see [`crate::freeze`]): it keeps its place in the
    /// list, and so its engine id, but the engine neither builds nor processes it, and no cable
    /// touches it. `false` for every device of a document without a frozen track.
    pub frozen: bool,
}

/// A macro target with the value the macro currently gives it.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedTarget {
    /// The driven device (path key).
    pub device: DeviceKey,
    /// The param key.
    pub param: String,
    /// Value at macro 0.
    pub min: f32,
    /// Value at macro 1.
    pub max: f32,
    /// The value the target is set to now (`macro_value` of the effective macro value).
    pub value: f32,
}

/// The flattened rack.
#[derive(Debug, Clone, PartialEq)]
pub struct ExpandedRack {
    /// Flat devices in the deterministic order (the order of engine ids).
    pub devices: Vec<ExpandedDevice>,
    /// Flat cables, sorted; ends are path keys (and `interface`).
    pub cables: Vec<Cable>,
    /// Every container's path key, mapped to its `~split` device.
    pub alias: BTreeMap<DeviceKey, DeviceKey>,
    /// `(container path, macro index 0..8, targets)` for every macro that has targets, outermost
    /// container first (the order macros are evaluated in).
    pub macro_targets: Vec<(DeviceKey, u8, Vec<ResolvedTarget>)>,
}

impl ExpandedRack {
    /// One `set_param` per engine device for the macros of `container` (a path key) and of every
    /// container inside it: `(device path, param key, value)`. Targets that are the macro param
    /// of an inner container are not engine devices and are skipped (the inner container's own
    /// entries follow).
    pub fn macro_updates(&self, container: &DeviceKey) -> Vec<(DeviceKey, String, f32)> {
        let inside = format!("{}/", container.0);
        let mut out = Vec::new();
        for (c, _, targets) in &self.macro_targets {
            if c != container && !c.0.starts_with(&inside) {
                continue;
            }
            for t in targets {
                if !self.alias.contains_key(&t.device) {
                    out.push((t.device.clone(), t.param.clone(), t.value));
                }
            }
        }
        out
    }
}

/// Maps macro value `m` onto a param: `min + (max - min) * m`, clamped to the descriptor
/// range, and for discrete and choice params rounded to the nearest step (ties up). A NaN gives
/// the descriptor default. Only IEEE `+ - *` and comparisons, so every platform agrees.
pub fn macro_value(min: f32, max: f32, m: f32, p: &ParamDescriptor) -> f32 {
    let v = min + (max - min) * m;
    if v.is_nan() {
        return p.default;
    }
    let v = if v < p.min {
        p.min
    } else if v > p.max {
        p.max
    } else {
        v
    };
    if !p.discrete {
        return v;
    }
    // Round half up by comparison arithmetic: `v - p.min` is not negative here.
    let steps = (v - p.min + 0.5) as i64;
    let r = p.min + steps as f32;
    if r > p.max { p.max } else { r }
}

/// The flat lanes a macro automation lane expands to: one lane per engine target,
/// `(device path, param key, points)`, with every point mapped by [`macro_value`] (a discrete
/// target rounds each point). A target that is the macro of an inner container chains through
/// that container's own mapping. `None` when `container` is not a container or `param` is not
/// one of its `macroN` params; an unmapped macro gives no lanes.
pub fn expand_macro_lane(
    doc: &RackDoc,
    registry: &Registry,
    container: &DeviceKey,
    param: &str,
    points: &[AutomationPoint],
) -> Option<Vec<(DeviceKey, String, Vec<AutomationPoint>)>> {
    let index = macro_index(param)?;
    let inner = doc.inner(container)?;
    let mut out = Vec::new();
    lanes(inner, registry, container, index, points, &mut out);
    Some(out)
}

fn macro_index(param: &str) -> Option<usize> {
    let n: usize = param.strip_prefix("macro")?.parse().ok()?;
    (1..=MACRO_COUNT)
        .contains(&n)
        .then_some(n - 1)
        .filter(|_| param == format!("macro{n}"))
}

fn lanes(
    inner: &ContainerDoc,
    registry: &Registry,
    container: &DeviceKey,
    index: usize,
    points: &[AutomationPoint],
    out: &mut Vec<(DeviceKey, String, Vec<AutomationPoint>)>,
) {
    for t in &inner.macros[index].targets {
        let Some(dev) = find_relative(&inner.devices, &t.device.0) else {
            continue;
        };
        let Some(pd) = registry
            .get(&dev.type_id)
            .and_then(|f| f.descriptor.param_by_key(&t.param))
        else {
            continue;
        };
        let mapped: Vec<AutomationPoint> = points
            .iter()
            .map(|p| AutomationPoint {
                value: macro_value(t.min, t.max, p.value, pd),
                ..*p
            })
            .collect();
        let path = DeviceKey(format!("{}/{}", container.0, t.device.0));
        match (&dev.inner, macro_index(&t.param)) {
            (Some(next), Some(k)) => lanes(next, registry, &path, k, &mapped, out),
            _ => out.push((path, t.param.clone(), mapped)),
        }
    }
}

// ------------------------------------------------------------------------- the expansion

/// One scope of the tree (the top level or a container's inside).
struct ScopeData<'a> {
    /// `""` at the top, else `layer/` (with the trailing slash).
    prefix: String,
    devices: &'a [RackDevice],
    cables: &'a [Cable],
    splits: &'a [NoteSplit],
    /// The parent scope and the container's local key in it.
    parent: Option<(usize, &'a DeviceKey)>,
    /// Local key of a nested container to its scope.
    children: BTreeMap<&'a DeviceKey, usize>,
}

struct Builder<'a> {
    registry: &'a Registry,
    scopes: Vec<ScopeData<'a>>,
    devices: Vec<ExpandedDevice>,
    cables: Vec<Cable>,
    alias: BTreeMap<DeviceKey, DeviceKey>,
    macro_targets: Vec<(DeviceKey, u8, Vec<ResolvedTarget>)>,
    /// Params set by macros: device path to param to value.
    overrides: BTreeMap<String, BTreeMap<String, f32>>,
    /// A pure pass-through loop was found (no device on it).
    looped: bool,
}

/// How many scope hops a signal trace may take before it is a loop.
const MAX_HOPS: usize = 4096;

impl RackDoc {
    /// Flattens the containers (see the module documentation of `crate::expand`). Pure and
    /// deterministic; a document without containers expands to its own device and cable lists.
    /// The frozen tracks of the document are applied last (see [`crate::freeze`]): their devices
    /// are flagged and unplugged and a `<track>/~freeze` player takes their place.
    pub fn expand(&self, registry: &Registry) -> ExpandedRack {
        let mut expanded = expand_with_loops(&self.devices, &self.cables, registry).0;
        crate::macros::apply(self, registry, &mut expanded);
        crate::freeze::apply(self, &mut expanded);
        expanded
    }
}

/// [`RackDoc::expand`] and whether a device-less pass-through loop was found.
pub(crate) fn expand_with_loops(
    devices: &[RackDevice],
    cables: &[Cable],
    registry: &Registry,
) -> (ExpandedRack, bool) {
    let mut b = Builder {
        registry,
        scopes: Vec::new(),
        devices: Vec::new(),
        cables: Vec::new(),
        alias: BTreeMap::new(),
        macro_targets: Vec::new(),
        overrides: BTreeMap::new(),
        looped: false,
    };
    let top = b.build_scope(devices, cables, &[], String::new(), None);
    b.emit_devices(top);
    b.emit_cables(top);
    b.cables.sort();
    let looped = b.looped;
    (
        ExpandedRack {
            devices: b.devices,
            cables: b.cables,
            alias: b.alias,
            macro_targets: b.macro_targets,
        },
        looped,
    )
}

impl<'a> Builder<'a> {
    fn build_scope(
        &mut self,
        devices: &'a [RackDevice],
        cables: &'a [Cable],
        splits: &'a [NoteSplit],
        prefix: String,
        parent: Option<(usize, &'a DeviceKey)>,
    ) -> usize {
        let index = self.scopes.len();
        self.scopes.push(ScopeData {
            prefix: prefix.clone(),
            devices,
            cables,
            splits,
            parent,
            children: BTreeMap::new(),
        });
        for d in devices {
            if let Some(inner) = &d.inner {
                let child = self.build_scope(
                    &inner.devices,
                    &inner.cables,
                    &inner.splits,
                    format!("{prefix}{}/", d.key.0),
                    Some((index, &d.key)),
                );
                self.scopes[index].children.insert(&d.key, child);
            }
        }
        index
    }

    fn effective_macro(&self, path: &str, device: &RackDevice, k: usize) -> f32 {
        let key = format!("macro{}", k + 1);
        self.overrides
            .get(path)
            .and_then(|o| o.get(&key))
            .copied()
            .or_else(|| device.params.get(&key).copied())
            .unwrap_or(0.0)
    }

    /// Pass 1: flat devices in order, macro values, split params, aliases.
    fn emit_devices(&mut self, scope: usize) {
        let devices = self.scopes[scope].devices;
        let prefix = self.scopes[scope].prefix.clone();
        for d in devices {
            let path = format!("{prefix}{}", d.key.0);
            let Some(inner) = &d.inner else {
                let mut params = d.params.clone();
                if let Some(o) = self.overrides.get(&path) {
                    params.extend(o.iter().map(|(k, v)| (k.clone(), *v)));
                }
                self.devices.push(ExpandedDevice {
                    key: DeviceKey(path),
                    type_id: d.type_id.clone(),
                    params,
                    zones: d.zones.clone(),
                    steps: d.steps.clone(),
                    plugin: d.plugin.clone(),
                    frozen: false,
                });
                continue;
            };
            let split_key = DeviceKey(format!("{path}/~split"));
            self.alias
                .insert(DeviceKey(path.clone()), split_key.clone());
            for k in 0..MACRO_COUNT {
                self.apply_macro(&path, d, inner, k);
            }
            self.devices.push(ExpandedDevice {
                key: split_key,
                type_id: SPLIT_TYPE.to_owned(),
                params: split_params(&inner.splits),
                zones: Vec::new(),
                steps: None,
                plugin: None,
                frozen: false,
            });
            let child = self.scopes[scope].children[&d.key];
            self.emit_devices(child);
        }
    }

    /// Evaluates macro `k` of the container `d` at `path`: records the target values as
    /// overrides (outermost first, so an inner container sees its driven `macroN`).
    fn apply_macro(&mut self, path: &str, d: &RackDevice, inner: &ContainerDoc, k: usize) {
        let m = &inner.macros[k];
        if m.targets.is_empty() {
            return;
        }
        let value = self.effective_macro(path, d, k);
        let mut resolved = Vec::new();
        for t in &m.targets {
            let Some(dev) = find_relative(&inner.devices, &t.device.0) else {
                continue;
            };
            let Some(pd) = self
                .registry
                .get(&dev.type_id)
                .and_then(|f| f.descriptor.param_by_key(&t.param))
            else {
                continue;
            };
            let v = macro_value(t.min, t.max, value, pd);
            let device = DeviceKey(format!("{path}/{}", t.device.0));
            self.overrides
                .entry(device.0.clone())
                .or_default()
                .insert(t.param.clone(), v);
            resolved.push(ResolvedTarget {
                device,
                param: t.param.clone(),
                min: t.min,
                max: t.max,
                value: v,
            });
        }
        if !resolved.is_empty() {
            self.macro_targets
                .push((DeviceKey(path.to_owned()), k as u8, resolved));
        }
    }

    /// The flat output feeding the cable end `from` of `scope`, if any.
    fn source(&self, scope: usize, from: &SocketRef, hops: usize) -> Result<Option<SocketRef>, ()> {
        if hops > MAX_HOPS {
            return Err(());
        }
        let sc = &self.scopes[scope];
        let device = &from.device;
        if device.0 == INTERFACE_KEY && sc.parent.is_none() {
            return Ok(Some(from.clone()));
        }
        if device.0 == BOUNDARY_KEY {
            // An output of the boundary: what arrives at the container.
            let Some((parent, container)) = sc.parent else {
                return Ok(None);
            };
            let Some(cable) = self.scopes[parent]
                .cables
                .iter()
                .find(|c| &c.to.device == container && c.to.socket == from.socket)
            else {
                return Ok(None);
            };
            return self.source(parent, &cable.from, hops + 1);
        }
        if let Some(&child) = sc.children.get(device) {
            // An output of a nested container: what its inside sends to its boundary.
            let Some(cable) = self.scopes[child]
                .cables
                .iter()
                .find(|c| c.to.device.0 == BOUNDARY_KEY && c.to.socket == from.socket)
            else {
                return Ok(None);
            };
            return self.source(child, &cable.from, hops + 1);
        }
        if !sc.devices.iter().any(|d| &d.key == device) {
            return Ok(None);
        }
        Ok(Some(SocketRef {
            device: DeviceKey(format!("{}{}", sc.prefix, device.0)),
            socket: from.socket.clone(),
        }))
    }

    /// Pass 2: the flat cables of `scope` and, recursively, of every container inside.
    fn emit_cables(&mut self, scope: usize) {
        let cables: Vec<Cable> = self.scopes[scope].cables.to_vec();
        let prefix = self.scopes[scope].prefix.clone();
        for c in &cables {
            let to_device = &c.to.device;
            if to_device.0 == BOUNDARY_KEY {
                continue; // consumed by the trace outward
            }
            let to = if let Some(&child) = self.scopes[scope].children.get(to_device) {
                // Audio into a container is consumed inward; notes go to its split.
                if !self.is_notes_socket(scope, to_device, &c.to.socket) {
                    continue;
                }
                SocketRef {
                    device: DeviceKey(format!("{}~split", self.scopes[child].prefix)),
                    socket: "notes_in".to_owned(),
                }
            } else if to_device.0 == INTERFACE_KEY && self.scopes[scope].parent.is_none() {
                c.to.clone()
            } else if self.scopes[scope]
                .devices
                .iter()
                .any(|d| &d.key == to_device)
            {
                SocketRef {
                    device: DeviceKey(format!("{prefix}{}", to_device.0)),
                    socket: c.to.socket.clone(),
                }
            } else {
                continue;
            };
            match self.source(scope, &c.from, 0) {
                Ok(Some(from)) => self.cables.push(Cable { from, to }),
                Ok(None) => {}
                Err(()) => self.looped = true,
            }
        }
        // The splits of the container whose inside this scope is.
        if self.scopes[scope].parent.is_some() {
            let splits = self.scopes[scope].splits;
            for (i, s) in splits.iter().enumerate() {
                let from = SocketRef {
                    device: DeviceKey(format!("{prefix}~split")),
                    socket: format!("out_{}", i + 1),
                };
                if let Some(&child) = self.scopes[scope].children.get(&s.target) {
                    self.cables.push(Cable {
                        from,
                        to: SocketRef {
                            device: DeviceKey(format!("{}~split", self.scopes[child].prefix)),
                            socket: "notes_in".to_owned(),
                        },
                    });
                } else if let Some(d) = self.scopes[scope]
                    .devices
                    .iter()
                    .find(|d| d.key == s.target)
                {
                    let socket = self
                        .registry
                        .get(&d.type_id)
                        .and_then(|f| notes_input(f.descriptor));
                    if let Some(socket) = socket {
                        self.cables.push(Cable {
                            from,
                            to: SocketRef {
                                device: DeviceKey(format!("{prefix}{}", s.target.0)),
                                socket: socket.to_owned(),
                            },
                        });
                    }
                }
            }
        }
        let devices = self.scopes[scope].devices;
        for d in devices {
            if let Some(&child) = self.scopes[scope].children.get(&d.key) {
                self.emit_cables(child);
            }
        }
    }

    fn is_notes_socket(&self, scope: usize, container: &DeviceKey, socket: &str) -> bool {
        self.scopes[scope]
            .devices
            .iter()
            .find(|d| &d.key == container)
            .and_then(|d| self.registry.get(&d.type_id))
            .and_then(|f| f.descriptor.socket_by_key(socket))
            .is_some_and(|s| s.kind == atomik_device_sdk::SocketKind::Notes)
    }
}

/// The split device's params for a split list (unused slots stay at their defaults).
fn split_params(splits: &[NoteSplit]) -> BTreeMap<String, f32> {
    let mut p = BTreeMap::new();
    for (i, s) in splits.iter().enumerate() {
        let k = i + 1;
        p.insert(format!("s{k}.on"), 1.0);
        p.insert(format!("s{k}.key_lo"), f32::from(s.key_lo));
        p.insert(format!("s{k}.key_hi"), f32::from(s.key_hi));
        p.insert(format!("s{k}.vel_lo"), f32::from(s.vel_lo));
        p.insert(format!("s{k}.vel_hi"), f32::from(s.vel_hi));
        p.insert(format!("s{k}.transpose"), f32::from(s.transpose));
    }
    p
}

impl RackDoc {
    /// The `set_param` calls for the macros of `container` (a path key) and of every container
    /// inside it at the current macro values: `(device path, param key, value)`. A convenience
    /// over [`RackDoc::expand`] and [`ExpandedRack::macro_updates`] for the app's reaction to a
    /// macro change ([`crate::SyncOp::MacrosChanged`], a `ParamChanged` of `macroN`).
    pub fn macro_updates(
        &self,
        registry: &Registry,
        container: &DeviceKey,
    ) -> Vec<(DeviceKey, String, f32)> {
        self.expand(registry).macro_updates(container)
    }
}
