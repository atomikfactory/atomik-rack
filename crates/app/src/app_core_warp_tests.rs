//! Warp in the app core: the buffer cache (a worker renders, the song is submitted again with the
//! buffer, stale buffers go) and the commands that read the samples. The fixtures are procedural
//! files in temporary folders.

use super::*;
use crate::sink::{RecordingSink, SinkCall};
use atomik_audio_file::{AudioData, write_wav_f32};
use atomik_core::PPQ;
use atomik_model::{AudioClipContent, ClipContent, WarpMode, WarpSettings, empty_doc};
use atomik_ui::{ImportTarget, WarpValues};
use std::sync::atomic::{AtomicU64, Ordering};

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("atomik-app-warp-{tag}-{}-{n}", std::process::id()));
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

/// A noise burst of 2000 frames every `period` frames, starting at `first` (deterministic).
fn clicks(period: usize, first: usize) -> impl Fn(usize) -> f32 {
    move |i| {
        if i < first || (i - first) % period >= 2_000 {
            return 0.0;
        }
        let k = ((i - first) % period) as u32;
        let mut x = k.wrapping_mul(2_654_435_761).wrapping_add(12_345);
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        ((x >> 8) as f32 / 8_388_608.0 - 1.0) * 0.8 * (-(k as f32) / 400.0).exp()
    }
}

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

fn values(enabled: bool, mode: WarpMode, bpm_tenths: u32, st: i32) -> WarpValues {
    WarpValues {
        enabled,
        mode,
        bpm_tenths,
        semitones: st,
        cents: 0,
    }
}

/// The last song the sink was given.
fn last_song(c: &mut AppCore<RecordingSink>) -> Option<atomik_engine::SongInput> {
    c.sink_mut()
        .take()
        .into_iter()
        .rev()
        .find_map(|call| match call {
            SinkCall::Song(input) => Some(input),
            _ => None,
        })
}

#[test]
fn applying_the_panel_sets_every_selected_clip_in_one_undo_step() {
    let t = Tmp::new("apply");
    let (a, b) = (
        wav(&t, "a.wav", 48_000, clicks(24_000, 0)),
        wav(&t, "b.wav", 48_000, clicks(12_000, 0)),
    );
    let mut c = core();
    let ids = import(&mut c, &[a, b]);
    select(&mut c, &ids);
    let before = c.doc().clone();
    c.apply_warp(&values(true, WarpMode::Tonal, 905, 3));
    c.wait_background();
    assert!(c.last_rejection().is_none(), "{:?}", c.last_rejection());
    for id in &ids {
        let w = audio(&c, *id).warp;
        assert!(w.enabled && w.mode == WarpMode::Tonal && w.transpose_cents == 300);
        assert!((w.source_bpm - 90.5).abs() < 1e-9);
    }
    c.undo();
    assert_eq!(c.doc(), &before);
    // Nothing selected, nothing to do.
    select(&mut c, &[]);
    c.apply_warp(&values(true, WarpMode::Beats, 1200, 0));
    assert!(c.last_rejection().unwrap().contains("Select"));
}

#[test]
fn a_reversed_clip_is_left_out_of_a_warp() {
    let t = Tmp::new("reversed");
    let a = wav(&t, "a.wav", 48_000, clicks(24_000, 0));
    let mut c = core();
    let ids = import(&mut c, &[a]);
    let clip = c.doc().song.clip(ids[0]).unwrap().2.clone();
    let mut content = audio(&c, ids[0]);
    content.reverse = true;
    c.apply_command(Command::Song(SongCommand::SetAudioClip {
        id: ids[0],
        start: clip.start,
        length: clip.length,
        muted: false,
        content,
        merge: None,
    }))
    .unwrap();
    select(&mut c, &ids);
    c.apply_warp(&values(true, WarpMode::Beats, 1200, 0));
    assert!(c.last_rejection().unwrap().contains("reversed"));
    assert!(!audio(&c, ids[0]).warp.enabled);
}

#[test]
fn a_loop_as_bars_gets_its_tempo_from_its_length_and_the_length_of_the_bars() {
    let t = Tmp::new("bars");
    // 2 s of audio: 2 bars at 240 BPM, 1 bar at 120.
    let a = wav(&t, "loop.wav", 96_000, clicks(24_000, 0));
    let mut c = core();
    let ids = import(&mut c, &[a]);
    select(&mut c, &ids);
    c.warp_bars(1);
    c.wait_background();
    let w = audio(&c, ids[0]).warp;
    assert!(w.enabled && (w.source_bpm - 120.0).abs() < 1e-9, "{w:?}");
    assert_eq!(c.doc().song.clip(ids[0]).unwrap().2.length, 4 * PPQ);
    c.warp_bars(2);
    assert!((audio(&c, ids[0]).warp.source_bpm - 240.0).abs() < 1e-9);
    assert_eq!(c.doc().song.clip(ids[0]).unwrap().2.length, 8 * PPQ);
    // 16 bars of 2 s is 1920 BPM: out of range, nothing changes.
    let before = c.doc().clone();
    c.warp_bars(16);
    assert_eq!(c.doc(), &before);
    assert!(c.last_rejection().unwrap().contains("999"));
}

