//! The Video window in the app core, with the fake tool (no FFmpeg is needed): the picture at
//! the song position while stopped (locate, scrub, before and after the film, a start offset, a
//! trim), the decode-ahead while it plays, the timecode, what the window says when it has no
//! picture, and the proxy being built when a project with a film opens.
//!
//! The fake pipes real (tiny, solid colour) JPEGs, frame `n` being red, green, blue or white
//! for `n % 4` = 0, 1, 2 or 3, so a test sees which frame the window holds from its pixels.

use super::*;
use crate::sink::RecordingSink;
use atomik_model::{VideoClip, VideoCommand, VideoFacts};
use atomik_ui::VideoScreen;
use atomik_video::fake::{FakeTool, Solid, solid_jpeg};
use atomik_video::{Platform, samples};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// The film is 24000/1001 fps: a frame lasts 41.708 ms.
const FRAME_S: f64 = 1001.0 / 24000.0;
const RED: [u8; 3] = [255, 0, 0];
const GREEN: [u8; 3] = [0, 255, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const WHITE: [u8; 3] = [255, 255, 255];

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "atomik-app-video-view-{tag}-{}-{n}",
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

fn tool(frames: usize) -> FakeTool {
    let colours = [Solid::Red, Solid::Green, Solid::Blue, Solid::White];
    FakeTool::new().probe(samples::MOV_PRORES).frames(
        (0..frames)
            .map(|n| solid_jpeg(colours[n % 4]).to_vec())
            .collect(),
    )
}

fn core(t: &Tmp, tool: FakeTool) -> AppCore<RecordingSink> {
    let mut c = AppCore::with_doc(RecordingSink::new(), atomik_model::empty_doc(), None);
    c.set_video_tool(Arc::new(tool));
    c.set_video_folder(Some(t.programs()));
    c.set_video_cache(t.path("cache"));
    c
}

/// A core whose song has the film imported and prepared, with the Video window open.
fn viewing(t: &Tmp, frames: usize) -> AppCore<RecordingSink> {
    let mut c = core(t, tool(frames));
    c.import_video(t.film()).unwrap();
    c.wait_background();
    assert!(c.video_prepared().is_some(), "{:?}", c.last_rejection());
    c.set_video_shown(true);
    c
}

fn tick_at(c: &AppCore<RecordingSink>, seconds: f64) -> i64 {
    atomik_model::audio_trim::TempoMap::new(&c.doc().song.tempo)
        .tick_at_seconds(seconds)
        .round() as i64
}

/// What the window holds now: the colour of its picture (`Ok`) or what else it shows.
#[derive(Debug, PartialEq)]
enum Held {
    Picture([u8; 3]),
    Black,
    Message(String),
    Progress,
}

fn held(c: &AppCore<RecordingSink>) -> Held {
    match &c.video_window().frame().screen {
        VideoScreen::Picture(image) => {
            let at = ((image.height / 2 * image.width + image.width / 2) * 4) as usize;
            Held::Picture([image.rgba[at], image.rgba[at + 1], image.rgba[at + 2]])
        }
        VideoScreen::Black => Held::Black,
        VideoScreen::Message(m) => Held::Message(m.clone()),
        VideoScreen::Progress(..) => Held::Progress,
    }
}

fn near(a: [u8; 3], b: [u8; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= 24)
}

/// Runs frames until the window holds `want` (the worker decodes in the background).
fn settle(c: &mut AppCore<RecordingSink>, want: [u8; 3]) {
    for _ in 0..600 {
        c.frame();
        if matches!(held(c), Held::Picture(p) if near(p, want)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("the window never held {want:?}: {:?}", held(c));
}

fn settle_black(c: &mut AppCore<RecordingSink>) {
    for _ in 0..50 {
        c.frame();
        if held(c) == Held::Black {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("the window is not black: {:?}", held(c));
}

/// Moves the stopped song to `seconds` (a locate, a ruler click or a scrub does the same).
fn go(c: &mut AppCore<RecordingSink>, seconds: f64) {
    let tick = tick_at(c, seconds);
    c.locate(tick);
}

#[test]
fn stopped_the_window_shows_the_frame_at_the_position() {
    let t = Tmp::new("stopped");
    let mut c = viewing(&t, 24);
    // The middle of frame n is (n + 0.5) frames in; frame n is red, green, blue or white.
    for (n, want) in [
        (0, RED),
        (1, GREEN),
        (2, BLUE),
        (3, WHITE),
        (4, RED),
        (9, GREEN),
    ] {
        go(&mut c, (f64::from(n) + 0.5) * FRAME_S);
        settle(&mut c, want);
    }
    // Back to an earlier frame (a scrub backwards).
    go(&mut c, 2.5 * FRAME_S);
    settle(&mut c, BLUE);
    go(&mut c, 0.5 * FRAME_S);
    settle(&mut c, RED);
}

#[test]
fn a_frame_changes_when_the_position_crosses_its_start() {
    let t = Tmp::new("edge");
    let mut c = viewing(&t, 24);
    // 5 ms before and 5 ms after the start of frame 1.
    go(&mut c, FRAME_S - 0.005);
    settle(&mut c, RED);
    go(&mut c, FRAME_S + 0.005);
    settle(&mut c, GREEN);
}

#[test]
fn before_the_start_and_after_the_end_it_is_black_with_the_timecode() {
    let t = Tmp::new("outside");
    let mut c = viewing(&t, 24);
    // The film starts 2 s into the song.
    c.apply_command(Command::Video(VideoCommand::Move {
        start_us: 2_000_000,
        merge: None,
    }))
    .unwrap();
    go(&mut c, 1.0);
    settle_black(&mut c);
    // One second before the start is 24 frames before frame 0 (rounded down at 23.976 fps):
    // the timecode of a film that starts at 01:00:00:00 is 00:59:59:00 one second before.
    let tc = c.video_window().frame().timecode.clone();
    assert_eq!(tc, "00:59:59:00", "{tc}");
    assert_eq!(c.video_window().frame().frame_number, Some(-24));
    // 2.5 frames after the start: the picture and its timecode.
    go(&mut c, 2.0 + 2.5 * FRAME_S);
    settle(&mut c, BLUE);
    assert_eq!(c.video_window().frame().timecode, "01:00:00:02");
    // Far past the end of the film (10.4 s long, starting at 2 s).
    go(&mut c, 60.0);
    settle_black(&mut c);
    assert!(c.video_window().frame().timecode.starts_with("01:00:"));
    assert_eq!(c.video_window().frame().frame_number, Some(1_390));
}

#[test]
fn the_trim_decides_which_frame_plays_at_the_start() {
    let t = Tmp::new("trim");
    let mut c = viewing(&t, 24);
    // Trim the film's first 3 frames away at the playhead: frame 3 (white) now plays at the
    // song position of the trim point.
    go(&mut c, 3.0 * FRAME_S + 0.001);
    c.trim_video_start_to_playhead();
    let clip = c.video_clip().unwrap().clone();
    assert!(
        clip.in_us > 100_000 && clip.in_us < 130_000,
        "{}",
        clip.in_us
    );
    settle(&mut c, WHITE);
    go(&mut c, 4.5 * FRAME_S);
    settle(&mut c, RED);
    // The song's start is now before the film: black.
    go(&mut c, 0.0);
    settle_black(&mut c);
}

#[test]
fn a_tempo_change_does_not_move_the_picture() {
    let t = Tmp::new("tempo");
    let mut c = viewing(&t, 24);
    go(&mut c, 2.5 * FRAME_S);
    settle(&mut c, BLUE);
    let before = c.video_clip().unwrap().start_us;
    // Another tempo: the same time in seconds is a different tick, and the same frame.
    c.apply_command(Command::Song(atomik_model::SongCommand::SetTempoMap(vec![
        atomik_model::TempoEvent::step(0, 60.0),
    ])))
    .unwrap();
    go(&mut c, 2.5 * FRAME_S);
    settle(&mut c, BLUE);
    assert_eq!(c.video_clip().unwrap().start_us, before);
}

#[test]
fn playing_decodes_ahead_and_follows_the_position() {
    let t = Tmp::new("play");
    let mut c = viewing(&t, 24);
    go(&mut c, 0.5 * FRAME_S);
    settle(&mut c, RED);
    c.playing = true;
    // The next frames are asked for and arrive without the position moving.
    for _ in 0..600 {
        c.frame();
        if c.viewer_cached().len() >= 10 {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let held_frames = c.viewer_cached();
    assert!(
        (0..=8).all(|n| held_frames.contains(&n)),
        "decoded ahead: {held_frames:?}"
    );
    // The position moves on by several frames at once (a slow UI): the window shows the newest
    // frame, it does not play the ones in between.
    go(&mut c, 14.5 * FRAME_S);
    settle(&mut c, BLUE);
    c.playing = false;
}

#[test]
fn near_the_end_of_a_loop_the_start_of_the_loop_is_decoded_ahead() {
    let t = Tmp::new("loop");
    let mut c = viewing(&t, 24);
    let (start, end) = (tick_at(&c, 8.0 * FRAME_S), tick_at(&c, 16.0 * FRAME_S));
    c.apply_command(Command::Song(atomik_model::SongCommand::SetLoop {
        range: (start, end),
        enabled: true,
    }))
    .unwrap();
    c.playing = true;
    go(&mut c, 14.5 * FRAME_S);
    settle(&mut c, BLUE);
    for _ in 0..600 {
        c.frame();
        if c.viewer_cached().contains(&8) {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(c.viewer_cached().contains(&8), "{:?}", c.viewer_cached());
    // The wrap: the position jumps back to the start of the loop, and so does the picture.
    go(&mut c, 8.5 * FRAME_S);
    settle(&mut c, RED);
    c.playing = false;
}

#[test]
fn the_window_says_why_it_has_no_picture() {
    let t = Tmp::new("messages");
    let mut c = core(&t, tool(8));
    c.set_video_shown(true);
    c.frame();
    let Held::Message(none) = held(&c) else {
        panic!("{:?}", held(&c))
    };
    assert!(none.starts_with("The song has no video"), "{none}");
    // A film that is not there.
    let missing = t.path("gone.mov");
    c.apply_command(Command::Video(VideoCommand::Set(VideoClip::new(
        missing.to_string_lossy().into_owned(),
        facts(),
    ))))
    .unwrap();
    c.frame();
    let Held::Message(gone) = held(&c) else {
        panic!("{:?}", held(&c))
    };
    assert!(gone.contains("gone.mov is missing"), "{gone}");
    assert!(gone.contains("Relink Video"), "{gone}");
    assert!(
        c.video_window().frame().timecode.is_empty(),
        "no timecode over a message"
    );
}

fn facts() -> VideoFacts {
    VideoFacts {
        duration_us: 10_427_083,
        fps_num: 24_000,
        fps_den: 1_001,
        width: 1920,
        height: 1080,
        timecode: Some("01:00:00:00".to_owned()),
        has_audio: true,
    }
}

#[test]
fn a_film_without_ffmpeg_says_to_install_it() {
    let t = Tmp::new("noffmpeg");
    let mut c = core(&t, tool(8));
    c.set_video_folder(Some(t.path("nowhere")));
    let film = t.film();
    c.apply_command(Command::Video(VideoCommand::Set(VideoClip::new(
        film.to_string_lossy().into_owned(),
        facts(),
    ))))
    .unwrap();
    c.set_video_shown(true);
    c.wait_background();
    c.frame();
    let Held::Message(text) = held(&c) else {
        panic!("{:?}", held(&c))
    };
    assert!(text.contains("FFmpeg"), "{text}");
    assert!(text.contains("video needs it"), "{text}");
    // It is said in the window, not as a toast.
    assert!(
        !c.last_rejection()
            .unwrap_or_default()
            .contains("video needs it")
    );
}

#[test]
fn the_progress_of_the_proxy_shows_in_the_window() {
    let t = Tmp::new("progress");
    let reached = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let go_on = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (r, g) = (Arc::clone(&reached), Arc::clone(&go_on));
    let slow = tool(24).on_frame(move |n| {
        if n == 3 {
            r.store(true, Ordering::SeqCst);
            while !g.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    });
    let mut c = core(&t, slow);
    c.set_video_shown(true);
    c.import_video(t.film()).unwrap();
    while !reached.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(2));
    }
    for _ in 0..200 {
        c.frame();
        if held(&c) == Held::Progress {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(held(&c), Held::Progress);
    let VideoScreen::Progress(fraction, text) = &c.video_window().frame().screen else {
        panic!()
    };
    assert!(*fraction > 0.0 && *fraction < 1.0, "{fraction}");
    assert!(text.starts_with("Preparing video scene.mov"), "{text}");
    go_on.store(true, Ordering::SeqCst);
    c.wait_background();
    c.frame();
    go(&mut c, 0.5 * FRAME_S);
    settle(&mut c, RED);
}

#[test]
fn opening_a_project_with_a_film_builds_its_proxy_quietly_once() {
    let t = Tmp::new("open");
    let tool = Arc::new(tool(24));
    let mut c = AppCore::with_doc(RecordingSink::new(), atomik_model::empty_doc(), None);
    c.set_video_tool(Arc::clone(&tool) as Arc<dyn atomik_video::VideoTool>);
    c.set_video_folder(Some(t.programs()));
    c.set_video_cache(t.path("cache"));
    let film = t.film();
    c.apply_command(Command::Video(VideoCommand::Set(VideoClip::new(
        film.to_string_lossy().into_owned(),
        facts(),
    ))))
    .unwrap();
    assert!(c.video_prepared().is_none());
    // What opening a project does: the build starts on the next frame, with no toast.
    c.video_opened();
    assert!(!c.video_busy(), "nothing starts before the frame");
    c.frame();
    assert!(c.video_busy());
    assert!(
        !c.last_rejection().unwrap_or_default().contains("Preparing"),
        "{:?}",
        c.last_rejection()
    );
    c.wait_background();
    assert!(c.video_prepared().is_some());
    assert!(
        !c.last_rejection()
            .unwrap_or_default()
            .starts_with("Prepared")
    );
    // The proxy is there: opening the window starts nothing.
    c.set_video_shown(true);
    assert!(!c.video_busy());
    let streams = tool
        .calls()
        .iter()
        .filter(|call| call.has("-nostdin"))
        .count();
    assert_eq!(streams, 1);
}

#[test]
fn a_core_without_tools_and_cache_never_starts_a_build_at_open() {
    let mut c = AppCore::with_doc(RecordingSink::new(), atomik_model::empty_doc(), None);
    c.apply_command(Command::Video(VideoCommand::Set(VideoClip::new(
        "/somewhere/film.mov",
        facts(),
    ))))
    .unwrap();
    c.video_opened();
    c.frame();
    assert!(!c.video_busy());
}

#[test]
fn a_build_that_failed_is_not_started_again_by_the_window() {
    let t = Tmp::new("retry");
    let failing = tool(24).stream_fails("Invalid data found");
    let mut c = core(&t, failing);
    let film = t.film();
    c.apply_command(Command::Video(VideoCommand::Set(VideoClip::new(
        film.to_string_lossy().into_owned(),
        facts(),
    ))))
    .unwrap();
    c.set_video_shown(true);
    assert!(c.video_busy(), "opening the window starts the build");
    c.wait_background();
    assert!(c.video_prepared().is_none());
    c.frame();
    c.frame();
    assert!(!c.video_busy(), "no endless retries");
    let Held::Message(text) = held(&c) else {
        panic!("{:?}", held(&c))
    };
    assert!(text.contains("Prepare Video"), "{text}");
}

#[test]
fn closing_the_window_frees_the_pictures_and_the_worker() {
    let t = Tmp::new("close");
    let mut c = viewing(&t, 24);
    go(&mut c, 0.5 * FRAME_S);
    settle(&mut c, RED);
    assert!(!c.viewer_cached().is_empty());
    c.set_video_shown(false);
    assert!(c.viewer_cached().is_empty());
    assert!(!c.video_shown());
    // Opened again, it decodes again.
    c.set_video_shown(true);
    settle(&mut c, RED);
}

#[test]
fn the_overlay_command_switches_it_off_and_on() {
    let t = Tmp::new("overlay");
    let mut c = core(&t, tool(4));
    assert!(c.video_window().prefs().overlay.timecode);
    c.toggle_video_overlay();
    assert!(!c.video_window().prefs().overlay.any());
    c.toggle_video_overlay();
    assert!(c.video_window().prefs().overlay.timecode);
}
