//! The stateless tools: they read files and write WAVs, and keep nothing between calls.

use crate::analysis;
use crate::rpc::{Server, Tool};
use crate::tool::{Args, ToolOutput, definition};
use atomik_library::{FileKind, classify};
use atomik_model::{
    ClipContent, ControllerKind, ItemKind, Lane, LaneKind, RackDoc, insert_bypass_states,
    inserts_bypassed, summarize_item,
};
use atomik_render::project::{ProjectOptions, is_project, render_doc_in};
use atomik_render::wav::{SampleFormat, read_wav_file, write_wav_file};
use atomik_render::{default_registry, patch::Patch, render_built};
use serde_json::{Value, json};
use std::num::NonZeroUsize;
use std::path::Path;

/// The stateless tools.
pub const TOOLS: &[Tool] = &[
    Tool {
        name: "list_devices",
        definition: list_devices_def,
        run: list_devices,
    },
    Tool {
        name: "project_inspect",
        definition: project_inspect_def,
        run: project_inspect,
    },
    Tool {
        name: "validate",
        definition: validate_def,
        run: validate,
    },
    Tool {
        name: "bounce",
        definition: bounce_def,
        run: bounce,
    },
    Tool {
        name: "analyze_audio",
        definition: analyze_audio_def,
        run: analyze_audio,
    },
    Tool {
        name: "compare_audio",
        definition: compare_audio_def,
        run: compare_audio,
    },
    Tool {
        name: "list_plugins",
        definition: list_plugins_def,
        run: list_plugins,
    },
];

fn r6(x: f32) -> f64 {
    (f64::from(x) * 1e6).round() / 1e6
}

/// A text of `path` (a project folder resolves to its `.atomik` file), and that file's path.
pub(crate) fn read_project_text(
    server: &Server,
    name: &str,
) -> Result<(std::path::PathBuf, String), String> {
    let path = server.paths().read(name)?;
    let file = atomik_render::bundle::resolve_project_path(&path);
    if file.is_dir() {
        return Err(format!(
            "{}: no project file (.atomik) in this folder",
            file.display()
        ));
    }
    let text = std::fs::read_to_string(&file)
        .map_err(|e| format!("{}: cannot read: {e}", file.display()))?;
    Ok((file, text))
}

// ---------------------------------------------------------------- list_devices

fn list_devices_def() -> Value {
    definition(
        "list_devices",
        "The built-in device types: type id, name, kind, parameter and socket counts. With \
`type_id`, the full descriptor (every parameter with its range and default, every socket). Use \
it to write a patch or a project that loads.",
        &[
            (
                "type_id",
                "string",
                "One device type id, e.g. atomik.poly-synth, for its full descriptor.",
            ),
            (
                "kind",
                "string",
                "Only this kind: instrument, effect, mixer, utility, interface, container.",
            ),
            (
                "query",
                "string",
                "Only types whose id or name contains this text (case-insensitive).",
            ),
        ],
        &[],
    )
}

fn kind_name(kind: atomik_device_sdk::DeviceKind) -> String {
    format!("{kind:?}").to_lowercase()
}

fn list_devices(server: &mut Server, args: &Args) -> Result<ToolOutput, String> {
    let _ = server;
    let registry = default_registry();
    if let Some(type_id) = args.opt_str("type_id")? {
        let d = registry
            .descriptor(type_id)
            .ok_or_else(|| format!("unknown device type `{type_id}`"))?;
        let params: Vec<Value> = d
            .params
            .iter()
            .map(|p| {
                json!({
                    "key": p.key, "name": p.name,
                    "min": r6(p.min), "max": r6(p.max), "default": r6(p.default),
                    "taper": format!("{:?}", p.taper), "unit": format!("{:?}", p.unit),
                    "discrete": p.discrete,
                })
            })
            .collect();
        let sockets: Vec<Value> = d
            .sockets
            .iter()
            .map(|s| {
                json!({
                    "key": s.key, "name": s.name,
                    "kind": format!("{:?}", s.kind).to_lowercase(),
                    "direction": format!("{:?}", s.direction).to_lowercase(),
                })
            })
            .collect();
        return Ok(ToolOutput::json(&json!({
            "type_id": d.type_id, "name": d.name, "kind": kind_name(d.kind),
            "version": d.version, "params": params, "sockets": sockets,
        })));
    }
    let kind = args.opt_str("kind")?.map(str::to_lowercase);
    let query = args.opt_str("query")?.map(str::to_lowercase);
    let mut out = Vec::new();
    for id in registry.type_ids() {
        let Some(d) = registry.descriptor(id) else {
            continue;
        };
        if kind.as_deref().is_some_and(|k| k != kind_name(d.kind)) {
            continue;
        }
        if let Some(q) = &query
            && !d.type_id.to_lowercase().contains(q)
            && !d.name.to_lowercase().contains(q)
        {
            continue;
        }
        out.push(json!({
            "type_id": d.type_id, "name": d.name, "kind": kind_name(d.kind),
            "params": d.params.len(), "sockets": d.sockets.len(),
        }));
    }
    Ok(ToolOutput::json(
        &json!({ "count": out.len(), "devices": out }),
    ))
}

