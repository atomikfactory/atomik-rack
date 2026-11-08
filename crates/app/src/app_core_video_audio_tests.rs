//! Import Video Audio in the app core, with the fake tool (no FFmpeg is needed): the track is
//! chosen (the only one, one of several through the list), the sound lands on a new track named
//! after the film, aligned with the film's first frame to the sample, in one undo step; keep with
//! video moves and trims it with the film; no FFmpeg, no audio and a stopped job leave the song
//! and its bundle alone.

use super::*;
use crate::sink::RecordingSink;
use atomik_model::{AudioLink, ClipContent, VideoCommand};
use atomik_video::fake::{FakeTool, tiny_jpeg, wav_pcm16};
use atomik_video::{Platform, VideoTool, samples};
use std::sync::atomic::{AtomicU64, Ordering};

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "atomik-app-video-audio-{tag}-{}-{n}",
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

/// A stereo WAV of `frames` frames at `rate`.
fn wav(rate: u32, frames: usize) -> Vec<u8> {
    let samples: Vec<i16> = (0..frames * 2).map(|i| (i % 2000) as i16).collect();
    wav_pcm16(rate, 2, &samples)
}

fn tool(rate: u32, frames: usize) -> FakeTool {
    FakeTool::new()
        .frames((0..6).map(|i| tiny_jpeg(640, 360, i)).collect())
        .audio(wav(rate, frames))
}

fn core(t: &Tmp, tool: impl VideoTool + 'static) -> AppCore<RecordingSink> {
    let mut c = AppCore::with_doc(RecordingSink::new(), atomik_model::empty_doc(), None);
    c.set_video_tool(Arc::new(tool));
    c.set_video_folder(Some(t.programs()));
    c.set_video_cache(t.path("cache"));
    c
}

/// A core with the film imported (the probe is the tool's).
fn with_film(t: &Tmp, tool: impl VideoTool + 'static, name: &str) -> AppCore<RecordingSink> {
    let mut c = core(t, tool);
    c.import_video(t.film(name)).unwrap();
    c.wait_background();
    assert!(c.video_clip().is_some());
    c
}

fn toast<S: EngineSink>(c: &AppCore<S>) -> String {
    c.last_rejection().unwrap_or_default().to_owned()
}

/// Every audio clip of the song: track, start, length, source offset, asset.
fn clips(c: &AppCore<RecordingSink>) -> Vec<(String, i64, i64, u64, String)> {
    let mut out = Vec::new();
    for t in &c.doc().song.tracks {
        for lane in &t.lanes {
            for clip in &lane.clips {
                if let ClipContent::Audio(a) = &clip.content {
                    out.push((
                        t.device.0.clone(),
                        clip.start,
                        clip.length,
                        a.source_offset,
                        a.asset.clone(),
                    ));
                }
            }
        }
    }
    out
}

fn frames_of(us: i64, rate: u32) -> i128 {
    (i128::from(us) * i128::from(rate) + 500_000).div_euclid(1_000_000)
}

/// The song frame the asset's frame 0 plays on, for the one clip.
fn asset_zero(c: &AppCore<RecordingSink>) -> i128 {
    let (_, start, _, offset, _) = clips(c).remove(0);
    i128::from(c.doc().song.frame_at(start, c.doc().sample_rate)) - i128::from(offset)
}

fn assets_in_bundle(c: &AppCore<RecordingSink>) -> usize {
    let Some(root) = c.bundle_root() else {
        return 0;
    };
    std::fs::read_dir(root.join("Audio"))
        .map_or(0, |d| d.flatten().filter(|e| e.path().is_file()).count())
}

