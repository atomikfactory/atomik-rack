//! Patch file I/O: reads format v1 and v2, writes v2.
//!
//! v2 is v1 plus three optional per-device layout fields: `name` (string), `folded` (bool)
//! and `color` (`"#rrggbb"` or `null`). Device order is rack order. `notes` are accepted (as
//! an array) and ignored: they are not part of the rack document. Every error names the JSON
//! path of the offending value, as `atomik-render` does.

use crate::container::{BOUNDARY_KEY, FORBIDDEN_INSIDE, MAX_CONTAINER_DEPTH, SPLIT_TYPE};
use crate::container_io;
use crate::plugin_io;
use crate::{Cable, DeviceKey, INTERFACE_KEY, ModelError, RackDevice, RackDoc, SocketRef, rules};
use atomik_device_sdk::DeviceKind;
use atomik_device_sdk::is_plugin_type_id;
use atomik_device_sdk::{Direction, Registry};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

/// The `format` tag of a patch file.
pub const PATCH_FORMAT: &str = "atomik-patch";
/// The version [`crate::save_patch`] writes.
pub const PATCH_VERSION: u32 = 2;
/// Accepted sample rates in Hz.
pub const SAMPLE_RATES: [u32; 3] = [44_100, 48_000, 96_000];
/// Most host output channels a patch may ask for.
pub const MAX_OUTPUT_CHANNELS: u16 = 32;
/// `length_beats` written by [`crate::save_patch`]. The rack document has no song length;
/// the field is written so a saved rack stays a valid render input.
pub const SAVE_LENGTH_BEATS: f64 = 16.0;

/// A JSON path such as `devices[0].params."filter.cutoff"`; the root prints as `$`.
#[derive(Debug, Clone, Default)]
pub(crate) struct JsonPath(String);

impl JsonPath {
    pub(crate) fn field(&self, name: &str) -> Self {
        let plain = name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        let part = if plain {
            name.to_owned()
        } else {
            Value::from(name).to_string()
        };
        if self.0.is_empty() {
            Self(part)
        } else {
            Self(format!("{}.{part}", self.0))
        }
    }

    pub(crate) fn index(&self, index: usize) -> Self {
        Self(format!("{}[{index}]", self.0))
    }

    pub(crate) fn error(&self, message: impl std::fmt::Display) -> ModelError {
        ModelError::Patch(format!("{self}: {message}"))
    }
}

impl std::fmt::Display for JsonPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_empty() { "$" } else { &self.0 })
    }
}

pub(crate) fn describe(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// An object with only `allowed` keys and every `required` key present.
pub(crate) fn object<'v>(
    v: &'v Value,
    path: &JsonPath,
    allowed: &[&str],
    required: &[&str],
) -> Result<&'v Map<String, Value>, ModelError> {
    let obj = v
        .as_object()
        .ok_or_else(|| path.error(format!("expected an object, found {}", describe(v))))?;
    if let Some(key) = obj.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(path.field(key).error(format!(
            "unknown field; expected one of: {}",
            allowed.join(", ")
        )));
    }
    if let Some(missing) = required.iter().find(|k| !obj.contains_key(**k)) {
        return Err(path.field(missing).error("missing field"));
    }
    Ok(obj)
}

pub(crate) fn uint(v: &Value, path: &JsonPath, max: u64) -> Result<u64, ModelError> {
    match v.as_u64() {
        None => Err(path.error(format!("expected an unsigned integer, found {v}"))),
        Some(n) if n > max => Err(path.error(format!("{n} is outside 0..={max}"))),
        Some(n) => Ok(n),
    }
}

pub(crate) fn number(v: &Value, path: &JsonPath) -> Result<f64, ModelError> {
    v.as_f64()
        .ok_or_else(|| path.error(format!("expected a number, found {}", describe(v))))
}

pub(crate) fn string<'v>(v: &'v Value, path: &JsonPath) -> Result<&'v str, ModelError> {
    v.as_str()
        .ok_or_else(|| path.error(format!("expected a string, found {}", describe(v))))
}

pub(crate) fn array<'v>(v: &'v Value, path: &JsonPath) -> Result<&'v [Value], ModelError> {
    v.as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| path.error(format!("expected an array, found {}", describe(v))))
}

