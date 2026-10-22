//! Samplers in the app core: sample maps to the engine, importing samples
//! as zones in one undo step, missing assets, saving and reopening, auditions and the sample
//! editor pane. Every fixture is procedural and lives in a temporary folder; the engine is the
//! recording sink.

use super::*;
use crate::sink::{RecordingSink, SinkCall};
use atomik_audio_file::{AudioData, write_wav_f32};
use atomik_device_sdk::SampleMapRef;
use atomik_engine::SongAsset;
use atomik_model::{ZoneDoc, empty_doc};
use atomik_ui::ImportTarget;
use std::sync::atomic::{AtomicU64, Ordering};

const SAMPLER: &str = "atomik.sampler";

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "atomik-app-samples-{tag}-{}-{n}",
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

fn sampler_core_with(sink: RecordingSink) -> AppCore<RecordingSink> {
    let mut c = AppCore::with_doc(sink, empty_doc(), None);
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: SAMPLER.into(),
        key: Some(key("smp")),
    })
    .unwrap();
    c.sink_mut().take();
    c
}

fn sampler_core() -> AppCore<RecordingSink> {
    sampler_core_with(RecordingSink::new())
}

fn zones(c: &AppCore<RecordingSink>) -> Vec<ZoneDoc> {
    c.doc().device(&key("smp")).unwrap().zones.clone()
}

