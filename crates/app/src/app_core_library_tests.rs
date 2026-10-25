//! The library in the app core. Save: the selected devices become a patch or a container item,
//! a selected note clip a clip item, in the library folder, with the audio copied beside the
//! item by a worker; saving changes neither the document nor the undo history. Load: items
//! come back as one undo step, with their audio. Every test uses its own temporary library
//! folder.

use super::*;
use crate::sink::RecordingSink;
use atomik_audio_file::{AudioData, Bundle, write_wav_f32};
use atomik_core::PPQ;
use atomik_library::{FileKind, LibrarySection, device_uri};
use atomik_model::{
    Clip, ClipContent, ClipId, Command, ContainerCommand, DeviceKey, MeterEvent, Note, RackSlot,
    SongCommand, TempoEvent, ZoneDoc, empty_doc, load_clip_file, load_device_item, save_project,
};
use atomik_ui::{BrowseAction, Place, TransportAction};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

const SYNTH: &str = "atomik.poly-synth";
const DELAY: &str = "atomik.stereo-delay";
const BAR: i64 = 4 * PPQ;

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "atomik-app-library-{tag}-{}-{n}",
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

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

/// A core whose library folder is `<tmp>/Library` (not created: the first save makes it).
fn core(lib: &Tmp) -> AppCore<RecordingSink> {
    let mut c = AppCore::with_registry(RecordingSink::new(), default_registry(), empty_doc(), None);
    c.view_mut().set_show_console_devices(false);
    c.set_library_root(Some(lib.path("Library")));
    c
}

fn add(c: &mut AppCore<RecordingSink>, type_id: &str, k: &str) {
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: type_id.to_string(),
        key: Some(key(k)),
    })
    .unwrap();
}

fn rename(c: &mut AppCore<RecordingSink>, k: &str, name: &str) {
    c.apply_command(Command::Rename {
        key: key(k),
        name: name.to_string(),
    })
    .unwrap();
}

fn select(c: &mut AppCore<RecordingSink>, ks: &[&str]) {
    let ks: Vec<DeviceKey> = ks.iter().map(|k| key(k)).collect();
    c.after_view_change();
    c.view_mut().set_selection(&ks, None);
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
    panic!("the library worker stayed busy");
}

