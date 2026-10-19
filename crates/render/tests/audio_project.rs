//! Bounces of projects with audio tracks.
//!
//! The assets are procedural (no files in the repository): each test builds a bundle in a
//! temporary folder, a document with an audio track over it, and bounces it with
//! [`render_doc_in`].
//!
//! The console between the track and the output (channel strip, master) is not the subject
//! here, so the expectations are relative to a **calibration bounce**: a unity clip of a
//! constant asset gives the console's gain `c` at the bounce output; gain, offset and fades
//! must then show up exactly as the model's `flatten_audio` regions say (a ratio against `c`).

use atomik_audio_file::{AudioData, Bundle};
use atomik_core::PPQ;
use atomik_model::{
    AssetMeta, Command, DeviceKey, Document, FadeCurve, RackDoc, SongCommand, TempoEvent,
    empty_doc, save_project,
};
use atomik_render::project::{ProjectOptions, render_doc_in, render_project_file};
use atomik_render::{Rendered, default_registry};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "atomik-render-audio-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn options(block: usize, workers: u16) -> ProjectOptions {
    ProjectOptions {
        host_block: NonZeroUsize::new(block).unwrap(),
        workers,
        loop_count: None,
    }
}

fn bits(r: &Rendered) -> Vec<u32> {
    r.channels.iter().flatten().map(|x| x.to_bits()).collect()
}

/// One clip to place: `(start tick, length ticks, source offset, gain dB, fade in, fade out)`.
type ClipSpec = (i64, i64, u64, f32, u64, u64);

/// A bundle holding `asset`, and a document with one audio track playing it as `clips`.
fn project(dir: &Path, asset: &AudioData, clips: &[ClipSpec]) -> (RackDoc, Bundle) {
    let mut bundle = Bundle::create(dir).unwrap();
    let info = bundle.add_recording(asset, "fixture.wav").unwrap();
    let registry = default_registry();
    let mut d = Document::new(empty_doc());
    let mut apply = |c: Command| {
        d.apply(c, &registry).unwrap();
    };
    apply(Command::RegisterAsset {
        hash: info.hash.clone(),
        meta: AssetMeta {
            channels: info.channels,
            sample_rate: info.sample_rate,
            frames: info.frames,
            original_name: info.original_name.clone(),
        },
    });
    apply(Command::Song(SongCommand::AddAudioTrack { name: None }));
    apply(Command::Song(SongCommand::SetSongEnd(6 * PPQ)));
    for &(start, length, source_offset, gain_db, fade_in, fade_out) in clips {
        apply(Command::Song(SongCommand::AddAudioClip {
            device: DeviceKey("audio-track".into()),
            start,
            length,
            asset: info.hash.clone(),
            source_offset,
            gain_db,
            fade_in,
            fade_out,
            fade_curve: FadeCurve::Linear,
        }));
    }
    (d.doc().clone(), bundle)
}

fn constant(value: f32, frames: usize) -> AudioData {
    AudioData::new(48_000, vec![vec![value; frames], vec![value; frames]]).unwrap()
}

fn render(doc: &RackDoc, bundle: &Bundle, block: usize, workers: u16) -> Rendered {
    render_doc_in(
        doc,
        &default_registry(),
        &options(block, workers),
        Some(bundle.root()),
    )
    .unwrap()
}

/// The console's gain at the bounce output (pan centre, fader at unity), measured from a
/// unity clip of a constant 0.25 asset.
fn console_gain() -> f32 {
    let t = TempDir::new("calibration");
    let (doc, bundle) = project(
        &t.0.join("song"),
        &constant(0.25, 48_000),
        &[(0, 2 * PPQ, 0, 0.0, 0, 0)],
    );
    let r = render(&doc, &bundle, 512, 0);
    r.channels[0][20_000] / 0.25
}

