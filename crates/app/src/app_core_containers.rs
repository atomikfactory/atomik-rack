//! Containers in the app core.
//!
//! # The engine never sees a container
//!
//! [`RackDoc::expand`] compiles the tree into a flat device list and a flat cable list
//! ([`atomik_model::ExpandedRack`]): a container's slot holds its `<container>/~split` device
//! ([`atomik_model::SPLIT_TYPE`]) and then its inner devices (depth first), keyed by their **path
//! key** (`layer/synth`). Everything that builds or feeds the engine reads the expansion:
//!
//! * [`AppCore::rebuild_engine`] inserts the expanded devices under ids 1, 2, 3... in that order
//!   (a bounce does the same with [`atomik_render::project::expanded_ids`], so a fresh load and a
//!   bounce agree), each with its stored params, **macro values applied**, and the split
//!   device's params from the splits; then data, then one routing built from the expanded cables
//!   (insert, data, routing).
//! * A `SyncOp::DeviceAdded(container)` inserts the container's whole inside, a
//!   `DeviceRemoved(container)` removes every id whose key starts with `container/`
//!   (`DeviceAdded(path)` of an inner device inserts that one device). Ids of later edits are
//!   incremental, as for the top level.
//! * A `ParamChanged` of a container's `macroN`, and `MacrosChanged` (a mapping edit, its undo and
//!   redo), send one `set_param` per target with the linear mapping of
//!   [`atomik_model::macro_value`]; a target a removed mapping frees gets its stored value back.
//!   `SplitsChanged` sends the changed params of the `~split` device (the others keep what the
//!   engine has).
//! * Notes for a **container** (the keyboard, an audition, a song track) go to its `~split`
//!   device ([`AppCore::flat_id`]); the song builder does the same for tracks
//!   ([`atomik_render::project::song_input_with_assets`]) and expands macro automation into
//!   one lane per target.
//! * Meters, sample maps and pattern data of inner devices are looked up by path key (the
//!   expansion's `zones` and the document's [`RackDoc::resolve`]); inner sample and pattern
//!   edits open their editors through path keys.
//!
//! # Scope and the container editor
//!
//! The rack view's scope (which container is shown) is view state only: it is reset
//! to the top level when a project is loaded. [`Pane::ContainerEditor`](crate::Pane) shows the
//! splits and macros editor in place of the arrangement ([`AppCore::open_container_editor`]); it
//! closes on `CloseContainerEditor`, when its container vanishes and on a new document.
//!
//! # What an inner device cannot do
//!
//! Song tracks cannot bind inner devices, so slices to a track, a step clip and the step
//! editor for an inner drum machine are refused with a message ([`AppCore::refuse_inner`]).

use super::AppCore;
use crate::sink::EngineSink;
use crate::target::Target;
use atomik_model::{
    DeviceKey, ExpandedDevice, ExpandedRack, RackDevice, RackDoc, SPLIT_TYPE, SocketRef,
};
use atomik_ui::{ContainerEditorView, ContainerTab};
use std::collections::BTreeMap;

/// The suffix of a container's split device key (`layer/~split`).
pub(super) const SPLIT_SUFFIX: &str = "/~split";

/// The key of a container's split device.
pub(super) fn split_key(container: &DeviceKey) -> DeviceKey {
    DeviceKey(format!("{}{SPLIT_SUFFIX}", container.0))
}

/// The container a split key stands for; any other key as it is.
pub(super) fn container_of_split(key: &DeviceKey) -> DeviceKey {
    key.0
        .strip_suffix(SPLIT_SUFFIX)
        .map_or_else(|| key.clone(), |container| DeviceKey(container.to_string()))
}

/// True when `key` names a container of `doc` (top level or inner).
pub(super) fn is_container(doc: &RackDoc, key: &DeviceKey) -> bool {
    doc.resolve(key).is_some_and(|d| d.inner.is_some())
}

/// The flat device that stands for `key`: a container's split device, else `key`.
pub(super) fn flat_key(doc: &RackDoc, key: &DeviceKey) -> DeviceKey {
    if is_container(doc, key) {
        split_key(key)
    } else {
        key.clone()
    }
}

/// The device type of a flat key: a document device (top level or inner) or a split device.
pub(super) fn type_of(doc: &RackDoc, key: &DeviceKey) -> Option<String> {
    match doc.resolve(key) {
        Some(d) => Some(d.type_id.clone()),
        None => key
            .0
            .ends_with(SPLIT_SUFFIX)
            .then(|| SPLIT_TYPE.to_string()),
    }
}

/// The flat device that takes the notes sent to `key`, and its type: a container's split device,
/// else the device itself.
pub(super) fn notes_device(doc: &RackDoc, key: &DeviceKey) -> Option<(DeviceKey, String)> {
    let device = doc.resolve(key)?;
    if device.inner.is_some() {
        Some((split_key(key), SPLIT_TYPE.to_string()))
    } else {
        Some((key.clone(), device.type_id.clone()))
    }
}

