//! Stems: the strips of the console rendered to audio of their own. They line up with each other
//! and with the master (a latent insert is compensated), the channels add up to the master, a
//! stem is the same bits at every host block size and worker count, the files are named and
//! numbered in console order, and a stopped export leaves nothing behind. The AIFF container
//! round-trips what the WAV one holds.

use atomik_model::{RackDoc, StemKind, StemSource, load_project, stem_sources};
use atomik_render::default_registry;
use atomik_render::export::{
    BitDepth, ExportContainer, ExportFormat, ExportRequest, ExportSpan, render_export, write_export,
};
use atomik_render::plugin_pass::RenderControl;
use atomik_render::project::{PluginBounce, ProjectOptions};
use atomik_render::stems::{
    Stem, StemRequest, StemStrip, export_stems, render_stems, stems_folder_name,
};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "atomik-render-stems-{tag}-{}-{n}",
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

/// Two clicks, one of them through an insert with 2400 samples of latency.
fn clicks() -> RackDoc {
    let text = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/projects/pdc-clicks.atomik"),
    )
    .expect("example project");
    load_project(&text, &default_registry()).expect("loads")
}

fn options(block: usize, workers: u16) -> ProjectOptions {
    ProjectOptions {
        host_block: NonZeroUsize::new(block).unwrap(),
        workers,
        loop_count: None,
    }
}

fn request(doc: &RackDoc, strip: StemStrip) -> StemRequest {
    StemRequest {
        span: ExportSpan::Song,
        tail_seconds: 0.0,
        strip,
        sources: stem_sources(doc, &default_registry()),
    }
}

fn stems(doc: &RackDoc, request: &StemRequest, options: &ProjectOptions) -> Vec<Stem> {
    let mut out = Vec::new();
    let mut take = |s: Stem| {
        out.push(s);
        Ok(())
    };
    render_stems(
        doc,
        &default_registry(),
        (options, None),
        &mut PluginBounce::default(),
        request,
        (RenderControl::default(), &mut take),
    )
    .expect("renders")
    .expect("not stopped");
    out
}

fn onset(channels: &[Vec<f32>]) -> usize {
    channels[0]
        .iter()
        .zip(&channels[1])
        .position(|(l, r)| l.abs().max(r.abs()) > 0.01)
        .expect("a sound")
}

fn bits(stems: &[Stem]) -> Vec<Vec<u32>> {
    stems
        .iter()
        .map(|s| {
            s.audio
                .channels
                .iter()
                .flatten()
                .map(|x| x.to_bits())
                .collect()
        })
        .collect()
}

#[test]
fn the_stems_line_up_with_the_master_through_a_latent_insert() {
    let doc = clicks();
    let all = stems(&doc, &request(&doc, StemStrip::Full), &options(512, 0));
    let kinds: Vec<StemKind> = all.iter().map(|s| s.source.kind).collect();
    assert_eq!(
        kinds,
        [StemKind::Channel, StemKind::Channel, StemKind::Master]
    );
    // Click B's insert delays it by 2400 samples: Click A is held back to match, and the
    // onset of every stem is where the master has it.
    let delays: Vec<u64> = all.iter().map(|s| s.delay_frames).collect();
    assert_eq!(delays, [2400, 0, 0], "{:?}", delays);
    let onsets: Vec<usize> = all.iter().map(|s| onset(&s.audio.channels)).collect();
    assert_eq!(onsets[0], onsets[2], "A against the master: {onsets:?}");
    assert_eq!(onsets[1], onsets[2], "B against the master: {onsets:?}");
}

#[test]
fn the_channels_add_up_to_the_master() {
    let doc = clicks();
    let all = stems(&doc, &request(&doc, StemStrip::Full), &options(512, 0));
    let master = render_export(
        &doc,
        &default_registry(),
        (&options(512, 0), None),
        &mut PluginBounce::default(),
        &ExportRequest {
            span: ExportSpan::Song,
            tail_seconds: 0.0,
        },
        RenderControl::default(),
    )
    .unwrap()
    .unwrap();
    let frames = master.audio.frames();
    for s in &all {
        assert_eq!(s.audio.frames(), frames, "{}", s.source.name);
    }
    // The master's own stem is the export itself, bit for bit.
    assert_eq!(all[2].audio.channels, master.audio.channels);
    let worst = (0..2)
        .flat_map(|c| (0..frames).map(move |i| (c, i)))
        .map(|(c, i)| {
            let sum = all[0].audio.channels[c][i] + all[1].audio.channels[c][i];
            (sum - master.audio.channels[c][i]).abs()
        })
        .fold(0.0f32, f32::max);
    assert!(
        worst < 1e-5,
        "the channels differ from the master by {worst}"
    );
}

#[test]
fn a_stem_is_the_same_bits_at_every_block_size_and_worker_count() {
    let doc = clicks();
    let req = request(&doc, StemStrip::Full);
    let reference = bits(&stems(&doc, &req, &options(512, 0)));
    for (block, workers) in [(64, 0), (100, 0), (512, 2), (4096, 3)] {
        assert_eq!(
            bits(&stems(&doc, &req, &options(block, workers))),
            reference,
            "block {block}, {workers} workers"
        );
    }
    // And the pass size changes nothing: one strip per request gives the same bits.
    let first = StemRequest {
        sources: vec![req.sources[1].clone()],
        ..req
    };
    assert_eq!(
        bits(&stems(&doc, &first, &options(512, 0)))[0],
        reference[1]
    );
}

