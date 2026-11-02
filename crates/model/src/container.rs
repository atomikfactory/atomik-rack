//! The container: a device whose inside is another rack.
//!
//! A [`ContainerDoc`] holds inner devices, inner cables (with **local** keys), eight macros
//! and up to eight note splits. Inner devices are addressed from outside by **path keys**
//! (`layer/synth`, `layer/fx/comp`; `/` and `~` are not valid in a device key). The reserved
//! pseudo-device key [`BOUNDARY_KEY`] names the container's inside edge in an inner cable: its
//! Audio outputs `in_l`, `in_r` carry what arrives at the container, its Audio inputs `out_l`,
//! `out_r` leave through the container. The boundary has no notes socket (notes enter through
//! the splits).
//!
//! Containers vanish before the engine sees them: [`RackDoc::expand`] compiles the tree into a
//! flat device and cable list (see [`crate::expand`]). Everything here is about the document:
//! the types, path resolution, and the validation that every command and the project loader
//! share ([`RackDoc::check_containers`]).

use crate::console::{self, BUS_TYPE, CHANNEL_TYPE, MASTER_TYPE};
use crate::song::AUDIO_TRACK_TYPE;
use crate::{Cable, DeviceKey, INTERFACE_KEY, ModelError, RackDevice, RackDoc, SocketRef, rules};
use atomik_device_sdk::{DeviceDescriptor, DeviceKind, Direction, Registry, SocketKind};
use std::collections::BTreeMap;

/// Nesting depth limit: a path has at most this many container segments.
pub const MAX_CONTAINER_DEPTH: usize = 3;
/// Most devices in one container.
pub const MAX_INNER_DEVICES: usize = 32;
/// Most inner devices in the whole project (the engine arena holds 4096).
pub const MAX_PROJECT_INNER: usize = 256;
/// Most note splits in one container.
pub const MAX_SPLITS: usize = 8;
/// Macros per container.
pub const MACRO_COUNT: usize = 8;
/// Most targets of one macro.
pub const MAX_MACRO_TARGETS: usize = 32;
/// Most macro targets in one container.
pub const MAX_CONTAINER_TARGETS: usize = 128;
/// Longest macro name, in characters.
pub const MAX_MACRO_NAME_CHARS: usize = 24;
/// The reserved pseudo-device key of a container's inside edge.
pub const BOUNDARY_KEY: &str = "boundary";
/// Type id of the container device.
pub const CONTAINER_TYPE: &str = "atomik.container";
/// Type id of the internal note split device the expansion creates.
pub const SPLIT_TYPE: &str = "atomik.note-split";
/// Device types that cannot live inside a container (console devices and audio tracks; the
/// interface is not a device in the list at all).
pub const FORBIDDEN_INSIDE: [&str; 4] = [CHANNEL_TYPE, BUS_TYPE, MASTER_TYPE, AUDIO_TRACK_TYPE];

/// A container's inside.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ContainerDoc {
    /// Inner devices in rack order (keys are local to the container).
    pub devices: Vec<RackDevice>,
    /// Inner cables, in canonical order (sorted), with local keys; an end may be
    /// [`BOUNDARY_KEY`].
    pub cables: Vec<Cable>,
    /// The eight macros.
    pub macros: [MacroDoc; MACRO_COUNT],
    /// Note splits, in slot order (the first is the split device's `out_1`).
    pub splits: Vec<NoteSplit>,
    /// The inside is unfolded in the rack: the container's devices are drawn under its panel.
    /// Presentation only (the engine never sees it); stored only when `true`, as the optional
    /// `"open"` field of `"inner"` (still the same project format).
    pub open: bool,
}

/// One macro: a name and the inner params it drives.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MacroDoc {
    /// User name, at most [`MAX_MACRO_NAME_CHARS`] characters; empty means "Macro N".
    pub name: String,
    /// The driven params, at most [`MAX_MACRO_TARGETS`].
    pub targets: Vec<MacroTarget>,
}

/// One macro target: macro value `m` (0 to 1) sets the param to `min + (max - min) * m`.
#[derive(Debug, Clone, PartialEq)]
pub struct MacroTarget {
    /// The driven device, as a path relative to the container (`synth`, `fx/comp`).
    pub device: DeviceKey,
    /// The param key. On an inner container only its `macroN` params may be driven.
    pub param: String,
    /// Value at macro 0 (may be above `max`: inverted).
    pub min: f32,
    /// Value at macro 1.
    pub max: f32,
}

