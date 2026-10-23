//! Validation of song commands and their translation into reversible edits.

use crate::console_cmd::{Planner, add_channel, console_available, is_instrument};
use crate::history::Edit;
use crate::song::{
    AUDIO_GAIN_DB_MAX, AUDIO_GAIN_DB_MIN, AUDIO_TRACK_TYPE, AudioClipContent, BEAT_UNITS, Clip,
    ClipContent, ClipId, Lane, LaneKind, MAX_AUDIO_FRAMES, MAX_BEATS_PER_BAR, MAX_LOOP_REPEATS,
    MAX_MARKER_NAME, MAX_TICK, Marker, MeterEvent, RangeMarker, SongCommand, TempoEvent, Track,
};
use crate::{
    Cable, DeviceKey, INTERFACE_KEY, ModelError, RackDevice, RackDoc, SocketRef, TEMPO_MAX,
    TEMPO_MIN, rules,
};
use atomik_core::compile_meter;
use atomik_device_sdk::{DeviceDescriptor, Direction, Registry, SocketKind};
use std::collections::BTreeMap;

fn bad(message: impl Into<String>) -> ModelError {
    ModelError::BadSong(message.into())
}

fn range(message: impl Into<String>) -> ModelError {
    ModelError::OutOfRange(message.into())
}

/// True if the device type has a Notes input (a track can play it).
pub(crate) fn has_notes_input(desc: &DeviceDescriptor) -> bool {
    desc.sockets
        .iter()
        .any(|s| s.kind == SocketKind::Notes && s.direction == Direction::Input)
}

/// True if the device type is the audio track player (a track on it has an audio lane).
pub(crate) fn is_audio_track_type(desc: &DeviceDescriptor) -> bool {
    desc.type_id == AUDIO_TRACK_TYPE
}

/// The descriptor of device `key`, which must exist.
fn device_descriptor(
    doc: &RackDoc,
    registry: &Registry,
    key: &DeviceKey,
) -> Result<&'static DeviceDescriptor, ModelError> {
    rules::descriptor(registry, &rules::device(doc, key)?.type_id)
}

/// A new, empty track for `device`: just the note lane.
pub(crate) fn new_track(device: DeviceKey) -> Track {
    Track {
        device,
        muted: false,
        solo: false,
        armed: false,
        chase: true,
        lanes: vec![Lane {
            kind: LaneKind::Notes,
            clips: Vec::new(),
        }],
    }
}

/// A new, empty audio track for `device`: just the audio lane.
pub(crate) fn new_audio_track(device: DeviceKey) -> Track {
    Track {
        device,
        muted: false,
        solo: false,
        armed: false,
        chase: true,
        lanes: vec![Lane {
            kind: LaneKind::Audio,
            clips: Vec::new(),
        }],
    }
}

fn track_index(doc: &RackDoc, device: &DeviceKey) -> Result<usize, ModelError> {
    doc.song
        .tracks
        .iter()
        .position(|t| &t.device == device)
        .ok_or_else(|| bad(format!("{device} has no track")))
}

/// `0 <= x <= MAX_TICK`.
pub(crate) fn tick_ok(x: i64) -> bool {
    (0..=MAX_TICK).contains(&x)
}

/// `0 < x <= MAX_TICK`.
pub(crate) fn length_ok(x: i64) -> bool {
    (1..=MAX_TICK).contains(&x)
}

/// Checks clip geometry: ticks in range, positive lengths, bounded loop repetitions.
pub(crate) fn check_geometry(
    start: i64,
    length: i64,
    offset: i64,
    loop_length: Option<i64>,
) -> Result<(), String> {
    if !tick_ok(start) {
        return Err(format!("clip start {start} is outside 0..={MAX_TICK}"));
    }
    if !length_ok(length) {
        return Err(format!(
            "clip length {length} must be greater than 0 and at most {MAX_TICK}"
        ));
    }
    if !tick_ok(offset) {
        return Err(format!("clip offset {offset} is outside 0..={MAX_TICK}"));
    }
    if let Some(n) = loop_length {
        if !length_ok(n) {
            return Err(format!(
                "loop length {n} must be greater than 0 and at most {MAX_TICK}"
            ));
        }
        if length > n * MAX_LOOP_REPEATS {
            return Err(format!(
                "clip length {length} holds more than {MAX_LOOP_REPEATS} repetitions of its loop ({n})"
            ));
        }
    }
    Ok(())
}

