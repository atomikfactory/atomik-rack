//! The project format's field table.
//!
//! Every JSON object a project file holds is one [`Obj`]; every field it may carry is one row of
//! [`SPECS`]: its name, the version that introduced it (`since`), the version that dropped it
//! (`until`, exclusive), whether a file must have it, and whether an older reader may ignore it.
//! The reader asks [`fields`] and [`required`] for the lists of an object at a file's version, so
//! adding a field is one row here and the code that reads and writes its value.
//!
//! Rows are in the order the field is written, which is also the order the "expected one of"
//! error lists them. A base field (one every version has) has `since` 1: the table does not track
//! when an object itself appeared.
//!
//! # Forward compatibility
//!
//! A file newer than the reader is refused unless it says an older reader may open it. The
//! writer puts two root fields after `"version"` when the format has changed in ways an older
//! reader can skip:
//!
//! - `"compat"`: the oldest version that can read the file ([`compat_version`]: the newest
//!   version that added a required field, removed a field or is listed in [`BREAKING_VERSIONS`]).
//! - `"ignorable"`: an object from the path of a field (`song.tracks[].lanes[].clips[].tail`,
//!   arrays written `[]`) to the version that added it; the fields of rows marked
//!   [`FieldSpec::ignorable`] that a reader of version `compat` lacks ([`ignorable_paths`]).
//!
//! A reader that finds `compat` at or below its own version drops every listed field it does not
//! know, with one warning each, and reads the rest as its own version; any other unknown field is
//! still an error. Both fields are written only when `compat` is below the file's version, so a
//! format whose newest change is not ignorable writes neither.
//!
//! A row may be marked ignorable only when its object has a place in the file a path can name
//! ([`Obj::paths`]), and only for a field whose absence leaves the document playing and bouncing
//! as it did; anything else is not ignorable, and the bump raises `compat`.

use super::PROJECT_VERSION;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// The kinds of JSON object in a project file.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) enum Obj {
    Root,
    Device,
    Zone,
    Patterns,
    Pattern,
    Step,
    CurvePattern,
    CurveStep,
    DrumPattern,
    DrumLane,
    Asset,
    Console,
    Channel,
    Bus,
    Vca,
    Snapshot,
    SnapshotVca,
    Song,
    TempoEvent,
    MeterEvent,
    Loop,
    Marker,
    Range,
    Groove,
    Group,
    Track,
    Frozen,
    NotesLane,
    AudioLane,
    AutomationLane,
    TakeGroup,
    NoteClip,
    AudioClip,
    AutomationClip,
    Audio,
    Warp,
    Grid,
    Scale,
    ControllerLane,
}

