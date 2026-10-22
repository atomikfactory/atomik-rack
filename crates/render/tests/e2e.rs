//! End-to-end tests of the `atomik-render` binary against `examples/patches/`
//!
//! # Goldens
//!
//! `tests/golden/slice-demo.f32.sha256` (exact bytes, Windows x86-64 only) and
//! `tests/golden/slice-demo.fingerprint.json` (per-half-beat RMS, all platforms) describe
//! `examples/patches/slice-demo.json` rendered as 32-bit float with the default block size.
//! After an intentional change to the sound, regenerate both on Windows x86-64 with:
//!
//! ```text
//! ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-render --test e2e golden
//! ```
//!
//! then listen to the new render before committing the goldens.

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
// only the exact-bytes golden uses it
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_atomik-render");
/// slice-demo: 16 beats + 2 tail beats at 120 BPM, 48 kHz.
const SLICE_DEMO_FRAMES: usize = 18 * 24_000;
const FINGERPRINT_WINDOW_FRAMES: usize = 12_000; // half a beat at 120 BPM, 48 kHz
const FINGERPRINT_FLOOR_DB: f64 = -120.0;
const FINGERPRINT_TOLERANCE_DB: f64 = 0.5;
const UPDATE_ENV: &str = "ATOMIK_UPDATE_GOLDEN";

/// A fresh directory under the system temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("atomik-render-e2e-{tag}-{}", std::process::id()));
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

fn example(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/patches")
        .join(name)
}

fn golden(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name)
}

fn render(patch: &Path, out: &Path, extra: &[&str]) -> Output {
    Command::new(BIN)
        .arg(patch)
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

fn render_slice_demo(dir: &TempDir, name: &str, extra: &[&str]) -> Vec<u8> {
    let out = dir.path(name);
    let output = render(&example("slice-demo.json"), &out, extra);
    assert_success(&output);
    std::fs::read(&out).expect("read rendered wav")
}

#[test]
fn cli_render_example_writes_expected_length() {
    let dir = TempDir::new("length");
    // minimal: 2 beats + 2 tail beats.
    for (patch, frames) in [
        ("slice-demo.json", SLICE_DEMO_FRAMES),
        ("minimal.json", 4 * 24_000),
    ] {
        let out = dir.path("out.wav");
        let output = render(&example(patch), &out, &[]);
        assert_success(&output);
        assert!(
            output.stderr.is_empty(),
            "{patch} renders without warnings: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let wav = atomik_render::wav::read_wav_file(&out).expect("valid wav");
        assert_eq!(wav.channels.len(), 2, "{patch}");
        assert_eq!(wav.sample_rate, 48_000, "{patch}");
        assert_eq!(wav.format, atomik_render::wav::SampleFormat::F32);
        assert!(wav.channels.iter().all(|c| c.len() == frames), "{patch}");
        assert!(
            wav.channels.iter().flatten().any(|x| x.abs() > 1e-3),
            "{patch} is not silent"
        );
        assert!(wav.channels.iter().flatten().all(|x| x.is_finite()));
    }
}

#[test]
fn cli_render_pcm24_writes_24bit() {
    let dir = TempDir::new("pcm24");
    let out = dir.path("out.wav");
    assert_success(&render(
        &example("minimal.json"),
        &out,
        &["--format", "pcm24"],
    ));
    let wav = atomik_render::wav::read_wav_file(&out).unwrap();
    assert_eq!(wav.format, atomik_render::wav::SampleFormat::Pcm24);
}

#[test]
fn cli_unknown_param_exits_nonzero_naming_json_path() {
    let dir = TempDir::new("ac-r2");
    let text = std::fs::read_to_string(example("minimal.json")).unwrap();
    let mut v: serde_json::Value = serde_json::from_str(&text).unwrap();
    v["devices"][0]["params"]["filter.cutof"] = serde_json::json!(800);
    let patch = dir.path("typo.json");
    std::fs::write(&patch, v.to_string()).unwrap();
    let out = dir.path("out.wav");
    let output = render(&patch, &out, &[]);
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.starts_with("error: devices[0].params.\"filter.cutof\": "),
        "{stderr}"
    );
    assert!(!out.exists(), "nothing is written on error");
}

#[test]
fn cli_block_size_does_not_change_output() {
    let dir = TempDir::new("blocks");
    let one = render_slice_demo(&dir, "b1.wav", &["--block", "1"]);
    let big = render_slice_demo(&dir, "b4096.wav", &["--block", "4096"]);
    assert!(one == big, "HI-2: --block 1 and --block 4096 differ");
    let odd = render_slice_demo(&dir, "b100.wav", &["--block", "100", "--format", "pcm24"]);
    let odd2 = render_slice_demo(&dir, "b63.wav", &["--block", "63", "--format", "pcm24"]);
    assert!(odd == odd2, "HI-2: pcm24 output depends on the block size");
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))] // only the exact-bytes golden uses it
fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn updating() -> bool {
    std::env::var_os(UPDATE_ENV).is_some_and(|v| v == "1")
}

