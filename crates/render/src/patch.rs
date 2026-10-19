//! The patch file format, versions 1 and 2: serde model, validation and JSON paths.
//!
//! Version 2 is version 1 plus optional per-device rack layout: `name` (string),
//! `folded` (boolean) and `color` (`"#rrggbb"` or `null`). Layout does not affect the sound,
//! so it is validated and then ignored: a v2 file renders exactly like the same v1 file.

use serde::Deserialize;
use std::collections::BTreeMap;

/// A parsed and validated patch.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Patch {
    /// Format tag, always [`FORMAT_TAG`].
    pub format: String,
    /// Format version: [`FORMAT_VERSION`] or [`FORMAT_VERSION_V2`].
    pub version: u32,
    /// Sample rate in Hz: 44 100, 48 000 or 96 000.
    pub sample_rate: u32,
    /// Tempo in BPM (20..=999).
    pub tempo_bpm: f64,
    /// Seed for every pseudo-random source.
    #[serde(default)]
    pub seed: u64,
    /// Musical length in beats; the render adds a [`TAIL_BEATS`] tail.
    pub length_beats: f64,
    /// Host output channels (`interface:out_1..out_N`), 1..=32.
    pub output_channels: u16,
    /// Device instances, in order.
    pub devices: Vec<DeviceSpec>,
    /// Cables between sockets.
    #[serde(default)]
    pub cables: Vec<CableSpec>,
    /// Notes to play.
    #[serde(default)]
    pub notes: Vec<NoteSpec>,
}

/// One device instance of a patch.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceSpec {
    /// Patch-unique id matching `[a-z][a-z0-9_-]*` (not `interface`).
    pub id: String,
    /// Registered device type id, e.g. `atomik.poly-synth`.
    #[serde(rename = "type")]
    pub device_type: String,
    /// Plain param values by param key.
    #[serde(default)]
    pub params: BTreeMap<String, f64>,
}

/// A cable `"device:socket"` → `"device:socket"`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CableSpec {
    /// Output endpoint.
    pub from: String,
    /// Input endpoint.
    pub to: String,
}

/// A note sent to a Notes input.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NoteSpec {
    /// Target device id.
    pub device: String,
    /// Target Notes input socket key.
    pub socket: String,
    /// Start, in beats from the beginning.
    pub at_beats: f64,
    /// Duration in beats (> 0).
    pub length_beats: f64,
    /// MIDI key 0..=127.
    pub key: u8,
    /// Velocity in (0, 1].
    pub velocity: f32,
}

impl NoteSpec {
    /// `(on, off)` frames: `beats_to_frames(..).round()` of `at_beats` and of
    /// `at_beats + length_beats`. The Off is at least one frame after the On, so a very short
    /// note is still released.
    pub fn frames(&self, tempo: atomik_core::Tempo, sr: atomik_core::SampleRate) -> (u64, u64) {
        let frame = |beats: f64| atomik_core::beats_to_frames(beats, tempo, sr).round() as u64;
        let on = frame(self.at_beats);
        let off = frame(self.at_beats + self.length_beats).max(on.saturating_add(1));
        (on, off)
    }
}

/// The only accepted `format` tag.
pub const FORMAT_TAG: &str = "atomik-patch";
/// Format version 1, the original renderer input.
pub const FORMAT_VERSION: u32 = 1;
/// Format version 2: version 1 plus per-device rack layout (`name`, `folded`, `color`).
pub const FORMAT_VERSION_V2: u32 = 2;
/// The per-device fields that version 2 adds; they carry no sound and are ignored.
pub const LAYOUT_FIELDS: [&str; 3] = ["name", "folded", "color"];
/// Beats rendered after `length_beats` so releases and echoes can ring out.
pub const TAIL_BEATS: f64 = 2.0;

/// A non-fatal problem found while building a patch (for example a clamped param value).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    /// JSON path of the value concerned.
    pub path: String,
    /// What was done about it.
    pub message: String,
}