/// One note split: notes in the key and velocity range go to `target`, transposed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteSplit {
    /// Local key of an inner device with a Notes input (one split per target).
    pub target: DeviceKey,
    /// Lowest key, 0..=127.
    pub key_lo: u8,
    /// Highest key, `key_lo..=127`.
    pub key_hi: u8,
    /// Lowest velocity, 1..=127.
    pub vel_lo: u8,
    /// Highest velocity, `vel_lo..=127`.
    pub vel_hi: u8,
    /// Transpose in semitones, -48..=48.
    pub transpose: i8,
}

impl NoteSplit {
    /// A split of the whole keyboard and every velocity, untransposed.
    pub fn full(target: DeviceKey) -> Self {
        Self {
            target,
            key_lo: 0,
            key_hi: 127,
            vel_lo: 1,
            vel_hi: 127,
            transpose: 0,
        }
    }
}

/// A container edit. Each is one undo step; a rejected command changes nothing.
#[derive(Debug, Clone, PartialEq)]
pub enum ContainerCommand {
    /// Add a device inside `container` (a path key) at `index` (clamped). `key` is a **local**
    /// key (generated when `None`). With `auto_split`, a device with a Notes input also gets a
    /// full-range split (when there is room), in the same step.
    AddInner {
        /// The container (path key).
        container: DeviceKey,
        /// Position among its devices.
        index: usize,
        /// Device type.
        type_id: String,
        /// Local key, or `None` to generate one.
        key: Option<DeviceKey>,
        /// Also add a full-range split to the new device.
        auto_split: bool,
    },
    /// Replace the split list. Consecutive commands with the same `merge` id on the same
    /// container merge into one undo step (drags; ids from 7 * 2^40).
    SetSplits {
        /// The container (path key).
        container: DeviceKey,
        /// The new list.
        splits: Vec<NoteSplit>,
        /// Drag-merge group.
        merge: Option<u64>,
    },
    /// Add or edit the target of that device and param on macro `index` (0-based). The param
    /// is driven by that macro only. `merge` merges range drags.
    SetMacro {
        /// The container (path key).
        container: DeviceKey,
        /// Macro index, 0..8.
        index: u8,
        /// The target.
        target: MacroTarget,
        /// Drag-merge group.
        merge: Option<u64>,
    },
    /// Remove the target of `device` and `param` from macro `index`.
    RemoveMacroTarget {
        /// The container (path key).
        container: DeviceKey,
        /// Macro index, 0..8.
        index: u8,
        /// Target device (relative path).
        device: DeviceKey,
        /// Param key.
        param: String,
    },
    /// Name a macro (at most [`MAX_MACRO_NAME_CHARS`] characters; empty clears it).
    RenameMacro {
        /// The container (path key).
        container: DeviceKey,
        /// Macro index, 0..8.
        index: u8,
        /// The name.
        name: String,
    },
    /// Unfold (`open`) or fold the container `container` (a path key) in the rack view: whether
    /// its devices are drawn under its panel. Presentation only; one undo step.
    SetOpen {
        /// The container (path key).
        container: DeviceKey,
        /// Whether the inside is unfolded.
        open: bool,
    },
    /// Deep copy of a container, placed right after the original: a new key, the name
    /// `<name> copy`, no outer cables, no console channel, no song track.
    DuplicateContainer {
        /// The container to copy (path key).
        container: DeviceKey,
    },
    /// Wrap the top-level devices `keys` in a new container called `name` (empty: "Container"),
    /// placed at the first device's slot, in one undo step. The cables between the devices stay
    /// inside; every cable to a device outside goes through the container's own sockets, the
    /// first source on `in_l`, and a cable the container has no socket for (CV, a third audio
    /// input or output, a notes output, a second notes source) refuses the command and names
    /// each such cable. A device with a song track, a console channel or an FX return slot
    /// passes them to the container (at most one such device may be combined); the track's
    /// automation lanes are dropped. A device a container cannot hold (a mixer strip, an audio
    /// track) stays in the rack and the rest is combined. See [`RackDoc::container_move_notes`]
    /// for what is dropped and what stays.
    Combine {
        /// The top-level devices to combine.
        keys: Vec<DeviceKey>,
        /// The container's name; empty for the default.
        name: String,
    },
    /// Dissolve the top-level container `container`: its devices come back to the rack where it
    /// was, the container's cables are joined to theirs, and its track, channels and FX return go
    /// to the device that carries them (see [`ContainerCommand::Combine`]). The container's macros
    /// and their automation are dropped. Refused when a split narrows or transposes notes that a
    /// cable or the song sends, when the track would have to play several devices, or when no
    /// device can take the track, channel or return.
    Uncombine {
        /// The container (top-level key).
        container: DeviceKey,
    },
    /// Move the top-level device `key` into the top-level container `container` at position
    /// `index` among its devices (clamped), splicing its cables through the container's sockets as
    /// [`ContainerCommand::Combine`] does. A device with a song track, a console channel or an FX
    /// return is refused (combine it into a new container instead).
    MoveIntoContainer {
        /// The device (top-level key).
        key: DeviceKey,
        /// The container (top-level key).
        container: DeviceKey,
        /// Position among the container's devices.
        index: usize,
    },
    /// Move the device `key` (a path key `container/device` of a top-level container) out to the
    /// rack at `index` (clamped; never above the master). Its cables to the container's boundary
    /// join what the container's sockets were cabled to; a cable to a device that stays inside
    /// refuses the command.
    MoveOutOfContainer {
        /// The inner device (path key).
        key: DeviceKey,
        /// Rack position.
        index: usize,
    },
}