/// Parses `"#rrggbb"` (either case).
pub(crate) fn parse_color(v: &Value, path: &JsonPath) -> Result<Option<[u8; 3]>, ModelError> {
    if v.is_null() {
        return Ok(None);
    }
    let bad = || path.error(format!("expected a colour \"#rrggbb\" or null, found {v}"));
    let hex = v
        .as_str()
        .and_then(|s| s.strip_prefix('#'))
        .filter(|h| h.len() == 6 && h.chars().all(|c| c.is_ascii_hexdigit()))
        .ok_or_else(bad)?;
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| bad());
    Ok(Some([byte(0)?, byte(2)?, byte(4)?]))
}

const ROOT_FIELDS: &[&str] = &[
    "format",
    "version",
    "sample_rate",
    "tempo_bpm",
    "seed",
    "length_beats",
    "output_channels",
    "devices",
    "cables",
    "notes",
];
const ROOT_REQUIRED: &[&str] = &[
    "format",
    "version",
    "sample_rate",
    "tempo_bpm",
    "length_beats",
    "output_channels",
    "devices",
];
const DEVICE_FIELDS_V1: &[&str] = &["id", "type", "params"];
pub(crate) const DEVICE_FIELDS_V2: &[&str] = &["id", "type", "params", "name", "folded", "color"];
/// Project format v4 adds `zones`.
pub(crate) const DEVICE_FIELDS_V4: &[&str] =
    &["id", "type", "params", "name", "folded", "color", "zones"];
/// Project format v7 adds `steps`.
pub(crate) const DEVICE_FIELDS_V7: &[&str] = &[
    "id", "type", "params", "name", "folded", "color", "zones", "steps",
];
/// Project format v8 adds `inner` (a container's inside).
pub(crate) const DEVICE_FIELDS_V8: &[&str] = &[
    "id", "type", "params", "name", "folded", "color", "zones", "steps", "inner",
];
/// Project format v9 adds `plugin` (a plugin device's document).
pub(crate) const DEVICE_FIELDS_V9: &[&str] = &[
    "id", "type", "params", "name", "folded", "color", "zones", "steps", "inner", "plugin",
];
/// Project format v11 adds `column` (the rack column of a top-level device; omitted for 0).
pub(crate) const DEVICE_FIELDS_V11: &[&str] = &[
    "id", "type", "params", "name", "folded", "color", "column", "zones", "steps", "inner",
    "plugin",
];
/// Project format v32 adds `curves`.
pub(crate) const DEVICE_FIELDS_V32: &[&str] = &[
    "id", "type", "params", "name", "folded", "color", "column", "zones", "steps", "curves",
    "inner", "plugin",
];
/// Project format v33 adds `drums`.
pub(crate) const DEVICE_FIELDS_V33: &[&str] = &[
    "id", "type", "params", "name", "folded", "color", "column", "zones", "steps", "curves",
    "drums", "inner", "plugin",
];

/// How the device objects of a file are read.
#[derive(Clone, Copy)]
pub(crate) struct DeviceFormat {
    /// The fields a device object may have.
    pub(crate) fields: &'static [&'static str],
    /// Zones carry `reverse` (project format v6 and later).
    pub(crate) zone_reverse: bool,
}

/// Loads a v1 or v2 patch (see the module docs).
pub(crate) fn load(json: &str, registry: &Registry) -> Result<RackDoc, ModelError> {
    let root = JsonPath::default();
    let value: Value =
        serde_json::from_str(json).map_err(|e| root.error(format!("invalid JSON: {e}")))?;
    let obj = value
        .as_object()
        .ok_or_else(|| root.error(format!("expected an object, found {}", describe(&value))))?;
    // The tag and version decide how the rest is read, so they are checked first.
    let null = Value::Null;
    let format_path = root.field("format");
    let format = string(obj.get("format").unwrap_or(&null), &format_path)?;
    if format != PATCH_FORMAT {
        return Err(format_path.error(format!("expected \"{PATCH_FORMAT}\", found {format:?}")));
    }
    let version_path = root.field("version");
    let version = uint(obj.get("version").unwrap_or(&null), &version_path, u64::MAX)?;
    if !(1..=u64::from(PATCH_VERSION)).contains(&version) {
        return Err(version_path.error(format!(
            "unsupported version {version}; this reader reads versions 1 and {PATCH_VERSION}"
        )));
    }
    object(&value, &root, ROOT_FIELDS, ROOT_REQUIRED)?;
    let field = |name: &str| (obj.get(name), root.field(name));
    let (v, p) = field("length_beats");
    let length = number(v.unwrap_or(&null), &p)?;
    if !(length.is_finite() && length > 0.0) {
        return Err(p.error("must be a number greater than 0"));
    }
    if let (Some(v), p) = field("notes") {
        array(v, &p)?;
    }
    let device_fields = if version == 1 {
        DEVICE_FIELDS_V1
    } else {
        DEVICE_FIELDS_V2
    };
    let fmt = DeviceFormat {
        fields: device_fields,
        zone_reverse: false,
    };
    load_rack(obj, &root, fmt, registry)
}

