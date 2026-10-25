//! Save to Library in the app core: the selected devices become a patch or a container item,
//! a selected note clip a clip item, in the library folder, with the audio copied beside the
//! item by a worker. Saving changes neither the document nor the undo history. Every test
//! uses its own temporary library folder.

use super::*;
use crate::sink::RecordingSink;
use atomik_audio_file::{AudioData, Bundle, write_wav_f32};
use atomik_core::PPQ;
use atomik_library::{FileKind, LibrarySection};
use atomik_model::{
    Clip, ClipContent, ClipId, Command, DeviceKey, MeterEvent, Note, SongCommand, TempoEvent,
    ZoneDoc, empty_doc, load_clip_file, load_device_item, save_project,
};
use atomik_ui::{BrowseAction, Place};
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
