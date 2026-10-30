//! Production code reports through the `log` facade, not `eprintln!`: nothing prints to a
//! stderr that a GUI user never sees. The command-line programs below are the exception: their
//! stderr is their output (or, for the plugin host and scanner, what the app reads from them).

use std::fs;
use std::path::{Path, PathBuf};

/// Crates whose binaries are command-line tools or child processes.
const PROGRAM_CRATES: &[&str] = &["mcp", "play", "plugin-scan", "plugin-hostd", "testplugins"];

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// The lines of production code in `text`: everything before the first `#[cfg(test)]`.
fn production_lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.lines()
        .enumerate()
        .take_while(|(_, l)| l.trim() != "#[cfg(test)]")
}

#[test]
fn no_production_file_prints_with_eprintln() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut offenders = Vec::new();
    for entry in fs::read_dir(&crates).unwrap().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if PROGRAM_CRATES.contains(&name.as_str()) {
            continue;
        }
        let mut files = Vec::new();
        rust_files(&entry.path().join("src"), &mut files);
        for file in files {
            let stem = file.file_stem().unwrap().to_string_lossy().into_owned();
            if stem == "tests" || stem.ends_with("_tests") {
                continue;
            }
            let text = fs::read_to_string(&file).unwrap();
            for (n, line) in production_lines(&text) {
                let code = line.trim_start();
                if code.contains("eprintln!") && !code.starts_with("//") {
                    offenders.push(format!("{}:{}", file.display(), n + 1));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "use log::warn! / log::error! instead of eprintln!:\n{}",
        offenders.join("\n")
    );
}
