# Testing

Atomik Rack is built test-first (see [CONTRIBUTING.md](../CONTRIBUTING.md)). This page explains how to run the checks and what the special tests prove.

## Quality gates

Run these from the repository root. CI runs the same commands; the workflow is in [.github/workflows/ci.yml](../.github/workflows/ci.yml).

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check                     # needs: cargo install cargo-deny
sh scripts/check-public.sh                 # repository hygiene (no local paths, secrets or tool files)
```

- `rust-toolchain.toml` pins Rust 1.99, and `rustup` installs it on first use.
- Lints are set in the root `Cargo.toml` (`unsafe_code = "deny"`, clippy `all = deny`, `float_cmp = deny`) and in [clippy.toml](../clippy.toml) (bans `HashMap`, `HashSet`, `Instant::now` and `SystemTime::now`).
- CI also runs `cargo test --workspace --release` once on Linux, because optimisation can change floating-point behaviour. It also runs `cargo llvm-cov` and fails below 90 % of lines (binary entry points and GPU or audio-device glue are excluded; the exact filter is in the workflow).

Optional coverage report (needs `cargo install cargo-llvm-cov`):

```sh
cargo llvm-cov --workspace --html
```

Useful subsets:

```sh
cargo test -p atomik-engine                 # graph, compile, engine, offline
cargo test -p atomik-mcp                    # the QA server: protocol, tools, headless sessions
cargo test -p atomik-devices                # the built-in devices
cargo test -p atomik-render --test e2e      # end-to-end CLI tests and goldens
```

Tests follow the naming pattern `method_scenario_expectation`. Unit tests live in a `#[cfg(test)] mod tests` in the same file. Cross-crate tests live in `crates/<crate>/tests/*.rs`. Fakes (`ConstantSource`, `ImpulseOnNote`, `Passthrough`, `NoteThru`, `CallLog`, `DropProbe`, `Allocator`, `RecordingSink`) are in [crates/testkit/src/fakes.rs](../crates/testkit/src/fakes.rs).

## Allocation guard (HI-1)

[crates/testkit/src/no_alloc.rs](../crates/testkit/src/no_alloc.rs) provides a counting global allocator. It forwards to the system allocator and counts heap operations per thread while a guarded section is active.

- **Install it once per test binary** with `atomik_testkit::install_alloc_guard!();`. Every integration-test file that renders in-process starts with it, after any `//!` docs. The exception is `crates/render/tests/e2e.rs`, which only runs the CLI as a child process. The unit-test binaries of `atomik-engine`, `atomik-devices` and `atomik-testkit` install it in their `lib.rs`.
- **Guard a section** with `atomik_testkit::no_alloc::assert_no_alloc(|| ...)`. The call panics with `HI-1 violated: N heap operation(s) inside a no-alloc section` if the closure allocated, reallocated or freed memory on this thread. It panics with a different message if the guard was not installed, so a missing guard cannot pass silently.
- The guard panics in test code rather than aborting, so a `#[should_panic]` test can prove that it bites. See `allocating_device_is_caught_by_guard` in [crates/engine/tests/alloc.rs](../crates/engine/tests/alloc.rs) and `assert_no_alloc_catches_allocation` in [crates/testkit/src/lib.rs](../crates/testkit/src/lib.rs).

Where the guard is used:

