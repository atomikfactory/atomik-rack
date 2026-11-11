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
    if let Some(c) = a.fade_out_curve {
        let _ = write!(extra, ", \"fade_out_curve\": \"{}\"", c.id());
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
    if !a.pitch_edits.is_empty() {
        let edits: Vec<String> = a
            .pitch_edits
            .iter()
            .map(|e| {
                format!(
                    "[{}, {}, {}, {}, {}]",
                    e.src_start, e.src_end, e.shift_cents, e.drift, e.vibrato_keep
                )
            })
            .collect();
        let _ = write!(extra, ", \"pitch_edits\": [{}]", edits.join(", "));
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
    list_each(w, indent, notes, |w, n| {
        let _ = write!(w, "[{}, {}, {}, ", n.tick, n.length, n.key);
        write_f32(w, n.velocity);
        w.push_str(if n.muted { ", true]" } else { "]" });
    });
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
{indent}\"controllers\": "
        );
        write_controller_lanes(w, &c.controllers, indent);
    }
}

/// Writes a list of controller lanes as a JSON array whose lines start at `indent` (the closing
/// bracket too), without a trailing newline. A note clip's `"controllers"` and a track's own.
pub(crate) fn write_controller_lanes(w: &mut String, lanes: &[ControllerLane], indent: &str) {
    w.push('[');
    for (n, lane) in lanes.iter().enumerate() {
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
        list_each(w, &format!("{indent}  "), &lane.points, write_point);
        let _ = write!(w, "}}");
    }
    let _ = write!(
        w,
        "
{indent}]"
    );
}