// ------------------------------------------------------------- project_inspect

fn project_inspect_def() -> Value {
    definition(
        "project_inspect",
        "Loads a project (.atomik, or a bundle folder), a patch or a library item and describes it: \
devices, cables, the song (tracks, lanes, clips, tempo, loop, markers) and the console. `kind` is \
`project`, `patch`, or for a library item (by extension) `patch_item` (.atomikpatch), \
`container_item` (.atomikcontainer) or `clip` (.atomikclip: tempo, meter and the clip's length, \
offset, loop, notes); an item also has a one line `summary`. Fails with the JSON path of the \
first problem when the file does not load.",
        &[
            (
                "path",
                "string",
                "Project file, bundle folder, patch file or library item.",
            ),
            (
                "include_params",
                "boolean",
                "List every device's saved parameter values (default false).",
            ),
        ],
        &["path"],
    )
}

/// The controller lanes of a note lane: for each kind in use (pitch bend, aftertouch, CC, poly
/// aftertouch) its name, number (a CC's or a key) and the number of points over all the lane's
/// clips. Empty for a lane without controller data.
fn controller_summary(lane: &Lane) -> Vec<Value> {
    let mut counts: std::collections::BTreeMap<ControllerKind, usize> =
        std::collections::BTreeMap::new();
    for c in &lane.clips {
        for l in &c.controllers {
            *counts.entry(l.kind).or_default() += l.points.len();
        }
    }
    counts
        .into_iter()
        .map(|(kind, points)| match kind {
            ControllerKind::PitchBend => json!({"kind": "pitch_bend", "points": points}),
            ControllerKind::ChannelPressure => json!({"kind": "pressure", "points": points}),
            ControllerKind::Cc(n) => json!({"kind": "cc", "number": n, "points": points}),
            ControllerKind::PolyPressure(k) => {
                json!({"kind": "poly_pressure", "number": k, "points": points})
            }
        })
        .collect()
}

/// The audio clips of an audio lane, in lane order: id, start and length in ticks, the asset
/// (its first 8 hex digits), the source offset, the clip gain in dB, the fades, and what audio
/// editing added when it is in use: `reverse`, the `warp` settings (`enabled`, `mode`, source
/// `bpm`, `transpose_cents` and the number of `markers`), the number of `envelope` points and the
/// crossfade shape (`crossfade`, `off` for a hard cut) when the clip has a crossfade zone over
/// an earlier clip, with the zone's length in ticks. Empty for lanes of other kinds.
fn audio_summary(lane: &Lane) -> Vec<Value> {
    let zones = atomik_model::song::crossfades(lane);
    lane.clips
        .iter()
        .filter_map(|c| {
            let ClipContent::Audio(a) = &c.content else {
                return None;
            };
            let mut v = json!({
                "id": c.id.0, "start": c.start, "length": c.length, "muted": c.muted,
                "asset": a.asset.chars().take(8).collect::<String>(),
                "source_offset": a.source_offset, "gain_db": a.gain_db,
                "fade_in": a.fade_in, "fade_out": a.fade_out,
                "fade_curve": a.fade_curve.id(),
                "crossfade": a.xfade.map_or("off", atomik_model::FadeCurve::id),
            });
            if a.reverse {
                v["reverse"] = json!(true);
            }
            if a.warp != atomik_model::WarpSettings::default() {
                v["warp"] = json!({
                    "enabled": a.warp.enabled, "mode": a.warp.mode.id(),
                    "bpm": a.warp.source_bpm, "transpose_cents": a.warp.transpose_cents,
                    "markers": a.warp.markers.len(),
                });
            }
            if !a.envelope.is_empty() {
                v["envelope"] = json!(a.envelope.len());
            }
            if let Some(z) = zones.iter().find(|z| z.later == c.id) {
                v["crossfade_ticks"] = json!(z.end - z.start);
            }
            Some(v)
        })
        .collect()
}

/// The take groups of a lane: id, number of takes, range, the comp as `{take, start, end}`
/// (takes are 0-based), the take that plays alone when the comp is one whole pick, and the ids of
/// the lane clips the comp compiles to. Empty for a lane without groups.
fn take_summary(lane: &Lane, open: &std::collections::BTreeSet<u32>) -> Vec<Value> {
    lane.take_groups
        .iter()
        .map(|g| {
            let (start, end) = g.range();
            let comp: Vec<Value> = g
                .comp
                .iter()
                .map(|s| json!({"take": s.take, "start": s.start, "end": s.end}))
                .collect();
            let mut v = json!({
                "id": g.id,
                "takes": g.takes.len(),
                "start": start,
                "end": end,
                "comp": comp,
                "picked": g.picked(),
                "clips": g.clips.iter().map(|c| c.0).collect::<Vec<_>>(),
            });
            if open.contains(&g.id) {
                v["open"] = json!(true);
            }
            v
        })
        .collect()
}

