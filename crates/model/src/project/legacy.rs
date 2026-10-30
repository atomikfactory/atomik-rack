//! The hand-written field lists the table in `schema.rs` replaces.

pub(super) const ROOT_FIELDS_V1: &[&str] = &[
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
pub(super) const ROOT_REQUIRED_V1: &[&str] = &[
    "format",
    "version",
    "sample_rate",
    "tempo_bpm",
    "output_channels",
    "devices",
    "song",
];
pub(super) const ROOT_FIELDS_V2: &[&str] = &[
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
pub(super) const ROOT_REQUIRED_V2: &[&str] = &[
    "format",
    "version",
    "sample_rate",
    "tempo_bpm",
    "output_channels",
    "devices",
    "console",
    "song",
];
pub(super) const ROOT_FIELDS_V3: &[&str] = &[
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
pub(super) const ROOT_FIELDS_V4: &[&str] = ROOT_FIELDS_V3;
pub(super) const ROOT_FIELDS_V5: &[&str] = ROOT_FIELDS_V3;
pub(super) const ROOT_FIELDS_V6: &[&str] = ROOT_FIELDS_V3;
pub(super) const ROOT_FIELDS_V7: &[&str] = ROOT_FIELDS_V3;
pub(super) const ROOT_FIELDS_V8: &[&str] = ROOT_FIELDS_V3;
pub(super) const ROOT_FIELDS_V9: &[&str] = ROOT_FIELDS_V3;
pub(super) const ROOT_FIELDS_V10: &[&str] = ROOT_FIELDS_V3;
pub(super) const ROOT_FIELDS_V11: &[&str] = ROOT_FIELDS_V12;
pub(super) const ROOT_FIELDS_V12: &[&str] = &[
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
pub(super) const ROOT_FIELDS_V29: &[&str] = &[
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
    "midi_maps",
    "macros",
    "assets",
    "song",
];
pub(super) const ROOT_FIELDS_V28: &[&str] = &[
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
    "midi_maps",
    "assets",
    "song",
];
pub(super) const PATTERNS_FIELDS: &[&str] = &["patterns"];
pub(super) const PATTERN_FIELDS: &[&str] = &["name", "length", "resolution", "swing", "steps"];
pub(super) const STEP_FIELDS: &[&str] = &["on", "key", "velocity", "gate", "prob", "tie"];
pub(super) const CURVE_PATTERN_FIELDS: &[&str] = &["a", "b"];
pub(super) const CURVE_STEP_FIELDS: &[&str] = &["value", "slide"];
pub(super) const DRUM_PATTERN_FIELDS: &[&str] = &["name", "length", "resolution", "swing", "lanes"];
pub(super) const DRUM_LANE_FIELDS: &[&str] = &["key", "role", "mute", "velocity", "prob"];
pub(super) const GRID_FIELDS: &[&str] = &["resolution", "steps", "swing"];
pub(super) const SCALE_FIELDS: &[&str] = &["root", "kind"];
pub(super) const ZONE_FIELDS: &[&str] = &[
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
pub(super) const ZONE_FIELDS_V6: &[&str] = &[
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
pub(super) const ASSET_FIELDS: &[&str] = &["channels", "sample_rate", "frames", "name"];
pub(super) const AUDIO_LANE_FIELDS: &[&str] = &["kind", "clips"];
pub(super) const AUDIO_LANE_FIELDS_V20: &[&str] = &["kind", "clips", "take_groups"];
pub(super) const AUDIO_CLIP_FIELDS: &[&str] = &[
    "id",
    "start",
    "length",
    "offset",
    "loop_length",
    "muted",
    "audio",
];
pub(super) const AUDIO_FIELDS: &[&str] = &[
    "asset",
    "offset",
    "gain_db",
    "fade_in",
    "fade_out",
    "fade_curve",
];
pub(super) const AUDIO_FIELDS_V23: &[&str] = &[
    "asset",
    "offset",
    "gain_db",
    "fade_in",
    "fade_out",
    "fade_curve",
    "reverse",
    "envelope",
    "xfade",
];
pub(super) const AUDIO_FIELDS_V24: &[&str] = &[
    "asset",
    "offset",
    "gain_db",
    "fade_in",
    "fade_out",
    "fade_curve",
    "reverse",
    "envelope",
    "xfade",
    "warp",
];
pub(super) const WARP_FIELDS: &[&str] = &["mode", "bpm", "enabled", "transpose", "markers"];
pub(super) const CONSOLE_FIELDS: &[&str] = &["channels", "master", "returns", "compensation"];
pub(super) const CONSOLE_FIELDS_V12: &[&str] =
    &["channels", "buses", "master", "returns", "compensation"];
pub(super) const CONSOLE_FIELDS_V26: &[&str] = &[
    "channels",
    "buses",
    "vcas",
    "master",
    "returns",
    "compensation",
];
pub(super) const CONSOLE_FIELDS_V27: &[&str] = &[
    "channels",
    "buses",
    "vcas",
    "snapshots",
    "master",
    "returns",
    "compensation",
];
pub(super) const SNAPSHOT_FIELDS: &[&str] = &["id", "name", "devices", "vcas"];
pub(super) const SNAPSHOT_REQUIRED: &[&str] = &["id", "name", "devices"];
pub(super) const SNAPSHOT_VCA_FIELDS: &[&str] =
    &["id", "name", "level_db", "mute", "solo", "members"];
pub(super) const BUS_FIELDS: &[&str] = &["bus", "name"];
pub(super) const VCA_FIELDS: &[&str] =
    &["id", "name", "color", "level_db", "mute", "solo", "members"];
pub(super) const VCA_REQUIRED: &[&str] = &["id", "name", "members"];
pub(super) const CHANNEL_FIELDS: &[&str] = &["channel", "source"];
pub(super) const SONG_FIELDS: &[&str] =
    &["tempo", "meter", "loop", "end", "next_clip_id", "tracks"];
pub(super) const SONG_FIELDS_V14: &[&str] = &[
    "tempo",
    "meter",
    "loop",
    "end",
    "next_clip_id",
    "markers",
    "ranges",
    "tracks",
];
pub(super) const SONG_FIELDS_V15: &[&str] = &[
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
pub(super) const SONG_FIELDS_V21: &[&str] = &[
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
pub(super) const SONG_FIELDS_V22: &[&str] = &[
    "tempo",
    "meter",
    "loop",
    "end",
    "next_clip_id",
    "markers",
    "ranges",
    "track_heights",
    "grooves",
    "groups",
    "tracks",
];
pub(super) const SONG_FIELDS_V35: &[&str] = &[
    "tempo",
    "meter",
    "loop",
    "end",
    "next_clip_id",
    "tails",
    "markers",
    "ranges",
    "track_heights",
    "grooves",
    "groups",
    "tracks",
];
pub(super) const GROOVE_FIELDS: &[&str] = &["name", "step", "slots"];
pub(super) const GROUP_FIELDS: &[&str] = &["id", "name", "parent", "collapsed", "color"];
pub(super) const TRACK_FIELDS: &[&str] = &["device", "muted", "solo", "armed", "lanes"];
pub(super) const TRACK_FIELDS_V17: &[&str] =
    &["device", "muted", "solo", "armed", "no_chase", "lanes"];
pub(super) const TRACK_FIELDS_V22: &[&str] = &[
    "device", "muted", "solo", "armed", "no_chase", "group", "color", "comment", "lanes",
];
pub(super) const TRACK_FIELDS_V25: &[&str] = &[
    "device", "muted", "solo", "armed", "no_chase", "group", "color", "comment", "frozen", "lanes",
];
pub(super) const TRACK_FIELDS_V31: &[&str] = &[
    "device", "muted", "solo", "armed", "no_chase", "notes_to", "group", "color", "comment",
    "frozen", "lanes",
];
pub(super) const FROZEN_FIELDS: &[&str] = &["start", "length", "effects", "audio"];
pub(super) const NOTES_LANE_FIELDS: &[&str] = &["kind", "clips"];
pub(super) const NOTES_LANE_FIELDS_V20: &[&str] = &["kind", "clips", "take_groups"];
pub(super) const NOTES_LANE_FIELDS_V30: &[&str] =
    &["kind", "name", "muted", "clips", "take_groups"];
pub(super) const TAKE_GROUP_FIELDS: &[&str] = &["takes", "comp", "clips"];
pub(super) const TAKE_GROUP_FIELDS_V22: &[&str] = &["takes", "comp", "clips", "open"];
pub(super) const AUTOMATION_LANE_FIELDS: &[&str] = &["kind", "param", "clips"];
pub(super) const AUTOMATION_LANE_FIELDS_V18: &[&str] = &["kind", "param", "write", "clips"];
pub(super) const AUTOMATION_LANE_FIELDS_V22: &[&str] =
    &["kind", "param", "write", "folded", "clips"];
pub(super) const AUTOMATION_LANE_FIELDS_V36: &[&str] = &[
    "kind", "param", "write", "folded", "muted", "static", "clips",
];
pub(super) const NOTE_CLIP_FIELDS: &[&str] = &[
    "id",
    "start",
    "length",
    "offset",
    "loop_length",
    "muted",
    "notes",
];
pub(super) const NOTE_CLIP_FIELDS_V5: &[&str] = &[
    "id",
    "start",
    "length",
    "offset",
    "loop_length",
    "muted",
    "grid",
    "notes",
];
pub(super) const NOTE_CLIP_FIELDS_V16: &[&str] = &[
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
pub(super) const NOTE_CLIP_FIELDS_V19: &[&str] = &[
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
pub(super) const CONTROLLER_LANE_FIELDS: &[&str] = &["kind", "number", "points"];
pub(super) const AUTOMATION_CLIP_FIELDS: &[&str] = &[
    "id",
    "start",
    "length",
    "offset",
    "loop_length",
    "muted",
    "points",
];