fn files_in(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

fn note_clip(start: i64) -> Clip {
    Clip {
        id: ClipId(0),
        start,
        length: BAR,
        offset: 0,
        loop_length: None,
        muted: false,
        scale: None,
        controllers: Vec::new(),
        grid: None,
        content: ClipContent::Notes(vec![
            Note::new(0, PPQ / 2, 60, 0.8),
            Note::new(PPQ, PPQ / 2, 64, 0.6),
        ]),
    }
}

/// Synth `a` named "Bass" with a track and a note clip at bar 5; the clip is selected.
fn with_clip(lib: &Tmp) -> AppCore<RecordingSink> {
    let mut c = core(lib);
    add(&mut c, SYNTH, "a");
    rename(&mut c, "a", "Bass");
    c.apply_command(Command::Song(SongCommand::AddTrack { device: key("a") }))
        .unwrap();
    c.apply_command(Command::Song(SongCommand::AddClip {
        device: key("a"),
        lane: 0,
        clip: note_clip(4 * BAR),
    }))
    .unwrap();
    c.after_view_change();
    let id = c.doc().song.tracks[0].lanes[0].clips[0].id;
    c.arrangement_mut().set_selected_clips(&[id]);
    c
}

// ------------------------------------------------------------------ devices

#[test]
fn save_selection_to_library_one_device_writes_a_patch_named_after_it() {
    let lib = Tmp::new("one");
    let mut c = core(&lib);
    add(&mut c, SYNTH, "a");
    rename(&mut c, "a", "Warm Pad");
    select(&mut c, &["a"]);
    c.save_selection_to_library();
    assert!(c.last_rejection().unwrap().contains("Saved 'Warm Pad'"));
    let dir = lib.path("Library/Patches");
    assert_eq!(files_in(&dir), ["Warm Pad.atomikpatch"]);
    let text = std::fs::read_to_string(dir.join("Warm Pad.atomikpatch")).unwrap();
    let item = load_device_item(&text, c.registry()).unwrap();
    assert_eq!(item.devices.len(), 1);
    assert_eq!(item.devices[0].type_id, SYNTH);
    assert_eq!(item.devices[0].name, "Warm Pad");
    assert_eq!(
        atomik_model::ItemKind::of(&item),
        atomik_model::ItemKind::Patch
    );
}

#[test]
fn save_selection_to_library_changes_neither_the_document_nor_the_history() {
    let lib = Tmp::new("pure");
    let mut c = core(&lib);
    add(&mut c, SYNTH, "a");
    add(&mut c, DELAY, "b");
    select(&mut c, &["a", "b"]);
    let before = save_project(c.doc());
    c.save_selection_to_library();
    assert_eq!(save_project(c.doc()), before);
    assert!(c.undo(), "the last step is still the device I added");
    assert!(c.doc().devices.iter().all(|d| d.key.0 != "b"));
}

#[test]
fn save_selection_to_library_a_single_container_goes_to_containers() {
    let lib = Tmp::new("container");
    let mut c = core(&lib);
    add(&mut c, "atomik.container", "box");
    rename(&mut c, "box", "My Chain");
    select(&mut c, &["box"]);
    c.save_selection_to_library();
    assert_eq!(
        files_in(&lib.path("Library/Containers")),
        ["My Chain.atomikcontainer"]
    );
    assert!(!lib.path("Library/Patches").exists());
}

#[test]
fn save_selection_to_library_several_devices_are_named_first_plus_the_rest() {
    let lib = Tmp::new("several");
    let mut c = core(&lib);
    add(&mut c, SYNTH, "a");
    add(&mut c, DELAY, "b");
    add(&mut c, DELAY, "c");
    rename(&mut c, "a", "Lead");
    select(&mut c, &["a", "b", "c"]);
    c.save_selection_to_library();
    assert_eq!(
        files_in(&lib.path("Library/Patches")),
        ["Lead+2.atomikpatch"]
    );
}

#[test]
fn save_selection_to_library_twice_picks_a_unique_name() {
    let lib = Tmp::new("twice");
    let mut c = core(&lib);
    add(&mut c, SYNTH, "a");
    rename(&mut c, "a", "Pad");
    select(&mut c, &["a"]);
    c.save_selection_to_library();
    c.save_selection_to_library();
    assert_eq!(
        files_in(&lib.path("Library/Patches")),
        ["Pad 2.atomikpatch", "Pad.atomikpatch"]
    );
}

#[test]
fn save_selection_to_library_replaces_characters_a_file_name_cannot_hold() {
    let lib = Tmp::new("chars");
    let mut c = core(&lib);
    add(&mut c, SYNTH, "a");
    rename(&mut c, "a", "Lead: 80s/90s?");
    select(&mut c, &["a"]);
    c.save_selection_to_library();
    assert_eq!(
        files_in(&lib.path("Library/Patches")),
        ["Lead_ 80s_90s_.atomikpatch"]
    );
}

#[test]
fn save_selection_to_library_makes_the_library_and_section_folders_when_missing() {
    let lib = Tmp::new("lazy");
    let mut c = core(&lib);
    add(&mut c, SYNTH, "a");
    select(&mut c, &["a"]);
    assert!(!lib.path("Library").exists());
    c.save_selection_to_library();
    assert!(lib.path("Library/Patches").is_dir());
    assert!(!lib.path("Library/Clips").exists(), "only the used section");
}

#[test]
fn save_selection_to_library_is_deterministic() {
    let (a, b) = (Tmp::new("det-a"), Tmp::new("det-b"));
    for lib in [&a, &b] {
        let mut c = core(lib);
        add(&mut c, SYNTH, "a");
        add(&mut c, DELAY, "b");
        select(&mut c, &["a", "b"]);
        c.save_selection_to_library();
    }
    let read = |lib: &Tmp| std::fs::read(lib.path("Library/Patches/Poly Synth+1.atomikpatch"));
    assert_eq!(read(&a).unwrap(), read(&b).unwrap());
}

#[test]
fn save_selection_to_library_without_a_folder_nothing_selected_or_the_master_reports() {
    let lib = Tmp::new("refuse");
    let mut c = core(&lib);
    add(&mut c, SYNTH, "a");
    select(&mut c, &["a"]);
    c.set_library_root(None);
    c.save_selection_to_library();
    assert!(c.last_rejection().unwrap().contains("library folder"));
    c.set_library_root(Some(lib.path("Library")));
    select(&mut c, &[]);
    c.save_selection_to_library();
    assert!(c.last_rejection().unwrap().contains("Nothing is selected"));
    let master = c.doc().console.master.clone().unwrap();
    c.view_mut().set_show_console_devices(true);
    c.after_view_change();
    c.view_mut()
        .set_selection(std::slice::from_ref(&master), None);
    c.save_selection_to_library();
    assert!(c.last_rejection().unwrap().contains("Master"));
    assert!(!lib.path("Library").exists(), "nothing was written");
}

// ------------------------------------------------------------------ audio

/// A bundle in `<tmp>/Song` holding one asset, and a sampler "s" using it.
fn sampler_with_audio(lib: &Tmp) -> (AppCore<RecordingSink>, String, PathBuf) {
    let wav = lib.path("kick.wav");
    let data: Vec<f32> = (0..4_800).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
    write_wav_f32(&wav, &AudioData::new(48_000, vec![data]).unwrap()).unwrap();
    let mut c = core(lib);
    c.import_audio(vec![wav], atomik_ui::ImportTarget::NewTracks, 0);
    c.wait_background();
    let hash = c.doc().assets.keys().next().expect("an asset").clone();
    let source = c.bundle_root().expect("a bundle").to_path_buf();
    let source_file = Bundle::open(&source).unwrap().asset_path(&hash);
    add(&mut c, "atomik.sampler", "s");
    rename(&mut c, "s", "Kit");
    c.apply_command(Command::SetZones {
        device: key("s"),
        zones: vec![ZoneDoc::new(hash.clone(), 4_800)],
        merge: None,
    })
    .unwrap();
    select(&mut c, &["s"]);
    (c, hash, source_file)
}

#[test]
fn save_selection_to_library_copies_the_audio_into_a_sibling_bundle() {
    let lib = Tmp::new("audio");
    let (mut c, hash, source_file) = sampler_with_audio(&lib);
    c.save_selection_to_library();
    wait(&mut c);
    let copied = lib.path(&format!("Library/Patches/Kit/Audio/{hash}.wav"));
    assert_eq!(
        std::fs::read(&copied).unwrap(),
        std::fs::read(&source_file).unwrap()
    );
    assert!(lib.path("Library/Patches/Kit.atomikpatch").is_file());
    // The item names the asset in its pool, so a load finds the pool entry.
    let text = std::fs::read_to_string(lib.path("Library/Patches/Kit.atomikpatch")).unwrap();
    let item = load_device_item(&text, c.registry()).unwrap();
    assert!(item.assets.contains_key(&hash));
    assert!(c.last_rejection().unwrap().contains("Saved 'Kit'"));
    // The bundle folder is not an item and does not show as a row.
    assert_eq!(
        atomik_library::classify(&lib.path("Library/Patches/Kit"), true),
        Some(FileKind::Folder)
    );
}

#[test]
fn save_selection_to_library_twice_gives_each_copy_its_own_bundle() {
    let lib = Tmp::new("audio2");
    let (mut c, hash, _) = sampler_with_audio(&lib);
    c.save_selection_to_library();
    wait(&mut c);
    c.save_selection_to_library();
    wait(&mut c);
    for stem in ["Kit", "Kit 2"] {
        assert!(
            lib.path(&format!("Library/Patches/{stem}/Audio/{hash}.wav"))
                .is_file(),
            "{stem}"
        );
    }
}

#[test]
fn save_selection_to_library_with_audio_missing_from_the_project_still_saves_and_says_so() {
    let lib = Tmp::new("audio-missing");
    let (mut c, hash, source_file) = sampler_with_audio(&lib);
    std::fs::remove_file(&source_file).unwrap();
    c.save_selection_to_library();
    wait(&mut c);
    assert!(lib.path("Library/Patches/Kit.atomikpatch").is_file());
    assert!(
        !lib.path(&format!("Library/Patches/Kit/Audio/{hash}.wav"))
            .exists()
    );
    let message = c.last_rejection().unwrap();
    assert!(
        message.contains("Kit") && message.contains("missing"),
        "{message}"
    );
}

// ------------------------------------------------------------------ browser

#[test]
fn save_selection_to_library_with_the_panel_open_shows_the_section_and_selects_the_row() {
    let lib = Tmp::new("shows");
    let mut c = core(&lib);
    c.set_file_browser_visible(true);
    wait(&mut c);
    add(&mut c, SYNTH, "a");
    rename(&mut c, "a", "Pad");
    select(&mut c, &["a"]);
    c.save_selection_to_library();
    assert_eq!(
        c.file_browser().location(),
        Some(&Place::Library(LibrarySection::Patches))
    );
    wait(&mut c);
    let rows = c.browser.trace.iter().rev().find_map(|t| match t {
        super::browser::ViewCall::SetRows(r) => Some(r.clone()),
        _ => None,
    });
    assert!(rows.is_some());
    let selected = c.browser.trace.iter().rev().find_map(|t| match t {
        super::browser::ViewCall::Select(p) => Some(p.clone()),
        _ => None,
    });
    assert_eq!(selected, Some(lib.path("Library/Patches/Pad.atomikpatch")));
}

#[test]
fn save_selection_to_library_with_the_panel_closed_only_says_so() {
    let lib = Tmp::new("closed");
    let mut c = core(&lib);
    add(&mut c, SYNTH, "a");
    select(&mut c, &["a"]);
    c.save_selection_to_library();
    assert_eq!(
        c.last_rejection(),
        Some("Saved 'Poly Synth' to the library")
    );
    assert!(c.file_browser().location().is_none());
}

// ------------------------------------------------------------------ clips

#[test]
fn save_clip_to_library_writes_a_clip_named_after_the_track_and_bar() {
    let lib = Tmp::new("clip");
    let mut c = with_clip(&lib);
    c.save_clip_to_library();
    assert_eq!(
        c.last_rejection(),
        Some("Saved 'Bass bar 5' to the library")
    );
    let path = lib.path("Library/Clips/Bass bar 5.atomikclip");
    let file = load_clip_file(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(file.clip.start, 0);
    assert_eq!(file.clip.id, ClipId(0));
    assert_eq!(file.clip.length, BAR);
    assert_eq!(file.clip.content, note_clip(0).content);
    assert_eq!(file.meter, (4, 4));
    assert!((file.tempo_bpm - 120.0).abs() < 1e-9);
}

#[test]
fn save_clip_to_library_keeps_the_tempo_and_meter_at_the_clip_start() {
    let lib = Tmp::new("clip-map");
    let mut c = with_clip(&lib);
    c.apply_command(Command::Song(SongCommand::SetTempoMap(vec![
        TempoEvent::step(0, 120.0),
        TempoEvent::step(2 * BAR, 90.0),
    ])))
    .unwrap();
    c.apply_command(Command::Song(SongCommand::SetMeterMap(vec![
        MeterEvent {
            bar: 1,
            num: 4,
            den: 4,
        },
        MeterEvent {
            bar: 3,
            num: 3,
            den: 4,
        },
    ])))
    .unwrap();
    c.save_clip_to_library();
    // The clip starts at 4 * BAR: after the change to 3/4 at bar 3 and the one to 90 BPM.
    let names = files_in(&lib.path("Library/Clips"));
    assert_eq!(names.len(), 1, "{names:?}");
    let text = std::fs::read_to_string(lib.path("Library/Clips").join(&names[0])).unwrap();
    let file = load_clip_file(&text).unwrap();
    assert_eq!(file.meter, (3, 4));
    assert!((file.tempo_bpm - 90.0).abs() < 1e-9);
}

#[test]
fn save_clip_to_library_needs_exactly_one_note_clip() {
    let lib = Tmp::new("clip-refuse");
    let mut c = with_clip(&lib);
    c.arrangement_mut().set_selected_clips(&[]);
    c.save_clip_to_library();
    assert!(c.last_rejection().unwrap().contains("exactly one"));
    c.apply_command(Command::Song(SongCommand::AddClip {
        device: key("a"),
        lane: 0,
        clip: note_clip(0),
    }))
    .unwrap();
    let ids: Vec<ClipId> = c.doc().song.tracks[0].lanes[0]
        .clips
        .iter()
        .map(|x| x.id)
        .collect();
    c.arrangement_mut().set_selected_clips(&ids);
    c.save_clip_to_library();
    assert!(c.last_rejection().unwrap().contains("exactly one"));
    assert!(!lib.path("Library").exists());
}

#[test]
fn save_clip_to_library_twice_picks_a_unique_name_and_leaves_the_document_alone() {
    let lib = Tmp::new("clip-twice");
    let mut c = with_clip(&lib);
    let before = save_project(c.doc());
    c.save_clip_to_library();
    c.save_clip_to_library();
    assert_eq!(
        files_in(&lib.path("Library/Clips")),
        ["Bass bar 5 2.atomikclip", "Bass bar 5.atomikclip"]
    );
    assert_eq!(save_project(c.doc()), before);
}

#[test]
fn save_clip_to_library_with_the_panel_open_shows_the_clips_section() {
    let lib = Tmp::new("clip-shows");
    let mut c = with_clip(&lib);
    c.set_file_browser_visible(true);
    wait(&mut c);
    c.save_clip_to_library();
    wait(&mut c);
    assert_eq!(
        c.file_browser().location(),
        Some(&Place::Library(LibrarySection::Clips))
    );
    let _ = BrowseAction::Refresh;
}

// ------------------------------------------------------------------ load

/// Saves synth "Lead" and a delay as one patch of `lib` and returns its path.
fn saved_patch(lib: &Tmp) -> PathBuf {
    let mut c = core(lib);
    add(&mut c, SYNTH, "a");
    add(&mut c, DELAY, "b");
    rename(&mut c, "a", "Lead");
    select(&mut c, &["a", "b"]);
    c.save_selection_to_library();
    lib.path("Library/Patches/Lead+1.atomikpatch")
}

fn type_count(c: &AppCore<RecordingSink>, type_id: &str) -> usize {
    c.doc()
        .devices
        .iter()
        .filter(|d| d.type_id == type_id)
        .count()
}

#[test]
fn load_library_item_patch_undo_removes_all_pasted() {
    let lib = Tmp::new("load-patch");
    let path = saved_patch(&lib);
    let mut c = core(&lib);
    add(&mut c, SYNTH, "x");
    let before = save_project(c.doc());
    c.load_library_item(&path, LoadTarget::Default);
    assert_eq!(type_count(&c, SYNTH), 2);
    assert_eq!(type_count(&c, DELAY), 1);
    assert!(c.doc().devices.iter().any(|d| d.name == "Lead"));
    assert!(c.undo(), "one undo step");
    assert_eq!(save_project(c.doc()), before);
}

#[test]
fn load_library_item_container_adds_the_container_with_its_inside() {
    let lib = Tmp::new("load-container");
    let mut a = core(&lib);
    add(&mut a, "atomik.container", "box");
    a.apply_command(Command::Container(ContainerCommand::AddInner {
        container: key("box"),
        index: 0,
        type_id: SYNTH.to_string(),
        key: None,
        auto_split: false,
    }))
    .unwrap();
    rename(&mut a, "box", "My Chain");
    select(&mut a, &["box"]);
    a.save_selection_to_library();
    let path = lib.path("Library/Containers/My Chain.atomikcontainer");
    let mut c = core(&lib);
    c.load_library_item(&path, LoadTarget::Default);
    let container = c
        .doc()
        .devices
        .iter()
        .find(|d| d.type_id == "atomik.container")
        .expect("the container")
        .key
        .clone();
    assert_eq!(c.doc().scope_devices(Some(&container)).unwrap().len(), 1);
    assert!(c.undo());
    assert_eq!(type_count(&c, "atomik.container"), 0);
}

#[test]
fn load_library_item_a_container_file_holding_a_patch_is_refused() {
    let lib = Tmp::new("load-wrong");
    let patch = saved_patch(&lib);
    let wrong = lib.path("Library/Containers/Fake.atomikcontainer");
    std::fs::create_dir_all(wrong.parent().unwrap()).unwrap();
    std::fs::copy(&patch, &wrong).unwrap();
    let mut c = core(&lib);
    let before = save_project(c.doc());
    c.load_library_item(&wrong, LoadTarget::Default);
    assert!(c.last_rejection().unwrap().contains("not a container"));
    assert_eq!(save_project(c.doc()), before);
}

#[test]
fn load_library_item_a_missing_or_damaged_file_reports_and_changes_nothing() {
    let lib = Tmp::new("load-bad");
    let mut c = core(&lib);
    let before = save_project(c.doc());
    c.load_library_item(
        &lib.path("Library/Patches/Gone.atomikpatch"),
        LoadTarget::Default,
    );
    assert!(c.last_rejection().unwrap().contains("Gone"));
    let bad = lib.path("Library/Patches/Bad.atomikpatch");
    std::fs::create_dir_all(bad.parent().unwrap()).unwrap();
    std::fs::write(&bad, "{ not json").unwrap();
    c.load_library_item(&bad, LoadTarget::Default);
    assert!(c.last_rejection().unwrap().contains("Bad"));
    c.load_library_item(&lib.path("notes.txt"), LoadTarget::Default);
    assert!(c.last_rejection().unwrap().contains("not a library item"));
    assert_eq!(save_project(c.doc()), before);
}

#[test]
fn load_library_item_into_a_rack_slot_lands_at_the_slot() {
    let lib = Tmp::new("load-slot");
    let path = saved_patch(&lib);
    let mut c = core(&lib);
    add(&mut c, DELAY, "x");
    let columns = c.doc().columns;
    c.load_library_item(
        &path,
        LoadTarget::Rack {
            container: None,
            slot: RackSlot {
                column: columns,
                index: 0,
            },
        },
    );
    assert_eq!(c.doc().columns, columns + 1, "a new column");
    let new: Vec<&str> = c
        .doc()
        .devices
        .iter()
        .filter(|d| d.column == columns && !atomik_model::console::is_console_type(&d.type_id))
        .map(|d| d.type_id.as_str())
        .collect();
    assert_eq!(new, [SYNTH, DELAY]);
    assert!(c.undo());
    assert_eq!(c.doc().columns, columns);
}

#[test]
fn load_library_item_into_a_container_target_goes_inside_at_the_index() {
    let lib = Tmp::new("load-inside");
    let path = saved_patch(&lib);
    let mut c = core(&lib);
    add(&mut c, "atomik.container", "box");
    c.load_library_item(
        &path,
        LoadTarget::Rack {
            container: Some(key("box")),
            slot: RackSlot {
                column: 0,
                index: 0,
            },
        },
    );
    assert_eq!(c.doc().scope_devices(Some(&key("box"))).unwrap().len(), 2);
    assert!(c.undo());
    assert_eq!(c.doc().scope_devices(Some(&key("box"))).unwrap().len(), 0);
}

#[test]
fn load_library_item_device_uri_adds_the_device_like_the_browser() {
    let lib = Tmp::new("load-uri");
    let mut c = core(&lib);
    c.load_library_item(&device_uri(DELAY), LoadTarget::Default);
    assert_eq!(type_count(&c, DELAY), 1);
    c.load_library_item(&device_uri(SYNTH), LoadTarget::Default);
    assert_eq!(type_count(&c, SYNTH), 1);
    assert_eq!(
        c.doc().song.tracks.len(),
        1,
        "an instrument gets its track through the creation chain"
    );
    c.load_library_item(&device_uri("atomik.nothing"), LoadTarget::Default);
    assert!(c.last_rejection().unwrap().contains("atomik.nothing"));
}

#[test]
fn load_library_item_device_uri_into_a_slot_and_a_container() {
    let lib = Tmp::new("load-uri-slot");
    let mut c = core(&lib);
    add(&mut c, "atomik.container", "box");
    let columns = c.doc().columns;
    c.load_library_item(
        &device_uri(DELAY),
        LoadTarget::Rack {
            container: None,
            slot: RackSlot {
                column: columns,
                index: 0,
            },
        },
    );
    assert!(
        c.doc()
            .devices
            .iter()
            .any(|d| d.type_id == DELAY && d.column == columns)
    );
    c.load_library_item(
        &device_uri(SYNTH),
        LoadTarget::Rack {
            container: Some(key("box")),
            slot: RackSlot {
                column: 0,
                index: 0,
            },
        },
    );
    assert_eq!(c.doc().scope_devices(Some(&key("box"))).unwrap().len(), 1);
}

#[test]
fn browse_load_loads_a_patch_and_open_project_asks_the_shell() {
    let lib = Tmp::new("browse-load");
    let path = saved_patch(&lib);
    let mut c = core(&lib);
    c.browse(BrowseAction::Load(path.clone()));
    assert_eq!(type_count(&c, DELAY), 1);
    assert_eq!(c.take_open_request(), None, "a patch is not a project");
    c.browse(BrowseAction::OpenProject(path.clone()));
    assert_eq!(c.take_open_request(), Some(path));
    assert_eq!(c.take_open_request(), None, "once");
    let project = lib.path("Library/Projects/Song.atomik");
    c.browse(BrowseAction::Load(project.clone()));
    assert_eq!(c.take_open_request(), Some(project));
}

// ------------------------------------------------------------------ load: audio

#[test]
fn load_library_item_with_a_sample_finds_its_audio() {
    let lib = Tmp::new("load-audio");
    let (mut a, hash, _) = sampler_with_audio(&lib);
    a.save_selection_to_library();
    wait(&mut a);
    let path = lib.path("Library/Patches/Kit.atomikpatch");
    let mut c = core(&lib);
    c.load_library_item(&path, LoadTarget::Default);
    c.wait_background();
    assert!(c.doc().assets.contains_key(&hash), "the pool entry");
    assert!(c.missing_assets().is_empty(), "{:?}", c.missing_assets());
    assert!(c.song_assets().contains_key(&hash), "loaded for the engine");
    let root = c.bundle_root().expect("a session bundle").to_path_buf();
    assert!(Bundle::open(&root).unwrap().exists(&hash), "copied in");
    assert_eq!(type_count(&c, "atomik.sampler"), 1);
}

#[test]
fn load_library_item_with_a_sample_into_a_slot_registers_the_audio_too() {
    let lib = Tmp::new("load-audio-slot");
    let (mut a, hash, _) = sampler_with_audio(&lib);
    a.save_selection_to_library();
    wait(&mut a);
    let path = lib.path("Library/Patches/Kit.atomikpatch");
    let mut c = core(&lib);
    let columns = c.doc().columns;
    c.load_library_item(
        &path,
        LoadTarget::Rack {
            container: None,
            slot: RackSlot {
                column: columns,
                index: 0,
            },
        },
    );
    c.wait_background();
    assert!(c.doc().assets.contains_key(&hash));
    assert!(c.song_assets().contains_key(&hash));
}

#[test]
fn load_library_item_without_the_audio_folder_the_asset_is_missing_but_the_device_loads() {
    let lib = Tmp::new("load-audio-gone");
    let (mut a, hash, _) = sampler_with_audio(&lib);
    a.save_selection_to_library();
    wait(&mut a);
    std::fs::remove_dir_all(lib.path("Library/Patches/Kit")).unwrap();
    let mut c = core(&lib);
    c.load_library_item(
        &lib.path("Library/Patches/Kit.atomikpatch"),
        LoadTarget::Default,
    );
    c.wait_background();
    assert_eq!(type_count(&c, "atomik.sampler"), 1);
    let missing: Vec<&str> = c.missing_assets().iter().map(|m| m.hash.as_str()).collect();
    assert_eq!(missing, [hash.as_str()], "reported, so it can be relinked");
    assert!(c.last_rejection().unwrap().contains("audio"));
}

#[test]
fn load_library_item_twice_loads_the_audio_once() {
    let lib = Tmp::new("load-audio-twice");
    let (mut a, hash, _) = sampler_with_audio(&lib);
    a.save_selection_to_library();
    wait(&mut a);
    let path = lib.path("Library/Patches/Kit.atomikpatch");
    let mut c = core(&lib);
    c.load_library_item(&path, LoadTarget::Default);
    c.wait_background();
    c.load_library_item(&path, LoadTarget::Default);
    c.wait_background();
    assert_eq!(type_count(&c, "atomik.sampler"), 2);
    assert!(c.missing_assets().is_empty());
    assert!(c.song_assets().contains_key(&hash));
}

// ------------------------------------------------------------------ load: clips

/// A core with synth "t" and its track, and a clip item in the library.
fn track_core(lib: &Tmp) -> (AppCore<RecordingSink>, PathBuf) {
    let mut a = with_clip(lib);
    a.save_clip_to_library();
    let path = lib.path("Library/Clips/Bass bar 5.atomikclip");
    let mut c = core(lib);
    add(&mut c, SYNTH, "t");
    c.apply_command(Command::Song(SongCommand::AddTrack { device: key("t") }))
        .unwrap();
    c.after_view_change();
    (c, path)
}

fn clips_of(c: &AppCore<RecordingSink>) -> Vec<(i64, i64)> {
    c.doc().song.tracks[0].lanes[0]
        .clips
        .iter()
        .map(|x| (x.start, x.length))
        .collect()
}

#[test]
fn load_library_item_clip_lands_on_the_selected_track_at_the_snapped_playhead() {
    let lib = Tmp::new("load-clip");
    let (mut c, path) = track_core(&lib);
    c.arrangement_mut().set_selected_track(Some(key("t")));
    c.transport_action(TransportAction::Locate(BAR + PPQ / 8));
    let at = c.snapped_playhead();
    assert!(at > 0);
    c.load_library_item(&path, LoadTarget::Default);
    assert_eq!(clips_of(&c), [(at, BAR)]);
    assert!(c.undo(), "one undo step");
    assert!(clips_of(&c).is_empty());
}

#[test]
fn load_library_item_clip_without_a_note_track_selected_says_so() {
    let lib = Tmp::new("load-clip-none");
    let (mut c, path) = track_core(&lib);
    let before = save_project(c.doc());
    c.load_library_item(&path, LoadTarget::Default);
    assert_eq!(
        c.last_rejection(),
        Some("Select an instrument track to load a clip")
    );
    assert_eq!(save_project(c.doc()), before);
}

#[test]
fn load_library_item_clip_on_a_track_target_goes_there() {
    let lib = Tmp::new("load-clip-track");
    let (mut c, path) = track_core(&lib);
    c.load_library_item(
        &path,
        LoadTarget::Track {
            device: key("t"),
            lane: 0,
            tick: 2 * BAR,
        },
    );
    assert_eq!(clips_of(&c), [(2 * BAR, BAR)]);
    let notes = &c.doc().song.tracks[0].lanes[0].clips[0].content;
    assert_eq!(*notes, note_clip(0).content);
}

#[test]
fn load_library_item_clip_on_a_missing_track_reports_and_changes_nothing() {
    let lib = Tmp::new("load-clip-bad");
    let (mut c, path) = track_core(&lib);
    let before = save_project(c.doc());
    c.load_library_item(
        &path,
        LoadTarget::Track {
            device: key("nobody"),
            lane: 0,
            tick: 0,
        },
    );
    assert!(c.last_rejection().is_some());
    assert_eq!(save_project(c.doc()), before);
}

#[test]
fn save_selection_to_library_with_the_panel_open_offers_the_new_name() {
    let lib = Tmp::new("offers-name");
    let mut c = core(&lib);
    c.set_file_browser_visible(true);
    wait(&mut c);
    add(&mut c, SYNTH, "a");
    rename(&mut c, "a", "Pad");
    select(&mut c, &["a"]);
    c.save_selection_to_library();
    wait(&mut c);
    assert!(c.file_browser().is_text_owner(), "the rename entry is open");
    assert!(c.take_browser_focus_request(), "the panel takes the keys");
    assert!(!c.take_browser_focus_request(), "once");
}

#[test]
fn save_selection_to_library_with_the_panel_closed_offers_nothing() {
    let lib = Tmp::new("offers-none");
    let mut c = core(&lib);
    add(&mut c, SYNTH, "a");
    select(&mut c, &["a"]);
    c.save_selection_to_library();
    assert!(!c.file_browser().is_text_owner());
    assert!(!c.take_browser_focus_request());
}
