//! `.atomik` project files: one JSON document holding the rack (the
//! patch v2 rack fields) and the song.
//!
//! ```text
//! { "format": "atomik-project", "version": 37,
//!   "sample_rate", "tempo_bpm", "seed", "output_channels", "devices", "cables"  (as patch v2;
//!       a device may also have "zones", v4, written only when non-empty, "steps", v7,
//!       written only when present, "inner", v8, a container's inside, see below, and
//!       "plugin", v9, a plugin device's document, see below, and "patch", v36, the name of
//!       the patch the device took its sound from, written only when non-empty, and
//!       "midi_out", v37, a MIDI Out device's port: { "port": string }, written only when set)
//!   "input_channels": 0..=32,                                   (v3; optional, default 0)
//!   "console": { "channels": [{ "channel": key, "source": key | null }…],
//!                "buses"?: [{ "bus": key, "name": string }…]     (v12; written only when non-empty),
//!                "vcas"?: [{ "id", "name", "color"? ("#rrggbb"), "level_db"?, "mute"?, "solo"?,
//!                            "members": [key…] }…]            (v26; written only when non-empty),
//!                "snapshots"?: [{ "id", "name", "devices": {"<key>": {"<param>": value…}…},
//!                                 "vcas"?: [{ "id", "name", "level_db"?, "mute"?, "solo"?,
//!                                            "members": [key…] }…] }…]
//!                                                              (v27; written only when non-empty),
//!                "master": key | null, "returns": [key | null × 8], "compensation": bool },
//!   "midi_maps"?: [{ "id", "source", "target", options… }…]       (v28; written only when non-empty),
//!   "macros"?: { "assigns"?: [{ "id", "bank", "macro", "device", "param", "min", "max", "invert"? }…], "names"?: [{ "bank", "macro", "name" }…] }
//!                                                               (v29; written only when non-empty),
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
//!             "groups"?: [{"id", "name", "parent"?, "collapsed"?, "color"?}…]
//!                                                                     (v22; written only when non-empty),
//!             "tracks": [{ "device", "muted", "solo", "armed", "no_chase"? (v17),
//!                          "notes_to"? (v31, a device key: where the track's notes enter),
//!                          "group"?, "color"?, "comment"? (v22), "frozen"? (v25),
//!                          "lanes": [{ "kind": "notes" | "automation" | "audio",
//!                                      "name"?, "muted"? (v30, notes lanes, after "kind"),
//!                                      "param"?,
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
//!                                                      "fade_curve": "linear"|"equal_power"|…,
//!                                                      "reverse"?, "envelope"?, "xfade"? (v23),
//!                                                      "warp"? (v24) }
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
//! A CV sequencer's `"curves"` (v32) is `{"patterns": [...]}` with 1 to 8 patterns, each
//! `{"a": [...], "b": [...]}` of exactly 64 steps `{"value": -100..=100, "slide"}` (every field
//! required). Only a device whose type takes curves may have `"curves"`; one without it has flat
//! curves. Files before version 32 take none (unknown field).
//!
//! A drum generator's `"drums"` (v33) is `{"patterns": [...]}` with 1 to 8 patterns, each
//! `{"name", "length", "resolution", "swing", "lanes": [...]}` with exactly 16 lanes
//! `{"key", "role", "mute", "velocity": [64 numbers], "prob": [64 numbers]}` (every field
//! required). Only a device whose type takes drum data may have `"drums"`, and it has no
//! `"steps"`. Files before version 33 take none (unknown field).
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
//! **Version 4** files have no grids. **Version 5** zones have no `"reverse"`. **Version 6**
//! files have no steps. **Version 7** files have no containers. **Version 8** files have no
//! plugin devices. **Version 9** notes have no mute flag. **Version 11** files have no output
//! buses (`"console"."buses"` is an unknown field there): a bus is an `atomik.bus` device listed
//! in `"buses"` (console order) with a unique, trimmed, non-empty name of at most 32 characters;
//! buses need a master; their routing is ordinary cables. **Version 12** files have a single
//! meter and no tempo ramps: `"meter"` is `[beats_per_bar, beat_unit]` (it loads as one event at
//! bar 1; the reader takes that shape in any version) and a tempo event has no `"ramp"` (unknown
//! field there). From version 13 `"meter"` is written as a list of `{"bar", "num", "den"}`
//! changes, first at bar 1, bars strictly increasing, each starting on a bar line; a tempo event
//! with `"ramp": true` moves linearly (in BPM, over ticks) to the next event's BPM, and the last
//! event cannot ramp. **Version 13** files have no song markers: from version 14 a song may carry
//! `"markers"` (named points) and `"ranges"` (named sections, `start < end`, which may overlap),
//! each sorted by tick, with a trimmed non-empty name of at most 64 characters and an optional
//! `"#rrggbb"` colour; both are omitted when empty, and a file that lists them out of order loads
//! sorted (ids are not stored; a loaded song numbers them from 1). **Version 14** files have no
//! track heights: from version 15 a song may carry `"track_heights"`, an object from device key
//! to the height in pixels (at vertical zoom 1) of that track's first lane in the arrangement,
//! `24..=400`; it is omitted when empty, keys of devices without a track in the file are ignored,
//! and a track without an entry has the default height of 60. **Version 15** files have no clip
//! scales: from version 16 a note clip may carry `"scale": {"root": 0..=11, "kind": "major" |
//! "natural_minor" | "harmonic_minor" | "melodic_minor" | "dorian" | "phrygian" | "lydian" |
//! "mixolydian" | "locrian" | "major_pentatonic" | "minor_pentatonic" | "blues" | "whole_tone" |
//! "phrygian_dominant" | "hungarian_minor"}` after its `"grid"`; it is omitted when there is none
//! (a chromatic scale is not stored and is refused on load), and only note clips take it.
//! **Version 16** files have no chase flag: from version 17 a track may carry `"no_chase": true`
//! after its `"armed"` (a track that does not start the notes that span the play position on
//! play, locate and loop wrap); it is omitted when the track chases, so a file without it chases.
//! **Version 17** files have no lane write modes: from version 18 an automation lane may carry
//! `"write": "overwrite" | "latch"` after its `"param"` (how automation recording writes into it;
//! `"touch"`, the default, is not stored and is refused on load, and only automation lanes take
//! it). **Version 18** files have no controller lanes: from version 19 a note clip may carry
//! `"controllers": [{"kind": "pitch_bend" | "pressure" | "cc" | "poly_pressure", "number"?:
//! 0..=127 (the controller of a "cc" lane, the key of a "poly_pressure" lane; required there,
//! refused on the others), "points": [[tick, value, "linear"|"step"]…]}…]` after its
//! `"scale"` (pitch bend and aftertouch, content ticks like the notes): the lanes are sorted by
//! kind (pitch bend, pressure, cc by number, poly_pressure by key) without duplicates, each has
//! at least one point, the points are sorted by tick without duplicates, and a value lies in
//! `-1..=1` for pitch bend and `0..=1` for the others; the field is omitted when the clip has no
//! lanes, and only note clips take it. **Version 19** files have no take groups: from version 20
//! a notes or audio lane may carry `"take_groups": [{"takes": [<clip>…], "comp": [[take, start,
//! end]…], "clips": [id…]}…]` after its `"clips"` (see [`crate::takes`]): `"takes"` are
//! whole clips of the lane's kind (one to 256, same fields as the lane's clips, ids unique among
//! all clips of the song and below `next_clip_id`, none of them in the lane's `"clips"`);
//! `"comp"` lists segments of take indices over the group's range (the earliest take start to the
//! latest take end), sorted, disjoint and non-empty; `"clips"` names the lane clips compiled from
//! the comp (an id that is not on the lane, a comp clip removed or moved since, is dropped on
//! load and not written). The field is omitted when the lane has no groups; a loaded song numbers
//! its groups from 1 in file order (ids are not stored). **Version 20** files have no groove
//! library: from version 21 a song may carry `"grooves": [{"name", "step": "eighth" | "sixteenth"
//! | "thirty_second", "slots": [[offset, velocity]…]}…]` after its `"track_heights"` (see
//! [`crate::groove`]): a name is trimmed, non-empty, at most 32 characters, unique and not the
//! name of a built-in groove; a groove has 1 to 32 slots, each an `offset` in steps
//! (`-0.5..=0.5`) and a velocity scale (`0..=2`); at most 64 grooves; the field is omitted when
//! the song has none. **Version 21** files have no track organisation: from version 22 a song may
//! carry `"groups"` (folders: `"id"` from 1, unique; a trimmed `"name"` of 1 to 64 characters; an
//! optional `"parent"` folder, at most 8 levels deep and no loops; `"collapsed": true`; an
//! optional `"#rrggbb"` `"color"`; at most 256, written sorted by id) after its `"grooves"`, and
//! a track may carry `"group"` (its innermost folder), `"color"` and `"comment"` (trimmed, 1 to
//! 1000 characters, newlines allowed) after its `"no_chase"`, an automation lane `"folded": true`
//! after its `"write"`, and a take group `"open": true` after its `"clips"` (its take rows are
//! shown). Every folder has at least one track, and the tracks of a folder, with those of its
//! sub-folders, follow each other (see [`crate::organise`]); a file that breaks either is
//! refused. All of it is omitted when there is none. **Version 22** files have no audio editing:
//! from version 23 the `"audio"` object of an audio clip may carry `"reverse": true` (it plays
//! its span of the asset backwards), `"envelope": [[frame, gain_db]…]` (gain breakpoints in
//! frames from the clip start, strictly increasing, at most 512, each `gain_db` in `-120..=24`)
//! and `"xfade": "off" | "linear" | "equal_power" | "exponential" | "s_curve"` (the crossfade
//! with the clip it overlaps at its start); `"fade_curve"` also takes `"exponential"` and
//! `"s_curve"`. `reverse` and `envelope` are omitted when unused; a file of version 23 or later
//! without `"xfade"` crossfades with `"equal_power"` (the default of a new clip), while an older
//! file loads with every clip at `"off"`, so a project written before crossfades keeps its hard
//! cuts and its bounce. **Version 23** files have no warp: from version 24 the `"audio"` object
//! of an audio clip may carry `"warp": {"mode": "beats" | "tonal" | "repitch", "bpm": 20..=999,
//! "enabled"?: true, "transpose"?: cents, "markers"?: [[tick, source_frame]…]}` (see
//! [`crate::warp`]): `"enabled"` follows the song tempo (the clip plays its asset through a time
//! map in beats at `"bpm"`, the tempo it was recorded at), `"transpose"` is in cents within
//! ±2400 and changes pitch without changing length (Beats and Tonal), and `"markers"` pin a
//! source frame (from the clip's `"offset"`) to a tick from the clip start, at most 512, strictly
//! increasing in both. The object is omitted for the default (not enabled, `"beats"`, 120 BPM, no
//! transpose, no markers), a reversed clip cannot be warped or transposed, and a file written
//! before version 24 plays every clip in real time as it did. **Version 24** files have no frozen
//! tracks: from version 25 a track may carry `"frozen": {"start": tick, "length": ticks,
//! "effects"?: [device key…], "audio": {…}}` after its `"comment"` (see [`crate::freeze`]):
//! its instrument and the listed effects, which must be the first effects of the insert chain of
//! the console channel the track plays through (in chain order, none of them a container), are
//! replaced by `"audio"`, a plain audio clip object (an asset of the pool, no reverse, no warp)
//! that plays from `"start"` for `"length"` ticks. The track's clips, instrument and effects are
//! stored as they were. The field is omitted for a track that is not frozen, so a project without
//! one differs from its version 24 file in the version line alone. **Version 25** files have no
//! VCA groups: from version 26 the console may carry `"vcas"` after its `"buses"` (see
//! [`crate::console_vca`]): `[{"id", "name", "color"?, "level_db"?, "mute"?, "solo"?,
//! "members"}…]`, at most 64 groups; `"id"` counts from 1 and is unique, `"name"` is trimmed,
//! non-empty, at most 32 characters and unique ignoring case, `"color"` is `"#rrggbb"`,
//! `"level_db"` (the offset added to the members' faders) lies in `-60..=12` and is omitted at 0,
//! `"mute"` and `"solo"` are omitted when false, and `"members"` lists console channels or buses
//! (any order, loaded in console order), none of them in two groups. The field is omitted when
//! there are no groups, so a project without one differs from its version 25 file in the version
//! line alone, and plays and bounces bit for bit as it did. **Version 26** files have no mixer
//! snapshots: from version 27 the console may carry `"snapshots"` after its `"vcas"` (see
//! [`crate::console_snapshot`]): `[{"id", "name", "devices", "vcas"?}…]`, at most 16; `"id"`
//! counts from 1 and is unique, `"name"` is trimmed, non-empty, at most 32 characters and unique
//! ignoring case, `"devices"` maps a device key to the params of that device that are not at
//! their default (a strip's params, or the `"bypass"` of an insert or return effect; every
//! captured device is listed, with `{}` when all its params are at the default), and `"vcas"`
//! lists the groups as `{"id", "name", "level_db"?, "mute"?, "solo"?, "members"}`. A snapshot may
//! name devices, params and groups that do not exist (they were deleted after it was taken;
//! recall skips them), so the loader checks the shape and the numbers, not the references. The
//! compare slot is not saved. The field is omitted when there are no snapshots, so a project
//! without one differs from its version 26 file in the version line alone, and plays and bounces
//! bit for bit as it did. **Version 27** files have no MIDI mappings: from version 28 the root
//! may carry `"midi_maps"` after `"console"` (see [`crate::midi_map`] and `midi_map_io.rs`):
//! `[{"id", "source", "target", "min"?, "max"?, "invert"?, "mode"?, "step"?, "pickup"?,
//! "consume"?}…]`, at most 512, ids from 1 and unique, listed in id order; a source is a
//! `"cc"`, `"cc14"`, `"pitch_bend"` or `"note"` with its `"number"`, an optional `"channel"` and
//! an optional `"port"`; a target is a `"param"` (device key and param key), a `"vca"` group id,
//! `"tempo"`, a `"transport"` action or the `"selected"` strip's param. The loader checks the
//! shape and the numbers, not the references (a mapping may name a device that was deleted; it
//! does nothing). The field is omitted when there are no mappings, so a project without one
//! differs from its version 27 file in the version line alone, and plays and bounces bit for bit
//! as it did. **Version 28** files have no macros: from version 29 the root may carry `"macros"`
//! after `"midi_maps"` (see [`crate::macros`] and `macros_io.rs`): `{"assigns"?: [{"id", "bank",
//! "macro", "device", "param", "min", "max", "invert"?}…], "names"?: [{"bank", "macro",
//! "name"}…]}`; an assignment binds macro `"macro"` (1 to 8) of the `atomik.macro-bank` device
//! `"bank"` to a param of a top-level device with the plain values at macro 0 and macro 1; ids
//! count from 1 and are unique, at most 512, a param is assigned at most once, and the loader
//! checks the shape and the numbers, not the references (an assignment may name a device that was
//! deleted; it does nothing). A name is trimmed, 1 to 24 characters, one per bank and macro. The
//! field is omitted when there is nothing, so a project without macros differs from its version
//! 28 file in the version line alone, and plays and bounces bit for bit as it did. **Version 29**
//! files have one note lane per track: from version 30 a track may have several note lanes, a
//! leading block of `"notes"` lanes (one to 64, all feeding the track's device) before its
//! automation lanes, and a notes lane may carry `"name"` (trimmed, 1 to 32 characters, without
//! control characters) and `"muted": true` (a muted lane plays nothing; `false` is the default
//! and is refused) after its `"kind"`. A notes lane after an automation lane is refused, and
//! `"name"` and `"muted"` are unknown fields on automation and audio lanes. Both fields are
//! omitted when unset, so a project whose tracks each have one unnamed note lane differs from its
//! version 29 file in the version line alone, and plays and bounces bit for bit as it did.
//! **Version 30** files have no player stacks: from version 31 a track may carry `"notes_to"`
//! after its `"no_chase"` (the key of the device its notes enter, the top of a stack of note
//! players above its instrument; see [`crate::WorkflowCommand::CreatePlayer`]), omitted when the
//! notes enter the track's own device (naming that device is refused). The loader checks the
//! shape, not the reference: a `"notes_to"` that names a device that is not in the file is kept
//! and plays the track's own device ([`crate::RackDoc::notes_entry`]). The field is omitted when
//! unset, so a project without stacks differs from its version 30 file in the version line alone,
//! and plays and bounces bit for bit as it did. **Version 31** files have no CV sequencer curves:
//! from version 32 a device whose type takes curves (the CV sequencer) may carry `"curves"` after
//! its `"steps"`: `{"patterns": [{"a": [...], "b": [...]}…]}` with one to eight patterns
//! (pattern `i` goes with pattern `i` of `"steps"`; a pattern the file holds none for plays flat
//! zeros), each row `"a"` and `"b"` holding exactly 64 steps `{"value": -100..=100, "slide"}`
//! (every field required; steps beyond the pattern length are kept). The field is omitted when
//! the device has none (flat), so a project without curves differs from its version 31 file in
//! the version line alone, and plays and bounces bit for bit as it did. **Version 32** files have
//! no drum generator patterns: from version 33 a device whose type takes drum data (the drum
//! generator) may carry `"drums"` after its `"curves"` (such a device has no `"steps"`, and no
//! other type has `"drums"`): `{"patterns": [{"name", "length": 1..=64, "resolution", "swing":
//! 0..=75, "lanes": [...]}…]}` with one to eight patterns, each holding exactly 16 lanes
//! `{"key": 0..=127, "role": "kick" | "snare" | "clap" | "closed_hat" | "open_hat" | "tom_low" |
//! "tom_mid" | "tom_high" | "cymbal" | "percussion", "mute", "velocity": [...], "prob": [...]}`
//! where `"velocity"` (0 = no hit, up to 127) and `"prob"` (0..=100) hold exactly 64 steps each
//! (every field required; steps beyond the pattern length are kept). The field is omitted when
//! the device has none, so a project without a drum generator differs from its version 32 file in
//! the version line alone, and plays and bounces bit for bit as it did. **Version 33** files have
//! tracks only on devices with a Notes input (and audio-track devices): from version 34 a device
//! without one (a console channel or bus, an effect) may have a track that holds automation lanes
//! alone: every lane of it is an `"automation"` lane (at least one; its params are the device's,
//! never an `internal.` one), it takes no `"solo"`, `"armed"`, `"no_chase"`, `"notes_to"` or
//! `"frozen"` (they are refused; `"muted"` stops its automation), and a track on a device with a
//! Notes input still starts with its notes lane. The shape of a track and of its lanes is
//! otherwise unchanged, so a project without such a track differs from its version 33 file in the
//! version line alone, and plays and bounces bit for bit as it did. **Version 34** files have no
//! note tails: from version 35 a song may carry `"tails": true` after its `"next_clip_id"` (a
//! note plays to its own end even where its clip ends, its loop repeats or a later clip takes
//! over; only the note start is held to the audible span, see
//! [`crate::song::flatten_notes_with`]). It is omitted when off (`false` is refused), and a file
//! without it cuts every note at the end of its clip, loop pass or covering clip as before, so a
//! project written before version 35 plays and bounces bit for bit as it did. A new project has
//! it on. **Version 35** files have no held or static automation lanes: from version 36 an
//! automation lane may carry `"muted": true` (the lane drives nothing and its param keeps the
//! value it has; `false` is the default and is refused) and `"static": value` (the value of the
//! param's range the lane gives wherever no clip is audible: before the first clip, between clips
//! and after the last one, and for the whole song when it has no clip; see
//! [`crate::song::flatten_automation`]) after its `"folded"`; the value is clamped into the
//! param's range on load. Both are omitted when unset, and `"muted"` stays an unknown field on
//! audio lanes (notes lanes take it from version 30), so a project without them differs from its
//! version 35 file in the version line alone, and plays and bounces bit for bit as it did. From
//! version 36 a device may carry `"patch"` after its `"name"` (the name of the patch it last took
//! its sound from, trimmed, 1 to 64 characters, without control characters; display only),
//! omitted when empty (`""` is refused), so a project whose devices have none differs from its
//! version 35 file in the version line alone, and plays and bounces bit for bit as it did.
//! From version 37 a MIDI Out device (`atomik.midi-out`) may carry
//! `"midi_out": { "port": name }` after its `"plugin"` position (the output port it sends to:
//! trimmed, 1 to 128 characters, without control characters), omitted while the device uses the
//! fallback port of the app settings; the field is refused on any other device type and in a
//! version 36 file, so a project without a MIDI Out device differs from its version 36 file in the
//! version line alone, and plays and bounces bit for bit as it did. Saving always writes version 37.

