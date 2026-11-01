//! The expanded poly synth, the mono bass and the dual layer bounced through the real build path
//! (an inline patch: the poly synth with every appended param on, a bass line and two layers).
//!
//! * it sounds, stays finite and bounded, and the spread and the layers make it stereo;
//! * it is bit-identical at every host block size, under the allocation guard (HI-1 and HI-2);
//! * the same patch bounced twice is identical;
//! * the poly synth with the appended params left alone sounds like a patch that never names
//!   them (the defaults of the appended params are neutral through the build path too).
atomik_testkit::install_alloc_guard!();

use atomik_engine::{OfflineJob, render_offline_with};
use atomik_render::build::build;
use atomik_render::patch::Patch;
use atomik_testkit::no_alloc::assert_no_alloc;

const FRAMES: usize = 3 * 48_000;

/// A poly synth pad with the appended params on, a mono bass and a dual layer lead.
const PATCH: &str = r#"{
  "format": "atomik-patch", "version": 1, "sample_rate": 48000, "tempo_bpm": 110, "seed": 5,
  "length_beats": 4, "output_channels": 2,
  "devices": [
    { "id": "pad", "type": "atomik.poly-synth", "params": {
        "osc1.table": 12, "osc2.table": 17, "osc2.mode": 3, "osc2.amount": 0.2,
        "filter.type": 1, "filter.drive": 0.3, "filter.cutoff": 1800, "filter2.mode": 2,
        "filter2.cutoff": 900, "lfo2.wave": 4, "lfo2.rate": 3, "lfo2.to_cutoff": 1,
        "lfo2.to_level": 0.2, "env.mod.amount": 0.3, "env.mod.dest": 1, "glide": 40,
        "spread": 0.7, "drift": 0.6, "chorus.mix": 0.3, "delay.mix": 0.2, "delay.time": 220 } },
    { "id": "bass", "type": "atomik.mono-bass", "params": { "filter.cutoff": 380 } },
    { "id": "lead", "type": "atomik.dual-layer", "params": {
        "layer.mode": 0, "layer.morph": 0.4, "layer.age": 0.5, "a.osc1.table": 3,
        "b.osc2.mode": 1, "b.filter.cutoff": 2500, "spread": 0.5 } },
    { "id": "mixer", "type": "atomik.line-mixer", "params": { "ch1.level": -6, "ch2.level": -4, "ch3.level": -8 } }
  ],
  "cables": [
    { "from": "pad:out_l", "to": "mixer:ch1_in_l" }, { "from": "pad:out_r", "to": "mixer:ch1_in_r" },
    { "from": "bass:out_l", "to": "mixer:ch2_in_l" }, { "from": "bass:out_r", "to": "mixer:ch2_in_r" },
    { "from": "lead:out_l", "to": "mixer:ch3_in_l" }, { "from": "lead:out_r", "to": "mixer:ch3_in_r" },
    { "from": "mixer:master_l", "to": "interface:out_1" },
    { "from": "mixer:master_r", "to": "interface:out_2" }
  ],
  "notes": [
    { "device": "pad", "socket": "notes_in", "at_beats": 0, "length_beats": 3, "key": 60, "velocity": 0.8 },
    { "device": "pad", "socket": "notes_in", "at_beats": 0, "length_beats": 3, "key": 64, "velocity": 0.8 },
    { "device": "pad", "socket": "notes_in", "at_beats": 0.5, "length_beats": 3, "key": 67, "velocity": 0.8 },
    { "device": "bass", "socket": "notes_in", "at_beats": 0, "length_beats": 1.5, "key": 36, "velocity": 0.9 },
    { "device": "bass", "socket": "notes_in", "at_beats": 1, "length_beats": 1.5, "key": 43, "velocity": 0.9 },
    { "device": "bass", "socket": "notes_in", "at_beats": 2.5, "length_beats": 1, "key": 41, "velocity": 0.9 },
    { "device": "lead", "socket": "notes_in", "at_beats": 1, "length_beats": 1, "key": 72, "velocity": 0.8 },
    { "device": "lead", "socket": "notes_in", "at_beats": 2, "length_beats": 1.5, "key": 76, "velocity": 0.8 }
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
    assert_ne!(
        l, r,
        "the spread, the echo and the chorus make the channels differ"
    );
}

#[test]
fn the_patch_is_bit_identical_for_every_host_block() {
    let reference = bounce(PATCH, 1);
    for block in [7, 63, 64, 65, 100, 512, 4096] {
        assert!(
            bounce(PATCH, block) == reference,
            "HI-2: host block {block} differs from host block 1"
        );
    }
}

#[test]
fn the_same_patch_twice_is_identical() {
    assert!(bounce(PATCH, 64) == bounce(PATCH, 64));
}

#[test]
fn naming_the_appended_params_at_their_defaults_changes_nothing() {
    let plain = r#"{
      "format": "atomik-patch", "version": 1, "sample_rate": 48000, "tempo_bpm": 110, "seed": 5,
      "length_beats": 4, "output_channels": 2,
      "devices": [
        { "id": "s", "type": "atomik.poly-synth", "params": { "filter.cutoff": 1500 } },
        { "id": "mixer", "type": "atomik.line-mixer", "params": {} }
      ],
      "cables": [
        { "from": "s:out_l", "to": "mixer:ch1_in_l" }, { "from": "s:out_r", "to": "mixer:ch1_in_r" },
        { "from": "mixer:master_l", "to": "interface:out_1" },
        { "from": "mixer:master_r", "to": "interface:out_2" }
      ],
      "notes": [
        { "device": "s", "socket": "notes_in", "at_beats": 0, "length_beats": 2, "key": 57, "velocity": 0.9 },
        { "device": "s", "socket": "notes_in", "at_beats": 1, "length_beats": 2, "key": 64, "velocity": 0.7 }
      ]
    }"#;
    let named = plain.replace(
        r#""filter.cutoff": 1500"#,
        r#""filter.cutoff": 1500, "osc1.table": 0, "osc2.table": 0, "osc2.mode": 0, "osc2.amount": 0,
           "filter.type": 0, "filter.drive": 0, "filter2.mode": 0, "filter.link": 0.3,
           "filter2.routing": 1, "lfo2.rate": 9, "lfo2.to_pitch": 0, "lfo2.to_cutoff": 0,
           "lfo2.to_level": 0, "env.mod.amount": 0, "env.mod.dest": 3, "glide": 0, "legato": 0,
           "spread": 0, "drift": 0, "chorus.mix": 0, "delay.mix": 0, "delay.time": 90, "delay.feedback": 0.35"#,
    );
    assert_ne!(plain, named);
    assert!(bounce(plain, 64) == bounce(&named, 64));
}
