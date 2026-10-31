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