/// Checks one note; `Err` names what is wrong.
pub(crate) fn check_note(tick: i64, length: i64, key: u8, velocity: f32) -> Result<(), String> {
    if !tick_ok(tick) {
        return Err(format!("note tick {tick} is outside 0..={MAX_TICK}"));
    }
    if !length_ok(length) {
        return Err(format!(
            "note length {length} must be greater than 0 and at most {MAX_TICK}"
        ));
    }
    if key > 127 {
        return Err(format!("note key {key} is outside 0..=127"));
    }
    if !(velocity > 0.0 && velocity <= 1.0) {
        return Err(format!("note velocity {velocity} is outside (0, 1]"));
    }
    Ok(())
}

/// Validates content against the lane kind; automation values are clamped by the param's
/// descriptor. Returns the normalized content.
pub(crate) fn check_content(
    doc: &RackDoc,
    content: ClipContent,
    kind: &LaneKind,
    desc: &DeviceDescriptor,
) -> Result<ClipContent, ModelError> {
    match (content, kind) {
        (ClipContent::Audio(a), LaneKind::Audio) => {
            check_audio(doc, &a)?;
            Ok(ClipContent::Audio(a))
        }
        (ClipContent::Notes(notes), LaneKind::Notes) => {
            for (i, n) in notes.iter().enumerate() {
                check_note(n.tick, n.length, n.key, n.velocity).map_err(range)?;
                if i > 0 && (notes[i - 1].tick, notes[i - 1].key) >= (n.tick, n.key) {
                    return Err(bad(format!(
                        "notes must be sorted by (tick, key) without duplicates (note {i})"
                    )));
                }
            }
            Ok(ClipContent::Notes(notes))
        }
        (ClipContent::Automation(mut points), LaneKind::Automation(param)) => {
            // A dead plugin param has no descriptor: its values are kept as they are.
            let p = desc.param_by_key(param);
            if p.is_none() && !crate::plugin::is_dead_plugin_param(desc, param) {
                return Err(ModelError::UnknownKey(param.clone()));
            }
            for i in 0..points.len() {
                let pt = &mut points[i];
                if !tick_ok(pt.tick) {
                    return Err(range(format!(
                        "point tick {} is outside 0..={MAX_TICK}",
                        pt.tick
                    )));
                }
                if pt.value.is_nan() {
                    return Err(range(format!("point {i} value is NaN")));
                }
                if let Some(p) = p {
                    pt.value = p.clamp(pt.value);
                }
                if i > 0 && points[i - 1].tick >= points[i].tick {
                    return Err(bad(format!(
                        "points must be sorted by tick without duplicates (point {i})"
                    )));
                }
            }
            Ok(ClipContent::Automation(points))
        }
        (ClipContent::Audio(_), LaneKind::Notes) => Err(bad("audio content on the note lane")),
        (ClipContent::Audio(_), LaneKind::Automation(p)) => {
            Err(bad(format!("audio content on the {p} lane")))
        }
        (_, LaneKind::Audio) => Err(bad("note or automation content on the audio lane")),
        (_, LaneKind::Notes) => Err(bad("automation content on the note lane")),
        (_, LaneKind::Automation(p)) => Err(bad(format!("note content on the {p} lane"))),
    }
}

/// Checks audio clip content against the pool: the asset exists, the source offset is inside
/// it, the gain is finite and in range, the fades are bounded.
pub(crate) fn check_audio(doc: &RackDoc, a: &AudioClipContent) -> Result<(), ModelError> {
    let meta = doc
        .assets
        .get(&a.asset)
        .ok_or_else(|| ModelError::UnknownKey(format!("asset {}", a.asset)))?;
    if a.source_offset > meta.frames {
        return Err(range(format!(
            "source offset {} is beyond the asset's {} frames",
            a.source_offset, meta.frames
        )));
    }
    if !(AUDIO_GAIN_DB_MIN..=AUDIO_GAIN_DB_MAX).contains(&a.gain_db) {
        return Err(range(format!(
            "gain {} dB is outside {AUDIO_GAIN_DB_MIN}..={AUDIO_GAIN_DB_MAX}",
            a.gain_db
        )));
    }
    for (name, frames) in [("fade in", a.fade_in), ("fade out", a.fade_out)] {
        if frames > MAX_AUDIO_FRAMES {
            return Err(range(format!(
                "{name} of {frames} frames is above {MAX_AUDIO_FRAMES}"
            )));
        }
    }
    Ok(())
}