/// The flat devices a `DeviceAdded(key)` inserts, in the expansion's order: a container's inside
/// (its split and every inner device), else the device itself.
pub(super) fn subtree<'a>(
    doc: &RackDoc,
    expanded: &'a ExpandedRack,
    key: &DeviceKey,
) -> Vec<&'a ExpandedDevice> {
    if is_container(doc, key) {
        let prefix = format!("{}/", key.0);
        expanded
            .devices
            .iter()
            .filter(|d| d.key.0.starts_with(&prefix))
            .collect()
    } else {
        expanded.devices.iter().filter(|d| &d.key == key).collect()
    }
}

/// The engine devices a `DeviceRemoved(key)` removes: `key` itself and, for a container, every
/// id whose key starts with `key/`.
pub(super) fn flat_keys_of<V>(ids: &BTreeMap<DeviceKey, V>, key: &DeviceKey) -> Vec<DeviceKey> {
    let prefix = format!("{}/", key.0);
    ids.keys()
        .filter(|k| *k == key || k.0.starts_with(&prefix))
        .cloned()
        .collect()
}

/// Every `(flat device, param)` some macro drives with the value it gives it now; container targets
/// (an inner container's `macroN`, which has no engine device) are left out.
pub(super) fn macro_values(expanded: &ExpandedRack) -> BTreeMap<(DeviceKey, String), f32> {
    expanded
        .macro_targets
        .iter()
        .flat_map(|(_, _, targets)| targets)
        .filter(|t| !expanded.alias.contains_key(&t.device))
        .map(|t| ((t.device.clone(), t.param.clone()), t.value))
        .collect()
}

/// The flat devices that publish values for the views: effects with a display and the players.
pub(super) fn metered_keys(expanded: &ExpandedRack) -> Vec<DeviceKey> {
    expanded
        .devices
        .iter()
        .filter(|d| {
            atomik_ui::fx::FxKind::of(&d.type_id).is_some_and(|k| k.meter_count() > 0)
                || atomik_devices::is_player(&d.type_id)
                || atomik_devices::is_modulator(&d.type_id)
        })
        .map(|d| d.key.clone())
        .collect()
}

impl<S: EngineSink> AppCore<S> {
    // ------------------------------------------------------------------ flat ids and keys

    /// The engine id that plays for `key`: the device's own, or for a **container** its `~split`
    /// device (where its notes go). Inner devices are addressed by path key.
    pub fn flat_id(&self, key: &DeviceKey) -> Option<atomik_engine::DeviceInstanceId> {
        self.ids.get(&flat_key(self.doc(), key)).copied()
    }

    /// Every device of the project with its path key, containers' insides included, in the
    /// expansion's order (the devices themselves; split devices are not in the document).
    pub(super) fn flat_devices(&self) -> Vec<(DeviceKey, &RackDevice)> {
        let doc = self.doc();
        if !doc.has_containers() {
            return doc.devices.iter().map(|d| (d.key.clone(), d)).collect();
        }
        doc.expand(&self.registry)
            .devices
            .into_iter()
            .filter_map(|e| doc.resolve(&e.key).map(|d| (e.key, d)))
            .collect()
    }

    /// True (and a message) when `device` is inside a container and cannot have a song track, a
    /// step clip or the step editor.
    pub(super) fn refuse_inner(&mut self, device: &DeviceKey, what: &str) -> bool {
        if !device.0.contains('/') {
            return false;
        }
        let name = self
            .doc()
            .resolve(device)
            .map_or_else(|| device.0.clone(), |d| d.name.clone());
        self.reject(format!(
            "{name} is inside a container: {what} needs a song track, and only top-level devices have one"
        ));
        true
    }

    // ------------------------------------------------------------------ macros and splits

    /// One `set_param` per macro target whose value moved: a target the engine already
    /// has at that value is skipped, and a param a macro drove before this edit and no longer does
    /// gets its stored value back (the descriptor default when the document has none).
    pub(super) fn send_macro_values(&mut self, expanded: &ExpandedRack) {
        let now = macro_values(expanded);
        let freed: Vec<(DeviceKey, String)> = self
            .macro_sent
            .keys()
            .filter(|k| !now.contains_key(*k))
            .cloned()
            .collect();
        for (device, param) in freed {
            if let Some(value) = self.stored_value(&device, &param) {
                self.send_param(&device, &param, value);
            }
        }
        for ((device, param), value) in &now {
            if self.macro_sent.get(&(device.clone(), param.clone())) != Some(value) {
                self.send_param(device, param, *value);
            }
        }
        self.macro_sent = now;
    }