fn drop_files(c: &mut AppCore<RecordingSink>, paths: Vec<PathBuf>, at_key: Option<u8>) {
    c.handle_action(UiAction::ImportSamples {
        paths,
        device: key("smp"),
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

fn roots(c: &AppCore<RecordingSink>) -> Vec<u8> {
    zones(c).iter().map(|z| z.root_key).collect()
}

// ------------------------------------------------------------------ import: one undo step

#[test]
fn a_drop_onto_a_sampler_adds_all_zones_in_one_undo_step() {
    let t = Tmp::new("drop");
    let mut c = sampler_core();
    let (c3, e3) = (
        wav(&t, "tone_C3.wav", 4_800, 1.0),
        wav(&t, "tone_E3.wav", 2_400, 0.5),
    );
    drop_files(&mut c, vec![c3, e3], None);
    assert!(c.last_rejection().is_none(), "{:?}", c.last_rejection());
    assert_eq!(roots(&c), [48, 52], "mapped by the note names (C4 = 60)");
    assert_eq!(c.doc().assets.len(), 2);
    // One map reached the engine for the whole drop.
    let calls = c.sink_mut().take();
    let sent = sample_calls(&calls);
    assert_eq!(sent.len(), 1, "{calls:?}");
    assert_eq!(sent[0].1.as_ref().unwrap().zones().len(), 2);
    assert_eq!(Some(sent[0].0), c.device_id(&key("smp")));

    // Undo removes both zones at once and keeps the device; the engine gets the empty map.
    assert!(c.undo());
    assert!(zones(&c).is_empty());
    assert!(c.doc().device(&key("smp")).is_some(), "only the zones went");
    let cleared = sample_calls(&c.sink_mut().take());
    assert_eq!(cleared.len(), 1);
    assert!(cleared[0].1.is_none());

    // Redo brings both back, and the engine the matching map again.
    assert!(c.redo());
    assert_eq!(roots(&c), [48, 52]);
    let again = sample_calls(&c.sink_mut().take());
    assert_eq!(again.len(), 1);
    assert_eq!(again[0].1.as_ref().unwrap().zones().len(), 2);
}

#[test]
fn a_drop_at_a_key_puts_a_single_file_there_and_later_drops_append() {
    let t = Tmp::new("atkey");
    let mut c = sampler_core();
    drop_files(&mut c, vec![wav(&t, "one.wav", 2_400, 1.0)], Some(72));
    assert_eq!(roots(&c), [72]);
    // A second drop appends to the existing zones (still one step each).
    drop_files(&mut c, vec![wav(&t, "two.wav", 2_400, 0.5)], None);
    assert_eq!(roots(&c), [72, 60]);
    c.undo();
    assert_eq!(roots(&c), [72]);
}

#[test]
fn a_failing_file_is_reported_by_name_and_the_others_still_become_zones() {
    let t = Tmp::new("fail");
    let mut c = sampler_core();
    let bad = t.path("broken_C2.wav");
    std::fs::write(&bad, b"not audio").unwrap();
    drop_files(&mut c, vec![bad, wav(&t, "good_C3.wav", 2_400, 1.0)], None);
    assert_eq!(roots(&c), [48]);
    let message = c.last_rejection().unwrap();
    assert!(message.contains("broken_C2.wav"), "{message}");
    // If every file fails nothing is added and there is no undo step for the zones.
    let before = c.document().can_undo();
    let junk = t.path("junk.wav");
    std::fs::write(&junk, b"nope").unwrap();
    drop_files(&mut c, vec![junk], None);
    assert_eq!(roots(&c), [48]);
    assert_eq!(c.document().can_undo(), before);
}

#[test]
fn only_samplers_take_dropped_samples() {
    let t = Tmp::new("notsampler");
    let mut c = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: "atomik.poly-synth".into(),
        key: Some(key("syn")),
    })
    .unwrap();
    c.handle_action(UiAction::ImportSamples {
        paths: vec![wav(&t, "x.wav", 100, 1.0)],
        device: key("syn"),
        at_key: None,
    });
    c.wait_background();
    assert!(c.doc().assets.is_empty());
    assert!(
        c.last_rejection()
            .unwrap()
            .contains("does not take samples")
    );
}

// ------------------------------------------------------------------ maps to the engine

#[test]
fn a_live_zone_edit_is_exactly_one_sample_map_swap() {
    let t = Tmp::new("edit");
    let mut c = sampler_core();
    drop_files(&mut c, vec![wav(&t, "a_C3.wav", 4_800, 1.0)], None);
    c.sink_mut().take();
    let mut edited = zones(&c);
    edited[0].gain_db = -6.0;
    c.handle_action(UiAction::Command(Command::SetZones {
        device: key("smp"),
        zones: edited,
        merge: None,
    }));
    let calls = c.sink_mut().take();
    let sent = sample_calls(&calls);
    assert_eq!(sent.len(), 1, "{calls:?}");
    assert!((sent[0].1.as_ref().unwrap().zones()[0].gain_db + 6.0).abs() < 1e-6);
    assert!(
        calls.iter().all(|c| matches!(c, SinkCall::Samples { .. })),
        "nothing else is sent: {calls:?}"
    );
}

#[test]
fn a_zone_drag_swaps_per_move_and_ends_as_one_undo_step() {
    let t = Tmp::new("drag");
    let mut c = sampler_core();
    drop_files(&mut c, vec![wav(&t, "a_C3.wav", 4_800, 1.0)], None);
    c.sink_mut().take();
    for step in 1..=3 {
        let mut z = zones(&c);
        z[0].key_hi = 100 + step;
        c.handle_action(UiAction::Command(Command::SetZones {
            device: key("smp"),
            zones: z,
            merge: Some(77),
        }));
    }
    c.handle_action(UiAction::EndMerge);
    assert_eq!(sample_calls(&c.sink_mut().take()).len(), 3);
    assert_eq!(zones(&c)[0].key_hi, 103);
    c.undo();
    assert_eq!(zones(&c)[0].key_hi, 127, "one undo reverts the whole drag");
}

#[test]
fn zone_edits_keep_the_buffers_by_pointer_so_voices_can_follow() {
    let t = Tmp::new("ptr");
    let mut c = sampler_core();
    drop_files(&mut c, vec![wav(&t, "a_C3.wav", 4_800, 1.0)], None);
    let first = sample_calls(&c.sink_mut().take())[0].1.clone().unwrap();
    let mut z = zones(&c);
    z[0].tune_cents = 5.0;
    c.handle_action(UiAction::Command(Command::SetZones {
        device: key("smp"),
        zones: z,
        merge: None,
    }));
    let second = sample_calls(&c.sink_mut().take())[0].1.clone().unwrap();
    assert!(Arc::ptr_eq(&first.buffers()[0], &second.buffers()[0]));
}

#[test]
fn a_memory_asset_of_an_audio_track_shares_its_samples_with_the_sampler() {
    let t = Tmp::new("shared");
    let mut c = sampler_core();
    c.import_audio(
        vec![wav(&t, "a.wav", 4_800, 1.0)],
        ImportTarget::NewTracks,
        0,
    );
    c.wait_background();
    let hash = c.doc().assets.keys().next().unwrap().clone();
    c.sink_mut().take();
    c.apply_command(Command::SetZones {
        device: key("smp"),
        zones: vec![ZoneDoc::new(hash.clone(), 4_800)],
        merge: None,
    })
    .unwrap();
    let map = sample_calls(&c.sink_mut().take())[0].1.clone().unwrap();
    let SongAsset::Memory { data, .. } = &c.song_assets()[&hash] else {
        panic!("a small asset is held in memory");
    };
    assert!(Arc::ptr_eq(&map.buffers()[0].data, data));
}

#[test]
fn a_streamed_asset_is_loaded_fully_into_ram_for_the_sampler() {
    let t = Tmp::new("stream");
    let mut sink = RecordingSink::new();
    sink.stream_threshold = Some(1_000);
    let mut c = sampler_core_with(sink);
    c.import_audio(
        vec![wav(&t, "long.wav", 48_000, 1.0)],
        ImportTarget::NewTracks,
        0,
    );
    c.wait_background();
    let hash = c.doc().assets.keys().next().unwrap().clone();
    assert!(
        c.song_assets()[&hash].is_stream(),
        "the audio track streams it"
    );
    c.sink_mut().take();
    c.apply_command(Command::SetZones {
        device: key("smp"),
        zones: vec![ZoneDoc::new(hash.clone(), 48_000)],
        merge: None,
    })
    .unwrap();
    // Until the worker delivers the RAM copy the zone is left out of the map.
    let first = sample_calls(&c.sink_mut().take());
    assert_eq!(first.len(), 1);
    assert!(first[0].1.is_none(), "silent until loaded");
    assert!(c.is_busy(), "a worker reads the file");
    c.wait_background();
    let later = sample_calls(&c.sink_mut().take());
    assert_eq!(later.len(), 1, "{later:?}");
    let map = later[0].1.clone().unwrap();
    assert_eq!(map.buffers()[0].frames, 48_000);
    assert!(
        map.buffers()[0].data[0].iter().any(|s| *s != 0.0),
        "the whole sample is in RAM"
    );
    assert!(
        c.song_assets()[&hash].is_stream(),
        "the audio track still streams"
    );
}

/// A project with a sampler, two layered zones on two assets, saved as a bundle in `t`.
fn saved_project(t: &Tmp) -> (PathBuf, Vec<String>) {
    let mut c = sampler_core();
    drop_files(
        &mut c,
        vec![
            wav(t, "a_C3.wav", 4_800, 1.0),
            wav(t, "b_C4.wav", 2_400, 0.5),
        ],
        None,
    );
    let hashes: Vec<String> = zones(&c).iter().map(|z| z.asset.clone()).collect();
    c.save_as(&t.path("song.atomik")).unwrap();
    (t.path("song").join("song.atomik"), hashes)
}

#[test]
fn open_and_restart_send_every_sampler_its_map_after_its_insertion() {
    let t = Tmp::new("restart");
    let (file, _) = saved_project(&t);
    let mut c = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    c.open(&file).unwrap();
    c.wait_background();
    let id = c.device_id(&key("smp")).unwrap();
    let calls = c.sink_mut().take();
    let position = |pred: &dyn Fn(&SinkCall) -> bool| calls.iter().rposition(pred);
    let insert = position(&|x| matches!(x, SinkCall::Insert { id: i, .. } if *i == id)).unwrap();
    let samples = position(&|x| matches!(x, SinkCall::Samples { id: i, map: Some(_) } if *i == id))
        .expect("a map after the assets loaded");
    assert!(insert < samples, "insert, then samples: {calls:?}");
    let last = sample_calls(&calls).pop().unwrap().1.unwrap();
    assert_eq!(last.zones().len(), 2);

    // A restart rebuilds the engine: reset, inserts, then the map of each sampler.
    c.rebuild_engine();
    let calls = c.sink_mut().take();
    let reset = calls
        .iter()
        .position(|x| matches!(x, SinkCall::Reset { .. }))
        .unwrap();
    let id = c.device_id(&key("smp")).unwrap();
    let insert = calls
        .iter()
        .position(|x| matches!(x, SinkCall::Insert { id: i, .. } if *i == id))
        .unwrap();
    let samples = calls
        .iter()
        .position(|x| matches!(x, SinkCall::Samples { id: i, map: Some(_) } if *i == id))
        .unwrap();
    assert!(reset < insert && insert < samples, "{calls:?}");
    assert_eq!(sample_calls(&calls).len(), 1, "one map per sampler");
}

#[test]
fn every_sampler_of_a_restarted_engine_gets_its_own_map() {
    let t = Tmp::new("two");
    let mut c = sampler_core();
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: SAMPLER.into(),
        key: Some(key("smp2")),
    })
    .unwrap();
    drop_files(&mut c, vec![wav(&t, "a_C3.wav", 2_400, 1.0)], None);
    c.handle_action(UiAction::ImportSamples {
        paths: vec![
            wav(&t, "b_C3.wav", 2_400, 0.5),
            wav(&t, "c_C4.wav", 2_400, 0.25),
        ],
        device: key("smp2"),
        at_key: None,
    });
    c.wait_background();
    c.sink_mut().take();
    c.rebuild_engine();
    let calls = c.sink_mut().take();
    let sent = sample_calls(&calls);
    let mut zone_counts: Vec<(u32, usize)> = sent
        .iter()
        .map(|(id, m)| (id.0, m.as_ref().unwrap().zones().len()))
        .collect();
    zone_counts.sort_unstable();
    assert_eq!(zone_counts.len(), 2, "{calls:?}");
    assert_eq!(zone_counts.iter().map(|x| x.1).collect::<Vec<_>>(), [1, 2]);
    for (id, _) in &sent {
        let insert = calls
            .iter()
            .position(|x| matches!(x, SinkCall::Insert { id: i, .. } if i == id))
            .unwrap();
        let samples = calls
            .iter()
            .position(|x| matches!(x, SinkCall::Samples { id: i, .. } if i == id))
            .unwrap();
        assert!(insert < samples);
    }
}