#[test]
fn the_only_audio_track_goes_on_a_new_track_named_after_the_film_lined_up_with_the_picture() {
    let t = Tmp::new("one");
    let fake = Arc::new(tool(48_000, 96_000));
    let calls = Arc::clone(&fake);
    let mut c = with_film(&t, fake, "Night Drive.mp4");
    let steps = |c: &mut AppCore<RecordingSink>| {
        let mut n = 0;
        while c.undo() {
            n += 1;
        }
        for _ in 0..n {
            c.redo();
        }
        n
    };
    let before = steps(&mut c);
    c.import_video_audio(None).unwrap();
    assert!(
        toast(&c).starts_with("Importing the audio of Night Drive.mp4"),
        "{}",
        toast(&c)
    );
    c.wait_background();
    assert!(toast(&c).starts_with("Imported the audio of Night Drive.mp4"));
    // A new track named after the film, with the clip at the song's start and no skipping.
    let tracks = &c.doc().song.tracks;
    assert_eq!(tracks.len(), 1);
    let name = c
        .doc()
        .devices
        .iter()
        .find(|d| d.key == tracks[0].device)
        .map(|d| d.name.clone());
    assert_eq!(name.as_deref(), Some("Night Drive"));
    let all = clips(&c);
    assert_eq!(all.len(), 1);
    let (_, start, length, offset, asset) = &all[0];
    assert_eq!((*start, *offset), (0, 0));
    assert!(*length > 0);
    // The asset is in the project and in the bundle (hashed like any imported audio).
    let meta = &c.doc().assets[asset];
    assert_eq!((meta.sample_rate, meta.frames), (48_000, 96_000));
    assert_eq!(meta.original_name, "Night Drive audio.wav");
    assert!(assets_in_bundle(&c) > 0);
    // The film is linked to the clip, kept.
    let link = c.video_clip().unwrap().audio_link.unwrap();
    assert!(link.keep);
    assert_eq!(link.lead_us, 0);
    assert_eq!(c.video_audio_kept(), Some(true));
    // The extraction was asked for the track, as a WAV, on a list of arguments.
    let asked = calls
        .calls()
        .into_iter()
        .find(|call| call.has("-vn"))
        .expect("ffmpeg was run for the audio");
    assert_eq!(asked.after("-map").unwrap(), "0:1");
    assert!(asked.has("-nostdin") && asked.has("pcm_s24le"));
    // One undo step takes the track, the clip and the link away; a redo brings them back.
    assert_eq!(steps(&mut c), before + 1, "the import is one undo step");
    assert!(c.undo());
    assert!(c.doc().song.tracks.is_empty());
    assert!(c.video_clip().unwrap().audio_link.is_none());
    assert!(c.redo());
    assert_eq!(clips(&c).len(), 1);
    assert!(c.video_clip().unwrap().audio_link.is_some());
}

#[test]
fn the_rate_of_the_track_is_converted_and_the_first_sample_stays_on_the_first_frame() {
    let t = Tmp::new("rate");
    // 44.1 kHz sound in a 48 kHz song: one second becomes 48 000 frames.
    let mut c = with_film(&t, tool(44_100, 44_100), "scene.mp4");
    c.apply_command(Command::Video(VideoCommand::Move {
        start_us: 1_234_567,
        merge: None,
    }))
    .unwrap();
    c.import_video_audio(None).unwrap();
    c.wait_background();
    let meta = c.doc().assets.values().next().unwrap().clone();
    assert_eq!((meta.sample_rate, meta.frames), (48_000, 48_000));
    assert_eq!(c.video_clip().unwrap().start_us, 1_234_567);
    assert_eq!(asset_zero(&c), frames_of(1_234_567, 48_000));
}

#[test]
fn the_start_times_of_the_file_offset_the_sound() {
    let t = Tmp::new("lead");
    // The stand-in probe says the sound starts 7 ms after the picture.
    let mut c = with_film(
        &t,
        tool(48_000, 96_000).probe(samples::MKV_HEVC),
        "scene.mkv",
    );
    // Two tracks: ask for the second.
    c.import_video_audio(Some(2)).unwrap();
    c.wait_background();
    let link = c.video_clip().unwrap().audio_link.unwrap();
    assert_eq!(link.lead_us, 7_000);
    assert_eq!(asset_zero(&c), frames_of(7_000, 48_000));
}

