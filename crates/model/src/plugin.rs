//! Third-party plugin devices in the document.
//!
//! A plugin device is an ordinary [`crate::RackDevice`] whose type id is `clap.<id>` or
//! `vst3.<hex>` and which carries a [`PluginDoc`]: which plugin it is, a compact snapshot of
//! its layout (so a device can be built, cabled and saved when the plugin is missing),
//! the plugin's opaque state blob, the panel pins and the per-instance switches. Its params
//! live in [`crate::RackDevice::params`] under the keys `p<plugin param id>` (plus the
//! host-owned `bypass`) and are stored **sparsely**: only keys set by `SetParam`.
//!
//! The model never loads a plugin and never depends on a plugin crate: the descriptor of an
//! installed plugin is registered by the app from the scan cache
//! ([`atomik_device_sdk::Registry::register_plugin`]); that of a missing plugin comes from the
//! saved layout ([`register_stand_ins`]).
//!
//! # What the format saves of the layout
//!
//! Saving writes only the layout rows the project **uses** (a stored value, an automation
//! lane, a macro target or a pin), sorted by id, so a plugin with 2 000 params does not write
//! 2 000 rows per instance. In memory the layout may hold the full table (the app has it from
//! the scan, [`crate::PluginCommand::SyncLayout`]); [`crate::RackDoc::compacted`] is the
//! document as a save and reload gives it back.
//!
//! # Dead keys
//!
//! A `p<id>` key that the descriptor does not have (a newer plugin version dropped the param)
//! is kept in `params`, in automation lanes and in macro targets, and saved back unchanged;
//! nothing treats it as an error at load. New edits cannot name a dead key.

use crate::{DeviceKey, ModelError, RackDevice, RackDoc, RackSlot};
use atomik_device_sdk::{
    Channels, PluginFormat, PluginKindHint, PluginLayout, PluginRef, PluginStateBlob,
    descriptor_for, is_plugin_type_id,
};
use atomik_device_sdk::{DeviceDescriptor, Registry};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// Largest plugin state blob, in bytes.
pub const MAX_PLUGIN_STATE_BYTES: u64 = 16 * 1024 * 1024;
/// Largest total of all plugin state blobs of a project, in bytes.
pub const MAX_PROJECT_PLUGIN_STATE_BYTES: u64 = 128 * 1024 * 1024;
/// Most params one plugin device pins as panel knobs.
pub const MAX_PINNED_PARAMS: usize = 16;
/// Most param rows a layout holds (the SDK's `MAX_PLUGIN_PARAMS`).
pub const MAX_LAYOUT_PARAMS: usize = atomik_device_sdk::MAX_PLUGIN_PARAMS;
/// First merge id of a plugin editor gesture: `SetParam`s with the same merge
/// id on the same param are one undo step. Clear of the macro drags (7 x 2^40).
pub const PLUGIN_GESTURE_MERGE_BASE: u64 = 8 << 40;

/// What the document keeps about one plugin device.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginDoc {
    /// The plugin's format.
    pub format: PluginFormat,
    /// The plugin's own id (the CLAP id string, or the 32 lowercase hex digits of the VST3 class
    /// id). `format` and `id` give the device's type id.
    pub id: String,
    /// Where the plugin was when the project was saved. Shown, never used to load anything.
    pub path_hint: Option<String>,
    /// The layout snapshot: name, vendor, version, ports and params. Saved compactly (see the
    /// module docs of `plugin.rs`).
    pub layout: PluginLayout,
    /// The plugin's opaque state, or `None` while it has none. Shared, so cloning a document
    /// (every command plans on a copy) does not copy megabytes.
    pub state: Option<Arc<PluginStateBlob>>,
    /// Up to [`MAX_PINNED_PARAMS`] param ids shown as knobs on the device panel, in order.
    pub pinned: Vec<u32>,
    /// The device runs on the audio callback thread.
    pub run_on_audio_thread: bool,
    /// The panel's power switch: off behaves like bypass with processing stopped.
    pub powered: bool,
}

impl PluginDoc {
    /// A new plugin doc: no state, no pins, powered, not pinned to the audio thread.
    pub fn new(plugin: &PluginRef, layout: PluginLayout, path_hint: Option<String>) -> Self {
        Self {
            format: plugin.format,
            id: plugin.id.clone(),
            path_hint,
            layout,
            state: None,
            pinned: Vec::new(),
            run_on_audio_thread: false,
            powered: true,
        }
    }