#[test]
fn undoing_the_removal_of_a_sampler_inserts_it_and_sends_its_map() {
    let t = Tmp::new("undo-remove");
    let mut c = sampler_core();
    drop_files(&mut c, vec![wav(&t, "a_C3.wav", 2_400, 1.0)], None);
    c.apply_command(Command::RemoveDevice { key: key("smp") })
        .unwrap();
    c.sink_mut().take();
    assert!(c.undo());
    let calls = c.sink_mut().take();
    let id = c.device_id(&key("smp")).unwrap();
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
fn a_sampler_without_zones_gets_no_map_call() {
    let mut c = sampler_core();
    c.rebuild_engine();
    assert!(sample_calls(&c.sink_mut().take()).is_empty());
}

// ------------------------------------------------------------------ missing assets

#[test]
fn zones_of_a_missing_asset_stay_in_the_document_and_play_silence() {
    let t = Tmp::new("missing");
    let (file, hashes) = saved_project(&t);
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
    assert!(c.last_rejection().unwrap().contains("a_C3.wav"));
    assert_eq!(zones(&c).len(), 2, "both zones are kept");
    // The engine's map holds only the zone whose audio exists.
    let sent = sample_calls(&c.sink_mut().take());
    let map = sent.last().unwrap().1.clone().unwrap();
    assert_eq!(map.zones().len(), 1);
    assert_eq!(map.buffers()[0].frames, 2_400);
    // Opening the editor tells it which assets are missing: the zone row is drawn in the
    // record colour and its waveform is blank.
    c.open_sample_editor(key("smp"));
    let mut dl = atomik_ui::DisplayList::new();
    c.paint_pane(Pane::SampleEditor, &mut dl);
    let names = dl.texts();
    assert!(names.iter().any(|t| t.contains("a_C3.wav")), "{names:?}");
    // Saving keeps the reference (the file may come back).
    c.save().unwrap();
    assert!(std::fs::read_to_string(&file).unwrap().contains(&hashes[0]));
}

#[test]
fn a_sampler_whose_every_asset_is_missing_is_silent() {
    let t = Tmp::new("allmissing");
    let (file, hashes) = saved_project(&t);
    for h in &hashes {
        std::fs::remove_file(t.path("song").join("Audio").join(format!("{h}.wav"))).unwrap();
    }
    let mut c = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    c.open(&file).unwrap();
    c.wait_background();
    assert_eq!(c.missing_assets().len(), 2);
    let sent = sample_calls(&c.sink_mut().take());
    assert!(sent.iter().all(|(_, m)| m.is_none()), "{sent:?}");
    assert_eq!(zones(&c).len(), 2);
}

// ------------------------------------------------------------------ save, reopen, save as

fn audio_files(root: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(root.join("Audio"))
        .map(|d| {
            d.flatten()
                .filter(|e| e.path().is_file())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

#[test]
fn save_and_reopen_keep_the_zones_and_their_audio_files() {
    let t = Tmp::new("roundtrip");
    let (file, hashes) = saved_project(&t);
    let root = t.path("song");
    let mut expected: Vec<String> = hashes.iter().map(|h| format!("{h}.wav")).collect();
    expected.sort();
    assert_eq!(audio_files(&root), expected, "zone assets are not orphans");
    let mut c = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    c.open(&file).unwrap();
    c.wait_background();
    assert!(c.missing_assets().is_empty());
    assert_eq!(roots(&c), [48, 60]);
    let doc = c.doc().clone();
    c.save().unwrap();
    assert_eq!(audio_files(&root), expected);
    let mut again = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    again.open(&file).unwrap();
    assert_eq!(again.doc(), &doc, "save, reopen, identical");
}

#[test]
fn the_audio_only_the_undo_history_references_survives_a_save_then_goes_next_session() {
    let t = Tmp::new("history");
    let mut c = sampler_core();
    drop_files(&mut c, vec![wav(&t, "a_C3.wav", 4_800, 1.0)], None);
    let hash = zones(&c)[0].asset.clone();
    // Remove the zone: only the undo history still names the asset.
    c.apply_command(Command::SetZones {
        device: key("smp"),
        zones: Vec::new(),
        merge: None,
    })
    .unwrap();
    c.save_as(&t.path("song.atomik")).unwrap();
    let root = t.path("song");
    assert_eq!(audio_files(&root), [format!("{hash}.wav")], "kept for undo");
    assert!(c.doc().assets.contains_key(&hash));
    assert!(c.undo());
    assert_eq!(roots(&c), [48], "undo finds its audio");
    // A fresh session has no history: its save deletes the orphan.
    let mut fresh = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    c.apply_command(Command::SetZones {
        device: key("smp"),
        zones: Vec::new(),
        merge: None,
    })
    .unwrap();
    c.save().unwrap();
    fresh.open(&root.join("song.atomik")).unwrap();
    fresh.wait_background();
    fresh.save().unwrap();
    assert!(audio_files(&root).is_empty(), "{:?}", audio_files(&root));
}

#[test]
fn save_as_copies_the_zone_assets_and_the_scratch_bundle_goes() {
    let t = Tmp::new("saveas");
    let mut c = sampler_core();
    drop_files(
        &mut c,
        vec![
            wav(&t, "a_C3.wav", 4_800, 1.0),
            wav(&t, "b_C4.wav", 2_400, 0.5),
        ],
        None,
    );
    let scratch = c.bundle_root().unwrap().to_path_buf();
    assert!(scratch.starts_with(std::env::temp_dir()));
    let hashes: Vec<String> = zones(&c).iter().map(|z| z.asset.clone()).collect();
    let original = std::fs::read(scratch.join("Audio").join(format!("{}.wav", hashes[0]))).unwrap();
    let out = t.path("out");
    std::fs::create_dir_all(&out).unwrap();
    c.save_as(&out.join("one.atomik")).unwrap();
    let first = out.join("one");
    assert_eq!(c.bundle_root(), Some(first.as_path()));
    assert!(!scratch.exists());
    assert_eq!(audio_files(&first).len(), 2);
    let copied = std::fs::read(first.join("Audio").join(format!("{}.wav", hashes[0]))).unwrap();
    assert!(copied == original, "copied bit for bit");
    // Save As again somewhere else copies the whole bundle once more.
    let other = t.path("other");
    std::fs::create_dir_all(&other).unwrap();
    c.save_as(&other.join("two.atomik")).unwrap();
    assert_eq!(audio_files(&other.join("two")).len(), 2);
    let mut reopened = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    reopened
        .open(&other.join("two").join("two.atomik"))
        .unwrap();
    reopened.wait_background();
    assert!(reopened.missing_assets().is_empty());
    assert_eq!(roots(&reopened), [48, 60]);
}

// ------------------------------------------------------------------ audition

fn live_notes(calls: &[SinkCall]) -> Vec<NoteKind> {
    calls
        .iter()
        .filter_map(|c| match c {
            SinkCall::LiveNote { kind, .. } => Some(*kind),
            _ => None,
        })
        .collect()
}

#[test]
fn a_velocity_audition_sends_velocity_over_127_and_a_matching_off() {
    let mut c = sampler_core();
    for (velocity, expected) in [(127u8, 1.0f32), (64, 64.0 / 127.0), (1, 1.0 / 127.0)] {
        c.handle_action(UiAction::AuditionVelocity {
            device: key("smp"),
            key: 60,
            velocity,
            on: true,
        });
        c.handle_action(UiAction::AuditionVelocity {
            device: key("smp"),
            key: 60,
            velocity,
            on: false,
        });
        assert_eq!(
            live_notes(&c.sink_mut().take()),
            [
                NoteKind::On {
                    key: 60,
                    velocity: expected
                },
                NoteKind::Off { key: 60 }
            ]
        );
    }
    // The plain audition keeps its fixed velocity.
    c.audition(&key("smp"), 62, true);
    assert_eq!(
        live_notes(&c.sink_mut().take()),
        [NoteKind::On {
            key: 62,
            velocity: AUDITION_VELOCITY
        }]
    );
}

// ------------------------------------------------------------------ the pane

#[test]
fn the_sample_editor_opens_on_a_sampler_and_closes_on_request() {
    let mut c = sampler_core();
    assert!(c.sample_editor_device().is_none());
    c.handle_action(UiAction::OpenSampleEditor(key("smp")));
    assert_eq!(c.sample_editor_device(), Some(&key("smp")));
    assert!(c.sample_editor().is_present());
    c.handle_action(UiAction::CloseSampleEditor);
    assert!(c.sample_editor_device().is_none());
    // Not a sampler: ignored.
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: "atomik.poly-synth".into(),
        key: Some(key("syn")),
    })
    .unwrap();
    c.handle_action(UiAction::OpenSampleEditor(key("syn")));
    assert!(c.sample_editor_device().is_none());
    // The default target is the first sampler.
    assert!(c.open_sample_editor_default());
    assert_eq!(c.sample_editor_device(), Some(&key("smp")));
}

#[test]
fn the_pane_closes_when_its_sampler_is_removed_and_stays_closed_on_undo() {
    let mut c = sampler_core();
    c.open_sample_editor(key("smp"));
    c.apply_command(Command::RemoveDevice { key: key("smp") })
        .unwrap();
    assert!(c.sample_editor_device().is_none(), "no longer present");
    c.undo();
    assert!(c.sample_editor_device().is_none());
    assert!(c.doc().device(&key("smp")).is_some());
}

#[test]
fn opening_the_editor_replaces_the_piano_roll_and_a_new_document_closes_it() {
    let mut c = sampler_core();
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: "atomik.poly-synth".into(),
        key: Some(key("syn")),
    })
    .unwrap();
    c.apply_command(Command::Song(SongCommand::AddTrack { device: key("syn") }))
        .unwrap();
    c.apply_command(Command::Song(SongCommand::AddClip {
        device: key("syn"),
        lane: 0,
        clip: atomik_model::Clip {
            id: atomik_model::ClipId(0),
            start: 0,
            length: atomik_core::PPQ,
            offset: 0,
            loop_length: None,
            muted: false,
            grid: None,
            content: atomik_model::ClipContent::Notes(Vec::new()),
        },
    }))
    .unwrap();
    assert!(c.open_piano_roll_default());
    c.open_sample_editor(key("smp"));
    assert!(c.piano_roll_clip().is_none());
    assert!(c.sample_editor_device().is_some());
    assert!(c.open_piano_roll_default());
    assert!(c.sample_editor_device().is_none(), "the roll replaces it");
    c.open_sample_editor(key("smp"));
    c.new_document();
    assert!(c.sample_editor_device().is_none());
}

