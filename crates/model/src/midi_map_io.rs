//! Reading and writing MIDI mappings: the `"midi_maps"` list of a project (format 28) and the
//! preset file.
//!
//! ```text
//! "midi_maps": [
//!   { "id": 1,
//!     "source": { "kind": "cc" | "cc14" | "pitch_bend" | "note",
//!                 "number": 0..=127 (cc, note) | 0..=31 (cc14; not on pitch_bend),
//!                 "channel": 0..=15 (omitted: any channel),
//!                 "port": "name" (omitted: any port) },
//!     "target": { "kind": "param", "device": "synth1", "param": "filter.cutoff" }
//!              | { "kind": "vca", "id": 2 } | { "kind": "tempo" }
//!              | { "kind": "transport", "action": "play" | "stop" | "toggle_play" | "record" | "loop" | "return_to_zero" }
//!              | { "kind": "selected", "param": "fader" },
//!     "min": 0..=1, "max": 0..=1, "invert": true,
//!     "mode": "relative_twos" | "relative_offset" | "relative_sign" | "toggle" | "momentary",
//!     "step": (0, 1], "pickup": false, "consume": false } …
//! ]
//! ```
//!
//! Only `id`, `source` and `target` are required; an omitted option is its default (range 0 to 1,
//! not inverted, absolute, step 1/64, soft takeover on, message consumed). Ids are from 1 and
//! unique, at most [`MAX_MIDI_MAPS`] mappings. The loader checks the shape and the numbers, not
//! the references: a mapping may name a device that was deleted (it is listed as missing and does
//! nothing). Mappings are listed in id order.
//!
//! A preset file is `{"format": "atomik-midi-preset", "version": 1, "name": "...", "maps": [...]}`
//! with the same mapping objects without `id`, and the target addressing by role: `{"kind":
//! "device", "type": "atomik.channel", "nth": 2, "param": "fader"}` is the fader of the third
//! channel strip of any project (the sources never carry a port).

use crate::midi_map::{
    DEFAULT_SENSITIVITY, MAX_MIDI_MAPS, MAX_PORT_NAME_CHARS, MapMode, MapOptions, MidiMapping,
    MidiPreset, MidiSource, MidiTarget, PresetMap, PresetTarget, SourceKind, TransportTarget,
};
use crate::patch::{JsonPath, array, f32_text, json_str, number, object, string, uint};
use crate::{DeviceKey, ModelError};
use serde_json::{Map, Value};
use std::collections::BTreeSet;
use std::fmt::Write as _;

/// The `format` tag of a preset file.
pub const MIDI_PRESET_FORMAT: &str = "atomik-midi-preset";
/// The version a preset file is written with.
const MIDI_PRESET_VERSION: u64 = 1;

const MAP_FIELDS: &[&str] = &[
    "id", "source", "target", "min", "max", "invert", "mode", "step", "pickup", "consume",
];
const MAP_REQUIRED: &[&str] = &["id", "source", "target"];
const PRESET_MAP_FIELDS: &[&str] = &[
    "source", "target", "min", "max", "invert", "mode", "step", "pickup", "consume",
];
const PRESET_MAP_REQUIRED: &[&str] = &["source", "target"];
const SOURCE_FIELDS: &[&str] = &["kind", "number", "channel", "port"];
const PRESET_FIELDS: &[&str] = &["format", "version", "name", "maps"];

// ---------------------------------------------------------------------------- writing

fn source_json(s: &MidiSource) -> String {
    let mut out = String::from("{");
    let (kind, number) = match s.kind {
        SourceKind::Cc(n) => ("cc", Some(n)),
        SourceKind::Cc14(n) => ("cc14", Some(n)),
        SourceKind::PitchBend => ("pitch_bend", None),
        SourceKind::Note(k) => ("note", Some(k)),
    };
    let _ = write!(out, "\"kind\": \"{kind}\"");
    if let Some(n) = number {
        let _ = write!(out, ", \"number\": {n}");
    }
    if let Some(c) = s.channel {
        let _ = write!(out, ", \"channel\": {c}");
    }
    if let Some(p) = &s.port {
        let _ = write!(out, ", \"port\": {}", json_str(p));
    }
    out.push('}');
    out
}