impl Obj {
    /// Where the object sits in a file, arrays written `[]`. Empty for the objects whose place a
    /// path cannot name (a device may be inside a container, and so is everything on it; an asset
    /// has its hash for a key): none of their fields can be ignorable.
    pub(crate) const fn paths(self) -> &'static [&'static str] {
        match self {
            Obj::Root => &[""],
            Obj::Console => &["console"],
            Obj::Channel => &["console.channels[]"],
            Obj::Bus => &["console.buses[]"],
            Obj::Vca => &["console.vcas[]"],
            Obj::Snapshot => &["console.snapshots[]"],
            Obj::SnapshotVca => &["console.snapshots[].vcas[]"],
            Obj::Song => &["song"],
            Obj::TempoEvent => &["song.tempo[]"],
            Obj::MeterEvent => &["song.meter[]"],
            Obj::Loop => &["song.loop"],
            Obj::Marker => &["song.markers[]"],
            Obj::Range => &["song.ranges[]"],
            Obj::Groove => &["song.grooves[]"],
            Obj::Group => &["song.groups[]"],
            Obj::Track => &["song.tracks[]"],
            Obj::Frozen => &["song.tracks[].frozen"],
            Obj::NotesLane | Obj::AudioLane | Obj::AutomationLane => &["song.tracks[].lanes[]"],
            Obj::TakeGroup => &["song.tracks[].lanes[].take_groups[]"],
            Obj::NoteClip | Obj::AudioClip | Obj::AutomationClip => &[
                "song.tracks[].lanes[].clips[]",
                "song.tracks[].lanes[].take_groups[].takes[]",
            ],
            Obj::Audio => &[
                "song.tracks[].lanes[].clips[].audio",
                "song.tracks[].lanes[].take_groups[].takes[].audio",
            ],
            Obj::Warp => &[
                "song.tracks[].lanes[].clips[].audio.warp",
                "song.tracks[].lanes[].take_groups[].takes[].audio.warp",
            ],
            Obj::Grid => &[
                "song.tracks[].lanes[].clips[].grid",
                "song.tracks[].lanes[].take_groups[].takes[].grid",
            ],
            Obj::Scale => &[
                "song.tracks[].lanes[].clips[].scale",
                "song.tracks[].lanes[].take_groups[].takes[].scale",
            ],
            Obj::ControllerLane => &[
                "song.tracks[].lanes[].clips[].controllers[]",
                "song.tracks[].lanes[].take_groups[].takes[].controllers[]",
            ],
            Obj::Asset
            | Obj::Device
            | Obj::Zone
            | Obj::Patterns
            | Obj::Pattern
            | Obj::Step
            | Obj::CurvePattern
            | Obj::CurveStep
            | Obj::DrumPattern
            | Obj::DrumLane => &[],
        }
    }
}

/// One field of one object, and when it exists.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FieldSpec {
    pub(crate) object: Obj,
    pub(crate) name: &'static str,
    /// The first format version that has the field (1 for a base field).
    pub(crate) since: u32,
    /// The first version that no longer has it ([`NEVER`] while it is current).
    pub(crate) until: u32,
    /// A file that has the field's version must write it.
    pub(crate) required: bool,
    /// A reader older than `since` may drop the field with a warning (see the module docs).
    pub(crate) ignorable: bool,
}

/// `until` of a field that is still current.
pub(crate) const NEVER: u32 = u32::MAX;

const fn field(object: Obj, name: &'static str, since: u32) -> FieldSpec {
    FieldSpec {
        object,
        name,
        since,
        until: NEVER,
        required: false,
        ignorable: false,
    }
}

impl FieldSpec {
    const fn req(mut self) -> Self {
        self.required = true;
        self
    }

    /// Marks the field as droppable by an older reader.
    const fn ignorable(mut self) -> Self {
        self.ignorable = true;
        self
    }

    /// Whether files of `version` have the field.
    const fn present_in(&self, version: u32) -> bool {
        self.since <= version && version < self.until
    }
}

/// Format versions whose change an older reader cannot skip although no field row shows it (a
/// field that changed meaning, a rule that tightened). [`compat_version`] is at least the newest
/// of them. Empty: every change so far added or removed fields.
pub(crate) const BREAKING_VERSIONS: &[u32] = &[];

use Obj::*;

