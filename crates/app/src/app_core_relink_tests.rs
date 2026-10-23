//! Relinking missing audio in the app core: any file replaces the asset in one undo step, the
//! same audio restores the file, a folder search matches by name, bad files change nothing.
//! Every fixture is procedural and lives in a temporary folder.

use super::super::*;
use crate::sink::{RecordingSink, SinkCall};
use atomik_audio_file::{AudioData, write_wav_f32};
use atomik_engine::SongInput;
use atomik_model::{ClipContent, ZoneDoc, empty_doc};
use atomik_render::default_registry;
use atomik_render::project::{ProjectOptions, render_doc_in};
use atomik_ui::ImportTarget;
use std::sync::atomic::AtomicU64;

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "atomik-app-relink-core-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn core() -> AppCore<RecordingSink> {
    let mut c = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    c.sink_mut().take();
    c
}

/// A mono ramp of `frames` frames; `seed` scales it (so distinct audio, distinct assets).
fn wav_in(dir: &Path, name: &str, frames: usize, seed: f32) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let data: Vec<f32> = (0..frames)
        .map(|i| (i as f32 / frames as f32) * seed)
        .collect();
    let path = dir.join(name);
    write_wav_f32(&path, &AudioData::new(48_000, vec![data]).unwrap()).unwrap();
    path
}

fn wav(t: &Tmp, name: &str, frames: usize, seed: f32) -> PathBuf {
    wav_in(&t.0, name, frames, seed)
}

fn import(c: &mut AppCore<RecordingSink>, path: &Path) {
    c.import_audio(vec![path.to_path_buf()], ImportTarget::NewTracks, 0);
    c.wait_background();
}

fn audio_assets(c: &AppCore<RecordingSink>) -> Vec<String> {
    let mut out = Vec::new();
    for lane in c.doc().song.tracks.iter().flat_map(|t| &t.lanes) {
        for clip in &lane.clips {
            if let ClipContent::Audio(a) = &clip.content {
                out.push(a.asset.clone());
            }
        }
    }
    out
}

fn last_song(c: &mut AppCore<RecordingSink>) -> SongInput {
    c.sink_mut()
        .take()
        .into_iter()
        .rev()
        .find_map(|call| match call {
            SinkCall::Song(input) => Some(input),
            _ => None,
        })
        .expect("a song was submitted")
}

fn bounce(c: &AppCore<RecordingSink>) -> Vec<Vec<f32>> {
    render_doc_in(
        c.doc(),
        &default_registry(),
        &ProjectOptions::default(),
        c.bundle_root(),
    )
    .expect("the bounce renders")
    .channels
}

fn peak(channels: &[Vec<f32>]) -> f32 {
    channels
        .iter()
        .flatten()
        .fold(0.0_f32, |m, x| m.max(x.abs()))
}

/// A saved project with one clip of `kick.wav` whose asset file was then deleted.
struct Missing {
    t: Tmp,
    file: PathBuf,
    root: PathBuf,
    hash: String,
}

fn missing_project(tag: &str) -> Missing {
    let t = Tmp::new(tag);
    let original = wav_in(&t.path("source"), "kick.wav", 4_800, 1.0);
    let mut c = core();
    import(&mut c, &original);
    let hash = c.doc().assets.keys().next().unwrap().clone();
    c.save_as(&t.path("song.atomik")).unwrap();
    let root = t.path("song");
    std::fs::remove_file(root.join("Audio").join(format!("{hash}.wav"))).unwrap();
    Missing {
        file: root.join("song.atomik"),
        root,
        hash,
        t,
    }
}

fn open(m: &Missing) -> AppCore<RecordingSink> {
    let mut c = core();
    c.open(&m.file).unwrap();
    c.wait_background();
    c
}

#[test]
fn a_different_file_replaces_the_missing_asset_in_one_undo_step() {
    let m = missing_project("replace");
    let mut c = open(&m);
    assert_eq!(c.missing_assets().len(), 1);
    assert!(
        last_song(&mut c).audio.is_empty(),
        "silent while it is missing"
    );
    let other = wav(&m.t, "snare.wav", 4_800, 0.5);
    c.relink_asset(&m.hash, other);
    c.wait_background();

    assert!(c.missing_assets().is_empty(), "{:?}", c.missing_assets());
    let new = audio_assets(&c).pop().unwrap();
    assert_ne!(new, m.hash);
    assert_eq!(audio_assets(&c), std::slice::from_ref(&new));
    assert_eq!(c.doc().assets[&new].original_name, "snare.wav");
    assert!(c.song_assets().contains_key(&new));
    assert!(!c.song_assets().contains_key(&m.hash));
    assert!(c.peaks().contains_key(&new));
    assert!(
        c.last_rejection().unwrap().contains("relinked 1, 0 still"),
        "{:?}",
        c.last_rejection()
    );
    // The engine got a song with the audio.
    assert_eq!(last_song(&mut c).audio.len(), 1);
    // The bounce plays it, the same every time.
    let first = bounce(&c);
    assert!(peak(&first) > 0.01);
    assert_eq!(first, bounce(&c));

    // Undo brings the old, missing state back; redo relinks again.
    let after = c.doc().clone();
    assert!(c.undo());
    assert_eq!(audio_assets(&c), std::slice::from_ref(&m.hash));
    assert_eq!(c.missing_assets().len(), 1);
    assert_eq!(c.missing_assets()[0].hash, m.hash);
    assert!(last_song(&mut c).audio.is_empty());
    assert!(c.redo());
    assert_eq!(c.doc(), &after);
    assert!(c.missing_assets().is_empty());
    assert_eq!(last_song(&mut c).audio.len(), 1);
    // One step: the next undo leaves the relink behind for good.
    assert!(c.undo());
    assert_eq!(audio_assets(&c), std::slice::from_ref(&m.hash));
}

