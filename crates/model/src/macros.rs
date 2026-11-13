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
//! * The target is a device of the rack or one inside a container (a path key such as
//!   `layer/synth`) that is not a bank or a container, a param that exists and is not internal and
//!   that no macro of a container drives, and not the solo of a console strip (the mix derives the
//!   silenced strips from it). A device that holds assigned params cannot move into or out of a
//!   container (its key would change).
//! * A target may have an automation lane of its own: the lane sets the base and the macro adds
//!   its range (see [`crate::macro_lane`]). A lane exists only for a device of the rack.
//! * An assignment outlives its devices like a MIDI mapping does: while the bank or the target is
//!   missing it does nothing, and the undo of the removal brings it back to life.
//!
//! # The value
//!
//! `v = min + (max - min) * curve(m)`, with `m` the macro (`1 - m` when inverted), clamped to the
//! descriptor range and rounded for a discrete param: [`crate::macro_value`], which uses only
//! `+ - *`, so every platform agrees and a bounce is reproducible. `min` and `max` are plain
//! values of the param (not fractions of a curve). The default [`MacroCurve::Linear`] moves a
//! cutoff linearly in Hz across the macro, as the container macros do; [`MacroCurve::Exp`] (slow
//! start, `m * m`) and [`MacroCurve::Log`] (fast start, the mirror of it) bend the travel between
//! the two ends, all in the same plain-value range.
//!
//! # Smoothing
//!
//! An assignment may carry a smoothing time (`smooth_ms`, at most [`MAX_MACRO_SMOOTH_MS`]): when a
//! live macro moves, the app glides the target to its new value over that time instead of
//! jumping. It is a live-playing aid only: the saved value, an automation lane and a bounce use the
//! unsmoothed mapping, so a render stays reproducible.
//!
//! # Editing a driven param directly
//!
//! Turning the knob of a driven param in the rack does not fight the macro and is not refused
//! there: [`MacroCommand::MoveRange`] shifts the whole range of the assignment so that the param
//! shows the new value at the macro position it has now (as far as the param range allows).
//! Drags carry a merge id and are one undo step. (A plain `SetParam` stays refused.)
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
/// The longest smoothing time of an assignment, in milliseconds.
pub const MAX_MACRO_SMOOTH_MS: u16 = 5000;

/// How the macro travel maps onto the range of an assignment.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MacroCurve {
    /// The value moves in step with the macro.
    #[default]
    Linear,
    /// Slow start, steep end (`m * m`): the low half of the macro covers a quarter of the range.
    Exp,
    /// Steep start, slow end (`1 - (1 - m)^2`): the low half of the macro covers three quarters.
    Log,
}

impl MacroCurve {
    /// Every curve, in menu order.
    pub const ALL: [MacroCurve; 3] = [MacroCurve::Linear, MacroCurve::Exp, MacroCurve::Log];

