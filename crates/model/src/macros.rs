//! Macro banks: eight macro knobs that drive params of other devices.
//!
//! A **bank** is a rack device of type [`MACRO_BANK_TYPE`] (`atomik.macro-bank`) whose params
//! `macro1` to `macro8` are the knobs. They are ordinary params: automatable (the bank has a
//! song track, which is also how an effect or a channel strip is automated, since a track needs
//! a device that takes notes), undoable and learnable from a MIDI controller. What makes them
//! macros is the project's [`MacroSet`]: the **assignments** that bind a macro to a param of
//! another top-level device with a range and an invert flag, and the macros' names.
//!
//! # The rules
//!
//! * A param is driven by **at most one macro** in the whole project. Assigning a param that is
//!   already assigned moves it (the old assignment goes in the same undo step).
//! * Any number of params may follow one macro (at most [`MAX_MACRO_ASSIGNS`] in the project).
//! * A direct `SetParam` on a driven param is refused ([`crate::ModelError::MacroDriven`]);
//!   change the macro, or edit the range of the assignment. Removing an assignment leaves the
//!   param at the value the macro gave it, so nothing jumps.
//! * Assigning to a macro that has no other assignment moves the macro to the position that
//!   gives the target its current value, so nothing jumps either; a macro that already drives
//!   others pulls the new target to its own position.
//! * The target is a top-level device that is not a bank or a container (a container has its own
//!   macros), a param that exists and is not internal, and not the solo of a console strip (the
//!   mix derives the silenced strips from it). A target with automation clips on its own lane is
//!   refused (the lane would fight the macro).
//! * An assignment outlives its devices like a MIDI mapping does: while the bank or the target is
//!   missing it does nothing, and the undo of the removal brings it back to life.
//!
//! # The value
//!
//! `v = min + (max - min) * m`, with `m` the macro (`1 - m` when inverted), clamped to the
//! descriptor range and rounded for a discrete param: [`crate::macro_value`], which uses only
//! `+ - *`, so every platform agrees and a bounce is reproducible. `min` and `max` are plain
//! values of the param (not fractions of a curve), so a cutoff moves linearly in Hz across the
//! macro, as the container macros do.
//!
//! # Where it is applied
//!
//! [`RackDoc::expand`] sets the driven params of the expanded devices (and lists the targets
//! with the container macros in [`crate::ExpandedRack::macro_targets`]), so a live engine
//! rebuild and a bounce see the same values. A song lane on a macro param expands into one lane
//! per target ([`expand_bank_lane`]) with every point mapped by the same function, which equals
//! hand-made lanes on the targets bit for bit.

use crate::container::{MACRO_COUNT, MAX_MACRO_NAME_CHARS};
use crate::expand::{ResolvedTarget, macro_value};
use crate::history::Edit;
use crate::{
    Command, DeviceKey, ExpandedRack, ModelError, RackDevice, RackDoc, console, rules, song,
};
use atomik_core::ParamDescriptor;
use atomik_device_sdk::Registry;

/// Type id of the macro bank device (the model never depends on the device crate).
pub const MACRO_BANK_TYPE: &str = "atomik.macro-bank";
/// Most assignments a project holds.
pub const MAX_MACRO_ASSIGNS: usize = 512;

/// One param driven by one macro of a bank.
#[derive(Debug, Clone, PartialEq)]
pub struct MacroAssign {
    /// Id: counts from 1 (`max + 1`), unique in the project.
    pub id: u32,
    /// The bank device (top-level key).
    pub bank: DeviceKey,
    /// The macro, 0-based (`macro1` is 0).
    pub index: u8,
    /// The driven device (top-level key).
    pub device: DeviceKey,
    /// The driven param.
    pub param: String,
    /// The plain value at macro 0 (at macro 1 when inverted).
    pub min: f32,
    /// The plain value at macro 1 (at macro 0 when inverted).
    pub max: f32,
    /// The macro runs backwards: macro 1 gives `min`.
    pub invert: bool,
}

/// The name of one macro of a bank (a macro without an entry is called `Macro n`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacroName {
    /// The bank device.
    pub bank: DeviceKey,
    /// The macro, 0-based.
    pub index: u8,
    /// The name: trimmed, 1 to [`MAX_MACRO_NAME_CHARS`] characters.
    pub name: String,
}

