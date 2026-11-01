//! Reading a track: its lanes, frozen audio and the checks that need the whole song.

use super::*;

/// Where each clip id was first seen, for duplicate messages.
pub(super) type ClipFirst = BTreeMap<u64, JsonPath>;
/// The write modes found while loading tracks, by `(device, param)`.
pub(super) type LaneWrites = BTreeMap<(DeviceKey, String), WriteMode>;

/// What loading the tracks finds besides the tracks, put into the song once they are all read.
#[derive(Default)]
pub(super) struct Loose {
    pub(super) lane_write: LaneWrites,
    /// The folder of a track, with the path of its `"group"`.
    pub(super) members: Vec<(DeviceKey, u32, JsonPath)>,
    pub(super) colors: BTreeMap<DeviceKey, [u8; 3]>,
    pub(super) comments: BTreeMap<DeviceKey, String>,
    pub(super) folded: BTreeSet<(DeviceKey, String)>,
    /// One flag per take group, in file order: its take rows are shown.
    pub(super) open: Vec<bool>,
    /// What the file had that the loaded song does not (see the load warnings).
    pub(super) warnings: Vec<String>,
}

/// A note lane name: trimmed, non-empty, at most [`MAX_LANE_NAME_CHARS`] characters, without
/// control characters.
pub(super) fn lane_name_ok(name: &str) -> bool {
    !name.is_empty()
        && name.trim() == name
        && name.chars().count() <= MAX_LANE_NAME_CHARS
        && !name.chars().any(char::is_control)
}