    /// The (format, id) pair.
    pub fn plugin_ref(&self) -> PluginRef {
        PluginRef {
            format: self.format,
            id: self.id.clone(),
        }
    }

    /// The registry type id, `clap.<id>` or `vst3.<id>`.
    pub fn type_id(&self) -> String {
        self.plugin_ref().type_id()
    }

    /// The plugin's name (from the layout snapshot).
    pub fn name(&self) -> &str {
        &self.layout.name
    }

    /// The plugin's vendor.
    pub fn vendor(&self) -> &str {
        &self.layout.vendor
    }

    /// The plugin's version string.
    pub fn version(&self) -> &str {
        &self.layout.version
    }
}

/// A plugin edit (wrapped in `Command::Plugin`). Every command is one undo step except
/// [`PluginCommand::SyncState`] and [`PluginCommand::SyncLayout`], which are not undo steps (like
/// `RegisterAsset`): they report data the plugin owns, so undoing other edits never rolls a
/// patch back. Neither returns a sync op, leaves the redo history alone and ends no drag-merge
/// group. A patch or preset change inside the plugin's own window is not undoable.
#[derive(Debug, Clone, PartialEq)]
pub enum PluginCommand {
    /// Add a plugin device at rack `index` (clamped), at the top level or inside `container` (a
    /// path key). The plugin's type must be registered ([`atomik_device_sdk::Registry::register_plugin`]
    /// from the scan cache) and `layout` must be its layout. The device is named after the
    /// plugin and keyed after its name; an instrument also gets its console channel, in the same
    /// undo step; inside a container `auto_split` adds a full-range note split.
    Add {
        /// Rack position (clamped).
        index: usize,
        /// The container (path key) to add into, or `None` for the top level.
        container: Option<DeviceKey>,
        /// The plugin.
        plugin: PluginRef,
        /// Its layout, as the scan reports it.
        layout: PluginLayout,
        /// Where the plugin was found (kept in the project as a hint).
        path_hint: Option<String>,
        /// Explicit (local) key, or `None` to generate one.
        key: Option<DeviceKey>,
        /// Also add a full-range split to an instrument inside a container.
        auto_split: bool,
    },
    /// [`PluginCommand::Add`] at a rack column slot at the top level (see
    /// [`crate::Command::AddDeviceAt`] for how the slot resolves): the plugin lands at
    /// `slot.index` of `slot.column`, and `slot.column == columns` appends a new column in the
    /// same undo step. For a plugin that is neither an instrument nor an effect (those go
    /// through the creation commands); an instrument still gets its console channel.
    AddAt {
        /// Where the plugin goes.
        slot: RackSlot,
        /// The plugin.
        plugin: PluginRef,
        /// Its layout, as the scan reports it.
        layout: PluginLayout,
        /// Where the plugin was found (kept in the project as a hint).
        path_hint: Option<String>,
        /// Explicit (local) key, or `None` to generate one.
        key: Option<DeviceKey>,
    },
    /// Store the plugin's state blob (not an undo step; refused above 16 MiB per blob or 128 MiB
    /// per project). The app sends it after every capture.
    SyncState {
        /// The plugin device (path key allowed).
        device: DeviceKey,
        /// The state.
        state: PluginStateBlob,
    },
    /// Replace the device's layout snapshot with the real plugin's (not an undo step): the app
    /// sends it when the instance is created, so the document holds the full param table and a
    /// newer version's vendor and version strings. Pins and params are left as they are.
    SyncLayout {
        /// The plugin device (path key allowed).
        device: DeviceKey,
        /// The layout.
        layout: PluginLayout,
    },
    /// Set the panel pins (at most 16 distinct ids of the layout, in order).
    SetPinned {
        /// The plugin device.
        device: DeviceKey,
        /// The pinned param ids.
        pinned: Vec<u32>,
    },
    /// Switch RUN ON AUDIO THREAD.
    SetRunMode {
        /// The plugin device.
        device: DeviceKey,
        /// The new setting.
        run_on_audio_thread: bool,
    },
    /// Switch the panel's power.
    SetPower {
        /// The plugin device.
        device: DeviceKey,
        /// The new setting.
        powered: bool,
    },
    /// Re-point a (stand-in) plugin device at another registered plugin, keeping its state,
    /// cables, params, automation, macros and console channel. Refused when the kinds differ,
    /// when a cabled socket is missing in the new plugin, or when an automated or macro-mapped
    /// param is (the error names it). Pins the new layout lacks are dropped (undo brings them
    /// back); stored values of missing params stay as dead keys.
    Relink {
        /// The plugin device.
        device: DeviceKey,
        /// The new plugin.
        plugin: PluginRef,
        /// Its layout.
        layout: PluginLayout,
        /// The new path hint.
        path_hint: Option<String>,
    },
}

