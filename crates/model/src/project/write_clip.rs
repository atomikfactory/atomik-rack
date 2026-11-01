//! Writing clips and take groups.

use super::*;

/// The JSON object of an audio clip's content (the value of `"audio"`).
pub(super) fn audio_json(a: &AudioClipContent) -> String {
    let mut extra = String::new();
    if a.reverse {
        extra.push_str(", \"reverse\": true");
    }
    if !a.envelope.is_empty() {
        let points: Vec<String> = a
            .envelope
            .iter()
            .map(|p| format!("[{}, {}]", p.frame, f32_text(p.gain_db)))
            .collect();
        let _ = write!(extra, ", \"envelope\": [{}]", points.join(", "));
    }
    if a.xfade != Some(FadeCurve::EqualPower) {
        let _ = write!(
            extra,
            ", \"xfade\": \"{}\"",
            a.xfade.map_or("off", FadeCurve::id)
        );
    }
    if a.warp != WarpSettings::default() {
        let ws = &a.warp;
        let mut warp = format!(
            "\"mode\": \"{}\", \"bpm\": {}",
            ws.mode.id(),
            f64_text(ws.source_bpm)
        );
        if ws.enabled {
            warp.push_str(", \"enabled\": true");
        }
        if ws.transpose_cents != 0 {
            let _ = write!(warp, ", \"transpose\": {}", ws.transpose_cents);
        }
        if !ws.markers.is_empty() {
            let markers: Vec<String> = ws
                .markers
                .iter()
                .map(|m| format!("[{}, {}]", m.tick, m.source))
                .collect();
            let _ = write!(warp, ", \"markers\": [{}]", markers.join(", "));
        }
        let _ = write!(extra, ", \"warp\": {{{warp}}}");
    }

    format!(
        "{{\"asset\": {}, \"offset\": {}, \"gain_db\": {}, \"fade_in\": {}, \"fade_out\": {}, \"fade_curve\": \"{}\"{extra}}}",
        json_str(&a.asset),
        a.source_offset,
        f32_text(a.gain_db),
        a.fade_in,
        a.fade_out,
        a.fade_curve.id()
    )
}

/// Writes a note clip's `"notes"` and, when set, its `"grid"`, `"scale"` and `"controllers"`
/// at `indent`, without a trailing newline. Shared by the project and clip file writers.
pub(crate) fn write_note_clip_tail(w: &mut String, c: &Clip, notes: &[Note], indent: &str) {
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
                ControllerKind::PolyPressure(k) => ("poly_pressure", format!(", \"number\": {k}")),
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

/// Writes one clip object of a lane's `"clips"` (or of a take group's `"takes"`), after the
/// separator `sep` of its list.
pub(super) fn write_clip(w: &mut String, c: &Clip, sep: &str) {
    let loop_length = c.loop_length.map_or("null".to_owned(), |n| n.to_string());
    let _ = write!(
        w,
        "{sep}\n              {{\n                \"id\": {}, \"start\": {}, \"length\": {}, \"offset\": {}, \"loop_length\": {}, \"muted\": {}{},\n",
        c.id.0,
        c.start,
        c.length,
        c.offset,
        loop_length,
        c.muted,
        clip_meta_text(c)
    );
    let indent = "                ";
    match &c.content {
        ClipContent::Audio(a) => {
            let _ = write!(w, "{indent}\"audio\": {}", audio_json(a));
        }
        ClipContent::Notes(notes) => write_note_clip_tail(w, c, notes, indent),
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
pub(super) fn write_take_groups(w: &mut String, lane: &Lane, open: &BTreeSet<u32>) {
    let groups = &lane.take_groups;
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
            .map(|s| {
                if s.xfade == 0 {
                    format!("[{}, {}, {}]", s.take, s.start, s.end)
                } else {
                    format!("[{}, {}, {}, {}]", s.take, s.start, s.end, s.xfade)
                }
            })
            .collect();
        // A comp clip removed or moved by a plain clip edit keeps its id in the group (the next
        // comp change skips it); only the ids still on the lane are written.
        let ids: Vec<String> = group
            .clips
            .iter()
            .filter(|id| lane.clips.iter().any(|c| c.id == **id))
            .map(|c| c.0.to_string())
            .collect();
        let _ = write!(
            w,
            "\n                ],\n                \"comp\": [{}],\n                \"clips\": [{}]{}\n              }}",
            comp.join(", "),
            ids.join(", "),
            if open.contains(&group.id) {
                ",\n                \"open\": true"
            } else {
                ""
            }
        );
    }
    let _ = write!(w, "\n            ]");
}

/// `items` joined as indented lines inside `[` … `]`, or `[]` when empty.
pub(crate) fn list(w: &mut String, indent: &str, items: impl IntoIterator<Item = String>) {
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

/// The `, "name": …, "color": …` text a clip adds after its `"muted"` field (empty when it has
/// neither). Shared by the project and clip file writers.
pub(crate) fn clip_meta_text(c: &Clip) -> String {
    let mut out = String::new();
    if let Some(name) = &c.name {
        let _ = write!(out, ", \"name\": {}", json_str(name));
    }
    if let Some([r, g, b]) = c.color {
        let _ = write!(out, ", \"color\": \"#{r:02x}{g:02x}{b:02x}\"");
    }
    out
}