#[test]
fn a_film_with_several_audio_tracks_asks_which_and_changes_nothing_until_chosen() {
    let t = Tmp::new("choose");
    let mut c = with_film(
        &t,
        tool(48_000, 48_000).probe(samples::MKV_HEVC),
        "scene.mkv",
    );
    c.import_video_audio(None).unwrap();
    c.wait_background();
    assert!(c.doc().song.tracks.is_empty());
    assert!(c.doc().assets.is_empty());
    assert_eq!(assets_in_bundle(&c), 0);
    assert!(toast(&c).contains("2 audio tracks"), "{}", toast(&c));
    let choice = c
        .take_video_audio_choice()
        .expect("the list to choose from");
    assert!(choice.film.ends_with("scene.mkv"));
    assert_eq!(
        choice.tracks,
        vec![
            (1, "1: 5.1 opus, eng, Surround".to_owned()),
            (2, "2: stereo opus, fre, Stereo (Francais)".to_owned()),
        ]
    );
    assert!(c.take_video_audio_choice().is_none(), "taken once");
    // The user picks the second: the job starts again for that track.
    c.import_video_audio(Some(2)).unwrap();
    c.wait_background();
    assert_eq!(clips(&c).len(), 1);
    assert!(toast(&c).contains("stereo opus"), "{}", toast(&c));
    // A number that is not a track is a message, not a crash.
    let mut c = with_film(
        &t,
        tool(48_000, 48_000).probe(samples::MKV_HEVC),
        "other.mkv",
    );
    c.import_video_audio(Some(9)).unwrap();
    c.wait_background();
    assert!(toast(&c).contains("no audio track 9"), "{}", toast(&c));
    assert!(c.doc().song.tracks.is_empty());
}

#[test]
fn without_an_audio_track_it_says_so_and_never_starts_ffmpeg() {
    let t = Tmp::new("silent");
    let mut c = with_film(&t, tool(48_000, 100).probe(samples::NO_AUDIO), "silent.mp4");
    assert!(!c.video_clip().unwrap().facts.has_audio);
    let err = c.import_video_audio(None).unwrap_err();
    assert_eq!(err, "silent.mp4 has no audio track to import");
    assert_eq!(toast(&c), err);
    assert!(!c.video_busy());
    assert!(c.doc().song.tracks.is_empty());
    // A film whose facts say it has sound but whose file does not (it was replaced): the worker
    // finds out.
    let mut doc = atomik_model::empty_doc();
    let mut facts = c.video_clip().unwrap().facts.clone();
    facts.has_audio = true;
    doc.video = Some(atomik_model::VideoClip::new(
        t.film("silent.mp4").to_string_lossy(),
        facts,
    ));
    let mut c = AppCore::with_doc(RecordingSink::new(), doc, None);
    c.set_video_tool(Arc::new(tool(48_000, 100).probe(samples::NO_AUDIO)));
    c.set_video_folder(Some(t.programs()));
    c.set_video_cache(t.path("cache"));
    c.import_video_audio(None).unwrap();
    c.wait_background();
    assert_eq!(toast(&c), "silent.mp4 has no audio track to import");
    assert!(c.doc().song.tracks.is_empty());
    assert!(c.doc().assets.is_empty());
}

#[test]
fn without_ffmpeg_it_says_how_to_set_it_up_and_changes_nothing() {
    let t = Tmp::new("noffmpeg");
    let mut c = with_film(&t, tool(48_000, 48_000), "scene.mp4");
    c.set_video_folder(Some(t.path("nowhere")));
    c.import_video_audio(None).unwrap();
    c.wait_background();
    assert!(toast(&c).contains("Install it"), "{}", toast(&c));
    assert!(c.doc().song.tracks.is_empty());
    assert!(c.doc().assets.is_empty());
    assert_eq!(assets_in_bundle(&c), 0);
    // No film, or a missing file, is a message at once.
    let mut c = core(&t, tool(48_000, 48_000));
    assert_eq!(
        c.import_video_audio(None).unwrap_err(),
        "The song has no video"
    );
}

