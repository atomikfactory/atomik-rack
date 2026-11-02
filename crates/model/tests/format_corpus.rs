//! The saved-file corpus: a real project written by each released format version, kept in
//! `tests/fixtures`. Every file must load, and saving what it loads must be canonical.
//!
//! `scripts/extract-format-fixtures.sh` produced `demo-v13` onwards from the demo project at the
//! commit that introduced each version; the older files were committed when their version was
//! current.

mod common;

use atomik_model::{PROJECT_VERSION, load_project, save_project};
use common::console_registry;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::PathBuf;

/// Versions with no saved file: they predate the first committed fixtures of their kind (the
/// reader is covered by synthetic edits in the format tests).
const NO_SAVED_FILE: &[u64] = &[2, 3, 4, 5];

fn fixtures() -> Vec<(String, String)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut files: Vec<(String, String)> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "atomik"))
        .map(|p| {
            let text = std::fs::read_to_string(&p).unwrap().replace("\r\n", "\n");
            (p.file_name().unwrap().to_string_lossy().into_owned(), text)
        })
        .collect();
    files.sort();
    files
}

fn version_of(text: &str) -> u64 {
    let v: Value = serde_json::from_str(text).unwrap();
    v["version"].as_u64().unwrap()
}

#[test]
fn every_released_version_has_a_saved_file() {
    let have: BTreeSet<u64> = fixtures().iter().map(|(_, t)| version_of(t)).collect();
    for v in 1..=u64::from(PROJECT_VERSION) {
        assert_eq!(
            have.contains(&v),
            !NO_SAVED_FILE.contains(&v),
            "version {v}: fixture present {} but listed as missing {}",
            have.contains(&v),
            NO_SAVED_FILE.contains(&v)
        );
    }
}

#[test]
fn every_fixture_loads_and_resaves_canonically() {
    let reg = console_registry();
    for (name, text) in fixtures() {
        let doc = load_project(&text, &reg).unwrap_or_else(|e| panic!("{name}: {e}"));
        let first = save_project(&doc);
        assert_eq!(first, save_project(&doc), "{name}: saving twice differs");
        let again = load_project(&first, &reg).unwrap_or_else(|e| panic!("{name} resaved: {e}"));
        let second = save_project(&again);
        assert_eq!(first, second, "{name}: the resave is not canonical");
        if version_of(&text) == u64::from(PROJECT_VERSION) {
            assert_eq!(first, text, "{name}: a current file resaves byte for byte");
        }
    }
}

// ---- the corpus folder -------------------------------------------------------------------------
//
// `tests/fixtures/corpus/atomik-vN.atomik` holds one real saved file per released version (see
// `scripts/format-corpus.sh`); `corpus/summary.json` pins what each file contains once loaded, so a
// reader that starts reading an old file differently fails here and not in a user's project.
// After an intentional change, regenerate the summary with
// `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-model --test format_corpus` and review the diff.

/// Versions whose writer predates the public history: no saved file exists for them.
const CORPUS_MISSING: &[u64] = &[2, 3, 4, 5];

/// Load warnings a corpus file may carry, per file name (none so far).
const ALLOWED_WARNINGS: &[(&str, &str)] = &[];

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/corpus")
}

fn corpus() -> Vec<(u64, String, String)> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(corpus_dir()).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if let Some(n) = name
            .strip_prefix("atomik-v")
            .and_then(|r| r.strip_suffix(".atomik"))
        {
            let version: u64 = n
                .parse()
                .unwrap_or_else(|_| panic!("{name}: not a version"));
            let text = std::fs::read_to_string(&path)
                .unwrap()
                .replace("\r\n", "\n");
            files.push((version, name, text));
        }
    }
    files.sort();
    files
}

/// Counts and note values of a project as the current writer saves it (tracks, lanes, clips and
/// notes are the song's own content; devices and cables also depend on what the writer adds).
fn summary(saved: &str) -> Value {
    fn walk(v: &Value, key: &str, out: &mut std::collections::BTreeMap<&'static str, f64>) {
        let bump =
            |out: &mut std::collections::BTreeMap<&'static str, f64>, k: &'static str, n: f64| {
                *out.entry(k).or_insert(0.0) += n;
            };
        match v {
            Value::Array(items) => {
                match key {
                    "tracks" => bump(out, "tracks", items.len() as f64),
                    "lanes" => bump(out, "lanes", items.len() as f64),
                    "clips" => bump(out, "clips", items.len() as f64),
                    "notes" => {
                        bump(out, "notes", items.len() as f64);
                        for note in items {
                            let sum: f64 = note
                                .as_array()
                                .map(|n| n.iter().take(3).filter_map(Value::as_f64).sum())
                                .unwrap_or(0.0);
                            bump(out, "note_sum", sum);
                        }
                    }
                    _ => {}
                }
                for item in items {
                    walk(item, "", out);
                }
            }
            Value::Object(map) => {
                for (k, item) in map {
                    walk(item, k, out);
                }
            }
            _ => {}
        }
    }
    let doc: Value = serde_json::from_str(saved).unwrap();
    let mut counts = std::collections::BTreeMap::new();
    walk(&doc, "", &mut counts);
    let mut out = serde_json::Map::new();
    for key in ["tracks", "lanes", "clips", "notes", "note_sum"] {
        out.insert(key.into(), counts.get(key).copied().unwrap_or(0.0).into());
    }
    out.insert("sample_rate".into(), doc["sample_rate"].clone());
    out.insert("tempo_bpm".into(), doc["tempo_bpm"].clone());
    Value::Object(out)
}

