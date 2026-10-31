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

/// The guitar amp reports its 4x oversampler's 35 frames. Its filters and clips reshape a click,
/// so its start cannot be compared with the dry click's; what the compensation does is still
/// exact: the gap between the amp channel and the dry one is 35 frames wider with compensation
/// off than with it on.
#[test]
fn the_guitar_amp_is_compensated_by_its_oversampler_latency() {
    let amp: &[(&str, f32)] = &[
        ("gain", 0.0),
        ("cab", 3.0),
        ("tight", 20.0),
        ("level", 0.0),
        ("chorus.on", 0.0),
        ("delay.on", 0.0),
        ("reverb.on", 0.0),
    ];
    let mut doc = click_doc(("atomik.guitar-amp", amp));
    let on = render(&doc, 512, 0);
    doc.console.compensation = false;
    let off = render(&doc, 512, 0);
    for beat in 1..8 {
        let at = beat * BEAT;
        let apart =
            |x: &[Vec<f32>]| start_of_click(&x[1], at) as i64 - start_of_click(&x[0], at) as i64;
        let moved = apart(&off) - apart(&on);
        assert!(
            (moved - 35).abs() <= 1,
            "beat {beat}: moved {moved}, want 35"
        );
    }
    let doc = click_doc(("atomik.guitar-amp", amp));
    let reference: Vec<u32> = render(&doc, 512, 0)
        .iter()
        .flatten()
        .map(|x| x.to_bits())
        .collect();
    for (block, workers) in [(64, 0), (333, 3)] {
        let got: Vec<u32> = render(&doc, block, workers)
            .iter()
            .flatten()
            .map(|x| x.to_bits())
            .collect();
        assert!(got == reference, "block {block}, workers {workers}");
    }
}

// ---------------------------------------------------------------------------------------------
// Return alignment: FX returns, sidechain keys and split paths (project flag `align_returns`)
// ---------------------------------------------------------------------------------------------

/// A latent effect: the compressor at ratio 1 delays by its lookahead and changes nothing else.
fn latent(ms: f32) -> (&'static str, Vec<(&'static str, f32)>) {
    (
        "atomik.compressor",
        vec![("threshold", 0.0), ("ratio", 1.0), ("lookahead", ms)],
    )
}

/// The frames of `ms` lookahead at 48 kHz.
fn frames(ms: f32) -> usize {
    (ms * 48.0) as usize
}

