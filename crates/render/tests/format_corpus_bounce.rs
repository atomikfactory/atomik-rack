//! The sound of every saved file in the format corpus
//! (`crates/model/tests/fixtures/corpus/atomik-vN.atomik`, one real file per released format
//! version) is pinned, so a change in how an old file is read cannot silently change how it sounds.
//!
//! `tests/golden/corpus.fingerprint.json` holds the RMS level in dB of every second of each
//! channel of each file's bounce; it is checked on every platform within 0.5 dB, like the other
//! fingerprints. `tests/golden/corpus.f32.sha256.json` holds the SHA-256 of each bounce's 32-bit float
//! bytes and is checked on Windows x86-64 only, where the goldens are produced (see
//! `docs/TESTING.md`, "Golden files"). After an intentional change to the sound, regenerate both on
//! Windows x86-64 with
//!
//! ```text
//! ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-render --test format_corpus_bounce
//! ```
//!
//! and review the diff: a file that moves on its own, without a change to the sound, is a bug in
//! the reader.

use atomik_render::default_registry;
use atomik_render::project::{ProjectOptions, render_project_json};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const WINDOW_FRAMES: usize = 48_000;
const FLOOR_DB: f64 = -120.0;
const TOLERANCE_DB: f64 = 0.5;
/// Files that reference audio kept in a bundle folder the corpus does not carry: they load (the
/// model corpus test) but cannot be bounced.
const NO_BOUNCE: &[&str] = &["v8"];

fn corpus_files() -> Vec<(String, String)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../model/tests/fixtures/corpus");
    let mut files: Vec<(u64, String, String)> = std::fs::read_dir(&dir)
        .expect("the format corpus")
        .filter_map(|e| {
            let path = e.ok()?.path();
            let name = path.file_name()?.to_string_lossy().into_owned();
            let version = name
                .strip_prefix("atomik-v")?
                .strip_suffix(".atomik")?
                .parse()
                .ok()?;
            let text = std::fs::read_to_string(&path).ok()?;
            Some((version, format!("v{version}"), text))
        })
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|(_, n, t)| (n, t))
        .filter(|(n, _)| !NO_BOUNCE.contains(&n.as_str()))
        .collect()
}

fn golden(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name)
}

fn updating() -> bool {
    std::env::var_os("ATOMIK_UPDATE_GOLDEN").is_some_and(|v| v == "1")
}

/// RMS in dB (floored) of each window of each channel.
fn fingerprint(channels: &[Vec<f32>]) -> Vec<Vec<f64>> {
    channels
        .iter()
        .map(|c| {
            c.chunks(WINDOW_FRAMES)
                .map(|w| {
                    let ms = w.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / w.len() as f64;
                    (10.0 * ms.log10()).max(FLOOR_DB)
                })
                .map(|db| (db * 100.0).round() / 100.0)
                .collect()
        })
        .collect()
}

struct Bounce {
    channels: Vec<Vec<f32>>,
}

fn bounce(text: &str) -> Bounce {
    let rendered = render_project_json(text, &default_registry(), &ProjectOptions::default())
        .expect("a corpus file bounces");
    Bounce {
        channels: rendered.channels,
    }
}

#[test]
fn every_corpus_file_sounds_as_pinned() {
    let files = corpus_files();
    assert!(files.len() >= 35, "the corpus is there: {}", files.len());
    let got: serde_json::Map<String, Value> = files
        .iter()
        .map(|(name, text)| (name.clone(), json!(fingerprint(&bounce(text).channels))))
        .collect();
    let path = golden("corpus.fingerprint.json");
    if updating() {
        let doc = json!({
            "about": "RMS dB of each second of each channel of the bounce of every corpus file",
            "window_frames": WINDOW_FRAMES,
            "floor_db": FLOOR_DB,
            "rms_db": got,
        });
        std::fs::write(&path, serde_json::to_string_pretty(&doc).unwrap() + "\n").unwrap();
    }
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let want = doc["rms_db"].as_object().expect("rms_db");
    assert_eq!(
        want.keys().collect::<Vec<_>>().len(),
        got.len(),
        "the corpus and its fingerprints list different files"
    );
    for (name, fp) in &got {
        let expected: Vec<Vec<f64>> = serde_json::from_value(want[name].clone())
            .unwrap_or_else(|_| panic!("{name}: no fingerprint pinned"));
        let fp: Vec<Vec<f64>> = serde_json::from_value(fp.clone()).unwrap();
        assert_eq!(fp.len(), expected.len(), "{name}: channel count");
        for (ch, (g, e)) in fp.iter().zip(&expected).enumerate() {
            assert_eq!(g.len(), e.len(), "{name}: window count of channel {ch}");
            for (w, (a, b)) in g.iter().zip(e).enumerate() {
                assert!(
                    (a - b).abs() <= TOLERANCE_DB,
                    "{name}, channel {ch}, second {w}: {a} dB vs pinned {b} dB"
                );
            }
        }
    }
}

/// Exact bytes of every file's bounce (Windows x86-64 only: the other platforms' math libraries
/// differ in the last bits).
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
#[test]
fn every_corpus_file_hashes_as_pinned_on_windows() {
    use sha2::{Digest, Sha256};
    let got: serde_json::Map<String, Value> = corpus_files()
        .iter()
        .map(|(name, text)| {
            let mut hasher = Sha256::new();
            for channel in bounce(text).channels {
                for sample in channel {
                    hasher.update(sample.to_le_bytes());
                }
            }
            let hex: String = hasher
                .finalize()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            (name.clone(), json!(hex))
        })
        .collect();
    let path = golden("corpus.f32.sha256.json");
    if updating() {
        std::fs::write(&path, serde_json::to_string_pretty(&got).unwrap() + "\n").unwrap();
    }
    let want: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        Value::Object(got),
        want,
        "a corpus bounce changed (see the module docs)"
    );
}