// ------------------------------------------------------------------------- paths

/// Splits a path key into its parent path and local key: `a/b/c` is (`Some(a/b)`, `c`); a
/// top-level key has no parent.
pub fn split_path(key: &DeviceKey) -> (Option<DeviceKey>, DeviceKey) {
    match key.0.rsplit_once('/') {
        Some((parent, local)) => (
            Some(DeviceKey(parent.to_owned())),
            DeviceKey(local.to_owned()),
        ),
        None => (None, key.clone()),
    }
}

/// The scope and the view form of an expanded (flat) cable: `a/x:o -> a/y:i` is the cable
/// `x:o -> y:i` of the container `a`'s inside; a top-level cable keeps its keys and has no
/// scope. `None` for a cable that has no counterpart in one scope's view: ends in different
/// scopes (the flat cable of a splice) or on a container's `~split`.
pub fn scoped_cable(cable: &crate::Cable) -> Option<(Option<DeviceKey>, crate::Cable)> {
    if cable.from.device.0.contains('~') || cable.to.device.0.contains('~') {
        return None;
    }
    let (scope, from) = split_path(&cable.from.device);
    let (to_scope, to) = split_path(&cable.to.device);
    (scope == to_scope).then(|| {
        let end = |device, at: &crate::SocketRef| crate::SocketRef {
            device,
            socket: at.socket.clone(),
        };
        (
            scope,
            crate::Cable {
                from: end(from, &cable.from),
                to: end(to, &cable.to),
            },
        )
    })
}

/// `path` extended by the local key `local` (`a`, `b` is `a/b`).
pub(crate) fn join(parent: Option<&DeviceKey>, local: &DeviceKey) -> DeviceKey {
    match parent {
        Some(p) => DeviceKey(format!("{}/{}", p.0, local.0)),
        None => local.clone(),
    }
}

/// The device at the relative path `path` (`a` or `a/b`) inside `devices`.
pub(crate) fn find_relative<'d>(devices: &'d [RackDevice], path: &str) -> Option<&'d RackDevice> {
    let mut parts = path.split('/');
    let first = parts.next()?;
    let mut dev = devices.iter().find(|d| d.key.0 == first)?;
    for p in parts {
        dev = dev.inner.as_ref()?.devices.iter().find(|d| d.key.0 == p)?;
    }
    Some(dev)
}

/// Mutable [`find_relative`].
pub(crate) fn find_relative_mut<'d>(
    devices: &'d mut [RackDevice],
    path: &str,
) -> Option<&'d mut RackDevice> {
    let mut parts = path.split('/');
    let first = parts.next()?;
    let mut dev = devices.iter_mut().find(|d| d.key.0 == first)?;
    for p in parts {
        dev = dev
            .inner
            .as_mut()?
            .devices
            .iter_mut()
            .find(|d| d.key.0 == p)?;
    }
    Some(dev)
}

impl RackDoc {
    /// The device at `key`, which may be a path key (`layer/synth`): the first segment is a
    /// top-level device, each next one a device inside the previous container. [`RackDoc::device`]
    /// stays top-level only, so an engine-facing site that forgot paths fails loudly.
    pub fn resolve(&self, key: &DeviceKey) -> Option<&RackDevice> {
        find_relative(&self.devices, &key.0)
    }

    /// Mutable [`RackDoc::resolve`].
    pub fn resolve_mut(&mut self, key: &DeviceKey) -> Option<&mut RackDevice> {
        find_relative_mut(&mut self.devices, &key.0)
    }

