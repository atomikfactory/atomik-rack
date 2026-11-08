//! The video row in the app core, with the fake tool (no FFmpeg is needed): the state the core
//! hands the row (preview, timecodes, why there is none), the thumbnail worker that starts for a
//! prepared film and stops with a collapsed or hidden row, and the pictures that reach the row
//! for the slots in view.

use super::*;
use crate::sink::RecordingSink;
use atomik_model::{VideoClip, VideoCommand, VideoFacts};
use atomik_ui::kurbo::Rect;
use atomik_video::fake::{FakeTool, Solid, solid_jpeg};
use atomik_video::{Platform, samples};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "atomik-app-video-thumbs-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn programs(&self) -> PathBuf {
        let dir = self.path("ffmpeg-folder");
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["ffmpeg", "ffprobe"] {
            std::fs::write(dir.join(Platform::current().exe_name(name)), b"x").unwrap();
        }
        dir
    }

    fn film(&self) -> PathBuf {
        let p = self.path("scene.mov");
        std::fs::write(&p, b"pretend film").unwrap();
        p
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn core(t: &Tmp, frames: usize) -> AppCore<RecordingSink> {
    let colours = [Solid::Red, Solid::Green, Solid::Blue, Solid::White];
    let tool = FakeTool::new().probe(samples::MOV_PRORES).frames(
        (0..frames)
            .map(|n| solid_jpeg(colours[n % 4]).to_vec())
            .collect(),
    );
    let mut c = AppCore::with_doc(RecordingSink::new(), atomik_model::empty_doc(), None);
    c.set_video_tool(Arc::new(tool));
    c.set_video_folder(Some(t.programs()));
    c.set_video_cache(t.path("cache"));
    c.arrangement_mut()
        .set_bounds(Rect::new(0.0, 0.0, 1200.0, 420.0));
    c
}

/// A core whose song has the film imported and prepared.
fn prepared(t: &Tmp, frames: usize) -> AppCore<RecordingSink> {
    let mut c = core(t, frames);
    c.import_video(t.film()).unwrap();
    c.wait_background();
    assert!(c.video_prepared().is_some(), "{:?}", c.last_rejection());
    c
}

/// Runs frames until `done` holds (the worker decodes in the background).
fn settle(
    c: &mut AppCore<RecordingSink>,
    what: &str,
    done: impl Fn(&AppCore<RecordingSink>) -> bool,
) {
    for _ in 0..600 {
        c.frame();
        if done(c) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("never: {what}");
}

#[test]
fn a_prepared_film_gets_its_state_and_the_pictures_of_the_slots_in_view() {
    let t = Tmp::new("pictures");
    let mut c = prepared(&t, 24);
    settle(&mut c, "the first thumbnail", |c| {
        c.arrangement().video_has_thumb(0)
    });
    assert!(c.thumbs_running());
    let feed = c.arrangement().video_feed().clone();
    assert_eq!(feed.preview_frames, Some(24));
    assert!(!feed.missing);
    // The film starts at 01:00:00:00; the playhead is at the start of the song.
    assert_eq!(feed.playhead_timecode, "01:00:00:00");
    assert_eq!(feed.start_timecode, "01:00:00:00");
    assert!(feed.end_timecode.starts_with("01:00:10:"), "{feed:?}");
    // Every slot in view ends up with its picture (the preview has 24 frames).
    let slots = c.arrangement().video_slots();
    assert!(!slots.is_empty());
    settle(&mut c, "every visible slot has its picture", |c| {
        c.arrangement()
            .video_slots()
            .iter()
            .all(|s| c.arrangement().video_has_thumb(s.frame))
    });
}

#[test]
fn the_timecode_follows_the_playhead() {
    let t = Tmp::new("timecode");
    let mut c = prepared(&t, 24);
    let tick = atomik_model::audio_trim::TempoMap::new(&c.doc().song.tempo)
        .tick_at_seconds(2.0)
        .round() as i64;
    c.locate(tick);
    c.frame();
    // 2 s at 23.976 fps is frame 47 (floor of 47.95): 01:00:01:23.
    assert_eq!(
        c.arrangement().video_feed().playhead_timecode,
        "01:00:01:23"
    );
}

#[test]
fn a_trimmed_film_shows_the_timecodes_of_its_trim() {
    let t = Tmp::new("trim");
    let mut c = prepared(&t, 24);
    c.apply_command(Command::Video(VideoCommand::Trim {
        in_us: 1_000_000,
        out_us: 4_000_000,
        merge: None,
    }))
    .unwrap();
    c.frame();
    let feed = c.arrangement().video_feed();
    // 1 s in is frame 23 (floor of 23.98): 01:00:00:23. 4 s is frame 95: 01:00:03:23.
    assert_eq!(feed.start_timecode, "01:00:00:23");
    assert_eq!(feed.end_timecode, "01:00:03:23");
}

#[test]
fn a_collapsed_or_hidden_row_closes_the_worker_and_opening_it_starts_it_again() {
    let t = Tmp::new("collapse");
    let mut c = prepared(&t, 24);
    settle(&mut c, "the worker", |c| c.thumbs_running());
    c.arrangement_mut().set_video_row_collapsed(true);
    c.frame();
    assert!(!c.thumbs_running());
    assert_eq!(c.arrangement().video_thumbs().0, 0, "the pictures go too");
    // The state still reaches the (thin) row.
    assert_eq!(c.arrangement().video_feed().preview_frames, Some(24));
    c.arrangement_mut().set_video_row_collapsed(false);
    settle(&mut c, "the worker again", |c| c.thumbs_running());
    settle(&mut c, "a picture again", |c| {
        c.arrangement().video_has_thumb(0)
    });
    c.arrangement_mut().set_video_row(false);
    c.frame();
    assert!(!c.thumbs_running());
    c.arrangement_mut().set_video_row(true);
    settle(&mut c, "the worker after showing", |c| c.thumbs_running());
}

#[test]
fn a_song_without_a_film_runs_nothing() {
    let t = Tmp::new("none");
    let mut c = core(&t, 4);
    c.frame();
    assert!(!c.thumbs_running());
    assert_eq!(c.arrangement().video_row_rect().height(), 0.0);
    assert_eq!(c.arrangement().video_thumbs().0, 0);
}

#[test]
fn a_film_without_a_preview_says_why_and_asks_for_no_pictures() {
    let t = Tmp::new("nopreview");
    let mut c = core(&t, 4);
    let facts = VideoFacts {
        duration_us: 5_000_000,
        fps_num: 25,
        fps_den: 1,
        width: 640,
        height: 360,
        timecode: None,
        has_audio: false,
    };
    c.apply_command(Command::Video(VideoCommand::Set(VideoClip::new(
        t.film().to_string_lossy(),
        facts,
    ))))
    .unwrap();
    c.frame();
    let feed = c.arrangement().video_feed();
    assert_eq!(feed.preview_frames, None);
    assert!(!feed.note.is_empty(), "{feed:?}");
    assert!(!c.thumbs_running());
    assert!(c.arrangement().video_slots().is_empty());
}

#[test]
fn a_missing_file_reaches_the_row_as_missing() {
    let t = Tmp::new("missing");
    let mut c = core(&t, 4);
    let facts = VideoFacts {
        duration_us: 5_000_000,
        fps_num: 25,
        fps_den: 1,
        width: 640,
        height: 360,
        timecode: None,
        has_audio: false,
    };
    let gone = t.path("not-there.mov");
    c.apply_command(Command::Video(VideoCommand::Set(VideoClip::new(
        gone.to_string_lossy(),
        facts,
    ))))
    .unwrap();
    c.frame();
    assert!(c.arrangement().video_feed().missing);
    assert!(!c.thumbs_running());
}