// ------------------------------------------------------------------------------ param keys

/// The plugin param id of a key `p<decimal>` in its canonical form (no leading zeros, fits 32
/// bits), or `None` for `bypass` and anything else.
pub(crate) fn param_id_of_key(key: &str) -> Option<u32> {
    let digits = key.strip_prefix('p')?;
    let id: u32 = digits.parse().ok()?;
    (digits == id.to_string()).then_some(id)
}

/// True for a `p<id>` key of a plugin device that the descriptor does not have: kept, never an
/// error at load.
pub(crate) fn is_dead_plugin_param(desc: &DeviceDescriptor, param: &str) -> bool {
    is_plugin_type_id(desc.type_id)
        && param_id_of_key(param).is_some()
        && desc.param_by_key(param).is_none()
}

// ------------------------------------------------------------------------------ layouts

/// What is wrong with a layout: the row (index into `params`) when it is about one.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LayoutFault {
    pub(crate) row: Option<usize>,
    pub(crate) message: String,
}

fn fault(message: impl Into<String>) -> LayoutFault {
    LayoutFault {
        row: None,
        message: message.into(),
    }
}

/// Checks that a descriptor can be built from `layout` for `plugin` (the SDK's rules, without
/// building or interning one): a valid plugin id, at most 2 048 params with distinct ids and
/// valid ranges, at least one socket.
pub(crate) fn validate_layout(
    plugin: &PluginRef,
    layout: &PluginLayout,
) -> Result<(), LayoutFault> {
    let id_ok = match plugin.format {
        PluginFormat::Clap => {
            !plugin.id.is_empty()
                && plugin.id.len() <= 256
                && plugin
                    .id
                    .chars()
                    .all(|c| !c.is_whitespace() && !c.is_control())
        }
        PluginFormat::Vst3 => {
            plugin.id.len() == 32
                && plugin
                    .id
                    .bytes()
                    .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        }
    };
    if !id_ok {
        return Err(fault(format!("{:?} is not a valid plugin id", plugin.id)));
    }
    if layout.params.len() > MAX_LAYOUT_PARAMS {
        return Err(fault(format!(
            "a layout holds at most {MAX_LAYOUT_PARAMS} params, found {}",
            layout.params.len()
        )));
    }
    let mut seen = BTreeSet::new();
    for (i, p) in layout.params.iter().enumerate() {
        let row = |message: String| LayoutFault {
            row: Some(i),
            message,
        };
        if !seen.insert(p.id) {
            return Err(row(format!("duplicate param id {}", p.id)));
        }
        if !(p.min.is_finite() && p.max.is_finite() && p.default.is_finite()) {
            return Err(row(format!(
                "param {} has a value that is not finite",
                p.id
            )));
        }
        match plugin.format {
            PluginFormat::Clap if p.min > p.max => {
                return Err(row(format!(
                    "param {} has an empty range: min {} is above max {}",
                    p.id, p.min, p.max
                )));
            }
            PluginFormat::Vst3 if p.stepped && p.steps == 0 => {
                return Err(row(format!("stepped param {} needs steps above 0", p.id)));
            }
            _ => {}
        }
    }
    if socket_keys(layout).is_empty() {
        return Err(fault(
            "a plugin needs at least one audio or notes port (socket)",
        ));
    }
    Ok(())
}

/// The socket keys a layout gives its device, in the SDK's order.
pub(crate) fn socket_keys(layout: &PluginLayout) -> Vec<&'static str> {
    let mut keys = Vec::new();
    if layout.main_in.is_some() {
        keys.extend(["in_l", "in_r"]);
    }
    if layout.sidechain {
        keys.extend(["sc_in_l", "sc_in_r"]);
    }
    if layout.notes_in {
        keys.push("notes_in");
    }
    if layout.main_out.is_some() {
        keys.extend(["out_l", "out_r"]);
    }
    if layout.notes_out {
        keys.push("notes_out");
    }
    keys
}

