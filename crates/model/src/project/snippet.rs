//! Text forms of what Copy puts on the system clipboard.
//!
//! The app copies clips, notes, automation points and devices between two running instances as
//! text. Nothing new is invented for the contents: a clip is the project's own clip object
//! (`write_clip` / `load_clip`), notes are an `atomik-clip` file, automation points are the
//! project's point tuples, and devices (with the cables between them, their song tracks and the
//! pool entries they need) are an `atomik-project` text like a library device item. The app adds
//! its envelope around them.
//!
//! Every reader checks what it reads like the project loader does and returns
//! [`ModelError::Load`] with `"<json path>: <what is wrong>"`; none panics on hostile text.

use super::*;
use crate::clip_io::{ClipFile, load_clip_file, save_clip_file};
use crate::patch::{JsonPath, array, json_str, object, string, uint};
use crate::song::{AutomationPoint, ClipContent, ClipId, LaneKind, Note, Song};
use crate::workflow::{ClipEntry, ClipSnapshot, DeviceSnapshot, NoteSnapshot, PointSnapshot};
use crate::{empty_doc, empty_song};

/// The most items one clipboard text may hold, a guard against hostile text (a real selection is
/// far below it).
const MAX_ITEMS: usize = 100_000;

fn parse(text: &str) -> Result<Value, ModelError> {
    serde_json::from_str(text).map_err(|e| JsonPath::default().error(format!("invalid JSON: {e}")))
}

fn assets_json(assets: &BTreeMap<String, AssetMeta>) -> String {
    let rows: Vec<String> = assets
        .iter()
        .map(|(hash, m)| {
            format!(
                "{}: {{\"channels\": {}, \"sample_rate\": {}, \"frames\": {}, \"name\": {}}}",
                json_str(hash),
                m.channels,
                m.sample_rate,
                m.frames,
                json_str(&m.original_name)
            )
        })
        .collect();
    format!("{{{}}}", rows.join(", "))
}

// ---------------------------------------------------------------------------------- clips

/// The text of a clip selection: its entries (track key, lane, note lane and the project's clip
/// object), origin, span and pool entries.
pub fn clips_to_text(s: &ClipSnapshot) -> String {
    let mut w = String::new();
    let _ = write!(
        w,
        "{{\"origin\": {}, \"span\": {}, \"assets\": {}, \"entries\": [",
        s.origin,
        s.span,
        assets_json(&s.assets)
    );
    for (i, e) in s.entries.iter().enumerate() {
        let (lane, param) = match &e.lane {
            LaneKind::Notes => ("notes", None),
            LaneKind::Audio => ("audio", None),
            LaneKind::Automation(p) => ("automation", Some(p)),
        };
        let sep = if i == 0 { "" } else { "," };
        let _ = write!(
            w,
            "{sep}\n{{\"device\": {}, \"lane\": \"{lane}\", ",
            json_str(&e.device.0)
        );
        if let Some(p) = param {
            let _ = write!(w, "\"param\": {}, ", json_str(p));
        }
        let _ = write!(w, "\"note_lane\": {}, \"clip\": ", e.note_lane);
        let mut clip = String::new();
        write_clip(&mut clip, &e.clip, "");
        w.push_str(clip.trim());
        w.push('}');
    }
    w.push_str("]}");
    w
}

/// Reads [`clips_to_text`].
pub fn clips_from_text(text: &str) -> Result<ClipSnapshot, ModelError> {
    let root = JsonPath::default();
    let value = parse(text)?;
    let obj = object(
        &value,
        &root,
        &["origin", "span", "assets", "entries"],
        &["origin", "span", "assets", "entries"],
    )?;
    let origin = int(&obj["origin"], &root.field("origin"))?;
    let span = int(&obj["span"], &root.field("span"))?;
    let assets = load_assets(&obj["assets"], &root.field("assets"))?;
    let mut holder = empty_doc();
    holder.assets = assets.clone();
    let song = empty_song(120.0);
    let entries_path = root.field("entries");
    let items = array(&obj["entries"], &entries_path)?;
    if items.len() > MAX_ITEMS {
        return Err(entries_path.error("too many clips"));
    }
    let mut entries = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let p = entries_path.index(i);
        let e = object(
            item,
            &p,
            &["device", "lane", "param", "note_lane", "clip"],
            &["device", "lane", "note_lane", "clip"],
        )?;
        let device = DeviceKey(string(&e["device"], &p.field("device"))?.to_owned());
        let lane_path = p.field("lane");
        let lane = match string(&e["lane"], &lane_path)? {
            "notes" => LaneKind::Notes,
            "audio" => LaneKind::Audio,
            "automation" => {
                let param = e
                    .get("param")
                    .ok_or_else(|| p.error("an automation clip names its \"param\""))?;
                LaneKind::Automation(string(param, &p.field("param"))?.to_owned())
            }
            other => {
                return Err(lane_path.error(format!(
                    "expected \"notes\", \"audio\" or \"automation\", found {other:?}"
                )));
            }
        };
        let note_lane = uint(&e["note_lane"], &p.field("note_lane"), 4096)? as usize;
        let clip_obj = match &lane {
            LaneKind::Notes => Obj::NoteClip,
            LaneKind::Audio => Obj::AudioClip,
            LaneKind::Automation(_) => Obj::AutomationClip,
        };
        let clip = load_clip(
            &e["clip"],
            &p.field("clip"),
            (clip_obj, u64::from(PROJECT_VERSION)),
            &lane,
            (&holder, None),
            &song,
            &BTreeMap::new(),
        )?;
        entries.push(ClipEntry {
            device,
            lane,
            note_lane,
            clip,
        });
    }
    Ok(ClipSnapshot {
        entries,
        origin,
        span,
        assets,
    })
}

