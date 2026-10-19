//! Drum machines in the app core (app part): pad and background
//! drops, sample maps, the step editor pane, creating a pattern, step edits reaching the
//! engine song, individual outputs through console channels (live twin of the bounce), and
//! missing pad assets. Fixtures are procedural; the engine is the recording sink or the offline
//! sink.

use super::*;
use crate::sink::{RecordingSink, SinkCall};
use atomik_audio_file::{AudioData, write_wav_f32};
use atomik_core::PPQ;
use atomik_device_sdk::SampleMapRef;
use atomik_engine::SongInput;
use atomik_model::{
    Clip, ClipContent, ConsoleCommand, Note, StepGrid, StepResolution, ZoneDoc, empty_doc, pad_key,
    pad_zones,
};
use atomik_ui::kurbo::Rect;
use atomik_ui::{Key, Modifiers, PointerButton};
use std::sync::atomic::{AtomicU64, Ordering};

const DRUMS: &str = "atomik.drum-machine";
const BAR: i64 = 4 * PPQ;

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("atomik-app-drums-{tag}-{}-{n}", std::process::id()));
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

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

/// A mono ramp of `frames` frames; `seed` makes distinct audio (so distinct assets).
fn wav(dir: &Tmp, name: &str, frames: usize, seed: f32) -> PathBuf {
    let data: Vec<f32> = (0..frames)
        .map(|i| (i as f32 / frames as f32) * seed)
        .collect();
    let path = dir.path(name);
    write_wav_f32(&path, &AudioData::new(48_000, vec![data]).unwrap()).unwrap();
    path
}

fn drum_core() -> AppCore<RecordingSink> {
    let mut c = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: DRUMS.into(),
        key: Some(key("drm")),
    })
    .unwrap();
    c.sink_mut().take();
    c
}

fn zones(c: &AppCore<RecordingSink>) -> Vec<ZoneDoc> {
    c.doc().device(&key("drm")).unwrap().zones.clone()
}

fn pad_files(c: &AppCore<RecordingSink>, pad: u8) -> Vec<(u8, u8, String)> {
    let z = zones(c);
    pad_zones(&z, pad)
        .map(|i| (z[i].vel_lo, z[i].vel_hi, c.asset_name(&z[i].asset)))
        .collect()
}

fn drop_files(c: &mut AppCore<RecordingSink>, paths: Vec<PathBuf>, at_key: Option<u8>) {
    c.handle_action(UiAction::ImportSamples {
        paths,
        device: key("drm"),
        at_key,
    });
    c.wait_background();
}

type SamplesCall = (DeviceInstanceId, Option<SampleMapRef>);

fn sample_calls(calls: &[SinkCall]) -> Vec<SamplesCall> {
    calls
        .iter()
        .filter_map(|c| match c {
            SinkCall::Samples { id, map } => Some((*id, map.clone())),
            _ => None,
        })
        .collect()
}

fn songs(calls: &[SinkCall]) -> Vec<&SongInput> {
    calls
        .iter()
        .filter_map(|c| match c {
            SinkCall::Song(s) => Some(s),
            _ => None,
        })
        .collect()
}

fn note_clip(start: i64, length: i64, notes: Vec<Note>) -> Clip {
    Clip {
        id: atomik_model::ClipId(0),
        start,
        length,
        offset: 0,
        loop_length: None,
        muted: false,
        grid: None,
        content: ClipContent::Notes(notes),
    }
}

fn add_track_and_clip(c: &mut AppCore<RecordingSink>, device: &str, clip: Clip) -> ClipId {
    c.apply_command(Command::Song(SongCommand::AddTrack {
        device: key(device),
    }))
    .unwrap();
    c.apply_command(Command::Song(SongCommand::AddClip {
        device: key(device),
        lane: 0,
        clip,
    }))
    .unwrap();
    c.document().last_clip_id().unwrap()
}

fn add_synth(c: &mut AppCore<RecordingSink>) {
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: "atomik.poly-synth".into(),
        key: Some(key("syn")),
    })
    .unwrap();
}

// ------------------------------------------------------------------ pad loading