/// Every field of every object, in write order.
pub(crate) static SPECS: &[FieldSpec] = &[
    // The root.
    field(Root, "format", 1).req(),
    field(Root, "version", 1).req(),
    field(Root, "compat", 37).ignorable(),
    field(Root, "ignorable", 37).ignorable(),
    field(Root, "sample_rate", 1).req(),
    field(Root, "tempo_bpm", 1).req(),
    field(Root, "seed", 1),
    field(Root, "output_channels", 1).req(),
    field(Root, "input_channels", 3),
    field(Root, "rack_columns", 11),
    field(Root, "devices", 1).req(),
    field(Root, "cables", 1),
    field(Root, "console", 2).req(),
    field(Root, "midi_maps", 28),
    field(Root, "macros", 29),
    field(Root, "assets", 3),
    field(Root, "song", 1).req(),
    // A device (the fields shared with the patch format come first).
    field(Device, "id", 1),
    field(Device, "type", 1),
    field(Device, "params", 1),
    field(Device, "name", 1),
    field(Device, "patch", 36),
    field(Device, "folded", 1),
    field(Device, "color", 1),
    field(Device, "column", 11),
    field(Device, "zones", 4),
    field(Device, "steps", 7),
    field(Device, "curves", 32),
    field(Device, "drums", 33),
    field(Device, "inner", 8),
    field(Device, "plugin", 9),
    field(Device, "midi_out", 37),
    // A sampler zone.
    field(Zone, "asset", 1).req(),
    field(Zone, "key_lo", 1).req(),
    field(Zone, "key_hi", 1).req(),
    field(Zone, "vel_lo", 1).req(),
    field(Zone, "vel_hi", 1).req(),
    field(Zone, "root_key", 1).req(),
    field(Zone, "transpose", 1).req(),
    field(Zone, "tune_cents", 1).req(),
    field(Zone, "gain_db", 1).req(),
    field(Zone, "pan", 1).req(),
    field(Zone, "start", 1).req(),
    field(Zone, "end", 1).req(),
    field(Zone, "loop_mode", 1).req(),
    field(Zone, "loop_start", 1).req(),
    field(Zone, "loop_end", 1).req(),
    field(Zone, "crossfade", 1).req(),
    field(Zone, "trigger", 1).req(),
    field(Zone, "reverse", 6).req(),
    // Step, curve and drum pattern data.
    field(Patterns, "patterns", 1).req(),
    field(Pattern, "name", 1).req(),
    field(Pattern, "length", 1).req(),
    field(Pattern, "resolution", 1).req(),
    field(Pattern, "swing", 1).req(),
    field(Pattern, "steps", 1).req(),
    field(Step, "on", 1).req(),
    field(Step, "key", 1).req(),
    field(Step, "velocity", 1).req(),
    field(Step, "gate", 1).req(),
    field(Step, "prob", 1).req(),
    field(Step, "tie", 1).req(),
    field(CurvePattern, "a", 1).req(),
    field(CurvePattern, "b", 1).req(),
    field(CurveStep, "value", 1).req(),
    field(CurveStep, "slide", 1).req(),
    field(DrumPattern, "name", 1).req(),
    field(DrumPattern, "length", 1).req(),
    field(DrumPattern, "resolution", 1).req(),
    field(DrumPattern, "swing", 1).req(),
    field(DrumPattern, "lanes", 1).req(),
    field(DrumLane, "key", 1).req(),
    field(DrumLane, "role", 1).req(),
    field(DrumLane, "mute", 1).req(),
    field(DrumLane, "velocity", 1).req(),
    field(DrumLane, "prob", 1).req(),
    // The asset pool.
    field(Asset, "channels", 1).req(),
    field(Asset, "sample_rate", 1).req(),
    field(Asset, "frames", 1).req(),
    field(Asset, "name", 1),
    // The console.
    field(Console, "channels", 1).req(),
    field(Console, "buses", 12),
    field(Console, "vcas", 26),
    field(Console, "snapshots", 27),
    field(Console, "master", 1).req(),
    field(Console, "returns", 1).req(),
    field(Console, "compensation", 1).req(),
    field(Console, "align_returns", 37),
    field(Channel, "channel", 1).req(),
    field(Channel, "source", 1).req(),
    field(Bus, "bus", 1).req(),
    field(Bus, "name", 1).req(),
    field(Vca, "id", 1).req(),
    field(Vca, "name", 1).req(),
    field(Vca, "color", 1),
    field(Vca, "level_db", 1),
    field(Vca, "mute", 1),
    field(Vca, "solo", 1),
    field(Vca, "members", 1).req(),
    field(Snapshot, "id", 1).req(),
    field(Snapshot, "name", 1).req(),
    field(Snapshot, "devices", 1).req(),
    field(Snapshot, "vcas", 1),
    field(SnapshotVca, "id", 1).req(),
    field(SnapshotVca, "name", 1).req(),
    field(SnapshotVca, "level_db", 1),
    field(SnapshotVca, "mute", 1),
    field(SnapshotVca, "solo", 1),
    field(SnapshotVca, "members", 1).req(),
    // The song.
    field(Song, "tempo", 1).req(),
    field(Song, "meter", 1).req(),
    field(Song, "loop", 1).req(),
    field(Song, "end", 1).req(),
    field(Song, "next_clip_id", 1).req(),
    field(Song, "tails", 35),
    field(Song, "markers", 14),
    field(Song, "ranges", 14),
    field(Song, "track_heights", 15),
    field(Song, "grooves", 21),
    field(Song, "groups", 22),
    field(Song, "tracks", 1).req(),
    field(TempoEvent, "tick", 1).req(),
    field(TempoEvent, "bpm", 1).req(),
    field(TempoEvent, "ramp", 13),
    field(MeterEvent, "bar", 1).req(),
    field(MeterEvent, "num", 1).req(),
    field(MeterEvent, "den", 1).req(),
    field(Loop, "start", 1).req(),
    field(Loop, "end", 1).req(),
    field(Loop, "enabled", 1).req(),
    field(Marker, "tick", 1).req(),
    field(Marker, "name", 1).req(),
    field(Marker, "color", 1),
    field(Range, "start", 1).req(),
    field(Range, "end", 1).req(),
    field(Range, "name", 1).req(),
    field(Range, "color", 1),
    field(Groove, "name", 1).req(),
    field(Groove, "step", 1).req(),
    field(Groove, "slots", 1).req(),
    field(Group, "id", 1).req(),
    field(Group, "name", 1).req(),
    field(Group, "parent", 1),
    field(Group, "collapsed", 1),
    field(Group, "color", 1),
    // A track and its frozen audio.
    field(Track, "device", 1).req(),
    field(Track, "muted", 1),
    field(Track, "solo", 1),
    field(Track, "armed", 1),
    field(Track, "no_chase", 17),
    field(Track, "notes_to", 31),
    field(Track, "group", 22),
    field(Track, "color", 22),
    field(Track, "comment", 22),
    field(Track, "frozen", 25),
    field(Track, "lanes", 1).req(),
    field(Frozen, "start", 1).req(),
    field(Frozen, "length", 1).req(),
    field(Frozen, "effects", 1),
    field(Frozen, "audio", 1).req(),
    // Lanes.
    field(NotesLane, "kind", 1).req(),
    field(NotesLane, "name", 30),
    field(NotesLane, "muted", 30),
    field(NotesLane, "clips", 1).req(),
    field(NotesLane, "take_groups", 20),
    field(AudioLane, "kind", 1).req(),
    field(AudioLane, "clips", 1).req(),
    field(AudioLane, "take_groups", 20),
    field(AutomationLane, "kind", 1).req(),
    field(AutomationLane, "param", 1).req(),
    field(AutomationLane, "write", 18),
    field(AutomationLane, "folded", 22),
    field(AutomationLane, "muted", 36),
    field(AutomationLane, "static", 36),
    field(AutomationLane, "clips", 1).req(),
    field(TakeGroup, "takes", 1).req(),
    field(TakeGroup, "comp", 1).req(),
    field(TakeGroup, "clips", 1).req(),
    field(TakeGroup, "open", 22),
    // Clips: the last field of each is its content.
    field(NoteClip, "id", 1).req(),
    field(NoteClip, "start", 1).req(),
    field(NoteClip, "length", 1).req(),
    field(NoteClip, "offset", 1),
    field(NoteClip, "loop_length", 1),
    field(NoteClip, "muted", 1),
    field(NoteClip, "grid", 5),
    field(NoteClip, "scale", 16),
    field(NoteClip, "controllers", 19),
    field(NoteClip, "notes", 1).req(),
    field(AudioClip, "id", 1).req(),
    field(AudioClip, "start", 1).req(),
    field(AudioClip, "length", 1).req(),
    field(AudioClip, "offset", 1),
    field(AudioClip, "loop_length", 1),
    field(AudioClip, "muted", 1),
    field(AudioClip, "audio", 1).req(),
    field(AutomationClip, "id", 1).req(),
    field(AutomationClip, "start", 1).req(),
    field(AutomationClip, "length", 1).req(),
    field(AutomationClip, "offset", 1),
    field(AutomationClip, "loop_length", 1),
    field(AutomationClip, "muted", 1),
    field(AutomationClip, "points", 1).req(),
    field(ControllerLane, "kind", 1).req(),
    field(ControllerLane, "number", 1),
    field(ControllerLane, "points", 1).req(),
    field(Grid, "resolution", 1).req(),
    field(Grid, "steps", 1).req(),
    field(Grid, "swing", 1).req(),
    field(Scale, "root", 1).req(),
    field(Scale, "kind", 1).req(),
    field(Audio, "asset", 1).req(),
    field(Audio, "offset", 1).req(),
    field(Audio, "gain_db", 1),
    field(Audio, "fade_in", 1),
    field(Audio, "fade_out", 1),
    field(Audio, "fade_curve", 1),
    field(Audio, "reverse", 23),
    field(Audio, "envelope", 23),
    field(Audio, "xfade", 23),
    field(Audio, "warp", 24),
    field(Warp, "mode", 1).req(),
    field(Warp, "bpm", 1).req(),
    field(Warp, "enabled", 1),
    field(Warp, "transpose", 1),
    field(Warp, "markers", 1),
];

