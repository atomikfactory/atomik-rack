//! The organ bounced through the real build path (an inline patch: a chord and a bass line with
//! percussion, chorus and overdrive, played through the rotary speaker).
//!
//! * it sounds, stays finite and bounded, and the rotary speaker makes it stereo;
//! * it is bit-identical at every host block size, under the allocation guard, so the wheels,
//!   the percussion and the scanner do not depend on how the host cuts the render;
//! * the same patch bounced twice is identical and a different seed gives different wheel
//!   phases and click noise;
//! * the drawbars change the sound, and the organ alone with no scanner is mono.
atomik_testkit::install_alloc_guard!();

use atomik_engine::{OfflineJob, render_offline_with};
use atomik_render::build::build;
use atomik_render::patch::Patch;
use atomik_testkit::no_alloc::assert_no_alloc;

const FRAMES: usize = 2 * 48_000;

/// A chord and a pedal note, percussion on the second harmonic, a full chorus and some drive, into
/// the rotary speaker at its fast speed.
const PATCH: &str = r#"{
  "format": "atomik-patch", "version": 1, "sample_rate": 48000, "tempo_bpm": 120, "seed": 7,
  "length_beats": 4, "output_channels": 2,
  "devices": [
    { "id": "organ", "type": "atomik.organ", "params": {
        "drawbar.16": 8, "drawbar.5_1_3": 3, "drawbar.8": 8, "drawbar.4": 6, "drawbar.2": 4,
        "drawbar.1": 2, "perc.mode": 1, "perc.decay": 1, "key.click": 0.4,
        "scanner.type": 6, "out.drive": 0.3, "out.level": -3 } },
    { "id": "rotary", "type": "atomik.rotary-speaker", "params": { "speed": 2, "mix": 1 } }
  ],
  "cables": [
    { "from": "organ:out_l", "to": "rotary:in_l" }, { "from": "organ:out_r", "to": "rotary:in_r" },
    { "from": "rotary:out_l", "to": "interface:out_1" },
    { "from": "rotary:out_r", "to": "interface:out_2" }
  ],
  "notes": [
    { "device": "organ", "socket": "notes_in", "at_beats": 0, "length_beats": 2, "key": 57, "velocity": 0.8 },
    { "device": "organ", "socket": "notes_in", "at_beats": 0, "length_beats": 2, "key": 60, "velocity": 0.8 },
    { "device": "organ", "socket": "notes_in", "at_beats": 0, "length_beats": 2, "key": 64, "velocity": 0.8 },
    { "device": "organ", "socket": "notes_in", "at_beats": 0.5, "length_beats": 1.5, "key": 36, "velocity": 0.8 },
    { "device": "organ", "socket": "notes_in", "at_beats": 2, "length_beats": 1.5, "key": 55, "velocity": 0.8 },
    { "device": "organ", "socket": "notes_in", "at_beats": 2, "length_beats": 1.5, "key": 62, "velocity": 0.8 },
    { "device": "organ", "socket": "notes_in", "at_beats": 2.5, "length_beats": 1, "key": 67, "velocity": 0.8 }
  ]
}"#;

/// The organ straight to the outputs: no scanner, so the two channels are the same.
const DRY: &str = r#"{
  "format": "atomik-patch", "version": 1, "sample_rate": 48000, "tempo_bpm": 120, "seed": 7,
  "length_beats": 2, "output_channels": 2,
  "devices": [ { "id": "organ", "type": "atomik.organ", "params": { "perc.mode": 2 } } ],
  "cables": [
    { "from": "organ:out_l", "to": "interface:out_1" }, { "from": "organ:out_r", "to": "interface:out_2" }
  ],
  "notes": [
    { "device": "organ", "socket": "notes_in", "at_beats": 0, "length_beats": 1, "key": 60, "velocity": 0.8 },
    { "device": "organ", "socket": "notes_in", "at_beats": 0.5, "length_beats": 1, "key": 67, "velocity": 0.8 }
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
    assert!(l.iter().chain(&r).all(|s| s.is_finite() && s.abs() < 2.0));
    assert!(rms(&l) > 0.05 && rms(&r) > 0.05, "{} {}", rms(&l), rms(&r));
    assert_ne!(l, r, "the microphones of the rotary speaker differ");
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
fn the_drawbars_change_the_sound() {
    let less = PATCH.replacen(r#""drawbar.4": 6"#, r#""drawbar.4": 0"#, 1);
    assert_ne!(less, PATCH);
    let (a, _) = channels(&bounce(PATCH, 64));
    let (b, _) = channels(&bounce(&less, 64));
    assert!(a != b);
    assert!(b.iter().all(|s| s.is_finite() && s.abs() < 2.0));
}

#[test]
fn the_organ_alone_without_a_scanner_is_mono_and_the_notes_are_heard() {
    let (l, r) = channels(&bounce(DRY, 64));
    assert_eq!(l, r);
    assert!(rms(&l[..48_000]) > 0.05);
    // After the notes and their release the output is exactly silent.
    assert!(l[3 * 48_000 / 2..].iter().all(|s| *s == 0.0));
}