#[test]
fn the_editor_pane_routes_its_events_and_edits_through_the_core() {
    let t = Tmp::new("pane");
    let mut c = sampler_core();
    drop_files(&mut c, vec![wav(&t, "a_C3.wav", 4_800, 1.0)], None);
    c.open_sample_editor(key("smp"));
    c.sample_editor_mut()
        .set_bounds(atomik_ui::kurbo::Rect::new(0.0, 0.0, 1000.0, 560.0));
    c.sink_mut().take();
    // Delete removes the selected zone: the view emits a SetZones, the core applies it.
    let actions = c
        .pane_command(Pane::SampleEditor, "edit.delete", None)
        .expect("the view takes edit.delete");
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, UiAction::Command(Command::SetZones { .. }))),
        "{actions:?}"
    );
    assert!(zones(&c).is_empty());
    assert_eq!(sample_calls(&c.sink_mut().take()).len(), 1);
    // Esc closes it (nothing left to cancel or deselect).
    let esc = UiEvent::Key {
        key: atomik_ui::Key::Escape,
        modifiers: atomik_ui::Modifiers::NONE,
        pressed: true,
    };
    let _ = c.handle_pane_event(Pane::SampleEditor, &esc);
    assert!(c.sample_editor_device().is_none());
}

#[test]
fn peaks_of_imported_samples_reach_the_editor_and_the_arrangement() {
    let t = Tmp::new("peaks");
    let mut c = sampler_core();
    c.open_sample_editor(key("smp"));
    drop_files(&mut c, vec![wav(&t, "a_C3.wav", 4_800, 1.0)], None);
    let hash = zones(&c)[0].asset.clone();
    assert!(c.peaks().contains_key(&hash));
    let mut dl = atomik_ui::DisplayList::new();
    c.paint_pane(Pane::SampleEditor, &mut dl);
    assert!(dl.texts().iter().any(|t| t.contains("a_C3.wav")));
}