    /// The name the project file and the panel use.
    pub fn name(self) -> &'static str {
        match self {
            MacroCurve::Linear => "linear",
            MacroCurve::Exp => "exp",
            MacroCurve::Log => "log",
        }
    }

    /// The curve called `name`.
    pub fn from_name(name: &str) -> Option<MacroCurve> {
        Self::ALL.into_iter().find(|c| c.name() == name)
    }

    /// The next curve in [`MacroCurve::ALL`], wrapping.
    pub fn next(self) -> MacroCurve {
        let at = Self::ALL.iter().position(|c| *c == self).unwrap_or(0);
        Self::ALL[(at + 1) % Self::ALL.len()]
    }

    /// The share of the range (0 to 1) at macro position `m` (0 to 1, clamped). Only `+ - *`.
    pub fn shape(self, m: f32) -> f32 {
        match self {
            MacroCurve::Linear => m,
            MacroCurve::Exp => {
                let m = m.clamp(0.0, 1.0);
                m * m
            }
            MacroCurve::Log => {
                let m = m.clamp(0.0, 1.0);
                m * (2.0 - m)
            }
        }
    }

    /// The macro position that gives the share `y` of the range: the inverse of
    /// [`MacroCurve::shape`] (the one place that takes a square root).
    pub fn unshape(self, y: f32) -> f32 {
        let y = y.clamp(0.0, 1.0);
        match self {
            MacroCurve::Linear => y,
            MacroCurve::Exp => y.sqrt(),
            MacroCurve::Log => 1.0 - (1.0 - y).sqrt(),
        }
    }
}

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
    /// How the macro travel maps onto the range.
    pub curve: MacroCurve,
    /// The glide time of a live move, in milliseconds (0: none, at most [`MAX_MACRO_SMOOTH_MS`]).
    pub smooth_ms: u16,
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
        /// Drag-merge group (consecutive ranges of one drag are one undo step), or `None`.
        merge: Option<u64>,
    },
    /// Change the curve and the smoothing time of assignment `id`.
    SetShape {
        /// The assignment.
        id: u32,
        /// How the macro travel maps onto the range.
        curve: MacroCurve,
        /// The glide time of a live move in milliseconds, at most [`MAX_MACRO_SMOOTH_MS`].
        smooth_ms: u16,
    },
    /// A direct edit of a driven param: shift the range of its assignment so the param shows
    /// `value` at the macro position it has now (as far as the param range allows).
    MoveRange {
        /// The driven device.
        device: DeviceKey,
        /// The driven param.
        param: String,
        /// The value the param should show now.
        value: f32,
        /// Drag-merge group, or `None`.
        merge: Option<u64>,
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
    macro_value(a.min, a.max, assign_share(a, m), p)
}

/// The share of the range (0 to 1) assignment `a` gives at macro value `m`: the direction and
/// the curve applied.
pub(crate) fn assign_share(a: &MacroAssign, m: f32) -> f32 {
    a.curve.shape(if a.invert { 1.0 - m } else { m })
}

