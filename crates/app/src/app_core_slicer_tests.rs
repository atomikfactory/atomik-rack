//! Loop slicers in the app core (app part): loading a loop (first
//! file only, detection on a worker, beats and zones), `SliceLoop` in both modes, "To track",
//! the slice editor pane and the sample maps reaching the engine. Fixtures are procedural
//! (clicks at known frames); the engine is the recording sink.

use super::*;
use crate::sink::{RecordingSink, SinkCall};
use atomik_audio_file::{AudioData, write_wav_f32};
use atomik_core::PPQ;
use atomik_device_sdk::SampleMapRef;
use atomik_model::{ClipContent, Note, ZoneDoc, empty_doc, to_track_notes};
use atomik_ui::{Key, Modifiers, SliceMode};
use std::sync::atomic::{AtomicU64, Ordering};

const SLICER: &str = "atomik.loop-slicer";
const BEAT_FRAMES: usize = 24_000; // 120 BPM at 48 kHz
const LOOP_BEATS: usize = 6;

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "atomik-app-slicer-{tag}-{}-{n}",
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

/// A mono loop of six beats: a decaying square burst at every beat (`seed` changes the burst
/// period, so different loops are different assets).
fn loop_wav(dir: &Tmp, name: &str, seed: usize) -> PathBuf {
    let mut data = vec![0.0f32; BEAT_FRAMES * LOOP_BEATS];
    for beat in 0..LOOP_BEATS {
        for i in 0..2_400usize {
            let amp = 0.8 * (1.0 - i as f32 / 2_400.0);
            let sign = if (i / (20 + seed)).is_multiple_of(2) {
                1.0
            } else {
                -1.0
            };
            data[beat * BEAT_FRAMES + i] = amp * sign;
        }
    }
    let path = dir.path(name);
    write_wav_f32(&path, &AudioData::new(48_000, vec![data]).unwrap()).unwrap();
    path
}

fn slicer_core_with(sink: RecordingSink) -> AppCore<RecordingSink> {
    let mut c = AppCore::with_doc(sink, empty_doc(), None);
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: SLICER.into(),
        key: Some(key("slc")),
    })
    .unwrap();
    c.sink_mut().take();
    c
}

fn slicer_core() -> AppCore<RecordingSink> {
    slicer_core_with(RecordingSink::new())
}

fn zones(c: &AppCore<RecordingSink>) -> Vec<ZoneDoc> {
    c.doc().device(&key("slc")).unwrap().zones.clone()
}

fn beats(c: &AppCore<RecordingSink>) -> Option<f32> {
    c.doc()
        .device(&key("slc"))
        .unwrap()
        .params
        .get("loop.beats")
        .copied()
}

fn drop_loop(c: &mut AppCore<RecordingSink>, paths: Vec<PathBuf>) {
    c.handle_action(UiAction::ImportSamples {
        paths,
        device: key("slc"),
        at_key: None,
    });
    c.wait_background();
}