/// The names of an object's fields at `version`, and the ones a file must have, in table order.
fn derive(
    specs: &[FieldSpec],
    object: Obj,
    version: u32,
) -> (Vec<&'static str>, Vec<&'static str>) {
    let here = specs
        .iter()
        .filter(|s| s.object == object && s.present_in(version));
    let all = here.clone().map(|s| s.name).collect();
    let required = here.filter(|s| s.required).map(|s| s.name).collect();
    (all, required)
}

type Lists = (Vec<&'static str>, Vec<&'static str>);

/// The lists of every object at every version this reader reads, built once.
fn lists() -> &'static BTreeMap<(Obj, u32), Lists> {
    static LISTS: OnceLock<BTreeMap<(Obj, u32), Lists>> = OnceLock::new();
    LISTS.get_or_init(|| {
        let mut map = BTreeMap::new();
        for object in SPECS.iter().map(|s| s.object) {
            for version in 1..=PROJECT_VERSION {
                map.entry((object, version))
                    .or_insert_with(|| derive(SPECS, object, version));
            }
        }
        map
    })
}

fn lists_of(object: Obj, version: u64) -> &'static Lists {
    let version = version.clamp(1, u64::from(PROJECT_VERSION)) as u32;
    &lists()[&(object, version)]
}

/// The fields an object may have in a file of `version`.
pub(crate) fn fields(object: Obj, version: u64) -> &'static [&'static str] {
    &lists_of(object, version).0
}