/// The song's folders of tracks, in display order: id, name, parent, collapsed, colour and the
/// devices of the tracks inside (sub-folders included). Empty for a song without folders.
fn folder_summary(doc: &RackDoc) -> Vec<Value> {
    use atomik_model::OutlineKind;
    doc.song
        .outline()
        .into_iter()
        .filter_map(|e| match e.kind {
            OutlineKind::Folder(id) => doc.song.org.group(id).map(|g| {
                let mut v = json!({
                    "id": g.id, "name": g.name, "collapsed": g.collapsed,
                    "tracks": doc.song.group_tracks(id).into_iter()
                        .map(|i| doc.song.tracks[i].device.as_str().to_owned())
                        .collect::<Vec<_>>(),
                });
                if let Some(p) = e.parent {
                    v["parent"] = json!(p);
                }
                if let Some([r, gr, b]) = g.color {
                    v["color"] = json!(format!("#{r:02x}{gr:02x}{b:02x}"));
                }
                v
            }),
            OutlineKind::Track(_) => None,
        })
        .collect()
}

/// The song's groove library: name, step (`1/8`, `1/16`, `1/32`) and the slots as
/// `[offset in steps, velocity scale]`. Empty for a song without grooves (the built-in ones are
/// not listed).
fn groove_summary(doc: &RackDoc) -> Vec<Value> {
    doc.song
        .grooves
        .iter()
        .map(|g| {
            let step = match g.step {
                atomik_model::StepResolution::Eighth => "1/8",
                atomik_model::StepResolution::Sixteenth => "1/16",
                atomik_model::StepResolution::ThirtySecond => "1/32",
            };
            let slots: Vec<Value> = g
                .slots
                .iter()
                .map(|s| json!([s.offset, s.velocity]))
                .collect();
            json!({"name": g.name, "step": step, "slots": slots})
        })
        .collect()
}

/// The mixer snapshots: id and name (what the `mixer.*_snapshot` commands take as their argument),
/// how many strips and effects each holds, and whether the mix is exactly that snapshot now (what
/// a recall of it would change nothing of); `skips` lists what a recall would skip because it was
/// deleted.
fn snapshot_summary(doc: &RackDoc) -> Vec<Value> {
    let registry = default_registry();
    doc.console
        .snapshots
        .iter()
        .map(|s| {
            let mut v = json!({
                "id": s.id,
                "name": s.name,
                "devices": s.state.devices.len(),
                "vcas": s.state.vcas.len(),
                "matches_mix": doc.snapshot_matches(
                    s.id,
                    &registry,
                    atomik_model::SnapshotScope::ALL,
                ),
            });
            let skips = doc.snapshot_notes(s.id);
            if !skips.is_empty() {
                v["skips"] = json!(skips);
            }
            v
        })
        .collect()
}

/// The VCA groups: id (what `mixer.add_to_vca` and `mixer.delete_vca` take as their argument),
/// name, colour, offset in dB, mute, solo and the members with the level each plays at (its own
/// fader plus the offset; `null` for -inf).
fn vca_summary(doc: &RackDoc) -> Vec<Value> {
    let registry = default_registry();
    doc.console
        .vcas
        .iter()
        .map(|g| {
            let members: Vec<Value> = g
                .members
                .iter()
                .map(|m| {
                    let db = doc
                        .effective_fader_db(m, &registry)
                        .filter(|d| d.is_finite());
                    json!({"strip": m.as_str(), "effective_db": db})
                })
                .collect();
            let mut v = json!({
                "id": g.id,
                "name": g.name,
                "level_db": g.level_db,
                "mute": g.mute,
                "solo": g.solo,
                "members": members,
            });
            if let Some([r, gr, b]) = g.color {
                v["color"] = json!(format!("#{r:02x}{gr:02x}{b:02x}"));
            }
            v
        })
        .collect()
}

/// The channel strips (and buses) that use the strip upgrades or have inserts: the input trim in
/// dB, the polarity, the parallel compressor (when it is on or set), and the insert chain with
/// the bypass of each effect (`null` for an effect without one) and of the whole chain. A strip
/// with no trim, normal polarity, no parallel compression and no inserts is left out, so a
/// project that does not use any of it reads as before.
fn strip_summary(doc: &RackDoc) -> Vec<Value> {
    let registry = default_registry();
    let console = &doc.console;
    console
        .channels
        .iter()
        .map(|c| &c.channel)
        .chain(console.buses.iter().map(|b| &b.bus))
        .filter_map(|key| {
            let device = doc.device(key)?;
            let desc = registry.get(&device.type_id)?.descriptor;
            let value = |k: &str| {
                desc.param_by_key(k)
                    .map(|p| p.clamp(device.params.get(k).copied().unwrap_or(p.default)))
            };
            let trim = value("input.gain").unwrap_or(0.0);
            let inverted = value("input.phase").unwrap_or(0.0) >= 0.5;
            let parallel_on = value("pcomp.on").unwrap_or(0.0) >= 0.5;
            let parallel_set = parallel_on
                || desc
                    .params
                    .iter()
                    .filter(|p| p.key.starts_with("pcomp."))
                    .any(|p| device.params.contains_key(p.key));
            let inserts = insert_bypass_states(doc, &registry, key);
            let chain = atomik_model::insert_chain(doc, &registry, key);
            if trim == 0.0 && !inverted && !parallel_set && chain.is_empty() {
                return None;
            }
            let mut v = json!({
                "strip": key.as_str(),
                "input_gain_db": trim,
                "polarity_inverted": inverted,
            });
            if parallel_set {
                v["parallel"] = json!({
                    "on": parallel_on,
                    "threshold_db": value("pcomp.threshold"),
                    "ratio": value("pcomp.ratio"),
                    "attack_ms": value("pcomp.attack"),
                    "release_ms": value("pcomp.release"),
                    "makeup_db": value("pcomp.makeup"),
                    "mix": value("pcomp.mix"),
                });
            }
            if !chain.is_empty() {
                let effects: Vec<Value> = chain
                    .iter()
                    .map(|k| {
                        let bypassed = inserts.iter().find(|(e, _)| e == k).map(|(_, on)| *on);
                        json!({"device": k.as_str(), "bypassed": bypassed})
                    })
                    .collect();
                v["inserts"] = Value::Array(effects);
                v["inserts_bypassed"] = json!(inserts_bypassed(doc, &registry, key));
            }
            Some(v)
        })
        .collect()
}

