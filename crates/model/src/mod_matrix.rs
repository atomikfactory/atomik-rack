//! The modulation matrix of a device: which source moves which target, per slot.
//!
//! A device that declares [`ModTables`] (see the device SDK) has [`MOD_SLOTS`] slots. The
//! document keeps the structure of each slot, [`ModSlot`] (a source, a target and an optional
//! `via` source that scales the amount), in [`RackDevice::mods`]; the **amount** of slot `n` is
//! the ordinary param `mod<n>_amount`, so it is saved, undone, automated, mapped and put in a
//! patch like any other. Ids are indices into the device's tables and are stored as ids: a
//! device only ever appends to its tables.
//!
//! The engine does not know slots. [`expand`](crate::RackDoc::expand) turns the structure into
//! the device's `internal.mod<n>_source`, `_target` and `_via` params (id + 1, 0 for none) and the
//! device sums the slots itself with a `ModBus`; an edit reports
//! [`SyncOp::ModSlotsChanged`](crate::SyncOp::ModSlotsChanged) and the app re-sends the three
//! params of every slot.
//!
//! The list is stored only when a slot is in use (project format 41), position `n` of the list is
//! slot `n`, and the list never ends in an empty slot.

use crate::history::{Edit, Swap};
use crate::{DeviceKey, ModelError, RackDevice, RackDoc, rules};
use atomik_device_sdk::{MOD_SLOTS, ModField, ModTables, Registry, mod_param_key};
use std::collections::BTreeMap;

/// The structure of one used slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModSlot {
    /// Id of the source in the device's source table.
    pub source: u16,
    /// Id of the target in the device's target table.
    pub target: u16,
    /// Id of a source that scales the amount (clamped to `0..=1`), or `None` for no scaling.
    pub via: Option<u16>,
}

impl ModSlot {
    /// A slot without a `via` source.
    pub const fn new(source: u16, target: u16) -> Self {
        Self {
            source,
            target,
            via: None,
        }
    }

    /// The same slot scaled by `via`.
    pub const fn via(mut self, via: u16) -> Self {
        self.via = Some(via);
        self
    }
}

/// Checks a slot list against the tables of its device (`None`: the device has no matrix, so the
/// list must be empty): at most [`MOD_SLOTS`] entries, no trailing empty slot, every id inside
/// its table.
pub(crate) fn check_slots(
    mods: &[Option<ModSlot>],
    tables: Option<&ModTables>,
) -> Result<(), String> {
    if mods.is_empty() {
        return Ok(());
    }
    let Some(tables) = tables else {
        return Err("this device has no modulation matrix".to_owned());
    };
    if mods.len() > MOD_SLOTS {
        return Err(format!(
            "{} slots is more than the {MOD_SLOTS} a device has",
            mods.len()
        ));
    }
    if mods.last() == Some(&None) {
        return Err("the slot list ends in an empty slot".to_owned());
    }
    for (i, slot) in mods.iter().enumerate() {
        let Some(slot) = slot else { continue };
        if usize::from(slot.source) >= tables.sources.len() {
            return Err(format!(
                "slot {i} names source {}, the device has {}",
                slot.source,
                tables.sources.len()
            ));
        }
        if usize::from(slot.target) >= tables.targets.len() {
            return Err(format!(
                "slot {i} names target {}, the device has {}",
                slot.target,
                tables.targets.len()
            ));
        }
        if let Some(via) = slot.via
            && usize::from(via) >= tables.sources.len()
        {
            return Err(format!(
                "slot {i} scales by source {via}, the device has {}",
                tables.sources.len()
            ));
        }
    }
    Ok(())
}

/// The engine params of a slot list: `internal.mod<n>_source`, `_target` and `_via` of every
/// slot, id + 1 (0 for none). With `all` the empty slots are listed too (at 0), which is what a
/// live update sends so a removed slot goes quiet; without it only used slots are listed, which
/// is what a device starts from (a missing param is its default, 0).
pub fn engine_params(mods: &[Option<ModSlot>], all: bool) -> Vec<(String, f32)> {
    let mut out = Vec::new();
    let slots = if all { MOD_SLOTS } else { mods.len() };
    for n in 0..slots.min(MOD_SLOTS) {
        let slot = mods.get(n).copied().flatten();
        if slot.is_none() && !all {
            continue;
        }
        let id = |v: Option<u16>| v.map_or(0.0, |v| f32::from(v) + 1.0);
        out.push((
            mod_param_key(n, ModField::Source),
            id(slot.map(|s| s.source)),
        ));
        out.push((
            mod_param_key(n, ModField::Target),
            id(slot.map(|s| s.target)),
        ));
        out.push((
            mod_param_key(n, ModField::Via),
            id(slot.and_then(|s| s.via)),
        ));
    }
    out
}