fn loaded(t: &Tmp) -> AppCore<RecordingSink> {
    let mut c = slicer_core();
    drop_loop(&mut c, vec![loop_wav(t, "beat.wav", 0)]);
    assert!(c.last_rejection().is_none(), "{:?}", c.last_rejection());
    c
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

fn clips(c: &AppCore<RecordingSink>) -> Vec<atomik_model::Clip> {
    c.doc()
        .song
        .track(&key("slc"))
        .map(|t| t.lanes[0].clips.clone())
        .unwrap_or_default()
}

fn notes_of(clip: &atomik_model::Clip) -> Vec<Note> {
    match &clip.content {
        ClipContent::Notes(n) => n.clone(),
        other => panic!("not notes: {other:?}"),
    }
}

// ------------------------------------------------------------------ loading a loop

#[test]
fn a_dropped_loop_is_sliced_at_its_transients_and_gets_its_beats() {
    let t = Tmp::new("load");
    let mut c = slicer_core();
    c.handle_action(UiAction::ImportSamples {
        paths: vec![loop_wav(&t, "beat.wav", 0)],
        device: key("slc"),
        at_key: None,
    });
    // The UI thread returned at once: nothing is sliced until the workers report.
    assert!(c.is_busy());
    assert!(zones(&c).is_empty());
    c.wait_background();
    assert!(c.last_rejection().is_none(), "{:?}", c.last_rejection());
    let z = zones(&c);
    // One slice per click, each within 5 ms of its beat (the detector moves back at most 5 ms).
    assert_eq!(
        z.len(),
        LOOP_BEATS,
        "{:?}",
        z.iter().map(|z| z.start).collect::<Vec<_>>()
    );
    for (i, zone) in z.iter().enumerate() {
        let click = (i * BEAT_FRAMES) as i64;
        assert!(
            (zone.start as i64 - click).abs() <= 240,
            "slice {i} starts at {} (click {click})",
            zone.start
        );
        assert_eq!(zone.key_lo, 24 + i as u8);
        assert_eq!(zone.key_hi, zone.key_lo);
    }
    assert_eq!(z[0].start, 0);
    assert_eq!(z.last().unwrap().end, (BEAT_FRAMES * LOOP_BEATS) as u64);
    // 3 s at 120 BPM: six beats.
    assert_eq!(beats(&c), Some(6.0));
}

#[test]
fn loading_is_one_undo_step_for_the_beats_and_the_slices() {
    let t = Tmp::new("undo");
    let mut c = loaded(&t);
    assert!(c.undo());
    assert!(zones(&c).is_empty(), "one undo takes the slices");
    assert_eq!(beats(&c), None, "and restores the default beats");
    assert!(
        c.doc().device(&key("slc")).is_some(),
        "the next undo would be the device's own step"
    );
    assert!(c.redo());
    assert_eq!(zones(&c).len(), LOOP_BEATS);
    assert_eq!(beats(&c), Some(6.0));
    assert!(!c.redo(), "a single step was recorded");
}

#[test]
fn the_loop_reaches_the_engine_as_one_map() {
    let t = Tmp::new("map");
    let mut c = slicer_core();
    drop_loop(&mut c, vec![loop_wav(&t, "beat.wav", 0)]);
    let sent = sample_calls(&c.sink_mut().take());
    assert_eq!(sent.len(), 1, "one SetZones, one map");
    assert_eq!(sent[0].1.as_ref().unwrap().zones().len(), LOOP_BEATS);
    assert_eq!(Some(sent[0].0), c.device_id(&key("slc")));
}

#[test]
fn only_the_first_dropped_file_is_used_and_the_rest_are_reported() {
    let t = Tmp::new("first");
    let mut c = slicer_core();
    drop_loop(
        &mut c,
        vec![
            loop_wav(&t, "first.wav", 0),
            loop_wav(&t, "second.wav", 3),
            loop_wav(&t, "third.wav", 5),
        ],
    );
    assert_eq!(c.doc().assets.len(), 1, "only one file was imported");
    assert_eq!(
        c.doc().assets.values().next().unwrap().original_name,
        "first.wav"
    );
    let note = c.last_rejection().unwrap();
    assert!(
        note.contains("second.wav") && note.contains("third.wav"),
        "{note}"
    );
    assert!(note.contains("ignored"), "{note}");
    assert_eq!(zones(&c).len(), LOOP_BEATS);
}

#[test]
fn a_second_loop_replaces_the_first() {
    let t = Tmp::new("replace");
    let mut c = loaded(&t);
    let first = zones(&c)[0].asset.clone();
    drop_loop(&mut c, vec![loop_wav(&t, "other.wav", 7)]);
    let z = zones(&c);
    assert_ne!(z[0].asset, first);
    let asset = &z[0].asset;
    assert!(z.iter().all(|s| &s.asset == asset));
}

#[test]
fn a_result_for_a_closed_document_is_dropped() {
    let t = Tmp::new("stale");
    let mut c = slicer_core();
    let doc = c.doc().clone();
    c.handle_action(UiAction::ImportSamples {
        paths: vec![loop_wav(&t, "beat.wav", 0)],
        device: key("slc"),
        at_key: None,
    });
    // The same rack is opened again before the workers report (a new generation).
    c.open_doc(doc);
    c.wait_background();
    assert!(
        zones(&c).is_empty(),
        "the old import must not touch the new document"
    );
    assert!(c.doc().assets.is_empty());
    assert_eq!(beats(&c), None);
}

#[test]
fn the_panel_sensitivity_decides_how_many_slices_the_detector_finds() {
    // A steady tone with strong bursts on the beats and weak ones half a beat later: a higher
    // sensitivity finds more of them (the detector looks at relative change).
    let t = Tmp::new("sens");
    let mut data: Vec<f32> = (0..BEAT_FRAMES * LOOP_BEATS)
        .map(|i| {
            if (i / 37).is_multiple_of(2) {
                0.3
            } else {
                -0.3
            }
        })
        .collect();
    for step in 0..LOOP_BEATS * 2 {
        let amp = if step.is_multiple_of(2) { 0.6 } else { 0.12 };
        for i in 0..1_200usize {
            let sign = if (i / 20).is_multiple_of(2) {
                1.0
            } else {
                -1.0
            };
            data[step * BEAT_FRAMES / 2 + i] += amp * (1.0 - i as f32 / 1_200.0) * sign;
        }
    }
    let path = t.path("two.wav");
    write_wav_f32(&path, &AudioData::new(48_000, vec![data]).unwrap()).unwrap();
    let count = |sensitivity: u8| {
        let mut c = slicer_core();
        c.view_mut().set_slice_sensitivity(&key("slc"), sensitivity);
        drop_loop(&mut c, vec![path.clone()]);
        zones(&c).len()
    };
    let (low, high) = (count(0), count(100));
    assert!(low < high, "{low} vs {high}");
}

// ------------------------------------------------------------------ SliceLoop

#[test]
fn a_grid_slicing_keeps_the_per_slice_fields_by_index_and_is_one_step() {
    let t = Tmp::new("grid");
    let mut c = loaded(&t);
    let mut z = zones(&c);
    z[1].gain_db = -6.0;
    z[1].reverse = true;
    z[2].pan = 0.5;
    c.apply_command(Command::SetZones {
        device: key("slc"),
        zones: z,
        merge: None,
    })
    .unwrap();
    c.sink_mut().take();
    // 6 beats in 1/16: 24 slices.
    c.handle_action(UiAction::SliceLoop {
        device: key("slc"),
        mode: SliceMode::Grid { division: 4 },
    });
    let z = zones(&c);
    assert_eq!(z.len(), 24);
    assert_eq!(z[1].start, (BEAT_FRAMES / 4) as u64, "even grid");
    assert_eq!((z[1].gain_db, z[1].reverse), (-6.0, true));
    assert_eq!(z[2].pan, 0.5);
    assert_eq!(z[3].gain_db, 0.0);
    assert_eq!(
        z[3].pan, 0.0,
        "slices past the old count copy the last one's fields"
    );
    assert_eq!(sample_calls(&c.sink_mut().take()).len(), 1, "one map");
    // One undo step: back to the six transient slices with their fields.
    assert!(c.undo());
    let back = zones(&c);
    assert_eq!(back.len(), LOOP_BEATS);
    assert_eq!(back[1].gain_db, -6.0);
}

#[test]
fn a_grid_follows_the_loop_beats_param() {
    let t = Tmp::new("gridbeats");
    let mut c = loaded(&t);
    c.apply_command(Command::SetParam {
        key: key("slc"),
        param: "loop.beats".into(),
        value: 4.0,
        merge: None,
    })
    .unwrap();
    c.handle_action(UiAction::SliceLoop {
        device: key("slc"),
        mode: SliceMode::Grid { division: 1 },
    });
    assert_eq!(zones(&c).len(), 4, "4 beats in 1/4");
}

#[test]
fn transient_re_slicing_runs_on_a_worker_and_keeps_the_fields() {
    let t = Tmp::new("trans");
    let mut c = loaded(&t);
    c.handle_action(UiAction::SliceLoop {
        device: key("slc"),
        mode: SliceMode::Grid { division: 2 },
    });
    assert_eq!(zones(&c).len(), 12);
    let mut z = zones(&c);
    z[2].gain_db = -3.0;
    z[2].transpose = 5;
    c.apply_command(Command::SetZones {
        device: key("slc"),
        zones: z,
        merge: None,
    })
    .unwrap();
    c.sink_mut().take();
    c.handle_action(UiAction::SliceLoop {
        device: key("slc"),
        mode: SliceMode::Transients { sensitivity: 50 },
    });
    assert_eq!(zones(&c).len(), 12, "the worker has not reported yet");
    c.wait_background();
    let z = zones(&c);
    assert_eq!(z.len(), LOOP_BEATS);
    assert_eq!(
        (z[2].gain_db, z[2].transpose),
        (-3.0, 5),
        "fields stay with their index"
    );
    assert_eq!(sample_calls(&c.sink_mut().take()).len(), 1);
    // One step back to the grid.
    assert!(c.undo());
    assert_eq!(zones(&c).len(), 12);
    assert_eq!(zones(&c)[2].gain_db, -3.0);
}

#[test]
fn re_slicing_to_the_same_slices_adds_no_undo_step() {
    let t = Tmp::new("same");
    let mut c = loaded(&t);
    let before = zones(&c);
    c.handle_action(UiAction::SliceLoop {
        device: key("slc"),
        mode: SliceMode::Transients { sensitivity: 50 },
    });
    c.wait_background();
    assert_eq!(zones(&c), before);
    // The next undo still takes the load's slices, not an identical no-op step.
    assert!(c.undo());
    assert!(zones(&c).is_empty());
}

#[test]
fn slicing_without_a_loop_is_refused_and_changes_nothing() {
    let mut c = slicer_core();
    for mode in [
        SliceMode::Grid { division: 4 },
        SliceMode::Transients { sensitivity: 50 },
    ] {
        c.handle_action(UiAction::SliceLoop {
            device: key("slc"),
            mode,
        });
        c.wait_background();
        assert!(zones(&c).is_empty());
        assert!(c.last_rejection().unwrap().contains("load a loop"));
        assert!(!c.document().can_undo() || c.doc().device(&key("slc")).unwrap().params.is_empty());
    }
}

#[test]
fn a_loop_whose_audio_is_missing_cannot_be_detected_and_stays_silent() {
    let t = Tmp::new("missing");
    let mut c = loaded(&t);
    c.save_as(&t.path("song.atomik")).unwrap();
    let hash = zones(&c)[0].asset.clone();
    let file = t.path("song").join("song.atomik");
    std::fs::remove_file(t.path("song").join("Audio").join(format!("{hash}.wav"))).unwrap();
    let mut c = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    c.open(&file).unwrap();
    c.wait_background();
    assert_eq!(c.missing_assets().len(), 1);
    assert_eq!(
        zones(&c).len(),
        LOOP_BEATS,
        "the slices stay in the document"
    );
    let sent = sample_calls(&c.sink_mut().take());
    assert!(sent.iter().all(|(_, m)| m.is_none()), "silent: {sent:?}");
    // Detection reports the missing audio and changes nothing; the grid still works (it
    // needs no samples) and the map stays empty.
    c.handle_action(UiAction::SliceLoop {
        device: key("slc"),
        mode: SliceMode::Transients { sensitivity: 50 },
    });
    c.wait_background();
    assert!(
        c.last_rejection().unwrap().contains("missing"),
        "{:?}",
        c.last_rejection()
    );
    assert_eq!(zones(&c).len(), LOOP_BEATS);
    // The slice editor lists the asset as missing.
    c.open_slice_editor(key("slc"));
    let mut dl = atomik_ui::DisplayList::new();
    c.paint_pane(Pane::SliceEditor, &mut dl);
    assert!(!dl.texts().is_empty());
}

// ------------------------------------------------------------------ To track

#[test]
fn to_track_without_a_track_adds_the_track_and_the_clip_in_one_step() {
    let t = Tmp::new("totrack");
    let mut c = loaded(&t);
    assert!(c.doc().song.track(&key("slc")).is_none());
    c.handle_action(UiAction::SlicesToTrack { device: key("slc") });
    assert!(c.last_rejection().is_none(), "{:?}", c.last_rejection());
    let clips = clips(&c);
    assert_eq!(clips.len(), 1);
    let clip = &clips[0];
    assert_eq!(clip.start, 0);
    assert_eq!(clip.length, 6 * PPQ);
    let device = c.doc().device(&key("slc")).unwrap();
    let frames = c.doc().assets.get(&device.zones[0].asset).unwrap().frames;
    let (expected, len) = to_track_notes(&device.zones, frames, 6);
    assert_eq!(notes_of(clip), expected);
    assert_eq!(len as i64, clip.length);
    assert_eq!(notes_of(clip).len(), LOOP_BEATS);
    // The new clip is selected.
    assert_eq!(c.arrangement().selected_clips(), [clip.id]);
    // One undo takes the clip and the track away together.
    assert!(c.undo());
    assert!(c.doc().song.track(&key("slc")).is_none());
    assert_eq!(zones(&c).len(), LOOP_BEATS, "the slices stay");
    assert!(c.redo());
    assert_eq!(
        c.doc().song.track(&key("slc")).unwrap().lanes[0]
            .clips
            .len(),
        1
    );
}

#[test]
fn to_track_puts_the_clip_at_the_playhead_snapped_to_the_bar() {
    let t = Tmp::new("playhead");
    let mut c = loaded(&t);
    c.apply_command(Command::Song(SongCommand::AddTrack { device: key("slc") }))
        .unwrap();
    c.transport_action(atomik_ui::TransportAction::Locate(5 * PPQ + 10));
    c.handle_action(UiAction::SlicesToTrack { device: key("slc") });
    assert_eq!(clips(&c)[0].start, 4 * PPQ, "snapped to the nearest bar");
    assert!(c.undo());
    assert!(
        clips(&c).is_empty(),
        "one step: the track alone was there before"
    );
}

#[test]
fn to_track_replaces_the_selected_clip_of_the_slicer_in_one_step() {
    let t = Tmp::new("replace");
    let mut c = loaded(&t);
    c.handle_action(UiAction::SlicesToTrack { device: key("slc") });
    let first = clips(&c)[0].clone();
    // A grid of 1/8 over 6 beats: 12 notes; shorter loop: 4 beats.
    c.apply_command(Command::SetParam {
        key: key("slc"),
        param: "loop.beats".into(),
        value: 4.0,
        merge: None,
    })
    .unwrap();
    c.handle_action(UiAction::SliceLoop {
        device: key("slc"),
        mode: SliceMode::Grid { division: 2 },
    });
    assert_eq!(zones(&c).len(), 8);
    c.arrangement_mut().set_selected_clips(&[first.id]);
    c.handle_action(UiAction::SlicesToTrack { device: key("slc") });
    let after = clips(&c);
    assert_eq!(after.len(), 1, "replaced, not added");
    assert_eq!(after[0].id, first.id);
    assert_eq!(after[0].start, first.start);
    assert_eq!(after[0].length, 4 * PPQ);
    assert_eq!(notes_of(&after[0]).len(), 8);
    // One undo gives the old notes and length back.
    assert!(c.undo());
    let back = clips(&c);
    assert_eq!(back[0].length, 6 * PPQ);
    assert_eq!(notes_of(&back[0]), notes_of(&first));
    assert_eq!(zones(&c).len(), 8, "the undo took only the clip edit");
}

#[test]
fn to_track_adds_a_new_clip_when_the_selected_one_is_not_the_slicers() {
    let t = Tmp::new("other");
    let mut c = loaded(&t);
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
            length: PPQ,
            offset: 0,
            loop_length: None,
            muted: false,
            grid: None,
            content: ClipContent::Notes(Vec::new()),
        },
    }))
    .unwrap();
    let other = c.document().last_clip_id().unwrap();
    c.arrangement_mut().set_selected_clips(&[other]);
    c.handle_action(UiAction::SlicesToTrack { device: key("slc") });
    assert_eq!(clips(&c).len(), 1, "a clip on the slicer's track");
    let synth = c.doc().song.track(&key("syn")).unwrap();
    assert_eq!(synth.lanes[0].clips.len(), 1);
    assert!(
        notes_of(&synth.lanes[0].clips[0]).is_empty(),
        "the synth clip is untouched"
    );
}