/// Audio clips keep their position in ticks only: no content offset, no loop.
fn check_audio_shape(offset: i64, loop_length: Option<i64>) -> Result<(), ModelError> {
    if offset != 0 || loop_length.is_some() {
        return Err(range(
            "an audio clip has no content offset and no loop (its source offset is in its content)",
        ));
    }
    Ok(())
}

/// The audio lane of `device`'s track.
fn audio_lane(doc: &RackDoc, device: &DeviceKey) -> Result<usize, ModelError> {
    doc.song.tracks[track_index(doc, device)?]
        .audio_lane()
        .ok_or_else(|| bad(format!("{device} is not an audio track")))
}

/// Plans `AddAudioTrack` (see the command docs).
fn add_audio_track(
    doc: &RackDoc,
    registry: &Registry,
    name: Option<String>,
) -> Result<Edit, ModelError> {
    let desc = rules::descriptor(registry, AUDIO_TRACK_TYPE)?;
    if name.as_ref().is_some_and(|n| n.trim().is_empty()) {
        return Err(bad("an audio track name cannot be blank"));
    }
    let key = rules::generate_key(doc, AUDIO_TRACK_TYPE);
    let insert = Edit::Insert {
        index: doc.devices.len(),
        device: RackDevice {
            key: key.clone(),
            type_id: AUDIO_TRACK_TYPE.to_owned(),
            name: name.unwrap_or_else(|| rules::unique_name(doc, desc.name)),
            params: BTreeMap::new(),
            folded: false,
            color: None,
            zones: Vec::new(),
            steps: None,
            inner: None,
            plugin: None,
            column: 0,
        },
        cables: Vec::new(),
    };
    let mut p = Planner::new(doc, registry);
    p.push(insert.clone());
    if is_instrument(registry, AUDIO_TRACK_TYPE) && console_available(registry) {
        // As `AddDevice` does: a channel that cannot be wired leaves the plain device.
        let mut with_channel = Planner::new(doc, registry);
        with_channel.push(insert);
        if add_channel(&mut with_channel, &key).is_ok() {
            p = with_channel;
        }
    }
    let index = p.doc().song.tracks.len();
    p.push(Edit::Track {
        index,
        old: None,
        new: Some(new_audio_track(key.clone())),
    });
    let inputs = p.doc().input_channels;
    for (input, socket) in [(1u16, "in_l"), (2, "in_r")] {
        if inputs == 0 {
            break;
        }
        p.connect(Cable {
            from: SocketRef {
                device: DeviceKey(INTERFACE_KEY.to_owned()),
                socket: format!("in_{}", input.min(inputs)),
            },
            to: SocketRef {
                device: key.clone(),
                socket: socket.to_owned(),
            },
        })?;
    }
    Ok(p.finish())
}

/// Checks a tempo map: non-empty, first at tick 0, strictly increasing ticks, tempos in range.
pub(crate) fn check_tempo_map(map: &[TempoEvent]) -> Result<(), String> {
    let first = map
        .first()
        .ok_or("the tempo map needs at least one event")?;
    if first.tick != 0 {
        return Err(format!("the first tempo event is at {}, not 0", first.tick));
    }
    for (i, e) in map.iter().enumerate() {
        if !tick_ok(e.tick) || (i > 0 && map[i - 1].tick >= e.tick) {
            return Err(format!(
                "tempo event {i}: ticks must increase strictly within 0..={MAX_TICK}"
            ));
        }
        if !(TEMPO_MIN..=TEMPO_MAX).contains(&e.bpm) {
            return Err(format!(
                "tempo event {i}: {} BPM is outside {TEMPO_MIN}..={TEMPO_MAX}",
                e.bpm
            ));
        }
        if e.ramp && i + 1 == map.len() {
            return Err(format!(
                "tempo event {i}: the last event cannot ramp (there is no next event)"
            ));
        }
    }
    Ok(())
}

/// Checks a meter map: first at bar 1, bars strictly increasing, signatures supported.
pub(crate) fn check_meter_map(map: &[MeterEvent]) -> Result<(), String> {
    let segments = compile_meter(map).map_err(|e| e.to_string())?;
    if segments.last().is_some_and(|s| s.tick > MAX_TICK) {
        return Err(format!(
            "meter changes must lie within 0..={MAX_TICK} ticks"
        ));
    }
    Ok(())
}

