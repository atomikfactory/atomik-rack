//! `.atomik` project bounces.
//!
//! # Goldens
//!
//! `tests/golden/demo-project.f32.sha256` (exact bytes, Windows x86-64 only) and
//! `tests/golden/demo-project.fingerprint.json` (per-half-beat RMS, all platforms) describe
//! `examples/projects/demo.atomik` bounced as 32-bit float with the default block size, no
//! loop. After an intentional change to the sound, regenerate both on Windows x86-64 with:
//!
//! ```text
//! ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-render --test project golden
//! ```
//!
//! then listen to the new render before committing the goldens.

use atomik_render::project::{ProjectOptions, render_project_json};
use atomik_render::{default_registry, wav};
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
use sha2::{Digest, Sha256};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_atomik-render");
/// demo.atomik: 8 bars of 4/4 at 120 BPM (16 s) plus 2 tail beats (1 s), 48 kHz.
const DEMO_FRAMES: usize = 17 * 48_000;
/// One pass of the demo loop (bars 1–9, the whole song).
const DEMO_LOOP_FRAMES: usize = 16 * 48_000;
const FINGERPRINT_WINDOW_FRAMES: usize = 12_000;
const FINGERPRINT_FLOOR_DB: f64 = -120.0;
const FINGERPRINT_TOLERANCE_DB: f64 = 0.5;
const UPDATE_ENV: &str = "ATOMIK_UPDATE_GOLDEN";

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "atomik-render-project-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
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

fn demo_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/projects/demo.atomik")
}

fn demo_text() -> String {
    std::fs::read_to_string(demo_path()).expect("demo project")
}

fn golden(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name)
}

fn render(project: &Path, out: &Path, extra: &[&str]) -> Output {
    Command::new(BIN)
        .arg(project)
        .arg("-o")
        .arg(out)
        .args(extra)
        .output()
        .expect("run atomik-render")
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "exit {:?}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn options(block: usize, workers: u16) -> ProjectOptions {
    ProjectOptions {
        host_block: NonZeroUsize::new(block).unwrap(),
        workers,
        loop_count: None,
    }
}

fn rms(samples: &[f32]) -> f64 {
    (samples.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / samples.len().max(1) as f64)
        .sqrt()
}

fn bits(channels: &[Vec<f32>]) -> Vec<u32> {
    channels.iter().flatten().map(|x| x.to_bits()).collect()
}

#[test]
fn the_cli_bounces_a_project_for_its_song_end_plus_two_beats() {
    let dir = TempDir::new("cli");
    let out = dir.path("demo.wav");
    let output = render(&demo_path(), &out, &["--stats"]);
    assert_success(&output);
    let wav = wav::read_wav_file(&out).unwrap();
    assert_eq!(wav.sample_rate, 48_000);
    assert_eq!(wav.channels.len(), 2);
    assert_eq!(wav.channels[0].len(), DEMO_FRAMES);
    // Every bar plays something (bass and lead tracks).
    for bar in 0..8 {
        let w = &wav.channels[0][bar * 96_000..(bar + 1) * 96_000];
        assert!(rms(w) > 1e-3, "bar {} is silent", bar + 1);
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("frames: 816000"), "{stdout}");
}

#[test]
fn the_library_and_the_cli_agree() {
    let dir = TempDir::new("agree");
    let out = dir.path("demo.wav");
    assert_success(&render(&demo_path(), &out, &[]));
    let wav = wav::read_wav_file(&out).unwrap();
    let lib = render_project_json(&demo_text(), &default_registry(), &options(512, 0)).unwrap();
    assert_eq!(lib.sample_rate, 48_000);
    assert!(bits(&lib.channels) == bits(&wav.channels));
}

/// HI-2: host blocks {1, 64, 4096} × workers {0, 4} give identical bytes.
#[test]
fn e3_3_project_bounces_are_bit_identical_across_blocks_and_workers() {
    let text = demo_text();
    let registry = default_registry();
    let reference = bits(
        &render_project_json(&text, &registry, &options(64, 0))
            .unwrap()
            .channels,
    );
    assert_eq!(reference.len(), 2 * DEMO_FRAMES);
    for block in [1, 64, 4096] {
        for workers in [0, 4] {
            let got = render_project_json(&text, &registry, &options(block, workers)).unwrap();
            assert!(
                bits(&got.channels) == reference,
                "block {block}, workers {workers} differ"
            );
        }
    }
}

#[test]
fn the_cli_accepts_workers_and_block_and_stays_identical() {
    let dir = TempDir::new("workers");
    let a = dir.path("a.wav");
    let b = dir.path("b.wav");
    assert_success(&render(&demo_path(), &a, &["--block", "4096"]));
    assert_success(&render(
        &demo_path(),
        &b,
        &["--block", "1", "--workers", "4"],
    ));
    assert!(std::fs::read(&a).unwrap() == std::fs::read(&b).unwrap());
}

