//! Reading clips: note, automation and audio content, take groups and warp.

use super::*;

/// Reads a lane's `"take_groups"` (ids are numbered later, in file order).
pub(super) fn load_take_groups(
    v: &Value,
    path: &JsonPath,
    (clip_obj, version): (Obj, u64),
    kind: &LaneKind,
    (doc, desc): (&RackDoc, &atomik_device_sdk::DeviceDescriptor),
    song: &Song,
    (lane_clips, clip_first, loose): (&[Clip], &mut ClipFirst, &mut Loose),
) -> Result<Vec<TakeGroup>, ModelError> {
    let mut claimed: BTreeMap<u64, JsonPath> = BTreeMap::new();
    let mut groups = Vec::new();
    for (g, item) in array(v, path)?.iter().enumerate() {
        let gp = path.index(g);
        let obj = schema::object(item, &gp, Obj::TakeGroup, version)?;
        loose.open.push(flag(obj, &gp, "open")?);
        let takes_path = gp.field("takes");
        let mut takes = Vec::new();
        for (k, t) in array(&obj["takes"], &takes_path)?.iter().enumerate() {
            let tp = takes_path.index(k);
            let clip = load_clip(
                t,
                &tp,
                (clip_obj, version),
                kind,
                (doc, desc),
                song,
                clip_first,
            )?;
            if clip.loop_length.is_some() && clip.content.is_audio() {
                return Err(tp
                    .field("loop_length")
                    .error("an audio take cannot loop (convert the loop to clips first)"));
            }
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
            let a = comp_tuple(item, &sp)?;
            group.comp.push(CompSegment {
                take: uint(&a[0], &sp.index(0), u64::from(u32::MAX))? as usize,
                start: tick(&a[1], &sp.index(1))?,
                end: tick(&a[2], &sp.index(2))?,
                xfade: match a.get(3) {
                    Some(x) => uint(x, &sp.index(3), crate::song::MAX_TICK as u64)?,
                    None => 0,
                },
            });
        }
        check_comp(&group.comp, group.takes.len(), group.range())
            .map_err(|m| comp_path.error(m))?;
        let ids_path = gp.field("clips");
        for (i, item) in array(&obj["clips"], &ids_path)?.iter().enumerate() {
            let ip = ids_path.index(i);
            let id = uint(item, &ip, u64::MAX)?;
            // An id that is not on the lane names a comp clip that was removed or moved by a
            // plain clip edit (files saved before the writer dropped those ids); the group only
            // keeps the ids to swap its clips on the next comp change, so it is dropped.
            if !lane_clips.iter().any(|c| c.id.0 == id) {
                loose.warnings.push(format!(
                    "{ip}: comp clip {id} is not on the lane and was dropped from the take group"
                ));
                continue;
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

pub(super) fn load_clip(
    v: &Value,
    path: &JsonPath,
    (clip_obj, version): (Obj, u64),
    kind: &LaneKind,
    (doc, desc): (&RackDoc, &atomik_device_sdk::DeviceDescriptor),
    song: &Song,
    clip_first: &BTreeMap<u64, JsonPath>,
) -> Result<Clip, ModelError> {
    let content_field = match kind {
        LaneKind::Notes => "notes",
        LaneKind::Audio => "audio",
        LaneKind::Automation(_) => "points",
    };
    let obj = schema::object(v, path, clip_obj, version)?;
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
    let (length, offset, loop_length) = clip_span(obj, path)?;
    let loop_path = path.field("loop_length");
    if kind.is_audio() && offset != 0 {
        return Err(path
            .field("offset")
            .error("an audio clip has no content offset (use audio.offset)"));
    }
    let muted = flag(obj, path, "muted")?;
    let (name, color) = load_clip_meta(obj, path)?;
    let content_path = path.field(content_field);
    if kind.is_audio() {
        let audio = load_audio(&obj["audio"], &content_path, doc, version)?;
        if loop_length.is_some() && audio.reverse {
            return Err(loop_path.error("a reversed audio clip cannot loop"));
        }
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
            name,
            color,
            content: ClipContent::Audio(audio),
        });
    }
    let items = array(&obj[content_field], &content_path)?;
    let content = match kind {
        LaneKind::Audio => unreachable!("audio clips return above"),
        LaneKind::Notes => ClipContent::Notes(load_notes(items, &content_path, version)?.into()),
        LaneKind::Automation(param) => {
            // `None`: a dead plugin param, whose values are kept as they are.
            let pd = desc.param_by_key(param);
            let mut points: Vec<AutomationPoint> = Vec::with_capacity(items.len());
            let lazy = JsonPath::lazy();
            for (i, item) in items.iter().enumerate() {
                let point = match read_point(item, &lazy, pd) {
                    Ok(point) => point,
                    Err(_) => read_point(item, &content_path.index(i), pd)?,
                };
                if let Some(prev) = points.last()
                    && prev.tick >= point.tick
                {
                    return Err(content_path
                        .index(i)
                        .error("points must be sorted by tick without duplicates"));
                }
                points.push(point);
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
        name,
        color,
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

/// One point of an automation clip, checked on its own. `pd` is the automated param, `None` for a
/// dead plugin param whose values are kept as they are.
fn read_point(
    item: &Value,
    p: &JsonPath,
    pd: Option<&atomik_core::ParamDescriptor>,
) -> Result<AutomationPoint, ModelError> {
    let a = tuple(item, p, 3, "a point [tick, value, curve]")?;
    let tick = tick(&a[0], &p.index(0))?;
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
    Ok(AutomationPoint { tick, value, curve })
}

/// A clip's `"length"`, optional `"offset"` and optional `"loop_length"` (`null` is none), with
/// the repetition limit of a looped clip. Shared by the project and clip file loaders.
pub(crate) fn clip_span(
    obj: &serde_json::Map<String, Value>,
    path: &JsonPath,
) -> Result<(i64, i64, Option<i64>), ModelError> {
    let len = length(&obj["length"], &path.field("length"))?;
    let offset = match obj.get("offset") {
        Some(v) => tick(v, &path.field("offset"))?,
        None => 0,
    };
    let loop_path = path.field("loop_length");
    let loop_length = match obj.get("loop_length") {
        None | Some(Value::Null) => None,
        Some(v) => {
            let n = length(v, &loop_path)?;
            if len > n * MAX_LOOP_REPEATS {
                return Err(loop_path.error(format!(
                    "the clip ({len} ticks) holds more than {MAX_LOOP_REPEATS} repetitions of {n}"
                )));
            }
            Some(n)
        }
    };
    Ok((len, offset, loop_length))
}

/// One note of a `"notes"` list, checked on its own.
fn read_note(item: &Value, p: &JsonPath, version: u64) -> Result<Note, ModelError> {
    let n = note_tuple(item, p, version)?;
    let note = Note {
        muted: match n.get(4) {
            Some(m) => boolean(m, &p.index(4))?,
            None => false,
        },
        ..Note::new(
            tick(&n[0], &p.index(0))?,
            length(&n[1], &p.index(1))?,
            uint(&n[2], &p.index(2), 127)? as u8,
            number(&n[3], &p.index(3))? as f32,
        )
    };
    check_note(note.tick, note.length, note.key, note.velocity).map_err(|m| p.index(3).error(m))?;
    Ok(note)
}

/// Reads a note clip's `"notes"` list (`path` is the list's path): each note is checked and
/// the list must be sorted by `(tick, key)` without duplicates.
///
/// A note is read with a lazy path (no text is built); one that fails is read again with its
/// real path, so the error names it exactly.
pub(crate) fn load_notes(
    items: &[Value],
    content_path: &JsonPath,
    version: u64,
) -> Result<Vec<Note>, ModelError> {
    let lazy = JsonPath::lazy();
    let mut notes: Vec<Note> = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let note = match read_note(item, &lazy, version) {
            Ok(note) => note,
            Err(_) => read_note(item, &content_path.index(i), version)?,
        };
        if let Some(prev) = notes.last()
            && (prev.tick, prev.key) >= (note.tick, note.key)
        {
            return Err(content_path
                .index(i)
                .error("notes must be sorted by (tick, key) without duplicates"));
        }
        notes.push(note);
    }
    Ok(notes)
}

/// Reads and validates a note clip's `"grid"` object.
pub(crate) fn load_grid(v: &Value, path: &JsonPath) -> Result<StepGrid, ModelError> {
    let g = schema::current(v, path, Obj::Grid)?;
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
pub(crate) fn load_controllers(
    v: &Value,
    path: &JsonPath,
) -> Result<Vec<ControllerLane>, ModelError> {
    let mut lanes: Vec<ControllerLane> = Vec::new();
    for (i, item) in array(v, path)?.iter().enumerate() {
        let p = path.index(i);
        let obj = schema::current(item, &p, Obj::ControllerLane)?;
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
pub(crate) fn load_scale(v: &Value, path: &JsonPath) -> Result<Scale, ModelError> {
    let s = schema::current(v, path, Obj::Scale)?;
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
pub(super) fn load_audio(
    v: &Value,
    path: &JsonPath,
    doc: &RackDoc,
    version: u64,
) -> Result<AudioClipContent, ModelError> {
    let a = schema::object(v, path, Obj::Audio, version)?;
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
    let curve = |name: &str| -> Result<FadeCurve, ModelError> {
        let cp = path.field(name);
        let id = string(&a[name], &cp)?;
        FadeCurve::from_id(id).ok_or_else(|| {
            cp.error(format!(
                "expected \"linear\", \"equal_power\", \"exponential\" or \"s_curve\", found {id:?}"
            ))
        })
    };
    let fade_curve = if a.contains_key("fade_curve") {
        let c = curve("fade_curve")?;
        if version < 23 && !matches!(c, FadeCurve::Linear | FadeCurve::EqualPower) {
            return Err(path
                .field("fade_curve")
                .error(format!("{:?} needs version 23 or later", c.id())));
        }
        c
    } else {
        FadeCurve::Linear
    };
    let reverse = match a.get("reverse") {
        None => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(path.field("reverse").error("expected true or false")),
    };
    let envelope = match a.get("envelope") {
        None => Vec::new(),
        Some(e) => {
            let ep = path.field("envelope");
            let items = array(e, &ep)?;
            if items.len() > MAX_GAIN_POINTS {
                return Err(ep.error(format!(
                    "{} points is above the {MAX_GAIN_POINTS} allowed",
                    items.len()
                )));
            }
            let mut points: Vec<GainPoint> = Vec::with_capacity(items.len());
            for (i, item) in items.iter().enumerate() {
                let ip = ep.index(i);
                let t = tuple(item, &ip, 2, "a point [frame, gain_db]")?;
                let frame = uint(&t[0], &ip.index(0), MAX_AUDIO_FRAMES)?;
                let gp = ip.index(1);
                let gain_db = number(&t[1], &gp)? as f32;
                if !(AUDIO_GAIN_DB_MIN..=AUDIO_GAIN_DB_MAX).contains(&gain_db) {
                    return Err(gp.error(format!(
                        "{gain_db} dB is outside {AUDIO_GAIN_DB_MIN}..={AUDIO_GAIN_DB_MAX}"
                    )));
                }
                if points.last().is_some_and(|p| p.frame >= frame) {
                    return Err(ip
                        .index(0)
                        .error("envelope frames must be strictly increasing"));
                }
                points.push(GainPoint { frame, gain_db });
            }
            points
        }
    };
    let xfade = match a.get("xfade") {
        None if version >= 23 => Some(FadeCurve::EqualPower),
        None => None,
        Some(x) => {
            if string(x, &path.field("xfade"))? == "off" {
                None
            } else {
                Some(curve("xfade")?)
            }
        }
    };
    let warp = match a.get("warp") {
        None => WarpSettings::default(),
        Some(w) => load_warp(w, &path.field("warp"))?,
    };
    if reverse && warp.active() {
        return Err(path
            .field("warp")
            .error("a reversed clip cannot be warped or transposed"));
    }
    Ok(AudioClipContent {
        asset: asset.to_owned(),
        source_offset,
        gain_db,
        fade_in: frames("fade_in")?,
        fade_out: frames("fade_out")?,
        fade_curve,
        reverse,
        envelope,
        xfade,
        warp,
    })
}

/// Reads and validates a clip's `"warp"` object.
pub(super) fn load_warp(v: &Value, path: &JsonPath) -> Result<WarpSettings, ModelError> {
    let w = schema::current(v, path, Obj::Warp)?;
    let mode_path = path.field("mode");
    let id = string(&w["mode"], &mode_path)?;
    let mode = WarpMode::from_id(id).ok_or_else(|| {
        mode_path.error(format!(
            "expected \"beats\", \"tonal\" or \"repitch\", found {id:?}"
        ))
    })?;
    let bpm_path = path.field("bpm");
    let source_bpm = number(&w["bpm"], &bpm_path)?;
    let enabled = flag(w, path, "enabled")?;
    let transpose_cents = match w.get("transpose") {
        None => 0,
        Some(t) => {
            let tp = path.field("transpose");
            let n = int(t, &tp)?;
            i32::try_from(n)
                .ok()
                .filter(|c| c.abs() <= WARP_TRANSPOSE_MAX_CENTS)
                .ok_or_else(|| {
                    tp.error(format!("{n} cents is outside ±{WARP_TRANSPOSE_MAX_CENTS}"))
                })?
        }
    };
    let markers = match w.get("markers") {
        None => Vec::new(),
        Some(m) => {
            let mp = path.field("markers");
            let items = array(m, &mp)?;
            let mut out = Vec::with_capacity(items.len().min(MAX_WARP_MARKERS));
            for (i, item) in items.iter().enumerate() {
                let ip = mp.index(i);
                let t = tuple(item, &ip, 2, "a marker [tick, source_frame]")?;
                out.push(WarpMarker {
                    tick: int(&t[0], &ip.index(0))?,
                    source: uint(&t[1], &ip.index(1), MAX_AUDIO_FRAMES)?,
                });
            }
            out
        }
    };
    let settings = WarpSettings {
        enabled,
        mode,
        source_bpm,
        transpose_cents,
        markers,
    };
    settings.validate().map_err(|e| path.error(e))?;
    Ok(settings)
}

/// A clip's optional `"name"` (see [`crate::clip_name_ok`]) and `"color"` (`"#rrggbb"`). Shared by
/// the project and clip file loaders.
pub(crate) fn load_clip_meta(
    obj: &serde_json::Map<String, Value>,
    path: &JsonPath,
) -> Result<(Option<String>, Option<[u8; 3]>), ModelError> {
    let name = match obj.get("name") {
        Some(v) => {
            let p = path.field("name");
            let text = string(v, &p)?;
            if !crate::clip_name_ok(text) {
                return Err(p.error(format!(
                    "the clip name must be trimmed and non-empty, at most {} characters, without control characters",
                    crate::MAX_CLIP_NAME_CHARS
                )));
            }
            Some(text.to_owned())
        }
        None => None,
    };
    let color = match obj.get("color") {
        Some(v) => {
            let p = path.field("color");
            match parse_color(v, &p)? {
                Some(c) => Some(c),
                None => return Err(p.error("expected a colour \"#rrggbb\"")),
            }
        }
        None => None,
    };
    Ok((name, color))
}

/// A comp segment's elements: `[take, start, end]`, plus the crossfade in ticks when it is set.
fn comp_tuple<'v>(v: &'v Value, path: &JsonPath) -> Result<&'v [Value], ModelError> {
    match v.as_array() {
        Some(a) if a.len() == 3 || a.len() == 4 => Ok(a),
        _ => Err(path.error(format!(
            "expected a segment [take, start, end] or [take, start, end, crossfade], found {v}"
        ))),
    }
}