/// The project's macro data: the assignments and the names (see the module documentation).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MacroSet {
    /// The assignments in id order.
    pub assigns: Vec<MacroAssign>,
    /// The names, in bank and macro order.
    pub names: Vec<MacroName>,
}

impl MacroSet {
    /// True when there is neither an assignment nor a name.
    pub fn is_empty(&self) -> bool {
        self.assigns.is_empty() && self.names.is_empty()
    }

    /// The assignment that drives `param` of `device`, if any (whether or not it is live).
    pub fn driving(&self, device: &DeviceKey, param: &str) -> Option<&MacroAssign> {
        self.assigns
            .iter()
            .find(|a| &a.device == device && a.param == param)
    }

    /// The assignments of macro `index` of `bank`, in id order.
    pub fn of_macro<'a>(
        &'a self,
        bank: &'a DeviceKey,
        index: u8,
    ) -> impl Iterator<Item = &'a MacroAssign> {
        self.assigns
            .iter()
            .filter(move |a| &a.bank == bank && a.index == index)
    }

    /// The stored name of macro `index` of `bank`.
    pub fn name_of(&self, bank: &DeviceKey, index: u8) -> Option<&str> {
        self.names
            .iter()
            .find(|n| &n.bank == bank && n.index == index)
            .map(|n| n.name.as_str())
    }
}

/// A macro edit (each one undo step; `Assign`, `SetRange`, `Remove`, `Clear` and `Rename` all
/// report [`crate::SyncOp::MacrosChanged`] for the banks they touch).
#[derive(Debug, Clone, PartialEq)]
pub enum MacroCommand {
    /// Drive `param` of `device` with macro `index` of `bank`. `bank: None` makes a new bank
    /// (a device at the end of the rack) in the same undo step. `min` and `max` default to the
    /// param's range. A param that is already assigned moves.
    Assign {
        /// The bank, or `None` for a new one.
        bank: Option<DeviceKey>,
        /// The macro, 0-based (below 8).
        index: u8,
        /// The driven device.
        device: DeviceKey,
        /// The driven param.
        param: String,
        /// The value at macro 0, `None` for the param's minimum.
        min: Option<f32>,
        /// The value at macro 1, `None` for the param's maximum.
        max: Option<f32>,
        /// The macro runs backwards.
        invert: bool,
    },
    /// Change the range and direction of assignment `id`.
    SetRange {
        /// The assignment.
        id: u32,
        /// The value at macro 0.
        min: f32,
        /// The value at macro 1.
        max: f32,
        /// The macro runs backwards.
        invert: bool,
    },
    /// Remove assignment `id`; the param keeps the value the macro gave it.
    Remove {
        /// The assignment.
        id: u32,
    },
    /// Remove every assignment of macro `index` of `bank` (the name stays).
    Clear {
        /// The bank.
        bank: DeviceKey,
        /// The macro, 0-based.
        index: u8,
    },
    /// Name macro `index` of `bank`; an empty name goes back to `Macro n`.
    Rename {
        /// The bank.
        bank: DeviceKey,
        /// The macro, 0-based.
        index: u8,
        /// The new name.
        name: String,
    },
}

fn bad(message: impl Into<String>) -> ModelError {
    ModelError::BadMacro(message.into())
}

/// The param key of macro `index` (0-based): `macro1` to `macro8`.
pub fn macro_key(index: u8) -> String {
    format!("macro{}", u32::from(index) + 1)
}

/// The index of a `macroN` param key.
pub fn macro_index_of(param: &str) -> Option<u8> {
    let n: usize = param.strip_prefix("macro")?.parse().ok()?;
    ((1..=MACRO_COUNT).contains(&n) && param == format!("macro{n}")).then(|| (n - 1) as u8)
}

/// The value assignment `a` gives a param for macro value `m`.
pub fn assign_value(a: &MacroAssign, m: f32, p: &ParamDescriptor) -> f32 {
    macro_value(a.min, a.max, if a.invert { 1.0 - m } else { m }, p)
}

