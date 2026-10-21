//! `.atomik` project files: one JSON document holding the rack (the
//! patch v2 rack fields) and the song.
//!
//! ```text
//! { "format": "atomik-project", "version": 10,
//!   "sample_rate", "tempo_bpm", "seed", "output_channels", "devices", "cables"  (as patch v2;
//!       a device may also have "zones", v4, written only when non-empty, "steps", v7,
//!       written only when present, "inner", v8, a container's inside, see below, and
//!       "plugin", v9, a plugin device's document, see below)
//!   "input_channels": 0..=32,                                   (v3; optional, default 0)
//!   "console": { "channels": [{ "channel": key, "source": key | null }…],
//!                "master": key | null, "returns": [key | null × 8], "compensation": bool },
//!   "assets": { "<64 hex digits>": { "channels", "sample_rate", "frames", "name" }… },
//!                                                               (v3; optional, default {})
//!   "song": { "tempo": [{"tick", "bpm"}…], "meter": [beats_per_bar, beat_unit],
//!             "loop": {"start", "end", "enabled"}, "end", "next_clip_id",
//!             "tracks": [{ "device", "muted", "solo", "armed",
//!                          "lanes": [{ "kind": "notes" | "automation" | "audio", "param"?,
//!                                      "clips": [{ "id", "start", "length", "offset",
//!                                                  "loop_length" (int or null), "muted",
//!                                                  "grid"? (v5, note clips only, see below),
//!                                                  "notes": [[tick, length, key, velocity]…]
//!                                                      (v10: a muted note has a fifth element,
//!                                                      `true`, see below)
//!                                                  | "points": [[tick, value, "linear"|"step"]…]
//!                                                  | "audio": { "asset", "offset" (frames),
//!                                                      "gain_db", "fade_in", "fade_out" (frames),
//!                                                      "fade_curve": "linear"|"equal_power" }
//!                                               }…] }…] }…] } }
//! ```
//!
//! An audio track (a track on an `atomik.audio-track` device, v3 only) has exactly one lane,
//! `"audio"`, whose clips carry `"audio"` content, with `offset` 0 and `loop_length` null; every
//! `"audio"."asset"` must be a key of `"assets"`, and its `offset` at most that asset's
//! `frames`. In `"audio"` only `asset` and `offset` are required (`gain_db` 0, fades 0 and
//! `"linear"` otherwise); saving writes all of them.
//!
//! A sampler's `"zones"` (v4) is an array, in list order, of
//! `{ "asset", "key_lo", "key_hi", "vel_lo", "vel_hi", "root_key", "transpose", "tune_cents",
//! "gain_db", "pan", "start", "end", "loop_mode": "off"|"forward"|"ping_pong", "loop_start",
//! "loop_end", "crossfade", "trigger": "gate"|"one_shot", "reverse": bool (v6) }`, every field
//! required. Files before version 6 have no `"reverse"` (unknown field; the zone plays forward). Only a
//! device whose type takes samples may have zones; each `"asset"` must be a key of `"assets"`
//! (as for audio clips) and the zone must satisfy the SDK `Zone` rules against that asset's
//! `frames`. A zone's asset *file* missing from the bundle is not a load error (the app plays
//! silence).
//!
//! A step sequencer's `"steps"` (v7) is `{"patterns": [...]}` with 1 to 8 patterns, each
//! `{"name" (at most 64 characters), "length": 1..=64, "resolution": "quarter" | "eighth" |
//! "eighth_triplet" | "sixteenth" | "sixteenth_triplet" | "thirty_second" |
//! "thirty_second_triplet", "swing": 0..=75, "steps": [...]}` where `"steps"` holds exactly 64
//! entries `{"on", "key": 0..=127, "velocity": 1..=127, "gate": 1..=100, "prob": 0..=100,
//! "tie"}` (every field required; entries beyond `length` are kept). Every step is written in
//! full. Only a device whose type takes pattern data may have `"steps"` (a sequencer without it
//! plays one empty pattern); files before version 7 take none (unknown field).
//!
//! A container's `"inner"` (v8, only on devices whose type is a container, always written
//! for them) is `{"devices": [...], "cables": [...], "macros": [...], "splits": [...]}`: inner
//! devices and cables in the same form as at the top level with **local** keys (the container's
//! inside edge is the reserved key `boundary`), exactly 8 macros `{"name" (at most 24
//! characters), "targets": [{"device" (a path relative to the container), "param", "min", "max"}]}`
//! and up to 8 note splits `{"target", "key_lo", "key_hi", "vel_lo", "vel_hi", "transpose"}`.
//! See [`crate::container_io`] and [`crate::RackDoc::check_containers`] for the rules (depth 3, 32
//! devices per container, 256 inner devices per project, no console devices, audio tracks or
//! interface inside, no loop in the expanded graph). A project file before version 8 has no
//! `"inner"` (unknown field); a device keyed `boundary` in such a file is renamed on load.
//!
//! A plugin device's `"plugin"` (v9, only on devices whose type id is `clap.<id>` or
//! `vst3.<hex>`, always written for them, after `"inner"`) is `{"format": "clap" | "vst3", "id",
//! "name", "vendor", "version", "path_hint"?, "layout": {"main_in", "main_out", "sidechain",
//! "notes_in", "notes_out", "kind", "params": [rows]}, "state"?: base64, "pinned", "run_on_audio_thread",
//! "powered"}`; see [`crate::plugin_io`]. Its params are in the device's `"params"` under the keys
//! `p<plugin param id>` and `bypass`, stored sparsely; the layout rows written are those the project
//! uses. A plugin the registry does not know loads as a **stand-in** built from the saved layout; a
//! `p<id>` key the descriptor lacks is kept as a dead key (never a load error). A project file
//! before version 9 has no `"plugin"` (unknown field).
//!
//! A muted note (v10) is written `[tick, length, key, velocity, true]`; an unmuted one stays a
//! 4-tuple, so a file without muted notes differs from version 9 only in its version. A version 10
//! reader also accepts an explicit `false` as the fifth element, which must be a boolean; files
//! before version 10 take 4-tuples only (a fifth element is an error naming the version).
//!
//! A note clip's `"grid"` (v5) is written only when the clip has one, after `"notes"`:
//! `{"resolution": "eighth" | "sixteenth" | "thirty_second", "steps": 16..=64, "swing": 0..=75}`,
//! every field required. Audio and automation clips, and files before version 5, take no
//! `"grid"` (unknown field).
//!
//! Ticks are JSON integers (exact). Floats are written as their shortest round-trip text and
//! read with `float_roundtrip`, so save → load is bit-exact, and saving is canonical (save →
//! load → save gives the same text). `song.tempo[0].bpm` must equal `tempo_bpm`. Patch v1/v2
//! files load as projects with an empty song and an empty console. Every error names its
//! JSON path.
//!
//! **Version 1** files (no `"console"`) are migrated on load: with the console devices in the
//! registry, every instrument gets a channel in rack order, wired as `AddChannel` does;
//! otherwise the console stays empty. **Version 2** files (no inputs, no pool, no audio
//! lanes) load with `input_channels` 0 and an empty pool. **Version 3** files have no zones.
//! **Version 4** files have no grids. **Version 5** zones have no `"reverse"`. **Version 6** files have no steps. **Version 7** files have no containers. **Version 8** files have no plugin devices. **Version 9** notes have no mute flag. Saving always writes version 10.