/// What a corpus file must satisfy; `Err` names the first failure.
fn check_corpus_file(name: &str, text: &str) -> Result<Value, String> {
    let reg = console_registry();
    let (doc, warnings) = atomik_model::load_project_with_warnings(text, &reg)
        .map_err(|e| format!("{name}: does not load: {e}"))?;
    for w in &warnings {
        if !ALLOWED_WARNINGS
            .iter()
            .any(|(n, m)| *n == name && w.contains(m))
        {
            return Err(format!("{name}: load warning: {w}"));
        }
    }
    let first = save_project(&doc);
    let again = load_project(&first, &reg).map_err(|e| format!("{name}: resave: {e}"))?;
    if save_project(&again) != first {
        return Err(format!("{name}: the resave is not canonical"));
    }
    Ok(summary(&first))
}

#[test]
fn corpus_has_a_file_for_every_released_version() {
    let have: BTreeSet<u64> = corpus().iter().map(|(v, _, _)| *v).collect();
    for v in 1..=u64::from(PROJECT_VERSION) {
        assert_eq!(
            have.contains(&v),
            !CORPUS_MISSING.contains(&v),
            "version {v}: atomik-v{v}.atomik present {} but listed as missing {}",
            have.contains(&v),
            CORPUS_MISSING.contains(&v)
        );
    }
    assert!(
        have.iter().all(|v| *v <= u64::from(PROJECT_VERSION)),
        "a corpus file is newer than the reader"
    );
}

#[test]
fn corpus_loads_and_resaves_canonically() {
    let path = corpus_dir().join("summary.json");
    let mut got = serde_json::Map::new();
    for (version, name, text) in corpus() {
        assert_eq!(
            version_of(&text),
            version,
            "{name}: the file is another version"
        );
        let summary = check_corpus_file(&name, &text).unwrap_or_else(|e| panic!("{e}"));
        got.insert(format!("v{version}"), summary);
    }
    let got = Value::Object(got);
    if std::env::var_os("ATOMIK_UPDATE_GOLDEN").is_some_and(|v| v == "1") {
        std::fs::write(&path, serde_json::to_string_pretty(&got).unwrap() + "\n").unwrap();
    }
    let want: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        got, want,
        "what a corpus file holds changed (see the module notes)"
    );
}

/// A reader that drops what it should read (a lane, a note, a device) must fail the corpus test.
#[test]
fn a_broken_reader_path_fails_the_corpus_check() {
    let (_, name, text) = corpus().into_iter().find(|(v, _, _)| *v == 20).unwrap();
    let pinned: Value =
        serde_json::from_str(&std::fs::read_to_string(corpus_dir().join("summary.json")).unwrap())
            .unwrap();
    let ok = check_corpus_file(&name, &text).unwrap();
    assert_eq!(ok, pinned["v20"], "the unbroken file matches its pin");

    // The reader skips every note of the first clip.
    let mut v: Value = serde_json::from_str(&text).unwrap();
    let notes = v["song"]["tracks"][0]["lanes"][0]["clips"][0]["notes"]
        .as_array_mut()
        .unwrap();
    assert!(!notes.is_empty());
    notes.clear();
    let broken = check_corpus_file(&name, &v.to_string()).unwrap();
    assert_ne!(broken, pinned["v20"], "dropped notes are noticed");

    // A file that no longer loads is an error, not a pass.
    let mut v: Value = serde_json::from_str(&text).unwrap();
    v["devices"] = Value::String("broken".into());
    assert!(check_corpus_file(&name, &v.to_string()).is_err());

    // An unexpected load warning is an error.
    let mut v: Value = serde_json::from_str(&text).unwrap();
    v["song"]["track_heights"] = serde_json::json!({ "ghost": 80 });
    assert!(
        check_corpus_file(&name, &v.to_string())
            .unwrap_err()
            .contains("warning")
    );
}

/// The release gate: `scripts/release/check-corpus.sh` passes for the current format and fails for
/// a version with no corpus file (a dry run with a temporary version), and the release workflow
/// runs it.
#[test]
fn the_release_check_needs_a_corpus_file_for_the_format_version() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let run = |version: Option<&str>| {
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("scripts/release/check-corpus.sh")
            .current_dir(&root);
        if let Some(v) = version {
            cmd.env("ATOMIK_CORPUS_VERSION", v);
        }
        cmd.output()
    };
    let workflow = std::fs::read_to_string(root.join(".github/workflows/release.yml")).unwrap();
    assert!(workflow.contains("scripts/release/check-corpus.sh"));
    let Ok(current) = run(None) else {
        return; // no `sh` on this machine
    };
    assert!(
        current.status.success(),
        "{}",
        String::from_utf8_lossy(&current.stderr)
    );
    let missing = run(Some(&(PROJECT_VERSION + 1).to_string())).unwrap();
    assert!(
        !missing.status.success(),
        "a version without a corpus file must fail"
    );
    assert!(String::from_utf8_lossy(&missing.stderr).contains("no corpus file"));
}