#[test]
fn to_track_without_a_loop_is_refused() {
    let mut c = slicer_core();
    c.handle_action(UiAction::SlicesToTrack { device: key("slc") });
    assert!(c.last_rejection().unwrap().contains("load a loop"));
    assert!(c.doc().song.track(&key("slc")).is_none());
}

// ------------------------------------------------------------------ the pane

#[test]
fn the_slice_editor_opens_on_a_slicer_with_the_panel_sensitivity_and_closes() {
    let mut c = slicer_core();
    c.view_mut().set_slice_sensitivity(&key("slc"), 73);
    assert!(c.slice_editor_device().is_none());
    c.handle_action(UiAction::OpenSliceEditor(key("slc")));
    assert_eq!(c.slice_editor_device(), Some(&key("slc")));
    assert!(c.slice_editor().is_present());
    assert_eq!(c.slice_editor().sensitivity(), 73);
    c.handle_action(UiAction::CloseSliceEditor);
    assert!(c.slice_editor_device().is_none());
    // Not a slicer: ignored (a sampler takes samples too).
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: "atomik.sampler".into(),
        key: Some(key("smp")),
    })
    .unwrap();
    c.handle_action(UiAction::OpenSliceEditor(key("smp")));
    assert!(c.slice_editor_device().is_none());
    // The default is the selected slicer, else the first.
    assert!(c.open_slice_editor_default());
    assert_eq!(c.slice_editor_device(), Some(&key("slc")));
}