/// The fields an object must have in a file of `version`.
pub(crate) fn required(object: Obj, version: u64) -> &'static [&'static str] {
    &lists_of(object, version).1
}

/// [`crate::patch::object`] with the lists of the table: reads an object of a file of `version`,
/// refusing an unknown field and a missing required one.
pub(crate) fn object<'v>(
    v: &'v Value,
    path: &crate::patch::JsonPath,
    object: Obj,
    version: u64,
) -> Result<&'v serde_json::Map<String, Value>, crate::ModelError> {
    crate::patch::object(v, path, fields(object, version), required(object, version))
}

/// The fields an object has in the current version, in table order.
pub(crate) fn current_fields(kind: Obj) -> &'static [&'static str] {
    fields(kind, u64::from(PROJECT_VERSION))
}

/// [`object`] for an object that has the same fields in every version.
pub(crate) fn current<'v>(
    v: &'v Value,
    path: &crate::patch::JsonPath,
    kind: Obj,
) -> Result<&'v serde_json::Map<String, Value>, crate::ModelError> {
    object(v, path, kind, u64::from(PROJECT_VERSION))
}
/// The oldest version of a reader that can open a file of `version` by ignoring fields (see the
/// module docs): `version` itself when the newest change is not ignorable.
pub(crate) fn compat_version(specs: &[FieldSpec], version: u32) -> u32 {
    let added = specs
        .iter()
        .filter(|s| s.since <= version && !s.ignorable)
        .map(|s| s.since);
    let removed = specs.iter().filter(|s| s.until <= version).map(|s| s.until);
    let breaking = BREAKING_VERSIONS.iter().copied().filter(|v| *v <= version);
    added.chain(removed).chain(breaking).max().unwrap_or(1)
}

