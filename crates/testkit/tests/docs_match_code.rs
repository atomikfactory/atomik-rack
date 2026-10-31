//! The documents agree with the code on what a test can check: the crate table of
//! `docs/ARCHITECTURE.md` against the `Cargo.toml` files, and the functions the documents cite
//! against the sources.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root")
}

fn read(rel: &str) -> String {
    fs::read_to_string(root().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// What the `Cargo.toml` files and sources say about one crate.
#[derive(Debug)]
struct Crate {
    /// The atomik crates it depends on, without the `atomik-` prefix (not dev-dependencies).
    deps: BTreeSet<String>,
    /// `crate` when the whole crate allows `unsafe`, else the files that do, from the crate root.
    unsafe_in: BTreeSet<String>,
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if entry.file_name() != "target" {
                rust_files(&path, out);
            }
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// The crates of `crates/` (not the test plugins) by their short name (`core` for `atomik-core`).
fn workspace_crates() -> BTreeMap<String, Crate> {
    let mut crates = BTreeMap::new();
    for entry in fs::read_dir(root().join("crates")).unwrap().flatten() {
        let dir = entry.path();
        let manifest = dir.join("Cargo.toml");
        if !manifest.exists() || entry.file_name() == "testplugins" {
            continue;
        }
        let text = fs::read_to_string(&manifest).unwrap();
        let name = text
            .lines()
            .find_map(|l| l.strip_prefix("name = \""))
            .and_then(|l| l.strip_suffix('"'))
            .and_then(|n| n.strip_prefix("atomik-"))
            .unwrap_or_else(|| panic!("{}: no atomik package name", manifest.display()))
            .to_owned();
        let mut deps = BTreeSet::new();
        let mut section = String::new();
        let mut crate_unsafe = false;
        for line in text.lines() {
            if line.starts_with('[') {
                section = line.trim().to_owned();
            } else if section.ends_with("dependencies]") && !section.contains("dev-") {
                if let Some(dep) = line.strip_prefix("atomik-") {
                    let end = dep.find(['.', ' ', '=']).unwrap_or(dep.len());
                    deps.insert(dep[..end].to_owned());
                }
            } else if line.replace(' ', "") == "unsafe_code=\"allow\"" {
                crate_unsafe = true;
            }
        }
        let mut unsafe_in = BTreeSet::new();
        if crate_unsafe {
            unsafe_in.insert("crate".to_owned());
        } else {
            let mut files = Vec::new();
            rust_files(&dir, &mut files);
            for file in files {
                let src = fs::read_to_string(&file).unwrap();
                if src.lines().any(|l| l.starts_with("#![allow(unsafe_code)]")) {
                    let rel = file.strip_prefix(&dir).unwrap();
                    unsafe_in.insert(rel.to_string_lossy().replace('\\', "/"));
                }
            }
        }
        crates.insert(name, Crate { deps, unsafe_in });
    }
    crates
}

/// One row of the crate table of ARCHITECTURE.md section 1.
struct Row {
    name: String,
    deps: BTreeSet<String>,
    unsafe_in: BTreeSet<String>,
    text: String,
}

/// The cell's items: the text outside parentheses (notes and external crates), split at commas,
/// `none` meaning nothing.
fn items(cell: &str) -> BTreeSet<String> {
    let mut depth = 0;
    let outside: String = cell
        .chars()
        .filter(|&c| {
            match c {
                '(' => depth += 1,
                ')' => depth -= 1,
                _ => {}
            }
            depth == 0 && c != ')'
        })
        .collect();
    outside
        .split(',')
        .map(|i| i.trim().trim_matches('`').to_owned())
        .filter(|i| !i.is_empty() && i != "none")
        .collect()
}

fn table_rows() -> Vec<Row> {
    // The first table of section 1: from its header to the first line that is not a row.
    read("docs/ARCHITECTURE.md")
        .lines()
        .skip_while(|l| !l.starts_with("| Crate | Path |"))
        .skip(2)
        .take_while(|l| l.starts_with('|'))
        .map(|l| {
            let cells: Vec<&str> = l
                .trim()
                .trim_matches('|')
                .split('|')
                .map(str::trim)
                .collect();
            assert_eq!(cells.len(), 5, "a crate row has five cells: {l}");
            Row {
                name: cells[0]
                    .trim_matches('`')
                    .strip_prefix("atomik-")
                    .unwrap()
                    .to_owned(),
                deps: items(cells[2]),
                unsafe_in: items(cells[3]),
                text: l.to_owned(),
            }
        })
        .collect()
}

#[test]
fn architecture_crate_table_matches_cargo_toml() {
    let actual = workspace_crates();
    let rows = table_rows();
    let listed: BTreeSet<&str> = rows.iter().map(|r| r.name.as_str()).collect();
    let present: BTreeSet<&str> = actual.keys().map(String::as_str).collect();
    assert_eq!(
        listed, present,
        "docs/ARCHITECTURE.md section 1 lists the crates of crates/ (not the test plugins)"
    );
    for row in &rows {
        let crate_ = &actual[&row.name];
        assert_eq!(
            row.deps, crate_.deps,
            "atomik-{}: the table's dependencies differ from its Cargo.toml",
            row.name
        );
        assert_eq!(
            row.unsafe_in, crate_.unsafe_in,
            "atomik-{}: the table's `unsafe` column differs from the code (`crate` when the \
             whole crate allows it, else the files with `#![allow(unsafe_code)]`)",
            row.name
        );
    }
}

#[test]
fn the_crate_table_quotes_the_project_format_version() {
    let source = read("crates/model/src/project/mod.rs");
    let version = source
        .lines()
        .find_map(|l| l.strip_prefix("pub const PROJECT_VERSION: u32 = "))
        .and_then(|l| l.strip_suffix(';'))
        .expect("PROJECT_VERSION");
    let model = table_rows()
        .into_iter()
        .find(|r| r.name == "model")
        .expect("the model row");
    assert!(
        model.text.contains(&format!("format v{version})")),
        "the atomik-model row of the crate table says `format v{version}`"
    );
}

/// Names the documents may cite although no source under `crates/` contains them (a function of
/// an external tool, say). A cited name that is gone from the code is a stale
/// document, except the generated names listed here.
const NOT_IN_CRATES: &[&str] = &[
    // Socket keys the device macros build with `concat!`: the sources hold `send1_level_cv_in`.
    "pad16_gate_cv_in",
    "pad16_gate_cv_out",
    "send8_level_cv_in",
];

/// The names of the files and folders under `dir` (without extension): test files and golden
/// files are cited by name too.
fn file_names(dir: &Path, ids: &mut BTreeSet<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if let Some(stem) = path.file_stem() {
            ids.insert(stem.to_string_lossy().into_owned());
        }
        if path.is_dir() && entry.file_name() != "target" {
            file_names(&path, ids);
        }
    }
}

/// Every snake_case identifier in the sources of `crates/`, and the names of its files.
fn source_identifiers() -> BTreeSet<String> {
    let mut files = Vec::new();
    rust_files(&root().join("crates"), &mut files);
    let mut ids = BTreeSet::new();
    for file in files {
        let src = fs::read_to_string(file).unwrap();
        let mut word = String::new();
        for c in src.chars().chain(std::iter::once(' ')) {
            if c.is_ascii_alphanumeric() || c == '_' {
                word.push(c);
            } else {
                if word.contains('_') {
                    ids.insert(std::mem::take(&mut word));
                }
                word.clear();
            }
        }
    }
    file_names(&root().join("crates"), &mut ids);
    ids
}

/// The inline code spans of a Markdown text, outside fenced blocks.
fn code_spans(text: &str) -> Vec<String> {
    let mut spans = Vec::new();
    let mut fenced = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        let mut parts = line.split('`');
        parts.next();
        while let Some(span) = parts.next() {
            if parts.next().is_none() {
                break;
            }
            spans.push(span.to_owned());
        }
    }
    spans
}

/// The function a span cites, when it is a snake_case name (`name`, `name()`, `Type::name`,
/// `module::name(args)`): the last path segment.
fn cited_function(span: &str) -> Option<&str> {
    let span = span.trim().strip_prefix("fn ").unwrap_or(span.trim());
    let call = span.split('(').next().unwrap_or(span);
    let name = call.rsplit("::").next()?;
    let snake = name.contains('_')
        && name.len() >= 5
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    let path_ok = call
        .split("::")
        .all(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
    (snake && path_ok).then_some(name)
}

#[test]
fn cited_functions_exist() {
    let ids = source_identifiers();
    let mut missing = BTreeMap::new();
    for doc in ["docs/ARCHITECTURE.md", "docs/TESTING.md", "docs/DEVICES.md"] {
        for span in code_spans(&read(doc)) {
            if let Some(name) = cited_function(&span)
                && !ids.contains(name)
                && !NOT_IN_CRATES.contains(&name)
            {
                missing
                    .entry(name.to_owned())
                    .or_insert_with(Vec::new)
                    .push(doc);
            }
        }
    }
    assert!(
        missing.is_empty(),
        "the documents cite names that no source under crates/ contains (renamed or removed? \
         fix the document): {missing:#?}"
    );
}
