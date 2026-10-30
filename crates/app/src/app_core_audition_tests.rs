//! The speaker tool in the app core: the span that is read, press and release against the
//! engine sink, and the worker that ends when the stream is let go of.

use super::audition::{feed_span, span_chunks};
use super::*;
use crate::import_warp::WarpImports;
use crate::sink::{RecordingSink, SinkCall};
use atomik_audio_file::{AudioData, write_wav_f32};
use atomik_engine::SongAsset;
use atomik_model::empty_doc;
use atomik_ui::ImportTarget;
use std::time::Duration;

fn ramp(frames: usize) -> SongAsset {
    SongAsset::memory(Arc::from(vec![
        (0..frames).map(|i| i as f32).collect::<Vec<f32>>(),
        (0..frames).map(|i| -(i as f32)).collect::<Vec<f32>>(),
    ]))
}

fn collect(asset: &SongAsset, start: u64, end: u64, reverse: bool) -> Vec<f32> {
    let mut left = Vec::new();
    span_chunks(asset, start, end, reverse, |planes| {
        left.extend_from_slice(&planes[0]);
        true
    });
    left
}

fn core_with_asset() -> (AppCore<RecordingSink>, String, std::path::PathBuf) {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("atomik-app-audition-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("loop.wav");
    let data: Vec<f32> = (0..96_000).map(|i| (i % 100) as f32 / 100.0).collect();
    write_wav_f32(&path, &AudioData::new(48_000, vec![data]).unwrap()).unwrap();
    let mut c = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    c.set_warp_imports(WarpImports::Off);
    c.import_audio(vec![path], ImportTarget::NewTracks, 0);
    c.wait_background();
    let hash = c.doc().assets.keys().next().expect("an asset").clone();
    (c, hash, dir)
}

fn preview_calls(c: &AppCore<RecordingSink>) -> Vec<SinkCall> {
    c.sink()
        .calls
        .iter()
        .filter(|call| matches!(call, SinkCall::PreviewStart { .. } | SinkCall::PreviewStop))
        .cloned()
        .collect()
}

fn press(hash: &str, start: u64, end: u64, reverse: bool, gain_db: f32) -> UiAction {
    UiAction::AuditionSpan {
        asset: hash.to_owned(),
        start_frame: start,
        end_frame: end,
        reverse,
        gain_db,
    }
}

#[test]
fn a_forward_span_is_read_in_order_and_clamped_to_the_asset() {
    let a = ramp(100_000);
    let got = collect(&a, 10, 20, false);
    assert_eq!(got, (10..20).map(|i| i as f32).collect::<Vec<_>>());
    // Across several chunks, and past the end of the asset.
    let long = collect(&a, 5, 99_990 + 50_000, false);
    assert_eq!(long.len(), 99_995);
    assert_eq!(long[0], 5.0);
    assert_eq!(*long.last().unwrap(), 99_999.0);
    assert!(collect(&a, 20, 20, false).is_empty());
    assert!(collect(&a, 30, 20, false).is_empty());
}

#[test]
fn a_reversed_span_plays_from_its_end_down() {
    let a = ramp(100_000);
    let got = collect(&a, 10, 20, true);
    assert_eq!(got, (10..20).rev().map(|i| i as f32).collect::<Vec<_>>());
    let long = collect(&a, 7, 60_000, true);
    assert_eq!(long.len(), 59_993);
    assert_eq!(long[0], 59_999.0);
    assert_eq!(*long.last().unwrap(), 7.0);
    assert!(long.windows(2).all(|w| (w[0] - w[1] - 1.0).abs() < 1e-3));
}

#[test]
fn a_press_starts_the_preview_voice_and_the_release_stops_it() {
    let (mut c, hash, dir) = core_with_asset();
    c.handle_action(press(&hash, 1_000, 40_000, false, -6.0));
    let calls = preview_calls(&c);
    let [SinkCall::PreviewStart { gain }] = calls[..] else {
        panic!("{calls:?}")
    };
    assert!((gain - 0.501_187).abs() < 1e-4, "{gain}");
    assert!(c.sink().preview_active());
    assert!(c.auditioning());
    c.handle_action(UiAction::StopAudition);
    assert_eq!(
        preview_calls(&c),
        [SinkCall::PreviewStart { gain }, SinkCall::PreviewStop]
    );
    assert!(!c.sink().preview_active());
    assert!(!c.auditioning());
    // A second release does nothing.
    c.handle_action(UiAction::StopAudition);
    assert_eq!(preview_calls(&c).len(), 2);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_new_press_replaces_the_audition_and_an_empty_span_plays_nothing() {
    let (mut c, hash, dir) = core_with_asset();
    c.handle_action(press(&hash, 0, 10_000, false, 0.0));
    c.handle_action(press(&hash, 500, 20_000, true, 0.0));
    assert!(c.sink().preview_active());
    // The first stream was replaced by the second, not left running beside it.
    assert_eq!(c.sink().previews.len(), 1);
    c.handle_action(UiAction::StopAudition);
    c.handle_action(press(&hash, 500, 500, false, 0.0));
    assert!(!c.sink().preview_active());
    // A span past the end of the file is clamped to nothing.
    c.handle_action(press(&hash, 5_000_000, 6_000_000, false, 0.0));
    assert!(!c.sink().preview_active());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_unknown_asset_is_refused_with_a_message() {
    let (mut c, _, dir) = core_with_asset();
    c.handle_action(press(&"0".repeat(64), 0, 100, false, 0.0));
    assert!(c.last_rejection().is_some());
    assert!(preview_calls(&c).is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_browser_preview_started_after_the_audition_is_not_stopped_by_the_release() {
    let (mut c, hash, dir) = core_with_asset();
    c.handle_action(press(&hash, 0, 40_000, false, 0.0));
    // Another stream takes the voice (what a browser preview does).
    let (_feed, stream) = atomik_engine::preview_channel();
    c.sink_mut().preview_start(stream, 1.0).unwrap();
    c.handle_action(UiAction::StopAudition);
    assert!(
        c.sink().preview_active(),
        "the newer stream keeps playing after the release"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_reader_ends_when_the_stream_is_let_go_of() {
    // More frames than the ring holds, so the reader must wait for room.
    let asset = ramp(PREVIEW_FRAMES * 3);
    let (feed, stream) = atomik_engine::preview_channel();
    let monitor = feed.monitor();
    let end = asset.frames();
    let worker = std::thread::spawn(move || feed_span(feed, &asset, 0, end, false));
    std::thread::sleep(Duration::from_millis(20));
    assert!(!worker.is_finished(), "waiting for room in a full ring");
    drop(stream);
    worker
        .join()
        .expect("the reader ends once the stream is gone");
    assert!(monitor.is_abandoned());
}

/// More frames than a preview ring holds.
const PREVIEW_FRAMES: usize = atomik_engine::PREVIEW_RING_FRAMES;