use crate::console::{
    CHANNEL_TYPE, Console, ConsoleChannel, FX_SLOTS, MASTER_TYPE, is_console_type,
};
use crate::patch::{
    DEVICE_FIELDS_V2, DEVICE_FIELDS_V4, DEVICE_FIELDS_V7, DEVICE_FIELDS_V8, DEVICE_FIELDS_V9,
    DEVICE_FIELDS_V11, DeviceFormat, JsonPath, array, describe, f32_text, f64_text, json_str,
    load_rack, number, object, string, uint, write_devices_and_cables,
};
use crate::song::{
    AUDIO_GAIN_DB_MAX, AUDIO_GAIN_DB_MIN, AudioClipContent, AutomationPoint, Clip, ClipContent,
    ClipId, Curve, FadeCurve, Lane, LaneKind, MAX_AUDIO_FRAMES, MAX_LOOP_REPEATS, Note, Song,
    StepGrid, StepResolution, TempoEvent, Track,
};
use crate::song_cmd::{
    check_end, check_loop, check_meter, check_note, has_notes_input, is_audio_track_type,
    length_ok, tick_ok,
};
use crate::steps::{
    MAX_NAME_CHARS, MAX_PATTERNS, MAX_STEPS, PatternDoc, SeqResolution, StepDoc, StepSeqDoc,
};
use crate::{AssetMeta, ZoneDoc, is_valid_asset_hash};
use crate::{DeviceKey, ModelError, PATCH_FORMAT, RackDoc, TEMPO_MAX, TEMPO_MIN, rules};
use atomik_device_sdk::Registry;
use atomik_device_sdk::{LoopMode, TriggerMode};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// The `format` tag of a project file.
pub const PROJECT_FORMAT: &str = "atomik-project";
/// The project format version [`crate::save_project`] writes. [`crate::load_project`] reads
/// it and versions 1 (no console section; migrated on load), 2 (no inputs, pool or audio) and
/// 3 (no zones), 4 (no grids), 5 (no zone reverse), 6 (no steps), 7 (no containers) and 8 (no
/// plugin devices) and 9 (no note mute) and 10 (no rack columns: one column on load).
pub const PROJECT_VERSION: u32 = 11;

const ROOT_FIELDS_V1: &[&str] = &[
    "format",
    "version",
    "sample_rate",
    "tempo_bpm",
    "seed",
    "output_channels",
    "devices",
    "cables",
    "song",
];
const ROOT_REQUIRED_V1: &[&str] = &[
    "format",
    "version",
    "sample_rate",
    "tempo_bpm",
    "output_channels",
    "devices",
    "song",
];
const ROOT_FIELDS_V2: &[&str] = &[
    "format",
    "version",
    "sample_rate",
    "tempo_bpm",
    "seed",
    "output_channels",
    "devices",
    "cables",
    "console",
    "song",
];
const ROOT_REQUIRED_V2: &[&str] = &[
    "format",
    "version",
    "sample_rate",
    "tempo_bpm",
    "output_channels",
    "devices",
    "console",
    "song",
];
const ROOT_FIELDS_V3: &[&str] = &[
    "format",
    "version",
    "sample_rate",
    "tempo_bpm",
    "seed",
    "output_channels",
    "input_channels",
    "devices",
    "cables",
    "console",
    "assets",
    "song",
];
const ROOT_FIELDS_V4: &[&str] = ROOT_FIELDS_V3;
const ROOT_FIELDS_V5: &[&str] = ROOT_FIELDS_V3;
const ROOT_FIELDS_V6: &[&str] = ROOT_FIELDS_V3;
const ROOT_FIELDS_V7: &[&str] = ROOT_FIELDS_V3;
const ROOT_FIELDS_V8: &[&str] = ROOT_FIELDS_V3;
const ROOT_FIELDS_V9: &[&str] = ROOT_FIELDS_V3;
const ROOT_FIELDS_V10: &[&str] = ROOT_FIELDS_V3;
const ROOT_FIELDS_V11: &[&str] = &[
    "format",
    "version",
    "sample_rate",
    "tempo_bpm",
    "seed",
    "output_channels",
    "input_channels",
    "rack_columns",
    "devices",
    "cables",
    "console",
    "assets",
    "song",
];
const PATTERNS_FIELDS: &[&str] = &["patterns"];
const PATTERN_FIELDS: &[&str] = &["name", "length", "resolution", "swing", "steps"];
const STEP_FIELDS: &[&str] = &["on", "key", "velocity", "gate", "prob", "tie"];
const GRID_FIELDS: &[&str] = &["resolution", "steps", "swing"];
const ZONE_FIELDS: &[&str] = &[
    "asset",
    "key_lo",
    "key_hi",
    "vel_lo",
    "vel_hi",
    "root_key",
    "transpose",
    "tune_cents",
    "gain_db",
    "pan",
    "start",
    "end",
    "loop_mode",
    "loop_start",
    "loop_end",
    "crossfade",
    "trigger",
];
const ZONE_FIELDS_V6: &[&str] = &[
    "asset",
    "key_lo",
    "key_hi",
    "vel_lo",
    "vel_hi",
    "root_key",
    "transpose",
    "tune_cents",
    "gain_db",
    "pan",
    "start",
    "end",
    "loop_mode",
    "loop_start",
    "loop_end",
    "crossfade",
    "trigger",
    "reverse",
];
const ASSET_FIELDS: &[&str] = &["channels", "sample_rate", "frames", "name"];
const AUDIO_LANE_FIELDS: &[&str] = &["kind", "clips"];
const AUDIO_CLIP_FIELDS: &[&str] = &[
    "id",
    "start",
    "length",
    "offset",
    "loop_length",
    "muted",
    "audio",
];
const AUDIO_FIELDS: &[&str] = &[
    "asset",
    "offset",
    "gain_db",
    "fade_in",
    "fade_out",
    "fade_curve",
];
const CONSOLE_FIELDS: &[&str] = &["channels", "master", "returns", "compensation"];
const CHANNEL_FIELDS: &[&str] = &["channel", "source"];
const SONG_FIELDS: &[&str] = &["tempo", "meter", "loop", "end", "next_clip_id", "tracks"];
const TRACK_FIELDS: &[&str] = &["device", "muted", "solo", "armed", "lanes"];
const NOTES_LANE_FIELDS: &[&str] = &["kind", "clips"];
const AUTOMATION_LANE_FIELDS: &[&str] = &["kind", "param", "clips"];
const NOTE_CLIP_FIELDS: &[&str] = &[
    "id",
    "start",
    "length",
    "offset",
    "loop_length",
    "muted",
    "notes",
];
const NOTE_CLIP_FIELDS_V5: &[&str] = &[
    "id",
    "start",
    "length",
    "offset",
    "loop_length",
    "muted",
    "grid",
    "notes",
];
const AUTOMATION_CLIP_FIELDS: &[&str] = &[
    "id",
    "start",
    "length",
    "offset",
    "loop_length",
    "muted",
    "points",
];

fn int(v: &Value, path: &JsonPath) -> Result<i64, ModelError> {
    v.as_i64()
        .ok_or_else(|| path.error(format!("expected an integer, found {v}")))
}

fn boolean(v: &Value, path: &JsonPath) -> Result<bool, ModelError> {
    v.as_bool()
        .ok_or_else(|| path.error(format!("expected a boolean, found {}", describe(v))))
}

/// An optional boolean field (default `false`).
fn flag(
    obj: &serde_json::Map<String, Value>,
    path: &JsonPath,
    name: &str,
) -> Result<bool, ModelError> {
    obj.get(name)
        .map_or(Ok(false), |v| boolean(v, &path.field(name)))
}

/// A tick in `0..=MAX_TICK`.
fn tick(v: &Value, path: &JsonPath) -> Result<i64, ModelError> {
    let t = int(v, path)?;
    if !tick_ok(t) {
        return Err(path.error(format!("{t} is outside 0..={}", crate::song::MAX_TICK)));
    }
    Ok(t)
}

/// A length in `1..=MAX_TICK`.
fn length(v: &Value, path: &JsonPath) -> Result<i64, ModelError> {
    let t = int(v, path)?;
    if !length_ok(t) {
        return Err(path.error(format!(
            "{t} must be greater than 0 and at most {}",
            crate::song::MAX_TICK
        )));
    }
    Ok(t)
}

/// A fixed-size JSON array (a note or point tuple).
fn tuple<'v>(
    v: &'v Value,
    path: &JsonPath,
    n: usize,
    shape: &str,
) -> Result<&'v [Value], ModelError> {
    match v.as_array() {
        Some(a) if a.len() == n => Ok(a),
        _ => Err(path.error(format!("expected {shape}, found {v}"))),
    }
}

/// A note's elements: `[tick, length, key, velocity]`, plus the mute flag from version 10 on.
fn note_tuple<'v>(v: &'v Value, path: &JsonPath, version: u64) -> Result<&'v [Value], ModelError> {
    let note_mute = version >= 10;
    match v.as_array() {
        Some(a) if a.len() == 4 || (note_mute && a.len() == 5) => Ok(a),
        Some(a) if a.len() == 5 => Err(path.error(format!(
            "a note with a mute flag needs project version 10 or later, but this file is version {version}"
        ))),
        _ if note_mute => Err(path.error(format!(
            "expected a note [tick, length, key, velocity] or [tick, length, key, velocity, muted], found {v}"
        ))),
        _ => Err(path.error(format!(
            "expected a note [tick, length, key, velocity], found {v}"
        ))),
    }
}