#[test]
fn a_drop_on_a_pad_replaces_that_pad_in_one_undo_step_with_one_map_each_time() {
    let t = Tmp::new("pad");
    let mut c = drum_core();
    drop_files(
        &mut c,
        vec![wav(&t, "kick_a.wav", 2_400, 1.0)],
        Some(pad_key(1)),
    );
    assert!(c.last_rejection().is_none(), "{:?}", c.last_rejection());
    assert_eq!(pad_files(&c, 1), [(1, 127, "kick_a.wav".to_string())]);
    let z = zones(&c);
    assert_eq!((z[0].key_lo, z[0].key_hi, z[0].root_key), (36, 36, 36));
    let calls = c.sink_mut().take();
    let sent = sample_calls(&calls);
    assert_eq!(sent.len(), 1, "{calls:?}");
    assert_eq!(sent[0].1.as_ref().unwrap().zones().len(), 1);
    assert_eq!(Some(sent[0].0), c.device_id(&key("drm")));

    // A second drop on the same pad replaces it (not appends); another pad adds.
    drop_files(
        &mut c,
        vec![wav(&t, "kick_b.wav", 2_400, 0.5)],
        Some(pad_key(1)),
    );
    assert_eq!(pad_files(&c, 1), [(1, 127, "kick_b.wav".to_string())]);
    assert_eq!(zones(&c).len(), 1);
    assert_eq!(sample_calls(&c.sink_mut().take()).len(), 1);
    drop_files(
        &mut c,
        vec![wav(&t, "snare.wav", 2_400, 0.25)],
        Some(pad_key(2)),
    );
    assert_eq!(zones(&c).len(), 2);
    c.sink_mut().take();

    // Each drop is one undo step, and the engine gets the matching map.
    assert!(c.undo());
    assert_eq!(zones(&c).len(), 1);
    let sent = sample_calls(&c.sink_mut().take());
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].1.as_ref().unwrap().zones().len(), 1);
    assert!(c.undo());
    assert_eq!(pad_files(&c, 1), [(1, 127, "kick_a.wav".to_string())]);
    assert_eq!(sample_calls(&c.sink_mut().take()).len(), 1);
    assert!(c.redo());
    assert_eq!(pad_files(&c, 1), [(1, 127, "kick_b.wav".to_string())]);
}

#[test]
fn several_files_on_a_pad_become_velocity_layers_in_one_step() {
    let t = Tmp::new("layers");
    let mut c = drum_core();
    drop_files(
        &mut c,
        vec![
            wav(&t, "snare_hard.wav", 2_400, 1.0),
            wav(&t, "snare_a_soft.wav", 2_400, 0.5),
            wav(&t, "snare_mid.wav", 2_400, 0.7),
        ],
        Some(pad_key(3)),
    );
    // In file-name byte order, velocities split evenly: 1..=43, 44..=85, 86..=127.
    assert_eq!(
        pad_files(&c, 3),
        [
            (1, 43, "snare_a_soft.wav".to_string()),
            (44, 85, "snare_hard.wav".to_string()),
            (86, 127, "snare_mid.wav".to_string()),
        ]
    );
    let sent = sample_calls(&c.sink_mut().take());
    assert_eq!(sent.len(), 1, "one map for the whole drop");
    assert_eq!(sent[0].1.as_ref().unwrap().zones().len(), 3);
    assert!(c.undo());
    assert!(zones(&c).is_empty(), "one undo removes every layer");
    assert!(c.doc().device(&key("drm")).is_some());
}

#[test]
fn a_background_drop_fills_the_empty_pads_in_order_in_one_step() {
    let t = Tmp::new("bg");
    let mut c = drum_core();
    drop_files(
        &mut c,
        vec![wav(&t, "kick.wav", 2_400, 1.0)],
        Some(pad_key(1)),
    );
    c.sink_mut().take();
    drop_files(
        &mut c,
        vec![
            wav(&t, "c_clap.wav", 2_400, 0.5),
            wav(&t, "b_snare.wav", 2_400, 0.25),
        ],
        None,
    );
    assert!(c.last_rejection().is_none(), "{:?}", c.last_rejection());
    // Pad 1 is taken: the files (in name order) go to pads 2 and 3.
    assert_eq!(pad_files(&c, 1), [(1, 127, "kick.wav".to_string())]);
    assert_eq!(pad_files(&c, 2), [(1, 127, "b_snare.wav".to_string())]);
    assert_eq!(pad_files(&c, 3), [(1, 127, "c_clap.wav".to_string())]);
    assert_eq!(sample_calls(&c.sink_mut().take()).len(), 1);
    assert!(c.undo());
    assert_eq!(zones(&c).len(), 1, "the whole fill is one step");
    // A key outside the pads counts as the background.
    drop_files(&mut c, vec![wav(&t, "x.wav", 2_400, 0.1)], Some(60));
    assert_eq!(pad_files(&c, 2), [(1, 127, "x.wav".to_string())]);
}

