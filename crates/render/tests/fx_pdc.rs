//! PDC through the effects in a bounce: identical clicks
//! summed through parallel console channels stay sample-aligned at the master when one of them
//! goes through an effect that reports latency.
//!
//! Each project has two identical click synths, "Click A" hard left (dry) and "Click B" hard
//! right (an effect on its insert). The effects: the compressor with its 5 ms lookahead (240
//! frames at 48 kHz), the distortion with 4x oversampling (35), the mastering suite (89, a
//! constant) and, as zero-latency controls, the EQ, the reverb and the modulation at `mix` 0.
//! With compensation on both clicks start at the same frame; with it off Click B starts later by
//! exactly the effect's latency (so the test is sensitive to a missing or wrong delay).

use atomik_core::PPQ;
use atomik_model::{
    Cable, Clip, ClipContent, ClipId, Command, DeviceKey, Document, Note, RackDoc, SocketRef,
    SongCommand, empty_doc,
};
use atomik_render::default_registry;
use atomik_render::project::{ProjectOptions, render_doc};
use std::num::NonZeroUsize;

const BEAT: usize = 24_000;

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn sref(d: &str, s: &str) -> SocketRef {
    SocketRef {
        device: key(d),
        socket: s.to_string(),
    }
}

/// The two-click project with `effect` (type id, params) on Click B's insert.
fn click_doc(effect: (&str, &[(&str, f32)])) -> RackDoc {
    let registry = default_registry();
    let mut doc = Document::new(empty_doc());
    let mut apply = |c: Command| {
        doc.apply(c.clone(), &registry)
            .unwrap_or_else(|e| panic!("{c:?}: {e}"));
    };
    let set = |k: &str, p: &str, v: f32| Command::SetParam {
        key: key(k),
        param: p.into(),
        value: v,
        merge: None,
    };
    for k in ["click-a", "click-b"] {
        apply(Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.poly-synth".into(),
            key: Some(key(k)),
        });
        for (p, v) in [
            ("osc1.wave", 1.0),
            ("osc.mix", 0.0),
            ("filter.cutoff", 20_000.0),
            ("filter.env_amount", 0.0),
            ("filter.key_track", 0.0),
            ("filter.resonance", 0.0),
            ("env.amp.attack", 0.5),
            ("env.amp.decay", 25.0),
            ("env.amp.sustain", 0.0),
            ("env.amp.release", 0.5),
            ("voices", 1.0),
        ] {
            apply(set(k, p, v));
        }
        apply(Command::Song(SongCommand::AddTrack { device: key(k) }));
    }
    let channel = |d: &Document, source: &str| {
        d.doc()
            .console
            .channel_of(&key(source))
            .unwrap()
            .channel
            .0
            .clone()
    };
    let (ch_a, ch_b) = (channel(&doc, "click-a"), channel(&doc, "click-b"));
    let mut apply = |c: Command| {
        doc.apply(c.clone(), &registry)
            .unwrap_or_else(|e| panic!("{c:?}: {e}"));
    };
    apply(set(&ch_a, "pan", -1.0));
    apply(set(&ch_b, "pan", 1.0));
    let (type_id, params) = effect;
    apply(Command::AddDevice {
        index: usize::MAX,
        type_id: type_id.into(),
        key: Some(key("fx")),
    });
    for (p, v) in params {
        apply(set("fx", p, *v));
    }
    for side in ["l", "r"] {
        for (from, to) in [
            (
                sref(&ch_b, &format!("insert_send_{side}")),
                sref("fx", &format!("in_{side}")),
            ),
            (
                sref("fx", &format!("out_{side}")),
                sref(&ch_b, &format!("insert_return_{side}")),
            ),
        ] {
            apply(Command::Connect(Cable { from, to }));
        }
    }
    let bars = 2;
    let length = bars * 4 * PPQ;
    for k in ["click-a", "click-b"] {
        let notes = (0..bars * 4)
            .map(|b| Note::new(b * PPQ, PPQ / 8, 84, 1.0))
            .collect();
        apply(Command::Song(SongCommand::AddClip {
            device: key(k),
            lane: 0,
            clip: Clip {
                id: ClipId(0),
                start: 0,
                length,
                offset: 0,
                loop_length: None,
                muted: false,
                scale: None,
                controllers: Vec::new(),
                grid: None,
                content: ClipContent::Notes(notes),
            },
        }));
    }
    apply(Command::Song(SongCommand::SetSongEnd(length)));
    doc.doc().clone()
}

