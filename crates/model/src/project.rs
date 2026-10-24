//! `.atomik` project files: one JSON document holding the rack (the
//! patch v2 rack fields) and the song.
//!
//! ```text
//! { "format": "atomik-project", "version": 21,
//!   "sample_rate", "tempo_bpm", "seed", "output_channels", "devices", "cables"  (as patch v2;
//!       a device may also have "zones", v4, written only when non-empty, "steps", v7,
//!       written only when present, "inner", v8, a container's inside, see below, and
//!       "plugin", v9, a plugin device's document, see below)
//!   "input_channels": 0..=32,                                   (v3; optional, default 0)
//!   "console": { "channels": [{ "channel": key, "source": key | null }…],
//!                "buses"?: [{ "bus": key, "name": string }…]     (v12; written only when non-empty),
//!                "master": key | null, "returns": [key | null × 8], "compensation": bool },
//!   "assets": { "<64 hex digits>": { "channels", "sample_rate", "frames", "name" }… },
//!                                                               (v3; optional, default {})
//!   "song": { "tempo": [{"tick", "bpm", "ramp"? (v13; written only when true)}…],
//!             "meter": [{"bar", "num", "den"}…] (v13; before it one [beats_per_bar, beat_unit]),
//!             "loop": {"start", "end", "enabled"}, "end", "next_clip_id",
//!             "markers"?: [{"tick", "name", "color"? ("#rrggbb")}…]   (v14; written only when non-empty),
//!             "ranges"?: [{"start", "end", "name", "color"?}…]        (v14; written only when non-empty),
//!             "track_heights"?: {"<device key>": 24..=400, …}         (v15; written only when non-empty),
//!             "grooves"?: [{"name", "step", "slots": [[offset, velocity]…]}…]
//!                                                                     (v21; written only when non-empty),
//!             "tracks": [{ "device", "muted", "solo", "armed",
//!                          "lanes": [{ "kind": "notes" | "automation" | "audio", "param"?,
//!                                      "write"? (v18, automation lanes: "overwrite" | "latch"),
//!                                      "take_groups"? (v20, notes and audio lanes, after "clips"),
//!                                      "clips": [{ "id", "start", "length", "offset",
//!                                                  "loop_length" (int or null), "muted",
//!                                                  "grid"? (v5, note clips only, see below),
//!                                                  "controllers"? (v19, note clips only, see below),
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
//! **Version 4** files have no grids. **Version 5** zones have no `"reverse"`. **Version 6** files have no steps. **Version 7** files have no containers. **Version 8** files have no plugin devices. **Version 9** notes have no mute flag. **Version 11** files have no output buses (`"console"."buses"` is an unknown field there): a bus is an `atomik.bus` device listed in `"buses"` (console order) with a unique, trimmed, non-empty name of at most 32 characters; buses need a master; their routing is ordinary cables. **Version 12** files have a single meter and no tempo ramps: `"meter"` is `[beats_per_bar, beat_unit]` (it loads as one event at bar 1; the reader takes that shape in any version) and a tempo event has no `"ramp"` (unknown field there). From version 13 `"meter"` is written as a list of `{"bar", "num", "den"}` changes, first at bar 1, bars strictly increasing, each starting on a bar line; a tempo event with `"ramp": true` moves linearly (in BPM, over ticks) to the next event's BPM, and the last event cannot ramp. **Version 13** files have no song markers: from version 14 a song may carry `"markers"` (named points) and `"ranges"` (named sections, `start < end`, which may overlap), each sorted by tick, with a trimmed non-empty name of at most 64 characters and an optional `"#rrggbb"` colour; both are omitted when empty, and a file that lists them out of order loads sorted (ids are not stored; a loaded song numbers them from 1). **Version 14** files have no track heights: from version 15 a song may carry `"track_heights"`, an object from device key to the height in pixels (at vertical zoom 1) of that track's first lane in the arrangement, `24..=400`; it is omitted when empty, keys of devices without a track in the file are ignored, and a track without an entry has the default height of 60. **Version 15** files have no clip scales: from version 16 a note clip may carry `"scale": {"root": 0..=11, "kind": "major" | "natural_minor" | "harmonic_minor" | "melodic_minor" | "dorian" | "phrygian" | "lydian" | "mixolydian" | "locrian" | "major_pentatonic" | "minor_pentatonic" | "blues" | "whole_tone" | "phrygian_dominant" | "hungarian_minor"}` after its `"grid"`; it is omitted when there is none (a chromatic scale is not stored and is refused on load), and only note clips take it. **Version 16** files have no chase flag: from version 17 a track may carry `"no_chase": true` after its `"armed"` (a track that does not start the notes that span the play position on play, locate and loop wrap); it is omitted when the track chases, so a file without it chases. **Version 17** files have no lane write modes: from version 18 an automation lane may carry `"write": "overwrite" | "latch"` after its `"param"` (how automation recording writes into it; `"touch"`, the default, is not stored and is refused on load, and only automation lanes take it). **Version 18** files have no controller lanes: from version 19 a note clip may carry `"controllers": [{"kind": "pitch_bend" | "pressure" | "cc" | "poly_pressure", "number"?: 0..=127 (the controller of a "cc" lane, the key of a "poly_pressure" lane; required there, refused on the others), "points": [[tick, value, "linear"|"step"]…]}…]` after its `"scale"` (pitch bend and aftertouch, content ticks like the notes): the lanes are sorted by kind (pitch bend, pressure, cc by number, poly_pressure by key) without duplicates, each has at least one point, the points are sorted by tick without duplicates, and a value lies in `-1..=1` for pitch bend and `0..=1` for the others; the field is omitted when the clip has no lanes, and only note clips take it. **Version 19** files have no take groups: from version 20 a notes or audio lane may carry `"take_groups": [{"takes": [<clip>…], "comp": [[take, start, end]…], "clips": [id…]}…]` after its `"clips"` (see [`crate::takes`]): `"takes"` are whole clips of the lane's kind (one to 256, same fields as the lane's clips, ids unique among all clips of the song and below `next_clip_id`, none of them in the lane's `"clips"`); `"comp"` lists segments of take indices over the group's range (the earliest take start to the latest take end), sorted, disjoint and non-empty; `"clips"` names the lane clips compiled from the comp. The field is omitted when the lane has no groups; a loaded song numbers its groups from 1 in file order (ids are not stored). **Version 20** files have no groove library: from version 21 a song may carry `"grooves": [{"name", "step": "eighth" | "sixteenth" | "thirty_second", "slots": [[offset, velocity]…]}…]` after its `"track_heights"` (see [`crate::groove`]): a name is trimmed, non-empty, at most 32 characters, unique and not the name of a built-in groove; a groove has 1 to 32 slots, each an `offset` in steps (`-0.5..=0.5`) and a velocity scale (`0..=2`); at most 64 grooves; the field is omitted when the song has none. Saving always writes version 21.