use crate::console::{
    BUS_TYPE, CHANNEL_TYPE, Console, ConsoleBus, ConsoleChannel, FX_SLOTS, MASTER_TYPE,
    MAX_BUS_NAME_CHARS, is_console_type,
};
use crate::console::{
    MAX_SNAPSHOT_NAME_CHARS, MAX_SNAPSHOTS, MAX_VCA_NAME_CHARS, MAX_VCAS, MixerSnapshot,
    MixerState, SnapshotVca, VcaGroup,
};
use crate::controller::{ControllerKind, ControllerLane, check_lanes};
use crate::curves::{CURVE_VALUE_MAX, CurveDoc, CurvePatternDoc, CurveStepDoc};
use crate::drums::{DrumDoc, DrumLaneDoc, DrumPatternDoc, DrumStepDoc};
use crate::freeze::Frozen;
use crate::groove::{Groove, GrooveSlot, MAX_GROOVES, builtin_groove};
use crate::organise::{
    MAX_GROUP_DEPTH, MAX_GROUPS, MAX_TRACK_COMMENT, Organisation, TrackGroup, comment_ok,
    group_name_ok,
};
use crate::patch::{
    DeviceFormat, JsonPath, array, describe, f32_text, f64_text, json_str, load_rack, number,
    parse_color, string, uint, write_devices_and_cables,
};
use crate::song::{
    AUDIO_GAIN_DB_MAX, AUDIO_GAIN_DB_MIN, AudioClipContent, AutomationPoint, Clip, ClipContent,
    ClipId, Curve, FadeCurve, GainPoint, Lane, LaneKind, MAX_AUDIO_FRAMES, MAX_GAIN_POINTS,
    MAX_LOOP_REPEATS, MAX_MARKER_NAME, MAX_TRACK_HEIGHT, MIN_TRACK_HEIGHT, Marker, MeterEvent,
    Note, RangeMarker, Song, StepGrid, StepResolution, TempoEvent, Track,
};
use crate::song::{MAX_LANE_NAME_CHARS, MAX_NOTE_LANES, MAX_TICK};
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
use crate::warp::{MAX_WARP_MARKERS, WARP_TRANSPOSE_MAX_CENTS, WarpMarker, WarpMode, WarpSettings};
use crate::{AssetMeta, ZoneDoc, is_valid_asset_hash};
use crate::{DeviceKey, ModelError, PATCH_FORMAT, RackDoc, TEMPO_MAX, TEMPO_MIN, WriteMode, rules};
use atomik_core::{VCA_MAX_DB, VCA_MIN_DB};
use atomik_device_sdk::{DRUM_LANES, DrumRole, Registry};
use atomik_device_sdk::{LoopMode, TriggerMode};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::sync::Arc;