/// The macro position that gives a param the plain value `v` through `a` (the inverse of
/// [`assign_value`], clamped to 0..=1; 0 for an empty range).
pub fn assign_position(a: &MacroAssign, v: f32) -> f32 {
    let span = a.max - a.min;
    if span == 0.0 || !span.is_finite() {
        return 0.0;
    }
    let m = ((v - a.min) / span).clamp(0.0, 1.0);
    if a.invert { 1.0 - m } else { m }
}

impl RackDoc {
    /// The macro value `m` of macro `index` of `bank` (the stored one, or 0).
    pub fn bank_macro_value(&self, bank: &DeviceKey, index: u8) -> f32 {
        self.device(bank)
            .and_then(|d| d.params.get(&macro_key(index)))
            .copied()
            .unwrap_or(0.0)
    }

    /// The macro banks of the rack, in rack order.
    pub fn banks(&self) -> impl Iterator<Item = &RackDevice> {
        self.devices.iter().filter(|d| d.type_id == MACRO_BANK_TYPE)
    }

    /// The display name of macro `index` of `bank`: its stored name, else `Macro n`.
    pub fn macro_name(&self, bank: &DeviceKey, index: u8) -> String {
        self.macros.name_of(bank, index).map_or_else(
            || format!("Macro {}", u32::from(index) + 1),
            ToOwned::to_owned,
        )
    }

    /// True when the bank of `a` and its target device exist (it may still name a param the
    /// device lacks: the evaluation checks that with the descriptor).
    pub fn macro_assign_live(&self, a: &MacroAssign) -> bool {
        self.device(&a.bank)
            .is_some_and(|d| d.type_id == MACRO_BANK_TYPE)
            && self.device(&a.device).is_some()
    }

    /// The live assignment that drives `param` of `device`, if any: what refuses a `SetParam`
    /// and what the views draw locked.
    pub fn bank_driver(&self, device: &DeviceKey, param: &str) -> Option<&MacroAssign> {
        self.macros
            .driving(device, param)
            .filter(|a| self.macro_assign_live(a))
    }

    /// The value the macro gives `param` of `device` now, if a live assignment drives it.
    pub fn driven_value(
        &self,
        registry: &Registry,
        device: &DeviceKey,
        param: &str,
    ) -> Option<f32> {
        let a = self.bank_driver(device, param)?;
        let d = self.device(device)?;
        let p = registry.get(&d.type_id)?.descriptor.param_by_key(param)?;
        Some(assign_value(a, self.bank_macro_value(&a.bank, a.index), p))
    }

    /// The value a param has now: the macro's when one drives it, else the stored value or the
    /// descriptor default.
    pub fn effective_param(
        &self,
        registry: &Registry,
        device: &DeviceKey,
        param: &str,
    ) -> Option<f32> {
        self.driven_value(registry, device, param)
            .or_else(|| self.param_value(device, param, registry))
    }

    /// The `(device, param, value)` of every target of macro `index` of `bank` at the current
    /// macro value, in assignment order (dormant assignments left out).
    pub fn bank_updates(
        &self,
        registry: &Registry,
        bank: &DeviceKey,
    ) -> Vec<(DeviceKey, String, f32)> {
        self.macros
            .assigns
            .iter()
            .filter(|a| &a.bank == bank)
            .filter_map(|a| {
                let v = self.driven_value(registry, &a.device, &a.param)?;
                Some((a.device.clone(), a.param.clone(), v))
            })
            .collect()
    }

    /// The params of `device` that a live assignment drives, as `(param, macro index)`.
    pub fn driven_params(&self, device: &DeviceKey) -> Vec<(String, u8)> {
        self.macros
            .assigns
            .iter()
            .filter(|a| &a.device == device && self.macro_assign_live(a))
            .map(|a| (a.param.clone(), a.index))
            .collect()
    }

    /// A copy of device `key` whose driven params hold the value the macros give them (what a
    /// view of the device shows); the plain device when nothing drives it.
    pub fn device_with_driven_values(
        &self,
        registry: &Registry,
        key: &DeviceKey,
    ) -> Option<RackDevice> {
        let mut d = self.device(key)?.clone();
        for (param, _) in self.driven_params(key) {
            if let Some(v) = self.driven_value(registry, key, &param) {
                d.params.insert(param, v);
            }
        }
        Some(d)
    }