/// The project's MIDI mappings: id, the source as text, the target as text and the options that
/// differ from the defaults.
fn midi_map_summary(doc: &RackDoc) -> Vec<Value> {
    doc.midi_maps
        .iter()
        .map(|m| {
            let o = &m.options;
            let mut v = json!({
                "id": m.id,
                "source": m.source.describe(),
                "target": m.target.describe(),
                "mode": o.mode.id(),
                "min": r6(o.min),
                "max": r6(o.max),
            });
            if o.invert {
                v["invert"] = json!(true);
            }
            if !o.pickup {
                v["pickup"] = json!(false);
            }
            if !o.consume {
                v["consume"] = json!(false);
            }
            v
        })
        .collect()
}

/// The description of a document, shared with `session_status`.
pub fn doc_json(doc: &RackDoc, include_params: bool) -> Value {
    let devices: Vec<Value> = doc
        .devices
        .iter()
        .map(|d| {
            let mut v = json!({
                "key": d.key.as_str(), "type_id": d.type_id, "name": d.name,
                "column": d.column, "folded": d.folded,
                "plugin": d.plugin.is_some(), "container": d.inner.is_some(),
                "sample_zones": d.zones.len(), "step_sequencer": d.steps.is_some(), "cv_curves": d.curves.is_some(),
                "drum_patterns": d.drums.as_ref().map_or(0, |x| x.patterns.len()),
            });
            if include_params {
                let params: serde_json::Map<String, Value> = d
                    .params
                    .iter()
                    .map(|(k, x)| (k.clone(), json!(r6(*x))))
                    .collect();
                v["params"] = Value::Object(params);
            }
            v
        })
        .collect();
    let cables: Vec<String> = doc
        .cables
        .iter()
        .map(|c| {
            format!(
                "{}:{} -> {}:{}",
                c.from.device, c.from.socket, c.to.device, c.to.socket
            )
        })
        .collect();
    let tracks: Vec<Value> = doc
        .song
        .tracks
        .iter()
        .map(|t| {
            let lanes: Vec<Value> = t
                .lanes
                .iter()
                .enumerate()
                .map(|(index, l)| {
                    let kind = match &l.kind {
                        LaneKind::Notes => "notes".to_string(),
                        LaneKind::Automation(p) => format!("automation:{p}"),
                        LaneKind::Audio => "audio".to_string(),
                    };
                    let items: usize = l
                        .clips
                        .iter()
                        .map(|c| match &c.content {
                            ClipContent::Notes(n) => n.len(),
                            ClipContent::Automation(a) => a.len(),
                            ClipContent::Audio(_) => 1,
                        })
                        .sum();
                    let mut lane = json!({"kind": kind, "clips": l.clips.len(), "items": items});
                    if t.is_note_lane(index) {
                        // The lane number the lane commands and the headers use, and what the
                        // header shows (the name, else Notes, Notes 2, ...).
                        lane["index"] = json!(index);
                        lane["label"] = json!(t.lane_label(index));
                        if let Some(name) = &l.name {
                            lane["name"] = json!(name);
                        }
                        if l.muted {
                            lane["muted"] = json!(true);
                        }
                    }
                    let controllers = controller_summary(l);
                    if !controllers.is_empty() {
                        lane["controllers"] = Value::Array(controllers);
                    }
                    let audio = audio_summary(l);
                    if !audio.is_empty() {
                        lane["audio_clips"] = Value::Array(audio);
                    }
                    let groups = take_summary(l, &doc.song.org.open_takes);
                    if !groups.is_empty() {
                        lane["take_groups"] = Value::Array(groups);
                    }
                    if let LaneKind::Automation(p) = &l.kind {
                        lane["write"] = json!(doc.song.write_mode(&t.device, p).name());
                        if doc.song.lane_folded(&t.device, p) {
                            lane["folded"] = json!(true);
                        }
                    }
                    lane
                })
                .collect();
            let mut track = json!({
                "device": t.device.as_str(), "muted": t.muted, "solo": t.solo,
                "armed": t.armed, "chase": t.chase, "lanes": lanes,
            });
            if let Some(entry) = &t.notes_to {
                // The top of the track's player stack: where its notes enter, when not at its
                // own device.
                track["notes_to"] = json!(entry.as_str());
            }
            if let Some(g) = doc.song.track_group(&t.device) {
                track["folder"] = json!(g);
            }
            if let Some([r, g, b]) = doc.song.track_color(&t.device) {
                track["color"] = json!(format!("#{r:02x}{g:02x}{b:02x}"));
            }
            let comment = doc.song.track_comment(&t.device);
            if !comment.is_empty() {
                track["comment"] = json!(comment);
            }
            if let Some(f) = &t.frozen {
                // The render a frozen track plays instead of its instrument and the effects it
                // froze (the track's own clips stay in `lanes`).
                track["frozen"] = json!({
                    "start": f.start, "length": f.length,
                    "asset": f.audio.asset.get(..8).unwrap_or(&f.audio.asset),
                    "effects": f.effects.iter().map(|e| e.as_str()).collect::<Vec<_>>(),
                });
            }
            track
        })
        .collect();
    let ppq = atomik_core::PPQ;
    let grooves = groove_summary(doc);
    let folders = folder_summary(doc);
    let mut out = json!({
        "sample_rate": doc.sample_rate,
        "tempo_bpm": doc.tempo_bpm,
        "output_channels": doc.output_channels,
        "input_channels": doc.input_channels,
        "devices": devices,
        "cables": cables,
        "plugins": doc.plugin_devices().len(),
        "assets": doc.assets.len(),
        "song": {
            "ticks_per_beat": ppq,
            "end_ticks": doc.song.end,
            "end_beats": doc.song.end as f64 / ppq as f64,
            "loop": {
                "enabled": doc.song.loop_enabled,
                "start_ticks": doc.song.loop_range.0,
                "end_ticks": doc.song.loop_range.1,
            },
            "tempo_events": doc.song.tempo.iter()
                .map(|t| json!({"tick": t.tick, "bpm": t.bpm, "ramp": t.ramp}))
                .collect::<Vec<_>>(),
            "meter_events": doc.song.meter.len(),
            "markers": doc.song.markers.iter()
                .map(|m| json!({"tick": m.tick, "name": m.name}))
                .collect::<Vec<_>>(),
            "ranges": doc.song.ranges.len(),
            "tracks": tracks,
        },
        "console": { "master": doc.console.master.is_some() },
    });
    let vcas = vca_summary(doc);
    if !vcas.is_empty() {
        out["console"]["vcas"] = Value::Array(vcas);
    }
    let snapshots = snapshot_summary(doc);
    if !snapshots.is_empty() {
        out["console"]["snapshots"] = Value::Array(snapshots);
    }
    if let Some(c) = &doc.console.compare {
        out["console"]["compare"] = json!({
            "snapshot": c.snapshot,
            "side": if c.on_snapshot { "B" } else { "A" },
        });
    }
    let strips = strip_summary(doc);
    if !strips.is_empty() {
        out["console"]["strips"] = Value::Array(strips);
    }
    let maps = midi_map_summary(doc);
    if !maps.is_empty() {
        out["midi_maps"] = Value::Array(maps);
    }
    let banks = crate::macro_tools::macros_json(doc);
    if !banks.is_empty() || !doc.macros.assigns.is_empty() {
        // An assignment whose bank is gone does nothing; it is counted, not listed.
        let orphans = doc
            .macros
            .assigns
            .iter()
            .filter(|a| doc.device(&a.bank).is_none())
            .count();
        out["macros"] = json!({"banks": banks});
        if orphans > 0 {
            out["macros"]["orphans"] = json!(orphans);
        }
    }
    if !grooves.is_empty() {
        out["song"]["grooves"] = Value::Array(grooves);
    }
    if !folders.is_empty() {
        out["song"]["folders"] = Value::Array(folders);
    }
    out
}