mod read_clip;
mod read_console;
mod read_devices;
mod read_legacy;
mod read_song;
mod read_track;
mod schema;
mod write;
mod write_clip;
mod write_console;
mod write_devices;

use read_clip::*;
use read_console::*;
use read_devices::*;
use read_legacy::*;
use read_song::*;
use read_track::*;
use schema::Obj;
use write_clip::*;
use write_console::*;

pub(crate) use read_clip::{clip_span, load_controllers, load_grid, load_notes, load_scale};
pub(crate) use read_devices::{load_curves, load_drums, load_steps, load_zones};
pub(crate) use write::save;
pub(crate) use write_clip::write_note_clip_tail;
pub(crate) use write_devices::{write_curves, write_drums, write_steps, write_zones};

/// The `format` tag of a project file.
pub const PROJECT_FORMAT: &str = "atomik-project";
/// The project format version [`crate::save_project`] writes. [`crate::load_project`] reads
/// it and versions 1 (no console section; migrated on load), 2 (no inputs, pool or audio) and
/// 3 (no zones), 4 (no grids), 5 (no zone reverse), 6 (no steps), 7 (no containers) and 8 (no
/// plugin devices) and 9 (no note mute) and 10 (no rack columns: one column on load) and 11
/// (no output buses) and 12 (no tempo ramps or meter changes) and 13 (no markers) and 14 (no
/// track heights) and 15 (no clip scales) and 21 (no track organisation) and 22 (no audio
/// editing) and 23 (no warp) and 24 (no frozen tracks) and 25 (no VCA groups) and 26 (no mixer
/// snapshots) and 27 (no MIDI mappings) and 28 (no macros) and 29 (one note lane per track) and
/// 30 (no player stacks) and 31 (no CV sequencer curves) and 32 (no drum generator patterns) and
/// 34 (no note tails) and 35 (no lane holds) and 36 (no MIDI Out port).
pub const PROJECT_VERSION: u32 = 37;

