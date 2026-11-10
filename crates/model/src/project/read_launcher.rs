//! Reading the song's optional `"launcher"` object (see [`crate::launcher`]).

use super::*;
use crate::launcher::{LaunchQuantize, Launcher, LauncherClip, MAX_SCENES, Scene};

fn read_quantize(v: &Value, path: &JsonPath) -> Result<LaunchQuantize, ModelError> {
    let text = string(v, path)?;
    LaunchQuantize::from_key(text).ok_or_else(|| {
        let all: Vec<&str> = LaunchQuantize::ALL.iter().map(|q| q.key()).collect();
        path.error(format!("expected one of {}", all.join(", ")))
    })
}

/// Reads the launcher: scenes, slots (note clips of note tracks) and the quantize settings. A
/// clip id must be below `next_clip_id` and unused by the lanes (`clip_first` holds theirs).
pub(super) fn load_launcher(
    v: &Value,
    path: &JsonPath,
    (doc, version): (&RackDoc, u64),
    song: &Song,
    clip_first: &mut BTreeMap<u64, JsonPath>,
) -> Result<Launcher, ModelError> {
    let obj = schema::object(v, path, Obj::Launcher, version)?;
    let mut launcher = Launcher::default();
    if let Some(q) = obj.get("quantize") {
        launcher.quantize = read_quantize(q, &path.field("quantize"))?;
    }
    match obj.get("stop_empty") {
        None => {}
        Some(Value::Bool(false)) => launcher.stop_empty = false,
        Some(_) => {
            return Err(path
                .field("stop_empty")
                .error("\"stop_empty\" is false when present"));
        }
    }
    if let Some(list) = obj.get("scenes") {
        let sp = path.field("scenes");
        let items = array(list, &sp)?;
        if items.len() > MAX_SCENES {
            return Err(sp.error(format!("at most {MAX_SCENES} scenes")));
        }
        for (i, item) in items.iter().enumerate() {
            let p = sp.index(i);
            let o = schema::object(item, &p, Obj::LauncherScene, version)?;
            let mut scene = Scene::default();
            if let Some(n) = o.get("name") {
                let np = p.field("name");
                let text = string(n, &np)?;
                if !crate::clip_name_ok(text) {
                    return Err(np.error("the scene name must be trimmed and short"));
                }
                scene.name = text.to_owned();
            }
            if let Some(c) = o.get("color") {
                let cp = p.field("color");
                scene.color = Some(
                    parse_color(c, &cp)?
                        .ok_or_else(|| cp.error("expected a colour \"#rrggbb\""))?,
                );
            }
            if let Some(t) = o.get("tempo") {
                let tp = p.field("tempo");
                let bpm = t
                    .as_f64()
                    .filter(|b| (crate::TEMPO_MIN..=crate::TEMPO_MAX).contains(b))
                    .ok_or_else(|| tp.error("expected a tempo in BPM"))?;
                scene.tempo = Some(bpm);
            }
            launcher.scenes.push(scene);
        }
    }
    if let Some(list) = obj.get("slots") {
        let sp = path.field("slots");
        for (i, item) in array(list, &sp)?.iter().enumerate() {
            let p = sp.index(i);
            let o = schema::object(item, &p, Obj::LauncherSlot, version)?;
            let key = DeviceKey(string(&o["track"], &p.field("track"))?.to_owned());
            let track = song
                .tracks
                .iter()
                .find(|t| t.device == key)
                .ok_or_else(|| p.field("track").error(format!("{:?} has no track", key.0)))?;
            if track.is_automation_only() {
                return Err(p.field("track").error("an automation track has no slots"));
            }
            let row = uint(&o["row"], &p.field("row"), MAX_SCENES as u64)? as usize;
            if row >= launcher.scenes.len() {
                return Err(p.field("row").error("the row is past the last scene"));
            }
            if launcher.slot(&key, row).is_some() {
                return Err(p.field("row").error("that slot is already used"));
            }
            let quantize = match o.get("quantize") {
                Some(q) => Some(read_quantize(q, &p.field("quantize"))?),
                None => None,
            };
            let clip = read_clip::load_clip(
                &o["clip"],
                &p.field("clip"),
                (
                    if track.is_audio() {
                        Obj::AudioClip
                    } else {
                        Obj::NoteClip
                    },
                    version,
                ),
                if track.is_audio() {
                    &LaneKind::Audio
                } else {
                    &LaneKind::Notes
                },
                (doc, None),
                song,
                clip_first,
            )?;
            clip_first.insert(clip.id.0, p.field("clip"));
            let list = launcher.slots.entry(key).or_default();
            if list.len() <= row {
                list.resize(row + 1, None);
            }
            list[row] = Some(LauncherClip { clip, quantize });
        }
    }
    launcher.check().map_err(|m| path.error(m))?;
    Ok(launcher)
}
