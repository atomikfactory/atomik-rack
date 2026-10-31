//! Exporting the song and the loop as audio files through the offline render: the song runs from
//! the start to the End plus an audible tail, the loop is exactly the frames between the locators,
//! both are the bounce of the same project (bit for bit before the quantisation), the bit depth
//! and the dither land in the file, and a stopped export leaves no file.

use atomik_core::PPQ;
use atomik_model::{RackDoc, load_project};
use atomik_render::default_registry;
use atomik_render::export::{
    BitDepth, DEFAULT_TAIL_SECONDS, ExportFormat, ExportRequest, ExportSpan, TAIL_FLOOR,
    export_audio_file, partial_path, render_export, write_export,
};
use atomik_render::plugin_pass::RenderControl;
use atomik_render::project::{PluginBounce, ProjectOptions, render_doc};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// The demo project: 8 bars at 120 BPM in 4/4 (16 s at 48 kHz), the End at bar 9.
const END_FRAMES: usize = 16 * 48_000;
const BAR: i64 = 4 * PPQ;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "atomik-render-export-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        Self(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn demo() -> RackDoc {
    let text = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/projects/demo.atomik"),
    )
    .expect("demo project");
    load_project(&text, &default_registry()).expect("demo loads")
}

fn options() -> ProjectOptions {
    ProjectOptions {
        host_block: NonZeroUsize::new(512).unwrap(),
        workers: 0,
        loop_count: None,
    }
}

fn song(tail_seconds: f64) -> ExportRequest {
    ExportRequest {
        span: ExportSpan::Song,
        tail_seconds,
    }
}

fn render(doc: &RackDoc, request: &ExportRequest) -> atomik_render::export::ExportRender {
    render_export(
        doc,
        &default_registry(),
        (&options(), None),
        &mut PluginBounce::default(),
        request,
        RenderControl::default(),
    )
    .expect("renders")
    .expect("not stopped")
}

fn bits(channels: &[Vec<f32>]) -> Vec<u32> {
    channels.iter().flatten().map(|x| x.to_bits()).collect()
}

#[test]
fn a_song_export_without_a_tail_is_exactly_the_start_to_the_end() {
    let doc = demo();
    let export = render(&doc, &song(0.0));
    assert_eq!(export.audio.frames(), END_FRAMES);
    assert_eq!(export.span_frames, END_FRAMES as u64);
    assert_eq!(export.tail_frames, 0);
    assert_eq!(export.audio.channels.len(), 2);
    assert_eq!(export.audio.sample_rate, 48_000);
    // The same samples as the bounce of the project (which goes two beats past the End).
    let bounce = render_doc(&doc, &default_registry(), &options()).unwrap();
    assert_eq!(
        bits(&export.audio.channels),
        bits(
            &bounce
                .channels
                .iter()
                .map(|c| c[..END_FRAMES].to_vec())
                .collect::<Vec<_>>()
        )
    );
}

#[test]
fn a_song_export_keeps_the_tail_while_it_is_audible_and_never_cuts_before_the_end() {
    let doc = demo();
    let export = render(&doc, &song(DEFAULT_TAIL_SECONDS));
    let frames = export.audio.frames();
    assert!(frames >= END_FRAMES);
    assert!(frames <= END_FRAMES + (DEFAULT_TAIL_SECONDS * 48_000.0) as usize);
    assert_eq!(export.tail_frames as usize, frames - END_FRAMES);
    // Past the End the demo (a release and an effect) rings out, then is cut where it fades.
    assert!(export.tail_frames > 0, "the demo has a tail");
    let last = export
        .audio
        .channels
        .iter()
        .map(|c| c.last().copied().unwrap().abs())
        .fold(0.0f32, f32::max);
    // The last frame of the tail is the last one above the floor, or the cap was reached.
    assert!(
        last > TAIL_FLOOR || frames == END_FRAMES + (DEFAULT_TAIL_SECONDS * 48_000.0) as usize,
        "ends at {last}"
    );
    // The part up to the End is the no-tail export, bit for bit.
    let plain = render(&doc, &song(0.0));
    for (a, b) in export.audio.channels.iter().zip(&plain.audio.channels) {
        assert_eq!(
            bits(&[a[..END_FRAMES].to_vec()]),
            bits(std::slice::from_ref(b))
        );
    }
}

