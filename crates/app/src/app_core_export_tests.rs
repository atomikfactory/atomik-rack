//! Exporting the song and the loop as audio files in the app core: the render runs on a worker and
//! writes the file the options describe, the live engine is not touched, a cancel leaves no
//! file, one export runs at a time, and the panel values map to the render request and format.

use super::*;
use crate::sink::{RecordingSink, SinkCall};
use atomik_core::PPQ;
use atomik_model::{
    Clip, ClipContent, CreateOptions, CreateSource, Note, WorkflowCommand, new_project_doc,
};
use atomik_render::export::BitDepth;
use atomik_ui::{EXPORT_RATES, ExportDepth, ExportDither, ExportScope, ExportValues};
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
            controllers: Vec::new(),
            grid: None,
            content: ClipContent::Notes(vec![
                Note::new(0, PPQ, 64, 0.8),
                Note::new(PPQ, PPQ, 67, 0.8),
            ]),
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
}

#[test]
fn the_panel_values_map_to_the_render_request_and_format() {
    let mut v = ExportValues {
        depth: ExportDepth::Pcm16,
        dither: ExportDither::Shaped,
        rate: Some(96_000),
        tail: true,
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
    assert!(export_request(ExportScope::Song, &v).tail_seconds > 0.0);
    v.tail = false;
    assert!(export_request(ExportScope::Song, &v).tail_seconds.abs() < f64::EPSILON);
    v.tail = true;
    assert!(export_request(ExportScope::Loop, &v).tail_seconds.abs() < f64::EPSILON);
}
