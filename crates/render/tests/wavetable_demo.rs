//! The `wavetable-demo` example patch (`examples/patches/wavetable-demo.json`): a four-voice
//! unison pad on two wavetable oscillators, bounced through the real build path.
//!
//! * it sounds, stays finite and stereo (the unison copies are spread);
//! * it is bit-identical at every host block size, under the allocation guard (HI-1 and HI-2);
//! * the same patch bounced twice is identical, and a different patch seed changes the
//!   unison phases (so the seed reaches the device).
atomik_testkit::install_alloc_guard!();

use atomik_engine::{OfflineJob, render_offline_with};
use atomik_render::build::build;
use atomik_render::patch::Patch;
use atomik_testkit::no_alloc::assert_no_alloc;
use std::path::Path;

const FRAMES: usize = 3 * 48_000;

fn patch_text() -> String {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/patches/wavetable-demo.json");
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

#[test]
fn the_pad_sounds_in_stereo_and_stays_bounded() {
    let out = bounce(&patch_text(), 64);
    let channel = |c: usize| -> Vec<f32> { out[c].iter().map(|b| f32::from_bits(*b)).collect() };
    let (l, r) = (channel(0), channel(1));
    assert!(l.iter().chain(&r).all(|s| s.is_finite() && s.abs() < 4.0));
    let rms = |x: &[f32]| (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt();
    assert!(rms(&l) > 0.02 && rms(&r) > 0.02, "{} {}", rms(&l), rms(&r));
    assert_ne!(l, r, "the unison spread makes the channels differ");
}

#[test]
fn the_pad_is_bit_identical_for_every_host_block() {
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
fn the_same_patch_twice_is_identical_and_the_seed_matters() {
    let text = patch_text();
    assert!(bounce(&text, 64) == bounce(&text, 64));
    let other = text.replace("\"seed\": 7", "\"seed\": 8");
    assert_ne!(other, text);
    assert!(
        bounce(&other, 64) != bounce(&text, 64),
        "the seed moves the unison phases"
    );
}