#[test]
fn a_relinked_project_saves_and_reopens_without_missing_files() {
    let m = missing_project("save");
    let mut c = open(&m);
    c.relink_asset(&m.hash, wav(&m.t, "snare.wav", 4_800, 0.5));
    c.wait_background();
    c.save().unwrap();
    let new = audio_assets(&c).pop().unwrap();
    assert!(m.root.join("Audio").join(format!("{new}.wav")).is_file());
    let saved = std::fs::read_to_string(&m.file).unwrap();
    assert!(
        !saved.contains(&m.hash),
        "the missing asset is gone from the file"
    );
    let mut again = core();
    again.open(&m.file).unwrap();
    again.wait_background();
    assert!(again.missing_assets().is_empty());
    assert!(
        again.last_rejection().is_none(),
        "{:?}",
        again.last_rejection()
    );
    assert_eq!(last_song(&mut again).audio.len(), 1);
    assert!(peak(&bounce(&again)) > 0.01);
}

#[test]
fn the_same_audio_restores_the_file_without_a_document_change() {
    let m = missing_project("same");
    let mut c = open(&m);
    let before = c.doc().clone();
    // The original file, moved elsewhere and renamed: same content, same hash.
    let moved = wav_in(&m.t.path("elsewhere"), "renamed.wav", 4_800, 1.0);
    assert_eq!(moved.file_name().unwrap(), "renamed.wav");
    c.relink_asset(&m.hash, moved);
    c.wait_background();
    assert!(c.missing_assets().is_empty());
    assert_eq!(c.doc(), &before, "no document change");
    assert!(!c.undo(), "and no undo step");
    assert!(
        m.root
            .join("Audio")
            .join(format!("{}.wav", m.hash))
            .is_file()
    );
    assert!(c.song_assets().contains_key(&m.hash));
    assert_eq!(last_song(&mut c).audio.len(), 1, "the clip plays again");
    assert!(peak(&bounce(&c)) > 0.01);
    assert!(!c.is_dirty());
}

#[test]
fn a_file_that_does_not_decode_is_reported_by_name_and_changes_nothing() {
    let m = missing_project("junk");
    let mut c = open(&m);
    let before = c.doc().clone();
    let junk = m.t.path("broken.wav");
    std::fs::write(&junk, b"this is not audio").unwrap();
    c.relink_asset(&m.hash, junk);
    c.wait_background();
    assert_eq!(c.doc(), &before);
    assert!(!c.undo());
    assert_eq!(c.missing_assets().len(), 1);
    let message = c.last_rejection().unwrap();
    assert!(message.contains("broken.wav"), "{message}");
    assert!(message.contains("relinked 0, 1 still missing"), "{message}");
}

#[test]
fn only_a_missing_asset_can_be_relinked() {
    let m = missing_project("notmissing");
    let mut c = open(&m);
    let before = c.doc().clone();
    c.relink_asset(&"0".repeat(64), wav(&m.t, "x.wav", 100, 1.0));
    c.wait_background();
    assert_eq!(c.doc(), &before);
    assert!(c.last_rejection().unwrap().contains("not missing"));
}

