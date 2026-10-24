//! Normalise and strip silence in the app core: they read the samples the engine holds, change
//! the selected audio clips in one undo step, and say why when they change nothing.
//! The fixtures are procedural files in temporary folders.

use super::*;
use crate::sink::RecordingSink;
use atomik_audio_file::{AudioData, write_wav_f32};
use atomik_core::PPQ;
use atomik_model::{AudioClipContent, ClipContent, empty_doc};
use atomik_ui::{ImportTarget, SilenceValues};
use std::sync::atomic::{AtomicU64, Ordering};

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "atomik-app-audio-edit-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
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

/// A mono file of `frames` frames: `value(i)`.
fn wav(dir: &Tmp, name: &str, frames: usize, value: impl Fn(usize) -> f32) -> PathBuf {
    let data: Vec<f32> = (0..frames).map(value).collect();
    let path = dir.0.join(name);
    write_wav_f32(&path, &AudioData::new(48_000, vec![data]).unwrap()).unwrap();
    path
}

/// Imports `paths` as one new track each at tick 0 and returns the clip ids in track order.
fn import(c: &mut AppCore<RecordingSink>, paths: &[PathBuf]) -> Vec<ClipId> {
    c.import_audio(paths.to_vec(), ImportTarget::NewTracks, 0);
    c.wait_background();
    assert!(c.last_rejection().is_none(), "{:?}", c.last_rejection());
    c.doc()
        .song
        .tracks
        .iter()
        .flat_map(|t| t.lanes.iter().flat_map(|l| l.clips.iter().map(|c| c.id)))
        .collect()
}

fn audio(c: &AppCore<RecordingSink>, id: ClipId) -> AudioClipContent {
    match &c.doc().song.clip(id).unwrap().2.content {
        ClipContent::Audio(a) => a.clone(),
        _ => panic!("not audio"),
    }
}

fn select(c: &mut AppCore<RecordingSink>, ids: &[ClipId]) {
    c.arrangement_mut().set_selected_clips(ids);
}

fn all_clips(c: &AppCore<RecordingSink>) -> Vec<(ClipId, i64, i64)> {
    let mut v: Vec<_> = c
        .doc()
        .song
        .tracks
        .iter()
        .flat_map(|t| t.lanes.iter().flat_map(|l| l.clips.iter()))
        .map(|c| (c.id, c.start, c.length))
        .collect();
    v.sort_by_key(|x| (x.1, x.0));
    v
}

// ------------------------------------------------------------------ normalise

#[test]
fn normalise_brings_the_peak_of_each_clip_to_zero_dbfs_in_one_undo_step() {
    let t = Tmp::new("normalise");
    let quiet = wav(&t, "quiet.wav", 48_000, |i| {
        ((i % 100) as f32 / 100.0) * 0.25
    });
    let quieter = wav(&t, "quieter.wav", 24_000, |i| {
        ((i % 50) as f32 / 50.0) * 0.125
    });
    let mut c = core();
    let ids = import(&mut c, &[quiet, quieter]);
    select(&mut c, &ids);
    let before = c.doc().clone();
    c.normalise_audio();
    assert!(c.last_rejection().is_none(), "{:?}", c.last_rejection());
    // The peak is 0.99 * 0.25 and 0.98 * 0.125 of full scale.
    for (id, peak) in [(ids[0], 0.2475f32), (ids[1], 0.1225)] {
        let a = audio(&c, id);
        let want = -20.0 * peak.log10();
        assert!((a.gain_db - want).abs() < 1e-3, "{id:?}: {}", a.gain_db);
        let g = atomik_core::db_to_gain(a.gain_db);
        assert!((peak * g - 1.0).abs() < 1e-4);
    }
    // One undo step for both clips.
    c.undo();
    assert_eq!(c.doc(), &before);
    c.redo();
    assert!(audio(&c, ids[1]).gain_db > 0.0);
}