/// True for a marker name the model accepts: trimmed, non-empty, at most
/// [`MAX_MARKER_NAME`] characters.
pub(crate) fn name_ok(name: &str) -> bool {
    name == name.trim() && !name.is_empty() && name.chars().count() <= MAX_MARKER_NAME
}

fn check_name(what: &str, i: usize, name: &str) -> Result<(), ModelError> {
    if name_ok(name) {
        Ok(())
    } else {
        Err(bad(format!(
            "{what} {i}: the name must be trimmed, non-empty and at most {MAX_MARKER_NAME} characters"
        )))
    }
}

fn check_ids(what: &str, ids: impl Iterator<Item = u32>) -> Result<(), ModelError> {
    let mut seen = std::collections::BTreeSet::new();
    for (i, id) in ids.enumerate() {
        if !seen.insert(id) {
            return Err(bad(format!("{what} {i}: id {id} is used twice")));
        }
    }
    Ok(())
}

/// Validates a marker list and returns it sorted by `(tick, id)`.
pub(crate) fn check_markers(mut markers: Vec<Marker>) -> Result<Vec<Marker>, ModelError> {
    for (i, m) in markers.iter().enumerate() {
        if !tick_ok(m.tick) {
            return Err(range(format!(
                "marker {i}: tick {} is outside 0..={MAX_TICK}",
                m.tick
            )));
        }
        check_name("marker", i, &m.name)?;
    }
    check_ids("marker", markers.iter().map(|m| m.id))?;
    markers.sort_by_key(|m| (m.tick, m.id));
    Ok(markers)
}

/// Validates track heights (each within the limits) and drops the entries of devices that
/// have no track in `song`.
pub(crate) fn check_track_heights(
    song: &crate::song::Song,
    heights: std::collections::BTreeMap<DeviceKey, u16>,
) -> Result<std::collections::BTreeMap<DeviceKey, u16>, ModelError> {
    use crate::song::{MAX_TRACK_HEIGHT, MIN_TRACK_HEIGHT};
    for (key, h) in &heights {
        if !(MIN_TRACK_HEIGHT..=MAX_TRACK_HEIGHT).contains(h) {
            return Err(range(format!(
                "track {:?}: height {h} is outside {MIN_TRACK_HEIGHT}..={MAX_TRACK_HEIGHT}",
                key.0
            )));
        }
    }
    Ok(heights
        .into_iter()
        .filter(|(key, _)| song.tracks.iter().any(|t| &t.device == key))
        .collect())
}

/// Validates a range list and returns it sorted by `(start, id)`.
pub(crate) fn check_ranges(mut ranges: Vec<RangeMarker>) -> Result<Vec<RangeMarker>, ModelError> {
    for (i, r) in ranges.iter().enumerate() {
        if !tick_ok(r.start) {
            return Err(range(format!(
                "range {i}: start {} is outside 0..={MAX_TICK}",
                r.start
            )));
        }
        if !tick_ok(r.end) || r.end <= r.start {
            return Err(range(format!(
                "range {i}: end {} must be above the start ({}) and within {MAX_TICK}",
                r.end, r.start
            )));
        }
        check_name("range", i, &r.name)?;
    }
    check_ids("range", ranges.iter().map(|r| r.id))?;
    ranges.sort_by_key(|r| (r.start, r.id));
    Ok(ranges)
}

/// Checks a time signature.
pub(crate) fn check_meter(beats_per_bar: u8, beat_unit: u8) -> Result<(), String> {
    if !(1..=MAX_BEATS_PER_BAR).contains(&beats_per_bar) {
        return Err(format!(
            "{beats_per_bar} beats per bar is outside 1..={MAX_BEATS_PER_BAR}"
        ));
    }
    if !BEAT_UNITS.contains(&beat_unit) {
        return Err(format!("beat unit {beat_unit} must be 2, 4, 8 or 16"));
    }
    Ok(())
}

/// Checks a loop range `[start, end)`.
pub(crate) fn check_loop(start: i64, end: i64) -> Result<(), String> {
    if !(tick_ok(start) && tick_ok(end) && start < end) {
        return Err(format!(
            "loop [{start}, {end}) needs 0 <= start < end <= {MAX_TICK}"
        ));
    }
    Ok(())
}

/// Checks the song end.
pub(crate) fn check_end(end: i64) -> Result<(), String> {
    if !length_ok(end) {
        return Err(format!(
            "song end {end} must be greater than 0 and at most {MAX_TICK}"
        ));
    }
    Ok(())
}

