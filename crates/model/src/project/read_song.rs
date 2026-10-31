//! Reading the song: tempo and meter, markers, ranges, heights, grooves, groups and the track list.

use super::*;

/// The song's meter: `[beats_per_bar, beat_unit]` (one meter for the whole song, what files
/// before version 13 write) or a list of `{"bar", "num", "den"}` changes (version 13). The reader
/// takes either shape in any version.
pub(super) fn load_meter(v: &Value, path: &JsonPath) -> Result<Vec<MeterEvent>, ModelError> {
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
        let e = schema::current(item, &p, Obj::MeterEvent)?;
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
pub(super) fn load_markers(v: Option<&Value>, path: &JsonPath) -> Result<Vec<Marker>, ModelError> {
    let Some(v) = v else {
        return Ok(Vec::new());
    };
    let mut markers = Vec::new();
    for (i, item) in array(v, path)?.iter().enumerate() {
        let p = path.index(i);
        let e = schema::current(item, &p, Obj::Marker)?;
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
/// devices without a track are ignored, with a warning (the file may have outlived a track).
pub(super) fn load_track_heights(
    v: Option<&Value>,
    path: &JsonPath,
    song: &Song,
    warnings: &mut Vec<String>,
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
        } else {
            warnings.push(format!(
                "{p}: the height of {:?} was ignored, the song has no track for that device",
                key.0
            ));
        }
    }
    Ok(heights)
}

/// The optional `"grooves"` list, in file order.
pub(super) fn load_grooves(v: Option<&Value>, path: &JsonPath) -> Result<Vec<Groove>, ModelError> {
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
        let e = schema::current(item, &p, Obj::Groove)?;
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

/// The optional `"groups"` list (folders), in file order. Parents must exist and nest at most
/// [`MAX_GROUP_DEPTH`] levels without a loop; whether each folder has tracks in one block is
/// checked once the tracks are loaded ([`link_groups`]).
pub(super) fn load_groups(
    v: Option<&Value>,
    path: &JsonPath,
) -> Result<Vec<TrackGroup>, ModelError> {
    let Some(v) = v else {
        return Ok(Vec::new());
    };
    let items = array(v, path)?;
    if items.len() > MAX_GROUPS {
        return Err(path.error(format!(
            "{} folders; a song holds at most {MAX_GROUPS}",
            items.len()
        )));
    }
    let mut groups: Vec<TrackGroup> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let p = path.index(i);
        let e = schema::current(item, &p, Obj::Group)?;
        let id_path = p.field("id");
        let id = uint(&e["id"], &id_path, u64::from(u32::MAX))? as u32;
        if id == 0 {
            return Err(id_path.error("folder ids start at 1"));
        }
        if groups.iter().any(|g| g.id == id) {
            return Err(id_path.error(format!("duplicate folder id {id}")));
        }
        let name_path = p.field("name");
        let name = string(&e["name"], &name_path)?;
        if !group_name_ok(name) {
            return Err(name_path.error(
                "the name must be trimmed, non-empty and at most 64 characters, without control characters",
            ));
        }
        let parent = e
            .get("parent")
            .map(|v| uint(v, &p.field("parent"), u64::from(u32::MAX)).map(|n| n as u32))
            .transpose()?;
        groups.push(TrackGroup {
            id,
            name: name.to_owned(),
            parent,
            collapsed: flag(e, &p, "collapsed")?,
            color: marker_color(e, &p)?,
        });
    }
    let org = Organisation {
        groups,
        ..Organisation::default()
    };
    for (i, g) in org.groups.iter().enumerate() {
        let Some(parent) = g.parent else { continue };
        let pp = path.index(i).field("parent");
        if org.group(parent).is_none() {
            return Err(pp.error(format!("no folder has id {parent}")));
        }
        let chain = org.chain(Some(g.id));
        if org.group(chain[0]).is_some_and(|top| top.parent.is_some()) {
            return Err(pp.error(format!(
                "folders nest at most {MAX_GROUP_DEPTH} levels deep and cannot contain themselves"
            )));
        }
    }
    Ok(org.groups)
}

/// Puts the folder of each track into `song.org` and checks the folders against the tracks:
/// every `"group"` names a folder, every folder has a track, and the tracks of a folder (with
/// those of its sub-folders) follow each other.
pub(super) fn link_groups(
    song: &mut Song,
    members: Vec<(DeviceKey, u32, JsonPath)>,
    groups_path: &JsonPath,
) -> Result<(), ModelError> {
    for (device, id, p) in &members {
        if song.org.group(*id).is_none() {
            return Err(p.error(format!("no folder has id {id}")));
        }
        song.org.members.insert(device.clone(), *id);
    }
    let normal = song.normalised_org();
    for (device, id, p) in &members {
        if normal.members.get(device) != Some(id) {
            return Err(p.error(format!(
                "the tracks of folder {id} are not one block: {:?} stands apart from them",
                device.0
            )));
        }
    }
    for (i, g) in song.org.groups.iter().enumerate() {
        if normal.group(g.id).is_none() {
            return Err(groups_path
                .index(i)
                .error(format!("folder {} has no tracks", g.id)));
        }
    }
    Ok(())
}

/// The optional `"ranges"` list, loaded like the markers (sorted by start).
pub(super) fn load_ranges(
    v: Option<&Value>,
    path: &JsonPath,
) -> Result<Vec<RangeMarker>, ModelError> {
    let Some(v) = v else {
        return Ok(Vec::new());
    };
    let mut ranges = Vec::new();
    for (i, item) in array(v, path)?.iter().enumerate() {
        let p = path.index(i);
        let e = schema::current(item, &p, Obj::Range)?;
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

pub(super) fn marker_name(v: &Value, path: &JsonPath) -> Result<String, ModelError> {
    let name = string(v, path)?;
    if !name_ok(name) {
        return Err(path.error(format!(
            "the name must be trimmed, non-empty and at most {MAX_MARKER_NAME} characters"
        )));
    }
    Ok(name.to_owned())
}

pub(super) fn marker_color(
    e: &serde_json::Map<String, Value>,
    path: &JsonPath,
) -> Result<Option<[u8; 3]>, ModelError> {
    e.get("color")
        .map_or(Ok(None), |v| parse_color(v, &path.field("color")))
}

pub(super) fn load_song(
    v: &Value,
    path: &JsonPath,
    (doc, registry): (&RackDoc, &Registry),
    version: u64,
    warnings: &mut Vec<String>,
) -> Result<Song, ModelError> {
    let obj = schema::object(v, path, Obj::Song, version)?;

    let tempo_path = path.field("tempo");
    let mut tempo = Vec::new();
    for (i, item) in array(&obj["tempo"], &tempo_path)?.iter().enumerate() {
        let p = tempo_path.index(i);
        let e = schema::object(item, &p, Obj::TempoEvent, version)?;
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
    let lp = schema::current(&obj["loop"], &loop_path, Obj::Loop)?;
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
        org: Organisation {
            groups: load_groups(obj.get("groups"), &path.field("groups"))?,
            ..Organisation::default()
        },
        tails: match obj.get("tails") {
            None => false,
            Some(Value::Bool(true)) => true,
            Some(_) => {
                return Err(path.field("tails").error(
                    "\"tails\" is true when present (a project without note tails omits it)",
                ));
            }
        },
    };
    let tracks_path = path.field("tracks");
    let mut track_first: BTreeMap<DeviceKey, JsonPath> = BTreeMap::new();
    let mut clip_first: BTreeMap<u64, JsonPath> = BTreeMap::new();
    let mut loose = Loose::default();
    for (i, item) in array(&obj["tracks"], &tracks_path)?.iter().enumerate() {
        let p = tracks_path.index(i);
        let track = load_track(
            item,
            &p,
            (doc, registry),
            version,
            &song,
            (&mut clip_first, &mut loose),
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
    song.lane_write = loose.lane_write;
    let groups = song
        .tracks
        .iter_mut()
        .flat_map(|t| &mut t.lanes)
        .flat_map(|l| &mut l.take_groups);
    for (id, group) in (1..).zip(groups) {
        group.id = id;
    }
    song.org.colors = loose.colors;
    song.org.comments = loose.comments;
    song.org.folded_lanes = loose.folded;
    let open: Vec<u32> = song
        .take_groups()
        .zip(&loose.open)
        .filter(|(_, open)| **open)
        .map(|((_, _, g), _)| g.id)
        .collect();
    song.org.open_takes = open.into_iter().collect();
    link_groups(&mut song, loose.members, &path.field("groups"))?;
    song.track_heights = load_track_heights(
        obj.get("track_heights"),
        &path.field("track_heights"),
        &song,
        warnings,
    )?;
    warnings.append(&mut loose.warnings);
    Ok(song)
}