#[test]
fn a_failing_ffmpeg_gives_its_message_and_leaves_the_bundle_alone() {
    let t = Tmp::new("fails");
    let mut c = with_film(
        &t,
        tool(48_000, 48_000).stream_fails("Invalid data found when processing input"),
        "scene.mp4",
    );
    c.import_video_audio(None).unwrap();
    c.wait_background();
    assert!(
        toast(&c).starts_with("Importing the audio of scene.mp4 failed:"),
        "{}",
        toast(&c)
    );
    assert!(toast(&c).contains("Invalid data"), "{}", toast(&c));
    assert!(c.doc().song.tracks.is_empty());
    assert_eq!(assets_in_bundle(&c), 0);
}

#[test]
fn stopping_the_job_leaves_no_asset_and_no_track() {
    use std::sync::{Mutex, mpsc};
    use std::time::Duration;
    let t = Tmp::new("cancel");
    let (started_tx, started_rx) = mpsc::channel::<()>();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    let (started_tx, go_rx) = (Mutex::new(started_tx), Mutex::new(go_rx));
    // The extraction is held at its first progress report until the test has pressed Cancel.
    let held = FakeTool::new()
        .audio(wav(48_000, 48_000))
        .on_frame(move |step| {
            if step == 1 {
                let _ = started_tx.lock().unwrap().send(());
                let _ = go_rx.lock().unwrap().recv_timeout(Duration::from_secs(20));
            }
        });
    let mut doc = atomik_model::empty_doc();
    doc.video = Some(atomik_model::VideoClip::new(
        t.film("scene.mp4").to_string_lossy(),
        atomik_model::VideoFacts {
            duration_us: 12_021_333,
            fps_num: 25,
            fps_den: 1,
            width: 1920,
            height: 1080,
            timecode: None,
            has_audio: true,
        },
    ));
    let mut c = AppCore::with_doc(RecordingSink::new(), doc, None);
    c.set_video_tool(Arc::new(held));
    c.set_video_folder(Some(t.programs()));
    c.set_video_cache(t.path("cache"));
    c.import_video_audio(None).unwrap();
    started_rx
        .recv_timeout(Duration::from_secs(20))
        .expect("the extraction started");
    assert!(
        c.video_progress()
            .is_some_and(|(_, line)| line.starts_with("Extracting the audio"))
    );
    c.cancel_video();
    go_tx.send(()).unwrap();
    c.wait_background();
    assert!(toast(&c).contains("cancelled"), "{}", toast(&c));
    assert!(c.doc().song.tracks.is_empty());
    assert!(c.doc().assets.is_empty());
    assert_eq!(assets_in_bundle(&c), 0, "nothing was stored");
    assert!(c.video_clip().unwrap().audio_link.is_none());
    assert!(!c.video_busy());
    assert!(!c.undo(), "no step was made");
}