/// Reads the rack fields shared by patches and projects (`sample_rate`, `tempo_bpm`, `seed`,
/// `output_channels`, `devices`, `cables`) from a root object whose fields were checked. The
/// song is empty at the file's tempo.
pub(crate) fn load_rack(
    obj: &Map<String, Value>,
    root: &JsonPath,
    fmt: DeviceFormat,
    registry: &Registry,
) -> Result<RackDoc, ModelError> {
    let null = Value::Null;
    let field = |name: &str| (obj.get(name), root.field(name));
    let (v, p) = field("sample_rate");
    let sample_rate = uint(v.unwrap_or(&null), &p, u64::from(u32::MAX))? as u32;
    if !SAMPLE_RATES.contains(&sample_rate) {
        return Err(p.error(format!(
            "{sample_rate} Hz is not supported; use 44100, 48000 or 96000"
        )));
    }
    let (v, p) = field("tempo_bpm");
    let tempo_bpm = number(v.unwrap_or(&null), &p)?;
    if !(crate::TEMPO_MIN..=crate::TEMPO_MAX).contains(&tempo_bpm) {
        return Err(p.error(format!("{tempo_bpm} BPM is outside 20..=999")));
    }
    let seed = match field("seed") {
        (Some(v), p) => uint(v, &p, u64::MAX)?,
        (None, _) => 0,
    };
    let (v, p) = field("output_channels");
    let output_channels = uint(v.unwrap_or(&null), &p, u64::from(u16::MAX))? as u16;
    if !(1..=MAX_OUTPUT_CHANNELS).contains(&output_channels) {
        return Err(p.error(format!(
            "{output_channels} is outside 1..={MAX_OUTPUT_CHANNELS}"
        )));
    }
    // Only project files may carry `input_channels` (their field lists allow it).
    let input_channels = match field("input_channels") {
        (Some(v), p) => {
            let n = uint(v, &p, u64::from(u16::MAX))? as u16;
            if n > crate::MAX_INPUT_CHANNELS {
                return Err(p.error(format!("{n} is outside 0..={}", crate::MAX_INPUT_CHANNELS)));
            }
            n
        }
        (None, _) => 0,
    };
    let mut doc = RackDoc {
        sample_rate,
        tempo_bpm,
        seed,
        output_channels,
        input_channels,
        assets: BTreeMap::new(),
        devices: Vec::new(),
        columns: 1,
        cables: Vec::new(),
        song: crate::song::empty_song(tempo_bpm),
        console: crate::console::Console::default(),
        midi_maps: Vec::new(),
        macros: crate::macros::MacroSet::default(),
    };
    let (v, p) = field("devices");
    let mut first_use: BTreeMap<String, JsonPath> = BTreeMap::new();
    for (i, item) in array(v.unwrap_or(&null), &p)?.iter().enumerate() {
        let path = p.index(i);
        let device = load_device(item, &path, fmt, 0, &doc.devices, registry, &first_use)?;
        first_use.insert(device.key.0.clone(), path.field("id"));
        doc.devices.push(device);
    }
    plugin_io::check_state_total(&doc.devices, &p, &mut 0)?;
    // The containers' rules (limits, keys, inner cables, splits, macros, loops) are checked once
    // the devices are read, before the outer cables, so a fault inside names its own path.
    doc.check_containers_at(registry)
        .map_err(|v| container_io::violation_error(root, &v))?;
    container_io::sort_inner_cables(&mut doc.devices);
    if let (Some(v), p) = field("cables") {
        for (i, item) in array(v, &p)?.iter().enumerate() {
            let cable = load_cable(item, &p.index(i), &doc, registry)?;
            doc.cables.push(cable);
        }
    }
    doc.cables.sort();
    Ok(doc)
}