/// Adds the slot params of `mods` to a device's flat params (see [`engine_params`]).
pub(crate) fn extend_params(params: &mut BTreeMap<String, f32>, mods: &[Option<ModSlot>]) {
    params.extend(engine_params(mods, false));
}

/// The param that holds the amount of slot `slot`.
pub fn amount_param(slot: usize) -> String {
    mod_param_key(slot.min(MOD_SLOTS - 1), ModField::Amount)
}

/// `mods` with slot `slot` set (or cleared), padded and trimmed to the canonical form.
fn with_slot(
    mods: &[Option<ModSlot>],
    slot: usize,
    value: Option<ModSlot>,
) -> Vec<Option<ModSlot>> {
    let mut out = mods.to_vec();
    if out.len() <= slot {
        out.resize(slot + 1, None);
    }
    out[slot] = value;
    while out.last() == Some(&None) {
        out.pop();
    }
    out
}

/// Plans `SetModSlot`: slot `slot` of the device takes `value`, or is cleared (`None`), which
/// also puts the slot's amount back to 0. One undo step.
pub(crate) fn plan_set_slot(
    doc: &RackDoc,
    registry: &Registry,
    key: &DeviceKey,
    slot: u8,
    value: Option<ModSlot>,
) -> Result<Edit, ModelError> {
    let device = rules::resolve(doc, key)?;
    let tables = registry
        .mod_tables(&device.type_id)
        .ok_or_else(|| ModelError::BadScope(format!("{key} has no modulation matrix")))?;
    let slot = usize::from(slot);
    if slot >= MOD_SLOTS {
        return Err(ModelError::BadScope(format!(
            "slot {slot} is outside 0..{MOD_SLOTS}"
        )));
    }
    let new = with_slot(&device.mods, slot, value);
    check_slots(&new, Some(tables)).map_err(ModelError::BadScope)?;
    if new == device.mods {
        return Err(ModelError::BadScope(format!(
            "slot {slot} of {key} already holds that"
        )));
    }
    let mut edits = vec![Edit::Swap(Swap::ModSlots {
        key: key.clone(),
        old: device.mods.clone(),
        new,
    })];
    let amount = amount_param(slot);
    if value.is_none()
        && let Some(&old) = device.params.get(&amount)
    {
        edits.push(Edit::Param {
            key: key.clone(),
            param: amount,
            old: Some(old),
            new: None,
            default: 0.0,
        });
    }
    Ok(if edits.len() == 1 {
        edits.remove(0)
    } else {
        Edit::Compound(edits)
    })
}

/// Reads a `"mods"` array: `[{ "slot", "source", "target", "via"? }, ...]`, slots ascending and
/// unique, ids inside the tables of the device type (a device without a matrix has none).
pub(crate) fn load_slots(
    v: &serde_json::Value,
    path: &crate::patch::JsonPath,
    tables: Option<&ModTables>,
) -> Result<Vec<Option<ModSlot>>, ModelError> {
    use crate::patch::{array, object, uint};
    let items = array(v, path)?;
    if items.is_empty() {
        return Err(path.error("an empty list is not stored; leave the field out"));
    }
    let mut out: Vec<Option<ModSlot>> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let item_path = path.field(&i.to_string());
        let obj = object(
            item,
            &item_path,
            &["slot", "source", "target", "via"],
            &["slot", "source", "target"],
        )?;
        let slot = uint(&obj["slot"], &item_path.field("slot"), MOD_SLOTS as u64 - 1)? as usize;
        if slot < out.len() {
            return Err(item_path
                .field("slot")
                .error("slots must be unique and in ascending order"));
        }
        let id = |name: &str| uint(&obj[name], &item_path.field(name), u64::from(u16::MAX));
        let via = match obj.get("via") {
            Some(v) => Some(uint(v, &item_path.field("via"), u64::from(u16::MAX))? as u16),
            None => None,
        };
        out.resize(slot, None);
        out.push(Some(ModSlot {
            source: id("source")? as u16,
            target: id("target")? as u16,
            via,
        }));
    }
    check_slots(&out, tables).map_err(|m| path.error(m))?;
    Ok(out)
}