/// The macro position that gives a param the plain value `v` through `a` (the inverse of
/// [`assign_value`], clamped to 0..=1; 0 for an empty range).
pub fn assign_position(a: &MacroAssign, v: f32) -> f32 {
    let span = a.max - a.min;
    if span == 0.0 || !span.is_finite() {
        return 0.0;
    }
    let m = a.curve.unshape(((v - a.min) / span).clamp(0.0, 1.0));
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
            && self.resolve(&a.device).is_some()
    }

    /// The live assignment that drives `param` of `device`, if any: what refuses a `SetParam`
    /// and what the views draw locked.
    pub fn bank_driver(&self, device: &DeviceKey, param: &str) -> Option<&MacroAssign> {
        self.macros
            .driving(device, param)
            .filter(|a| self.macro_assign_live(a))
    }

    /// The range a direct edit of the driven param would give its assignment: `(id, min, max)`
    /// of [`MacroCommand::MoveRange`] for `value`, or `None` when nothing drives the param or the
    /// range would not change (the edit is already what the param shows, or the range is at the
    /// end of the param's own range). The whole range shifts so the unrounded value at the macro's
    /// position lands on `value`, kept inside the param range at both ends.
    pub fn moved_range(
        &self,
        registry: &Registry,
        device: &DeviceKey,
        param: &str,
        value: f32,
    ) -> Option<(u32, f32, f32)> {
        let a = self.bank_driver(device, param)?;
        let p = registry
            .get(&self.resolve(device)?.type_id)?
            .descriptor
            .param_by_key(param)?;
        let at = a.min + (a.max - a.min) * assign_share(a, self.bank_macro_value(&a.bank, a.index));
        let (lo, hi) = (a.min.min(a.max), a.min.max(a.max));
        let shift = (p.clamp(value) - at).clamp(p.min - lo, p.max - hi);
        (shift != 0.0 && shift.is_finite()).then(|| {
            let moved = |v: f32| (v + shift).clamp(p.min, p.max);
            (a.id, moved(a.min), moved(a.max))
        })
    }

    /// The value the macro gives `param` of `device` now, if a live assignment drives it.
    pub fn driven_value(
        &self,
        registry: &Registry,
        device: &DeviceKey,
        param: &str,
    ) -> Option<f32> {
        let a = self.bank_driver(device, param)?;
        let d = self.resolve(device)?;
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
        // A target inside a container is a path key: its value goes into the inside.
        let mut devices = self.devices.clone();
        for a in &self.macros.assigns {
            if let Some(v) = self.driven_value(registry, &a.device, &a.param)
                && let Some(d) = crate::container::find_relative_mut(&mut devices, &a.device.0)
            {
                d.params.insert(a.param.clone(), v);
            }
        }
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
            song: std::sync::Arc::new(crate::empty_song(self.tempo_bpm)),
            console: self.console.clone(),
            midi_maps: Vec::new(),
            macros: self.macros.clone(),
            info: self.info.clone(),
            tune_cents: self.tune_cents,
            sort_mode: self.sort_mode,
            references: self.references.clone(),
            delivery: self.delivery.clone(),
            video: self.video.clone(),
            camera: self.camera.clone(),
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
        let Some(d) = self.resolve(device) else {
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
        if console::is_hidden_param(param) || desc.param_by_key(param).is_none() {
            return Some(format!("{} has no param {param:?}", d.name));
        }
        if param == "solo" && console::is_console_type(&d.type_id) {
            return Some("solo is derived by the mixer and cannot follow a macro".to_owned());
        }
        if let Some((container, index)) = self.macro_driver(device, param) {
            return Some(format!(
                "{} {param} is driven by macro {} of {container}: unmap it there first",
                d.name,
                index + 1
            ));
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
        // A target with a lane of its own plays that lane with the macro added (see
        // `crate::macro_lane`): the macro's lane is folded into it.
        if doc.has_own_lane_points(&a.device, &a.param) {
            continue;
        }
        let Some(p) = doc
            .resolve(&a.device)
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
                    .resolve(&a.device)
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
    let old = doc.resolve(key)?.params.get(param).copied();
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
                .resolve(&a.device)
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
                curve: MacroCurve::Linear,
                smooth_ms: 0,
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
            merge: _,
        } => {
            let Some(a) = new.assigns.iter_mut().find(|a| a.id == id) else {
                return Err(bad(format!("no assignment {id}")));
            };
            if let Some(p) = doc
                .resolve(&a.device)
                .and_then(|d| registry.get(&d.type_id))
                .and_then(|f| f.descriptor.param_by_key(&a.param))
            {
                check_range(p, min, max)?;
            } else if !(min.is_finite() && max.is_finite()) {
                return Err(ModelError::OutOfRange("the range is not finite".into()));
            }
            (a.min, a.max, a.invert) = (min, max, invert);
        }
        MacroCommand::SetShape {
            id,
            curve,
            smooth_ms,
        } => {
            if smooth_ms > MAX_MACRO_SMOOTH_MS {
                return Err(ModelError::OutOfRange(format!(
                    "a smoothing time is at most {MAX_MACRO_SMOOTH_MS} ms"
                )));
            }
            let Some(a) = new.assigns.iter_mut().find(|a| a.id == id) else {
                return Err(bad(format!("no assignment {id}")));
            };
            (a.curve, a.smooth_ms) = (curve, smooth_ms);
        }
        MacroCommand::MoveRange {
            device,
            param,
            value,
            merge: _,
        } => {
            if !value.is_finite() {
                return Err(ModelError::OutOfRange(format!(
                    "{device} {param:?} is not finite"
                )));
            }
            if doc.bank_driver(&device, &param).is_none() {
                return Err(bad(format!("{device} {param} is not driven by a macro")));
            }
            let Some((id, min, max)) = doc.moved_range(registry, &device, &param, value) else {
                return Err(bad("the macro is already like that"));
            };
            let Some(a) = new.assigns.iter_mut().find(|a| a.id == id) else {
                return Err(bad(format!("no assignment {id}")));
            };
            (a.min, a.max) = (min, max);
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
