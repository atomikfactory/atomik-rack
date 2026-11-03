//! The semi-modular synth bounced through the real build path (an inline patch: its sequencer
//! played from two held keys, a poly voice set and a mono line with the matrix in use).
//!
//! * it sounds, stays finite and bounded, and the key spread makes it stereo;
//! * it is bit-identical at every host block size, under the allocation guard, so the
//!   sequencer's clock and the matrix do not depend on how the host cuts the render;
//! * the same patch bounced twice is identical (a fixed seed: the noise slot, the random source
//!   and the sample and hold LFO are seeded);
//! * the sequencer changes the sound: the same keys with it off play two held notes.
atomik_testkit::install_alloc_guard!();

use atomik_engine::{OfflineJob, render_offline_with};
use atomik_render::build::build;
use atomik_render::patch::Patch;
use atomik_testkit::no_alloc::assert_no_alloc;

const FRAMES: usize = 2 * 48_000;

/// A sequenced voice with a filter pair, a shaper and two matrix slots, and a mono bass line
/// on the oscillator types the first one does not use.
const PATCH: &str = r#"{
  "format": "atomik-patch", "version": 1, "sample_rate": 48000, "tempo_bpm": 118, "seed": 11,
  "length_beats": 4, "output_channels": 2,
  "devices": [
    { "id": "arp", "type": "atomik.semi-modular", "params": {
        "seq.run": 1, "seq.rate": 3, "seq.swing": 20, "seq.gate": 70,
        "osc1.type": 1, "osc1.shape": 0.4, "osc2.type": 5, "osc2.wave": 3, "osc2.shape": 0.5,
        "osc2.level": 0.5, "osc3.type": 6, "osc3.wave": 2, "osc3.shape": 0.3, "osc3.level": 0.3,
        "filter1.mode": 5, "filter1.cutoff": 1400, "filter1.resonance": 0.5, "filter1.env": 2.5,
        "filter2.mode": 6, "filter2.cutoff": 900, "filter2.resonance": 0.4, "filter2.mix": 0.4,
        "filter.routing": 2, "shaper.type": 4, "shaper.drive": 9, "shaper.mix": 0.5,
        "lfo1.wave": 4, "lfo1.rate": 5, "internal.mod1_source": 4, "internal.mod1_target": 8,
        "mod1_amount": 0.6, "internal.mod2_source": 11, "internal.mod2_target": 2,
        "mod2_amount": 0.4, "master.spread": 0.6 } },
    { "id": "bass", "type": "atomik.semi-modular", "params": {
        "play.mode": 2, "play.glide": 80, "osc1.type": 0, "osc2.type": 7, "osc2.level": 0.08,
        "osc3.type": 3, "osc3.octave": -1, "osc3.level": 0.6, "filter1.cutoff": 500,
        "filter1.mode": 1 } },
    { "id": "mixer", "type": "atomik.line-mixer", "params": { "ch1.level": -6, "ch2.level": -5 } }
  ],
  "cables": [
    { "from": "arp:out_l", "to": "mixer:ch1_in_l" }, { "from": "arp:out_r", "to": "mixer:ch1_in_r" },
    { "from": "bass:out_l", "to": "mixer:ch2_in_l" }, { "from": "bass:out_r", "to": "mixer:ch2_in_r" },
    { "from": "mixer:master_l", "to": "interface:out_1" },
    { "from": "mixer:master_r", "to": "interface:out_2" }
  ],
  "notes": [
    { "device": "arp", "socket": "notes_in", "at_beats": 0, "length_beats": 2, "key": 57, "velocity": 0.8 },
    { "device": "arp", "socket": "notes_in", "at_beats": 2, "length_beats": 1.5, "key": 62, "velocity": 0.8 },
    { "device": "bass", "socket": "notes_in", "at_beats": 0, "length_beats": 1.5, "key": 33, "velocity": 0.9 },
    { "device": "bass", "socket": "notes_in", "at_beats": 1, "length_beats": 2, "key": 40, "velocity": 0.9 },
    { "device": "bass", "socket": "notes_in", "at_beats": 3, "length_beats": 1, "key": 36, "velocity": 0.9 }
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
    assert_ne!(l, r, "the key spread makes the channels differ");
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
fn the_same_patch_twice_is_identical() {
    assert!(bounce(PATCH, 64) == bounce(PATCH, 64));
}

#[test]
fn the_sequencer_changes_the_sound() {
    let off = PATCH.replacen(r#""seq.run": 1"#, r#""seq.run": 0"#, 1);
    assert_ne!(off, PATCH);
    assert!(bounce(&off, 64) != bounce(PATCH, 64));
    // Without the sequencer the held keys sound as two long notes: steadier than the pattern.
    let (a, _) = channels(&bounce(&off, 64));
    assert!(a.iter().all(|s| s.is_finite() && s.abs() < 4.0));
}