/// The ignorable fields a reader of version `compat` lacks, as `path.field` (the root's fields
/// have no path) to the version that added them: the root `"ignorable"` field of a file.
pub(crate) fn ignorable_paths(
    specs: &[FieldSpec],
    compat: u32,
    version: u32,
) -> BTreeMap<String, u32> {
    specs
        .iter()
        .filter(|s| s.ignorable && compat < s.since && s.since <= version)
        .filter(|s| !(s.object == Obj::Root && matches!(s.name, "compat" | "ignorable")))
        .flat_map(|s| {
            s.object.paths().iter().map(move |path| {
                let name = if path.is_empty() {
                    s.name.to_owned()
                } else {
                    format!("{path}.{}", s.name)
                };
                (name, s.since)
            })
        })
        .collect()
}

/// Prepares a file newer than this reader: when its `"compat"` allows it, drops the fields its
/// `"ignorable"` lists that this reader does not know (one warning each) and makes it a file of
/// this reader's version. Returns whether the file was changed. A file of this version or older
/// is left alone; a newer one without that permission is refused.
pub(crate) fn admit_newer(
    value: &mut Value,
    warnings: &mut Vec<String>,
) -> Result<bool, crate::ModelError> {
    use crate::patch::JsonPath;
    let root = JsonPath::default();
    let Some(obj) = value.as_object_mut() else {
        return Ok(false);
    };
    let version = match obj.get("version").and_then(Value::as_u64) {
        Some(v) if v > u64::from(PROJECT_VERSION) => v,
        _ => return Ok(false),
    };
    if obj.get("format").and_then(Value::as_str) != Some(super::PROJECT_FORMAT) {
        return Ok(false);
    }
    let unsupported = |why: String| {
        root.field("version").error(format!(
            "unsupported version {version}{why}; this reader reads versions 1 to {PROJECT_VERSION}"
        ))
    };
    let Some(compat) = obj.get("compat").and_then(Value::as_u64) else {
        return Err(unsupported(String::new()));
    };
    if compat > u64::from(PROJECT_VERSION) {
        return Err(unsupported(format!(
            " (it needs a reader of version {compat} or later)"
        )));
    }
    let mut ignorable = BTreeMap::new();
    if let Some(list) = obj.remove("ignorable") {
        let path = root.field("ignorable");
        let Some(map) = list.as_object() else {
            return Err(path.error("expected an object from field path to version"));
        };
        for (name, since) in map {
            let Some(since) = since.as_u64() else {
                return Err(path
                    .field(name)
                    .error("expected the version that added the field"));
            };
            ignorable.insert(name.clone(), since);
        }
    }
    obj.remove("compat");
    obj.insert("version".to_owned(), Value::from(PROJECT_VERSION));
    strip_ignorable(value, "", "", &ignorable, warnings);
    Ok(true)
}