/// The clip `id` with its track's device and lane index.
fn find_clip(doc: &RackDoc, id: ClipId) -> Result<(DeviceKey, usize, &Clip), ModelError> {
    doc.song
        .clip(id)
        .map(|(t, lane, c)| (t.device.clone(), lane, c))
        .ok_or_else(|| ModelError::UnknownKey(format!("clip {}", id.0)))
}

/// An insertion of `clip` under a freshly allocated id, plus the id bump.
fn insert_new(
    doc: &RackDoc,
    device: DeviceKey,
    lane: usize,
    mut clip: Clip,
) -> Result<(Edit, ClipId), ModelError> {
    let id = doc.song.next_clip_id;
    let next = id
        .checked_add(1)
        .ok_or_else(|| bad("clip ids are exhausted"))?;
    clip.id = ClipId(id);
    Ok((
        Edit::Compound(vec![
            Edit::Clip {
                device,
                lane,
                old: None,
                new: Some(clip),
            },
            Edit::NextClipId { old: id, new: next },
        ]),
        ClipId(id),
    ))
}

/// Validates a song command against `doc` and turns it into an edit, with the clip id it
/// allocates (if any).
pub(crate) fn plan(
    doc: &RackDoc,
    registry: &Registry,
    command: SongCommand,
) -> Result<(Edit, Option<ClipId>), ModelError> {
    let song = &doc.song;
    let edit = match command {
        SongCommand::AddTrack { device } => {
            let desc = device_descriptor(doc, registry, &device)?;
            let audio = is_audio_track_type(desc);
            if !audio && !has_notes_input(desc) {
                return Err(bad(format!("{device} has no Notes input")));
            }
            if song.track(&device).is_some() {
                return Err(bad(format!("{device} already has a track")));
            }
            Edit::Track {
                index: song.tracks.len(),
                old: None,
                new: Some(if audio {
                    new_audio_track(device)
                } else {
                    new_track(device)
                }),
            }
        }
        SongCommand::RemoveTrack { device } => {
            let index = track_index(doc, &device)?;
            Edit::Track {
                index,
                old: Some(song.tracks[index].clone()),
                new: None,
            }
        }
        SongCommand::SetTrackFlags {
            device,
            muted,
            solo,
            armed,
        } => {
            let t = &song.tracks[track_index(doc, &device)?];
            Edit::TrackFlags {
                device,
                old: [t.muted, t.solo, t.armed],
                new: [muted, solo, armed],
            }
        }
        SongCommand::SetTrackChase { device, chase } => {
            let t = &song.tracks[track_index(doc, &device)?];
            Edit::TrackChase {
                device,
                old: t.chase,
                new: chase,
            }
        }
        SongCommand::AddAutomationLane { device, param } => {
            let desc = device_descriptor(doc, registry, &device)?;
            if desc.param_by_key(&param).is_none() || crate::console::is_internal_param(&param) {
                return Err(ModelError::UnknownKey(param));
            }
            if !has_notes_input(desc) {
                return Err(bad(format!(
                    "{device} has no Notes input, so it has no track to automate"
                )));
            }
            let lane = Lane {
                kind: LaneKind::Automation(param.clone()),
                clips: Vec::new(),
            };
            match song.track(&device) {
                Some(t) if t.automation_lane(&param).is_some() => {
                    return Err(bad(format!("{device} already has a {param} lane")));
                }
                Some(t) => Edit::Lane {
                    index: t.lanes.len(),
                    device,
                    old: None,
                    new: Some(lane),
                },
                None => {
                    let mut track = new_track(device);
                    track.lanes.push(lane);
                    Edit::Track {
                        index: song.tracks.len(),
                        old: None,
                        new: Some(track),
                    }
                }
            }
        }
        SongCommand::AddClip { device, lane, clip } => {
            let desc = device_descriptor(doc, registry, &device)?;
            let t = &song.tracks[track_index(doc, &device)?];
            let kind = &t
                .lanes
                .get(lane)
                .ok_or_else(|| bad(format!("{device} has no lane {lane}")))?
                .kind;
            check_geometry(clip.start, clip.length, clip.offset, clip.loop_length)
                .map_err(range)?;
            if let Some(g) = &clip.grid {
                if !kind.is_notes() {
                    return Err(bad("only a note clip can have a step grid"));
                }
                g.validate().map_err(range)?;
            }
            if clip.scale.is_some() && !kind.is_notes() {
                return Err(bad("only a note clip can have a scale"));
            }
            if kind.is_audio() {
                check_audio_shape(clip.offset, clip.loop_length)?;
            }
            let content = check_content(doc, clip.content, kind, desc)?;
            let scale = clip.scale.filter(|s| !s.is_chromatic());
            let (edit, id) = insert_new(
                doc,
                device,
                lane,
                Clip {
                    content,
                    scale,
                    ..clip
                },
            )?;
            return Ok((edit, Some(id)));
        }
        SongCommand::RemoveClip { id } => {
            let (device, lane, c) = find_clip(doc, id)?;
            Edit::Clip {
                device,
                lane,
                old: Some(c.clone()),
                new: None,
            }
        }
        SongCommand::SetClipGeometry {
            id,
            start,
            length,
            offset,
            loop_length,
            muted,
            ..
        } => {
            let (device, lane, c) = find_clip(doc, id)?;
            check_geometry(start, length, offset, loop_length).map_err(range)?;
            if c.content.is_audio() {
                check_audio_shape(offset, loop_length)?;
            }
            Edit::Clip {
                device,
                lane,
                old: Some(c.clone()),
                new: Some(Clip {
                    start,
                    length,
                    offset,
                    loop_length,
                    muted,
                    ..c.clone()
                }),
            }
        }
        SongCommand::SetClipContent { id, content, .. } => {
            let (device, lane, c) = find_clip(doc, id)?;
            let desc = device_descriptor(doc, registry, &device)?;
            let kind = &song.tracks[track_index(doc, &device)?].lanes[lane].kind;
            let content = check_content(doc, content, kind, desc)?;
            Edit::Clip {
                device,
                lane,
                old: Some(c.clone()),
                new: Some(Clip {
                    content,
                    ..c.clone()
                }),
            }
        }
        SongCommand::SetClipGrid { clip, grid, .. } => {
            let (device, lane, c) = find_clip(doc, clip)?;
            if !c.content.is_notes() {
                return Err(bad(format!(
                    "clip {} is not a note clip, so it has no step grid",
                    clip.0
                )));
            }
            if let Some(g) = &grid {
                g.validate().map_err(range)?;
            }
            Edit::Clip {
                device,
                lane,
                old: Some(c.clone()),
                new: Some(Clip { grid, ..c.clone() }),
            }
        }
        SongCommand::SetClipScale { clip, scale } => {
            let (device, lane, c) = find_clip(doc, clip)?;
            if !c.content.is_notes() {
                return Err(bad(format!(
                    "clip {} is not a note clip, so it has no scale",
                    clip.0
                )));
            }
            Edit::Clip {
                device,
                lane,
                old: Some(c.clone()),
                new: Some(Clip {
                    scale: scale.filter(|s| !s.is_chromatic()),
                    ..c.clone()
                }),
            }
        }
        SongCommand::SplitClip { id, at } => {
            let (device, lane, c) = find_clip(doc, id)?;
            if !(c.start < at && at < c.end()) {
                return Err(range(format!(
                    "split point {at} is not strictly inside clip {} [{}, {})",
                    id.0,
                    c.start,
                    c.end()
                )));
            }
            let shift = c.offset + (at - c.start);
            let mut right = Clip {
                start: at,
                length: c.end() - at,
                offset: c.loop_length.map_or(shift, |n| shift.rem_euclid(n)),
                ..c.clone()
            };
            let mut left = Clip {
                length: at - c.start,
                ..c.clone()
            };
            if let ClipContent::Audio(a) = &c.content {
                // The right half starts the frames between the two ticks later in the asset
                // (tempo map and document sample rate), at most at the asset end. No fade is
                // invented at the cut: the left half keeps the fade in, the right the fade out.
                let delta =
                    song.frame_at(at, doc.sample_rate) - song.frame_at(c.start, doc.sample_rate);
                let asset_frames = doc.assets.get(&a.asset).map_or(u64::MAX, |m| m.frames);
                let mut r = a.clone();
                r.source_offset = a.source_offset.saturating_add(delta).min(asset_frames);
                r.fade_in = 0;
                right.content = ClipContent::Audio(r);
                right.offset = 0;
                if let ClipContent::Audio(l) = &mut left.content {
                    l.fade_out = 0;
                }
            }
            check_geometry(right.start, right.length, right.offset, right.loop_length)
                .map_err(range)?;
            let (insert, new_id) = insert_new(doc, device.clone(), lane, right)?;
            let edit = Edit::Compound(vec![
                Edit::Clip {
                    device,
                    lane,
                    old: Some(c.clone()),
                    new: Some(left),
                },
                insert,
            ]);
            return Ok((edit, Some(new_id)));
        }
        SongCommand::DuplicateClip { id, start } => {
            let (device, lane, c) = find_clip(doc, id)?;
            check_geometry(start, c.length, c.offset, c.loop_length).map_err(range)?;
            let (edit, new_id) = insert_new(doc, device, lane, Clip { start, ..c.clone() })?;
            return Ok((edit, Some(new_id)));
        }
        SongCommand::AddAudioTrack { name } => add_audio_track(doc, registry, name)?,
        SongCommand::AddAudioClip {
            device,
            start,
            length,
            asset,
            source_offset,
            gain_db,
            fade_in,
            fade_out,
            fade_curve,
        } => {
            device_descriptor(doc, registry, &device)?;
            let lane = audio_lane(doc, &device)?;
            check_geometry(start, length, 0, None).map_err(range)?;
            let content = AudioClipContent {
                asset,
                source_offset,
                gain_db,
                fade_in,
                fade_out,
                fade_curve,
            };
            check_audio(doc, &content)?;
            let clip = Clip {
                id: ClipId(0),
                start,
                length,
                offset: 0,
                loop_length: None,
                muted: false,
                grid: None,
                scale: None,
                content: ClipContent::Audio(content),
            };
            let (edit, id) = insert_new(doc, device, lane, clip)?;
            return Ok((edit, Some(id)));
        }
        SongCommand::SetAudioClip {
            id,
            start,
            length,
            muted,
            content,
            ..
        } => {
            let (device, lane, c) = find_clip(doc, id)?;
            if !c.content.is_audio() {
                return Err(bad(format!("clip {} is not an audio clip", id.0)));
            }
            check_geometry(start, length, 0, None).map_err(range)?;
            check_audio(doc, &content)?;
            Edit::Clip {
                device,
                lane,
                old: Some(c.clone()),
                new: Some(Clip {
                    start,
                    length,
                    muted,
                    content: ClipContent::Audio(content),
                    ..c.clone()
                }),
            }
        }
        SongCommand::SetTempoMap(map) | SongCommand::DragTempoMap { map, .. } => {
            check_tempo_map(&map).map_err(range)?;
            Edit::TempoMap {
                old: song.tempo.clone(),
                new: map,
            }
        }
        SongCommand::SetMeter {
            beats_per_bar,
            beat_unit,
        } => {
            check_meter(beats_per_bar, beat_unit).map_err(range)?;
            let mut new = song.meter.clone();
            if let Some(first) = new.first_mut() {
                (first.num, first.den) = (beats_per_bar, beat_unit);
            }
            Edit::MeterMap {
                old: song.meter.clone(),
                new,
            }
        }
        SongCommand::SetMeterMap(map) | SongCommand::DragMeterMap { map, .. } => {
            check_meter_map(&map).map_err(range)?;
            Edit::MeterMap {
                old: song.meter.clone(),
                new: map,
            }
        }
        SongCommand::SetMarkers(markers) | SongCommand::DragMarkers { markers, .. } => {
            let markers = check_markers(markers)?;
            Edit::Markers {
                old: song.markers.clone(),
                new: markers,
            }
        }
        SongCommand::SetRanges(ranges) | SongCommand::DragRanges { ranges, .. } => {
            let ranges = check_ranges(ranges)?;
            Edit::Ranges {
                old: song.ranges.clone(),
                new: ranges,
            }
        }
        SongCommand::SetTrackHeights(heights) | SongCommand::DragTrackHeights { heights, .. } => {
            let heights = check_track_heights(song, heights)?;
            Edit::TrackHeights {
                old: song.track_heights.clone(),
                new: heights,
            }
        }
        SongCommand::SetLoop { range: r, enabled } => {
            check_loop(r.0, r.1).map_err(range)?;
            Edit::Loop {
                old: (song.loop_range, song.loop_enabled),
                new: (r, enabled),
            }
        }
        SongCommand::SetSongEnd(end) => {
            check_end(end).map_err(range)?;
            Edit::SongEnd {
                old: song.end,
                new: end,
            }
        }
    };
    Ok((edit, None))
}
