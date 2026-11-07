//! The dynamic EQ in a patch bounce: an idle device copies the synth bit for bit, a band set to
//! the key input is driven by a cabled source (and by its own input when nothing is cabled), and
//! the render is the same at every host block size.
atomik_testkit::install_alloc_guard!();

use atomik_engine::{OfflineJob, render_offline_with};
use atomik_render::build::{BuildOptions, build_with};
use atomik_render::default_registry;
use atomik_render::patch::Patch;
use atomik_testkit::no_alloc::assert_no_alloc;

/// How the key socket of the dynamic EQ is fed.
#[derive(Clone, Copy, PartialEq)]
enum Key {
    /// Nothing is cabled.
    None,
    /// A second synth that plays nothing.
    Silent,
    /// A second synth that plays the same notes, louder.
    Loud,
}

/// A one-voice synth playing a short phrase through the dynamic EQ (when `params` is given) to
/// the interface.
fn patch_json(params: Option<&str>, key: Key) -> String {
    let synth = |id: &str, level_db: f32| {
        format!(
            r#"{{"id": "{id}", "type": "atomik.poly-synth",
            "params": {{"osc1.wave": 1, "osc.mix": 0.0, "voices": 1, "filter.cutoff": 12000,
                       "env.amp.attack": 2, "env.amp.decay": 200, "env.amp.sustain": 0.8,
                       "env.amp.release": 80, "master.level": {level_db}}}}}"#
        )
    };
    let mut devices = vec![synth("s", -6.0)];
    let mut cables = Vec::new();
    let mut notes = Vec::new();
    let mut play = |id: &str| {
        for (i, at) in [0.0, 1.0, 2.0, 3.0].iter().enumerate() {
            let k = [45, 48, 52, 57][i];
            notes.push(format!(
                r#"{{"device": "{id}", "socket": "notes_in", "at_beats": {at}, "length_beats": 0.75, "key": {k}, "velocity": 0.9}}"#
            ));
        }
    };
    play("s");
    match params {
        None => {
            cables.push(r#"{"from": "s:out_l", "to": "interface:out_1"}"#.to_owned());
            cables.push(r#"{"from": "s:out_r", "to": "interface:out_2"}"#.to_owned());
        }
        Some(params) => {
            devices.push(format!(
                r#"{{"id": "fx", "type": "atomik.dynamic-eq", "params": {params}}}"#
            ));
            for side in ["l", "r"] {
                cables.push(format!(
                    r#"{{"from": "s:out_{side}", "to": "fx:in_{side}"}}"#
                ));
            }
            cables.push(r#"{"from": "fx:out_l", "to": "interface:out_1"}"#.to_owned());
            cables.push(r#"{"from": "fx:out_r", "to": "interface:out_2"}"#.to_owned());
            if key != Key::None {
                devices.push(synth("k", if key == Key::Loud { 0.0 } else { -6.0 }));
                if key == Key::Loud {
                    play("k");
                }
                for side in ["l", "r"] {
                    cables.push(format!(
                        r#"{{"from": "k:out_{side}", "to": "fx:sc_in_{side}"}}"#
                    ));
                }
            }
        }
    }
    format!(
        r#"{{"format": "atomik-patch", "version": 1, "sample_rate": 48000, "tempo_bpm": 120,
            "seed": 3, "length_beats": 4, "output_channels": 2,
            "devices": [{}], "cables": [{}], "notes": [{}]}}"#,
        devices.join(","),
        cables.join(","),
        notes.join(",")
    )
}

fn render(json: &str, host_block: usize) -> Vec<Vec<f32>> {
    let patch = Patch::from_json(json).unwrap_or_else(|e| panic!("{e}\n{json}"));
    let built = build_with(&patch, &default_registry(), &BuildOptions::default()).unwrap();
    let job = OfflineJob {
        frames: built.frames,
        host_block,
        notes: &built.notes,
        params: &[],
        inputs: &[],
    };
    render_offline_with(built.plan, built.config, &job, |r| assert_no_alloc(r))
}

fn rms(x: &[f32]) -> f64 {
    (x.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
}

fn max_diff(a: &[Vec<f32>], b: &[Vec<f32>]) -> f32 {
    a.iter()
        .flatten()
        .zip(b.iter().flatten())
        .fold(0.0f32, |m, (x, y)| m.max((x - y).abs()))
}

/// Band 2 at the register of the phrase: cuts 18 dB when its (key) level is above -30 dB.
const KEYED: &str = r#"{"band2.on": 1, "band2.freq": 220, "band2.q": 0.5, "band2.threshold": -30,
    "band2.range": -18, "band2.ratio": 20, "band2.attack": 1, "band2.release": 50,
    "band2.source": 1}"#;

#[test]
fn an_idle_dynamic_eq_leaves_the_synth_bit_for_bit() {
    let dry = render(&patch_json(None, Key::None), 64);
    assert!(rms(&dry[0]) > 0.02, "the dry phrase sounds");
    let idle = render(&patch_json(Some("{}"), Key::None), 64);
    for ch in 0..2 {
        assert!(
            dry[ch]
                .iter()
                .zip(&idle[ch])
                .all(|(x, y)| x.to_bits() == y.to_bits()),
            "channel {ch}"
        );
    }
}

#[test]
fn a_band_set_to_the_key_follows_the_cabled_source() {
    let dry = render(&patch_json(None, Key::None), 64);
    // A silent key: the band never moves, the synth passes (to rounding).
    let silent = render(&patch_json(Some(KEYED), Key::Silent), 64);
    assert!(
        max_diff(&dry, &silent) < 2e-3,
        "{}",
        max_diff(&dry, &silent)
    );
    // A loud key at the same notes cuts the band 18 dB.
    let loud = render(&patch_json(Some(KEYED), Key::Loud), 64);
    assert!(
        rms(&loud[0]) < rms(&dry[0]) * 0.7,
        "dry {}, keyed {}",
        rms(&dry[0]),
        rms(&loud[0])
    );
    // With nothing cabled the band listens to the synth itself, which is loud enough.
    let own = render(&patch_json(Some(KEYED), Key::None), 64);
    assert!(rms(&own[0]) < rms(&dry[0]) * 0.7);
}

#[test]
fn the_keyed_render_is_the_same_at_every_block_size() {
    let json = patch_json(Some(KEYED), Key::Loud);
    let a = render(&json, 64);
    assert!(a.iter().flatten().all(|x| x.is_finite()));
    for block in [64, 333, 1024] {
        let b = render(&json, block);
        for ch in 0..2 {
            assert!(
                a[ch]
                    .iter()
                    .zip(&b[ch])
                    .all(|(x, y)| x.to_bits() == y.to_bits()),
                "block {block}, channel {ch}"
            );
        }
    }
}