#[test]
fn the_default_target_prefers_the_selected_slicer() {
    let mut c = slicer_core();
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: SLICER.into(),
        key: Some(key("slc2")),
    })
    .unwrap();
    c.select(Some(key("slc2")));
    assert!(c.open_slice_editor_default());
    assert_eq!(c.slice_editor_device(), Some(&key("slc2")));
}

#[test]
fn the_pane_closes_when_its_slicer_is_removed_and_stays_closed_on_undo() {
    let mut c = slicer_core();
    c.open_slice_editor(key("slc"));
    c.apply_command(Command::RemoveDevice { key: key("slc") })
        .unwrap();
    assert!(c.slice_editor_device().is_none(), "no longer present");
    c.undo();
    assert!(c.slice_editor_device().is_none());
    assert!(c.doc().device(&key("slc")).is_some());
}

#[test]
fn the_editors_are_exclusive_and_a_new_document_closes_the_slice_editor() {
    let mut c = slicer_core();
    for (type_id, k) in [("atomik.sampler", "smp"), ("atomik.drum-machine", "drm")] {
        c.apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: type_id.into(),
            key: Some(key(k)),
        })
        .unwrap();
    }
    c.apply_command(Command::Song(SongCommand::AddTrack { device: key("drm") }))
        .unwrap();
    c.apply_command(Command::Song(SongCommand::AddClip {
        device: key("drm"),
        lane: 0,
        clip: atomik_model::Clip {
            id: atomik_model::ClipId(0),
            start: 0,
            length: PPQ,
            offset: 0,
            loop_length: None,
            muted: false,
            grid: None,
            content: ClipContent::Notes(Vec::new()),
        },
    }))
    .unwrap();
    c.open_slice_editor(key("slc"));
    c.open_sample_editor(key("smp"));
    assert!(c.slice_editor_device().is_none() && c.sample_editor_device().is_some());
    c.open_slice_editor(key("slc"));
    assert!(
        c.sample_editor_device().is_none(),
        "the slice editor replaces it"
    );
    c.open_step_editor(key("drm"), None);
    assert!(c.slice_editor_device().is_none() && c.step_editor_device().is_some());
    c.open_slice_editor(key("slc"));
    assert!(c.step_editor_device().is_none());
    assert!(c.open_piano_roll_default());
    assert!(c.slice_editor_device().is_none(), "the roll replaces it");
    c.open_slice_editor(key("slc"));
    assert!(c.piano_roll_clip().is_none());
    c.new_document();
    assert!(c.slice_editor_device().is_none());
    // Opening a document closes it too.
    let doc = c.doc().clone();
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: SLICER.into(),
        key: Some(key("slc")),
    })
    .unwrap();
    c.open_slice_editor(key("slc"));
    c.open_doc(doc);
    assert!(c.slice_editor_device().is_none());
}