fn target_json(t: &MidiTarget) -> String {
    match t {
        MidiTarget::Param { device, param } => format!(
            "{{\"kind\": \"param\", \"device\": {}, \"param\": {}}}",
            json_str(&device.0),
            json_str(param)
        ),
        MidiTarget::Vca { id } => format!("{{\"kind\": \"vca\", \"id\": {id}}}"),
        MidiTarget::Tempo => "{\"kind\": \"tempo\"}".to_owned(),
        MidiTarget::Transport(a) => {
            format!("{{\"kind\": \"transport\", \"action\": \"{}\"}}", a.id())
        }
        MidiTarget::SelectedStrip { param } => {
            format!("{{\"kind\": \"selected\", \"param\": {}}}", json_str(param))
        }
    }
}

fn preset_target_json(t: &PresetTarget) -> String {
    match t {
        PresetTarget::Device {
            type_id,
            nth,
            param,
        } => format!(
            "{{\"kind\": \"device\", \"type\": {}, \"nth\": {nth}, \"param\": {}}}",
            json_str(type_id),
            json_str(param)
        ),
        PresetTarget::Vca { id } => target_json(&MidiTarget::Vca { id: *id }),
        PresetTarget::Tempo => target_json(&MidiTarget::Tempo),
        PresetTarget::Transport(a) => target_json(&MidiTarget::Transport(*a)),
        PresetTarget::SelectedStrip { param } => target_json(&MidiTarget::SelectedStrip {
            param: param.clone(),
        }),
    }
}

/// The options that differ from the defaults, as `, "name": value` pairs.
fn options_json(o: &MapOptions) -> String {
    let d = MapOptions::default();
    let mut out = String::new();
    if o.min.to_bits() != d.min.to_bits() {
        let _ = write!(out, ", \"min\": {}", f32_text(o.min));
    }
    if o.max.to_bits() != d.max.to_bits() {
        let _ = write!(out, ", \"max\": {}", f32_text(o.max));
    }
    if o.invert {
        out.push_str(", \"invert\": true");
    }
    if o.mode != MapMode::Absolute {
        let _ = write!(out, ", \"mode\": \"{}\"", o.mode.id());
    }
    if o.sensitivity.to_bits() != DEFAULT_SENSITIVITY.to_bits() {
        let _ = write!(out, ", \"step\": {}", f32_text(o.sensitivity));
    }
    if !o.pickup {
        out.push_str(", \"pickup\": false");
    }
    if !o.consume {
        out.push_str(", \"consume\": false");
    }
    out
}

fn mapping_json(m: &MidiMapping) -> String {
    format!(
        "{{\"id\": {}, \"source\": {}, \"target\": {}{}}}",
        m.id,
        source_json(&m.source),
        target_json(&m.target),
        options_json(&m.options)
    )
}

/// Writes `"midi_maps": [...]` (with its trailing comma and newline) when there are mappings, in
/// id order, one mapping per line.
pub(crate) fn write_midi_maps(w: &mut String, maps: &[MidiMapping]) {
    if maps.is_empty() {
        return;
    }
    let mut sorted: Vec<&MidiMapping> = maps.iter().collect();
    sorted.sort_by_key(|m| m.id);
    let _ = write!(w, "  \"midi_maps\": [");
    for (i, m) in sorted.iter().enumerate() {
        let sep = if i == 0 { "" } else { "," };
        let _ = write!(w, "{sep}\n    {}", mapping_json(m));
    }
    let _ = writeln!(w, "\n  ],");
}

