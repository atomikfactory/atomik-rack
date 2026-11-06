//! The factory library in the app core: its own root in the browser, the marks of its manifest,
//! read-only items, and (the content test) every shipped patch loads, renders and stays bounded.

use super::browser::ViewCall;
use super::*;
use crate::factory::{Factory, MANIFEST};
use crate::sink::RecordingSink;
use atomik_library::{FileRow, LibrarySection};
use atomik_model::empty_doc;
use atomik_ui::kurbo::Rect;
use atomik_ui::{BrowseAction, Place};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "atomik-app-factory-core-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.0.join(rel)
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// `root/Patches/{Warm.atomikpatch, Cold.atomikpatch}` and a manifest that marks `Warm`.
fn factory_folder(tmp: &Tmp) -> PathBuf {
    let root = tmp.path("Factory");
    write(&root.join("Patches/Warm.atomikpatch"), "{}");
    write(&root.join("Patches/Cold.atomikpatch"), "{}");
    write(
        &root.join(MANIFEST),
        r#"{ "version": 1, "items": { "Patches/Warm.atomikpatch":
            { "author": "Atomik", "categories": ["Pad"], "tags": ["warm"] } } }"#,
    );
    root
}

fn core(tmp: &Tmp) -> AppCore<RecordingSink> {
    let mut c = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    c.file_browser_mut()
        .set_bounds(Rect::new(0.0, 0.0, 300.0, 600.0));
    c.set_library_root(Some(tmp.path("Library")));
    c
}

/// Shows the panel (the first show lists an empty folder, never the home folder).
fn shown(tmp: &Tmp) -> AppCore<RecordingSink> {
    let mut c = core(tmp);
    let empty = tmp.path("Empty");
    std::fs::create_dir_all(&empty).unwrap();
    c.set_file_browser_folder(Some(empty));
    c.set_file_browser_visible(true);
    wait(&mut c);
    c
}

