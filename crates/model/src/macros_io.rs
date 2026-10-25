//! Reading and writing the project's macros: the `"macros"` object (format 29).
//!
//! ```text
//! "macros": {
//!   "assigns": [ { "id": 1, "bank": "macro-bank", "macro": 1..=8,
//!                  "device": "synth1", "param": "filter.cutoff",
//!                  "min": 20.0, "max": 18000.0, "invert": true } … ],
//!   "names":   [ { "bank": "macro-bank", "macro": 1..=8, "name": "Brightness" } … ]
//! }
//! ```
//!
//! `"assigns"` are in id order, ids from 1 and unique, at most [`MAX_MACRO_ASSIGNS`]; `"invert"`
//! is omitted when false; `"min"` and `"max"` are finite plain values. A param is assigned at
//! most once (the loader names the path of a second assignment). `"names"` are trimmed, 1 to 24
//! characters, one per bank and macro, in bank and macro order. Either list is omitted when
//! empty, and the object when both are. The loader checks the shape and the numbers, not the
//! references: an assignment may name a bank or a device that was deleted (it does nothing).

use crate::container::{MACRO_COUNT, MAX_MACRO_NAME_CHARS};
use crate::macros::{MAX_MACRO_ASSIGNS, MacroAssign, MacroName, MacroSet};
use crate::patch::{JsonPath, array, f32_text, json_str, number, object, string, uint};
use crate::{DeviceKey, ModelError};
use serde_json::Value;
use std::collections::BTreeSet;
use std::fmt::Write as _;

const MACROS_FIELDS: &[&str] = &["assigns", "names"];
const ASSIGN_FIELDS: &[&str] = &[
    "id", "bank", "macro", "device", "param", "min", "max", "invert",
];
const ASSIGN_REQUIRED: &[&str] = &["id", "bank", "macro", "device", "param", "min", "max"];
const NAME_FIELDS: &[&str] = &["bank", "macro", "name"];

/// Writes `"macros"` (with its trailing comma and newline) after `"midi_maps"`; nothing for an
/// empty set.
pub(crate) fn write_macros(w: &mut String, set: &MacroSet) {
    if set.is_empty() {
        return;
    }
    let _ = write!(w, "  \"macros\": {{");
    let mut parts = Vec::new();
    if !set.assigns.is_empty() {
        let mut sorted: Vec<&MacroAssign> = set.assigns.iter().collect();
        sorted.sort_by_key(|a| a.id);
        let rows: Vec<String> = sorted
            .iter()
            .map(|a| {
                let invert = if a.invert { ", \"invert\": true" } else { "" };
                format!(
                    "      {{\"id\": {}, \"bank\": {}, \"macro\": {}, \"device\": {}, \"param\": {}, \"min\": {}, \"max\": {}{invert}}}",
                    a.id,
                    json_str(&a.bank.0),
                    u32::from(a.index) + 1,
                    json_str(&a.device.0),
                    json_str(&a.param),
                    f32_text(a.min),
                    f32_text(a.max),
                )
            })
            .collect();
        parts.push(format!("    \"assigns\": [\n{}\n    ]", rows.join(",\n")));
    }
    if !set.names.is_empty() {
        let rows: Vec<String> = set
            .names
            .iter()
            .map(|n| {
                format!(
                    "      {{\"bank\": {}, \"macro\": {}, \"name\": {}}}",
                    json_str(&n.bank.0),
                    u32::from(n.index) + 1,
                    json_str(&n.name),
                )
            })
            .collect();
        parts.push(format!("    \"names\": [\n{}\n    ]", rows.join(",\n")));
    }
    let _ = writeln!(w, "\n{}\n  }},", parts.join(",\n"));
}

fn macro_number(v: &Value, path: &JsonPath) -> Result<u8, ModelError> {
    let n = uint(v, path, MACRO_COUNT as u64)?;
    if n == 0 {
        return Err(path.error(format!("macros are 1 to {MACRO_COUNT}")));
    }
    Ok((n - 1) as u8)
}

fn finite(v: &Value, path: &JsonPath) -> Result<f32, ModelError> {
    let x = number(v, path)? as f32;
    if !x.is_finite() {
        return Err(path.error("the value is not finite"));
    }
    Ok(x)
}

/// Reads the project's `"macros"` object (see the module documentation).
pub(crate) fn load_macros(v: &Value, path: &JsonPath) -> Result<MacroSet, ModelError> {
    let o = object(v, path, MACROS_FIELDS, &[])?;
    let mut set = MacroSet::default();
    if let Some(list) = o.get("assigns") {
        let lp = path.field("assigns");
        let items = array(list, &lp)?;
        if items.len() > MAX_MACRO_ASSIGNS {
            return Err(lp.error(format!(
                "at most {MAX_MACRO_ASSIGNS} assignments, found {}",
                items.len()
            )));
        }
        let mut ids = BTreeSet::new();
        let mut driven = BTreeSet::new();
        for (i, item) in items.iter().enumerate() {
            let p = lp.index(i);
            let a = object(item, &p, ASSIGN_FIELDS, ASSIGN_REQUIRED)?;
            let id_path = p.field("id");
            let id = uint(&a["id"], &id_path, u64::from(u32::MAX))? as u32;
            if id == 0 {
                return Err(id_path.error("assignment ids count from 1"));
            }
            if !ids.insert(id) {
                return Err(id_path.error(format!("the assignment id {id} is used twice")));
            }
            let device = string(&a["device"], &p.field("device"))?.to_owned();
            let param = string(&a["param"], &p.field("param"))?.to_owned();
            if !driven.insert((device.clone(), param.clone())) {
                return Err(p.field("param").error(format!(
                    "{device} {param} is assigned twice: a param follows one macro"
                )));
            }
            let invert = match a.get("invert") {
                Some(b) => b
                    .as_bool()
                    .ok_or_else(|| p.field("invert").error("expected true or false"))?,
                None => false,
            };
            set.assigns.push(MacroAssign {
                id,
                bank: DeviceKey(string(&a["bank"], &p.field("bank"))?.to_owned()),
                index: macro_number(&a["macro"], &p.field("macro"))?,
                device: DeviceKey(device),
                param,
                min: finite(&a["min"], &p.field("min"))?,
                max: finite(&a["max"], &p.field("max"))?,
                invert,
            });
        }
        set.assigns.sort_by_key(|a| a.id);
    }
    if let Some(list) = o.get("names") {
        let lp = path.field("names");
        let mut seen = BTreeSet::new();
        for (i, item) in array(list, &lp)?.iter().enumerate() {
            let p = lp.index(i);
            let n = object(item, &p, NAME_FIELDS, NAME_FIELDS)?;
            let bank = DeviceKey(string(&n["bank"], &p.field("bank"))?.to_owned());
            let index = macro_number(&n["macro"], &p.field("macro"))?;
            let name_path = p.field("name");
            let name = string(&n["name"], &name_path)?;
            if name != name.trim() || name.is_empty() {
                return Err(name_path.error("a macro name is trimmed and not empty"));
            }
            if name.chars().count() > MAX_MACRO_NAME_CHARS {
                return Err(name_path.error(format!(
                    "a macro name has at most {MAX_MACRO_NAME_CHARS} characters"
                )));
            }
            if !seen.insert((bank.clone(), index)) {
                return Err(p.error("the macro is named twice"));
            }
            set.names.push(MacroName {
                bank,
                index,
                name: name.to_owned(),
            });
        }
        set.names
            .sort_by(|a, b| (&a.bank, a.index).cmp(&(&b.bank, b.index)));
    }
    Ok(set)
}