/// Reads one device object. `depth` is the nesting depth of the scope the device is in: 0 at
/// the top level, a container's own depth for a device inside it. `siblings` are the devices
/// of the scope read so far.
pub(crate) fn load_device(
    item: &Value,
    path: &JsonPath,
    fmt: DeviceFormat,
    depth: usize,
    siblings: &[RackDevice],
    registry: &Registry,
    first_use: &BTreeMap<String, JsonPath>,
) -> Result<RackDevice, ModelError> {
    let obj = object(item, path, fmt.fields, &["id", "type"])?;
    let id_path = path.field("id");
    let id = string(&obj["id"], &id_path)?;
    if id == INTERFACE_KEY {
        return Err(id_path.error("\"interface\" is reserved for Audio I/O"));
    }
    if id == BOUNDARY_KEY {
        return Err(id_path.error("\"boundary\" is reserved for a container's inside edge"));
    }
    if !rules::is_valid_key(id) {
        return Err(id_path.error(format!("{id:?} must match [a-z][a-z0-9_-]*")));
    }
    if let Some(first) = first_use.get(id) {
        return Err(id_path.error(format!(
            "duplicate device id {id:?} (first used at {first})"
        )));
    }
    let type_path = path.field("type");
    let type_id = string(&obj["type"], &type_path)?;
    let plugin_path = path.field("plugin");
    if is_plugin_type_id(type_id) && !obj.contains_key("plugin") {
        return Err(plugin_path.error(format!(
            "missing field: the plugin device {type_id:?} needs a plugin object"
        )));
    }
    let desc = rules::descriptor(registry, type_id).map_err(|_| {
        let known: Vec<_> = registry.type_ids().collect();
        type_path.error(format!(
            "unknown device type {type_id:?}; known types: {}",
            known.join(", ")
        ))
    })?;
    if type_id == SPLIT_TYPE {
        return Err(type_path.error(format!(
            "{type_id} is internal and is not allowed in a project"
        )));
    }
    if depth > 0 && FORBIDDEN_INSIDE.contains(&type_id) {
        return Err(type_path.error(format!("{type_id} is not allowed inside a container")));
    }
    let mut params = BTreeMap::new();
    if let Some(v) = obj.get("params") {
        let params_path = path.field("params");
        let map = v.as_object().ok_or_else(|| {
            params_path.error(format!("expected an object, found {}", describe(v)))
        })?;
        for (key, value) in map {
            let param_path = params_path.field(key);
            let plain = number(value, &param_path)?;
            let Some(p) = desc.param_by_key(key) else {
                // A param a newer plugin version dropped is kept as it is.
                if crate::plugin::is_dead_plugin_param(desc, key) {
                    if !(plain as f32).is_finite() {
                        return Err(param_path.error(format!("{plain} is out of range")));
                    }
                    params.insert(key.clone(), plain as f32);
                    continue;
                }
                return Err(param_path.error(format!("{} has no param {key:?}", desc.type_id)));
            };
            if crate::console::is_internal_param(key) {
                return Err(
                    param_path.error(format!("{key:?} is set by the app and is never stored"))
                );
            }
            params.insert(key.clone(), p.clamp(plain as f32));
        }
    }
    let name = match obj.get("name") {
        Some(v) => string(v, &path.field("name"))?.to_owned(),
        None => rules::unique_name_in(siblings, desc.name),
    };
    let folded = match obj.get("folded") {
        Some(v) => v.as_bool().ok_or_else(|| {
            path.field("folded")
                .error(format!("expected a boolean, found {}", describe(v)))
        })?,
        None => false,
    };
    let color = match obj.get("color") {
        Some(v) => parse_color(v, &path.field("color"))?,
        None => None,
    };
    let plugin = match obj.get("plugin") {
        Some(v) if is_plugin_type_id(type_id) => {
            Some(Box::new(plugin_io::load_plugin(v, &plugin_path, type_id)?))
        }
        Some(_) => {
            return Err(plugin_path.error(format!("{type_id} is not a plugin device")));
        }
        None => None,
    };
    let column = match obj.get("column") {
        Some(v) => {
            let column_path = path.field("column");
            if depth > 0 {
                return Err(column_path.error("a device inside a container has no column"));
            }
            uint(v, &column_path, u64::from(crate::MAX_RACK_COLUMNS - 1))? as u16
        }
        None => 0,
    };
    Ok(RackDevice {
        key: DeviceKey(id.to_owned()),
        type_id: type_id.to_owned(),
        name,
        params,
        folded,
        color,
        // Checked against the pool and the registry by the project loader.
        zones: match obj.get("zones") {
            Some(v) => crate::project::load_zones(v, &path.field("zones"), fmt.zone_reverse)?,
            None => Vec::new(),
        },
        steps: match obj.get("steps") {
            Some(v) => {
                let steps_path = path.field("steps");
                if !registry.takes_player_data(type_id) {
                    return Err(steps_path.error(format!("{type_id} does not take pattern data")));
                }
                if registry.takes_drums(type_id) {
                    return Err(steps_path.error(format!("{type_id} takes drum data, not steps")));
                }
                Some(crate::project::load_steps(v, &steps_path)?)
            }
            None => None,
        },
        curves: match obj.get("curves") {
            Some(v) => {
                let curves_path = path.field("curves");
                if !registry.takes_curves(type_id) {
                    return Err(curves_path.error(format!("{type_id} does not take curves")));
                }
                Some(crate::project::load_curves(v, &curves_path)?)
            }
            None => None,
        },
        drums: match obj.get("drums") {
            Some(v) => {
                let drums_path = path.field("drums");
                if !registry.takes_drums(type_id) {
                    return Err(drums_path.error(format!("{type_id} does not take drum data")));
                }
                Some(crate::project::load_drums(v, &drums_path)?)
            }
            None => None,
        },
        inner: match obj.get("inner") {
            Some(v) => {
                let inner_path = path.field("inner");
                if desc.kind != DeviceKind::Container {
                    return Err(inner_path.error(format!("{type_id} does not have an inside")));
                }
                if depth + 1 > MAX_CONTAINER_DEPTH {
                    return Err(container_io::too_deep(&inner_path));
                }
                Some(Box::new(container_io::load_inner(
                    v,
                    &inner_path,
                    depth + 1,
                    fmt,
                    registry,
                )?))
            }
            // A container without `inner` has an empty inside.
            None if desc.kind == DeviceKind::Container => Some(Box::default()),
            None => None,
        },
        plugin,
        column,
    })
}

