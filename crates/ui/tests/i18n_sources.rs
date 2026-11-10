//! The extractor: scans the sources of the program for the texts written straight into a call
//! that shows or looks up text (`TextRun::new("..")`, `t("..")`, `tr("..")`, `tf("..", ..)`) and
//! checks that every one has a French entry. A text that reaches the screen by another road (a
//! label returned by a function, a table of choices) is found by the tests that draw the views
//! in French; this test is the quick net for the calls that hold their text in plain sight, and
//! it fails the moment someone writes a new one without a French line.

use atomik_ui::Lang;
use atomik_ui::i18n;
use std::path::{Path, PathBuf};

/// Texts that are names and stay as they are.
const NAMES: &[&str] = &["Atomik Rack"];

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if !name.ends_with("_tests.rs") && name != "tests.rs" {
                out.push(path);
            }
        }
    }
}

/// The value of the Rust string literal that starts at `s[0]` (a `"`), and what follows it.
fn literal(s: &str) -> Option<(String, &str)> {
    let mut chars = s.char_indices();
    if chars.next()?.1 != '"' {
        return None;
    }
    let mut out = String::new();
    let mut escaped = false;
    let mut skip_to = 0;
    for (i, c) in chars {
        if i < skip_to {
            continue;
        }
        if escaped {
            escaped = false;
            match c {
                'n' => out.push('\n'),
                '"' => out.push('"'),
                '\'' => out.push('\''),
                '\\' => out.push('\\'),
                'u' => {
                    let rest = &s[i + 1..];
                    let end = rest.find('}')?;
                    let code =
                        u32::from_str_radix(rest[1..end].trim_start_matches('{'), 16).ok()?;
                    out.push(char::from_u32(code)?);
                    skip_to = i + 1 + end + 1;
                }
                _ => return None,
            }
        } else if c == '\\' {
            escaped = true;
        } else if c == '"' {
            return Some((out, &s[i + 1..]));
        } else {
            out.push(c);
        }
    }
    None
}

/// The texts written straight into the calls, with the file they are in.
fn written_texts() -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    sources(&root.join("src"), &mut files);
    sources(&root.join("../app/src"), &mut files);
    let mut found = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(&file).unwrap();
        // The tests of a file sit at its end.
        let text = text
            .split("#[cfg(test)]")
            .next()
            .unwrap_or_default()
            .to_owned();
        for call in [
            "TextRun::new(",
            "i18n::t(",
            "i18n::tr(",
            " t(",
            " tr(",
            "(t(",
            "(tr(",
            "i18n::tf(",
        ] {
            let mut rest = text.as_str();
            while let Some(at) = rest.find(call) {
                rest = &rest[at + call.len()..];
                let arg = rest.trim_start();
                if let Some((lit, _)) = literal(arg) {
                    found.push((lit, file.display().to_string()));
                }
            }
        }
    }
    found
}

#[test]
fn every_text_written_into_a_call_has_a_french_entry() {
    let mut missing: Vec<String> = written_texts()
        .into_iter()
        .filter(|(t, _)| t.chars().filter(|c| c.is_alphabetic()).count() >= 2)
        .filter(|(t, _)| !NAMES.contains(&t.as_str()) && !i18n::has(Lang::French, t))
        // A format string with a value is looked up by its shape.
        .filter(|(t, _)| !t.contains('{'))
        .map(|(t, f)| format!("{t:?} ({})", f.rsplit(['/', '\\']).next().unwrap_or(&f)))
        .collect();
    missing.sort();
    missing.dedup();
    assert!(
        missing.is_empty(),
        "{} texts written into a call have no French entry:\n{}",
        missing.len(),
        missing.join("\n")
    );
}

#[test]
fn the_scan_finds_the_calls_it_is_looking_for() {
    let texts = written_texts();
    assert!(texts.len() > 40, "{} texts found", texts.len());
    assert!(
        texts
            .iter()
            .any(|(t, _)| t == "OUTPUTS" || t == "STEP EDITOR")
    );
}
