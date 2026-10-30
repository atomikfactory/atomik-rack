//! The Markdown files stay reviewable and their links stay true: the long guides are hard-wrapped,
//! and every relative link and `#anchor` of every tracked Markdown file resolves.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root")
}

/// The Markdown files of the repository: the tracked ones when `git` answers, else every one
/// outside the build output and the untracked planning folder.
fn markdown_files() -> Vec<PathBuf> {
    let root = root();
    if let Ok(out) = Command::new("git")
        .args(["ls-files", "*.md"])
        .current_dir(&root)
        .output()
        && out.status.success()
    {
        let files: Vec<PathBuf> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|l| root.join(l))
            .filter(|p| p.exists())
            .collect();
        if !files.is_empty() {
            return files;
        }
    }
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            if path.is_dir() {
                if !["target", ".git", "not-for-git"].iter().any(|n| name == *n) {
                    walk(&path, out);
                }
            } else if path.extension().is_some_and(|e| e == "md") {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(&root, &mut out);
    out
}

/// The lines of a Markdown text with their numbers (from 1), outside fenced code blocks.
fn prose_lines(text: &str) -> Vec<(usize, &str)> {
    let mut fenced = false;
    let mut lines = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
        } else if !fenced {
            lines.push((i + 1, line));
        }
    }
    lines
}

/// `line` with the content of every inline code span replaced by `x`.
fn without_code(line: &str) -> String {
    let mut out = String::new();
    let mut in_code = false;
    for part in line.split('`') {
        if in_code {
            out.push('x');
        } else {
            out.push_str(part);
        }
        in_code = !in_code;
    }
    out
}

/// The documents whose paragraphs are hard-wrapped, and the width past which a line is too long.
const WRAPPED: &[&str] = &[
    "CHANGELOG.md",
    "docs/ARCHITECTURE.md",
    "docs/DEVICES.md",
    "docs/TESTING.md",
];
const WIDTH: usize = 120;

#[test]
fn the_long_guides_are_hard_wrapped() {
    let mut long = Vec::new();
    for doc in WRAPPED {
        let text = fs::read_to_string(root().join(doc)).unwrap();
        for (n, line) in prose_lines(&text) {
            // Tables and a line that is one long code span cannot be broken.
            if line.starts_with('|') || without_code(line).chars().count() <= WIDTH {
                continue;
            }
            long.push(format!(
                "{doc}:{n}: {} columns, {}",
                line.chars().count(),
                line.chars().take(50).collect::<String>()
            ));
        }
    }
    assert!(
        long.is_empty(),
        "lines past {WIDTH} columns (wrap them at about 100, tables and long code spans excepted):\n{}",
        long.join("\n")
    );
}

/// GitHub's anchor of a heading: lowercase, letters, digits, `-` and `_` kept, spaces to `-`,
/// everything else dropped.
fn slug(heading: &str) -> String {
    heading
        .trim()
        .to_lowercase()
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('-'),
            c if c.is_alphanumeric() || c == '-' || c == '_' => Some(c),
            _ => None,
        })
        .collect()
}

/// The anchors a Markdown text offers (a repeated heading gets `-1`, `-2`, ...).
fn anchors(text: &str) -> BTreeSet<String> {
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    let mut out = BTreeSet::new();
    for (_, line) in prose_lines(text) {
        let hashes = line.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&hashes) && line[hashes..].starts_with(' ') {
            let base = slug(line[hashes..].trim_end_matches('#'));
            let n = seen.entry(base.clone()).or_insert(0);
            out.insert(if *n == 0 { base } else { format!("{base}-{n}") });
            *n += 1;
        }
    }
    out
}

/// The link targets of a line: inline `](target)` ones and a reference definition's.
fn link_targets(line: &str) -> Vec<String> {
    let line = without_code(line);
    let mut targets = Vec::new();
    let mut rest = line.as_str();
    while let Some(at) = rest.find("](") {
        let after = &rest[at + 2..];
        let mut depth = 0;
        let mut end = after.len();
        for (i, c) in after.char_indices() {
            match c {
                '(' => depth += 1,
                ')' if depth == 0 => {
                    end = i;
                    break;
                }
                ')' => depth -= 1,
                _ => {}
            }
        }
        let target = after[..end].trim().trim_start_matches('<');
        let target = target.split([' ', '>']).next().unwrap_or("");
        if !target.is_empty() {
            targets.push(target.to_owned());
        }
        rest = &after[end.min(after.len())..];
    }
    if let Some(def) = line.strip_prefix('[')
        && let Some((_, url)) = def.split_once("]:")
        && let Some(url) = url.split_whitespace().next()
    {
        targets.push(url.to_owned());
    }
    targets
}

#[test]
fn markdown_links_and_anchors_resolve() {
    let root = root();
    let mut cache: BTreeMap<PathBuf, BTreeSet<String>> = BTreeMap::new();
    let mut broken = Vec::new();
    for file in markdown_files() {
        let text = fs::read_to_string(&file).unwrap();
        let dir = file.parent().unwrap().to_owned();
        for (n, line) in prose_lines(&text) {
            for target in link_targets(line) {
                if target.contains("://") || target.starts_with("mailto:") {
                    continue;
                }
                let (path, fragment) = target.split_once('#').unwrap_or((&target, ""));
                let resolved = if path.is_empty() {
                    file.clone()
                } else if let Some(abs) = path.strip_prefix('/') {
                    root.join(abs)
                } else {
                    dir.join(path)
                };
                let here = file.strip_prefix(&root).unwrap().display();
                if !resolved.exists() {
                    broken.push(format!("{here}:{n}: {target} does not exist"));
                    continue;
                }
                if fragment.is_empty() || resolved.extension().is_none_or(|e| e != "md") {
                    continue;
                }
                let known = cache
                    .entry(resolved.clone())
                    .or_insert_with(|| anchors(&fs::read_to_string(&resolved).unwrap_or_default()));
                if !known.contains(fragment) {
                    broken.push(format!("{here}:{n}: {target}: no heading with that anchor"));
                }
            }
        }
    }
    assert!(
        broken.is_empty(),
        "broken Markdown links:\n{}",
        broken.join("\n")
    );
}
