//! Audio in the app core: bundles, save/open, import, peaks, engine assets.
//! All fixtures are procedural and live in temporary folders.

use super::*;
use crate::app_core::engine_config;
use crate::sink::{RecordingSink, SinkCall};
use atomik_audio_file::{AudioData, content_hash, write_wav_f32};
use atomik_core::PPQ;
use atomik_engine::{Graph, SongInput};
use atomik_model::{ClipContent, empty_doc};
use atomik_ui::ImportTarget;

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("atomik-app-audio-{tag}-{}-{n}", std::process::id()));
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

/// A mono ramp file of `frames` frames at `rate` (each frame count and `seed` gives a
/// distinct asset).
fn wav(dir: &Tmp, name: &str, frames: usize, rate: u32, seed: f32) -> PathBuf {
    let data: Vec<f32> = (0..frames)
        .map(|i| (i as f32 / frames as f32) * seed)
        .collect();
    let path = dir.path(name);
    write_wav_f32(&path, &AudioData::new(rate, vec![data]).unwrap()).unwrap();
    path
}

fn last_song(c: &mut AppCore<RecordingSink>) -> SongInput {
    let calls = c.sink_mut().take();
    calls
        .into_iter()
        .rev()
        .find_map(|call| match call {
            SinkCall::Song(input) => Some(input),
            _ => None,
        })
        .expect("a song was submitted")
}

fn audio_clips(c: &AppCore<RecordingSink>) -> Vec<(String, i64, i64, String)> {
    let mut out = Vec::new();
    for t in &c.doc().song.tracks {
        for lane in &t.lanes {
            for clip in &lane.clips {
                if let ClipContent::Audio(a) = &clip.content {
                    out.push((t.device.0.clone(), clip.start, clip.length, a.asset.clone()));
                }
            }
        }
    }
    out
}

fn import(c: &mut AppCore<RecordingSink>, paths: &[PathBuf], target: ImportTarget, tick: i64) {
    c.import_audio(paths.to_vec(), target, tick);
    c.wait_background();
}

// ------------------------------------------------------------------ import

#[test]
fn new_track_imports_name_each_track_after_its_file_and_place_clips_at_the_tick() {
    let t = Tmp::new("newtracks");
    let kick = wav(&t, "kick.wav", 48_000, 48_000, 1.0);
    let snare = wav(&t, "snare.wav", 24_000, 48_000, 0.5);
    let mut c = core();
    import(&mut c, &[kick, snare], ImportTarget::NewTracks, 4 * PPQ);
    assert!(c.last_rejection().is_none(), "{:?}", c.last_rejection());
    let names: Vec<&str> = c
        .doc()
        .song
        .tracks
        .iter()
        .map(|t| c.doc().device(&t.device).unwrap().name.as_str())
        .collect();
    assert_eq!(names, ["kick", "snare"]);
    let clips = audio_clips(&c);
    assert_eq!(clips.len(), 2);
    // 1 s and 0.5 s at 120 BPM: two beats and one beat.
    assert_eq!((clips[0].1, clips[0].2), (4 * PPQ, 2 * PPQ));
    assert_eq!((clips[1].1, clips[1].2), (4 * PPQ, PPQ));
    assert_eq!(c.doc().assets.len(), 2);
    assert_eq!(c.doc().assets[&clips[0].3].original_name, "kick.wav");
}