#[test]
fn dry_stems_are_the_channels_sources_before_the_strip() {
    let doc = clicks();
    let dry = stems(&doc, &request(&doc, StemStrip::Dry), &options(512, 0));
    // Only the two channels have a source: the master has no dry signal.
    assert_eq!(dry.len(), 2);
    assert!(dry.iter().all(|s| s.source.kind == StemKind::Channel));
    let full = stems(&doc, &request(&doc, StemStrip::Full), &options(512, 0));
    // The strips pan the clicks hard left and right, so the dry stems are not the full ones.
    assert_ne!(bits(&dry)[0], bits(&full)[0]);
    assert!(onset(&dry[0].audio.channels) > 0);
}

#[test]
fn the_files_are_numbered_in_console_order() {
    let doc = clicks();
    let dir = TempDir::new("files");
    let folder = dir.path(&stems_folder_name("my / song"));
    assert!(
        folder
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("Bounced my - song")
    );
    let format = ExportFormat::new(BitDepth::Pcm24);
    let report = export_stems(
        &doc,
        &default_registry(),
        (&options(512, 0), None),
        &mut PluginBounce::default(),
        (&request(&doc, StemStrip::Full), &format),
        (&folder, RenderControl::default()),
    )
    .unwrap()
    .unwrap();
    let names: Vec<String> = report
        .files
        .iter()
        .map(|(p, _)| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names.len(), 3);
    assert!(
        names[0].starts_with("01 ") && names[0].ends_with(".wav"),
        "{names:?}"
    );
    assert!(names[2].starts_with("03 Master"), "{names:?}");
    for (path, _) in &report.files {
        let wav = atomik_audio_file::decode_file(path).unwrap();
        assert_eq!(wav.frames() as u64, report.frames);
        assert_eq!(wav.channels.len(), 2);
    }
    // No temporary file is left.
    let left: Vec<_> = std::fs::read_dir(&folder).unwrap().collect();
    assert_eq!(left.len(), 3);
}

#[test]
fn a_stopped_export_leaves_nothing_and_a_dry_request_without_sources_is_refused() {
    let doc = clicks();
    let dir = TempDir::new("stop");
    let folder = dir.path("Bounced song");
    let cancel = AtomicBool::new(true);
    let control = RenderControl {
        cancel: Some(&cancel),
        progress: None,
    };
    let done = export_stems(
        &doc,
        &default_registry(),
        (&options(512, 0), None),
        &mut PluginBounce::default(),
        (
            &request(&doc, StemStrip::Full),
            &ExportFormat::new(BitDepth::Pcm16),
        ),
        (&folder, control),
    )
    .unwrap();
    assert!(done.is_none());
    assert!(!folder.exists(), "the folder it made is gone");
    let none = StemRequest {
        sources: vec![StemSource {
            dry: None,
            ..request(&doc, StemStrip::Full).sources[2].clone()
        }],
        ..request(&doc, StemStrip::Dry)
    };
    let refused = export_stems(
        &doc,
        &default_registry(),
        (&options(512, 0), None),
        &mut PluginBounce::default(),
        (&none, &ExportFormat::new(BitDepth::Pcm16)),
        (&folder, RenderControl::default()),
    );
    assert!(refused.is_err());
    assert!(!folder.exists());
}

#[test]
fn an_aiff_file_holds_the_codes_a_wav_file_does() {
    let doc = clicks();
    let export = render_export(
        &doc,
        &default_registry(),
        (&options(512, 0), None),
        &mut PluginBounce::default(),
        &ExportRequest {
            span: ExportSpan::Song,
            tail_seconds: 0.0,
        },
        RenderControl::default(),
    )
    .unwrap()
    .unwrap();
    let dir = TempDir::new("aiff");
    for depth in [BitDepth::Pcm16, BitDepth::Pcm24, BitDepth::Float32] {
        let wav = ExportFormat::new(depth);
        let aiff = ExportFormat {
            container: ExportContainer::Aiff,
            ..wav
        };
        let (a, b) = (dir.path("a.wav"), dir.path("b.aif"));
        write_export(&a, &export, &wav).unwrap();
        write_export(&b, &export, &aiff).unwrap();
        let (x, y) = (
            atomik_audio_file::decode_file(&a).unwrap(),
            atomik_audio_file::decode_file(&b).unwrap(),
        );
        assert_eq!(y.sample_rate, 48_000, "{depth:?}");
        assert_eq!(x.channels, y.channels, "{depth:?}");
        if depth == BitDepth::Float32 {
            assert_eq!(y.channels, export.audio.channels, "float is untouched");
        }
    }
    // Truncated AIFF files error instead of panicking.
    let bytes = std::fs::read(dir.path("b.aif")).unwrap();
    for cut in [0, 4, 12, 40, bytes.len() / 2] {
        let _ = atomik_audio_file::decode_bytes(&bytes[..cut], Some("aif"));
    }
}