fn project_inspect(server: &mut Server, args: &Args) -> Result<ToolOutput, String> {
    let (file, text) = read_project_text(server, args.str("path")?)?;
    let item = item_kind(&file);
    if item == Some(FileKind::Clip) {
        return clip_inspect(&file, &text);
    }
    let doc = atomik_model::load_project(&text, &default_registry()).map_err(|e| e.to_string())?;
    let mut v = doc_json(&doc, args.opt_bool("include_params")?.unwrap_or(false));
    v["file"] = json!(file.to_string_lossy());
    v["kind"] = json!(match item {
        Some(FileKind::Patch) => "patch_item",
        Some(FileKind::Container) => "container_item",
        _ if is_project(&text) => "project",
        _ => "patch",
    });
    if let Some(summary) = item
        .and_then(model_kind)
        .and_then(|k| summarize_item(k, &text))
    {
        v["summary"] = json!(summary.detail);
    }
    Ok(ToolOutput::json(&v))
}

/// The library kind of a file by its extension (`.atomikpatch`, `.atomikcontainer`,
/// `.atomikclip`, in any case), or `None` for anything else.
fn item_kind(file: &Path) -> Option<FileKind> {
    classify(file, false)
        .filter(|k| matches!(k, FileKind::Patch | FileKind::Container | FileKind::Clip))
}

fn model_kind(kind: FileKind) -> Option<ItemKind> {
    match kind {
        FileKind::Patch => Some(ItemKind::Patch),
        FileKind::Container => Some(ItemKind::Container),
        FileKind::Clip => Some(ItemKind::Clip),
        _ => None,
    }
}