#[test]
fn existing_track_imports_follow_one_another_in_path_order() {
    let t = Tmp::new("existing");
    let a = wav(&t, "a.wav", 48_000, 48_000, 1.0);
    let b = wav(&t, "b.wav", 24_000, 48_000, 0.5);
    let d = wav(&t, "d.wav", 12_000, 48_000, 0.25);
    let mut c = core();
    import(&mut c, std::slice::from_ref(&a), ImportTarget::NewTracks, 0);
    let track = c.doc().song.tracks[0].device.clone();
    let before = audio_clips(&c).len();
    import(
        &mut c,
        &[b, d, a],
        ImportTarget::ExistingTrack(track.clone()),
        PPQ,
    );
    let clips = audio_clips(&c);
    assert_eq!(clips.len(), before + 3);
    let new: Vec<(i64, i64)> = clips[before..].iter().map(|x| (x.1, x.2)).collect();
    assert_eq!(
        new,
        [(PPQ, PPQ), (2 * PPQ, PPQ / 2), (2 * PPQ + PPQ / 2, 2 * PPQ)]
    );
    assert!(clips[before..].iter().all(|x| x.0 == track.0));
}

#[test]
fn clip_lengths_are_rounded_up_to_whole_ticks() {
    let t = Tmp::new("round");
    // 100 001 frames at 120 BPM and 48 kHz need 64 001 ticks.
    let f = wav(&t, "odd.wav", 100_001, 48_000, 1.0);
    let mut c = core();
    import(&mut c, &[f], ImportTarget::NewTracks, 0);
    assert_eq!(audio_clips(&c)[0].2, 64_001);
}

#[test]
fn a_failing_file_is_reported_by_name_and_the_others_still_import() {
    let t = Tmp::new("errors");
    let good1 = wav(&t, "good1.wav", 4_800, 48_000, 1.0);
    let junk = t.path("junk.wav");
    std::fs::write(&junk, b"this is not audio").unwrap();
    let gone = t.path("gone.flac");
    let text = t.path("notes.txt");
    std::fs::write(&text, b"hello").unwrap();
    let good2 = wav(&t, "good2.wav", 9_600, 48_000, 0.5);
    let mut c = core();
    import(
        &mut c,
        &[good1, junk, gone, text, good2],
        ImportTarget::NewTracks,
        0,
    );
    assert_eq!(audio_clips(&c).len(), 2);
    let message = c
        .last_rejection()
        .expect("the failures are reported")
        .to_owned();
    for name in ["junk.wav", "gone.flac", "notes.txt"] {
        assert!(message.contains(name), "{message}");
    }
    assert!(!message.contains("good1.wav"));
}

#[test]
fn imports_run_off_the_ui_thread_and_apply_when_polled() {
    let t = Tmp::new("async");
    let f = wav(&t, "a.wav", 48_000, 48_000, 1.0);
    let mut c = core();
    c.import_audio(vec![f], ImportTarget::NewTracks, 0);
    assert!(c.is_busy());
    assert_eq!(
        c.import_progress().as_deref(),
        Some("importing audio 0/1"),
        "nothing applied yet: the call returned at once"
    );
    assert!(c.doc().assets.is_empty());
    c.wait_background();
    assert!(!c.is_busy());
    assert!(c.import_progress().is_none());
    assert_eq!(c.doc().assets.len(), 1);
}

#[test]
fn a_different_sample_rate_is_converted_to_the_session_rate() {
    let t = Tmp::new("rate");
    let f = wav(&t, "cd.wav", 44_100, 44_100, 1.0);
    let mut c = core();
    import(&mut c, &[f], ImportTarget::NewTracks, 0);
    let meta = c.doc().assets.values().next().unwrap();
    assert_eq!(meta.sample_rate, 48_000);
    assert!((meta.frames as i64 - 48_000).abs() <= 2, "{}", meta.frames);
}

