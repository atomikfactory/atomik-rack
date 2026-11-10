//! Reading the song's optional harmony fields: `"key"`, `"key_changes"` and `"chords"` (see
//! [`crate::harmony`]).

use super::*;
use crate::harmony::{ChordEvent, Harmony, Key, KeyChange, MAX_CHORDS, MAX_KEY_CHANGES};
use crate::theory::{ChordKind, ScaleKind};

/// Reads a pitch class `0..12`.
fn pitch_class(v: &Value, path: &JsonPath) -> Result<u8, ModelError> {
    Ok(uint(v, path, 11)? as u8)
}

/// Reads the `"root"` and `"scale"` of a key object.
fn load_key_fields(o: &serde_json::Map<String, Value>, path: &JsonPath) -> Result<Key, ModelError> {
    let root = pitch_class(&o["root"], &path.field("root"))?;
    let sp = path.field("scale");
    let name = string(&o["scale"], &sp)?;
    match ScaleKind::from_id(name) {
        Some(ScaleKind::Chromatic) => {
            Err(sp.error("a song key needs a scale that is not chromatic"))
        }
        Some(scale) => Ok(Key::new(root, scale)),
        None => Err(sp.error(format!("unknown scale {name:?}"))),
    }
}

/// Reads the harmony of a song from its three optional fields.
pub(super) fn load_harmony(
    obj: &serde_json::Map<String, Value>,
    path: &JsonPath,
    version: u64,
) -> Result<Harmony, ModelError> {
    let mut harmony = Harmony::default();
    if let Some(k) = obj.get("key") {
        let kp = path.field("key");
        let o = schema::object(k, &kp, Obj::Key, version)?;
        harmony.key = Some(load_key_fields(o, &kp)?);
    }
    if let Some(list) = obj.get("key_changes") {
        let lp = path.field("key_changes");
        let items = array(list, &lp)?;
        if items.len() > MAX_KEY_CHANGES {
            return Err(lp.error(format!("at most {MAX_KEY_CHANGES} key changes")));
        }
        for (i, item) in items.iter().enumerate() {
            let p = lp.index(i);
            let o = schema::object(item, &p, Obj::KeyChange, version)?;
            harmony.changes.push(KeyChange {
                tick: tick(&o["tick"], &p.field("tick"))?,
                key: load_key_fields(o, &p)?,
            });
        }
    }
    if let Some(list) = obj.get("chords") {
        let lp = path.field("chords");
        let items = array(list, &lp)?;
        if items.len() > MAX_CHORDS {
            return Err(lp.error(format!("at most {MAX_CHORDS} chords")));
        }
        for (i, item) in items.iter().enumerate() {
            let p = lp.index(i);
            let o = schema::object(item, &p, Obj::Chord, version)?;
            let qp = p.field("quality");
            let quality = string(&o["quality"], &qp)?;
            let quality = ChordKind::from_id(quality)
                .ok_or_else(|| qp.error(format!("unknown chord quality {quality:?}")))?;
            let bass = match o.get("bass") {
                Some(b) => Some(pitch_class(b, &p.field("bass"))?),
                None => None,
            };
            harmony.chords.push(ChordEvent {
                tick: tick(&o["tick"], &p.field("tick"))?,
                length: tick(&o["length"], &p.field("length"))?,
                root: pitch_class(&o["root"], &p.field("root"))?,
                quality,
                bass,
            });
        }
    }
    harmony.check().map_err(|m| path.error(m))?;
    Ok(harmony)
}