impl std::fmt::Display for Warning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path, self.message)
    }
}

/// Clamps a plain param value through its descriptor. Returns the value to apply and, if it
/// differs from the one written in the patch, a warning at `path`.
pub fn clamp_param(
    descriptor: &atomik_core::ParamDescriptor,
    value: f64,
    path: &JsonPath,
) -> (f32, Option<Warning>) {
    let plain = value as f32;
    let clamped = descriptor.clamp(plain);
    if clamped.to_bits() == plain.to_bits() {
        return (clamped, None);
    }
    let (min, max) = (descriptor.min, descriptor.max);
    let message = if (min..=max).contains(&plain) {
        format!("{value} is not a whole number; rounded to {clamped}")
    } else {
        format!("{value} is outside {min}..={max}; clamped to {clamped}")
    };
    let warning = Warning {
        path: path.to_string(),
        message,
    };
    (clamped, Some(warning))
}

/// A patch that failed to parse or validate.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{path}: {message}")]
pub struct PatchError {
    /// JSON path of the offending value.
    pub path: String,
    /// What is wrong.
    pub message: String,
}

/// Accepted sample rates in Hz.
pub const SAMPLE_RATES: [u32; 3] = [44_100, 48_000, 96_000];
/// Longest accepted render (body plus tail), in seconds.
pub const MAX_RENDER_SECONDS: f64 = 3_600.0;
/// Id reserved for the AudioInterface.
pub const INTERFACE_ID: &str = "interface";

/// Builds JSON paths such as `devices[0].params."filter.cutoff"`. The root is `$`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JsonPath(String);

impl JsonPath {
    /// The document root.
    pub fn root() -> Self {
        Self::default()
    }

    /// `self.name`; names that are not plain identifiers are quoted (`."filter.cutoff"`).
    pub fn field(&self, name: &str) -> Self {
        let plain = name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        let part = if plain {
            name.to_owned()
        } else {
            serde_json::Value::from(name).to_string()
        };
        if self.0.is_empty() {
            Self(part)
        } else {
            Self(format!("{}.{part}", self.0))
        }
    }

    /// `self[index]`.
    pub fn index(&self, index: usize) -> Self {
        Self(format!("{}[{index}]", self.0))
    }

