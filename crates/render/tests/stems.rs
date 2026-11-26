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

#[test]
fn flac_stems_hold_the_codes_of_the_wav_ones_and_report_their_loudness() {
    let doc = clicks();
    let dir = TempDir::new("flac");
    let mut written = Vec::new();
    for container in [ExportContainer::Wav, ExportContainer::Flac] {
        let folder = dir.path(&format!("{container:?}"));
        let format = ExportFormat {
            container,
            ..ExportFormat::new(BitDepth::Pcm16)
        };
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
        assert_eq!(report.loudness.len(), report.files.len());
        written.push(report);
    }
    let (wav, flac) = (&written[0], &written[1]);
    assert_eq!(wav.loudness, flac.loudness);
    for ((a, _), (b, _)) in wav.files.iter().zip(&flac.files) {
        assert_eq!(b.extension().unwrap(), "flac");
        let (x, y) = (
            atomik_audio_file::decode_file(a).unwrap(),
            atomik_audio_file::decode_file(b).unwrap(),
        );
        assert_eq!(x.channels, y.channels);
    }
    // The master is audible: it has a peak.
    assert!(flac.loudness[2].true_peak_dbtp.is_some());
}

#[test]
fn stems_of_a_range_are_the_same_frames_of_the_whole_song_stems() {
    let mut doc = clicks();
    let whole = stems(&doc, &request(&doc, StemStrip::Full), &options(512, 0));
    let total = whole[0].audio.frames();
    // Take the second half of the song by its locators.
    let end = doc.song.end_tick();
    doc.song_mut().loop_range = (end / 2, end);
    let from = doc.song.frame_at(end / 2, doc.sample_rate) as usize;
    let to = doc.song.frame_at(end, doc.sample_rate) as usize;
    let part = stems(
        &doc,
        &StemRequest {
            span: ExportSpan::Loop,
            ..request(&doc, StemStrip::Full)
        },
        &options(512, 0),
    );
    assert!(to <= total && from < to);
    for (p, w) in part.iter().zip(&whole) {
        assert_eq!(p.audio.frames(), to - from, "{}", p.source.name);
        assert_eq!(
            p.audio.channels[0],
            w.audio.channels[0][from..to],
            "{}",
            p.source.name
        );
    }
}

#[test]
fn stems_of_a_time_selection_are_the_same_frames_whatever_the_locators_say() {
    let mut doc = clicks();
    let whole = stems(&doc, &request(&doc, StemStrip::Full), &options(512, 0));
    let end = doc.song.end_tick();
    // The locators are somewhere else: the selection decides.
    doc.song_mut().loop_range = (0, end / 4);
    let (start, stop) = (end / 3, end / 2);
    let from = doc.song.frame_at(start, doc.sample_rate) as usize;
    let to = doc.song.frame_at(stop, doc.sample_rate) as usize;
    let part = stems(
        &doc,
        &StemRequest {
            span: ExportSpan::Range(start, stop),
            ..request(&doc, StemStrip::Full)
        },
        &options(512, 0),
    );
    assert!(from < to && to <= whole[0].audio.frames());
    for (p, w) in part.iter().zip(&whole) {
        assert_eq!(p.audio.frames(), to - from, "{}", p.source.name);
        assert_eq!(
            p.audio.channels[0],
            w.audio.channels[0][from..to],
            "{}",
            p.source.name
        );
    }
}

// ---------------------------------------------------------------------------------------------
// The level of the stems: one gain from the mix, or one each.

use atomik_audio_file::AudioData;
use atomik_render::export::ExportRender;
use atomik_render::loudness::measure;
use atomik_render::normalise::Loudness;
use atomik_render::stems::{StemsLevel, StemsOutput, StemsReport, export_stems_with};

fn float() -> ExportFormat {
    ExportFormat::new(BitDepth::Float32)
}

/// Exports the stems of `strip` to `folder` as float WAV files with `level`.
fn leveled(doc: &RackDoc, strip: StemStrip, level: StemsLevel, folder: &Path) -> StemsReport {
    let format = float();
    let output = StemsOutput {
        folder,
        extension: "wav",
        level,
    };
    export_stems_with(
        (doc, &default_registry()),
        (&options(512, 0), None),
        &mut PluginBounce::default(),
        &request(doc, strip),
        (&output, RenderControl::default()),
        &mut |path, _, render| write_export(path, render, &format).map(Some),
    )
    .unwrap()
    .unwrap()
}