/// Parses and validates one cable against the devices and the cables loaded so far, with
/// the same rules (and error locations) as the renderer.
fn load_cable(
    item: &Value,
    path: &JsonPath,
    doc: &RackDoc,
    registry: &Registry,
) -> Result<Cable, ModelError> {
    let obj = object(item, path, &["from", "to"], &["from", "to"])?;
    let (from_path, to_path) = (path.field("from"), path.field("to"));
    let from = endpoint(&obj["from"], &from_path)?;
    let to = endpoint(&obj["to"], &to_path)?;
    let (_, from_dir) =
        rules::socket(doc, registry, &from).map_err(|e| end_error(&from_path, &from, e))?;
    let (_, to_dir) = rules::socket(doc, registry, &to).map_err(|e| end_error(&to_path, &to, e))?;
    let (from_text, to_text) = (from.to_string(), to.to_string());
    if from_dir != Direction::Output {
        return Err(from_path.error(format!(
            "{from_text:?} is an input; a cable starts at an output"
        )));
    }
    if !to_dir.is_input() {
        return Err(to_path.error(format!(
            "{to_text:?} is an output; a cable ends at an input"
        )));
    }
    if to_dir == Direction::Input && doc.cable_into(&to).is_some() {
        return Err(to_path.error(format!("{to_text:?} is already connected")));
    }
    let cable = Cable { from, to };
    rules::check_cable(doc, registry, &cable).map_err(|e| match e {
        ModelError::BadCable(m) => path.error(m),
        other => path.error(other),
    })?;
    Ok(cable)
}

fn end_error(path: &JsonPath, at: &SocketRef, e: ModelError) -> ModelError {
    match e {
        ModelError::UnknownDevice(d) => path.error(format!("no device has id {d:?}")),
        ModelError::UnknownKey(s) => path.error(format!("{:?} has no socket {s:?}", at.device.0)),
        other => path.error(other),
    }
}