- **Engine tests.** These render through `render_block` in [crates/engine/tests/common/mod.rs](../crates/engine/tests/common/mod.rs), which wraps every `Engine::render` call.
- **Offline renders.** Use `render_offline_with(.., |f| assert_no_alloc(f))` to guard every inner `render` of a whole offline run. Examples are in [crates/engine/tests/determinism.rs](../crates/engine/tests/determinism.rs) and [crates/render/tests/alloc.rs](../crates/render/tests/alloc.rs).
- **Device tests.** These use `DeviceHarness` ([crates/testkit/src/harness.rs](../crates/testkit/src/harness.rs)), which guards every `process` call. Wrap `set_param` and `reset` yourself (see [EXTENDING.md](EXTENDING.md#5-testing-the-device)).

The guard only sees the current thread, and only code that runs inside the closure. The end-to-end tests in `crates/render/tests/e2e.rs` run the CLI binary as a child process, which has no guard installed. The real demo patch is guarded separately by `render_slice_demo_is_allocation_free`.

## Determinism tests (HI-2)

| Test | File | What it proves |
|---|---|---|
| `render_any_host_block_is_bit_identical` (proptest, 16 cases) | [crates/engine/tests/determinism.rs](../crates/engine/tests/determinism.rs) | A patch with a passthrough chain, fan-out, interface in→out, notes through a note-thru device, params and seeded noise input renders to the same **bit patterns** with host blocks 1, 7, 63, 64, 65, 100, 512 and 4096 |
| `render_note_at_frame_lands_at_frame` (proptest) | [crates/engine/tests/notes.rs](../crates/engine/tests/notes.rs) | A note at frame f produces an impulse at exactly frame f, for any host block |
| `render_offline_note_at_frame_lands_at_frame` (proptest) | [crates/engine/tests/offline.rs](../crates/engine/tests/offline.rs) | The same through `render_offline` |
| `notes_across_ramps_land_on_the_closed_form_frames`, `ramped_notes_do_not_depend_on_the_host_block` | [crates/engine/tests/tempo_ramp.rs](../crates/engine/tests/tempo_ramp.rs) | Notes across tempo ramps land at the frames of the log formula `60 / PPQ * L / (b1 - b0) * ln(b(x) / b0)`, which the test writes out itself (it does not call `atomik_core`), and the same events come out at host blocks 1, 63, 64, 333 and 4096 |
| `clips_start_and_end_at_the_closed_form_frames_across_ramps`, `a_ramped_bounce_is_the_same_at_any_host_block` | [crates/render/tests/tempo_ramp.rs](../crates/render/tests/tempo_ramp.rs) | The same oracle for the audio clips of a bounce, and byte-identical bounces at several host blocks |
| `the_live_path_plays_a_ramped_project_exactly_like_the_bounce` | [crates/app/src/app_core_tempo_tests.rs](../crates/app/src/app_core_tempo_tests.rs) | The drum demo under a ramping tempo map and a changing meter, played live through the app, is the bounce bit for bit at host blocks 64, 480 and 512 |
| `cli_block_size_does_not_change_output` | [crates/render/tests/e2e.rs](../crates/render/tests/e2e.rs) | `--block 1` and `--block 4096` produce byte-identical WAV files (f32), and `--block 100` and `--block 63` do too (24-bit) |
| `process_is_deterministic_for_a_seed` | [crates/devices/src/poly_synth.rs](../crates/devices/src/poly_synth.rs) | The synth's noise and sample & hold are reproducible for a seed |

Property-test regressions are saved in `proptest-regressions/` folders and `*.proptest-regressions` files next to the tests. Commit them.

## Golden files

[crates/render/tests/golden/](../crates/render/tests/golden) holds two goldens of [examples/patches/slice-demo.json](../examples/patches/slice-demo.json), rendered as 32-bit float with the default block size (512):

| File | Checked by | Platforms | Tolerance |
|---|---|---|---|
| `slice-demo.f32.sha256` | `golden_hash_matches` | **Windows x86-64 only** (the test is `#[cfg]`-gated) | exact bytes |
| `slice-demo.fingerprint.json` | `golden_fingerprint_matches` | all | ±0.5 dB per window |

**Why there is a fingerprint.** The devices use transcendental functions (`sin`, `tan`, `tanh`, `powf`, …) from the platform's math library. Those can differ in the last bit between operating systems, so an exact hash is only meaningful on the platform that produced it. The fingerprint is a coarse check that every platform still produces the same music. It stores the RMS level in dB (floored at −120 dB) of every half-beat window (12 000 frames) of each channel. It catches real regressions, such as a silent device, a wrong note or a broken cable, without depending on last-bit equality.

**Regenerating the goldens.** Do this only after an intentional change to the sound, and listen to the new render first. Then run, on Windows x86-64:

```sh
ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-render --test e2e golden
```

This rewrites both files. Use Windows x86-64 because that is the only platform that checks the hash. Review the diff of the fingerprint: large changes in level should be explainable by your change.

## Real-time factor test (ignored by default)

`realtime_factor_release` in [crates/render/tests/e2e.rs](../crates/render/tests/e2e.rs) checks the render speed. It renders `slice-demo.json` with `--stats` and asserts a real-time factor of at least 50×. Wall-clock timing depends on the machine and the build profile, so the test is `#[ignore]` and must run in release:

```sh
cargo test --release -p atomik-render -- --ignored
```

You can also measure by hand:

```sh
cargo run --release -p atomik-render -- examples/patches/slice-demo.json -o demo.wav --stats
```

The file browser has three more timing tests with the same rule (`#[ignore]`, release only):

```sh
cargo test -p atomik-library --release -- --ignored
cargo test -p atomik-ui --release --test file_browser -- --ignored
cargo test -p atomik-audio-file --release -- --ignored stream
```

They check a two-word query over 50 000 rows, filtering and sorting 100 000 rows in the panel, and the latency of the first decoded chunk of a file. The preview voice's allocation test is not ignored: [crates/engine/tests/preview_alloc.rs](../crates/engine/tests/preview_alloc.rs) runs with the normal suite. A normal `cargo test --workspace` reports these ignored tests besides the real-time factor test and the `ignore` code example in the `no_alloc` module docs.

## Listening check

This is a manual check. A person listens to the slice-demo render and confirms there are no objectionable clicks, zipper noise or aliasing. No automated test replaces it.

## Plugin tests (CLAP and VST3)

Third-party plugin hosting is tested against plugins built in this repository, never against plugins installed on the machine. `crates/testplugins/core` holds the DSP of the test plugins (a gain effect with latency, sidechain and tail, a deterministic synth, a note effect, a 2 000-param bank that grows a param at run time), `clap-shell` is a minimal raw `clap-sys` CLAP plugin (so the host is tested against a plugin not written with its own CLAP library), and `clap`, `clap-bad` (init fails), `clap-hang` (init never returns), `clap-crash` (segfaults in `create` or `process` when an environment variable asks for it), `clap-leak` (instance counters) and `clap-variants` (a MIDI-only synth, a mono effect, plugins that fail at activation or return errors or NaN) are the libraries.

- **Building.** The tests build the libraries themselves: `atomik_plugin_host::testing::clap_test_plugin("atomik-test-clap")` runs `cargo build -p ...` (the `CARGO` environment variable, the current `CARGO_TARGET_DIR`) under a lock file in the target directory, so several test binaries running at once do not race, and copies the library to `<target>/<profile>/clap-plugins/*.clap`. A missing or unbuildable plugin fails the test loudly with the package named. The scanner tests also need `atomik-plugin-scan` built (`cargo test -p atomik-plugin-scan` does it).
- **Reference oracle.** The effect and synth tests compare the output of the plugin through the real CLAP backend and the `PluginDevice` with the DSP core run directly, bit for bit, at host blocks 1, 64, 333, 480 and 512 and workers 0, 1, 2, 4, under the allocation guard.
- **Processes.** `plugin-scan/tests/scanner.rs` and `plugin-clap/tests/crash.rs` start real child processes (a scanner that is killed at a timeout, a plugin that segfaults); `hang.rs` proves the in-process loader would block, which is why scanning is out of process.
- **Unsafe policy.** `plugin-host/tests/unsafe_policy.rs` fails if `unsafe` appears outside `plugin-clap`, `plugin-vst3`, the test plugins and the audited `fpenv.rs`, or if a block lacks a `SAFETY:` comment. The app, render and UI tests therefore cannot probe plugin libraries; they go through the manager and the slot's message ring.
- **VST3.** The same DSP core sits behind `vst3-shell` (a plugin shell over the `vst3` crate's plugin-side helpers: factory, `IComponent`, `IAudioProcessor`, `IEditController`, `IMidiMapping`, `IUnitInfo`, note expressions, a headless `IPlugView`, a split component/controller variant) and the libraries `vst3` (the four test plugins, same names as the CLAP ones), `vst3-variants` (a split gain, a mono effect, a 5.1-only effect the host refuses, plugins that fail to initialise, set up, start or process, one that returns NaN), `vst3-bad`, `vst3-hang`, `vst3-crash`, `vst3-leak`. `atomik_plugin_host::testing::vst3_test_plugin("atomik-test-vst3")` builds a library and installs it as a `.vst3` **bundle** with the layout of the running OS. The shell announces a latency change itself (`restartComponent(kLatencyChanged)`), like the CLAP effect asks for a restart, so a test must not announce it by hand.
- **One suite, two formats.** `plugin-vst3/tests/{process,lifecycle,state,engine,leak}.rs` loop over `[Clap, Vst3]` with one `Kit` that maps the CLAP plain values the tests think in to each format's host values, so the host contract is checked against both backends through the same traits; `cross_format.rs` requires the CLAP and the VST3 effect to give the same bits (and a rack mixing both to bounce identically across blocks and workers), `vst3_host.rs` holds the VST3 specifics (proxies, buses, parameter queues, `IMidiMapping`, the state sections, restarts, the editor, edits made in it). `render/tests/plugin_demo_vst3.rs` does the same for a whole project (`examples/projects/plugin-demo-vst3` against its all-CLAP twin).
- **Live equals bounce.** A bounce with plugins runs the plugin loop of `render/src/plugin_pass.rs` (a pre-roll, then a manager poll and a PDC recompile after every 64-frame batch); the twin tests in `app/src/app_core_plugins*_tests.rs` therefore settle the live side first (`plugin_test_support::settle_steps`) and, where a restart is part of the test, step the live engine one batch per frame (`stepped`), the cadence of the bounce. `app_core_plugins_latency_tests.rs` has the twins of a saved latency and of a latency a container macro moves mid-song, in both formats.
- **Real plugins** are only for manual checks (the ignored tests in `app_core_plugins_real_tests.rs` and `plugin-vst3/tests/real_plugin.rs`, driven by the `ATOMIK_REAL_*` environment variables, and `plugin_scan_fallback.rs`, which scans the folders named by `ATOMIK_SCAN_SMOKE`, run against plugins installed on your machine); no golden hash is ever taken over a real plugin.
- **Quick runs.** `cargo test -p atomik-plugin-host`, `-p atomik-plugin-clap`, `-p atomik-plugin-scan`, `-p atomik-plugin-vst3`, `cargo test -p atomik-render --test plugin_demo --test plugin_demo_vst3`, `cargo test -p atomik-app --lib -- plugin stress`.

## Key scripts and the workflow tests

Keys are tested through the registry, never through a window. The OS is a parameter (`Os::Windows`, `Os::Mac`, `Os::Linux`), not `cfg`, so the macOS and Linux key paths run on any host.

- **Router level.** `atomik_ui::keymap::script::KeyScript` (behind the `testkit` feature) drives `Keymap::resolve` with chord text (`down Cmd+D`, `ctx rack`, `expect command edit.duplicate`); the golden listings `crates/ui/tests/keymaps/{win,mac,linux}-{standard,legacy}.txt` pin every bundled binding.
- **View level.** The views take a registry command (`ArrangementView::command`, `PianoRollView::command`, `RackView::command`) and answer `CommandOutcome::{Handled, Declined, NotApplicable}`; `crates/ui/tests/workflow_{arrangement,piano_roll,rack}.rs` assert the actions, the tool state, snap, zoom and follow of each command, and the gestures of every tool with the modifier roles held. `workflow_snapshot.rs` holds the display-list goldens of the toolbars and the create-mode browser (`ATOMIK_UPDATE_GOLDEN=1` regenerates them).
- **Shell level.** `crates/app/src/shell_workflow_tests.rs` builds a headless `Shell` (a `NullBackend` session), presses chords with `event_for(os, "Ctrl+Return")` and asserts documents, transport position, layout, focus and the status line: the transport and numpad keys, the tools, zoom and follow, the pane keys and the Mac secondaries, the create keys, remaps that shadow or unbind a chord, the legacy profile, the one-time notice. Tests that touch `workflow.json` use their own temporary settings folder, never the real configuration directory.

The keymap table test `every_legacy_key_is_unchanged_in_standard_or_a_listed_move` checks every legacy binding against the standard profile (a reviewed constant lists the moved keys), and a shell test binds the commands that are not handled and asserts they change nothing (`auto_route_does_nothing_without_a_selected_device_or_in_a_time_view`).

The clipboard, rack selection and context menus add three layers on the same pattern. `crates/ui/tests/clip_multiselect.rs` (Shift and Ctrl clicks across tracks, the rubber band, a lane drag, a Ctrl copy drag and a group drag applied to a real `Document` so the undo steps are counted), `crates/model/tests/move_clips.rs` (`MoveClips`: lane rule, copy ids, rejections, one merged step for a group drag), `crates/ui/tests/workflow_selection.rs`, `workflow_arrangement_selection.rs` and the selection tests in `workflow_piano_roll.rs` drive the views with pointer events and commands (toggle and range roles per OS, select all, the neighbour keys, delete, duplicate, join, the group drag, the context-menu requests). `crates/app/src/app_core_clipboard_tests.rs` runs the clipboard in the app core with the recording sink (every paste is one undo step and compared as saved text; the clipboard survives New and Open; a cross-song paste registers its assets). `crates/app/src/shell_clipboard_tests.rs` presses the clipboard chords and right-clicks in a headless `Shell` on Windows and macOS, checks the menu items, their enabling and the derived shortcut text, runs a seeded random gesture script (no refused command, undo returns the empty rack) and holds the display-list goldens of the menus (`rack_context_menu_*`, `arrangement_context_menu_*` in `crates/app/tests/golden`, `rack_selection_*` and `rack_group_drag` in `crates/ui/tests/golden`).

The creation chain has `crates/model/tests/workflow_create.rs` (the rules table: instrument with channel, track and arming, no track for an audio-only device, no channel for a silent one, the options, effects onto a channel, a return, a device or nothing, the master seam, auto-route, disconnect, delete bridging; one undo step and byte-identical redo each), `workflow_create_props.rs` (random create, auto-route, disconnect, delete, undo and redo sequences compared as saved text), `crates/ui/tests/workflow_create.rs` (the commands the browser and the rack emit, Shift and Alt), `crates/app/src/app_core_create_tests.rs` (arming and selection in the core, the auto-armed list, live equals bounce for a created project) and `shell_create_tests.rs` (key scripts on three operating systems, the Workflow setting, the menu item).

The master section and Audio I/O are covered in the app by `crates/app/src/app_core_starter_tests.rs` (File, New is the starter and Open never creates a master; the status text after a create with the master selected; the host channel counts reaching the lamps; the Audio I/O meter feed on a recording sink and on a real offline engine, including the clip latch and the console not losing the peaks the feed read; the track input cables; live equals bounce for the starter with an instrument and a send effect, and with the meter feed running every block; the example project opens as it is), `shell_console_tests.rs` (the one-time notice, File, New through the shell), `shell_create_tests.rs` (a new project as key scripts on three operating systems, the ninth effect), `cli.rs` (the start document) and `crates/app/tests/smoke.rs` (the app starts with the starter project and paints it); in `atomik-render`, `crates/render/tests/master_demo.rs` generates and checks `examples/projects/master-demo` (regenerate with `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-render --test master_demo regenerate`).

## Markers and ranges

`crates/model/tests/markers.rs` covers the commands (sorting, validation, undo, drag merging), project format 14 (canonical save, version 13 files, JSON-path errors) and the previous and next marker helpers. `crates/ui/tests/markers.rs` drives the arrangement with pointer events and commands (adding, dragging, renaming in place, ranges, the loop from a range, the context-menu commands, the list) and holds the pure edits; `cargo test -p atomik-ui --test markers -- --ignored` writes a preview PNG to `target/preview`. `crates/app/src/shell_marker_tests.rs` presses the keys in a headless `Shell` (the `Text` context while a name is typed, the View toggles and the saved view state, the menus), and `app_core_marker_tests.rs` checks that a marker edit is one undo step and that the recording sink hears nothing of it.

## Deep zoom and vertical zoom

`crates/audio-file` tests `raw_min_max` against the peak buckets over the same frames (and the edges: before 0, past the end, NaN). In `atomik-ui`, `timeline.rs` holds the grid tests at extreme zooms (a readable step at every zoom from the deepest to far out, no zero step or runaway line count for zero, negative, NaN and infinite zooms, thinned lines for a range far too long, the divisor chain of the steps) and the snap tests (the exact tick steps of the triplets, triplet rounding that restarts at bar lines and follows a meter change, Grid following the visible grid); `wave_paint.rs` tests which source a zoom picks, that sample columns equal pyramid buckets where both are exact, the line mode threshold and `line_frames`; `crates/ui/tests/deep_zoom.rs` paints the arrangement, the sample editor and the slice editor at deep zoom (the line and the dots, no line without samples, the paint cost bounded across zooms, samples placed at their real time); `crates/ui/tests/vertical_zoom.rs` covers row heights and the zoom limits, the header edge drag (one merged undo step, the zoom, the limits, Escape, double-click), Alt+wheel anchoring, the commands, clips and buttons hit at heights from 24 to 400 px, painting at both extremes and the piano roll's key zoom (anchoring, notes, keys and the pencil at every key height). `crates/model/tests/track_heights.rs` covers the commands, undo, drag merging, validation and project format 15; the app tests (`shell_workflow_tests.rs`, `ui_state.rs`, `app_core_audio_tests.rs`) check the keys, the saved zooms and that RAM assets, and only those, hand their samples to the views.

## Scales, chords and fold

`crates/model/src/theory.rs` tests every scale's intervals, the modes as rotations of Major, nearest-key rounding with its ties at both ends of the keyboard, degree stepping across octaves and at keys 0 and 127, `degrees_between` as the inverse of `step`, every chord and inversion, the diatonic chords of C major and A minor, and detection. `crates/model/tests/clip_scale.rs` covers `SetClipScale` (undo, chromatic as none, note clips only, split, duplicate, join) and project format 16 (canonical round trip, every kind, version 15 files, JSON-path errors). In `atomik-ui`, `key_rows.rs` and `chord_edit.rs` hold the row mapping and the pure edits with exact ticks (chordify, every arpeggio pattern with octaves, gates and triplet steps, the pinned random sequence, strum up and down), `context_menu.rs` tests submenus (hover, click, nesting, the side they open on, the release of the opening press), and `crates/ui/tests/piano_roll_scales.rs` drives the roll through a real `Document`: the scale commands, the scale bar, hit testing, drawing, dragging, band select, audition and the key zoom in both folds, snap to scale for the pencil, drags and transposes, conform, the chord stamp (audition, drag to length, Escape, merging with existing notes), and chordify, arpeggiate and strum with undo as one step; `seq_snapshot.rs` holds the display-list goldens of a scale and the two folds (`cargo test -p atomik-ui --test seq_preview -- --ignored` writes PNGs). In `atomik-app`, `context_menus.rs` checks that every item of the scale and chord menus is a registry command with an argument it takes, and `shell_scale_tests.rs` clicks through the menus and the scale bar in a headless `Shell`, presses `Shift+S`, `Shift+F` and `Shift+C`, checks the saved window state and holds the golden of the notes menu with its submenus open.

## The Character effect in the app and the bounce

`crates/app/src/app_core_character_tests.rs` holds the app half of the Character tests: the four meter values reaching the view (recording sink and a real offline engine, the inner Character of a container by path key), the tiered latency published by the real engine (no epoch bump inside a class, one at the end of a fade across the classes), the routing recompile on a class change, and the live twins of the `character-demo` bounce (host blocks 64, 480 and 512; a preset change across the classes made through the app; undo, redo and an engine restart; a class change driven by a song lane, stepped one batch per frame like the bounce loop). A live twin must start from a cold engine: devices keep free-running state, so an engine that rendered idle frames before Play no longer equals a bounce bit for bit. In `atomik-render`, `tests/character_demo.rs` generates and checks the example project (`ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-render --test character_demo regenerate`; determinism across host blocks 64, 333, 480, 512 and workers 0, 2, 3) and `tests/character_pdc.rs` aligns identical clicks through a Character in both latency classes and across a class change in the middle of the song (the regression test of the bounce that did not follow the engine's latency epoch). `--stress-character` is tested in `stress.rs` (structure, preset cycle, class changes under load, determinism, combination with the other loads) and `cli.rs`.

## QA with the MCP server

[MCP.md](MCP.md) describes `atomik-mcp`, a development server that gives an MCP client the checks a person does by hand: bounce a project or patch and compare it with an earlier render, measure a WAV (levels, silence, loudness windows, spectrum), and drive a headless `Shell` (menu commands, pointer, keys, the texts the UI draws, PNG screenshots, undo, save, a bounce of the open document). Use it for the QA of a change before review, and to keep the evidence; it does not replace the tests of this page, and a behaviour it shows is worth a headless test.

`crates/mcp/tests` tests the server itself: `protocol.rs` (the handshake, the tool list, the JSON-RPC errors, the real binary over stdio), `stateless.rs` (every stateless tool against the example projects: bounces bit-identical across host blocks 64, 333 and 512 and worker counts 0 and 2, a changed patch caught against its reference, the plugin refusal, the path policy for reads, writes and `..` through a folder that does not exist yet) and `session.rs` (every session tool against a real headless shell: commands and their toggles, undo by command and by chord, the discard confirmation, the dialog requests, pointer and key input, texts at two scales, screenshots, save and read back, and a session bounce that equals the bounce of the same file and hears an edit). The analysis maths (levels, silence, windows, spectrum, comparison) has unit tests in `crates/mcp/src/analysis.rs`.

## Relinking missing audio

`crates/model/tests/relink.rs` covers `Command::RelinkAsset` (every reference kind swapped, a container's inner sampler included, one undo step with undo and redo restoring the document byte for byte, the sync ops, every refusal, a replacement too short for zones or a clip offset, a saved and reloaded project) with a property test over random relink, undo and redo runs. The app side is `crates/app/src/app_core_relink_tests.rs` (a different file replaces the asset with the clips loading, a non-silent bounce that is the same on every run, save and reopen without missing files, undo bringing the missing list back and redo clearing it, the same audio restoring the file with no document change and no undo step, a folder search matching by name in any case through nested folders and leaving the rest, a file that does not decode reported by name, the sampler editor's missing flag following relink, undo and redo), `audio_relink.rs` (the bounded, deterministic scan) and `shell_relink_tests.rs` (the menu item enabled only while audio is missing, the panel, LOCATE... and SEARCH FOLDER... as platform requests, Esc). `crates/ui/tests/relink_panel.rs` covers the panel's paint, its buttons, scrolling and a small window, and `crates/mcp/tests/session.rs` drives `session_relink` (argument validation, path policy, the end-to-end flow with a bounce, asset choice by name, a folder search).

## MIDI files

Each layer has its tests. `crates/midi-io/src/smf_tests.rs` reads hand-made files (running status, velocity 0, every channel message, meta events, SysEx and unknown meta skipped, RIFF-wrapped files, truncated chunks), refuses the malformed ones with the right error (type 2, SMPTE, no header, no track, a data byte with no status, a five-byte delta) and runs property tests that feed random bytes, random track bodies and mutated or truncated valid files to the reader (it must never panic) and write-then-read random files. `crates/app/src/midi_file_tests.rs` holds the conversion: a clip and a whole song (tempo steps, meter, markers, controllers of every kind, off-grid ticks, a clip length that is not a bar) exported and imported back equal, loops and muted notes in the export, overlapping notes of one key, a ramp as a staircase, a linear controller sampled, the type 0 split by channel with a different division, the note repairs (retrigger, zero length, hanging note, stray note-off), controller clean-up, tempo, meter and marker mapping, files with no notes, SMPTE and clamped huge ticks. `crates/model/tests/midi_import.rs` covers `ImportMidi` (one undo step, unarmed tracks and names, tempo, meter and markers, the song end, every rejection leaving the document alone). `crates/app/src/app_core_midi_tests.rs` and `shell_midi_tests.rs` run the commands on a headless core and `Shell`: what an export picks, a song written to a file and imported into a new project, one clip on a selected track, new tracks at the playhead, bad, truncated and corrupted files (the document stays valid), the menu entries and their enabled flags, the dialog requests and a file dropped on the window. The QA server's `session_midi` has a test in `crates/mcp/tests/session.rs`.

Hand checks, not automated (they need another program or a real window):

- Export a song from the app, open the `.mid` in another DAW or a notation program and check the tempo changes, time signatures, markers, track names and the notes; then import a file made there (and a type 0 file with drums on channel 10) and listen to it.
- Drag a `.mid` from the operating system's file manager onto the arrangement and onto the rack, and check the hover and the new tracks.
- Import a long, dense file (a few hundred thousand events) and check that the window stays responsive and that undo removes it in one step.
- Try File > Import MIDI with a note track selected and with nothing selected, and the three Export MIDI entries with and without a selection, in the in-window menu and, on macOS, the native one.

## Track organisation

Each layer has its tests. `crates/model/tests/organise.rs` runs the eleven commands through a `Document`: grouping gathers the tracks at the first one and is one undo step, nesting and its depth limit, ungrouping into the parent, every refusal changing nothing, `MoveTrack` and `MoveGroup` (a track passing its neighbour, leaving a folder at its edge and entering one, a folder jumping a track or a whole folder, a single track leaving its folder taking the folder with it, the quiet ends of the list, `can_move_track` and `can_move_group` agreeing with the commands), colours and comments (limits, clearing, undo), folded lanes and open take rows, a removed track keeping its entries for undo but not in the file, a document that breaks the one-block invariant read defensively, batching colours and folds into one step, and project format 22 (canonical round trip, a committed golden `tests/golden/organised_project.json`, a version 21 file loading unchanged, every new field unknown before 22, bad folders, parents, loops, empty folders, split blocks and comments refused with their JSON path, and save and load after each step of a command sequence). `crates/render/tests/organise.rs` bounces a song before and after folders, colours, comments, folds and moves, at any block size and worker count and through a saved project, and checks that muting a folder's tracks in one batch is muting each track. `crates/ui/tests/organise.rs` drives the arrangement (shift-click selection and Group Tracks from headers or clips, the folder row's place, collapsing and the rows that leave the layout, double-clicks, the buttons as one batch, typing a name and a comment in place, the tooltip, colours, the lane chevron and the strip, fold lanes, take rows kept in the song and following undo, the move keys, Ungroup and Delete on a folder, the context menu request, the menu state, every height preset including Fit at three zooms) with a display-list golden (`arrangement_folders_1x.json`) and an ignored PNG preview (`cargo test -p atomik-ui --test organise preview_png -- --ignored`, written to `target/preview/`). `crates/app/src/shell_organise_tests.rs` covers the Track menu and its state, the keys, the typed edits through the `Text` context, the header and folder row context menus (choosing a colour and a height), the presets and a saved and reopened project; `context_menus.rs` and `commands.rs` check the menu specs. The MCP tests cover the inspection and a folder driven by the pointer, commands and keys.

Hand checks, not automated (they need a real window):

- Group a few tracks, collapse and expand the folder, rename it, drag the window narrower and check the folder row and the headers at the default, the smallest and the largest track heights, in the dark and light themes.
- Shift-click and ctrl-click headers, then try Group Tracks, the move keys (Alt+Up and Alt+Down are free on your OS and keyboard layout) and the Track menu, in the in-window menu and, on macOS, the native one.
- Type a long folder name and a long comment, press Escape and Enter, click away while typing, and hover the note mark of a comment near the right edge of the window.
- Fold and unfold automation lanes, record automation into a folded lane, and open and close take rows, saving and reopening the project in between.
- Open a large project with many tracks and a few folders and check that scrolling and the Fit preset stay responsive.

## Hardware and manual tests

Tests that need real hardware or a plugin installed on your machine are `#[ignore]`d and never run in CI. Run them by hand, one binary at a time, on a machine where playing sound is acceptable:

- `crates/audio-io/tests/hw_loopback.rs`, `crates/play/tests/hw_cable.rs` and `crates/app/tests/hw_e5.rs` play and record through a virtual audio cable (a loopback device whose endpoints are named `CABLE Input` and `CABLE Output`); they refuse to open any other device. Run them with `cargo test -p <crate> --test <name> -- --ignored --nocapture`.
- `crates/app/src/app_core_plugins_real_tests.rs` and `crates/plugin-vst3/tests/real_plugin.rs` load a real third-party plugin named by `ATOMIK_REAL_VST3` (and the other `ATOMIK_REAL_*` variables documented at the top of each file).
- Listening to a render for clicks, zipper noise and aliasing is a manual check; no automated test replaces it.

A normal `cargo test --workspace` reports these as ignored.

The audio tests use the same guard as the engine tests: every engine render in `audio_stream.rs`, `record_tap.rs` and `audio_bounce_alloc.rs` runs under `assert_no_alloc`, and the tests with workers also install `forbid_on_this_thread` so that an allocation on a worker thread is counted. The worker counter is process-global, so tests that use it live in their own test binary (`audio_bounce_alloc.rs`, `audio_stream_workers.rs`).

MIDI controller data has a test at each layer. `crates/model/tests/controllers.rs` covers `SetClipControllers` (sorting, empty lanes, validation, undo, a drag as one merge group, note clips only), split, duplicate and join, `flatten_controllers` (offsets, loops, hiding, a cut ramp) and project format 19 (byte-exact round trip, version 18 files without lanes, `controllers` before 19, bad lanes with their JSON path). `crates/engine/tests/song_controllers.rs` (under the counting allocator) plays lanes against a note log: frames, ramps, the order against notes at one tick, the chase on a seek and a loop wrap, the reset on stop, compile-time refusals and independence of the host block size. `crates/devices/src/poly_synth.rs` checks the bend range and the mod wheel by frequency. `crates/app/src/record_tests.rs` and `app_core_song_tests.rs` record bends and CCs through a scripted transport (laps, bursts, only-controller takes, the engine's song input). `crates/ui/tests/piano_roll_controllers.rs` drives the area with the pointer through a real `Document` (chips, draw, drag, delete, curve, cancel, snap, painted text) and `seq_snapshot.rs` keeps a golden of it. `crates/mcp/tests/stateless.rs` checks the inspection.

Automation recording has a test at each layer. `crates/model/tests/automation_rec.rs` covers the write modes, the pure `PassRecorder` and `thin_points` (a ramp, a trapezoid, a sine within the tolerance, step points), the `SetLaneWriteMode` command (undo, rejections), a take as one batch step, and project format 18 (byte-exact round trip, version 17 files on Touch, `write` before 18, bad names, a note lane). `crates/engine/tests/song_automation_hold.rs` (under the counting allocator) holds and releases lanes while playing. `crates/app/src/app_core_autorec_tests.rs` runs takes through the core with the recording sink and a scripted transport (the three modes, the hold calls, loop passes, thinning, one undo step for several lanes, cancel on an engine restart) and once on a real offline engine. `crates/ui/tests/automation_write.rs` drives the lane header buttons, and `crates/mcp/tests/session.rs` clicks them through the MCP session.

Takes and comping have a test at each layer. `crates/model/src/takes_tests.rs` holds the pure comp rules (cutting a range out of a comp, joining touching segments, the layered comp of a fresh group, what deleting a take hands on, the comp check, the compiled note and audio clips with their source offsets and fades). `crates/model/tests/takes.rs` runs the five commands through a `Document` (each one undo step with byte-exact redo, every refusal changing nothing, what the engine's flatten sees, the audio regions of a comp, referenced assets and relink, project format 20 with a loader test for a version 19 file and every bad group named by its JSON path) with a property test over random take edits (undo, redo and a save round trip of every state). `crates/app/src/app_core_song_tests.rs` and `app_core_rec_audio_tests.rs` record loop passes through the core (MIDI and audio) into one group and one undo step. `crates/ui/tests/takes.rs` drives the strip and the take rows with the pointer through a real document (open and close, pick, comp drag, Escape, delete, flatten, painted names) and keeps a display-list golden (`arrangement_takes_1x.json`; `cargo test -p atomik-ui --test takes -- --ignored` writes a preview PNG). `crates/render/tests/takes.rs` bounces an audio comp (the tone of each segment, sample-exact inside a segment, no click at the splices where the comp fades while the same cuts as plain clips jump) and a note comp (the pitch of each bar), and checks that a comp bounces like its flattened group, after undo, after a save and load, and at any host block and worker count. `crates/mcp/tests/stateless.rs` checks the inspection.

Quantize and grooves have a test at each layer. The unit tests in `crates/model/src/groove.rs` pin the arithmetic: snapping to the nearest target, strength 0 and 0.5, idempotence, the 50 to 75 % swing scale, a note captured by the nearest *swung* position, the song's grid under a clip that starts off the grid, groove offsets and velocity, a groove setting the grid, selected notes only, snapped ends, `(tick, key)` collisions that keep both notes, humanize that is bounded, deterministic, independent of the other notes and inside the tick range, extracting a groove, the built-in grooves, validation messages, and the splitmix hash against the DSP crate's pinned value. `crates/model/tests/groove.rs` runs the commands through a `Document` (one undo step over several clips with byte-exact redo, what the engine's flatten plays, `QuantizeNotes`, every refusal changing nothing, humanize across a save and load, the library undo and re-apply by name, the 64-groove limit, `ExtractGroove` and applying the extracted groove to another track, the clips a take group compiled left alone) and project format 21 (round trip, canonical text, a version 20 file loads without grooves and refuses the field, bad grooves refused with their JSON path). `crates/ui/tests/quantize_panel.rs` drives the panel (steps and ranges, the wheel, the groove picker, the dimmed grid, APPLY and EXTRACT needing a target, the library list scrolling and deleting, the settings becoming valid parameters) with a display-list golden (`quantize_panel_1x.json`) and an ignored PNG preview (`cargo test -p atomik-ui --test quantize_panel preview_png -- --ignored`, written to `target/preview/`). `crates/app/src/shell_quantize_tests.rs` covers the menu item, `Ctrl+K` in the arrangement and in the roll and the roll's QUANT and `...` buttons (the panel's last settings applied without opening it, one undo step, a collision keeping both notes), the panel opening and closing, settings kept between openings, APPLY on selected clips and on selected notes of the roll as one undo step, EXTRACT choosing the new groove and DELETE; the MCP tests cover the inspection and the panel through `session_command`; `crates/render/tests/groove.rs` bounces swung, grooved and humanized songs (closed-form onset frames, the same bytes as hand-placed notes, any block and any worker count).