/// Loads a project, or a patch v1/v2 as a song-less project.
pub(crate) fn load(json: &str, registry: &Registry) -> Result<RackDoc, ModelError> {
    load_with_warnings(json, registry).map(|(doc, _)| doc)
}

/// [`load`] and the load warnings (the rename of a device keyed `boundary`, see
/// [`rename_boundary`]).
pub(crate) fn load_with_warnings(
    json: &str,
    registry: &Registry,
) -> Result<(RackDoc, Vec<String>), ModelError> {
    let root = JsonPath::default();
    let mut value: Value =
        serde_json::from_str(json).map_err(|e| root.error(format!("invalid JSON: {e}")))?;
    let mut warnings = Vec::new();
    // A file older than version 8 may use `boundary` as a device key; it is reserved now.
    let old = value
        .get("version")
        .and_then(Value::as_u64)
        .is_some_and(|v| v < 8);
    if old && rename_boundary(&mut value, &mut warnings) {
        let text = value.to_string();
        let doc = if value.get("format").and_then(Value::as_str) == Some(PATCH_FORMAT) {
            crate::patch::load(&text, registry)?
        } else {
            load_value(&value, &text, registry)?
        };
        return Ok((doc, warnings));
    }
    load_value(&value, json, registry).map(|doc| (doc, warnings))
}

fn load_value(value: &Value, json: &str, registry: &Registry) -> Result<RackDoc, ModelError> {
    let root = JsonPath::default();
    let obj = value
        .as_object()
        .ok_or_else(|| root.error(format!("expected an object, found {}", describe(value))))?;
    let null = Value::Null;
    let format_path = root.field("format");
    let format = string(obj.get("format").unwrap_or(&null), &format_path)?;
    if format == PATCH_FORMAT {
        return crate::patch::load(json, registry);
    }
    if format != PROJECT_FORMAT {
        return Err(format_path.error(format!(
            "expected \"{PROJECT_FORMAT}\" (or a patch, \"{PATCH_FORMAT}\"), found {format:?}"
        )));
    }
    let version_path = root.field("version");
    let version = uint(obj.get("version").unwrap_or(&null), &version_path, u64::MAX)?;
    let (fields, required) = match version {
        1 => (ROOT_FIELDS_V1, ROOT_REQUIRED_V1),
        2 => (ROOT_FIELDS_V2, ROOT_REQUIRED_V2),
        3 => (ROOT_FIELDS_V3, ROOT_REQUIRED_V2),
        4 => (ROOT_FIELDS_V4, ROOT_REQUIRED_V2),
        5 => (ROOT_FIELDS_V5, ROOT_REQUIRED_V2),
        6 => (ROOT_FIELDS_V6, ROOT_REQUIRED_V2),
        7 => (ROOT_FIELDS_V7, ROOT_REQUIRED_V2),
        8 => (ROOT_FIELDS_V8, ROOT_REQUIRED_V2),
        9 => (ROOT_FIELDS_V9, ROOT_REQUIRED_V2),
        10 => (ROOT_FIELDS_V10, ROOT_REQUIRED_V2),
        11 => (ROOT_FIELDS_V11, ROOT_REQUIRED_V2),
        _ => {
            return Err(version_path.error(format!(
                "unsupported version {version}; this reader reads versions 1 to {PROJECT_VERSION}"
            )));
        }
    };
    object(value, &root, fields, required)?;
    let device_fields = if version >= 11 {
        DEVICE_FIELDS_V11
    } else if version >= 9 {
        DEVICE_FIELDS_V9
    } else if version >= 8 {
        DEVICE_FIELDS_V8
    } else if version >= 7 {
        DEVICE_FIELDS_V7
    } else if version >= 4 {
        DEVICE_FIELDS_V4
    } else {
        DEVICE_FIELDS_V2
    };
    let fmt = DeviceFormat {
        fields: device_fields,
        zone_reverse: version >= 6,
    };
    // A plugin the registry does not know is built from its saved layout; the loader
    // reads with a copy of the registry that has these stand-in types.
    let stand_ins = if version >= 9 {
        crate::plugin_io::stand_in_registry(
            obj.get("devices").unwrap_or(&null),
            &root.field("devices"),
            registry,
        )?
    } else {
        None
    };
    let registry = stand_ins.as_ref().unwrap_or(registry);
    let mut doc = load_rack(obj, &root, fmt, registry)?;
    if let Some(v) = obj.get("rack_columns") {
        let p = root.field("rack_columns");
        let n = uint(v, &p, u64::from(crate::MAX_RACK_COLUMNS))? as u16;
        if n == 0 {
            return Err(p.error(format!("0 is outside 1..={}", crate::MAX_RACK_COLUMNS)));
        }
        doc.columns = n;
    }
    check_device_columns(&doc, &root.field("devices"))?;
    if let Some(assets) = obj.get("assets") {
        doc.assets = load_assets(assets, &root.field("assets"))?;
    }
    check_device_zones(&doc, &root.field("devices"), registry)?;
    doc.song = load_song(&obj["song"], &root.field("song"), (&doc, registry), version)?;
    if version == 1 {
        return Ok(migrate_v1(doc, registry));
    }
    doc.console = load_console(&obj["console"], &root.field("console"), &doc)?;
    Ok(doc)
}

/// A device's column must exist and the list must be sorted by column.
fn check_device_columns(doc: &RackDoc, path: &JsonPath) -> Result<(), ModelError> {
    let mut last = 0;
    for (i, d) in doc.devices.iter().enumerate() {
        let p = path.index(i).field("column");
        if d.column >= doc.columns {
            return Err(p.error(format!(
                "column {} does not exist (rack_columns is {})",
                d.column, doc.columns
            )));
        }
        if d.column < last {
            return Err(p.error(format!(
                "devices must be sorted by column; column {} follows column {last}",
                d.column
            )));
        }
        last = d.column;
    }
    Ok(())
}

/// Version 1 → 2: when the registry has the console devices, every
/// instrument gets a channel, in rack order, exactly as `ConsoleCommand::AddChannel` wires
/// it (so the master is created with the first one). Instruments that cannot get one (no
/// audio output) are skipped. Without console devices the console stays empty.
fn migrate_v1(doc: RackDoc, registry: &Registry) -> RackDoc {
    if !crate::console_cmd::console_available(registry) {
        return doc;
    }
    let instruments: Vec<DeviceKey> = doc
        .devices
        .iter()
        .filter(|d| crate::console_cmd::is_instrument(registry, &d.type_id))
        .map(|d| d.key.clone())
        .collect();
    let mut document = crate::Document::new(doc);
    for source in instruments {
        // A rejected channel leaves the document unchanged.
        let _ = document.apply(
            crate::Command::Console(crate::ConsoleCommand::AddChannel { source }),
            registry,
        );
    }
    document.doc().clone()
}

