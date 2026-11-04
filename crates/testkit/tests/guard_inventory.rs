//! Which integration-test files render audio in-process without the allocation guard.
//!
//! Every file of `crates/*/tests/*.rs` that calls a render entry point (see [`RENDER_CALLS`]) must
//! install the guard (`atomik_testkit::install_alloc_guard!`), and bounce through
//! `render_guarded` (in `crates/render/tests/common`) or an `assert_no_alloc` section, or it must
//! be on [`WITHOUT_GUARD`] with the reason it cannot yet. The list only shrinks: the test fails
//! when a listed file gains the guard, no longer renders, or is gone, and when a new file renders
//! without the guard and without an entry. See `docs/TESTING.md`, "Allocation guard".

use std::fs;
use std::path::{Path, PathBuf};

/// Names whose presence in a test file means that it renders audio in-process.
const RENDER_CALLS: &[&str] = &[
    "render_offline",
    "Engine::render",
    "render_doc",
    "render_project",
    "render_song",
    "render_core",
    "export_stems",
    "render_freeze",
    "\"bounce\"",
    "session_bounce",
];

/// Why a file cannot take the guard yet.
mod why {
    /// A QA-server test: the bounce runs behind the session or tool API.
    pub const SERVER: &str =
        "drives the QA server: its bounce runs behind the tool API, which the test cannot wrap";
    /// Plugins are third-party code.
    pub const PLUGIN: &str = "loads real plugins: what a plugin allocates is outside the contract";
    /// A bounce through an entry point the seam is not wired into.
    pub const ENTRY: &str = "bounces through an entry point (project file or JSON, the CLI, \
        freeze, stems, export) that `with_render_guard` does not wrap yet";
    /// A device demo that goes through the slower bounce paths.
    pub const DEMO: &str = "renders a demo project through `render_project_*`; to move to \
        `render_guarded` when the project helpers take a document";
    /// Not an audio render at all.
    pub const PREVIEW: &str = "its own `render_doc` draws a pixel preview, not audio";
}

/// Test files, as `crate/file.rs`, that render without the allocation guard, with the reason.
const WITHOUT_GUARD: &[(&str, &str)] = &[
    ("mcp/macros.rs", why::SERVER),
    ("mcp/midi.rs", why::SERVER),
    ("mcp/protocol.rs", why::SERVER),
    ("mcp/session.rs", why::SERVER),
    ("mcp/stateless.rs", why::SERVER),
    ("render/audio_demo.rs", why::DEMO),
    ("render/audio_editing.rs", why::ENTRY),
    ("render/audio_nan.rs", why::ENTRY),
    ("render/audio_project.rs", why::ENTRY),
    ("render/character_demo.rs", why::DEMO),
    ("render/character_qa.rs", why::ENTRY),
    ("render/container_demo.rs", why::DEMO),
    ("render/determinism_examples.rs", why::ENTRY),
    ("render/device_patch_bounce.rs", why::ENTRY),
    ("render/drum_demo.rs", why::DEMO),
    ("render/export.rs", why::ENTRY),
    ("render/format_corpus_bounce.rs", why::ENTRY),
    ("render/freeze.rs", why::ENTRY),
    ("render/fx_demo.rs", why::DEMO),
    ("render/master_demo.rs", why::DEMO),
    ("render/meter_probe.rs", why::ENTRY),
    ("render/midi_out_bounce.rs", why::ENTRY),
    ("render/old_projects_4m.rs", why::ENTRY),
    ("render/player_demo.rs", why::DEMO),
    ("render/plugin_demo.rs", why::PLUGIN),
    ("render/plugin_demo_vst3.rs", why::PLUGIN),
    ("render/project.rs", why::ENTRY),
    ("render/qa_audio_workers.rs", why::ENTRY),
    ("render/sampler_demo.rs", why::DEMO),
    ("render/slicer_demo.rs", why::DEMO),
    ("render/slicer_roundtrip.rs", why::ENTRY),
    ("render/stems.rs", why::ENTRY),
    ("ui/console_preview.rs", why::PREVIEW),
];