use crate::console::{
    BUS_TYPE, CHANNEL_TYPE, Console, ConsoleBus, ConsoleChannel, FX_SLOTS, MASTER_TYPE,
    MAX_BUS_NAME_CHARS, is_console_type,
};
use crate::controller::{ControllerKind, ControllerLane, check_lanes};
use crate::groove::{Groove, GrooveSlot, MAX_GROOVES, builtin_groove};
use crate::patch::{
    DEVICE_FIELDS_V2, DEVICE_FIELDS_V4, DEVICE_FIELDS_V7, DEVICE_FIELDS_V8, DEVICE_FIELDS_V9,
    DEVICE_FIELDS_V11, DeviceFormat, JsonPath, array, describe, f32_text, f64_text, json_str,
    load_rack, number, object, parse_color, string, uint, write_devices_and_cables,
};
use crate::song::{
    AUDIO_GAIN_DB_MAX, AUDIO_GAIN_DB_MIN, AudioClipContent, AutomationPoint, Clip, ClipContent,
    ClipId, Curve, FadeCurve, Lane, LaneKind, MAX_AUDIO_FRAMES, MAX_LOOP_REPEATS, MAX_MARKER_NAME,
    MAX_TRACK_HEIGHT, MIN_TRACK_HEIGHT, Marker, MeterEvent, Note, RangeMarker, Song, StepGrid,
    StepResolution, TempoEvent, Track,
};
use crate::song_cmd::{
    check_end, check_loop, check_meter, check_meter_map, check_note, has_notes_input,
    is_audio_track_type, length_ok, name_ok, tick_ok,
};
use crate::steps::{
    MAX_NAME_CHARS, MAX_PATTERNS, MAX_STEPS, PatternDoc, SeqResolution, StepDoc, StepSeqDoc,
};
use crate::takes::{CompSegment, TakeGroup, check_comp};
use crate::takes_cmd::MAX_TAKES;
use crate::theory::{Scale, ScaleKind};
use crate::{AssetMeta, ZoneDoc, is_valid_asset_hash};
use crate::{DeviceKey, ModelError, PATCH_FORMAT, RackDoc, TEMPO_MAX, TEMPO_MIN, WriteMode, rules};
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
/// plugin devices) and 9 (no note mute) and 10 (no rack columns: one column on load) and 11
/// (no output buses) and 12 (no tempo ramps or meter changes) and 13 (no markers) and 14 (no
/// track heights) and 15 (no clip scales).
pub const PROJECT_VERSION: u32 = 21;

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
const ROOT_FIELDS_V11: &[&str] = ROOT_FIELDS_V12;
const ROOT_FIELDS_V12: &[&str] = &[
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
const SCALE_FIELDS: &[&str] = &["root", "kind"];
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
const AUDIO_LANE_FIELDS_V20: &[&str] = &["kind", "clips", "take_groups"];
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
const CONSOLE_FIELDS_V12: &[&str] = &["channels", "buses", "master", "returns", "compensation"];
const BUS_FIELDS: &[&str] = &["bus", "name"];
const CHANNEL_FIELDS: &[&str] = &["channel", "source"];
const SONG_FIELDS: &[&str] = &["tempo", "meter", "loop", "end", "next_clip_id", "tracks"];
const SONG_FIELDS_V14: &[&str] = &[
    "tempo",
    "meter",
    "loop",
    "end",
    "next_clip_id",
    "markers",
    "ranges",
    "tracks",
];
const SONG_FIELDS_V15: &[&str] = &[
    "tempo",
    "meter",
    "loop",
    "end",
    "next_clip_id",
    "markers",
    "ranges",
    "track_heights",
    "tracks",
];
const SONG_FIELDS_V21: &[&str] = &[
    "tempo",
    "meter",
    "loop",
    "end",
    "next_clip_id",
    "markers",
    "ranges",
    "track_heights",
    "grooves",
    "tracks",
];
const GROOVE_FIELDS: &[&str] = &["name", "step", "slots"];
const TRACK_FIELDS: &[&str] = &["device", "muted", "solo", "armed", "lanes"];
const TRACK_FIELDS_V17: &[&str] = &["device", "muted", "solo", "armed", "no_chase", "lanes"];
const NOTES_LANE_FIELDS: &[&str] = &["kind", "clips"];
const NOTES_LANE_FIELDS_V20: &[&str] = &["kind", "clips", "take_groups"];
const TAKE_GROUP_FIELDS: &[&str] = &["takes", "comp", "clips"];
const AUTOMATION_LANE_FIELDS: &[&str] = &["kind", "param", "clips"];
const AUTOMATION_LANE_FIELDS_V18: &[&str] = &["kind", "param", "write", "clips"];
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
const NOTE_CLIP_FIELDS_V16: &[&str] = &[
    "id",
    "start",
    "length",
    "offset",
    "loop_length",
    "muted",
    "grid",
    "scale",
    "notes",
];
const NOTE_CLIP_FIELDS_V19: &[&str] = &[
    "id",
    "start",
    "length",
    "offset",
    "loop_length",
    "muted",
    "grid",
    "scale",
    "controllers",
    "notes",
];
const CONTROLLER_LANE_FIELDS: &[&str] = &["kind", "number", "points"];
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
        12..=21 => (ROOT_FIELDS_V12, ROOT_REQUIRED_V2),
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
    doc.console = load_console(&obj["console"], &root.field("console"), &doc, version)?;
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
fn load_console(
    v: &Value,
    path: &JsonPath,
    doc: &RackDoc,
    version: u64,
) -> Result<Console, ModelError> {
    let fields = if version >= 12 {
        CONSOLE_FIELDS_V12
    } else {
        CONSOLE_FIELDS
    };
    let obj = object(v, path, fields, CONSOLE_FIELDS)?;
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

    if let Some(buses) = obj.get("buses") {
        let buses_path = path.field("buses");
        for (i, item) in array(buses, &buses_path)?.iter().enumerate() {
            let p = buses_path.index(i);
            let b = object(item, &p, BUS_FIELDS, BUS_FIELDS)?;
            let bus_path = p.field("bus");
            let bus = device_ref(&b["bus"], &bus_path)?
                .ok_or_else(|| bus_path.error("expected a device id, found null"))?;
            if type_of(&bus) != BUS_TYPE {
                return Err(bus_path.error(format!(
                    "{:?} is a {}, not an {BUS_TYPE}",
                    bus.0,
                    type_of(&bus)
                )));
            }
            if console.bus_index(&bus).is_some() {
                return Err(bus_path.error(format!("{:?} is listed twice", bus.0)));
            }
            if console.master.is_none() {
                return Err(bus_path.error("the console has no master, so it has no buses"));
            }
            let name_path = p.field("name");
            let name = string(&b["name"], &name_path)?;
            if name.trim() != name || name.is_empty() {
                return Err(name_path.error(format!(
                    "{name:?} is not a valid bus name: it must be non-empty with no space at either end"
                )));
            }
            if name.chars().count() > MAX_BUS_NAME_CHARS {
                return Err(name_path.error(format!(
                    "a bus name has at most {MAX_BUS_NAME_CHARS} characters"
                )));
            }
            if console.bus_by_name(name).is_some() {
                return Err(name_path.error(format!("the bus name {name:?} is used twice")));
            }
            console.buses.push(ConsoleBus {
                bus,
                name: name.to_owned(),
            });
        }
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

/// The song's meter: `[beats_per_bar, beat_unit]` (one meter for the whole song, what files
/// before version 13 write) or a list of `{"bar", "num", "den"}` changes (version 13). The reader
/// takes either shape in any version.
fn load_meter(v: &Value, path: &JsonPath) -> Result<Vec<MeterEvent>, ModelError> {
    if v.as_array()
        .is_some_and(|a| a.first().is_some_and(Value::is_number))
    {
        let meter = tuple(v, path, 2, "[beats_per_bar, beat_unit]")?;
        let num = uint(&meter[0], &path.index(0), 255)? as u8;
        let den = uint(&meter[1], &path.index(1), 255)? as u8;
        check_meter(num, 4).map_err(|m| path.index(0).error(m))?;
        check_meter(1, den).map_err(|m| path.index(1).error(m))?;
        return Ok(vec![MeterEvent { bar: 1, num, den }]);
    }
    let mut meter: Vec<MeterEvent> = Vec::new();
    for (i, item) in array(v, path)?.iter().enumerate() {
        let p = path.index(i);
        let fields = &["bar", "num", "den"];
        let e = object(item, &p, fields, fields)?;
        let bar = uint(&e["bar"], &p.field("bar"), u64::from(u32::MAX))? as u32;
        let num = uint(&e["num"], &p.field("num"), 255)? as u8;
        let den = uint(&e["den"], &p.field("den"), 255)? as u8;
        check_meter(num, 4).map_err(|m| p.field("num").error(m))?;
        check_meter(1, den).map_err(|m| p.field("den").error(m))?;
        match meter.last() {
            None if bar != 1 => {
                return Err(p
                    .field("bar")
                    .error(format!("the first meter event must be at bar 1, not {bar}")));
            }
            Some(prev) if bar <= prev.bar => {
                return Err(p.field("bar").error(format!(
                    "bars must increase strictly ({bar} after {})",
                    prev.bar
                )));
            }
            _ => {}
        }
        meter.push(MeterEvent { bar, num, den });
    }
    if meter.is_empty() {
        return Err(path.error("the meter map needs at least one event"));
    }
    check_meter_map(&meter).map_err(|m| path.error(m))?;
    Ok(meter)
}

/// The optional `"markers"` list: sorted by tick on load (equal ticks keep file order), ids
/// numbered from 1 in that order.
fn load_markers(v: Option<&Value>, path: &JsonPath) -> Result<Vec<Marker>, ModelError> {
    let Some(v) = v else {
        return Ok(Vec::new());
    };
    let mut markers = Vec::new();
    for (i, item) in array(v, path)?.iter().enumerate() {
        let p = path.index(i);
        let e = object(item, &p, &["tick", "name", "color"], &["tick", "name"])?;
        markers.push(Marker {
            id: 0,
            tick: tick(&e["tick"], &p.field("tick"))?,
            name: marker_name(&e["name"], &p.field("name"))?,
            color: marker_color(e, &p)?,
        });
    }
    markers.sort_by_key(|m| m.tick);
    for (id, m) in (1..).zip(markers.iter_mut()) {
        m.id = id;
    }
    Ok(markers)
}

/// The optional `"track_heights"` object: device key to a height in `24..=400`. The keys of
/// devices without a track are ignored (the file may have outlived a track).
fn load_track_heights(
    v: Option<&Value>,
    path: &JsonPath,
    song: &Song,
) -> Result<BTreeMap<DeviceKey, u16>, ModelError> {
    let Some(v) = v else {
        return Ok(BTreeMap::new());
    };
    let map = v
        .as_object()
        .ok_or_else(|| path.error(format!("expected an object, found {}", describe(v))))?;
    let mut heights = BTreeMap::new();
    for (key, item) in map {
        let p = path.field(key);
        let h = uint(item, &p, u64::from(u16::MAX))? as u16;
        if !(MIN_TRACK_HEIGHT..=MAX_TRACK_HEIGHT).contains(&h) {
            return Err(p.error(format!(
                "height {h} is outside {MIN_TRACK_HEIGHT}..={MAX_TRACK_HEIGHT}"
            )));
        }
        let key = DeviceKey(key.clone());
        if song.tracks.iter().any(|t| t.device == key) {
            heights.insert(key, h);
        }
    }
    Ok(heights)
}

/// The optional `"grooves"` list, in file order.
fn load_grooves(v: Option<&Value>, path: &JsonPath) -> Result<Vec<Groove>, ModelError> {
    let Some(v) = v else {
        return Ok(Vec::new());
    };
    let items = array(v, path)?;
    if items.len() > MAX_GROOVES {
        return Err(path.error(format!(
            "{} grooves; a song holds at most {MAX_GROOVES}",
            items.len()
        )));
    }
    let mut grooves: Vec<Groove> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let p = path.index(i);
        let e = object(item, &p, GROOVE_FIELDS, GROOVE_FIELDS)?;
        let sp = p.field("step");
        let step = match string(&e["step"], &sp)? {
            "eighth" => StepResolution::Eighth,
            "sixteenth" => StepResolution::Sixteenth,
            "thirty_second" => StepResolution::ThirtySecond,
            other => {
                return Err(sp.error(format!(
                    "expected \"eighth\", \"sixteenth\" or \"thirty_second\", found {other:?}"
                )));
            }
        };
        let slots_path = p.field("slots");
        let mut slots = Vec::new();
        for (j, slot) in array(&e["slots"], &slots_path)?.iter().enumerate() {
            let sp = slots_path.index(j);
            let pair = array(slot, &sp)?;
            let [offset, velocity] = pair else {
                return Err(sp.error(format!("expected a slot [offset, velocity], found {slot}")));
            };
            slots.push(GrooveSlot {
                offset: number(offset, &sp.index(0))? as f32,
                velocity: number(velocity, &sp.index(1))? as f32,
            });
        }
        let groove = Groove {
            name: string(&e["name"], &p.field("name"))?.to_string(),
            step,
            slots,
        };
        groove.validate().map_err(|m| p.error(m))?;
        if builtin_groove(&groove.name).is_some() {
            return Err(p.field("name").error(format!(
                "{:?} is the name of a built-in groove",
                groove.name
            )));
        }
        if grooves.iter().any(|g| g.name == groove.name) {
            return Err(p
                .field("name")
                .error(format!("a second groove named {:?}", groove.name)));
        }
        grooves.push(groove);
    }
    Ok(grooves)
}

/// The optional `"ranges"` list, loaded like the markers (sorted by start).
fn load_ranges(v: Option<&Value>, path: &JsonPath) -> Result<Vec<RangeMarker>, ModelError> {
    let Some(v) = v else {
        return Ok(Vec::new());
    };
    let mut ranges = Vec::new();
    for (i, item) in array(v, path)?.iter().enumerate() {
        let p = path.index(i);
        let e = object(
            item,
            &p,
            &["start", "end", "name", "color"],
            &["start", "end", "name"],
        )?;
        let start = tick(&e["start"], &p.field("start"))?;
        let end = tick(&e["end"], &p.field("end"))?;
        if end <= start {
            return Err(p
                .field("end")
                .error(format!("the end ({end}) must be above the start ({start})")));
        }
        ranges.push(RangeMarker {
            id: 0,
            start,
            end,
            name: marker_name(&e["name"], &p.field("name"))?,
            color: marker_color(e, &p)?,
        });
    }
    ranges.sort_by_key(|r| r.start);
    for (id, r) in (1..).zip(ranges.iter_mut()) {
        r.id = id;
    }
    Ok(ranges)
}

fn marker_name(v: &Value, path: &JsonPath) -> Result<String, ModelError> {
    let name = string(v, path)?;
    if !name_ok(name) {
        return Err(path.error(format!(
            "the name must be trimmed, non-empty and at most {MAX_MARKER_NAME} characters"
        )));
    }
    Ok(name.to_owned())
}

fn marker_color(
    e: &serde_json::Map<String, Value>,
    path: &JsonPath,
) -> Result<Option<[u8; 3]>, ModelError> {
    e.get("color")
        .map_or(Ok(None), |v| parse_color(v, &path.field("color")))
}

fn load_song(
    v: &Value,
    path: &JsonPath,
    (doc, registry): (&RackDoc, &Registry),
    version: u64,
) -> Result<Song, ModelError> {
    let allowed = if version >= 21 {
        SONG_FIELDS_V21
    } else if version >= 15 {
        SONG_FIELDS_V15
    } else if version >= 14 {
        SONG_FIELDS_V14
    } else {
        SONG_FIELDS
    };
    let obj = object(v, path, allowed, SONG_FIELDS)?;

    let tempo_path = path.field("tempo");
    let mut tempo = Vec::new();
    for (i, item) in array(&obj["tempo"], &tempo_path)?.iter().enumerate() {
        let p = tempo_path.index(i);
        let (allowed, required): (&[&str], &[&str]) = if version >= 13 {
            (&["tick", "bpm", "ramp"], &["tick", "bpm"])
        } else {
            (&["tick", "bpm"], &["tick", "bpm"])
        };
        let e = object(item, &p, allowed, required)?;
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
        let ramp = flag(e, &p, "ramp")?;
        tempo.push(TempoEvent { tick: t, bpm, ramp });
    }
    if tempo.is_empty() {
        return Err(tempo_path.error("the tempo map needs at least one event"));
    }
    if let Some(last) = tempo.last().filter(|e| e.ramp) {
        return Err(tempo_path
            .index(tempo.len() - 1)
            .field("ramp")
            .error(format!(
                "the last tempo event (tick {}) cannot ramp: there is no next event",
                last.tick
            )));
    }

    let meter = load_meter(&obj["meter"], &path.field("meter"))?;

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
        meter,
        loop_range,
        loop_enabled,
        end,
        next_clip_id,
        markers: load_markers(obj.get("markers"), &path.field("markers"))?,
        ranges: load_ranges(obj.get("ranges"), &path.field("ranges"))?,
        track_heights: BTreeMap::new(),
        lane_write: BTreeMap::new(),
        grooves: load_grooves(obj.get("grooves"), &path.field("grooves"))?,
    };
    let tracks_path = path.field("tracks");
    let mut track_first: BTreeMap<DeviceKey, JsonPath> = BTreeMap::new();
    let mut clip_first: BTreeMap<u64, JsonPath> = BTreeMap::new();
    let mut lane_write: BTreeMap<(DeviceKey, String), WriteMode> = BTreeMap::new();
    for (i, item) in array(&obj["tracks"], &tracks_path)?.iter().enumerate() {
        let p = tracks_path.index(i);
        let track = load_track(
            item,
            &p,
            (doc, registry),
            version,
            &song,
            (&mut clip_first, &mut lane_write),
        )?;
        if let Some(first) = track_first.get(&track.device) {
            return Err(p.field("device").error(format!(
                "{:?} already has a track ({first})",
                track.device.0
            )));
        }
        track_first.insert(track.device.clone(), p);
        song.tracks.push(track);
    }
    song.lane_write = lane_write;
    let groups = song
        .tracks
        .iter_mut()
        .flat_map(|t| &mut t.lanes)
        .flat_map(|l| &mut l.take_groups);
    for (id, group) in (1..).zip(groups) {
        group.id = id;
    }
    song.track_heights = load_track_heights(
        obj.get("track_heights"),
        &path.field("track_heights"),
        &song,
    )?;
    Ok(song)
}

/// Where each clip id was first seen, for duplicate messages.
type ClipFirst = BTreeMap<u64, JsonPath>;
/// The write modes found while loading tracks, by `(device, param)`.
type LaneWrites = BTreeMap<(DeviceKey, String), WriteMode>;

fn load_track(
    v: &Value,
    path: &JsonPath,
    (doc, registry): (&RackDoc, &Registry),
    version: u64,
    song: &Song,
    (clip_first, lane_write): (&mut ClipFirst, &mut LaneWrites),
) -> Result<Track, ModelError> {
    let fields = if version >= 17 {
        TRACK_FIELDS_V17
    } else {
        TRACK_FIELDS
    };
    let obj = object(v, path, fields, &["device", "lanes"])?;
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
            (
                if version >= 20 {
                    AUDIO_LANE_FIELDS_V20
                } else {
                    AUDIO_LANE_FIELDS
                },
                AUDIO_CLIP_FIELDS,
            )
        } else if i == 0 {
            (
                if version >= 20 {
                    NOTES_LANE_FIELDS_V20
                } else {
                    NOTES_LANE_FIELDS
                },
                if version >= 19 {
                    NOTE_CLIP_FIELDS_V19
                } else if version >= 16 {
                    NOTE_CLIP_FIELDS_V16
                } else if version >= 5 {
                    NOTE_CLIP_FIELDS_V5
                } else {
                    NOTE_CLIP_FIELDS
                },
            )
        } else if version >= 18 {
            (AUTOMATION_LANE_FIELDS_V18, AUTOMATION_CLIP_FIELDS)
        } else {
            (AUTOMATION_LANE_FIELDS, AUTOMATION_CLIP_FIELDS)
        };
        let required = if version >= 18 && fields == AUTOMATION_LANE_FIELDS_V18 {
            AUTOMATION_LANE_FIELDS
        } else if fields == AUDIO_LANE_FIELDS_V20 || fields == NOTES_LANE_FIELDS_V20 {
            AUDIO_LANE_FIELDS
        } else {
            fields
        };
        let lane = object(item, &p, fields, required)?;
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
            if let Some(w) = lane.get("write") {
                let write_path = p.field("write");
                let mode = WriteMode::from_name(string(w, &write_path)?)
                    .filter(|m| *m != WriteMode::default())
                    .ok_or_else(|| {
                        write_path
                            .error("expected \"overwrite\" or \"latch\" (touch is the default)")
                    })?;
                lane_write.insert((device.clone(), param.to_owned()), mode);
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
        let take_groups = match lane.get("take_groups") {
            Some(v) => load_take_groups(
                v,
                &p.field("take_groups"),
                (clip_fields, version),
                &kind,
                (doc, desc),
                song,
                (&clips, clip_first),
            )?,
            None => Vec::new(),
        };
        lanes.push(Lane {
            kind,
            clips,
            take_groups,
        });
    }
    if lanes.is_empty() {
        return Err(lanes_path.error("a track needs its note lane"));
    }
    Ok(Track {
        device,
        muted: flag(obj, path, "muted")?,
        solo: flag(obj, path, "solo")?,
        armed: flag(obj, path, "armed")?,
        chase: !flag(obj, path, "no_chase")?,
        lanes,
    })
}

