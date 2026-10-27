//! The `fm-demo` example patch (`examples/patches/fm-demo.json`): an electric piano, a mono bass
//! and a bell, all `atomik.fm-synth` loaded with factory sounds, bounced through the real build
//! path.
//!
//! * it sounds, stays finite and stereo (the piano and the bell are spread by key);
//! * it is bit-identical at every host block size, under the allocation guard (HI-1 and HI-2);
//! * the same patch bounced twice is identical;
//! * the three devices carry exactly the params of the factory sounds, so the patch and the
//!   sounds do not drift apart;
//! * every algorithm bounces finite and within the output bound at the host block of the engine.
atomik_testkit::install_alloc_guard!();

use atomik_devices::fm_synth::sounds;
use atomik_engine::{OfflineJob, render_offline_with};
use atomik_render::build::build;
use atomik_render::patch::Patch;
use atomik_testkit::no_alloc::assert_no_alloc;
use std::path::Path;

const FRAMES: usize = 4 * 48_000;

fn patch_text() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/patches/fm-demo.json");
    std::fs::read_to_string(path).unwrap()
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
fn the_demo_sounds_in_stereo_and_stays_bounded() {
    let (l, r) = channels(&bounce(&patch_text(), 64));
    assert!(l.iter().chain(&r).all(|s| s.is_finite() && s.abs() < 4.0));
    assert!(rms(&l) > 0.02 && rms(&r) > 0.02, "{} {}", rms(&l), rms(&r));
    assert_ne!(l, r, "the key spread makes the channels differ");
    // The bell rings into the silence of the second bar: the end is not dead.
    let tail = &l[FRAMES - 24_000..];
    assert!(rms(tail) > 1e-3, "{}", rms(tail));
}

#[test]
fn the_demo_is_bit_identical_for_every_host_block() {
    let text = patch_text();
    let reference = bounce(&text, 1);
    for block in [7, 63, 64, 65, 100, 512, 4096] {
        assert!(
            bounce(&text, block) == reference,
            "HI-2: host block {block} differs from host block 1"
        );
    }
}

#[test]
fn the_same_patch_twice_is_identical() {
    let text = patch_text();
    assert!(bounce(&text, 64) == bounce(&text, 64));
}

#[test]
fn the_demo_devices_carry_the_factory_sounds() {
    let patch: serde_json::Value = serde_json::from_str(&patch_text()).unwrap();
    for (id, sound) in [("keys", 1), ("bass", 3), ("bell", 2)] {
        let dev = patch["devices"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["id"] == id)
            .unwrap();
        let params = dev["params"].as_object().unwrap();
        for (key, value) in sounds::values(sound) {
            // The patch lists what differs from the defaults; the rest is the default.
            let default = atomik_devices::fm_synth::DESCRIPTOR
                .param_by_key(key)
                .unwrap()
                .default;
            let got = params
                .get(key)
                .map_or(default, |v| v.as_f64().unwrap() as f32);
            assert_eq!(got, value, "{id}: {key}");
        }
        for key in params.keys() {
            assert!(
                sounds::SOUNDS[sound].changes.iter().any(|(k, _)| k == key),
                "{id}: {key} is not part of the sound"
            );
        }
    }
}

#[test]
fn every_algorithm_bounces_finite_and_bounded() {
    let text = patch_text();
    for alg in 0..atomik_dsp::fm::ALGORITHM_COUNT {
        let edited = text.replace("\"algorithm\": 1,", &format!("\"algorithm\": {alg},"));
        let (l, r) = channels(&bounce(&edited, 64));
        assert!(
            l.iter().chain(&r).all(|s| s.is_finite() && s.abs() < 4.0),
            "algorithm {alg}"
        );
    }
}