/// Reads a device's `"zones"` array (shape only; the pool and ranges are checked by
/// [`check_device_zones`] once the pool is loaded).
pub(crate) fn load_zones(
    v: &Value,
    path: &JsonPath,
    with_reverse: bool,
) -> Result<Vec<ZoneDoc>, ModelError> {
    let fields = if with_reverse {
        ZONE_FIELDS_V6
    } else {
        ZONE_FIELDS
    };
    let mut zones = Vec::new();
    for (i, item) in array(v, path)?.iter().enumerate() {
        let p = path.index(i);
        let z = object(item, &p, fields, fields)?;
        let u = |name: &str, max: u64| uint(&z[name], &p.field(name), max);
        let f = |name: &str| number(&z[name], &p.field(name)).map(|x| x as f32);
        let transpose_path = p.field("transpose");
        let transpose = int(&z["transpose"], &transpose_path)?;
        let transpose = i8::try_from(transpose)
            .map_err(|_| transpose_path.error(format!("{transpose} is outside -128..=127")))?;
        let loop_mode = match string(&z["loop_mode"], &p.field("loop_mode"))? {
            "off" => LoopMode::Off,
            "forward" => LoopMode::Forward,
            "ping_pong" => LoopMode::PingPong,
            other => {
                return Err(p.field("loop_mode").error(format!(
                    "expected \"off\", \"forward\" or \"ping_pong\", found {other:?}"
                )));
            }
        };
        let trigger = match string(&z["trigger"], &p.field("trigger"))? {
            "gate" => TriggerMode::Gate,
            "one_shot" => TriggerMode::OneShot,
            other => {
                return Err(p.field("trigger").error(format!(
                    "expected \"gate\" or \"one_shot\", found {other:?}"
                )));
            }
        };
        zones.push(ZoneDoc {
            asset: string(&z["asset"], &p.field("asset"))?.to_owned(),
            key_lo: u("key_lo", 255)? as u8,
            key_hi: u("key_hi", 255)? as u8,
            vel_lo: u("vel_lo", 255)? as u8,
            vel_hi: u("vel_hi", 255)? as u8,
            root_key: u("root_key", 255)? as u8,
            transpose,
            tune_cents: f("tune_cents")?,
            gain_db: f("gain_db")?,
            pan: f("pan")?,
            start: u("start", MAX_AUDIO_FRAMES)?,
            end: u("end", MAX_AUDIO_FRAMES)?,
            loop_mode,
            loop_start: u("loop_start", MAX_AUDIO_FRAMES)?,
            loop_end: u("loop_end", MAX_AUDIO_FRAMES)?,
            crossfade: u("crossfade", MAX_AUDIO_FRAMES)?,
            trigger,
            reverse: if with_reverse {
                boolean(&z["reverse"], &p.field("reverse"))?
            } else {
                false
            },
        });
    }
    Ok(zones)
}

/// An unsigned integer in `lo..=hi`.
fn ranged(v: &Value, path: &JsonPath, lo: u64, hi: u64) -> Result<u8, ModelError> {
    let n = uint(v, path, u64::MAX)?;
    if !(lo..=hi).contains(&n) {
        return Err(path.error(format!("{n} is outside {lo}..={hi}")));
    }
    Ok(n as u8)
}

/// Reads a device's `"steps"` object (v7). Whether the device may have one is checked by the
/// caller, which knows the registry.
pub(crate) fn load_steps(v: &Value, path: &JsonPath) -> Result<StepSeqDoc, ModelError> {
    let o = object(v, path, PATTERNS_FIELDS, PATTERNS_FIELDS)?;
    let list_path = path.field("patterns");
    let list = array(&o["patterns"], &list_path)?;
    if !(1..=MAX_PATTERNS).contains(&list.len()) {
        return Err(list_path.error(format!(
            "{} patterns is outside 1..={MAX_PATTERNS}",
            list.len()
        )));
    }
    let mut patterns = Vec::with_capacity(list.len());
    for (i, item) in list.iter().enumerate() {
        let p = list_path.index(i);
        let o = object(item, &p, PATTERN_FIELDS, PATTERN_FIELDS)?;
        let name_path = p.field("name");
        let name = string(&o["name"], &name_path)?;
        if name.chars().count() > MAX_NAME_CHARS {
            return Err(name_path.error(format!("longer than {MAX_NAME_CHARS} characters")));
        }
        let resolution_path = p.field("resolution");
        let resolution_name = string(&o["resolution"], &resolution_path)?;
        let resolution = SeqResolution::from_name(resolution_name).ok_or_else(|| {
            resolution_path.error(format!(
                "unknown resolution {resolution_name:?}; expected one of: {}",
                SeqResolution::ALL.map(SeqResolution::name).join(", ")
            ))
        })?;
        let length = ranged(&o["length"], &p.field("length"), 1, MAX_STEPS as u64)?;
        let swing = ranged(&o["swing"], &p.field("swing"), 0, 75)?;
        let steps_path = p.field("steps");
        let raw = array(&o["steps"], &steps_path)?;
        if raw.len() != MAX_STEPS {
            return Err(steps_path.error(format!(
                "{} entries, exactly {MAX_STEPS} are required",
                raw.len()
            )));
        }
        let mut steps = Vec::with_capacity(MAX_STEPS);
        for (j, item) in raw.iter().enumerate() {
            let sp = steps_path.index(j);
            let s = object(item, &sp, STEP_FIELDS, STEP_FIELDS)?;
            steps.push(StepDoc {
                on: boolean(&s["on"], &sp.field("on"))?,
                key: ranged(&s["key"], &sp.field("key"), 0, 127)?,
                velocity: ranged(&s["velocity"], &sp.field("velocity"), 1, 127)?,
                gate: ranged(&s["gate"], &sp.field("gate"), 1, 100)?,
                prob: ranged(&s["prob"], &sp.field("prob"), 0, 100)?,
                tie: boolean(&s["tie"], &sp.field("tie"))?,
            });
        }
        patterns.push(PatternDoc {
            name: name.to_owned(),
            length,
            resolution,
            swing,
            steps,
        });
    }
    Ok(StepSeqDoc { patterns })
}

/// Validates every device's zones (inside containers too) against the loaded pool and the
/// registry, as `Command::SetZones` does.
fn check_device_zones(
    doc: &RackDoc,
    path: &JsonPath,
    registry: &Registry,
) -> Result<(), ModelError> {
    check_zones_in(&doc.devices, doc, path, registry)
}

fn check_zones_in(
    devices: &[crate::RackDevice],
    doc: &RackDoc,
    path: &JsonPath,
    registry: &Registry,
) -> Result<(), ModelError> {
    for (i, d) in devices.iter().enumerate() {
        if !d.zones.is_empty() {
            let p = path.index(i).field("zones");
            if !registry.takes_samples(&d.type_id) {
                return Err(p.error(format!("{} does not take samples", d.type_id)));
            }
            crate::zones::check_zones(&doc.assets, &d.zones)
                .map_err(|(j, m)| p.index(j).error(m))?;
        }
        if let Some(inner) = &d.inner {
            let p = path.index(i).field("inner").field("devices");
            check_zones_in(&inner.devices, doc, &p, registry)?;
        }
    }
    Ok(())
}

/// A device keyed `boundary` in a file older than version 8: the key is reserved for a
/// container's inside edge, so the device becomes `boundary-1` (or the first free
/// `boundary-N`) and every reference to it follows: cables, console channels, sources, master
/// and returns, song tracks. Returns whether anything was renamed; the warning says what.
fn rename_boundary(value: &mut Value, warnings: &mut Vec<String>) -> bool {
    let key_of = |d: &Value| d.get("id").and_then(Value::as_str).map(str::to_owned);
    let Some(devices) = value.get("devices").and_then(Value::as_array) else {
        return false;
    };
    let ids: Vec<String> = devices.iter().filter_map(key_of).collect();
    if !ids.iter().any(|k| k == crate::BOUNDARY_KEY) {
        return false;
    }
    let new = (1u32..)
        .map(|n| format!("boundary-{n}"))
        .find(|k| !ids.contains(k))
        .expect("a free key exists");
    let rename = |v: &mut Value| {
        if v.as_str() == Some(crate::BOUNDARY_KEY) {
            *v = Value::from(new.as_str());
        }
    };
    let rename_end = |v: &mut Value| {
        if let Some(text) = v.as_str()
            && let Some(socket) = text.strip_prefix("boundary:")
        {
            *v = Value::from(format!("{new}:{socket}"));
        }
    };
    if let Some(list) = value.get_mut("devices").and_then(Value::as_array_mut) {
        for d in list {
            if let Some(id) = d.get_mut("id") {
                rename(id);
            }
        }
    }
    if let Some(list) = value.get_mut("cables").and_then(Value::as_array_mut) {
        for c in list {
            for end in ["from", "to"] {
                if let Some(v) = c.get_mut(end) {
                    rename_end(v);
                }
            }
        }
    }
    if let Some(console) = value.get_mut("console") {
        if let Some(list) = console.get_mut("channels").and_then(Value::as_array_mut) {
            for c in list {
                for field in ["channel", "source"] {
                    if let Some(v) = c.get_mut(field) {
                        rename(v);
                    }
                }
            }
        }
        if let Some(v) = console.get_mut("master") {
            rename(v);
        }
        if let Some(list) = console.get_mut("returns").and_then(Value::as_array_mut) {
            list.iter_mut().for_each(rename);
        }
    }
    if let Some(list) = value
        .get_mut("song")
        .and_then(|s| s.get_mut("tracks"))
        .and_then(Value::as_array_mut)
    {
        for t in list {
            if let Some(v) = t.get_mut("device") {
                rename(v);
            }
        }
    }
    warnings.push(format!(
        "the device \"boundary\" was renamed \"{new}\": the key is reserved for the inside edge of a container"
    ));
    true
}