/// Checks that the registered descriptor of the plugin has exactly the sockets `layout` implies.
pub(crate) fn check_matches_registered(
    desc: &DeviceDescriptor,
    layout: &PluginLayout,
) -> Result<(), ModelError> {
    let have: Vec<&str> = desc.sockets.iter().map(|s| s.key).collect();
    if have != socket_keys(layout) {
        return Err(ModelError::BadPlugin(format!(
            "the layout of {} does not match the registered plugin (sockets {:?} against {:?})",
            desc.type_id,
            socket_keys(layout),
            have
        )));
    }
    Ok(())
}

/// The key base for a device made from a plugin named `name`: lowercase letters and digits,
/// other runs become one `-`.
pub(crate) fn key_base_of_name(name: &str) -> String {
    let mut base = String::new();
    let mut dash = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            if dash && !base.is_empty() {
                base.push('-');
            }
            dash = false;
            base.push(c.to_ascii_lowercase());
        } else {
            dash = true;
        }
    }
    if !base.starts_with(|c: char| c.is_ascii_lowercase()) {
        base.insert_str(0, if base.is_empty() { "plugin" } else { "p-" });
    }
    if base == crate::INTERFACE_KEY || base == crate::BOUNDARY_KEY {
        base.push_str("-device");
    }
    base
}

// ------------------------------------------------------------------------------ document

/// Every plugin device of `devices`, depth first in document order, with its path key.
pub(crate) fn walk_plugins<'d>(
    devices: &'d [RackDevice],
    prefix: &str,
    out: &mut Vec<(String, &'d RackDevice)>,
) {
    for d in devices {
        let path = format!("{prefix}{}", d.key.0);
        if d.plugin.is_some() {
            out.push((path.clone(), d));
        }
        if let Some(inner) = &d.inner {
            walk_plugins(&inner.devices, &format!("{path}/"), out);
        }
    }
}

impl RackDoc {
    /// Every plugin device (at any depth) as `(path key, device)`, in document order.
    pub fn plugin_devices(&self) -> Vec<(DeviceKey, &RackDevice)> {
        let mut out = Vec::new();
        walk_plugins(&self.devices, "", &mut out);
        out.into_iter().map(|(p, d)| (DeviceKey(p), d)).collect()
    }

    /// The param ids each plugin device (path key) uses: a stored value, an automation lane, a
    /// macro target or a pin. Saving writes exactly these layout rows (if the layout has them).
    pub(crate) fn plugin_param_usage(&self) -> BTreeMap<String, BTreeSet<u32>> {
        let mut used: BTreeMap<String, BTreeSet<u32>> = BTreeMap::new();
        for (path, d) in {
            let mut v = Vec::new();
            walk_plugins(&self.devices, "", &mut v);
            v
        } {
            let ids = used.entry(path).or_default();
            ids.extend(d.params.keys().filter_map(|k| param_id_of_key(k)));
            if let Some(p) = &d.plugin {
                ids.extend(p.pinned.iter().copied());
            }
        }
        for t in &self.song.tracks {
            for lane in &t.lanes {
                if let crate::LaneKind::Automation(param) = &lane.kind
                    && let Some(id) = param_id_of_key(param)
                    && let Some(ids) = used.get_mut(&t.device.0)
                {
                    ids.insert(id);
                }
            }
        }
        fn macros(
            devices: &[RackDevice],
            prefix: &str,
            used: &mut BTreeMap<String, BTreeSet<u32>>,
        ) {
            for d in devices {
                let Some(inner) = &d.inner else { continue };
                let path = format!("{prefix}{}", d.key.0);
                for m in &inner.macros {
                    for t in &m.targets {
                        if let Some(id) = param_id_of_key(&t.param)
                            && let Some(ids) = used.get_mut(&format!("{path}/{}", t.device.0))
                        {
                            ids.insert(id);
                        }
                    }
                }
                macros(&inner.devices, &format!("{path}/"), used);
            }
        }
        macros(&self.devices, "", &mut used);
        used
    }

    /// The document as a save and reload gives it back: every plugin layout keeps only the rows
    /// the project uses (see the module docs of `plugin.rs`). Idempotent.
    pub fn compacted(&self) -> RackDoc {
        let used = self.plugin_param_usage();
        let mut doc = self.clone();
        for (path, ids) in used {
            if let Some(p) = doc
                .resolve_mut(&DeviceKey(path))
                .and_then(|d| d.plugin.as_mut())
            {
                p.layout.params.retain(|row| ids.contains(&row.id));
                p.layout.params.sort_by_key(|row| row.id);
            }
        }
        doc
    }

