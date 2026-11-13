//! Library patch previews: Space on a patch or container row of the browser plays it without
//! loading it (`BrowseAction::Preview` on an `.atomikpatch` or `.atomikcontainer`; Space again,
//! Escape or another preview stops it).
//!
//! The item is read and turned into a **hidden scratch project** that nothing else ever sees: a
//! copy of the song's rack is not used, the project is built from the item alone.
//!
//! * An item that holds an **instrument** (a synth patch, a container with an instrument inside,
//!   a synth followed by effects) is pasted as a copy would be (the instrument gets its console
//!   channel and the master, the end of a chain of several devices gets a channel too) and a
//!   song track on the device that takes notes plays a short built-in phrase: a low arpeggio
//!   and a four-note chord, at 120 BPM (a drum machine plays a one-bar pattern on its pad keys
//!   instead, since the arpeggio would fall on pads that a kit leaves empty).
//! * An item that holds **effects** only (a single-device effect patch, a chain of effects or
//!   an effect container) goes into the insert chain of the channel of a built-in synth that
//!   plays a short staccato phrase, so the effect is heard on a signal with gaps for its tail.
//! * Anything else (plugins, which the preview does not host yet, CV utilities, an item with
//!   nothing that makes or takes sound) is refused with a message in the status line.
//!
//! The scratch project is rendered offline, with the same renderer as a bounce, on a worker
//! thread (`atomik-patch-preview`): the devices are built there, never on the audio thread, and
//! dropped there when the preview is over. The result (a few seconds with the tail, scaled down
//! when it would clip) is fed into the **preview voice** of the file browser
//! (`atomik_engine::preview_channel`), so the press and stop rules are those of an audio file
//! preview: it ends by itself, a stop or another preview replaces it, and the audio thread only
//! ever sees a stream of samples (no allocation, no lock, no device built or dropped on it).
//! The stream starts at once and plays silence until the render is in the ring.
//!
//! The document, the selection and the undo history are never touched (the patch strip of a
//! device, which auditions a patch in place on that device, is the other way to hear one), and
//! an offline render never hears a preview: bounces and exports read no preview voice. The
//! worker's failure (a device panic, a sample file that is gone) reaches the status line from
//! the next frame.

use super::*;
use atomik_core::PPQ;
use atomik_device_sdk::{DeviceDescriptor, DeviceKind, Direction, SocketKind};
use atomik_engine::{PreviewFeed, preview_channel};
use atomik_model::console::BUS_TYPE;
use atomik_model::{
    Clip, ConsoleCommand, DeviceSnapshot, InsertEffect, ItemKind, Note, load_device_item,
};
use atomik_render::Rendered;
use atomik_render::project::{ProjectOptions, render_doc_in};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

/// The synth that plays the phrase for an effect.
const SOURCE_TYPE: &str = "atomik.poly-synth";
/// The key of the built-in synth in the scratch project.
const SOURCE_KEY: &str = "preview_source";
/// The song end of the scratch project in beats (the render adds its tail after it).
const SONG_BEATS: i64 = 8;
/// The most devices an effect chain may hold (the channel's insert slots).
const MAX_CHAIN: usize = 16;
/// The loudest sample the preview plays: a louder render is scaled down to it.
const CEILING: f32 = 0.9;
/// The level the end of a render is cut at: the tail ends where the signal stays below it.
const SILENCE: f32 = 1.0e-4;
/// How long the cut end rings on and fades, in seconds.
const END_HOLD: f64 = 0.05;
/// How long the worker waits for room in a full ring.
const WAIT: Duration = Duration::from_millis(2);

/// The error text of a preview that is not a refusal at the press.
type Failure = String;

/// The patch preview that is running: where its worker reports a failure.
#[derive(Debug)]
pub(super) struct PatchPreview {
    /// The worker's failure, if it has one.
    failures: Receiver<Failure>,
}

/// A note of the phrase in beats: `(start, length, key, velocity)`.
pub(super) type Phrase = &'static [(f64, f64, u8, f32)];

/// The phrase an instrument plays: a four-note arpeggio, then a chord held two beats.
const INSTRUMENT_PHRASE: Phrase = &[
    (0.0, 0.5, 48, 0.8),
    (0.5, 0.5, 55, 0.7),
    (1.0, 0.5, 60, 0.7),
    (1.5, 0.5, 64, 0.7),
    (2.0, 2.0, 48, 0.8),
    (2.0, 2.0, 55, 0.7),
    (2.0, 2.0, 60, 0.7),
    (2.0, 2.0, 64, 0.7),
];