/// Reads the sample pool: asset ids (64 lowercase hex digits) with their audio format.
fn load_assets(v: &Value, path: &JsonPath) -> Result<BTreeMap<String, AssetMeta>, ModelError> {
    let map = v
        .as_object()
        .ok_or_else(|| path.error(format!("expected an object, found {}", describe(v))))?;
    let mut assets = BTreeMap::new();
    for (hash, item) in map {
        let p = path.field(hash);
        if !is_valid_asset_hash(hash) {
            return Err(p.error("asset ids are 64 lowercase hex digits"));
        }
        let a = object(
            item,
            &p,
            ASSET_FIELDS,
            &["channels", "sample_rate", "frames"],
        )?;
        let channels = uint(&a["channels"], &p.field("channels"), u64::from(u16::MAX))? as u16;
        if channels == 0 {
            return Err(p.field("channels").error("an asset has at least 1 channel"));
        }
        let sample_rate = uint(
            &a["sample_rate"],
            &p.field("sample_rate"),
            u64::from(u32::MAX),
        )? as u32;
        if sample_rate == 0 {
            return Err(p
                .field("sample_rate")
                .error("the sample rate must be above 0"));
        }
        let frames = uint(&a["frames"], &p.field("frames"), MAX_AUDIO_FRAMES)?;
        if frames == 0 {
            return Err(p.field("frames").error("an asset has at least 1 frame"));
        }
        let original_name = match a.get("name") {
            Some(n) => string(n, &p.field("name"))?.to_owned(),
            None => String::new(),
        };
        assets.insert(
            hash.clone(),
            AssetMeta {
                channels,
                sample_rate,
                frames,
                original_name,
            },
        );
    }
    Ok(assets)
}

/// Reads and validates the console section against the loaded rack (the invariants
/// are listed in [`crate::console`]).
fn load_console(v: &Value, path: &JsonPath, doc: &RackDoc) -> Result<Console, ModelError> {
    let obj = object(v, path, CONSOLE_FIELDS, CONSOLE_FIELDS)?;
    // `null`, or the key of an existing device.
    let device_ref = |v: &Value, p: &JsonPath| -> Result<Option<DeviceKey>, ModelError> {
        if v.is_null() {
            return Ok(None);
        }
        let key = DeviceKey(string(v, p)?.to_owned());
        if doc.device(&key).is_none() {
            return Err(p.error(format!("no device has id {:?}", key.0)));
        }
        Ok(Some(key))
    };
    let type_of = |k: &DeviceKey| doc.device(k).map_or("", |d| d.type_id.as_str());
    let mut console = Console::default();

    let channels_path = path.field("channels");
    for (i, item) in array(&obj["channels"], &channels_path)?.iter().enumerate() {
        let p = channels_path.index(i);
        let ch = object(item, &p, CHANNEL_FIELDS, CHANNEL_FIELDS)?;
        let ch_path = p.field("channel");
        let channel = device_ref(&ch["channel"], &ch_path)?
            .ok_or_else(|| ch_path.error("expected a device id, found null"))?;
        if type_of(&channel) != CHANNEL_TYPE {
            return Err(ch_path.error(format!(
                "{:?} is a {}, not an {CHANNEL_TYPE}",
                channel.0,
                type_of(&channel)
            )));
        }
        if console.channel_index(&channel).is_some() {
            return Err(ch_path.error(format!("{:?} is listed twice", channel.0)));
        }
        let src_path = p.field("source");
        let source = device_ref(&ch["source"], &src_path)?;
        if let Some(s) = &source
            && is_console_type(type_of(s))
        {
            return Err(src_path.error(format!(
                "{:?} is a console device; it cannot feed a channel",
                s.0
            )));
        }
        console.channels.push(ConsoleChannel { channel, source });
    }

    let master_path = path.field("master");
    console.master = device_ref(&obj["master"], &master_path)?;
    if let Some(m) = &console.master
        && type_of(m) != MASTER_TYPE
    {
        return Err(master_path.error(format!(
            "{:?} is a {}, not an {MASTER_TYPE}",
            m.0,
            type_of(m)
        )));
    }

    let returns_path = path.field("returns");
    let returns = array(&obj["returns"], &returns_path)?;
    if returns.len() != FX_SLOTS {
        return Err(returns_path.error(format!(
            "expected {FX_SLOTS} return slots, found {}",
            returns.len()
        )));
    }
    for (slot, item) in returns.iter().enumerate() {
        let p = returns_path.index(slot);
        let Some(effect) = device_ref(item, &p)? else {
            continue;
        };
        if console.master.is_none() {
            return Err(p.error("the console has no master, so it has no returns"));
        }
        if is_console_type(type_of(&effect)) {
            return Err(p.error(format!(
                "{:?} is a console device, not a return effect",
                effect.0
            )));
        }
        if let Some(first) = console.return_slot(&effect) {
            return Err(p.error(format!("{:?} is already on return {}", effect.0, first + 1)));
        }
        if let Some(c) = console.channel_of(&effect) {
            return Err(p.error(format!(
                "{:?} feeds channel {:?}; it cannot also be a return effect",
                effect.0, c.channel.0
            )));
        }
        console.returns[slot] = Some(effect);
    }
    console.compensation = boolean(&obj["compensation"], &path.field("compensation"))?;
    Ok(console)
}

fn load_song(
    v: &Value,
    path: &JsonPath,
    (doc, registry): (&RackDoc, &Registry),
    version: u64,
) -> Result<Song, ModelError> {
    let obj = object(v, path, SONG_FIELDS, SONG_FIELDS)?;

    let tempo_path = path.field("tempo");
    let mut tempo = Vec::new();
    for (i, item) in array(&obj["tempo"], &tempo_path)?.iter().enumerate() {
        let p = tempo_path.index(i);
        let e = object(item, &p, &["tick", "bpm"], &["tick", "bpm"])?;
        let tick_path = p.field("tick");
        let t = tick(&e["tick"], &tick_path)?;
        if i == 0 && t != 0 {
            return Err(
                tick_path.error(format!("the first tempo event must be at tick 0, not {t}"))
            );
        }
        if let Some(prev) = tempo.last().map(|e: &TempoEvent| e.tick)
            && t <= prev
        {
            return Err(tick_path.error(format!("ticks must increase strictly ({t} after {prev})")));
        }
        let bpm_path = p.field("bpm");
        let bpm = number(&e["bpm"], &bpm_path)?;
        if !(TEMPO_MIN..=TEMPO_MAX).contains(&bpm) {
            return Err(bpm_path.error(format!("{bpm} BPM is outside 20..=999")));
        }
        if i == 0 && bpm.to_bits() != doc.tempo_bpm.to_bits() {
            return Err(bpm_path.error(format!("{bpm} must equal tempo_bpm ({})", doc.tempo_bpm)));
        }
        tempo.push(TempoEvent { tick: t, bpm });
    }
    if tempo.is_empty() {
        return Err(tempo_path.error("the tempo map needs at least one event"));
    }

    let meter_path = path.field("meter");
    let meter = tuple(&obj["meter"], &meter_path, 2, "[beats_per_bar, beat_unit]")?;
    let beats_per_bar = uint(&meter[0], &meter_path.index(0), 255)? as u8;
    let beat_unit = uint(&meter[1], &meter_path.index(1), 255)? as u8;
    check_meter(beats_per_bar, 4).map_err(|m| meter_path.index(0).error(m))?;
    check_meter(1, beat_unit).map_err(|m| meter_path.index(1).error(m))?;

    let loop_path = path.field("loop");
    let lp = object(
        &obj["loop"],
        &loop_path,
        &["start", "end", "enabled"],
        &["start", "end", "enabled"],
    )?;
    let loop_range = (
        int(&lp["start"], &loop_path.field("start"))?,
        int(&lp["end"], &loop_path.field("end"))?,
    );
    check_loop(loop_range.0, loop_range.1).map_err(|m| loop_path.error(m))?;
    let loop_enabled = boolean(&lp["enabled"], &loop_path.field("enabled"))?;

    let end_path = path.field("end");
    let end = int(&obj["end"], &end_path)?;
    check_end(end).map_err(|m| end_path.error(m))?;
    let next_clip_id = uint(
        &obj["next_clip_id"],
        &path.field("next_clip_id"),
        u64::MAX - 1,
    )?;

    let mut song = Song {
        tracks: Vec::new(),
        tempo,
        beats_per_bar,
        beat_unit,
        loop_range,
        loop_enabled,
        end,
        next_clip_id,
    };
    let tracks_path = path.field("tracks");
    let mut track_first: BTreeMap<DeviceKey, JsonPath> = BTreeMap::new();
    let mut clip_first: BTreeMap<u64, JsonPath> = BTreeMap::new();
    for (i, item) in array(&obj["tracks"], &tracks_path)?.iter().enumerate() {
        let p = tracks_path.index(i);
        let track = load_track(item, &p, (doc, registry), version, &song, &mut clip_first)?;
        if let Some(first) = track_first.get(&track.device) {
            return Err(p.field("device").error(format!(
                "{:?} already has a track ({first})",
                track.device.0
            )));
        }
        track_first.insert(track.device.clone(), p);
        song.tracks.push(track);
    }
    Ok(song)
}

