//! The render crate's integration tests, folded into one test executable so a change in a
//! low-level crate links one test binary instead of eighty. One file is one module: run its tests
//! with `cargo test -p atomik-render --test suite fx_demo::`, one test with
//! `cargo test -p atomik-render --test suite <test_name>`.
//!
//! Two files stay their own executable, directly in `tests/`:
//! - `e2e.rs`: runs the CLI as a child process, and holds the ignored real-time-factor check
//!   (`perf.yml` runs it alone in release: a speed measurement must not share cores with 600
//!   other tests);
//! - `plugin_sandbox_bounce.rs`: starts plugin processes through the CLI and makes a plugin start
//!   late on purpose (`ATOMIK_TEST_START_MS`), so it keeps a machine to itself.
//!
//! The allocation guard is installed once here. It counts per thread, so a test that is not
//! under `assert_no_alloc` is unaffected, and no test in this suite reads a process-wide
//! counter (`worker_violations`, `allocations_of`): `alloc.rs` and `render_guard.rs` fold like
//! the rest. Every temporary folder a test makes carries a prefix of its own file, so no two
//! files share a name.
atomik_testkit::install_alloc_guard!();

mod common;

mod ac_u2;
mod alloc;
mod audio_demo;
mod audio_editing;
mod audio_nan;
mod audio_project;
mod automation_only;
mod automation_shown;
mod character_demo;
mod character_pdc;
mod character_qa;
mod clip_param_lanes;
mod clipper;
mod combine;
mod console;
mod container_demo;
mod container_modulation;
mod controller_lanes;
mod cv_utilities;
mod delivery;
mod determinism_examples;
mod device_input;
mod device_patch_bounce;
mod drum_demo;
mod drum_designer;
mod duck_from;
mod dynamic_eq;
mod effects_expansion;
mod effects_reverb;
mod energy;
mod export;
mod feedback;
mod fm_demo;
mod format_corpus_bounce;
mod freeze;
mod fx_demo;
mod fx_pdc;
mod granular_demo;
mod groove;
mod guitar_amp_demo;
mod macros;
mod masking;
mod master_demo;
mod master_tune;
mod meter_probe;
mod midi_maps;
mod midi_out_bounce;
mod mix_channels;
mod mixcheck;
mod mod_matrix;
mod note_expression;
mod note_lanes;
mod note_lanes_chase;
mod note_mute;
mod note_tails;
mod note_tools;
mod old_projects_4m;
mod organ;
mod organise;
mod player_demo;
mod player_stack;
mod plucked;
mod plugin_demo;
mod plugin_demo_vst3;
mod project;
mod qa_audio_workers;
mod qa_render;
mod rack_sorting;
mod render_guard;
mod sampler_demo;
mod semi_modular;
mod slicer_demo;
mod slicer_roundtrip;
mod snapshots;
mod sound_analyst;
mod stems;
mod synth_expansion;
mod takes;
mod tempo_ramp;
mod translation;
mod utility_devices;
mod vca;
mod warp;
mod wavetable_demo;