    /// A copy for the views when some param shows a value other than the stored one: the devices
    /// carry the driven values (see [`RackDoc::device_with_driven_values`]) and the song is empty,
    /// as in the scope document of a container. `None` when no live assignment changes a
    /// value, so a project without macros costs the views nothing.
    pub fn view_with_driven_values(&self, registry: &Registry) -> Option<RackDoc> {
        let differs = self.macros.assigns.iter().any(|a| {
            self.driven_value(registry, &a.device, &a.param)
                .is_some_and(|v| {
                    self.param_value(&a.device, &a.param, registry)
                        .is_none_or(|stored| stored.to_bits() != v.to_bits())
                })
        });
        if !differs {
            return None;
        }
        let devices = self
            .devices
            .iter()
            .map(|d| {
                self.device_with_driven_values(registry, &d.key)
                    .unwrap_or_else(|| d.clone())
            })
            .collect();
        Some(RackDoc {
            sample_rate: self.sample_rate,
            tempo_bpm: self.tempo_bpm,
            seed: self.seed,
            output_channels: self.output_channels,
            input_channels: self.input_channels,
            assets: self.assets.clone(),
            devices,
            columns: self.columns,
            cables: self.cables.clone(),
            song: crate::empty_song(self.tempo_bpm),
            console: self.console.clone(),
            midi_maps: Vec::new(),
            macros: self.macros.clone(),
        })
    }

    /// Why `param` of `device` cannot be a macro target, or `None` when it can (not counting the
    /// assignment it may already have or the automation on its lane).
    pub fn macro_target_problem(
        &self,
        registry: &Registry,
        device: &DeviceKey,
        param: &str,
    ) -> Option<String> {
        if device.0.contains('/') {
            return Some(format!(
                "{device} is inside a container: use the container's own macros"
            ));
        }
        let Some(d) = self.device(device) else {
            return Some(format!("no device {device}"));
        };
        if d.type_id == MACRO_BANK_TYPE {
            return Some("a macro bank is not driven by a macro".to_owned());
        }
        if d.inner.is_some() {
            return Some(format!("{} is a container: use its own macros", d.name));
        }
        let Some(desc) = registry.get(&d.type_id).map(|f| f.descriptor) else {
            return Some(format!("unknown device type {}", d.type_id));
        };
        if console::is_internal_param(param) || desc.param_by_key(param).is_none() {
            return Some(format!("{} has no param {param:?}", d.name));
        }
        if param == "solo" && console::is_console_type(&d.type_id) {
            return Some("solo is derived by the mixer and cannot follow a macro".to_owned());
        }
        None
    }
}

/// The lanes a song lane on a macro param of `bank` expands to: one per live target,
/// `(device, param, points)`, every point mapped by [`assign_value`]. `None` when `bank` is not
/// a bank or `param` is not one of its `macroN` params; an unassigned macro gives no lanes.
pub fn expand_bank_lane(
    doc: &RackDoc,
    registry: &Registry,
    bank: &DeviceKey,
    param: &str,
    points: &[song::AutomationPoint],
) -> Option<Vec<(DeviceKey, String, Vec<song::AutomationPoint>)>> {
    if doc.device(bank)?.type_id != MACRO_BANK_TYPE {
        return None;
    }
    let index = macro_index_of(param)?;
    let mut out = Vec::new();
    for a in doc.macros.of_macro(bank, index) {
        if !doc.macro_assign_live(a) {
            continue;
        }
        let Some(p) = doc
            .device(&a.device)
            .and_then(|d| registry.get(&d.type_id))
            .and_then(|f| f.descriptor.param_by_key(&a.param))
        else {
            continue;
        };
        let mapped = points
            .iter()
            .map(|pt| song::AutomationPoint {
                value: assign_value(a, pt.value, p),
                ..*pt
            })
            .collect();
        out.push((a.device.clone(), a.param.clone(), mapped));
    }
    Some(out)
}