fn decoded(report: &StemsReport) -> Vec<AudioData> {
    report
        .files
        .iter()
        .map(|(p, _)| atomik_audio_file::decode_file(p).unwrap())
        .collect()
}

/// `a` is `b` times the gain `db`, sample by sample, within float rounding.
fn is_gain_of(a: &AudioData, b: &AudioData, db: f32) -> bool {
    let g = 10f32.powf(db / 20.0);
    a.channels
        .iter()
        .zip(&b.channels)
        .all(|(x, y)| x.len() == y.len() && x.iter().zip(y).all(|(p, q)| (p - q * g).abs() <= 3e-5))
}

/// The largest difference between `x + y` and `z`.
fn sum_gap(x: &AudioData, y: &AudioData, z: &AudioData) -> f32 {
    (0..2)
        .flat_map(|c| {
            (0..z.channels[c].len())
                .map(move |i| (x.channels[c][i] + y.channels[c][i] - z.channels[c][i]).abs())
        })
        .fold(0.0, f32::max)
}

fn target(lufs: f32) -> Loudness {
    // No ceiling in the way of the levels these tests ask for.
    Loudness::Target {
        lufs,
        ceiling_dbtp: 20.0,
    }
}

#[test]
fn with_nothing_asked_the_stems_are_the_bytes_render_stems_and_write_export_make() {
    let doc = clicks();
    let dir = TempDir::new("identical");
    let names = atomik_model::stem_file_names(&request(&doc, StemStrip::Full).sources);
    let reference = stems(&doc, &request(&doc, StemStrip::Full), &options(512, 0));
    for (depth, tag) in [(BitDepth::Pcm16, "16"), (BitDepth::Pcm24, "24")] {
        let format = ExportFormat::new(depth);
        let folder = dir.path(&format!("new{tag}"));
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
        assert!(report.common.is_none() && report.normalise.iter().all(Option::is_none));
        for stem in &reference {
            let by_hand = dir.path(&format!("hand{tag}-{}.wav", stem.index));
            let render = ExportRender {
                audio: stem.audio.clone(),
                span_frames: 0,
                tail_frames: 0,
                warnings: Vec::new(),
            };
            write_export(&by_hand, &render, &format).unwrap();
            let written = folder.join(format!("{}.wav", names[stem.index]));
            assert_eq!(
                std::fs::read(&written).unwrap(),
                std::fs::read(&by_hand).unwrap(),
                "{} at {tag} bits",
                names[stem.index]
            );
        }
    }
}