    /// The devices of a scope: the top level for `None`, else the inside of the container at
    /// that path key. `None` when there is no such container.
    pub fn scope_devices(&self, container: Option<&DeviceKey>) -> Option<&[RackDevice]> {
        match container {
            None => Some(&self.devices),
            Some(c) => Some(&self.resolve(c)?.inner.as_ref()?.devices),
        }
    }

    /// The cables of a scope (see [`RackDoc::scope_devices`]); local keys inside a container.
    pub fn scope_cables(&self, container: Option<&DeviceKey>) -> Option<&[Cable]> {
        match container {
            None => Some(&self.cables),
            Some(c) => Some(&self.resolve(c)?.inner.as_ref()?.cables),
        }
    }

    /// The inside of the container at path key `key`.
    pub fn inner(&self, key: &DeviceKey) -> Option<&ContainerDoc> {
        self.resolve(key)?.inner.as_deref()
    }

    /// Mutable [`RackDoc::inner`].
    pub fn inner_mut(&mut self, key: &DeviceKey) -> Option<&mut ContainerDoc> {
        self.resolve_mut(key)?.inner.as_deref_mut()
    }

    /// The macro that drives `param` of the device at path key `device`: the container (path
    /// key) and the macro index (0-based), outermost first. `None` when it is free.
    pub fn macro_driver(&self, device: &DeviceKey, param: &str) -> Option<(DeviceKey, u8)> {
        let segments: Vec<&str> = device.0.split('/').collect();
        for depth in 1..segments.len() {
            let container = DeviceKey(segments[..depth].join("/"));
            let relative = segments[depth..].join("/");
            let Some(inner) = self.inner(&container) else {
                continue;
            };
            for (i, m) in inner.macros.iter().enumerate() {
                if m.targets
                    .iter()
                    .any(|t| t.device.0 == relative && t.param == param)
                {
                    return Some((container, i as u8));
                }
            }
        }
        None
    }

    /// Total devices inside containers, at any depth.
    pub fn inner_device_count(&self) -> usize {
        fn count(devices: &[RackDevice]) -> usize {
            devices
                .iter()
                .map(|d| {
                    d.inner
                        .as_ref()
                        .map_or(0, |i| i.devices.len() + count(&i.devices))
                })
                .sum()
        }
        count(&self.devices)
    }

    /// True when any device is a container.
    pub fn has_containers(&self) -> bool {
        self.devices.iter().any(|d| d.inner.is_some())
    }

    /// Validates every container of the project:
    /// limits, keys, allowed types, inner cables, splits, macros, and the expanded graph (no
    /// cycle, no cable twice). Commands plan on a scratch copy and run this on the result; the
    /// loader runs it on the loaded document.
    pub fn check_containers(&self, registry: &Registry) -> Result<(), ModelError> {
        self.check_containers_at(registry).map_err(|v| v.error)
    }

    /// [`RackDoc::check_containers`] with the JSON path of the violation (for the loader).
    pub(crate) fn check_containers_at(&self, registry: &Registry) -> Result<(), Violation> {
        let mut ctx = CheckCtx::default();
        for (i, d) in self.devices.iter().enumerate() {
            let at = |v: Violation| v.under(&[Seg::F("devices"), Seg::I(i)]);
            match (&d.inner, kind_of(registry, &d.type_id)) {
                (Some(inner), Some(DeviceKind::Container)) => {
                    ctx.prefix = d.key.0.clone();
                    inner
                        .check(registry, 1, &mut ctx)
                        .map_err(|v| at(v.under(&[Seg::F("inner")])))?;
                }
                (Some(_), _) => {
                    return Err(at(Violation::new(
                        vec![Seg::F("inner")],
                        ModelError::NotAllowedInContainer(format!(
                            "{} does not have an inside",
                            d.type_id
                        )),
                    )));
                }
                (None, Some(DeviceKind::Container)) => {
                    return Err(at(Violation::new(
                        vec![Seg::F("inner")],
                        ModelError::BadScope("a container has an inside".into()),
                    )));
                }
                (None, _) => {}
            }
            if d.type_id == SPLIT_TYPE {
                return Err(at(Violation::new(
                    vec![Seg::F("type")],
                    ModelError::NotAllowedInContainer(format!(
                        "{SPLIT_TYPE} is internal and cannot be added"
                    )),
                )));
            }
        }
        if ctx.inner_total > MAX_PROJECT_INNER {
            return Err(Violation::new(
                vec![Seg::F("devices")],
                ModelError::ContainerFull(format!(
                    "a project holds at most {MAX_PROJECT_INNER} devices inside containers"
                )),
            ));
        }
        // The flattened graph: cycles and the same cable twice.
        if self.has_containers() {
            for (i, d) in self.devices.iter().enumerate() {
                if d.inner.is_none() {
                    continue;
                }
                if let Some(problem) =
                    rules::expanded_problem(std::slice::from_ref(d), &[], registry)
                {
                    return Err(Violation::new(
                        vec![
                            Seg::F("devices"),
                            Seg::I(i),
                            Seg::F("inner"),
                            Seg::F("cables"),
                        ],
                        ModelError::BadCable(problem),
                    ));
                }
            }
        }
        Ok(())
    }
}