#[test]
fn a_unity_clip_plays_its_asset_through_the_console_and_nothing_else() {
    let t = TempDir::new("unity");
    let (doc, bundle) = project(
        &t.0.join("song"),
        &constant(0.25, 48_000),
        &[(PPQ, 2 * PPQ, 0, 0.0, 0, 0)],
    );
    // The track's input cables from the interface resolve (silent inputs in a bounce).
    assert!(doc.input_channels >= 1);
    assert!(
        doc.cables
            .iter()
            .any(|c| c.from.device.as_str() == "interface" && c.from.socket == "in_1")
    );
    let r = render(&doc, &bundle, 512, 0);
    assert_eq!(r.channels.len(), 2);
    let c = console_gain();
    assert!((c - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6, "{c}");
    // One beat at 120 BPM and 48 kHz is 24 000 frames: the clip sits at 24 000..72 000.
    for ch in &r.channels {
        assert!(ch[..24_000].iter().all(|x| *x == 0.0), "silence before");
        assert!(
            ch[24_000..72_000]
                .iter()
                .all(|x| (x - 0.25 * c).abs() < 1e-6)
        );
        assert!(ch[72_000..].iter().all(|x| *x == 0.0), "silence after");
    }
}

#[test]
fn gain_offset_and_fades_follow_the_model_regions() {
    let t = TempDir::new("shape");
    let ramp: Vec<f32> = (0..48_000).map(|i| i as f32 / 48_000.0).collect();
    let asset = AudioData::new(48_000, vec![ramp.clone(), ramp.clone()]).unwrap();
    let gain_db = -6.0;
    let clips = [
        // Starts 12 000 frames into the ramp, at -6 dB.
        (0, 2 * PPQ, 12_000, gain_db, 0, 0),
        // Whole ramp with a 4 800-frame fade in and a 9 600-frame fade out.
        (4 * PPQ, 2 * PPQ, 0, 0.0, 4_800, 9_600),
    ];
    let (doc, bundle) = project(&t.0.join("song"), &asset, &clips);
    let r = render(&doc, &bundle, 512, 0);
    let c = f64::from(console_gain());
    let g = f64::from(atomik_core::db_to_gain(gain_db));
    let expect = |value: f64| (c * value) as f32;
    for ch in &r.channels {
        // Clip 1: offset and gain; it plays only 36 000 frames (the asset ends).
        for t in [0usize, 1, 999, 20_000, 35_999] {
            let want = expect(g * f64::from(ramp[12_000 + t]));
            assert!(
                (ch[t] - want).abs() < 2e-6,
                "clip 1 at {t}: {} vs {want}",
                ch[t]
            );
        }
        assert!(
            ch[36_000..96_000].iter().all(|x| *x == 0.0),
            "asset end, gap"
        );
        // Clip 2 starts at frame 96 000. Fade in: n / 4 800. Fade out: (48 000 - n) / 9 600.
        let fade = |n: usize| -> f64 {
            let mut f = 1.0;
            if n < 4_800 {
                f *= n as f64 / 4_800.0;
            }
            if n >= 48_000 - 9_600 {
                f *= (48_000 - n) as f64 / 9_600.0;
            }
            f
        };
        for n in [
            0usize, 1, 2_400, 4_799, 4_800, 20_000, 38_400, 43_000, 47_999,
        ] {
            let want = expect(fade(n) * f64::from(ramp[n]));
            let got = ch[96_000 + n];
            assert!((got - want).abs() < 2e-6, "clip 2 at {n}: {got} vs {want}");
        }
        assert!(ch[144_000..].iter().all(|x| *x == 0.0));
    }
}

#[test]
fn audio_keeps_its_length_when_the_tempo_changes_but_the_clip_start_follows_the_map() {
    let t = TempDir::new("tempo");
    let (doc, bundle) = project(
        &t.0.join("song"),
        &constant(0.25, 48_000),
        // Two beats at the second tempo (60 BPM) are 2 s: room for the whole asset.
        &[(6 * PPQ, 2 * PPQ, 0, 0.0, 0, 0)],
    );
    let mut d = Document::new(doc);
    d.apply(
        Command::Song(SongCommand::SetTempoMap(vec![
            TempoEvent {
                tick: 0,
                bpm: 120.0,
            },
            TempoEvent {
                tick: 4 * PPQ,
                bpm: 60.0,
            },
        ])),
        &default_registry(),
    )
    .unwrap();
    d.apply(
        Command::Song(SongCommand::SetSongEnd(9 * PPQ)),
        &default_registry(),
    )
    .unwrap();
    let r = render(d.doc(), &bundle, 512, 0);
    let c = console_gain();
    // 4 beats at 120 BPM (2 s) + 2 beats at 60 BPM (2 s) = frame 192 000.
    let ch = &r.channels[0];
    assert!(ch[..192_000].iter().all(|x| *x == 0.0));
    assert!(
        ch[192_000..240_000]
            .iter()
            .all(|x| (x - 0.25 * c).abs() < 1e-6)
    );
    assert!(ch[240_000..].iter().all(|x| *x == 0.0), "not stretched");
}

/// HI-2: host blocks {1, 64, 4096} x workers {0, 4} give identical bytes.
#[test]
fn audio_bounces_are_bit_identical_across_blocks_and_workers() {
    let t = TempDir::new("identical");
    let tone: Vec<f32> = (0..48_000).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
    let inverted: Vec<f32> = tone.iter().map(|x| -x).collect();
    let asset = AudioData::new(48_000, vec![tone, inverted]).unwrap();
    let clips = [
        (0, 2 * PPQ, 3_000, -3.0, 240, 2_400),
        (PPQ, 2 * PPQ, 0, 0.0, 4_800, 4_800),
        (4 * PPQ, PPQ, 100, 1.5, 0, 0),
    ];
    let (doc, bundle) = project(&t.0.join("song"), &asset, &clips);
    let reference = bits(&render(&doc, &bundle, 64, 0));
    assert!(
        reference.iter().any(|b| f32::from_bits(*b) != 0.0),
        "the bounce is not silent"
    );
    for block in [1, 64, 4096] {
        for workers in [0, 4] {
            assert!(
                bits(&render(&doc, &bundle, block, workers)) == reference,
                "block {block}, workers {workers}"
            );
        }
    }
}

#[test]
fn a_project_file_bounces_with_its_bundle_and_a_missing_asset_is_named() {
    let t = TempDir::new("file");
    let (doc, bundle) = project(
        &t.0.join("song"),
        &constant(0.25, 48_000),
        &[(0, 2 * PPQ, 0, 0.0, 0, 0)],
    );
    let file = t.0.join("song").join("song.atomik");
    std::fs::write(&file, save_project(&doc)).unwrap();
    let registry = default_registry();
    let by_file = render_project_file(&file, &registry, &options(512, 0)).unwrap();
    let by_folder = render_project_file(&t.0.join("song"), &registry, &options(512, 0)).unwrap();
    let direct = render(&doc, &bundle, 512, 0);
    assert!(bits(&by_file) == bits(&direct));
    assert!(bits(&by_folder) == bits(&direct));
    // The flat layout (project next to its bundle folder) works as well.
    let flat = t.0.join("flat.atomik");
    std::fs::write(&flat, save_project(&doc)).unwrap();
    std::fs::rename(t.0.join("song"), t.0.join("flat")).unwrap();
    let by_flat = render_project_file(&flat, &registry, &options(512, 0)).unwrap();
    assert!(bits(&by_flat) == bits(&direct));
    // A missing asset fails and names the asset, in the library and on the command line.
    let audio = t.0.join("flat").join("Audio");
    for entry in std::fs::read_dir(&audio).unwrap().flatten() {
        if entry.path().extension().is_some_and(|e| e == "wav") {
            std::fs::remove_file(entry.path()).unwrap();
        }
    }
    let e = render_project_file(&flat, &registry, &options(512, 0)).unwrap_err();
    assert!(e.to_string().contains("fixture.wav"), "{e}");
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let wav = t.0.join("out.wav");
    let code = atomik_render::cli::main_with(
        [
            std::ffi::OsString::from("atomik-render"),
            flat.clone().into_os_string(),
            std::ffi::OsString::from("-o"),
            wav.clone().into_os_string(),
        ],
        &mut out,
        &mut err,
    );
    assert_eq!(code, atomik_render::cli::EXIT_FAILURE);
    let err = String::from_utf8_lossy(&err);
    assert!(err.contains("fixture.wav"), "{err}");
    assert!(!wav.exists(), "no partial output");
}