    /// The value the document stores for a param (the descriptor default when none is stored).
    fn stored_value(&self, device: &DeviceKey, param: &str) -> Option<f32> {
        let d = self.doc().resolve(device)?;
        match d.params.get(param) {
            Some(v) => Some(*v),
            None => Some(
                self.registry
                    .get(&d.type_id)?
                    .descriptor
                    .param_by_key(param)?
                    .default,
            ),
        }
    }

    /// Sends the params of `container`'s split device that differ from what the engine has
    /// (slots no longer used go back to their defaults).
    pub(super) fn send_split_params(&mut self, expanded: &ExpandedRack, container: &DeviceKey) {
        let key = split_key(container);
        let Some(device) = expanded.devices.iter().find(|d| d.key == key) else {
            return;
        };
        let wanted = self.full_split_params(&device.params);
        let sent = self.split_sent.entry(key.clone()).or_default().clone();
        for (param, value) in &wanted {
            if sent.get(param) != Some(value) {
                self.send_param(&key, param, *value);
            }
        }
        self.split_sent.insert(key, wanted);
    }

    /// Every param of the split device: the given ones over the descriptor defaults.
    pub(super) fn full_split_params(&self, given: &BTreeMap<String, f32>) -> BTreeMap<String, f32> {
        let Some(factory) = self.registry.get(SPLIT_TYPE) else {
            return given.clone();
        };
        factory
            .descriptor
            .params
            .iter()
            .map(|p| {
                (
                    p.key.to_string(),
                    given.get(p.key).copied().unwrap_or(p.default),
                )
            })
            .collect()
    }

    // ------------------------------------------------------------------ container editor pane

    /// The container editor.
    pub fn container_editor(&self) -> &ContainerEditorView {
        &self.container_editor
    }

    /// The container editor, mutably (bounds, scale, focus).
    pub fn container_editor_mut(&mut self) -> &mut ContainerEditorView {
        &mut self.container_editor
    }

    /// The container open in the editor (shown in place of the arrangement).
    pub fn container_editor_device(&self) -> Option<&DeviceKey> {
        self.container_open
            .then(|| self.container_editor.container())
            .flatten()
    }

    /// Opens the container editor on `container` (ignored for a device that is not a
    /// container), on the MACROS tab with macro `macros` selected when given, else on the
    /// SPLITS tab; it replaces the piano roll and the other editors.
    pub fn open_container_editor(&mut self, container: DeviceKey, macros: Option<u8>) {
        if !is_container(self.doc(), &container) {
            return;
        }
        self.close_piano_roll();
        self.close_sample_editor();
        self.close_step_editor();
        self.close_slice_editor();
        self.close_seq_editor();
        self.container_editor.open(container);
        self.container_open = true;
        match macros {
            Some(i) => {
                self.container_editor.set_tab(ContainerTab::Macros);
                self.container_editor.select_macro(usize::from(i));
            }
            None => self.container_editor.set_tab(ContainerTab::Splits),
        }
        self.container_editor
            .sync(self.document.doc(), &self.registry);
    }

    /// Opens the container editor on the selected container, else the first container of the
    /// rack. False when the rack has none.
    pub fn open_container_editor_default(&mut self) -> bool {
        let doc = self.doc();
        let key = self
            .view
            .selected()
            .filter(|k| is_container(doc, k))
            .cloned()
            .or_else(|| {
                doc.devices
                    .iter()
                    .find(|d| d.inner.is_some())
                    .map(|d| d.key.clone())
            });
        match key {
            Some(key) => {
                self.open_container_editor(key, None);
                true
            }
            None => false,
        }
    }

    /// Closes the container editor (the arrangement shows again).
    pub fn close_container_editor(&mut self) {
        self.container_open = false;
        self.container_editor.set_focused(false);
    }

    // ------------------------------------------------------------------ audition

    /// The notes inputs (flat, with engine ids) an audition of `device` reaches when `device` is a
    /// note player: what its `notes_out` is cabled to in the expanded rack.
    pub(super) fn player_audition_targets(&self, device: &DeviceKey) -> Vec<Target> {
        let expanded = self.doc().expand(&self.registry);
        expanded
            .cables
            .iter()
            .filter(|c| c.from.device == *device && c.from.socket == "notes_out")
            .filter_map(|c| self.flat_target(&expanded, &c.to))
            .collect()
    }

    fn flat_target(&self, expanded: &ExpandedRack, to: &SocketRef) -> Option<Target> {
        let dest = expanded.devices.iter().find(|d| d.key == to.device)?;
        let socket = self
            .registry
            .get(&dest.type_id)?
            .descriptor
            .socket_by_key(&to.socket)?
            .id;
        Some((*self.ids.get(&to.device)?, socket))
    }
}