/// Writes a non-empty slot list as the value of `"mods"` (the field name is written by the
/// caller), one object per used slot.
pub(crate) fn write_slots(w: &mut String, mods: &[Option<ModSlot>], indent: &str) {
    use std::fmt::Write as _;
    w.push('[');
    let mut first = true;
    for (n, slot) in mods.iter().enumerate() {
        let Some(s) = slot else { continue };
        let sep = if first { "" } else { "," };
        first = false;
        let _ = write!(
            w,
            "{sep}\n{indent}  {{ \"slot\": {n}, \"source\": {}, \"target\": {}",
            s.source, s.target
        );
        if let Some(via) = s.via {
            let _ = write!(w, ", \"via\": {via}");
        }
        w.push_str(" }");
    }
    let _ = write!(w, "\n{indent}]");
}

impl RackDevice {
    /// Slot `slot` of the device's modulation matrix, if it is in use.
    pub fn mod_slot(&self, slot: usize) -> Option<ModSlot> {
        self.mods.get(slot).copied().flatten()
    }

    /// The amount of slot `slot` (its `mod<n>_amount` param), 0 when unset.
    pub fn mod_amount(&self, slot: usize) -> f32 {
        self.params.get(&amount_param(slot)).copied().unwrap_or(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_core::ParamId;
    use atomik_device_sdk::{ModSource, ModTarget};

    const TABLES: ModTables = ModTables {
        sources: &[
            ModSource {
                id: 0,
                key: "a",
                name: "A",
                bipolar: false,
            },
            ModSource {
                id: 1,
                key: "b",
                name: "B",
                bipolar: true,
            },
        ],
        targets: &[ModTarget {
            id: 0,
            key: "t",
            name: "T",
            param: ParamId(0),
        }],
        first_param: 1,
    };

    #[test]
    fn slots_are_checked_against_the_tables() {
        let ok = vec![Some(ModSlot::new(0, 0).via(1))];
        assert_eq!(check_slots(&ok, Some(&TABLES)), Ok(()));
        assert_eq!(check_slots(&[], None), Ok(()));
        assert!(check_slots(&ok, None).is_err());
        assert!(check_slots(&[Some(ModSlot::new(2, 0))], Some(&TABLES)).is_err());
        assert!(check_slots(&[Some(ModSlot::new(0, 1))], Some(&TABLES)).is_err());
        assert!(check_slots(&[Some(ModSlot::new(0, 0).via(2))], Some(&TABLES)).is_err());
        assert!(check_slots(&[Some(ModSlot::new(0, 0)), None], Some(&TABLES)).is_err());
        let many = vec![Some(ModSlot::new(0, 0)); MOD_SLOTS + 1];
        assert!(check_slots(&many, Some(&TABLES)).is_err());
    }

    #[test]
    fn a_slot_list_becomes_engine_params() {
        let mods = vec![None, Some(ModSlot::new(1, 0).via(0))];
        let used = engine_params(&mods, false);
        assert_eq!(
            used,
            vec![
                ("internal.mod2_source".to_owned(), 2.0),
                ("internal.mod2_target".to_owned(), 1.0),
                ("internal.mod2_via".to_owned(), 1.0),
            ]
        );
        let all = engine_params(&mods, true);
        assert_eq!(all.len(), MOD_SLOTS * 3);
        assert_eq!(all[0], ("internal.mod1_source".to_owned(), 0.0));
        assert_eq!(all[3], ("internal.mod2_source".to_owned(), 2.0));
        assert!(engine_params(&[], false).is_empty());
    }

    #[test]
    fn the_canonical_list_has_no_trailing_empty_slot() {
        let a = Some(ModSlot::new(0, 0));
        assert_eq!(with_slot(&[], 2, a), vec![None, None, a]);
        assert_eq!(with_slot(&[None, None, a], 2, None), Vec::new());
        assert_eq!(with_slot(&[a, a], 1, None), vec![a]);
    }
}