#[test]
fn undo_steps_are_one_per_existing_track_file_and_two_per_new_track_file() {
    let t = Tmp::new("undo");
    let f = wav(&t, "a.wav", 4_800, 48_000, 1.0);
    let mut c = core();
    import(&mut c, std::slice::from_ref(&f), ImportTarget::NewTracks, 0);
    assert!(c.undo(), "the clip");
    assert!(
        c.doc()
            .song
            .tracks
            .iter()
            .all(|t| t.lanes[0].clips.is_empty())
    );
    assert!(c.undo(), "the track");
    assert!(c.doc().song.tracks.is_empty());
    assert!(!c.undo(), "RegisterAsset is not an undo step");
    assert_eq!(c.doc().assets.len(), 1, "and the pool keeps the asset");
    c.redo();
    c.redo();
    let track = c.doc().song.tracks[0].device.clone();
    import(&mut c, &[f], ImportTarget::ExistingTrack(track), PPQ);
    assert_eq!(audio_clips(&c).len(), 2);
    assert!(c.undo());
    assert_eq!(audio_clips(&c).len(), 1, "one step for the file");
}

#[test]
fn replacing_the_document_discards_imports_in_flight() {
    let t = Tmp::new("discard");
    let f = wav(&t, "a.wav", 48_000, 48_000, 1.0);
    let mut c = core();
    c.import_audio(vec![f], ImportTarget::NewTracks, 0);
    c.new_document();
    c.wait_background();
    assert!(c.doc().assets.is_empty() && c.doc().song.tracks.is_empty());
    assert!(c.peaks().is_empty());
}

// ------------------------------------------------------------------ engine and peaks

#[test]
fn imported_audio_reaches_the_engine_as_memory_assets_and_the_view_as_peaks() {
    let t = Tmp::new("engine");
    let f = wav(&t, "a.wav", 48_000, 48_000, 1.0);
    let mut c = core();
    import(&mut c, &[f], ImportTarget::NewTracks, PPQ);
    let hash = c.doc().assets.keys().next().unwrap().clone();
    let input = last_song(&mut c);
    assert_eq!(input.audio.len(), 1);
    let region = &input.audio[0].regions[0];
    assert_eq!((region.song_start_frame, region.frames), (24_000, 48_000));
    assert_eq!(input.assets.len(), 1);
    assert!(!input.assets[0].is_stream());
    assert_eq!(input.assets[0].frames(), 48_000);
    assert!(c.peaks().contains_key(&hash));
    assert!(c.song_assets().contains_key(&hash));
    // A restart of the engine keeps playing the audio.
    c.rebuild_engine();
    assert_eq!(last_song(&mut c).audio.len(), 1);
}

#[test]
fn the_interface_inputs_of_the_document_resolve_the_audio_track_cables() {
    let t = Tmp::new("inputs");
    let f = wav(&t, "a.wav", 4_800, 48_000, 1.0);
    let mut c = core();
    assert_eq!(engine_config(c.doc()).input_channels, 2);
    import(&mut c, &[f], ImportTarget::NewTracks, 0);
    assert!(c.last_rejection().is_none(), "{:?}", c.last_rejection());
    let interface = c.topology().descriptor(Graph::INTERFACE).unwrap();
    assert!(interface.socket_by_key("in_1").is_some());
    let audio_track = c.device_id(&DeviceKey("audio-track".into())).unwrap();
    assert!(
        c.topology().cables().any(|cable| {
            cable.from.device == Graph::INTERFACE && cable.to.device == audio_track
        }),
        "interface:in_1 is cabled to the track"
    );
}

// ------------------------------------------------------------------ save, open, bundles

fn import_one(
    c: &mut AppCore<RecordingSink>,
    t: &Tmp,
    name: &str,
    frames: usize,
    seed: f32,
) -> String {
    let before: Vec<String> = c.doc().assets.keys().cloned().collect();
    let f = wav(t, name, frames, 48_000, seed);
    import(c, &[f], ImportTarget::NewTracks, 0);
    c.doc()
        .assets
        .keys()
        .find(|h| !before.contains(h))
        .expect("a new asset")
        .clone()
}