#[test]
fn taken_together_every_stem_gets_the_gain_of_the_master_and_still_adds_up_to_it() {
    let doc = clicks();
    let dir = TempDir::new("together");
    let plain = leveled(&doc, StemStrip::Full, StemsLevel::default(), &dir.path("p"));
    let master_lufs = plain.loudness[2].integrated_lufs.expect("audible");
    let level = StemsLevel {
        loudness: target(master_lufs - 6.0),
        together: true,
    };
    let together = leveled(&doc, StemStrip::Full, level, &dir.path("t"));
    let common = together.common.expect("one gain");
    assert!(!common.guide && common.held_db == 0.0);
    assert!((common.gain_db + 6.0).abs() < 0.01, "{common:?}");
    assert_eq!(common.before, plain.loudness[2], "the mix is the master");
    // The files stay in the order of the request, whatever order they were rendered in.
    let names: Vec<String> = together
        .files
        .iter()
        .map(|(p, _)| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert!(
        names[0].starts_with("01 ") && names[2].starts_with("03 Master"),
        "{names:?}"
    );
    let (before, after) = (decoded(&plain), decoded(&together));
    for (a, b) in after.iter().zip(&before) {
        assert!(is_gain_of(a, b, common.gain_db));
    }
    // The two channels still add up to the master.
    assert!(sum_gap(&after[0], &after[1], &after[2]) < 1e-4);
    // Each stem's report has its level before and after, and the same gain.
    for (n, (was, now)) in together.normalise.iter().zip(&plain.loudness).enumerate() {
        let n = n_of(n, was.as_ref());
        assert_eq!(n.before, *now);
        assert!((n.gain_db - common.gain_db).abs() < 1e-6);
        assert!(!n.clipped);
    }
    // The same export twice is the same bits.
    let again = leveled(&doc, StemStrip::Full, level, &dir.path("t2"));
    assert_eq!(decoded(&again), after);
}

fn n_of(
    _at: usize,
    report: Option<&atomik_render::normalise::NormaliseReport>,
) -> atomik_render::normalise::NormaliseReport {
    *report.expect("a report for every stem")
}

#[test]
fn each_stem_is_brought_to_the_target_on_its_own_and_no_longer_adds_up() {
    let doc = clicks();
    let dir = TempDir::new("each");
    let plain = leveled(&doc, StemStrip::Full, StemsLevel::default(), &dir.path("p"));
    let master_lufs = plain.loudness[2].integrated_lufs.expect("audible");
    let level = StemsLevel {
        loudness: target(master_lufs - 6.0),
        together: false,
    };
    let each = leveled(&doc, StemStrip::Full, level, &dir.path("e"));
    assert!(each.common.is_none());
    for (i, l) in each.loudness.iter().enumerate() {
        let lufs = l.integrated_lufs.unwrap();
        assert!(
            (lufs - (master_lufs - 6.0)).abs() < 0.15,
            "stem {i}: {lufs}"
        );
    }
    // The click on one side alone is 3 LU quieter than the master, so it is turned up 3 LU
    // more than the master is: the stems do not add up to the master any more.
    let gains: Vec<f32> = each
        .normalise
        .iter()
        .map(|n| n_of(0, n.as_ref()).gain_db)
        .collect();
    assert!(
        gains[0] > gains[2] + 2.0 && gains[1] > gains[2] + 2.0,
        "{gains:?}"
    );
    let after = decoded(&each);
    assert!(sum_gap(&after[0], &after[1], &after[2]) > 1e-3);
    // The report says what each was before.
    for (n, p) in each.normalise.iter().zip(&plain.loudness) {
        assert_eq!(n_of(0, n.as_ref()).before, *p);
    }
}

#[test]
fn a_target_the_peaks_cannot_reach_holds_the_one_gain_at_the_ceiling() {
    let doc = clicks();
    let dir = TempDir::new("held");
    let plain = leveled(&doc, StemStrip::Full, StemsLevel::default(), &dir.path("p"));
    let master = plain.loudness[2];
    let level = StemsLevel {
        loudness: Loudness::Target {
            lufs: master.integrated_lufs.unwrap() + 20.0,
            ceiling_dbtp: -1.0,
        },
        together: true,
    };
    let held = leveled(&doc, StemStrip::Full, level, &dir.path("h"));
    let common = held.common.unwrap();
    assert!(common.held_db > 1.0, "{common:?}");
    let peak = held.loudness[2].true_peak_dbtp.unwrap();
    assert!(
        (peak + 1.0).abs() < 0.05,
        "the master ends on the ceiling: {peak}"
    );
    assert!(
        common.summary().contains("short of"),
        "{}",
        common.summary()
    );
    // Still one gain: no limiter, so the files are the plain ones times a number.
    let (before, after) = (decoded(&plain), decoded(&held));
    for (a, b) in after.iter().zip(&before) {
        assert!(is_gain_of(a, b, common.gain_db));
    }
}

#[test]
fn a_gain_that_pushes_a_stem_past_full_scale_is_flagged() {
    let doc = clicks();
    let dir = TempDir::new("clip");
    let plain = leveled(&doc, StemStrip::Full, StemsLevel::default(), &dir.path("p"));
    let level = StemsLevel {
        loudness: target(plain.loudness[2].integrated_lufs.unwrap() + 30.0),
        together: true,
    };
    let hot = leveled(&doc, StemStrip::Full, level, &dir.path("h"));
    assert!(hot.normalise.iter().flatten().any(|n| n.clipped), "{hot:?}");
}

#[test]
fn a_guide_measures_the_stems_and_changes_nothing() {
    let doc = clicks();
    let dir = TempDir::new("guide");
    let plain = leveled(&doc, StemStrip::Full, StemsLevel::default(), &dir.path("p"));
    for (together, name) in [(true, "t"), (false, "e")] {
        let level = StemsLevel {
            loudness: Loudness::Guide { lufs: -9.0 },
            together,
        };
        let guided = leveled(&doc, StemStrip::Full, level, &dir.path(name));
        assert_eq!(decoded(&guided), decoded(&plain), "together {together}");
        assert!(guided.normalise.iter().flatten().all(|n| n.guide));
        assert_eq!(guided.common.is_some(), together);
    }
}

#[test]
fn dry_stems_take_their_one_gain_from_the_sum_of_the_stems() {
    let doc = clicks();
    let dir = TempDir::new("dry");
    let plain = leveled(&doc, StemStrip::Dry, StemsLevel::default(), &dir.path("p"));
    assert!(plain.files.len() >= 2, "{:?}", plain.files);
    // The mix of a dry export is what the stems add up to.
    let mut sum: Option<AudioData> = None;
    for stem in decoded(&plain) {
        sum = Some(match sum {
            None => stem,
            Some(mut total) => {
                for (t, c) in total.channels.iter_mut().zip(&stem.channels) {
                    for (x, y) in t.iter_mut().zip(c) {
                        *x += y;
                    }
                }
                total
            }
        });
    }
    let mix = measure(&sum.unwrap());
    let level = StemsLevel {
        loudness: target(mix.integrated_lufs.expect("audible") - 6.0),
        together: true,
    };
    let together = leveled(&doc, StemStrip::Dry, level, &dir.path("t"));
    let common = together.common.unwrap();
    assert!((common.gain_db + 6.0).abs() < 0.01, "{common:?}");
    assert!(
        (common.before.integrated_lufs.unwrap() - mix.integrated_lufs.unwrap()).abs() < 0.01,
        "{:?} against {mix:?}",
        common.before
    );
    for (a, b) in decoded(&together).iter().zip(&decoded(&plain)) {
        assert!(is_gain_of(a, b, common.gain_db));
    }
}

#[test]
fn a_stopped_export_with_a_level_leaves_nothing_either_way() {
    let doc = clicks();
    let dir = TempDir::new("stopped");
    let cancel = AtomicBool::new(true);
    let control = RenderControl {
        cancel: Some(&cancel),
        progress: None,
    };
    for (strip, together) in [
        (StemStrip::Full, true),
        (StemStrip::Full, false),
        (StemStrip::Dry, true),
    ] {
        let folder = dir.path(&format!("{strip:?}{together}"));
        let format = float();
        let output = StemsOutput {
            folder: &folder,
            extension: "wav",
            level: StemsLevel {
                loudness: target(-14.0),
                together,
            },
        };
        let done = export_stems_with(
            (&doc, &default_registry()),
            (&options(512, 0), None),
            &mut PluginBounce::default(),
            &request(&doc, strip),
            (&output, control),
            &mut |path, _, render| write_export(path, render, &format).map(Some),
        )
        .unwrap();
        assert!(done.is_none());
        assert!(!folder.exists(), "{strip:?} together {together}");
    }
}

#[test]
fn a_writer_that_stops_ends_the_export_with_nothing_written() {
    let doc = clicks();
    let dir = TempDir::new("writer");
    let folder = dir.path("Bounced song");
    let output = StemsOutput {
        folder: &folder,
        extension: "wav",
        level: StemsLevel::default(),
    };
    let flag = AtomicBool::new(false);
    let control = RenderControl {
        cancel: Some(&flag),
        progress: None,
    };
    let mut seen = 0;
    let done = export_stems_with(
        (&doc, &default_registry()),
        (&options(512, 0), None),
        &mut PluginBounce::default(),
        &request(&doc, StemStrip::Full),
        (&output, control),
        &mut |path, _, render| {
            seen += 1;
            if seen == 2 {
                // The user pressed Cancel while the second stem was being written.
                flag.store(true, Ordering::Relaxed);
                return Ok(None);
            }
            write_export(path, render, &float()).map(Some)
        },
    )
    .unwrap();
    assert!(done.is_none());
    assert!(!folder.exists(), "the first stem and the folder are gone");
}