#[test]
fn files_with_no_empty_pad_left_are_reported_by_name_and_count() {
    let t = Tmp::new("full");
    let mut c = drum_core();
    let paths: Vec<PathBuf> = (0..18)
        .map(|i| wav(&t, &format!("f{i:02}.wav"), 600 + i, 0.1 + i as f32 * 0.01))
        .collect();
    drop_files(&mut c, paths, None);
    assert_eq!(zones(&c).len(), 16);
    let message = c.last_rejection().unwrap();
    assert!(message.contains("2 file"), "{message}");
    assert!(
        message.contains("f16.wav") && message.contains("f17.wav"),
        "{message}"
    );
    assert!(!message.contains("f15.wav"), "{message}");
    // One undo step took them all.
    assert!(c.undo());
    assert!(zones(&c).is_empty());
}

#[test]
fn a_failing_file_is_reported_and_the_others_still_load_on_the_pad() {
    let t = Tmp::new("fail");
    let mut c = drum_core();
    let bad = t.path("broken.wav");
    std::fs::write(&bad, b"not audio").unwrap();
    drop_files(
        &mut c,
        vec![bad, wav(&t, "good.wav", 2_400, 1.0)],
        Some(pad_key(5)),
    );
    assert_eq!(pad_files(&c, 5), [(1, 127, "good.wav".to_string())]);
    assert!(c.last_rejection().unwrap().contains("broken.wav"));
    // Every file failing adds nothing and no undo step.
    let before = c.document().can_undo();
    let junk = t.path("junk.wav");
    std::fs::write(&junk, b"nope").unwrap();
    drop_files(&mut c, vec![junk], Some(pad_key(6)));
    assert!(pad_files(&c, 6).is_empty());
    assert_eq!(c.document().can_undo(), before);
}

#[test]
fn a_sampler_still_maps_by_note_names() {
    let t = Tmp::new("sampler");
    let mut c = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: "atomik.sampler".into(),
        key: Some(key("drm")),
    })
    .unwrap();
    drop_files(
        &mut c,
        vec![wav(&t, "tone_C3.wav", 2_400, 1.0)],
        Some(pad_key(2)),
    );
    // A sampler takes `at_key` as the base key of an unnamed file only; this one has a name.
    assert_eq!(zones(&c)[0].root_key, 48);
}

// ------------------------------------------------------------------ maps to the engine

fn saved_drums(t: &Tmp) -> (PathBuf, Vec<String>) {
    let mut c = drum_core();
    drop_files(
        &mut c,
        vec![wav(t, "kick.wav", 4_800, 1.0)],
        Some(pad_key(1)),
    );
    drop_files(
        &mut c,
        vec![wav(t, "snare.wav", 2_400, 0.5)],
        Some(pad_key(2)),
    );
    let hashes: Vec<String> = zones(&c).iter().map(|z| z.asset.clone()).collect();
    c.save_as(&t.path("song.atomik")).unwrap();
    (t.path("song").join("song.atomik"), hashes)
}

#[test]
fn open_and_restart_send_the_drum_machine_its_map_after_its_insertion() {
    let t = Tmp::new("restart");
    let (file, _) = saved_drums(&t);
    let mut c = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    c.open(&file).unwrap();
    c.wait_background();
    let id = c.device_id(&key("drm")).unwrap();
    let calls = c.sink_mut().take();
    let insert = calls
        .iter()
        .rposition(|x| matches!(x, SinkCall::Insert { id: i, .. } if *i == id))
        .unwrap();
    let samples = calls
        .iter()
        .rposition(|x| matches!(x, SinkCall::Samples { id: i, map: Some(_) } if *i == id))
        .expect("a map after the assets loaded");
    assert!(insert < samples, "insert, then samples: {calls:?}");
    assert_eq!(
        sample_calls(&calls).pop().unwrap().1.unwrap().zones().len(),
        2
    );

    c.rebuild_engine();
    let calls = c.sink_mut().take();
    let id = c.device_id(&key("drm")).unwrap();
    let insert = calls
        .iter()
        .position(|x| matches!(x, SinkCall::Insert { id: i, .. } if *i == id))
        .unwrap();
    let samples = calls
        .iter()
        .position(|x| matches!(x, SinkCall::Samples { id: i, map: Some(_) } if *i == id))
        .unwrap();
    assert!(insert < samples, "{calls:?}");
    assert_eq!(sample_calls(&calls).len(), 1);
}

#[test]
fn undoing_the_removal_of_a_drum_machine_inserts_it_and_sends_its_map() {
    let t = Tmp::new("undo-remove");
    let mut c = drum_core();
    drop_files(
        &mut c,
        vec![wav(&t, "kick.wav", 2_400, 1.0)],
        Some(pad_key(1)),
    );
    c.apply_command(Command::RemoveDevice { key: key("drm") })
        .unwrap();
    c.sink_mut().take();
    assert!(c.undo());
    let calls = c.sink_mut().take();
    let id = c.device_id(&key("drm")).unwrap();
    let insert = calls
        .iter()
        .position(|x| matches!(x, SinkCall::Insert { id: i, .. } if *i == id))
        .expect("inserted again");
    let samples = calls
        .iter()
        .position(|x| matches!(x, SinkCall::Samples { id: i, map: Some(_) } if *i == id))
        .expect("and given its map");
    assert!(insert < samples, "{calls:?}");
    assert_eq!(sample_calls(&calls).len(), 1, "no duplicate map");
}