// ---------------------------------------------------------------------------------- notes

/// The text of copied notes: an `atomik-clip` file whose length is the span.
pub fn notes_to_text(s: &NoteSnapshot) -> Result<String, ModelError> {
    let notes: Vec<Note> = s.notes.clone();
    let clip = Clip {
        id: ClipId(0),
        start: 0,
        length: s.span.max(1),
        offset: 0,
        loop_length: None,
        muted: false,
        name: None,
        color: None,
        grid: None,
        scale: None,
        controllers: Vec::new(),
        param_lanes: Vec::new(),
        midi: Default::default(),
        groove: None,
        follow_song_key: true,
        content: ClipContent::Notes(notes.into()),
    };
    save_clip_file(&ClipFile {
        tempo_bpm: 120.0,
        meter: (4, 4),
        clip,
    })
}

/// Reads [`notes_to_text`].
pub fn notes_from_text(text: &str) -> Result<NoteSnapshot, ModelError> {
    let file = load_clip_file(text)?;
    let ClipContent::Notes(notes) = file.clip.content else {
        return Err(JsonPath::default().error("not a note clip"));
    };
    if notes.len() > MAX_ITEMS {
        return Err(JsonPath::default().error("too many notes"));
    }
    Ok(NoteSnapshot {
        notes: notes.to_vec(),
        span: file.clip.length,
    })
}

// ---------------------------------------------------------------------------------- points

/// The text of copied automation points: `{"span", "points": [[tick, value, curve]…]}`.
pub fn points_to_text(s: &PointSnapshot) -> String {
    let mut w = format!("{{\"span\": {}, \"points\": [", s.span);
    for (i, p) in s.points.iter().enumerate() {
        if i > 0 {
            w.push_str(", ");
        }
        write_point(&mut w, p);
    }
    w.push_str("]}");
    w
}

/// Reads [`points_to_text`].
pub fn points_from_text(text: &str) -> Result<PointSnapshot, ModelError> {
    let root = JsonPath::default();
    let value = parse(text)?;
    let obj = object(&value, &root, &["span", "points"], &["span", "points"])?;
    let span = int(&obj["span"], &root.field("span"))?;
    let points_path = root.field("points");
    let items = array(&obj["points"], &points_path)?;
    if items.len() > MAX_ITEMS {
        return Err(points_path.error("too many points"));
    }
    let mut points: Vec<AutomationPoint> = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let point = read_point(item, &points_path.index(i), None)?;
        if let Some(prev) = points.last()
            && prev.tick >= point.tick
        {
            return Err(points_path
                .index(i)
                .error("points must be sorted by tick without duplicates"));
        }
        points.push(point);
    }
    Ok(PointSnapshot { points, span })
}

// -------------------------------------------------------------------------------- devices

/// The text of copied devices: an `atomik-project` holding the devices (all in the first rack
/// column), the cables between them, their song tracks and the pool entries they need, with an
/// empty console. `rate` and `tempo` are the source document's. A track keeps no live groove
/// (the groove templates belong to the song it came from).
pub fn devices_to_text(s: &DeviceSnapshot, rate: u32, tempo: f64) -> String {
    let mut item = empty_doc();
    item.sample_rate = rate;
    item.tempo_bpm = tempo;
    item.devices = s.devices.clone();
    for d in &mut item.devices {
        d.column = 0;
    }
    item.cables = s.cables.clone();
    item.cables.sort();
    item.assets = s.assets.clone();
    let mut song: Song = empty_song(tempo);
    song.tracks = s.tracks.clone();
    for t in &mut song.tracks {
        t.groove = None;
        for lane in &mut t.lanes {
            for c in lane
                .clips
                .iter_mut()
                .chain(lane.take_groups.iter_mut().flat_map(|g| g.takes.iter_mut()))
            {
                c.groove = None;
            }
        }
    }
    song.next_clip_id = song
        .tracks
        .iter()
        .flat_map(|t| t.lanes.iter())
        .flat_map(|l| {
            l.clips
                .iter()
                .chain(l.take_groups.iter().flat_map(|g| g.takes.iter()))
        })
        .map(|c| c.id.0)
        .max()
        .map_or(1, |m| m + 1);
    item.song = Arc::new(song);
    save(&item)
}

/// Reads [`devices_to_text`] (any `atomik-project` works): the devices, cables, tracks and pool
/// entries as a snapshot to paste. Plugin devices the registry lacks load as stand-ins.
pub fn devices_from_text(text: &str, registry: &Registry) -> Result<DeviceSnapshot, ModelError> {
    let doc = load(text, registry)?;
    Ok(DeviceSnapshot {
        devices: doc.devices,
        cables: doc.cables,
        tracks: doc.song.tracks.clone(),
        assets: doc.assets,
    })
}
