//! Two pieces of view state a project keeps so that the screen looks the same when it is opened
//! again: which insert areas of the rack are folded, and what each Spectrum device draws as its
//! second curve.
//!
//! Neither has a part in the sound. Both are root fields written only when set (project format
//! 45, ignorable: a reader that does not know them plays and bounces the project as it did) and
//! edited with one undo step each ([`crate::Command::SetInsertsFolded`],
//! [`crate::Command::SetSpectrumCurves`]) that reports [`crate::SyncOp::ViewChanged`] and never
//! touches the engine.
//!
//! * **Folded inserts** (`"inserts_folded"`): the keys of the console strips whose insert area in
//!   the rack is folded to its header. Kept sorted and without repeats.
//! * **Spectrum curves** (`"spectrum_curves"`): per Spectrum device, the source of its second
//!   curve: the reference track of the song or another console strip. A device without a row
//!   draws none.
//!
//! A key whose device is gone is dropped when the project is written, so the lists do not grow
//! with every deleted strip. The loader and the commands enforce the same limits.

use crate::history::{Edit, Swap};
use crate::patch::{JsonPath, array, json_str, object, string};
use crate::{DeviceKey, ModelError, RackDoc};
use serde_json::Value;
use std::fmt::Write as _;

/// Most entries of either list.
pub const MAX_VIEW_ENTRIES: usize = 512;

const ROW_REQUIRED: &[&str] = &["device", "from"];
const ROW_FIELDS: &[&str] = &["device", "from", "strip"];

/// Where the second curve of a Spectrum device comes from.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum CurveFrom {
    /// The reference track of the song (its long-term spectrum, at the level it is heard at).
    Reference,
    /// A console strip, by the key of its device.
    Strip(DeviceKey),
}

/// The second curve of one Spectrum device.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SpectrumCurve {
    /// The Spectrum device.
    pub device: DeviceKey,
    /// What its second curve shows.
    pub from: CurveFrom,
}

fn check_len(len: usize, what: &str) -> Result<(), ModelError> {
    if len > MAX_VIEW_ENTRIES {
        return Err(ModelError::OutOfRange(format!(
            "{len} {what}, at most {MAX_VIEW_ENTRIES} are allowed"
        )));
    }
    Ok(())
}

/// Plans `Command::SetInsertsFolded`: the list is sorted and without repeats, within the limit
/// and different from the current one.
pub(crate) fn plan_folds(doc: &RackDoc, keys: &[DeviceKey]) -> Result<Edit, ModelError> {
    check_len(keys.len(), "folded insert areas")?;
    let mut new = keys.to_vec();
    new.sort();
    new.dedup();
    if doc.folded_inserts == new {
        return Err(ModelError::BadScope(
            "the insert areas are already folded that way".into(),
        ));
    }
    Ok(Edit::Swap(Swap::InsertsFolded {
        old: doc.folded_inserts.clone(),
        new,
    }))
}

/// Plans `Command::SetSpectrumCurves`: one row per device, within the limit and different from
/// the current list.
pub(crate) fn plan_curves(doc: &RackDoc, rows: &[SpectrumCurve]) -> Result<Edit, ModelError> {
    check_len(rows.len(), "spectrum curves")?;
    let mut new = rows.to_vec();
    new.sort();
    if new.windows(2).any(|w| w[0].device == w[1].device) {
        return Err(ModelError::BadScope(
            "a Spectrum device has one second curve".into(),
        ));
    }
    if doc.spectrum_curves == new {
        return Err(ModelError::BadScope(
            "the spectrum curves are already that".into(),
        ));
    }
    Ok(Edit::Swap(Swap::SpectrumCurves {
        old: doc.spectrum_curves.clone(),
        new,
    }))
}

/// Writes `"inserts_folded"` (with its trailing comma and newline); nothing for an empty list.
/// Keys of devices the project no longer has are left out.
pub(crate) fn write_folds(w: &mut String, doc: &RackDoc) {
    let keys: Vec<String> = doc
        .folded_inserts
        .iter()
        .filter(|k| doc.device(k).is_some())
        .map(|k| json_str(&k.0))
        .collect();
    if keys.is_empty() {
        return;
    }
    let _ = writeln!(w, "  \"inserts_folded\": [{}],", keys.join(", "));
}