#[test]
fn a_zone_edit_on_a_drum_machine_is_exactly_one_map_swap_and_a_drum_machine_without_zones_gets_none()
 {
    let t = Tmp::new("edit");
    let mut c = drum_core();
    c.rebuild_engine();
    assert!(
        sample_calls(&c.sink_mut().take()).is_empty(),
        "no zones, no call"
    );
    drop_files(
        &mut c,
        vec![wav(&t, "kick.wav", 2_400, 1.0)],
        Some(pad_key(1)),
    );
    c.sink_mut().take();
    let mut edited = zones(&c);
    edited[0].gain_db = -6.0;
    c.handle_action(UiAction::Command(Command::SetZones {
        device: key("drm"),
        zones: edited,
        merge: None,
    }));
    let calls = c.sink_mut().take();
    let sent = sample_calls(&calls);
    assert_eq!(sent.len(), 1, "{calls:?}");
    assert!((sent[0].1.as_ref().unwrap().zones()[0].gain_db + 6.0).abs() < 1e-6);
}

#[test]
fn a_loaded_streamed_asset_reaches_the_drum_machine_when_it_arrives() {
    let t = Tmp::new("stream");
    let mut sink = RecordingSink::new();
    sink.stream_threshold = Some(1_000);
    let mut c = AppCore::with_doc(sink, empty_doc(), None);
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: DRUMS.into(),
        key: Some(key("drm")),
    })
    .unwrap();
    c.import_audio(
        vec![wav(&t, "long.wav", 48_000, 1.0)],
        atomik_ui::ImportTarget::NewTracks,
        0,
    );
    c.wait_background();
    let hash = c.doc().assets.keys().next().unwrap().clone();
    c.sink_mut().take();
    let mut z = ZoneDoc::new(hash, 48_000);
    (z.key_lo, z.key_hi, z.root_key) = (36, 36, 36);
    c.apply_command(Command::SetZones {
        device: key("drm"),
        zones: vec![z],
        merge: None,
    })
    .unwrap();
    let first = sample_calls(&c.sink_mut().take());
    assert!(first[0].1.is_none(), "silent until the RAM copy arrives");
    c.wait_background();
    let later = sample_calls(&c.sink_mut().take());
    assert_eq!(later.len(), 1, "{later:?}");
    assert_eq!(later[0].1.as_ref().unwrap().buffers()[0].frames, 48_000);
}

// ------------------------------------------------------------------ missing pad assets

#[test]
fn a_pad_whose_asset_is_missing_plays_silence_and_is_reported() {
    let t = Tmp::new("missing");
    let (file, hashes) = saved_drums(&t);
    std::fs::remove_file(
        t.path("song")
            .join("Audio")
            .join(format!("{}.wav", hashes[0])),
    )
    .unwrap();
    let mut c = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    c.open(&file).unwrap();
    c.wait_background();
    let missing: Vec<&str> = c.missing_assets().iter().map(|m| m.hash.as_str()).collect();
    assert_eq!(missing, [hashes[0].as_str()]);
    assert!(c.last_rejection().unwrap().contains("kick.wav"));
    assert_eq!(zones(&c).len(), 2, "both zones stay in the document");
    // The engine's map holds only the pad whose audio exists.
    let sent = sample_calls(&c.sink_mut().take());
    let map = sent.last().unwrap().1.clone().unwrap();
    assert_eq!(map.zones().len(), 1);
    assert_eq!((map.zones()[0].key_lo, map.zones()[0].key_hi), (37, 37));
    // Both assets missing: no map, silence, still two zones.
    std::fs::remove_file(
        t.path("song")
            .join("Audio")
            .join(format!("{}.wav", hashes[1])),
    )
    .unwrap();
    let mut c = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    c.open(&file).unwrap();
    c.wait_background();
    assert_eq!(c.missing_assets().len(), 2);
    assert!(
        sample_calls(&c.sink_mut().take())
            .iter()
            .all(|(_, m)| m.is_none())
    );
}

// ------------------------------------------------------------------ the step editor pane

fn step_core() -> (AppCore<RecordingSink>, ClipId) {
    let mut c = drum_core();
    let id = add_track_and_clip(&mut c, "drm", note_clip(BAR, BAR, Vec::new()));
    c.sink_mut().take();
    (c, id)
}

