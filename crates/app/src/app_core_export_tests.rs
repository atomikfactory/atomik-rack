//! Exporting the song and the loop as audio files in the app core: the render runs on a worker and
//! writes the file the options describe, the live engine is not touched, a cancel leaves no
//! file, one export runs at a time, and the panel values map to the render request and format.

use super::export::{export_format_at, file_extension};
use super::*;
use crate::sink::{RecordingSink, SinkCall};
use atomik_core::PPQ;
use atomik_model::{
    Clip, ClipContent, CreateOptions, CreateSource, Note, WorkflowCommand, new_project_doc,
};
use atomik_render::export::{BitDepth, ExportContainer, ExportSpan};
use atomik_ui::{EXPORT_RATES, ExportDepth, ExportDither, ExportFile, ExportScope, ExportValues};
use std::sync::atomic::{AtomicU64, Ordering};

const BAR: i64 = 4 * PPQ;

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

struct Tmp(PathBuf);

impl Tmp {
    fn new() -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("atomik-app-core-export-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The starter project with a synth `lead` that plays two bars of notes.
fn core() -> AppCore<RecordingSink> {
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

fn values(depth: ExportDepth) -> ExportValues {
    ExportValues {
        depth,
        ..ExportValues::default()
    }
}

#[test]
fn a_song_export_renders_on_a_worker_and_writes_the_file_without_touching_the_engine() {
    let t = Tmp::new();
    let mut c = core();
    let dirty = c.is_dirty();
    let end = c.doc().song.end_tick();
    let end_frames = c.doc().song.frame_at(end, c.doc().sample_rate);
    let path = t.file("song.wav");
    assert_eq!(c.exports_finished(), 0);
    c.start_audio_export(ExportScope::Song, path.clone(), values(ExportDepth::Pcm16))
        .unwrap();
    assert!(c.audio_export_busy());
    assert!(
        !c.can_export_audio(ExportScope::Song),
        "one export at a time"
    );
    let (fraction, text) = c.audio_export_progress().unwrap();
    assert!((0.0..=1.0).contains(&fraction));
    assert!(text.contains("song"), "{text}");
    c.wait_background();
    assert!(!c.audio_export_busy());
    assert!(c.audio_export_progress().is_none());
    assert_eq!(c.exports_finished(), 1, "a finished export is counted");
    let message = c.last_rejection().unwrap();
    assert!(
        message.contains("Exported song.wav") && message.contains("16-bit"),
        "{message}"
    );
    // A 16-bit WAV from the start to the End, plus the tail of the release.
    let spec = hound::WavReader::open(&path).unwrap().spec();
    assert_eq!((spec.bits_per_sample, spec.channels), (16, 2));
    let decoded = atomik_audio_file::decode_file(&path).unwrap();
    assert!(decoded.frames() as u64 >= end_frames);
    assert!(decoded.channels[0].iter().any(|x| x.abs() > 1e-3));
    // The project and the live engine are as they were.
    assert_eq!(c.is_dirty(), dirty);
    assert!(c.sink_mut().take().is_empty(), "the engine was not touched");
}

#[test]
fn a_loop_export_is_exactly_the_locator_range_at_the_chosen_rate() {
    let t = Tmp::new();
    let mut c = core();
    c.apply_command(Command::Song(SongCommand::SetLoop {
        range: (BAR, BAR + BAR / 2),
        enabled: true,
    }))
    .unwrap();
    c.sink_mut().take();
    let path = t.file("loop.wav");
    let chosen = ExportValues {
        depth: ExportDepth::Pcm24,
        rate: Some(EXPORT_RATES[0]),
        ..ExportValues::default()
    };
    c.start_audio_export(ExportScope::Loop, path.clone(), chosen)
        .unwrap();
    c.wait_background();
    let decoded = atomik_audio_file::decode_file(&path).unwrap();
    // Half a bar at 120 BPM and 48 kHz is 48 000 frames; at 44.1 kHz, 44 100.
    assert_eq!(decoded.sample_rate, 44_100);
    assert_eq!(decoded.frames(), 44_100);
    assert!(c.last_rejection().unwrap().contains("24-bit"));
    assert!(c.last_rejection().unwrap().contains("44.1 kHz"));
    // The transport and the loop were not touched.
    assert!(
        !c.sink_mut()
            .take()
            .iter()
            .any(|call| matches!(call, SinkCall::Transport(_)))
    );
}

#[test]
fn a_cancelled_export_leaves_no_file_and_a_second_one_can_start() {
    let t = Tmp::new();
    let mut c = core();
    let path = t.file("song.wav");
    c.start_audio_export(ExportScope::Song, path.clone(), values(ExportDepth::Pcm24))
        .unwrap();
    // A second export while one runs is refused, and says why.
    let busy = c.start_audio_export(
        ExportScope::Loop,
        t.file("other.wav"),
        values(ExportDepth::Pcm24),
    );
    assert!(busy.unwrap_err().contains("running"));
    c.cancel_audio_export();
    c.wait_background();
    assert!(!c.audio_export_busy());
    assert!(!path.exists(), "no file");
    assert!(
        !atomik_render::export::partial_path(&path).exists(),
        "no partial file"
    );
    assert!(c.last_rejection().unwrap().contains("cancelled"));
    assert_eq!(c.exports_finished(), 0, "a cancelled export is not counted");
    c.cancel_audio_export();
    assert!(c.last_rejection().unwrap().contains("No export is running"));
    // The next one goes through.
    c.start_audio_export(
        ExportScope::Song,
        path.clone(),
        values(ExportDepth::Float32),
    )
    .unwrap();
    c.wait_background();
    assert!(path.exists());
    assert!(c.last_rejection().unwrap().contains("32-bit float"));
    assert_eq!(c.exports_finished(), 1);
}

#[test]
fn the_panel_values_map_to_the_render_request_and_format() {
    let mut v = ExportValues {
        depth: ExportDepth::Pcm16,
        dither: ExportDither::Shaped,
        rate: Some(96_000),
        tail: true,
        ..ExportValues::default()
    };
    let f = export_format(&v);
    assert_eq!(f.bit_depth, BitDepth::Pcm16);
    assert_eq!(f.dither, atomik_dsp::DitherMode::Shaped);
    assert_eq!(f.sample_rate, Some(96_000));
    // The dither is for 16-bit files only.
    for depth in [ExportDepth::Pcm24, ExportDepth::Float32] {
        v.depth = depth;
        assert_eq!(export_format(&v).dither, atomik_dsp::DitherMode::None);
    }
    v.dither = ExportDither::Off;
    v.depth = ExportDepth::Pcm16;
    assert_eq!(export_format(&v).dither, atomik_dsp::DitherMode::None);
    // A song keeps its tail when asked to; a loop never has one.
    assert!(export_request(ExportScope::Song, &v, None).tail_seconds > 0.0);
    v.tail = false;
    assert!(
        export_request(ExportScope::Song, &v, None)
            .tail_seconds
            .abs()
            < f64::EPSILON
    );
    v.tail = true;
    assert!(
        export_request(ExportScope::Loop, &v, None)
            .tail_seconds
            .abs()
            < f64::EPSILON
    );
}

#[test]
fn a_stems_export_writes_a_file_per_strip_in_a_folder_without_touching_the_engine() {
    let t = Tmp::new();
    let mut c = core();
    let dirty = c.is_dirty();
    let folder = t.file("Bounced song");
    let aiff = ExportValues {
        file: ExportFile::Aiff,
        ..values(ExportDepth::Pcm24)
    };
    c.start_audio_export(ExportScope::Stems, folder.clone(), aiff)
        .unwrap();
    assert!(c.audio_export_busy());
    let (_, text) = c.audio_export_progress().unwrap();
    assert!(text.contains("stems"), "{text}");
    c.wait_background();
    assert!(!c.audio_export_busy());
    let message = c.last_rejection().unwrap();
    assert!(
        message.contains("Exported 2 stems to Bounced song") && message.contains("24-bit"),
        "{message}"
    );
    // One channel and the master, numbered, in the chosen container.
    let mut names: Vec<String> = std::fs::read_dir(&folder)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names.len(), 2, "{names:?}");
    assert!(
        names[0].starts_with("01 ") && names[0].ends_with(".aif"),
        "{names:?}"
    );
    assert!(names[1].starts_with("02 Master"), "{names:?}");
    let master = atomik_audio_file::decode_file(&folder.join(&names[1])).unwrap();
    assert!(master.channels[0].iter().any(|x| x.abs() > 1e-3));
    assert_eq!(c.is_dirty(), dirty);
    assert!(c.sink_mut().take().is_empty(), "the engine was not touched");
}

#[test]
fn stems_need_a_strip_and_a_song_and_the_file_extension_follows_the_container() {
    let t = Tmp::new();
    let c = AppCore::with_doc(RecordingSink::new(), new_project_doc(), None);
    let why = c.export_audio_refusal(ExportScope::Stems).unwrap();
    assert!(why.contains("no mixer strips"), "{why}");
    assert!(!c.can_export_audio(ExportScope::Stems));
    // A song export takes the extension of the container, whatever the dialog added.
    let mut c = core();
    let aiff = ExportValues {
        file: ExportFile::Aiff,
        ..values(ExportDepth::Pcm16)
    };
    c.start_audio_export(ExportScope::Song, t.file("mix.wav"), aiff)
        .unwrap();
    c.wait_background();
    assert!(t.file("mix.aif").is_file() && !t.file("mix.wav").exists());
}

#[test]
fn the_dry_bounce_of_a_clip_is_a_new_track_without_the_strip() {
    let mut c = core();
    let clip = c.doc().song.track(&key("lead")).unwrap().lanes[0].clips[0].id;
    c.arrangement.set_selected_clips(&[clip]);
    let before = c.doc().song.tracks.len();
    c.bounce_clips_dry();
    c.wait_background();
    assert_eq!(c.doc().song.tracks.len(), before + 1);
    let new = c.doc().song.tracks.last().unwrap();
    let name = &c.doc().device(&new.device).unwrap().name;
    assert!(name.ends_with("(dry bounce)"), "{name}");
    assert!(c.last_rejection().unwrap().contains("Bounced"));
}

#[test]
fn a_flac_export_is_a_flac_file_that_says_how_loud_it_is() {
    let t = Tmp::new();
    let mut c = core();
    let flac = ExportValues {
        file: ExportFile::Flac,
        ..values(ExportDepth::Pcm16)
    };
    // The extension follows the container, as for the others.
    c.start_audio_export(ExportScope::Song, t.file("mix.wav"), flac)
        .unwrap();
    c.wait_background();
    let path = t.file("mix.flac");
    assert!(path.is_file() && !t.file("mix.wav").exists());
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(&bytes[..4], b"fLaC");
    let decoded = atomik_audio_file::decode_file(&path).unwrap();
    assert!(decoded.channels[0].iter().any(|x| x.abs() > 1e-3));
    // The toast carries the loudness of what was written.
    let message = c.last_rejection().unwrap();
    assert!(
        message.contains("Exported mix.flac")
            && message.contains("LUFS")
            && message.contains("dBTP"),
        "{message}"
    );
    // 32-bit float has no FLAC form: refused before anything runs.
    let float = ExportValues {
        file: ExportFile::Flac,
        ..values(ExportDepth::Float32)
    };
    let why = c
        .start_audio_export(ExportScope::Song, t.file("float.wav"), float)
        .unwrap_err();
    assert!(why.contains("FLAC"), "{why}");
    assert!(!c.audio_export_busy());
}

#[test]
fn stems_can_be_exported_between_the_locators_without_a_tail() {
    let t = Tmp::new();
    let mut c = core();
    c.apply_command(Command::Song(SongCommand::SetLoop {
        range: (BAR, BAR + BAR / 2),
        enabled: false,
    }))
    .unwrap();
    c.sink_mut().take();
    let folder = t.file("Bounced range");
    let range = ExportValues {
        file: ExportFile::Flac,
        locators: true,
        tail: true,
        ..values(ExportDepth::Pcm24)
    };
    assert_eq!(
        export_request(ExportScope::Stems, &range, None).span,
        ExportSpan::Loop
    );
    assert!(
        export_request(ExportScope::Stems, &range, None)
            .tail_seconds
            .abs()
            < f64::EPSILON
    );
    c.start_audio_export(ExportScope::Stems, folder.clone(), range)
        .unwrap();
    c.wait_background();
    let mut files: Vec<_> = std::fs::read_dir(&folder)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    files.sort();
    assert_eq!(files.len(), 2, "{files:?}");
    for file in &files {
        assert_eq!(file.extension().unwrap(), "flac");
        // Half a bar at 120 BPM and 48 kHz.
        let decoded = atomik_audio_file::decode_file(file).unwrap();
        assert_eq!(decoded.frames(), 48_000, "{}", file.display());
    }
    let message = c.last_rejection().unwrap();
    assert!(
        message.contains("Exported 2 stems") && message.contains("Master"),
        "{message}"
    );
    // The whole song is still exportable as stems.
    assert!(c.can_export_audio(ExportScope::Stems));
}

#[test]
fn a_selection_export_is_exactly_the_range_selected_on_a_lane() {
    let t = Tmp::new();
    let mut c = core();
    // The locators are somewhere else: only the selection counts.
    c.apply_command(Command::Song(SongCommand::SetLoop {
        range: (0, BAR),
        enabled: false,
    }))
    .unwrap();
    c.sink_mut().take();
    assert_eq!(c.export_selection(), None);
    assert!(!c.can_export_audio(ExportScope::Selection));
    let why = c.export_audio_refusal(ExportScope::Selection).unwrap();
    assert!(why.contains("Select a time range"), "{why}");
    let path = t.file("selection.wav");
    let chosen = ExportValues::default();
    assert!(
        c.start_audio_export(ExportScope::Selection, path.clone(), chosen)
            .is_err()
    );
    assert!(!path.exists());

    c.arrangement_mut()
        .set_lane_range(key("lead"), 0, BAR / 2, BAR);
    assert_eq!(c.export_selection(), Some((BAR / 2, BAR)));
    assert!(c.can_export_audio(ExportScope::Selection));
    let request = export_request(ExportScope::Selection, &chosen, c.export_selection());
    assert_eq!(request.span, ExportSpan::Range(BAR / 2, BAR));
    assert!(request.tail_seconds.abs() < f64::EPSILON);
    c.start_audio_export(ExportScope::Selection, path.clone(), chosen)
        .unwrap();
    assert!(c.audio_export_busy());
    assert_eq!(
        c.audio_export_progress().map(|p| p.1).as_deref(),
        Some("Rendering the selection")
    );
    c.wait_background();
    let decoded = atomik_audio_file::decode_file(&path).unwrap();
    // Half a bar at 120 BPM and 48 kHz.
    assert_eq!(decoded.frames(), 48_000);
    assert!(decoded.channels[0].iter().any(|x| x.abs() > 1e-4));
    assert!(
        c.last_rejection()
            .unwrap()
            .contains("Exported selection.wav")
    );
}

#[test]
fn stems_can_be_exported_for_the_selection_and_say_so_when_there_is_none() {
    let t = Tmp::new();
    let mut c = core();
    let folder = t.file("Bounced selection");
    let chosen = ExportValues {
        selection: true,
        file: ExportFile::Flac,
        ..values(ExportDepth::Pcm24)
    };
    assert!(
        c.start_audio_export(ExportScope::Stems, folder.clone(), chosen)
            .is_err()
    );
    assert!(c.last_rejection().unwrap().contains("Select a time range"));
    c.arrangement_mut()
        .set_lane_range(key("lead"), 0, BAR, BAR + BAR / 2);
    assert_eq!(
        export_request(ExportScope::Stems, &chosen, c.export_selection()).span,
        ExportSpan::Range(BAR, BAR + BAR / 2)
    );
    c.start_audio_export(ExportScope::Stems, folder.clone(), chosen)
        .unwrap();
    c.wait_background();
    let mut files: Vec<_> = std::fs::read_dir(&folder)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    files.sort();
    assert_eq!(files.len(), 2, "{files:?}");
    for file in &files {
        let decoded = atomik_audio_file::decode_file(file).unwrap();
        assert_eq!(decoded.frames(), 48_000, "{}", file.display());
    }
    // The selection is not part of what is remembered between sessions.
    let prefs = crate::export_prefs::ExportPrefs::from_values(&chosen, None);
    assert!(!prefs.values().selection);
}

// ---------------------------------------------------------------------- compressed formats

use atomik_ui::{EncoderState, ExportQuality};
use atomik_video::Platform;
use atomik_video::fake::FakeTool;

impl Tmp {
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

/// A core whose FFmpeg is `tool` (it "encodes" by writing its canned bytes).
fn lossy_core(t: &Tmp, tool: &Arc<FakeTool>) -> AppCore<RecordingSink> {
    let mut c = core();
    c.set_video_tool(Arc::clone(tool) as Arc<dyn atomik_video::VideoTool>);
    c.set_video_folder(Some(t.programs()));
    c
}

fn encoder() -> Arc<FakeTool> {
    Arc::new(FakeTool::new().audio(b"ENCODED".to_vec()))
}

fn lossy(file: ExportFile) -> ExportValues {
    ExportValues {
        file,
        ..ExportValues::default()
    }
}

/// The calls that encode (not the version check).
fn encodes(tool: &FakeTool) -> Vec<atomik_video::fake::Call> {
    tool.calls().into_iter().filter(|c| c.has("-vn")).collect()
}

fn tags_of(call: &atomik_video::fake::Call) -> Vec<String> {
    let args: Vec<String> = call
        .args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    args.windows(2)
        .filter(|w| w[0] == "-metadata")
        .map(|w| w[1].clone())
        .collect()
}

#[test]
fn a_song_is_exported_as_mp3_through_ffmpeg_with_the_song_information_as_tags() {
    let t = Tmp::new();
    let tool = encoder();
    let mut c = lossy_core(&t, &tool);
    c.apply_command(Command::SetSongInfo(atomik_model::SongInfo {
        title: "Night \"Drive\"; take=2".to_owned(),
        author: "Zoe".to_owned(),
        ..atomik_model::SongInfo::default()
    }))
    .unwrap();
    let dirty = c.is_dirty();
    let chosen = ExportValues {
        mp3: ExportQuality::Cbr(256),
        ..lossy(ExportFile::Mp3)
    };
    // The dialog proposed a .wav name: the file gets the extension of the format.
    c.start_audio_export(ExportScope::Song, t.file("song.wav"), chosen)
        .unwrap();
    c.wait_background();
    let out = t.file("song.mp3");
    assert_eq!(std::fs::read(&out).unwrap(), b"ENCODED");
    assert!(!t.file("song.wav").exists());
    assert!(
        !atomik_video::encode::partial_path(&out).exists(),
        "no partial file"
    );
    let message = c.last_rejection().unwrap();
    assert!(
        message.contains("Exported song.mp3") && message.contains("MP3 256 kbps"),
        "{message}"
    );
    assert!(
        message.contains("LUFS"),
        "the loudness is reported: {message}"
    );
    let calls = encodes(&tool);
    assert_eq!(calls.len(), 1, "one encoding call");
    let call = &calls[0];
    assert_eq!(call.after("-c:a").unwrap(), "libmp3lame");
    assert_eq!(call.after("-b:a").unwrap(), "256k");
    assert_eq!(
        tags_of(call),
        ["title=Night \"Drive\"; take=2", "artist=Zoe"],
        "title and author, each one argument"
    );
    // The input is the temporary WAV the render wrote: it is gone with its folder.
    let input = call.after("-i").unwrap().to_string_lossy().into_owned();
    let wav = PathBuf::from(input.strip_prefix("file:").unwrap());
    assert!(!wav.exists(), "the temporary WAV was removed");
    assert!(!wav.parent().unwrap().exists());
    assert_eq!(c.is_dirty(), dirty, "the project is untouched");
    assert!(c.sink_mut().take().is_empty(), "the engine was not touched");
}

#[test]
fn aac_and_opus_use_their_own_containers_extensions_and_bitrates() {
    let t = Tmp::new();
    let tool = encoder();
    let mut c = lossy_core(&t, &tool);
    c.start_audio_export(
        ExportScope::Song,
        t.file("a.wav"),
        ExportValues {
            aac: 320,
            ..lossy(ExportFile::Aac)
        },
    )
    .unwrap();
    c.wait_background();
    assert!(t.file("a.m4a").is_file());
    c.start_audio_export(
        ExportScope::Song,
        t.file("b.wav"),
        ExportValues {
            opus: 96,
            ..lossy(ExportFile::Opus)
        },
    )
    .unwrap();
    c.wait_background();
    assert!(t.file("b.opus").is_file());
    let calls = encodes(&tool);
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].after("-c:a").unwrap(), "aac");
    assert_eq!(calls[0].after("-b:a").unwrap(), "320k");
    assert_eq!(calls[0].after("-f").unwrap(), "ipod");
    assert_eq!(calls[1].after("-c:a").unwrap(), "libopus");
    assert_eq!(calls[1].after("-b:a").unwrap(), "96k");
    assert_eq!(calls[1].after("-f").unwrap(), "opus");
    // A song without information writes no tags.
    assert!(tags_of(&calls[0]).is_empty());
}

#[test]
fn a_loop_and_a_selection_are_compressed_through_the_same_path() {
    let t = Tmp::new();
    let tool = encoder();
    let mut c = lossy_core(&t, &tool);
    c.apply_command(Command::Song(SongCommand::SetLoop {
        range: (BAR, BAR + BAR / 2),
        enabled: true,
    }))
    .unwrap();
    c.start_audio_export(
        ExportScope::Loop,
        t.file("loop.wav"),
        lossy(ExportFile::Mp3),
    )
    .unwrap();
    c.wait_background();
    assert!(t.file("loop.mp3").is_file());
    c.arrangement_mut()
        .set_lane_range(key("lead"), 0, BAR, BAR + BAR / 2);
    c.start_audio_export(
        ExportScope::Selection,
        t.file("sel.wav"),
        lossy(ExportFile::Opus),
    )
    .unwrap();
    c.wait_background();
    assert!(t.file("sel.opus").is_file());
    assert_eq!(encodes(&tool).len(), 2);
}

#[test]
fn the_rate_is_one_the_format_holds_and_the_wav_is_float() {
    let mut v = lossy(ExportFile::Mp3);
    let f = export_format_at(&v, 44_100);
    assert_eq!(f.container, ExportContainer::Wav);
    assert_eq!(f.bit_depth, BitDepth::Float32);
    assert_eq!(f.dither, atomik_dsp::DitherMode::None);
    assert_eq!(
        f.sample_rate,
        Some(44_100),
        "the song rate when none is asked"
    );
    v.rate = Some(96_000);
    assert_eq!(export_format_at(&v, 44_100).sample_rate, Some(48_000));
    v.file = ExportFile::Opus;
    v.rate = None;
    assert_eq!(export_format_at(&v, 44_100).sample_rate, Some(48_000));
    v.file = ExportFile::Aac;
    v.rate = Some(96_000);
    assert_eq!(export_format_at(&v, 44_100).sample_rate, Some(96_000));
    // A lossless format is the plain format of the panel.
    let wav = ExportValues {
        depth: ExportDepth::Pcm16,
        ..ExportValues::default()
    };
    assert_eq!(export_format_at(&wav, 44_100), export_format(&wav));
    assert_eq!(file_extension(&wav), "wav");
    assert_eq!(file_extension(&lossy(ExportFile::Aac)), "m4a");
}

#[test]
fn a_cancel_before_the_encoder_runs_leaves_no_file_and_no_temporary_wav() {
    let t = Tmp::new();
    let tool = encoder();
    let mut c = lossy_core(&t, &tool);
    let hold = crate::export_worker::hold::next_export();
    c.start_audio_export(
        ExportScope::Song,
        t.file("song.wav"),
        lossy(ExportFile::Mp3),
    )
    .unwrap();
    c.cancel_audio_export();
    drop(hold);
    c.wait_background();
    assert!(!t.file("song.mp3").exists());
    assert!(encodes(&tool).is_empty(), "the encoder never started");
    assert!(c.last_rejection().unwrap().contains("cancelled"));
}

#[test]
fn a_cancel_while_the_encoder_runs_removes_the_partial_file_and_the_temporary_wav() {
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
    let mut c = lossy_core(&t, &tool);
    c.start_audio_export(
        ExportScope::Song,
        t.file("song.wav"),
        lossy(ExportFile::Mp3),
    )
    .unwrap();
    // The encoder has written part of its file: stop the export.
    reached
        .recv_timeout(std::time::Duration::from_secs(60))
        .unwrap();
    c.cancel_audio_export();
    release.send(()).unwrap();
    c.wait_background();
    assert!(!c.audio_export_busy());
    let out = t.file("song.mp3");
    assert!(!out.exists(), "no result file");
    assert!(
        !atomik_video::encode::partial_path(&out).exists(),
        "no partial file"
    );
    let call = &encodes(&tool)[0];
    let input = call.after("-i").unwrap().to_string_lossy().into_owned();
    let wav = PathBuf::from(input.strip_prefix("file:").unwrap());
    assert!(!wav.exists(), "the temporary WAV was removed");
    assert!(c.last_rejection().unwrap().contains("cancelled"));
}

#[test]
fn a_failing_encoder_says_why_and_leaves_nothing_but_an_old_file() {
    let t = Tmp::new();
    let tool = Arc::new(
        FakeTool::new()
            .audio(b"HALF".to_vec())
            .stream_fails("Unknown encoder 'libopus'"),
    );
    let mut c = lossy_core(&t, &tool);
    std::fs::write(t.file("song.opus"), b"old").unwrap();
    c.start_audio_export(
        ExportScope::Song,
        t.file("song.wav"),
        lossy(ExportFile::Opus),
    )
    .unwrap();
    c.wait_background();
    let message = c.last_rejection().unwrap();
    assert!(
        message.contains("Export of song.opus failed") && message.contains("libopus"),
        "{message}"
    );
    assert_eq!(std::fs::read(t.file("song.opus")).unwrap(), b"old");
    assert!(!atomik_video::encode::partial_path(&t.file("song.opus")).exists());
}

#[test]
fn without_ffmpeg_the_compressed_formats_are_refused_and_the_panel_is_told() {
    let t = Tmp::new();
    // No real tools in this core (a headless run).
    let mut c = core();
    let err = c
        .start_audio_export(ExportScope::Song, t.file("a.wav"), lossy(ExportFile::Mp3))
        .unwrap_err();
    assert!(err.contains("FFmpeg"), "{err}");
    assert!(!c.audio_export_busy());
    // Tools, but no FFmpeg in the folder named in Settings.
    let empty = t.file("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let mut c = core();
    c.set_video_tool(Arc::new(FakeTool::new()));
    c.set_video_folder(Some(empty));
    assert_eq!(c.export_encoder(), EncoderState::Checking);
    c.check_ffmpeg().unwrap();
    c.wait_background();
    let EncoderState::Missing(why) = c.export_encoder() else {
        panic!("{:?}", c.export_encoder());
    };
    assert!(
        why.contains("Settings > Video") && why.contains("PATH"),
        "{why}"
    );
    assert!(why.len() <= 90, "short enough for a tooltip: {why}");
    let err = c
        .start_audio_export(ExportScope::Song, t.file("a.wav"), lossy(ExportFile::Aac))
        .unwrap_err();
    assert_eq!(err, why);
    assert!(!t.file("a.m4a").exists());
    // WAV still works in the same core.
    c.start_audio_export(
        ExportScope::Song,
        t.file("a.wav"),
        values(ExportDepth::Pcm16),
    )
    .unwrap();
    c.wait_background();
    assert!(t.file("a.wav").is_file());
}

#[test]
fn ffmpeg_found_makes_the_encoder_ready() {
    let t = Tmp::new();
    let tool = encoder();
    let mut c = lossy_core(&t, &tool);
    assert_eq!(c.export_encoder(), EncoderState::Checking);
    c.check_ffmpeg().unwrap();
    c.wait_background();
    assert_eq!(c.export_encoder(), EncoderState::Ready);
}

#[test]
fn stems_are_not_compressed() {
    let t = Tmp::new();
    let tool = encoder();
    let mut c = lossy_core(&t, &tool);
    let err = c
        .start_audio_export(ExportScope::Stems, t.file("stems"), lossy(ExportFile::Mp3))
        .unwrap_err();
    assert!(err.contains("WAV, AIFF or FLAC"), "{err}");
    assert!(encodes(&tool).is_empty());
    assert!(!c.audio_export_busy());
}

#[test]
fn the_export_choices_for_a_compressed_format_survive_a_restart() {
    let t = Tmp::new();
    let chosen = ExportValues {
        file: ExportFile::Mp3,
        mp3: ExportQuality::Vbr(2),
        aac: 256,
        opus: 160,
        ..ExportValues::default()
    };
    let path = t.file("export.json");
    crate::export_prefs::ExportPrefs::from_values(&chosen, None)
        .save(&path)
        .unwrap();
    assert_eq!(
        crate::export_prefs::ExportPrefs::load(&path).values(),
        chosen
    );
}

/// The real FFmpeg, when it is installed: one MP3 export of the starter project. Run with
/// `cargo test -p atomik-app --lib real_ffmpeg -- --ignored`.
#[test]
#[ignore = "needs a real FFmpeg"]
fn real_ffmpeg_exports_an_mp3_of_the_song() {
    let t = Tmp::new();
    let mut c = core();
    c.set_video_tool(Arc::new(atomik_video::SystemTool));
    c.check_ffmpeg().unwrap();
    c.wait_background();
    if c.export_encoder() != EncoderState::Ready {
        println!("no FFmpeg here: {:?}", c.export_encoder());
        return;
    }
    c.start_audio_export(
        ExportScope::Song,
        t.file("real.wav"),
        lossy(ExportFile::Mp3),
    )
    .unwrap();
    c.wait_background();
    let message = c.last_rejection().unwrap().to_owned();
    println!("{message}");
    let decoded = atomik_audio_file::decode_file(&t.file("real.mp3"));
    assert!(message.contains("Exported real.mp3"), "{message}");
    if let Ok(audio) = decoded {
        assert!(audio.frames() > 1000);
    }
}

// ---------------------------------------------------------------------------------------------
// Export presets: the mix-down, the loudness target and the extra MP3.

use atomik_render::loudness::measure;
use atomik_ui::ExportLoudness;

fn measured(
    path: &Path,
) -> (
    atomik_audio_file::AudioData,
    atomik_render::loudness::LoudnessReport,
) {
    let audio = atomik_audio_file::decode_file(path).unwrap();
    let report = measure(&audio);
    (audio, report)
}

/// The Streaming preset without its MP3: the WAV-only form, which needs no FFmpeg.
fn streaming_wav() -> ExportValues {
    ExportValues {
        also_mp3: false,
        ..crate::export_presets::builtin()[0].values
    }
}

#[test]
fn the_streaming_preset_brings_a_wav_export_to_minus_14_with_the_peaks_under_minus_1() {
    let t = Tmp::new();
    let mut c = core();
    c.start_audio_export(ExportScope::Song, t.file("song.wav"), streaming_wav())
        .unwrap();
    c.wait_background();
    let (audio, after) = measured(&t.file("song.wav"));
    assert_eq!(audio.channels.len(), 2);
    let lufs = after.integrated_lufs.expect("the song is audible");
    assert!((lufs + 14.0).abs() <= 0.5, "{after:?}");
    assert!(after.true_peak_dbtp.unwrap() <= -1.0, "{after:?}");
    // The report says what was measured before and what was done.
    let message = c.last_rejection().unwrap();
    assert!(
        message.contains("Exported song.wav")
            && message.contains("; was ")
            && message.contains("LUFS"),
        "{message}"
    );
    assert!(
        message.contains("turned up") || message.contains("turned down"),
        "{message}"
    );
}

#[test]
fn a_peaky_song_stays_under_the_ceiling_and_the_report_names_any_limiting() {
    let t = Tmp::new();
    let mut c = core();
    // Very short loud notes: peaks that a gain towards the target would push past the ceiling.
    c.apply_command(Command::Song(SongCommand::AddClip {
        device: key("lead"),
        lane: 0,
        clip: Clip {
            id: ClipId(1),
            start: 2 * BAR,
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
                (0..16)
                    .map(|i| Note::new(i * PPQ / 2, PPQ / 64, 40, 1.0))
                    .collect::<Vec<_>>()
                    .into(),
            ),
        },
    }))
    .unwrap();
    let loud = ExportValues {
        loudness: ExportLoudness::Target(-6),
        ..streaming_wav()
    };
    c.start_audio_export(ExportScope::Song, t.file("peaky.wav"), loud)
        .unwrap();
    c.wait_background();
    let (_, after) = measured(&t.file("peaky.wav"));
    assert!(after.true_peak_dbtp.unwrap() <= -1.0, "{after:?}");
    let message = c.last_rejection().unwrap();
    assert!(message.contains("Exported peaky.wav"), "{message}");
    if message.contains("limited") {
        assert!(message.contains("under -1.0 dBTP"), "{message}");
    }
}

#[test]
fn a_guide_changes_nothing_and_a_mono_export_has_one_channel() {
    let t = Tmp::new();
    let mut c = core();
    c.start_audio_export(
        ExportScope::Song,
        t.file("plain.wav"),
        values(ExportDepth::Pcm24),
    )
    .unwrap();
    c.wait_background();
    let guide = ExportValues {
        loudness: ExportLoudness::Guide(-9),
        ..values(ExportDepth::Pcm24)
    };
    c.start_audio_export(ExportScope::Song, t.file("club.wav"), guide)
        .unwrap();
    c.wait_background();
    let (plain, _) = measured(&t.file("plain.wav"));
    let (club, _) = measured(&t.file("club.wav"));
    assert_eq!(
        plain.channels, club.channels,
        "a guide adds no gain and no limiter"
    );
    let message = c.last_rejection().unwrap();
    assert!(
        message.contains("-9 LUFS guide") && message.contains("nothing changed"),
        "{message}"
    );

    let mono = ExportValues {
        mono: true,
        ..values(ExportDepth::Pcm24)
    };
    c.start_audio_export(ExportScope::Song, t.file("mono.wav"), mono)
        .unwrap();
    c.wait_background();
    let (m, _) = measured(&t.file("mono.wav"));
    assert_eq!(m.channels.len(), 1);
    assert_eq!(m.frames(), plain.frames());
}

#[test]
fn stems_ignore_the_loudness_and_the_mix_down() {
    let t = Tmp::new();
    let mut c = core();
    let stems = ExportValues {
        mono: true,
        loudness: ExportLoudness::Target(-14),
        also_mp3: true,
        ..values(ExportDepth::Pcm24)
    };
    c.start_audio_export(ExportScope::Stems, t.file("stems"), stems)
        .unwrap();
    c.wait_background();
    let message = c.last_rejection().unwrap();
    assert!(message.starts_with("Exported"), "{message}");
    let files: Vec<_> = std::fs::read_dir(t.file("stems"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert!(!files.is_empty());
    for f in files {
        assert_eq!(f.extension().unwrap(), "wav", "no extra MP3 for stems");
        assert_eq!(measured(&f).0.channels.len(), 2, "stems stay stereo");
    }
}

#[test]
fn an_extra_mp3_is_encoded_from_the_same_finished_render_next_to_the_wav() {
    let t = Tmp::new();
    let tool = encoder();
    let mut c = lossy_core(&t, &tool);
    let both = ExportValues {
        also_mp3: true,
        ..streaming_wav()
    };
    c.start_audio_export(ExportScope::Song, t.file("song.wav"), both)
        .unwrap();
    c.wait_background();
    assert!(t.file("song.wav").is_file());
    let mp3 = t.file("song.mp3");
    assert_eq!(std::fs::read(&mp3).unwrap(), b"ENCODED");
    assert!(!atomik_video::encode::partial_path(&mp3).exists());
    let message = c.last_rejection().unwrap();
    assert!(
        message.contains("Exported song.wav, and song.mp3 (MP3 320 kbps)"),
        "{message}"
    );
    let calls = encodes(&tool);
    assert_eq!(calls.len(), 1, "one encoding call");
    assert_eq!(calls[0].after("-c:a").unwrap(), "libmp3lame");
    assert_eq!(calls[0].after("-b:a").unwrap(), "320k");
    // The temporary WAV it read is gone with its folder.
    let input = calls[0].after("-i").unwrap().to_string_lossy().into_owned();
    let wav = PathBuf::from(input.strip_prefix("file:").unwrap());
    assert!(!wav.exists() && !wav.parent().unwrap().exists());
}

#[test]
fn the_extra_mp3_needs_ffmpeg_and_a_cancel_leaves_no_file() {
    let t = Tmp::new();
    let mut c = core();
    let both = ExportValues {
        also_mp3: true,
        ..streaming_wav()
    };
    let why = c
        .start_audio_export(ExportScope::Song, t.file("song.wav"), both)
        .unwrap_err();
    assert!(why.contains("FFmpeg"), "{why}");
    assert!(!t.file("song.wav").exists());

    // Cancelled before the render starts: nothing is written, not the WAV and not the MP3.
    let tool = encoder();
    let mut c = lossy_core(&t, &tool);
    let hold = crate::export_worker::hold::next_export();
    c.start_audio_export(ExportScope::Song, t.file("song.wav"), both)
        .unwrap();
    c.cancel_audio_export();
    drop(hold);
    c.wait_background();
    assert!(!t.file("song.wav").exists() && !t.file("song.mp3").exists());
}

#[test]
fn the_extra_file_and_the_finish_follow_the_values() {
    use super::export::{extra_file, finish_for, wants_extra_mp3};
    let v = ExportValues {
        also_mp3: true,
        ..ExportValues::default()
    };
    assert_eq!(
        extra_file(ExportScope::Song, Path::new("d/song.wav"), &v),
        Some(PathBuf::from("d/song.mp3"))
    );
    assert!(extra_file(ExportScope::Stems, Path::new("d/stems"), &v).is_none());
    assert!(!wants_extra_mp3(ExportScope::Song, &lossy(ExportFile::Mp3)));
    assert!(finish_for(false, ExportLoudness::Off).is_noop());
    let f = finish_for(true, ExportLoudness::Target(-16));
    assert!(f.mono);
    assert_eq!(
        f.loudness,
        atomik_render::normalise::Loudness::Target {
            lufs: -16.0,
            ceiling_dbtp: -1.0
        }
    );
}

#[test]
#[ignore = "needs a real FFmpeg"]
fn real_ffmpeg_writes_the_streaming_preset_as_a_wav_and_an_mp3() {
    let t = Tmp::new();
    let mut c = core();
    c.set_video_tool(Arc::new(atomik_video::SystemTool));
    c.check_ffmpeg().unwrap();
    c.wait_background();
    if c.export_encoder() != EncoderState::Ready {
        println!("no FFmpeg here: {:?}", c.export_encoder());
        return;
    }
    let streaming = crate::export_presets::builtin()[0].values;
    c.start_audio_export(ExportScope::Song, t.file("stream.wav"), streaming)
        .unwrap();
    c.wait_background();
    let message = c.last_rejection().unwrap().to_owned();
    println!("{message}");
    let (_, wav) = measured(&t.file("stream.wav"));
    assert!(
        (wav.integrated_lufs.unwrap() + 14.0).abs() <= 0.5,
        "{wav:?}"
    );
    assert!(wav.true_peak_dbtp.unwrap() <= -1.0, "{wav:?}");
    if let Ok(audio) = atomik_audio_file::decode_file(&t.file("stream.mp3")) {
        let heard = measure(&audio);
        assert!(
            (heard.integrated_lufs.unwrap() + 14.0).abs() <= 1.0,
            "{heard:?}"
        );
    }
}

#[test]
#[ignore = "needs a real FFmpeg"]
fn real_ffmpeg_writes_the_podcast_preset_as_a_mono_mp3_at_minus_16() {
    let t = Tmp::new();
    let mut c = core();
    c.set_video_tool(Arc::new(atomik_video::SystemTool));
    c.check_ffmpeg().unwrap();
    c.wait_background();
    if c.export_encoder() != EncoderState::Ready {
        println!("no FFmpeg here: {:?}", c.export_encoder());
        return;
    }
    let podcast = crate::export_presets::builtin()[1].values;
    c.start_audio_export(ExportScope::Song, t.file("pod.wav"), podcast)
        .unwrap();
    c.wait_background();
    let message = c.last_rejection().unwrap().to_owned();
    println!("{message}");
    assert!(message.contains("Exported pod.mp3"), "{message}");
    // The file holds one channel (asked of ffprobe, which is installed with FFmpeg).
    if let Ok(out) = std::process::Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "a:0", "-show_entries"])
        .args(["stream=channels", "-of", "csv=p=0"])
        .arg(t.file("pod.mp3"))
        .output()
    {
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            "1",
            "a mono file"
        );
    }
    if let Ok(audio) = atomik_audio_file::decode_file(&t.file("pod.mp3")) {
        let heard = measure(&audio);
        assert!(
            (heard.integrated_lufs.unwrap() + 16.0).abs() <= 1.0,
            "{heard:?}"
        );
    }
}