/// The phrase a drum machine plays: a bar of kick, snare, clap, hats, percussion, toms and a rim
/// click on the pad keys of a kit (36 to 46).
const DRUM_PHRASE: Phrase = &[
    (0.0, 0.25, 36, 0.9),
    (0.0, 0.25, 42, 0.6),
    (0.5, 0.25, 42, 0.5),
    (1.0, 0.25, 38, 0.9),
    (1.0, 0.25, 39, 0.6),
    (1.5, 0.25, 42, 0.5),
    (2.0, 0.25, 36, 0.9),
    (2.0, 0.25, 42, 0.6),
    (2.5, 0.25, 43, 0.7),
    (2.75, 0.25, 41, 0.8),
    (3.0, 0.25, 45, 0.8),
    (3.25, 0.25, 37, 0.7),
    (3.5, 0.25, 46, 0.7),
];

/// The built-in drum machine, which plays [`DRUM_PHRASE`].
const DRUM_TYPE: &str = "atomik.drum-machine";

/// The phrase an effect hears: short notes with gaps between them.
pub(super) const EFFECT_PHRASE: Phrase = &[
    (0.0, 0.25, 60, 0.9),
    (1.0, 0.25, 64, 0.8),
    (2.0, 0.25, 67, 0.8),
    (3.0, 0.25, 72, 0.8),
];

/// A note clip of `phrase` at tick 0, a bar long.
pub(super) fn phrase_clip(phrase: Phrase) -> Clip {
    #[allow(clippy::cast_possible_truncation)] // beats of a constant table
    let ticks = |beats: f64| (beats * PPQ as f64).round() as i64;
    let notes: Vec<Note> = phrase
        .iter()
        .map(|&(at, length, key, velocity)| Note::new(ticks(at), ticks(length), key, velocity))
        .collect();
    Clip {
        id: ClipId(0),
        start: 0,
        length: 4 * PPQ,
        offset: 0,
        loop_length: None,
        muted: false,
        scale: None,
        param_lanes: Vec::new(),
        midi: Default::default(),
        groove: None,
        follow_song_key: true,
        controllers: Vec::new(),
        name: None,
        color: None,
        grid: None,
        content: ClipContent::Notes(notes.into()),
    }
}

fn socket_of(desc: &DeviceDescriptor, key: &str, kind: SocketKind, dir: Direction) -> bool {
    desc.socket_by_key(key)
        .is_some_and(|s| s.kind == kind && s.direction == dir)
}

/// Whether the device is a plugin or holds one.
fn holds_plugin(device: &RackDevice) -> bool {
    device.plugin.is_some()
        || device
            .inner
            .as_ref()
            .is_some_and(|i| i.devices.iter().any(holds_plugin))
}

/// Whether the device is of type `type_id` or holds one.
fn holds_type(device: &RackDevice, type_id: &str) -> bool {
    device.type_id == type_id
        || device
            .inner
            .as_ref()
            .is_some_and(|i| i.devices.iter().any(|d| holds_type(d, type_id)))
}

/// Whether the device is an instrument or holds one.
fn holds_instrument(device: &RackDevice, registry: &Registry) -> bool {
    registry
        .descriptor(&device.type_id)
        .is_some_and(|d| d.kind == DeviceKind::Instrument)
        || device
            .inner
            .as_ref()
            .is_some_and(|i| i.devices.iter().any(|d| holds_instrument(d, registry)))
}

/// Whether the device is a stereo effect (or a container that is not an instrument): audio in
/// and out on `in_l`/`in_r` and `out_l`/`out_r`.
fn is_stereo_effect(device: &RackDevice, registry: &Registry) -> bool {
    let Some(desc) = registry.descriptor(&device.type_id) else {
        return false;
    };
    let kind_ok = match desc.kind {
        DeviceKind::Effect => true,
        DeviceKind::Container => !holds_instrument(device, registry),
        _ => false,
    };
    kind_ok
        && ["in_l", "in_r"]
            .iter()
            .all(|k| socket_of(desc, k, SocketKind::Audio, Direction::Input))
        && ["out_l", "out_r"]
            .iter()
            .all(|k| socket_of(desc, k, SocketKind::Audio, Direction::Output))
}