/// Sets the driven params of the expanded devices and lists the targets (see the module
/// documentation). A document without assignments is untouched.
pub(crate) fn apply(doc: &RackDoc, registry: &Registry, expanded: &mut ExpandedRack) {
    for bank in doc.banks() {
        for index in 0..MACRO_COUNT as u8 {
            let m = doc.bank_macro_value(&bank.key, index);
            let mut resolved = Vec::new();
            for a in doc.macros.of_macro(&bank.key, index) {
                if !doc.macro_assign_live(a) {
                    continue;
                }
                let Some(p) = doc
                    .device(&a.device)
                    .and_then(|d| registry.get(&d.type_id))
                    .and_then(|f| f.descriptor.param_by_key(&a.param))
                else {
                    continue;
                };
                let v = assign_value(a, m, p);
                let Some(target) = expanded.devices.iter_mut().find(|e| e.key == a.device) else {
                    continue;
                };
                target.params.insert(a.param.clone(), v);
                resolved.push(ResolvedTarget {
                    device: a.device.clone(),
                    param: a.param.clone(),
                    min: a.min,
                    max: a.max,
                    value: v,
                });
            }
            if !resolved.is_empty() {
                expanded
                    .macro_targets
                    .push((bank.key.clone(), index, resolved));
            }
        }
    }
}

fn next_id(set: &MacroSet) -> u32 {
    set.assigns.iter().map(|a| a.id).max().unwrap_or(0) + 1
}

fn check_range(p: &ParamDescriptor, min: f32, max: f32) -> Result<(), ModelError> {
    for (what, v) in [("min", min), ("max", max)] {
        if !v.is_finite() || v < p.min || v > p.max {
            return Err(ModelError::OutOfRange(format!(
                "the {what} {v} is outside {}..={} ({})",
                p.min, p.max, p.name
            )));
        }
    }
    Ok(())
}

fn check_index(index: u8) -> Result<(), ModelError> {
    if usize::from(index) >= MACRO_COUNT {
        return Err(bad(format!("macros are 1 to {MACRO_COUNT}")));
    }
    Ok(())
}

fn check_bank(doc: &RackDoc, bank: &DeviceKey) -> Result<(), ModelError> {
    match doc.device(bank) {
        Some(d) if d.type_id == MACRO_BANK_TYPE => Ok(()),
        Some(d) => Err(bad(format!("{} is not a macro bank", d.name))),
        None => Err(ModelError::UnknownDevice(bank.0.clone())),
    }
}

fn param_edit(doc: &RackDoc, key: &DeviceKey, param: &str, new: f32, default: f32) -> Option<Edit> {
    let old = doc.device(key)?.params.get(param).copied();
    (old.unwrap_or(default).to_bits() != new.to_bits()).then(|| Edit::Param {
        key: key.clone(),
        param: param.to_owned(),
        old,
        new: Some(new),
        default,
    })
}

/// The edits that leave each freed param at the value its macro gave it.
fn freeing_edits(doc: &RackDoc, registry: &Registry, freed: &[&MacroAssign]) -> Vec<Edit> {
    freed
        .iter()
        .filter_map(|a| {
            let v = doc.driven_value(registry, &a.device, &a.param)?;
            let default = doc
                .device(&a.device)
                .and_then(|d| registry.get(&d.type_id))
                .and_then(|f| f.descriptor.param_by_key(&a.param))
                .map_or(0.0, |p| p.default);
            param_edit(doc, &a.device, &a.param, v, default)
        })
        .collect()
}