fn load_track(
    v: &Value,
    path: &JsonPath,
    (doc, registry): (&RackDoc, &Registry),
    version: u64,
    song: &Song,
    clip_first: &mut BTreeMap<u64, JsonPath>,
) -> Result<Track, ModelError> {
    let obj = object(v, path, TRACK_FIELDS, &["device", "lanes"])?;
    let device_path = path.field("device");
    let device = DeviceKey(string(&obj["device"], &device_path)?.to_owned());
    let dev = doc
        .device(&device)
        .ok_or_else(|| device_path.error(format!("no device has id {:?}", device.0)))?;
    let desc = rules::descriptor(registry, &dev.type_id)?;
    let audio_track = is_audio_track_type(desc);
    if !audio_track && !has_notes_input(desc) {
        return Err(device_path.error(format!(
            "{:?} ({}) has no Notes input, so it cannot have a track",
            device.0, dev.type_id
        )));
    }
    let lanes_path = path.field("lanes");
    let mut lanes: Vec<Lane> = Vec::new();
    for (i, item) in array(&obj["lanes"], &lanes_path)?.iter().enumerate() {
        let p = lanes_path.index(i);
        let kind_path = p.field("kind");
        let kind_text = item
            .as_object()
            .map(|o| o.get("kind").unwrap_or(&Value::Null))
            .map(|k| string(k, &kind_path))
            .transpose()?;
        if audio_track && i > 0 {
            return Err(p.error("an audio track has a single lane"));
        }
        let want = if audio_track {
            "audio"
        } else if i == 0 {
            "notes"
        } else {
            "automation"
        };
        if let Some(k) = kind_text
            && k != want
        {
            return Err(kind_path.error(if audio_track {
                format!("the lane of an audio track must be \"audio\", found {k:?}")
            } else if i == 0 {
                format!("the first lane must be \"notes\", found {k:?}")
            } else {
                format!("lanes after the first must be \"automation\", found {k:?}")
            }));
        }
        let (fields, clip_fields) = if audio_track {
            (AUDIO_LANE_FIELDS, AUDIO_CLIP_FIELDS)
        } else if i == 0 {
            (
                NOTES_LANE_FIELDS,
                if version >= 5 {
                    NOTE_CLIP_FIELDS_V5
                } else {
                    NOTE_CLIP_FIELDS
                },
            )
        } else {
            (AUTOMATION_LANE_FIELDS, AUTOMATION_CLIP_FIELDS)
        };
        let lane = object(item, &p, fields, fields)?;
        let kind = if audio_track {
            LaneKind::Audio
        } else if i == 0 {
            LaneKind::Notes
        } else {
            let param_path = p.field("param");
            let param = string(&lane["param"], &param_path)?;
            let dead = crate::plugin::is_dead_plugin_param(desc, param);
            if (desc.param_by_key(param).is_none() && !dead)
                || crate::console::is_internal_param(param)
            {
                return Err(param_path.error(format!("{} has no param {param:?}", desc.type_id)));
            }
            if lanes
                .iter()
                .any(|l| matches!(&l.kind, LaneKind::Automation(q) if q == param))
            {
                return Err(param_path.error(format!("duplicate automation lane for {param:?}")));
            }
            LaneKind::Automation(param.to_owned())
        };
        let clips_path = p.field("clips");
        let mut clips = Vec::new();
        for (j, item) in array(&lane["clips"], &clips_path)?.iter().enumerate() {
            let cp = clips_path.index(j);
            let clip = load_clip(
                item,
                &cp,
                (clip_fields, version),
                &kind,
                (doc, desc),
                song,
                clip_first,
            )?;
            clip_first.insert(clip.id.0, cp.field("id"));
            clips.push(clip);
        }
        clips.sort_by_key(|c: &Clip| (c.start, c.id));
        lanes.push(Lane { kind, clips });
    }
    if lanes.is_empty() {
        return Err(lanes_path.error("a track needs its note lane"));
    }
    Ok(Track {
        device,
        muted: flag(obj, path, "muted")?,
        solo: flag(obj, path, "solo")?,
        armed: flag(obj, path, "armed")?,
        lanes,
    })
}

fn load_clip(
    v: &Value,
    path: &JsonPath,
    (fields, version): (&[&str], u64),
    kind: &LaneKind,
    (doc, desc): (&RackDoc, &atomik_device_sdk::DeviceDescriptor),
    song: &Song,
    clip_first: &BTreeMap<u64, JsonPath>,
) -> Result<Clip, ModelError> {
    let content_field = fields[fields.len() - 1];
    let obj = object(v, path, fields, &["id", "start", "length", content_field])?;
    let id_path = path.field("id");
    let id = uint(&obj["id"], &id_path, u64::MAX)?;
    if let Some(first) = clip_first.get(&id) {
        return Err(id_path.error(format!("duplicate clip id {id} (first used at {first})")));
    }
    if id >= song.next_clip_id {
        return Err(id_path.error(format!(
            "clip id {id} is not below next_clip_id ({})",
            song.next_clip_id
        )));
    }
    let start = tick(&obj["start"], &path.field("start"))?;
    let length = length(&obj["length"], &path.field("length"))?;
    let offset = match obj.get("offset") {
        Some(v) => tick(v, &path.field("offset"))?,
        None => 0,
    };
    let loop_path = path.field("loop_length");
    let loop_length = match obj.get("loop_length") {
        None | Some(Value::Null) => None,
        Some(v) => {
            let n = self::length(v, &loop_path)?;
            if length > n * MAX_LOOP_REPEATS {
                return Err(loop_path.error(format!(
                    "the clip ({length} ticks) holds more than {MAX_LOOP_REPEATS} repetitions of {n}"
                )));
            }
            Some(n)
        }
    };
    if kind.is_audio() {
        if offset != 0 {
            return Err(path
                .field("offset")
                .error("an audio clip has no content offset (use audio.offset)"));
        }
        if loop_length.is_some() {
            return Err(loop_path.error("an audio clip cannot loop"));
        }
    }
    let muted = flag(obj, path, "muted")?;
    let content_path = path.field(content_field);
    if kind.is_audio() {
        let audio = load_audio(&obj["audio"], &content_path, doc)?;
        return Ok(Clip {
            id: ClipId(id),
            start,
            length,
            offset,
            loop_length,
            muted,
            grid: None,
            content: ClipContent::Audio(audio),
        });
    }
    let items = array(&obj[content_field], &content_path)?;
    let content = match kind {
        LaneKind::Audio => unreachable!("audio clips return above"),
        LaneKind::Notes => {
            let mut notes: Vec<Note> = Vec::with_capacity(items.len());
            for (i, item) in items.iter().enumerate() {
                let p = content_path.index(i);
                let n = note_tuple(item, &p, version)?;
                let note = Note {
                    muted: match n.get(4) {
                        Some(m) => boolean(m, &p.index(4))?,
                        None => false,
                    },
                    ..Note::new(
                        tick(&n[0], &p.index(0))?,
                        self::length(&n[1], &p.index(1))?,
                        uint(&n[2], &p.index(2), 127)? as u8,
                        number(&n[3], &p.index(3))? as f32,
                    )
                };
                check_note(note.tick, note.length, note.key, note.velocity)
                    .map_err(|m| p.index(3).error(m))?;
                if let Some(prev) = notes.last()
                    && (prev.tick, prev.key) >= (note.tick, note.key)
                {
                    return Err(p.error("notes must be sorted by (tick, key) without duplicates"));
                }
                notes.push(note);
            }
            ClipContent::Notes(notes)
        }
        LaneKind::Automation(param) => {
            // `None`: a dead plugin param, whose values are kept as they are.
            let pd = desc.param_by_key(param);
            let mut points: Vec<AutomationPoint> = Vec::with_capacity(items.len());
            for (i, item) in items.iter().enumerate() {
                let p = content_path.index(i);
                let a = tuple(item, &p, 3, "a point [tick, value, curve]")?;
                let t = tick(&a[0], &p.index(0))?;
                let value = number(&a[1], &p.index(1))? as f32;
                let value = match pd {
                    Some(pd) => pd.clamp(value),
                    None if value.is_finite() => value,
                    None => return Err(p.index(1).error(format!("{value} is out of range"))),
                };
                let curve = match string(&a[2], &p.index(2))? {
                    "linear" => Curve::Linear,
                    "step" => Curve::Step,
                    other => {
                        return Err(p
                            .index(2)
                            .error(format!("expected \"linear\" or \"step\", found {other:?}")));
                    }
                };
                if let Some(prev) = points.last()
                    && prev.tick >= t
                {
                    return Err(p.error("points must be sorted by tick without duplicates"));
                }
                points.push(AutomationPoint {
                    tick: t,
                    value,
                    curve,
                });
            }
            ClipContent::Automation(points)
        }
    };
    Ok(Clip {
        id: ClipId(id),
        start,
        length,
        offset,
        loop_length,
        muted,
        grid: match obj.get("grid") {
            Some(g) => Some(load_grid(g, &path.field("grid"))?),
            None => None,
        },
        content,
    })
}