fn int(v: &Value, path: &JsonPath) -> Result<i64, ModelError> {
    v.as_i64()
        .ok_or_else(|| path.error(format!("expected an integer, found {v}")))
}

fn boolean(v: &Value, path: &JsonPath) -> Result<bool, ModelError> {
    v.as_bool()
        .ok_or_else(|| path.error(format!("expected a boolean, found {}", describe(v))))
}

/// An optional boolean field (default `false`).
pub(crate) fn flag(
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

/// [`load`] and the load warnings (see [`crate::load_project_with_warnings`]).
pub(crate) fn load_with_warnings(
    json: &str,
    registry: &Registry,
) -> Result<(RackDoc, Vec<String>), ModelError> {
    let root = JsonPath::default();
    let mut value: Value =
        serde_json::from_str(json).map_err(|e| root.error(format!("invalid JSON: {e}")))?;
    let mut warnings = Vec::new();
    // A file from a newer build that says an older reader may skip its additions.
    if schema::admit_newer(&mut value, &mut warnings)? {
        let text = value.to_string();
        let doc = load_value(&value, &text, registry, &mut warnings)?;
        return Ok((doc, warnings));
    }
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
            load_value(&value, &text, registry, &mut warnings)?
        };
        return Ok((doc, warnings));
    }
    let doc = load_value(&value, json, registry, &mut warnings)?;
    Ok((doc, warnings))
}