pub(crate) fn endpoint(v: &Value, path: &JsonPath) -> Result<SocketRef, ModelError> {
    let text = string(v, path)?;
    let (device, socket) = text
        .split_once(':')
        .filter(|(d, s)| !d.is_empty() && !s.is_empty())
        .ok_or_else(|| path.error(format!("{text:?} must have the form \"device:socket\"")))?;
    Ok(SocketRef {
        device: DeviceKey(device.to_owned()),
        socket: socket.to_owned(),
    })
}

/// How `serde_json` reads a JSON number. This crate and `atomik-render` enable its
/// `float_roundtrip` feature, so parsing is correctly rounded and the shortest text of a
/// value reads back exactly.
#[cfg(test)]
fn serde_f64(text: &str) -> Option<f64> {
    serde_json::from_str::<f64>(text).ok()
}

/// Shortest text of a param value; read back as `f64` then `as f32` (as this reader and the
/// renderer do), it is exactly `x`.
pub(crate) fn f32_text(x: f32) -> String {
    if x == 0.0 && x.is_sign_negative() {
        // `-0` would be read as the integer 0.
        "-0.0".to_owned()
    } else {
        format!("{x}")
    }
}

/// Shortest text of an `f64` (the tempo); read back, it is exactly `x`.
pub(crate) fn f64_text(x: f64) -> String {
    format!("{x}")
}

pub(crate) fn json_str(s: &str) -> String {
    Value::from(s).to_string()
}

/// Writes a v2 patch. Field and device order are fixed, so the output is deterministic.
pub(crate) fn save(doc: &RackDoc) -> String {
    let mut out = String::new();
    let w = &mut out;
    // Writing to a String cannot fail.
    let _ = writeln!(w, "{{");
    let _ = writeln!(w, "  \"format\": {},", json_str(PATCH_FORMAT));
    let _ = writeln!(w, "  \"version\": {PATCH_VERSION},");
    let _ = writeln!(w, "  \"sample_rate\": {},", doc.sample_rate);
    let _ = writeln!(w, "  \"tempo_bpm\": {},", f64_text(doc.tempo_bpm));
    let _ = writeln!(w, "  \"seed\": {},", doc.seed);
    let _ = writeln!(w, "  \"length_beats\": {SAVE_LENGTH_BEATS},");
    let _ = writeln!(w, "  \"output_channels\": {},", doc.output_channels);
    write_devices_and_cables(w, doc, false);
    let _ = writeln!(w, "\n}}");
    out
}

/// Writes `  "devices": [...],` then `  "cables": [...]`, without a newline after the last `]`.
///
/// `zones` also writes each non-empty zone list (project v4), each device's `steps` and `curves` (project
/// v7) and each container's `inner` (project v8); patches have none of them.
pub(crate) fn write_devices_and_cables(w: &mut String, doc: &RackDoc, zones: bool) {
    let used = PluginUse(if zones {
        doc.plugin_param_usage()
    } else {
        BTreeMap::new()
    });
    let _ = write!(w, "  \"devices\": [");
    for (i, d) in doc.devices.iter().enumerate() {
        let sep = if i == 0 { "" } else { "," };
        let _ = writeln!(w, "{sep}");
        write_device(w, d, "    ", zones, &d.key.0, &used);
    }
    let _ = writeln!(w, "{}],", if doc.devices.is_empty() { "" } else { "\n  " });
    let _ = write!(w, "  \"cables\": [");
    for (i, c) in doc.cables.iter().enumerate() {
        let sep = if i == 0 { "" } else { "," };
        let _ = write!(
            w,
            "{sep}\n    {{\"from\": {}, \"to\": {}}}",
            json_str(&c.from.to_string()),
            json_str(&c.to.to_string())
        );
    }
    let _ = write!(w, "{}]", if doc.cables.is_empty() { "" } else { "\n  " });
}

/// The param ids each plugin device (by path key) uses, so that only those layout rows are
/// written (see [`crate::plugin`]).
pub(crate) struct PluginUse(pub(crate) BTreeMap<String, BTreeSet<u32>>);