#[test]
fn the_pane_routes_its_events_and_esc_closes_it() {
    let t = Tmp::new("routing");
    let mut c = loaded(&t);
    c.open_slice_editor(key("slc"));
    c.slice_editor_mut()
        .set_bounds(atomik_ui::kurbo::Rect::new(0.0, 0.0, 1000.0, 560.0));
    c.sink_mut().take();
    // Delete removes the marker of the selected slice: the view emits a SetZones.
    c.slice_editor_mut().set_selected_slice(Some(2));
    let del = UiEvent::Key {
        key: Key::Delete,
        modifiers: Modifiers::NONE,
        pressed: true,
    };
    let actions = c.handle_pane_event(Pane::SliceEditor, &del);
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, UiAction::Command(Command::SetZones { .. }))),
        "{actions:?}"
    );
    assert_eq!(zones(&c).len(), LOOP_BEATS - 1);
    assert_eq!(sample_calls(&c.sink_mut().take()).len(), 1);
    let esc = UiEvent::Key {
        key: Key::Escape,
        modifiers: Modifiers::NONE,
        pressed: true,
    };
    // The first Esc may clear the selection; the pane closes once nothing is left to cancel.
    for _ in 0..3 {
        let _ = c.handle_pane_event(Pane::SliceEditor, &esc);
    }
    assert!(c.slice_editor_device().is_none());
}