/// Plans a macro command: one [`Edit::Macros`] (the whole set swaps), with the edits it needs
/// beside it (a new bank, a macro moved to keep its target, a freed param kept at its value).
pub(crate) fn plan(
    doc: &RackDoc,
    registry: &Registry,
    command: MacroCommand,
) -> Result<Edit, ModelError> {
    let old = doc.macros.clone();
    let mut new = old.clone();
    let mut before: Vec<Edit> = Vec::new();
    let mut after: Vec<Edit> = Vec::new();
    match command {
        MacroCommand::Assign {
            bank,
            index,
            device,
            param,
            min,
            max,
            invert,
        } => {
            check_index(index)?;
            // A new bank is made first, on a scratch copy the rest is planned against.
            let mut scratch;
            let doc_now: &RackDoc = match &bank {
                Some(b) => {
                    check_bank(doc, b)?;
                    doc
                }
                None => {
                    let (edit, _) = crate::Document::plan(
                        doc,
                        Command::AddDevice {
                            index: doc.devices.len(),
                            type_id: MACRO_BANK_TYPE.to_owned(),
                            key: None,
                        },
                        registry,
                    )?;
                    scratch = doc.clone();
                    edit.perform(&mut scratch)?;
                    before.push(edit);
                    &scratch
                }
            };
            let bank_key = match bank {
                Some(b) => b,
                None => doc_now
                    .devices
                    .last()
                    .map(|d| d.key.clone())
                    .ok_or_else(|| bad("the bank was not created"))?,
            };
            if let Some(problem) = doc_now.macro_target_problem(registry, &device, &param) {
                return Err(bad(problem));
            }
            let d = rules::resolve(doc_now, &device)?;
            let p = rules::descriptor(registry, &d.type_id)?
                .param_by_key(&param)
                .ok_or_else(|| ModelError::UnknownKey(param.clone()))?;
            let (min, max) = (min.unwrap_or(p.min), max.unwrap_or(p.max));
            check_range(p, min, max)?;
            if let Some(t) = doc_now.song.track(&device)
                && let Some(i) = t.automation_lane(&param)
                && !t.lanes[i].clips.is_empty()
            {
                return Err(bad(format!(
                    "{} {} has automation on its own lane: delete it first",
                    d.name, p.name
                )));
            }
            // The macro that has nothing yet takes the position that keeps the target's value.
            let alone = new.of_macro(&bank_key, index).next().is_none();
            let stored = doc_now
                .param_value(&device, &param, registry)
                .unwrap_or(p.default);
            new.assigns
                .retain(|a| !(a.device == device && a.param == param));
            let assign = MacroAssign {
                id: next_id(&new),
                bank: bank_key.clone(),
                index,
                device,
                param,
                min,
                max,
                invert,
            };
            if alone {
                let m = assign_position(&assign, stored);
                let key = macro_key(index);
                if let Some(e) = param_edit(doc_now, &bank_key, &key, m, 0.0) {
                    after.push(e);
                }
            }
            new.assigns.push(assign);
        }
        MacroCommand::SetRange {
            id,
            min,
            max,
            invert,
        } => {
            let Some(a) = new.assigns.iter_mut().find(|a| a.id == id) else {
                return Err(bad(format!("no assignment {id}")));
            };
            if let Some(p) = doc
                .device(&a.device)
                .and_then(|d| registry.get(&d.type_id))
                .and_then(|f| f.descriptor.param_by_key(&a.param))
            {
                check_range(p, min, max)?;
            } else if !(min.is_finite() && max.is_finite()) {
                return Err(ModelError::OutOfRange("the range is not finite".into()));
            }
            (a.min, a.max, a.invert) = (min, max, invert);
        }
        MacroCommand::Remove { id } => {
            let Some(a) = old.assigns.iter().find(|a| a.id == id) else {
                return Err(bad(format!("no assignment {id}")));
            };
            after = freeing_edits(doc, registry, &[a]);
            new.assigns.retain(|a| a.id != id);
        }
        MacroCommand::Clear { bank, index } => {
            let freed: Vec<&MacroAssign> = old.of_macro(&bank, index).collect();
            if freed.is_empty() {
                return Err(bad("the macro has no assignment"));
            }
            after = freeing_edits(doc, registry, &freed);
            new.assigns
                .retain(|a| !(a.bank == bank && a.index == index));
        }
        MacroCommand::Rename { bank, index, name } => {
            check_index(index)?;
            let name = name.trim().to_owned();
            if name.chars().count() > MAX_MACRO_NAME_CHARS {
                return Err(bad(format!(
                    "a macro name has at most {MAX_MACRO_NAME_CHARS} characters"
                )));
            }
            new.names.retain(|n| !(n.bank == bank && n.index == index));
            if !name.is_empty() {
                new.names.push(MacroName { bank, index, name });
                new.names
                    .sort_by(|a, b| (&a.bank, a.index).cmp(&(&b.bank, b.index)));
            }
        }
    }
    if new.assigns.len() > MAX_MACRO_ASSIGNS {
        return Err(bad(format!("at most {MAX_MACRO_ASSIGNS} assignments")));
    }
    if new == old && before.is_empty() {
        return Err(bad("the macro is already like that"));
    }
    new.assigns.sort_by_key(|a| a.id);
    let mut edits = before;
    edits.push(Edit::Macros { old, new });
    edits.extend(after);
    Ok(if edits.len() == 1 {
        edits.remove(0)
    } else {
        Edit::Compound(edits)
    })
}