pub(super) fn load_track(
    v: &Value,
    path: &JsonPath,
    (doc, registry): (&RackDoc, &Registry),
    version: u64,
    song: &Song,
    (clip_first, loose): (&mut ClipFirst, &mut Loose),
) -> Result<Track, ModelError> {
    let obj = schema::object(v, path, Obj::Track, version)?;
    let device_path = path.field("device");
    let device = DeviceKey(string(&obj["device"], &device_path)?.to_owned());
    let dev = doc
        .device(&device)
        .ok_or_else(|| device_path.error(format!("no device has id {:?}", device.0)))?;
    let desc = rules::descriptor(registry, &dev.type_id)?;
    let audio_track = is_audio_track_type(desc);
    // From version 34 a device without a Notes input (a channel strip, an effect) may have a
    // track that holds automation lanes alone.
    let automation_track = !audio_track && !has_notes_input(desc);
    if automation_track && version < 34 {
        return Err(device_path.error(format!(
            "{:?} ({}) has no Notes input, so it cannot have a track before version 34",
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
        // From version 30 the note lanes are a leading block: a track has one to
        // `MAX_NOTE_LANES` of them, then its automation lanes.
        let block_open = version >= 30 && lanes.iter().all(|l| l.kind.is_notes());
        let notes_lane = !audio_track
            && !automation_track
            && (i == 0 || (block_open && kind_text == Some("notes")));
        if version >= 30
            && !audio_track
            && !automation_track
            && kind_text == Some("notes")
            && !block_open
        {
            return Err(kind_path.error("a notes lane cannot follow an automation lane"));
        }
        if notes_lane && i >= MAX_NOTE_LANES {
            return Err(p.error(format!("a track has at most {MAX_NOTE_LANES} note lanes")));
        }
        let want = if audio_track {
            "audio"
        } else if i == 0 && !automation_track {
            "notes"
        } else {
            "automation"
        };
        if let Some(k) = kind_text
            && k != want
            && !(notes_lane && k == "notes")
        {
            return Err(kind_path.error(if audio_track {
                format!("the lane of an audio track must be \"audio\", found {k:?}")
            } else if automation_track {
                format!("the lanes of an automation track must be \"automation\", found {k:?}")
            } else if i == 0 {
                format!("the first lane must be \"notes\", found {k:?}")
            } else {
                format!("lanes after the first must be \"automation\", found {k:?}")
            }));
        }
        let (lane_obj, clip_obj) = if audio_track {
            (Obj::AudioLane, Obj::AudioClip)
        } else if notes_lane {
            (Obj::NotesLane, Obj::NoteClip)
        } else {
            (Obj::AutomationLane, Obj::AutomationClip)
        };
        let lane = schema::object(item, &p, lane_obj, version)?;
        let mut static_value = None;
        let kind = if audio_track {
            LaneKind::Audio
        } else if notes_lane {
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
                loose
                    .lane_write
                    .insert((device.clone(), param.to_owned()), mode);
            }
            if flag(lane, &p, "folded")? {
                loose.folded.insert((device.clone(), param.to_owned()));
            }
            if let Some(v) = lane.get("static") {
                let sp = p.field("static");
                let value = number(v, &sp)? as f32;
                let value = match desc.param_by_key(param) {
                    Some(pd) => pd.clamp(value),
                    None if value.is_finite() => value,
                    None => return Err(sp.error(format!("{value} is out of range"))),
                };
                static_value = Some(value);
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
                (clip_obj, version),
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
                (clip_obj, version),
                &kind,
                (doc, desc),
                song,
                (&clips, clip_first, loose),
            )?,
            None => Vec::new(),
        };
        let mut lane_out = Lane::new(kind)
            .with_clips(clips)
            .with_take_groups(take_groups);
        if let Some(name) = lane.get("name") {
            let np = p.field("name");
            let name = string(name, &np)?;
            if !lane_name_ok(name) {
                return Err(np.error(format!(
                    "the lane name must be trimmed and non-empty, at most {MAX_LANE_NAME_CHARS} characters, without control characters"
                )));
            }
            lane_out.name = Some(name.to_owned());
        }
        if let Some(m) = lane.get("muted") {
            let mp = p.field("muted");
            if !boolean(m, &mp)? {
                return Err(mp.error("expected true (false is the default)"));
            }
            lane_out.muted = true;
        }
        lane_out.static_value = static_value;
        lanes.push(lane_out);
    }
    if lanes.is_empty() {
        return Err(lanes_path.error(if automation_track {
            "a track needs a lane"
        } else {
            "a track needs its note lane"
        }));
    }
    if let Some(g) = obj.get("group") {
        let gp = path.field("group");
        let id = uint(g, &gp, u64::from(u32::MAX))? as u32;
        loose.members.push((device.clone(), id, gp));
    }
    if let Some(c) = marker_color(obj, path)? {
        loose.colors.insert(device.clone(), c);
    }
    if let Some(text) = obj.get("comment") {
        let cp = path.field("comment");
        let text = string(text, &cp)?;
        if !comment_ok(text) {
            return Err(cp.error(format!(
                "the comment must be trimmed and non-empty, at most {MAX_TRACK_COMMENT} characters, without control characters but newlines"
            )));
        }
        loose.comments.insert(device.clone(), text.to_owned());
    }
    let frozen = match obj.get("frozen") {
        None => None,
        Some(f) => Some(load_frozen(f, &path.field("frozen"), doc, version)?),
    };
    // Only the shape is checked: a key that names no device plays the track's own device.
    let notes_to = match obj.get("notes_to") {
        None => None,
        Some(k) => {
            let np = path.field("notes_to");
            let key = string(k, &np)?;
            if key.is_empty() || key == device.0 {
                return Err(np.error(
                    "expected the key of another device (omit it to enter the track's own device)",
                ));
            }
            Some(DeviceKey(key.to_owned()))
        }
    };
    let track = Track {
        device,
        muted: flag(obj, path, "muted")?,
        solo: flag(obj, path, "solo")?,
        armed: flag(obj, path, "armed")?,
        chase: !flag(obj, path, "no_chase")?,
        notes_to,
        direct_record: flag(obj, path, "direct_record")?,
        frozen,
        lanes,
    };
    if automation_track {
        // An automation track plays no notes and no audio: nothing of that can be set on it.
        for (field, set) in [
            ("solo", track.solo),
            ("armed", track.armed),
            ("no_chase", !track.chase),
            ("notes_to", track.notes_to.is_some()),
            ("direct_record", track.direct_record),
            ("frozen", track.frozen.is_some()),
        ] {
            if set {
                return Err(path.field(field).error(
                    "an automation track (a device without a Notes input) cannot be soloed, armed, frozen or take notes; only muted",
                ));
            }
        }
    }
    Ok(track)
}

/// Reads a track's `"frozen"` object. Whether its effects are the first of the channel's insert
/// chain needs the console, which loads after the song: see [`check_frozen_tracks`].
pub(super) fn load_frozen(
    v: &Value,
    path: &JsonPath,
    doc: &RackDoc,
    version: u64,
) -> Result<Frozen, ModelError> {
    let obj = schema::current(v, path, Obj::Frozen)?;
    let start_path = path.field("start");
    let start = int(&obj["start"], &start_path)?;
    if !tick_ok(start) {
        return Err(start_path.error(format!("{start} is outside 0..={MAX_TICK}")));
    }
    let length_path = path.field("length");
    let length = int(&obj["length"], &length_path)?;
    if !length_ok(length) {
        return Err(length_path.error(format!("{length} is outside 1..={MAX_TICK}")));
    }
    let audio_path = path.field("audio");
    let audio = load_audio(&obj["audio"], &audio_path, doc, version)?;
    if audio.reverse || audio.warp.active() {
        return Err(audio_path.error("a frozen render is plain audio: no reverse, no warp"));
    }
    let mut effects: Vec<DeviceKey> = Vec::new();
    if let Some(list) = obj.get("effects") {
        let ep = path.field("effects");
        for (i, item) in array(list, &ep)?.iter().enumerate() {
            let ip = ep.index(i);
            let key = DeviceKey(string(item, &ip)?.to_owned());
            if doc.device(&key).is_none() {
                return Err(ip.error(format!("no device has id {:?}", key.0)));
            }
            if effects.contains(&key) {
                return Err(ip.error(format!("{:?} is listed twice", key.0)));
            }
            effects.push(key);
        }
    }
    Ok(Frozen {
        start,
        length,
        audio,
        effects,
    })
}

/// Checks the frozen tracks of a loaded document against its console: the track plays through a
/// channel it is cabled to, and its frozen effects are the first of that channel's insert chain.
pub(super) fn check_frozen_tracks(
    doc: &RackDoc,
    registry: &Registry,
    path: &JsonPath,
) -> Result<(), ModelError> {
    for (i, t) in doc.song.tracks.iter().enumerate() {
        let Some(f) = &t.frozen else { continue };
        let p = path.index(i).field("frozen");
        let chain = crate::freeze::track_chain(doc, registry, &t.device).map_err(|e| p.error(e))?;
        let n = f.effects.len();
        if n > chain.effects.len() || chain.effects[..n] != f.effects[..] {
            return Err(p.field("effects").error(format!(
                "the frozen effects must be the first effects of the insert chain of {}",
                chain.channel
            )));
        }
    }
    Ok(())
}