/// Writes one device object whose braces are at indent `ind` (its fields two spaces deeper),
/// without a trailing newline. See [`write_devices_and_cables`] for `project`. `path` is the
/// device's path key.
pub(crate) fn write_device(
    w: &mut String,
    d: &RackDevice,
    ind: &str,
    project: bool,
    path: &str,
    used: &PluginUse,
) {
    let p = format!("{ind}  ");
    let _ = write!(
        w,
        "{ind}{{\n{p}\"id\": {}, \"type\": {},\n{p}\"name\": {}, \"folded\": {}",
        json_str(&d.key.0),
        json_str(&d.type_id),
        json_str(&d.name),
        d.folded
    );
    if let Some([r, g, b]) = d.color {
        let _ = write!(w, ", \"color\": \"#{r:02x}{g:02x}{b:02x}\"");
    }
    // Columns exist at the top level of a project only (never in a patch or a container).
    if project && d.column != 0 && !path.contains('/') {
        let _ = write!(w, ", \"column\": {}", d.column);
    }
    if !d.params.is_empty() {
        let _ = write!(w, ",\n{p}\"params\": {{");
        for (j, (k, v)) in d.params.iter().enumerate() {
            let sep = if j == 0 { "" } else { "," };
            let _ = write!(w, "{sep}\n{p}  {}: {}", json_str(k), f32_text(*v));
        }
        let _ = write!(w, "\n{p}}}");
    }
    if project && !d.zones.is_empty() {
        let _ = write!(w, ",\n{p}\"zones\": ");
        crate::project::write_zones(w, &d.zones, &p);
    }
    if let Some(steps) = d.steps.as_ref().filter(|_| project) {
        let _ = write!(w, ",\n{p}\"steps\": ");
        crate::project::write_steps(w, steps, &p);
    }
    if let Some(curves) = d.curves.as_ref().filter(|_| project) {
        let _ = write!(
            w,
            ",
{p}\"curves\": "
        );
        crate::project::write_curves(w, curves, &p);
    }
    if let Some(drums) = d.drums.as_ref().filter(|_| project) {
        let _ = write!(
            w,
            ",
{p}\"drums\": "
        );
        crate::project::write_drums(w, drums, &p);
    }
    if let Some(inner) = d.inner.as_ref().filter(|_| project) {
        let _ = write!(w, ",\n{p}\"inner\": ");
        container_io::write_inner(w, inner, &p, path, used);
    }
    if let Some(plugin) = d.plugin.as_ref().filter(|_| project) {
        let _ = write!(w, ",\n{p}\"plugin\": ");
        plugin_io::write_plugin(w, plugin, &p, used.0.get(path));
    }
    let _ = write!(w, "\n{ind}}}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f32_text_reads_back_exactly() {
        for x in [
            0.22f32,
            0.35,
            1.0,
            -0.0,
            0.0,
            20_000.0,
            f32::MAX,
            f32::MIN_POSITIVE,
            1e-45,
        ] {
            let back = serde_f64(&f32_text(x)).unwrap() as f32;
            assert_eq!(back.to_bits(), x.to_bits(), "{x}");
        }
        assert_eq!(f32_text(0.22), "0.22");
        assert_eq!(f32_text(-0.0), "-0.0");
    }

    #[test]
    fn f64_text_survives_serde_double_rounding() {
        // Without `float_roundtrip`, serde_json reads this tempo's shortest text one ULP off.
        let x = 188.110_848_031_224_26_f64;
        assert_eq!(serde_f64(&f64_text(x)).unwrap().to_bits(), x.to_bits());
        assert_eq!(f64_text(120.0), "120");
        assert_eq!(f64_text(97.25), "97.25");
    }

    proptest::proptest! {
        #[test]
        fn number_text_round_trips_through_serde(x in proptest::num::f64::NORMAL | proptest::num::f64::ZERO, y in proptest::num::f32::ANY) {
            proptest::prop_assert_eq!(serde_f64(&f64_text(x)).unwrap().to_bits(), x.to_bits());
            if y.is_finite() {
                proptest::prop_assert_eq!((serde_f64(&f32_text(y)).unwrap() as f32).to_bits(), y.to_bits());
            }
        }
    }

    #[test]
    fn json_path_quotes_non_identifiers() {
        let p = JsonPath::default()
            .field("devices")
            .index(0)
            .field("params");
        assert_eq!(
            p.field("filter.cutoff").to_string(),
            "devices[0].params.\"filter.cutoff\""
        );
        assert_eq!(JsonPath::default().to_string(), "$");
    }
}