fn options(block: usize, workers: u16) -> ProjectOptions {
    ProjectOptions {
        host_block: NonZeroUsize::new(block).unwrap(),
        workers,
        loop_count: None,
    }
}

fn render(doc: &RackDoc, block: usize, workers: u16) -> Vec<Vec<f32>> {
    render_doc(doc, &default_registry(), &options(block, workers))
        .unwrap()
        .channels
}

/// The first frame in `[from, from + BEAT / 2)` whose magnitude reaches half the window peak.
fn start_of_click(x: &[f32], from: usize) -> usize {
    let w = &x[from..from + BEAT / 2];
    let peak = w.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    assert!(peak > 0.01, "a click at {from}: peak {peak}");
    from + w.iter().position(|v| v.abs() >= 0.5 * peak).unwrap()
}

/// `(name, type id, params, latency at 48 kHz)`.
type Case = (
    &'static str,
    &'static str,
    &'static [(&'static str, f32)],
    usize,
);

const CASES: &[Case] = &[
    (
        "compressor, 5 ms lookahead",
        "atomik.compressor",
        &[("threshold", 0.0), ("ratio", 1.0), ("lookahead", 5.0)],
        240,
    ),
    (
        "distortion, 4x oversampling",
        "atomik.distortion",
        &[("drive", 0.0), ("oversample", 2.0), ("mix", 1.0)],
        35,
    ),
    (
        "distortion, 2x oversampling",
        "atomik.distortion",
        &[("drive", 0.0), ("oversample", 1.0), ("mix", 1.0)],
        29,
    ),
    (
        "mastering (multiband off)",
        "atomik.mastering",
        &[("multiband.on", 0.0)],
        89,
    ),
    ("eq", "atomik.eq", &[], 0),
    ("reverb, dry", "atomik.reverb", &[("mix", 0.0)], 0),
    ("modulation, dry", "atomik.modulation", &[("mix", 0.0)], 0),
];

#[test]
fn compensation_aligns_clicks_through_every_effect_and_off_it_offsets_by_the_latency() {
    for (name, type_id, params, latency) in CASES {
        let mut doc = click_doc((type_id, params));
        let on = render(&doc, 512, 0);
        doc.console.compensation = false;
        let off = render(&doc, 512, 0);
        for beat in 1..8 {
            let at = beat * BEAT;
            let (a_on, b_on) = (start_of_click(&on[0], at), start_of_click(&on[1], at));
            let (a_off, b_off) = (start_of_click(&off[0], at), start_of_click(&off[1], at));
            assert!(
                a_on.abs_diff(b_on) <= 1,
                "{name}, beat {beat}: aligned with compensation ({a_on} vs {b_on})"
            );
            assert!(
                (b_off as i64 - a_off as i64 - *latency as i64).abs() <= 1,
                "{name}, beat {beat}: offset {} without compensation, want {latency}",
                b_off as i64 - a_off as i64
            );
        }
        eprintln!("{name}: aligned, {latency} frames of PDC");
    }
}

/// With the multiband on, the crossovers add phase shift, not delay: the click still starts
/// within 32 frames (0.7 ms; 18 measured) of the dry one: the all-pass phase of the crossovers
/// moves the edge of a square click, it is not a PDC error (the reported latency stays 89).
#[test]
fn mastering_with_its_multiband_stays_aligned_within_the_crossover_phase() {
    let doc = click_doc(("atomik.mastering", &[]));
    let on = render(&doc, 512, 0);
    for beat in 1..8 {
        let at = beat * BEAT;
        let d = start_of_click(&on[0], at).abs_diff(start_of_click(&on[1], at));
        assert!(d <= 32, "beat {beat}: {d} frames apart");
    }
}

#[test]
fn the_pdc_bounces_are_bit_identical_across_host_blocks_and_workers() {
    for (name, type_id, params, _) in CASES {
        let doc = click_doc((type_id, params));
        let reference: Vec<u32> = render(&doc, 512, 0)
            .iter()
            .flatten()
            .map(|x| x.to_bits())
            .collect();
        for block in [64, 333, 4096] {
            for workers in [0, 3] {
                let got: Vec<u32> = render(&doc, block, workers)
                    .iter()
                    .flatten()
                    .map(|x| x.to_bits())
                    .collect();
                assert!(got == reference, "{name}: block {block}, workers {workers}");
            }
        }
    }
}