const GUARD: &str = "install_alloc_guard!";

fn crates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates")
}

/// Every `crates/<crate>/tests/<file>.rs` as `crate/file.rs`, with its source, sorted.
fn test_files() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for krate in fs::read_dir(crates_dir()).unwrap().flatten() {
        let tests = krate.path().join("tests");
        let Ok(entries) = fs::read_dir(&tests) else {
            continue;
        };
        for file in entries.flatten().map(|e| e.path()) {
            if file.extension().is_some_and(|e| e == "rs") {
                let name = format!(
                    "{}/{}",
                    krate.file_name().to_string_lossy(),
                    file.file_name().unwrap().to_string_lossy()
                );
                out.push((name, fs::read_to_string(&file).unwrap()));
            }
        }
    }
    out.sort();
    out
}

fn renders(src: &str) -> bool {
    RENDER_CALLS.iter().any(|call| src.contains(call))
}

#[test]
fn every_in_process_render_test_has_the_guard_or_a_reason() {
    let listed: Vec<&str> = WITHOUT_GUARD.iter().map(|(f, _)| *f).collect();
    let files = test_files();
    let unguarded: Vec<&str> = files
        .iter()
        .filter(|(_, src)| renders(src) && !src.contains(GUARD))
        .map(|(name, _)| name.as_str())
        .collect();
    let new: Vec<_> = unguarded.iter().filter(|f| !listed.contains(f)).collect();
    assert!(
        new.is_empty(),
        "these test files render without the allocation guard: install it \
         (`atomik_testkit::install_alloc_guard!();`, bounce through `render_guarded`), or list \
         them in WITHOUT_GUARD with the reason: {new:?}"
    );
    let stale: Vec<_> = listed.iter().filter(|f| !unguarded.contains(f)).collect();
    assert!(
        stale.is_empty(),
        "these files are on WITHOUT_GUARD but now have the guard, no longer render or are gone: \
         remove them: {stale:?}"
    );
}

#[test]
fn the_list_is_sorted_and_every_entry_gives_a_reason() {
    let names: Vec<&str> = WITHOUT_GUARD.iter().map(|(f, _)| *f).collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        names, sorted,
        "keep WITHOUT_GUARD sorted, one entry per file"
    );
    for (file, reason) in WITHOUT_GUARD {
        assert!(reason.len() > 20, "{file}: give a reason");
    }
}

#[test]
fn a_guarded_file_that_renders_stays_guarded_by_a_real_section() {
    // Installing the guard proves nothing unless something runs under it: a guarded render
    // test must also open a no-alloc section (directly, through `render_guarded`, or the
    // device harness, which guards every `process`).
    let sections = [
        "assert_no_alloc",
        "render_guarded",
        "DeviceHarness",
        "render_block",
        "render_offline_with",
        "peak_heap_of",
        "forbid_on_this_thread",
    ];
    let mut idle = Vec::new();
    for (name, src) in test_files() {
        let crate_name = name.split('/').next().unwrap();
        // The guard-only files of the engine and the devices render through their harnesses.
        if matches!(crate_name, "render")
            && renders(&src)
            && src.contains(GUARD)
            && !sections.iter().any(|s| src.contains(s))
        {
            idle.push(name);
        }
    }
    assert!(
        idle.is_empty(),
        "these render tests install the guard but never open a no-alloc section: {idle:?}"
    );
}

#[test]
fn the_scan_notices_a_render_call_without_the_guard() {
    // The mechanism itself: a file with a render call and no guard is reported.
    let unguarded = "fn t() { render_doc(&d, &r, &o); }";
    let guarded = "atomik_testkit::install_alloc_guard!();\nfn t() { render_doc(&d); }";
    assert!(renders(unguarded) && !unguarded.contains(GUARD));
    assert!(renders(guarded) && guarded.contains(GUARD));
    assert!(!renders("fn t() { add(1, 2); }"));
}