#[test]
fn the_step_editor_opens_on_a_drum_machine_and_resolves_its_clip() {
    let (mut c, id) = step_core();
    assert!(c.step_editor_device().is_none());
    // No selection, the playhead outside the clip: the empty state.
    c.handle_action(UiAction::OpenStepEditor {
        device: key("drm"),
        clip: None,
    });
    assert_eq!(c.step_editor_device(), Some(&key("drm")));
    assert!(c.step_editor().clip().is_none());
    // The playhead inside the clip: the clip under it.
    c.transport_action(atomik_ui::TransportAction::Locate(BAR + 10));
    c.handle_action(UiAction::OpenStepEditor {
        device: key("drm"),
        clip: None,
    });
    assert_eq!(c.step_editor().clip(), Some(id));
    // The selected clip wins.
    c.transport_action(atomik_ui::TransportAction::Locate(0));
    c.arrangement_mut().set_selected_clips(&[id]);
    c.handle_action(UiAction::OpenStepEditor {
        device: key("drm"),
        clip: None,
    });
    assert_eq!(c.step_editor().clip(), Some(id));
    c.handle_action(UiAction::CloseStepEditor);
    assert!(c.step_editor_device().is_none());
    // Not a drum machine: ignored.
    add_synth(&mut c);
    c.handle_action(UiAction::OpenStepEditor {
        device: key("syn"),
        clip: None,
    });
    assert!(c.step_editor_device().is_none());
    // The default target is the first drum machine.
    assert!(c.open_step_editor_default());
    assert_eq!(c.step_editor_device(), Some(&key("drm")));
}

#[test]
fn the_step_editor_excludes_the_piano_roll_and_the_sample_editor() {
    let (mut c, id) = step_core();
    add_synth(&mut c);
    let syn_clip = add_track_and_clip(&mut c, "syn", note_clip(0, BAR, Vec::new()));
    c.open_piano_roll(syn_clip);
    c.open_step_editor(key("drm"), Some(id));
    assert!(
        c.piano_roll_clip().is_none(),
        "the step editor replaced the roll"
    );
    assert!(c.step_editor_device().is_some());
    c.open_sample_editor(key("drm"));
    assert!(
        c.step_editor_device().is_none(),
        "the sample editor replaced it"
    );
    assert!(c.sample_editor_device().is_some());
    c.open_step_editor(key("drm"), Some(id));
    assert!(c.sample_editor_device().is_none());
    c.open_piano_roll(syn_clip);
    assert!(
        c.step_editor_device().is_none(),
        "the roll replaced the step editor"
    );
    // A new document closes it.
    c.open_step_editor(key("drm"), Some(id));
    c.new_document();
    assert!(c.step_editor_device().is_none());
}

#[test]
fn the_pane_closes_when_its_drum_machine_or_clip_goes_away() {
    let (mut c, id) = step_core();
    c.open_step_editor(key("drm"), Some(id));
    c.apply_command(Command::Song(SongCommand::RemoveClip { id }))
        .unwrap();
    assert!(c.step_editor_device().is_none(), "the clip is gone");
    c.undo();
    assert!(c.step_editor_device().is_none(), "and stays closed on undo");
    c.open_step_editor(key("drm"), Some(id));
    c.apply_command(Command::RemoveDevice { key: key("drm") })
        .unwrap();
    assert!(c.step_editor_device().is_none());
}

#[test]
fn a_double_click_on_a_drum_clip_opens_the_step_editor_and_other_clips_the_roll() {
    let (mut c, id) = step_core();
    add_synth(&mut c);
    let syn_clip = add_track_and_clip(&mut c, "syn", note_clip(0, BAR, Vec::new()));
    // The arrangement emits OpenPianoRoll for a double-click on any note clip.
    c.handle_action(UiAction::OpenPianoRoll(id));
    assert_eq!(c.step_editor().clip(), Some(id));
    assert!(c.piano_roll_clip().is_none());
    c.handle_action(UiAction::OpenPianoRoll(syn_clip));
    assert_eq!(c.piano_roll_clip(), Some(syn_clip));
    assert!(c.step_editor_device().is_none());
    // View > Piano Roll still opens a drum clip in the piano roll.
    c.arrangement_mut().set_selected_clips(&[id]);
    assert!(c.open_piano_roll_default());
    assert_eq!(c.piano_roll_clip(), Some(id));
    assert!(c.step_editor_device().is_none());
}

// ------------------------------------------------------------------ creating a pattern

fn clip_of(c: &AppCore<RecordingSink>, id: ClipId) -> Clip {
    c.doc().song.clip(id).unwrap().2.clone()
}