#[test]
fn a_folder_search_matches_by_name_in_nested_folders_and_leaves_the_others() {
    let t = Tmp::new("folder");
    let mut c = core();
    let (kick, snare) = (
        wav_in(&t.path("source"), "kick.wav", 4_800, 1.0),
        wav_in(&t.path("source"), "snare.wav", 2_400, 0.5),
    );
    c.import_audio(vec![kick, snare], ImportTarget::NewTracks, 0);
    c.wait_background();
    let hashes: Vec<String> = c.doc().assets.keys().cloned().collect();
    c.save_as(&t.path("song.atomik")).unwrap();
    let root = t.path("song");
    for h in &hashes {
        std::fs::remove_file(root.join("Audio").join(format!("{h}.wav"))).unwrap();
    }
    let mut c = core();
    c.open(&root.join("song.atomik")).unwrap();
    c.wait_background();
    assert_eq!(c.missing_assets().len(), 2);

    // KICK.WAV lives two folders down in another case and with other audio; no snare anywhere.
    let found = wav_in(
        &t.path("search").join("a").join("b"),
        "KICK.WAV",
        4_800,
        0.25,
    );
    wav_in(&t.path("search"), "hat.wav", 4_800, 0.25);
    c.relink_search_folder(t.path("search"));
    c.wait_background();

    let names: Vec<&str> = c.missing_assets().iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["snare.wav"], "kick relinked, snare still missing");
    let message = c.last_rejection().unwrap();
    assert!(message.contains("relinked 1, 1 still missing"), "{message}");
    assert_eq!(
        c.doc().assets[&audio_assets(&c)[0]].original_name,
        "KICK.WAV",
        "the kick clip plays the file the search found"
    );
    assert!(found.is_file());
    // One undo step for the one relink.
    assert!(c.undo());
    assert_eq!(c.missing_assets().len(), 2);
}

#[test]
fn a_folder_search_that_finds_the_original_file_restores_it() {
    let m = missing_project("foldersame");
    let mut c = open(&m);
    let before = c.doc().clone();
    // The folder holds the original audio under a different case.
    let dir = m.t.path("backup");
    wav_in(&dir, "KICK.wav", 4_800, 1.0);
    c.relink_search_folder(dir);
    c.wait_background();
    assert!(c.missing_assets().is_empty());
    assert_eq!(c.doc(), &before);
    assert!(c.song_assets().contains_key(&m.hash));
    assert!(c.last_rejection().unwrap().contains("relinked 1, 0 still"));
}

#[test]
fn a_folder_search_without_matches_says_so() {
    let m = missing_project("nomatch");
    let mut c = open(&m);
    let dir = m.t.path("empty");
    wav_in(&dir, "other.wav", 100, 1.0);
    c.relink_search_folder(dir);
    c.wait_background();
    assert_eq!(c.missing_assets().len(), 1);
    let message = c.last_rejection().unwrap();
    assert!(message.contains("no matching audio files"), "{message}");
    assert!(message.contains("1 still missing"), "{message}");
}

#[test]
fn a_folder_search_with_nothing_missing_is_refused() {
    let t = Tmp::new("nothing");
    let mut c = core();
    c.relink_search_folder(t.path("anywhere"));
    c.wait_background();
    assert!(c.last_rejection().unwrap().contains("no missing"));
}

#[test]
fn the_search_runs_off_the_ui_thread() {
    let m = missing_project("async");
    let mut c = open(&m);
    c.relink_search_folder(m.t.path("source"));
    assert!(c.is_busy(), "the call returned before the scan finished");
    c.wait_background();
    assert!(!c.is_busy());
    assert!(c.missing_assets().is_empty(), "the original is in `source`");
}

#[test]
fn relinking_swaps_the_zones_of_a_sampler_and_the_editor_stops_showing_missing() {
    let t = Tmp::new("sampler");
    let mut c = core();
    c.apply_command(atomik_model::Command::AddDevice {
        index: usize::MAX,
        type_id: "atomik.sampler".into(),
        key: Some(atomik_model::DeviceKey("smp".into())),
    })
    .unwrap();
    c.handle_action(atomik_ui::UiAction::ImportSamples {
        paths: vec![wav_in(&t.path("source"), "a_C3.wav", 4_800, 1.0)],
        device: atomik_model::DeviceKey("smp".into()),
        at_key: None,
    });
    c.wait_background();
    let key = atomik_model::DeviceKey("smp".into());
    let hash = c.doc().device(&key).unwrap().zones[0].asset.clone();
    c.save_as(&t.path("song.atomik")).unwrap();
    std::fs::remove_file(t.path("song").join("Audio").join(format!("{hash}.wav"))).unwrap();
    let mut c = core();
    c.open(&t.path("song").join("song.atomik")).unwrap();
    c.wait_background();
    c.open_sample_editor(key.clone());
    c.sink_mut().take();

    c.relink_asset(&hash, wav_in(&t.path("new"), "b_C3.wav", 4_800, 0.5));
    c.wait_background();
    let zones: Vec<ZoneDoc> = c.doc().device(&key).unwrap().zones.clone();
    assert_eq!(zones.len(), 1);
    assert_ne!(zones[0].asset, hash);
    assert!(c.missing_assets().is_empty());
    // The sampler got a playable map.
    let sent = c
        .sink_mut()
        .take()
        .into_iter()
        .filter_map(|call| match call {
            SinkCall::Samples { map, .. } => Some(map),
            _ => None,
        })
        .next_back()
        .expect("a map was sent");
    assert_eq!(sent.expect("a map with a zone").zones().len(), 1);
}