#[test]
fn the_editor_buttons_reach_the_core() {
    let t = Tmp::new("buttons");
    let mut c = loaded(&t);
    c.open_slice_editor(key("slc"));
    c.slice_editor_mut()
        .set_bounds(atomik_ui::kurbo::Rect::new(0.0, 0.0, 1000.0, 560.0));
    let press = |pos| UiEvent::PointerDown {
        pos,
        button: atomik_ui::PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    };
    let release = |pos| UiEvent::PointerUp {
        pos,
        button: atomik_ui::PointerButton::Primary,
        modifiers: Modifiers::NONE,
    };
    // "To track" through the pane.
    let p = c.slice_editor().to_track_button_center();
    c.handle_pane_event(Pane::SliceEditor, &press(p));
    c.handle_pane_event(Pane::SliceEditor, &release(p));
    assert_eq!(clips(&c).len(), 1);
    // A grid division selects the grid; Slice then cuts evenly.
    let d = c.slice_editor().division_button_center(4).unwrap();
    c.handle_pane_event(Pane::SliceEditor, &press(d));
    c.handle_pane_event(Pane::SliceEditor, &release(d));
    let s = c.slice_editor().slice_button_center();
    c.handle_pane_event(Pane::SliceEditor, &press(s));
    c.handle_pane_event(Pane::SliceEditor, &release(s));
    assert_eq!(zones(&c).len(), 24);
}