#[test]
fn edit_with_no_clip_creates_a_one_bar_clip_with_a_grid_in_one_step_when_the_track_exists() {
    let mut c = drum_core();
    c.apply_command(Command::Song(SongCommand::AddTrack { device: key("drm") }))
        .unwrap();
    c.transport_action(atomik_ui::TransportAction::Locate(2 * BAR + 5_000));
    c.handle_action(UiAction::OpenStepEditor {
        device: key("drm"),
        clip: None,
    });
    assert!(c.step_editor().clip().is_none(), "the empty state");
    c.sink_mut().take();
    c.handle_action(UiAction::CreateStepClip { device: key("drm") });
    let id = c
        .step_editor()
        .clip()
        .expect("the editor shows the new clip");
    let clip = clip_of(&c, id);
    assert_eq!(clip.start, 2 * BAR, "the playhead snapped to the bar");
    assert_eq!(clip.length, BAR);
    assert_eq!(
        clip.grid,
        Some(StepGrid {
            resolution: StepResolution::Sixteenth,
            steps: 16,
            swing: 0
        })
    );
    assert!(matches!(&clip.content, ClipContent::Notes(n) if n.is_empty()));
    assert_eq!(c.arrangement().selected_clips(), [id]);
    assert_eq!(songs(&c.sink_mut().take()).len(), 1, "one song resubmit");
    // One undo step removes the clip (the track was there before).
    assert!(c.undo());
    assert!(c.doc().song.clip(id).is_none());
    assert!(c.doc().song.track(&key("drm")).is_some());
    assert!(
        c.step_editor_device().is_none(),
        "the clip is gone: the pane closes"
    );
    assert!(c.redo());
    assert_eq!(clip_of(&c, id).grid.unwrap().steps, 16);
}

#[test]
fn creating_a_pattern_without_a_track_is_one_undo_step() {
    let mut c = drum_core();
    c.handle_action(UiAction::CreateStepClip { device: key("drm") });
    let id = c.step_editor().clip().unwrap();
    assert_eq!(clip_of(&c, id).start, 0);
    // A batch of AddTrack and AddClip: one undo step takes the clip and the track.
    assert!(c.undo());
    assert!(c.doc().song.clip(id).is_none());
    assert!(c.doc().song.track(&key("drm")).is_none());
    assert!(
        c.doc().device(&key("drm")).is_some(),
        "only the device add is left"
    );
    assert!(c.redo());
    assert!(c.doc().song.clip(id).is_some());
    assert!(c.doc().song.track(&key("drm")).is_some());
}

#[test]
fn creating_a_pattern_snaps_to_the_nearest_bar_and_needs_a_drum_machine() {
    let mut c = drum_core();
    c.transport_action(atomik_ui::TransportAction::Locate(BAR + BAR / 2 + 1));
    c.handle_action(UiAction::CreateStepClip { device: key("drm") });
    assert_eq!(clip_of(&c, c.step_editor().clip().unwrap()).start, 2 * BAR);
    add_synth(&mut c);
    let steps = c.document().can_undo();
    c.handle_action(UiAction::CreateStepClip { device: key("syn") });
    assert!(c.last_rejection().unwrap().contains("not a drum machine"));
    assert!(c.doc().song.track(&key("syn")).is_none());
    assert_eq!(c.document().can_undo(), steps);
}

// ------------------------------------------------------------------ step edits reach the engine

fn press(pos: atomik_ui::kurbo::Point) -> UiEvent {
    UiEvent::PointerDown {
        pos,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    }
}

fn release(pos: atomik_ui::kurbo::Point) -> UiEvent {
    UiEvent::PointerUp {
        pos,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
    }
}

#[test]
fn step_editor_edits_reach_the_document_and_the_engine_song() {
    let mut c = drum_core();
    c.handle_action(UiAction::CreateStepClip { device: key("drm") });
    let id = c.step_editor().clip().unwrap();
    c.step_editor_mut()
        .set_bounds(Rect::new(0.0, 0.0, 1000.0, 600.0));
    c.after_view_change();
    c.sink_mut().take();
    // A click on pad 1, step 4 adds the note; the song is resubmitted with it.
    let at = c.step_editor().cell_center(1, 4);
    c.handle_pane_event(Pane::StepEditor, &press(at));
    c.handle_pane_event(Pane::StepEditor, &release(at));
    let notes = |c: &AppCore<RecordingSink>| match clip_of(c, id).content {
        ClipContent::Notes(n) => n,
        _ => unreachable!(),
    };
    assert_eq!(notes(&c).len(), 1);
    assert_eq!((notes(&c)[0].key, notes(&c)[0].tick), (36, 4 * 3840));
    let calls = c.sink_mut().take();
    let sent = songs(&calls);
    assert!(!sent.is_empty(), "{calls:?}");
    let last = sent.last().unwrap();
    assert_eq!(
        last.tracks[0]
            .notes
            .iter()
            .map(|n| (n.key, n.tick.0))
            .collect::<Vec<_>>(),
        [(36, 4 * 3840)]
    );
    // A second click on the same cell removes it again: still one undo step each.
    c.handle_pane_event(Pane::StepEditor, &press(at));
    c.handle_pane_event(Pane::StepEditor, &release(at));
    assert!(notes(&c).is_empty());
    assert!(c.undo());
    assert_eq!(notes(&c).len(), 1);
    // Ctrl+Z through the pane's keys undoes too.
    let undo = UiEvent::Key {
        key: Key::Char('z'),
        modifiers: Modifiers {
            ctrl: true,
            ..Modifiers::NONE
        },
        pressed: true,
    };
    c.handle_pane_event(Pane::StepEditor, &undo);
    assert!(notes(&c).is_empty());
}