/// A preset file's text (stable: the mappings in preset order, one per line).
pub fn save_midi_preset(preset: &MidiPreset) -> String {
    let mut out = String::new();
    let w = &mut out;
    let _ = writeln!(w, "{{");
    let _ = writeln!(w, "  \"format\": {},", json_str(MIDI_PRESET_FORMAT));
    let _ = writeln!(w, "  \"version\": {MIDI_PRESET_VERSION},");
    let _ = writeln!(w, "  \"name\": {},", json_str(&preset.name));
    let _ = write!(w, "  \"maps\": [");
    for (i, m) in preset.maps.iter().enumerate() {
        let sep = if i == 0 { "" } else { "," };
        let _ = write!(
            w,
            "{sep}\n    {{\"source\": {}, \"target\": {}{}}}",
            source_json(&m.source),
            preset_target_json(&m.target),
            options_json(&m.options)
        );
    }
    let _ = writeln!(w, "{}]", if preset.maps.is_empty() { "" } else { "\n  " });
    let _ = writeln!(w, "}}");
    out
}

// ---------------------------------------------------------------------------- reading

fn source_from(v: &Value, path: &JsonPath, allow_port: bool) -> Result<MidiSource, ModelError> {
    let o = object(v, path, SOURCE_FIELDS, &["kind"])?;
    let kind_path = path.field("kind");
    let kind = string(&o["kind"], &kind_path)?;
    let number_at = |max: u64| -> Result<u8, ModelError> {
        let p = path.field("number");
        let n = o
            .get("number")
            .ok_or_else(|| p.error(format!("a {kind} source needs a number")))?;
        Ok(uint(n, &p, max)? as u8)
    };
    let kind = match kind {
        "cc" => SourceKind::Cc(number_at(127)?),
        "cc14" => SourceKind::Cc14(number_at(31)?),
        "note" => SourceKind::Note(number_at(127)?),
        "pitch_bend" => {
            if o.contains_key("number") {
                return Err(path
                    .field("number")
                    .error("a pitch bend source has no number"));
            }
            SourceKind::PitchBend
        }
        other => {
            return Err(kind_path.error(format!(
                "unknown source kind {other:?}; expected cc, cc14, pitch_bend or note"
            )));
        }
    };
    let channel = match o.get("channel") {
        Some(c) => Some(uint(c, &path.field("channel"), 15)? as u8),
        None => None,
    };
    let port = match o.get("port") {
        Some(p) => {
            let pp = path.field("port");
            if !allow_port {
                return Err(pp.error("a preset has no input port"));
            }
            let name = string(p, &pp)?;
            if name.is_empty() || name.chars().count() > MAX_PORT_NAME_CHARS {
                return Err(pp.error(format!(
                    "a port name has 1 to {MAX_PORT_NAME_CHARS} characters"
                )));
            }
            Some(name.to_owned())
        }
        None => None,
    };
    Ok(MidiSource {
        port,
        channel,
        kind,
    })
}

/// A target of either flavour, as the loader of one of them asks.
enum AnyTarget {
    Concrete(MidiTarget),
    Role(PresetTarget),
}