/// Reads a lane's `"take_groups"` (ids are numbered later, in file order).
fn load_take_groups(
    v: &Value,
    path: &JsonPath,
    (clip_fields, version): (&[&str], u64),
    kind: &LaneKind,
    (doc, desc): (&RackDoc, &atomik_device_sdk::DeviceDescriptor),
    song: &Song,
    (lane_clips, clip_first): (&[Clip], &mut ClipFirst),
) -> Result<Vec<TakeGroup>, ModelError> {
    let mut claimed: BTreeMap<u64, JsonPath> = BTreeMap::new();
    let mut groups = Vec::new();
    for (g, item) in array(v, path)?.iter().enumerate() {
        let gp = path.index(g);
        let obj = object(item, &gp, TAKE_GROUP_FIELDS, TAKE_GROUP_FIELDS)?;
        let takes_path = gp.field("takes");
        let mut takes = Vec::new();
        for (k, t) in array(&obj["takes"], &takes_path)?.iter().enumerate() {
            let tp = takes_path.index(k);
            let clip = load_clip(
                t,
                &tp,
                (clip_fields, version),
                kind,
                (doc, desc),
                song,
                clip_first,
            )?;
            clip_first.insert(clip.id.0, tp.field("id"));
            takes.push(clip);
        }
        if takes.is_empty() || takes.len() > MAX_TAKES {
            return Err(takes_path.error(format!("a take group needs 1..={MAX_TAKES} takes")));
        }
        let mut group = TakeGroup {
            id: 0,
            takes,
            comp: Vec::new(),
            clips: Vec::new(),
        };
        let comp_path = gp.field("comp");
        for (i, item) in array(&obj["comp"], &comp_path)?.iter().enumerate() {
            let sp = comp_path.index(i);
            let a = tuple(item, &sp, 3, "a segment [take, start, end]")?;
            group.comp.push(CompSegment {
                take: uint(&a[0], &sp.index(0), u64::from(u32::MAX))? as usize,
                start: tick(&a[1], &sp.index(1))?,
                end: tick(&a[2], &sp.index(2))?,
            });
        }
        check_comp(&group.comp, group.takes.len(), group.range())
            .map_err(|m| comp_path.error(m))?;
        let ids_path = gp.field("clips");
        for (i, item) in array(&obj["clips"], &ids_path)?.iter().enumerate() {
            let ip = ids_path.index(i);
            let id = uint(item, &ip, u64::MAX)?;
            if !lane_clips.iter().any(|c| c.id.0 == id) {
                return Err(ip.error(format!("clip {id} is not on this lane")));
            }
            if let Some(first) = claimed.insert(id, ip.clone()) {
                return Err(ip.error(format!("clip {id} is already a comp clip ({first})")));
            }
            group.clips.push(ClipId(id));
        }
        groups.push(group);
    }
    Ok(groups)
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
            scale: None,
            controllers: Vec::new(),
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
        scale: match obj.get("scale") {
            Some(s) => Some(load_scale(s, &path.field("scale"))?),
            None => None,
        },
        controllers: match obj.get("controllers") {
            Some(c) => load_controllers(c, &path.field("controllers"))?,
            None => Vec::new(),
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

/// Reads a note clip's `"controllers"` list (see the module docs).
fn load_controllers(v: &Value, path: &JsonPath) -> Result<Vec<ControllerLane>, ModelError> {
    let mut lanes: Vec<ControllerLane> = Vec::new();
    for (i, item) in array(v, path)?.iter().enumerate() {
        let p = path.index(i);
        let obj = object(item, &p, CONTROLLER_LANE_FIELDS, &["kind", "points"])?;
        let kind_path = p.field("kind");
        let name = string(&obj["kind"], &kind_path)?;
        let ccnum = match obj.get("number") {
            Some(n) => Some(uint(n, &p.field("number"), 127)? as u8),
            None => None,
        };
        let kind = match (name, ccnum) {
            ("pitch_bend", None) => ControllerKind::PitchBend,
            ("pressure", None) => ControllerKind::ChannelPressure,
            ("cc", Some(n)) => ControllerKind::Cc(n),
            ("poly_pressure", Some(n)) => ControllerKind::PolyPressure(n),
            ("cc" | "poly_pressure", None) => {
                return Err(p.error(format!("a \"{name}\" lane needs a \"number\"")));
            }
            ("pitch_bend" | "pressure", Some(_)) => {
                return Err(p.error(format!("a \"{name}\" lane has no \"number\"")));
            }
            (other, _) => {
                return Err(kind_path.error(format!(
                    "expected \"pitch_bend\", \"pressure\", \"cc\" or \"poly_pressure\", found {other:?}"
                )));
            }
        };
        let points_path = p.field("points");
        let mut points: Vec<AutomationPoint> = Vec::new();
        for (j, item) in array(&obj["points"], &points_path)?.iter().enumerate() {
            let q = points_path.index(j);
            let a = tuple(item, &q, 3, "a point [tick, value, curve]")?;
            let curve = match string(&a[2], &q.index(2))? {
                "linear" => Curve::Linear,
                "step" => Curve::Step,
                other => {
                    return Err(q
                        .index(2)
                        .error(format!("expected \"linear\" or \"step\", found {other:?}")));
                }
            };
            points.push(AutomationPoint {
                tick: tick(&a[0], &q.index(0))?,
                value: number(&a[1], &q.index(1))? as f32,
                curve,
            });
        }
        lanes.push(ControllerLane::new(kind, points));
    }
    check_lanes(&lanes, crate::song::MAX_TICK).map_err(|m| path.error(m))?;
    Ok(lanes)
}

/// Reads a note clip's `"scale"` object: a root `0..=11` and a kind name. A chromatic scale is
/// not stored, so it is refused.
fn load_scale(v: &Value, path: &JsonPath) -> Result<Scale, ModelError> {
    let s = object(v, path, SCALE_FIELDS, SCALE_FIELDS)?;
    let root = uint(&s["root"], &path.field("root"), 11)? as u8;
    let kp = path.field("kind");
    let name = string(&s["kind"], &kp)?;
    match ScaleKind::from_id(name) {
        Some(ScaleKind::Chromatic) => {
            Err(kp.error("a chromatic scale is not stored (leave the scale out)"))
        }
        Some(kind) => Ok(Scale::new(root, kind)),
        None => Err(kp.error(format!("unknown scale {name:?}"))),
    }
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
/// Writes one clip object of a lane's `"clips"` (or of a take group's `"takes"`), after the
/// separator `sep` of its list.
fn write_clip(w: &mut String, c: &Clip, sep: &str) {
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
            if let Some(s) = c.scale.filter(|s| !s.is_chromatic()) {
                let _ = write!(
                    w,
                    ",
{indent}\"scale\": {{\"root\": {}, \"kind\": \"{}\"}}",
                    s.root,
                    s.kind.id()
                );
            }
            if !c.controllers.is_empty() {
                let _ = write!(
                    w,
                    ",
{indent}\"controllers\": ["
                );
                for (n, lane) in c.controllers.iter().enumerate() {
                    let (kind, number) = match lane.kind {
                        ControllerKind::PitchBend => ("pitch_bend", String::new()),
                        ControllerKind::ChannelPressure => ("pressure", String::new()),
                        ControllerKind::Cc(k) => ("cc", format!(", \"number\": {k}")),
                        ControllerKind::PolyPressure(k) => {
                            ("poly_pressure", format!(", \"number\": {k}"))
                        }
                    };
                    let sep = if n == 0 { "" } else { "," };
                    let _ = write!(
                        w,
                        "{sep}
{indent}  {{\"kind\": \"{kind}\"{number}, \"points\": "
                    );
                    list(
                        w,
                        &format!("{indent}  "),
                        lane.points.iter().map(|p| {
                            let curve = match p.curve {
                                Curve::Linear => "linear",
                                Curve::Step => "step",
                            };
                            format!("[{}, {}, \"{curve}\"]", p.tick, f32_text(p.value))
                        }),
                    );
                    let _ = write!(w, "}}");
                }
                let _ = write!(
                    w,
                    "
{indent}]"
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

/// Writes a lane's `"take_groups"` field (see [`crate::takes`]) after its `"clips"`.
fn write_take_groups(w: &mut String, groups: &[TakeGroup]) {
    let _ = write!(w, ",\n            \"take_groups\": [");
    for (g, group) in groups.iter().enumerate() {
        let sep = if g == 0 { "" } else { "," };
        let _ = write!(w, "{sep}\n              {{\n                \"takes\": [");
        for (k, c) in group.takes.iter().enumerate() {
            write_clip(w, c, if k == 0 { "" } else { "," });
        }
        let comp: Vec<String> = group
            .comp
            .iter()
            .map(|s| format!("[{}, {}, {}]", s.take, s.start, s.end))
            .collect();
        let ids: Vec<String> = group.clips.iter().map(|c| c.0.to_string()).collect();
        let _ = write!(
            w,
            "\n                ],\n                \"comp\": [{}],\n                \"clips\": [{}]\n              }}",
            comp.join(", "),
            ids.join(", ")
        );
    }
    let _ = write!(w, "\n            ]");
}

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
    if !console.buses.is_empty() {
        let _ = write!(w, "    \"buses\": ");
        list(
            w,
            "    ",
            console.buses.iter().map(|b| {
                format!(
                    "{{\"bus\": {}, \"name\": {}}}",
                    json_str(&b.bus.0),
                    json_str(&b.name)
                )
            }),
        );
        let _ = writeln!(w, ",");
    }
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
        song.tempo.iter().map(|e| {
            let ramp = if e.ramp { ", \"ramp\": true" } else { "" };
            format!(
                "{{\"tick\": {}, \"bpm\": {}{ramp}}}",
                e.tick,
                f64_text(e.bpm)
            )
        }),
    );
    let _ = writeln!(w, ",");
    let _ = write!(w, "    \"meter\": ");
    list(
        w,
        "    ",
        song.meter.iter().map(|e| {
            format!(
                "{{\"bar\": {}, \"num\": {}, \"den\": {}}}",
                e.bar, e.num, e.den
            )
        }),
    );
    let _ = writeln!(w, ",");
    let _ = writeln!(
        w,
        "    \"loop\": {{\"start\": {}, \"end\": {}, \"enabled\": {}}},",
        song.loop_range.0, song.loop_range.1, song.loop_enabled
    );
    let _ = writeln!(w, "    \"end\": {},", song.end);
    let _ = writeln!(w, "    \"next_clip_id\": {},", song.next_clip_id);
    let color = |c: Option<[u8; 3]>| {
        c.map_or(String::new(), |[r, g, b]| {
            format!(", \"color\": \"#{r:02x}{g:02x}{b:02x}\"")
        })
    };
    if !song.markers.is_empty() {
        let _ = write!(w, "    \"markers\": ");
        list(
            w,
            "    ",
            song.markers.iter().map(|m| {
                format!(
                    "{{\"tick\": {}, \"name\": {}{}}}",
                    m.tick,
                    json_str(&m.name),
                    color(m.color)
                )
            }),
        );
        let _ = writeln!(w, ",");
    }
    if !song.ranges.is_empty() {
        let _ = write!(w, "    \"ranges\": ");
        list(
            w,
            "    ",
            song.ranges.iter().map(|r| {
                format!(
                    "{{\"start\": {}, \"end\": {}, \"name\": {}{}}}",
                    r.start,
                    r.end,
                    json_str(&r.name),
                    color(r.color)
                )
            }),
        );
        let _ = writeln!(w, ",");
    }
    let heights: Vec<String> = song
        .track_heights
        .iter()
        .filter(|(key, _)| song.tracks.iter().any(|t| &t.device == *key))
        .map(|(key, h)| format!("{}: {h}", json_str(&key.0)))
        .collect();
    if !heights.is_empty() {
        let _ = writeln!(w, "    \"track_heights\": {{{}}},", heights.join(", "));
    }
    if !song.grooves.is_empty() {
        let _ = write!(w, "    \"grooves\": ");
        list(
            w,
            "    ",
            song.grooves.iter().map(|g| {
                let step = match g.step {
                    StepResolution::Eighth => "eighth",
                    StepResolution::Sixteenth => "sixteenth",
                    StepResolution::ThirtySecond => "thirty_second",
                };
                let slots: Vec<String> = g
                    .slots
                    .iter()
                    .map(|s| format!("[{}, {}]", f32_text(s.offset), f32_text(s.velocity)))
                    .collect();
                format!(
                    "{{\"name\": {}, \"step\": \"{step}\", \"slots\": [{}]}}",
                    json_str(&g.name),
                    slots.join(", ")
                )
            }),
        );
        let _ = writeln!(w, ",");
    }
    let _ = write!(w, "    \"tracks\": [");
    for (i, t) in song.tracks.iter().enumerate() {
        let sep = if i == 0 { "" } else { "," };
        let _ = write!(
            w,
            "{sep}\n      {{\n        \"device\": {}, \"muted\": {}, \"solo\": {}, \"armed\": {},{}\n        \"lanes\": [",
            json_str(&t.device.0),
            t.muted,
            t.solo,
            t.armed,
            if t.chase { "" } else { " \"no_chase\": true," }
        );
        for (j, lane) in t.lanes.iter().enumerate() {
            let sep = if j == 0 { "" } else { "," };
            let kind = match &lane.kind {
                LaneKind::Notes => "\"kind\": \"notes\"".to_owned(),
                LaneKind::Audio => "\"kind\": \"audio\"".to_owned(),
                LaneKind::Automation(p) => {
                    let write = match song.write_mode(&t.device, p) {
                        WriteMode::Touch => String::new(),
                        mode => format!(", \"write\": \"{}\"", mode.name()),
                    };
                    format!(
                        "\"kind\": \"automation\", \"param\": {}{write}",
                        json_str(p)
                    )
                }
            };
            let _ = write!(
                w,
                "{sep}\n          {{\n            {kind},\n            \"clips\": ["
            );
            for (k, c) in lane.clips.iter().enumerate() {
                write_clip(w, c, if k == 0 { "" } else { "," });
            }
            let _ = write!(
                w,
                "{}]",
                if lane.clips.is_empty() {
                    ""
                } else {
                    "\n            "
                }
            );
            if !lane.take_groups.is_empty() {
                write_take_groups(w, &lane.take_groups);
            }
            let _ = write!(w, "\n          }}");
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