/// `project_inspect` of an `.atomikclip`: the clip's span, flags and counts.
fn clip_inspect(file: &Path, text: &str) -> Result<ToolOutput, String> {
    let clip = atomik_model::load_clip_file(text).map_err(|e| e.to_string())?;
    let c = &clip.clip;
    let notes = match &c.content {
        ClipContent::Notes(n) => n.len(),
        _ => 0,
    };
    let mut v = json!({
        "file": file.to_string_lossy(),
        "kind": "clip",
        "tempo_bpm": clip.tempo_bpm,
        "meter": {"num": clip.meter.0, "den": clip.meter.1},
        "clip": {
            "length": c.length,
            "offset": c.offset,
            "loop_length": c.loop_length,
            "muted": c.muted,
            "notes": notes,
            "grid": c.grid.is_some(),
            "scale": c.scale.is_some(),
            "controllers": c.controllers.len(),
        },
    });
    if let Some(summary) = summarize_item(ItemKind::Clip, text) {
        v["summary"] = json!(summary.detail);
    }
    Ok(ToolOutput::json(&v))
}

// ------------------------------------------------------------------- validate

fn validate_def() -> Value {
    definition(
        "validate",
        "Checks that a project or patch loads and builds, without rendering. The result is data: \
`valid` false carries the error (a JSON path and what is wrong), warnings are listed either way.",
        &[(
            "path",
            "string",
            "Project file, bundle folder or patch file.",
        )],
        &["path"],
    )
}

fn validate(server: &mut Server, args: &Args) -> Result<ToolOutput, String> {
    let (file, text) = read_project_text(server, args.str("path")?)?;
    let registry = default_registry();
    let item = item_kind(&file);
    let project = is_project(&text);
    let outcome: Result<Vec<String>, String> = match item {
        Some(FileKind::Clip) => atomik_model::load_clip_file(&text)
            .map(|_| Vec::new())
            .map_err(|e| e.to_string()),
        Some(kind) => atomik_model::load_device_item(&text, &registry)
            .map_err(|e| e.to_string())
            .and_then(|snapshot| {
                if kind == FileKind::Container && ItemKind::of(&snapshot) != ItemKind::Container {
                    Err("a container item holds exactly one container device".to_owned())
                } else {
                    Ok(Vec::new())
                }
            }),
        None if project => atomik_model::load_project_with_warnings(&text, &registry)
            .map(|(_, w)| w)
            .map_err(|e| e.to_string()),
        None => Patch::from_json(&text)
            .map_err(|e| e.to_string())
            .and_then(|p| {
                atomik_render::build::build(&p, &registry)
                    .map(|b| b.warnings.iter().map(ToString::to_string).collect())
                    .map_err(|e| e.to_string())
            }),
    };
    let kind = match item {
        Some(FileKind::Patch) => "patch_item",
        Some(FileKind::Container) => "container_item",
        Some(_) => "clip",
        None if project => "project",
        None => "patch",
    };
    Ok(ToolOutput::json(&match outcome {
        Ok(warnings) => json!({
            "valid": true, "kind": kind, "file": file.to_string_lossy(), "warnings": warnings,
        }),
        Err(error) => json!({
            "valid": false, "kind": kind, "file": file.to_string_lossy(), "error": error,
        }),
    }))
}

// --------------------------------------------------------------------- bounce

fn bounce_def() -> Value {
    definition(
        "bounce",
        "Renders a project (its song, end + 2 beats) or a patch offline to a WAV file, the same \
code as `atomik-render`, and returns the audio report (levels, silence, loudness windows, \
spectrum). With `reference`, also compares the render with that WAV. Projects that use \
third-party plugins are refused: bounce those with `atomik-render --allow-plugins`.",
        &[
            (
                "path",
                "string",
                "Project file, bundle folder or patch file.",
            ),
            (
                "output",
                "string",
                "Output WAV (default target/mcp/<name>.wav under the first allowed folder).",
            ),
            ("format", "string", "f32 (default) or pcm24."),
            (
                "block",
                "integer",
                "Host block size in frames (default 512); the output does not depend on it.",
            ),
            (
                "workers",
                "integer",
                "Engine worker threads, projects only (default 0); the output does not depend on it.",
            ),
            (
                "loop_count",
                "integer",
                "Projects only: play the song loop this many times.",
            ),
            (
                "window_s",
                "number",
                "Loudness window of the report in seconds (default 0.5).",
            ),
            ("reference", "string", "A WAV to compare the render with."),
            (
                "tolerance_db",
                "number",
                "With reference: the largest loudness difference per window that still matches (default 0.5).",
            ),
        ],
        &["path"],
    )
}

/// What a bounce produced.
pub struct Bounced {
    /// One buffer per output channel.
    pub channels: Vec<Vec<f32>>,
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Warnings from building.
    pub warnings: Vec<String>,
    /// Wall time in seconds.
    pub wall_s: f64,
}