fn target_from(v: &Value, path: &JsonPath, preset: bool) -> Result<AnyTarget, ModelError> {
    let kind_path = path.field("kind");
    let kind = string(
        v.get("kind")
            .ok_or_else(|| kind_path.error("a target needs a kind"))?,
        &kind_path,
    )?;
    let param_of = |o: &Map<String, Value>| -> Result<String, ModelError> {
        let p = path.field("param");
        let name = string(
            o.get("param")
                .ok_or_else(|| p.error("a target needs a param"))?,
            &p,
        )?;
        if name.is_empty() {
            return Err(p.error("the param key is empty"));
        }
        Ok(name.to_owned())
    };
    Ok(match kind {
        "param" if !preset => {
            let o = object(
                v,
                path,
                &["kind", "device", "param"],
                &["kind", "device", "param"],
            )?;
            let device = string(&o["device"], &path.field("device"))?;
            AnyTarget::Concrete(MidiTarget::Param {
                device: DeviceKey(device.to_owned()),
                param: param_of(o)?,
            })
        }
        "device" if preset => {
            let o = object(
                v,
                path,
                &["kind", "type", "nth", "param"],
                &["kind", "type", "nth", "param"],
            )?;
            AnyTarget::Role(PresetTarget::Device {
                type_id: string(&o["type"], &path.field("type"))?.to_owned(),
                nth: uint(&o["nth"], &path.field("nth"), 4096)? as usize,
                param: param_of(o)?,
            })
        }
        "vca" => {
            let o = object(v, path, &["kind", "id"], &["kind", "id"])?;
            let id_path = path.field("id");
            let id = uint(&o["id"], &id_path, u64::from(u32::MAX))? as u32;
            if id == 0 {
                return Err(id_path.error("VCA ids count from 1"));
            }
            if preset {
                AnyTarget::Role(PresetTarget::Vca { id })
            } else {
                AnyTarget::Concrete(MidiTarget::Vca { id })
            }
        }
        "tempo" => {
            object(v, path, &["kind"], &["kind"])?;
            if preset {
                AnyTarget::Role(PresetTarget::Tempo)
            } else {
                AnyTarget::Concrete(MidiTarget::Tempo)
            }
        }
        "transport" => {
            let o = object(v, path, &["kind", "action"], &["kind", "action"])?;
            let ap = path.field("action");
            let id = string(&o["action"], &ap)?;
            let action = TransportTarget::from_id(id)
                .ok_or_else(|| ap.error(format!("unknown transport action {id:?}")))?;
            if preset {
                AnyTarget::Role(PresetTarget::Transport(action))
            } else {
                AnyTarget::Concrete(MidiTarget::Transport(action))
            }
        }
        "selected" => {
            let o = object(v, path, &["kind", "param"], &["kind", "param"])?;
            let param = param_of(o)?;
            if preset {
                AnyTarget::Role(PresetTarget::SelectedStrip { param })
            } else {
                AnyTarget::Concrete(MidiTarget::SelectedStrip { param })
            }
        }
        other => {
            let expected = if preset {
                "device, vca, tempo, transport or selected"
            } else {
                "param, vca, tempo, transport or selected"
            };
            return Err(kind_path.error(format!(
                "unknown target kind {other:?}; expected {expected}"
            )));
        }
    })
}

fn options_from(
    o: &Map<String, Value>,
    path: &JsonPath,
    source: &MidiSource,
) -> Result<MapOptions, ModelError> {
    let mut opts = MapOptions::default();
    let unit = |name: &str, default: f32| -> Result<f32, ModelError> {
        match o.get(name) {
            None => Ok(default),
            Some(v) => {
                let p = path.field(name);
                let x = number(v, &p)? as f32;
                if !(x.is_finite() && (0.0..=1.0).contains(&x)) {
                    return Err(p.error(format!("{x} is outside 0..=1")));
                }
                Ok(x)
            }
        }
    };
    opts.min = unit("min", opts.min)?;
    opts.max = unit("max", opts.max)?;
    if let Some(v) = o.get("invert") {
        opts.invert = flag(v, &path.field("invert"))?;
    }
    if let Some(v) = o.get("mode") {
        let p = path.field("mode");
        let id = string(v, &p)?;
        opts.mode = MapMode::from_id(id).ok_or_else(|| p.error(format!("unknown mode {id:?}")))?;
    }
    if let Some(v) = o.get("step") {
        let p = path.field("step");
        let x = number(v, &p)? as f32;
        if !(x.is_finite() && x > 0.0 && x <= 1.0) {
            return Err(p.error(format!("{x} is outside (0, 1]")));
        }
        opts.sensitivity = x;
    }
    if let Some(v) = o.get("pickup") {
        opts.pickup = flag(v, &path.field("pickup"))?;
    }
    if let Some(v) = o.get("consume") {
        opts.consume = flag(v, &path.field("consume"))?;
    }
    if let Some(problem) = opts.problem(source) {
        return Err(path.field("mode").error(problem));
    }
    Ok(opts)
}