#[test]
fn peaks_of_the_loop_reach_the_slice_editor() {
    let t = Tmp::new("peaks");
    let mut c = slicer_core();
    c.open_slice_editor(key("slc"));
    drop_loop(&mut c, vec![loop_wav(&t, "beat.wav", 0)]);
    let hash = zones(&c)[0].asset.clone();
    assert!(c.peaks().contains_key(&hash));
    let mut dl = atomik_ui::DisplayList::new();
    c.paint_pane(Pane::SliceEditor, &mut dl);
    assert!(
        dl.texts().iter().any(|t| t.contains("beat.wav")),
        "{:?}",
        dl.texts()
    );
}

// ------------------------------------------------------------------ maps

/// A project with a slicer and a loop, saved as a bundle in `t`.
fn saved_project(t: &Tmp) -> PathBuf {
    let mut c = loaded(t);
    c.save_as(&t.path("song.atomik")).unwrap();
    t.path("song").join("song.atomik")
}

#[test]
fn every_zone_edit_swaps_the_slicers_map_once() {
    let t = Tmp::new("edit");
    let mut c = loaded(&t);
    c.sink_mut().take();
    let mut z = zones(&c);
    z[0].gain_db = -3.0;
    c.apply_command(Command::SetZones {
        device: key("slc"),
        zones: z,
        merge: None,
    })
    .unwrap();
    let sent = sample_calls(&c.sink_mut().take());
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].1.as_ref().unwrap().zones().len(), LOOP_BEATS);
}