/// Removes the fields of `value` that `ignorable` lists as added after this reader's version,
/// walking the tree: `schema_path` is the path of the current node as the list spells it,
/// `json_path` the same with the array indices, for the warning.
fn strip_ignorable(
    value: &mut Value,
    schema_path: &str,
    json_path: &str,
    ignorable: &BTreeMap<String, u64>,
    warnings: &mut Vec<String>,
) {
    match value {
        Value::Object(map) => {
            let keys: Vec<String> = map.keys().cloned().collect();
            for key in keys {
                let at = if schema_path.is_empty() {
                    key.clone()
                } else {
                    format!("{schema_path}.{key}")
                };
                let json_at = if json_path.is_empty() {
                    key.clone()
                } else {
                    format!("{json_path}.{key}")
                };
                if ignorable
                    .get(&at)
                    .is_some_and(|since| *since > u64::from(PROJECT_VERSION))
                {
                    map.remove(&key);
                    warnings.push(format!(
                        "{json_at}: ignored, a field of a newer project format"
                    ));
                } else if let Some(child) = map.get_mut(&key) {
                    strip_ignorable(child, &at, &json_at, ignorable, warnings);
                }
            }
        }
        Value::Array(items) => {
            let at = format!("{schema_path}[]");
            for (i, item) in items.iter_mut().enumerate() {
                let json_at = format!("{json_path}[{i}]");
                strip_ignorable(item, &at, &json_at, ignorable, warnings);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn the_table_is_well_formed() {
        let mut seen = BTreeSet::new();
        for s in SPECS {
            assert!(
                seen.insert((s.object, s.name)),
                "{:?}.{} is listed twice",
                s.object,
                s.name
            );
            assert!(
                s.since >= 1 && s.since < s.until,
                "{:?}.{}",
                s.object,
                s.name
            );
            assert!(
                !s.ignorable || !s.object.paths().is_empty(),
                "{:?}.{} is ignorable, but its object has no path an older reader could match",
                s.object,
                s.name
            );
        }
        // The last field of a clip is its content: the loader reads it by that name.
        for (clip, content) in [
            (Obj::NoteClip, "notes"),
            (Obj::AudioClip, "audio"),
            (Obj::AutomationClip, "points"),
        ] {
            for v in 1..=u64::from(PROJECT_VERSION) {
                assert_eq!(fields(clip, v).last(), Some(&content), "{clip:?} v{v}");
            }
        }
        assert_eq!(current_fields(Obj::CurvePattern), ["a", "b"]);
    }

    #[test]
    fn a_field_is_listed_from_the_version_that_added_it() {
        assert!(!fields(Obj::Device, 35).contains(&"patch"));
        assert!(fields(Obj::Device, 36).contains(&"patch"));
        assert!(!fields(Obj::Device, 36).contains(&"midi_out"));
        assert!(fields(Obj::Device, 37).contains(&"midi_out"));
        assert!(!fields(Obj::Root, 27).contains(&"midi_maps"));
        assert!(fields(Obj::Root, 28).contains(&"midi_maps"));
        assert!(!required(Obj::Root, 1).contains(&"console"));
        assert!(required(Obj::Root, 2).contains(&"console"));
        // A version past the reader's gets the newest lists.
        assert_eq!(
            fields(Obj::Song, 99),
            fields(Obj::Song, u64::from(PROJECT_VERSION))
        );
    }

    const fn row(name: &'static str, since: u32) -> FieldSpec {
        field(Obj::NoteClip, name, since)
    }

    #[test]
    fn the_compat_version_is_the_newest_change_an_older_reader_cannot_skip() {
        let specs = [
            row("a", 1),
            row("b", 5),
            row("c", 7).ignorable(),
            row("d", 8).ignorable(),
        ];
        assert_eq!(compat_version(&specs, 8), 5);
        assert_eq!(compat_version(&specs, 6), 5);
        assert_eq!(compat_version(&specs, 4), 1);
        let mut dropped = row("e", 3);
        dropped.until = 6;
        let specs = [row("c", 7).ignorable(), dropped];
        assert_eq!(
            compat_version(&specs, 8),
            6,
            "a removed field is not skippable"
        );
        assert_eq!(compat_version(&specs, 5), 3);
        // Today's table has no ignorable row, so the current version is its own compat.
        assert_eq!(compat_version(SPECS, PROJECT_VERSION), PROJECT_VERSION);
    }

    #[test]
    fn the_ignorable_list_names_what_a_reader_of_the_compat_version_lacks() {
        let specs = [
            row("b", 5),
            row("c", 7).ignorable(),
            row("d", 8).ignorable(),
            field(Obj::Root, "x", 8).ignorable(),
            field(Obj::Device, "y", 8).ignorable(),
        ];
        let list = ignorable_paths(&specs, 5, 8);
        let names: Vec<&str> = list.keys().map(String::as_str).collect();
        assert_eq!(
            names,
            [
                "song.tracks[].lanes[].clips[].c",
                "song.tracks[].lanes[].clips[].d",
                "song.tracks[].lanes[].take_groups[].takes[].c",
                "song.tracks[].lanes[].take_groups[].takes[].d",
                "x",
            ],
            "a device field has no path, a field a reader of 7 knows is left out for it"
        );
        assert_eq!(list["x"], 8);
        assert_eq!(ignorable_paths(&specs, 7, 8).len(), 3);
    }

    fn newer(extra: Value) -> Value {
        let mut v = serde_json::json!({
            "format": "atomik-project",
            "version": PROJECT_VERSION + 1,
            "song": { "tracks": [ { "lanes": [ { "clips": [ { "id": 1 }, { "id": 2 } ] } ] } ] }
        });
        v.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        v
    }

    #[test]
    fn a_newer_file_without_compat_is_left_for_the_loader_to_refuse() {
        let mut v = newer(serde_json::json!({}));
        let mut warnings = Vec::new();
        let e = admit_newer(&mut v, &mut warnings).unwrap_err();
        assert!(e.to_string().contains("unsupported version"), "{e}");
        let mut v = newer(serde_json::json!({ "compat": PROJECT_VERSION + 1 }));
        let e = admit_newer(&mut v, &mut warnings).unwrap_err();
        assert!(e.to_string().contains("needs a reader of version"), "{e}");
        // Files of this reader's version or older are not its business.
        let mut v = serde_json::json!({ "format": "atomik-project", "version": 1 });
        assert!(!admit_newer(&mut v, &mut warnings).unwrap());
    }

    #[test]
    fn a_newer_file_loses_the_listed_fields_and_becomes_a_file_of_this_version() {
        let tail = "song.tracks[].lanes[].clips[].tail".to_owned();
        let mut v = newer(serde_json::json!({
            "compat": PROJECT_VERSION,
            "ignorable": { tail: PROJECT_VERSION + 1, "song.tracks[].lanes[].clips[].id": PROJECT_VERSION },
        }));
        v["song"]["tracks"][0]["lanes"][0]["clips"][1]["tail"] = serde_json::json!(true);
        v["song"]["tracks"][0]["lanes"][0]["clips"][1]["other"] = serde_json::json!(true);
        let mut warnings = Vec::new();
        assert!(admit_newer(&mut v, &mut warnings).unwrap());
        assert_eq!(
            warnings,
            ["song.tracks[0].lanes[0].clips[1].tail: ignored, a field of a newer project format"]
        );
        assert_eq!(v["version"], PROJECT_VERSION);
        assert!(v.get("compat").is_none() && v.get("ignorable").is_none());
        let clip = &v["song"]["tracks"][0]["lanes"][0]["clips"][1];
        assert!(clip.get("tail").is_none(), "the listed field is gone");
        assert!(
            clip.get("other").is_some(),
            "an unlisted one is left for the reader to refuse"
        );
        assert_eq!(clip["id"], 2, "a field this reader knows stays");
    }
}