fn wait<S: EngineSink>(c: &mut AppCore<S>) {
    for _ in 0..3_000 {
        c.frame();
        if !c.browser_busy() {
            c.frame();
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("the browser stayed busy");
}

/// The rows the panel holds: the last `SetRows` plus the `AppendRows` after it.
fn rows(c: &AppCore<impl EngineSink>) -> Vec<FileRow> {
    let mut out = Vec::new();
    for call in &c.browser.trace {
        match call {
            ViewCall::SetRows(r) => out.clone_from(r),
            ViewCall::AppendRows(r) => out.extend(r.iter().cloned()),
            _ => {}
        }
    }
    out
}

fn row_named(rows: &[FileRow], name: &str) -> FileRow {
    rows.iter()
        .find(|r| r.name == name)
        .unwrap_or_else(|| panic!("{name} is not listed: {rows:?}"))
        .clone()
}

fn says(c: &AppCore<RecordingSink>, text: &str) {
    let message = c.last_rejection().unwrap_or_default().to_lowercase();
    assert!(message.contains(text), "{text:?}: {message}");
}

#[test]
fn the_factory_is_a_root_of_its_own_with_the_sections_it_has() {
    let tmp = Tmp::new("places");
    let mut c = shown(&tmp);
    assert!(c.factory_root().is_none());
    c.set_factory_root(Some(factory_folder(&tmp)));
    let places = c
        .browser
        .trace
        .iter()
        .rev()
        .find_map(|t| match t {
            ViewCall::SetPlaces(p) => Some(p.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(places.factory, [LibrarySection::Patches]);
    assert_eq!(c.factory_root(), Some(tmp.path("Factory")));
    assert!(c.in_factory(&tmp.path("Factory/Patches/Warm.atomikpatch")));
    assert!(!c.in_factory(&tmp.path("Library/Patches/Warm.atomikpatch")));
    c.set_factory_root(None);
    assert!(c.factory_root().is_none());
}

#[test]
fn a_factory_section_lists_its_items_with_the_marks_of_the_manifest() {
    let tmp = Tmp::new("list");
    let mut c = core(&tmp);
    c.set_factory_root(Some(factory_folder(&tmp)));
    // The user library has a patch of the same name: only the factory one is listed here.
    write(&tmp.path("Library/Patches/Other.atomikpatch"), "{}");
    c.browse(BrowseAction::Show(Place::Factory(LibrarySection::Patches)));
    wait(&mut c);
    let listed = rows(&c);
    let names: Vec<&str> = listed.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names.len(), 2, "{names:?}");
    let warm = row_named(&listed, "Warm.atomikpatch");
    assert_eq!(warm.tags, ["warm"]);
    assert_eq!(warm.categories, ["Pad"]);
    assert_eq!(warm.author, "Atomik");
    let cold = row_named(&listed, "Cold.atomikpatch");
    assert!(cold.tags.is_empty() && cold.categories.is_empty() && cold.author.is_empty());
    // A section the factory has no folder for lists nothing.
    c.browse(BrowseAction::Show(Place::Factory(LibrarySection::Clips)));
    wait(&mut c);
    assert!(rows(&c).is_empty());
}

#[test]
fn favourites_and_user_tags_work_on_factory_items_and_keep_the_factory_marks() {
    let tmp = Tmp::new("marks");
    let mut c = core(&tmp);
    c.set_factory_root(Some(factory_folder(&tmp)));
    c.browse(BrowseAction::Show(Place::Factory(LibrarySection::Patches)));
    wait(&mut c);
    let warm = tmp.path("Factory/Patches/Warm.atomikpatch");
    c.browse(BrowseAction::ToggleFavorite(vec![warm.clone()]));
    let marks = c
        .browser
        .trace
        .iter()
        .rev()
        .find_map(|t| match t {
            ViewCall::PatchMarks(m) => Some(m.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(marks, [(warm.clone(), true, vec!["warm".to_string()])]);
    assert!(c.library_mark(&warm).favorite);
    // The user's own database holds only what the user did.
    assert!(c.library_mark(&warm).tags.is_empty());
    // A fresh listing shows both: the star and the factory tag.
    c.browse(BrowseAction::Show(Place::Factory(LibrarySection::Patches)));
    wait(&mut c);
    let row = row_named(&rows(&c), "Warm.atomikpatch");
    assert!(row.favorite);
    assert_eq!(row.tags, ["warm"]);
}

#[test]
fn factory_items_cannot_be_deleted_or_renamed_and_the_files_stay() {
    let tmp = Tmp::new("readonly");
    let mut c = core(&tmp);
    c.set_factory_root(Some(factory_folder(&tmp)));
    let warm = tmp.path("Factory/Patches/Warm.atomikpatch");
    c.browse(BrowseAction::DeleteItem(warm.clone()));
    says(&c, "factory content");
    assert!(warm.is_file());
    c.browse(BrowseAction::RenameItem {
        path: warm.clone(),
        to: "Hot".into(),
    });
    says(&c, "factory content");
    assert!(warm.is_file());
    assert!(!tmp.path("Factory/Patches/Hot.atomikpatch").exists());
}

#[test]
fn losing_the_factory_while_it_is_shown_falls_back_to_the_library_section() {
    let tmp = Tmp::new("lost");
    let mut c = shown(&tmp);
    c.set_factory_root(Some(factory_folder(&tmp)));
    c.browse(BrowseAction::Show(Place::Factory(LibrarySection::Patches)));
    wait(&mut c);
    c.set_factory_root(None);
    wait(&mut c);
    assert_eq!(
        c.file_browser_mut().location(),
        Some(&Place::Library(LibrarySection::Patches))
    );
}

#[test]
fn an_unreadable_manifest_still_opens_the_folder_without_marks() {
    let tmp = Tmp::new("badmanifest");
    let root = factory_folder(&tmp);
    write(&root.join(MANIFEST), "{ nope");
    assert!(Factory::open(&root).is_err());
    let mut c = core(&tmp);
    c.set_factory_root(Some(root));
    c.browse(BrowseAction::Show(Place::Factory(LibrarySection::Patches)));
    wait(&mut c);
    let warm = row_named(&rows(&c), "Warm.atomikpatch");
    assert!(warm.tags.is_empty() && warm.author.is_empty());
}

// ------------------------------------------------------------------------------ the shipped content

use super::factory_build::kits::{MAX_SECONDS, RATE, key_of, render_hit};
use super::factory_build::{
    AUTHOR, CLIPS, CONTAINERS, ClipDef, ContainerDef, DEFS, Def, KITS, KitDef, clip_notes,
    clip_path, clip_text, container_path, container_text, item_text, manifest_text, marks,
    patch_path, write_all,
};
use super::patch_preview_ops::preview_doc;
use atomik_audio_file::{AudioData, content_hash, read_wav_f32, write_wav_f32};
use atomik_core::PPQ;
use atomik_library::FileKind;
use atomik_model::{
    ClipContent, ItemKind, ModelError, load_clip_file, load_device_item, summarize_item,
};
use atomik_render::default_registry;
use atomik_render::project::{ProjectOptions, render_doc_in};

/// The factory folder of this source checkout.
fn shipped() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../factory")
}

fn patch_file(def: &Def) -> PathBuf {
    shipped().join(patch_path(def.name))
}

fn kit_file(kit: &KitDef) -> PathBuf {
    shipped().join(patch_path(kit.name))
}

fn container_file(def: &ContainerDef) -> PathBuf {
    shipped().join(container_path(def.name))
}

fn clip_file(def: &ClipDef) -> PathBuf {
    shipped().join(clip_path(def.name))
}

/// The names in `dir`, sorted.
fn names_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// What a rendered phrase looks like: peak and RMS in dB, the share of the energy in the
/// first difference (a rough brightness) and whether every sample is a number.
struct Stats {
    peak: f32,
    rms_db: f32,
    brightness: f32,
    finite: bool,
    seconds: f32,
}

fn stats_of(planes: &[Vec<f32>], rate: u32) -> Stats {
    let all = planes.iter().flatten().copied();
    let finite = all.clone().all(f32::is_finite);
    let peak = all.clone().fold(0.0f32, |p, s| p.max(s.abs()));
    let (mut energy, mut diff, mut n) = (0.0f64, 0.0f64, 0usize);
    for plane in planes {
        for pair in plane.windows(2) {
            energy += f64::from(pair[1]).powi(2);
            diff += f64::from(pair[1] - pair[0]).powi(2);
            n += 1;
        }
    }
    let rms = (energy / n.max(1) as f64).sqrt() as f32;
    Stats {
        peak,
        rms_db: 20.0 * rms.max(1e-9).log10(),
        brightness: (diff / energy.max(1e-12)).sqrt() as f32,
        finite,
        seconds: planes.first().map_or(0, Vec::len) as f32 / rate as f32,
    }
}

/// The preview phrase of the item file `path` (what Space plays in the browser), rendered the
/// way the preview does, with the audio of the item beside it when it has some.
fn render_item(path: &Path) -> (Vec<Vec<f32>>, u32, Stats) {
    let registry = default_registry();
    let text = std::fs::read_to_string(path).unwrap();
    let snapshot = load_device_item(&text, &registry).unwrap();
    let doc = preview_doc(&snapshot, &registry).unwrap();
    let root = path.with_extension("");
    let root = root.is_dir().then_some(root);
    let rendered =
        render_doc_in(&doc, &registry, &ProjectOptions::default(), root.as_deref()).unwrap();
    let rate = rendered.sample_rate;
    let planes = rendered.channels;
    let stats = stats_of(&planes, rate);
    (planes, rate, stats)
}

fn render_phrase(def: &Def) -> (Vec<Vec<f32>>, u32, Stats) {
    render_item(&patch_file(def))
}

#[test]
fn every_table_row_has_its_file_and_a_manifest_entry_and_the_folders_hold_nothing_else() {
    let unique = |names: Vec<&str>| {
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "names are unique");
    };
    unique(
        DEFS.iter()
            .map(|d| d.name)
            .chain(KITS.iter().map(|k| k.name))
            .collect(),
    );
    unique(CONTAINERS.iter().map(|c| c.name).collect());
    unique(CLIPS.iter().map(|c| c.name).collect());
    assert!((20..=40).contains(&DEFS.len()), "{}", DEFS.len());
    assert_eq!(KITS.len(), 4);
    assert_eq!(CLIPS.len(), 12);
    let chains = CONTAINERS.iter().filter(|c| c.chain).count();
    assert_eq!(chains, 10, "effect chains");
    assert_eq!(CONTAINERS.len() - chains, 3, "modular containers");
    // Patches: one file per patch and kit, and a folder per kit that holds only its audio.
    let mut patches: Vec<String> = DEFS
        .iter()
        .map(|d| format!("{}.atomikpatch", d.name))
        .chain(KITS.iter().map(|k| format!("{}.atomikpatch", k.name)))
        .chain(KITS.iter().map(|k| k.name.to_owned()))
        .collect();
    patches.sort();
    assert_eq!(names_in(&shipped().join("Patches")), patches);
    for kit in KITS {
        let folder = shipped().join("Patches").join(kit.name);
        assert_eq!(names_in(&folder), ["Audio"], "{}", kit.name);
        let text = std::fs::read_to_string(kit_file(kit)).unwrap();
        let snapshot = load_device_item(&text, &default_registry()).unwrap();
        let mut wanted: Vec<String> = snapshot.assets.keys().map(|h| format!("{h}.wav")).collect();
        wanted.sort();
        assert_eq!(names_in(&folder.join("Audio")), wanted, "{}", kit.name);
    }
    let mut containers: Vec<String> = CONTAINERS
        .iter()
        .map(|c| format!("{}.atomikcontainer", c.name))
        .collect();
    containers.sort();
    assert_eq!(names_in(&shipped().join("Containers")), containers);
    let mut clips: Vec<String> = CLIPS
        .iter()
        .map(|c| format!("{}.atomikclip", c.name))
        .collect();
    clips.sort();
    assert_eq!(names_in(&shipped().join("Clips")), clips);
    // The manifest on disk is the one the tables give, and marks every item.
    let manifest = std::fs::read_to_string(shipped().join(MANIFEST)).unwrap();
    assert_eq!(manifest, manifest_text(), "run write_the_factory_library");
    let factory = Factory::open(&shipped()).unwrap();
    assert_eq!(
        factory.sections(),
        [
            LibrarySection::Patches,
            LibrarySection::Containers,
            LibrarySection::Clips
        ]
    );
    let marks = marks();
    assert_eq!(
        marks.len(),
        DEFS.len() + KITS.len() + CONTAINERS.len() + CLIPS.len()
    );
    for mark in marks {
        let (mut tags, mut categories, mut author) = (Vec::new(), Vec::new(), String::new());
        factory.merge(
            &shipped().join(&mark.path),
            &mut tags,
            &mut categories,
            &mut author,
        );
        assert_eq!(categories, [mark.category], "{}", mark.path);
        assert_eq!(author, AUTHOR);
        assert!(tags.len() >= 3, "{}: {tags:?}", mark.path);
    }
    for def in DEFS {
        assert!(!def.sound.is_empty());
    }
    for kit in KITS {
        assert!(!kit.sound.is_empty());
    }
    for def in CONTAINERS {
        assert!(!def.sound.is_empty());
    }
    for def in CLIPS {
        assert!(!def.sound.is_empty());
    }
}

#[test]
fn every_factory_patch_is_a_single_device_item_for_its_device_and_matches_its_table() {
    let registry = default_registry();
    for def in DEFS {
        let text = std::fs::read_to_string(patch_file(def)).unwrap();
        let snapshot = load_device_item(&text, &registry).unwrap();
        assert_eq!(ItemKind::of(&snapshot), ItemKind::Patch, "{}", def.name);
        let device = snapshot.single_device().unwrap();
        assert_eq!(device.type_id, def.device, "{}", def.name);
        assert_eq!(device.patch_name, def.name);
        assert!(snapshot.assets.is_empty(), "{} needs no audio", def.name);
        for &(param, value) in def.params {
            let stored = device.params.get(param).copied();
            assert_eq!(stored, Some(value), "{}: {param}", def.name);
        }
        let slots = device.mods.iter().filter(|m| m.is_some()).count();
        assert_eq!(slots, def.mods.len(), "{}: modulation slots", def.name);
        // The file is what the table writes (up to the format version of the day).
        assert_eq!(
            summary_of(&text),
            summary_of(&item_text(def)),
            "{}: run write_the_factory_library",
            def.name
        );
    }
}

/// The device list of an item text, as JSON (the part the table decides).
fn summary_of(text: &str) -> serde_json::Value {
    let v: serde_json::Value = serde_json::from_str(text).unwrap();
    v["devices"].clone()
}

#[test]
fn every_factory_patch_plays_a_phrase_with_audible_bounded_sound() {
    for def in DEFS {
        let (_, _, s) = render_phrase(def);
        assert!(s.finite, "{}: not a number somewhere", def.name);
        assert!(s.peak > 0.05, "{}: peak {} is too quiet", def.name, s.peak);
        assert!(s.peak <= 1.0, "{}: peak {} clips", def.name, s.peak);
        assert!(s.rms_db > -45.0, "{}: {} dB RMS", def.name, s.rms_db);
        assert!(s.seconds > 3.0, "{}: {} s", def.name, s.seconds);
    }
}

// ----------------------------------------------------------------------------------- containers

#[test]
fn every_factory_container_is_one_container_with_its_parts_cables_and_macros() {
    let registry = default_registry();
    for def in CONTAINERS {
        let text = std::fs::read_to_string(container_file(def)).unwrap();
        let snapshot = load_device_item(&text, &registry).unwrap();
        assert_eq!(ItemKind::of(&snapshot), ItemKind::Container, "{}", def.name);
        let container = snapshot.single_device().unwrap();
        assert_eq!(container.name, def.name);
        assert_eq!(container.patch_name, def.name);
        let inner = container.inner.as_ref().unwrap();
        let types: Vec<&str> = inner.devices.iter().map(|d| d.type_id.as_str()).collect();
        let wanted: Vec<&str> = def.parts.iter().map(|p| p.device).collect();
        assert_eq!(types, wanted, "{}", def.name);
        let keys: Vec<&str> = inner.devices.iter().map(|d| d.key.0.as_str()).collect();
        let wanted: Vec<&str> = def.parts.iter().map(|p| p.key).collect();
        assert_eq!(keys, wanted, "{}", def.name);
        for (device, part) in inner.devices.iter().zip(def.parts) {
            for &(param, value) in part.params {
                assert_eq!(
                    device.params.get(param).copied(),
                    Some(value),
                    "{}: {}.{param}",
                    def.name,
                    part.key
                );
            }
        }
        // A chain has a cable pair into, between and out of its stages; the others say theirs.
        let cables = if def.chain {
            2 * (def.parts.len() + 1)
        } else {
            def.wires.len()
        };
        assert_eq!(inner.cables.len(), cables, "{}", def.name);
        let named = inner.macros.iter().filter(|m| !m.name.is_empty()).count();
        assert_eq!(named, def.macros.len(), "{}", def.name);
        assert!(!def.macros.is_empty(), "{}", def.name);
        for (m, wanted) in inner.macros.iter().zip(def.macros) {
            assert_eq!(m.name, wanted.name);
            assert_eq!(m.targets.len(), wanted.targets.len(), "{}", def.name);
        }
        assert_eq!(
            summarize_item(ItemKind::Container, &text).unwrap().detail,
            format!("Container, {} inside", def.parts.len())
        );
        assert_eq!(
            summary_of(&text),
            summary_of(&container_text(def)),
            "{}: run write_the_factory_library",
            def.name
        );
    }
}

#[test]
fn every_factory_container_plays_audible_bounded_sound() {
    for def in CONTAINERS {
        let (_, _, s) = render_item(&container_file(def));
        assert!(s.finite, "{}: not a number somewhere", def.name);
        assert!(s.peak > 0.05, "{}: peak {} is too quiet", def.name, s.peak);
        assert!(s.peak <= 1.0, "{}: peak {} clips", def.name, s.peak);
        assert!(s.rms_db > -45.0, "{}: {} dB RMS", def.name, s.rms_db);
        assert!(s.seconds > 3.0, "{}: {} s", def.name, s.seconds);
    }
}

#[test]
fn the_macros_of_a_factory_container_change_the_sound() {
    // Moving the first macro of an effect chain from one end to the other must change what
    // comes out: a macro that drives nothing audible would be a dead knob.
    let registry = default_registry();
    for def in CONTAINERS.iter().filter(|c| c.chain) {
        let text = std::fs::read_to_string(container_file(def)).unwrap();
        let snapshot = load_device_item(&text, &registry).unwrap();
        let render = |value: f32| {
            let mut snapshot = snapshot.clone();
            let container = &mut snapshot.devices[0];
            container.params.insert("macro1".to_owned(), value);
            let doc = preview_doc(&snapshot, &registry).unwrap();
            render_doc_in(&doc, &registry, &ProjectOptions::default(), None)
                .unwrap()
                .channels
        };
        let (low, high) = (render(0.0), render(1.0));
        let frames = low[0].len().min(high[0].len());
        let diff: f32 = (0..frames)
            .map(|i| (low[0][i] - high[0][i]).abs())
            .fold(0.0, f32::max);
        assert!(diff > 1.0e-3, "{}: macro 1 changes nothing", def.name);
    }
}

// ------------------------------------------------------------------------------------- drum kits

#[test]
fn every_kit_is_a_drum_machine_with_a_zone_per_pad_and_its_audio_beside_it() {
    let registry = default_registry();
    for kit in KITS {
        let text = std::fs::read_to_string(kit_file(kit)).unwrap();
        let snapshot = load_device_item(&text, &registry).unwrap();
        assert_eq!(ItemKind::of(&snapshot), ItemKind::Patch, "{}", kit.name);
        let device = snapshot.single_device().unwrap();
        assert_eq!(device.type_id, "atomik.drum-machine", "{}", kit.name);
        assert_eq!(device.patch_name, kit.name);
        assert_eq!(device.zones.len(), kit.hits.len(), "{}", kit.name);
        assert_eq!(snapshot.assets.len(), kit.hits.len(), "{}", kit.name);
        let audio = shipped().join("Patches").join(kit.name).join("Audio");
        for (hit, zone) in kit.hits.iter().zip(&device.zones) {
            let what = format!("{}: {}", kit.name, hit.name);
            let key = key_of(hit.pad);
            assert_eq!(
                (zone.key_lo, zone.key_hi, zone.root_key),
                (key, key, key),
                "{what}"
            );
            let meta = &snapshot.assets[&zone.asset];
            assert_eq!(meta.original_name, hit.name);
            assert_eq!((meta.channels, meta.sample_rate), (1, RATE), "{what}: mono");
            assert!(
                meta.frames as f64 <= MAX_SECONDS * f64::from(RATE),
                "{what}: short"
            );
            assert!(meta.frames > 480, "{what}: not a click");
            assert_eq!(zone.end, meta.frames, "{what}: the whole sample");
            let pad = |name: &str| {
                device
                    .params
                    .get(&format!("pad{}.{name}", hit.pad))
                    .copied()
            };
            assert_eq!(pad("level"), Some(hit.level), "{what}");
            assert_eq!(pad("pan"), Some(hit.pan), "{what}");
            assert_eq!(pad("choke"), Some(f32::from(hit.choke)), "{what}");
            // The file is the audio the pool entry names.
            let data = read_wav_f32(&audio.join(format!("{}.wav", zone.asset))).unwrap();
            assert_eq!(
                content_hash(&data),
                zone.asset,
                "{what}: the id is the content"
            );
            assert_eq!(data.channels.len(), 1, "{what}");
            assert_eq!(data.frames() as u64, meta.frames, "{what}");
            let peak = data.channels[0].iter().fold(0.0f32, |p, s| p.max(s.abs()));
            assert!((0.85..=0.95).contains(&peak), "{what}: peak {peak}");
            assert!(data.channels[0].iter().all(|s| s.is_finite()), "{what}");
        }
    }
}

#[test]
fn every_kit_plays_a_drum_pattern_with_audible_bounded_sound() {
    for kit in KITS {
        let (_, _, s) = render_item(&kit_file(kit));
        assert!(s.finite, "{}: not a number somewhere", kit.name);
        assert!(s.peak > 0.2, "{}: peak {} is too quiet", kit.name, s.peak);
        assert!(s.peak <= 1.0, "{}: peak {} clips", kit.name, s.peak);
        assert!(s.rms_db > -40.0, "{}: {} dB RMS", kit.name, s.rms_db);
    }
}

#[test]
fn every_pad_of_a_kit_sounds_alone() {
    // Each pad's own hit, struck once through the kit item with its audio: nothing is silent
    // and nothing clips (the pad level and pan are part of the kit).
    let registry = default_registry();
    for kit in KITS {
        let text = std::fs::read_to_string(kit_file(kit)).unwrap();
        let snapshot = load_device_item(&text, &registry).unwrap();
        let root = shipped().join("Patches").join(kit.name);
        for hit in kit.hits {
            let mut one = snapshot.clone();
            // The preview's track plays the pad keys of its phrase; a copy of the kit with only
            // this pad's zone makes every other note silent.
            let key = key_of(hit.pad);
            one.devices[0].zones.retain(|z| z.key_lo == key);
            let doc = preview_doc(&one, &registry).unwrap();
            let rendered =
                render_doc_in(&doc, &registry, &ProjectOptions::default(), Some(&root)).unwrap();
            let s = stats_of(&rendered.channels, rendered.sample_rate);
            assert!(
                s.finite && s.peak > 0.02,
                "{}: {}: {}",
                kit.name,
                hit.name,
                s.peak
            );
            assert!(s.peak <= 1.0, "{}: {}: {}", kit.name, hit.name, s.peak);
        }
    }
}

#[test]
fn a_hit_renders_to_a_short_mono_levelled_sample_on_any_platform() {
    let kit = &KITS[0];
    for hit in kit.hits.iter().take(3) {
        let data = render_hit(kit, hit);
        assert_eq!(data.sample_rate, RATE);
        assert_eq!(data.channels.len(), 1);
        let peak = data.channels[0].iter().fold(0.0f32, |p, s| p.max(s.abs()));
        assert!((peak - 0.9).abs() < 1.0e-4, "{}: {peak}", hit.name);
        assert!(data.frames() as f64 <= MAX_SECONDS * f64::from(RATE));
        let tail = data.channels[0][data.frames() - 1];
        assert!(tail.abs() < 0.01, "{}: the end is faded out", hit.name);
    }
}

#[test]
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
fn the_kits_are_the_exact_bytes_the_generator_renders() {
    use super::factory_build::build_kit;
    // The renders are deterministic: writing the library again gives the same item text and
    // the same audio (compared exactly on the platform the library was written on, since the
    // maths of another platform may differ in the last bit of a sample).
    for kit in KITS {
        let built = build_kit(kit);
        let on_disk = std::fs::read_to_string(kit_file(kit)).unwrap();
        assert_eq!(
            on_disk, built.text,
            "{}: run write_the_factory_library",
            kit.name
        );
        let audio = shipped().join("Patches").join(kit.name).join("Audio");
        for (hash, data) in &built.audio {
            let stored = read_wav_f32(&audio.join(format!("{hash}.wav"))).unwrap();
            assert_eq!(&stored, data, "{}", kit.name);
        }
    }
}

#[test]
fn a_kit_item_of_any_audio_keeps_its_shape() {
    // The item writer around the samples is pure: the same samples give the same text, and a
    // zone names the audio it plays.
    let kit = &KITS[1];
    let samples = |level: f32| -> Vec<AudioData> {
        (0..kit.hits.len())
            .map(|i| {
                let wave: Vec<f32> = (0..2_000)
                    .map(|n| level * ((n + i * 7) as f32 * 0.05).sin())
                    .collect();
                AudioData::new(RATE, vec![wave]).unwrap()
            })
            .collect()
    };
    let a = super::factory_build::kit_item(kit, samples(0.5));
    let b = super::factory_build::kit_item(kit, samples(0.5));
    assert_eq!(a.text, b.text);
    assert_eq!(a.audio.len(), kit.hits.len());
    let c = super::factory_build::kit_item(kit, samples(0.25));
    assert_ne!(a.text, c.text, "other audio is another asset");
}

// ----------------------------------------------------------------------------------------- clips

#[test]
fn every_factory_clip_is_a_note_clip_of_its_table_and_plays_audible_bounded_sound() {
    for def in CLIPS {
        let text = std::fs::read_to_string(clip_file(def)).unwrap();
        let file = load_clip_file(&text).unwrap();
        assert_eq!(file.clip.name.as_deref(), Some(def.name));
        assert_eq!(file.tempo_bpm, def.bpm, "{}", def.name);
        assert_eq!(file.meter, (4, 4));
        // The file is what the table writes (the writer is pure: on every platform alike).
        assert_eq!(
            text,
            clip_text(def),
            "{}: run write_the_factory_library",
            def.name
        );
        let ClipContent::Notes(notes) = &file.clip.content else {
            panic!("{}: not a note clip", def.name);
        };
        assert_eq!(notes.to_vec(), clip_notes(def), "{}", def.name);
        assert!(notes.len() >= 8, "{}: {} notes", def.name, notes.len());
        assert_eq!(file.clip.length, def.bars * 4 * PPQ, "{}", def.name);
        for n in notes.iter() {
            assert!(
                n.tick >= 0 && n.tick + n.length <= file.clip.length,
                "{}",
                def.name
            );
            assert!((24..=96).contains(&n.key), "{}: key {}", def.name, n.key);
            assert!(n.velocity > 0.0 && n.velocity <= 1.0);
        }
        let summary = summarize_item(ItemKind::Clip, &text).unwrap().detail;
        assert_eq!(
            summary,
            format!(
                "{} {}, {} notes",
                def.bars,
                if def.bars == 1 { "bar" } else { "bars" },
                notes.len()
            ),
            "{}",
            def.name
        );
        // The built-in voice of the clip preview plays it: audible and inside full scale.
        let timed = super::clip_preview_ops::timed_notes(&file.clip, file.tempo_bpm);
        assert_eq!(
            timed.len(),
            notes.len(),
            "{}: every note is inside the clip",
            def.name
        );
        let mono = super::clip_preview_ops::render_notes(&timed);
        let s = stats_of(&[mono], 44_100);
        assert!(
            s.finite && s.peak > 0.05 && s.peak <= 1.0,
            "{}: {}",
            def.name,
            s.peak
        );
    }
}

#[test]
fn the_drum_clips_play_the_pad_keys_of_the_kits() {
    for def in CLIPS.iter().filter(|c| c.category == "Drums") {
        for n in clip_notes(def) {
            assert!(
                [36, 37, 38, 39, 41, 42, 43, 45, 46].contains(&n.key),
                "{}: key {} is not a pad of the kits",
                def.name,
                n.key
            );
        }
    }
}

#[test]
fn a_clip_whose_pattern_does_not_fill_it_stops_the_writer() {
    let broken = ClipDef {
        name: "Short Lane",
        category: "Drums",
        tags: &[],
        sound: "",
        bpm: 120.0,
        bars: 1,
        pattern: super::factory_build::clips::Pattern::Drums {
            step: 0.25,
            lanes: &[(36, "X...")],
        },
    };
    let caught = std::panic::catch_unwind(|| clip_notes(&broken));
    assert!(caught.is_err());
}

#[test]
fn a_clip_file_that_is_not_an_atomik_clip_is_refused_by_the_reader_the_writer_uses() {
    let result: Result<_, ModelError> = load_clip_file("{}");
    assert!(result.is_err());
}

// ------------------------------------------------------------------------------------- the folder

#[test]
fn the_factory_folder_stays_small() {
    let total: u64 = walk_sizes(&shipped());
    assert!(total < 2 * 1024 * 1024, "{total} bytes");
    // The audio of the kits is nearly all of it.
    let audio: u64 = KITS
        .iter()
        .map(|k| walk_sizes(&shipped().join("Patches").join(k.name)))
        .sum();
    assert!(audio < total, "{audio} of {total} bytes");
    assert!(
        total - audio < 512 * 1024,
        "{} bytes of items",
        total - audio
    );
}

fn walk_sizes(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| {
            let e = e.unwrap();
            let meta = e.metadata().unwrap();
            if meta.is_dir() {
                walk_sizes(&e.path())
            } else {
                meta.len()
            }
        })
        .sum()
}

/// The file names of a section and the category each is marked with.
type Listed = Vec<(String, &'static str)>;

#[test]
fn the_factory_items_list_from_the_shipped_folder_with_their_categories() {
    let tmp = Tmp::new("shipped");
    let mut c = core(&tmp);
    c.set_factory_root(Some(shipped()));
    let sections: [(LibrarySection, Listed, FileKind); 3] = [
        (
            LibrarySection::Patches,
            DEFS.iter()
                .map(|d| (format!("{}.atomikpatch", d.name), d.category))
                .chain(
                    KITS.iter()
                        .map(|k| (format!("{}.atomikpatch", k.name), k.category)),
                )
                .collect(),
            FileKind::Patch,
        ),
        (
            LibrarySection::Containers,
            CONTAINERS
                .iter()
                .map(|d| (format!("{}.atomikcontainer", d.name), d.category))
                .collect(),
            FileKind::Container,
        ),
        (
            LibrarySection::Clips,
            CLIPS
                .iter()
                .map(|d| (format!("{}.atomikclip", d.name), d.category))
                .collect(),
            FileKind::Clip,
        ),
    ];
    for (section, wanted, kind) in sections {
        c.browse(BrowseAction::Show(Place::Factory(section)));
        wait(&mut c);
        let listed = rows(&c);
        // The audio of a kit is no row of its own.
        assert_eq!(listed.len(), wanted.len(), "{section:?}: {listed:?}");
        for (name, category) in wanted {
            let row = row_named(&listed, &name);
            assert_eq!(row.categories, [category], "{name}");
            assert_eq!(row.author, AUTHOR);
            assert_eq!(row.kind, kind, "{name}");
        }
    }
}

#[test]
fn loading_a_factory_kit_brings_its_audio_into_the_project() {
    let tmp = Tmp::new("kit-load");
    let mut c = core(&tmp);
    let kit = &KITS[0];
    c.load_library_item(&kit_file(kit), LoadTarget::Default);
    c.wait_background();
    assert!(c.missing_assets().is_empty(), "{:?}", c.missing_assets());
    let drum = c
        .doc()
        .devices
        .iter()
        .find(|d| d.type_id == "atomik.drum-machine")
        .expect("the kit's drum machine");
    assert_eq!(drum.zones.len(), kit.hits.len());
    for zone in &drum.zones {
        assert!(c.doc().assets.contains_key(&zone.asset), "the pool entry");
        assert!(
            c.song_assets().contains_key(&zone.asset),
            "loaded for the engine"
        );
    }
    // The factory folder itself is untouched by loading from it.
    assert!(
        kit_file(kit).is_file()
            && shipped()
                .join("Patches")
                .join(kit.name)
                .join("Audio")
                .is_dir()
    );
}

#[test]
fn loading_a_factory_container_adds_it_with_its_inside() {
    let tmp = Tmp::new("container-load");
    let mut c = core(&tmp);
    let def = CONTAINERS.iter().find(|d| !d.chain).unwrap();
    c.load_library_item(&container_file(def), LoadTarget::Default);
    c.wait_background();
    let container = c
        .doc()
        .devices
        .iter()
        .find(|d| d.type_id == atomik_model::CONTAINER_TYPE)
        .expect("the container");
    assert_eq!(
        container.inner.as_ref().unwrap().devices.len(),
        def.parts.len()
    );
    assert_eq!(container.name, def.name);
}

/// Rewrites the shipped files from the tables (see `app_core_factory_build.rs`).
#[test]
#[ignore = "writes the factory folder: ATOMIK_WRITE_FACTORY=1"]
fn write_the_factory_library() {
    assert!(
        std::env::var_os("ATOMIK_WRITE_FACTORY").is_some(),
        "set ATOMIK_WRITE_FACTORY=1 to rewrite the factory folder"
    );
    write_all(&shipped());
}

/// Renders the preview phrase of every factory patch, kit and container into
/// `ATOMIK_FACTORY_RENDER_DIR` as `<name>.wav`, with a `report.txt` of the levels: the way to
/// listen to the library.
#[test]
#[ignore = "writes files: ATOMIK_FACTORY_RENDER_DIR=<folder>"]
fn render_the_factory_library_to_wav() {
    let dir = PathBuf::from(
        std::env::var_os("ATOMIK_FACTORY_RENDER_DIR")
            .expect("set ATOMIK_FACTORY_RENDER_DIR to a folder"),
    );
    std::fs::create_dir_all(&dir).unwrap();
    let mut report = String::new();
    let mut items: Vec<(&str, &str, PathBuf)> = Vec::new();
    for def in DEFS {
        items.push((def.name, def.device, patch_file(def)));
    }
    for kit in KITS {
        items.push((kit.name, "drum kit", kit_file(kit)));
    }
    for def in CONTAINERS {
        items.push((def.name, "container", container_file(def)));
    }
    for (name, what, path) in items {
        let (planes, rate, s) = render_item(&path);
        let data = AudioData::new(rate, planes).unwrap();
        write_wav_f32(&dir.join(format!("{name}.wav")), &data).unwrap();
        report.push_str(&format!(
            "{:<20} {:<18} peak {:6.3} rms {:6.1} dB bright {:5.3} {:4.1} s\n",
            name, what, s.peak, s.rms_db, s.brightness, s.seconds
        ));
    }
    // And every sample of every kit, with the same figures.
    for kit in KITS {
        for hit in kit.hits {
            let data = render_hit(kit, hit);
            write_wav_f32(&dir.join(format!("{}.wav", hit.name)), &data).unwrap();
            let s = stats_of(&data.channels, data.sample_rate);
            report.push_str(&format!(
                "{:<20} {:<18} peak {:6.3} rms {:6.1} dB bright {:5.3} {:4.2} s\n",
                hit.name, "kit sample", s.peak, s.rms_db, s.brightness, s.seconds
            ));
        }
    }
    std::fs::write(dir.join("report.txt"), report).unwrap();
}

#[test]
fn every_factory_patch_applies_in_place_to_a_device_of_its_type() {
    let registry = default_registry();
    for def in DEFS {
        let mut doc = Document::new(empty_doc());
        let key = DeviceKey("d".to_string());
        doc.apply(
            Command::AddDevice {
                index: usize::MAX,
                type_id: def.device.to_string(),
                key: Some(key.clone()),
            },
            &registry,
        )
        .unwrap();
        let text = std::fs::read_to_string(patch_file(def)).unwrap();
        let snapshot = load_device_item(&text, &registry).unwrap();
        doc.apply(
            Command::Workflow(WorkflowCommand::ApplyPatch {
                device: key.clone(),
                snapshot,
            }),
            &registry,
        )
        .unwrap_or_else(|e| panic!("{}: {e}", def.name));
        let device = doc.doc().resolve(&key).unwrap();
        assert_eq!(device.patch_name, def.name);
        for &(param, value) in def.params {
            assert_eq!(
                device.params.get(param).copied(),
                Some(value),
                "{}",
                def.name
            );
        }
    }
}