fn load_value(
    value: &Value,
    json: &str,
    registry: &Registry,
    warnings: &mut Vec<String>,
) -> Result<RackDoc, ModelError> {
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
    if !(1..=u64::from(PROJECT_VERSION)).contains(&version) {
        return Err(version_path.error(format!(
            "unsupported version {version}; this reader reads versions 1 to {PROJECT_VERSION}"
        )));
    }
    schema::object(value, &root, Obj::Root, version)?;
    let fmt = DeviceFormat {
        fields: schema::fields(Obj::Device, version),
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
    doc.song = Arc::new(load_song(
        &obj["song"],
        &root.field("song"),
        (&doc, registry),
        version,
        warnings,
    )?);
    if version == 1 {
        return Ok(migrate_v1(doc, registry, warnings));
    }
    doc.console = load_console(&obj["console"], &root.field("console"), &doc, version)?;
    if let Some(maps) = obj.get("midi_maps") {
        doc.midi_maps = crate::midi_map_io::load_midi_maps(maps, &root.field("midi_maps"))?;
    }
    if let Some(macros) = obj.get("macros") {
        doc.macros = crate::macros_io::load_macros(macros, &root.field("macros"))?;
    }
    check_frozen_tracks(&doc, registry, &root.field("song").field("tracks"))?;
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
