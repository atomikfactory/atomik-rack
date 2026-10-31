//! The `audio-demo` example project and its bundle, and every example project.
//!
//! `examples/projects/audio-demo.atomik` sits next to its bundle folder `audio-demo/`, whose
//! `Audio/` holds one **procedurally generated** mono asset ([`demo_asset`]: a one-bar drum
//! and bass loop at 120 BPM plus a short crash, no third-party audio). Regenerate both after
//! an intentional change with:
//!
//! ```text
//! ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-render --test audio_demo regenerate
//! ```
//!
//! The generator uses only IEEE `+ - * /` and comparisons (no libm calls), so it gives the same
//! samples on every platform; the committed WAV is checked against it bit for bit.

use atomik_audio_file::{AudioData, Bundle, content_hash};
use atomik_model::{AssetMeta, RackDoc, load_project, save_project};
use atomik_render::bundle::{existing_bundle_root, missing_assets};
use atomik_render::default_registry;
use atomik_render::project::{ProjectOptions, render_doc_in, render_project_file};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

/// Frames of the demo asset: one 120 BPM bar (96 000) plus a 4 000-frame crash.
const DEMO_FRAMES: usize = 100_000;
/// Largest accepted size of the committed asset (the brief's budget).
const MAX_ASSET_BYTES: u64 = 400 * 1024;
const UPDATE_ENV: &str = "ATOMIK_UPDATE_GOLDEN";

fn projects_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/projects")
}

fn demo_file() -> PathBuf {
    projects_dir().join("audio-demo.atomik")
}

/// A sine-shaped wave of phase `x` in `[0, 1)` from two parabolas (peak error about 6 %,
/// which is fine for a drum loop and uses no libm call).
fn wave(x: f32) -> f32 {
    if x < 0.5 {
        16.0 * x * (0.5 - x)
    } else {
        -16.0 * (x - 0.5) * (1.0 - x)
    }
}

/// xorshift32 noise in `[-1, 1)`.
struct Noise(u32);

impl Noise {
    fn next(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        (x >> 8) as f32 / 8_388_608.0 - 1.0
    }
}

/// The demo asset: kicks on beats 1 and 3, closed hats on the off-beats, a four-note bass
/// line and a crash in the last 4 000 frames (after the bar), 48 kHz mono, peak below 0.9.
pub fn demo_asset() -> AudioData {
    const RATE: f32 = 48_000.0;
    const BEAT: usize = 24_000;
    let mut out = vec![0.0f32; DEMO_FRAMES];
    let mut noise = Noise(0x2545_f491);
    // Kicks: a sine-shaped sweep from 120 Hz down, with an exponential-like decay.
    for start in [0, 2 * BEAT] {
        let (mut phase, mut freq, mut amp) = (0.0f32, 120.0f32, 0.85f32);
        for s in out.iter_mut().skip(start).take(9_000) {
            *s += wave(phase) * amp;
            phase += freq / RATE;
            phase -= if phase >= 1.0 { 1.0 } else { 0.0 };
            freq = 45.0 + (freq - 45.0) * 0.9992;
            amp *= 0.9996;
        }
    }
    // Hats: differentiated noise with a fast decay.
    for k in 0..4 {
        let start = k * BEAT + BEAT / 2;
        let (mut amp, mut last) = (0.22f32, 0.0f32);
        for s in out.iter_mut().skip(start).take(3_000) {
            let n = noise.next();
            *s += (n - last) * 0.5 * amp;
            last = n;
            amp *= 0.998;
        }
    }
    // Bass: a saw through a one-pole low-pass, one note per beat.
    for (k, freq) in [55.0f32, 55.0, 65.4, 49.0].into_iter().enumerate() {
        let (mut phase, mut amp, mut low) = (0.0f32, 0.35f32, 0.0f32);
        for s in out.iter_mut().skip(k * BEAT).take(BEAT + 2_000) {
            low += (phase * 2.0 - 1.0 - low) * 0.06;
            *s += low * amp;
            phase += freq / RATE;
            phase -= if phase >= 1.0 { 1.0 } else { 0.0 };
            amp *= 0.99992;
        }
    }
    // Crash after the bar, fading out to silence at the very end.
    let mut amp = 0.4f32;
    for (i, s) in out.iter_mut().enumerate().skip(2 * 48_000) {
        let left = (DEMO_FRAMES - i) as f32;
        let fade = if left < 200.0 {
            (left - 1.0) / 200.0
        } else {
            1.0
        };
        *s += noise.next() * amp * fade;
        amp *= 0.9993;
    }
    AudioData::new(48_000, vec![out]).expect("valid mono data")
}

/// The project document of the committed file with its placeholder asset swapped for `info`.
fn with_asset(mut doc: RackDoc, hash: &str, meta: AssetMeta) -> RackDoc {
    let old: Vec<String> = doc.assets.keys().cloned().collect();
    assert_eq!(old.len(), 1, "the example has one asset");
    doc.assets.clear();
    doc.assets.insert(hash.to_owned(), meta);
    for track in &mut doc.song_mut().tracks {
        for lane in &mut track.lanes {
            for clip in &mut lane.clips {
                if let atomik_model::ClipContent::Audio(a) = &mut clip.content {
                    a.asset = hash.to_owned();
                }
            }
        }
    }
    doc
}

