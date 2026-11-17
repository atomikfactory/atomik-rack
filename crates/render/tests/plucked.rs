//! Plucked Strings bounced through the real build path (an inline patch: a strummed chord and a
//! bass line, a course and a body).
//!
//! * it sounds, stays finite and bounded, in stereo;
//! * it is bit-identical at every host block size, under the allocation guard, so the control
//!   step, the picks and the glides do not depend on how the host cuts the render;
//! * the same patch bounced twice is identical and a different seed plucks differently;
//! * the body, the pick and the palm mute change the sound.
atomik_testkit::install_alloc_guard!();

use atomik_engine::{OfflineJob, render_offline_with};
use atomik_render::build::build;
use atomik_render::patch::Patch;
use atomik_testkit::no_alloc::assert_no_alloc;

const FRAMES: usize = 3 * 48_000;

/// A chord with a staggered strum, a low line and a short note, on a 12-string-like course.
const PATCH: &str = r#"{
  "format": "atomik-patch", "version": 1, "sample_rate": 48000, "tempo_bpm": 120, "seed": 7,
  "length_beats": 6, "output_channels": 2,
  "devices": [
    { "id": "pluck", "type": "atomik.plucked", "params": {
        "body": 1, "course.mode": 1, "course.detune": 9, "decay": 2500, "sustain": 0.4,
        "pick.hardness": 0.6, "stiffness": 0.3, "stereo.width": 0.6, "out.gain": -3 } }
  ],
  "cables": [
    { "from": "pluck:out_l", "to": "interface:out_1" }, { "from": "pluck:out_r", "to": "interface:out_2" }
  ],
  "notes": [
    { "device": "pluck", "socket": "notes_in", "at_beats": 0, "length_beats": 2, "key": 40, "velocity": 0.8 },
    { "device": "pluck", "socket": "notes_in", "at_beats": 0.0625, "length_beats": 2, "key": 52, "velocity": 0.7 },
    { "device": "pluck", "socket": "notes_in", "at_beats": 0.125, "length_beats": 2, "key": 56, "velocity": 0.7 },
    { "device": "pluck", "socket": "notes_in", "at_beats": 0.1875, "length_beats": 2, "key": 59, "velocity": 0.75 },
    { "device": "pluck", "socket": "notes_in", "at_beats": 0.25, "length_beats": 2, "key": 64, "velocity": 0.8 },
    { "device": "pluck", "socket": "notes_in", "at_beats": 2.5, "length_beats": 0.25, "key": 47, "velocity": 0.6 },
    { "device": "pluck", "socket": "notes_in", "at_beats": 3, "length_beats": 1, "key": 76, "velocity": 1.0 }
  ]
}"#;

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
    let (l, r) = channels(&bounce(PATCH, 64));
    assert!(l.iter().chain(&r).all(|s| s.is_finite() && s.abs() < 4.0));
    assert!(rms(&l) > 0.02 && rms(&r) > 0.02, "{} {}", rms(&l), rms(&r));
    assert_ne!(l, r, "the keys and the strings of the course are spread");
}

#[test]
fn the_patch_is_bit_identical_for_every_host_block() {
    let reference = bounce(PATCH, 1);
    for block in [7, 63, 64, 65, 100, 512, 4096] {
        assert!(
            bounce(PATCH, block) == reference,
            "host block {block} differs from host block 1"
        );
    }
}

#[test]
fn the_same_patch_twice_is_identical_and_the_seed_matters() {
    assert!(bounce(PATCH, 64) == bounce(PATCH, 64));
    let other = PATCH.replacen(r#""seed": 7"#, r#""seed": 8"#, 1);
    assert_ne!(other, PATCH);
    assert!(bounce(&other, 64) != bounce(PATCH, 64));
}

#[test]
fn the_body_the_pick_and_the_palm_mute_change_the_sound() {
    let base = bounce(PATCH, 64);
    for (from, to) in [
        (r#""body": 1"#, r#""body": 7"#),
        (r#""pick.hardness": 0.6"#, r#""pick.hardness": 0.1"#),
        (
            r#""stiffness": 0.3"#,
            r#""stiffness": 0.3, "palm_mute": 0.7"#,
        ),
    ] {
        let changed = PATCH.replacen(from, to, 1);
        assert_ne!(changed, PATCH, "{from}");
        let out = bounce(&changed, 64);
        assert!(out != base, "{from}");
        let (l, r) = channels(&out);
        assert!(l.iter().chain(&r).all(|s| s.is_finite() && s.abs() < 4.0));
    }
    // Muted, the held chord rings less: its second half is quieter.
    let (open, _) = channels(&base);
    let muted = PATCH.replacen(
        r#""stiffness": 0.3"#,
        r#""stiffness": 0.3, "palm_mute": 0.9"#,
        1,
    );
    let (closed, _) = channels(&bounce(&muted, 64));
    let tail = 24_000..43_000;
    assert!(rms(&closed[tail.clone()]) < 0.5 * rms(&open[tail]));
}