fn kind_of(registry: &Registry, type_id: &str) -> Option<DeviceKind> {
    registry.get(type_id).map(|f| f.descriptor.kind)
}

// ------------------------------------------------------------------------- validation

/// One element of a JSON path.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Seg {
    /// An object field.
    F(&'static str),
    /// An array index.
    I(usize),
}

/// A violation of the container rules, with where it is (relative to the checked object).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Violation {
    pub(crate) path: Vec<Seg>,
    pub(crate) error: ModelError,
}

impl Violation {
    pub(crate) fn new(path: Vec<Seg>, error: ModelError) -> Self {
        Self { path, error }
    }

    /// The same violation seen from an enclosing object.
    pub(crate) fn under(mut self, prefix: &[Seg]) -> Self {
        let mut path = prefix.to_vec();
        path.append(&mut self.path);
        self.path = path;
        self
    }

    /// The message without the error's category prefix.
    pub(crate) fn message(&self) -> String {
        match &self.error {
            ModelError::ContainerTooDeep(m)
            | ModelError::ContainerFull(m)
            | ModelError::NotAllowedInContainer(m)
            | ModelError::BadSplit(m)
            | ModelError::BadMacro(m)
            | ModelError::BadScope(m)
            | ModelError::BadCable(m)
            | ModelError::OutOfRange(m) => m.clone(),
            ModelError::BadKey(k) => format!("invalid or duplicate device key {k:?}"),
            ModelError::UnknownDevice(d) => format!("no device has id {d:?}"),
            ModelError::UnknownKey(k) => format!("no such socket or param {k:?}"),
            other => other.to_string(),
        }
    }
}

/// State threaded through the recursive check.
#[derive(Default)]
pub(crate) struct CheckCtx {
    /// Path key of the container being checked.
    pub(crate) prefix: String,
    /// Devices seen inside containers so far.
    pub(crate) inner_total: usize,
    /// `(absolute device path, param)` of every driven param, with its macro.
    driven: BTreeMap<(String, String), (String, usize)>,
}

fn at_key(path: &str, local: &str) -> String {
    if path.is_empty() {
        local.to_owned()
    } else {
        format!("{path}/{local}")
    }
}

/// The first Notes input of a descriptor.
pub(crate) fn notes_input(desc: &DeviceDescriptor) -> Option<&'static str> {
    desc.sockets
        .iter()
        .find(|s| s.kind == SocketKind::Notes && s.direction == Direction::Input)
        .map(|s| s.key)
}

/// Kind and direction of a socket of the container scope `devices`, local keys. The boundary
/// has Audio outputs `in_l`, `in_r` and Audio inputs `out_l`, `out_r`.
pub(crate) fn scope_socket(
    devices: &[RackDevice],
    registry: &Registry,
    at: &SocketRef,
) -> Result<(SocketKind, Direction), ModelError> {
    if at.device.0 == BOUNDARY_KEY {
        return match at.socket.as_str() {
            "in_l" | "in_r" => Ok((SocketKind::Audio, Direction::Output)),
            "out_l" | "out_r" => Ok((SocketKind::Audio, Direction::Input)),
            other => Err(ModelError::UnknownKey(other.to_owned())),
        };
    }
    let dev = devices
        .iter()
        .find(|d| d.key == at.device)
        .ok_or_else(|| ModelError::UnknownDevice(at.device.0.clone()))?;
    let s = rules::descriptor(registry, &dev.type_id)?
        .socket_by_key(&at.socket)
        .ok_or_else(|| ModelError::UnknownKey(at.socket.clone()))?;
    Ok((s.kind, s.direction))
}