#[test]
fn normalise_measures_only_the_span_the_clip_plays() {
    let t = Tmp::new("span");
    // A loud burst early in the file and a quiet tail: the clip plays the tail.
    let file = wav(
        &t,
        "burst.wav",
        48_000,
        |i| if i < 1_000 { 0.9 } else { 0.1 },
    );
    let mut c = core();
    let ids = import(&mut c, &[file]);
    let clip = c.doc().song.clip(ids[0]).unwrap().2.clone();
    let mut a = audio(&c, ids[0]);
    a.source_offset = 2_000;
    c.apply_command(Command::Song(SongCommand::SetAudioClip {
        id: ids[0],
        start: clip.start,
        length: clip.length,
        muted: false,
        content: a,
        merge: None,
    }))
    .unwrap();
    select(&mut c, &ids);
    c.normalise_audio();
    let db = audio(&c, ids[0]).gain_db;
    assert!(
        (db - 20.0).abs() < 1e-3,
        "the 0.1 tail goes to full scale: {db}"
    );
}

#[test]
fn normalise_says_why_it_changed_nothing() {
    let t = Tmp::new("why");
    let silent = wav(&t, "silent.wav", 4_800, |_| 0.0);
    let full = wav(&t, "full.wav", 4_800, |i| if i == 10 { 1.0 } else { 0.0 });
    let mut c = core();
    let ids = import(&mut c, &[silent, full]);
    select(&mut c, &[]);
    c.normalise_audio();
    assert_eq!(
        c.last_rejection(),
        Some("Select an audio clip to normalise")
    );
    select(&mut c, &ids[..1]);
    c.normalise_audio();
    assert!(c.last_rejection().unwrap().contains("silent"));
    // A clip already at 0 dBFS is left alone; one undo step is not wasted on it.
    select(&mut c, &ids[1..]);
    c.normalise_audio();
    assert!(c.last_rejection().unwrap().contains("already"));
    assert!(audio(&c, ids[1]).gain_db.abs() < 1e-3);
}

#[test]
fn a_note_clip_is_not_normalised() {
    let mut c = core();
    select(&mut c, &[ClipId(1)]);
    c.normalise_audio();
    assert!(c.last_rejection().is_some());
}

// ------------------------------------------------------------------ strip silence

/// Three bursts of 0.5 in 1 s of silence (48 000 frames): at 3000..9000, 20000..30000 and
/// 40000..46000. With a minimum silence of 50 ms (2400 frames) the lead-in goes, the two gaps go
/// and the 2000 frames at the end stay.
fn bursts() -> impl Fn(usize) -> f32 {
    |i| {
        if (3_000..9_000).contains(&i)
            || (20_000..30_000).contains(&i)
            || (40_000..46_000).contains(&i)
        {
            0.5
        } else {
            0.0
        }
    }
}

fn values() -> SilenceValues {
    SilenceValues {
        threshold_db: -40,
        min_silence_ms: 50,
        pad_before_ms: 0,
        pad_after_ms: 0,
        fade_ms: 0,
    }
}

#[test]
fn strip_silence_cuts_the_clip_into_its_sounds_in_one_undo_step() {
    let t = Tmp::new("strip");
    let file = wav(&t, "bursts.wav", 48_000, bursts());
    let mut c = core();
    let ids = import(&mut c, &[file]);
    select(&mut c, &ids);
    let before = c.doc().clone();
    c.strip_silence(&values());
    assert!(c.last_rejection().is_none(), "{:?}", c.last_rejection());
    let clips = all_clips(&c);
    assert_eq!(clips.len(), 3, "{clips:?}");
    // The three pieces start where their sounds do (ticks at 120 BPM: 1.5625 frames each way
    // round to the nearest tick) and the first keeps the id.
    assert_eq!(clips[0].0, ids[0]);
    let ticks = |frames: f64| (frames * 16.0 / 25.0).round() as i64;
    assert_eq!(clips[0].1, ticks(3_000.0));
    assert_eq!(clips[1].1, ticks(20_000.0));
    assert_eq!(clips[2].1, ticks(40_000.0));
    // The pieces play the same source frames at the same times.
    let a: Vec<u64> = clips.iter().map(|x| audio(&c, x.0).source_offset).collect();
    assert_eq!(a, [3_000, 20_000, 40_000]);
    // The pieces are the selection.
    let mut sel = c.arrangement().selected_clips();
    sel.sort();
    assert_eq!(sel, clips.iter().map(|x| x.0).collect::<Vec<_>>());
    // One step.
    c.undo();
    assert_eq!(c.doc(), &before);
    c.redo();
    assert_eq!(all_clips(&c).len(), 3);
}