/// Bounces a project document, refusing third-party plugins (they need `atomik-render
/// --allow-plugins`, with its sandbox and plugin cache).
///
/// # Errors
/// The renderer's message, or the plugins refusal.
pub fn render_doc_checked(
    doc: &RackDoc,
    registry: &atomik_device_sdk::Registry,
    block: usize,
    workers: u16,
    loop_count: Option<u32>,
    bundle_root: Option<&Path>,
) -> Result<Bounced, String> {
    let block = NonZeroUsize::new(block).ok_or("block must be at least 1")?;
    let used = doc.plugin_devices();
    if !used.is_empty() {
        let names: Vec<String> = used
            .iter()
            .map(|(_, d)| format!("{} ({})", d.name, d.type_id))
            .collect();
        return Err(format!(
            "this project uses third-party plugins ({}); bounce it with `atomik-render --allow-plugins`",
            names.join(", ")
        ));
    }
    let options = ProjectOptions {
        host_block: block,
        workers,
        loop_count,
    };
    #[allow(clippy::disallowed_methods)]
    // Justification: wall time is only reported; it never influences a sample.
    let started = std::time::Instant::now();
    let mut r = render_doc_in(doc, registry, &options, bundle_root).map_err(|e| e.to_string())?;
    Ok(Bounced {
        warnings: r.warnings.drain(..).map(|w| w.to_string()).collect(),
        channels: r.channels,
        sample_rate: r.sample_rate,
        wall_s: started.elapsed().as_secs_f64(),
    })
}

/// Renders project text (`project` true) or patch text.
///
/// # Errors
/// The renderer's message (a JSON path or file for problems in the file).
pub fn render_text(
    text: &str,
    bundle_of: &Path,
    block: usize,
    workers: u16,
    loop_count: Option<u32>,
) -> Result<Bounced, String> {
    let registry = default_registry();
    if is_project(text) {
        let doc = atomik_model::load_project(text, &registry).map_err(|e| e.to_string())?;
        let root = atomik_render::bundle::existing_bundle_root(bundle_of);
        render_doc_checked(&doc, &registry, block, workers, loop_count, root.as_deref())
    } else {
        if workers != 0 || loop_count.is_some() {
            return Err("workers and loop_count apply to projects, not patches".to_string());
        }
        let block = NonZeroUsize::new(block).ok_or("block must be at least 1")?;
        #[allow(clippy::disallowed_methods)]
        // Justification: wall time is only reported; it never influences a sample.
        let started = std::time::Instant::now();
        let patch = Patch::from_json(text).map_err(|e| e.to_string())?;
        let mut built =
            atomik_render::build::build(&patch, &registry).map_err(|e| e.to_string())?;
        let warnings = built.warnings.drain(..).map(|w| w.to_string()).collect();
        Ok(Bounced {
            channels: render_built(built, block),
            sample_rate: patch.sample_rate,
            warnings,
            wall_s: started.elapsed().as_secs_f64(),
        })
    }
}

/// The report of a bounce: the audio report plus the run's numbers.
pub fn bounce_json(b: &Bounced, window_s: f64) -> Value {
    let frames = analysis::frames(&b.channels);
    let seconds = frames as f64 / f64::from(b.sample_rate.max(1));
    let mut v = json!({
        "wall_s": (b.wall_s * 1000.0).round() / 1000.0,
        "realtime_factor": if b.wall_s > 0.0 { (seconds / b.wall_s * 10.0).round() / 10.0 } else { 0.0 },
        "warnings": b.warnings,
    });
    v["audio"] = analysis::report(&b.channels, b.sample_rate, window_s);
    v
}

fn sample_format(args: &Args) -> Result<SampleFormat, String> {
    match args.opt_str("format")? {
        None | Some("f32") => Ok(SampleFormat::F32),
        Some("pcm24") => Ok(SampleFormat::Pcm24),
        Some(other) => Err(format!("format must be f32 or pcm24, not `{other}`")),
    }
}

fn block_arg(args: &Args) -> Result<usize, String> {
    let block = args.opt_u64("block")?.unwrap_or(512);
    if (1..=1_048_576).contains(&block) {
        Ok(block as usize)
    } else {
        Err("block must be 1..=1048576".to_string())
    }
}

/// Writes a bounce to `out` and, with a reference WAV, compares them.
fn finish_bounce(
    server: &Server,
    b: &Bounced,
    out: &Path,
    format: SampleFormat,
    args: &Args,
) -> Result<Value, String> {
    write_wav_file(out, &b.channels, b.sample_rate, format).map_err(|e| e.to_string())?;
    let window_s = args.opt_f64("window_s")?.unwrap_or(0.5);
    let mut v = bounce_json(b, window_s);
    v["output"] = json!(out.to_string_lossy());
    if let Some(reference) = args.opt_str("reference")? {
        let tolerance = args.opt_f64("tolerance_db")?.unwrap_or(0.5);
        let wav = read_wav_file(&server.paths().read(reference)?).map_err(|e| e.to_string())?;
        let c = analysis::compare(
            &b.channels,
            b.sample_rate,
            &wav.channels,
            wav.sample_rate,
            window_s,
        );
        v["comparison"] = c.to_json(tolerance);
        v["comparison"]["reference"] = json!(reference);
    }
    Ok(v)
}