    /// Total state bytes of all plugin devices, skipping the one at `except` (path key).
    pub(crate) fn plugin_state_bytes(&self, except: Option<&DeviceKey>) -> u64 {
        let mut all = Vec::new();
        walk_plugins(&self.devices, "", &mut all);
        all.iter()
            .filter(|(p, _)| except.is_none_or(|k| k.0 != *p))
            .filter_map(|(_, d)| d.plugin.as_ref()?.state.as_ref())
            .map(|s| s.0.len() as u64)
            .sum()
    }
}

/// Adds the param rows of `from` that `into` lacks, keeping the rows sorted by id (a stand-in
/// descriptor is the union of the rows the instances of one plugin saved).
pub(crate) fn merge_rows(into: &mut PluginLayout, from: &PluginLayout) {
    for row in &from.params {
        if !into.params.iter().any(|r| r.id == row.id) {
            into.params.push(row.clone());
        }
    }
    into.params.sort_by_key(|r| r.id);
}

/// The type ids of the plugin devices of `doc` that `registry` does not know, from the saved
/// layouts, as `(type id, plugin, layout)`, sorted by type id.
fn missing_types(doc: &RackDoc, registry: &Registry) -> Vec<(String, PluginRef, PluginLayout)> {
    let mut found: BTreeMap<String, (PluginRef, PluginLayout)> = BTreeMap::new();
    for (_, d) in doc.plugin_devices() {
        let Some(p) = &d.plugin else { continue };
        if registry.get(&d.type_id).is_none() {
            // Every instance saved only the rows it uses, so the type's stand-in descriptor is
            // the union of the rows of all its instances (the first instance's other fields win).
            found
                .entry(d.type_id.clone())
                .and_modify(|(_, layout)| merge_rows(layout, &p.layout))
                .or_insert_with(|| (p.plugin_ref(), p.layout.clone()));
        }
    }
    found.into_iter().map(|(t, (p, l))| (t, p, l)).collect()
}

/// Registers, for every plugin device of `doc` whose type `registry` does not know (not
/// installed, not scanned, disabled), a descriptor built from the saved layout, so the
/// document works with commands, expansion and the console while the plugin is missing. Returns
/// the registered type ids, sorted. Types the registry knows are left alone. A layout that cannot
/// be built is an error.
///
/// Call it after [`crate::load_project`] (and after a rescan changes the registry): loading
/// itself never changes the registry it is given.
pub fn register_stand_ins(
    doc: &RackDoc,
    registry: &mut Registry,
) -> Result<Vec<String>, ModelError> {
    let mut added = Vec::new();
    for (type_id, plugin, layout) in missing_types(doc, registry) {
        validate_layout(&plugin, &layout)
            .map_err(|f| ModelError::BadPlugin(format!("{type_id}: {}", f.message)))?;
        let desc = descriptor_for(&plugin, &layout)
            .map_err(|e| ModelError::BadPlugin(format!("{type_id}: {e}")))?;
        registry
            .register_plugin(type_id.clone(), desc)
            .map_err(|e| ModelError::BadPlugin(format!("{type_id}: {e}")))?;
        added.push(type_id);
    }
    Ok(added)
}

/// The channel layout name used in the project text.
pub(crate) fn channels_name(c: Option<Channels>) -> Option<&'static str> {
    match c {
        None => None,
        Some(Channels::Mono) => Some("mono"),
        Some(Channels::Stereo) => Some("stereo"),
    }
}

/// The kind name used in the project text.
pub(crate) fn kind_name(k: PluginKindHint) -> &'static str {
    match k {
        PluginKindHint::Instrument => "instrument",
        PluginKindHint::Effect => "effect",
        PluginKindHint::NoteEffect => "note_effect",
    }
}

/// The device at path key `key` with its plugin doc, or the error a command reports.
pub(crate) fn plugin_device<'d>(
    doc: &'d RackDoc,
    key: &DeviceKey,
) -> Result<(&'d RackDevice, &'d PluginDoc), ModelError> {
    let d = crate::rules::resolve(doc, key)?;
    let p = d.plugin.as_deref().ok_or_else(|| {
        ModelError::BadPlugin(format!("{key} ({}) is not a plugin device", d.type_id))
    })?;
    Ok((d, p))
}