#[test]
fn a_swing_edit_retimes_the_notes_in_the_engine_song_without_touching_the_document_notes() {
    let mut c = drum_core();
    let notes: Vec<Note> = [(0i64, 36u8), (1, 36), (2, 36), (3, 36)]
        .iter()
        .map(|&(step, key)| Note {
            tick: step * 3840,
            length: 3840,
            key,
            velocity: 0.8,
        })
        .collect();
    let id = add_track_and_clip(&mut c, "drm", note_clip(0, BAR, notes.clone()));
    c.sink_mut().take();
    c.handle_action(UiAction::Command(Command::Song(SongCommand::SetClipGrid {
        clip: id,
        grid: Some(StepGrid {
            resolution: StepResolution::Sixteenth,
            steps: 16,
            swing: 50,
        }),
        merge: None,
    })));
    let calls = c.sink_mut().take();
    let ticks: Vec<i64> = songs(&calls).last().unwrap().tracks[0]
        .notes
        .iter()
        .map(|n| n.tick.0)
        .collect();
    // Odd steps move by (50 * 3840 + 100) / 200 = 960 ticks.
    assert_eq!(ticks, [0, 3840 + 960, 2 * 3840, 3 * 3840 + 960]);
    match clip_of(&c, id).content {
        ClipContent::Notes(n) => assert_eq!(n, notes, "the document notes are unchanged"),
        _ => unreachable!(),
    }
}

// ------------------------------------------------------------------ individual outputs: live = bounce

fn demo_file() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/projects/drum-demo/drum-demo.atomik")
}

/// The engine's command FIFO delays the live transport by one batch (see the sampler twin).
const LATENCY: usize = atomik_core::BATCH;

fn bounce_of(doc: &atomik_model::RackDoc) -> Vec<Vec<f32>> {
    let root = demo_file().parent().unwrap().to_path_buf();
    atomik_render::project::render_doc_in(
        doc,
        &default_registry(),
        &atomik_render::project::ProjectOptions::default(),
        Some(&root),
    )
    .unwrap()
    .channels
}

fn live_core() -> AppCore<crate::offline_sink::OfflineSink> {
    let mut core = AppCore::with_doc(crate::offline_sink::OfflineSink::new(), empty_doc(), None);
    core.open(&demo_file()).unwrap();
    core.wait_background();
    assert!(core.missing_assets().is_empty());
    assert!(
        core.last_rejection().is_none(),
        "{:?}",
        core.last_rejection()
    );
    core
}

fn snare_channel(doc: &atomik_model::RackDoc) -> DeviceKey {
    doc.console
        .channels
        .iter()
        .find(|c| doc.device(&c.channel).is_some_and(|d| d.name == "Snare"))
        .unwrap()
        .channel
        .clone()
}

fn set_param(
    core: &mut AppCore<crate::offline_sink::OfflineSink>,
    device: &DeviceKey,
    p: &str,
    v: f32,
) {
    core.apply_command(Command::SetParam {
        key: device.clone(),
        param: p.into(),
        value: v,
        merge: None,
    })
    .unwrap();
}

fn play(
    core: &mut AppCore<crate::offline_sink::OfflineSink>,
    frames: usize,
    block: usize,
) -> Vec<Vec<f32>> {
    core.transport_action(atomik_ui::TransportAction::Play);
    core.sink_mut().render_blocks(frames + LATENCY, block)
}

fn same_bits(live: &[Vec<f32>], bounce: &[Vec<f32>]) -> bool {
    live.iter().zip(bounce).all(|(a, b)| {
        a[LATENCY..]
            .iter()
            .zip(b)
            .all(|(x, y)| x.to_bits() == y.to_bits())
            && a[..LATENCY].iter().all(|x| *x == 0.0)
    })
}