fn applied(doc: &mut Document, registry: &Registry, command: Command) -> Result<(), String> {
    doc.apply(command, registry)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// The keys of the top-level devices of `doc`.
fn keys_of(doc: &Document) -> Vec<DeviceKey> {
    doc.doc().devices.iter().map(|d| d.key.clone()).collect()
}

/// The devices a paste added to `doc` that are not console devices, in rack order.
fn added_since(doc: &Document, before: &[DeviceKey]) -> Vec<DeviceKey> {
    doc.doc()
        .devices
        .iter()
        .filter(|d| !before.contains(&d.key))
        .filter(|d| !matches!(d.type_id.as_str(), CHANNEL_TYPE | MASTER_TYPE | BUS_TYPE))
        .map(|d| d.key.clone())
        .collect()
}

/// The scratch project that plays `snapshot` (see the module docs).
///
/// # Errors
/// The item holds a plugin, or nothing that makes or takes sound, or the model refuses a step.
pub(super) fn preview_doc(
    snapshot: &DeviceSnapshot,
    registry: &Registry,
) -> Result<RackDoc, String> {
    if snapshot.devices.is_empty() {
        return Err("it holds no devices".into());
    }
    if snapshot.devices.iter().any(holds_plugin) {
        return Err("a patch with a plugin cannot be previewed yet: load it to hear it".into());
    }
    let mut base = new_project_doc();
    base.assets.clone_from(&snapshot.assets);
    let mut doc = Document::new(base);
    if snapshot
        .devices
        .iter()
        .any(|d| holds_instrument(d, registry))
    {
        instrument_scene(&mut doc, snapshot, registry)?;
    } else {
        effect_scene(&mut doc, snapshot, registry)?;
    }
    applied(
        &mut doc,
        registry,
        Command::Song(SongCommand::SetSongEnd(SONG_BEATS * PPQ)),
    )?;
    Ok(doc.doc().clone())
}

/// The pasted items of an instrument patch, wired to the master, with a track that plays the
/// first device that takes notes.
fn instrument_scene(
    doc: &mut Document,
    snapshot: &DeviceSnapshot,
    registry: &Registry,
) -> Result<(), String> {
    let before = keys_of(doc);
    applied(
        doc,
        registry,
        Command::Workflow(WorkflowCommand::PasteDevices {
            snapshot: snapshot.clone(),
            container: None,
            at: usize::MAX,
            route: true,
        }),
    )?;
    let pasted = added_since(doc, &before);
    let descriptor_of = |doc: &Document, key: &DeviceKey| {
        doc.doc()
            .devices
            .iter()
            .find(|d| &d.key == key)
            .and_then(|d| registry.descriptor(&d.type_id))
            .copied()
    };
    // The end of a chain of devices has no channel yet: give it one.
    for key in &pasted {
        let Some(desc) = descriptor_of(doc, key) else {
            continue;
        };
        let feeds_on = doc.doc().cables.iter().any(|c| {
            &c.from.device == key
                && pasted.contains(&c.to.device)
                && desc
                    .socket_by_key(&c.from.socket)
                    .is_some_and(|s| s.kind == SocketKind::Audio)
        });
        if !feeds_on && doc.doc().console.channel_of(key).is_none() {
            // A device with no stereo audio output stays unwired: other devices carry the sound.
            let _ = applied(
                doc,
                registry,
                Command::Console(ConsoleCommand::AddChannel {
                    source: key.clone(),
                }),
            );
        }
    }
    let entry = pasted.iter().find(|key| {
        descriptor_of(doc, key).is_some_and(|desc| {
            desc.sockets
                .iter()
                .filter(|s| s.kind == SocketKind::Notes && s.direction == Direction::Input)
                .any(|s| {
                    !doc.doc()
                        .cables
                        .iter()
                        .any(|c| &c.to.device == *key && c.to.socket == s.key)
                })
        })
    });
    let Some(entry) = entry.cloned() else {
        return Err("it has no device that takes notes to play".into());
    };
    let drums = doc
        .doc()
        .devices
        .iter()
        .any(|d| d.key == entry && holds_type(d, DRUM_TYPE));
    let phrase = if drums {
        DRUM_PHRASE
    } else {
        INSTRUMENT_PHRASE
    };
    play_track(doc, registry, &entry, phrase)
}

/// A track on `device` that plays `phrase`.
fn play_track(
    doc: &mut Document,
    registry: &Registry,
    device: &DeviceKey,
    phrase: Phrase,
) -> Result<(), String> {
    applied(
        doc,
        registry,
        Command::Song(SongCommand::AddTrack {
            device: device.clone(),
        }),
    )?;
    applied(
        doc,
        registry,
        Command::Song(SongCommand::AddClip {
            device: device.clone(),
            lane: 0,
            clip: phrase_clip(phrase),
        }),
    )
}

/// The effects of an effect item in the insert chain of a built-in synth that plays the phrase.
fn effect_scene(
    doc: &mut Document,
    snapshot: &DeviceSnapshot,
    registry: &Registry,
) -> Result<(), String> {
    if !snapshot
        .devices
        .iter()
        .all(|d| is_stereo_effect(d, registry))
    {
        return Err("it holds no instrument and is not a chain of stereo effects".into());
    }
    if snapshot.devices.len() > MAX_CHAIN {
        return Err(format!("it holds more than {MAX_CHAIN} effects"));
    }
    let source = DeviceKey(SOURCE_KEY.to_string());
    applied(
        doc,
        registry,
        Command::AddDevice {
            index: usize::MAX,
            type_id: SOURCE_TYPE.to_string(),
            key: Some(source.clone()),
        },
    )?;
    play_track(doc, registry, &source, EFFECT_PHRASE)?;
    let channel = doc
        .doc()
        .console
        .channel_of(&source)
        .map(|c| c.channel.clone())
        .ok_or_else(|| "the built-in synth has no channel".to_string())?;
    // The effects go in the chain in the item's order; the cables of the item are not kept.
    let mut chain = snapshot.clone();
    chain.cables.clear();
    let before = keys_of(doc);
    applied(
        doc,
        registry,
        Command::Workflow(WorkflowCommand::PasteDevices {
            snapshot: chain,
            container: None,
            at: usize::MAX,
            route: false,
        }),
    )?;
    for (slot, key) in added_since(doc, &before).into_iter().enumerate() {
        applied(
            doc,
            registry,
            Command::Console(ConsoleCommand::SetInsert {
                channel: channel.clone(),
                slot,
                effect: Some(InsertEffect::Existing(key)),
            }),
        )?;
    }
    Ok(())
}

/// Renders the scratch project `doc` and prepares what the preview voice plays: two planes at
/// the render's rate, the silence at the end cut and a short fade there, scaled down when the
/// peak is above [`CEILING`]. Returns the planes and the rate.
///
/// # Errors
/// The render fails (a device panics, a sample file is missing) or the result is silent.
pub(super) fn render_preview(
    doc: &RackDoc,
    bundle_root: Option<&Path>,
) -> Result<(Vec<Vec<f32>>, u32), String> {
    let registry = atomik_render::default_registry();
    let rendered = catch_unwind(AssertUnwindSafe(|| {
        render_doc_in(doc, &registry, &ProjectOptions::default(), bundle_root)
    }))
    .map_err(|_| "a device failed while the preview rendered".to_string())?
    .map_err(|e| e.to_string())?;
    finish_planes(rendered)
}

fn finish_planes(rendered: Rendered) -> Result<(Vec<Vec<f32>>, u32), String> {
    let rate = rendered.sample_rate;
    let frames = rendered.channels.iter().map(Vec::len).min().unwrap_or(0);
    let mut planes: Vec<Vec<f32>> = rendered
        .channels
        .into_iter()
        .take(2)
        .map(|mut c| {
            c.truncate(frames);
            for s in &mut c {
                if !s.is_finite() {
                    *s = 0.0;
                }
            }
            c
        })
        .collect();
    match planes.len() {
        0 => return Err("the render has no audio".into()),
        1 => planes.push(planes[0].clone()),
        _ => {}
    }
    let level = |i: usize| planes.iter().fold(0.0f32, |p, c| p.max(c[i].abs()));
    let last = (0..frames).rev().find(|&i| level(i) > SILENCE);
    let Some(last) = last else {
        return Err("it makes no sound".into());
    };
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // a short constant
    let hold = (END_HOLD * f64::from(rate)).round() as usize;
    let end = (last + 1 + hold).min(frames);
    for c in &mut planes {
        c.truncate(end);
        let fade = hold.min(end);
        for (i, s) in c[end - fade..].iter_mut().enumerate() {
            #[allow(clippy::cast_precision_loss)] // a fade position
            let gain = 1.0 - (i + 1) as f32 / fade as f32;
            *s *= gain;
        }
    }
    let peak = planes
        .iter()
        .flat_map(|c| c.iter())
        .fold(0.0f32, |p, s| p.max(s.abs()));
    if peak > CEILING {
        let scale = CEILING / peak;
        for s in planes.iter_mut().flat_map(|c| c.iter_mut()) {
            *s *= scale;
        }
    }
    Ok((planes, rate))
}

/// The worker: renders the scratch project and feeds the result, or reports why not.
fn feed_render(
    mut feed: PreviewFeed,
    doc: RackDoc,
    bundle_root: Option<PathBuf>,
    failures: Sender<Failure>,
) {
    match render_preview(&doc, bundle_root.as_deref()) {
        Ok((planes, _)) => {
            let mut done = 0;
            let total = planes[0].len();
            while done < total {
                if feed.is_abandoned() {
                    return;
                }
                let n = feed.push(&planes, done);
                if n == 0 {
                    std::thread::sleep(WAIT);
                }
                done += n;
            }
        }
        Err(message) => {
            let _ = failures.send(message);
        }
    }
    feed.finish();
}

impl<S: EngineSink> AppCore<S> {
    /// `BrowseAction::Preview` of a patch or container row: see the module docs. A refusal goes
    /// to the status line.
    pub(super) fn preview_library_patch(&mut self, path: &Path, container: bool) {
        if let Err(message) = self.start_patch_preview(path, container) {
            self.reject(message);
        }
    }

    fn start_patch_preview(&mut self, path: &Path, container: bool) -> Result<(), String> {
        let name = path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let text = atomik_model::read_document_text(path)
            .map_err(|e| format!("'{name}' cannot be read: {e}"))?;
        let snapshot = load_device_item(&text, &self.registry)
            .map_err(|e| format!("'{name}' cannot be loaded: {e}"))?;
        if container && ItemKind::of(&snapshot) != ItemKind::Container {
            return Err(format!("'{name}' is not a container"));
        }
        let doc = preview_doc(&snapshot, &self.registry)
            .map_err(|e| format!("'{name}' cannot be previewed: {e}"))?;
        // Every other preview gives way: a browser file, a clip, an earlier patch.
        self.stop_preview();
        if !self.sink.has_engine() {
            return Err("Previewing needs audio: it is off or its stream did not open".into());
        }
        let rate = doc.sample_rate;
        let (mut feed, stream) = preview_channel();
        feed.set_source_rate(rate);
        let monitor = stream.monitor();
        self.sink
            .preview_start(stream, self.preview_gain())
            .map_err(|e| format!("preview: {e}"))?;
        let (send, failures) = channel();
        let root = path.with_extension("");
        let root = root.is_dir().then_some(root);
        let spawned = std::thread::Builder::new()
            .name("atomik-patch-preview".into())
            .spawn(move || feed_render(feed, doc, root, send));
        if let Err(e) = spawned {
            let _ = self.sink.preview_stop();
            return Err(format!("preview: cannot start the renderer: {e}"));
        }
        // The song end and the render's tail at 120 BPM (a render cut at its silence is shorter).
        let seconds = (SONG_BEATS + 2) as f64 * 0.5;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // a short constant
        let frames = (seconds * f64::from(rate)).round() as u64;
        self.adopt_preview(path.to_path_buf(), monitor, frames);
        self.patch_preview = Some(PatchPreview { failures });
        Ok(())
    }

    /// Forgets the failure channel of the patch preview (a stop or a new preview).
    pub(super) fn forget_patch_preview(&mut self) {
        self.patch_preview = None;
    }

    /// One frame of the patch preview: a failure of the worker goes to the status line.
    pub(super) fn poll_patch_preview(&mut self) {
        let Some(preview) = &self.patch_preview else {
            return;
        };
        if let Ok(message) = preview.failures.try_recv() {
            self.patch_preview = None;
            self.reject(format!("The patch preview failed: {message}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sink::{RecordingSink, SinkCall};
    use atomik_model::{ContainerCommand, empty_doc, save_device_item};
    use atomik_render::default_registry;
    use atomik_ui::BrowseAction;
    use std::sync::atomic::{AtomicU64, Ordering};

    const SYNTH: &str = "atomik.poly-synth";
    const DELAY: &str = "atomik.stereo-delay";
    const REVERB: &str = "atomik.reverb";

    fn tmp(tag: &str) -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "atomik-app-patch-preview-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn key(k: &str) -> DeviceKey {
        DeviceKey(k.to_string())
    }

    fn apply(doc: &mut Document, command: Command) {
        doc.apply(command.clone(), &default_registry())
            .unwrap_or_else(|e| panic!("{command:?}: {e}"));
    }

    /// A project with the devices `types` (keys `d0`, `d1`, ...), each on its channel when it is
    /// an instrument.
    fn project_of(types: &[&str]) -> Document {
        let mut doc = Document::new(empty_doc());
        for (i, t) in types.iter().enumerate() {
            apply(
                &mut doc,
                Command::AddDevice {
                    index: usize::MAX,
                    type_id: (*t).to_string(),
                    key: Some(key(&format!("d{i}"))),
                },
            );
        }
        doc
    }

    /// The item text of the devices `keys` of `doc`, as Save to Library writes it.
    fn item_text(doc: &Document, keys: &[&str]) -> String {
        let keys: Vec<DeviceKey> = keys.iter().map(|k| key(k)).collect();
        let snapshot = DeviceSnapshot::capture(doc.doc(), &keys, false).unwrap();
        save_device_item(&snapshot, doc.doc())
    }

    /// The item text of a container that holds all the devices of a project of `types`.
    fn container_text(types: &[&str]) -> String {
        let mut doc = project_of(types);
        let keys = (0..types.len()).map(|i| key(&format!("d{i}"))).collect();
        apply(
            &mut doc,
            Command::Container(ContainerCommand::Combine {
                keys,
                name: "Combo".into(),
            }),
        );
        let container: Vec<DeviceKey> = doc
            .doc()
            .devices
            .iter()
            .filter(|d| d.type_id == atomik_model::CONTAINER_TYPE)
            .map(|d| d.key.clone())
            .collect();
        let snapshot = DeviceSnapshot::capture(doc.doc(), &container, false).unwrap();
        save_device_item(&snapshot, doc.doc())
    }

    /// The item text of a container with a synth inside, played through the container's note
    /// split and cabled to its outputs.
    fn synth_container_text() -> String {
        let mut doc = Document::new(empty_doc());
        apply(
            &mut doc,
            Command::AddDevice {
                index: usize::MAX,
                type_id: atomik_model::CONTAINER_TYPE.into(),
                key: Some(key("c")),
            },
        );
        apply(
            &mut doc,
            Command::Container(ContainerCommand::AddInner {
                container: key("c"),
                index: usize::MAX,
                type_id: SYNTH.into(),
                key: Some(key("syn")),
                auto_split: true,
            }),
        );
        for side in ["l", "r"] {
            apply(
                &mut doc,
                Command::Connect(atomik_model::Cable {
                    from: atomik_model::SocketRef {
                        device: key("c/syn"),
                        socket: format!("out_{side}"),
                    },
                    to: atomik_model::SocketRef {
                        device: key("c/boundary"),
                        socket: format!("out_{side}"),
                    },
                }),
            );
        }
        let snapshot = DeviceSnapshot::capture(doc.doc(), &[key("c")], false).unwrap();
        save_device_item(&snapshot, doc.doc())
    }

    fn scene_of(text: &str) -> Result<RackDoc, String> {
        let registry = default_registry();
        let snapshot = load_device_item(text, &registry).unwrap();
        preview_doc(&snapshot, &registry)
    }

    fn peak(planes: &[Vec<f32>]) -> f32 {
        planes
            .iter()
            .flat_map(|c| c.iter())
            .fold(0.0f32, |p, s| p.max(s.abs()))
    }

    fn phrase_notes(doc: &RackDoc) -> usize {
        match &doc.song.tracks[0].lanes[0].clips[0].content {
            ClipContent::Notes(n) => n.len(),
            _ => 0,
        }
    }

    fn insert_types(doc: &RackDoc) -> Vec<String> {
        let channel = doc
            .console
            .channel_of(&key(SOURCE_KEY))
            .unwrap()
            .channel
            .clone();
        atomik_model::insert_chain(doc, &default_registry(), &channel)
            .iter()
            .map(|k| doc.resolve(k).unwrap().type_id.clone())
            .collect()
    }

    #[test]
    fn a_synth_patch_plays_the_phrase_on_its_own_track_and_makes_sound() {
        let text = item_text(&project_of(&[SYNTH]), &["d0"]);
        let doc = scene_of(&text).unwrap();
        assert_eq!(doc.song.tracks.len(), 1);
        assert_eq!(phrase_notes(&doc), INSTRUMENT_PHRASE.len());
        let track = doc.song.tracks[0].device.clone();
        assert!(doc.console.channel_of(&track).is_some(), "it has a channel");
        let (planes, rate) = render_preview(&doc, None).unwrap();
        assert_eq!((planes.len(), rate), (2, 48_000));
        assert_eq!(planes[0].len(), planes[1].len());
        let p = peak(&planes);
        assert!((0.02..=CEILING + 1e-6).contains(&p), "{p}");
        // The end is faded out where the sound stops.
        assert!(planes[0][planes[0].len() - 1].abs() < 1e-3);
    }

    #[test]
    fn an_effect_patch_goes_in_the_insert_chain_of_a_built_in_synth() {
        let text = item_text(&project_of(&[DELAY]), &["d0"]);
        let doc = scene_of(&text).unwrap();
        assert_eq!(insert_types(&doc), [DELAY]);
        assert_eq!(phrase_notes(&doc), EFFECT_PHRASE.len());
        let (planes, _) = render_preview(&doc, None).unwrap();
        assert!(peak(&planes) > 0.01);
        // The delay rings on after the notes: the render is longer than the dry phrase.
        let mut dry = Document::new(doc.clone());
        let channel = doc
            .console
            .channel_of(&key(SOURCE_KEY))
            .unwrap()
            .channel
            .clone();
        apply(
            &mut dry,
            Command::Console(ConsoleCommand::SetInsert {
                channel,
                slot: 0,
                effect: None,
            }),
        );
        let (dry_planes, _) = render_preview(dry.doc(), None).unwrap();
        assert!(planes[0].len() > dry_planes[0].len());
    }

    #[test]
    fn a_drum_machine_plays_a_drum_pattern_on_its_pad_keys_and_a_synth_keeps_its_phrase() {
        let drums = scene_of(&item_text(&project_of(&["atomik.drum-machine"]), &["d0"])).unwrap();
        assert_eq!(phrase_notes(&drums), DRUM_PHRASE.len());
        let ClipContent::Notes(notes) = &drums.song.tracks[0].lanes[0].clips[0].content else {
            panic!("a note clip");
        };
        assert!(
            notes.iter().all(|n| (36..=51).contains(&n.key)),
            "pad keys only"
        );
        let synth = scene_of(&item_text(&project_of(&[SYNTH]), &["d0"])).unwrap();
        assert_eq!(phrase_notes(&synth), INSTRUMENT_PHRASE.len());
        // A container that holds a drum machine plays the pattern too.
        let text = container_text(&["atomik.drum-machine"]);
        assert_eq!(phrase_notes(&scene_of(&text).unwrap()), DRUM_PHRASE.len());
    }

    #[test]
    fn a_chain_of_two_effects_keeps_their_order() {
        let text = item_text(&project_of(&[DELAY, REVERB]), &["d0", "d1"]);
        let doc = scene_of(&text).unwrap();
        assert_eq!(insert_types(&doc), [DELAY, REVERB]);
    }

    #[test]
    fn a_container_with_an_instrument_plays_the_phrase_through_its_own_channel() {
        let doc = scene_of(&synth_container_text()).unwrap();
        assert_eq!(doc.song.tracks.len(), 1);
        let container = doc.song.tracks[0].device.clone();
        assert_eq!(
            doc.resolve(&container).unwrap().type_id,
            atomik_model::CONTAINER_TYPE
        );
        assert!(doc.console.channel_of(&container).is_some());
        let (planes, _) = render_preview(&doc, None).unwrap();
        assert!(peak(&planes) > 0.02);
    }

    #[test]
    fn an_effect_container_goes_in_the_insert_chain() {
        let doc = scene_of(&container_text(&[DELAY, REVERB])).unwrap();
        assert_eq!(insert_types(&doc), [atomik_model::CONTAINER_TYPE]);
        let (planes, _) = render_preview(&doc, None).unwrap();
        assert!(peak(&planes) > 0.01);
    }

    #[test]
    fn a_synth_with_its_effect_on_the_channel_previews_with_both() {
        let mut project = project_of(&[SYNTH, DELAY]);
        let channel = project.doc().console.channels[0].channel.clone();
        apply(
            &mut project,
            Command::Console(ConsoleCommand::SetInsert {
                channel,
                slot: 0,
                effect: Some(InsertEffect::Existing(key("d1"))),
            }),
        );
        let text = item_text(&project, &["d0", "d1"]);
        let doc = scene_of(&text).unwrap();
        assert_eq!(doc.song.tracks.len(), 1);
        let (planes, _) = render_preview(&doc, None).unwrap();
        assert!(peak(&planes) > 0.02);
    }

    #[test]
    fn an_item_without_sound_and_an_empty_item_are_refused() {
        let registry = default_registry();
        let text = item_text(&project_of(&["atomik.cv-math"]), &["d0"]);
        let why = scene_of(&text).unwrap_err();
        assert!(why.contains("not a chain of stereo effects"), "{why}");
        let empty = DeviceSnapshot {
            devices: Vec::new(),
            cables: Vec::new(),
            tracks: Vec::new(),
            assets: Default::default(),
        };
        assert!(preview_doc(&empty, &registry).is_err());
    }

    #[test]
    fn a_plugin_item_is_refused_before_anything_is_built() {
        let registry = default_registry();
        let text = item_text(&project_of(&[SYNTH]), &["d0"]);
        let mut snapshot = load_device_item(&text, &registry).unwrap();
        let plugin = atomik_device_sdk::PluginRef {
            format: atomik_device_sdk::PluginFormat::Clap,
            id: "test.plugin".into(),
        };
        let layout = atomik_device_sdk::PluginLayout {
            name: "Test".into(),
            vendor: String::new(),
            version: String::new(),
            kind_hint: atomik_device_sdk::PluginKindHint::Instrument,
            main_in: None,
            main_out: None,
            sidechain: false,
            notes_in: true,
            notes_out: false,
            aux_out: 0,
            params: Vec::new(),
        };
        snapshot.devices[0].plugin = Some(Box::new(atomik_model::PluginDoc::new(
            &plugin, layout, None,
        )));
        let why = preview_doc(&snapshot, &registry).unwrap_err();
        assert!(why.contains("plugin"), "{why}");
    }

    #[test]
    fn a_silent_or_loud_render_is_cut_and_scaled() {
        let render = |left: Vec<f32>, right: Vec<f32>| Rendered {
            channels: vec![left, right],
            sample_rate: 48_000,
            warnings: Vec::new(),
            plugins: 0,
        };
        let silent = render(vec![0.0; 100], vec![0.0; 100]);
        assert!(finish_planes(silent).unwrap_err().contains("no sound"));
        let mut left = vec![0.0f32; 48_000];
        left[100] = 4.0;
        left[1_000] = -2.0;
        left[2_000] = f32::NAN;
        let (planes, rate) = finish_planes(render(left, vec![0.0; 48_000])).unwrap();
        assert_eq!(rate, 48_000);
        // Cut 50 ms after the last sound; the loudest sample is the ceiling.
        assert_eq!(planes[0].len(), 1_001 + 2_400);
        assert!((peak(&planes) - CEILING).abs() < 1e-6);
        assert!(planes[0].iter().all(|s| s.is_finite()));
        assert_eq!(planes[0].last().copied().map(f32::abs), Some(0.0));
    }

    fn core_with_item(text: &str, ext: &str) -> (AppCore<RecordingSink>, PathBuf) {
        let dir = tmp("core");
        let path = dir.join(format!("item.{ext}"));
        std::fs::write(&path, text).unwrap();
        (
            AppCore::with_doc(RecordingSink::new(), empty_doc(), None),
            path,
        )
    }

    fn previews(c: &AppCore<RecordingSink>) -> Vec<SinkCall> {
        c.sink()
            .calls
            .iter()
            .filter(|call| matches!(call, SinkCall::PreviewStart { .. } | SinkCall::PreviewStop))
            .cloned()
            .collect()
    }

    #[test]
    fn previewing_a_patch_starts_the_preview_voice_and_changes_nothing() {
        let text = item_text(&project_of(&[SYNTH]), &["d0"]);
        let (mut c, path) = core_with_item(&text, "atomikpatch");
        let revision = c.document().revision();
        let depth = c.document().undo_depth();
        c.browse(BrowseAction::Preview(path.clone()));
        assert_eq!(previews(&c), [SinkCall::PreviewStart { gain: 1.0 }]);
        assert_eq!(c.preview_path(), Some(path.as_path()));
        assert_eq!(c.document().revision(), revision);
        assert_eq!(c.document().undo_depth(), depth);
        assert!(c.doc().devices.is_empty(), "no device was loaded");
        // Space again, or Escape, stops it like an audio file.
        c.browse(BrowseAction::StopPreview);
        assert_eq!(previews(&c).last(), Some(&SinkCall::PreviewStop));
        assert_eq!(c.preview_path(), None);
        assert_eq!(c.document().revision(), revision);
    }

    #[test]
    fn a_patch_preview_plays_at_the_preview_level() {
        let text = item_text(&project_of(&[SYNTH]), &["d0"]);
        let (mut c, path) = core_with_item(&text, "atomikpatch");
        c.set_preview_db(6.0);
        c.browse(BrowseAction::Preview(path));
        let [SinkCall::PreviewStart { gain }] = previews(&c)[..] else {
            panic!("{:?}", previews(&c))
        };
        assert!((gain - 1.995_262).abs() < 1e-4, "{gain}");
    }

    #[test]
    fn a_container_row_previews_and_a_patch_named_as_a_container_is_refused() {
        let (mut c, path) = core_with_item(&synth_container_text(), "atomikcontainer");
        c.browse(BrowseAction::Preview(path));
        assert_eq!(previews(&c), [SinkCall::PreviewStart { gain: 1.0 }]);
        let patch = item_text(&project_of(&[SYNTH]), &["d0"]);
        let (mut c, path) = core_with_item(&patch, "atomikcontainer");
        c.browse(BrowseAction::Preview(path));
        assert!(previews(&c).is_empty());
        assert!(
            c.last_rejection()
                .is_some_and(|m| m.contains("not a container"))
        );
    }

    #[test]
    fn another_preview_replaces_it_and_a_refusal_starts_nothing() {
        let text = item_text(&project_of(&[DELAY]), &["d0"]);
        let (mut c, path) = core_with_item(&text, "atomikpatch");
        c.browse(BrowseAction::Preview(path.clone()));
        c.browse(BrowseAction::Preview(path.clone()));
        let calls = previews(&c);
        assert_eq!(calls.len(), 3, "start, stop, start: {calls:?}");
        assert_eq!(calls[1], SinkCall::PreviewStop);
        // A file that is not a patch is a status message.
        let bad = path.with_file_name("bad.atomikpatch");
        std::fs::write(&bad, "nonsense").unwrap();
        c.browse(BrowseAction::StopPreview);
        let before = previews(&c).len();
        c.browse(BrowseAction::Preview(bad));
        assert_eq!(previews(&c).len(), before);
        assert!(
            c.last_rejection()
                .is_some_and(|m| m.contains("cannot be loaded"))
        );
    }

    #[test]
    fn without_audio_the_patch_preview_is_refused() {
        let text = item_text(&project_of(&[SYNTH]), &["d0"]);
        let (mut c, path) = core_with_item(&text, "atomikpatch");
        c.sink_mut().no_engine = true;
        c.browse(BrowseAction::Preview(path));
        assert_eq!(c.preview_path(), None);
        assert!(c.last_rejection().is_some());
    }

    #[test]
    fn the_worker_gives_up_when_the_stream_is_dropped() {
        let text = item_text(&project_of(&[SYNTH]), &["d0"]);
        let doc = scene_of(&text).unwrap();
        let (feed, stream) = preview_channel();
        drop(stream);
        let (send, _failures) = channel();
        // The ring would fill and block a worker that kept waiting: it returns at once.
        std::thread::spawn(move || feed_render(feed, doc, None, send))
            .join()
            .unwrap();
    }

    #[test]
    fn a_failure_of_the_worker_reaches_the_status_line() {
        let text = item_text(&project_of(&[SYNTH]), &["d0"]);
        let (mut c, path) = core_with_item(&text, "atomikpatch");
        c.browse(BrowseAction::Preview(path));
        let (send, failures) = channel();
        c.patch_preview = Some(PatchPreview { failures });
        send.send("a sample file is gone".to_string()).unwrap();
        c.transport_frame(1.0);
        assert!(
            c.last_rejection()
                .is_some_and(|m| m.contains("a sample file is gone"))
        );
        assert!(c.patch_preview.is_none());
    }
}