impl ContainerDoc {
    /// Checks this container and everything inside it. `depth` is this container's depth (a
    /// top-level container is 1). Paths in the violation are relative to this object.
    pub(crate) fn check(
        &self,
        registry: &Registry,
        depth: usize,
        ctx: &mut CheckCtx,
    ) -> Result<(), Violation> {
        let v = |path: Vec<Seg>, e: ModelError| Violation::new(path, e);
        if self.devices.len() > MAX_INNER_DEVICES {
            return Err(v(
                vec![Seg::F("devices")],
                ModelError::ContainerFull(format!(
                    "a container holds at most {MAX_INNER_DEVICES} devices, found {}",
                    self.devices.len()
                )),
            ));
        }
        ctx.inner_total += self.devices.len();
        let own_prefix = ctx.prefix.clone();
        for (i, d) in self.devices.iter().enumerate() {
            let dev = |field: &'static str| vec![Seg::F("devices"), Seg::I(i), Seg::F(field)];
            let key = d.key.0.as_str();
            if key == INTERFACE_KEY || key == BOUNDARY_KEY {
                return Err(v(
                    dev("id"),
                    ModelError::BadKey(format!("{key:?} is reserved")),
                ));
            }
            if !rules::is_valid_key(key) {
                return Err(v(
                    dev("id"),
                    ModelError::BadKey(format!("{key:?} must match [a-z][a-z0-9_-]*")),
                ));
            }
            if self.devices[..i].iter().any(|o| o.key == d.key) {
                return Err(v(
                    dev("id"),
                    ModelError::BadKey(format!("duplicate device id {key:?}")),
                ));
            }
            let Some(factory) = registry.get(&d.type_id) else {
                return Err(v(dev("type"), ModelError::UnknownType(d.type_id.clone())));
            };
            if FORBIDDEN_INSIDE.contains(&d.type_id.as_str()) || d.type_id == SPLIT_TYPE {
                return Err(v(
                    dev("type"),
                    ModelError::NotAllowedInContainer(format!(
                        "{} is not allowed inside a container",
                        d.type_id
                    )),
                ));
            }
            if !d.zones.is_empty() && !registry.takes_samples(&d.type_id) {
                return Err(v(
                    dev("zones"),
                    ModelError::BadZones(format!("{} does not take samples", d.type_id)),
                ));
            }
            if d.curves.is_some() && !registry.takes_curves(&d.type_id) {
                return Err(v(
                    dev("curves"),
                    ModelError::BadPatterns(format!("{} does not take curves", d.type_id)),
                ));
            }
            if d.drums.is_some() && !registry.takes_drums(&d.type_id) {
                return Err(v(
                    dev("drums"),
                    ModelError::BadPatterns(format!("{} does not take drum data", d.type_id)),
                ));
            }
            if d.steps.is_some() && registry.takes_drums(&d.type_id) {
                return Err(v(
                    dev("steps"),
                    ModelError::BadPatterns(format!("{} takes drum data, not steps", d.type_id)),
                ));
            }
            if d.steps.is_some() && !registry.takes_player_data(&d.type_id) {
                return Err(v(
                    dev("steps"),
                    ModelError::BadPatterns(format!("{} does not take pattern data", d.type_id)),
                ));
            }
            match (&d.inner, factory.descriptor.kind == DeviceKind::Container) {
                (Some(inner), true) => {
                    if depth + 1 > MAX_CONTAINER_DEPTH {
                        return Err(v(
                            vec![Seg::F("devices"), Seg::I(i)],
                            ModelError::ContainerTooDeep(format!(
                                "containers nest at most {MAX_CONTAINER_DEPTH} levels deep"
                            )),
                        ));
                    }
                    ctx.prefix = at_key(&own_prefix, key);
                    let r = inner.check(registry, depth + 1, ctx);
                    ctx.prefix = own_prefix.clone();
                    r.map_err(|e| e.under(&[Seg::F("devices"), Seg::I(i), Seg::F("inner")]))?;
                }
                (Some(_), false) => {
                    return Err(v(
                        dev("inner"),
                        ModelError::NotAllowedInContainer(format!(
                            "{} does not have an inside",
                            d.type_id
                        )),
                    ));
                }
                (None, true) => {
                    return Err(v(
                        dev("inner"),
                        ModelError::BadScope("a container has an inside".into()),
                    ));
                }
                (None, false) => {}
            }
        }
        self.check_cables(registry)?;
        self.check_splits(registry)?;
        self.check_macros(registry, &own_prefix, ctx)?;
        Ok(())
    }