/// Writes a note clip's `"param_lanes"` after its notes tail (nothing without lanes). The clip
/// file of the library leaves them out: a lane names devices of one project.
fn write_param_lanes(w: &mut String, c: &Clip, indent: &str) {
    if c.param_lanes.is_empty() {
        return;
    }
    let _ = write!(
        w,
        ",
{indent}\"param_lanes\": ["
    );
    for (n, lane) in c.param_lanes.iter().enumerate() {
        let sep = if n == 0 { "" } else { "," };
        let _ = write!(
            w,
            "{sep}
{indent}  {{\"device\": {}, \"param\": {}, \"points\": ",
            json_str(&lane.device.0),
            json_str(&lane.param)
        );
        list_each(w, &format!("{indent}  "), &lane.points, write_point);
        let _ = write!(w, "}}");
    }
    let _ = write!(
        w,
        "
{indent}]"
    );
}

/// Writes a note clip's `"programs"` and `"sysex"` after its parameter lanes (nothing for a clip
/// without any). A program change is `[tick, program]`, or `[tick, program, msb, lsb]` when a bank
/// select goes with it (`null` for the byte it lacks). A SysEx message is `[tick, "7E 7F 09 01"]` (hex bytes between `F0` and `F7`). The
/// clip file of the library leaves them out.
fn write_midi_data(w: &mut String, c: &Clip, indent: &str) {
    if !c.midi.programs.is_empty() {
        let _ = write!(
            w,
            ",
{indent}\"programs\": "
        );
        list_each(w, indent, &c.midi.programs, |w, p| {
            if p.has_bank() {
                let byte = |b: Option<u8>| b.map_or("null".to_owned(), |b| b.to_string());
                let _ = write!(
                    w,
                    "[{}, {}, {}, {}]",
                    p.tick,
                    p.program,
                    byte(p.bank_msb),
                    byte(p.bank_lsb)
                );
            } else {
                let _ = write!(w, "[{}, {}]", p.tick, p.program);
            }
        });
    }
    if !c.midi.sysex.is_empty() {
        let _ = write!(
            w,
            ",
{indent}\"sysex\": "
        );
        list_each(w, indent, &c.midi.sysex, |w, s| {
            let _ = write!(
                w,
                "[{}, \"{}\"]",
                s.tick,
                crate::midi_data::sysex_to_hex(&s.data)
            );
        });
    }
    write_expression(w, c, indent);
}

/// Writes a note clip's `"expression"`: one `[tick, key, pitch, pressure, timbre]` per note that
/// has curves and still exists in the clip (an entry of a note moved or deleted since is left out),
/// each curve a list of `[ticks after the note start, value]` (`[]` for a curve without points).
fn write_expression(w: &mut String, c: &Clip, indent: &str) {
    let ClipContent::Notes(notes) = &c.content else {
        return;
    };
    let kept: Vec<&crate::NoteExpression> = c
        .midi
        .expression
        .iter()
        .filter(|e| {
            !e.is_empty()
                && notes
                    .binary_search_by_key(&(e.tick, e.key), |n| (n.tick, n.key))
                    .is_ok()
        })
        .collect();
    if kept.is_empty() {
        return;
    }
    let _ = write!(
        w,
        ",
{indent}\"expression\": "
    );
    list_each(w, indent, &kept, |w, e| {
        let _ = write!(w, "[{}, {}", e.tick, e.key);
        for kind in crate::ExpressionKind::ALL {
            let _ = write!(w, ", [");
            for (i, p) in e.curve(kind).iter().enumerate() {
                let sep = if i == 0 { "" } else { ", " };
                let _ = write!(w, "{sep}[{}, ", p.tick);
                write_f32(w, p.value);
                let _ = write!(w, "]");
            }
            let _ = write!(w, "]");
        }
        let _ = write!(w, "]");
    });
}

/// Writes a note clip's `"groove"` (and `"groove_strength"` unless it is 100).
fn write_clip_groove(w: &mut String, c: &Clip, indent: &str) {
    let Some(g) = c.groove.as_deref() else {
        return;
    };
    let _ = write!(
        w,
        ",
{indent}\"groove\": {}",
        json_str(&g.name)
    );
    if g.strength < crate::groove::GROOVE_STRENGTH_MAX {
        let _ = write!(
            w,
            ",
{indent}\"groove_strength\": {}",
            g.strength
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
        ClipContent::Notes(notes) => {
            write_note_clip_tail(w, c, notes, indent);
            write_param_lanes(w, c, indent);
            write_midi_data(w, c, indent);
            write_clip_groove(w, c, indent);
        }
        ClipContent::Automation(points) => {
            let _ = write!(w, "{indent}\"points\": ");
            list_each(w, indent, points, write_point);
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
            "\n                ],\n                \"comp\": [{}],\n                \"clips\": [{}]{}{}\n              }}",
            comp.join(", "),
            ids.join(", "),
            if open.contains(&group.id) {
                ",\n                \"open\": true"
            } else {
                ""
            },
            match group.link {
                Some(tag) => format!(",\n                \"link\": {tag}"),
                None => String::new(),
            }
        );
    }
    let _ = write!(w, "\n            ]");
}

/// `items` joined as indented lines inside `[` … `]`, or `[]` when empty.
pub(crate) fn list(w: &mut String, indent: &str, items: impl IntoIterator<Item = String>) {
    list_each(w, indent, items, |w, item| w.push_str(&item));
}

/// [`list`] where `write_item` writes each item straight into the output: no text per item.
pub(crate) fn list_each<T>(
    w: &mut String,
    indent: &str,
    items: impl IntoIterator<Item = T>,
    mut write_item: impl FnMut(&mut String, T),
) {
    let mut any = false;
    for (i, item) in items.into_iter().enumerate() {
        w.push_str(if i == 0 { "[" } else { "," });
        let _ = write!(w, "\n{indent}  ");
        write_item(w, item);
        any = true;
    }
    if any {
        let _ = write!(w, "\n{indent}]");
    } else {
        w.push_str("[]");
    }
}

/// [`f32_text`] written into `w`.
fn write_f32(w: &mut String, x: f32) {
    if x == 0.0 && x.is_sign_negative() {
        w.push_str("-0.0");
    } else {
        let _ = write!(w, "{x}");
    }
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

/// One automation point: `[tick, value, "curve"]`, `[tick, value, "bend", amount]` for a bent one.
pub(super) fn write_point(w: &mut String, p: &AutomationPoint) {
    let _ = write!(w, "[{}, ", p.tick);
    write_f32(w, p.value);
    match p.curve {
        Curve::Linear => w.push_str(", \"linear\"]"),
        Curve::Step => w.push_str(", \"step\"]"),
        Curve::Bend(n) => {
            let _ = write!(w, ", \"bend\", {n}]");
        }
    }
}