#[test]
fn a_song_without_anything_after_the_end_gets_no_tail() {
    // A dry, silent-after-the-end project: remove everything that rings by muting the song and
    // keeping the End: the output is silence, so no frame is above the floor and the file is
    // cut to the End.
    let mut doc = demo();
    for track in &mut doc.song_mut().tracks {
        track.muted = true;
    }
    let export = render(&doc, &song(DEFAULT_TAIL_SECONDS));
    assert_eq!(export.audio.frames(), END_FRAMES);
    assert_eq!(export.tail_frames, 0);
}

#[test]
fn a_loop_export_is_exactly_the_frames_between_the_locators_and_has_no_tail() {
    let mut doc = demo();
    doc.song_mut().loop_range = (BAR, 3 * BAR + BAR / 4);
    // Whether the loop is on does not matter.
    doc.song_mut().loop_enabled = false;
    let export = render(
        &doc,
        &ExportRequest {
            span: ExportSpan::Loop,
            tail_seconds: DEFAULT_TAIL_SECONDS,
        },
    );
    let (from, to) = (2 * 48_000, 6 * 48_000 + 24_000);
    assert_eq!(export.audio.frames(), to - from);
    assert_eq!(export.span_frames, (to - from) as u64);
    assert_eq!(export.tail_frames, 0);
    // The samples are those of the bounce between the locators.
    let bounce = render_doc(&doc, &default_registry(), &options()).unwrap();
    assert_eq!(
        bits(&export.audio.channels),
        bits(
            &bounce
                .channels
                .iter()
                .map(|c| c[from..to].to_vec())
                .collect::<Vec<_>>()
        )
    );
    let audible = export.audio.channels[0].iter().any(|x| x.abs() > 1e-3);
    assert!(audible, "the loop plays");
}

#[test]
fn an_empty_loop_or_song_is_refused_and_a_bad_tail_too() {
    let registry = default_registry();
    let run = |doc: &RackDoc, request: &ExportRequest| {
        render_export(
            doc,
            &registry,
            (&options(), None),
            &mut PluginBounce::default(),
            request,
            RenderControl::default(),
        )
    };
    let mut doc = demo();
    doc.song_mut().loop_range = (BAR, BAR);
    let err = run(
        &doc,
        &ExportRequest {
            span: ExportSpan::Loop,
            tail_seconds: 0.0,
        },
    )
    .unwrap_err();
    assert!(err.to_string().contains("loop is empty"), "{err}");
    let err = run(&demo(), &song(61.0)).unwrap_err();
    assert!(err.to_string().contains("tail"), "{err}");
}

#[test]
fn the_file_has_the_chosen_depth_and_matches_the_bounce_within_the_quantisation() {
    let doc = demo();
    let dir = TempDir::new("depth");
    let export = render(&doc, &song(0.0));
    for (depth, name, tolerance) in [
        // TPDF dither of ±1 LSB, then the rounding: 1.5 LSB at most.
        (BitDepth::Pcm16, "16.wav", 1.5 / 32_768.0),
        (BitDepth::Pcm24, "24.wav", 0.5 / 8_388_608.0),
        (BitDepth::Float32, "f32.wav", 0.0),
    ] {
        let path = dir.path(name);
        let report = write_export(&path, &export, &ExportFormat::new(depth)).unwrap();
        assert_eq!(report.frames, END_FRAMES as u64);
        assert_eq!(report.sample_rate, 48_000);
        assert!(!partial_path(&path).exists(), "the temporary file is gone");
        let spec = hound::WavReader::open(&path).unwrap().spec();
        let (bits_per_sample, format) = match depth {
            BitDepth::Pcm16 => (16, hound::SampleFormat::Int),
            BitDepth::Pcm24 => (24, hound::SampleFormat::Int),
            BitDepth::Float32 => (32, hound::SampleFormat::Float),
        };
        assert_eq!(
            (spec.bits_per_sample, spec.sample_format, spec.channels),
            (bits_per_sample, format, 2)
        );
        let decoded = atomik_audio_file::decode_file(&path).unwrap();
        assert_eq!(decoded.frames(), END_FRAMES);
        for (a, b) in export.audio.channels.iter().zip(&decoded.channels) {
            let worst = a
                .iter()
                .zip(b)
                .map(|(x, y)| (x - y).abs())
                .fold(0.0, f32::max);
            assert!(worst <= tolerance + 1e-6, "{name}: {worst}");
        }
    }
}