#[test]
fn save_as_with_audio_makes_a_bundle_folder_and_copies_the_assets() {
    let t = Tmp::new("saveas");
    let mut c = core();
    let hash = import_one(&mut c, &t, "a.wav", 4_800, 1.0);
    let scratch = c.bundle_root().unwrap().to_path_buf();
    assert!(scratch.starts_with(std::env::temp_dir()));
    let original = std::fs::read(scratch.join("Audio").join(format!("{hash}.wav"))).unwrap();
    let dest = t.path("out");
    std::fs::create_dir_all(&dest).unwrap();
    c.save_as(&dest.join("song.atomik")).unwrap();
    let root = dest.join("song");
    assert_eq!(c.file(), Some(root.join("song.atomik").as_path()));
    assert_eq!(c.bundle_root(), Some(root.as_path()));
    assert!(!c.is_dirty());
    let copied = std::fs::read(root.join("Audio").join(format!("{hash}.wav"))).unwrap();
    assert!(copied == original, "assets are copied bit for bit");
    assert!(!scratch.exists(), "the scratch bundle is gone");
    assert!(!dest.join("song.atomik").exists(), "no stray flat file");
}

#[test]
fn a_project_with_audio_saves_and_reopens_bit_exactly() {
    let t = Tmp::new("roundtrip");
    let mut c = core();
    let hash = import_one(&mut c, &t, "a.wav", 9_600, 1.0);
    c.save_as(&t.path("song.atomik")).unwrap();
    let file = t.path("song").join("song.atomik");
    let text = std::fs::read_to_string(&file).unwrap();
    let wav_path = t.path("song").join("Audio").join(format!("{hash}.wav"));
    let wav_bytes = std::fs::read(&wav_path).unwrap();
    let doc = c.doc().clone();

    let mut other = core();
    other.open(&file).unwrap();
    other.wait_background();
    assert_eq!(other.doc(), &doc);
    assert!(other.missing_assets().is_empty());
    assert!(other.peaks().contains_key(&hash), "peaks fed after open");
    assert_eq!(other.song_assets()[&hash].frames(), 9_600);
    assert_eq!(
        last_song(&mut other).audio.len(),
        1,
        "the song got its audio"
    );
    // Open by folder works too.
    let mut by_folder = core();
    by_folder.open(&t.path("song")).unwrap();
    assert_eq!(by_folder.doc(), &doc);
    // Saving again changes nothing on disk.
    other.save().unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), text);
    assert!(std::fs::read(&wav_path).unwrap() == wav_bytes);
    // And the stored asset still is what its name says.
    let stored = Bundle::open(&t.path("song")).unwrap().load(&hash).unwrap();
    assert_eq!(content_hash(&stored), hash);
}

#[test]
fn projects_without_audio_stay_one_plain_file() {
    let t = Tmp::new("plain");
    let mut c = core();
    c.save_as(&t.path("plain.atomik")).unwrap();
    assert!(t.path("plain.atomik").is_file());
    assert!(!t.path("plain").exists(), "no bundle folder");
    assert!(c.bundle_root().is_none());
}

