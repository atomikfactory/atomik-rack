//! Which `atomik-render` and `atomik-mcp` integration-test files install the allocation guard.
//! Most do not (see `docs/TESTING.md`, "Allocation guard"): a render test that does not install it
//! cannot catch an allocation in a render path. The files below are the known gaps; the test fails
//! when a file gains the guard (remove it from the list) or a new file has none (add the guard, or
//! list it with the reason).

use std::fs;
use std::path::Path;

/// Test files, as `crate/file.rs`, that render without the allocation guard.
const WITHOUT_GUARD: &[&str] = &[
    "render/ac_u2.rs",
    "render/audio_demo.rs",
    "render/audio_editing.rs",
    "render/audio_nan.rs",
    "render/audio_project.rs",
    "render/automation_only.rs",
    "render/automation_shown.rs",
    "render/character_demo.rs",
    "render/character_pdc.rs",
    "render/character_qa.rs",
    "render/combine.rs",
    "render/console.rs",
    "render/container_demo.rs",
    "render/controller_lanes.rs",
    "render/device_patch_bounce.rs",
    "render/drum_demo.rs",
    "render/e2e.rs",
    "render/export.rs",
    "render/freeze.rs",
    "render/fx_demo.rs",
    "render/fx_pdc.rs",
    "render/groove.rs",
    "render/macros.rs",
    "render/master_demo.rs",
    "render/meter_probe.rs",
    "render/midi_maps.rs",
    "render/midi_out_bounce.rs",
    "render/note_lanes.rs",
    "render/note_lanes_chase.rs",
    "render/note_mute.rs",
    "render/note_tails.rs",
    "render/note_tools.rs",
    "render/old_projects_4m.rs",
    "render/organise.rs",
    "render/player_demo.rs",
    "render/player_stack.rs",
    "render/plugin_demo.rs",
    "render/plugin_demo_vst3.rs",
    "render/plugin_sandbox_bounce.rs",
    "render/project.rs",
    "render/qa_audio_workers.rs",
    "render/sampler_demo.rs",
    "render/slicer_demo.rs",
    "render/slicer_roundtrip.rs",
    "render/snapshots.rs",
    "render/takes.rs",
    "render/tempo_ramp.rs",
    "render/vca.rs",
    "render/warp.rs",
    "mcp/library.rs",
    "mcp/macros.rs",
    "mcp/midi.rs",
    "mcp/protocol.rs",
    "mcp/session.rs",
    "mcp/session_library.rs",
    "mcp/session_patches.rs",
    "mcp/session_windows.rs",
    "mcp/song_end.rs",
    "mcp/stateless.rs",
];

#[test]
fn the_render_tests_without_the_allocation_guard_are_the_listed_ones() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates");
    let mut without = Vec::new();
    for krate in ["render", "mcp"] {
        let mut files: Vec<_> = fs::read_dir(root.join(krate).join("tests"))
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "rs"))
            .collect();
        files.sort();
        for file in files {
            let src = fs::read_to_string(&file).unwrap();
            if !src.contains("install_alloc_guard!") {
                let name = file.file_name().unwrap().to_string_lossy().into_owned();
                without.push(format!("{krate}/{name}"));
            }
        }
    }
    let listed: Vec<String> = WITHOUT_GUARD.iter().map(|s| (*s).to_owned()).collect();
    assert_eq!(
        without, listed,
        "the files without the allocation guard changed: update WITHOUT_GUARD (and \
         docs/TESTING.md if the picture changed)"
    );
}