fn bounce(server: &mut Server, args: &Args) -> Result<ToolOutput, String> {
    let (file, text) = read_project_text(server, args.str("path")?)?;
    let workers = u16::try_from(args.opt_u64("workers")?.unwrap_or(0))
        .ok()
        .filter(|w| *w <= 64)
        .ok_or("workers must be 0..=64")?;
    let loop_count = args
        .opt_u64("loop_count")?
        .map(|n| u32::try_from(n).ok().filter(|n| (1..=10_000).contains(n)))
        .map(|n| n.ok_or("loop_count must be 1..=10000"))
        .transpose()?;
    let b = render_text(&text, &file, block_arg(args)?, workers, loop_count)?;
    let stem = file.file_stem().map_or_else(
        || "bounce".to_string(),
        |s| s.to_string_lossy().into_owned(),
    );
    let out = match args.opt_str("output")? {
        Some(o) => server.paths().write(o)?,
        None => server
            .paths()
            .write(&server.paths().default_output(&format!("{stem}.wav")))?,
    };
    let v = finish_bounce(server, &b, &out, sample_format(args)?, args)?;
    Ok(ToolOutput::json(&v))
}

// -------------------------------------------------------------- analyze_audio

fn analyze_audio_def() -> Value {
    definition(
        "analyze_audio",
        "Measures a WAV file: peak and RMS in dBFS per channel, DC offset, clipped and \
non-finite samples, silent stretches, RMS per window (the loudness fingerprint), and a coarse \
spectrum (eight bands and the dominant frequency). dB values are floored at -240.",
        &[
            ("path", "string", "WAV file (32-bit float or 24-bit PCM)."),
            (
                "window_s",
                "number",
                "Loudness window in seconds (default 0.5).",
            ),
        ],
        &["path"],
    )
}

fn analyze_audio(server: &mut Server, args: &Args) -> Result<ToolOutput, String> {
    let path = server.paths().read(args.str("path")?)?;
    let wav = read_wav_file(&path).map_err(|e| e.to_string())?;
    let mut v = analysis::report(
        &wav.channels,
        wav.sample_rate,
        args.opt_f64("window_s")?.unwrap_or(0.5),
    );
    v["file"] = json!(path.to_string_lossy());
    Ok(ToolOutput::json(&v))
}

// -------------------------------------------------------------- compare_audio

fn compare_audio_def() -> Value {
    definition(
        "compare_audio",
        "Compares two WAV files: same shape (rate, channels, length), bit-identical, largest \
sample difference, the first differing frame, and the largest per-window loudness difference in \
dB with where it happens. `matches` is true when the shapes agree and no window differs by more \
than `tolerance_db` (the goldens use 0.5).",
        &[
            ("a", "string", "First WAV (usually the reference)."),
            ("b", "string", "Second WAV."),
            ("tolerance_db", "number", "Default 0.5."),
            (
                "window_s",
                "number",
                "Loudness window in seconds (default 0.5).",
            ),
        ],
        &["a", "b"],
    )
}

fn compare_audio(server: &mut Server, args: &Args) -> Result<ToolOutput, String> {
    let read = |name: &str| read_wav_file(&server.paths().read(name)?).map_err(|e| e.to_string());
    let (a, b) = (read(args.str("a")?)?, read(args.str("b")?)?);
    let c = analysis::compare(
        &a.channels,
        a.sample_rate,
        &b.channels,
        b.sample_rate,
        args.opt_f64("window_s")?.unwrap_or(0.5),
    );
    Ok(ToolOutput::json(
        &c.to_json(args.opt_f64("tolerance_db")?.unwrap_or(0.5)),
    ))
}

// --------------------------------------------------------------- list_plugins

fn list_plugins_def() -> Value {
    definition(
        "list_plugins",
        "The third-party plugins the app has scanned (read from its scan cache, plugins.json in \
the app's config folder): type id, name, vendor, format, file, and the blacklist. Scan with the \
app first; this tool never loads a plugin.",
        &[(
            "cache",
            "string",
            "A plugins.json to read instead of the app's.",
        )],
        &[],
    )
}

fn list_plugins(server: &mut Server, args: &Args) -> Result<ToolOutput, String> {
    let cache_path = args
        .opt_str("cache")?
        .map(|c| server.paths().read(c))
        .transpose()?;
    let (cache, status) = atomik_render::plugins::load_cache(cache_path.as_deref());
    let mut plugins = Vec::new();
    for file in cache.files() {
        for p in &file.plugins {
            plugins.push(json!({
                "type_id": p.plugin.type_id(),
                "name": p.layout.name, "vendor": p.layout.vendor, "version": p.layout.version,
                "format": p.plugin.format.name(),
                "file": file.path.to_string_lossy(),
                "params": p.layout.params.len(),
                "unsupported": p.unsupported,
            }));
        }
    }
    let blacklist: Vec<Value> = cache
        .blacklist()
        .map(|b| json!({"file": b.path.to_string_lossy(), "format": b.format.name(), "reason": b.reason}))
        .collect();
    Ok(ToolOutput::json(&json!({
        "cache": format!("{status:?}"),
        "count": plugins.len(),
        "plugins": plugins,
        "blacklist": blacklist,
    })))
}