/// Writes `"spectrum_curves"` (with its trailing comma and newline); nothing without a row. A row
/// whose device or strip is gone is left out.
pub(crate) fn write_curves(w: &mut String, doc: &RackDoc) {
    let rows: Vec<String> = doc
        .spectrum_curves
        .iter()
        .filter(|r| doc.device(&r.device).is_some())
        .filter(|r| match &r.from {
            CurveFrom::Reference => true,
            CurveFrom::Strip(k) => doc.device(k).is_some(),
        })
        .map(|r| match &r.from {
            CurveFrom::Reference => format!(
                "    {{\"device\": {}, \"from\": \"reference\"}}",
                json_str(&r.device.0)
            ),
            CurveFrom::Strip(k) => format!(
                "    {{\"device\": {}, \"from\": \"strip\", \"strip\": {}}}",
                json_str(&r.device.0),
                json_str(&k.0)
            ),
        })
        .collect();
    if rows.is_empty() {
        return;
    }
    let _ = writeln!(w, "  \"spectrum_curves\": [\n{}\n  ],", rows.join(",\n"));
}

/// Reads the project's `"inserts_folded"` list.
pub(crate) fn load_folds(v: &Value, path: &JsonPath) -> Result<Vec<DeviceKey>, ModelError> {
    let items = array(v, path)?;
    if items.len() > MAX_VIEW_ENTRIES {
        return Err(path.error(format!(
            "{} folded insert areas, at most {MAX_VIEW_ENTRIES} are allowed",
            items.len()
        )));
    }
    let mut keys = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        keys.push(DeviceKey(string(item, &path.index(i))?.to_owned()));
    }
    keys.sort();
    keys.dedup();
    Ok(keys)
}

/// Reads the project's `"spectrum_curves"` list.
pub(crate) fn load_curves(v: &Value, path: &JsonPath) -> Result<Vec<SpectrumCurve>, ModelError> {
    let items = array(v, path)?;
    if items.len() > MAX_VIEW_ENTRIES {
        return Err(path.error(format!(
            "{} spectrum curves, at most {MAX_VIEW_ENTRIES} are allowed",
            items.len()
        )));
    }
    let mut rows: Vec<SpectrumCurve> = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let at = path.index(i);
        let o = object(item, &at, ROW_FIELDS, ROW_REQUIRED)?;
        let device = DeviceKey(string(&o["device"], &at.field("device"))?.to_owned());
        let from = match string(&o["from"], &at.field("from"))? {
            "reference" => CurveFrom::Reference,
            "strip" => {
                let strip = o
                    .get("strip")
                    .ok_or_else(|| at.error("a strip curve names its strip".to_owned()))?;
                CurveFrom::Strip(DeviceKey(string(strip, &at.field("strip"))?.to_owned()))
            }
            other => {
                return Err(at
                    .field("from")
                    .error(format!("expected reference or strip, found {other:?}")));
            }
        };
        if rows.iter().any(|r| r.device == device) {
            return Err(at
                .field("device")
                .error(format!("{} has two second curves", json_str(&device.0))));
        }
        rows.push(SpectrumCurve { device, from });
    }
    rows.sort();
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(s: &str) -> DeviceKey {
        DeviceKey(s.to_owned())
    }

    #[test]
    fn the_loaders_read_what_the_writers_write() {
        let text = r#"{
            "inserts_folded": ["b", "a", "a"],
            "spectrum_curves": [
                {"device": "z", "from": "reference"},
                {"device": "m", "from": "strip", "strip": "a"}
            ]
        }"#;
        let value: Value = serde_json::from_str(text).unwrap();
        let at = JsonPath::lazy();
        assert_eq!(
            load_folds(&value["inserts_folded"], &at).unwrap(),
            vec![key("a"), key("b")],
            "sorted, without repeats"
        );
        let rows = load_curves(&value["spectrum_curves"], &at).unwrap();
        assert_eq!(rows[0].device, key("m"));
        assert_eq!(rows[0].from, CurveFrom::Strip(key("a")));
        assert_eq!(rows[1].from, CurveFrom::Reference);
    }

    #[test]
    fn the_loader_refuses_nonsense() {
        let at = JsonPath::lazy();
        let bad = |s: &str| load_curves(&serde_json::from_str::<Value>(s).unwrap(), &at).is_err();
        assert!(bad(r#"[{"device":"a","from":"moon"}]"#));
        assert!(bad(r#"[{"device":"a","from":"strip"}]"#));
        assert!(bad(
            r#"[{"device":"a","from":"reference"},{"device":"a","from":"reference"}]"#
        ));
        assert!(bad(r#"[{"from":"reference"}]"#));
        assert!(load_folds(&serde_json::json!([1]), &at).is_err());
    }
}
