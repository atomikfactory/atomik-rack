//! The film of the song in the app core, with the fake tool (no FFmpeg is needed): Import Video
//! (the probe puts the film in the song, the proxy builds on, undo), what it says without
//! FFmpeg, replacing a film, Relink Video (and its warning), a missing file at open, and the
//! Move and Trim commands through the playhead.

use super::*;
use crate::sink::RecordingSink;
use atomik_model::{VideoClip, VideoFacts};
use atomik_video::fake::{FakeTool, tiny_jpeg};
use atomik_video::{Platform, samples};
use std::sync::atomic::{AtomicU64, Ordering};

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "atomik-app-video-clip-{tag}-{}-{n}",
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

    fn film(&self, name: &str) -> PathBuf {
        let p = self.path(name);
        std::fs::write(&p, name.as_bytes()).unwrap();
        p
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn core(t: &Tmp, tool: FakeTool) -> AppCore<RecordingSink> {
    let mut c = AppCore::with_doc(RecordingSink::new(), atomik_model::empty_doc(), None);
    c.set_video_tool(Arc::new(tool));
    c.set_video_folder(Some(t.programs()));
    c.set_video_cache(t.path("cache"));
    c
}

fn tool() -> FakeTool {
    FakeTool::new().frames((0..6).map(|i| tiny_jpeg(640, 360, i)).collect())
}

fn toast<S: EngineSink>(c: &AppCore<S>) -> String {
    c.last_rejection().unwrap_or_default().to_owned()
}

fn clip(c: &AppCore<RecordingSink>) -> VideoClip {
    c.video_clip().cloned().expect("the song has a video")
}

#[test]
fn importing_puts_the_film_at_the_song_start_and_builds_its_proxy() {
    let t = Tmp::new("import");
    let mut c = core(&t, tool());
    assert!(c.video_clip().is_none());
    let film = t.film("scene.mp4");
    c.import_video(film.clone()).unwrap();
    assert!(toast(&c).starts_with("Importing video scene.mp4"));
    c.wait_background();
    let v = clip(&c);
    assert_eq!(PathBuf::from(&v.path), film);
    assert_eq!((v.start_us, v.in_us, v.out_us), (0, 0, 12_021_333));
    assert_eq!(
        v.facts,
        VideoFacts {
            duration_us: 12_021_333,
            fps_num: 25,
            fps_den: 1,
            width: 1920,
            height: 1080,
            timecode: None,
            has_audio: true,
        }
    );
    assert!(!c.video_missing());
    // The proxy was built after the film was in the song.
    assert!(c.video_prepared().is_some());
    assert!(toast(&c).starts_with("Prepared scene.mp4"), "{}", toast(&c));
    // One undo step takes it out and a redo puts it back.
    assert!(c.undo());
    assert!(c.video_clip().is_none());
    assert!(c.redo());
    assert_eq!(clip(&c), v);
}

#[test]
fn the_film_is_in_the_song_as_soon_as_the_probe_is_in() {
    let t = Tmp::new("early");
    let mut c = core(&t, tool());
    c.import_video(t.film("scene.mp4")).unwrap();
    // Drain until the song has the film: the job may still be building the proxy.
    while c.video_clip().is_none() {
        c.wait_background();
    }
    assert!(c.undo(), "the import is one undo step");
    c.wait_background();
}

#[test]
fn importing_without_ffmpeg_changes_nothing_and_says_how_to_set_it_up() {
    let t = Tmp::new("none");
    let mut c = core(&t, FakeTool::new());
    c.set_video_folder(Some(t.path("nowhere")));
    c.import_video(t.film("scene.mp4")).unwrap();
    c.wait_background();
    assert!(c.video_clip().is_none());
    assert!(!c.undo(), "nothing was done");
    assert!(toast(&c).contains("video needs it"), "{}", toast(&c));
    assert!(toast(&c).contains("Install it"), "{}", toast(&c));
}

#[test]
fn a_file_that_cannot_be_probed_is_a_toast_and_leaves_the_song_alone() {
    let t = Tmp::new("badfile");
    let mut c = core(&t, FakeTool::new().probe_fails());
    c.import_video(t.film("notes.txt")).unwrap();
    c.wait_background();
    assert!(c.video_clip().is_none());
    assert!(
        toast(&c).starts_with("Cannot use notes.txt:"),
        "{}",
        toast(&c)
    );
    let err = c.import_video(t.path("absent.mp4")).unwrap_err();
    assert!(err.contains("is not a file"), "{err}");
}

#[test]
fn importing_again_replaces_the_film_in_one_undoable_step() {
    let t = Tmp::new("replace");
    let mut c = core(&t, tool());
    c.import_video(t.film("one.mp4")).unwrap();
    c.wait_background();
    c.move_video_to_playhead();
    let first = clip(&c);
    c.set_video_tool(Arc::new(tool().probe(samples::NO_AUDIO)));
    c.import_video(t.film("two.mp4")).unwrap();
    c.wait_background();
    let second = clip(&c);
    assert!(second.path.ends_with("two.mp4"));
    assert_eq!(second.facts.duration_us, 5_000_000);
    assert!(!second.facts.has_audio);
    assert_eq!(second.start_us, 0, "the new film starts at the song start");
    assert!(c.undo());
    assert_eq!(clip(&c), first);
}

#[test]
fn a_job_in_progress_refuses_a_second_one() {
    let t = Tmp::new("busy");
    let mut c = core(&t, tool());
    c.import_video(t.film("one.mp4")).unwrap();
    let err = c.import_video(t.film("two.mp4")).unwrap_err();
    assert!(err.contains("already running"), "{err}");
    c.wait_background();
}

fn with_missing_film(t: &Tmp) -> AppCore<RecordingSink> {
    let mut doc = atomik_model::empty_doc();
    let mut v = VideoClip::new(
        t.path("gone.mp4").to_string_lossy(),
        VideoFacts {
            duration_us: 12_021_333,
            fps_num: 25,
            fps_den: 1,
            width: 1920,
            height: 1080,
            timecode: None,
            has_audio: true,
        },
    );
    v.start_us = 3_000_000;
    v.in_us = 1_000_000;
    doc.video = Some(v);
    let mut c = AppCore::with_doc(RecordingSink::new(), doc, None);
    c.set_video_tool(Arc::new(tool()));
    c.set_video_folder(Some(t.programs()));
    c.set_video_cache(t.path("cache"));
    c
}

#[test]
fn a_song_whose_film_is_gone_opens_marked_missing() {
    let t = Tmp::new("missing");
    let c = with_missing_film(&t);
    assert!(c.video_clip().is_some());
    assert!(c.video_missing());
    assert!(
        toast(&c).contains("The video gone.mp4 is missing"),
        "{}",
        toast(&c)
    );
    assert!(toast(&c).contains("File > Video"), "{}", toast(&c));
    // The facts are there without FFmpeg.
    assert_eq!(clip(&c).facts.duration_us, 12_021_333);
    // A film that is there is not missing.
    std::fs::write(t.path("gone.mp4"), b"back").unwrap();
    c.forget_video_file();
    assert!(!c.video_missing());
}

#[test]
fn relinking_points_the_film_at_the_file_keeps_start_and_trim_and_is_one_undo_step() {
    let t = Tmp::new("relink");
    let mut c = with_missing_film(&t);
    let before = clip(&c);
    let found = t.film("found.mp4");
    c.relink_video(found.clone()).unwrap();
    assert!(toast(&c).starts_with("Relinking video to found.mp4"));
    c.wait_background();
    let v = clip(&c);
    assert_eq!(PathBuf::from(&v.path), found);
    assert_eq!(
        (v.start_us, v.in_us, v.out_us),
        (3_000_000, 1_000_000, 12_021_333)
    );
    assert!(!c.video_missing());
    // Same length and rate: no warning, and the proxy message follows.
    assert!(toast(&c).starts_with("Prepared found.mp4"), "{}", toast(&c));
    assert!(c.undo());
    assert_eq!(clip(&c), before);
    assert!(c.video_missing());
}

#[test]
fn relinking_to_another_length_or_rate_warns_and_keeps_the_warning_on_screen() {
    let t = Tmp::new("warn");
    let mut c = with_missing_film(&t);
    c.set_video_tool(Arc::new(tool().probe(samples::MOV_PRORES)));
    c.relink_video(t.film("other.mov")).unwrap();
    c.wait_background();
    let message = toast(&c);
    assert!(
        message.starts_with("Relinked the video to other.mov, but"),
        "{message}"
    );
    assert!(
        message.contains("the length is 10.43 s instead of 12.02 s"),
        "{message}"
    );
    assert!(
        message.contains("the frame rate is 23.976 fps instead of 25.000 fps"),
        "{message}"
    );
    assert!(message.contains("may not match"), "{message}");
    // The relink went through all the same.
    assert!(clip(&c).path.ends_with("other.mov"));
    assert_eq!(
        clip(&c).out_us,
        10_427_083,
        "the out point follows the shorter film"
    );
}

#[test]
fn relinking_needs_a_film_in_the_song_and_a_file() {
    let t = Tmp::new("relink-none");
    let mut c = core(&t, tool());
    let err = c.relink_video(t.film("a.mp4")).unwrap_err();
    assert_eq!(err, "The song has no video");
    let mut c = with_missing_film(&t);
    let err = c.relink_video(t.path("nothing.mp4")).unwrap_err();
    assert!(err.contains("is not a file"), "{err}");
}

#[test]
fn a_probe_that_arrives_for_another_project_is_not_put_in_this_one() {
    let t = Tmp::new("other-project");
    let mut c = core(&t, tool());
    c.import_video(t.film("scene.mp4")).unwrap();
    // The user opens another project while the probe runs.
    c.audio.generation += 1;
    c.wait_background();
    assert!(c.video_clip().is_none());
    assert!(
        toast(&c).contains("The project changed") || toast(&c).starts_with("Prepared"),
        "{}",
        toast(&c)
    );
}

fn at_bar(c: &mut AppCore<RecordingSink>, bars: i64) {
    // 120 BPM in 4/4: one bar is two seconds.
    c.transport_action(TransportAction::Locate(bars * 4 * atomik_core::PPQ));
}

#[test]
fn move_to_playhead_starts_the_film_at_the_playhead_in_seconds() {
    let t = Tmp::new("move");
    let mut c = core(&t, tool());
    c.import_video(t.film("scene.mp4")).unwrap();
    c.wait_background();
    at_bar(&mut c, 2);
    c.move_video_to_playhead();
    assert_eq!(clip(&c).start_us, 4_000_000);
    assert_eq!(toast(&c), "The video starts at 0:04.000");
    assert!(c.undo());
    assert_eq!(clip(&c).start_us, 0);
    // The same place again changes nothing (the model refuses and the toast says so).
    at_bar(&mut c, 0);
    c.move_video_to_playhead();
    assert!(toast(&c).starts_with("Video: "), "{}", toast(&c));
}

#[test]
fn a_tempo_change_does_not_move_the_film_in_time() {
    let t = Tmp::new("tempo");
    let mut c = core(&t, tool());
    c.import_video(t.film("scene.mp4")).unwrap();
    c.wait_background();
    at_bar(&mut c, 2);
    c.move_video_to_playhead();
    let at = clip(&c).start_us;
    c.apply_command(Command::SetTempo(60.0)).unwrap();
    assert_eq!(clip(&c).start_us, at);
    // At 60 BPM the same four seconds are bar 2 (four beats a bar).
    assert_eq!(clip(&c).start_tick(&c.doc().song), 4 * atomik_core::PPQ);
}

#[test]
fn trimming_to_the_playhead_cuts_the_start_and_the_end_with_undo() {
    let t = Tmp::new("trim");
    let mut c = core(&t, tool());
    c.import_video(t.film("scene.mp4")).unwrap();
    c.wait_background();
    // The playhead at 4 s of a film that starts at 0: cut the first four seconds.
    at_bar(&mut c, 2);
    c.trim_video_start_to_playhead();
    let v = clip(&c);
    assert_eq!(
        (v.start_us, v.in_us, v.out_us),
        (4_000_000, 4_000_000, 12_021_333)
    );
    assert_eq!(toast(&c), "The video now starts at 0:04.000");
    // And the end at 8 s.
    at_bar(&mut c, 4);
    c.trim_video_end_to_playhead();
    let v = clip(&c);
    assert_eq!(
        (v.start_us, v.in_us, v.out_us),
        (4_000_000, 4_000_000, 8_000_000)
    );
    assert_eq!(v.length_us(), 4_000_000);
    assert!(c.undo());
    assert_eq!(clip(&c).out_us, 12_021_333);
    assert!(c.undo());
    assert_eq!(clip(&c).in_us, 0);
    // Outside the film (here, past its end) nothing is trimmed.
    at_bar(&mut c, 20);
    c.trim_video_end_to_playhead();
    assert_eq!(toast(&c), "The playhead is outside the video");
    assert_eq!(clip(&c).out_us, 12_021_333);
}

#[test]
fn remove_takes_the_film_out_and_every_edit_needs_one() {
    let t = Tmp::new("remove");
    let mut c = core(&t, tool());
    for run in [
        |c: &mut AppCore<RecordingSink>| c.remove_video(),
        |c: &mut AppCore<RecordingSink>| c.move_video_to_playhead(),
        |c: &mut AppCore<RecordingSink>| c.trim_video_start_to_playhead(),
        |c: &mut AppCore<RecordingSink>| c.trim_video_end_to_playhead(),
    ] {
        run(&mut c);
        assert_eq!(toast(&c), "The song has no video");
    }
    c.import_video(t.film("scene.mp4")).unwrap();
    c.wait_background();
    c.remove_video();
    assert!(c.video_clip().is_none());
    assert_eq!(toast(&c), "Video scene.mp4 removed from the song");
    assert!(c.undo());
    assert!(c.video_clip().is_some());
}