/// Reads and validates a note clip's `"grid"` object.
fn load_grid(v: &Value, path: &JsonPath) -> Result<StepGrid, ModelError> {
    let g = object(v, path, GRID_FIELDS, GRID_FIELDS)?;
    let rp = path.field("resolution");
    let resolution = match string(&g["resolution"], &rp)? {
        "eighth" => StepResolution::Eighth,
        "sixteenth" => StepResolution::Sixteenth,
        "thirty_second" => StepResolution::ThirtySecond,
        other => {
            return Err(rp.error(format!(
                "expected \"eighth\", \"sixteenth\" or \"thirty_second\", found {other:?}"
            )));
        }
    };
    let grid = StepGrid {
        resolution,
        steps: uint(&g["steps"], &path.field("steps"), u64::from(u16::MAX))? as u16,
        swing: uint(&g["swing"], &path.field("swing"), 255)? as u8,
    };
    grid.validate().map_err(|m| path.error(m))?;
    Ok(grid)
}

/// Reads and validates an audio clip's `"audio"` object against the pool.
fn load_audio(v: &Value, path: &JsonPath, doc: &RackDoc) -> Result<AudioClipContent, ModelError> {
    let a = object(v, path, AUDIO_FIELDS, &["asset", "offset"])?;
    let asset_path = path.field("asset");
    let asset = string(&a["asset"], &asset_path)?;
    let meta = doc
        .assets
        .get(asset)
        .ok_or_else(|| asset_path.error(format!("asset {asset:?} is not in the assets pool")))?;
    let offset_path = path.field("offset");
    let source_offset = uint(&a["offset"], &offset_path, MAX_AUDIO_FRAMES)?;
    if source_offset > meta.frames {
        return Err(offset_path.error(format!(
            "{source_offset} is beyond the asset's {} frames",
            meta.frames
        )));
    }
    let gain_db = match a.get("gain_db") {
        Some(g) => {
            let gp = path.field("gain_db");
            let g = number(g, &gp)? as f32;
            if !(AUDIO_GAIN_DB_MIN..=AUDIO_GAIN_DB_MAX).contains(&g) {
                return Err(gp.error(format!(
                    "{g} dB is outside {AUDIO_GAIN_DB_MIN}..={AUDIO_GAIN_DB_MAX}"
                )));
            }
            g
        }
        None => 0.0,
    };
    let frames = |name: &str| -> Result<u64, ModelError> {
        a.get(name)
            .map_or(Ok(0), |v| uint(v, &path.field(name), MAX_AUDIO_FRAMES))
    };
    let fade_curve = match a.get("fade_curve") {
        None => FadeCurve::Linear,
        Some(c) => {
            let cp = path.field("fade_curve");
            match string(c, &cp)? {
                "linear" => FadeCurve::Linear,
                "equal_power" => FadeCurve::EqualPower,
                other => {
                    return Err(cp.error(format!(
                        "expected \"linear\" or \"equal_power\", found {other:?}"
                    )));
                }
            }
        }
    };
    Ok(AudioClipContent {
        asset: asset.to_owned(),
        source_offset,
        gain_db,
        fade_in: frames("fade_in")?,
        fade_out: frames("fade_out")?,
        fade_curve,
    })
}

/// Writes a project. Field, device, track, lane and clip order are fixed, so the output is
/// deterministic and canonical.
pub(crate) fn save(doc: &RackDoc) -> String {
    let mut out = String::new();
    let w = &mut out;
    // Writing to a String cannot fail.
    let _ = writeln!(w, "{{");
    let _ = writeln!(w, "  \"format\": {},", json_str(PROJECT_FORMAT));
    let _ = writeln!(w, "  \"version\": {PROJECT_VERSION},");
    let _ = writeln!(w, "  \"sample_rate\": {},", doc.sample_rate);
    let _ = writeln!(w, "  \"tempo_bpm\": {},", f64_text(doc.tempo_bpm));
    let _ = writeln!(w, "  \"seed\": {},", doc.seed);
    let _ = writeln!(w, "  \"output_channels\": {},", doc.output_channels);
    let _ = writeln!(w, "  \"input_channels\": {},", doc.input_channels);
    // Optional, like the device `column`: a single-column rack (the default) writes neither.
    if doc.columns != 1 {
        let _ = writeln!(w, "  \"rack_columns\": {},", doc.columns);
    }
    write_devices_and_cables(w, doc, true);
    let _ = writeln!(w, ",");
    write_console(w, &doc.console);
    write_assets(w, &doc.assets);
    write_song(w, &doc.song);
    let _ = writeln!(w, "}}");
    out
}

/// `items` joined as indented lines inside `[` … `]`, or `[]` when empty.
fn list(w: &mut String, indent: &str, items: impl IntoIterator<Item = String>) {
    let mut any = false;
    for (i, item) in items.into_iter().enumerate() {
        let sep = if i == 0 { "[" } else { "," };
        let _ = write!(w, "{sep}\n{indent}  {item}");
        any = true;
    }
    let _ = write!(
        w,
        "{}",
        if any {
            format!("\n{indent}]")
        } else {
            "[]".into()
        }
    );
}

/// A device's zones as a JSON array, one zone per line; `p` is the indent of the device's
/// fields (6 at the top level).
pub(crate) fn write_zones(w: &mut String, zones: &[ZoneDoc], p: &str) {
    list(
        w,
        p,
        zones.iter().map(|z| {
            let loop_mode = match z.loop_mode {
                LoopMode::Off => "off",
                LoopMode::Forward => "forward",
                LoopMode::PingPong => "ping_pong",
            };
            let trigger = match z.trigger {
                TriggerMode::Gate => "gate",
                TriggerMode::OneShot => "one_shot",
            };
            format!(
                "{{\"asset\": {}, \"key_lo\": {}, \"key_hi\": {}, \"vel_lo\": {}, \"vel_hi\": {}, \"root_key\": {}, \"transpose\": {}, \"tune_cents\": {}, \"gain_db\": {}, \"pan\": {}, \"start\": {}, \"end\": {}, \"loop_mode\": \"{loop_mode}\", \"loop_start\": {}, \"loop_end\": {}, \"crossfade\": {}, \"trigger\": \"{trigger}\", \"reverse\": {}}}",
                json_str(&z.asset),
                z.key_lo,
                z.key_hi,
                z.vel_lo,
                z.vel_hi,
                z.root_key,
                z.transpose,
                f32_text(z.tune_cents),
                f32_text(z.gain_db),
                f32_text(z.pan),
                z.start,
                z.end,
                z.loop_start,
                z.loop_end,
                z.crossfade,
                z.reverse
            )
        }),
    );
}