#[test]
fn the_same_project_exports_the_same_bytes_and_the_dither_choice_changes_them() {
    let doc = demo();
    let dir = TempDir::new("bytes");
    let export = render(&doc, &song(0.0));
    let bytes = |name: &str, format: ExportFormat| {
        let path = dir.path(name);
        write_export(&path, &export, &format).unwrap();
        std::fs::read(path).unwrap()
    };
    let a = bytes("a.wav", ExportFormat::new(BitDepth::Pcm16));
    assert_eq!(a, bytes("b.wav", ExportFormat::new(BitDepth::Pcm16)));
    let plain = ExportFormat {
        dither: atomik_dsp::DitherMode::None,
        ..ExportFormat::new(BitDepth::Pcm16)
    };
    assert_ne!(a, bytes("c.wav", plain));
}

#[test]
fn another_sample_rate_converts_the_loop_to_the_rounded_frame_count() {
    let mut doc = demo();
    doc.song_mut().loop_range = (0, BAR);
    let export = render(
        &doc,
        &ExportRequest {
            span: ExportSpan::Loop,
            tail_seconds: 0.0,
        },
    );
    assert_eq!(export.audio.frames(), 2 * 48_000);
    let dir = TempDir::new("rate");
    for rate in [44_100u32, 88_200, 96_000] {
        let path = dir.path(&format!("{rate}.wav"));
        let format = ExportFormat {
            sample_rate: Some(rate),
            ..ExportFormat::new(BitDepth::Pcm24)
        };
        let report = write_export(&path, &export, &format).unwrap();
        assert_eq!(report.sample_rate, rate);
        assert_eq!(report.frames, 2 * u64::from(rate));
        let decoded = atomik_audio_file::decode_file(&path).unwrap();
        assert_eq!(decoded.sample_rate, rate);
        assert_eq!(decoded.frames(), 2 * rate as usize);
    }
    let bad = ExportFormat {
        sample_rate: Some(1_000),
        ..ExportFormat::new(BitDepth::Pcm24)
    };
    assert!(write_export(&dir.path("bad.wav"), &export, &bad).is_err());
    assert!(!dir.path("bad.wav").exists());
}

#[test]
fn a_stopped_export_writes_no_file_and_keeps_the_one_that_was_there() {
    let doc = demo();
    let dir = TempDir::new("cancel");
    let path = dir.path("song.wav");
    std::fs::write(&path, b"the previous export").unwrap();
    let cancel = AtomicBool::new(true);
    let control = RenderControl {
        cancel: Some(&cancel),
        progress: None,
    };
    let done = export_audio_file(
        &doc,
        &default_registry(),
        (&options(), None),
        &mut PluginBounce::default(),
        (&song(0.0), &ExportFormat::new(BitDepth::Pcm16)),
        (&path, control),
    )
    .unwrap();
    assert!(done.is_none());
    assert_eq!(std::fs::read(&path).unwrap(), b"the previous export");
    assert!(!partial_path(&path).exists());
    // A complete export replaces it.
    let done = export_audio_file(
        &doc,
        &default_registry(),
        (&options(), None),
        &mut PluginBounce::default(),
        (&song(0.0), &ExportFormat::new(BitDepth::Pcm16)),
        (&path, RenderControl::default()),
    )
    .unwrap()
    .expect("finished");
    assert_eq!(done.frames, END_FRAMES as u64);
    assert_eq!(
        atomik_audio_file::decode_file(&path).unwrap().frames(),
        END_FRAMES
    );
}

#[test]
fn progress_is_reported_while_the_export_renders() {
    let doc = demo();
    let seen = std::sync::Mutex::new(Vec::new());
    let report = |p: f32| seen.lock().unwrap().push(p);
    let control = RenderControl {
        cancel: None,
        progress: Some(&report),
    };
    render_export(
        &doc,
        &default_registry(),
        (&options(), None),
        &mut PluginBounce::default(),
        &song(0.0),
        control,
    )
    .unwrap()
    .unwrap();
    let seen = seen.into_inner().unwrap();
    assert!(seen.len() > 3, "{seen:?}");
    assert!(seen.windows(2).all(|w| w[0] <= w[1]));
    assert!(seen.iter().all(|p| (0.0..=1.0).contains(p)));
}