    /// An error at this path.
    pub fn error(&self, message: impl Into<String>) -> PatchError {
        PatchError {
            path: self.to_string(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for JsonPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0.is_empty() {
            f.write_str("$")
        } else {
            f.write_str(&self.0)
        }
    }
}

/// True if `id` matches `[a-z][a-z0-9_-]*`.
fn is_valid_device_id(id: &str) -> bool {
    let mut chars = id.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// Expected JSON shape of a value, checked before typed deserialization so that every
/// structural error can name its JSON path.
#[derive(Clone, Copy)]
enum Shape {
    Str,
    /// An unsigned integer no greater than the bound.
    UInt(u64),
    Num,
    /// An array of objects with these fields.
    Objects(&'static [Field]),
    /// An object whose values are all numbers (`params`).
    NumMap,
    Bool,
    /// A colour `"#rrggbb"` (either case) or `null`.
    Color,
}

struct Field {
    name: &'static str,
    shape: Shape,
    required: bool,
}

const fn req(name: &'static str, shape: Shape) -> Field {
    Field {
        name,
        shape,
        required: true,
    }
}

const fn opt(name: &'static str, shape: Shape) -> Field {
    Field {
        name,
        shape,
        required: false,
    }
}

const DEVICE_FIELDS: &[Field] = &[
    req("id", Shape::Str),
    req("type", Shape::Str),
    opt("params", Shape::NumMap),
];
const DEVICE_FIELDS_V2: &[Field] = &[
    req("id", Shape::Str),
    req("type", Shape::Str),
    opt("params", Shape::NumMap),
    opt("name", Shape::Str),
    opt("folded", Shape::Bool),
    opt("color", Shape::Color),
];
const CABLE_FIELDS: &[Field] = &[req("from", Shape::Str), req("to", Shape::Str)];
const NOTE_FIELDS: &[Field] = &[
    req("device", Shape::Str),
    req("socket", Shape::Str),
    req("at_beats", Shape::Num),
    req("length_beats", Shape::Num),
    req("key", Shape::UInt(127)),
    req("velocity", Shape::Num),
];
const ROOT_FIELDS: &[Field] = &[
    req("format", Shape::Str),
    req("version", Shape::UInt(u32::MAX as u64)),
    req("sample_rate", Shape::UInt(u32::MAX as u64)),
    req("tempo_bpm", Shape::Num),
    opt("seed", Shape::UInt(u64::MAX)),
    req("length_beats", Shape::Num),
    req("output_channels", Shape::UInt(u16::MAX as u64)),
    req("devices", Shape::Objects(DEVICE_FIELDS)),
    opt("cables", Shape::Objects(CABLE_FIELDS)),
    opt("notes", Shape::Objects(NOTE_FIELDS)),
];
const ROOT_FIELDS_V2: &[Field] = &[
    req("format", Shape::Str),
    req("version", Shape::UInt(u32::MAX as u64)),
    req("sample_rate", Shape::UInt(u32::MAX as u64)),
    req("tempo_bpm", Shape::Num),
    opt("seed", Shape::UInt(u64::MAX)),
    req("length_beats", Shape::Num),
    req("output_channels", Shape::UInt(u16::MAX as u64)),
    req("devices", Shape::Objects(DEVICE_FIELDS_V2)),
    opt("cables", Shape::Objects(CABLE_FIELDS)),
    opt("notes", Shape::Objects(NOTE_FIELDS)),
];

fn describe(v: &serde_json::Value) -> &'static str {
    use serde_json::Value;
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

fn check_object(
    v: &serde_json::Value,
    path: &JsonPath,
    fields: &[Field],
) -> Result<(), PatchError> {
    let obj = v
        .as_object()
        .ok_or_else(|| path.error(format!("expected an object, found {}", describe(v))))?;
    for key in obj.keys() {
        if !fields.iter().any(|f| f.name == key) {
            let names: Vec<_> = fields.iter().map(|f| f.name).collect();
            return Err(path.field(key).error(format!(
                "unknown field; expected one of: {}",
                names.join(", ")
            )));
        }
    }
    for f in fields {
        match obj.get(f.name) {
            Some(value) => check_shape(value, &path.field(f.name), f.shape)?,
            None if f.required => return Err(path.field(f.name).error("missing field")),
            None => {}
        }
    }
    Ok(())
}

fn check_shape(v: &serde_json::Value, path: &JsonPath, shape: Shape) -> Result<(), PatchError> {
    let found = || format!(", found {}", describe(v));
    match shape {
        Shape::Str if !v.is_string() => Err(path.error(format!("expected a string{}", found()))),
        Shape::Num if !v.is_number() => Err(path.error(format!("expected a number{}", found()))),
        Shape::UInt(max) => match v.as_u64() {
            None => Err(path.error(format!("expected an unsigned integer, found {v}"))),
            Some(n) if n > max => Err(path.error(format!("{n} is outside 0..={max}"))),
            Some(_) => Ok(()),
        },
        Shape::Objects(fields) => {
            let items = v
                .as_array()
                .ok_or_else(|| path.error(format!("expected an array{}", found())))?;
            items
                .iter()
                .enumerate()
                .try_for_each(|(i, item)| check_object(item, &path.index(i), fields))
        }
        Shape::NumMap => {
            let obj = v
                .as_object()
                .ok_or_else(|| path.error(format!("expected an object{}", found())))?;
            obj.iter()
                .try_for_each(|(k, value)| check_shape(value, &path.field(k), Shape::Num))
        }
        Shape::Bool if !v.is_boolean() => Err(path.error(format!("expected a boolean{}", found()))),
        Shape::Color if !(v.is_null() || v.as_str().is_some_and(is_color)) => {
            Err(path.error(format!("expected a colour \"#rrggbb\" or null, found {v}")))
        }
        Shape::Str | Shape::Num | Shape::Bool | Shape::Color => Ok(()),
    }
}

/// True for `#rrggbb` with hexadecimal digits of either case.
fn is_color(text: &str) -> bool {
    text.strip_prefix('#')
        .is_some_and(|h| h.len() == 6 && h.chars().all(|c| c.is_ascii_hexdigit()))
}

fn check_format(format: &str) -> Result<(), PatchError> {
    if format == FORMAT_TAG {
        Ok(())
    } else {
        Err(JsonPath::root()
            .field("format")
            .error(format!("expected \"{FORMAT_TAG}\", found {format:?}")))
    }
}

fn check_version(version: u64) -> Result<(), PatchError> {
    if (u64::from(FORMAT_VERSION)..=u64::from(FORMAT_VERSION_V2)).contains(&version) {
        Ok(())
    } else {
        Err(JsonPath::root().field("version").error(format!(
            "unsupported version {version}; this renderer reads versions              {FORMAT_VERSION} and {FORMAT_VERSION_V2}"
        )))
    }
}

impl Patch {
    /// Parses and validates a patch. Syntax errors carry serde_json's line and column at
    /// path `$`; every other error names the JSON path of the offending value.
    pub fn from_json(text: &str) -> Result<Patch, PatchError> {
        let root = JsonPath::root();
        let value: serde_json::Value =
            serde_json::from_str(text).map_err(|e| root.error(format!("invalid JSON: {e}")))?;
        let obj = value
            .as_object()
            .ok_or_else(|| root.error(format!("expected an object, found {}", describe(&value))))?;
        // The tag and version decide how the rest is read, so they are checked first.
        let format = obj.get("format").unwrap_or(&serde_json::Value::Null);
        check_shape(format, &root.field("format"), Shape::Str)?;
        check_format(format.as_str().unwrap_or_default())?;
        let version = obj.get("version").unwrap_or(&serde_json::Value::Null);
        check_shape(version, &root.field("version"), Shape::UInt(u64::MAX))?;
        check_version(version.as_u64().unwrap_or_default())?;
        let v2 = version.as_u64() == Some(u64::from(FORMAT_VERSION_V2));
        check_object(&value, &root, if v2 { ROOT_FIELDS_V2 } else { ROOT_FIELDS })?;
        let mut value = value;
        if v2 {
            // Layout is checked above and carries no sound: drop it before the serde model.
            let devices = value.get_mut("devices").and_then(|d| d.as_array_mut());
            for device in devices.into_iter().flatten() {
                if let Some(obj) = device.as_object_mut() {
                    LAYOUT_FIELDS.iter().for_each(|f| drop(obj.remove(*f)));
                }
            }
        }
        // The shape pass mirrors the serde model, so this only fails if they disagree.
        let patch: Patch = serde_json::from_value(value).map_err(|e| root.error(e.to_string()))?;
        patch.validate()?;
        patch.reject_container_devices()?;
        Ok(patch)
    }

    /// A patch file is a flat rack: the container (an inside that only the model understands)
    /// and the internal note splitter (made by the expansion) cannot be written by hand.
    fn reject_container_devices(&self) -> Result<(), PatchError> {
        for (i, device) in self.devices.iter().enumerate() {
            let path = JsonPath::root().field("devices").index(i).field("type");
            match device.device_type.as_str() {
                "atomik.container" => {
                    return Err(path.error(
                        "atomik.container cannot be used in a patch file: containers exist only in projects (.atomik), where the renderer flattens them",
                    ));
                }
                "atomik.note-split" => {
                    return Err(path.error(
                        "atomik.note-split is internal: only a project container creates it",
                    ));
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Checks every value that does not need the device registry.
    pub fn validate(&self) -> Result<(), PatchError> {
        let root = JsonPath::root();
        check_format(&self.format)?;
        check_version(u64::from(self.version))?;
        if !SAMPLE_RATES.contains(&self.sample_rate) {
            return Err(root.field("sample_rate").error(format!(
                "{} Hz is not supported; use 44100, 48000 or 96000",
                self.sample_rate
            )));
        }
        let tempo = self.tempo()?;
        if !(self.length_beats.is_finite() && self.length_beats > 0.0) {
            return Err(root
                .field("length_beats")
                .error("must be a number greater than 0"));
        }
        let seconds = (self.length_beats + TAIL_BEATS) * 60.0 / tempo.bpm();
        if seconds > MAX_RENDER_SECONDS {
            return Err(root.field("length_beats").error(format!(
                "the render would last {seconds:.0} s; the limit is 1 hour"
            )));
        }
        let max = atomik_engine::MAX_INTERFACE_CHANNELS;
        if !(1..=max).contains(&self.output_channels) {
            return Err(root
                .field("output_channels")
                .error(format!("{} is outside 1..={max}", self.output_channels)));
        }
        self.validate_devices(&root)?;
        self.validate_notes(&root)
    }

    /// Frames rendered: `length_beats` plus [`TAIL_BEATS`], rounded to the nearest frame
    /// (0 if the tempo or sample rate is invalid).
    pub fn total_frames(&self) -> usize {
        let sr = atomik_core::SampleRate::new(f64::from(self.sample_rate));
        match (self.tempo(), sr) {
            (Ok(tempo), Ok(sr)) => {
                atomik_core::beats_to_frames(self.length_beats + TAIL_BEATS, tempo, sr).round()
                    as usize
            }
            _ => 0,
        }
    }

    /// The validated tempo.
    pub fn tempo(&self) -> Result<atomik_core::Tempo, PatchError> {
        atomik_core::Tempo::new(self.tempo_bpm).map_err(|_| {
            JsonPath::root()
                .field("tempo_bpm")
                .error(format!("{} BPM is outside 20..=999", self.tempo_bpm))
        })
    }

    fn validate_devices(&self, root: &JsonPath) -> Result<(), PatchError> {
        let mut seen: BTreeMap<&str, JsonPath> = BTreeMap::new();
        for (i, device) in self.devices.iter().enumerate() {
            let path = root.field("devices").index(i).field("id");
            if device.id == INTERFACE_ID {
                return Err(path.error("\"interface\" is reserved for Audio I/O"));
            }
            if !is_valid_device_id(&device.id) {
                return Err(path.error(format!("{:?} must match [a-z][a-z0-9_-]*", device.id)));
            }
            if let Some(first) = seen.get(device.id.as_str()) {
                return Err(path.error(format!(
                    "duplicate device id {:?} (first used at {first})",
                    device.id
                )));
            }
            seen.insert(&device.id, path);
        }
        Ok(())
    }

    fn validate_notes(&self, root: &JsonPath) -> Result<(), PatchError> {
        for (i, note) in self.notes.iter().enumerate() {
            let path = root.field("notes").index(i);
            if !(note.at_beats.is_finite() && note.at_beats >= 0.0) {
                return Err(path.field("at_beats").error("must be a number, 0 or more"));
            }
            if !(note.length_beats.is_finite() && note.length_beats > 0.0) {
                return Err(path
                    .field("length_beats")
                    .error("must be a number greater than 0"));
            }
            if !(note.velocity > 0.0 && note.velocity <= 1.0) {
                return Err(path
                    .field("velocity")
                    .error(format!("{} is outside (0, 1]", note.velocity)));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) const EXAMPLE: &str = r#"{
      "format": "atomik-patch", "version": 1,
      "sample_rate": 48000, "tempo_bpm": 120, "seed": 1,
      "length_beats": 16, "output_channels": 2,
      "devices": [
        { "id": "synth", "type": "atomik.poly-synth", "params": { "filter.cutoff": 1200, "osc1.wave": 0 } },
        { "id": "delay", "type": "atomik.stereo-delay", "params": { "mix": 0.35 } },
        { "id": "mixer", "type": "atomik.line-mixer" }
      ],
      "cables": [
        { "from": "synth:out_l", "to": "delay:in_l" }, { "from": "synth:out_r", "to": "delay:in_r" },
        { "from": "delay:out_l", "to": "mixer:ch1_in_l" }, { "from": "delay:out_r", "to": "mixer:ch1_in_r" },
        { "from": "mixer:master_l", "to": "interface:out_1" }, { "from": "mixer:master_r", "to": "interface:out_2" }
      ],
      "notes": [ { "device": "synth", "socket": "notes_in", "at_beats": 0, "length_beats": 1, "key": 60, "velocity": 0.8 } ]
    }"#;

    #[test]
    fn patch_parse_valid_example() {
        let p = Patch::from_json(EXAMPLE).expect("the 03 §5 example is valid");
        assert_eq!(p.format, FORMAT_TAG);
        assert_eq!((p.version, p.sample_rate, p.seed), (1, 48_000, 1));
        assert!((p.tempo_bpm - 120.0).abs() < 1e-12);
        assert!((p.length_beats - 16.0).abs() < 1e-12);
        assert_eq!(p.output_channels, 2);
        let ids: Vec<_> = p.devices.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, ["synth", "delay", "mixer"]);
        assert_eq!(p.devices[1].device_type, "atomik.stereo-delay");
        assert_eq!(p.devices[0].params.len(), 2);
        assert!(p.devices[2].params.is_empty());
        assert_eq!(p.cables.len(), 6);
        assert_eq!(p.cables[4].to, "interface:out_1");
        assert_eq!(p.notes.len(), 1);
        assert_eq!(p.notes[0].key, 60);
    }

    /// The example with one edit applied, re-serialized.
    pub(crate) fn edited(edit: impl FnOnce(&mut serde_json::Value)) -> String {
        let mut v: serde_json::Value = serde_json::from_str(EXAMPLE).unwrap();
        edit(&mut v);
        v.to_string()
    }

    fn assert_rejects(text: &str, path: &str, needle: &str) {
        let e = Patch::from_json(text).expect_err("patch must be rejected");
        assert_eq!(e.path, path, "{e}");
        assert!(e.message.contains(needle), "{e:?} lacks {needle:?}");
        assert!(e.to_string().starts_with(&format!("{path}: ")), "{e}");
    }

    #[test]
    fn patch_validate_rejects_bad_header_values_with_paths() {
        use serde_json::json;
        let cases: [(&str, serde_json::Value, &str); 10] = [
            ("format", json!("other"), "atomik-patch"),
            ("version", json!(3), "version 3"),
            ("version", json!(0), "version 0"),
            ("sample_rate", json!(22050), "44100, 48000 or 96000"),
            ("tempo_bpm", json!(19.5), "20..=999"),
            ("tempo_bpm", json!(1000), "20..=999"),
            ("length_beats", json!(0), "greater than 0"),
            ("length_beats", json!(100_000), "1 hour"),
            ("output_channels", json!(0), "1..=32"),
            ("output_channels", json!(33), "1..=32"),
        ];
        for (field, value, needle) in cases {
            let text = edited(|v| v[field] = value);
            assert_rejects(&text, field, needle);
        }
    }

    #[test]
    fn patch_validate_rejects_bad_device_ids_with_paths() {
        use serde_json::json;
        for bad in ["Synth", "1synth", "", "syn th", "synth!", "interface"] {
            let text = edited(|v| v["devices"][1]["id"] = json!(bad));
            let needle = if bad == "interface" {
                "reserved"
            } else {
                "[a-z][a-z0-9_-]*"
            };
            assert_rejects(&text, "devices[1].id", needle);
        }
        for good in ["a", "my_synth-2"] {
            let text = edited(|v| v["devices"][1]["id"] = json!(good));
            assert!(Patch::from_json(&text).is_ok(), "{good}");
        }
        let text = edited(|v| v["devices"][2]["id"] = json!("synth"));
        assert_rejects(&text, "devices[2].id", "devices[0].id");
    }

    #[test]
    fn patch_structural_errors_name_json_path() {
        use serde_json::json;
        type Edit = Box<dyn FnOnce(&mut serde_json::Value)>;
        let cases: Vec<(Edit, &str, &str)> = vec![
            (
                Box::new(|v| v["colour"] = json!(1)),
                "colour",
                "unknown field",
            ),
            (
                Box::new(|v| v["devices"][1]["colour"] = json!(1)),
                "devices[1].colour",
                "unknown field",
            ),
            (
                Box::new(|v| v["cables"][3]["via"] = json!("x")),
                "cables[3].via",
                "unknown field",
            ),
            (
                Box::new(|v| v["notes"][0]["pitch"] = json!(60)),
                "notes[0].pitch",
                "unknown field",
            ),
            (
                Box::new(|v| v["sample_rate"] = json!("48k")),
                "sample_rate",
                "expected an unsigned integer",
            ),
            (
                Box::new(|v| v["sample_rate"] = json!(48000.5)),
                "sample_rate",
                "expected an unsigned integer",
            ),
            (
                Box::new(|v| v["tempo_bpm"] = json!("fast")),
                "tempo_bpm",
                "expected a number",
            ),
            (
                Box::new(|v| v["notes"][0]["key"] = json!(128)),
                "notes[0].key",
                "0..=127",
            ),
            (
                Box::new(|v| v["devices"][0]["params"]["filter.cutoff"] = json!("high")),
                "devices[0].params.\"filter.cutoff\"",
                "expected a number",
            ),
            (
                Box::new(|v| v["devices"][0]["params"] = json!([1])),
                "devices[0].params",
                "expected an object",
            ),
            (
                Box::new(|v| v["cables"][3]["to"] = json!(5)),
                "cables[3].to",
                "expected a string",
            ),
            (
                Box::new(|v| v["devices"] = json!({})),
                "devices",
                "expected an array",
            ),
            (
                Box::new(|v| v["devices"][2] = json!("mixer")),
                "devices[2]",
                "expected an object",
            ),
            (
                Box::new(|v| {
                    v["devices"][0].as_object_mut().unwrap().remove("type");
                }),
                "devices[0].type",
                "missing field",
            ),
            (
                Box::new(|v| {
                    v.as_object_mut().unwrap().remove("tempo_bpm");
                }),
                "tempo_bpm",
                "missing field",
            ),
            (
                Box::new(|v| {
                    v["format"] = json!("other");
                    v["junk"] = json!(1);
                }),
                "format",
                "atomik-patch",
            ),
            (Box::new(|v| *v = json!([])), "$", "expected an object"),
        ];
        for (edit, path, needle) in cases {
            assert_rejects(&edited(edit), path, needle);
        }
    }

    #[test]
    fn patch_syntax_error_reports_line_and_column() {
        let e = Patch::from_json("{\n  \"format\": \"atomik-patch\",\n  oops\n}").unwrap_err();
        assert_eq!(e.path, "$");
        assert!(e.message.contains("line 3"), "{e}");
        assert!(e.message.contains("column"), "{e}");
    }

    #[test]
    fn patch_optional_fields_default() {
        let text = edited(|v| {
            let o = v.as_object_mut().unwrap();
            o.remove("seed");
            o.remove("cables");
            o.remove("notes");
        });
        let p = Patch::from_json(&text).unwrap();
        assert_eq!(p.seed, 0);
        assert!(p.cables.is_empty() && p.notes.is_empty());
    }

    fn note(at_beats: f64, length_beats: f64) -> NoteSpec {
        NoteSpec {
            device: "synth".into(),
            socket: "notes_in".into(),
            at_beats,
            length_beats,
            key: 60,
            velocity: 1.0,
        }
    }

    #[test]
    fn notes_convert_beats_to_frames() {
        use atomik_core::{SampleRate, Tempo};
        let (t120, sr48) = (
            Tempo::new(120.0).unwrap(),
            SampleRate::new(48_000.0).unwrap(),
        );
        assert_eq!(note(0.0, 1.0).frames(t120, sr48), (0, 24_000));
        assert_eq!(note(0.5, 1.5).frames(t120, sr48), (12_000, 48_000));
        // 97 BPM at 44.1 kHz: 1/3 beat = 9092.78… frames → rounds to nearest.
        let (t97, sr44) = (
            Tempo::new(97.0).unwrap(),
            SampleRate::new(44_100.0).unwrap(),
        );
        assert_eq!(
            note(1.0 / 3.0, 1.0 / 3.0).frames(t97, sr44),
            (9_093, 18_186)
        );
        // An Off never lands on the On frame, even for a tiny length.
        assert_eq!(note(1.0, 1e-9).frames(t120, sr48), (24_000, 24_001));
        // Absurdly late notes saturate instead of overflowing.
        assert_eq!(note(1e30, 1.0).frames(t120, sr48), (u64::MAX, u64::MAX));
        let p = Patch::from_json(EXAMPLE).unwrap();
        assert_eq!(p.total_frames(), 18 * 24_000, "16 beats + 2 tail beats");
    }

    #[test]
    fn patch_validate_rejects_bad_note_values_with_paths() {
        use serde_json::json;
        let cases: [(&str, serde_json::Value, &str); 4] = [
            ("at_beats", json!(-0.5), "0 or more"),
            ("length_beats", json!(0), "greater than 0"),
            ("velocity", json!(0), "(0, 1]"),
            ("velocity", json!(1.5), "(0, 1]"),
        ];
        for (field, value, needle) in cases {
            let text = edited(|v| v["notes"][0][field] = value);
            assert_rejects(&text, &format!("notes[0].{field}"), needle);
        }
    }

    /// The example as format v2 with layout on every device.
    pub(crate) fn v2_example() -> String {
        use serde_json::json;
        edited(|v| {
            v["version"] = json!(2);
            v["devices"][0]["name"] = json!("Lead \"1\"");
            v["devices"][0]["folded"] = json!(true);
            v["devices"][0]["color"] = json!("#A0b1c2");
            v["devices"][1]["color"] = serde_json::Value::Null;
            v["devices"][2]["folded"] = json!(false);
        })
    }

    #[test]
    fn patch_v2_reads_like_v1_ignoring_layout() {
        let v1 = Patch::from_json(EXAMPLE).unwrap();
        let v2 = Patch::from_json(&v2_example()).expect("v2 with layout is valid");
        assert_eq!(v2.version, 2);
        assert_eq!(Patch { version: 1, ..v2 }, v1);
    }

    #[test]
    fn patch_layout_fields_are_v2_only() {
        use serde_json::json;
        for field in ["name", "folded", "color"] {
            let text = edited(|v| v["devices"][1][field] = json!(1));
            assert_rejects(&text, &format!("devices[1].{field}"), "unknown field");
        }
    }

    #[test]
    fn patch_v2_layout_errors_name_json_path() {
        use serde_json::json;
        let cases: [(&str, serde_json::Value, &str); 5] = [
            ("name", json!(1), "expected a string"),
            ("folded", json!("yes"), "expected a boolean"),
            ("color", json!("#12345"), "#rrggbb"),
            ("color", json!("#gg0000"), "#rrggbb"),
            ("color", json!([1, 2, 3]), "#rrggbb"),
        ];
        for (field, value, needle) in cases {
            let mut v: serde_json::Value = serde_json::from_str(&v2_example()).unwrap();
            v["devices"][1][field] = value;
            assert_rejects(&v.to_string(), &format!("devices[1].{field}"), needle);
        }
    }
}
