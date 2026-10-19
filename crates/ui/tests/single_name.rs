//! The audio device has one name in everything a user can read. No source file
//! of any crate spells "Audio Interface" or captions the panel "INTERFACE". The type id
//! (`atomik.audio-interface`) and the reserved key (`interface`) are not user text and stay.

use std::path::{Path, PathBuf};

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// Files that may mention the old names: none today (the legacy-migration allowlist of the
/// plan is empty because no stored file carries either string).
const ALLOWLIST: &[&str] = &[];

#[test]
fn no_source_spells_the_old_names() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    for c in std::fs::read_dir(&crates).unwrap().flatten() {
        rust_files(&c.path().join("src"), &mut files);
    }
    assert!(files.len() > 100, "the scan found the sources");
    let mut hits = Vec::new();
    for f in files {
        let rel = f
            .strip_prefix(&crates)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if ALLOWLIST.contains(&rel.as_str()) {
            continue;
        }
        let text = std::fs::read_to_string(&f).unwrap();
        for (n, line) in text.lines().enumerate() {
            if line.contains("Audio Interface") || line.contains("\"INTERFACE\"") {
                hits.push(format!("{rel}:{}: {}", n + 1, line.trim()));
            }
        }
    }
    assert!(hits.is_empty(), "old names found:\n{}", hits.join("\n"));
}