    fn check_cables(&self, registry: &Registry) -> Result<(), Violation> {
        for (i, c) in self.cables.iter().enumerate() {
            let at = |end: &'static str| vec![Seg::F("cables"), Seg::I(i), Seg::F(end)];
            let end = |s: &SocketRef, field: &'static str| {
                scope_socket(&self.devices, registry, s).map_err(|e| {
                    let e = match e {
                        ModelError::UnknownKey(k) => {
                            ModelError::BadCable(format!("{:?} has no socket {k:?}", s.device.0))
                        }
                        other => other,
                    };
                    Violation::new(at(field), e)
                })
            };
            let from = end(&c.from, "from")?;
            let to = end(&c.to, "to")?;
            rules::check_endpoints(from, to, c, &self.cables[..i])
                .map_err(|e| Violation::new(vec![Seg::F("cables"), Seg::I(i)], e))
                .map_err(|mut v| {
                    // Name the offending end when the rule is about one.
                    if let ModelError::BadCable(m) = &v.error {
                        if m.contains("is an input") {
                            v.path.push(Seg::F("from"));
                        } else if m.contains("is an output") || m.contains("already connected") {
                            v.path.push(Seg::F("to"));
                        }
                    }
                    v
                })?;
        }
        Ok(())
    }

    fn check_splits(&self, registry: &Registry) -> Result<(), Violation> {
        let bad = |path: Vec<Seg>, m: String| Violation::new(path, ModelError::BadSplit(m));
        if self.splits.len() > MAX_SPLITS {
            return Err(bad(
                vec![Seg::F("splits")],
                format!(
                    "a container has at most {MAX_SPLITS} splits, found {}",
                    self.splits.len()
                ),
            ));
        }
        for (i, s) in self.splits.iter().enumerate() {
            let field = |f: &'static str| vec![Seg::F("splits"), Seg::I(i), Seg::F(f)];
            let Some(target) = self.devices.iter().find(|d| d.key == s.target) else {
                return Err(bad(
                    field("target"),
                    format!("no device has id {:?}", s.target.0),
                ));
            };
            let desc = rules::descriptor(registry, &target.type_id)
                .map_err(|e| Violation::new(field("target"), e))?;
            let Some(notes) = notes_input(desc) else {
                return Err(bad(
                    field("target"),
                    format!("{:?} has no Notes input", s.target.0),
                ));
            };
            if self.splits[..i].iter().any(|o| o.target == s.target) {
                return Err(bad(
                    field("target"),
                    format!("{:?} already has a split", s.target.0),
                ));
            }
            if self
                .cables
                .iter()
                .any(|c| c.to.device == s.target && c.to.socket == notes)
            {
                return Err(bad(
                    field("target"),
                    format!(
                        "{:?} is already fed by a cable on its Notes input; remove the cable first",
                        s.target.0
                    ),
                ));
            }
            for (name, value) in [("key_lo", s.key_lo), ("key_hi", s.key_hi)] {
                if value > 127 {
                    return Err(bad(
                        field(if name == "key_lo" { "key_lo" } else { "key_hi" }),
                        format!("{value} is outside 0..=127"),
                    ));
                }
            }
            for (name, value) in [("vel_lo", s.vel_lo), ("vel_hi", s.vel_hi)] {
                if !(1..=127).contains(&value) {
                    return Err(bad(
                        field(if name == "vel_lo" { "vel_lo" } else { "vel_hi" }),
                        format!("{value} is outside 1..=127"),
                    ));
                }
            }
            if s.key_lo > s.key_hi {
                return Err(bad(
                    vec![Seg::F("splits"), Seg::I(i)],
                    format!("key_lo {} is above key_hi {}", s.key_lo, s.key_hi),
                ));
            }
            if s.vel_lo > s.vel_hi {
                return Err(bad(
                    vec![Seg::F("splits"), Seg::I(i)],
                    format!("vel_lo {} is above vel_hi {}", s.vel_lo, s.vel_hi),
                ));
            }
            if !(-48..=48).contains(&s.transpose) {
                return Err(bad(
                    field("transpose"),
                    format!("{} is outside -48..=48", s.transpose),
                ));
            }
        }
        Ok(())
    }

    fn check_macros(
        &self,
        registry: &Registry,
        prefix: &str,
        ctx: &mut CheckCtx,
    ) -> Result<(), Violation> {
        let bad = |path: Vec<Seg>, m: String| Violation::new(path, ModelError::BadMacro(m));
        let mut total = 0;
        for (i, m) in self.macros.iter().enumerate() {
            let at = |extra: Vec<Seg>| {
                let mut p = vec![Seg::F("macros"), Seg::I(i)];
                p.extend(extra);
                p
            };
            if m.name.chars().count() > MAX_MACRO_NAME_CHARS {
                return Err(bad(
                    at(vec![Seg::F("name")]),
                    format!("a macro name is at most {MAX_MACRO_NAME_CHARS} characters"),
                ));
            }
            if m.targets.len() > MAX_MACRO_TARGETS {
                return Err(bad(
                    at(vec![Seg::F("targets")]),
                    format!(
                        "a macro has at most {MAX_MACRO_TARGETS} targets, found {}",
                        m.targets.len()
                    ),
                ));
            }
            total += m.targets.len();
            if total > MAX_CONTAINER_TARGETS {
                return Err(bad(
                    at(vec![Seg::F("targets")]),
                    format!("a container has at most {MAX_CONTAINER_TARGETS} macro targets"),
                ));
            }
            for (j, t) in m.targets.iter().enumerate() {
                let at_t = |f: &'static str| at(vec![Seg::F("targets"), Seg::I(j), Seg::F(f)]);
                let Some(dev) = find_relative(&self.devices, &t.device.0) else {
                    return Err(bad(
                        at_t("device"),
                        format!("no device has id {:?}", t.device.0),
                    ));
                };
                let desc = rules::descriptor(registry, &dev.type_id)
                    .map_err(|e| Violation::new(at_t("device"), e))?;
                // A dead plugin param stays mapped; new mappings are refused by the
                // command (`SetMacro`).
                let dead = crate::plugin::is_dead_plugin_param(desc, &t.param);
                if (desc.param_by_key(&t.param).is_none() && !dead)
                    || console::is_internal_param(&t.param)
                {
                    return Err(bad(
                        at_t("param"),
                        format!("{} has no param {:?}", desc.type_id, t.param),
                    ));
                }
                if !t.min.is_finite() {
                    return Err(bad(at_t("min"), format!("{} is not finite", t.min)));
                }
                if !t.max.is_finite() {
                    return Err(bad(at_t("max"), format!("{} is not finite", t.max)));
                }
                let abs = (at_key(prefix, &t.device.0), t.param.clone());
                if let Some((other, k)) = ctx.driven.get(&abs) {
                    return Err(bad(
                        at(vec![Seg::F("targets"), Seg::I(j)]),
                        format!(
                            "{}.{} is already driven by macro {} of {other:?}",
                            t.device.0,
                            t.param,
                            k + 1
                        ),
                    ));
                }
                ctx.driven.insert(abs, (prefix.to_owned(), i));
            }
        }
        Ok(())
    }
}