#[test]
fn open_restart_and_undoing_a_removal_send_the_slicer_its_map_after_its_insert() {
    let t = Tmp::new("restart");
    let file = saved_project(&t);
    let mut c = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    c.open(&file).unwrap();
    c.wait_background();
    let id = c.device_id(&key("slc")).unwrap();
    let calls = c.sink_mut().take();
    let insert = calls
        .iter()
        .rposition(|x| matches!(x, SinkCall::Insert { id: i, .. } if *i == id))
        .unwrap();
    let samples = calls
        .iter()
        .rposition(|x| matches!(x, SinkCall::Samples { id: i, map: Some(_) } if *i == id))
        .expect("a map once the loop loaded");
    assert!(insert < samples, "insert, then samples: {calls:?}");
    // A restart: reset, insert, map.
    c.rebuild_engine();
    let calls = c.sink_mut().take();
    let id = c.device_id(&key("slc")).unwrap();
    let pos = |pred: &dyn Fn(&SinkCall) -> bool| calls.iter().position(pred).unwrap();
    let reset = pos(&|x| matches!(x, SinkCall::Reset { .. }));
    let insert = pos(&|x| matches!(x, SinkCall::Insert { id: i, .. } if *i == id));
    let samples = pos(&|x| matches!(x, SinkCall::Samples { id: i, map: Some(_) } if *i == id));
    assert!(reset < insert && insert < samples, "{calls:?}");
    // Removing and un-removing the slicer inserts it and sends its map again.
    c.apply_command(Command::RemoveDevice { key: key("slc") })
        .unwrap();
    c.sink_mut().take();
    c.undo();
    let calls = c.sink_mut().take();
    let id = c.device_id(&key("slc")).unwrap();
    let insert = pos_in(
        &calls,
        |x| matches!(x, SinkCall::Insert { id: i, .. } if *i == id),
    );
    let samples = pos_in(
        &calls,
        |x| matches!(x, SinkCall::Samples { id: i, map: Some(_) } if *i == id),
    );
    assert!(insert < samples, "{calls:?}");
}

fn pos_in(calls: &[SinkCall], pred: impl Fn(&SinkCall) -> bool) -> usize {
    calls.iter().position(pred).expect("call present")
}

#[test]
fn a_slicer_without_zones_gets_no_map_call_on_insert() {
    let mut c = slicer_core();
    c.rebuild_engine();
    assert!(sample_calls(&c.sink_mut().take()).is_empty());
}