// ------------------------------------------------------------------ live = bounce

fn demo_file() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/projects/sampler-demo/sampler-demo.atomik")
}

/// The sampler demo played through the app's live path (a real engine rendered by hand) gives
/// the bounce's samples, bit for bit, whatever the host block (the automated twin of the hand check). The
/// live transport starts one engine batch (64 frames, the command FIFO) after the Play command,
/// so the live output is the bounce delayed by exactly that.
#[test]
fn the_live_path_plays_the_sampler_demo_exactly_like_the_bounce() {
    use crate::offline_sink::OfflineSink;
    const LATENCY: usize = atomik_core::BATCH;
    let file = demo_file();
    let options = atomik_render::project::ProjectOptions::default();
    let bounce =
        atomik_render::project::render_project_file(&file, &default_registry(), &options).unwrap();
    let frames = bounce.channels[0].len();
    assert!(bounce.channels[0].iter().any(|s| s.abs() > 1e-3));
    for block in [64, 480, 512] {
        let mut core = AppCore::with_doc(OfflineSink::new(), empty_doc(), None);
        core.open(&file).unwrap();
        core.wait_background();
        assert!(core.missing_assets().is_empty());
        assert!(
            core.last_rejection().is_none(),
            "{:?}",
            core.last_rejection()
        );
        core.transport_action(atomik_ui::TransportAction::Play);
        let live = core.sink_mut().render_blocks(frames + LATENCY, block);
        for (channel, (a, b)) in live.iter().zip(&bounce.channels).enumerate() {
            let same = a[LATENCY..]
                .iter()
                .zip(b)
                .all(|(x, y)| x.to_bits() == y.to_bits());
            assert!(
                same,
                "channel {channel} differs from the bounce at block {block}"
            );
            assert!(
                a[..LATENCY].iter().all(|x| *x == 0.0),
                "silent before the start"
            );
        }
    }
}