#[test]
fn save_removes_orphans_but_keeps_assets_the_undo_history_references() {
    let t = Tmp::new("orphans");
    let mut c = core();
    let first = import_one(&mut c, &t, "a.wav", 4_800, 1.0);
    c.save_as(&t.path("song.atomik")).unwrap();
    let root = t.path("song");
    let asset = |h: &str| root.join("Audio").join(format!("{h}.wav"));
    // A stray file nobody references, and a foreign file that is not an asset.
    let stray = "ab".repeat(32);
    std::fs::write(asset(&stray), b"stray").unwrap();
    std::fs::write(root.join("Audio").join("notes.txt"), b"keep me").unwrap();
    // A second asset whose clip is then deleted: undo history still references it.
    let second = import_one(&mut c, &t, "b.wav", 9_600, 0.5);
    let clip = c
        .doc()
        .song
        .tracks
        .iter()
        .flat_map(|t| t.lanes.iter())
        .flat_map(|l| l.clips.iter())
        .find(|k| matches!(&k.content, ClipContent::Audio(a) if a.asset == second))
        .unwrap()
        .id;
    c.apply_command(Command::Song(SongCommand::RemoveClip { id: clip }))
        .unwrap();
    c.save().unwrap();
    assert!(asset(&first).is_file());
    assert!(asset(&second).is_file(), "the undo history still needs it");
    assert!(!asset(&stray).exists(), "orphans are removed");
    assert!(root.join("Audio").join("notes.txt").is_file());
    let saved = std::fs::read_to_string(root.join("song.atomik")).unwrap();
    assert!(
        saved.contains(&first) && !saved.contains(&second),
        "pruned JSON"
    );
    // Undo brings the clip back and it still plays.
    assert!(c.undo());
    assert!(c.song_assets().contains_key(&second));
    // A fresh session has no history: now the unreferenced asset goes.
    let mut fresh = core();
    fresh.open(&root.join("song.atomik")).unwrap();
    fresh.wait_background();
    fresh.save().unwrap();
    assert!(asset(&first).is_file());
    // The saved song only references `first`, so `second`'s file is already gone on disk
    // after the save above; the pool matches the files.
    assert!(!asset(&second).exists());
    assert_eq!(fresh.doc().assets.len(), 1);
}

#[test]
fn removing_the_last_clip_and_saving_in_a_fresh_session_deletes_the_asset() {
    let t = Tmp::new("lastclip");
    let mut c = core();
    let hash = import_one(&mut c, &t, "a.wav", 4_800, 1.0);
    c.save_as(&t.path("song.atomik")).unwrap();
    let file = t.path("song").join("song.atomik");
    let mut fresh = core();
    fresh.open(&file).unwrap();
    fresh.wait_background();
    let id = fresh.doc().song.tracks[0].lanes[0].clips[0].id;
    fresh
        .apply_command(Command::Song(SongCommand::RemoveClip { id }))
        .unwrap();
    fresh.save().unwrap();
    let asset = t.path("song").join("Audio").join(format!("{hash}.wav"));
    assert!(asset.exists(), "this session's undo history still needs it");
    assert!(fresh.undo(), "and undo brings the clip back");
    assert!(fresh.song_assets().contains_key(&hash));
    fresh.undo();
    // The saved project no longer lists it; a new session saving cleans the file up.
    let mut next = core();
    next.open(&file).unwrap();
    next.wait_background();
    assert!(next.doc().assets.is_empty());
    next.save().unwrap();
    assert!(!asset.exists());
    assert!(
        next.peaks().is_empty(),
        "peaks of pruned assets are cleared"
    );
}

#[test]
fn missing_assets_are_reported_and_their_clips_stay_silent() {
    let t = Tmp::new("missing");
    let mut c = core();
    let hash = import_one(&mut c, &t, "a.wav", 4_800, 1.0);
    c.save_as(&t.path("song.atomik")).unwrap();
    let root = t.path("song");
    std::fs::remove_file(root.join("Audio").join(format!("{hash}.wav"))).unwrap();
    let mut other = core();
    other.open(&root.join("song.atomik")).unwrap();
    other.wait_background();
    let missing = other.missing_assets();
    assert_eq!(missing.len(), 1);
    assert_eq!(
        (missing[0].hash.as_str(), missing[0].name.as_str()),
        (hash.as_str(), "a.wav")
    );
    assert!(other.last_rejection().unwrap().contains("a.wav"));
    assert_eq!(audio_clips(&other).len(), 1, "the clip is still there");
    assert!(last_song(&mut other).audio.is_empty(), "and plays silence");
    // Saving does not lose the reference.
    other.save().unwrap();
    let saved = std::fs::read_to_string(root.join("song.atomik")).unwrap();
    assert!(saved.contains(&hash));
    // A project opened without any bundle folder reports everything missing.
    std::fs::remove_dir_all(&root).unwrap();
    std::fs::write(t.path("lone.atomik"), saved).unwrap();
    let mut lone = core();
    lone.open(&t.path("lone.atomik")).unwrap();
    assert_eq!(lone.missing_assets().len(), 1);
}