/// Exact-bytes golden. Transcendental functions (sin, tan, tanh, powf…) come from the
/// platform's math library, whose last-bit results differ between operating systems, so a
/// byte-identical golden is only meaningful on the platform that produced it (Windows x86-64).
/// Other platforms are covered by `golden_fingerprint_matches`.
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
#[test]
fn golden_hash_matches() {
    let dir = TempDir::new("golden-hash");
    let hash = sha256_hex(&render_slice_demo(&dir, "demo.wav", &[]));
    let path = golden("slice-demo.f32.sha256");
    if updating() {
        std::fs::write(&path, format!("{hash}\n")).unwrap();
    }
    let expected = std::fs::read_to_string(&path).expect("golden hash file");
    assert_eq!(
        hash,
        expected.trim(),
        "slice-demo render changed (see module docs)"
    );
}

/// `slice-demo.v2.json` (format v2: the v1 file plus layout fields) renders to the
/// same bytes as the v1 file, on every platform.
#[test]
fn ac_u2_v2_renders_bit_identically_to_v1() {
    let dir = TempDir::new("ac-u2");
    let v1 = render_slice_demo(&dir, "v1.wav", &[]);
    let out = dir.path("v2.wav");
    let output = render(&example("slice-demo.v2.json"), &out, &[]);
    assert_success(&output);
    assert!(
        output.stderr.is_empty(),
        "v2 renders without warnings: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let v2 = std::fs::read(&out).expect("read rendered wav");
    assert!(v1 == v2, "the v2 file renders differently from v1");
}

/// Against the exact-bytes golden of the v1 file (Windows x86-64, see above).
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
#[test]
fn ac_u2_v2_matches_v1_golden_hash() {
    let dir = TempDir::new("ac-u2-hash");
    let out = dir.path("v2.wav");
    assert_success(&render(&example("slice-demo.v2.json"), &out, &[]));
    let hash = sha256_hex(&std::fs::read(&out).unwrap());
    let expected = std::fs::read_to_string(golden("slice-demo.f32.sha256")).expect("golden");
    assert_eq!(hash, expected.trim(), "v2 SHA differs from the v1 golden");
}

/// RMS in dB (floored) of each half-beat window of each channel.
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

#[test]
fn golden_fingerprint_matches() {
    let dir = TempDir::new("golden-fp");
    let bytes = render_slice_demo(&dir, "demo.wav", &[]);
    let wav = atomik_render::wav::read_wav(std::io::Cursor::new(bytes)).unwrap();
    let got = fingerprint(&wav.channels);
    let path = golden("slice-demo.fingerprint.json");
    if updating() {
        let doc = serde_json::json!({
            "patch": "examples/patches/slice-demo.json",
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

/// Run with `cargo test --release -p atomik-render -- --ignored`.
#[test]
#[ignore = "performance check; run in release: cargo test --release -p atomik-render -- --ignored"]
fn realtime_factor_release() {
    let dir = TempDir::new("rtf");
    let out = dir.path("demo.wav");
    let output = render(&example("slice-demo.json"), &out, &["--stats"]);
    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let rtf: f64 = stdout
        .lines()
        .find_map(|l| l.strip_prefix("real-time factor: "))
        .and_then(|v| v.trim_end_matches('x').parse().ok())
        .expect("--stats prints the real-time factor");
    println!("{stdout}");
    assert!(rtf >= 50.0, "real-time factor {rtf} < 50");
}

#[test]
fn cli_mod_source_demo_is_block_independent_and_modulated() {
    let dir = TempDir::new("mod-source");
    let render_with = |name: &str, block: &str| {
        let out = dir.path(name);
        let output = render(&example("mod-source-demo.json"), &out, &["--block", block]);
        assert_success(&output);
        assert!(
            output.stderr.is_empty(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        std::fs::read(&out).expect("wav")
    };
    let a = render_with("m1.wav", "1");
    let b = render_with("m4096.wav", "4096");
    assert!(
        a == b,
        "HI-2: the Mod Source patch depends on the block size"
    );
    let wav = atomik_render::wav::read_wav_file(&dir.path("m1.wav")).expect("valid wav");
    assert!(wav.channels.iter().flatten().any(|x| x.abs() > 1e-3));
    assert!(wav.channels.iter().flatten().all(|x| x.is_finite()));
}
