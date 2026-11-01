//! The utility devices in a patch bounce: each effect changes a fixed synth signal, renders the
//! same bits at every host block size, never allocates on the render path (the allocation guard
//! wraps every render), and Spider Audio merges cables of different latency sample-aligned.
atomik_testkit::install_alloc_guard!();

use atomik_engine::{OfflineJob, render_offline_with};
use atomik_render::build::{BuildOptions, build_with};
use atomik_render::default_registry;
use atomik_render::patch::Patch;
use atomik_testkit::no_alloc::assert_no_alloc;

/// A one-voice synth playing a short phrase, optionally through `effect` (type id and its params
/// as a JSON object) on its way to the interface. The same notes also go to the effect's
/// `notes_in` socket when it has one.
fn patch_json(effect: Option<(&str, &str)>) -> String {
    let synth = r#"{"id": "s", "type": "atomik.poly-synth",
        "params": {"osc1.wave": 1, "osc.mix": 0.0, "voices": 1, "filter.cutoff": 12000,
                   "env.amp.attack": 2, "env.amp.decay": 200, "env.amp.sustain": 0.8,
                   "env.amp.release": 80}}"#;
    let mut devices = vec![synth.to_owned()];
    let mut cables = Vec::new();
    let mut notes = Vec::new();
    for (i, at) in [0.0, 1.0, 2.0, 3.0].iter().enumerate() {
        let key = [45, 48, 52, 57][i];
        notes.push(format!(
            r#"{{"device": "s", "socket": "notes_in", "at_beats": {at}, "length_beats": 0.75, "key": {key}, "velocity": 0.9}}"#
        ));
    }
    match effect {
        None => {
            cables.push(r#"{"from": "s:out_l", "to": "interface:out_1"}"#.to_owned());
            cables.push(r#"{"from": "s:out_r", "to": "interface:out_2"}"#.to_owned());
        }
        Some((type_id, params)) => {
            devices.push(format!(
                r#"{{"id": "fx", "type": "{type_id}", "params": {params}}}"#
            ));
            for side in ["l", "r"] {
                cables.push(format!(
                    r#"{{"from": "s:out_{side}", "to": "fx:in_{side}"}}"#
                ));
            }
            cables.push(r#"{"from": "fx:out_l", "to": "interface:out_1"}"#.to_owned());
            cables.push(r#"{"from": "fx:out_r", "to": "interface:out_2"}"#.to_owned());
            if type_id == "atomik.env-filter" {
                for at in [0.0, 1.0, 2.0, 3.0] {
                    notes.push(format!(
                        r#"{{"device": "fx", "socket": "notes_in", "at_beats": {at}, "length_beats": 0.5, "key": 60, "velocity": 1.0}}"#
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

fn render(json: &str, host_block: usize, align: bool) -> Vec<Vec<f32>> {
    let patch = Patch::from_json(json).unwrap_or_else(|e| panic!("{e}\n{json}"));
    let options = BuildOptions {
        align_inputs: align,
        ..BuildOptions::default()
    };
    let built = build_with(&patch, &default_registry(), &options).unwrap();
    let job = OfflineJob {
        frames: built.frames,
        host_block,
        notes: &built.notes,
        params: &[],
        inputs: &[],
    };
    render_offline_with(built.plan, built.config, &job, |r| assert_no_alloc(r))
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
}

/// Each effect with settings that make it audible on the phrase.
const EFFECTS: &[(&str, &str)] = &[
    ("atomik.gain-tool", r#"{"gain": -9, "pan": 0.5}"#),
    ("atomik.stereo-tool", r#"{"balance": 0.8, "invert_l": 1}"#),
    (
        "atomik.gate",
        r#"{"threshold": -12, "range": 60, "hold": 5, "release": 20}"#,
    ),
    (
        "atomik.dynamics",
        r#"{"threshold": -30, "ratio": 8, "makeup": 6}"#,
    ),
    (
        "atomik.duck",
        r#"{"mode": 0, "rate": 2, "depth": 1, "time": 100}"#,
    ),
    (
        "atomik.env-filter",
        r#"{"cutoff": 300, "amount": 4, "attack": 2, "decay": 300, "resonance": 0.5}"#,
    ),
];

#[test]
fn every_effect_changes_the_phrase_and_renders_the_same_bits_at_every_block_size() {
    let dry = render(&patch_json(None), 64, false);
    assert!(peak(&dry[0]) > 0.05, "the dry phrase sounds");
    for (type_id, params) in EFFECTS {
        let json = patch_json(Some((type_id, params)));
        let a = render(&json, 64, false);
        assert!(
            a.iter().flatten().all(|x| x.is_finite()),
            "{type_id}: finite"
        );
        let diff = a[0]
            .iter()
            .zip(&dry[0])
            .chain(a[1].iter().zip(&dry[1]))
            .fold(0.0f32, |m, (x, y)| m.max((x - y).abs()));
        assert!(diff > 1e-3, "{type_id} left the phrase alone ({diff})");
        assert!(peak(&a[0]).max(peak(&a[1])) > 1e-3, "{type_id}: silent");
        for block in [64, 333, 1024] {
            let b = render(&json, block, false);
            for ch in 0..2 {
                assert!(
                    a[ch]
                        .iter()
                        .zip(&b[ch])
                        .all(|(x, y)| x.to_bits() == y.to_bits()),
                    "{type_id}: block {block} differs on channel {ch}"
                );
            }
        }
    }
}

/// Two identical click synths, `a` direct and `b` through a compressor with 5 ms of lookahead
/// (240 frames), merged by Spider Audio.
fn merge_json(second: &str) -> String {
    let click = |id: &str| {
        format!(
            r#"{{"id": "{id}", "type": "atomik.poly-synth",
                "params": {{"osc1.wave": 1, "osc.mix": 0.0, "voices": 1, "filter.cutoff": 20000,
                           "filter.env_amount": 0, "filter.key_track": 0, "filter.resonance": 0,
                           "env.amp.attack": 0.5, "env.amp.decay": 25, "env.amp.sustain": 0,
                           "env.amp.release": 0.5}}}}"#
        )
    };
    let comp = r#"{"id": "c", "type": "atomik.compressor",
        "params": {"threshold": 0, "ratio": 1, "lookahead": 5}}"#;
    let spider = r#"{"id": "m", "type": "atomik.spider-audio"}"#;
    let cables = [
        ("a:out_l", "m:m1_l"),
        ("b:out_l", "c:in_l"),
        ("b:out_r", "c:in_r"),
        ("c:out_l", second),
        ("m:merge_out_l", "interface:out_1"),
    ]
    .map(|(f, t)| format!(r#"{{"from": "{f}", "to": "{t}"}}"#))
    .join(",");
    let notes = ["a", "b"]
        .map(|d| {
            format!(
                r#"{{"device": "{d}", "socket": "notes_in", "at_beats": 0, "length_beats": 0.125, "key": 84, "velocity": 1.0}}"#
            )
        })
        .join(",");
    format!(
        r#"{{"format": "atomik-patch", "version": 1, "sample_rate": 48000, "tempo_bpm": 120,
            "seed": 1, "length_beats": 2, "output_channels": 2,
            "devices": [{},{},{},{}], "cables": [{cables}], "notes": [{notes}]}}"#,
        click("a"),
        click("b"),
        comp,
        spider
    )
}

/// The merge project with the compressor branch cabled to `second`; `(aligned peak, unaligned
/// peak, single click peak)`.
fn merge_peaks(second: &str) -> (f32, f32, f32) {
    let json = merge_json(second);
    let aligned = render(&json, 64, true);
    let unaligned = render(&json, 64, false);
    // A single click's peak: the same project without the delayed branch.
    let alone = render(
        &json.replace(&format!(r#"{{"from": "c:out_l", "to": "{second}"}},"#), ""),
        64,
        true,
    );
    // The same bits at another block size.
    let other = render(&json, 333, true);
    assert!(
        aligned[0]
            .iter()
            .zip(&other[0])
            .all(|(x, y)| x.to_bits() == y.to_bits())
    );
    (peak(&aligned[0]), peak(&unaligned[0]), peak(&alone[0]))
}

#[test]
fn spider_audio_aligns_the_cables_of_one_merge_input_without_a_flag() {
    // Both cables into `m1_l`: a summing input delays the earlier cable by the difference, as a
    // console bus does, with the plain delay compensation.
    let json = merge_json("m:m1_l");
    let default = |compensation: bool| {
        let patch = Patch::from_json(&json).unwrap();
        let options = BuildOptions {
            compensation,
            ..BuildOptions::default()
        };
        let built = build_with(&patch, &default_registry(), &options).unwrap();
        let job = OfflineJob {
            frames: built.frames,
            host_block: 64,
            notes: &built.notes,
            params: &[],
            inputs: &[],
        };
        render_offline_with(built.plan, built.config, &job, |r| assert_no_alloc(r))
    };
    let (on, off) = (default(true), default(false));
    let single = peak(
        &render(
            &json.replace(r#"{"from": "c:out_l", "to": "m:m1_l"},"#, ""),
            64,
            false,
        )[0],
    );
    assert!(single > 0.01);
    assert!(peak(&on[0]) > 1.8 * single, "{} vs {single}", peak(&on[0]));
    assert!(
        peak(&off[0]) < 1.5 * single,
        "{} vs {single}",
        peak(&off[0])
    );
}

#[test]
fn spider_audio_aligns_separate_merge_inputs_with_the_input_alignment_option() {
    let (aligned, unaligned, single) = merge_peaks("m:m2_l");
    assert!(single > 0.01);
    assert!(aligned > 1.8 * single, "{aligned} vs {single}");
    assert!(unaligned < 1.5 * single, "{unaligned} vs {single}");
}
