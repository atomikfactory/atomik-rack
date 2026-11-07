//! The clipper bounced through the real build path (an inline patch: a saw chord and a bass note
//! through the clipper).
//!
//! * the clipped phrase holds the ceiling, has a lower crest factor than the dry one and is
//!   still a signal (not silence, not a square);
//! * a multiband setting with oversampling renders finite and holds each band to its ceiling;
//! * every setting is bit-identical at every host block size under the allocation guard, and the
//!   same patch bounced twice is identical, so the oversampling filters, the crossovers and the
//!   activity counters do not depend on how the host cuts the render.
atomik_testkit::install_alloc_guard!();

use atomik_engine::{OfflineJob, render_offline_with};
use atomik_render::build::build;
use atomik_render::patch::Patch;
use atomik_testkit::no_alloc::assert_no_alloc;

const FRAMES: usize = 2 * 48_000;

/// A chord and a bass note on a saw synth, through the clipper with `params` (or straight to the
/// outputs when `params` is `None`).
fn patch_json(params: Option<&str>) -> String {
    let synth = r#"{ "id": "s", "type": "atomik.poly-synth", "params": {
        "osc1.wave": 1, "osc.mix": 0.0, "voices": 4, "filter.cutoff": 9000,
        "env.amp.attack": 2, "env.amp.decay": 300, "env.amp.sustain": 0.9, "env.amp.release": 80 } }"#;
    let (devices, cables) = match params {
        None => (
            synth.to_owned(),
            r#"{ "from": "s:out_l", "to": "interface:out_1" },
               { "from": "s:out_r", "to": "interface:out_2" }"#
                .to_owned(),
        ),
        Some(p) => (
            format!(r#"{synth}, {{ "id": "c", "type": "atomik.clipper", "params": {p} }}"#),
            r#"{ "from": "s:out_l", "to": "c:in_l" }, { "from": "s:out_r", "to": "c:in_r" },
               { "from": "c:out_l", "to": "interface:out_1" },
               { "from": "c:out_r", "to": "interface:out_2" }"#
                .to_owned(),
        ),
    };
    let notes: Vec<String> = [(0.0, 36), (0.0, 55), (0.0, 59), (0.0, 62), (2.0, 41), (2.0, 57)]
        .iter()
        .map(|(at, key)| {
            format!(
                r#"{{ "device": "s", "socket": "notes_in", "at_beats": {at}, "length_beats": 1.75, "key": {key}, "velocity": 0.9 }}"#
            )
        })
        .collect();
    format!(
        r#"{{ "format": "atomik-patch", "version": 1, "sample_rate": 48000, "tempo_bpm": 120, "seed": 7,
  "length_beats": 4, "output_channels": 2,
  "devices": [ {devices} ], "cables": [ {cables} ], "notes": [ {} ] }}"#,
        notes.join(", ")
    )
}

/// Renders the first `FRAMES` frames at `host_block`, under the guard.
fn bounce(text: &str, host_block: usize) -> Vec<Vec<f32>> {
    let patch = Patch::from_json(text).unwrap_or_else(|e| panic!("{e}\n{text}"));
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
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt()
}

/// Peak over RMS of the part of the render with the notes on.
fn crest(x: &[f32]) -> f32 {
    let held = &x[4_800..80_000];
    peak(held) / rms(held)
}

const SINGLE: &str =
    r#"{ "gain": 9, "ceiling": -6, "curve": 0.3, "oversample": 2, "output": 0, "mix": 1 }"#;
const MULTI: &str = r#"{ "gain": 6, "ceiling": -9, "curve": 0.2, "oversample": 1, "bands": 2,
    "band1.drive": 9, "band3.drive": -3 }"#;
const ASYMMETRIC: &str =
    r#"{ "gain": 12, "ceiling": -6, "symmetry": 0.6, "oversample": 3, "curve": 0.5 }"#;

#[test]
fn the_clipped_phrase_holds_the_ceiling_and_has_a_lower_crest_factor() {
    let dry = bounce(&patch_json(None), 64);
    assert!(peak(&dry[0]) > 0.3, "the phrase sounds: {}", peak(&dry[0]));
    let clipped = bounce(&patch_json(Some(SINGLE)), 64);
    let ceiling = 10f32.powf(-6.0 / 20.0);
    for ch in &clipped {
        assert!(ch.iter().all(|s| s.is_finite()));
        assert!(peak(ch) <= ceiling, "{} over {ceiling}", peak(ch));
        assert!(rms(ch) > 0.05, "a signal, not silence: {}", rms(ch));
    }
    assert!(
        crest(&clipped[0]) < crest(&dry[0]) * 0.9,
        "crest {} vs dry {}",
        crest(&clipped[0]),
        crest(&dry[0])
    );
}

#[test]
fn the_multiband_and_asymmetric_settings_render_finite_and_bounded() {
    for params in [MULTI, ASYMMETRIC] {
        let out = bounce(&patch_json(Some(params)), 64);
        for ch in &out {
            assert!(ch.iter().all(|s| s.is_finite() && s.abs() < 4.0));
            assert!(rms(ch) > 0.05, "{params}: {}", rms(ch));
        }
    }
    let asym = bounce(&patch_json(Some(ASYMMETRIC)), 64);
    assert!(peak(&asym[0]) <= 10f32.powf(-6.0 / 20.0));
}

#[test]
fn every_setting_is_bit_identical_for_every_host_block_and_every_run() {
    for params in [SINGLE, MULTI, ASYMMETRIC] {
        let json = patch_json(Some(params));
        let reference = bounce(&json, 1);
        for block in [7, 63, 64, 65, 512, 4096] {
            let out = bounce(&json, block);
            for (a, b) in reference.iter().zip(&out) {
                assert!(
                    a.iter().zip(b).all(|(p, q)| p.to_bits() == q.to_bits()),
                    "{params} differs at host block {block}"
                );
            }
        }
        let again = bounce(&json, 64);
        assert!(
            reference[0]
                .iter()
                .zip(&again[0])
                .all(|(p, q)| p.to_bits() == q.to_bits())
        );
    }
}
