//! Video in the app core, with the fake tool: no FFmpeg is needed. Looking for FFmpeg (found,
//! missing, no tools at all), preparing a film (progress, the toast, the cache, a second use),
//! cancelling, and the refusals.

use super::*;
use crate::sink::RecordingSink;
use atomik_video::fake::{FakeTool, tiny_jpeg};
use atomik_video::{FfmpegStatus, Platform};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("atomik-app-video-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    /// A folder holding stand-ins for the two programs (the fake tool answers for them).
    fn programs(&self) -> PathBuf {
        let dir = self.path("ffmpeg-folder");
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["ffmpeg", "ffprobe"] {
            std::fs::write(
                dir.join(Platform::current().exe_name(name)),
                b"not a program",
            )
            .unwrap();
        }
        dir
    }

    fn film(&self) -> PathBuf {
        let p = self.path("film.mp4");
        std::fs::write(&p, b"pretend film").unwrap();
        p
    }

    fn cache(&self) -> PathBuf {
        self.path("cache")
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn core(t: &Tmp, tool: Arc<FakeTool>) -> AppCore<RecordingSink> {
    let mut c = AppCore::with_doc(RecordingSink::new(), atomik_model::empty_doc(), None);
    c.set_video_tool(tool);
    c.set_video_folder(Some(t.programs()));
    c.set_video_cache(t.cache());
    c
}

fn frames(n: u8) -> Vec<Vec<u8>> {
    (0..n).map(|i| tiny_jpeg(640, 360, i)).collect()
}

fn toast<S: EngineSink>(c: &AppCore<S>) -> String {
    c.last_rejection().unwrap_or_default().to_owned()
}

#[test]
fn a_core_without_tools_never_starts_a_program_and_reports_it() {
    let mut c = AppCore::with_doc(RecordingSink::new(), atomik_model::empty_doc(), None);
    assert_eq!(*c.video_status(), FfmpegStatus::Unchecked);
    c.check_ffmpeg().unwrap();
    assert!(c.video_busy());
    c.wait_background();
    assert!(!c.video_busy());
    // Nothing is found or nothing can run: either way video is off and the message says so.
    assert!(!c.video_status().is_usable());
    assert!(!toast(&c).is_empty());
}

#[test]
fn checking_finds_the_programs_in_the_settings_folder_and_says_the_version() {
    let t = Tmp::new("check");
    let tool = Arc::new(FakeTool::new().version("ffmpeg version 7.1.1 Copyright (c)"));
    let mut c = core(&t, Arc::clone(&tool));
    c.check_ffmpeg().unwrap();
    c.wait_background();
    let tools = c.video_status().tools().expect("found");
    assert_eq!(tools.version.to_string(), "FFmpeg 7.1");
    assert!(tools.ffmpeg.starts_with(t.programs()));
    let message = toast(&c);
    assert!(message.starts_with("FFmpeg 7.1 found at"), "{message}");
    assert_eq!(tool.calls().len(), 1);
}

#[test]
fn a_settings_folder_without_ffmpeg_turns_video_off_with_the_install_hint() {
    let t = Tmp::new("missing");
    let mut c = core(&t, Arc::new(FakeTool::new()));
    c.set_video_folder(Some(t.path("nowhere")));
    c.check_ffmpeg().unwrap();
    c.wait_background();
    assert!(matches!(c.video_status(), FfmpegStatus::Missing { .. }));
    let message = toast(&c);
    assert!(message.contains("FFmpeg was not found in"), "{message}");
    assert!(
        message.contains("Install it and set its folder here"),
        "{message}"
    );
}

#[test]
fn an_old_ffmpeg_is_reported_as_too_old() {
    let t = Tmp::new("old");
    let mut c = core(
        &t,
        Arc::new(FakeTool::new().version("ffmpeg version 3.4.13 x")),
    );
    c.check_ffmpeg().unwrap();
    c.wait_background();
    assert!(matches!(c.video_status(), FfmpegStatus::TooOld { .. }));
    assert!(toast(&c).contains("too old"));
}

#[test]
fn changing_the_folder_forgets_what_was_found() {
    let t = Tmp::new("folder");
    let mut c = core(&t, Arc::new(FakeTool::new()));
    c.check_ffmpeg().unwrap();
    c.wait_background();
    assert!(c.video_status().is_usable());
    // The same folder keeps the answer; another one, or none, asks again.
    c.set_video_folder(Some(t.programs()));
    assert!(c.video_status().is_usable());
    c.set_video_folder(None);
    assert_eq!(*c.video_status(), FfmpegStatus::Unchecked);
    assert_eq!(c.video_folder(), None);
    c.set_video_folder(Some(PathBuf::new()));
    assert_eq!(c.video_folder(), None);
}

#[test]
fn preparing_a_film_builds_its_proxy_and_keeps_the_result() {
    let t = Tmp::new("prepare");
    let tool = Arc::new(FakeTool::new().frames(frames(12)).chunk(500));
    let mut c = core(&t, Arc::clone(&tool));
    let film = t.film();
    c.start_prepare_video(film.clone()).unwrap();
    assert!(c.video_busy());
    assert!(toast(&c).starts_with("Preparing video film.mp4"));
    c.wait_background();
    assert!(!c.video_busy());
    assert_eq!(c.video_progress(), None);
    let message = toast(&c);
    assert!(
        message.starts_with("Prepared film.mp4: 12.0 s at 25.000 fps"),
        "{message}"
    );
    assert!(message.contains("proxy 640x360 (built)"), "{message}");
    let p = c.prepared_video(&film).expect("kept");
    assert_eq!(p.info.frames, 12);
    assert_eq!(c.prepared_videos().len(), 1);
    assert!(p.proxy.starts_with(t.cache()));
    assert!(c.video_status().is_usable());
    // A second preparation of the same film comes from the cache and keeps one entry.
    c.start_prepare_video(film.clone()).unwrap();
    c.wait_background();
    assert!(toast(&c).contains("(from the cache)"), "{}", toast(&c));
    assert_eq!(c.prepared_videos().len(), 1);
    let streams = tool.calls().iter().filter(|c| c.has("-nostdin")).count();
    assert_eq!(streams, 1);
}

#[test]
fn preparing_without_ffmpeg_says_how_to_get_it_and_changes_nothing() {
    let t = Tmp::new("nofftmpeg");
    let mut c = core(&t, Arc::new(FakeTool::new()));
    c.set_video_folder(Some(t.path("nowhere")));
    c.start_prepare_video(t.film()).unwrap();
    c.wait_background();
    assert!(!c.video_busy());
    assert!(c.prepared_videos().is_empty());
    assert!(toast(&c).contains("video needs it"), "{}", toast(&c));
    assert!(!t.cache().exists());
}

#[test]
fn a_failing_ffmpeg_is_a_toast_not_a_crash_and_leaves_no_proxy() {
    let t = Tmp::new("fail");
    let tool = FakeTool::new()
        .frames(frames(3))
        .stream_fails("Invalid data found");
    let mut c = core(&t, Arc::new(tool));
    c.start_prepare_video(t.film()).unwrap();
    c.wait_background();
    let message = toast(&c);
    assert!(
        message.starts_with("Preparing film.mp4 failed:"),
        "{message}"
    );
    assert!(message.contains("Invalid data found"), "{message}");
    assert!(c.prepared_videos().is_empty());
    let left = std::fs::read_dir(t.cache()).map(|d| d.count()).unwrap_or(0);
    assert_eq!(left, 0);
}

#[test]
fn a_job_in_progress_shows_its_progress_and_cancel_removes_the_partial_proxy() {
    let t = Tmp::new("cancel");
    let reached = Arc::new(AtomicBool::new(false));
    let go = Arc::new(AtomicBool::new(false));
    let (r, g) = (Arc::clone(&reached), Arc::clone(&go));
    let tool = FakeTool::new().frames(frames(40)).on_frame(move |n| {
        if n == 2 {
            r.store(true, Ordering::SeqCst);
            while !g.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    });
    let mut c = core(&t, Arc::new(tool));
    c.start_prepare_video(t.film()).unwrap();
    while !reached.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(2));
    }
    let (fraction, text) = c.video_progress().expect("a prepare job has progress");
    assert!((fraction - 2.0 / 300.0).abs() < 1e-6, "{fraction}");
    assert_eq!(text, "Preparing video film.mp4: 1%");
    // One job at a time.
    assert!(c.start_prepare_video(t.film()).is_err());
    assert!(c.check_ffmpeg().is_err());
    assert!(toast(&c).contains("already running"));
    c.cancel_video();
    go.store(true, Ordering::SeqCst);
    c.wait_background();
    assert!(!c.video_busy());
    assert_eq!(toast(&c), "Preparing film.mp4 cancelled");
    assert!(c.prepared_videos().is_empty());
    let left = std::fs::read_dir(t.cache()).map(|d| d.count()).unwrap_or(0);
    assert_eq!(left, 0, "no proxy and no part file");
}

#[test]
fn a_path_that_is_not_a_file_and_a_cancel_with_nothing_running_are_refused() {
    let t = Tmp::new("refuse");
    let mut c = core(&t, Arc::new(FakeTool::new()));
    let err = c.start_prepare_video(t.path("nothing.mp4")).unwrap_err();
    assert!(err.contains("is not a file"), "{err}");
    assert!(!c.video_busy());
    c.cancel_video();
    assert_eq!(toast(&c), "No video job is running");
}

#[test]
fn dropping_the_core_stops_a_job_that_runs() {
    let t = Tmp::new("drop");
    let reached = Arc::new(AtomicBool::new(false));
    let go = Arc::new(AtomicBool::new(false));
    let (r, g) = (Arc::clone(&reached), Arc::clone(&go));
    let tool = FakeTool::new().frames(frames(40)).on_frame(move |n| {
        if n == 1 {
            r.store(true, Ordering::SeqCst);
            while !g.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    });
    {
        let mut c = core(&t, Arc::new(tool));
        c.start_prepare_video(t.film()).unwrap();
        while !reached.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    go.store(true, Ordering::SeqCst);
    // The worker sees the flag the core set when it dropped, stops and removes its part file.
    for _ in 0..500 {
        let left = std::fs::read_dir(t.cache()).map(|d| d.count()).unwrap_or(0);
        if left == 0 {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("the part file stayed");
}