fn flag(v: &Value, path: &JsonPath) -> Result<bool, ModelError> {
    v.as_bool()
        .ok_or_else(|| path.error(format!("expected a boolean, found {v}")))
}

/// Reads the project's `"midi_maps"`: ids from 1 and unique, valid sources and options (the
/// references are not checked), at most [`MAX_MIDI_MAPS`].
pub(crate) fn load_midi_maps(v: &Value, path: &JsonPath) -> Result<Vec<MidiMapping>, ModelError> {
    let items = array(v, path)?;
    if items.len() > MAX_MIDI_MAPS {
        return Err(path.error(format!(
            "at most {MAX_MIDI_MAPS} mappings, found {}",
            items.len()
        )));
    }
    let mut seen = BTreeSet::new();
    let mut maps = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let p = path.index(i);
        let o = object(item, &p, MAP_FIELDS, MAP_REQUIRED)?;
        let id_path = p.field("id");
        let id = uint(&o["id"], &id_path, u64::from(u32::MAX))? as u32;
        if id == 0 {
            return Err(id_path.error("mapping ids count from 1"));
        }
        if !seen.insert(id) {
            return Err(id_path.error(format!("the mapping id {id} is used twice")));
        }
        let source = source_from(&o["source"], &p.field("source"), true)?;
        let AnyTarget::Concrete(target) = target_from(&o["target"], &p.field("target"), false)?
        else {
            unreachable!("a project target is concrete");
        };
        let options = options_from(o, &p, &source)?;
        maps.push(MidiMapping {
            id,
            source,
            target,
            options,
        });
    }
    Ok(maps)
}

/// Reads a preset file.
///
/// # Errors
/// [`ModelError::Load`] naming the JSON path of what is wrong (not a preset, a newer version,
/// a bad source, target or option).
pub fn load_midi_preset(json: &str) -> Result<MidiPreset, ModelError> {
    let root = JsonPath::default();
    let value: Value =
        serde_json::from_str(json).map_err(|e| root.error(format!("invalid JSON: {e}")))?;
    let o = object(&value, &root, PRESET_FIELDS, &["format", "version", "maps"])?;
    let format = string(&o["format"], &root.field("format"))?;
    if format != MIDI_PRESET_FORMAT {
        return Err(root.field("format").error(format!(
            "expected \"{MIDI_PRESET_FORMAT}\", found {format:?}"
        )));
    }
    let vp = root.field("version");
    let version = uint(&o["version"], &vp, u64::MAX)?;
    if version != MIDI_PRESET_VERSION {
        return Err(vp.error(format!(
            "unsupported version {version}; this reader reads version {MIDI_PRESET_VERSION}"
        )));
    }
    let name = match o.get("name") {
        Some(n) => string(n, &root.field("name"))?.to_owned(),
        None => String::new(),
    };
    let maps_path = root.field("maps");
    let items = array(&o["maps"], &maps_path)?;
    if items.len() > MAX_MIDI_MAPS {
        return Err(maps_path.error(format!("at most {MAX_MIDI_MAPS} mappings")));
    }
    let mut maps = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let p = maps_path.index(i);
        let m = object(item, &p, PRESET_MAP_FIELDS, PRESET_MAP_REQUIRED)?;
        let source = source_from(&m["source"], &p.field("source"), false)?;
        let AnyTarget::Role(target) = target_from(&m["target"], &p.field("target"), true)? else {
            unreachable!("a preset target is a role");
        };
        let options = options_from(m, &p, &source)?;
        maps.push(PresetMap {
            source,
            target,
            options,
        });
    }
    Ok(MidiPreset { name, maps })
}
