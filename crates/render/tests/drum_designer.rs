//! The drum machine with synthesised pad modules bounced through the real build path (an inline
//! patch: a kick, a snare, a hat and a tone pad, no samples at all).
//!
//! * it sounds, stays finite and bounded;
//! * it is bit-identical at every host block size, under the allocation guard;
//! * the same patch bounced twice is identical (the noise of a hit is seeded);
//! * a pad left on its sample module stays silent without a sample map, and choosing a module
//!   is what makes it sound.
atomik_testkit::install_alloc_guard!();

use atomik_engine::{OfflineJob, render_offline_with};
use atomik_render::build::build;
use atomik_render::patch::Patch;
use atomik_testkit::no_alloc::assert_no_alloc;

const FRAMES: usize = 2 * 48_000;

/// Pad 1 (key 36) a kick, pad 3 (38) a snare in choke group 1, pad 7 (42) a closed hat in the same
/// group and pad 8 (43) a tone.
const PATCH: &str = r#"{
  "format": "atomik-patch", "version": 1, "sample_rate": 48000, "tempo_bpm": 120, "seed": 5,
  "length_beats": 4, "output_channels": 2,
  "devices": [
    { "id": "drums", "type": "atomik.drum-machine", "params": {
        "master.level": -3,
        "pad1.module": 1, "pad1.decay": 400, "pad1.sweep": 0.7,
        "pad3.module": 2, "pad3.decay": 250, "pad3.snap": 0.7, "pad3.choke": 1,
        "pad7.module": 3, "pad7.decay": 90, "pad7.choke": 1, "pad7.drive": 0.3,
        "pad8.module": 5, "pad8.decay": 600, "pad8.tune": -5, "pad8.send1": 0.2 } }
  ],
  "cables": [
    { "from": "drums:out_l", "to": "interface:out_1" },
    { "from": "drums:out_r", "to": "interface:out_2" }
  ],
  "notes": [
    { "device": "drums", "socket": "notes_in", "at_beats": 0, "length_beats": 0.25, "key": 36, "velocity": 1.0 },
    { "device": "drums", "socket": "notes_in", "at_beats": 1, "length_beats": 0.25, "key": 38, "velocity": 0.9 },
    { "device": "drums", "socket": "notes_in", "at_beats": 1.5, "length_beats": 0.25, "key": 42, "velocity": 0.7 },
    { "device": "drums", "socket": "notes_in", "at_beats": 2, "length_beats": 0.25, "key": 36, "velocity": 1.0 },
    { "device": "drums", "socket": "notes_in", "at_beats": 2.5, "length_beats": 0.5, "key": 43, "velocity": 0.8 },
    { "device": "drums", "socket": "notes_in", "at_beats": 3, "length_beats": 0.25, "key": 38, "velocity": 1.0 }
  ]
}"#;

/// Renders the first `FRAMES` frames of `text` at `host_block`, under the guard.
fn bounce(text: &str, host_block: usize) -> Vec<Vec<u32>> {
    let patch = Patch::from_json(text).unwrap();
    let built = build(&patch, &atomik_render::default_registry()).unwrap();
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

fn channel(out: &[Vec<u32>], c: usize) -> Vec<f32> {
    out[c].iter().map(|b| f32::from_bits(*b)).collect()
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt()
}

#[test]
fn the_synthesised_kit_sounds_and_stays_bounded() {
    let out = bounce(PATCH, 64);
    let l = channel(&out, 0);
    assert!(l.iter().all(|s| s.is_finite() && s.abs() < 4.0));
    assert!(rms(&l) > 0.03, "{}", rms(&l));
    // The kick at beat 0 and the tone at beat 2.5 are both audible.
    let beat = 24_000;
    assert!(rms(&l[..beat / 2]) > 0.05);
    assert!(rms(&l[5 * beat / 2 + 2_000..3 * beat]) > 0.02);
}

#[test]
fn the_kit_is_bit_identical_for_every_host_block() {
    let reference = bounce(PATCH, 1);
    for block in [7, 63, 64, 65, 100, 512, 4096] {
        assert!(
            bounce(PATCH, block) == reference,
            "host block {block} differs from host block 1"
        );
    }
}

#[test]
fn the_same_kit_twice_is_identical() {
    assert!(bounce(PATCH, 64) == bounce(PATCH, 64));
}

#[test]
fn pads_without_a_module_stay_silent_without_samples() {
    let plain = PATCH
        .replace(r#""pad1.module": 1, "#, "")
        .replace(r#""pad3.module": 2, "#, "")
        .replace(r#""pad7.module": 3, "#, "")
        .replace(r#""pad8.module": 5, "#, "");
    assert!(!plain.contains(".module"));
    let l = channel(&bounce(&plain, 64), 0);
    assert!(l.iter().all(|s| *s == 0.0));
}