#[test]
fn loop_count_plays_the_loop_n_times_then_the_tail() {
    let registry = default_registry();
    let text = demo_text();
    let once = render_project_json(&text, &registry, &options(512, 0)).unwrap();
    let twice = render_project_json(
        &text,
        &registry,
        &ProjectOptions {
            loop_count: Some(2),
            ..options(512, 0)
        },
    )
    .unwrap();
    assert_eq!(twice.channels[0].len(), DEMO_FRAMES + DEMO_LOOP_FRAMES);
    // The first pass is the plain bounce; the second pass plays again (not silence).
    let first = &twice.channels[0][..DEMO_LOOP_FRAMES];
    assert!(bits(&[first.to_vec()]) == bits(&[once.channels[0][..DEMO_LOOP_FRAMES].to_vec()]));
    let second = &twice.channels[0][DEMO_LOOP_FRAMES..2 * DEMO_LOOP_FRAMES];
    assert!(rms(second) > 0.5 * rms(first), "the second pass plays");
    // After the last pass the song stops: only the tail rings out, then silence would follow.
    let tail = &twice.channels[0][2 * DEMO_LOOP_FRAMES..];
    assert!(rms(tail) < rms(second));
}

#[test]
fn the_cli_accepts_loop_count() {
    let dir = TempDir::new("loop-count");
    let out = dir.path("loop.wav");
    assert_success(&render(&demo_path(), &out, &["--loop-count", "2"]));
    let wav = wav::read_wav_file(&out).unwrap();
    assert_eq!(wav.channels[0].len(), DEMO_FRAMES + DEMO_LOOP_FRAMES);
    let (code, bad) = {
        let o = render(&demo_path(), &out, &["--loop-count", "0"]);
        (
            o.status.code(),
            String::from_utf8_lossy(&o.stderr).into_owned(),
        )
    };
    assert_eq!(code, Some(2), "{bad}");
}

#[test]
fn an_invalid_project_exits_2_naming_the_json_path() {
    let dir = TempDir::new("invalid");
    let text = demo_text().replacen("[0, 11520, 45, 0.95]", "[0, 11520, 145, 0.95]", 1);
    let project = dir.path("bad.atomik");
    std::fs::write(&project, text).unwrap();
    let out = dir.path("o.wav");
    let output = render(&project, &out, &[]);
    assert_eq!(output.status.code(), Some(2));
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.starts_with("error: "), "{err}");
    assert!(
        err.contains("song.tracks[0].lanes[0].clips[0].notes[0]"),
        "{err}"
    );
    assert!(!out.exists());
}

#[test]
fn a_patch_still_renders_and_workers_are_reported_as_ignored() {
    let dir = TempDir::new("patch");
    let patch = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/patches/minimal.json");
    let out = dir.path("p.wav");
    let output = render(&patch, &out, &["--workers", "2"]);
    assert_success(&output);
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("--workers"), "{err}");
    assert_eq!(
        wav::read_wav_file(&out).unwrap().channels[0].len(),
        4 * 24_000
    );
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn updating() -> bool {
    std::env::var_os(UPDATE_ENV).is_some_and(|v| v == "1")
}

/// Exact bytes of the demo bounce (Windows x86-64 only, see `e2e.rs`).
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
#[test]
fn golden_hash_matches() {
    let dir = TempDir::new("golden-hash");
    let out = dir.path("demo.wav");
    assert_success(&render(&demo_path(), &out, &[]));
    let hash = sha256_hex(&std::fs::read(&out).unwrap());
    let path = golden("demo-project.f32.sha256");
    if updating() {
        std::fs::write(&path, format!("{hash}\n")).unwrap();
    }
    let expected = std::fs::read_to_string(&path).expect("golden hash file");
    assert_eq!(
        hash,
        expected.trim(),
        "demo.atomik bounce changed (see module docs)"
    );
}

fn fingerprint(channels: &[Vec<f32>]) -> Vec<Vec<f64>> {
    channels
        .iter()
        .map(|c| {
            c.chunks(FINGERPRINT_WINDOW_FRAMES)
                .map(|w| {
                    let ms = w.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / w.len() as f64;
                    let db = 10.0 * ms.log10();
                    (db.max(FINGERPRINT_FLOOR_DB) * 100.0).round() / 100.0
                })
                .collect()
        })
        .collect()
}

/// On every platform: per-half-beat RMS of the demo bounce.
#[test]
fn golden_fingerprint_matches() {
    let dir = TempDir::new("golden-fp");
    let out = dir.path("demo.wav");
    assert_success(&render(&demo_path(), &out, &[]));
    let wav = wav::read_wav_file(&out).unwrap();
    let got = fingerprint(&wav.channels);
    let path = golden("demo-project.fingerprint.json");
    if updating() {
        let doc = serde_json::json!({
            "project": "examples/projects/demo.atomik",
            "format": "f32",
            "window_frames": FINGERPRINT_WINDOW_FRAMES,
            "floor_db": FINGERPRINT_FLOOR_DB,
            "rms_db": got,
        });
        std::fs::write(&path, serde_json::to_string_pretty(&doc).unwrap() + "\n").unwrap();
    }
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("golden fingerprint")).unwrap();
    let expected: Vec<Vec<f64>> = serde_json::from_value(doc["rms_db"].clone()).unwrap();
    assert_eq!(got.len(), expected.len(), "channel count");
    for (ch, (g, e)) in got.iter().zip(&expected).enumerate() {
        assert_eq!(g.len(), e.len(), "window count of channel {ch}");
        for (w, (a, b)) in g.iter().zip(e).enumerate() {
            assert!(
                (a - b).abs() <= FINGERPRINT_TOLERANCE_DB,
                "channel {ch}, window {w} (beat {}): {a} dB vs golden {b} dB",
                w as f64 / 2.0
            );
        }
    }
}
