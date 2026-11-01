//! Writing a project: the root, the asset pool and the song.

use super::*;

/// Writes a project. Field, device, track, lane and clip order are fixed, so the output is
/// deterministic and canonical.
pub(crate) fn save(doc: &RackDoc) -> String {
    let mut out = String::new();
    let w = &mut out;
    // Writing to a String cannot fail.
    let _ = writeln!(w, "{{");
    let _ = writeln!(w, "  \"format\": {},", json_str(PROJECT_FORMAT));
    let _ = writeln!(w, "  \"version\": {PROJECT_VERSION},");
    // A format whose newest change an older reader can skip says so (see `schema`); today none
    // is, so no file carries these.
    let compat = schema::compat_version(schema::SPECS, PROJECT_VERSION);
    if compat < PROJECT_VERSION {
        let _ = writeln!(w, "  \"compat\": {compat},");
        let ignorable = schema::ignorable_paths(schema::SPECS, compat, PROJECT_VERSION);
        let rows: Vec<String> = ignorable
            .iter()
            .map(|(name, since)| format!("    {}: {since}", json_str(name)))
            .collect();
        let _ = writeln!(w, "  \"ignorable\": {{\n{}\n  }},", rows.join(",\n"));
    }
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
    crate::midi_map_io::write_midi_maps(w, &doc.midi_maps);
    crate::macros_io::write_macros(w, &doc.macros);
    crate::song_info::write_info(w, &doc.info);
    write_assets(w, &doc.assets);
    write_song(w, &doc.song);
    let _ = writeln!(w, "}}");
    out
}

pub(super) fn write_assets(w: &mut String, assets: &BTreeMap<String, AssetMeta>) {
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

pub(super) fn write_song(w: &mut String, song: &Song) {
    let org = song.normalised_org();
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
    if song.tails {
        let _ = writeln!(w, "    \"tails\": true,");
    }
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
    if !org.groups.is_empty() {
        let _ = write!(w, "    \"groups\": ");
        list(
            w,
            "    ",
            org.groups.iter().map(|g| {
                let parent = g
                    .parent
                    .map_or(String::new(), |p| format!(", \"parent\": {p}"));
                let collapsed = if g.collapsed {
                    ", \"collapsed\": true"
                } else {
                    ""
                };
                format!(
                    "{{\"id\": {}, \"name\": {}{parent}{collapsed}{}}}",
                    g.id,
                    json_str(&g.name),
                    color(g.color)
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
            "{sep}\n      {{\n        \"device\": {}, \"muted\": {}, \"solo\": {}, \"armed\": {},{}{}{}{}{}{}{}\n        \"lanes\": [",
            json_str(&t.device.0),
            t.muted,
            t.solo,
            t.armed,
            if t.chase { "" } else { " \"no_chase\": true," },
            if t.direct_record {
                " \"direct_record\": true,"
            } else {
                ""
            },
            t.notes_to
                .as_ref()
                .filter(|k| **k != t.device)
                .map_or(String::new(), |k| format!(
                    " \"notes_to\": {},",
                    json_str(&k.0)
                )),
            org.members
                .get(&t.device)
                .map_or(String::new(), |g| format!(" \"group\": {g},")),
            org.colors
                .get(&t.device)
                .map_or(String::new(), |[r, g, b]| {
                    format!(" \"color\": \"#{r:02x}{g:02x}{b:02x}\",")
                }),
            org.comments
                .get(&t.device)
                .map_or(String::new(), |c| format!(" \"comment\": {},", json_str(c))),
            t.frozen.as_ref().map_or(String::new(), |f| {
                let effects = if f.effects.is_empty() {
                    String::new()
                } else {
                    let keys: Vec<String> = f.effects.iter().map(|k| json_str(&k.0)).collect();
                    format!(" \"effects\": [{}],", keys.join(", "))
                };
                format!(
                    " \"frozen\": {{\"start\": {}, \"length\": {},{effects} \"audio\": {}}},",
                    f.start,
                    f.length,
                    audio_json(&f.audio)
                )
            })
        );
        for (j, lane) in t.lanes.iter().enumerate() {
            let sep = if j == 0 { "" } else { "," };
            let kind = match &lane.kind {
                LaneKind::Notes => {
                    let name = lane
                        .name
                        .as_ref()
                        .map_or(String::new(), |n| format!(", \"name\": {}", json_str(n)));
                    let muted = if lane.muted { ", \"muted\": true" } else { "" };
                    format!("\"kind\": \"notes\"{name}{muted}")
                }
                LaneKind::Audio => "\"kind\": \"audio\"".to_owned(),
                LaneKind::Automation(p) => {
                    let write = match song.write_mode(&t.device, p) {
                        WriteMode::Touch => String::new(),
                        mode => format!(", \"write\": \"{}\"", mode.name()),
                    };
                    let write = if org.folded_lanes.contains(&(t.device.clone(), p.clone())) {
                        format!("{write}, \"folded\": true")
                    } else {
                        write
                    };
                    let write = if lane.muted {
                        format!("{write}, \"muted\": true")
                    } else {
                        write
                    };
                    let write = match lane.static_value {
                        Some(v) => format!("{write}, \"static\": {}", f32_text(v)),
                        None => write,
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
                write_take_groups(w, lane, &org.open_takes);
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