/// A new device of `type_id` as `AddDevice` and `AddInner` create it: default params, a
/// sequencer's default patterns, an empty inside for a container.
pub(crate) fn new_device(
    registry: &Registry,
    desc: &DeviceDescriptor,
    key: DeviceKey,
    name: String,
) -> RackDevice {
    RackDevice {
        key,
        type_id: desc.type_id.to_owned(),
        name,
        params: BTreeMap::new(),
        folded: false,
        color: None,
        zones: Vec::new(),
        steps: (registry.takes_player_data(desc.type_id) && !registry.takes_drums(desc.type_id))
            .then(crate::StepSeqDoc::default),
        curves: None,
        drums: registry
            .takes_drums(desc.type_id)
            .then(crate::DrumDoc::default),
        inner: (desc.kind == DeviceKind::Container).then(Box::default),
        plugin: None,
        column: 0,
        patch_name: String::new(),
        midi_out: None,
    }
}

/// Every pool asset the zones of `device` and of everything inside it reference.
pub(crate) fn collect_device_assets(
    device: &RackDevice,
    out: &mut std::collections::BTreeSet<String>,
) {
    out.extend(device.zones.iter().map(|z| z.asset.clone()));
    if let Some(inner) = &device.inner {
        for d in &inner.devices {
            collect_device_assets(d, out);
        }
    }
}

/// The sockets of the boundary pseudo-device as an inner cable sees them: the Audio outputs
/// `in_l`, `in_r` (what arrives at the container) and the Audio inputs `out_l`, `out_r` (what
/// leaves through it), as `(key, kind, direction)`. The boundary has no notes socket.
pub fn boundary_sockets() -> [(&'static str, SocketKind, Direction); 4] {
    [
        ("in_l", SocketKind::Audio, Direction::Output),
        ("in_r", SocketKind::Audio, Direction::Output),
        ("out_l", SocketKind::Audio, Direction::Input),
        ("out_r", SocketKind::Audio, Direction::Input),
    ]
}

impl MacroDoc {
    /// The name to show for macro `index` (0-based): the user's name, or `Macro N`.
    pub fn display_name(&self, index: usize) -> String {
        if self.name.is_empty() {
            format!("Macro {}", index + 1)
        } else {
            self.name.clone()
        }
    }
}
