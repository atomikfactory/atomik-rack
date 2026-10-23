//! `unsafe` is confined. The workspace denies `unsafe_code`; only the plugin
//! format backends and the test plugin libraries (which talk to C ABIs), the test allocator and
//! the host's float-environment and OLE modules and one test window helper may use it, and every `unsafe` block there has a
//! `SAFETY:` comment. This test reads the sources and fails if `unsafe` shows up anywhere else.

use std::fs;
use std::path::{Path, PathBuf};

/// Directories (relative to the workspace root, with `/`) whose crates may contain `unsafe`.
const ALLOWED_DIRS: &[&str] = &[
    "crates/plugin-clap/",
    "crates/plugin-vst3/",
    // The sandbox wire: the shared-memory mapping and the OS wake-up calls.
    "crates/plugin-ipc/",
    "crates/testplugins/clap/",
    "crates/testplugins/clap-shell/",
    "crates/testplugins/clap-bad/",
    "crates/testplugins/clap-hang/",
    "crates/testplugins/clap-crash/",
    "crates/testplugins/clap-leak/",
    "crates/testplugins/clap-variants/",
    "crates/testplugins/vst3/",
    "crates/testplugins/vst3-shell/",
    "crates/testplugins/vst3-bad/",
    "crates/testplugins/vst3-hang/",
    "crates/testplugins/vst3-crash/",
    "crates/testplugins/vst3-leak/",
    "crates/testplugins/vst3-variants/",
];