#[test]
fn a_save_in_place_of_a_flat_project_uses_the_sibling_bundle_folder() {
    let t = Tmp::new("flat");
    let mut c = core();
    c.save_as(&t.path("flat.atomik")).unwrap();
    assert_eq!(c.file(), Some(t.path("flat.atomik").as_path()));
    let hash = import_one(&mut c, &t, "a.wav", 4_800, 1.0);
    c.save().unwrap();
    assert_eq!(
        c.file(),
        Some(t.path("flat.atomik").as_path()),
        "Save never moves the file"
    );
    assert!(
        t.path("flat")
            .join("Audio")
            .join(format!("{hash}.wav"))
            .is_file()
    );
    let mut other = core();
    other.open(&t.path("flat.atomik")).unwrap();
    other.wait_background();
    assert!(other.missing_assets().is_empty());
    assert_eq!(other.doc(), c.doc());
}

#[test]
fn imports_after_opening_a_bundle_write_straight_into_it() {
    let t = Tmp::new("direct");
    let mut c = core();
    import_one(&mut c, &t, "a.wav", 4_800, 1.0);
    c.save_as(&t.path("song.atomik")).unwrap();
    let mut other = core();
    other.open(&t.path("song").join("song.atomik")).unwrap();
    other.wait_background();
    let second = import_one(&mut other, &t, "b.wav", 9_600, 0.5);
    assert_eq!(other.bundle_root(), Some(t.path("song").as_path()));
    assert!(
        t.path("song")
            .join("Audio")
            .join(format!("{second}.wav"))
            .is_file()
    );
}

// ------------------------------------------------------------------ dialog target

#[test]
fn the_dialog_imports_onto_the_selected_audio_track_at_the_snapped_playhead() {
    let t = Tmp::new("dialog");
    let f = wav(&t, "a.wav", 4_800, 48_000, 1.0);
    let mut c = core();
    let (target, tick) = c.import_dialog_target();
    assert_eq!((target, tick), (ImportTarget::NewTracks, 0));
    import(&mut c, &[f], ImportTarget::NewTracks, 0);
    let track = c.doc().song.tracks[0].device.clone();
    c.select(Some(track.clone()));
    c.transport_action(atomik_ui::TransportAction::Locate(PPQ + 5));
    let (target, tick) = c.import_dialog_target();
    assert_eq!(target, ImportTarget::ExistingTrack(track));
    // Snapped to the default 1/16 grid (PPQ / 4 = 3840 ticks): PPQ + 5 rounds to PPQ.
    assert_eq!(tick, PPQ);
    c.select(None);
    assert_eq!(c.import_dialog_target().0, ImportTarget::NewTracks);
}

#[test]
fn the_arrangement_drop_action_imports_through_the_core() {
    let t = Tmp::new("action");
    let f = wav(&t, "a.wav", 4_800, 48_000, 1.0);
    let mut c = core();
    c.handle_action(atomik_ui::UiAction::ImportAudio {
        paths: vec![f],
        target: ImportTarget::NewTracks,
        tick: 0,
    });
    c.wait_background();
    assert_eq!(audio_clips(&c).len(), 1);
}

#[test]
fn the_audio_demo_example_opens_with_its_bundle_and_plays() {
    let file =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/projects/audio-demo.atomik");
    let mut c = core();
    c.open(&file).unwrap();
    c.wait_background();
    assert!(c.missing_assets().is_empty(), "{:?}", c.missing_assets());
    assert!(c.last_rejection().is_none(), "{:?}", c.last_rejection());
    assert_eq!(c.song_assets().len(), 1);
    assert_eq!(c.peaks().len(), 1);
    let input = last_song(&mut c);
    assert_eq!(input.audio.len(), 1);
    assert_eq!(input.audio[0].regions.len(), 3);
}
