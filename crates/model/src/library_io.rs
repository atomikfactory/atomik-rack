//! Device items of the library: `.atomikpatch` and `.atomikcontainer` files.
//!
//! An item is an `atomik-project` text at the current version that holds only devices, cables
//! and the sample pool entries they need, with an empty song and an empty console. Any project
//! reader opens it; the library loads it as a [`DeviceSnapshot`] and pastes it, so a load is one
//! undo step with fresh keys like any other paste. A plugin device loads as a stand-in when its
//! plugin is not installed, as in a project.
//!
//! Also here: the kind of an item and the one line summaries the file browser shows (the
//! browser only needs the text, so the summaries read the JSON without building a document).

use crate::project;
use crate::{
    CONTAINER_TYPE, DeviceSnapshot, ModelError, RackDoc, empty_doc, empty_song, load_clip_file,
};
use atomik_core::PPQ;
use atomik_device_sdk::Registry;
use serde_json::Value;

/// Most device names a patch summary lists before the ellipsis.
const SUMMARY_NAMES: usize = 3;

/// The kinds of library file the browser summarises. `Patch` and `Container` are device items;
/// `Clip` and `Project` have their own writers (`atomik-clip`, `atomik-project`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    /// Devices and the cables between them.
    Patch,
    /// One container device with its inside.
    Container,
    /// One note clip.
    Clip,
    /// A whole project.
    Project,
}

impl ItemKind {
    /// The kind of device item a snapshot makes: a single container device is a
    /// [`Container`](Self::Container), anything else a [`Patch`](Self::Patch).
    pub fn of(snapshot: &DeviceSnapshot) -> Self {
        match snapshot.devices.as_slice() {
            [only] if only.type_id == CONTAINER_TYPE => Self::Container,
            _ => Self::Patch,
        }
    }
}

/// What the browser shows about an item: the detail column text. (The category of an item row
/// is empty; the app builds the library's own row info from this.)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemSummary {
    /// Such as "3 devices: Poly Synth, Delay, Reverb" or "4 bars, 32 notes".
    pub detail: String,
}

/// Writes the library item text of `snapshot`: an `atomik-project` with the snapshot's devices
/// (all in the first rack column), cables and pool entries, no tracks and no console. `doc` is
/// the document the snapshot came from; the item keeps its sample rate and tempo. The text is
/// canonical, so the same snapshot always gives the same bytes.
pub fn save_device_item(snapshot: &DeviceSnapshot, doc: &RackDoc) -> String {
    let mut item = empty_doc();
    item.sample_rate = doc.sample_rate;
    item.tempo_bpm = doc.tempo_bpm;
    item.song = empty_song(doc.tempo_bpm);
    item.devices = snapshot.devices.clone();
    for d in &mut item.devices {
        d.column = 0;
    }
    item.cables = snapshot.cables.clone();
    item.cables.sort();
    item.assets = snapshot.assets.clone();
    project::save(&item)
}

/// Reads an item text (any `atomik-project`, so a whole project works too) as a snapshot to
/// paste: its devices, cables and pool entries. Tracks, the console and the rest of the project
/// are left out. `registry` validates the devices; a plugin device it does not know becomes a
/// stand-in (the app registers those with [`register_stand_ins`](crate::register_stand_ins)
/// before the paste). Errors are [`ModelError::Patch`] with `"<json path>: <what is wrong>"`.
pub fn load_device_item(text: &str, registry: &Registry) -> Result<DeviceSnapshot, ModelError> {
    let doc = project::load(text, registry)?;
    Ok(DeviceSnapshot {
        devices: doc.devices,
        cables: doc.cables,
        tracks: Vec::new(),
        assets: doc.assets,
    })
}

/// The browser's one line summary of an item `text` of `kind`, or `None` when the text is not a
/// readable item of that kind. Patch "3 devices: Poly Synth, Delay, Reverb" (three names, then
/// ", ..."), container "Container, 5 inside", clip "4 bars, 32 notes", project "120 BPM, 8
/// tracks". Reads the JSON only: no registry, no validation beyond the shape it needs.
pub fn summarize_item(kind: ItemKind, text: &str) -> Option<ItemSummary> {
    let detail = match kind {
        ItemKind::Clip => clip_detail(text)?,
        _ => {
            let v: Value = serde_json::from_str(text).ok()?;
            match kind {
                ItemKind::Patch => patch_detail(&v)?,
                ItemKind::Container => container_detail(&v)?,
                _ => project_detail(&v)?,
            }
        }
    };
    Some(ItemSummary { detail })
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn patch_detail(v: &Value) -> Option<String> {
    let devices = v.get("devices")?.as_array()?;
    let mut names: Vec<&str> = devices
        .iter()
        .take(SUMMARY_NAMES)
        .map(|d| d.get("name").and_then(Value::as_str).unwrap_or("?"))
        .collect();
    if devices.len() > SUMMARY_NAMES {
        names.push("...");
    }
    Some(format!(
        "{}: {}",
        plural(devices.len(), "device", "devices"),
        names.join(", ")
    ))
}

fn container_detail(v: &Value) -> Option<String> {
    let inner = v.get("devices")?.as_array()?.first()?.get("inner")?;
    let n = inner.get("devices")?.as_array()?.len();
    Some(format!("Container, {n} inside"))
}

fn clip_detail(text: &str) -> Option<String> {
    let file = load_clip_file(text).ok()?;
    let crate::song::ClipContent::Notes(notes) = &file.clip.content else {
        return None;
    };
    let (num, den) = file.meter;
    let bar = (PPQ * 4 * i64::from(num) / i64::from(den)).max(1);
    let bars = (file.clip.length + bar - 1) / bar;
    Some(format!(
        "{}, {}",
        plural(usize::try_from(bars).ok()?, "bar", "bars"),
        plural(notes.len(), "note", "notes")
    ))
}

fn project_detail(v: &Value) -> Option<String> {
    let bpm = v.get("tempo_bpm")?.as_f64()?;
    let tracks = v.get("song")?.get("tracks")?.as_array()?.len();
    Some(format!("{bpm} BPM, {}", plural(tracks, "track", "tracks")))
}