/// One synth with a track and notes every `beats` beats; returns its console channel key.
fn add_voice(doc: &mut Document, key_name: &str, beats: i64, sustain: f32) -> String {
    let registry = default_registry();
    let mut apply = |c: Command| {
        doc.apply(c.clone(), &registry)
            .unwrap_or_else(|e| panic!("{c:?}: {e}"));
    };
    apply(Command::AddDevice {
        index: usize::MAX,
        type_id: "atomik.poly-synth".into(),
        key: Some(key(key_name)),
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
        ("env.amp.sustain", sustain),
        ("env.amp.release", 0.5),
        ("voices", 1.0),
    ] {
        apply(Command::SetParam {
            key: key(key_name),
            param: p.into(),
            value: v,
            merge: None,
        });
    }
    apply(Command::Song(SongCommand::AddTrack {
        device: key(key_name),
    }));
    let bars = 2;
    let length = bars * 4 * PPQ;
    let notes = (0..bars * 4 / beats)
        .map(|b| Note::new(b * beats * PPQ, beats * PPQ / 2, 84, 1.0))
        .collect();
    apply(Command::Song(SongCommand::AddClip {
        device: key(key_name),
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
    apply(Command::Song(SongCommand::SetSongEnd(length)));
    doc.doc()
        .console
        .channel_of(&key(key_name))
        .unwrap()
        .channel
        .0
        .clone()
}

fn connect_pair(doc: &mut Document, from: (&str, &str), to: (&str, &str)) {
    let registry = default_registry();
    for side in ["l", "r"] {
        doc.apply(
            Command::Connect(Cable {
                from: sref(from.0, &format!("{}_{side}", from.1)),
                to: sref(to.0, &format!("{}_{side}", to.1)),
            }),
            &registry,
        )
        .unwrap();
    }
}

fn set_param(doc: &mut Document, device: &str, param: &str, value: f32) {
    doc.apply(
        Command::SetParam {
            key: key(device),
            param: param.into(),
            value,
            merge: None,
        },
        &default_registry(),
    )
    .unwrap();
}

fn add_device(doc: &mut Document, name: &str, fx: (&str, &[(&str, f32)])) {
    doc.apply(
        Command::AddDevice {
            index: usize::MAX,
            type_id: fx.0.into(),
            key: Some(key(name)),
        },
        &default_registry(),
    )
    .unwrap();
    for (p, v) in fx.1 {
        set_param(doc, name, p, *v);
    }
}

/// A click through a send to a return holding a `ms` lookahead effect. The click strip is
/// panned hard left (the dry path), the return hard right (the wet path), so the two paths land
/// in different channels of the bounce.
fn send_doc(ms: f32, align: bool) -> RackDoc {
    let mut doc = Document::new(empty_doc());
    let channel = add_voice(&mut doc, "click", 1, 0.0);
    let (type_id, params) = latent(ms);
    add_device(&mut doc, "fx", (type_id, &params));
    set_param(&mut doc, &channel, "pan", -1.0);
    set_param(&mut doc, &channel, "send1.on", 1.0);
    set_param(&mut doc, &channel, "send1.level", 0.0);
    set_param(&mut doc, "master", "return1.pan", 1.0);
    doc.apply(
        Command::Console(atomik_model::ConsoleCommand::SetReturn {
            slot: 0,
            effect: Some(key("fx")),
        }),
        &default_registry(),
    )
    .unwrap();
    let mut out = doc.doc().clone();
    out.console.align_returns = align;
    out
}

#[test]
fn a_latent_send_effect_lands_with_the_dry_onset_when_returns_are_aligned() {
    for ms in [5.0, 2.0] {
        let want = frames(ms);
        let off = render(&send_doc(ms, false), 512, 0);
        let on = render(&send_doc(ms, true), 512, 0);
        for beat in 1..8 {
            let at = beat * BEAT;
            let late = start_of_click(&off[1], at) as i64 - start_of_click(&off[0], at) as i64;
            assert!(
                (late - want as i64).abs() <= 1,
                "{ms} ms, beat {beat}: the return is {late} frames late without alignment"
            );
            let gap = start_of_click(&on[1], at).abs_diff(start_of_click(&on[0], at));
            assert!(gap <= 1, "{ms} ms, beat {beat}: {gap} frames apart aligned");
        }
    }
}

#[test]
fn aligned_the_dry_channel_is_the_unaligned_one_delayed_by_the_effect_latency() {
    let off = render(&send_doc(5.0, false), 512, 0);
    let on = render(&send_doc(5.0, true), 512, 0);
    let shift = frames(5.0);
    assert!(max_difference(&on[0][shift..], &off[0][..off[0].len() - shift]) < 1e-6);
}

/// A sustained tone ducked by a click key. `key_ms`: lookahead of the effect on the key path
/// (`None`: straight in); `split`: the key is the tone's own insert send instead of the click's.
fn duck_doc(key_ms: Option<f32>, split: bool, align: bool) -> RackDoc {
    let mut doc = Document::new(empty_doc());
    let tone = add_voice(&mut doc, "tone", 4, 1.0);
    let click = (!split).then(|| add_voice(&mut doc, "click", 1, 0.0));
    add_device(
        &mut doc,
        "duck",
        (
            "atomik.compressor",
            &[
                ("threshold", -30.0),
                ("ratio", 20.0),
                ("attack", 0.1),
                ("release", 30.0),
                ("sc_enable", 1.0),
            ],
        ),
    );
    connect_pair(&mut doc, (&tone, "insert_send"), ("duck", "in"));
    connect_pair(&mut doc, ("duck", "out"), (&tone, "insert_return"));
    let key_from = click.as_deref().unwrap_or(&tone);
    match key_ms {
        Some(ms) => {
            let (type_id, params) = latent(ms);
            add_device(&mut doc, "delay", (type_id, &params));
            connect_pair(&mut doc, (key_from, "insert_send"), ("delay", "in"));
            connect_pair(&mut doc, ("delay", "out"), ("duck", "sc_in"));
        }
        None => connect_pair(&mut doc, (key_from, "insert_send"), ("duck", "sc_in")),
    }
    let mut out = doc.doc().clone();
    out.console.align_returns = align;
    out
}

/// With the key path 5 ms late and aligned, everything is the straight-in bounce delayed by
/// 5 ms: the main signal waits for its key. Unaligned, the ducking comes late instead.
fn key_alignment(split: bool) {
    let shift = frames(5.0);
    let straight = render(&duck_doc(None, split, true), 512, 0);
    let aligned = render(&duck_doc(Some(5.0), split, true), 512, 0);
    let unaligned = render(&duck_doc(Some(5.0), split, false), 512, 0);
    let n = straight[0].len() - shift;
    let peak = straight[0].iter().fold(0.0f32, |m, v| m.max(v.abs()));
    assert!(peak > 0.01, "the bounce has sound");
    let aligned_gap = max_difference(&aligned[0][shift..shift + n], &straight[0][..n]);
    assert!(
        aligned_gap < 1e-5 * peak,
        "aligned: the straight-in bounce, delayed ({aligned_gap})"
    );
    let unaligned_gap = max_difference(&unaligned[0][shift..shift + n], &straight[0][..n]);
    assert!(
        unaligned_gap > 1e-2 * peak,
        "unaligned: the key lands late, so the bounce differs ({unaligned_gap})"
    );
}

/// Largest absolute difference between two equally long signals.
fn max_difference(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b)
        .fold(0.0f32, |m, (x, y)| m.max((x - y).abs()))
}

#[test]
fn a_latent_sidechain_key_is_aligned_with_the_signal_it_ducks() {
    key_alignment(false);
}

#[test]
fn a_split_path_into_a_device_is_aligned_when_the_flag_is_on() {
    key_alignment(true);
}

#[test]
fn a_new_project_aligns_returns_and_an_empty_one_does_not() {
    assert!(atomik_model::new_project_doc().console.align_returns);
    assert!(!empty_doc().console.align_returns);
}