/// Single files that may contain `unsafe`, pinned by name.
const ALLOWED_FILES: &[&str] = &[
    // The counting allocator of the allocation guard (a test-only crate).
    "crates/testkit/src/no_alloc.rs",
    // Flush-to-zero and denormals-are-zero need the floating-point control register, which has
    // no safe API.
    "crates/plugin-host/src/fpenv.rs",
    // OleInitialize on the plugin main thread (Windows); the one other exception.
    "crates/plugin-host/src/ole.rs",
    // A stand-in for the app's main window in the editor tests (Windows).
    "crates/plugin-hostd/tests/support/owner_window.rs",
];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if path.is_dir() {
            if name != "target" && name != ".git" {
                rust_files(&path, out);
            }
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Source with comments, string literals and char literals blanked out (newlines kept), so
/// words inside them are never mistaken for code.
fn code_only(src: &str) -> String {
    let b: Vec<char> = src.chars().collect();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    let blank = |c: char| if c == '\n' { '\n' } else { ' ' };
    while i < b.len() {
        let c = b[i];
        let next = b.get(i + 1).copied();
        if c == '/' && next == Some('/') {
            while i < b.len() && b[i] != '\n' {
                out.push(' ');
                i += 1;
            }
        } else if c == '/' && next == Some('*') {
            let mut depth = 0;
            while i < b.len() {
                if b[i] == '/' && b.get(i + 1) == Some(&'*') {
                    depth += 1;
                    out.push_str("  ");
                    i += 2;
                } else if b[i] == '*' && b.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    out.push_str("  ");
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    out.push(blank(b[i]));
                    i += 1;
                }
            }
        } else if c == 'r' && matches!(next, Some('"' | '#')) && !prev_is_ident(&b, i) {
            // Raw string r"..." or r#"..."#.
            let mut j = i + 1;
            let mut hashes = 0;
            while b.get(j) == Some(&'#') {
                hashes += 1;
                j += 1;
            }
            if b.get(j) == Some(&'"') {
                j += 1;
                loop {
                    if j >= b.len() {
                        break;
                    }
                    if b[j] == '"' && (0..hashes).all(|k| b.get(j + 1 + k) == Some(&'#')) {
                        j += 1 + hashes;
                        break;
                    }
                    j += 1;
                }
                for &ch in &b[i..j.min(b.len())] {
                    out.push(blank(ch));
                }
                i = j;
            } else {
                out.push(c);
                i += 1;
            }
        } else if c == '"' {
            out.push(' ');
            i += 1;
            while i < b.len() && b[i] != '"' {
                if b[i] == '\\' && i + 1 < b.len() {
                    out.push(blank(b[i]));
                    i += 1;
                }
                out.push(blank(b[i]));
                i += 1;
            }
            out.push(' ');
            i += 1;
        } else if c == '\'' {
            // A char literal ('x', '\n', '\u{..}') or a lifetime ('a).
            let is_char = (next == Some('\\')
                && (i + 2..(i + 12).min(b.len())).any(|k| b[k] == '\''))
                || (next.is_some() && b.get(i + 2) == Some(&'\''));
            if is_char {
                let mut j = i + 1;
                if b[j] == '\\' {
                    j += 1;
                }
                while j < b.len() && b[j] != '\'' {
                    j += 1;
                }
                for &ch in &b[i..(j + 1).min(b.len())] {
                    out.push(blank(ch));
                }
                i = j + 1;
            } else {
                out.push(c);
                i += 1;
            }
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

fn prev_is_ident(b: &[char], i: usize) -> bool {
    i > 0 && (b[i - 1].is_alphanumeric() || b[i - 1] == '_')
}

/// Lines (1-based) where code uses `unsafe` as a keyword or names the `unsafe_code` lint.
fn unsafe_lines(code: &str) -> Vec<usize> {
    let mut hits = Vec::new();
    for (n, line) in code.lines().enumerate() {
        let mut rest = line;
        let mut offset = 0;
        while let Some(pos) = rest.find("unsafe") {
            let abs = offset + pos;
            let before = line[..abs].chars().next_back();
            let after = &line[abs + 6..];
            let word_before = before.is_some_and(|c| c.is_alphanumeric() || c == '_');
            let lint = after.starts_with("_code");
            let keyword =
                !word_before && !after.starts_with(|c: char| c.is_alphanumeric() || c == '_') && {
                    let t = after.trim_start();
                    t.is_empty()
                        || ["{", "fn", "impl", "trait", "extern"]
                            .iter()
                            .any(|k| t.starts_with(k))
                };
            if lint || keyword {
                hits.push(n + 1);
                break;
            }
            offset = abs + 6;
            rest = &line[offset..];
        }
    }
    hits
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/")
}

fn allowed(path: &str) -> bool {
    ALLOWED_FILES.contains(&path) || ALLOWED_DIRS.iter().any(|d| path.starts_with(d))
}

#[test]
fn unsafe_appears_only_where_the_policy_allows_it() {
    let root = workspace_root();
    let mut files = Vec::new();
    rust_files(&root.join("crates"), &mut files);
    assert!(files.len() > 100, "found the sources: {}", files.len());
    let mut violations = Vec::new();
    for f in &files {
        let path = rel(&root, f);
        if path == "crates/plugin-host/tests/unsafe_policy.rs" || allowed(&path) {
            continue;
        }
        let code = code_only(&fs::read_to_string(f).unwrap());
        for line in unsafe_lines(&code) {
            violations.push(format!("{path}:{line}"));
        }
    }
    assert!(
        violations.is_empty(),
        "`unsafe` outside the allowed crates: {violations:?}"
    );
}

#[test]
fn every_unsafe_block_in_the_allowed_files_has_a_safety_comment() {
    let root = workspace_root();
    let mut files = Vec::new();
    rust_files(&root.join("crates"), &mut files);
    let mut missing = Vec::new();
    for f in &files {
        let path = rel(&root, f);
        if !allowed(&path) {
            continue;
        }
        let src = fs::read_to_string(f).unwrap();
        let code = code_only(&src);
        let lines: Vec<&str> = src.lines().collect();
        for n in unsafe_lines(&code) {
            let text = code.lines().nth(n - 1).unwrap();
            let needs = text.contains("unsafe {")
                || text.trim_start().starts_with("unsafe impl")
                || text.contains("unsafe extern");
            if !needs {
                continue;
            }
            let from = n.saturating_sub(5);
            let documented = lines[from..n].iter().any(|l| l.contains("SAFETY:"));
            if !documented {
                missing.push(format!("{path}:{n}"));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "unsafe without a `SAFETY:` comment: {missing:?}"
    );
}

#[test]
fn crates_outside_the_allowance_inherit_the_workspace_lints() {
    let root = workspace_root();
    let mut manifests = Vec::new();
    for dir in ["crates", "crates/testplugins"] {
        for entry in fs::read_dir(root.join(dir)).unwrap().flatten() {
            let m = entry.path().join("Cargo.toml");
            if m.is_file() {
                manifests.push(m);
            }
        }
    }
    assert!(manifests.len() >= 16, "{}", manifests.len());
    for m in manifests {
        let path = rel(&root, &m);
        let dir = path.trim_end_matches("Cargo.toml");
        if ALLOWED_DIRS.contains(&dir) {
            continue;
        }
        let text = fs::read_to_string(&m).unwrap();
        assert!(
            text.contains("[lints]") && text.contains("workspace = true"),
            "{path} must inherit the workspace lints"
        );
        assert!(
            !text.contains("unsafe_code"),
            "{path} must not override the unsafe lint"
        );
    }
    let ws = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    assert!(
        ws.contains("unsafe_code = \"deny\""),
        "the workspace denies unsafe code"
    );
}

#[test]
fn the_scanner_ignores_comments_and_strings_and_finds_real_unsafe() {
    let hits = |src: &str| unsafe_lines(&code_only(src));
    assert!(hits("// unsafe { x }\n/* unsafe fn f() */\nlet s = \"unsafe {\";").is_empty());
    assert!(hits("let r = r#\"unsafe { }\"#;").is_empty());
    assert!(hits("fn unsafely() {} let x = 'u';").is_empty());
    assert!(hits("let unsafe_thing = 1;").is_empty());
    assert_eq!(hits("fn f() {\n    unsafe { g() }\n}"), vec![2]);
    assert_eq!(hits("unsafe impl Send for X {}"), vec![1]);
    assert_eq!(hits("pub unsafe fn h() {}"), vec![1]);
    assert_eq!(hits("#![allow(unsafe_code)]"), vec![1]);
    assert_eq!(hits("let c = '\"'; unsafe { y() }"), vec![1]);
    assert_eq!(hits("let l: &'static str = \"a\"; unsafe { y() }"), vec![1]);
}
