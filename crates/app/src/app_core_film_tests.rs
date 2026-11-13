//! Film export in the app core, with the fake tool (no FFmpeg is needed): which range of the song
//! the mix covers and how the cut is laid out, copy and re-encode, the second audio track, PCM in
//! MOV, black before and after for the whole song, cancel and failure cleanup, and the states in
//! which the export is refused with a message.

use super::export::film::{describe_film, film_plan};
use super::*;
use crate::sink::RecordingSink;
use atomik_core::PPQ;
use atomik_model::{
    Clip, ClipContent, CreateOptions, CreateSource, Note, VideoClip, VideoCommand, VideoFacts,
    WorkflowCommand, new_project_doc,
};
use atomik_render::export::ExportSpan;
use atomik_ui::{
    EncoderState, FilmCover, FilmPictureChoice, FilmQualityChoice, FilmSoundChoice, FilmValues,
};
use atomik_video::fake::{Call, FakeTool};
use atomik_video::{FilmPicture, FilmQuality, Platform};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

const BAR: i64 = 4 * PPQ;
const S: i64 = 1_000_000;

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

struct Tmp(PathBuf);

impl Tmp {
    fn new() -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("atomik-app-core-film-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    /// A folder holding stand-ins for the two FFmpeg programs.
    fn programs(&self) -> PathBuf {
        let dir = self.file("ffmpeg-folder");
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["ffmpeg", "ffprobe"] {
            std::fs::write(dir.join(Platform::current().exe_name(name)), b"x").unwrap();
        }
        dir
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The starter project with a synth `lead` that plays two bars of notes.
fn song_core() -> AppCore<RecordingSink> {
    let mut c = AppCore::with_doc(RecordingSink::new(), new_project_doc(), None);
    c.apply_command(Command::Workflow(WorkflowCommand::CreateInstrument {
        index: usize::MAX,
        source: CreateSource::Type("atomik.poly-synth".into()),
        key: Some(key("lead")),
        options: CreateOptions::default(),
    }))
    .unwrap();
    c.apply_command(Command::Song(SongCommand::AddClip {
        device: key("lead"),
        lane: 0,
        clip: Clip {
            id: ClipId(0),
            start: 0,
            length: 2 * BAR,
            offset: 0,
            loop_length: None,
            muted: false,
            scale: None,
            param_lanes: Vec::new(),
            midi: Default::default(),
            groove: None,
            follow_song_key: true,
            controllers: Vec::new(),
            name: None,
            color: None,
            grid: None,
            content: ClipContent::Notes(
                vec![Note::new(0, PPQ, 64, 0.8), Note::new(PPQ, PPQ, 67, 0.8)].into(),
            ),
        },
    }))
    .unwrap();
    c.sink_mut().take();
    c
}

fn facts(has_audio: bool) -> VideoFacts {
    VideoFacts {
        duration_us: 12_021_333,
        fps_num: 25,
        fps_den: 1,
        width: 1920,
        height: 1080,
        timecode: None,
        has_audio,
    }
}

/// Puts the film `scene film.mp4` (a real file, so it is not missing) in the song, laid out by
/// `place`.
fn put_film(
    t: &Tmp,
    c: &mut AppCore<RecordingSink>,
    has_audio: bool,
    place: impl FnOnce(&mut VideoClip),
) {
    let file = t.file("scene film.mp4");
    std::fs::write(&file, b"film").unwrap();
    let mut clip = VideoClip::new(file.to_string_lossy().into_owned(), facts(has_audio));
    place(&mut clip);
    c.apply_command(Command::Video(VideoCommand::Set(clip)))
        .unwrap();
}

/// A core whose FFmpeg is `tool` and whose song has a whole film at its start.
fn film_core(t: &Tmp, tool: &Arc<FakeTool>) -> AppCore<RecordingSink> {
    let mut c = song_core();
    c.set_video_tool(Arc::clone(tool) as Arc<dyn atomik_video::VideoTool>);
    c.set_video_folder(Some(t.programs()));
    put_film(t, &mut c, true, |_| {});
    c
}

fn maker() -> Arc<FakeTool> {
    Arc::new(FakeTool::new().audio(b"NEWFILM".to_vec()))
}

fn values() -> FilmValues {
    FilmValues::default()
}

/// The calls that make the film (not the probe or the version check).
fn makes(tool: &FakeTool) -> Vec<Call> {
    tool.calls()
        .into_iter()
        .filter(|c| c.has("-filter_complex"))
        .collect()
}

fn texts(call: &Call) -> Vec<String> {
    call.args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

fn has(args: &[String], pair: [&str; 2]) -> bool {
    args.windows(2).any(|w| w == pair)
}

fn graph(call: &Call) -> String {
    call.after("-filter_complex")
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

/// The inputs (after `-i`) of a call, without the `file:` prefix.
fn inputs(call: &Call) -> Vec<PathBuf> {
    let args = texts(call);
    args.windows(2)
        .filter(|w| w[0] == "-i")
        .map(|w| PathBuf::from(w[1].strip_prefix("file:").unwrap()))
        .collect()
}

// ------------------------------------------------------------------ the plan

#[test]
fn the_films_span_is_the_part_of_the_song_under_the_trimmed_film() {
    let t = Tmp::new();
    let mut c = song_core();
    // The film's seconds 1..5 play from song second 2: under the song's seconds 2..6.
    put_film(&t, &mut c, true, |v| {
        v.start_us = 2 * S;
        v.in_us = S as u64;
        v.out_us = 5 * S as u64;
    });
    let plan = film_plan(c.doc(), false).unwrap();
    // 120 bpm: a second is two beats.
    assert_eq!(
        plan.request.span,
        ExportSpan::Range(4 * PPQ, 12 * PPQ),
        "the range of the song under the film"
    );
    assert_eq!(plan.request.tail_seconds, 0.0, "a film has no tail");
    assert_eq!(plan.cut.in_us, S as u64);
    assert_eq!(plan.cut.length_us, 4 * S as u64);
    assert_eq!(plan.cut.total_us, 4 * S as u64);
    assert_eq!(plan.cut.mix_delay_us, 0, "the mix starts with the picture");
    assert!(!plan.cut.needs_black());
}

#[test]
fn a_film_that_begins_before_the_song_gets_silence_before_the_mix() {
    let t = Tmp::new();
    let mut c = song_core();
    put_film(&t, &mut c, true, |v| {
        v.start_us = -S;
        v.out_us = 4 * S as u64;
    });
    let plan = film_plan(c.doc(), false).unwrap();
    // The mix is the song from its start to the end of the film (3 s), after 1 s of silence.
    assert_eq!(plan.request.span, ExportSpan::Range(0, 6 * PPQ));
    assert_eq!(plan.cut.mix_delay_us, S as u64);
    assert_eq!(plan.cut.total_us, 4 * S as u64);
}

#[test]
fn a_film_that_ends_before_the_song_starts_has_nothing_under_it() {
    let t = Tmp::new();
    let mut c = song_core();
    put_film(&t, &mut c, true, |v| {
        v.start_us = -5 * S;
        v.out_us = 2 * S as u64;
    });
    let err = film_plan(c.doc(), false).unwrap_err();
    assert!(err.contains("ends before the song starts"), "{err}");
    // The whole song still has the film in it, with black after the song begins.
    assert!(film_plan(c.doc(), true).is_ok());
}

#[test]
fn the_whole_song_runs_from_the_start_to_the_end_with_black_around_the_film() {
    let t = Tmp::new();
    let mut c = song_core();
    put_film(&t, &mut c, true, |v| {
        v.start_us = 2 * S;
        v.out_us = S as u64;
    });
    let plan = film_plan(c.doc(), true).unwrap();
    assert_eq!(plan.request.span, ExportSpan::Song);
    let end = c.doc().song.end_tick();
    let song_us = (u128::from(c.doc().song.frame_at(end, c.doc().sample_rate)) * 1_000_000
        / u128::from(c.doc().sample_rate)) as u64;
    assert_eq!(plan.cut.black_before_us, 2 * S as u64);
    assert_eq!(plan.cut.mix_delay_us, 0);
    assert_eq!(plan.cut.total_us, song_us.max(3 * S as u64));
    assert_eq!(
        plan.cut.black_after_us,
        song_us.saturating_sub(3 * S as u64)
    );
    assert!(plan.cut.needs_black());
}

#[test]
fn a_song_without_a_film_has_no_plan() {
    let c = song_core();
    assert_eq!(
        film_plan(c.doc(), false).unwrap_err(),
        "The song has no video"
    );
}

// ------------------------------------------------------------------ the export

#[test]
fn the_default_export_copies_the_picture_and_writes_an_mp4_with_the_mix_as_aac() {
    let t = Tmp::new();
    let tool = maker();
    let mut c = film_core(&t, &tool);
    c.apply_command(Command::SetSongInfo(atomik_model::SongInfo {
        title: "Night \"Drive\"".to_owned(),
        author: "Zoe".to_owned(),
        ..atomik_model::SongInfo::default()
    }))
    .unwrap();
    let dirty = c.is_dirty();
    // The dialog proposed a .mp4 name: the file keeps it.
    c.start_film_export(t.file("out film.mp4"), values())
        .unwrap();
    assert!(c.audio_export_busy());
    c.wait_background();
    let out = t.file("out film.mp4");
    assert_eq!(std::fs::read(&out).unwrap(), b"NEWFILM");
    assert!(
        !atomik_video::encode::partial_path(&out).exists(),
        "no partial file"
    );
    let message = c.last_rejection().unwrap();
    assert!(
        message.contains("Exported out film.mp4")
            && message.contains("picture copied")
            && message.contains("AAC 192 kbps"),
        "{message}"
    );
    assert!(
        message.contains("LUFS"),
        "the loudness is reported: {message}"
    );
    let calls = makes(&tool);
    assert_eq!(calls.len(), 1, "one call makes the film");
    let a = texts(&calls[0]);
    assert!(has(&a, ["-c:v", "copy"]), "{a:?}");
    assert!(has(&a, ["-c:a", "aac"]));
    assert!(has(&a, ["-b:a", "192k"]));
    assert!(has(&a, ["-f", "mp4"]));
    assert!(a.contains(&"title=Night \"Drive\"".to_owned()));
    assert!(a.contains(&"artist=Zoe".to_owned()));
    // The film is the first input, the temporary mix the second; the mix is gone.
    let files = inputs(&calls[0]);
    assert_eq!(files[0], t.file("scene film.mp4"));
    assert!(!files[1].exists(), "the temporary mix was removed");
    assert!(!files[1].parent().unwrap().exists());
    assert_eq!(c.is_dirty(), dirty, "the project is untouched");
    assert!(c.sink_mut().take().is_empty(), "the engine was not touched");
    // The film is probed first: the picture's codec decides copy or re-encode.
    assert!(tool.calls().iter().any(|c| c.has("-show_streams")));
}

#[test]
fn a_trimmed_film_is_reencoded_exactly_with_the_chosen_quality() {
    let t = Tmp::new();
    let tool = maker();
    let mut c = film_core(&t, &tool);
    put_film(&t, &mut c, true, |v| {
        v.start_us = 2 * S;
        v.in_us = 3 * S as u64;
        v.out_us = 7 * S as u64;
    });
    let chosen = FilmValues {
        quality: FilmQualityChoice::High,
        ..values()
    };
    c.start_film_export(t.file("trim.mp4"), chosen).unwrap();
    c.wait_background();
    let a = texts(&makes(&tool)[0]);
    assert!(
        has(&a, ["-c:v", "libx264"]),
        "AUTO re-encodes a trim: {a:?}"
    );
    assert!(has(&a, ["-crf", "18"]), "{a:?}");
    assert_eq!(a.windows(2).find(|w| w[0] == "-ss").unwrap()[1], "3.000000");
    let message = c.last_rejection().unwrap();
    assert!(
        message.contains("re-encoded to H.264, high quality"),
        "{message}"
    );
}

#[test]
fn a_forced_copy_of_a_trimmed_start_says_the_cut_snapped() {
    let t = Tmp::new();
    let tool = maker();
    let mut c = film_core(&t, &tool);
    put_film(&t, &mut c, true, |v| {
        v.in_us = 3 * S as u64;
        v.out_us = 7 * S as u64;
    });
    let chosen = FilmValues {
        picture: FilmPictureChoice::Copy,
        ..values()
    };
    c.start_film_export(t.file("copy.mp4"), chosen).unwrap();
    c.wait_background();
    let a = texts(&makes(&tool)[0]);
    assert!(has(&a, ["-c:v", "copy"]), "{a:?}");
    let message = c.last_rejection().unwrap();
    assert!(message.contains("snapped to a keyframe"), "{message}");
}

#[test]
fn pcm_goes_into_a_mov_file_whatever_extension_the_dialog_picked() {
    let t = Tmp::new();
    let tool = maker();
    let mut c = film_core(&t, &tool);
    let chosen = FilmValues {
        sound: FilmSoundChoice::Pcm,
        ..values()
    };
    c.start_film_export(t.file("out.mp4"), chosen).unwrap();
    c.wait_background();
    assert!(t.file("out.mov").is_file());
    assert!(!t.file("out.mp4").exists());
    let a = texts(&makes(&tool)[0]);
    assert!(has(&a, ["-c:a", "pcm_s24le"]), "{a:?}");
    assert!(has(&a, ["-f", "mov"]));
    assert!(c.last_rejection().unwrap().contains("24-bit PCM"));
    assert_eq!(
        c.film_file_for(Path::new("a/b.wav"), &chosen),
        PathBuf::from("a/b.mov")
    );
    assert_eq!(
        c.film_file_for(Path::new("a/b.wav"), &values()),
        PathBuf::from("a/b.mp4")
    );
}

#[test]
fn the_films_own_sound_can_stay_as_a_second_track() {
    let t = Tmp::new();
    let tool = maker();
    let mut c = film_core(&t, &tool);
    let chosen = FilmValues {
        own_audio: true,
        ..values()
    };
    c.start_film_export(t.file("two.mp4"), chosen).unwrap();
    c.wait_background();
    let a = texts(&makes(&tool)[0]);
    let maps: Vec<&String> = a
        .windows(2)
        .filter(|w| w[0] == "-map")
        .map(|w| &w[1])
        .collect();
    assert_eq!(
        maps,
        ["0:0", "[mix]", "0:1"],
        "picture, mix, the film's sound"
    );
    assert!(has(&a, ["-metadata:s:a:1", "title=Film audio"]));
    assert!(
        c.last_rejection()
            .unwrap()
            .contains("film audio as track 2")
    );
}

#[test]
fn a_film_without_sound_cannot_keep_a_second_track() {
    let t = Tmp::new();
    let tool = maker();
    let mut c = film_core(&t, &tool);
    put_film(&t, &mut c, false, |_| {});
    assert!(!c.film_has_audio());
    let err = c
        .start_film_export(
            t.file("x.mp4"),
            FilmValues {
                own_audio: true,
                ..values()
            },
        )
        .unwrap_err();
    assert!(err.contains("no sound of its own"), "{err}");
    assert!(!c.audio_export_busy());
    assert!(makes(&tool).is_empty());
}

#[test]
fn the_whole_song_adds_black_and_reencodes_and_a_forced_copy_is_refused() {
    let t = Tmp::new();
    let tool = maker();
    let mut c = film_core(&t, &tool);
    put_film(&t, &mut c, true, |v| {
        v.start_us = 2 * S;
        v.out_us = 3 * S as u64;
    });
    let whole = FilmValues {
        cover: FilmCover::Song,
        ..values()
    };
    c.start_film_export(t.file("whole.mp4"), whole).unwrap();
    c.wait_background();
    let calls = makes(&tool);
    let a = texts(&calls[0]);
    assert!(
        has(&a, ["-c:v", "libx264"]),
        "black needs a re-encode: {a:?}"
    );
    let g = graph(&calls[0]);
    assert!(
        g.contains("color=c=black:s=1920x1080:r=25/1:d=2.000000"),
        "{g}"
    );
    assert!(g.contains("concat=n="), "{g}");
    assert!(t.file("whole.mp4").is_file());
    // A copy cannot carry black: the worker refuses it before it renders anything.
    let copy = FilmValues {
        picture: FilmPictureChoice::Copy,
        ..whole
    };
    c.start_film_export(t.file("copy.mp4"), copy).unwrap();
    c.wait_background();
    let message = c.last_rejection().unwrap();
    assert!(
        message.contains("Export of copy.mp4 failed") && message.contains("RE-ENCODE"),
        "{message}"
    );
    assert!(!t.file("copy.mp4").exists());
    assert_eq!(makes(&tool).len(), 1, "no second film was made");
}

#[test]
fn a_cancel_while_ffmpeg_runs_removes_the_partial_film_and_the_temporary_mix() {
    use std::sync::mpsc::channel;
    let t = Tmp::new();
    let (at_step, reached) = channel::<()>();
    let (release, wait) = channel::<()>();
    let wait = std::sync::Mutex::new(wait);
    let at_step = std::sync::Mutex::new(at_step);
    let tool = Arc::new(FakeTool::new().audio(b"HALF".to_vec()).on_frame(move |n| {
        if n == 1 {
            let _ = at_step.lock().unwrap().send(());
            let _ = wait
                .lock()
                .unwrap()
                .recv_timeout(std::time::Duration::from_secs(30));
        }
    }));
    let mut c = film_core(&t, &tool);
    std::fs::write(t.file("out.mp4"), b"old").unwrap();
    c.start_film_export(t.file("out.mp4"), values()).unwrap();
    reached
        .recv_timeout(std::time::Duration::from_secs(60))
        .unwrap();
    c.cancel_audio_export();
    release.send(()).unwrap();
    c.wait_background();
    assert!(!c.audio_export_busy());
    let out = t.file("out.mp4");
    assert_eq!(
        std::fs::read(&out).unwrap(),
        b"old",
        "the old film is untouched"
    );
    assert!(
        !atomik_video::encode::partial_path(&out).exists(),
        "no partial file"
    );
    let call = &makes(&tool)[0];
    assert!(!inputs(call)[1].exists(), "the temporary mix was removed");
    assert!(c.last_rejection().unwrap().contains("cancelled"));
}

#[test]
fn a_cancel_before_the_job_starts_leaves_no_file() {
    let t = Tmp::new();
    let tool = maker();
    let mut c = film_core(&t, &tool);
    let release = crate::export_worker::hold::next_export();
    c.start_film_export(t.file("out.mp4"), values()).unwrap();
    c.cancel_audio_export();
    drop(release);
    c.wait_background();
    assert!(!t.file("out.mp4").exists());
    assert!(
        !atomik_video::encode::partial_path(&t.file("out.mp4")).exists(),
        "no partial file"
    );
    assert!(c.last_rejection().unwrap().contains("cancelled"));
    assert!(makes(&tool).is_empty(), "FFmpeg never made a film");
}

#[test]
fn a_failing_ffmpeg_says_why_and_leaves_nothing_but_an_old_film() {
    let t = Tmp::new();
    let tool = Arc::new(
        FakeTool::new()
            .audio(b"HALF".to_vec())
            .stream_fails("Unknown encoder 'libx264'"),
    );
    let mut c = film_core(&t, &tool);
    std::fs::write(t.file("out.mp4"), b"old").unwrap();
    c.start_film_export(t.file("out.mp4"), values()).unwrap();
    c.wait_background();
    let message = c.last_rejection().unwrap();
    assert!(
        message.contains("Export of out.mp4 failed") && message.contains("libx264"),
        "{message}"
    );
    assert_eq!(std::fs::read(t.file("out.mp4")).unwrap(), b"old");
    assert!(!atomik_video::encode::partial_path(&t.file("out.mp4")).exists());
}

#[test]
fn an_unreadable_film_fails_before_anything_is_rendered() {
    let t = Tmp::new();
    let tool = Arc::new(FakeTool::new().audio(b"X".to_vec()).probe_fails());
    let mut c = film_core(&t, &tool);
    c.start_film_export(t.file("out.mp4"), values()).unwrap();
    c.wait_background();
    let message = c.last_rejection().unwrap();
    assert!(message.contains("the film cannot be read"), "{message}");
    assert!(!t.file("out.mp4").exists());
    assert!(makes(&tool).is_empty());
}

#[test]
fn the_progress_line_says_what_runs() {
    let t = Tmp::new();
    let tool = maker();
    let mut c = film_core(&t, &tool);
    let release = crate::export_worker::hold::next_export();
    c.start_film_export(t.file("out.mp4"), values()).unwrap();
    let (fraction, text) = c.audio_export_progress().unwrap();
    assert_eq!(fraction, 0.0);
    assert_eq!(text, "Making the film");
    drop(release);
    c.wait_background();
    assert!(c.audio_export_progress().is_none());
}

// ------------------------------------------------------------------ the states

#[test]
fn a_song_without_a_film_refuses_with_a_message() {
    let t = Tmp::new();
    let mut c = song_core();
    c.set_video_tool(maker());
    c.set_video_folder(Some(t.programs()));
    assert!(!c.can_export_film());
    let why = c.film_export_refusal().unwrap();
    assert!(
        why.contains("no video") && why.contains("Import Video"),
        "{why}"
    );
    let err = c.start_film_export(t.file("a.mp4"), values()).unwrap_err();
    assert_eq!(err, why);
    assert_eq!(c.last_rejection().unwrap(), why);
}

#[test]
fn a_missing_film_file_refuses_and_says_how_to_relink() {
    let t = Tmp::new();
    let tool = maker();
    let mut c = film_core(&t, &tool);
    assert!(c.can_export_film());
    std::fs::remove_file(t.file("scene film.mp4")).unwrap();
    // The answer is kept per path: Import, Relink and opening look again.
    c.forget_video_file();
    assert!(!c.can_export_film());
    let why = c.film_export_refusal().unwrap();
    assert!(
        why.contains("missing") && why.contains("Relink Video"),
        "{why}"
    );
    assert!(c.start_film_export(t.file("a.mp4"), values()).is_err());
    assert!(makes(&tool).is_empty());
}

#[test]
fn without_ffmpeg_the_export_is_refused_and_the_panel_is_told() {
    let t = Tmp::new();
    // No real tools in this core (a headless run).
    let mut c = song_core();
    put_film(&t, &mut c, true, |_| {});
    let why = c.film_export_refusal().unwrap();
    assert!(why.contains("FFmpeg"), "{why}");
    assert!(!c.can_export_film());
    // Tools, but no FFmpeg in the folder named in Settings.
    let empty = t.file("empty");
    std::fs::create_dir_all(&empty).unwrap();
    c.set_video_tool(Arc::new(FakeTool::new()));
    c.set_video_folder(Some(empty));
    assert_eq!(c.film_encoder(), EncoderState::Checking);
    assert!(c.can_export_film(), "still being looked for: not refused");
    c.check_ffmpeg().unwrap();
    c.wait_background();
    let EncoderState::Missing(why) = c.film_encoder() else {
        panic!("{:?}", c.film_encoder());
    };
    assert!(
        why.contains("Settings > Video") && why.contains("PATH") && why.contains("film"),
        "{why}"
    );
    assert!(why.len() <= 90, "short enough for a tooltip: {why}");
    assert!(!c.can_export_film());
    let err = c.start_film_export(t.file("a.mp4"), values()).unwrap_err();
    assert_eq!(err, why);
    assert!(!t.file("a.mp4").exists());
}

#[test]
fn ffmpeg_found_makes_the_encoder_ready() {
    let t = Tmp::new();
    let tool = maker();
    let mut c = film_core(&t, &tool);
    assert_eq!(c.film_encoder(), EncoderState::Checking);
    c.check_ffmpeg().unwrap();
    c.wait_background();
    assert_eq!(c.film_encoder(), EncoderState::Ready);
    assert!(c.can_export_film());
}

#[test]
fn only_one_export_runs_at_a_time() {
    let t = Tmp::new();
    let tool = maker();
    let mut c = film_core(&t, &tool);
    let release = crate::export_worker::hold::next_export();
    c.start_film_export(t.file("a.mp4"), values()).unwrap();
    assert!(!c.can_export_film());
    let why = c.film_export_refusal().unwrap();
    assert!(why.contains("export is running"), "{why}");
    assert!(c.start_film_export(t.file("b.mp4"), values()).is_err());
    assert!(!c.can_export_audio(atomik_ui::ExportScope::Song));
    drop(release);
    c.wait_background();
    assert!(t.file("a.mp4").is_file());
    assert!(!t.file("b.mp4").exists());
    assert!(c.can_export_film());
}

#[test]
fn taking_the_film_out_of_the_song_refuses_the_export_again() {
    let t = Tmp::new();
    let tool = maker();
    let mut c = film_core(&t, &tool);
    c.apply_command(Command::Video(VideoCommand::Remove))
        .unwrap();
    assert!(c.film_export_refusal().unwrap().contains("no video"));
}

#[test]
fn the_toast_of_a_copy_names_nothing_it_did_not_do() {
    let report = atomik_render::export::ExportReport {
        path: PathBuf::from("d/film.mp4"),
        frames: 48_000,
        sample_rate: 48_000,
        span_frames: 48_000,
        tail_frames: 0,
        warnings: Vec::new(),
        loudness: Default::default(),
        normalise: None,
    };
    let copied = describe_film(&report, FilmPicture::Copy, None);
    assert!(
        copied.starts_with("Exported film.mp4: 0:01.0, picture copied, AAC"),
        "{copied}"
    );
    assert!(!copied.contains("snapped"));
    let encoded = describe_film(&report, FilmPicture::Encode(FilmQuality::Small), None);
    assert!(encoded.contains("small size"), "{encoded}");
}

/// The real FFmpeg, when it is installed: a film of a generated film and the starter song, by
/// copy and, for the whole song with black around the film, by re-encode. Run with
/// `cargo test -p atomik-app --lib real_ffmpeg -- --ignored`.
#[test]
#[ignore = "needs a real FFmpeg"]
fn real_ffmpeg_makes_a_film_of_a_generated_film_and_the_song() {
    use atomik_video::runner::capture;
    use std::ffi::OsString;
    let t = Tmp::new();
    let mut c = song_core();
    c.set_video_tool(Arc::new(atomik_video::SystemTool));
    c.check_ffmpeg().unwrap();
    c.wait_background();
    let atomik_video::FfmpegStatus::Found(ffmpeg) = c.video_status().clone() else {
        println!("no FFmpeg here: {:?}", c.film_encoder());
        return;
    };
    let never = AtomicBool::new(false);
    let source = t.file("generated.mp4");
    let make: Vec<OsString> = [
        "-nostdin",
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "testsrc=size=320x180:rate=25:duration=3",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=330:duration=3:sample_rate=48000",
        "-pix_fmt",
        "yuv420p",
        "-g",
        "25",
        "-shortest",
        "-y",
    ]
    .map(OsString::from)
    .into_iter()
    .chain([source.clone().into_os_string()])
    .collect();
    capture(
        &atomik_video::SystemTool,
        &ffmpeg.ffmpeg,
        &make,
        1 << 20,
        &never,
    )
    .unwrap();
    let mut clip = VideoClip::new(
        source.to_string_lossy().into_owned(),
        VideoFacts {
            duration_us: 3_000_000,
            fps_num: 25,
            fps_den: 1,
            width: 320,
            height: 180,
            timecode: None,
            has_audio: true,
        },
    );
    clip.start_us = S;
    c.apply_command(Command::Video(VideoCommand::Set(clip)))
        .unwrap();
    let describe = |file: &Path| -> String {
        let args: Vec<OsString> = [
            "-v",
            "error",
            "-show_entries",
            "stream=codec_name,codec_type:format=duration",
            "-of",
            "default=nw=1",
        ]
        .map(OsString::from)
        .into_iter()
        .chain([file.as_os_str().to_owned()])
        .collect();
        capture(
            &atomik_video::SystemTool,
            &ffmpeg.ffprobe,
            &args,
            1 << 16,
            &never,
        )
        .unwrap()
    };
    let duration = |text: &str| -> f64 {
        text.lines()
            .find_map(|l| l.strip_prefix("duration="))
            .and_then(|d| d.parse().ok())
            .unwrap()
    };
    // The film's span, picture copied: 3 s, h264 and aac.
    c.start_film_export(t.file("span.mp4"), values()).unwrap();
    c.wait_background();
    let message = c.last_rejection().unwrap().to_owned();
    println!("{message}");
    assert!(message.contains("Exported span.mp4"), "{message}");
    let text = describe(&t.file("span.mp4"));
    println!("{text}");
    assert!(
        text.contains("codec_name=h264") && text.contains("codec_name=aac"),
        "{text}"
    );
    assert!((duration(&text) - 3.0).abs() < 0.1, "{text}");
    // The whole song with black before the film (it starts at 1 s), the mix as PCM in MOV.
    let whole = FilmValues {
        cover: FilmCover::Song,
        sound: FilmSoundChoice::Pcm,
        own_audio: true,
        ..values()
    };
    c.start_film_export(t.file("whole.mp4"), whole).unwrap();
    c.wait_background();
    let message = c.last_rejection().unwrap().to_owned();
    println!("{message}");
    assert!(message.contains("Exported whole.mov"), "{message}");
    let text = describe(&t.file("whole.mov"));
    println!("{text}");
    assert!(text.contains("codec_name=pcm_s24le"), "{text}");
    assert_eq!(text.matches("codec_type=audio").count(), 2, "{text}");
    assert!(
        duration(&text) >= 4.0 - 0.1,
        "black before and the film: {text}"
    );
}

#[test]
fn a_film_export_can_bring_the_mix_to_a_loudness_target() {
    let t = Tmp::new();
    let tool = maker();
    let mut c = film_core(&t, &tool);
    c.start_film_export_with(
        t.file("out film.mp4"),
        FilmValues {
            aac: 256,
            ..values()
        },
        atomik_ui::ExportLoudness::Target(-14),
    )
    .unwrap();
    c.wait_background();
    let message = c.last_rejection().unwrap();
    assert!(
        message.contains("Exported out film.mp4") && message.contains("AAC 256 kbps"),
        "{message}"
    );
    assert!(
        message.contains("; was ")
            && (message.contains("turned up") || message.contains("turned down")),
        "the report says what was done to the level: {message}"
    );
    let calls = makes(&tool);
    assert!(has(&texts(&calls[0]), ["-b:a", "256k"]));
}