fn rms(channel: &[f32], from: usize, to: usize) -> f64 {
    let w = &channel[from..to.min(channel.len())];
    (w.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / w.len().max(1) as f64).sqrt()
}

/// The drum demo (swung steps, a hat choke group, the snare through its own console channel)
/// played through the app's live path gives the bounce's samples, bit for bit, delayed by
/// the engine batch, whatever the host block.
#[test]
fn the_live_path_plays_the_drum_demo_exactly_like_the_bounce() {
    let reference = bounce_of(live_core().doc());
    let frames = reference[0].len();
    assert!(reference[0].iter().any(|s| s.abs() > 1e-3));
    for block in [64, 480, 512] {
        let mut core = live_core();
        let live = play(&mut core, frames, block);
        assert!(same_bits(&live, &reference), "block {block}");
    }
}

#[test]
fn muting_the_snare_channel_silences_only_the_snare_through_the_live_path() {
    let mut core = live_core();
    let channel = snare_channel(core.doc());
    set_param(&mut core, &channel, "mute", 1.0);
    let doc = core.doc().clone();
    let reference = bounce_of(&doc);
    let frames = reference[0].len();
    let live = play(&mut core, frames, 512);
    assert!(
        same_bits(&live, &reference),
        "live = bounce with the channel muted"
    );
    // Only the snare is gone: the same as a song without snare notes (the mix is a sum, so
    // the two differ by float rounding at most).
    let mut no_snare = live_core().doc().clone();
    for t in &mut no_snare.song.tracks {
        for clip in &mut t.lanes[0].clips {
            if let ClipContent::Notes(n) = &mut clip.content {
                n.retain(|n| n.key != pad_key(3));
            }
        }
    }
    let expected = bounce_of(&no_snare);
    for (a, b) in reference.iter().zip(&expected) {
        let worst = a
            .iter()
            .zip(b)
            .fold(0.0f32, |m, (x, y)| m.max((x - y).abs()));
        assert!(worst < 2e-5, "only the snare went: {worst}");
    }
}

#[test]
fn soloing_the_snare_channel_leaves_only_the_snare_through_the_live_path() {
    let mut core = live_core();
    let unsoloed = bounce_of(core.doc());
    let channel = snare_channel(core.doc());
    set_param(&mut core, &channel, "solo", 1.0);
    let reference = bounce_of(core.doc());
    let frames = reference[0].len();
    let live = play(&mut core, frames, 480);
    assert!(same_bits(&live, &reference), "live = bounce with the solo");
    // Before the first snare (step 4 is half a second in) the kick and hats are silenced; the
    // snare itself still sounds.
    assert!(rms(&unsoloed[0], 0, 20_000) > 1e-2);
    assert!(
        rms(&reference[0], 0, 20_000) < 1e-5,
        "{}",
        rms(&reference[0], 0, 20_000)
    );
    assert!(rms(&reference[0], 24_000, 30_000) > 1e-3, "the snare plays");
}

#[test]
fn the_to_channel_command_gives_a_pad_its_own_channel_and_muting_it_silences_only_that_pad() {
    // The command the drum panel's TO CH button emits, applied through the app (kick = pad 1).
    let mut core = live_core();
    let channels = core.doc().console.channels.len();
    core.handle_action(UiAction::Command(Command::Console(
        ConsoleCommand::AddChannelFor {
            source: key("drums"),
            out_l: "pad1_out_l".into(),
            out_r: "pad1_out_r".into(),
            name: None,
        },
    )));
    assert!(
        core.last_rejection().is_none(),
        "{:?}",
        core.last_rejection()
    );
    assert_eq!(core.doc().console.channels.len(), channels + 1);
    let channel = core.doc().console.channels.last().unwrap().channel.clone();
    assert_eq!(core.doc().device(&channel).unwrap().name, "Drums Pad 1");
    set_param(&mut core, &channel, "mute", 1.0);
    let reference = bounce_of(&core.doc().clone());
    let frames = reference[0].len();
    let live = play(&mut core, frames, 512);
    assert!(same_bits(&live, &reference), "live = bounce");
    let mut no_kick = live_core().doc().clone();
    for t in &mut no_kick.song.tracks {
        for clip in &mut t.lanes[0].clips {
            if let ClipContent::Notes(n) = &mut clip.content {
                n.retain(|n| n.key != pad_key(1));
            }
        }
    }
    let expected = bounce_of(&no_kick);
    for (a, b) in reference.iter().zip(&expected) {
        let worst = a
            .iter()
            .zip(b)
            .fold(0.0f32, |m, (x, y)| m.max((x - y).abs()));
        assert!(worst < 2e-5, "only the kick went: {worst}");
    }
}