/// A device's steps as a JSON object, one step per line; `ind` is the indent of the device's
/// fields (6 at the top level).
pub(crate) fn write_steps(w: &mut String, steps: &StepSeqDoc, ind: &str) {
    let (a, b, c) = (
        format!("{ind}  "),
        format!("{ind}    "),
        format!("{ind}      "),
    );
    let _ = write!(w, "{{\n{a}\"patterns\": [");
    for (i, p) in steps.patterns.iter().enumerate() {
        let sep = if i == 0 { "" } else { "," };
        let _ = write!(
            w,
            "{sep}\n{b}{{\n{c}\"name\": {}, \"length\": {}, \"resolution\": \"{}\", \"swing\": {},\n{c}\"steps\": ",
            json_str(&p.name),
            p.length,
            p.resolution.name(),
            p.swing
        );
        list(
            w,
            &c,
            p.steps.iter().map(|s| {
                format!(
                    "{{\"on\": {}, \"key\": {}, \"velocity\": {}, \"gate\": {}, \"prob\": {}, \"tie\": {}}}",
                    s.on, s.key, s.velocity, s.gate, s.prob, s.tie
                )
            }),
        );
        let _ = write!(w, "\n{b}}}");
    }
    let _ = write!(w, "\n{a}]\n{ind}}}");
}

fn key_or_null(k: Option<&DeviceKey>) -> String {
    k.map_or_else(|| "null".to_owned(), |k| json_str(&k.0))
}

fn write_console(w: &mut String, console: &Console) {
    let _ = writeln!(w, "  \"console\": {{");
    let _ = write!(w, "    \"channels\": ");
    list(
        w,
        "    ",
        console.channels.iter().map(|c| {
            format!(
                "{{\"channel\": {}, \"source\": {}}}",
                json_str(&c.channel.0),
                key_or_null(c.source.as_ref())
            )
        }),
    );
    let _ = writeln!(w, ",");
    let _ = writeln!(
        w,
        "    \"master\": {},",
        key_or_null(console.master.as_ref())
    );
    let returns: Vec<String> = console
        .returns
        .iter()
        .map(|r| key_or_null(r.as_ref()))
        .collect();
    let _ = writeln!(w, "    \"returns\": [{}],", returns.join(", "));
    let _ = writeln!(w, "    \"compensation\": {}", console.compensation);
    let _ = writeln!(w, "  }},");
}

fn write_assets(w: &mut String, assets: &BTreeMap<String, AssetMeta>) {
    let _ = write!(w, "  \"assets\": {{");
    for (i, (hash, m)) in assets.iter().enumerate() {
        let sep = if i == 0 { "" } else { "," };
        let _ = write!(
            w,
            "{sep}\n    {}: {{\"channels\": {}, \"sample_rate\": {}, \"frames\": {}, \"name\": {}}}",
            json_str(hash),
            m.channels,
            m.sample_rate,
            m.frames,
            json_str(&m.original_name)
        );
    }
    let _ = writeln!(w, "{}}},", if assets.is_empty() { "" } else { "\n  " });
}

fn write_song(w: &mut String, song: &Song) {
    let _ = writeln!(w, "  \"song\": {{");
    let _ = write!(w, "    \"tempo\": ");
    list(
        w,
        "    ",
        song.tempo
            .iter()
            .map(|e| format!("{{\"tick\": {}, \"bpm\": {}}}", e.tick, f64_text(e.bpm))),
    );
    let _ = writeln!(w, ",");
    let _ = writeln!(
        w,
        "    \"meter\": [{}, {}],",
        song.beats_per_bar, song.beat_unit
    );
    let _ = writeln!(
        w,
        "    \"loop\": {{\"start\": {}, \"end\": {}, \"enabled\": {}}},",
        song.loop_range.0, song.loop_range.1, song.loop_enabled
    );
    let _ = writeln!(w, "    \"end\": {},", song.end);
    let _ = writeln!(w, "    \"next_clip_id\": {},", song.next_clip_id);
    let _ = write!(w, "    \"tracks\": [");
    for (i, t) in song.tracks.iter().enumerate() {
        let sep = if i == 0 { "" } else { "," };
        let _ = write!(
            w,
            "{sep}\n      {{\n        \"device\": {}, \"muted\": {}, \"solo\": {}, \"armed\": {},\n        \"lanes\": [",
            json_str(&t.device.0),
            t.muted,
            t.solo,
            t.armed
        );
        for (j, lane) in t.lanes.iter().enumerate() {
            let sep = if j == 0 { "" } else { "," };
            let kind = match &lane.kind {
                LaneKind::Notes => "\"kind\": \"notes\"".to_owned(),
                LaneKind::Audio => "\"kind\": \"audio\"".to_owned(),
                LaneKind::Automation(p) => {
                    format!("\"kind\": \"automation\", \"param\": {}", json_str(p))
                }
            };
            let _ = write!(
                w,
                "{sep}\n          {{\n            {kind},\n            \"clips\": ["
            );
            for (k, c) in lane.clips.iter().enumerate() {
                let sep = if k == 0 { "" } else { "," };
                let loop_length = c.loop_length.map_or("null".to_owned(), |n| n.to_string());
                let _ = write!(
                    w,
                    "{sep}\n              {{\n                \"id\": {}, \"start\": {}, \"length\": {}, \"offset\": {}, \"loop_length\": {}, \"muted\": {},\n",
                    c.id.0, c.start, c.length, c.offset, loop_length, c.muted
                );
                let indent = "                ";
                match &c.content {
                    ClipContent::Audio(a) => {
                        let curve = match a.fade_curve {
                            FadeCurve::Linear => "linear",
                            FadeCurve::EqualPower => "equal_power",
                        };
                        let _ = write!(
                            w,
                            "{indent}\"audio\": {{\"asset\": {}, \"offset\": {}, \"gain_db\": {}, \"fade_in\": {}, \"fade_out\": {}, \"fade_curve\": \"{curve}\"}}",
                            json_str(&a.asset),
                            a.source_offset,
                            f32_text(a.gain_db),
                            a.fade_in,
                            a.fade_out
                        );
                    }
                    ClipContent::Notes(notes) => {
                        let _ = write!(w, "{indent}\"notes\": ");
                        list(
                            w,
                            indent,
                            notes.iter().map(|n| {
                                format!(
                                    "[{}, {}, {}, {}{}]",
                                    n.tick,
                                    n.length,
                                    n.key,
                                    f32_text(n.velocity),
                                    if n.muted { ", true" } else { "" }
                                )
                            }),
                        );
                        if let Some(g) = &c.grid {
                            let resolution = match g.resolution {
                                StepResolution::Eighth => "eighth",
                                StepResolution::Sixteenth => "sixteenth",
                                StepResolution::ThirtySecond => "thirty_second",
                            };
                            let _ = write!(
                                w,
                                ",
{indent}\"grid\": {{\"resolution\": \"{resolution}\", \"steps\": {}, \"swing\": {}}}",
                                g.steps, g.swing
                            );
                        }
                    }
                    ClipContent::Automation(points) => {
                        let _ = write!(w, "{indent}\"points\": ");
                        list(
                            w,
                            indent,
                            points.iter().map(|p| {
                                let curve = match p.curve {
                                    Curve::Linear => "linear",
                                    Curve::Step => "step",
                                };
                                format!("[{}, {}, \"{curve}\"]", p.tick, f32_text(p.value))
                            }),
                        );
                    }
                }
                let _ = write!(w, "\n              }}");
            }
            let _ = write!(
                w,
                "{}]\n          }}",
                if lane.clips.is_empty() {
                    ""
                } else {
                    "\n            "
                }
            );
        }
        let _ = write!(
            w,
            "{}]\n      }}",
            if t.lanes.is_empty() { "" } else { "\n        " }
        );
    }
    let _ = writeln!(w, "{}]", if song.tracks.is_empty() { "" } else { "\n    " });
    let _ = writeln!(w, "  }}");
}