#[test]
fn regenerate() {
    if std::env::var_os(UPDATE_ENV).is_none() {
        return;
    }
    let registry = default_registry();
    let text = std::fs::read_to_string(demo_file()).expect("the example project");
    let doc = load_project(&text, &registry).expect("a valid project");
    let root = projects_dir().join("audio-demo");
    let mut bundle = Bundle::create(&root).expect("bundle folder");
    let info = bundle
        .add_recording(&demo_asset(), "audio-demo.wav")
        .expect("asset written");
    // Only the asset belongs in the repository; the peak cache is rebuilt on demand.
    let _ = std::fs::remove_dir_all(bundle.peaks_dir());
    let doc = with_asset(
        doc,
        &info.hash,
        AssetMeta {
            channels: info.channels,
            sample_rate: info.sample_rate,
            frames: info.frames,
            original_name: info.original_name.clone(),
        },
    );
    // Old generations of the asset do not stay behind.
    let keep = doc.referenced_assets();
    bundle.remove_orphans(&keep).expect("orphans removed");
    std::fs::write(demo_file(), save_project(&doc)).expect("project written");
    eprintln!("audio-demo asset: {} ({} frames)", info.hash, info.frames);
}

#[test]
fn the_demo_generator_is_deterministic_and_within_budget() {
    let a = demo_asset();
    assert!(a == demo_asset());
    assert_eq!(
        (a.frames(), a.channel_count(), a.sample_rate),
        (100_000, 1, 48_000)
    );
    let peak = a.channels[0].iter().fold(0.0f32, |m, x| m.max(x.abs()));
    assert!((0.5..0.95).contains(&peak), "peak {peak}");
    assert!(a.channels[0].iter().all(|x| x.is_finite()));
    assert_eq!(*a.channels[0].last().unwrap(), 0.0, "ends silent");
    // 4 bytes a frame plus a small header.
    assert!((a.frames() as u64) * 4 + 44 <= MAX_ASSET_BYTES);
}

#[test]
fn the_committed_demo_bundle_is_intact_and_its_project_canonical() {
    let registry = default_registry();
    let text = std::fs::read_to_string(demo_file()).expect("the example project");
    let doc = load_project(&text, &registry).expect("a valid project");
    assert_eq!(save_project(&doc), text.replace("\r\n", "\n"), "canonical");
    let root = existing_bundle_root(&demo_file()).expect("the bundle folder sits next to it");
    assert_eq!(root, projects_dir().join("audio-demo"));
    let bundle = Bundle::open(&root).expect("bundle");
    assert!(missing_assets(&doc, Some(&bundle)).is_empty());
    assert_eq!(doc.assets.len(), 1);
    let (hash, meta) = doc.assets.iter().next().unwrap();
    assert_eq!(meta.frames, DEMO_FRAMES as u64);
    assert_eq!((meta.channels, meta.sample_rate), (1, 48_000));
    let stored = bundle.load(hash).expect("the asset decodes");
    assert_eq!(&content_hash(&stored), hash, "named by its content hash");
    assert!(stored == demo_asset(), "the file is the generator's output");
    let size = std::fs::metadata(bundle.asset_path(hash)).unwrap().len();
    assert!(size <= MAX_ASSET_BYTES, "{size} bytes");
}

fn options(block: usize) -> ProjectOptions {
    ProjectOptions {
        host_block: NonZeroUsize::new(block).unwrap(),
        workers: 0,
        loop_count: None,
    }
}

/// Every project in `examples/projects` loads and bounces (audio-demo with its bundle).
#[test]
fn every_example_project_bounces() {
    let registry = default_registry();
    let mut names = Vec::new();
    for entry in std::fs::read_dir(projects_dir()).unwrap().flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "atomik") {
            let r = render_project_file(&path, &registry, &options(512))
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            let peak = r
                .channels
                .iter()
                .flatten()
                .fold(0.0f32, |m, x| m.max(x.abs()));
            assert!(peak > 1e-3, "{} is silent", path.display());
            names.push(path.file_name().unwrap().to_string_lossy().into_owned());
        }
    }
    names.sort();
    assert!(names.contains(&"audio-demo.atomik".to_owned()), "{names:?}");
    assert!(names.len() >= 3, "{names:?}");
}

/// The audio-demo bounce, with only the audio track audible (solo), plays its three clips
/// where the song says.
#[test]
fn the_audio_demo_plays_its_clips_where_the_song_says() {
    let registry = default_registry();
    let text = std::fs::read_to_string(demo_file()).unwrap();
    let mut doc = load_project(&text, &registry).unwrap();
    let audio = doc
        .song_mut()
        .tracks
        .iter_mut()
        .find(|t| t.is_audio())
        .expect("an audio track");
    audio.solo = true;
    let root = existing_bundle_root(&demo_file());
    let r = render_doc_in(&doc, &registry, &options(512), root.as_deref()).unwrap();
    let rms = |from: usize, to: usize| -> f64 {
        let w = &r.channels[0][from..to];
        (w.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / w.len() as f64).sqrt()
    };
    // Clip 1 starts the song with the loop and ends where the asset does (frame 100 000);
    // clip 2 starts at bar 5 (frame 384 000 at 120 BPM) at -3 dB with fades; clip 3 at bar 7
    // (frame 576 000) plays the crash, asset frames 96 000 to the end.
    assert!(rms(0, 24_000) > 1e-2, "the loop's first beat");
    assert!(
        rms(100_000, 380_000) < 1e-9,
        "silence after the asset, before clip 2"
    );
    assert!(rms(384_000 + 480, 384_000 + 24_000) > 1e-2, "clip 2");
    assert!(
        rms(576_000, 576_000 + 2_000) > 1e-3,
        "clip 3 plays the crash"
    );
    assert!(rms(580_000, 700_000) < 1e-9, "silence after clip 3");
}