#[test]
fn moving_and_trimming_the_film_move_and_trim_the_kept_sound_in_one_undo_step() {
    let t = Tmp::new("keep");
    let mut c = with_film(&t, tool(48_000, 48_000 * 10), "scene.mp4");
    c.import_video_audio(None).unwrap();
    c.wait_background();
    assert_eq!(asset_zero(&c), 0);
    // Move the film to 2 s: the sound follows to the sample, and one undo puts both back.
    let film_before = c.video_clip().cloned().unwrap();
    let clips_before = clips(&c);
    c.apply_command(Command::Video(VideoCommand::Move {
        start_us: 2_000_000,
        merge: None,
    }))
    .unwrap();
    assert_eq!(asset_zero(&c), 96_000);
    assert!(c.undo());
    assert_eq!(c.video_clip().unwrap(), &film_before);
    assert_eq!(clips(&c), clips_before);
    assert!(c.redo());
    // Trim the first 3 s off: the sound loses them too and the picture keeps its place.
    c.apply_command(Command::Video(VideoCommand::Trim {
        in_us: 3_000_000,
        out_us: 9_000_000,
        merge: None,
    }))
    .unwrap();
    let (_, start, length, offset, _) = clips(&c).remove(0);
    let song = &c.doc().song;
    assert_eq!(
        song.frame_at(start, 48_000),
        5 * 48_000,
        "starts at the cut"
    );
    assert_eq!(offset, 3 * 48_000);
    assert_eq!(song.frame_at(start + length, 48_000), 11 * 48_000);
    // Turn it off: the film moves alone. Turn it on: the sound is lined up again.
    c.toggle_keep_video_audio();
    assert_eq!(c.video_audio_kept(), Some(false));
    let free = clips(&c);
    c.apply_command(Command::Video(VideoCommand::Move {
        start_us: 8_000_000,
        merge: None,
    }))
    .unwrap();
    assert_eq!(clips(&c), free);
    c.toggle_keep_video_audio();
    assert_eq!(c.video_audio_kept(), Some(true));
    assert_ne!(clips(&c), free);
    assert_eq!(
        c.doc().song.frame_at(clips(&c)[0].1, 48_000) as i128 - i128::from(clips(&c)[0].3),
        frames_of(8_000_000 - 3_000_000, 48_000)
    );
}

#[test]
fn the_toggle_needs_imported_audio() {
    let t = Tmp::new("toggle");
    let mut c = with_film(&t, tool(48_000, 48_000), "scene.mp4");
    c.toggle_keep_video_audio();
    assert_eq!(toast(&c), "The song has no imported video audio");
    assert_eq!(c.video_audio_kept(), None);
    // A link is made by the import only; a hand-made one to nothing does not break the film.
    c.apply_command(Command::Video(VideoCommand::LinkAudio(Some(AudioLink {
        clip: atomik_model::ClipId(99),
        lead_us: 0,
        keep: true,
    }))))
    .unwrap();
    c.apply_command(Command::Video(VideoCommand::Move {
        start_us: 1_000_000,
        merge: None,
    }))
    .unwrap();
    assert_eq!(c.video_clip().unwrap().start_us, 1_000_000);
}

#[test]
fn import_video_can_import_the_sound_too() {
    let t = Tmp::new("with");
    let mut c = core(&t, tool(48_000, 48_000));
    assert!(!c.import_audio_with_video());
    c.set_import_audio_with_video(true);
    c.import_video(t.film("both.mp4")).unwrap();
    c.wait_background();
    assert!(c.video_clip().is_some());
    assert_eq!(clips(&c).len(), 1, "the sound came with the film");
    assert!(
        toast(&c).starts_with("Imported the audio of both.mp4"),
        "{}",
        toast(&c)
    );
    assert_eq!(c.video_audio_kept(), Some(true));
    // A silent film just imports the film.
    let t2 = Tmp::new("with-silent");
    let mut c = core(&t2, tool(48_000, 48_000).probe(samples::NO_AUDIO));
    c.set_import_audio_with_video(true);
    c.import_video(t2.film("mute.mp4")).unwrap();
    c.wait_background();
    assert!(c.video_clip().is_some());
    assert!(clips(&c).is_empty());
}

#[test]
fn a_project_that_changed_meanwhile_does_not_get_the_track() {
    let t = Tmp::new("generation");
    let mut c = with_film(&t, tool(48_000, 48_000), "scene.mp4");
    c.import_video_audio(None).unwrap();
    c.audio.generation += 1;
    c.wait_background();
    assert!(toast(&c).contains("The project changed"), "{}", toast(&c));
    assert!(c.doc().song.tracks.is_empty());
}
