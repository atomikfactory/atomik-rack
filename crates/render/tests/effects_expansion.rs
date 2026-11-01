//! The expanded effects bounced through the real build path (an inline patch: a poly synth into
//! the distortion with a new curve, a cut and a body bank, into the echo with every loop stage on,
//! into the mastering with the soft clip, the longest look-ahead and the split-band width).
//!
//! * it sounds, stays finite and bounded, and stays stereo;
//! * it is bit-identical at every host block size, under the allocation guard;
//! * the same patch bounced twice is identical;
//! * the three effects with the new params named at their defaults sound like a patch that never
//!   names them (the defaults are neutral through the build path too).
atomik_testkit::install_alloc_guard!();

use atomik_engine::{OfflineJob, render_offline_with};
use atomik_render::build::build;
use atomik_render::patch::Patch;
use atomik_testkit::no_alloc::assert_no_alloc;

const FRAMES: usize = 3 * 48_000;

/// The patch with the effect params spliced in at `{DIST}`, `{ECHO}` and `{MASTER}`.
const TEMPLATE: &str = r#"{
  "format": "atomik-patch", "version": 1, "sample_rate": 48000, "tempo_bpm": 110, "seed": 5,
  "length_beats": 4, "output_channels": 2,
  "devices": [
    { "id": "synth", "type": "atomik.poly-synth", "params": { "filter.cutoff": 2400, "spread": 0.6 } },
    { "id": "dist", "type": "atomik.distortion", "params": { {DIST} } },
    { "id": "echo", "type": "atomik.echo", "params": { {ECHO} } },
    { "id": "master", "type": "atomik.mastering", "params": { {MASTER} } }
  ],
  "cables": [
    { "from": "synth:out_l", "to": "dist:in_l" }, { "from": "synth:out_r", "to": "dist:in_r" },
    { "from": "dist:out_l", "to": "echo:in_l" }, { "from": "dist:out_r", "to": "echo:in_r" },
    { "from": "echo:out_l", "to": "master:in_l" }, { "from": "echo:out_r", "to": "master:in_r" },
    { "from": "master:out_l", "to": "interface:out_1" },
    { "from": "master:out_r", "to": "interface:out_2" }
  ],
  "notes": [
    { "device": "synth", "socket": "notes_in", "at_beats": 0, "length_beats": 1.5, "key": 55, "velocity": 0.9 },
    { "device": "synth", "socket": "notes_in", "at_beats": 0, "length_beats": 1.5, "key": 62, "velocity": 0.9 },
    { "device": "synth", "socket": "notes_in", "at_beats": 1.5, "length_beats": 1, "key": 60, "velocity": 0.8 },
    { "device": "synth", "socket": "notes_in", "at_beats": 2.5, "length_beats": 1.5, "key": 67, "velocity": 0.8 }
  ]
}"#;

const DIST_ON: &str = r#""curve": 6, "drive": 18, "cut.low": 120, "cut.high": 9000, "body.mode": 2, "body.amount": 0.8, "body.size": 0.4"#;
const ECHO_ON: &str = r#""time.mode": 0, "time.ms": 230, "feedback": 0.6, "mix": 0.4, "color.drive": 0.5, "color.low": 200, "color.high": 6000, "diffusion": 0.6, "wobble.depth": 0.5, "wobble.rate": 1.3, "crush.bits": 10, "duck": 0.4, "ping_pong": 1"#;
const MASTER_ON: &str = r#""softclip": 0.5, "lookahead": 2, "width.split": 1, "width.freq": 300, "width.lo": 0.3, "width.hi": 1.5"#;

const DIST_PLAIN: &str = r#""curve": 2, "drive": 14"#;
const ECHO_PLAIN: &str = r#""time.mode": 0, "time.ms": 230, "feedback": 0.5"#;
const MASTER_PLAIN: &str = r#""width": 1.2"#;

const DIST_DEFAULTS: &str = r#""curve": 2, "drive": 14, "cut.low": 20, "cut.high": 20000, "body.mode": 0, "body.amount": 1, "body.size": 0.9"#;
const ECHO_DEFAULTS: &str = r#""time.mode": 0, "time.ms": 230, "feedback": 0.5, "mode": 0, "color.drive": 0, "color.low": 20, "color.high": 20000, "diffusion": 0, "wobble.depth": 0, "wobble.rate": 4, "crush.bits": 16, "duck": 0, "ping_pong": 0"#;
const MASTER_DEFAULTS: &str = r#""width": 1.2, "softclip": 0, "lookahead": 1, "width.split": 0, "width.freq": 900, "width.lo": 0.1, "width.hi": 1.9, "width.solo": 2"#;

fn patch(dist: &str, echo: &str, master: &str) -> String {
    TEMPLATE
        .replace("{DIST}", dist)
        .replace("{ECHO}", echo)
        .replace("{MASTER}", master)
}

/// Renders the first `FRAMES` frames of `text` at `host_block`, under the guard.
fn bounce(text: &str, host_block: usize) -> Vec<Vec<u32>> {
    let patch = Patch::from_json(text).unwrap();
    let built = build(&patch, &atomik_render::default_registry()).unwrap();
    assert!(built.warnings.is_empty(), "{:?}", built.warnings);
    let job = OfflineJob {
        frames: FRAMES,
        host_block,
        notes: &built.notes,
        params: &[],
        inputs: &[],
    };
    render_offline_with(built.plan, built.config, &job, |f| assert_no_alloc(f))
        .iter()
        .map(|ch| ch.iter().map(|s| s.to_bits()).collect())
        .collect()
}

fn channels(out: &[Vec<u32>]) -> (Vec<f32>, Vec<f32>) {
    let f = |c: usize| -> Vec<f32> { out[c].iter().map(|b| f32::from_bits(*b)).collect() };
    (f(0), f(1))
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt()
}

#[test]
fn the_patch_sounds_in_stereo_and_stays_bounded() {
    let (l, r) = channels(&bounce(&patch(DIST_ON, ECHO_ON, MASTER_ON), 64));
    assert!(l.iter().chain(&r).all(|s| s.is_finite() && s.abs() <= 1.05));
    assert!(rms(&l) > 0.01 && rms(&r) > 0.01, "{} {}", rms(&l), rms(&r));
    assert_ne!(l, r);
}

#[test]
fn the_patch_is_bit_identical_for_every_host_block() {
    let text = patch(DIST_ON, ECHO_ON, MASTER_ON);
    let reference = bounce(&text, 1);
    for block in [7, 63, 64, 65, 100, 512, 4096] {
        assert!(
            bounce(&text, block) == reference,
            "host block {block} differs from host block 1"
        );
    }
}

#[test]
fn the_same_patch_twice_is_identical() {
    let text = patch(DIST_ON, ECHO_ON, MASTER_ON);
    assert!(bounce(&text, 64) == bounce(&text, 64));
}

#[test]
fn naming_the_new_params_at_their_defaults_changes_nothing() {
    let plain = patch(DIST_PLAIN, ECHO_PLAIN, MASTER_PLAIN);
    let named = patch(DIST_DEFAULTS, ECHO_DEFAULTS, MASTER_DEFAULTS);
    assert_ne!(plain, named);
    assert!(bounce(&plain, 64) == bounce(&named, 64));
}

#[test]
fn the_new_params_change_the_sound() {
    let base = bounce(&patch(DIST_PLAIN, ECHO_PLAIN, MASTER_PLAIN), 64);
    let full = bounce(&patch(DIST_ON, ECHO_ON, MASTER_ON), 64);
    assert!(base != full);
}