#[test]
fn padding_and_fades_follow_the_settings_in_milliseconds() {
    let t = Tmp::new("pad");
    let file = wav(&t, "bursts.wav", 48_000, bursts());
    let mut c = core();
    let ids = import(&mut c, &[file]);
    select(&mut c, &ids);
    let v = SilenceValues {
        pad_before_ms: 10,
        pad_after_ms: 20,
        fade_ms: 5,
        ..values()
    };
    c.strip_silence(&v);
    let clips = all_clips(&c);
    assert_eq!(clips.len(), 3);
    // 10 ms = 480 frames before, 20 ms = 960 after.
    let first = audio(&c, clips[0].0);
    assert_eq!(first.source_offset, 3_000 - 480);
    let ticks = |frames: f64| (frames * 16.0 / 25.0).round() as i64;
    assert!((clips[0].2 - (ticks(9_000.0 + 960.0) - ticks(3_000.0 - 480.0))).abs() <= 1);
    // The cut edges get the 5 ms (240 frame) fade.
    assert_eq!((first.fade_in, first.fade_out), (240, 240));
}

#[test]
fn a_reversed_clip_is_stripped_from_the_other_side() {
    let t = Tmp::new("rev");
    let file = wav(&t, "bursts.wav", 48_000, bursts());
    let mut c = core();
    let ids = import(&mut c, &[file]);
    let clip = c.doc().song.clip(ids[0]).unwrap().2.clone();
    let mut a = audio(&c, ids[0]);
    a.reverse = true;
    c.apply_command(Command::Song(SongCommand::SetAudioClip {
        id: ids[0],
        start: clip.start,
        length: clip.length,
        muted: false,
        content: a,
        merge: None,
    }))
    .unwrap();
    select(&mut c, &ids);
    c.strip_silence(&values());
    let clips = all_clips(&c);
    assert_eq!(clips.len(), 3);
    // Played backwards the last burst comes first: the pieces' source offsets, in time order,
    // descend.
    let offsets: Vec<u64> = clips.iter().map(|x| audio(&c, x.0).source_offset).collect();
    assert_eq!(offsets, [40_000, 20_000, 3_000]);
    assert!(clips.iter().all(|x| audio(&c, x.0).reverse));
    // The first piece in time still begins where the clip does: the 2000 frames of silence at
    // the end of the file are short, so they stay and play first. The second starts after the
    // 8000-frame first piece and the 10000-frame gap.
    assert_eq!(clips[0].1, 0);
    let ticks = |frames: f64| (frames * 16.0 / 25.0).round() as i64;
    assert_eq!(clips[1].1, ticks(18_000.0));
}

#[test]
fn strip_silence_says_why_it_changed_nothing() {
    let t = Tmp::new("stripwhy");
    let silent = wav(&t, "silent.wav", 4_800, |_| 0.0);
    let steady = wav(&t, "steady.wav", 4_800, |_| 0.5);
    let mut c = core();
    let ids = import(&mut c, &[silent, steady]);
    select(&mut c, &[]);
    c.strip_silence(&values());
    assert_eq!(c.last_rejection(), Some("Select an audio clip to strip"));
    select(&mut c, &ids[..1]);
    c.strip_silence(&values());
    assert!(c.last_rejection().unwrap().contains("silent"));
    assert_eq!(all_clips(&c).len(), 2, "a silent clip is not deleted");
    select(&mut c, &ids[1..]);
    c.strip_silence(&values());
    assert!(c.last_rejection().unwrap().contains("No silence"));
    assert_eq!(all_clips(&c).len(), 2);
}

#[test]
fn the_panel_target_counts_the_selected_audio_clips() {
    let t = Tmp::new("target");
    let a = wav(&t, "a.wav", 4_800, |_| 0.5);
    let b = wav(&t, "b.wav", 2_400, |_| 0.25);
    let mut c = core();
    let ids = import(&mut c, &[a, b]);
    select(&mut c, &[]);
    assert_eq!(c.strip_silence_target(), (String::new(), false));
    select(&mut c, &ids[..1]);
    assert_eq!(c.strip_silence_target(), ("1 audio clip".to_owned(), true));
    select(&mut c, &ids);
    assert_eq!(c.strip_silence_target(), ("2 audio clips".to_owned(), true));
    let _ = PPQ;
}