#[test]
fn warp_to_song_tempo_finds_the_tempo_of_the_audio() {
    let t = Tmp::new("tempo");
    // Six beats at 100 BPM: a click every 28 800 frames.
    let a = wav(&t, "beats.wav", 172_800, clicks(28_800, 0));
    let mut c = core();
    let ids = import(&mut c, &[a]);
    select(&mut c, &ids);
    c.warp_to_song_tempo();
    c.wait_background();
    let w = audio(&c, ids[0]).warp;
    assert!(w.enabled);
    assert!((w.source_bpm - 100.0).abs() < 0.2, "{}", w.source_bpm);
    // The panel's DETECT agrees, in tenths.
    let tenths = c.detect_warp_tempo().unwrap();
    assert!((i64::from(tenths) - 1000).abs() <= 2, "{tenths}");
}

#[test]
fn without_a_tempo_in_the_audio_the_clip_takes_the_song_tempo_and_says_so() {
    let t = Tmp::new("flat");
    let a = wav(&t, "flat.wav", 48_000, |_| 0.1);
    let mut c = core();
    let ids = import(&mut c, &[a]);
    select(&mut c, &ids);
    c.warp_to_song_tempo();
    assert!(c.last_rejection().unwrap().contains("No tempo found"));
    let w = audio(&c, ids[0]).warp;
    assert!(w.enabled && (w.source_bpm - 120.0).abs() < 1e-9);
    assert!(c.detect_warp_tempo().is_none());
}

#[test]
fn quantize_audio_sets_markers_that_put_the_hits_on_the_grid() {
    let t = Tmp::new("quantize");
    // Hits every 12 000 frames (a 1/8 at 120 BPM) but 700 frames late.
    let a = wav(&t, "late.wav", 96_000, clicks(12_000, 700));
    let mut c = core();
    let ids = import(&mut c, &[a]);
    select(&mut c, &ids);
    let before = c.doc().clone();
    c.quantize_audio_clips(PPQ / 2, 1.0);
    c.wait_background();
    assert!(c.last_rejection().is_none(), "{:?}", c.last_rejection());
    let clip = c.doc().song.clip(ids[0]).unwrap().2.clone();
    let a = audio(&c, ids[0]);
    assert!(a.warp.enabled && a.warp.markers.len() > 3, "{:?}", a.warp);
    // The hit at source frame 12 700 plays on the second eighth now (tick PPQ / 2), to a tick.
    let tick = atomik_model::tick_at_source(&c.doc().song, 48_000, clip.start, &a, 12_700.0);
    assert!((tick - (PPQ / 2) as f64).abs() < 60.0, "{tick}");
    c.undo();
    assert_eq!(c.doc(), &before);
    // Silence has no transient to move.
    let s = wav(&t, "silent.wav", 48_000, |_| 0.0);
    let ids = import(&mut c, &[s]);
    select(&mut c, &ids[1..]);
    c.quantize_audio_clips(PPQ / 2, 1.0);
    assert!(c.last_rejection().unwrap().contains("transient"));
}

#[test]
fn a_warped_clip_plays_its_buffer_once_the_worker_is_done() {
    let t = Tmp::new("cache");
    let a = wav(&t, "beats.wav", 115_200, clicks(28_800, 0));
    let mut c = core();
    let ids = import(&mut c, &[a]);
    select(&mut c, &ids);
    c.sink_mut().take();
    c.apply_warp(&values(true, WarpMode::Beats, 1000, 0));
    // The song goes out at once with the asset in real time...
    let dry = last_song(&mut c).expect("a song was submitted");
    assert_eq!(dry.audio[0].regions[0].source_offset, 0);
    assert_eq!(dry.assets[0].frames(), 115_200);
    // ...and again with the rendered buffer when the worker is done.
    c.wait_background();
    assert!(!c.warp_busy());
    let warped = last_song(&mut c).expect("the song was submitted again");
    let clip = c.doc().song.clip(ids[0]).unwrap().2.clone();
    let frames =
        c.doc().song.frame_at(clip.end(), 48_000) - c.doc().song.frame_at(clip.start, 48_000);
    assert_eq!(warped.assets[0].frames(), frames);
    assert_eq!(c.audio.warps.len(), 1);
    // Editing the tempo makes the buffer stale: a new one is rendered and the old one dropped.
    c.apply_command(Command::Song(SongCommand::SetTempoMap(vec![
        atomik_model::TempoEvent::step(0, 90.0),
    ])))
    .unwrap();
    c.wait_background();
    assert_eq!(c.audio.warps.len(), 1);
    let again = last_song(&mut c).expect("the song was submitted for the new tempo");
    assert!(
        again.assets[0].frames() > frames,
        "slower tempo, longer buffer"
    );
    // Warp off: nothing is cached any more.
    select(&mut c, &ids);
    c.apply_warp(&values(false, WarpMode::Beats, 1000, 0));
    c.wait_background();
    assert!(c.audio.warps.is_empty());
    let off = last_song(&mut c).unwrap();
    assert_eq!(off.assets[0].frames(), 115_200);
}

#[test]
fn undoing_a_warp_cancels_its_render_and_keeps_the_song_dry() {
    let t = Tmp::new("undo");
    let a = wav(&t, "beats.wav", 480_000, clicks(28_800, 0));
    let mut c = core();
    let ids = import(&mut c, &[a]);
    select(&mut c, &ids);
    c.apply_warp(&values(true, WarpMode::Tonal, 1000, -2));
    c.undo();
    c.wait_background();
    assert!(c.audio.warps.is_empty());
    assert_eq!(audio(&c, ids[0]).warp, WarpSettings::default());
}
