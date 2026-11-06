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

// ------------------------------------------------------------------------ the shipped content

use super::factory_build::{AUTHOR, DEFS, Def, item_text, manifest_text, write_all};
use super::patch_preview_ops::preview_doc;
use atomik_audio_file::{AudioData, write_wav_f32};
use atomik_model::{ItemKind, load_device_item};
use atomik_render::default_registry;
use atomik_render::project::{ProjectOptions, render_doc_in};

/// The factory folder of this source checkout.
fn shipped() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../factory")
}

fn patch_file(def: &Def) -> PathBuf {
    shipped()
        .join("Patches")
        .join(format!("{}.atomikpatch", def.name))
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

fn render_phrase(def: &Def) -> (Vec<Vec<f32>>, u32, Stats) {
    let registry = default_registry();
    let text = std::fs::read_to_string(patch_file(def)).unwrap();
    let snapshot = load_device_item(&text, &registry).unwrap();
    let doc = preview_doc(&snapshot, &registry).unwrap();
    let rendered = render_doc_in(&doc, &registry, &ProjectOptions::default(), None).unwrap();
    let rate = rendered.sample_rate;
    let planes = rendered.channels;
    let all = planes.iter().flatten().copied();
    let finite = all.clone().all(f32::is_finite);
    let peak = all.clone().fold(0.0f32, |p, s| p.max(s.abs()));
    let (mut energy, mut diff, mut n) = (0.0f64, 0.0f64, 0usize);
    for plane in &planes {
        for pair in plane.windows(2) {
            energy += f64::from(pair[1]).powi(2);
            diff += f64::from(pair[1] - pair[0]).powi(2);
            n += 1;
        }
    }
    let rms = (energy / n.max(1) as f64).sqrt() as f32;
    let stats = Stats {
        peak,
        rms_db: 20.0 * rms.max(1e-9).log10(),
        brightness: (diff / energy.max(1e-12)).sqrt() as f32,
        finite,
        seconds: planes.first().map_or(0, Vec::len) as f32 / rate as f32,
    };
    (planes, rate, stats)
}

#[test]
fn every_definition_has_its_file_and_a_manifest_entry_and_the_folder_holds_nothing_else() {
    let mut names: Vec<&str> = DEFS.iter().map(|d| d.name).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), DEFS.len(), "patch names are unique");
    assert!((20..=32).contains(&DEFS.len()), "{}", DEFS.len());
    let mut on_disk: Vec<String> = std::fs::read_dir(shipped().join("Patches"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    on_disk.sort();
    let mut wanted: Vec<String> = DEFS
        .iter()
        .map(|d| format!("{}.atomikpatch", d.name))
        .collect();
    wanted.sort();
    assert_eq!(on_disk, wanted);
    // The manifest on disk is the one the tables give, and marks every item.
    let manifest = std::fs::read_to_string(shipped().join(MANIFEST)).unwrap();
    assert_eq!(manifest, manifest_text(), "run write_the_factory_library");
    let factory = Factory::open(&shipped()).unwrap();
    assert_eq!(factory.sections(), [LibrarySection::Patches]);
    for def in DEFS {
        let (mut tags, mut categories, mut author) = (Vec::new(), Vec::new(), String::new());
        factory.merge(&patch_file(def), &mut tags, &mut categories, &mut author);
        assert_eq!(categories, [def.category], "{}", def.name);
        assert_eq!(author, AUTHOR);
        assert!(tags.len() >= 3, "{}: {tags:?}", def.name);
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

#[test]
fn the_factory_folder_stays_small() {
    let total: u64 = walk_sizes(&shipped());
    assert!(total < 256 * 1024, "{total} bytes");
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

#[test]
fn the_factory_patches_list_from_the_shipped_folder_with_their_categories() {
    let tmp = Tmp::new("shipped");
    let mut c = core(&tmp);
    c.set_factory_root(Some(shipped()));
    c.browse(BrowseAction::Show(Place::Factory(LibrarySection::Patches)));
    wait(&mut c);
    let listed = rows(&c);
    assert_eq!(listed.len(), DEFS.len());
    for def in DEFS {
        let row = row_named(&listed, &format!("{}.atomikpatch", def.name));
        assert_eq!(row.categories, [def.category], "{}", def.name);
        assert_eq!(row.author, AUTHOR);
    }
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

/// Renders the preview phrase of every factory patch into `ATOMIK_FACTORY_RENDER_DIR` as
/// `<name>.wav`, with a `report.txt` of the levels: the way to listen to the library.
#[test]
#[ignore = "writes files: ATOMIK_FACTORY_RENDER_DIR=<folder>"]
fn render_the_factory_library_to_wav() {
    let dir = PathBuf::from(
        std::env::var_os("ATOMIK_FACTORY_RENDER_DIR")
            .expect("set ATOMIK_FACTORY_RENDER_DIR to a folder"),
    );
    std::fs::create_dir_all(&dir).unwrap();
    let mut report = String::new();
    for def in DEFS {
        let (planes, rate, s) = render_phrase(def);
        let data = AudioData::new(rate, planes).unwrap();
        write_wav_f32(&dir.join(format!("{}.wav", def.name)), &data).unwrap();
        report.push_str(&format!(
            "{:<20} {:<18} peak {:6.3} rms {:6.1} dB bright {:5.3} {:4.1} s\n",
            def.name, def.device, s.peak, s.rms_db, s.brightness, s.seconds
        ));
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
